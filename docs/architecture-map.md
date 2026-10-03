# Architecture Map

---
tasks: [01M3QEQPP31ZB29RH6YGFGTAPF]
---

1 つの変更で読むべき範囲（semantic context）を素早く絞るための索引。subsystem → 所有する crate/module →
最初に開く entry point → 関連する ADR/設計節、の対応だけを持つ。設計判断そのものは書かない
（`docs/DESIGN.md` と `docs/adr/` にリンクするだけで、内容はコピーしない）。実体は
`scripts/dev/check-architecture-map.py` が表内のソースパスと文書リンクの実在を検査する。

分割の詳細は entry point の module doc と各工程の記録を参照する。

## task-core — ドメインモデルと永続化（DB は正、LLM 呼び出しなし）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| Task 型・状態機械 | task-core | `crates/task-core/src/{model,transition}.rs` | [DESIGN §4.1–4.2](DESIGN.md#41-task) |
| Event（追記専用） | task-core::store | `crates/task-core/src/store/events.rs` | [DESIGN §4.3](DESIGN.md#43-event追記専用) |
| SQLite 永続化（facade + 領域別 impl） | task-core::store | `crates/task-core/src/store/mod.rs`（module map はここの doc comment） | [DESIGN §5.1](DESIGN.md#51-store-task-core) |
| 実行計画（ExecutionPlan/WorkUnit/Run） | task-core::execution_plan | `crates/task-core/src/execution_plan.rs`（`execution_plan/{validation,scheduling}.rs`） | [ADR-0072](adr/0072-task-execution-decomposition.md), [ADR-0074](adr/0074-parallel-work-units-checkpoints-milestones-quota.md) |
| 再帰task木（leaf/子task, gate, 上限, 生存確認） | task-core::tree | `crates/task-core/src/tree.rs`（`tree/{gate,limits,approval,liveness}.rs`） | [ADR-0079](adr/0079-recursive-task-decomposition.md) |
| Browser capability の待ち状態 | task-core::browser_wait | `crates/task-core/src/browser_wait.rs`（`browser_wait/sql.rs`） | [ADR-0078](adr/0078-browser-execution-capability.md), [ADR-0080](adr/0080-browser-phase2-policy-broker-approval.md) |
| Browser 制御・identity・live proxy の状態 | task-core | `crates/task-core/src/{browser_control,browser_identity,browser_live,browser_isolation}.rs` | [ADR-0099](adr/0099-browser-phase3-control-lease.md), [ADR-0100](adr/0100-browser-phase3-live-proxy-acl.md), [ADR-0101](adr/0101-browser-phase3-identity-contract.md) |
| 組織・案件・報告 | task-core::org | `crates/task-core/src/org.rs`, `crates/task-core/src/store/{org,projects}.rs` | [ADR-0033](adr/0033-organization-projects-and-reports.md) |
| ワークスペース（案件×リポジトリ） | task-core::repos | `crates/task-core/src/repos.rs`, `crates/task-core/src/store/repos.rs` | [ADR-0043](adr/0043-workspaces.md) |
| モデル/供給層ルーティング・quota | task-core | `crates/task-core/src/{routing,model_routing,model_policy,quota}.rs` | [ADR-0069](adr/0069-routing-four-layers.md) |
| 知識ベースの置き場 | task-core::knowledge | `crates/task-core/src/knowledge.rs`（`knowledge/layout.rs`） | [ADR-0068](adr/0068-knowledge-gc-and-repository-docs-maintenance.md) |
| クラスタ job の durable wait（PBS/Slurm の状態解釈） | task-core::cluster_job | `crates/task-core/src/cluster_job.rs`, `crates/task-core/migrations/0034_cluster_job_waits.sql`（schema 34）, `crates/task-core/src/store/{events,transition,task_store}.rs` | [ADR-0090](adr/0090-durable-wait-for-cluster-jobs.md) |

## task-dispatch — Dispatcher（facade + 責務別子モジュール、LLM 呼び出しなし）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| Dispatcher facade（tick・起動/停止順） | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher.rs`（module map はこの doc comment） | [ADR-0082](adr/0082-dispatcher-module-split.md), [記録](progress/phase-P0-dispatcher.md) |
| WorkUnit の gate・準備・並列実行 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/work_units.rs` | [ADR-0074](adr/0074-parallel-work-units-checkpoints-milestones-quota.md) |
| Browser backend の適合判定・fallback 候補 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/{dispatch_run,worker_task}.rs` | [ADR-0106](adr/0106-browser-phase4-conformance-dispatch.md), [ADR-0107](adr/0107-browser-fallback-candidate-preparation.md) |
| 木の子task の gate・一括作成 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/tree_units.rs` | [ADR-0079](adr/0079-recursive-task-decomposition.md) |
| 委譲/承認の子task 作成 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/child_tasks.rs` | [DESIGN §5.2](DESIGN.md#52-dispatcher-task-dispatch) |
| provider/account 選択・quota 見積り | task-dispatch | `crates/task-dispatch/src/{dispatcher/provider_select.rs,dispatcher/quota_book.rs,accounts.rs}` | [ADR-0069](adr/0069-routing-four-layers.md) |
| worker 起動・完了処理 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/{worker_task,worker_finish}.rs` | [DESIGN §5.3](DESIGN.md#53-worker-protocol-task-worker) |
| planner/reviewer の起動・判定 | task-dispatch | `crates/task-dispatch/src/{dispatcher/planner_flow.rs,dispatcher/review_spawn.rs,dispatcher/review_verdict.rs,review.rs}` | [ADR-0076](adr/0076-planner-reviewer-quota-roles.md) |
| cluster/ssh master・接続監視 | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/cluster.rs` | [ADR-0018](adr/0018-remote-clusters-over-ssh.md) |
| クラスタ job の poll・再開（`qstat -xf`/`sacct`） | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/cluster_job_wait.rs` | [ADR-0090](adr/0090-durable-wait-for-cluster-jobs.md) |
| CoS run の並列度の例外（`is_cos_run`） | task-dispatch::capacity | `crates/task-dispatch/src/capacity.rs` | [ADR-0089](adr/0089-cos-runs-bypass-concurrency.md) |
| run 途中のイベントの sink（worker/Reviewer） | task-dispatch::dispatcher | `crates/task-dispatch/src/dispatcher/sinks.rs` | [ADR-0082](adr/0082-dispatcher-module-split.md) |
| scratch/disk guard の後片付け | task-dispatch | `crates/task-dispatch/src/{dispatcher/housekeeping.rs,scratch_gc.rs}` | [ADR-0075](adr/0075-tiered-build-cache.md) |
| 工程統合・途中報告 | task-dispatch | `crates/task-dispatch/src/{dispatcher/phase_integration.rs,checkpoint.rs,reports.rs}` | [ADR-0074](adr/0074-parallel-work-units-checkpoints-milestones-quota.md) |
| 孤児run の回収・承認・policy | task-dispatch | `crates/task-dispatch/src/{orphan,approvals,policy,sessions,undeclared_artifacts}.rs` | [DESIGN §5.2](DESIGN.md#52-dispatcher-task-dispatch) |

## task-worker — worker プロトコルと adapter（1 run = 1 プロセス起動）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| RunRequest/RunContext 境界 | task-worker::protocol | `crates/task-worker/src/protocol.rs` | [DESIGN §5.3](DESIGN.md#53-worker-protocol-task-worker) |
| Adapter 選択 | task-worker::adapter | `crates/task-worker/src/adapter.rs` | [DESIGN §5.4](DESIGN.md#54-adapters) |
| Claude Code adapter（CLI起動 + prompt） | task-worker::claude_code | `crates/task-worker/src/claude_code.rs`（`claude_code/prompt.rs`） | [DESIGN §5.4](DESIGN.md#54-adapters) |
| Codex / ACP adapter | task-worker | `crates/task-worker/src/{codex,acp}.rs` | [DESIGN §5.4](DESIGN.md#54-adapters), [Phase 6 記録](progress/phase-001-050.md) |
| PaperQA2 / Local Deep Research adapter | task-worker | `crates/task-worker/src/{paperqa,local_deep_research}.rs`（`paperqa/render.rs`） | [DESIGN §5.4](DESIGN.md#54-adapters) |
| Browser capability（policy/credential 越境） | task-worker::browser | `crates/task-worker/src/browser{,_credential,_policy}.rs` | [ADR-0078](adr/0078-browser-execution-capability.md), [ADR-0080](adr/0080-browser-phase2-policy-broker-approval.md) |
| Browser 隔離 runtime・supervisor・egress | task-worker::browser_runtime | `crates/task-worker/src/{browser_runtime,browser_supervisor,browser_egress}.rs` | [ADR-0102](adr/0102-browser-phase4-isolation-injection-routing.md), [ADR-0105](adr/0105-browser-p4a-same-uid-bwrap-runtime.md), [ADR-0108](adr/0108-browser-p4a-relay-supervisor-launch-restore.md) |
| Browser CDP 注入・共有・操作 gate | task-worker | `crates/task-worker/src/{browser_cdp_sink,browser_shared_cdp,browser_action}.rs` | [ADR-0109](adr/0109-browser-p4b-injection-ipc-cdp-sink.md), [ADR-0110](adr/0110-browser-p4b-h3-shared-cdp-trusted-selector.md), [ADR-0113](adr/0113-browser-p3c-control-gate-action-server.md) |
| Browser 権限分離 launcher（userns owner 分離・固定 IPC） | task-worker::browser_launcher | `crates/task-worker/src/browser_launcher.rs`、`crates/task-worker/src/bin/celeris-browser-launcher.rs`（実装予定） | [ADR-0115](adr/0115-browser-ptrace-owner-ns-launcher.md), [ADR-0116](adr/0116-browser-launcher-implementation.md), [ADR-0138](adr/0138-browser-prod-admission-confidential-release.md)（本番 admission） |
| scratch/build cache（GC は別責務） | task-worker | `crates/task-worker/src/{scratch.rs,scratch/gc.rs,build_cache.rs,tiered.rs}` | [ADR-0075](adr/0075-tiered-build-cache.md) |
| ワークスペース/worktree 準備 | task-worker | `crates/task-worker/src/{workspace.rs,local_worktree.rs}` | [ADR-0043](adr/0043-workspaces.md) |
| worker の run から DB を読み取り専用（namespace・`launch`） | task-worker::db_guard | `crates/task-worker/src/db_guard.rs` | [ADR-0095](adr/0095-worker-runs-see-the-db-read-only.md) |

## task-ops — CLI/API から呼ぶ操作層（taskctl・task-api 共有）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| Task/Project の一覧・詳細 DTO | task-ops::view | `crates/task-ops/src/view.rs` | [DESIGN §5.9](DESIGN.md#59-cli-taskctl) |
| Event からの replay/整合性検査 | task-ops::replay | `crates/task-ops/src/replay.rs` | [DESIGN §4.3](DESIGN.md#43-event追記専用) |
| 知識ベース操作（KB・skill） | task-ops::knowledge | `crates/task-ops/src/knowledge.rs` | [ADR-0068](adr/0068-knowledge-gc-and-repository-docs-maintenance.md) |
| 決定の要求・plan/phase gate | task-ops | `crates/task-ops/src/{decision,plan_gate,phase_gate}.rs` | [ADR-0079](adr/0079-recursive-task-decomposition.md) |
| 再帰task木の操作（adopt/plan/view） | task-ops::tree | `crates/task-ops/src/tree{,_adopt,_plan,_view}.rs` | [ADR-0079](adr/0079-recursive-task-decomposition.md) |
| worker の run が宣言した後続 task（案件・repos の継承、`followups.json`） | task-ops::followup | `crates/task-ops/src/followup.rs`, `crates/task-dispatch/src/dispatcher/followups.rs` | [ADR-0098](adr/0098-worker-created-tasks-inherit-the-origin-project.md) |
| git 差分・変更取り込み判定 | task-ops::changes | `crates/task-ops/src/changes.rs` | [ADR-0043](adr/0043-workspaces.md) |
| ドキュメント整備の自動化 | task-ops::docs_maintenance | `crates/task-ops/src/docs_maintenance.rs` | [ADR-0068](adr/0068-knowledge-gc-and-repository-docs-maintenance.md) |

## task-api — HTTP API（`/api/v1`）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| router（facade、endpoint の行と順序） | task-api::handlers | `crates/task-api/src/handlers.rs` | [DESIGN §5.10](DESIGN.md#510-api-層task-apiadr-0013) |
| domain 別 handler（tasks/projects/accounts/…） | task-api::handlers | `crates/task-api/src/handlers/*.rs` | [DESIGN §5.10](DESIGN.md#510-api-層task-apiadr-0013) |
| SSE（イベント購読） | task-api::sse | `crates/task-api/src/sse.rs` | [DESIGN §5.10](DESIGN.md#510-api-層task-apiadr-0013) |
| 決定・木・実行系のサブAPI | task-api | `crates/task-api/src/{decisions,tree,execution}.rs` | [ADR-0079](adr/0079-recursive-task-decomposition.md) |
| Browser 制御・identity・Live View API | task-api | `crates/task-api/src/{browser_control,browser_identity,browser_live}.rs` | [ADR-0099](adr/0099-browser-phase3-control-lease.md), [ADR-0100](adr/0100-browser-phase3-live-proxy-acl.md), [ADR-0101](adr/0101-browser-phase3-identity-contract.md) |

## celeris — daemon 本体（配線・設定・自己更新）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| 起動順・配線（facade） | celeris::lib | `crates/celeris/src/lib.rs`（`daemon/` 配下は module doc） | [DESIGN §5](DESIGN.md#5-コンポーネント) |
| tick ループ・裏方処理 | celeris::daemon | `crates/celeris/src/daemon/tick_loop.rs` | [DESIGN §5](DESIGN.md#5-コンポーネント) |
| 設定（TOML → subsystem 別型・検証） | celeris::config | `crates/celeris/src/config/mod.rs`（module map はここの doc comment） | [DESIGN §3](DESIGN.md#3-リポジトリ構成) |
| self-deploy（release/verify/handoff） | celeris | `crates/celeris/src/{instance.rs,releases.rs,config/selfdeploy.rs}` | [selfdeploy.md](selfdeploy.md) |
| Knowledge GC・doc gardener | celeris | `crates/celeris/src/{knowledge_gc,knowledge_maint,doc_gardener}.rs` | [ADR-0068](adr/0068-knowledge-gc-and-repository-docs-maintenance.md) |
| cluster/accounts 管理の裏方 | celeris | `crates/celeris/src/{cluster_admin,accounts_admin}.rs` | [ADR-0017](adr/0017-account-management-from-gui.md), [ADR-0018](adr/0018-remote-clusters-over-ssh.md) |
| 2 層ビルドキャッシュ（L2 cache server） | celeris::cache_server | `crates/celeris/src/cache_server.rs` | [ADR-0075](adr/0075-tiered-build-cache.md) |

## 周辺 crate と結合テスト

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| `celerisctl`（CLI。migration をしない `open_client`） | celerisctl::main | `crates/celerisctl/src/main.rs` | [DESIGN §5.9](DESIGN.md#59-cli-taskctl), [ADR-0095](adr/0095-worker-runs-see-the-db-read-only.md) |
| `llm-proxy`（ローカル LLM 供給プロキシ） | llm-proxy::server | `crates/llm-proxy/src/server.rs` | [ADR-0053](adr/0053-llm-source-proxy.md), [ADR-0132](adr/0132-provider-llm-source-split-and-cheap-qwen.md) |
| `celeris-mcp`（外部エージェント向け MCP） | celeris-mcp::rpc | `crates/celeris-mcp/src/rpc.rs` | [ADR-0056](adr/0056-mcp-server.md) |
| `celeris-credentiald`（credential broker） | celeris-credentiald::lib | `crates/celeris-credentiald/src/lib.rs` | [ADR-0080](adr/0080-browser-phase2-policy-broker-approval.md) |
| `scratch-cache`（build cache L2 サーバー本体） | scratch-cache::server | `crates/scratch-cache/src/server.rs` | [ADR-0075](adr/0075-tiered-build-cache.md) |
| `tests/e2e`（daemon 起動を伴う結合テスト） | e2e | `tests/e2e/tests/scenarios.rs` | [DESIGN §6](DESIGN.md#6-実装フェーズと受け入れ条件) |

## GUI（`gui/`、置き換え予定）と web/（新 SPA、設計段階）

| subsystem | owner crate/module | entry point | ADR / 設計 |
|---|---|---|---|
| Task detail 画面（loader/action + タブ部品） | gui::app | `gui/app/{routes/tasks.$id.tsx,celeris/task-detail.server.ts,components/task-detail}` | [DESIGN Phase 9](DESIGN.md#phase-9-gui-のための基盤と-http-api-層) |
| Browser 操作・本人承認・Live View | gui::app | `gui/app/{routes/browser.control.ts,components/BrowserRunsPanel.tsx,celeris/browser-live.server.ts}` | [ADR-0080](adr/0080-browser-phase2-policy-broker-approval.md), [ADR-0099](adr/0099-browser-phase3-control-lease.md), [ADR-0100](adr/0100-browser-phase3-live-proxy-acl.md) |
| 組織図・アカウント・案件の画面 | gui::routes | `gui/app/routes/{org,accounts,projects.$id}.tsx` | [DESIGN Phase 9](DESIGN.md#phase-9-gui-のための基盤と-http-api-層) |
| 新 SPA（旧 GUI を daemon 遅延から切り離す） | web (設計中) | 未着手（Phase 0 は設計のみ） | [ADR-0081](adr/0081-web-spa-frontend.md), [implementation plan](web/implementation-plan.md) |

## この文書の作り方・保ち方

- 出典: `wu/*/artifacts/map.md`（p0-config・p1-handlers・p1-lib・p1-gui）、`wu/*/artifacts/p2.md`（p2-core・p2-worker）、
  `docs/progress/phase-P0-dispatcher.md`、および HEAD の実ソース（各 crate の `src/` 直下と module doc）。
- 行が増えすぎたら「主要 subsystem」の粒度を保つために統合する（1 crate 1 行までは削らない）。200 行を超えたら周辺 crate の表から削るのではなく、
  変更頻度の低い行をまとめる。
- 新しい crate や大きな module 分割をしたら、この表の対応行を 1 行更新する（新しい ADR/設計文書があればリンクを足す。内容はコピーしない）。
- パスの実在は `scripts/dev/check-architecture-map.py` で機械的に検査する（このファイルを手で直したら実行すること）。
