"""Measure native Go interface-receiver gaps and their changed-component coverage.

Builds the current native helper offline and type-checks synthetic source only;
reviewed code is never executed. --binary optionally measures pinned packets too.
Each fixture changes Concrete.Get so its receiver sites are in the changed package.
The classifications below are investigation hypotheses, not new resolver rules.
"""
import argparse
import base64
from collections import Counter
import gzip
import json
import os
from pathlib import Path
import shutil
import subprocess

from investigate_review_coverage import ROOT, MOD, PROFILE, LocalServer, admit, digest

COMMON = '''package app
type Value interface { Get() int }
type Concrete struct{}
func (Concrete) Get() int { return VALUE }
type Other struct{}
func (Other) Get() int { return 9 }
type Holder struct { value Value }
'''


def fixtures():
    # Group, name, declarations, Use body. The marker int is changed on both sides.
    yield 'control', 'concrete_receiver', '', 'return (Concrete{}).Get()'
    yield 'control', 'interface_conversion', '', 'return Value(Concrete{}).Get()'
    yield 'control', 'single_local', '', 'var v Value = Concrete{}; return v.Get()'
    yield 'control', 'concrete_return', 'func New() Concrete { return Concrete{} }', 'return New().Get()'
    yield 'control', 'concrete_field', 'type Fixed struct { value Concrete }', 'h := Fixed{}; return h.value.Get()'
    yield 'constructor', 'single_return', 'func New() Value { return Concrete{} }', 'return New().Get()'
    yield 'constructor', 'local_result', 'func New() Value { return Concrete{} }', 'v := New(); return v.Get()'
    yield 'constructor', 'pointer_return', 'func New() Value { return &Concrete{} }', 'return New().Get()'
    yield 'constructor', 'same_type_branches', 'func New(b bool) Value { if b { return Concrete{} }; return Concrete{} }', 'return New(true).Get()'
    yield 'constructor', 'forwarding_return', 'func Inner() Value { return Concrete{} }; func New() Value { return Inner() }', 'return New().Get()'
    yield 'counterexample', 'mixed_return_types', 'func New(b bool) Value { if b { return Concrete{} }; return Other{} }', 'return New(flag).Get()'
    yield 'counterexample', 'named_result_defer', 'func New() (v Value) { defer func() { v = Other{} }(); return Concrete{} }', 'return New().Get()'
    yield 'counterexample', 'parameter_passthrough', 'func New(v Value) Value { return v }', 'return New(input).Get()'
    yield 'counterexample', 'recursive_factory', 'func New(b bool) Value { if b { return Concrete{} }; return New(b) }', 'return New(flag).Get()'
    yield 'field', 'immediate_literal', '', 'return (Holder{value: Concrete{}}).value.Get()'
    yield 'field', 'keyed_local', '', 'h := Holder{value: Concrete{}}; return h.value.Get()'
    yield 'field', 'positional_local', '', 'h := Holder{Concrete{}}; return h.value.Get()'
    yield 'field', 'pointer_local', '', 'h := &Holder{value: Concrete{}}; return h.value.Get()'
    yield 'counterexample', 'field_reassigned', '', 'h := Holder{value: Concrete{}}; if flag { h.value = input }; return h.value.Get()'
    yield 'counterexample', 'pointer_alias_mutation', '', 'h := &Holder{value: Concrete{}}; alias := h; alias.value = input; return h.value.Get()'
    yield 'counterexample', 'implicit_address_receiver', 'func (h *Holder) Reset(v Value) { h.value = v }', 'h := Holder{value: Concrete{}}; h.Reset(input); return h.value.Get()'
    yield 'counterexample', 'holder_escape', 'func Replace(h *Holder, v Value) { h.value = v }', 'h := Holder{value: Concrete{}}; Replace(&h, input); return h.value.Get()'
    yield 'counterexample', 'closure_mutation', '', 'h := Holder{value: Concrete{}}; func() { h.value = input }(); return h.value.Get()'
    yield 'counterexample', 'zero_interface_field', '', 'var h Holder; return h.value.Get()'
    yield 'counterexample', 'local_reassigned', '', 'var v Value = Concrete{}; if flag { v = input }; return v.Get()'
    yield 'counterexample', 'local_address_escape', 'func Replace(v *Value, other Value) { *v = other }', 'var v Value = Concrete{}; Replace(&v, input); return v.Get()'
    yield 'field', 'promoted_interface', 'type Embedded struct { Value }', 'h := Embedded{Value: Concrete{}}; return h.Get()'


def source(declarations, body, value):
    return COMMON.replace('VALUE', str(value)) + declarations + '\nfunc Use(input Value, flag bool) int { ' + body + ' }\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--expect-resolved', action='store_true',
                        help='Require the proved constructor/field cases to resolve; unsafe cases must stay unresolved')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    output = args.output.resolve()
    helper = output / 'go-helper'
    native_dir = ROOT / 'crates/graphicle-graph/src/extractors'
    sources = ['go_types.go', 'go_review.go', 'go_semantic.go', 'go_review_cache.go']
    # Match the production helper's isolated, bundled build layout.
    build_dir = output / 'helper-source'
    build_dir.mkdir()
    for name in [*sources, 'go.mod', 'go.sum']:
        shutil.copyfile(native_dir / name, build_dir / name)
    shutil.copytree(native_dir / 'vendor', build_dir / 'vendor')
    env = {**os.environ, 'GOENV': 'off', 'GOFLAGS': '', 'GOWORK': 'off',
           'GO111MODULE': 'on', 'GOTOOLCHAIN': 'local', 'GOPROXY': 'off', 'GOSUMDB': 'off',
           'GOVCS': '*:off', 'GOPRIVATE': '', 'GONOPROXY': '', 'GONOSUMDB': '',
           'GOINSECURE': '', 'GOEXPERIMENT': '', 'CGO_ENABLED': '0',
           'GOCACHE': str(output / 'build-cache'), 'GOMODCACHE': str(output / 'module-cache')}
    subprocess.run(['go', 'build', '-mod=vendor', '-o', str(helper), *sources],
                   cwd=build_dir, env=env, check=True, timeout=120)
    result = {'format': 'graphicle.interface-receiver-investigation.v1',
              'source_revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'native_source_sha256': {p: digest(native_dir / p) for p in sources},
              'native_helper_sha256': digest(helper), 'tool_sha256': digest(Path(__file__)),
              'go_version': subprocess.check_output(['go', 'version'], text=True).strip(),
              'profile': PROFILE, 'expect_resolved': args.expect_resolved, 'synthetic': []}
    for group, name, declarations, body in fixtures():
        root = output / name
        (root / 'app').mkdir(parents=True)
        (root / 'app/go.mod').write_text(MOD)
        text = source(declarations, body, 2)
        (root / 'app/main.go').write_text(text)
        request = {'review': {'roots': [{'directory': 'app', 'module': '.'}], **PROFILE}}
        process = subprocess.run([str(helper)], input=json.dumps(request), text=True,
                                 cwd=root, env=env, capture_output=True, check=True, timeout=120)
        envelope = json.loads(process.stdout)
        assert envelope['encoding'] == 'gzip-base64-v1'
        facts = json.loads(gzip.decompress(base64.b64decode(envelope['facts'], validate=True)))
        (root / 'native.json').write_text(json.dumps(facts, indent=2) + '\n')
        world, = facts['worlds']
        assert not world['issues'], (name, world['issues'])
        assert world['diagnostic_count'] == 0, (name, world['diagnostics'])
        sites = [c for c in world['calls'] if c['relation'] == 'calls' and
                 '.Use|' in c['caller'] and c['spelling'].endswith('.Get')]
        site, = sites
        candidates = [c for c in world['calls'] if c['relation'] == 'may_call' and
                      c['file'] == site['file'] and c['start'] == site['start'] and c['end'] == site['end']]
        if group == 'control' or (args.expect_resolved and group != 'counterexample'):
            assert not site.get('reason') and 'Concrete).Get' in site['target'], (name, site)
        # Unknown input and named-return mutations must not become a false exact target.
        if group == 'counterexample':
            assert site.get('reason'), (name, site)
        result['synthetic'].append({'name': name, 'group': group, 'source': text,
                                    'receiver_call': site, 'candidate_targets': [c['target'] for c in candidates],
                                    'unresolved_by_cause': dict(Counter(c['reason'] for c in world['calls'] if c.get('reason')))})
        print(name, site.get('reason', 'resolved'), len(candidates), flush=True)
    if args.binary:
        result['packet_binary_sha256'] = digest(args.binary)
        with LocalServer(args.binary.resolve(), output / 'store', serve_args=['--no-worker']) as client:
            client.timeout = 150
            for row, (_, name, declarations, body) in zip(result['synthetic'], fixtures()):
                project = client.project('receiver-' + name)
                base = admit(project, {'go.mod': MOD, 'main.go': source(declarations, body, 1)}, 'a' * 40)
                head = admit(project, {'go.mod': MOD, 'main.go': source(declarations, body, 2)}, 'b' * 40)
                packet = project.impact([base], [head], profile=PROFILE, max_tokens=32000)
                (output / name / 'packet.json').write_text(json.dumps(packet, indent=2) + '\n')
                row['packet'] = {k: packet[k] for k in ['impact', 'risk_schema', 'analysis_coverage']}
                row['packet']['issue_histogram'] = packet['coverage']['issue_histogram']
                resolved = row['group'] == 'control' or (args.expect_resolved and row['group'] != 'counterexample')
                expected = 'complete' if resolved else 'incomplete'
                # This captures the current baseline; update when implementing a proved improvement.
                assert packet['analysis_coverage']['status'] == expected, (name, packet['analysis_coverage'])
                print(name, 'scoped', packet['analysis_coverage']['status'], flush=True)
            assert client.usage()['model_usage']['calls'] == 0
    (output / 'summary.json').write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
