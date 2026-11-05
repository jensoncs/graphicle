"""Explicit Go build scope: template isolation without hiding unmodeled changes."""
import os
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'crates/graphicle/tools'))
from investigate_review_coverage import LocalServer, MOD, PROFILE, admit
from benchmark_pinned_review import canonical

BINARY = Path(os.environ.get('GRAPHICLE_BIN', ROOT / 'crates/graphicle/target/debug/graphicle'))
TEMPLATE = 'templates/{{cookiecutter.serviceName}}/main.go'


def files(value=1):
    return {
        'go.mod': MOD + '\nrequire example.test/dep v1.0.0\n',
        'vendor/modules.txt': '# example.test/dep v1.0.0\n## explicit; go 1.23\nexample.test/dep\n',
        'vendor/example.test/dep/dep.go': 'package dep\nfunc Double(n int) int { return n * 2 }\n',
        'service/main.go': 'package service\nimport ("example.test/dep"; "example.test/g4/support")\n'
                           f'func Changed() int {{ return dep.Double(support.Value()) + {value} }}\n',
        'service/consumer/consumer.go': 'package consumer\nimport "example.test/g4/service"\n'
                                        'func Use() int { return service.Changed() }\n',
        'support/helper.go': 'package support\nfunc Value() int { return 7 }\n',
        TEMPLATE: 'package main\nfunc main() {}\n',
    }


@unittest.skipUnless(BINARY.is_file(), 'requires a built Graphicle')
class ReviewBuildScope(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix='graphicle-build-scope-')
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.server = LocalServer(BINARY, Path(cls.tmp.name) / 'store', serve_args=['--no-worker'])
        cls.client = cls.server.__enter__()
        cls.addClassCleanup(cls.server.__exit__, None, None, None)
        cls.client.timeout = 150

    def impact(self, project, base, head, modules):
        packet = project.impact([{**base, 'modules': modules}], [{**head, 'modules': modules}],
                                profile=PROFILE, max_tokens=16000)
        self.assertLessEqual(packet['tokens'], 16000)
        return packet

    def test_scope_reuses_facts_without_certifying_excluded_templates(self):
        project = self.client.project('template-scope-cache')
        base = admit(project, files(1), 'a' * 40)
        head = admit(project, files(2), 'b' * 40)
        broad = self.impact(project, base, head, ['.'])
        replay = self.impact(project, base, head, ['.'])
        for packet in [broad, replay]:
            for side in ['base', 'head']:
                self.assertFalse(packet['analysis_cache'][side + '_hit'])
                self.assertEqual(packet['coverage']['analysis_diagnostics'][side]['go']
                                 ['discovery_incomplete_package_instances'], 1)
        self.assertEqual(canonical(broad), canonical(replay))

        # The service imports an owned package outside its selected subtree and
        # a vendored API. Both remain admitted and available to the compiler.
        scoped = self.impact(project, base, head, ['service'])
        warm = self.impact(project, base, head, ['service'])
        for side in ['base', 'head']:
            self.assertFalse(scoped['analysis_cache'][side + '_hit'])
            self.assertTrue(warm['analysis_cache'][side + '_hit'])
            go = warm['coverage']['analysis_diagnostics'][side]['go']
            self.assertEqual(go['discovery_incomplete_package_instances'], 0)
            self.assertEqual(go['type_check_failed_package_instances'], 0)
            self.assertEqual(go['unavailable_external_package_instances'], 0)
            self.assertEqual(go['owned_package_instances'], 3)
            self.assertEqual(go['dependency_package_instances'], 1)
        self.assertEqual(canonical(scoped), canonical(warm))
        for packet in [broad, scoped, warm]:
            self.assertTrue(any(e['file'] == 'service/consumer/consumer.go'
                                for e in packet['entries']), 'Unchanged caller context was lost')
        self.assertEqual(warm['impact'], 'incomplete')
        self.assertEqual(warm['analysis_coverage']['status'], 'incomplete')
        self.assertFalse(warm['analysis_coverage']['review_skipping_permitted'])
        self.assertTrue(any(g['reason'] == 'go_source_outside_module_scope'
                            for g in warm['coverage']['issue_histogram']['groups']))

        next_head = admit(project, files(3), 'c' * 40)
        next_packet = self.impact(project, base, next_head, ['service'])
        self.assertTrue(next_packet['analysis_cache']['base_hit'])
        self.assertFalse(next_packet['analysis_cache']['head_hit'])
        self.assertGreaterEqual(next_packet['analysis_cache']['packages']['head']['hits'], 1)
        # Broadening scope must not reuse facts collected under the smaller scope.
        broad_again = self.impact(project, base, head, ['.'])
        self.assertFalse(broad_again['analysis_cache']['base_hit'])
        self.assertFalse(broad_again['analysis_cache']['head_hit'])
        self.assertEqual(canonical(broad), canonical(broad_again))

    def test_changed_template_remains_in_packet_and_invalidates_head_facts(self):
        project = self.client.project('changed-template')
        original = files()
        base = admit(project, original, 'a' * 40)
        self.impact(project, base, base, ['service'])
        changed = {**original, TEMPLATE: 'package main\nfunc main() { println("changed") }\n'}
        head = admit(project, changed, 'b' * 40)
        packet = self.impact(project, base, head, ['service'])
        self.assertTrue(packet['analysis_cache']['base_hit'])
        self.assertFalse(packet['analysis_cache']['head_hit'])
        self.assertEqual(packet['impact'], 'incomplete')
        self.assertGreater(packet['analysis_coverage']['unmapped_changed_files'], 0)
        self.assertTrue(any(f['file'] == 'app/' + TEMPLATE and f['classification'] == 'uncertain_scope'
                            for f in packet['file_changes']['examples']))
        self.assertTrue(any(d['file'] == 'app/' + TEMPLATE for d in packet['diffs']))
        self.assertEqual(packet['coverage']['budget_omitted_diffs'], 0)

    def test_imported_package_gap_still_blocks_scoped_analysis(self):
        project = self.client.project('imported-gap')
        before = files(1)
        before['support/helper.go'] += 'type ValueSource interface { Get() int }\nfunc Dispatch(v ValueSource) int { return v.Get() }\n'
        after = {**before, 'service/main.go': files(2)['service/main.go']}
        base = admit(project, before, 'a' * 40)
        head = admit(project, after, 'b' * 40)
        packet = self.impact(project, base, head, ['service'])
        self.assertGreater(packet['analysis_coverage']['blocking_component_issues'], 0)
        self.assertEqual(packet['analysis_coverage']['status'], 'incomplete')
        self.assertTrue(any(g.get('cause') == 'interface_dispatch'
                            for g in packet['coverage']['issue_histogram']['groups']))


if __name__ == '__main__':
    unittest.main()
