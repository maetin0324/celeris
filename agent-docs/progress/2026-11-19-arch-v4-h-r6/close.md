---
title: アーキ v4 H R6 close — R6 の全体の検査を提出時の HEAD で走らせ進捗を done にする
tasks: [01M7VJY8Q1MMAH87CSPQZWC7VY]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# close（WorkUnit）

完了日: 2026-11-20

## 結論

- 提出時の HEAD（`integrate wu/summary (phase impl)` の上にこの WorkUnit の commit を足したもの）で、受け入れ条件の 8 つの command を
  走らせ、全部 exit 0 だった。既存の prune・promote の試験 10 本も exit 0。
- [R6 の進捗ファイル](../2026-11-19-arch-v4-h-r6.md) を `status: done` にし、結論・証拠・測定・回帰条件・リリースゲート・本番への移行・
  未解決・提案を書いた。
- 実装はしていない。`crates/`・`scripts/`・policy の file は変えていない。本番の release.sh・verify.sh・promote.sh は走らせていない。

## commit（件名）

- 「close: R6 の全体の検査を提出時の HEAD で走らせ、進捗を done にする」

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

selfdeploy の試験は、先に `export CELERIS_LOGS_DIR=$(mktemp -d) CELERIS_BACKUPS_DIR=$(mktemp -d)` をしてから走らせた。

| 受け入れ条件 | command | exit | 要点 |
|---|---|---|---|
| 1 | `bash scripts/dev/test-affected.sh` | 0 | `test-affected: 試験対象の変更なし（docs のみ等）`。WorkUnit の基準と HEAD の差分は docs だけ。repo-health は `verdict=pass exit=0 … new=0 worsened=0 existing=695`（base=1680de5c）。python の試験 126 件 OK、repo_health_gate の 7 件 OK |
| 1 | `cargo clippy --workspace -- -D warnings` | 0 | 警告 0 |
| 1 | `cargo fmt --all -- --check` | 0 | 出力 0 行 |
| 1 | `bash scripts/selfdeploy/tests/release_parallel_test_gate.sh` | 0 | `release_parallel_test_gate: ok`（約 9 秒） |
| 1 | `bash scripts/selfdeploy/tests/release_record_summary_survives_prune.sh` | 0 | `release_record_summary_survives_prune: ok`（約 7 秒） |
| 2 | `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok` |
| 2 | `sh scripts/dev/check-adr-numbers.sh` | 0 | `check-adr-numbers: ok (468 files)` |
| 2 | `sh scripts/dev/progress-index.sh --check` | 0 | `progress-index --check: ok` |

追加で確かめたこと:

- 既存の prune・promote の試験（`ls scripts/selfdeploy/tests/*.sh | grep -E 'prune|promote'` の 10 本）→ 10 本とも exit 0。
  promote_authorization_marker・promote_backup_retention・promote_handoff_settled・promote_live_abort・promote_web_follows_release・
  prune_tests_stay_in_tmp・release_notes_promote・release_prune_production_scale・release_prune_stale_test_binaries・
  release_record_summary_survives_prune。
- 上の試験の後、一時の `CELERIS_LOGS_DIR`・`CELERIS_BACKUPS_DIR` に書かれた file は 0 件（`find … -type f | wc -l` → 0）。
  `ls /local/celeris/state/logs/release-records.jsonl` → 「No such file or directory」（試験が本番の log に書いていない）。
- R6 の task の差分（H の branch の `integrate wu/r7-rules (phase fix1)` から HEAD まで）は 22 file。`crates/` と policy の file を含まない。
  `grep -rn release-records crates/` → 0 件。

## 未解決

- この WorkUnit で新しく見つけたものは無い。R6 全体の未解決は [R6 の進捗ファイル](../2026-11-19-arch-v4-h-r6.md) の「未解決」に書いた。
