# task ごとの browser origin 要求

---
tasks: [01M470CJRXMPWS39S14PN7XPFP]
---

## 配置と検証

`Task.requirements.browser.allowed_domains` を正本とし、`NewTaskSpec`、`PlanUnitSpec`、`ExecutionChildSpec`、`ConsoleAction::CreateTask` に同じ欄を設けた。旧 task は欄を持たず読める。browser-enabled の新規 task は非空かつ明示的な origin を要する。構文は origin 葉の `AllowedOrigin::parse`、親子包含は `origin_covers` を使い、scheme・host・port を判定する。grant との交差は作成時にはしない。

## 作成経路の走査

- API `POST /tasks`: `task-api/src/handlers/tasks.rs::create_task` → `task-ops/src/add.rs::create_task_with_roles`。CLI の `celerisctl add` も同じ関数を使う。
- CoS 起票: `task-core/src/console_action.rs::ConsoleAction::CreateTask` → `task-ops/src/actions.rs::create_task_action` → `create_task_with_roles`。
- execution plan の task unit: `task-dispatch/src/dispatcher/tree_units.rs` の 3 呼出箇所 → `task-ops/src/tree.rs::build_child_task`。子の skills 確定後に検証する。
- execution plan/2 の children: `task-ops/src/delegate.rs::plan_children` → `task-core/src/delegate.rs::materialize_delegated_logging`。子の skills 確定後に検証する。
- 旧 plan.json の children: `task-core/src/plan.rs::NewTask` → `materialize_logging` → `TaskStore::complete_plan`。requirements を子に写し、保存時の検証を通る。
- 旧 delegate 形式: `task-core/src/delegate.rs::materialize_delegated_logging` → `TaskStore::delegate_children`。保存時の検証を通る。
- 直接保存: `TaskStore::create_task` / `complete_plan` / `delegate_children` は `SqliteStore::insert_tx` に合流し、イベント追記前に同じ決定的検証を行う。approval 子は browser skill を持たない。
- 編集: `PATCH /tasks/{id}` で browser-enabled skill を後付けする際も、`task-ops/src/edit.rs` が同じ検証で拒否する。

## 結果

- `browser_allowed_domains_` は API 2、task-core 7、task-ops 3 件を用意した。API は欠落・空・不正 origin を 422、CoS は failed action、plan unit と execution-plan/2 の child は欠落・空・不正・親超えを拒否した。store では親の wildcard に含まれる単一 origin の子を受け入れることも固定した。
- `UPDATE_SCHEMA=1 cargo test -p task-core schema`、`UPDATE_SCHEMA=1 cargo test -p task-worker committed_schema_matches_generated`、`UPDATE_SCHEMA=1 cargo test -p task-api schema::tests::committed_schema_matches_generated` で schema を再生成し、更新変数なしの `committed_schema_matches_generated` と `node web/scripts/gen-types.mjs --check` が通過。
- `cargo test -p task-core -p task-ops -q` は task-core 726 件、task-ops 499 件が通過。`UPDATE_SCHEMA=1 cargo test -p task-api -q` も通過。`cargo clippy --workspace -- -D warnings`、web typecheck、web test（vitest 361 件・server 52 件）も通過。
- 全体の `test-parallel.sh` は開始時点の browser fixture と worker protocol schema が旧状態だったため失敗した。fixture と schema は更新後に対象試験で確認した。一方、origin 葉の host→origin 移行に伴う `task-worker::browser_policy::tests::prepared_file_is_hash_bound_nonempty_default_deny` と `browser::tests::credential_request_origin_outside_effective_domain_is_denied` は、後続の enforce 葉が browser policy と credential 判定を origin 対応へ変えるまで残る。task-req の検証には含めない。
- 前回の範囲 check で `tests/e2e/tests/scenarios.rs` と `tests/e2e/tests/worker_db_read_only.rs` が許可範囲外と分かったため、この枝では元に戻した。両ファイルの `Task` リテラルに `requirements: Default::default()` を追加する必要があり、全体 e2e のコンパイルは統合工程で対応する。
- 再実行: 指定の `browser_allowed_domains_` 件数・task-core/task-api/task-dispatch 試験 check は exit 0（件数 9、task-core 7 件、task-api 2 件、task-dispatch 0 件）。`cargo test -p task-ops browser_allowed_domains_ -q` は 3 件通過。指定の差分範囲 check、`cargo fmt --all --check`、`node web/scripts/gen-types.mjs --check` も exit 0。
