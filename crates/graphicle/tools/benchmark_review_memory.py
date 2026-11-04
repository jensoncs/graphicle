"""Longer sequential review memory investigation on macOS, without models.

Uses the same cases as benchmark_review_soak.py. Only private SQLite backups are
modified. Thirty unseen heads, idle pauses, ten cached replays, and a restart
separate repeated compiler work from process retention. Native libproc samples
the server and its currently discoverable descendants every 250 ms. RSS sums
can double-count shared pages and sampling can miss short-lived processes.
Selected heads also get independent fresh-CLI correctness controls after the
server has stopped. This is not a production memory limit or leak detector.
"""
import argparse
import ctypes
import errno
import json
import os
from pathlib import Path
import re
import statistics
import sys
import threading
import time

from benchmark_pinned_review import canonical, review, save, sha
from benchmark_review_cache_reuse import cache_counts, clone_store, environment
from benchmark_review_server_reuse import log_stats
from benchmark_review_soak import LoggedServer, batch_log, call, check, job_state, prepare_heads


class MacProcesses:
    """Read only a known server PID and its descendants; no command lines."""

    def __init__(self):
        if sys.platform != 'darwin':
            raise RuntimeError('This sampler requires macOS libproc')
        self.lib = ctypes.CDLL('/usr/lib/libproc.dylib', use_errno=True)
        self.lib.proc_listchildpids.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
        self.lib.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64,
                                         ctypes.c_void_p, ctypes.c_int]
        self.lib.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]

    def tree(self, root):
        pending, seen, rows, vanished = [root], set(), [], 0
        while pending:
            pid = pending.pop()
            if pid in seen:
                continue
            seen.add(pid)
            # proc_taskinfo: six uint64_t, then twelve int32_t (96 bytes).
            task = ctypes.create_string_buffer(96)
            count = self.lib.proc_pidinfo(pid, 4, 0, task, len(task))
            if count != len(task):
                error = ctypes.get_errno()
                if pid != root and error in (errno.ESRCH, errno.ENOENT):
                    vanished += 1
                    continue
                raise OSError(error, f'proc_pidinfo failed for {pid}: {count}')
            # rusage_info_v0: uuid followed by ten uint64_t (96 bytes).
            usage = ctypes.create_string_buffer(96)
            status = self.lib.proc_pid_rusage(pid, 0, usage)
            if status:
                error = ctypes.get_errno()
                if pid != root and error in (errno.ESRCH, errno.ENOENT):
                    vanished += 1
                    continue
                raise OSError(error, f'proc_pid_rusage failed for {pid}')
            rows.append({'pid': pid,
                         'rss_bytes': int.from_bytes(task.raw[8:16], sys.byteorder),
                         'footprint_bytes': int.from_bytes(usage.raw[72:80], sys.byteorder),
                         'threads': int.from_bytes(task.raw[84:88], sys.byteorder)})
            children = (ctypes.c_int * 4096)()
            count = self.lib.proc_listchildpids(pid, children, ctypes.sizeof(children))
            if count < 0:
                error = ctypes.get_errno()
                if pid != root and error in (errno.ESRCH, errno.ENOENT):
                    vanished += 1
                    continue
                raise OSError(error, f'proc_listchildpids failed for {pid}')
            # Unlike proc_listpids, this convenience API returns PID count.
            if count >= len(children):
                raise RuntimeError('Child PID buffer filled; sample would be incomplete')
            pending.extend(children[:count])
        server = next(row for row in rows if row['pid'] == root)
        return {'server_rss_bytes': server['rss_bytes'],
                'server_footprint_bytes': server['footprint_bytes'],
                'tree_rss_bytes': sum(row['rss_bytes'] for row in rows),
                'tree_footprint_bytes': sum(row['footprint_bytes'] for row in rows),
                'server_threads': server['threads'], 'children': len(rows) - 1,
                'vanished_during_sample': vanished, 'processes': rows}


class Monitor:
    def __init__(self, pid, path, interval=0.25):
        self.pid, self.path, self.interval = pid, path, interval
        self.reader = MacProcesses()
        self.samples, self.error = [], None
        self.label = 'startup'
        self.lock = threading.Lock()
        self.done = threading.Event()
        self.started = time.monotonic()
        self.thread = threading.Thread(target=self.run, daemon=True)

    def run(self):
        try:
            with self.path.open('w') as stream:
                while not self.done.is_set():
                    with self.lock:
                        label = self.label
                    start = time.monotonic()
                    sample = {'elapsed_seconds': start - self.started, 'label': label,
                              **self.reader.tree(self.pid)}
                    sample['sample_seconds'] = time.monotonic() - start
                    self.samples.append(sample)
                    stream.write(json.dumps(sample) + '\n')
                    stream.flush()
                    self.done.wait(max(0, self.interval - sample['sample_seconds']))
        except Exception as exc:
            self.error = exc

    def mark(self, label):
        self.check()
        with self.lock:
            self.label = label

    def check(self):
        if self.error:
            raise RuntimeError('Memory sampling failed') from self.error

    def summary(self, label):
        self.check()
        rows = [row for row in self.samples if row['label'] == label]
        assert rows, f'No samples for {label}'
        fields = ['server_rss_bytes', 'server_footprint_bytes', 'tree_rss_bytes',
                  'tree_footprint_bytes', 'server_threads', 'children']
        return {'samples': len(rows), 'sample_seconds_max': max(r['sample_seconds'] for r in rows),
                'vanished_during_sample': sum(r['vanished_during_sample'] for r in rows),
                **{key: {'first': rows[0][key], 'last': rows[-1][key],
                         'max': max(r[key] for r in rows),
                         'median': statistics.median(r[key] for r in rows)} for key in fields}}

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.done.set()
        self.thread.join(timeout=10)
        assert not self.thread.is_alive(), 'Sampler did not stop'
        self.check()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--cases', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--heads', type=int, default=30)
    parser.add_argument('--replays', type=int, default=10)
    parser.add_argument('--idle-seconds', type=float, default=60)
    args = parser.parse_args()
    assert args.heads >= 3 and args.replays >= 1 and args.idle_seconds >= 1
    os.umask(0o077)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve()
    cases = json.loads(args.cases.read_text())
    assert len({case['name'] for case in cases}) == len(cases)
    assert all(re.fullmatch(r'[A-Za-z0-9_-]+', case['name']) for case in cases)
    config = root / 'models.json'
    save(config, {'models': {}})
    result = {'binary_sha256': sha(binary), 'heads_per_case': args.heads,
              'replays_per_case': args.replays, 'idle_seconds': args.idle_seconds,
              'sample_interval_seconds': 0.25, 'cases': {}, 'complete': False}

    for case in cases:
        name = case['name']
        folder = root / name
        work = folder / 'server'
        clone_store(case['seed_store'], work / 'store')
        original_jobs = job_state(case['seed_store'])
        data = {'rows': [], 'idles': [], 'controls': [], 'checkpoints': [],
                'original_jobs': original_jobs}
        result['cases'][name] = data
        checkpoint_heads = {args.heads // 3, 2 * args.heads // 3, args.heads}
        control_heads = {1, args.heads // 2, args.heads}
        references = {}

        def persist():
            save(root / 'summary.json', result)

        def idle(monitor, label):
            monitor.mark(label)
            print(name, label, 'idle', args.idle_seconds, 'seconds', flush=True)
            until = time.monotonic() + args.idle_seconds
            while time.monotonic() < until:
                time.sleep(min(0.5, max(0, until - time.monotonic())))
                monitor.check()
            data['idles'].append({'label': label, 'memory': monitor.summary(label)})
            persist()

        def measured(client, monitor, request, index, stage, expect_hit):
            label = f'{stage}-{index}'
            monitor.mark(label)
            offset = (work / 'server.log').stat().st_size
            packet, seconds = call(client, request)
            log = batch_log(work, offset, 1)
            assert packet['head'] == request['head'] and packet['base'] == request['base']
            assert packet['tokens'] <= request['max_tokens']
            assert packet['analysis_cache']['base_hit']
            assert packet['analysis_cache']['head_hit'] == expect_hit
            if not expect_hit:
                check(packet, request)
            save(work / f'{label}.json', packet)
            row = {'stage': stage, 'index': index, 'seconds': seconds,
                   'analysis_cache': packet['analysis_cache'], 'tokens': packet['tokens'],
                   'entries': len(packet['entries']), 'impact': packet['impact'],
                   'memory': monitor.summary(label), **log_stats(log)}
            data['rows'].append(row)
            assert job_state(work / 'store') == jobs
            persist()
            print(name, label, round(seconds, 3), 's; RSS/tree MiB',
                  round(row['memory']['server_rss_bytes']['max'] / 2**20, 1),
                  round(row['memory']['tree_rss_bytes']['max'] / 2**20, 1), flush=True)
            return packet

        server = LoggedServer(binary, work / 'store', config, env=environment(work),
                              serve_args=['--no-worker'], timeout=600, startup_timeout=120)
        with server as client, Monitor(server.process.pid, folder / 'memory.jsonl') as monitor:
            idle(monitor, 'before-admission')
            monitor.mark('admission')
            requests = prepare_heads(client, work, case, head_count=args.heads + 1)
            jobs = job_state(work / 'store')
            data.update(base=requests[0]['base'], heads=[r['head'] for r in requests],
                        profile=requests[0]['profile'], max_tokens=requests[0]['max_tokens'],
                        queued_jobs=len(jobs), admission_memory=monitor.summary('admission'))
            # Controls preserve the original cache state and all admitted snapshots.
            clone_store(work / 'store', folder / 'control-seed')
            measured(client, monitor, requests[0], 0, 'prime', False)
            for index in range(1, args.heads + 1):
                packet = measured(client, monitor, requests[index], index, 'new-head', False)
                if index in control_heads:
                    references[index] = canonical(packet)
                if index in checkpoint_heads:
                    idle(monitor, f'after-head-{index}')
                    data['checkpoints'].append({'after_head': index, **cache_counts(work / 'store')})
                    persist()
            for index in range(1, args.replays + 1):
                packet = measured(client, monitor, requests[-1], index, 'cached-replay', True)
                assert canonical(packet) == references[args.heads]
            idle(monitor, 'after-cached-replays')
            assert client.usage()['model_usage']['calls'] == 0
            assert job_state(work / 'store') == jobs
        (work / 'server.log').rename(work / 'retained-server.log')

        # Same persisted store, new process: cache-hit replay followed by idle.
        restarted = LoggedServer(binary, work / 'store', config, env=environment(work),
                                 serve_args=['--no-worker'], timeout=600, startup_timeout=120)
        with restarted as client, Monitor(restarted.process.pid, folder / 'restart-memory.jsonl') as monitor:
            idle(monitor, 'restart-before-review')
            packet = measured(client, monitor, requests[-1], 1, 'restart-replay', True)
            assert canonical(packet) == references[args.heads]
            idle(monitor, 'restart-after-review')
            assert client.usage()['model_usage']['calls'] == 0
            assert job_state(work / 'store') == jobs

        for index in sorted(control_heads):
            control = folder / f'control-{index}'
            clone_store(folder / 'control-seed', control / 'store')
            packet, stats = review(binary, control, folder / f'request-{index}.json',
                                   'review', config, environment(control))
            check(packet, requests[index])
            equal = canonical(packet) == references[index]
            data['controls'].append({'index': index, 'packet_equal': equal, **stats})
            persist()
            assert equal, 'Fresh CLI packet differs'
            print(name, 'control', index, 'equal', equal, flush=True)
        assert job_state(case['seed_store']) == original_jobs
        data['jobs_unchanged'] = True
        data['complete'] = True
        persist()
    result['complete'] = True
    save(root / 'summary.json', result)


if __name__ == '__main__':
    main()
