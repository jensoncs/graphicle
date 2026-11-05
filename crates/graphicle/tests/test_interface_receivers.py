"""Exact interface targets, reassignment refusal, and retained-store invalidation."""
import base64
from contextlib import closing
import gzip
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'crates/graphicle/tools'))
from investigate_interface_receivers import fixtures, source
from investigate_review_coverage import LocalServer, MOD, PROFILE, admit

BINARY = Path(os.environ.get('GRAPHICLE_BIN', ROOT / 'crates/graphicle/target/debug/graphicle'))
BEFORE = os.environ.get('GRAPHICLE_BEFORE_RECEIVERS_BIN')


def head_calls(store, packet):
    pin, = packet['head']
    with closing(sqlite3.connect((store / 'reviews.sqlite3').as_uri() + '?mode=ro', uri=True)) as db:
        rows = db.execute('SELECT DISTINCT a.facts FROM repo_review_analysis a '
                          'JOIN repo_review_analysis_inputs i ON i.repository_id=a.repository_id AND i.id=a.id '
                          'WHERE a.repository_id=? AND i.input_repository=? AND i.snapshot=?',
                          (pin['repository_id'], pin['repository_id'], pin['snapshot'])).fetchall()
    if len(rows) != 1:
        raise AssertionError(f'Expected one exact pinned analysis, got {len(rows)}')
    envelope = json.loads(rows[0][0])
    assert envelope['encoding'] == 'gzip-base64-v1'
    facts = json.loads(gzip.decompress(base64.b64decode(envelope['facts'], validate=True)))
    return [c for w in facts['worlds'] for c in w['calls']]


@unittest.skipUnless(BINARY.is_file(), 'requires a built Graphicle')
class InterfaceReceivers(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='receiver-test-')
        self.addCleanup(self.tmp.cleanup)
        self.store = Path(self.tmp.name) / 'store'

    def receiver(self, packet):
        site, = [c for c in head_calls(self.store, packet) if c['relation'] == 'calls'
                 and '.Use|' in c['caller'] and c['spelling'].endswith('.Get')]
        return site

    def test_matrix_resolves_proved_cases_and_keeps_unknown_receivers(self):
        with LocalServer(BINARY, self.store, serve_args=['--no-worker'], timeout=150) as client:
            for group, name, declarations, body in fixtures():
                with self.subTest(name=name):
                    p = client.project(name)
                    base = admit(p, {'go.mod': MOD, 'main.go': source(declarations, body, 1)}, 'a' * 40)
                    head = admit(p, {'go.mod': MOD, 'main.go': source(declarations, body, 2)}, 'b' * 40)
                    packet = p.impact([base], [head], profile=PROFILE, max_tokens=64000)
                    resolved = group != 'counterexample'
                    self.assertEqual(packet['analysis_coverage']['status'], 'complete' if resolved else 'incomplete')
                    self.assertEqual(packet['analysis_coverage']['blocking_component_issues'], 0 if resolved else 2)
                    self.assertEqual(packet['impact'], 'potential_impact' if resolved else 'incomplete')
                    site = self.receiver(packet)
                    if resolved:
                        self.assertNotIn('reason', site)
                        self.assertIn('Concrete).Get', site['target'])
                    else:
                        self.assertEqual(site['reason'], 'interface_dispatch')
                    if name == 'single_return':
                        self.assertTrue(any('.Use|' in c['caller'] and c['spelling'] == 'New'
                                            and '.New|' in c.get('target', '') for c in head_calls(self.store, packet)))
            self.assertEqual(client.usage()['model_usage']['calls'], 0)

    def test_changed_initializer_and_reassignment_invalidate_warm_targets_and_reuse(self):
        with LocalServer(BINARY, self.store, serve_args=['--no-worker'], timeout=150) as client:
            for kind in ['factory', 'field']:
                with self.subTest(kind=kind):
                    p = client.project(kind)
                    def files(value, target, reassigned=False):
                        if kind == 'factory':
                            declarations = ('func New(v Value) Value { var result Value = ' + target +
                                            '{}; ' + ('result = v; ' if reassigned else '') + 'return result }')
                            body = 'return New(input).Get()'
                        else:
                            declarations = ''
                            body = ('h := Holder{value: ' + target + '{}}; ' +
                                    ('h.value = input; ' if reassigned else '') + 'return h.value.Get()')
                        return {'go.mod': MOD, 'main.go': source(declarations, body, value)}
                    base = admit(p, files(1, 'Concrete'), 'a' * 40)
                    head = admit(p, files(2, 'Concrete'), 'b' * 40)
                    first = p.impact([base], [head], profile=PROFILE, max_tokens=64000)
                    warm = p.impact([base], [head], profile=PROFILE, max_tokens=64000)
                    self.assertTrue(warm['analysis_cache']['head_hit'])
                    self.assertEqual(warm['packet_id'], first['packet_id'])
                    self.assertIn('Concrete).Get', self.receiver(warm)['target'])
                    reviewed = [e['id'] for e in first['entries'] if e['side'] == 'head']
                    changed = admit(p, files(2, 'Other'), 'c' * 40)
                    second = p.impact([base], [changed], profile=PROFILE, max_tokens=64000,
                                      previous_packet_id=first['packet_id'], reviewed_entries=reviewed)
                    self.assertFalse(second['analysis_cache']['head_hit'])
                    self.assertIn('Other).Get', self.receiver(second)['target'])
                    self.assertEqual(second['analysis_coverage']['status'], 'complete')
                    self.assertEqual(second['review_reuse']['eligible'], [])
                    uncertain = admit(p, files(2, 'Other', reassigned=True), 'd' * 40)
                    third = p.impact([base], [uncertain], profile=PROFILE, max_tokens=64000)
                    self.assertEqual(third['analysis_coverage']['status'], 'incomplete')
                    self.assertEqual(self.receiver(third)['reason'], 'interface_dispatch')
                    self.assertEqual(third['review_reuse']['eligible'], [])

    @unittest.skipUnless(BEFORE, 'set GRAPHICLE_BEFORE_RECEIVERS_BIN to test analyzer upgrade')
    def test_analyzer_upgrade_does_not_reuse_old_incomplete_facts(self):
        with LocalServer(BEFORE, self.store, serve_args=['--no-worker'], timeout=150) as client:
            p = client.project('upgrade')
            def files(n):
                return {'go.mod': MOD, 'main.go': source('', 'h:=Holder{value:Concrete{}}; return h.value.Get()', n)}
            base, head = admit(p, files(1), 'a' * 40), admit(p, files(2), 'b' * 40)
            old = p.impact([base], [head], profile=PROFILE, max_tokens=64000)
            self.assertEqual(old['analysis_coverage']['status'], 'incomplete')
        with LocalServer(BINARY, self.store, serve_args=['--no-worker'], timeout=150) as client:
            p = client.project('upgrade')
            new = p.impact([base], [head], profile=PROFILE, max_tokens=64000)
            self.assertFalse(new['analysis_cache']['base_hit'])
            self.assertFalse(new['analysis_cache']['head_hit'])
            self.assertEqual(new['analysis_coverage']['status'], 'complete')
            warm = p.impact([base], [head], profile=PROFILE, max_tokens=64000)
            self.assertTrue(warm['analysis_cache']['base_hit'])
            self.assertTrue(warm['analysis_cache']['head_hit'])
            self.assertEqual(warm['packet_id'], new['packet_id'])


if __name__ == '__main__':
    unittest.main()
