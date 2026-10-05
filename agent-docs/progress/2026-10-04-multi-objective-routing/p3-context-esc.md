---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: close
status: done
completed: 2026-10-05
---

# Phase 3 (p3-context-esc) close: 統合後 HEAD での全体検査と記録

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` の Phase 3（task metadata の
RoutingContext 搬送・軌跡 escalation・feature/request/outcome event）を、全 unit
（context・escalation・events・model-fix・model-fix2・integrate-model・dispatch-context・
dispatch-esc・proxy-context・worker-transport・integrate-runtime・audit-api・daemon-wire・
dispatch-outcome・integrate-wire・gui・web・integrate-surface）を統合した後の HEAD
`31edde850f12`（`celeris-wu/01M4577C9412HCDQEV1AFTT69C/close`、base `31edde85`、
範囲の基点 `git merge-base HEAD celeris/01M4577C9412HCDQEV1AFTT69C` = `31edde85`）で
全体検査した。新しいコードは書かず、検査で見つけた記録の不整合 1 件のみ直した。
ADR への Phase 3 付記（実装と突き合わせた内容）は本 run で `agent-docs/adr/2026-10-04-multi-objective-model-routing.md`
の末尾に追加した。

## 証拠（統合後 HEAD `31edde850f12` で実行）

| # | コマンド | 結果 |
| --- | --- | --- |
| 1 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest Summary 原文: `Summary [  72.914s] 3993 tests run: 3993 passed (1 slow), 12 skipped`（slow 1 件は `celeris::instance_handoff a_stale_heartbeat_promotes_the_standby`、60.5 秒）。`CELERIS_TEST_SUMMARY` の `passed` 欄が 0 になる集計器の不具合は Summary 原文と exit code を採用（p3-escalation 以降で既知）。doc-test は 1 crate 1 件 ignored のみ、0 failed |
| 2 | `cargo clippy --workspace -- -D warnings` | exit 0（`Finished dev profile`） |
| 3 | Phase 3 の試験 5 件 + 回帰（計画 check #2 と同じ filterset、`cargo nextest run --no-fail-fast`） | `Starting 26 tests across 121 binaries (3979 tests skipped)` / `Summary [   0.254s] 26 tests run: 26 passed, 3979 skipped`、exit 0。以下すべて 0 件実行でなく通る |
| 4 | `corepack pnpm@11.27.0 -C gui run typecheck`（react-router typegen && tsc -b） | exit 0 |
| 5 | `corepack pnpm@11.27.0 -C gui run test`（vitest） | exit 0。`Test Files 94 passed (94)` / `Tests 1334 passed (1334)` |
| 6 | `pnpm -C web run typecheck`（tsc -b） | exit 0 |
| 7 | `pnpm -C web run test`（vitest + node --test server/*.test.mjs） | exit 0。vitest `Test Files 29 passed (29)` / `Tests 207 passed (207)`、node `pass 42 / fail 0` |
| 8 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（`ok (137 files)`） |
| 9 | `sh scripts/dev/check-doc-links.sh` | exit 0 |
| 10 | `sh scripts/dev/progress-index.sh --check` | exit 0 |
| 11 | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| 12 | 範囲 check（`B=$(git merge-base HEAD celeris/01M4577C9412HCDQEV1AFTT69C) && git diff --name-only $B -- . ':(exclude)agent-docs/progress' ':(exclude)agent-docs/adr' ':(exclude)docs/architecture-map.md'`） | 差分 0 件（本 WU が変えるのは progress と ADR のみ） |

### 試験 5 件と回帰の内訳（#3 で通った 26 件）

Phase 3 の試験 5 件（同名の異なる crate は両方実行）:

| 試験名（ADR §10 Phase 3） | 実行された binary |
| --- | --- |
| `routing_context_extracts_acceptance_tools_environment_and_history` | task-dispatch `dispatcher::tests::routing_context` |
| `routing_context_ref_propagates_and_rejects_spoofing` | llm-proxy `proxy_routing_context`・task-worker `acp::tests`（+ 同名の `aider::tests::routing_context_ref_propagates_to_aider_proxy_settings` も filterset で走る） |
| `routing_trajectory_escalates_one_lane_and_respects_caps` | task-core `retry_policy::tests`・task-dispatch `dispatcher::tests::routing_and_quota` |
| `routing_reward_waits_for_review_and_supersedes_idempotently` | task-core `model_router::feedback::tests`・task-ops `routing_outcome::tests` |
| `routing_audit_links_run_request_and_actual_source` | task-api `routing` |

回帰 12 件（Phase 3 回帰 5 件 + Phase 2 回帰 7 件。`test(/cheap_local_first_/)` は便宜上
`cheap_local_first_*` 8 件を含む 13 件で実行）:

| 試験名 | crate |
| --- | --- |
| `repeated_review_failures_escalate_the_retry_lane_one_step` | task-dispatch |
| `never_escalates_above_the_ceiling_or_on_supply_side_failures` | task-core |
| `work_unit_lane_is_capped_by_the_task_lane` | task-core |
| `joins_routing_usage_wall_retries_and_review_per_run` | task-core |
| `routing_is_empty_without_runs_and_404_for_unknown_tasks` | task-api |
| `cheap_local_first_falls_to_pool_when_local_full` / `_local_down` / `_standard_lane_is_unchanged` / `_routing_decided_records_reason` | task-dispatch |
| `select_provider_sticks_to_the_sessions_account_over_a_better_scoring_one` | task-dispatch |
| `cheap_only_legacy_mappings_never_route_frontier_or_standard_to_qwen` | llm-proxy |
| `claude_429_falls_back_to_the_next_account_and_records_a_cooldown` | llm-proxy |

## 検査で見つけた記録の不整合（修正済み）

- `p3-worker-transport.md` の front matter `status: complete` が他の p3 進捗ファイル
  （`done-in-branch`）と揃っていなかった。本 run で `done-in-branch` に直した。
  progress-index は subdirectory の unit ファイルを `--check` 対象にしないため検査は
  通り続けていた（`--check` の対象は `YYYY-MM-DD-*.md` のトップレベルのみ）。

## 未解決事項

- **`task-api::query::EVENT_TYPES`（65 件）と schema の Event variant 全体（71 件）の 6 件のずれ。**
  `execution_routed`・`knowledge_curation_applied`・`merge_candidate_stale`・`phase_integrated`・
  `work_unit_committed`・`work_units_serialized` を EVENT_TYPES が含まない。Phase 3 の 3 event
  （`routing_features_recorded`・`routing_request_decided`・`routing_outcome_recorded`）は
  両方（EVENT_TYPES と `web/api/realtime/event-kinds.ts`・`invalidation-map.ts`）に含まれる。
  Phase 3 の範囲外なので触れていない（p3-web にも記録済み）。
- **delivery 側の reopen が outcome 追記を呼ばない。** `routing_outcome_recorded` の追起は
  `task-ops::comment::reopen` にだけ配線済みで、`celeris/src/delivery.rs` の `Trigger::Reopen`
  は未配線。投影は冪等なので次の完了時に届く（p3-dispatch-outcome にも記録済み）。
- **未知モデル・価格未登録の cost は補完できず `reward = None` のまま。** pricing 表で補完するのは
  既知モデルのみで、推測しない設計どおり（p3-dispatch-outcome にも記録済み）。
- **組織 privacy 制約（server/org 単位の `Constraints`）の設定経路が未実装。**
  daemon は `dispatch_settings()` が既定の制約を渡すため、現状の本番配線では
  `context_transport_unsupported` による proxy 経由行の除外は発動しない
  （p3-daemon-wire にも記録済み）。
- **`RoutingContextRegistry::register` の署名が ADR §10 の表（`register(run_id, ctx, ttl)`）と
  違い `now` を引数に取る。** 決定的試験のために時計を注入する形で実装した
  （p3-context に記録済み。ADR 付記でこの署名を正本として書いた）。
- **gui の Playwright 実ブラウザ script `scripts/check-task-routing.mjs` は新しい欄
  （実 source・outcome・escalation 監査）の期待値を持たない。** 既存の期待値は新表示でも
  成立する（追加欄は既存 testid に衝突しない）が、実ブラウザでの確認は未実施
  （p3-gui にも記録済み）。

## 提案

- EVENT_TYPES と schema の Event variant の 6 件のずれは、意図的な部分集合なら query.rs に
  理由を、揃えるなら別 task で揃える（どちらかは人の判断）。
- delivery の reopen（`celeris/src/delivery.rs` の `Trigger::Reopen`）にも
  `record_routing_outcomes` を呼ぶ小さな follow-up。
- gui の `scripts/check-task-routing.mjs` に実 source・outcome・escalation 監査の期待値を
  足す follow-up（実ブラウザでの確認が欲しければ）。
- `RewardNormalization`（cash 1.0 USD / wall 3,600,000 ms / retries 4）の参照値は
  Phase 3 では設定に出していない。shadow/offline 評価（Phase 4 以降）の前に設定化すると、
  試験の正規化値と運用値が揃う。
