---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: dispatch-context
status: done-in-branch
completed: 2026-10-05
---

# Phase 3 dispatch-context: run 開始時の RoutingContext・registry 登録・features 記録

`task-dispatch::dispatcher::routing_context` を追加した。`dispatch_one` は `spawn_worker` の直前に task / WU / 担当の実効 profile / task の events から `RoutingContext` を決定的に組む（LLM なし）。

- 欄と出自（`field_provenance`）: task_id=`task.id`、work_unit_id=`work_unit.id`、run_id=`dispatch.run_id`、org_node=`task.assignee`、role・phase=`dispatch.run_role`（planner→planning、worker→implementation、reviewer→review）、harness・required_tools=`dispatch.adapter`（claude-code/codex/acp/opencode は tool calling 必須）、task_kind=`task.kind`、acceptance_criteria=`task.acceptance(+work_unit.spec.checks)`（ID は `acceptance:<i>`・`check:<i>`。`project_run_outcomes` の failed_criterion_ids と同じ形）、required_tool_ids=`org.profile.tools`（deny_tools を除く）、environment.locality/host=`task.workspace(.cluster)`、input_tokens=`estimator:chars/4(...)`、attempts=`work_unit.runs` または `task.attempts`、review_failures/check_failures=`events:attempt_history(<reopen 区間>)`（WU は `WorkUnitChecksFailed` を足す）、priority=`task.priority`。
- 取れない欄は `missing_fields`（担当なし・WU なし・profile なし・受け入れ条件なし、常に `environment.external_network` と `output_reserve`）。objective・受け入れ条件の文面・command・account・パスは入れない。
- `RoutingDecided` を書いた run だけ、同じ decision_id（`record.optimizer.decision_id`、無ければ `run:<run_id>`）で `routing_features_recorded`（stage=dispatch）を 1 回追記する。
- `Dispatcher::set_routing_context_registry`。未設定なら登録しない。設定時は run_id に結んで ttl=wall+lease_grace で登録し、ref を `RunContext.routing_context_ref`（新しい Option 欄。serde default・skip if none）に載せる。`take_running_by_run_id`（完了）と `stop_run`（打ち切り・lease 喪失）で release する。
- reviewer run（`review.rs`）への配線、daemon の registry 配線、task-worker の搬送は後続の unit（daemon-wire・worker-transport）。

## 検証（HEAD 73186c35 からの作業ツリー）

| コマンド | 結果 |
| --- | --- |
| `cargo nextest run -p task-dispatch -E 'test(routing_context) \| test(cheap_local_first) \| test(select_provider_sticks_to_the_sessions_account_over_a_better_scoring_one) \| test(lane_policy_decides)'` | 12 passed（routing_context 2、cheap_local_first_* 6 と legacy 2、select_provider_sticks… 1、lane_policy 1） |
| `UPDATE_SCHEMA=1 cargo nextest run -p task-worker -E 'test(schema)'` | 2 passed、`docs/protocol/worker-protocol.schema.json` に `routing_context_ref` を追加 |
| `cargo nextest run -p task-core -p task-api -E 'test(schema)'` | 31 passed（docs/api/v1 の食い違いなし。再生成不要） |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace -- -D warnings` | exit 0 |
| `cargo clippy -p task-dispatch -p task-worker --all-targets -- -D warnings` | exit 0 |
| `bash scripts/dev/test-parallel.sh` | exit 0、3978 passed、12 skipped |

## 未解決

- reviewer run は context を組まない（`RoutingRunRole::Reviewer` は写像と試験のみ）。
- `routing_context_ref` を adapter が LLM 要求へどう載せるかは worker-transport unit。同じ欄名を使うこと。

## 提案

- `required_tool_ids` は担当の profile の tools をそのまま使う。WU の spec に道具の要求欄ができたら、そちらを優先する。
