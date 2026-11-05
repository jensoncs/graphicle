"""Regression checks for large-manifest and cold-capture benchmark admission."""
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
TOOLS = ROOT / 'crates/graphicle/tools'
sys.path.insert(0, str(TOOLS))
from benchmark_pinned_review import LocalServer
from benchmark_review_cache_reuse import cache_counts, environment
from benchmark_review_soak import check, job_state, prepare_manifest

BINARY = Path(os.environ.get('GRAPHICLE_BIN', ROOT / 'crates/graphicle/target/debug/graphicle')).resolve()


class ProtocolChecks(unittest.TestCase):
    def test_manifest_boundaries_preserve_first_part_metadata_and_continuation(self):
        class Client:
            def __init__(self):
                self.calls = []

            def command(self, operation, **args):
                self.calls.append((operation, args))
                return {'sync_id': 'pending', 'snapshot': 'complete'}

        for count in [0, 5000, 5001, 10001]:
            with self.subTest(count=count):
                client = Client()
                manifest = [{'path': str(i)} for i in range(count)]
                metadata = {'commit': 'pinned', 'scope_complete': True}
                result = prepare_manifest(client, 'repo', 'previous', manifest, metadata)
                self.assertEqual(result['snapshot'], 'complete')
                self.assertEqual(len(client.calls), max(1, (count + 4999) // 5000))
                self.assertEqual([entry for _, args in client.calls for entry in args['manifest']], manifest)
                for index, (operation, args) in enumerate(client.calls):
                    self.assertEqual(operation, 'sync.prepare')
                    self.assertLessEqual(len(args['manifest']), 5000)
                    self.assertEqual(args['manifest_complete'], index == len(client.calls) - 1)
                    if index == 0:
                        self.assertEqual(args['expected_snapshot'], 'previous')
                        self.assertEqual(args['metadata'], metadata)
                        self.assertNotIn('sync_id', args)
                    else:
                        self.assertEqual(args['sync_id'], 'pending')
                        self.assertNotIn('metadata', args)
                        self.assertNotIn('expected_snapshot', args)

    def test_measured_reviews_still_refuse_wrong_pins_cache_states_and_budgets(self):
        request = {'base': ['base'], 'head': ['new-head'], 'max_tokens': 8000}
        packet = {**request, 'tokens': 100, 'analysis_cache': {'base_hit': True, 'head_hit': False}}
        check(packet, request)
        for update in [{'base': ['wrong']}, {'head': ['wrong']}, {'tokens': 8001},
                       {'analysis_cache': {'base_hit': False, 'head_hit': False}},
                       {'analysis_cache': {'base_hit': True, 'head_hit': True}}]:
            with self.subTest(update=update), self.assertRaises(AssertionError):
                check({**copy.deepcopy(packet), **update}, request)


@unittest.skipUnless(BINARY.is_file(), 'build graphicle or set GRAPHICLE_BIN')
class LargeColdCapture(unittest.TestCase):
    def test_full_soak_accepts_multipart_manifest_and_warms_cold_base(self):
        with tempfile.TemporaryDirectory(prefix='graphicle-large-cold-soak-') as tmp:
            root = Path(tmp)
            store = root / 'captured-store'
            config = root / 'models.json'
            config.write_text('{"models":{}}')
            policy = hashlib.sha256(b'large cold captured fixture').hexdigest()
            binary_sha = hashlib.sha256(b'\0').hexdigest()
            filler = [{'path': f'assets/{i:05}.bin', 'sha256': binary_sha, 'bytes': 1}
                      for i in range(5001)]

            def admit(client, repo, expected, value):
                texts = {'go.mod': 'module example.test/soak\n\ngo 1.23\n',
                         'main.go': f'package soak\nfunc Changed() int {{ return {value} }}\n'}
                manifest = filler + [{'path': path, 'sha256': hashlib.sha256(text.encode()).hexdigest(),
                                      'bytes': len(text.encode())} for path, text in texts.items()]
                commit = str(value) * 40
                page = prepare_manifest(client, repo, expected, manifest,
                                        {'commit': commit, 'policy_sha256': policy, 'scope_complete': True})
                snapshot, sync_id = page['snapshot'], page['sync_id']
                while True:
                    for path in page['missing']:
                        file = {'path': path, 'content': texts[path]} if path in texts else {
                            'path': path, 'content_base64': 'AA=='}
                        client.command('sync.upload', repository_id=repo, sync_id=sync_id, files=[file])
                    if not page.get('next_cursor'):
                        break
                    page = client.command('sync.missing', repository_id=repo, sync_id=sync_id,
                                          cursor=page['next_cursor'])
                client.command('sync.commit', repository_id=repo, sync_id=sync_id, compact=True)
                return {'repository_id': repo, 'commit': commit, 'snapshot': snapshot,
                        'policy_sha256': policy, 'directory': 'app', 'modules': ['.']}

            with LocalServer(BINARY, store, config, env=environment(root),
                             serve_args=['--no-worker'], timeout=120) as client:
                repo = client.project('large-cold-soak').id
                base = admit(client, repo, '', 1)
                head = admit(client, repo, base['snapshot'], 2)
            self.assertEqual(cache_counts(store)['full_analyses'], 0)
            jobs = job_state(store)
            request = {'repository_id': repo, 'base': [base], 'head': [head],
                       'profile': {'goos': 'linux', 'goarch': 'amd64', 'tags': [], 'tests': False},
                       'max_tokens': 8000}
            request_path = root / 'request.json'
            request_path.write_text(json.dumps(request))
            cases = root / 'cases.json'
            cases.write_text(json.dumps([{'name': 'large-cold', 'seed_store': str(store),
                                          'next_request': str(request_path), 'edit_path': 'main.go'}]))
            output = root / 'soak'
            completed = subprocess.run([sys.executable, str(TOOLS / 'benchmark_review_soak.py'),
                                       '--binary', str(BINARY), '--cases', str(cases), '--output', str(output)],
                                      capture_output=True, text=True, timeout=300)
            self.assertEqual(completed.returncode, 0, completed.stdout[-3000:] + completed.stderr[-3000:])
            results = json.loads((output / 'summary.json').read_text())
            self.assertTrue(results['complete'])
            self.assertEqual(len(results['rows']), 20)
            self.assertTrue(all(row['packet_equal'] for row in results['rows']))
            priming = results['cases']['large-cold']['prime_untimed']
            self.assertFalse(priming['analysis_cache']['base_hit'])
            self.assertFalse(priming['analysis_cache']['head_hit'])
            self.assertTrue(priming['verification']['analysis_cache']['base_hit'])
            self.assertTrue(priming['verification']['analysis_cache']['head_hit'])
            self.assertTrue(priming['verification']['packet_equal'])
            self.assertTrue(all(row['analysis_cache']['base_hit'] and not row['analysis_cache']['head_hit']
                                for row in results['rows']))
            self.assertEqual(job_state(store), jobs)
            self.assertEqual(cache_counts(store)['full_analyses'], 0)
            self.assertFalse(list(output.rglob('server.json')))


if __name__ == '__main__':
    unittest.main()
