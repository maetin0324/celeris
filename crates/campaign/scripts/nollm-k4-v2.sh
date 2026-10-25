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
# 実 archive の通し（偽 Host・模擬 executor。test が audit.json に各 case を書く）。
K4_AUDIT_DIR="$out" K4_REAL_ARCHIVE="$tgz" cargo test -p celeris-campaign --test preflight_input_archive \
  -- --ignored --exact real::real_archive_offline_audit >"$out/audit-test.log" 2>&1
audit_exit=$?
set -e
old_assets > "$out/old-assets-after.json"

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
lock = None
k4_lock = None
if lock_exit == 0:
    lock = json.load(open(os.path.join(out, 'lock.json')))
    k4_lock = next(c for c in lock['cases'] if c['id'] == 'k4-cmp4-validity-analysis')
lock_ok = lock_exit == 0 and k4_lock['inputs'] == [{"name": pin['name'], "sha256": pin['sha256']}]
cases = [{"name": "lock", "kind": "cli", "argv": lock_argv, "exit": lock_exit, "expect_exit": 0, "matches": lock_ok}]
audit_path = os.path.join(out, 'audit.json')
audit = json.load(open(audit_path)) if os.path.exists(audit_path) else {}
cases += audit.get('cases', [])
before = json.load(open(os.path.join(out, 'old-assets-before.json')))
after = json.load(open(os.path.join(out, 'old-assets-after.json')))
old_ok = before == after and all(r['sha256'] == r['expect'] for r in after)
new_sha = h(tgz)
summary = {
    "schema": "celeris.k4-nollm-v2/1",
    "archive": {"path": tgz, "sha256": new_sha, "case_pin": pin, "matches_case_pin": new_sha == pin['sha256']},
    "cases": cases,
    "audit_test": {"argv": ["cargo", "test", "-p", "celeris-campaign", "--test", "preflight_input_archive", "--",
                            "--ignored", "--exact", "real::real_archive_offline_audit"],
                   "env": {"K4_AUDIT_DIR": out, "K4_REAL_ARCHIVE": tgz}, "exit": audit_exit, "log": "audit-test.log"},
    "expected_counts": audit.get('expected_counts'),
    "negative_counts": audit.get('negative_counts'),
    "lock": None if lock is None else {"file": "lock.json", "sha256": lock['sha256'], "measurement_id": lock['measurement_id'],
                                       "campaign_sha256": lock['campaign_sha256'], "k4": k4_lock},
    "old_assets": {"count": len(after), "unchanged": old_ok,
                   "old_tgz": after[0], "before": "old-assets-before.json", "after": "old-assets-after.json"},
    "preflight_mode": audit.get('preflight_mode'),
    "llm_calls": 0, "hpc_jobs": 0, "production_daemon": False, "measurement_claim": False,
    "blocker": audit.get('blocker'),
}
ok = (audit_exit == 0 and new_sha == pin['sha256'] and old_ok and cases and all(c.get('matches') and c['exit'] == c['expect_exit'] for c in cases))
summary["all_match"] = bool(ok)
with open(os.path.join(out, 'summary.json'), 'w') as f:
    json.dump(summary, f, indent=2, ensure_ascii=False)
    f.write('\n')
for c in cases:
    print(f"{c['name']}: exit={c['exit']} expect={c['expect_exit']} {'ok' if c.get('matches') else 'MISMATCH'}")
print(f"old assets unchanged: {old_ok} ({len(after)} files); new archive sha256 {new_sha} pin match {new_sha == pin['sha256']}")
sys.exit(0 if ok else 1)
EOF
