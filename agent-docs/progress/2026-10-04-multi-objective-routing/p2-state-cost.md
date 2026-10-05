---
tasks: [01M44ZK8GADYD9PVAYBY73F6YC]
unit: close
status: done
completed: 2026-10-05
---
# Phase 2 (p2-state-cost): source 状態・effective cost・選択と予約の close

完了日: 2026-10-05

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` の Phase 2 の範囲を、統合後 HEAD（`762cb2254bb2`、integrate wu/web）で検査した。実装は段 model（cost・log-index・trace）→ runtime（proxy-select・proxy-fallback・dispatch-enforce）→ config-api（config・api）→ surface（schema・gui・web）で入っている。この段（close）では新しいコードを書いていない。検査は全部通った。各 unit の記録は同じディレクトリの `p2-<key>.md`。

## 検査の結果

| 検査 | コマンド | 結果 |
| --- | --- | --- |
| 整形 | `cargo fmt --all -- --check` | exit 0 |
| workspace 全試験 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest Summary 原文「`Summary [  73.028s] 3968 tests run: 3968 passed (1 slow), 12 skipped`」。doctest 0 failed。末尾の `CELERIS_TEST_SUMMARY` は `"passed": 0` と出すが、これは集計パーサーの既知の誤読で、件数は上の Summary 原文で数える |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| Phase 2 の試験と既存の回帰 | `cargo nextest run --workspace -E 'test(/routing_/) \| test(<回帰 7 件>)'` | exit 0、Summary「78 tests run: 78 passed, 3902 skipped」（0 件実行ではない） |
| web | `pnpm -C web typecheck` / `pnpm -C web test` | exit 0 / exit 0（vitest「Test Files 29 passed (29)」「Tests 203 passed (203)」、node「tests 42 / pass 42 / fail 0」） |
| gui | `corepack pnpm@11.27.0 typecheck`（gui/） | exit 0 |
| 文書検査 | `check-adr-numbers.sh`・`check-doc-links.sh`・`progress-index.sh`・`check-doc-layout.sh scripts/dev/docs-layout.tsv`・`check-architecture-map.py` | いずれも exit 0（付記・手順を書いた後に再実行） |

ADR §10 Phase 2 の表の試験は `cargo nextest list` で全件が存在し、上の 78 件に入っていることを確かめた。

| 試験 | crate |
| --- | --- |
| `routing_effective_cost_distinguishes_cash_shadow_and_resource` | task-core（`model_router::cost`） |
| `routing_source_state_stale_quota_is_unknown` | task-core（偽時計） |
| `routing_reservation_conflict_reselects_once`・`routing_shared_gpu_capacity_not_double_counted` | llm-proxy（`selection::state`） |
| `routing_proxy_fallback_preserves_constraints_and_stream_boundary` | llm-proxy（`tests/proxy_fallback.rs`、手動時計） |
| `routing_enforce_quota_defers_without_lane_downgrade` | task-dispatch |
| `routing_source_api_reports_freshness_and_cost_components` | task-api（`tests/routing_source_state.rs`） |

回帰（ADR の列挙どおり、変えずに通過）: `cheap_local_first_falls_to_pool_when_local_full`・`cheap_local_first_falls_to_pool_when_local_down`・`cheap_local_first_standard_lane_is_unchanged`・`cheap_local_first_routing_decided_records_reason`・`select_provider_sticks_to_the_sessions_account_over_a_better_scoring_one`・`cheap_only_legacy_mappings_never_route_frontier_or_standard_to_qwen`・`claude_429_falls_back_to_the_next_account_and_records_a_cooldown`。

task の受け入れ条件 4 への対応:
- 再現性: `constraints_exclude_before_score_and_reasons_are_traced`（llm-proxy）が同じ snapshot と時刻から同じ選択・同じ trace JSON を出すことを、`routing_enforce_quota_defers_without_lane_downgrade` が十分な残量で 2 回同じ結果を出すことを固定する。
- shadow price の単調性: `routing_effective_cost_distinguishes_cash_shadow_and_resource` が残量 0.5→0.1 で 5 倍、reset までの時間が半分で半分を固定する。
- cheap の Qwen 優先と fallback: 上の回帰 7 件と `routing_proxy_fallback_…`（`qwen/cheap` の制約で Claude 0 回）。

本番の `~/.config/celeris`・daemon・DB には触れていない（この段は検査と文書だけ）。

## 未解決事項

- **proxy の enforce は未配線**。`select_state`・`reserve_with_reselect`・`insert_routed`（llm-proxy）は試験済みの部品だが、`server.rs` は使っていない。proxy の候補順は mode によらず legacy の `attempts_for`。`[model_routing].enforce_routes` は検証と保持だけ。proxy に入ったのは同一要求内 fallback（分類別上限・総上限・deadline・breaker）で、これは mode によらず効く。
- **daemon の sources API は `deployments` を空で返す**（`crates/celeris/src/daemon/api.rs` の `LlmSourcesAdapter`）。`source_state_view` は試験済みだが、実際の SourceState・CostEstimate は渡していない。GUI/web の表示は fixture でだけ確かめた。
- **proxy の要求単位の trace を task events に足す sink は未実装**。routing audit API の `requests` は実運用ではいつも空、`audit_incomplete` は false。llm_proxy_requests の相関欄（migration 0048）にも書き込む経路が無い。
- enforce の defer（同じ lane に別 source が無い）は event を残さず tracing だけ。人が待ちの理由を見るには daemon ログを読む。
- pool 内の別 account は別 source として試さない（provider ごと外す）。
- dispatcher の `OBSERVATION_TTL_SECS` は定数。`[model_routing.retry]` は proxy 起動時に 1 度だけ読むので、変更は再起動で効く（reload では効かない）。
- 実ブラウザでの表示（gui の `scripts/check-task-routing.mjs`、web の Playwright e2e）は未実行。人の確認が要る。
- 親の進捗 `agent-docs/progress/2026-10-04-multi-objective-routing.md` の表は Phase 1/2 とも「未着手」のまま（共有の索引には追記しない規則に従った）。

## 提案

- Phase 3 の前に「proxy enforce の配線」を 1 段置く: `ProxyState` に `RoutingRuntime`（capacity・rates・freshness・window reserve）を渡す口を 1 つにまとめ、mode=enforce かつ route が `enforce_routes` に入る要求だけ `select_state` → `reserve_with_reselect` → `insert_routed` を通す。予約は応答 body の drop まで持ち、fallback で次の候補へ移るときは前の予約を返してから取り直す。
- 同じ段で、daemon の sources API に proxy の SourceState と CostEstimate を渡し、proxy trace を `RoutingDecided{run_id, record.optimizer.stage = "proxy"}` として append する sink を作る（routing audit は既にこの形を読む）。
- defer を人に見せるなら、状態が変わったときだけ記録する（毎 tick の event 増殖を避ける）。
