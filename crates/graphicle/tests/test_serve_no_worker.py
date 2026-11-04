"""Real HTTP lifecycle test: API-only serving preserves jobs for worker processes."""
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'sdk/python'))
from graphicle_sdk import Client, GraphicleError, LocalServer

BINARY = Path(os.environ.get('GRAPHICLE_BIN', ROOT / 'crates/graphicle/target/debug/graphicle'))


@unittest.skipUnless(BINARY.is_file(), 'build graphicle or set GRAPHICLE_BIN')
class ServeWithoutWorker(unittest.TestCase):
    def test_reviews_keep_jobs_pending_and_workers_can_resume_them(self):
        with tempfile.TemporaryDirectory(prefix='graphicle-no-worker-') as tmp:
            root = Path(tmp)
            store = root / 'store'
            config = root / 'models.json'
            config.write_text('{"models":{}}')
            env = {'PATH': os.environ['PATH'], 'HOME': tmp, 'TMPDIR': tmp,
                   'GRAPHICLE_PHASE_TIMINGS': '1', 'RUST_MIN_STACK': '16777216'}
            policy = hashlib.sha256(b'no-worker fixture').hexdigest()

            def jobs():
                with sqlite3.connect((store / 'reviews.sqlite3').as_uri() + '?mode=ro', uri=True) as db:
                    return db.execute('SELECT id,state,attempt,lease_token,lease_until,error FROM am_jobs ORDER BY id').fetchall()

            def pin(client, repo, expected, value):
                content = f'def changed():\n    return {value}\n'
                commit = str(value) * 40
                prepared = client.command('sync.prepare', repository_id=repo, expected_snapshot=expected,
                    manifest=[{'path': 'a.py', 'sha256': hashlib.sha256(content.encode()).hexdigest(), 'bytes': len(content)}],
                    metadata={'commit': commit, 'policy_sha256': policy, 'scope_complete': True})
                client.command('sync.upload', repository_id=repo, sync_id=prepared['sync_id'],
                               files=[{'path': 'a.py', 'content': content}])
                client.command('sync.commit', repository_id=repo, sync_id=prepared['sync_id'], compact=True)
                return {'repository_id': repo, 'commit': commit, 'snapshot': prepared['snapshot'],
                        'policy_sha256': policy, 'directory': 'app', 'modules': ['.']}

            server = LocalServer(BINARY, store, config, env=env, serve_args=['--no-worker'])
            with server as client:
                repo = client.project('no-worker').id
                base = pin(client, repo, '', 1)
                head = pin(client, repo, base['snapshot'], 2)
                pending = jobs()
                self.assertEqual(len(pending), 2)
                self.assertTrue(all(row[1] == 'queued' and row[2] == 0 for row in pending))
                request = {'repository_id': repo, 'base': [base], 'head': [head],
                           'profile': {'goos': 'linux', 'goarch': 'amd64', 'tags': [], 'tests': False},
                           'max_tokens': 8000}
                packet = client.command('graph.impact', **request)
                self.assertEqual(packet['head'], [head])
                self.assertLessEqual(packet['tokens'], 8000)
                with self.assertRaises(GraphicleError) as refused:
                    Client(server.url, client.workspace, 'invalid-capability').command('graph.impact', **request)
                self.assertIn(refused.exception.status, (401, 403, 404))
                # The default worker polls each second. Leave multiple opportunities.
                time.sleep(2.1)
                self.assertEqual(jobs(), pending)
                self.assertNotIn('phase=graph_materialize', server.logs())
                for _ in pending:
                    subprocess.run([str(BINARY), '--data-dir', str(store), '--config', str(config),
                                    'worker', '--once'], env=env, check=True, capture_output=True, timeout=30)
                resumed = {row[0]: row for row in jobs()}
                self.assertTrue(all(resumed[row[0]][1] == 'complete' for row in pending), jobs())
                pin(client, repo, head['snapshot'], 3)
                self.assertTrue(any(row[1] == 'queued' for row in jobs()))
            # Omitting the flag preserves ordinary serve's automatic worker behavior.
            with LocalServer(BINARY, store, config, env=env):
                deadline = time.monotonic() + 30
                while any(row[1] != 'complete' for row in jobs()) and time.monotonic() < deadline:
                    time.sleep(0.05)
                self.assertTrue(all(row[1] == 'complete' for row in jobs()), jobs())


if __name__ == '__main__':
    unittest.main()
