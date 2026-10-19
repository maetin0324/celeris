---
task: 01M59KY5FTRJTJ80PACNW3K533
wu: case-controls
status: done
completed: 2026-10-19
---
# case-controls: 短い ML case の状態は pending（記録は case 探索外）

## 実装事実

- 状態は `crates/campaign/optional/short-ml-campaign/status.json` にある（`cases/` の外）。`status` は `pending`、`confirm_at` は 2026-10-22T00:00:00Z。
- `claims`（final_generalization / real_pilot_confirmed / adopted）は全て false。未実施の 4 例・実 pilot は完了にしない。
- loader（`crates/campaign/src/cases.rs`）・grader・既存 case・manifest は base c9654bff から不変。K2/K4 は置き換えない。

## 外部参照

- 判断材料と旧判断の撤回履歴: `/local/celeris/data/workspaces/01M59KY5FTRJTJ80PACNW3K533/artifacts/ml-case-reassessment.json`
- レビュー: `/local/celeris/data/workspaces/01M59KY5FTRJTJ80PACNW3K533/artifacts/ml-case-review.md`

## 証拠コマンドと結果

- `python3 crates/campaign/scripts/verify-short-ml-case.py --check` → 状態整合の検査であり、採用を意味しない。
- `git diff c9654bff -- crates/campaign/src crates/campaign/cases crates/campaign/fixtures` → 差分ゼロ。
