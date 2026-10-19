---
title: "合成入力でモデル選択まで届く通常経路試験"
tasks: [01M59KY5FPEWVNW0DT07HZRA3Q]
unit: synth-e2e
status: done
updated: 2026-10-19
completed: 2026-10-19
---
# synth-e2e

`crates/task-dispatch/src/dispatcher/tests/jev_synth_e2e.rs`（`synthetic_cases_reach_model_selection_and_stage_counts_match`）を追加。既存の lane_assist 試験の helper（`hinted_task` / `jev_ok` 等を `pub(super)` 化）を再利用し、mock 分類を board に注入して通常の `tick` → run 終了まで進め、決定ログの run（提案先頭 model・実行 model）と `lane_assist` から `celeris_campaign::jev_stages::run_json` の入力を組んで集計する。task-dispatch に dev-dependency `celeris-campaign` を追加（循環なし）。

## case と確認
- Jev が lane を上げる（classification_raise、base と final が違う）／変化 0（Specified、final = base）／fallback timeout（fixed_fallback）／明示 lane（overridden_by=explicit）／B1 fixed（lane_assist 無し）。
- 全 C1 case で final_lane・proposed/exec model が記録され `real_api_verified=false`。
- 集計: C1 trials 4・jev_ok 3・jev_fallback 1（timeout）・explicit 抑止 1・ceiling/availability 0 のまま出る・lane_proposal_changed 1・final_lane_changed 1・model_unknown 0。B1 は分類なし。confidence は集計に出ない。
- 合成の結果は本測定の効果ではない（試験の冒頭コメントに明記）。

## 未実施
ceiling/availability 抑止の合成 case は未追加（集計側は stage-agg 試験で 0/非 0 を確認済み）。

## 証拠
`cargo test -p task-dispatch --lib jev_synth` → 1 passed。全体検査は報告本文に記載。
