"""Investigate retention during an uninterrupted burst of cached reviews on macOS.

Use an already reviewed store, its exact pinned request and a reference packet.
Only a private store backup is modified. Reuses the memory sampler and checks
from the longer new-head probe; no admission, new compiler inputs or models.
"""
import argparse
import json
import os
from pathlib import Path
import time

from benchmark_pinned_review import canonical, save, sha
from benchmark_review_cache_reuse import cache_counts, clone_store, environment
from benchmark_review_memory import Monitor
from benchmark_review_soak import LoggedServer, batch_log, call, job_state


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('binary', 'store', 'request', 'reference', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--count', type=int, default=50)
    parser.add_argument('--idle-seconds', type=float, default=60)
    args = parser.parse_args()
    assert args.count >= 1 and args.idle_seconds >= 1
    os.umask(0o077)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    work = root / 'server'
    clone_store(args.store, work / 'store')
    jobs = job_state(work / 'store')
    request = json.loads(args.request.read_text())
    reference = canonical(json.loads(args.reference.read_text()))
    config = root / 'models.json'
    save(config, {'models': {}})
    result = {'binary_sha256': sha(args.binary), 'base': request['base'], 'head': request['head'],
              'profile': request['profile'], 'max_tokens': request['max_tokens'],
              'rows': [], 'complete': False, 'cache_before': cache_counts(work / 'store')}
    server = LoggedServer(args.binary, work / 'store', config, env=environment(work),
                          serve_args=['--no-worker'], timeout=600, startup_timeout=120)
    with server as client, Monitor(server.process.pid, root / 'memory.jsonl') as monitor:
        for index in range(1, args.count + 1):
            label = f'cached-burst-{index}'
            monitor.mark(label)
            offset = (work / 'server.log').stat().st_size
            packet, seconds = call(client, request)
            batch_log(work, offset, 1)
            assert packet['base'] == request['base'] and packet['head'] == request['head']
            assert packet['tokens'] <= request['max_tokens']
            assert packet['analysis_cache']['base_hit'] and packet['analysis_cache']['head_hit']
            assert canonical(packet) == reference
            assert job_state(work / 'store') == jobs
            row = {'index': index, 'seconds': seconds, 'tokens': packet['tokens'],
                   'packet_equal': True, 'analysis_cache': packet['analysis_cache'],
                   'memory': monitor.summary(label)}
            result['rows'].append(row)
            save(root / 'summary.json', result)
            print(label, round(seconds, 3), 's; RSS MiB',
                  round(row['memory']['server_rss_bytes']['max'] / 2**20, 1), flush=True)
        monitor.mark('after-burst')
        until = time.monotonic() + args.idle_seconds
        while time.monotonic() < until:
            time.sleep(min(0.5, max(0, until - time.monotonic())))
            monitor.check()
        result['idle'] = {'seconds': args.idle_seconds, 'memory': monitor.summary('after-burst')}
        result['cache_after'] = cache_counts(work / 'store')
        assert result['cache_before'] == result['cache_after']
        assert client.usage()['model_usage']['calls'] == 0
        assert job_state(work / 'store') == jobs
    assert job_state(args.store) == jobs
    result['jobs_unchanged'] = True
    result['complete'] = True
    save(root / 'summary.json', result)


if __name__ == '__main__':
    main()
