"""Package-cache diagnostics survive the helper boundary without changing packets."""
import json
import os
from pathlib import Path
import re
import sqlite3
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'crates/graphicle/tools'))
from benchmark_pinned_review import canonical
from benchmark_review_cache_reuse import environment
from benchmark_review_soak import LoggedServer, batch_log
from investigate_review_coverage import MOD, PROFILE, admit

BINARY = Path(os.environ.get('GRAPHICLE_BIN', ROOT / 'crates/graphicle/target/debug/graphicle')).resolve()


def sources(value):
    return {'go.mod': MOD + '\nrequire example.test/dep v1.0.0\n',
            'vendor/modules.txt': '# example.test/dep v1.0.0\n## explicit; go 1.23\nexample.test/dep\n',
            'vendor/example.test/dep/dep.go': 'package dep\nfunc Double(n int) int { return n*2 }\n',
            'main.go': 'package app\nimport "example.test/dep"\n'
                       f'func Changed() int {{ return dep.Double({value}) }}\n'}


def events(log, phase):
    return [json.loads(line.split(' stats=', 1)[1]) for line in log.splitlines()
            if line.startswith(f'phase=review_package_cache_{phase} ')]


@unittest.skipUnless(BINARY.is_file(), 'requires a built Graphicle')
class PackageCacheDiagnostics(unittest.TestCase):
    def test_cold_warm_configuration_change_and_fresh_packet_equality(self):
        with tempfile.TemporaryDirectory(prefix='graphicle-cache-diagnostics-') as tmp:
            work = Path(tmp)
            with LoggedServer(BINARY, work / 'store', env=environment(work),
                              serve_args=['--no-worker'], timeout=150) as client:
                project = client.project('cache-diagnostics')
                base = admit(project, sources(1), 'a' * 40)
                head = admit(project, sources(2), 'b' * 40)

                def review(pin, profile=PROFILE):
                    offset = (work / 'server.log').stat().st_size
                    packet = project.impact([base], [pin], profile=profile, max_tokens=16000)
                    log = batch_log(work, offset, 1)
                    keys = dict(re.findall(r'phase=review_cache_side ms=0 side=(base|head) key=([0-9a-f]{64})', log))
                    for phase in ('load', 'absorb', 'save'):
                        for key in re.findall(rf'phase=review_package_cache_{phase} .*?key=([0-9a-f]{{64}})', log):
                            self.assertIn(key, keys.values())
                    for native in events(log, 'native'):
                        self.assertIn(native['key'], keys.values())
                        self.assertLess(len(json.dumps(native)), 8192)
                    self.assertLessEqual(packet['tokens'], 16000)
                    return packet, log

                cold, log = review(head)
                self.assertEqual(events(log, 'load')[0]['loaded_entries'], 0)
                native = events(log, 'native')
                self.assertEqual(len(native), 2)
                self.assertEqual(native[0]['identity'], native[1]['identity'])
                self.assertEqual(native[0]['configuration_sha256'], native[1]['configuration_sha256'])
                self.assertEqual(native[0]['counts']['miss_not_present'], 1)
                self.assertEqual(native[1]['counts']['hit_dependency'], 1)
                self.assertEqual(events(log, 'absorb')[0]['accepted_entries'], 1)
                writes = events(log, 'save')
                self.assertEqual(writes[0]['written_entries'], 1)
                self.assertEqual(writes[1]['refreshed_entries'], 1)
                self.assertEqual(writes[1]['retained_entries'], 1)
                self.assertGreater(writes[1]['retained_bytes'], 0)
                self.assertTrue(all(w['status'] == 'pending_commit' and w['evicted_entries'] == 0 for w in writes))
                self.assertIn('phase=review_cache_commit ', log)

                warm, log = review(head)
                self.assertEqual(canonical(cold), canonical(warm))
                self.assertTrue(warm['analysis_cache']['base_hit'] and warm['analysis_cache']['head_hit'])
                self.assertEqual(events(log, 'load'), [])
                self.assertEqual(events(log, 'native'), [])

                next_head = admit(project, sources(3), 'c' * 40)
                reused, log = review(next_head)
                self.assertTrue(reused['analysis_cache']['base_hit'])
                self.assertFalse(reused['analysis_cache']['head_hit'])
                loaded = events(log, 'load')[0]
                self.assertEqual(loaded['stored_entries'], 1)
                self.assertEqual(loaded['loaded_entries'], 1)
                self.assertEqual(loaded['loaded_bytes'], loaded['stored_bytes'])
                next_native = events(log, 'native')
                self.assertEqual(len(next_native), 1)
                self.assertEqual(next_native[0]['identity'], native[1]['identity'])
                self.assertEqual(next_native[0]['counts']['hit_dependency'], 1)

                # An optional cache cannot change the semantic answer.
                with sqlite3.connect(work / 'store/reviews.sqlite3') as db:
                    db.execute('PRAGMA foreign_keys=ON')
                    db.execute('DELETE FROM repo_review_analysis')
                    db.execute('DELETE FROM repo_review_package_facts')
                fresh, _ = review(next_head)
                self.assertEqual(canonical(reused), canonical(fresh))

                changed_profile, log = review(next_head, {**PROFILE, 'tags': ['probe']})
                different = events(log, 'native')[0]
                self.assertNotEqual(different['identity'], native[0]['identity'])
                self.assertEqual(different['configuration_sha256'], native[0]['configuration_sha256'])
                self.assertFalse(changed_profile['analysis_cache']['base_hit'])

                config_head = admit(project, {**sources(3), 'go.sum': '\n'}, 'd' * 40)
                _, log = review(config_head)
                different = events(log, 'native')[-1]
                self.assertNotEqual(different['configuration_sha256'], native[1]['configuration_sha256'])
                self.assertNotEqual(different['identity'], native[1]['identity'])
                self.assertEqual(different['counts']['miss_not_present'], 1)
                # Missing source still refuses whole-side caching, with the
                # actual native category rather than an opaque "other" bucket.
                missing = sources(4)
                missing['main.go'] = ('package app\nimport "example.test/missing"\n'
                                      'func Changed() int { return missing.Value() }\n')
                missing_head = admit(project, missing, 'e' * 40)
                incomplete, log = review(missing_head)
                self.assertEqual(incomplete['impact'], 'incomplete')
                self.assertFalse(incomplete['analysis_cache']['head_hit'])
                self.assertIn('status=non_cacheable_world_issue', log)
                self.assertIn('"unavailable_external_package":', log)
                self.assertNotIn('"other":', log)
                self.assertEqual(client.usage()['model_usage']['calls'], 0)


if __name__ == '__main__':
    unittest.main()
