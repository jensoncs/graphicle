"""Bounded sequential/concurrent pinned-review soak using serve --no-worker.

Cases: name, seed_store, next_request and edit_path. Each private store derives
11 distinct synthetic heads from the archived head: an unused owned Go constant
changes value. Head zero warms the helper and both sides' facts, then a replay
verifies the warm state; four heads run sequentially, four in
two concurrent pairs, and two sequentially again to check recovery. Each is
compared with a fresh CLI process on a
clone of the same post-warm cache state. Admission, cloning, warmup and model
calls are outside review timing. No original store or source is changed.
"""
import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import re
import time

from benchmark_pinned_review import LocalServer, canonical, review, save, sha
from benchmark_review_cache_reuse import clone_store, connect_readonly, environment
from benchmark_review_server_reuse import cache_digest, log_stats


class LoggedServer(LocalServer):
    """Keep the full probe log in a private file beyond the SDK's recent tail."""
    def _drain(self, stream, announced):
        def lines():
            with (self.data_dir.parent / 'server.log').open('w') as log:
                for line in stream:
                    log.write(line)
                    log.flush()
                    yield line
        super()._drain(lines(), announced)


def job_state(store):
    with closing(connect_readonly(store)) as db:
        assert db.execute('SELECT count(*) FROM am_connectors').fetchone()[0] == 0
        return db.execute('SELECT id,kind,state,attempt,lease_token,lease_until FROM am_jobs ORDER BY id').fetchall()


def prepare_manifest(client, repository, expected, manifest, metadata):
    """Use the existing multipart protocol; return the final missing-list page."""
    parts = [manifest[i:i + 5000] for i in range(0, len(manifest), 5000)] or [[]]
    prepared = None
    for index, part in enumerate(parts):
        args = {'repository_id': repository, 'manifest': part,
                'manifest_complete': index == len(parts) - 1, 'missing_limit': 256}
        if index == 0:
            args.update(expected_snapshot=expected, metadata=metadata)
        else:
            args['sync_id'] = prepared['sync_id']
        prepared = client.command('sync.prepare', **args)
    return prepared


def prepare_heads(client, work, case, head_count=11):
    request = json.loads(Path(case['next_request']).read_text())
    assert len(request['head']) == len(request['base']) == 1
    pin = request['head'][0]
    repo = request['repository_id']
    path = case['edit_path']
    assert path.endswith('.go') and not path.endswith('_test.go') and 'vendor' not in Path(path).parts
    with closing(connect_readonly(work / 'store')) as db:
        row = db.execute('SELECT manifest,metadata FROM repo_snapshots WHERE repository_id=? AND sha256=?',
                         (repo, pin['snapshot'])).fetchone()
        manifest, metadata = map(json.loads, row)
        original = db.execute('SELECT content FROM repo_objects WHERE repository_id=? AND sha256=?',
                              (repo, manifest[path]['sha256'])).fetchone()[0]
        original = original.encode() if isinstance(original, str) else original
        assert hashlib.sha256(original).hexdigest() == manifest[path]['sha256']
        expected = db.execute('SELECT snapshot FROM repo_projects WHERE id=?', (repo,)).fetchone()[0]
    marker = b'graphicleSoakRevision'
    assert marker not in original
    # Synthetic pins do not claim a new collection/verification of dependencies.
    metadata.pop('dependency_preparation', None)
    requests = []
    for index in range(head_count):
        content = original + b'\nconst ' + marker + b' = ' + str(index).encode() + b'\n'
        updated = {**manifest, path: {'sha256': hashlib.sha256(content).hexdigest(), 'bytes': len(content)}}
        commit = hashlib.sha1(f'graphicle synthetic soak {case["name"]} {index}'.encode()).hexdigest()
        page = prepare_manifest(client, repo, expected,
                                [{'path': p, **v} for p, v in updated.items()],
                                {**metadata, 'commit': commit})
        # Only this synthetic edit is new; all other objects must be retained.
        assert page['missing'] == [path] and not page.get('next_cursor')
        client.command('sync.upload', repository_id=repo, sync_id=page['sync_id'],
                       files=[{'path': path, 'content_base64': base64.b64encode(content).decode()}])
        client.command('sync.commit', repository_id=repo, sync_id=page['sync_id'], compact=True)
        expected = page['snapshot']
        next_request = {**request, 'head': [{**pin, 'commit': commit, 'snapshot': expected}]}
        requests.append(next_request)
        save(work.parent / f'request-{index}.json', next_request)
    assert len({r['head'][0]['snapshot'] for r in requests}) == head_count
    return requests


def call(client, request):
    started = time.monotonic()
    packet = client.command('graph.impact', **request)
    return packet, time.monotonic() - started


def batch_log(work, offset, count):
    until = time.monotonic() + 2
    while True:
        with (work / 'server.log').open() as stream:
            stream.seek(offset)
            log = stream.read()
        if len(re.findall(r'^phase=review_pack ', log, re.MULTILINE)) == count:
            assert 'phase=graph_' not in log, 'Unexpected background graph work'
            return log
        if time.monotonic() >= until:
            raise RuntimeError('Missing or unexpected review phases')
        time.sleep(0.01)


def check(packet, request, *, base_hit=True, head_hit=False):
    assert packet['base'] == request['base'] and packet['head'] == request['head']
    assert packet['tokens'] <= request['max_tokens']
    for side, expected in [('base', base_hit), ('head', head_hit)]:
        if expected is not None:
            assert packet['analysis_cache'][side + '_hit'] == expected, (
                f'Expected {side} cache hit={expected}; got {packet["analysis_cache"]}')


def prime_cache(client, work, request):
    """Untimed warmup works with cold captures; verify both sides before cloning."""
    offset = (work / 'server.log').stat().st_size
    packet, elapsed = call(client, request)
    check(packet, request, base_hit=None, head_hit=None)
    log = batch_log(work, offset, 1)
    save(work / 'prime.json', packet)
    (work / 'prime.log').write_text(log)
    stats = {'seconds': elapsed, 'analysis_cache': packet['analysis_cache'], **log_stats(log)}

    offset = (work / 'server.log').stat().st_size
    verified, elapsed = call(client, request)
    check(verified, request, base_hit=True, head_hit=True)
    assert canonical(verified) == canonical(packet), 'Warmup replay changed the packet'
    log = batch_log(work, offset, 1)
    save(work / 'prime-verify.json', verified)
    (work / 'prime-verify.log').write_text(log)
    stats['verification'] = {'seconds': elapsed, 'analysis_cache': verified['analysis_cache'],
                             'packet_equal': True, **log_stats(log)}
    return stats


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--cases', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve()
    cases = json.loads(args.cases.read_text())
    assert len({c['name'] for c in cases}) == len(cases)
    assert all(re.fullmatch(r'[A-Za-z0-9_-]+', c['name']) for c in cases)
    config = root / 'models.json'
    save(config, {'models': {}})
    result = {'binary_sha256': sha(binary), 'cases': {}, 'rows': [], 'batches': [], 'complete': False}
    for case_index, case in enumerate(cases):
        name = case['name']
        folder = root / name
        work = folder / 'server'
        clone_store(case['seed_store'], work / 'store')
        prior_jobs = job_state(work / 'store')
        server = LoggedServer(binary, work / 'store', config, env=environment(work),
                              serve_args=['--no-worker'], timeout=600, startup_timeout=120)
        with server as client:
            assert job_state(work / 'store') == prior_jobs
            requests = prepare_heads(client, work, case)
            jobs = job_state(work / 'store')
            assert len(jobs) > len(prior_jobs)
            priming = prime_cache(client, work, requests[0])
            assert job_state(work / 'store') == jobs
            digest = cache_digest(work / 'store')
            for index in range(1, 11):
                clone_store(work / 'store', folder / f'cli-{index}' / 'store')
                assert cache_digest(folder / f'cli-{index}' / 'store') == digest
            result['cases'][name] = {'base': requests[0]['base'], 'profile': requests[0]['profile'],
                'heads': [r['head'] for r in requests], 'edit_path': case['edit_path'],
                'max_tokens': requests[0]['max_tokens'], 'synthetic_heads': True,
                'initial_cache_digest': digest, 'queued_jobs_preserved': len(jobs),
                'prime_untimed': priming,
                'arm_order': ['cli', 'server'] if case_index % 2 == 0 else ['server', 'cli']}
            print(name, 'prepared 11 synthetic heads; queued jobs preserved', len(jobs), flush=True)
            references = {}
            def record(index, arm, packet, stats):
                check(packet, requests[index])
                value = canonical(packet)
                references.setdefault(index, value)
                equal = value == references[index]
                result['rows'].append({'case': name, 'head_index': index, 'arm': arm,
                    **stats, 'analysis_cache': packet['analysis_cache'], 'tokens': packet['tokens'],
                    'entries': len(packet['entries']), 'impact': packet['impact'],
                    'issue_count': packet['coverage']['issue_count'], 'packet_equal': equal})
                save(root / 'summary.json', result)
                print(name, index, arm, round(stats['seconds'], 3), 'equal', equal, flush=True)
                assert equal, 'Semantic packet drift'
            for arm in result['cases'][name]['arm_order']:
                if arm == 'cli':
                    for index in range(1, 11):
                        cli_work = folder / f'cli-{index}'
                        assert cache_digest(cli_work / 'store') == digest
                        packet, stats = review(binary, cli_work, folder / f'request-{index}.json',
                                               'review', config, environment(cli_work))
                        stats.update(log_stats((cli_work / 'review.log').read_text()))
                        record(index, 'cli', packet, stats)
                else:
                    for indices in [[1], [2], [3], [4], [5, 6], [7, 8], [9], [10]]:
                        assert job_state(work / 'store') == jobs
                        offset = (work / 'server.log').stat().st_size
                        started = time.monotonic()
                        if len(indices) == 1:
                            answers = [call(client, requests[indices[0]])]
                        else:
                            with ThreadPoolExecutor(max_workers=2) as pool:
                                futures = [pool.submit(call, client, requests[i]) for i in indices]
                                answers = [f.result() for f in futures]
                        wall = time.monotonic() - started
                        log = batch_log(work, offset, len(indices))
                        (work / ('batch-' + '-'.join(map(str, indices)) + '.log')).write_text(log)
                        stats = log_stats(log)
                        stage = 'concurrent' if len(indices) == 2 else ('recovery' if indices[0] >= 9 else 'sequential')
                        result['batches'].append({'case': name, 'heads': indices, 'stage': stage,
                            'concurrency': len(indices), 'seconds': wall, **stats})
                        for index, (packet, elapsed) in zip(indices, answers):
                            save(work / f'review-{index}.json', packet)
                            record(index, 'server', packet, {'seconds': elapsed,
                                'concurrency': len(indices), 'stage': stage, **(stats if len(indices) == 1 else {})})
                        assert job_state(work / 'store') == jobs
            assert client.usage()['model_usage']['calls'] == 0
            assert job_state(work / 'store') == jobs
    result['complete'] = True
    save(root / 'summary.json', result)


if __name__ == '__main__':
    main()
