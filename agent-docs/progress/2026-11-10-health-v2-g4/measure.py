#!/usr/bin/env python3
"""Measure the real repo-health checker on the generic fixture matrix (G4, unit measure).

Usage: python3 -I agent-docs/progress/2026-11-10-health-v2-g4/measure.py --out OUT.json

Builds every kind x variant of scripts/tests/repo_health_fixtures.py under a temporary directory,
runs the checker serially up to --reps times per repo, and writes precision, false_positives,
latency and cost.  Only the temporary directory and OUT are written; no model or network is used.
"""
from __future__ import annotations

import argparse
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[3]
TOOL_DIR = ROOT / 'scripts/dev/repo-health'
TOOL = TOOL_DIR / 'repo_health.py'
VALIDATOR = TOOL_DIR / 'validate_report.py'
sys.path[:0] = [str(ROOT / 'scripts/tests'), str(TOOL_DIR)]

from health.engine import is_fail_grade  # noqa: E402
from repo_health_fixtures import KINDS, VARIANTS, WEAKENINGS, build, git, violation_key  # noqa: E402

MAX_REPS = 5
CANDIDATE_VARIANTS = ('dup-contract', 'dup-state')
FAIL_VARIANTS = ('dep-inversion', *WEAKENINGS, 'history-smuggle')
CLEAN_VARIANTS = ('good', 'security-double', 'vendor-generated', 'docs-only')
# WU check 1 + review 1; delivery reuses the review's result (ADR health-generic-fixtures D4.3).
PATHS_PER_REVIEW = 2
LLM_OR_NETWORK = re.compile(
    r'\b(anthropic|openai)\b|^\s*(import|from)\s+(socket|urllib|http|requests|aiohttp)\b', re.I | re.M)


def fingerprint(rid: str, key: str) -> str:
    return rid + ':' + hashlib.sha256((rid + '\n' + key).encode()).hexdigest()[:16]


def label_fingerprint(label: str, language_kind: str) -> tuple[str, str]:
    """The (rule_id, contract fingerprint) a builder label names; deps labels carry only the edge."""
    rid, key = label.split(':', 1)
    return rid, fingerprint(rid, violation_key(language_kind) if rid == 'deps.violation' else key)


def repo_labels(kind: str, fixture) -> dict[str, list[str]]:
    """Expected labels per repo; multi concatenates the ts labels and then the rust labels."""
    if kind != 'multi':
        return {'.': list(fixture.expected)}
    half = len(fixture.expected) // 2
    return {'ts': list(fixture.expected[:half]), 'rust': list(fixture.expected[half:])}


def clean_env() -> dict[str, str]:
    return {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}


def run_checker(repo: Path, out: Path) -> tuple[int, float, dict | None]:
    out.unlink(missing_ok=True)
    started = time.monotonic()
    proc = subprocess.run([sys.executable, '-I', str(TOOL), '--repo', str(repo), '--base', 'main',
                           '--head', 'candidate', '--policy-version', '1', '--out', str(out)],
                          capture_output=True, text=True, env=clean_env(), cwd=repo)
    wall = time.monotonic() - started
    return proc.returncode, wall, json.loads(out.read_text()) if out.exists() else None


def signature(code: int, report: dict | None):
    if report is None:
        return code, None
    return code, tuple(sorted((f['fingerprint'], f['severity'], f['delta']) for f in report['findings']))


def measure_fixture(kind: str, variant: str, scratch: Path, reps: int, with_profile: bool) -> list[dict]:
    """One record per repo of the fixture; wall_secs is ordered by repetition."""
    name = f'{kind}-{variant}-{"profile" if with_profile else "builtin"}'
    root = scratch / name
    fixture = build(kind, variant, root / 'project', with_profile=with_profile)
    labels = repo_labels(kind, fixture)
    records = []
    for rel in fixture.repos:
        repo = root / 'project' / rel
        language = rel if kind == 'multi' else kind
        out = root / f'report-{rel.replace("/", "_").replace(".", "root")}.json'
        runs = [run_checker(repo, out) for _ in range(reps)]
        report = runs[0][2]
        valid = None
        if report is not None:
            # The last run's report is on disk and is the same as the first one when stable is true.
            valid = subprocess.run([sys.executable, '-I', str(VALIDATOR), str(out)],
                                   capture_output=True, text=True, env=clean_env()).returncode == 0
        changed = [] if variant == 'non-git' else git(repo, 'diff', '--name-only', 'main', 'candidate').splitlines()
        records.append({
            'kind': kind, 'variant': variant, 'repo': rel, 'language': language,
            'labels': labels.get(rel, []), 'changed': changed, 'report': report, 'report_valid': valid,
            'exit_codes': [r[0] for r in runs], 'wall_secs': [r[1] for r in runs],
            'stable': len({signature(r[0], r[2]) for r in runs}) == 1,
        })
    return records


def has_evidence(finding: dict, changed: list[str]) -> bool:
    return any(e.get('path') in changed for e in finding['evidence'])


def label_hit(record: dict, label: str) -> dict | None:
    if record['report'] is None:
        return None
    rid, fp = label_fingerprint(label, record['language'])
    for finding in record['report']['findings']:
        if finding['rule_id'] == rid and finding['fingerprint'] == fp:
            return finding
    return None


def candidate_detected(record: dict, finding: dict) -> bool:
    return finding['candidate'] is True and has_evidence(finding, record['changed'])


def candidate_as_expected(record: dict, finding: dict) -> bool:
    summary = record['report']['summary']
    return (finding['severity'] == 'warning' and finding['delta'] == 'new' and finding['exception'] is None
            and summary['verdict'] == 'pass' and record['exit_codes'][0] == 0)


def fail_detected(record: dict, finding: dict) -> bool:
    return is_fail_grade(finding) and has_evidence(finding, record['changed'])


def fail_as_expected(record: dict, finding: dict) -> bool:
    summary = record['report']['summary']
    return (finding['candidate'] is False and finding['delta'] == 'new' and summary['verdict'] == 'fail'
            and record['exit_codes'][0] == 1 and record['report']['policy']['source'] == 'base')


def ratio(numerator: int, denominator: int) -> float | None:
    return round(numerator / denominator, 4) if denominator else None


def tally(rows: list[tuple[str, str, bool, bool]]) -> dict:
    expected = len(rows)
    detected = sum(1 for r in rows if r[2])
    as_expected = sum(1 for r in rows if r[2] and r[3])
    return {'expected': expected, 'detected': detected, 'detection_rate': ratio(detected, expected),
            'as_expected': as_expected, 'as_expected_rate': ratio(as_expected, detected)}


def score(records: list[dict], variants: tuple[str, ...], detected, as_expected, emitted) -> dict:
    """Detection of the expected labels, the expected shape of what was detected, and what else was emitted."""
    rows, misses, unexpected = [], [], []
    emitted_count = 0
    for record in (r for r in records if r['variant'] in variants):
        wanted = set()
        for label in record['labels']:
            rid, fp = label_fingerprint(label, record['language'])
            wanted.add(fp)
            finding = label_hit(record, label)
            found = finding is not None and detected(record, finding)
            good = found and as_expected(record, finding)
            rows.append((record['kind'], record['variant'], found, good))
            if not good:
                misses.append({'kind': record['kind'], 'repo': record['repo'], 'variant': record['variant'],
                               'label': label, 'finding_present': finding is not None, 'detected': found})
        for finding in (record['report'] or {}).get('findings', []):
            if emitted(finding):
                emitted_count += 1
                if finding['fingerprint'] not in wanted:
                    unexpected.append(item(record, finding))
    summary = tally(rows)
    matched = emitted_count - len(unexpected)
    summary.update(emitted=emitted_count, matched_expected=matched, precision=ratio(matched, emitted_count),
                   by_kind={k: tally([r for r in rows if r[0] == k]) for k in KINDS},
                   by_variant={v: tally([r for r in rows if r[1] == v]) for v in variants},
                   misses=misses, unexpected=unexpected,
                   unexpected_by_rule=dict(Counter(u['rule_id'] for u in unexpected)))
    return summary


def item(record: dict, finding: dict) -> dict:
    return {'kind': record['kind'], 'repo': record['repo'], 'variant': record['variant'],
            'rule_id': finding['rule_id'], 'delta': finding['delta'], 'severity': finding['severity'],
            'exception': finding['exception'] is not None,
            'paths': sorted({e['path'] for e in finding['evidence'] if e.get('path')})}


def precision_section(records: list[dict]) -> dict:
    return {
        'candidate': score(records, CANDIDATE_VARIANTS, candidate_detected, candidate_as_expected,
                           lambda f: f['rule_id'].startswith('dup.') and f['candidate'] is True
                           and f['delta'] in ('new', 'worsened')),
        'fail_grade': score(records, FAIL_VARIANTS, fail_detected, fail_as_expected, is_fail_grade),
        'definitions': {
            'detection_rate': 'detected / expected labels (finding with the label fingerprint, its class, '
                              'and evidence on a path the candidate changed)',
            'as_expected_rate': 'detected findings that also have the contract shape / detected '
                                '(candidate: warning, new, no exception, exit 0, verdict pass; '
                                'fail_grade: candidate=false, new, exit 1, verdict fail, policy from base)',
            'precision': 'emitted findings matching an expected label / emitted findings of the class in '
                         'the labelled variants',
        },
    }


def clean_section(records: list[dict]) -> dict:
    fail_grade, warnings = [], []
    for record in (r for r in records if r['variant'] in CLEAN_VARIANTS and r['report']):
        for finding in record['report']['findings']:
            if is_fail_grade(finding):
                fail_grade.append(item(record, finding))
            elif finding['candidate'] is True and finding['severity'] == 'warning':
                warnings.append(item(record, finding))
    fresh = [w for w in warnings if w['delta'] in ('new', 'worsened')]
    return {
        'variants': list(CLEAN_VARIANTS),
        'fail_grade': {'count': len(fail_grade), 'items': fail_grade},
        'candidate_warnings': {
            'count': len(warnings),
            'new_or_worsened': len(fresh),
            'new_or_worsened_without_exception': sum(1 for w in fresh if not w['exception']),
            'by_rule': dict(Counter(w['rule_id'] for w in warnings)),
            'by_rule_new_or_worsened': dict(Counter(w['rule_id'] for w in fresh)),
            'by_variant_new_or_worsened': dict(Counter(w['variant'] for w in fresh)),
            'items': warnings,
        },
    }


def outside_expected_fail_grade(records: list[dict]) -> list[dict]:
    """Fail-grade findings of the labelled variants that no label names (ADR D4.2)."""
    out = []
    for record in (r for r in records if r['variant'] in (*CANDIDATE_VARIANTS, *FAIL_VARIANTS) and r['report']):
        wanted = {label_fingerprint(label, record['language'])[1] for label in record['labels']}
        out += [item(record, f) for f in record['report']['findings']
                if is_fail_grade(f) and f['fingerprint'] not in wanted]
    return out


def stats(values: list[float]) -> dict:
    return {'median_s': round(statistics.median(values), 4), 'max_s': round(max(values), 4), 'samples': len(values)}


def latency_section(records: list[dict], reps: int, startup: list[float]) -> dict:
    by_kind: dict[str, list[float]] = {}
    by_variant: dict[str, list[float]] = {}
    for kind in KINDS:
        for variant in VARIANTS:
            group = [r for r in records if r['kind'] == kind and r['variant'] == variant]
            # One checker invocation; multi is the sum over its repositories (ADR D4.3).
            samples = [sum(r['wall_secs'][i] for r in group) for i in range(reps)]
            by_kind.setdefault(kind, []).extend(samples)
            by_variant.setdefault(f'{kind}/{variant}', []).extend(samples)
    kinds = {k: stats(v) for k, v in by_kind.items()}
    return {
        'by_kind': kinds,
        'by_variant': {k: stats(v) for k, v in by_variant.items()},
        'python_startup': stats(startup),
        'estimated_added_per_review_s': {k: round(v['median_s'] * PATHS_PER_REVIEW, 4) for k, v in kinds.items()},
        'estimate_basis': f'checker median x {PATHS_PER_REVIEW} paths (WU check 1, review 1, delivery reuses the '
                          'result), at the size of these fixtures',
    }


def cost_section() -> dict:
    scanned = sorted(TOOL_DIR.rglob('*.py'))
    hits = [str(p.relative_to(ROOT)) for p in scanned if LLM_OR_NETWORK.search(p.read_text(encoding='utf-8'))]
    not_measured = ('未測定。人の決定により有料の再測定をしない。fixture で LLM の reviewer を動かしていないので、'
                    'token 数と USD の数値が無い。')
    return {
        'checker': {
            'llm_tokens': 0, 'usd': 0,
            'reason': '検査器は LLM を呼ばない決定的な検査である。',
            'source_scan': {'files': len(scanned), 'files_with_llm_or_network_reference': hits},
        },
        'design_review': {'llm_tokens': None, 'usd': None, 'status': 'unmeasured', 'reason': not_measured},
        'review': {'llm_tokens': None, 'usd': None, 'status': 'unmeasured', 'reason': not_measured},
    }


def matrix(records: list[dict]) -> list[dict]:
    rows = []
    for r in records:
        report = r['report']
        counts = report['summary']['counts'] if report else {}
        rows.append({'kind': r['kind'], 'variant': r['variant'], 'repo': r['repo'], 'exit_codes': r['exit_codes'],
                     'verdict': report['summary']['verdict'] if report else None,
                     'fail_grade': counts.get('fail_grade'), 'findings': counts.get('findings'),
                     'report_valid': r['report_valid'], 'stable_over_reps': r['stable']})
    return rows


def python_startup(reps: int) -> list[float]:
    values = []
    for _ in range(reps):
        started = time.monotonic()
        subprocess.run([sys.executable, '-I', '-c', 'pass'], check=True)
        values.append(time.monotonic() - started)
    return values


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--reps', type=int, default=MAX_REPS, choices=range(1, MAX_REPS + 1))
    args = parser.parse_args()
    load_before = os.getloadavg()
    with tempfile.TemporaryDirectory(prefix='orchard-measure-') as temporary:
        scratch = Path(temporary)
        records = [r for kind in KINDS for variant in VARIANTS
                   for r in measure_fixture(kind, variant, scratch, args.reps, True)]
        builtin = [r for kind in KINDS for variant in CLEAN_VARIANTS
                   for r in measure_fixture(kind, variant, scratch, 1, False)]
    startup = python_startup(args.reps)
    false_positives = clean_section(records)
    false_positives['outside_expected_fail_grade_in_labelled_variants'] = outside_expected_fail_grade(records)
    false_positives['builtin_profile'] = clean_section(builtin)
    tool_versions = sorted({r['report']['tool']['version'] for r in records if r['report']})
    result = {
        'precision': precision_section(records),
        'false_positives': false_positives,
        'latency': latency_section(records, args.reps, startup),
        'cost': cost_section(),
        'meta': {
            'reps': args.reps, 'kinds': list(KINDS), 'variants': list(VARIANTS),
            'fixture_repos': len(records), 'checker_runs': len(records) * args.reps + len(builtin),
            'tool_versions': tool_versions, 'python': platform.python_version(),
            'load_average_1m_before': round(load_before[0], 2), 'load_average_1m_after': round(os.getloadavg()[0], 2),
            'all_reports_valid': all(r['report_valid'] is not False for r in records),
            'all_runs_stable': all(r['stable'] for r in records),
        },
        'matrix': matrix(records),
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps({'out': str(args.out), **result['meta']}, ensure_ascii=False))
    return 0


if __name__ == '__main__':
    sys.exit(main())
