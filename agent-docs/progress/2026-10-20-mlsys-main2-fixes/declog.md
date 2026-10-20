---
task: 01M5C8KF8Y3MB6TT064AYV2F23
unit: declog
status: done
completed: 2026-10-20
---

# declog: decision-log `--verify-connected` の欠け

ADR 2026-10-20-mlsys-main2-fixes D3 のとおり、規則は変えず原因を直した。

## 原因（測定 DB の写し dl-src で確認）
- `grader_verdict`: 本番に `campaign_verdict` を書く writer が無かった。
- reviewer run の `proposed_first`: reviewer run には `routing_decided` が無く、`llm_route_selected`（`worker_started` の直後、run_id なし、
  `metadata.candidates` と `metadata.harness`）だけがある。投影が読んでいなかった。

## 変更
- `crates/task-dispatch/src/dispatcher/decision_log_wire.rs`: `RoutingDecided` の無い run は、直前に始まった run の `llm_route_selected`
  の候補列から `proposed_first` / `proposed_candidates` を埋める。候補が空なら欠測のまま。
- `crates/task-core/src/decision_log_export.rs`: `join_grades`。`celerisctl campaign export --grades <grades.jsonl>`
  （`crates/celerisctl/src/commands/campaign.rs`）で、試行の全 run に `pass`/`fail` を入れる。manifest hash の食い違いは exit 非 0。
  grade 行の無い試行は欠測のまま。`--verify-connected` は `--grades` 必須（clap `requires`）。

## 試験
- `cargo test -p task-dispatch --lib decision_log_wire` → 13 passed（`reviewer_run_gets_proposed_first_from_llm_route_selected`、
  `llm_route_selected_without_candidates_leaves_proposed_first_missing` を含む）。
- `cargo test -p celerisctl --test campaign_connected` → 8 passed（`grade_row_joins_every_run_of_the_trial`、
  `grade_with_foreign_manifest_hash_is_rejected`、`verify_connected_requires_grades`、grade 行なし → `grader_verdict is missing`）。
- `cargo clippy -p task-core -p task-dispatch -p celerisctl --all-targets -- -D warnings` → exit 0、`cargo fmt --all -- --check` → exit 0。
- workspace 全体の test-parallel は統合段（release gate）で実行。

## 本番への移行
- 測定の config・manifest の変更は不要。運用者は export を `--grades $OUT/grades.jsonl --verify-connected` で実行する。

## 未解決・提案
- `docs/ops/measure-run-final.md` §export（179 行）のコマンドに `--grades $OUT/grades.jsonl` を足す作業は、同じファイルを直す
  runbook unit と衝突するため本 unit では触っていない。統合段で足すこと。
