---
title: "Phase 5（p5-estimator）: 外部/学習 estimator の必要性評価と pluggable adapter（shadow → opt-in）"
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
status: blocked
updated: 2026-10-05
completed: 未（実 RouteLLM sidecar の評価が人の手順待ち。実装・偽 sidecar 検証・手順書は 2026-10-05 に統合済み）
---

# Phase 5（p5-estimator）: 外部/学習 estimator の必要性評価と pluggable adapter

ADR `agent-docs/adr/2026-10-04-multi-objective-model-routing.md` §10 Phase 5。人の回答 adapter-plus-routellm に従い、
汎用 sidecar adapter（既定 off・`shadow_only = true` 必須・失敗時は heuristic・決定権なし）と RouteLLM sidecar の
shadow 評価の道具立てを実装した。本番の切り替えはしていない。unit 別の記録は [p5-estimator/](p5-estimator/)、
実装と ADR の突き合わせは ADR 末尾の付記「Phase 5 の実装済み範囲（close による全体検査）」。

## 状態

- **Phase 5 の実評価は未完了（人の手順待ち）。** 人の決定 `routellm-weights-use`（2026-10-05）は weights を内部の shadow
  評価に限り承認し、実 sidecar の start-stop と上限付き shadow を人（Fable）が本番 host で
  `scripts/model-routing/real-sidecar-check.sh` と `docs/ops/model-routing-migration.md` §10 に沿って実行し、
  原票を `docs/reports/model-routing-routellm-shadow/` に置くとした。close 時点で原票は無く、転記していない。
  偽 sidecar の合格で置き換えていない（report §3・§4）。
- 必要性評価（assess）: 外部 estimator を adapter で shadow 比較する価値はある。自動学習・本番選択はしない
  （[assess.md](p5-estimator/assess.md)、ADR 付記「Phase 5 必要性評価」）。

## 証拠（close、統合後 HEAD `18f60cef8b39`、`CELERIS_WU_BASE = 18f60cef8b39bdb43e517ba3622b4c5c47e1e2e4`）

| # | コマンド | 結果 |
| --- | --- | --- |
| 1 | `bash scripts/dev/test-parallel.sh` | exit 0。`Summary [  74.457s] 4034 tests run: 4034 passed (1 slow), 13 skipped`、doc-test 0 failed、`test-parallel: ok`（`CELERIS_TEST_SUMMARY` の `passed: 0` は既知の集計器不具合。Summary 原文を採用） |
| 2 | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 3 | `cargo fmt --all -- --check` | exit 0 |
| 4 | `cargo nextest run --workspace --no-fail-fast -E 'test(/routing_sidecar_/) \| test(/routing_routellm_pair_adapter/) \| test(/routing_estimator_/) \| test(/routing_shadow_/) \| test(/routing_decision_shadow/) \| test(/routing_offline_replay/) \| test(/cheap_local_first_/) \| test(/cheap_only_legacy_mappings_never_route_frontier_or_standard_to_qwen/) \| test(/claude_429_falls_back_to_the_next_account_and_records_a_cooldown/) \| test(/select_provider_sticks_to_the_sessions_account_over_a_better_scoring_one/)'` | exit 0。`38 tests run: 38 passed, 4009 skipped`（Phase 5 の新試験と Phase 4・回帰。0 件実行でない） |
| 5 | `python3 -m unittest discover -s scripts/model-routing -p 'test_*.py'` | exit 0、`Ran 3 tests` OK |
| 6 | `sh scripts/model-routing/check-runbook.sh` | exit 0、`ok (routellm 0b64fdafe049e596a3f5657c219329f24af24198, routellm-weights-use=pending)` |
| 7 | `sh scripts/model-routing/fake-shadow-check.sh` | exit 0、`ok (validated, SIGTERM exit 0, pid gone, port … closed)` |
| 8 | `sh scripts/model-routing/real-sidecar-check.sh` | `not run: set CELERIS_ROUTELLM_REAL=1 and CELERIS_ROUTELLM_WEIGHTS_DIR to approved local weights`（未実行。合格扱いにしない） |
| 9 | `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` | exit 0 |
| 10 | `test -d docs/reports/model-routing-routellm-shadow` | 無い（原票なし → 転記なし） |

## 未解決事項

1. 実 RouteLLM sidecar の start-stop（`routing_routellm_real_sidecar_start_stop`）と上限付き shadow
   （`routing_routellm_real_shadow_within_caps`）が未実行。人の実行と原票待ち。手順書 §10.1 の block は weights の
   full revision・checksum が記録されるまで `pending`。
2. （解決済み）偽 sidecar の結合試験が回避していた食い違い 3 点（report §2）は shadow-join unit が直し、
   `3687abbb`（integrate wu/shadow-join）を close branch に取り込んだ。
3. 推移依存の lock と CPU/GPU wheel の選択、sidecar 推論の実資源費（daemon は名目 `SIDECAR_NOMINAL_CALL_USD` で数える）、
   raw score の v1 DTO 専用欄、校正（`calibration_version`）。

## 提案

- 人の実行後、close を再走して原票の実行コマンド・exit・件数・上限消費・制約違反数を report §3 に転記し、
  本ファイルを `status: done` にする。opt-in（本番判断への反映）は別の Phase で人が決める。
