"""Real pinned Go analysis: scoped coverage stays independent of risk and delivery."""
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'crates/graphicle/tools'))
from investigate_review_coverage import LocalServer, MOD, INTERFACE, PROFILE, admit, fixtures

BINARY = Path(os.environ.get('GRAPHICLE_BIN', ROOT / 'crates/graphicle/target/debug/graphicle'))
FORMAT = 'graphicle.changed-go-coverage.v1'


@unittest.skipUnless(BINARY.is_file(), 'requires a built Graphicle')
class ChangedGoCoverage(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='g4coverage-')
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.server = LocalServer(BINARY, Path(cls.tmp.name) / 'store', serve_args=['--no-worker'])
        cls.client = cls.server.__enter__()
        cls.addClassCleanup(cls.server.__exit__, None, None, None)
        cls.client.timeout = 150
        if cls.client.command('build.info').get('review_analysis_coverage') != FORMAT:
            raise AssertionError('Changed Go coverage capability is missing')

    def review(self, name, before, after, **admission):
        project = self.client.project(name)
        base = admit(project, before, 'a' * 40, **admission)
        head = admit(project, after, 'b' * 40, **admission)
        packet = project.impact([base], [head], profile=PROFILE, max_tokens=16000)
        return project, base, head, packet

    def check_coverage(self, packet, expected):
        c = packet['analysis_coverage']
        self.assertEqual(c['format'], FORMAT)
        self.assertEqual(c['status'], expected, c)
        self.assertFalse(c['review_skipping_permitted'])
        self.assertRegex(c['scope_sha256'], r'^[0-9a-f]{64}$')
        self.assertEqual(packet['risk_schema'], 'graphicle.review-risk.v2')
        self.assertEqual(sum(c[k] for k in ['blocking_component_issues', 'blocking_global_issues',
                                          'outside_component_issues', 'outside_scope_issues']),
                         packet['coverage']['issue_count'])
        # Constant-size summary rather than per-package diagnostics.
        self.assertLess(len(json.dumps(c)), 1000)

    def test_synthetic_matrix(self):
        complete = {'plain', 'unused_json', 'unrelated_interface_package', 'concrete_receiver',
                    'explicit_interface_conversion', 'unrelated_stdlib_api', 'local_interface_binding',
                    'stdlib_api', 'vendored_pure_api', 'packing_only'}
        global_complete = {'plain', 'concrete_receiver', 'explicit_interface_conversion',
                           'local_interface_binding', 'packing_only'}
        for name, files, changed in fixtures():
            with self.subTest(name=name):
                before = {p: t.replace('VALUE', '1') for p, t in files.items()}
                after = {p: t.replace('VALUE', '2' if changed else '1') for p, t in files.items()}
                _, _, _, packet = self.review(name, before, after)
                expected = 'complete' if name in complete else 'not_applicable' if not changed else 'incomplete'
                self.check_coverage(packet, expected)
                impact = 'potential_impact' if name in global_complete else 'no_changes_in_scope' if name == 'identical_plain' else 'incomplete'
                self.assertEqual(packet['impact'], impact)
                if name in {'stdlib_api', 'vendored_pure_api', 'unused_json'}:
                    self.assertGreater(packet['analysis_coverage']['outside_scope_issues'], 0)
                    self.assertFalse(any(e['review_basis']['complete'] for e in packet['entries'] if e['side'] == 'head'))
                if name == 'unrelated_interface_package':
                    self.assertGreater(packet['analysis_coverage']['outside_component_issues'], 0)

    def test_connected_calls_unknown_inputs_and_admission_still_block(self):
        simple = {'go.mod': MOD, 'main.go': 'package app\nfunc Changed() int { return 1 }\n'}
        changed = {**simple, 'main.go': simple['main.go'].replace('return 1', 'return 2')}
        interface = 'package dep\n' + INTERFACE + 'func Dispatch(v Value) int { return v.Get() }\n'
        # An imported supplier's unresolved call blocks, even without a direct call to Dispatch.
        direct = {**simple, 'dep/dep.go': interface, 'main.go':
                  'package app\nimport "example.test/g4/dep"\nfunc Changed() int { return (dep.Concrete{}).Get() + 1 }\n'}
        reverse = {**simple, 'dep/dep.go': interface.replace('package dep\n',
                   'package dep\nimport app "example.test/g4"\n') + 'func Consume() int { return app.Changed() }\n'}
        binding = {**simple, 'dep/dep.go': 'package dep\nfunc Number() int { return 2 }\n'}
        _, vendor, _ = next(f for f in fixtures() if f[0] == 'vendored_pure_api')
        vendor = {p: t.replace('VALUE', '1') for p, t in vendor.items()}
        embedded = {**simple, 'main.go': 'package app\nimport _ "embed"\n//go:embed data.txt\nvar Data string\n',
                    'data.txt': 'before'}
        cases = [
            ('changed-config', {**simple, 'config.json': '{}'}, {**changed, 'config.json': '{"x":1}'}, {}),
            ('partial-admission', simple, changed, {'scope_complete': False}),
            ('direct-import', direct, {**direct, 'main.go': direct['main.go'].replace('+ 1', '+ 2')}, {}),
            ('reverse-import', reverse, {**reverse, 'main.go': changed['main.go']}, {}),
            ('module-change', simple, {**changed, 'go.mod': MOD.replace('1.23', '1.22')}, {}),
            ('binding-change', binding, {**binding, 'main.go':
                'package app\nimport "example.test/g4/dep"\nfunc Changed() int { return dep.Number() }\n'}, {}),
            ('vendor-change', vendor, {**vendor, 'vendor/example.test/dep/dep.go':
                'package dep\nfunc Double(n int) int { return n * 3 }\n'}, {}),
            ('embed-change', embedded, {**embedded, 'data.txt': 'after'}, {}),
        ]
        for name, before, after, admission in cases:
            with self.subTest(name=name):
                _, _, _, packet = self.review(name, before, after, **admission)
                self.check_coverage(packet, 'incomplete')
                if name in {'direct-import', 'reverse-import'}:
                    self.assertGreater(packet['analysis_coverage']['blocking_component_issues'], 0)
                if name == 'partial-admission':
                    self.assertGreater(packet['analysis_coverage']['blocking_global_issues'], 0)
                if name == 'changed-config':
                    self.assertEqual(packet['analysis_coverage']['unmapped_changed_files'], 1)
                if name == 'binding-change':
                    self.assertTrue(packet['analysis_coverage']['binding_changed'])
                if name in {'vendor-change', 'embed-change'}:
                    self.assertGreater(packet['analysis_coverage']['unmapped_changed_files'], 0)

    def test_budget_delta_and_provided_diff_do_not_change_analysis(self):
        _, files, _ = next(f for f in fixtures() if f[0] == 'packing_only')
        before = {p: t.replace('VALUE', '1') for p, t in files.items()}
        after = {p: t.replace('VALUE', '2') for p, t in files.items()}
        project, base, head, full = self.review('delivery', before, after)
        self.check_coverage(full, 'complete')
        small = project.impact([base], [head], profile=PROFILE, max_tokens=4000)
        self.assertGreater(small['coverage']['budget_omitted'], 0)
        delta = project.impact([base], [head], profile=PROFILE, max_tokens=4000,
                               previous_packet_id=small['packet_id'], reuse_text=True)
        spans = [{'repository_id': project.id, 'side': side, 'file': 'main.go',
                  'file_sha256': hashlib.sha256(source['main.go'].encode()).hexdigest(),
                  'line_start': 1, 'line_end': len(source['main.go'].splitlines())}
                 for side, source in [('base', before), ('head', after)]]
        provided = project.impact([base], [head], profile=PROFILE, max_tokens=4000, provided_diff=spans)
        self.assertGreater(provided['coverage']['provided_diff']['entries'], 0)
        for packet in [small, delta, provided]:
            self.assertEqual(packet['analysis_coverage'], full['analysis_coverage'])
            self.assertEqual(packet['impact'], full['impact'])
        self.assertEqual(self.client.usage()['model_usage']['calls'], 0)


if __name__ == '__main__':
    unittest.main()
