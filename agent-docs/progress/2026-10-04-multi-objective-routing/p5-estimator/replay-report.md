---
tasks: [01M4651ZZP8FJKGG6W2WPFNKBH]
unit: replay-report
status: done
completed: 2026-10-05
---

# Phase 5 replay-report: estimator shadow の比較 report と RouteLLM pair adapter

## 変更

- `crates/task-ops/src/routing_replay/estimator.rs` を新設（`routing_replay.rs` の子 module）。
  - `EstimatorComparisonV1`: dataset の estimator shadow（`kind = estimator`）から対象数・評価済み数・coverage、
    completed/failed/timeout/dropped/prompt_required、推論 overhead（mean/p50/p95 ms）、heuristic 首位との一致/差、
    primary との一致、品質に数えた件数と unknown 件数、比較不能理由の内訳を出す。
  - pin した `EstimatorDescriptor` と `RouteLlmPairV1`（router・strong/weak・`calibration_version`）を report に保存。
    pair があれば対象は両 model が候補にある決定だけ。pair 外の選択は `outside_pinned_pair` で比較しない。
    版が pin と違う shadow（`id@version` 等と照合）は `estimator_version_mismatch`。
  - 未校正（`calibration_version` なし）の pair では primary と同じ選択でも `uncalibrated_raw_score` として
    品質成功に数えない。primary 以外の選択は `unselected_model_outcome_unknown`。
  - `routellm_pair_estimates`: `/estimate` 応答を pin した pair の 2 model にだけ写す。raw score は
    reasons の `raw_pair_score=<f>`（または `:`）から読み、未校正なら `index = None`、校正済みなら strong=raw・weak=1-raw。
    pair 外の model は sidecar が index を返しても `None`。descriptor id/version 不一致・不正 pair は拒否。
- `routing_replay.rs`: `ShadowV1.detail_code`（estimator のみ、allowlist の code だけ。自由文は出さない。
  serde default/skip で既存 JSONL は不変）、`ReportV1.estimator_comparison`（optional・skip）、
  `evaluate_with_estimator` を追加。`shadow_recorded` の timeout/drop 率から estimator shadow を除外。
  heuristic 首位の選び方を `heuristic_choice` に切り出し（挙動不変）。DB は読み取り専用のまま、HTTP は呼ばない。

## 証拠

- `cargo nextest run -p task-ops -E 'test(routing_estimator_shadow_report_records_coverage_and_limits) | test(routing_routellm_pair_adapter_preserves_unknown_models) | test(routing_offline_replay_is_deterministic_and_split_by_task)'` → 3 tests run: 3 passed。
- `cargo nextest run -p task-ops -E 'test(routing_)'` → 11 passed。
- `bash scripts/dev/test-parallel.sh` → exit 0、4023 tests run: 4023 passed, 12 skipped。
- `cargo clippy --workspace -- -D warnings` → exit 0。

## 未解決事項

- raw pair score の載せ方（reasons の `raw_pair_score=<f>`）は本 unit の仮の規約。routellm-wrapper unit が別の形で返すなら
  integrate-runtime で `RAW_PAIR_SCORE_PREFIXES` を合わせる。
- `prompt_required` は `ShadowReason` に variant が無いため、reason `prompt_required`（将来の variant）と
  detail の先頭 code `prompt_required` の両方を数える。proxy-shadow-est の記録形式に合わせて確認が要る。
- `ShadowRecord.policy_version` の estimator 版の書式（`<id>@<version>` 等）は proxy-shadow-est で確定。
- CLI（`celerisctl routing evaluate --policy estimator`）への配線は cli unit。

## 提案

- 校正（`calibration_version`）の定義と校正 dataset は paired outcome が集まった後の別 Phase で決める。
