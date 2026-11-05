"""Retained review service: real cancellation recovery and same-store context."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
import hashlib
import os
from pathlib import Path
import shlex
import shutil
import sqlite3
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'sdk/python'))
from graphicle_sdk import Client, GraphicleError, LocalServer

BINARY = Path(os.environ.get('GRAPHICLE_BIN', ROOT / 'crates/graphicle/target/debug/graphicle'))
PROFILE = {'goos': 'linux', 'goarch': 'amd64', 'tests': False}


def admit(project, value):
    files = {'go.mod': 'module example.test/service\n\ngo 1.23\n',
             'main.go': f'package service\nfunc CalculateTax(income int) int {{ return income / {value} }}\n'}
    policy = hashlib.sha256(b'review service fixture').hexdigest()
    commit = str(value) * 40
    prepared = project.command('sync.prepare',
        expected_snapshot=project.status(compact=True)['repository']['snapshot'],
        metadata={'commit': commit, 'policy_sha256': policy, 'scope_complete': True},
        manifest=[{'path': path, 'sha256': hashlib.sha256(text.encode()).hexdigest(), 'bytes': len(text.encode())}
                  for path, text in files.items()])
    if prepared['sync_id']:
        project.command('sync.upload', sync_id=prepared['sync_id'],
                        files=[{'path': path, 'content': files[path]} for path in prepared['missing']])
        project.command('sync.commit', sync_id=prepared['sync_id'], compact=True)
    return {'repository_id': project.id, 'directory': 'app', 'commit': commit,
            'snapshot': prepared['snapshot'], 'policy_sha256': policy}


def rows(store, sql):
    with closing(sqlite3.connect((store / 'reviews.sqlite3').as_uri() + '?mode=ro', uri=True)) as db:
        return db.execute(sql).fetchall()


@unittest.skipUnless(BINARY.is_file(), 'requires a built Graphicle')
class ReviewService(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='greview-', dir='/private/tmp' if sys.platform == 'darwin' else None)
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.store = self.root / 'store'

    def test_context_reuses_review_snapshot_with_worker_disabled(self):
        with LocalServer(BINARY, self.store, serve_args=['--no-worker']) as client:
            project = client.project('same-store')
            base, head = admit(project, 1), admit(project, 2)
            packet = project.impact([base], [head], profile=PROFILE, max_tokens=8000)
            self.assertEqual(packet['head'], [head | {'modules': ['.']}])
            jobs_sql = 'SELECT id,kind,state,attempt,lease_token,lease_until FROM am_jobs ORDER BY id'
            before = rows(self.store, jobs_sql)
            snapshots = rows(self.store, 'SELECT repository_id,sha256 FROM repo_snapshots ORDER BY sha256')
            # No sync, projection, or model call is needed to retrieve admitted file text.
            context = project.secondary_context('Primary diff', 'CalculateTax income', enabled=True,
                                                expected_snapshot=head['snapshot'])
            self.assertEqual(context['secondary']['status'], 'included', context)
            self.assertEqual(context['primary'], 'Primary diff')
            evidence = context['secondary']['evidence']
            self.assertTrue(evidence)
            self.assertTrue(all(e['snapshot'] == head['snapshot'] for e in evidence))
            self.assertTrue(any('income / 2' in e['text'] for e in evidence))
            self.assertFalse(any('income / 1' in e['text'] for e in evidence))
            self.assertEqual(rows(self.store, jobs_sql), before)
            self.assertEqual(rows(self.store, 'SELECT repository_id,sha256 FROM repo_snapshots ORDER BY sha256'), snapshots)
            admit(project, 3)
            stale = project.secondary_context('Primary diff', 'CalculateTax income', enabled=True,
                                              expected_snapshot=head['snapshot'])
            self.assertEqual(stale['secondary']['status'], 'stale_snapshot')
            self.assertEqual(stale['secondary']['evidence'], [])

    @unittest.skipUnless(os.name == 'posix' and shutil.which('go'), 'requires native Go and POSIX processes')
    def test_deadline_stops_children_retries_helper_and_keeps_warm_server(self):
        real_go = shutil.which('go')
        shim = self.root / 'bin'
        shim.mkdir()
        trigger, parent, child, builds = [self.root / name for name in ['block', 'parent', 'child', 'builds']]
        # Exercise the real subprocess boundary deterministically, including a
        # descendant holding inherited pipes. The next compilation uses real Go.
        wrapper = shim / 'go'
        wrapper.write_text(f'''#!/bin/sh
if [ "$1" = build ]; then
    echo build >> {shlex.quote(str(builds))}
    if [ -f {shlex.quote(str(trigger))} ]; then
        rm {shlex.quote(str(trigger))}
        echo $$ > {shlex.quote(str(parent))}
        sleep 60 &
        echo $! > {shlex.quote(str(child))}
        wait
    fi
fi
exec {shlex.quote(real_go)} "$@"
''')
        wrapper.chmod(0o700)
        env = {**os.environ, 'PATH': str(shim) + os.pathsep + os.environ['PATH']}
        with LocalServer(BINARY, self.store, env=env, serve_args=['--no-worker']) as client:
            client.timeout = 150
            project = client.project('deadline-recovery')
            base, head = admit(project, 1), admit(project, 2)
            request = {'repository_id': project.id, 'base': [base], 'head': [head],
                       'profile': PROFILE, 'max_tokens': 8000}
            for invalid in [0, -1, 600001, '100', True]:
                with self.subTest(timeout=invalid), self.assertRaises(GraphicleError) as error:
                    project.command('graph.impact', **{k: v for k, v in request.items() if k != 'repository_id'}, timeout_ms=invalid)
                self.assertEqual(error.exception.status, 400)
            trigger.touch()
            started = time.monotonic()
            with ThreadPoolExecutor(max_workers=1) as executor:
                future = executor.submit(client.command, 'graph.impact', **request, timeout_ms=2000)
                while not child.exists() and not future.done() and time.monotonic() - started < 5:
                    time.sleep(0.01)
                self.assertTrue(child.exists(), 'native compilation did not reach test blocker')
                # A second request cannot get stuck behind the helper initializer.
                waiting = time.monotonic()
                with self.assertRaises(GraphicleError) as queued:
                    client.request('POST', f'/v1/workspaces/{client.workspace}/repos/{project.id}/impact/report',
                                   {**request, 'timeout_ms': 100})
                self.assertEqual(queued.exception.code, 'timeout')
                self.assertLess(time.monotonic() - waiting, 1.5)
                with self.assertRaises(GraphicleError) as timed_out:
                    future.result(timeout=10)
            self.assertEqual(timed_out.exception.status, 504)
            self.assertEqual(timed_out.exception.code, 'timeout')
            self.assertLess(time.monotonic() - started, 7)
            for pid_file in [parent, child]:
                pid = int(pid_file.read_text())
                reaped_by = time.monotonic() + 3
                while True:
                    try:
                        os.kill(pid, 0)
                    except ProcessLookupError:
                        break
                    if sys.platform.startswith('linux'):
                        try:
                            # An orphaned grandchild may await the host's reaper;
                            # a zombie is stopped and cannot do further work.
                            if Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1].split()[0] == 'Z':
                                break
                        except FileNotFoundError:
                            break
                    if time.monotonic() >= reaped_by:
                        self.fail(f'Native process {pid} still running after timeout response')
                    time.sleep(0.01)
            self.assertEqual(rows(self.store, 'SELECT count(*) FROM repo_review_packets'), [(0,)])
            recovered = project.impact([base], [head], profile=PROFILE, max_tokens=8000, timeout_ms=120000)
            self.assertEqual(recovered['impact'], 'potential_impact', recovered['coverage'])
            self.assertEqual(len(builds.read_text().splitlines()), 2)
            warm = project.impact([base], [head], profile=PROFILE, max_tokens=8000, timeout_ms=10000)
            self.assertEqual(warm['packet_id'], recovered['packet_id'])
            self.assertTrue(warm['analysis_cache']['base_hit'])
            self.assertTrue(warm['analysis_cache']['head_hit'])
            self.assertEqual(len(builds.read_text().splitlines()), 2)
            client.health()


if __name__ == '__main__':
    unittest.main()
