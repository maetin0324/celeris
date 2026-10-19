---
task: 01M5AFX5TQKG3WAEYD1E0XQNMS
wu: exercise-case
status: blocked
completed: null
---
# exercise-case: sirius の queue 下限（24 core・GPU 1）で拒否、1 vnode case への改訂の判断待ち（approval_missing）、状態は pending のまま

## 事実
- 2026-10-19T16:34Z に実行直前の再確認をした。sirius の環境（python 3.9.25・numpy 1.23.5）は env.lock と一致した。
- sirius の PBS CPU job 1 本を投入する人の承認の記録が無いため、job は投入していない（docs/ops/short-ml-campaign.md の実行前条件 1）。
- 正例・負例 3 件は未実行。job id・raw・grader の exit は存在しない。費用は 0。
- status.json は pending のまま変えていない。K2/K4・arm 実行器・grader・loader・manifest も不変。
- 判断材料と再確認の記録は外部資料の `exercise` 節にある: `/local/celeris/data/workspaces/01M59KY5FTRJTJ80PACNW3K533/artifacts/ml-case-reassessment.json`（sha256 d36d2cb3…）と同じ dir の `ml-case-review.md`

## 投入試行（2026-10-19T16:44Z、run 01M5AGR6S7F9458635S123S7NY）
- 人の承認（sirius の debug queue に CPU job 最大 2 本）を受けて投入した。
- `qsub job.pbs` が exit 32 で拒否された: `Job rejected: -A option (project name) must be specified`。job は作られず、投入数は 0/2。
- 既存の人の決定（2026-09-30）は `-A NBBG`。一方、今回の承認は「課金属性なし」を前提にしているため、NBBG への課金を人の決定として確認する。
- 記録は外部資料の `exercise.attempts[0]` にある。

## 2 回目の投入試行（2026-10-19T16:47Z、run 01M5AGXYW1ZM5ZV17SHMB560DY）
- `-A NBBG` の承認を受けて `qsub -A NBBG job.pbs` を実行したが、exit 32 で拒否された: `Specify chunks as multiples of 'ncpus=24:mem=124gb:ngpus=1'`。job は作られず、投入数は 0/2。
- `qstat -Qf` を読んだ。debug と gen の下限はどちらも ncpus=24・mem=124gb・ngpus=1。1 core・GPU なしの job は sirius では投入できない。
- 承認範囲（GPU 不可）の外なので止めた。case の固定条件（ncpus=1）も、観測される ncpus=24 と食い違う。
- 記録は外部資料の `exercise.attempts[1]` にある。

## 証拠コマンド
- `python3 crates/campaign/scripts/verify-short-ml-case.py --check` → exit 0（状態整合のみ）
- `python3 crates/campaign/scripts/verify-short-ml-case.py --acceptance --evidence <上記 json>` → exit 1（pending なので非 0 が正しい）
- `python3 crates/campaign/scripts/verify-short-ml-case.py --placement-check` → exit 0

## 未解決事項
- 人の決定 sirius-full-vnode-case: 1 vnode（24 core・124 GB・GPU 1、walltime 600 s、-A NBBG、1 本、予備 1）で投入してよいか。あわせて、case の固定条件を ncpus=24 に改訂し pin を更新してよいか。
- 評価側の本番 collect 経路（`observe::SchedulerTransport` の ssh `qstat -x -f` 実装と CLI からの呼び出し）が未実装。承認後に限定配線が要る。
