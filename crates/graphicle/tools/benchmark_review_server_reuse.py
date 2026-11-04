"""Compare fresh CLI, fresh HTTP and warmed HTTP processes on a new head.

Use --binary with Go phase instrumentation and a JSON --cases array containing
name, seed_store, previous_request and next_request. Stores must already admit
both requests' snapshots. Only private clones are changed. Each round warms a
server on the previous head, then snapshots its cache state for the CLI and fresh
server controls. All three measured requests analyze the same unseen next head.
Startup, priming and cloning are outside HTTP timing; CLI timing includes launch.
No models, collection or deployment. Original sources/stores stay read-only.
Queued background work is cancelled only in scratch clones to isolate reviews;
fixtures with connectors are refused because they can schedule new work.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import time

from benchmark_pinned_review import LocalServer, canonical, phases, review, save, sha
from benchmark_review_cache_limit import native_phases
from benchmark_review_cache_reuse import cache_counts, clone_store, connect_readonly, environment


def cache_digest(store):
    """Include artifacts, access bindings and LRU order, not only row counts."""
    digest = hashlib.sha256()
    with connect_readonly(store) as db:
        for table in ['repo_review_analysis', 'repo_review_analysis_inputs',
                      'repo_review_package_facts', 'repo_review_package_inputs']:
            digest.update(table.encode())
            for row in db.execute('SELECT rowid,* FROM ' + table + ' ORDER BY rowid'):
                digest.update(json.dumps(row, separators=(',', ':')).encode())
    return digest.hexdigest()


def log_stats(log):
    phase_ms, peak = phases(log)
    rss = [float(x) for x in re.findall(r'^phase=\S+ ms=\d+ rss_mb=([\d.]+)', log, re.MULTILINE)]
    return {'phase_ms': phase_ms, 'phase_peak_rss_mib': peak,
            'sampled_current_rss_max_mib': max(rss, default=0),
            'first_phase_current_rss_mib': rss[0] if rss else 0,
            'native_phase_ns': native_phases(log)}


def assert_idle_worker(store):
    with connect_readonly(store) as db:
        assert db.execute("SELECT count(*) FROM am_jobs WHERE state IN ('queued','retry','running')").fetchone()[0] == 0
        assert db.execute('SELECT count(*) FROM am_connectors').fetchone()[0] == 0


def http_review(server, client, request, work, label):
    before = server.logs().splitlines()
    started = time.monotonic()
    packet = client.command('graph.impact', **request)
    elapsed = time.monotonic() - started
    # Stderr is drained on an SDK thread. Wait outside timing for the last phase.
    until = time.monotonic() + 2
    while True:
        lines = server.logs().splitlines()
        assert lines[:len(before)] == before, 'SDK log buffer rolled over'
        log = '\n'.join(lines[len(before):]) + '\n'
        if 'phase=review_pack ' in log:
            break
        if time.monotonic() >= until:
            raise RuntimeError('Missing review phase logs')
        time.sleep(0.01)
    save(work / (label + '.json'), packet)
    (work / (label + '.log')).write_text(log)
    assert 'phase=graph_' not in log, 'Background graph work contaminated the measurement'
    assert_idle_worker(work / 'store')
    stats = {'seconds': elapsed, **log_stats(log),
             'analysis_cache': packet['analysis_cache'], 'entries': len(packet['entries']),
             'impact': packet['impact'], 'issue_count': packet['coverage']['issue_count']}
    return packet, stats


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
    result = {'binary_sha256': sha(binary), 'cases': {}, 'rounds': [], 'rows': [], 'complete': False}
    references = {}
    for round_index, arms in enumerate([['cli', 'fresh-http', 'warm-http'],
                                       ['warm-http', 'fresh-http', 'cli']]):
        for case in cases:
            name = case['name']
            folder = root / name / str(round_index)
            warm_work = folder / 'warm-http'
            clone_store(case['seed_store'], warm_work / 'store')
            # Force the previous head through the helper without changing sources.
            with sqlite3.connect(warm_work / 'store/reviews.sqlite3') as db:
                db.execute('PRAGMA foreign_keys=ON')
                db.execute('DELETE FROM repo_review_analysis')
                assert db.execute('SELECT count(*) FROM am_connectors').fetchone()[0] == 0
                pending = db.execute("SELECT kind,state,count(*) FROM am_jobs WHERE state IN ('queued','retry','running') GROUP BY kind,state").fetchall()
                db.execute("UPDATE am_jobs SET state='cancelled',lease_token=NULL,lease_until=NULL WHERE state IN ('queued','retry','running')")
            assert_idle_worker(warm_work / 'store')
            previous = json.loads(Path(case['previous_request']).read_text())
            request = json.loads(Path(case['next_request']).read_text())
            assert len(request['head']) == len(previous['head']) == 1
            assert previous['base'] == request['base'] and previous['profile'] == request['profile']
            assert previous['head'][0]['snapshot'] != request['head'][0]['snapshot']
            assert request['head'][0]['snapshot'] not in [p['snapshot'] for p in request['base']]
            save(folder / 'request.json', request)
            result['cases'][name] = {'base': request['base'], 'previous_head': previous['head'],
                                    'head': request['head'], 'profile': request['profile'],
                                    'max_tokens': request['max_tokens'], 'request_sha256': sha(folder / 'request.json')}
            warm = LocalServer(binary, warm_work / 'store', config, env=environment(warm_work),
                               timeout=600, startup_timeout=120)
            with warm as warm_client:
                _, priming = http_review(warm, warm_client, previous, warm_work, 'prime')
                assert not priming['analysis_cache']['base_hit'] and not priming['analysis_cache']['head_hit']
                assert priming['native_phase_ns']['helper'] > 0
                counts = cache_counts(warm_work / 'store')
                assert counts['full_analyses'] == 2 and counts['package_entries'] > 0
                digest = cache_digest(warm_work / 'store')
                for arm in ['cli', 'fresh-http']:
                    clone_store(warm_work / 'store', folder / arm / 'store')
                    assert cache_digest(folder / arm / 'store') == digest
                result['rounds'].append({'case': name, 'round': round_index, 'order': arms,
                                         'cache_counts': counts, 'cache_digest': digest,
                                         'background_jobs_cancelled_in_clone': pending,
                                         'priming_untimed': priming})
                print(name, round_index, 'primed', flush=True)
                for arm in arms:
                    work = folder / arm
                    assert cache_digest(work / 'store') == digest
                    if arm == 'cli':
                        packet, stats = review(binary, work, folder / 'request.json', 'review',
                                               config, environment(work))
                        stats.update(log_stats((work / 'review.log').read_text()))
                    elif arm == 'warm-http':
                        packet, stats = http_review(warm, warm_client, request, work, 'review')
                    else:
                        fresh = LocalServer(binary, work / 'store', config, env=environment(work),
                                            timeout=600, startup_timeout=120)
                        began = time.monotonic()
                        with fresh as fresh_client:
                            startup = time.monotonic() - began
                            assert cache_digest(work / 'store') == digest
                            packet, stats = http_review(fresh, fresh_client, request, work, 'review')
                            assert fresh_client.usage()['model_usage']['calls'] == 0
                        stats['startup_seconds_untimed'] = startup
                    cache = packet['analysis_cache']
                    assert cache['base_hit'] and not cache['head_hit']
                    assert packet['head'] == request['head'] and packet['tokens'] <= request['max_tokens']
                    compared = canonical(packet)
                    references.setdefault(name, compared)
                    row = {'case': name, 'round': round_index, 'arm': arm, **stats,
                           'tokens': packet['tokens'], 'packet_equal': compared == references[name]}
                    result['rows'].append(row)
                    save(root / 'summary.json', result)
                    print(name, arm, json.dumps(row), flush=True)
                    assert row['packet_equal'], 'Semantic packet drift'
                assert warm_client.usage()['model_usage']['calls'] == 0
    result['complete'] = True
    save(root / 'summary.json', result)


if __name__ == '__main__':
    main()
