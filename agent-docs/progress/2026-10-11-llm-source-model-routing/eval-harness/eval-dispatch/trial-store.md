---
tasks: [01M4Q7C4X2QR3HYPTTQAG5GXQ9]
---
# eval-dispatch trial-store

完了日: 2026-10-12

評価 run の試行を持つ表 `eval_trials`（migration 0072）・型・store を task-core に足し、受け口の形を ADR 付記に固定した。

- migration: 全ブランチの `crates/task-core/migrations` を走査し最大が 0071 → `0072_eval_trials.sql`。`SCHEMA_VERSION = 72`。
- 型 `crates/task-core/src/routing_eval_trial.rs`（`EvalTrialSpec`・`EvalTrial`・`EvalTrialStatus`）、store `crates/task-core/src/store/eval_trials.rs`
  （`insert_eval_trials` 冪等・`eval_trials_list`・`eval_trial_get`・`eval_trial_claim_next`・`eval_trial_unclaim`・`eval_trial_mark_started`・
  `eval_trial_mark_scoring`・`eval_trial_finish`・`eval_trial_cancel`）。brief 契約に `eval_trial_unclaim`（admission 不可で queued に戻す。run 未記録のときだけ）を足した。
- 旧 schema 試験: cluster_job/tests.rs・cron/store_tests.rs の DROP 列に eval_trials と索引 2 本、delivery.rs の版数列に 72、版数 pin の 71 → 72。
- ADR `agent-docs/adr/2026-10-11-llm-source-model-routing-evaluation.md` に「付記 2026-10-12: 評価 run の受け口（eval-dispatch）」（契約 1〜7）。
- `docs/architecture-map.md` の task-core 節に 1 行。

Verification:
- `cargo test -p task-core`: exit 0。lib 937 passed（基点 931 + 新規 6 件）、0 failed。
- `cargo test -p celerisctl --test no_migrate`: exit 0。4 passed。
- `cargo clippy -p task-core --all-targets -- -D warnings`: exit 0。
- `sh scripts/dev/check-doc-links.sh`・`sh scripts/dev/check-adr-numbers.sh`: exit 0。

未解決事項:
- `python3 scripts/dev/check-architecture-map.py` は基点（132770e2）で既に exit 1（68 行目の `test-parallel.sh` 等 3 件）。本 unit の追加行は原因ではない。
- workspace 全体の `test-parallel.sh`・clippy は統合段（integrate-core 以降）の担当で、この unit では走らせていない。
