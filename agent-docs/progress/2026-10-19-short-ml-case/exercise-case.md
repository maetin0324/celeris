---
task: 01M5AFX5TQKG3WAEYD1E0XQNMS
wu: exercise-case
status: blocked
completed: null
---
# exercise-case: 実 job は承認待ち（approval_missing）、状態は pending のまま

## 事実
- 2026-10-19T16:34Z に実行直前の再確認をした。sirius の環境（python 3.9.25・numpy 1.23.5）は env.lock と一致した。
- sirius の PBS CPU job 1 本を投入する人の承認の記録が無いため、job は投入していない（docs/ops/short-ml-campaign.md の実行前条件 1）。
- 正例・負例 3 件は未実行。job id・raw・grader の exit は存在しない。費用は 0。
- status.json は pending のまま変えていない。K2/K4・arm 実行器・grader・loader・manifest も不変。
- 判断材料と再確認の記録は外部資料の `exercise` 節にある: `/local/celeris/data/workspaces/01M59KY5FTRJTJ80PACNW3K533/artifacts/ml-case-reassessment.json`（sha256 d36d2cb3…）と同じ dir の `ml-case-review.md`

## 証拠コマンド
- `python3 crates/campaign/scripts/verify-short-ml-case.py --check` → exit 0（状態整合のみ）
- `python3 crates/campaign/scripts/verify-short-ml-case.py --acceptance --evidence <上記 json>` → exit 1（pending なので非 0 が正しい）
- `python3 crates/campaign/scripts/verify-short-ml-case.py --placement-check` → exit 0

## 未解決事項
- 人の承認: sirius の debug queue に CPU job 1 本（1 core・2 GiB・walltime 600 s）を投入してよいか。
- 評価側の本番 collect 経路（`observe::SchedulerTransport` の ssh `qstat -x -f` 実装と CLI からの呼び出し）が未実装。承認後に限定配線が要る。
