---
title: eval-dispatch eval-admit（評価予約枠の config と admission 純関数）
tasks: [01M4Q7C4X2QR3HYPTTQAG5GXQ9]
status: done
updated: 2026-10-12
---

# eval-dispatch eval-admit

brief 契約 3 のうち config と admission の判断だけを置いた。評価 run の起動段（dispatcher の `eval_trials` tick）・表・API は他の unit が担当する。

- `crates/celeris/src/config/routing_eval.rs`: `[routing_eval] reserved_runs`（`deny_unknown_fields`、既定 0 = 評価 run を起こさない）。`Config.routing_eval` として `config/mod.rs` に登録。
- `Config::dispatch_config`（`config/dispatch.rs`）が `task_dispatch::capacity::EvalRuntimeConfig { reserved_runs }` を `DispatchConfig.eval` に入れて dispatcher の設定へ渡す。
- `crates/task-dispatch/src/capacity.rs`: `EvalRuntimeConfig`（既定 `reserved_runs = 0`）、`EvalDefer { Disabled | ReservedFull | AccountFull | QuotaCap }`（`as_str()` 付き）、純関数 `eval_admission(reserved_runs, eval_in_flight, account_in_use, account_limit, quota_ok) -> Result<(), EvalDefer>`。判断の順は reserved_runs = 0 → 予約枠満杯 → account 同時実行満杯 → quota 超過。
- `DispatchConfig` に欄を足したので、`DispatchConfig` を直接組み立てる試験 7 箇所（`dispatcher/tests/{mod,review,knowledge_fallback}.rs`・`tests/{unified_kill,cos_chat_triage}.rs`）と `celerisctl` の `Config` 直組み立て 1 箇所（`commands/worker_tests.rs`）に既定値を足した。

試験:
- `capacity::tests::eval_admission_table`（表駆動 11 行: 枠 0・枠 0 が他の理由より先・枠満杯 1/1 と 2/2・account 満杯・account が quota より先・account_limit = 0・quota 超過 ×2・通る場合 ×2）、`eval_defer_reasons_have_stable_names`、`eval_runtime_config_defaults_to_no_reserved_runs`。
- `config::routing_eval::tests`: 既定 0（節なし・節だけ）、`reserved_runs = 3` が `dispatch_config().eval.reserved_runs` に届く、未知欄・綴り間違いは設定エラーのまま。

Verification:
- `cargo test -p task-dispatch --lib`（TMPDIR=/local/x）: exit 0。938 passed, 0 failed。
- `cargo test -p celeris --lib`: exit 0。429 passed, 0 failed（`config::routing_eval` 3 件を含む）。
- `cargo test -p celerisctl --bins`: exit 0。127 passed, 0 failed。
- `cargo clippy --workspace --all-targets`: 警告・エラーなし。`cargo fmt --check -p task-dispatch -p celeris -p celerisctl`: 差分なし。
- `cargo check --workspace --all-targets`: exit 0。
- `scripts/dev/test-parallel.sh`（workspace 全体）はこの unit では走らせていない（担当は closeout）。変更が触る 3 crate（task-dispatch・celeris・celerisctl）の試験と workspace の clippy・check で代替した。

未解決事項:
- `eval_admission` の呼び出し側（`account_in_use`・`account_limit`・`quota_ok` の実測の集め方、defer した試行を `queued` に戻す経路）は起動段の unit（trial-run）が配線する。`EvalDefer::as_str()` はそのときの記録・ログ用。
- 評価 run を本番の `max_concurrency` に数えない規則（tick の段順: 本番の dispatch の後）も起動段の担当。この unit は config と純関数だけ。
- ADR 付記「評価 run の受け口（eval-dispatch）」は trial-store unit が書く。実装との食い違いが出たら同じ付記に追記する（この unit の config 欄名は `[routing_eval] reserved_runs`、型は `task_dispatch::capacity::EvalRuntimeConfig` / `EvalDefer` / `eval_admission`）。

提案: なし。
