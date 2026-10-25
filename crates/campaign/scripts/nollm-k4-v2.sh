#!/bin/sh
# K4 公開入力 v2 の無 LLM 通し（ADR 2026-10-25-k4-public-input-v2）。LLM・HPC・本番 daemon は使わない。
# 使い方: sh crates/campaign/scripts/nollm-k4-v2.sh <新tgz> <出力dir>
#   正例: stage-grader-inputs・正解報告の grade（32/30）・grade-out・check・export・preflight（偽 Host）。
#   独立負例: 全セル有効・失敗 job の W2 を有効・有効セル 1 つの数値 2% 誤り・欠測を分母から落とす（30/30）の grade。
#   事前拒否: archive 欠落と sha256 不一致の stage-grader-inputs（CLI）と preflight（偽 Host、transport 0 回）。
#   lock: 宣言資産の expected_lock（K4_LOCK_MANIFEST の campaign を repo の case 版で）を <出力dir>/lock.json に出す。
#   旧資産: 旧 tgz と旧資産一覧（K4_OLD_INVENTORY）の sha256 が実行の前後で固定値のままか確かめる。
# 結果は <出力dir>/summary.json（{"cases":[{"name","argv","exit","expect_exit",...}]}）。全 case が期待どおりなら exit 0。
set -eu
tgz=${1:?usage: nollm-k4-v2.sh <new.tgz> <out_dir>}
out=${2:?usage: nollm-k4-v2.sh <new.tgz> <out_dir>}
[ -f "$tgz" ] || { echo "archive not found: $tgz" >&2; exit 2; }
tgz=$(cd "$(dirname "$tgz")" && pwd)/$(basename "$tgz")
mkdir -p "$out"
out=$(cd "$out" && pwd)
repo=$(cd "$(dirname "$0")/../../.." && pwd)
old_tgz=${K4_OLD_TGZ:-/local/celeris/data/measure/main-3/out/snapshot/inputs/main-v3-n2-43706.tgz}
old_sha=128057cf4016535941963fc4142ba424c05c65889905ca2bbcc190f6161077a1
inv=${K4_OLD_INVENTORY:-/local/celeris/data/workspaces/01M5KVHA4222TK6VEKW599SNMA/artifacts/mlsys-main-4/old-assets-after.json}
lock_manifest=${K4_LOCK_MANIFEST:-/local/celeris/data/workspaces/01M5KVHA4222TK6VEKW599SNMA/artifacts/mlsys-main-4/revisions/judged-scope-v2/pinned/main/final.json}
cd "$repo"

# 旧資産の sha256（実行前）。
old_assets() {
  python3 -I - "$old_tgz" "$old_sha" "$inv" <<'EOF'
import hashlib, json, sys
def h(p):
    try:
        with open(p, 'rb') as f:
            return hashlib.file_digest(f, 'sha256').hexdigest()
    except OSError:
        return None
tgz, want, inv = sys.argv[1:]
rows = [{"path": tgz, "expect": want, "sha256": h(tgz)}]
for p, w in sorted(json.load(open(inv)).items()):
    rows.append({"path": p, "expect": w, "sha256": h(p)})
print(json.dumps(rows))
EOF
}
old_assets > "$out/old-assets-before.json"

bin_dir=$(cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug
cargo build -p celeris-campaign
bin=$bin_dir/celeris-campaign

# lock（新 expected_lock）。cases/grader/grade-case は repo の v2 版。
set +e
"$bin" lock --manifest "$lock_manifest" --cases-root crates/campaign/cases \
  --grader-root crates/campaign/fixtures/grader-private \
  --grade-case-root crates/campaign/fixtures/grade-cases >"$out/lock.json" 2>"$out/lock.stderr"
lock_exit=$?
# 実 archive の通し（既存の opt-in 試験。偽 Host・模擬 executor）。試験が <出力dir>/audit/summary.json に
# stage・grade・grade-out・check・export・全セル有効の負例・stage の欠落/不一致の各 CLI と preflight の結果を書き、
# preflight の拒否では transport 0 回と out 不在を assert する。
# 前回の audit は消す（残した fixture の .git/objects は読み取り専用で、上書きの copy が EACCES になる）。
[ -d "$out/audit" ] && chmod -R u+w "$out/audit"
rm -rf "$out/audit"
K4_AUDIT_DIR="$out/audit" K4_REAL_ARCHIVE="$tgz" cargo test -p celeris-campaign --test preflight_input_archive \
  -- --ignored --exact real::real_archive_offline_audit >"$out/audit-test.log" 2>&1
audit_exit=$?
set -e

# 独立負例（1 つずつ別の誤りだけを入れた報告）を試験の正例報告から作り、同じ grade argv で採点する。
python3 -I - "$out" <<'EOF'
import csv, io, json, os, shutil, sys
out = sys.argv[1]
audit = os.path.join(out, 'audit')
pos = os.path.join(audit, 'positive-artifact')
with open(os.path.join(pos, 'results/cells.csv'), newline='') as f:
    rows = list(csv.DictReader(f))
    fields = list(rows[0].keys())
with open(os.path.join(pos, 'report.md')) as f:
    report = f.read().splitlines()
# 先頭の file 数の metric（cells_with_cell_json・cells_<fs>）はそのまま、集計の metric は表から数え直す。
counted = ('metric cells_total', 'metric cells_valid')
head = [l for l in report if not l.startswith(counted)]
def metrics(rs):
    fss = sorted({r['fs'] for r in rs})
    lines = [f"metric cells_total = {len(rs)}"]
    lines += [f"metric cells_total_{fs} = {sum(r['fs'] == fs for r in rs)}" for fs in fss]
    v = [r for r in rs if r['valid'] == 'valid']
    lines += [f"metric cells_valid = {len(v)}"]
    lines += [f"metric cells_valid_{fs} = {sum(r['fs'] == fs for r in v)}" for fs in fss]
    return lines
assert '\n'.join(head + metrics(rows)) == '\n'.join(report), 'positive report layout changed'
def make(name, rs):
    d = os.path.join(out, name)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(os.path.join(d, 'results'))
    buf = io.StringIO()
    w = csv.DictWriter(buf, fieldnames=fields, quoting=csv.QUOTE_MINIMAL, lineterminator='\n')
    w.writeheader()
    w.writerows(rs)
    with open(os.path.join(d, 'results/cells.csv'), 'w') as f:
        f.write(buf.getvalue())
    with open(os.path.join(d, 'report.md'), 'w') as f:
        f.write('\n'.join(head + metrics(rs)) + '\n')
    return {"total": len(rs), "valid": sum(r['valid'] == 'valid' for r in rs)}
cp = lambda: [dict(r) for r in rows]
# 失敗 job（exit=-20）の BenchFS W2 だけを有効にした報告（32/32）。
w2 = cp()
for r in w2:
    if r['valid'] != 'valid':
        r['valid'], r['reasons'] = 'valid', ''
# 件数は正しく、有効セル 1 つの write_MiB_s だけを 2% 誤った報告（30/32）。
num = cp()
r = next(r for r in num if r['valid'] == 'valid' and r['write_MiB_s'])
r['write_MiB_s'] = f"{float(r['write_MiB_s']) * 1.02:.4f}"
# 欠測の 2 セルを表と分母から落とした報告（30/30）。
drop = [r for r in cp() if r['valid'] == 'valid']
counts = {
    "grade-invalid-w2-as-valid-negative": make('invalid-w2-as-valid-negative', w2),
    "grade-numeric-error-negative": make('numeric-error-negative', num),
    "grade-missing-dropped-from-denominator-negative": make('missing-dropped-from-denominator-negative', drop),
}
with open(os.path.join(out, 'negative-counts.json'), 'w') as f:
    json.dump(counts, f, indent=2)
EOF
grade_argv=$(python3 -I -c 'import json,sys; c=next(c for c in json.load(open(sys.argv[1]))["commands"] if c["name"]=="grade-positive"); print("\n".join(c["argv"]))' "$out/audit/summary.json")
: >"$out/negatives.tsv"
for name in invalid-w2-as-valid-negative numeric-error-negative missing-dropped-from-denominator-negative; do
  # 正例の argv の --artifact だけを負例の報告に差し替える。
  set --
  next_is_artifact=0
  while IFS= read -r a; do
    if [ "$next_is_artifact" = 1 ]; then a=$out/$name; next_is_artifact=0; fi
    [ "$a" = --artifact ] && next_is_artifact=1
    set -- "$@" "$a"
  done <<EOF
$grade_argv
EOF
  set +e
  "$@" >"$out/grade-$name.stdout" 2>"$out/grade-$name.stderr"
  e=$?
  set -e
  printf 'grade-%s\t%s\t%s\n' "$name" "$e" "$*" >>"$out/negatives.tsv"
done
old_assets > "$out/old-assets-after.json"
# 試験が残す fixture の写しは 1 GB を超える。再現に要る小さい file（manifest・case・grade-case）だけ残す。
[ -d "$out/audit/fixture" ] && find "$out/audit/fixture" -type f -size +1M -delete

python3 -I - "$out" "$bin" "$lock_manifest" "$lock_exit" "$audit_exit" "$tgz" "$repo" <<'EOF'
import hashlib, json, os, sys
out, bin_, lock_manifest, lock_exit, audit_exit, tgz, repo = sys.argv[1:]
lock_exit, audit_exit = int(lock_exit), int(audit_exit)
def h(p):
    with open(p, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()
case_json = os.path.join(repo, 'crates/campaign/cases/final/k4-cmp4-validity-analysis/case.json')
pin = json.load(open(case_json))['inputs'][0]
lock_argv = [bin_, 'lock', '--manifest', lock_manifest, '--cases-root', 'crates/campaign/cases',
             '--grader-root', 'crates/campaign/fixtures/grader-private',
             '--grade-case-root', 'crates/campaign/fixtures/grade-cases']
lock = k4_lock = None
if lock_exit == 0:
    lock = json.load(open(os.path.join(out, 'lock.json')))
    k4_lock = next(c for c in lock['cases'] if c['id'] == 'k4-cmp4-validity-analysis')
lock_ok = lock_exit == 0 and k4_lock['inputs'] == [{"name": pin['name'], "sha256": pin['sha256']}]
cases = [{"name": "lock", "kind": "cli", "argv": lock_argv, "exit": lock_exit, "expect_exit": 0}]
cases[0]["matches"] = lock_ok
audit_path = os.path.join(out, 'audit', 'summary.json')
audit = json.load(open(audit_path)) if os.path.exists(audit_path) else {}
# 試験の CLI 記録。名前が -negative で終わるものは拒否（exit 1）、他は exit 0 を期待する。
for c in audit.get('commands', []):
    want = 1 if c['name'].endswith('-negative') else 0
    cases.append({"name": c['name'], "kind": "cli", "argv": c['argv'], "exit": c['exit'], "expect_exit": want,
                  "matches": c['exit'] == want, "source": "audit/summary.json"})
# preflight は試験内の library 呼び出し（偽 Host）。拒否は試験が transport 0 回・out 不在・原因名を assert する。
test_argv = ["cargo", "test", "-p", "celeris-campaign", "--test", "preflight_input_archive", "--",
             "--ignored", "--exact", "real::real_archive_offline_audit"]
for name, file, want in [("preflight-positive", "preflight-positive.json", 0),
                         ("preflight-missing-negative", "preflight-missing.json", 1),
                         ("preflight-sha-negative", "preflight-sha.json", 1)]:
    p = os.path.join(out, 'audit', file)
    r = json.load(open(p)) if os.path.exists(p) else None
    exit_ = None if r is None else (0 if r['ok'] else 1)
    cases.append({"name": name, "kind": "library", "argv": test_argv + [f"--case={name}"], "exit": exit_,
                  "expect_exit": want, "paid_calls_made": None if r is None else r.get('paid_calls_made'),
                  "missing": None if r is None else [f"{m['check']}: {m['detail']}" for m in r['missing']],
                  "transport_calls_asserted_zero": want == 1 and audit_exit == 0,
                  "matches": exit_ == want and audit_exit == 0, "source": f"audit/{file}"})
neg_counts = json.load(open(os.path.join(out, 'negative-counts.json')))
for line in open(os.path.join(out, 'negatives.tsv')):
    name, e, argv = line.rstrip('\n').split('\t')
    cases.append({"name": name, "kind": "cli", "argv": argv.split(' '), "exit": int(e), "expect_exit": 1,
                  "matches": int(e) == 1, "reported_counts": neg_counts[name]})
before = json.load(open(os.path.join(out, 'old-assets-before.json')))
after = json.load(open(os.path.join(out, 'old-assets-after.json')))
old_ok = before == after and all(r['sha256'] == r['expect'] for r in after)
new_sha = h(tgz)
summary = {
    "schema": "celeris.k4-nollm-v2/2",
    "archive": {"path": tgz, "sha256": new_sha, "case_pin": pin, "matches_case_pin": new_sha == pin['sha256']},
    "cases": cases,
    "audit_test": {"argv": test_argv, "env": {"K4_AUDIT_DIR": os.path.join(out, 'audit'), "K4_REAL_ARCHIVE": tgz},
                   "exit": audit_exit, "log": "audit-test.log", "summary": "audit/summary.json"},
    "lock": None if lock is None else {"file": "lock.json", "sha256": lock['sha256'], "measurement_id": lock['measurement_id'],
                                       "campaign_sha256": lock['campaign_sha256'], "k4": k4_lock},
    "old_assets": {"count": len(after), "unchanged": old_ok,
                   "old_tgz": after[0], "before": "old-assets-before.json", "after": "old-assets-after.json"},
    "preflight_mode": "library check/run_gated with fake Host (no daemon, no LLM)",
    "llm_calls": 0, "hpc_jobs": 0, "production_daemon": False, "measurement_claim": False,
    "blocker": audit.get('blocker'),
}
ok = (audit_exit == 0 and new_sha == pin['sha256'] and old_ok and all(c['matches'] and c['exit'] == c['expect_exit'] for c in cases))
summary["all_match"] = bool(ok)
with open(os.path.join(out, 'summary.json'), 'w') as f:
    json.dump(summary, f, indent=2, ensure_ascii=False)
    f.write('\n')
for c in cases:
    print(f"{c['name']}: exit={c['exit']} expect={c['expect_exit']} {'ok' if c['matches'] else 'MISMATCH'}")
print(f"old assets unchanged: {old_ok} ({len(after)} files); new archive sha256 {new_sha} pin match {new_sha == pin['sha256']}")
sys.exit(0 if ok else 1)
EOF
