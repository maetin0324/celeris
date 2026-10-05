# Architecture Map

---
tasks: [01M3QEQPP31ZB29RH6YGFGTAPF, 01M3XZ5PYSTTC6GXAH8TZVRHSA]
---

1 つの変更で読むべき範囲（semantic context）を素早く絞るための索引。subsystem → 所有する crate/module →
最初に開く entry point → 関連する ADR/仕様、の対応だけを持つ。設計判断そのものは書かない
（`docs/SPEC.md` と `agent-docs/adr/` にリンクするだけで、内容はコピーしない）。実体は
`scripts/dev/check-architecture-map.py` が表内のソースパスと文書リンクの実在を検査する。

分割の詳細は entry point の module doc と各工程の記録を参照する。

## task-core — ドメインモデルと永続化（DB は正、LLM 呼び出しなし）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| Task 型・状態機械 | task-core | `crates/task-core/src/{model,transition}.rs` | [ADR-0002](../agent-docs/adr/0002-state-machine.md) |
| Event（追記専用） | task-core::store | `crates/task-core/src/store/events.rs` | [ADR-0001](../agent-docs/adr/0001-scope-and-principles.md) |
| SQLite 永続化（facade + 領域別 impl） | task-core::store | `crates/task-core/src/store/mod.rs`（module map はここの doc comment） | [ADR-0001](../agent-docs/adr/0001-scope-and-principles.md) |
| 実行計画（ExecutionPlan/WorkUnit/Run） | task-core::execution_plan | `crates/task-core/src/execution_plan.rs`（`execution_plan/{validation,scheduling}.rs`） | [ADR-0072](../agent-docs/adr/0072-task-execution-decomposition.md), [ADR-0074](../agent-docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md), [ADR-0134](../agent-docs/adr/0134-blocked-repair-replan-loop.md) |
| Execution gate（Complexity Gate の atomic/compound）と直行経路の判定（`direct_route::evaluate`・`ExecutionRouted`） | task-core::execution_gate / direct_route | `crates/task-core/src/execution_gate.rs`（`decide`・`out_of_scope_rule`）, `crates/task-core/src/direct_route.rs`（`evaluate`・`RouteDecision`） | [ADR-0072](../agent-docs/adr/0072-task-execution-decomposition.md), [ADR-0124](../agent-docs/adr/0124-atomic-direct-route.md) |
| expected/actual write-set の正規化・重なり判定・run/WU 実績 store | task-core::write_set / store | `crates/task-core/src/write_set.rs`, `crates/task-core/src/store/write_sets.rs` | [ADR-0130](../agent-docs/adr/0130-write-set-parallelism-and-behind.md) D1–D3 |
| target からの behind commits/age の観測・集約 | task-core::behind_target / store | `crates/task-core/src/behind_target.rs`, `crates/task-core/src/store/behind_targets.rs` | [ADR-0130](../agent-docs/adr/0130-write-set-parallelism-and-behind.md) D4 |
| 再帰task木（leaf/子task, gate, 上限, 生存確認） | task-core::tree | `crates/task-core/src/tree.rs`（`tree/{gate,limits,approval,liveness}.rs`） | [ADR-0079](../agent-docs/adr/0079-recursive-task-decomposition.md) |
| 継続セッション（node / WU 単位の continuation session） | task-core::node_session | `crates/task-core/src/node_session.rs`（`NodeSessionStore::{node_session_*,work_unit_session_*}`）, `crates/task-core/migrations/{0023_node_sessions,0043_work_unit_sessions}.sql` | [ADR-0054](../agent-docs/adr/0054-stateful-sessions-and-streaming-chat.md), [ADR-0140](../agent-docs/adr/0140-claude-session-resume.md) |
| execute continuation の session resume / checkpoint fallback | task-dispatch::sessions, dispatcher::continuation_session | `crates/task-dispatch/src/sessions.rs`（`decide_continuation`）, `crates/task-dispatch/src/dispatcher/continuation_session.rs`（`resolve_continuation_session`。dispatch_run.rs の WU run 開始前に呼ぶ）, `crates/task-dispatch/src/dispatcher/sinks.rs`（`StoreSink::session_resume_failed` の retire）, `[sessions] continuation_resume` | [ADR-0140](../agent-docs/adr/0140-claude-session-resume.md) |
| Browser capability の待ち状態 | task-core::browser_wait | `crates/task-core/src/browser_wait.rs`（`browser_wait/sql.rs`） | [ADR-0078](../agent-docs/adr/0078-browser-execution-capability.md), [ADR-0080](../agent-docs/adr/0080-browser-phase2-policy-broker-approval.md) |
| Browser 制御・identity・live proxy の状態 | task-core | `crates/task-core/src/{browser_control,browser_identity,browser_live,browser_isolation}.rs` | [ADR-0099](../agent-docs/adr/0099-browser-phase3-control-lease.md), [ADR-0100](../agent-docs/adr/0100-browser-phase3-live-proxy-acl.md), [ADR-0101](../agent-docs/adr/0101-browser-phase3-identity-contract.md) |
| 組織・案件・報告 | task-core::org | `crates/task-core/src/org.rs`, `crates/task-core/src/store/{org,projects}.rs` | [ADR-0033](../agent-docs/adr/0033-organization-projects-and-reports.md) |
| ワークスペース（案件×リポジトリ） | task-core::repos | `crates/task-core/src/repos.rs`, `crates/task-core/src/store/repos.rs` | [ADR-0043](../agent-docs/adr/0043-workspaces.md) |
| モデル/供給層ルーティング・quota | task-core | `crates/task-core/src/{routing,model_routing,model_policy,quota}.rs` | [ADR-0069](../agent-docs/adr/0069-routing-four-layers.md) |
| 多目的モデルルーティングの kernel（ModelProfile・SourceState・RoutingPolicy・RoutingContext・QualityEstimator・Optimizer・effective cost・trace・metrics・shadow 予約） | task-core::model_router | `crates/task-core/src/model_router/`（module map は `mod.rs` の doc comment。migrations `0048_routing_log_correlation.sql` / `0049_routing_shadow_budget.sql`） | [ADR 2026-10-04](../agent-docs/adr/2026-10-04-multi-objective-model-routing.md), [ADR-0069](../agent-docs/adr/0069-routing-four-layers.md), [ADR-0132](../agent-docs/adr/0132-provider-llm-source-split-and-cheap-qwen.md) |
| 知識ベースの置き場 | task-core::knowledge | `crates/task-core/src/knowledge.rs`（`knowledge/layout.rs`） | [ADR-0068](../agent-docs/adr/0068-knowledge-gc-and-repository-docs-maintenance.md) |
| 定期実行（cron job）・受信箱の片付け規則・日次整理 | task-core::cron / task-ops / task-dispatch | `crates/task-core/src/cron.rs`（`cron/store.rs`、migration `0046_cron_jobs.sql`）, `crates/task-ops/src/{cron_jobs,knowledge_curation}.rs`, `crates/task-ops/src/inbox.rs`（attention）, `crates/task-dispatch/src/dispatcher.rs`（tick の cron 段）, `crates/celerisctl/src/commands/curation.rs`（`curation validate`） | [ADR-0131](../agent-docs/adr/0131-cron-jobs.md)（付記 D12: 計画の事前検証と `_inbox` 上限） |
| クラスタ job の durable wait（PBS/Slurm の状態解釈） | task-core::cluster_job | `crates/task-core/src/cluster_job.rs`, `crates/task-core/migrations/0034_cluster_job_waits.sql`（schema 34）, `crates/task-core/src/store/{events,transition,task_store}.rs` | [ADR-0090](../agent-docs/adr/0090-durable-wait-for-cluster-jobs.md) |

## task-dispatch — Dispatcher（facade + 責務別子モジュール、LLM 呼び出しなし）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| Dispatcher facade（tick・起動/停止順） | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher.rs`（module map はこの doc comment） | [ADR-0082](../agent-docs/adr/0082-dispatcher-module-split.md), [記録](../agent-docs/progress/phase-P0-dispatcher.md) |
| WorkUnit の gate・準備・並列実行 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/work_units.rs`（`execution_gate_if_needed`・直行経路の記録 `execution_route_if_needed`・`direct_route_inputs`。試験は `tests/direct_route.rs`） | [ADR-0074](../agent-docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md), [ADR-0134](../agent-docs/adr/0134-blocked-repair-replan-loop.md), [ADR-0124](../agent-docs/adr/0124-atomic-direct-route.md), [WU の checks の指針](../agent-docs/guides/work-unit-checks.md) |
| write-set 予約 gate・Git 実績記録・behind 観測・stale sync 優先 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/{write_set_gate,write_set_record,behind_target,stale_priority}.rs` | [ADR-0130](../agent-docs/adr/0130-write-set-parallelism-and-behind.md) D2–D5 |
| Browser backend の適合判定・fallback 候補 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/{dispatch_run,worker_task}.rs` | [ADR-0106](../agent-docs/adr/0106-browser-phase4-conformance-dispatch.md), [ADR-0107](../agent-docs/adr/0107-browser-fallback-candidate-preparation.md) |
| 木の子task の gate・一括作成 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/tree_units.rs` | [ADR-0079](../agent-docs/adr/0079-recursive-task-decomposition.md) |
| 委譲/承認の子task 作成 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/child_tasks.rs` | [ADR-0005](../agent-docs/adr/0005-phase3-dispatch-and-worker.md) |
| provider/account 選択・quota 見積り | task-dispatch | `crates/task-dispatch/src/{dispatcher/provider_select.rs,dispatcher/quota_book.rs,accounts.rs}` | [ADR-0069](../agent-docs/adr/0069-routing-four-layers.md), [ADR-0132 付記（cheap lane のローカル優先）](../agent-docs/adr/0132-provider-llm-source-split-and-cheap-qwen.md) |
| 多目的ルーティングの dispatch 側（enforce 選択・defer、RoutingContext 抽出と registry、decision shadow、軌跡 escalation） | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/{routing_enforce,routing_context,routing_shadow}.rs`, `crates/task-dispatch/src/dispatcher/dispatch_run.rs`（lane 決定と outcome 追記） | [ADR 2026-10-04](../agent-docs/adr/2026-10-04-multi-objective-model-routing.md) §2・§5・§7.1 |
| worker 起動・完了処理 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/{worker_task,worker_finish}.rs` | [ADR-0003](../agent-docs/adr/0003-worker-protocol.md) |
| planner/reviewer の起動・判定（review 前 target 同期・reviewed/merge candidate SHA 記録・衝突時の IntegrationRepair 起票・stale の再同期〈attempts 不変〉） | task-dispatch | `crates/task-dispatch/src/{dispatcher/planner_flow.rs,dispatcher/review_spawn.rs,dispatcher/review_verdict.rs,review.rs}` | [ADR-0076](../agent-docs/adr/0076-planner-reviewer-quota-roles.md), [ADR-0118](../agent-docs/adr/0118-review-target-sync-and-merge-candidate.md), [ADR-0120](../agent-docs/adr/0120-pre-review-sync-integration-repair.md), [ADR-0124](../agent-docs/adr/0124-atomic-direct-route.md)（直行経路は planner を起こさず `crates/task-dispatch/src/dispatcher/dispatch_run.rs` の `is_planner_dispatch` で分岐） |
| cluster/ssh master・接続監視 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/cluster.rs` | [ADR-0018](../agent-docs/adr/0018-remote-clusters-over-ssh.md) |
| クラスタ job の poll・再開（`qstat -xf`/`sacct`） | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/cluster_job_wait.rs` | [ADR-0090](../agent-docs/adr/0090-durable-wait-for-cluster-jobs.md) |
| CoS run の並列度の例外（`is_cos_run`） | task-dispatch::capacity | `crates/task-dispatch/src/capacity.rs` | [ADR-0089](../agent-docs/adr/0089-cos-runs-bypass-concurrency.md) |
| run 途中のイベントの sink（worker/Reviewer） | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/sinks.rs` | [ADR-0082](../agent-docs/adr/0082-dispatcher-module-split.md) |
| scratch/disk guard の後片付け | task-dispatch | `crates/task-dispatch/src/{dispatcher/housekeeping.rs,scratch_gc.rs}` | [ADR-0075](../agent-docs/adr/0075-tiered-build-cache.md) |
| 工程統合・途中報告（target への rebase 同期、IntegrationRepair の完了・rollback・従来経路への復帰） | task-dispatch / task-ops | `crates/task-dispatch/src/{dispatcher/phase_integration.rs,dispatcher/worker_finish.rs,checkpoint.rs,reports.rs}`, `crates/task-ops/src/changes.rs`（`sync_onto_target`） | [ADR-0074](../agent-docs/adr/0074-parallel-work-units-checkpoints-milestones-quota.md), [ADR-0118](../agent-docs/adr/0118-review-target-sync-and-merge-candidate.md), [ADR-0120](../agent-docs/adr/0120-pre-review-sync-integration-repair.md), [ADR-0140](../agent-docs/adr/0140-claude-session-resume.md) |
| 並列取り込みの自動解消（記録・番号・生成物の分類と resolver、統合の依頼の型） | task-dispatch::auto_resolve | `crates/task-dispatch/src/auto_resolve.rs`（`auto_resolve/{classify,records,renumber,generated}.rs`）、段の統合は `crates/task-dispatch/src/integration.rs` | [2026-10-02 並列取り込みの自動解消](../agent-docs/adr/2026-10-02-parallel-integration-auto-resolve.md) |
| 持ち主の居ない `running` の `runs` 行と手元のレビューの取りこぼしの回収 | task-dispatch::dispatcher | `crates/task-dispatch/src/{dispatcher/ownerless_runs.rs,orphan.rs}`（判定の規則は orphan.rs の ownerless_run_decision） | [2026-10-03 lease を持たない run の行](../agent-docs/adr/2026-10-03-ownerless-running-runs.md) |
| 孤児run の回収・承認・policy | task-dispatch | `crates/task-dispatch/src/{orphan,approvals,policy,sessions,undeclared_artifacts}.rs` | [ADR-0005](../agent-docs/adr/0005-phase3-dispatch-and-worker.md), [ADR-0054](../agent-docs/adr/0054-stateful-sessions-and-streaming-chat.md), [ADR-0140](../agent-docs/adr/0140-claude-session-resume.md) |

## task-worker — worker プロトコルと adapter（1 run = 1 プロセス起動）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| RunRequest/RunContext 境界 | task-worker::protocol | `crates/task-worker/src/protocol.rs` | [ADR-0003](../agent-docs/adr/0003-worker-protocol.md) |
| Adapter 選択 | task-worker::adapter | `crates/task-worker/src/adapter.rs` | [ADR-0003](../agent-docs/adr/0003-worker-protocol.md) |
| Claude Code adapter（CLI起動 + prompt、run 内の再探索重複・resume の印を `Usage.duplicate_reads` / `session_resumed` へ） | task-worker::claude_code | `crates/task-worker/src/claude_code.rs`（`claude_code/prompt.rs`、`ExplorationTracker`。直行経路の節 `direct_route_section` は ADR-0124 D4、`RunContext.direct_route`） | [ADR-0140 session resume](../agent-docs/adr/0140-claude-session-resume.md), [ADR-0124 direct route](../agent-docs/adr/0124-atomic-direct-route.md) |
| Codex / ACP adapter | task-worker | `crates/task-worker/src/{codex,acp}.rs` | [ADR-0008](../agent-docs/adr/0008-phase6-approval-gate-and-codex-adapter.md), [ADR-0026](../agent-docs/adr/0026-acp-worker-adapter.md), [Phase 6 記録](../agent-docs/progress/phase-001-050.md) |
| PaperQA2 / Local Deep Research adapter | task-worker | `crates/task-worker/src/{paperqa,local_deep_research}.rs`（`paperqa/render.rs`） | [ADR-0027](../agent-docs/adr/0027-task-genres-and-research-harness.md) |
| Browser capability（policy/credential 越境） | task-worker::browser | `crates/task-worker/src/browser{,_credential,_policy}.rs` | [ADR-0078](../agent-docs/adr/0078-browser-execution-capability.md), [ADR-0080](../agent-docs/adr/0080-browser-phase2-policy-broker-approval.md) |
| Browser 隔離 runtime・supervisor・egress | task-worker::browser_runtime | `crates/task-worker/src/{browser_runtime,browser_supervisor,browser_egress}.rs` | [ADR-0102](../agent-docs/adr/0102-browser-phase4-isolation-injection-routing.md), [ADR-0105](../agent-docs/adr/0105-browser-p4a-same-uid-bwrap-runtime.md), [ADR-0108](../agent-docs/adr/0108-browser-p4a-relay-supervisor-launch-restore.md) |
| Browser CDP 注入・共有・操作 gate | task-worker | `crates/task-worker/src/{browser_cdp_sink,browser_shared_cdp,browser_action}.rs` | [ADR-0109](../agent-docs/adr/0109-browser-p4b-injection-ipc-cdp-sink.md), [ADR-0110](../agent-docs/adr/0110-browser-p4b-h3-shared-cdp-trusted-selector.md), [ADR-0113](../agent-docs/adr/0113-browser-p3c-control-gate-action-server.md) |
| Browser 権限分離 launcher（userns owner 分離・固定 IPC） | task-worker::browser_launcher | `crates/task-worker/src/browser_launcher/mod.rs`、`crates/task-worker/src/bin/celeris-browser-launcher.rs` | [ADR-0115](../agent-docs/adr/0115-browser-ptrace-owner-ns-launcher.md), [ADR-0116](../agent-docs/adr/0116-browser-launcher-implementation.md), [ADR-0138](../agent-docs/adr/0138-browser-prod-admission-confidential-release.md) |
| scratch/build cache（GC は別責務） | task-worker | `crates/task-worker/src/{scratch.rs,scratch/gc.rs,build_cache.rs,tiered.rs}` | [ADR-0075](../agent-docs/adr/0075-tiered-build-cache.md) |
| ワークスペース/worktree 準備 | task-worker | `crates/task-worker/src/{workspace.rs,local_worktree.rs}` | [ADR-0043](../agent-docs/adr/0043-workspaces.md) |
| worker の run から DB を読み取り専用（namespace・`launch`） | task-worker::db_guard | `crates/task-worker/src/db_guard.rs` | [ADR-0095](../agent-docs/adr/0095-worker-runs-see-the-db-read-only.md) |

## task-ops — CLI/API から呼ぶ操作層（taskctl・task-api 共有）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| Task/Project の一覧・詳細 DTO | task-ops::view | `crates/task-ops/src/view.rs`（TaskDetail の `execution.route` は ADR-0124 D3） | [ADR-0004](../agent-docs/adr/0004-taskctl-cli.md), [ADR-0124](../agent-docs/adr/0124-atomic-direct-route.md) |
| Event からの replay/整合性検査 | task-ops::replay | `crates/task-ops/src/replay.rs` | [ADR-0001](../agent-docs/adr/0001-scope-and-principles.md) |
| routing 監査（`GET /api/v1/tasks/{id}/routing` の projection）と offline replay（`routing export` / `routing evaluate`） | task-ops::routing_audit, routing_replay | `crates/task-ops/src/{routing_audit,routing_replay}.rs`, `crates/celerisctl/src/commands/routing.rs` | [ADR 2026-10-04](../agent-docs/adr/2026-10-04-multi-objective-model-routing.md) §6・§7.2・§9 |
| 知識ベース操作（KB・skill） | task-ops::knowledge | `crates/task-ops/src/knowledge.rs` | [ADR-0068](../agent-docs/adr/0068-knowledge-gc-and-repository-docs-maintenance.md) |
| 決定の要求・plan/phase gate | task-ops | `crates/task-ops/src/{decision,plan_gate,phase_gate}.rs` | [ADR-0079](../agent-docs/adr/0079-recursive-task-decomposition.md) |
| 再帰task木の操作（adopt/plan/view） | task-ops::tree | `crates/task-ops/src/tree{,_adopt,_plan,_view}.rs` | [ADR-0079](../agent-docs/adr/0079-recursive-task-decomposition.md) |
| worker の run が宣言した後続 task（案件・repos の継承、`followups.json`） | task-ops::followup | `crates/task-ops/src/followup.rs`, `crates/task-dispatch/src/dispatcher/followups.rs` | [ADR-0098](../agent-docs/adr/0098-worker-created-tasks-inherit-the-origin-project.md) |
| git 差分・変更取り込み判定 | task-ops::changes | `crates/task-ops/src/changes.rs` | [ADR-0043](../agent-docs/adr/0043-workspaces.md) |
| 受信箱（人の判断）と通知（知らせ）の 2 系統・Discord 送り出し | task-ops::inbox, task-core::notify, celeris::notify | `crates/task-ops/src/inbox.rs`, `crates/task-core/src/notify.rs`, `crates/celeris/src/notify.rs` | [ADR-0133](../agent-docs/adr/0133-inbox-and-notifications.md), [ADR-0037](../agent-docs/adr/0037-discord-notifications.md) |
| ドキュメント整備の自動化 | task-ops::docs_maintenance | `crates/task-ops/src/docs_maintenance.rs` | [ADR-0068](../agent-docs/adr/0068-knowledge-gc-and-repository-docs-maintenance.md) |

## task-api — HTTP API（`/api/v1`）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| router（facade、endpoint の行と順序） | task-api::handlers | `crates/task-api/src/handlers.rs` | [ADR-0013](../agent-docs/adr/0013-taskd-api-and-gui-foundations.md) |
| domain 別 handler（tasks/projects/accounts/…） | task-api::handlers | `crates/task-api/src/handlers/*.rs` | [ADR-0013](../agent-docs/adr/0013-taskd-api-and-gui-foundations.md) |
| SSE（イベント購読） | task-api::sse | `crates/task-api/src/sse.rs` | [ADR-0013](../agent-docs/adr/0013-taskd-api-and-gui-foundations.md) |
| 決定・木・実行系のサブAPI | task-api | `crates/task-api/src/{decisions,tree,execution}.rs` | [ADR-0079](../agent-docs/adr/0079-recursive-task-decomposition.md) |
| routing catalog API（`GET /api/v1/llm/routing/catalog`、redacted） | task-api::routing_catalog | `crates/task-api/src/routing_catalog.rs` | [ADR 2026-10-04](../agent-docs/adr/2026-10-04-multi-objective-model-routing.md) §9 |
| Browser 制御・identity・Live View API | task-api | `crates/task-api/src/{browser_control,browser_identity,browser_live}.rs` | [ADR-0099](../agent-docs/adr/0099-browser-phase3-control-lease.md), [ADR-0100](../agent-docs/adr/0100-browser-phase3-live-proxy-acl.md), [ADR-0101](../agent-docs/adr/0101-browser-phase3-identity-contract.md) |

## celeris — daemon 本体（配線・設定・自己更新）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| 起動順・配線（facade） | celeris::lib | `crates/celeris/src/lib.rs`（`daemon/` 配下は module doc） | [SPEC](SPEC.md), [ADR-0001](../agent-docs/adr/0001-scope-and-principles.md) |
| tick ループ・裏方処理 | celeris::daemon | `crates/celeris/src/daemon/tick_loop.rs` | [SPEC](SPEC.md), [ADR-0001](../agent-docs/adr/0001-scope-and-principles.md) |
| 設定（TOML → subsystem 別型・検証） | celeris::config | `crates/celeris/src/config/mod.rs`（module map はここの doc comment） | [ADR-0001](../agent-docs/adr/0001-scope-and-principles.md) |
| 多目的ルーティングの daemon 配線（`[model_routing]` の config/検証/reload、shadow budget の共有 DB、estimator sidecar の opt-in 制御） | celeris::config::model_routing, celeris::daemon | `crates/celeris/src/config/model_routing.rs`, `crates/celeris/src/daemon/{routing_shadow,routing_sidecar}.rs` | [ADR 2026-10-04](../agent-docs/adr/2026-10-04-multi-objective-model-routing.md) §9・§10 |
| self-deploy（release/verify/handoff） | celeris | `crates/celeris/src/{instance.rs,releases.rs,config/selfdeploy.rs}` | [selfdeploy.md](ops/selfdeploy.md) |
| Knowledge GC・doc gardener | celeris | `crates/celeris/src/{knowledge_gc,knowledge_maint,doc_gardener}.rs` | [ADR-0068](../agent-docs/adr/0068-knowledge-gc-and-repository-docs-maintenance.md), [ADR-0131 D6](../agent-docs/adr/0131-cron-jobs.md)（日次整理との関係） |
| cluster/accounts 管理の裏方 | celeris | `crates/celeris/src/{cluster_admin,accounts_admin}.rs` | [ADR-0017](../agent-docs/adr/0017-account-management-from-gui.md), [ADR-0018](../agent-docs/adr/0018-remote-clusters-over-ssh.md) |

## 周辺 crate と結合テスト

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| `celerisctl`（CLI。migration をしない `open_client`） | celerisctl::main | `crates/celerisctl/src/main.rs` | [ADR-0004](../agent-docs/adr/0004-taskctl-cli.md), [ADR-0095](../agent-docs/adr/0095-worker-runs-see-the-db-read-only.md) |
| `llm-proxy`（ローカル LLM 供給プロキシ） | llm-proxy::server | `crates/llm-proxy/src/server.rs` | [ADR-0053](../agent-docs/adr/0053-llm-source-proxy.md), [ADR-0132](../agent-docs/adr/0132-provider-llm-source-split-and-cheap-qwen.md) |
| llm-proxy の多目的ルーティング側（state 選択・fallback/breaker、shadow queue、estimator sidecar client、routing context 解決） | llm-proxy | `crates/llm-proxy/src/{selection_state,fallback,shadow,shadow_budget,estimator_sidecar,estimator_shadow,routing_context,legacy_catalog}.rs` | [ADR 2026-10-04](../agent-docs/adr/2026-10-04-multi-objective-model-routing.md) §2・§4・§5・§7 |
| `celeris-mcp`（外部エージェント向け MCP） | celeris-mcp::rpc | `crates/celeris-mcp/src/rpc.rs` | [ADR-0056](../agent-docs/adr/0056-mcp-server.md) |
| `celeris-credentiald`（credential broker） | celeris-credentiald::lib | `crates/celeris-credentiald/src/lib.rs` | [ADR-0080](../agent-docs/adr/0080-browser-phase2-policy-broker-approval.md) |
| `tests/e2e`（daemon 起動を伴う結合テスト） | e2e | `tests/e2e/tests/scenarios.rs` | [ADR-0010](../agent-docs/adr/0010-phase7-hardening.md) |

## GUI（`gui/`、置き換え予定）と web/（新 SPA、設計段階）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| Task detail 画面（loader/action + タブ部品） | gui::app | `gui/app/{routes/tasks.$id.tsx,celeris/task-detail.server.ts,components/task-detail}`（経路の表示は ADR-0124 D3） | [ADR-0013](../agent-docs/adr/0013-taskd-api-and-gui-foundations.md), [ADR-0020](../agent-docs/adr/0020-single-repository-with-the-gui.md), [ADR-0124](../agent-docs/adr/0124-atomic-direct-route.md) |
| Browser 操作・本人承認・Live View | gui::app | `gui/app/{routes/browser.control.ts,components/BrowserRunsPanel.tsx,celeris/browser-live.server.ts}` | [ADR-0080](../agent-docs/adr/0080-browser-phase2-policy-broker-approval.md), [ADR-0099](../agent-docs/adr/0099-browser-phase3-control-lease.md), [ADR-0100](../agent-docs/adr/0100-browser-phase3-live-proxy-acl.md) |
| 組織図・アカウント・案件の画面 | gui::routes | `gui/app/routes/{org,accounts,projects.$id}.tsx` | [ADR-0013](../agent-docs/adr/0013-taskd-api-and-gui-foundations.md), [ADR-0020](../agent-docs/adr/0020-single-repository-with-the-gui.md) |
| 新 SPA（旧 GUI を daemon 遅延から切り離す） | web (設計中) | 未着手（Phase 0 は設計のみ） | [ADR-0081](../agent-docs/adr/0081-web-spa-frontend.md), [implementation plan](../agent-docs/web/implementation-plan.md) |

## この文書の作り方・保ち方

- 出典: `wu/*/artifacts/map.md`（p0-config・p1-handlers・p1-lib・p1-gui）、`wu/*/artifacts/p2.md`（p2-core・p2-worker）、
  `agent-docs/progress/phase-P0-dispatcher.md`、および HEAD の実ソース（各 crate の `src/` 直下と module doc）。
- 行が増えすぎたら「主要 subsystem」の粒度を保つために統合する（1 crate 1 行までは削らない）。200 行を超えたら周辺 crate の表から削るのではなく、
  変更頻度の低い行をまとめる。
- 新しい crate や大きな module 分割をしたら、この表の対応行を 1 行更新する（新しい ADR/設計文書があればリンクを足す。内容はコピーしない）。
- パスの実在は `scripts/dev/check-architecture-map.py` で機械的に検査する（このファイルを手で直したら実行すること）。
