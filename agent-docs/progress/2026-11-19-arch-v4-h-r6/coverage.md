---
title: アーキ v4 H R6 coverage — selfdeploy の差分を設計 review の範囲に入れ、提出時の HEAD で不変条件の試験を走らせる
tasks: [01M7VJY8Q1MMAH87CSPQZWC7VY]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# coverage（WorkUnit）

完了日: 2026-11-20

## 結論

- 提出時の HEAD で、`(release|verify|promote)\.sh` を含む試験 22 本（`scripts/selfdeploy/tests/*.sh` の 21 本と
  `scripts/dev/test-release-script-gate.sh`）を、空の一時の `CELERIS_LOGS_DIR`・`CELERIS_BACKUPS_DIR` に向け `</dev/null` を付けて走らせた。
  22 本とも exit 0、合計 220 秒。一時 dir に書かれた file は 0 件。本番の `/local/celeris/state/logs/release-records.jsonl` は前後とも無い。
- `crates/` に `release-records`・`sd_release_record` の参照は 0 件。`scripts/selfdeploy` の台本（tests を除く `*.sh`・`*.py`）で
  `release-records` を参照する注釈でない行は 4 行で、4 行とも `lib.sh` の `sd_release_record` の本体（249〜295 行）の中にある。
  `sd_release_record` を呼ぶのは release.sh（3 か所）・verify.sh（1 か所）・promote.sh（1 か所）だけ。台本も crates も要約の log を判断に読まない
  （`unpromoted-release-no-takeover`）。
- 落ちた試験は無く、(2) の直しはしていない。`scripts/selfdeploy/`・`crates/`・ADR・policy の file は変えていない。本番の release.sh・verify.sh・
  promote.sh は走らせていない。

## 設計 review の範囲

R6 の final review は health の項目 `diff_design` だけで落ちた。採用した計画 v2 の WorkUnit `counts`・`summary` は `context.paths` を
`scripts/` と書き、selfdeploy の class の正規表現 `^scripts/selfdeploy/` に合わなかった。そのため計画は Low と判定されて設計 review が起きず、
次の 13 file の差分を覆う合格の設計 review が無かった（不変条件 `unpromoted-release-no-takeover`）。

- 台本: `scripts/selfdeploy/lib.sh`（`sd_release_record`）、`release.sh`・`verify.sh`・`promote.sh`（呼び出し）
- 試験: `scripts/selfdeploy/tests/` の browser_ledger_release_stages・release_parallel_test_gate・release_prune_stale_test_binaries・
  release_record_summary_survives_prune・release_uses_scratch_lease・release_web_bundle_requires_deps・release_web_required・
  verify_web_app_start と `fixtures/release-gate-tree.sh`

この WorkUnit は `context.paths` を `scripts/selfdeploy/` にした計画の leaf で、計画の採用前の設計 review が上の 13 file の差分と
[ADR 2026-11-20 release-record-summary-log](../../adr/2026-11-20-release-record-summary-log.md) の設計 brief（1 節）を見る。
R6 の task の差分（`integrate wu/r7-rules (phase fix1)` から提出時の HEAD まで）は 23 file で、`scripts/selfdeploy/` は上の 13 file。
`crates/` と policy の file は 0 file。

## (1) 不変条件の試験（提出時の HEAD で実行）

### 22 本の試験

command（runner は 1 本ずつ `bash "$f" </dev/null` で走らせ、exit と秒を取る）:

```bash
logs=$(mktemp -d); backups=$(mktemp -d)
export CELERIS_LOGS_DIR="$logs" CELERIS_BACKUPS_DIR="$backups"
for f in $(grep -lE '(release|verify|promote)\.sh' scripts/selfdeploy/tests/*.sh scripts/dev/test-release-script-gate.sh); do
  bash "$f" </dev/null >"log.$(basename "$f")" 2>&1; echo "$f $?"
done
find "$logs" "$backups" -mindepth 1 | wc -l    # → 0
```

結果: 22 本とも exit 0、合計 220 秒（`real 3m40s`）、一時 dir の file 0 件。

| 試験 | exit | 秒 |
|---|---|---|
| scripts/selfdeploy/tests/browser_ledger_release_stages.sh | 0 | 12 |
| scripts/selfdeploy/tests/bundle_includes_verify_helpers.sh | 0 | 0 |
| scripts/selfdeploy/tests/credentiald_switch.sh | 0 | 4 |
| scripts/selfdeploy/tests/prepare_timeout_test.sh | 0 | 0 |
| scripts/selfdeploy/tests/promote_authorization_marker.sh | 0 | 30 |
| scripts/selfdeploy/tests/promote_backup_retention.sh | 0 | 0 |
| scripts/selfdeploy/tests/promote_handoff_settled.sh | 0 | 1 |
| scripts/selfdeploy/tests/promote_live_abort.sh | 0 | 26 |
| scripts/selfdeploy/tests/promote_web_follows_release.sh | 0 | 6 |
| scripts/selfdeploy/tests/release_lock_bundled_scripts.sh | 0 | 5 |
| scripts/selfdeploy/tests/release_notes_promote.sh | 0 | 77 |
| scripts/selfdeploy/tests/release_parallel_test_gate.sh | 0 | 9 |
| scripts/selfdeploy/tests/release_prune_stale_test_binaries.sh | 0 | 3 |
| scripts/selfdeploy/tests/release_record_summary_survives_prune.sh | 0 | 7 |
| scripts/selfdeploy/tests/release_target_settled_after_build.sh | 0 | 1 |
| scripts/selfdeploy/tests/release_uses_scratch_lease.sh | 0 | 1 |
| scripts/selfdeploy/tests/release_web_bundle_requires_deps.sh | 0 | 4 |
| scripts/selfdeploy/tests/release_web_required.sh | 0 | 10 |
| scripts/selfdeploy/tests/repo_health_bundle.sh | 0 | 0 |
| scripts/selfdeploy/tests/sync_skills.sh | 0 | 0 |
| scripts/selfdeploy/tests/verify_web_app_start.sh | 0 | 12 |
| scripts/dev/test-release-script-gate.sh | 0 | 12 |

- `ls /local/celeris/state/logs/release-records.jsonl` → 走らせる前も後も「無い」（試験が本番の log に書いていない）。
- promote_*.sh の 5 本（`ls scripts/selfdeploy/tests/promote_*.sh` の全部）と release_notes_promote.sh はこの 22 本に入る。

### 要約の log を判断に読まないこと

command と結果（4 つとも条件どおり。runner は exit 0）:

- `grep -rn -e release-records -e sd_release_record crates/` → 0 件。
- `find scripts/selfdeploy -path scripts/selfdeploy/tests -prune -o -type f \( -name '*.sh' -o -name '*.py' \) -print | xargs grep -n release-records | grep -vE '^[^:]+:[0-9]+:[[:space:]]*#'`
  → 4 行（`lib.sh` の 250・252・287・291 行）。`sd_release_record() {` は 249 行、閉じ括弧は 295 行で、4 行とも本体の中。
  rollback.sh・status.sh には参照が無い。
- 同じ file の集合で `sd_release_record` の注釈でない行 → 定義（lib.sh:249）と、release.sh の 421・492・641 行、verify.sh の 773 行、
  promote.sh の 575 行。呼び出し元の file は release.sh・verify.sh・promote.sh だけ。
- `git diff --name-only <integrate wu/r7-rules (phase fix1)> HEAD -- crates/` → 0 file（ADR 1.7 の「crates を変えないので差分で確かめる」）。

## (2) 落ちた試験の直し

無し。(1) の 22 本が全部 exit 0 だったので、`scripts/selfdeploy/` は変えていない。ADR の D1〜D5 と設計 brief も変えていない。

## 文書と repo の検査（提出時の HEAD で実行）

| command | exit | 要点 |
|---|---|---|
| `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok` |
| `sh scripts/dev/check-adr-numbers.sh` | 0 | `check-adr-numbers: ok (468 files)` |
| `sh scripts/dev/progress-index.sh --check` | 0 | `progress-index --check: ok` |
| `bash scripts/dev/test-affected.sh`（一時の LOGS・BACKUPS dir、`</dev/null`） | 0 | `test-affected: 試験対象の変更なし（docs のみ等）`。repo-health は `verdict=pass exit=0 … new=0 worsened=0 existing=695`。python の試験 126 件 OK、repo_health_gate の 7 件 OK |
| `cargo clippy --workspace -- -D warnings` | 0 | 警告は vendor の deno_core の 1 件だけ（R6 の前から出ている。exit は 0） |
| `cargo fmt --all -- --check` | 0 | 出力 0 行 |

## commit（件名）

- 「coverage: selfdeploy の差分を設計 review の範囲に入れ、提出時の HEAD で不変条件の試験 22 本を記録する」

## 未解決

- 無し。この WorkUnit で新しく見つけたものは無い。R6 全体の未解決は [R6 の進捗ファイル](../2026-11-19-arch-v4-h-r6.md) の「未解決」に書いた。
