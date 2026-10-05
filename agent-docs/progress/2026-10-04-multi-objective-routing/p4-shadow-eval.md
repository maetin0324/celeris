---
tasks: [01M45RPPM85XTGYC17WCZQCER1]
unit: close
status: done
completed: 2026-10-05
---

# Phase 4 (p4-shadow-eval) close: 統合後 HEAD での全体検査と記録

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` の Phase 4（上限付き shadow と
offline replay 評価基盤）を、全 unit（shadow-model・metrics・integrate-model・config・
dispatch-shadow・proxy-shadow・proxy-caps・replay・integrate-runtime・audit-api・cli・
daemon-wire・integrate-wire・gui・ops-doc・web-shadow・integrate-surface）を統合した後の
HEAD `af1c9a8fcc0f`（`celeris-wu/01M45RPPM85XTGYC17WCZQCER1/close`、base `af1c9a8f`、
`CELERIS_WU_BASE = af1c9a8fcc0fcd42df48e81f9ac2e5959cef7628`）で全体検査した。新しいコードは
書かず、検査で見つけた記録の不整合は 0 件だった（既存の unit 別進捗・ADR 付記と実装の食い違いは
なく、本 run で修正したファイルは本ファイルと ADR 末尾の付記のみ）。ADR への Phase 4 付記
（実装と突き合わせた内容）は本 run で `agent-docs/adr/2026-10-04-multi-objective-model-routing.md`
の末尾に追加した。

## 証拠（統合後 HEAD `af1c9a8fcc0f` で実行）

| # | コマンド | 結果 |
| --- | --- | --- |
| 1 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest Summary 原文: `Summary [  73.421s] 4018 tests run: 4018 passed (1 slow), 12 skipped`（slow 1 件は `celeris::instance_handoff a_stale_heartbeat_promotes_the_standby`、60.451 秒）。doc-test は 1 件 ignored のみ（`Summary [8.4s]` 相当、0 failed）。`CELERIS_TEST_SUMMARY` の `passed` 欄が 0 になる集計器の不具合は p3-context-esc 以降で既知のため Summary 原文と exit code を採用。末尾 `test-parallel: ok` |
| 2 | `cargo clippy --workspace -- -D warnings` | exit 0（`Finished dev profile`） |
| 3 | `cargo fmt --all -- --check` | exit 0 |
| 4 | Phase 4 の試験 9 件 + 回帰（計画 check と同じ filterset、`cargo nextest run --no-fail-fast`） | `Starting 24 tests across 83 binaries (3005 tests skipped)` / `Summary [   1.926s] 24 tests run: 24 passed, 3005 skipped`、exit 0。以下すべて 0 件実行でなく通る |
| 5 | `corepack pnpm@11.27.0 -C gui install --offline` / `corepack pnpm@11.27.0 -C gui run typecheck`（react-router typegen && tsc -b） | install exit 0、typecheck exit 0 |
| 6 | `corepack pnpm@11.27.0 -C gui run test`（vitest） | exit 0。`Test Files 95 passed (95)` / `Tests 1336 passed (1336)`（`routing-shadow.test.tsx` 含む） |
| 7 | `corepack pnpm@12.6.0 -C web install --offline` / `corepack pnpm@12.6.0 -C web run typecheck`（tsc -b） | install exit 0、typecheck exit 0 |
| 8 | `corepack pnpm@12.6.0 -C web run test`（vitest + node --test server/*.test.mjs） | exit 0。vitest `Test Files 29 passed (29)` / `Tests 212 passed (212)`、node `pass 42 / fail 0` |
| 9 | `sh scripts/dev/check-doc-links.sh` | exit 0 |
| 10 | `sh scripts/dev/check-adr-numbers.sh` | exit 0 |
| 11 | `sh scripts/dev/progress-index.sh --check` | exit 0 |
| 12 | （参考）`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0（`ok`） |
| 13 | 範囲 check（`{ git diff --name-only "$CELERIS_WU_BASE"; git ls-files --others --exclude-standard; } \| sort -u` を許可 pattern で除外） | 範囲外 0 件（本 WU が変えるのは `agent-docs/` のみ） |

### 試験 9 件と回帰の内訳（#4 で通った 24 件）

Phase 4 の試験 9 件（同名の異なる crate は両方実行）:

| 試験名（ADR §10 Phase 4） | 実行された binary |
| --- | --- |
| `routing_shadow_opt_in_caps_survive_restart_and_handoff` | task-core `store::routing_shadow_tests`・llm-proxy `tests/proxy_shadow` |
| `routing_metrics_apgr_aiq_ibc_known_curves` | task-core `model_router::metrics::tests` |
| `routing_decision_shadow_never_calls_upstream_or_changes_primary` | task-dispatch `dispatcher::tests::routing_shadow`・llm-proxy `tests/proxy_shadow` |
| `routing_shadow_backpressure_timeout_and_no_tool_execution` | llm-proxy `shadow::tests` |
| `routing_offline_replay_is_deterministic_and_split_by_task` | task-ops `routing_replay::tests` |
| `routing_shadow_audit_keeps_primary_outcome_separate` | task-api `tests/routing` |
| `routing_cli_export_evaluate_is_read_only_and_reproducible` | celerisctl |
| `routing_shadow_config_defaults_off_and_requires_caps` | celeris `config::tests` |
| `routing_shadow_wiring_defaults_off_and_reloads` | celeris `daemon::routing_shadow::tests` |

回帰 13 件（`cheap_local_first_*` 9 件と account 回帰 4 件。計画 filterset の
`test(/cheap_local_first_/)` は celeris config 4 件 + task-dispatch 5 件の計 9 件に一致）:

| 試験名 | crate |
| --- | --- |
| `cheap_local_first_can_be_disabled` / `_concurrency_comes_from_the_provider_row` / `_legacy_production_yields_opencode_qwen` / `_celeris_row_needs_a_local_source_and_qwen_cheap` | celeris `config::tests` |
| `cheap_local_first_picks_local_when_free` / `_falls_to_pool_when_local_full` / `_falls_to_pool_when_local_down` / `_standard_lane_is_unchanged` / `_routing_decided_records_reason` / `_unsupported_adapter_goes_to_pool` | task-dispatch `dispatcher::tests::cheap_local_first` |
| `cheap_only_legacy_mappings_never_route_frontier_or_standard_to_qwen` | llm-proxy |
| `claude_429_falls_back_to_the_next_account_and_records_a_cooldown` | llm-proxy |
| `select_provider_sticks_to_the_sessions_account_over_a_better_scoring_one` | task-dispatch `dispatcher::tests::routing_and_quota` |

## 記録の不整合

0 件。unit 別進捗 13 本（p4-shadow-model・p4-metrics・p4-config・p4-dispatch-shadow・
p4-proxy-shadow・p4-proxy-caps・p4-replay・p4-cli・p4-daemon-wire・p4-audit-api・p4-gui・
p4-web-shadow・p4-ops-doc）と ADR の既存付記（Phase 4 shadow-model）は実装と一致していた。
ADR §6 の event 名は `routing_shadow_evaluated` と書くが実装は Objective どおり
`routing_shadow_recorded`（ADR §10 Phase 4 表の試験名・進捗・schema とも後者）であることは
shadow-model の付記で既に記録済みで、本 run の検査でも食い違いは無いことを確認した。
