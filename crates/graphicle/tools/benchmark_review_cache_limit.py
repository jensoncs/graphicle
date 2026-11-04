"""Compare two cache-load policies using identical, newly primed review stores.

Cases contain name, seed_store (already admitting both previous and next heads),
previous_request and next_request. All stores and requests remain private. The
binary variants must differ only in their load policy, with identical Go helpers.
Priming and cloning are untimed. Existing full facts must hit only for the base
in normal-reuse; the new head must always miss. No models or source collection.
"""
import argparse
import json
import os
from pathlib import Path
import re
import sqlite3

from benchmark_pinned_review import canonical, review, save, sha
from benchmark_review_cache_reuse import cache_counts, clone_store, environment


def native_phases(log):
    totals = {}
    for name, ns in re.findall(r'^go_phase=([a-z_]+) ns=(\d+)$', log, re.MULTILINE):
        totals[name] = totals.get(name, 0) + int(ns)
    return totals


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--cases', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    binaries = {'32mib': args.baseline.resolve(), '64mib': args.candidate.resolve()}
    cases = json.loads(args.cases.read_text())
    assert len({c['name'] for c in cases}) == len(cases)
    assert all(re.fullmatch(r'[A-Za-z0-9_-]+', c['name']) for c in cases)
    config = root / 'models.json'
    save(config, {'models': {}})
    result = {'binary_sha256': {k: sha(v) for k, v in binaries.items()},
              'cases': {}, 'rows': [], 'complete': False}
    for case in cases:
        name = case['name']
        folder = root / name
        previous = json.loads(Path(case['previous_request']).read_text())
        request = json.loads(Path(case['next_request']).read_text())
        assert len(request['head']) == len(previous['head']) == len(request['base']) == 1
        assert previous['base'] == request['base']
        assert previous['head'][0]['snapshot'] != request['head'][0]['snapshot']
        assert previous['profile'] == request['profile']
        clone_store(case['seed_store'], folder / 'seed' / 'store')
        with sqlite3.connect(folder / 'seed/store/reviews.sqlite3') as db:
            db.execute('PRAGMA foreign_keys=ON')
            db.execute('DELETE FROM repo_review_analysis')
            db.execute('DELETE FROM repo_review_package_facts')
        save(folder / 'previous.json', previous)
        save(folder / 'request.json', request)
        # Both variants use the same newly instrumented helper identity.
        _, stats = review(binaries['32mib'], folder / 'seed', folder / 'previous.json',
                          'prime', config, environment(folder / 'seed'))
        assert not stats['analysis_cache']['base_hit'] and not stats['analysis_cache']['head_hit']
        counts = cache_counts(folder / 'seed/store')
        assert counts['full_analyses'] == 2 and counts['package_entries'] > 0
        result['cases'][name] = {'base': request['base'], 'previous_head': previous['head'],
                                'head': request['head'], 'profile': request['profile'],
                                'max_tokens': request['max_tokens'], 'seed_cache': counts,
                                'request_sha256': sha(folder / 'request.json')}
        print(name, 'primed', json.dumps(counts), flush=True)
    # ABBA for each cache state, including cold controls to detect overhead.
    plan = [(policy, mode) for policy in ['32mib', '64mib', '64mib', '32mib']
            for mode in ['normal-reuse', 'packages-only']]
    plan += [('32mib', 'cold'), ('64mib', 'cold')]
    for index, (policy, mode) in enumerate(plan):
        for case in cases:
            folder = root / case['name']
            work = folder / f'{index}-{policy}-{mode}'
            clone_store(folder / 'seed/store', work / 'store')
            with sqlite3.connect(work / 'store/reviews.sqlite3') as db:
                db.execute('PRAGMA foreign_keys=ON')
                if mode != 'normal-reuse':
                    db.execute('DELETE FROM repo_review_analysis')
                if mode == 'cold':
                    db.execute('DELETE FROM repo_review_package_facts')
    reference = {}
    for index, (policy, mode) in enumerate(plan):
        for case in cases:
            name = case['name']
            folder = root / name
            work = folder / f'{index}-{policy}-{mode}'
            packet, stats = review(binaries[policy], work, folder / 'request.json',
                                   'review', config, environment(work))
            cache = stats['analysis_cache']
            assert cache['base_hit'] == (mode == 'normal-reuse') and not cache['head_hit']
            assert packet['head'] == result['cases'][name]['head']
            assert packet['tokens'] <= result['cases'][name]['max_tokens']
            compared = canonical(packet)
            reference.setdefault(name, compared)
            phases = native_phases((work / 'review.log').read_text())
            assert phases.get('helper', 0) > 0 and phases.get('type_check', 0) > 0
            row = {'case': name, 'policy': policy, 'mode': mode, 'order': index,
                   **stats, 'native_phase_ns': phases, 'tokens': packet['tokens'],
                   'packet_equal': compared == reference[name]}
            result['rows'].append(row)
            save(root / 'summary.json', result)
            print(name, policy, mode, json.dumps(row), flush=True)
            assert row['packet_equal'], 'Semantic packet drift'
    result['complete'] = True
    save(root / 'summary.json', result)


if __name__ == '__main__':
    main()
