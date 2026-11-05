"""Reproduce G4 coverage gates on isolated admitted snapshots, without models.

Use --binary PATH --output NEW_DIRECTORY. Optional --archive LABEL=PACKET_JSON
summarizes previously captured packets without opening or changing their stores.
Pair --archive-store LABEL=STORE with --archive to inspect retained compiler facts
for exactly the packet snapshots (missing/ambiguous cache matches are reported).
This is a diagnostic matrix, not a benchmark or a risk-verdict policy change.
"""
import argparse
import base64
from collections import Counter
from contextlib import closing
import gzip
import io
import hashlib
import json
from pathlib import Path
import sqlite3
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'sdk/python'))
from graphicle_sdk import LocalServer

PROFILE = {'goos': 'linux', 'goarch': 'amd64', 'tests': False}
POLICY = hashlib.sha256(b'G4 complete synthetic source admission v1').hexdigest()
MOD = 'module example.test/g4\n\ngo 1.23\n'
INTERFACE = '''type Value interface { Get() int }
type Concrete struct{}
func (Concrete) Get() int { return 7 }
'''


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def fixtures():
    simple = {'go.mod': MOD, 'main.go': 'package app\nfunc Changed() int { return VALUE }\n'}
    yield 'plain', simple, True
    yield 'identical_plain', simple, False
    yield 'unused_json', {**simple, 'tooling/settings.json': '{"editor": "default"}\n'}, True
    unused = {**simple, 'unused/dispatch.go': 'package unused\n' + INTERFACE +
              'func Dispatch(v Value) int { return v.Get() }\n'}
    yield 'unrelated_interface_package', unused, True
    yield 'identical_with_interface', unused, False
    yield 'unrelated_broken_package', {**simple, 'unused/broken.go':
                                      'package unused\nfunc Broken() int { return Missing }\n'}, True
    for name, call in [('concrete_receiver', '(Concrete{}).Get()'),
                       ('explicit_interface_conversion', 'Value(Concrete{}).Get()')]:
        yield name, {'go.mod': MOD, 'main.go': 'package app\n' + INTERFACE.replace('return 7', 'return VALUE') +
                     f'func Use() int {{ return {call} }}\n'}, True
    for name, declaration, body in [
        ('interface_field_literal', 'type Holder struct { value Value }\n',
         'h := Holder{value: Concrete{}}; return h.value.Get()'),
        ('interface_constructor_result', 'func New() Value { return Concrete{} }\n',
         'return New().Get()')]:
        yield name, {'go.mod': MOD, 'main.go': 'package app\n' +
              INTERFACE.replace('return 7', 'return VALUE') + declaration +
              f'func Use() int {{ {body} }}\n'}, True
    yield 'unrelated_stdlib_api', {**simple, 'unused/library.go':
          'package unused\nimport "strings"\nfunc Trim(s string) string { return strings.TrimSpace(s) }\n'}, True
    yield 'local_interface_binding', {'go.mod': MOD, 'main.go': 'package app\n' +
          INTERFACE.replace('return 7', 'return VALUE') +
          'func Use() int { var v Value = Concrete{}; return v.Get() }\n'}, True
    yield 'stdlib_api', {'go.mod': MOD, 'main.go':
          'package app\nimport "strings"\nfunc Changed(s string) string { return strings.TrimSpace(s) + "VALUE" }\n'}, True
    yield 'vendored_pure_api', {'go.mod': MOD + '\nrequire example.test/dep v1.0.0\n',
          'vendor/modules.txt': '# example.test/dep v1.0.0\n## explicit; go 1.23\nexample.test/dep\n',
          'vendor/example.test/dep/dep.go': 'package dep\nfunc Double(n int) int { return n * 2 }\n',
          'main.go': 'package app\nimport "example.test/dep"\nfunc Changed() int { return dep.Double(VALUE) }\n'}, True
    # Enough resolved callers to force delivery omissions at a small budget.
    yield 'packing_only', {**simple, 'callers.go': 'package app\n' + '\n'.join(
        f'func Caller{i}() int {{ return Changed() + {i} }}' for i in range(12))}, True


def admit(project, files, commit, *, scope_complete=True):
    prepared = project.command('sync.prepare',
        expected_snapshot=project.status(compact=True)['repository']['snapshot'],
        metadata={'commit': commit, 'policy_sha256': POLICY, 'scope_complete': scope_complete},
        manifest=[{'path': path, 'sha256': hashlib.sha256(text.encode()).hexdigest(), 'bytes': len(text.encode())}
                  for path, text in files.items()])
    if prepared['sync_id']:
        project.command('sync.upload', sync_id=prepared['sync_id'],
                        files=[{'path': path, 'content': files[path]} for path in prepared['missing']])
        project.command('sync.commit', sync_id=prepared['sync_id'], compact=True)
    return {'repository_id': project.id, 'directory': 'app', 'commit': commit,
            'snapshot': prepared['snapshot'], 'policy_sha256': POLICY}


def summarize(packet):
    coverage = packet['coverage']
    return {'impact': packet['impact'], 'risk_schema': packet['risk_schema'],
            'analysis_coverage': packet.get('analysis_coverage'),
            'coverage_complete': coverage['complete'], 'issue_count': coverage['issue_count'],
            'issue_histogram': coverage['issue_histogram'], 'budget_omitted': coverage['budget_omitted'],
            'entries': len(packet['entries']),
            'changes': packet['changes']['count'],
            'head_review_basis': {
                'complete': sum(e['review_basis']['complete'] for e in packet['entries'] if e['side'] == 'head'),
                'reasons': dict(Counter(e['review_basis']['reason'] for e in packet['entries'] if e['side'] == 'head'))},
            'analysis_diagnostics': coverage.get('analysis_diagnostics'),
            'source_admission': coverage.get('source_admission'), 'tokens': packet['tokens']}


def native_archive(store, packet):
    """Read only exact-pinned cached facts; never substitute a newer cache row."""
    result = {}
    with closing(sqlite3.connect((Path(store).resolve() / 'reviews.sqlite3').as_uri() + '?mode=ro', uri=True)) as db:
        for side in ['base', 'head']:
            pins = packet[side]
            if len(pins) != 1:
                result[side] = {'status': 'multi_repository_capture_not_supported'}
                continue
            pin = pins[0]
            rows = db.execute('SELECT DISTINCT a.id,a.facts FROM repo_review_analysis a '
                'JOIN repo_review_analysis_inputs i ON i.repository_id=a.repository_id AND i.id=a.id '
                'WHERE a.repository_id=? AND i.input_repository=? AND i.snapshot=?',
                (pin['repository_id'], pin['repository_id'], pin['snapshot'])).fetchall()
            if len(rows) != 1:
                result[side] = {'status': 'missing_or_ambiguous_cache', 'matches': len(rows)}
                continue
            key, encoded = rows[0]
            encoded = json.loads(encoded)
            if encoded['encoding'] != 'gzip-base64-v1':
                raise ValueError('Unsupported native fact encoding')
            with gzip.GzipFile(fileobj=io.BytesIO(base64.b64decode(encoded['facts'], validate=True))) as stream:
                raw = stream.read(64 * 1024 * 1024 + 1)
            if len(raw) > 64 * 1024 * 1024:
                raise ValueError('Native facts exceed 64 MiB')
            facts = json.loads(raw)
            calls = [(world['root'], call) for world in facts['worlds'] for call in world['calls']]
            interfaces = [(root, call) for root, call in calls if call.get('reason') == 'interface_dispatch']
            candidates = Counter((root, c['file'], c['start'], c['end']) for root, c in calls
                                 if c.get('evidence') == 'interface_implementation')
            result[side] = {'status': 'available', 'snapshot': pin['snapshot'], 'cache_key': key,
                'facts_sha256': hashlib.sha256(raw).hexdigest(),
                'unresolved_by_cause': dict(sorted(Counter(c['reason'] for _, c in calls if c.get('reason')).items())),
                'interface_sites': len(interfaces),
                'interface_sites_with_candidates': sum(bool(candidates[(r,c['file'],c['start'],c['end'])]) for r,c in interfaces),
                'interface_candidate_edges': sum(candidates.values()),
                'top_interface_spellings': Counter(c['spelling'] for _, c in interfaces).most_common(15),
                'top_interface_files': Counter(c['file'] for _, c in interfaces).most_common(10)}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--archive', action='append', default=[])
    parser.add_argument('--archive-store', action='append', default=[])
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    result = {'format': 'graphicle.g4-investigation.v1',
              'source_revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'binary_sha256': digest(args.binary), 'tool_sha256': digest(Path(__file__)), 'profile': PROFILE, 'synthetic': [], 'archives': []}
    archive_stores = dict(value.split('=', 1) for value in args.archive_store)
    for value in args.archive:
        label, path = value.split('=', 1)
        packet = json.loads(Path(path).read_text())
        result['archives'].append({'label': label, 'packet_sha256': digest(path),
                                   'base': packet['base'], 'head': packet['head'],
                                   'analyzer': packet.get('analyzer'), **summarize(packet)})
        if label in archive_stores:
            result['archives'][-1]['native_facts'] = native_archive(archive_stores[label], packet)
    with LocalServer(args.binary.resolve(), args.output / 'store', serve_args=['--no-worker']) as client:
        client.timeout = 150
        for name, template, changed in fixtures():
            project = client.project('g4-' + name)
            base = admit(project, {p: t.replace('VALUE', '1') for p, t in template.items()}, 'a' * 40)
            head = admit(project, {p: t.replace('VALUE', '2' if changed else '1') for p, t in template.items()}, 'b' * 40)
            for budget in ([4000, 64000] if name == 'packing_only' else [16000]):
                packet = project.impact([base], [head], profile=PROFILE, max_tokens=budget, timeout_ms=120000)
                (args.output / f'{name}-{budget}.json').write_text(json.dumps(packet, indent=2) + '\n')
                result['synthetic'].append({'name': name, 'changed': changed, 'max_tokens': budget,
                                            'files': sorted(template), **summarize(packet)})
                print(name, budget, packet['impact'], packet['coverage']['issue_count'],
                      packet['coverage']['budget_omitted'], flush=True)
        if client.usage()['model_usage']['calls'] != 0:
            raise AssertionError('Diagnostic matrix must not invoke models')
    (args.output / 'summary.json').write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
