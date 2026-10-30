"""Unpaid local RCA acceptance: native selection, budget, reuse and cache timings.

Run with --binary BUILT_GRAPHICLE --output NEW_DIRECTORY. Never calls a model.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'sdk/python'))
from graphicle_sdk import LocalServer, apply_review_delta


def sha(value):
    return hashlib.sha256(value if isinstance(value, bytes) else value.encode()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    evidence = {'binary_sha256': sha(args.binary.read_bytes()), 'model_calls': 0,
                'scope': 'local deterministic acceptance, not reviewer quality or billed cost', 'cases': []}
    with tempfile.TemporaryDirectory(prefix='review-rca-') as tmp:
        root = Path(tmp).resolve()
        repo = root / 'repo'
        repo.mkdir()
        env = {'PATH': os.environ['PATH'], 'HOME': str(root), 'TMPDIR': str(root),
               'GOPROXY': 'off', 'GOSUMDB': 'off', 'GOTOOLCHAIN': 'local', 'CGO_ENABLED': '0'}

        def git(*words):
            return subprocess.check_output(['git', '-C', str(repo), '-c', 'user.name=Fixture',
                '-c', 'user.email=fixture@example.invalid', '-c', 'core.hooksPath=/dev/null',
                '-c', 'commit.gpgsign=false', *words], env=env, stderr=subprocess.DEVNULL).decode().strip()

        git('init', '-q')
        originals = {'go.mod': 'module example/rca\ngo 1.23\n',
                     'main.go': 'package rca\n' + ''.join(f'func F{i}() int {{ return {i} }}\n' for i in range(20)),
                     'only_test.go': 'package rca\nfunc Check() int { return 1 }\n',
                     'README.md': 'Fixture documentation v1\n'}
        config = root / 'models.json'
        config.write_text('{"models":{}}')
        with LocalServer(args.binary, root / 'store', config, env=env, timeout=180) as client:
            project = client.project('rca')

            def admit(files, label):
                for path, body in files.items():
                    (repo / path).write_text(body)
                git('add', *sorted(files))
                git('commit', '--allow-empty', '-qm', label + '\n\nCo-authored-by: Codex <noreply@openai.com>')
                commit, policy = git('rev-parse', 'HEAD'), sha('complete local fixture')
                begin = project.command('sync.prepare', expected_snapshot=project.status()['repository']['snapshot'],
                    metadata={'commit': commit, 'policy_sha256': policy, 'scope_complete': True},
                    manifest=[{'path': p, 'sha256': sha(c), 'bytes': len(c.encode())} for p, c in files.items()])
                if begin.get('sync_id'):
                    project.command('sync.upload', sync_id=begin['sync_id'], files=[{'path': p, 'content': c} for p, c in files.items()])
                    project.command('sync.commit', sync_id=begin['sync_id'])
                return {'repository_id': project.id, 'directory': 'fixture', 'commit': commit,
                        'snapshot': begin['snapshot'], 'policy_sha256': policy}

            base = admit(originals, 'fixture base')
            production = {**originals, 'main.go': originals['main.go'].replace('return 0', 'return 100')}
            prod = admit(production, 'production change')
            excluded = admit({**originals, 'only_test.go': originals['only_test.go'].replace('return 1', 'return 2')}, 'test change')
            docs = admit({**originals, 'README.md': 'Fixture documentation v2\n'}, 'documentation change')
            profile = {'goos': 'linux', 'goarch': 'amd64', 'tests': False}

            def request(name, head, **options):
                started = time.monotonic()
                packet = project.impact([base], [head], profile=options.pop('profile', profile), **options)
                elapsed = time.monotonic() - started
                (args.output / (name + '.json')).write_text(json.dumps(packet, indent=2) + '\n')
                row = {'case': name, 'seconds': elapsed, 'required_entries': packet['coverage']['required_entries'],
                       'head_entries': sum(e['side'] == 'head' for e in packet['entries']),
                       'base_entries': sum(e['side'] == 'base' for e in packet['entries']),
                       'wire_text_bytes': sum(len(e.get('text', '').encode()) for e in packet['entries']),
                       'complete': packet['coverage']['complete'], 'analysis_cache': packet['analysis_cache']}
                evidence['cases'].append(row)
                return packet

            cold = request('production-cold', prod, max_tokens=64000)
            warm = request('production-warm', prod, max_tokens=64000)
            assert cold['packet_id'] == warm['packet_id']
            assert warm['analysis_cache']['base_hit'] and warm['analysis_cache']['head_hit']
            assert cold['coverage']['required_entries'] == 2
            assert request('excluded-test', excluded, max_tokens=64000)['coverage']['required_entries'] == 0
            assert request('included-test', excluded, profile={**profile, 'tests': True}, max_tokens=64000)['coverage']['required_entries'] == 2
            assert request('documentation', docs, max_tokens=64000)['coverage']['required_entries'] == 0
            tight = request('production-budget', prod, max_tokens=4000)
            assert tight['coverage']['current_head']['usable'] and any(e['side'] == 'head' for e in tight['entries'])
            all_changed = {**originals, 'main.go': 'package rca\n' + ''.join(f'func F{i}() int {{ return {i + 100} }}\n' for i in range(20))}
            first_pin = admit(all_changed, 'all declarations changed')
            first = request('all-first', first_pin, max_tokens=64000, reuse_text=True)
            next_pin = admit(all_changed, 'identical tree')
            delta = request('all-delta', next_pin, max_tokens=64000, reuse_text=True, previous_packet_id=first['packet_id'])
            heads = [e for e in delta['entries'] if e['side'] == 'head']
            assert len(heads) == 20 and all(e.get('text_ref') is True and 'text' not in e for e in heads)
            view = apply_review_delta(apply_review_delta(None, first), delta)
            full = request('offline-contract-full', next_pin, max_tokens=64000, reuse_text=True)
            assert view['context_order'] == full['context_order']
            assert {e['id']: e['text'] for e in view['entries']} == {e['id']: e['text'] for e in full['entries']}
            assert client.usage()['model_usage']['calls'] == 0
    evidence['state'] = 'passed'
    (args.output / 'summary.json').write_text(json.dumps(evidence, indent=2) + '\n')
    print(json.dumps(evidence, indent=2))


if __name__ == '__main__':
    main()
