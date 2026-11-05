"""Measure dependency-cache reuse on a new pinned head, without models.

Each JSON case has name, seed_store, archive_store, previous_request and
next_snapshot. seed_store must have reviewed previous_request with the measured
binary. archive_store supplies the next immutable snapshot. Originals are opened
read-only; only new private scratch copies are admitted, cleared and reviewed.

Compare empty compiler caches, package exports only, and normal reuse (including
unchanged base facts), in forward/reverse order. Admission and store preparation
are untimed. No source/dependency collection, models or OS cache flushing occurs.
"""
import argparse
import base64
from contextlib import closing
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import time

from benchmark_pinned_review import LocalServer, canonical, review, save, sha


def connect_readonly(store):
    return sqlite3.connect((Path(store).resolve() / 'reviews.sqlite3').as_uri() + '?mode=ro', uri=True)


def clone_store(source, destination):
    destination.mkdir(parents=True, exist_ok=False)
    with closing(connect_readonly(source)) as original, closing(sqlite3.connect(destination / 'reviews.sqlite3')) as target, target:
        original.backup(target)
    shutil.copyfile(Path(source) / 'local-session.json', destination / 'local-session.json')


def cache_counts(store):
    with closing(connect_readonly(store)) as db:
        full = db.execute('SELECT count(*) FROM repo_review_analysis').fetchone()[0]
        count, size = db.execute('SELECT count(*), coalesce(sum(length(data)),0) FROM repo_review_package_facts').fetchone()
    return {'full_analyses': full, 'package_entries': count, 'package_payload_bytes': size}


def environment(work):
    return {'PATH': os.environ['PATH'], 'HOME': str(work), 'TMPDIR': '/private/tmp',
            'GOENV': 'off', 'GOWORK': 'off', 'GOTOOLCHAIN': 'local', 'GOPROXY': 'off',
            'GOSUMDB': 'off', 'GOVCS': '*:off', 'CGO_ENABLED': '0',
            'GRAPHICLE_PHASE_TIMINGS': '1', 'RUST_MIN_STACK': '16777216'}


def prepare(binary, case, root, config):
    folder = root / case['name']
    folder.mkdir()
    request = json.loads(Path(case['previous_request']).read_text())
    assert len(request['base']) == len(request['head']) == 1
    repository = request['repository_id']
    assert all(pin['repository_id'] == repository for pin in request['base'] + request['head'])
    previous_pin = request['head'][0]
    with closing(connect_readonly(case['archive_store'])) as archive:
        old = json.loads(archive.execute('SELECT manifest FROM repo_snapshots WHERE repository_id=? AND sha256=?',
                                        (repository, previous_pin['snapshot'])).fetchone()[0])
        row = archive.execute('SELECT manifest,metadata FROM repo_snapshots WHERE repository_id=? AND sha256=?',
                              (repository, case['next_snapshot'])).fetchone()
        manifest, metadata = map(json.loads, row)
        changed = [p for p in sorted(old.keys() | manifest.keys()) if old.get(p) != manifest.get(p)]
        assert changed and case['next_snapshot'] not in [p['snapshot'] for p in request['base'] + request['head']]
        dependency_paths = {p for p in old.keys() | manifest.keys()
                            if 'vendor' in Path(p).parts or Path(p).name in ('go.mod', 'go.sum', 'go.work', 'go.work.sum')
                            or p == request['profile'].get('workspace')}
        assert not dependency_paths.intersection(changed), 'Dependency or build configuration changed'
        assert any(p.endswith('.go') and not p.endswith('_test.go') for p in changed), 'Need an owned production Go edit'
        next_pin = {**previous_pin, 'snapshot': case['next_snapshot'], 'commit': metadata['commit'],
                    'policy_sha256': metadata['policy_sha256']}
        assert next_pin['policy_sha256'] == previous_pin['policy_sha256'], 'Admission policy changed'
        columns = {r[1] for r in archive.execute('PRAGMA table_info(repo_snapshot_bindings)')}
        withheld = 'withheld_paths' if 'withheld_paths' in columns else 'NULL'
        binding = archive.execute(f'SELECT scope_complete,{withheld} FROM repo_snapshot_bindings WHERE repository_id=? AND snapshot=? AND commit_id=? AND policy_sha256=?',
                                  (repository, next_pin['snapshot'], next_pin['commit'], next_pin['policy_sha256'])).fetchone() if columns else None
        if binding:
            metadata['scope_complete'] = bool(binding[0])
            if binding[1] is not None:
                metadata['withheld_paths'] = json.loads(binding[1])
        request['head'] = [next_pin]
        request.pop('previous_packet_id', None)
        request_path = folder / 'request.json'
        save(request_path, request)
        clone_store(case['seed_store'], folder / 'seed')
        before = cache_counts(folder / 'seed')
        assert before['full_analyses'] >= 2 and before['package_entries'] > 0
        # Verify every archived object before admission; never trust a matching key alone.
        import hashlib
        for digest, size in {v['sha256']: v['bytes'] for v in manifest.values()}.items():
            value = archive.execute('SELECT content FROM repo_objects WHERE repository_id=? AND sha256=?',
                                    (repository, digest)).fetchone()[0]
            raw = value.encode() if isinstance(value, str) else value
            assert len(raw) == size and hashlib.sha256(raw).hexdigest() == digest
        server = LocalServer(binary, folder / 'seed', config, env=environment(folder), timeout=600)
        started = time.monotonic()
        with server as client:
            begin = client.command('sync.prepare', repository_id=repository, expected_snapshot=previous_pin['snapshot'],
                                   manifest=[{'path': p, **v} for p, v in manifest.items()], metadata=metadata,
                                   missing_limit=256)
            assert begin['snapshot'] == next_pin['snapshot']
            sync_id = begin['sync_id']
            page = begin
            missing = list(page['missing'])
            while page.get('next_cursor'):
                page = client.command('sync.missing', repository_id=repository, sync_id=sync_id,
                                      cursor=page['next_cursor'], missing_limit=256)
                missing.extend(page['missing'])
            for path in missing:
                value = archive.execute('SELECT content FROM repo_objects WHERE repository_id=? AND sha256=?',
                                        (repository, manifest[path]['sha256'])).fetchone()[0]
                raw = value.encode() if isinstance(value, str) else value
                client.command('sync.upload', repository_id=repository, sync_id=sync_id,
                               files=[{'path': path, 'content_base64': base64.b64encode(raw).decode()}])
            client.command('sync.commit', repository_id=repository, sync_id=sync_id, compact=True)
            assert client.usage()['model_usage']['calls'] == 0
        after = cache_counts(folder / 'seed')
        assert before == after, 'Admission changed previously primed compiler caches'
        info = {'previous_head': previous_pin, 'next_head': next_pin, 'profile': request['profile'],
                'changed_paths': changed, 'unchanged_dependency_paths': len(dependency_paths),
                'cache_before_next_admission': before, 'cache_after_next_admission': after,
                'admission_seconds_untimed': time.monotonic() - started, 'uploaded_objects': len(missing),
                'request_sha256': sha(request_path), 'max_tokens': request['max_tokens']}
        print(case['name'], 'prepared', json.dumps(info), flush=True)
        return info


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
    result = {'binary_sha256': sha(binary), 'cases': {}, 'rows': [],
              'method': 'cold, packages-only, normal-reuse, normal-reuse, packages-only, cold; separate clones from the same prior-review state; new head in every measured request'}
    for case in cases:
        result['cases'][case['name']] = prepare(binary, case, root, config)
    save(root / 'summary.json', result)
    order = ['cold', 'packages-only', 'normal-reuse', 'normal-reuse', 'packages-only', 'cold']
    # Finish copies and cache invalidation before any timed measurement.
    for index, arm in enumerate(order):
        for case in cases:
            folder = root / case['name']
            work = folder / f'{index}-{arm}'
            clone_store(folder / 'seed', work / 'store')
            with closing(sqlite3.connect(work / 'store/reviews.sqlite3')) as db, db:
                db.execute('PRAGMA foreign_keys=ON')
                if arm != 'normal-reuse':
                    db.execute('DELETE FROM repo_review_analysis')
                if arm == 'cold':
                    db.execute('DELETE FROM repo_review_package_facts')
            counts = cache_counts(work / 'store')
            assert counts['full_analyses'] == (2 if arm == 'normal-reuse' else 0)
            assert (counts['package_entries'] > 0) == (arm != 'cold')
    reference = {}
    for index, arm in enumerate(order):
        for case in cases:
            name = case['name']
            work = root / name / f'{index}-{arm}'
            counts = cache_counts(work / 'store')
            packet, stats = review(binary, work, root / name / 'request.json', 'review', config, environment(work))
            cache = packet['analysis_cache']
            assert cache['base_hit'] == (arm == 'normal-reuse') and not cache['head_hit'], (name, arm, 'unexpected full-analysis reuse')
            assert packet['tokens'] <= result['cases'][name]['max_tokens']
            assert packet['head'][0]['snapshot'] == result['cases'][name]['next_head']['snapshot']
            compared = canonical(packet)
            if name not in reference:
                reference[name] = compared
            row = {'case': name, 'arm': arm, 'order': index + 1, 'cache_before': counts,
                   **stats, 'tokens': packet['tokens'], 'packet_equal': reference[name] == compared}
            result['rows'].append(row)
            save(root / 'summary.json', result)
            print(name, arm, json.dumps(row), flush=True)
            assert row['packet_equal'], (name, arm, 'semantic packet drift')
    result['complete'] = True
    save(root / 'summary.json', result)


if __name__ == '__main__':
    main()
