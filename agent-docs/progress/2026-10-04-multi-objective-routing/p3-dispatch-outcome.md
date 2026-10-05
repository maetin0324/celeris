---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: dispatch-outcome
status: done-in-branch
completed: 2026-10-05
---

# Phase 3 dispatch-outcome: review・検査の結果から routing_outcome_recorded を決定的に追記する

ADR 2026-10-04-multi-objective-model-routing §6（outcome の event schema）と §10 Phase 3 に従い、run の結果を `routing_outcome_recorded` として追記する口を入れた。投影の規則（reward 式・supersede・未判定は None）は既存の `task_core::model_router::feedback::project_run_outcomes` をそのまま使う。

## 変更

- `task-core::model_router::feedback`: run の cash・tokens・wall・retries を、同じ run の複数の完了記録で**合算**するようにした（従来は上書き）。欠測同士は欠測のまま（`add_opt`）。
- `task-ops::routing_outcome::record_routing_outcomes(store, task_id)`: task の events を読み、`pending_run_outcomes`（events に同じ `outcome_id` が無いものだけ）を `routing_outcome_recorded` として append する。戻り値は追記件数。events は追記のみ。
- `WorkerStarted.model` と `WorkerFinished.usage` から既知モデルの欠測 cost を `estimate_cost_usd` で補完する。明示の `usage.cost_usd` は優先し、複数の完了記録は task-core の純粋投影で合算する。
- `task-ops::comment::reopen`: 再開の遷移の後に呼ぶ。
- `task-dispatch::dispatcher::routing_context::record_routing_outcomes`（Dispatcher の薄いラッパー、失敗は warn のみで dispatch を止めない）を、`drain_completions` の次の完了の後に呼ぶ:
  - `Completion::Worker`（worker run の完了）
  - `Completion::Review`（review の判定。`review_pass` / `review_fail` の適用を含む）
  - `Completion::WorkUnitChecks`（WU 受け入れ検査の完了）
  - `Completion::Integration`（統合検査の完了）

## 試験

`routing_reward_waits_for_review_and_supersedes_idempotently`（task-ops、`crates/task-ops/src/routing_outcome/tests.rs`）で次を固定した。

- review 未到着: 1 件。`review_passed = None`、`reward = None`。cash 0.5・wall 1,800,000 ms・retries 1 は run に入る。
- 同じ events での再呼び出し: 0 件（冪等）。
- review 合格の到着: 1 件。`review_passed = true`、`reward = 0.825`（= 1 − 0.2·0.5 − 0.1·0.5 − 0.1·0.25）。前の outcome を `supersedes` する。
- proxy の要求（`routing_request_decided`）を結んでも追記は 0 件。全 outcome の `request_id` は None（run の結果を要求に複写しない）。
- 後の review 訂正（`review_fail`）: 1 件。`supersedes` が直前の review 合格 outcome を指し、`failure_class = review_failed`。旧 outcome は書き換えない。再呼び出しは 0 件。

## 検証

| コマンド | 結果 |
| --- | --- |
| `cargo test -p task-ops routing_reward` | 1 passed（`routing_reward_waits_for_review_and_supersedes_idempotently`） |
| `cargo nextest run -p task-core -E 'test(routing)'` | 32 run: 32 passed |
| `cargo nextest run -p task-dispatch -E 'test(review) \| test(routing) \| test(integration)'` | 159 run: 159 passed（dispatcher の既存 review 試験を含む） |
| `cargo nextest run -p task-dispatch -p task-ops -p task-core` | 1923 run: 1923 passed, 1 skipped |
| `bash scripts/dev/test-parallel.sh` | exit 0。Summary 3986 tests run: 3986 passed (1 slow), 12 skipped |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `rustfmt --edition 2024` on the changed files | 整形済み（routing_context.rs の 1 行を折り返した） |
| `cargo test -p task-ops routing_outcome -- --nocapture` | 2 passed（review・冪等・supersede と複数完了記録の合算・単価補完） |
| `cargo test -p task-dispatch a_reviewer_run_not_launched_because_a_check_failed_is_closed -- --nocapture` | 1 passed（既存 review 試験） |

## 未解決事項

- **未知モデル・token 使用量欠測の cost は None。** 既知モデルは pricing 表で補完するが、価格が未登録なら推測せず reward を None にする。
- **reopen は `task-ops::comment::reopen` だけに配線した。** `celeris/src/delivery.rs:822` の `Trigger::Reopen` は未配線。投影は冪等なので、次の完了時には届く。
- **cancel などの完了記録の無い終端は、次の完了記録までの間 outcome が追記されない。** 中断の run は `WorkerFinished`（end=Cancelled 等）が来た時点で記録されるので、これは主に task 単位の終端（完了記録を伴わない遷移）の話。
- ADR の付記（実装突き合わせ）は書いていない。ADR 本文は他の WU（audit-api・daemon-wire）と並行で変わるため、close 工程で付記する。
- `RoutingOutcome` の `request_id` は常に None。request 単位の結果は ADR §6 のとおり持たない。

## 提案

- close 工程の ADR 付記では、「完了時の 4 つの呼び出し口」と「reopen は comment::reopen のみ」を書く。
- delivery 側の reopen（`celeris/src/delivery.rs`）も同じ口を呼ぶ小さな follow-up にできる。
- pricing 表の単価と `usage.cost_usd` の出所を audit で区別する欄は、後続の audit API で検討できる。
