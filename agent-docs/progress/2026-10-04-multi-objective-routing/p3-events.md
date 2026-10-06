---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: p3-events
phase: 3
status: done-in-branch
completed: 2026-10-05
---

# p3-events: feature/request/outcome の event schema と決定的な outcome 投影

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §5・§6・§10 Phase 3 の event を task-core に足した。

## 実装

- `crates/task-core/src/model_router/feedback.rs`（新規）
  - `RoutingFeaturesRecord { decision_id, context_version, features(JSON), provenance, missing_fields, run_id?, request_id?, stage?(dispatch/proxy) }`
  - `RoutingRequestRecord { request_id, decision_id, parent_decision_id?, run_id?, trace?: RoutingTraceV1, attempts[]: RequestSourceAttempt{source_id, model?, account_id?, fallback_reason?}, fallback_reason? }`
  - `RoutingOutcome { outcome_id, decision_id, run_id?, request_id?, evaluation_version, supersedes?, acceptance_passed?, review_passed?, failed_criterion_ids[], failure_class?, cash_usd?, tokens?, wall_ms?, retries?, reward? }`
  - 版: `ROUTING_CONTEXT_VERSION = "routing-context/1"`、`ROUTING_OUTCOME_EVALUATION_VERSION = "routing-outcome/1"`
  - `routing_reward(pass, cash, wall, retries, norm)` = `pass - 0.2*min(cash/cash_ref,1) - 0.1*min(wall/wall_ref,1) - 0.1*min(retries/4,1)`。どれかが欠ければ None。参照値 `RewardNormalization` の既定は 1.0 USD・3,600,000 ms。
  - `project_run_outcomes(events, norm)`: `RoutingDecided` があり `WorkerFinished` まで届いた worker run（reviewer run を除く）ごとに 1 件。decision_id は `record.optimizer.decision_id`、無ければ `run:<run_id>`。review（`Transitioned` の review_pass/review_fail と直前の `ReviewVerdict` の不合格）は直前の worker run に、WU 検査（`WorkUnitCheckFinished`/`WorkUnitChecksFailed`）は `run_id` で帰属。pass は review があれば review、無ければ検査の不合格だけを確定（合格だけでは未判定）。failure_class は `acceptance_failed` > `review_failed` > `RunEnd` の code（yielded/budget_exhausted/question/failed/supply_side/infra/lease_expired/idle_timeout/cancelled/waiting）。`outcome_id` は outcome の中身（supersedes を含み outcome_id を除く）の sha256 先頭 16 byte で `outcome-<hex>`。events 中の同じ run・decision の最新の `RoutingOutcomeRecorded` と結果が同じならそれをそのまま返し、違えば新 ID + `supersedes`。`request_id` は常に None（run の合否を要求へ複写しない）。
  - `pending_run_outcomes(events, norm)`: 投影のうち未追記（同じ outcome_id が events に無い）だけ。dispatch-outcome leaf が冪等に追記する入口。
- `crates/task-core/src/model.rs`: `Event::RoutingFeaturesRecorded { #[serde(flatten)] record }`・`RoutingRequestDecided { #[serde(flatten)] record }`・`RoutingOutcomeRecorded { #[serde(flatten)] outcome }`。wire は ADR §6 の欄名が event の直下に並ぶ。状態は変えない（replay は `_` で無視）。
- `crates/task-api/src/query.rs`: `EVENT_TYPES` 62→65、`event_type_name` に 3 種。
- `docs/api/v1/event.schema.json`・`docs/api/v1/api-v1.schema.json`: `UPDATE_SCHEMA=1` で再生成（Event に 3 variant）。

## 証拠

| 条件 | コマンド | 結果 |
| --- | --- | --- |
| 0. serde 名・event_type_name・EVENT_TYPES の一致と固定数 65 | `cargo nextest run -p task-api routing_feedback_event_types_match_their_serde_names cluster_job_wait_event_types_match_their_serde_names`（全体実行に含む） | pass |
| 1. 未レビュー None・到着後 1 件・再投影同一・supersede | `cargo nextest run -p task-core feedback` | 3 passed（`routing_reward_waits_for_review_and_supersedes_idempotently`、`routing_outcome_reads_work_unit_checks_by_run`、`routing_feedback_events_use_adr_wire_names_and_tolerate_missing_fields`） |
| 全体 | `bash scripts/dev/test-parallel.sh` | exit 0、nextest Summary「3972 tests run: 3972 passed (1 slow), 12 skipped」 |
| clippy | `cargo clippy --workspace -- -D warnings` / `cargo clippy -p task-core -p task-api --all-targets -- -D warnings` | 両方 exit 0 |

## 未解決

- web の `event-kinds.ts`・`invalidation-map.ts`、gui/web の生成型は未更新（surface 段の web leaf・audit-api leaf の担当）。
- WU 検査の合格だけ（review 無し）の run は reward=None のまま。WU 単位の pass をどう扱うか（task の final review を WU run に帰属させるか）は dispatch-outcome leaf で決める。
- `RewardNormalization` の参照値は設定に出していない（Phase 3 の設定は daemon-wire leaf）。

## 提案

- close leaf で ADR に付記: §6 の 3 event は `#[serde(flatten)]` で欄が event 直下、`evaluation_version = routing-outcome/1`、outcome_id は内容 hash、pass の確定規則（review 優先・検査不合格のみ確定）、failure_class の code 一覧、`request_id` を持つ outcome は投影しない（要求単位の評価は Phase 4 以降）。

## attempt 2（2026-10-05）

- check 不合格の原因: (a) `cargo fmt --check` の差分 → `cargo fmt --all` で解消（feedback.rs・feedback/tests.rs）。`cargo clippy -p task-core/-p task-api/-p task-ops --all-targets -- -D warnings && cargo fmt --all -- --check` は exit 0。
- (b) nextest の check は filterset が 1 文字ずつ `test(x)` に分割され `test())` で構文不正（exit 94）。成果では直せない → plan_issue。
- (c) 範囲 check が `docs/api/v1/{event,api-v1}.schema.json` を範囲外と見る。Event に variant を足すと `event_row_schema_matches_committed`（task-core）と `committed_schema_matches_generated`（task-api）が commit 済み schema との一致を要求するため、再生成は必須 → allow に `docs/api/v1` を足す plan_issue。

## attempt 3（2026-10-05）

- `git log -4 --oneline` と `git diff baccb0fd..HEAD --stat` で、成果 commit `55d9988b`・`7ff967a6`、3 event・outcome 投影・API の固定数 65・schema 再生成を確認。作業ツリーは clean。
- `cargo nextest run -p task-core -p task-api -E 'test(routing_reward_waits_for_review_and_supersedes_idempotently) | test(routing_outcome_reads_work_unit_checks_by_run) | test(routing_feedback_events_use_adr_wire_names_and_tolerate_missing_fields) | test(routing_feedback_event_types_match_their_serde_names) | test(cluster_job_wait_event_types_match_their_serde_names)'`: exit 0、5 passed。
- `bash scripts/dev/test-parallel.sh`: exit 0、nextest Summary は 3972 passed（1 slow）、12 skipped。集計行の `passed: 0` は既知のパーサー不具合であり、Summary 原文と exit code を採用した。
- `cargo clippy --workspace -- -D warnings`、`cargo clippy -p task-core -p task-api --all-targets -- -D warnings`、`cargo fmt --all -- --check`: すべて exit 0。
- 新たな実装上の未解決事項はない。上記の web・生成型・WU 合格のみの評価規則・正規化値の接続は、それぞれ既定の後続 leaf が扱う。
