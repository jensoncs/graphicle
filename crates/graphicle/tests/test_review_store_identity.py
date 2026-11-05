"""Packing must not depend on a project's randomly assigned store identity."""
from contextlib import ExitStack
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'crates/graphicle/tools'))
from benchmark_pinned_review import canonical, review, save
from benchmark_review_cache_reuse import environment
from benchmark_review_soak import LoggedServer
from investigate_review_coverage import MOD, PROFILE, admit

BINARY = Path(os.environ.get('GRAPHICLE_BIN', ROOT / 'crates/graphicle/target/debug/graphicle')).resolve()


def source(value):
    return {'go.mod': MOD,
            'main.go': f'package app\nfunc Changed() int {{ return {value} }}\n',
            'callers.go': 'package app\n' + '\n'.join(
                f'func Caller{i:02}() int {{ return Changed() + {i} }}' for i in range(12))}


def selected(packet):
    return {key: [{k: entry.get(k) for k in ('side', 'file', 'name', 'byte_start', 'byte_end', 'text', 'hunk')}
                  for entry in packet[key]] for key in ('entries', 'diffs')}


@unittest.skipUnless(BINARY.is_file(), 'requires a built Graphicle')
class StoreIndependentPacking(unittest.TestCase):
    def test_separate_stores_select_same_context_and_cli_matches_server(self):
        with tempfile.TemporaryDirectory(prefix='graphicle-store-identity-') as tmp, ExitStack() as stack:
            root = Path(tmp)
            config = root / 'models.json'
            save(config, {'models': {}})
            arms = []
            for index in range(2):
                work = root / str(index)
                client = stack.enter_context(LoggedServer(BINARY, work / 'store', config,
                                                         env=environment(work), serve_args=['--no-worker'], timeout=150))
                project = client.project('same-logical-project')
                base = {**admit(project, source(1), 'a' * 40), 'modules': ['.']}
                head = {**admit(project, source(2), 'b' * 40), 'modules': ['.']}
                request = {'repository_id': project.id, 'base': [base], 'head': [head],
                           'profile': PROFILE, 'compact_provenance': True}
                arms.append((work, client, request))
            self.assertNotEqual(arms[0][2]['repository_id'], arms[1][2]['repository_id'])
            for side in ('base', 'head'):
                self.assertEqual(arms[0][2][side][0]['snapshot'], arms[1][2][side][0]['snapshot'])
            omitted = False
            for budget in (4500, 6000, 8000, 12000):
                packets = []
                for work, client, args in arms:
                    request = {**args, 'max_tokens': budget}
                    packet = client.command('graph.impact', **request)
                    save(work / f'{budget}-request.json', request)
                    save(work / f'{budget}-server.json', packet)
                    self.assertLessEqual(packet['tokens'], budget)
                    self.assertTrue(all(e['repository_id'] == args['repository_id'] for e in packet['entries']))
                    packets.append(packet)
                    omitted |= packet['coverage']['budget_omitted_entries'] > 0
                self.assertEqual(selected(packets[0]), selected(packets[1]))
                self.assertEqual(packets[0]['coverage']['packing'], packets[1]['coverage']['packing'])
                self.assertEqual(packets[0]['coverage']['current_head'], packets[1]['coverage']['current_head'])
            self.assertTrue(omitted, 'Exercise constrained context selection')
            # Same-store CLI/server equality isolates transport from registration.
            for (work, client, _), packet in zip(arms, packets):
                cli, _ = review(BINARY, work, work / '12000-request.json', 'cli', config, environment(work))
                self.assertEqual(canonical(packet), canonical(cli))
                self.assertEqual(client.usage()['model_usage']['calls'], 0)


if __name__ == '__main__':
    unittest.main()
