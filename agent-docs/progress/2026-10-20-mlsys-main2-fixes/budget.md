---
title: "[mlsys-main-2 不具合 1/budget] per_case を task 予算として渡し、上限後は自動再開しない"
tasks: [01M5C8KF8Y3MB6TT064AYV2F23]
status: done
updated: 2026-10-20
completed: 2026-10-20
---
# token 上限（Budget.max_tokens）と再開抑止

ADR `agent-docs/adr/2026-10-20-mlsys-main2-fixes.md` D1 の実装。

- `task_core::Budget.max_tokens: Option<u64>`（task 木の全 run の token 合計上限。API・schema・生成型に反映）。
- campaign の `celeris_exec` が `per_case` を `max_tokens` として task 作成 body に載せる（`check_template` は template からの指定を拒否）。
- dispatcher（`worker_finish.rs`）: run 終了時に task の累計（reviewer 含む）が上限以上なら continuation・retry・reviewer を起こさず
  `BudgetKind::Tokens`（`is_continuable == false`）で task を failed にする（`token_budget_exhausted: used/max`）。
- 上限の内側なら従来どおり再開し、再開 run には残り token（`RunContext.token_cap_remaining`）を渡す。
- run の途中観測: `EventSink::usage_progress` → `runs.usage_json` へ累積を上書き（events へは書かない。終了時の確定値が正）。claude_code adapter が累積を出す。
- 旧い仕組みの削除: 予算切れ種別を問わず再開する経路は `Tokens` だけ除外した（置換ではなく追加のため、削除対象なし）。

## 証拠
- `cargo test -p celeris-campaign -p task-core -p task-worker -p task-dispatch -p task-api --lib --tests` → 全 `test result: ok`、failed 0。
  - `token_budget_exhausted_no_continuation_or_review`（上限超過の予算切れ後は run 1 回・Failed・途中 usage が索引に入る）
  - `token_budget_remaining_allows_continuation_inside_cap`（上限内は再開し残り 150,000 を渡す）
  - `celeris_body_sets_per_case_max_tokens`、`check_template_rejects_max_tokens`
- `cargo clippy --workspace --all-targets -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
- 全体 `test-parallel.sh` は release gate のみ（方針: full-tests-release-only）。

## 本番への移行
- 測定 manifest の `per_case` が次の campaign から task の予算になる（1,500,000 は運用者が manifest で設定）。config 欄の削除はなし。
- 本番 DB の schema 変更なし（`budget` は task の JSON、`max_tokens` は省略可）。

## 未解決
- 1 run の途中で上限を超えた場合の run 内停止（worker の強制中断）は、`token_cap_remaining` を adapter に渡すところまで。run 内の強制終了は次の run 終了時判定に頼る（超過は 1 run 分）。
