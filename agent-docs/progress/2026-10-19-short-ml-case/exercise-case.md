---
task: 01M5AFX5TQKG3WAEYD1E0XQNMS
wu: exercise-case
status: blocked
completed: null
---
# exercise-case: 46039.pbs は回収したが、compute node の numpy 不一致と空の log で正例不成立、人の決定待ち（状態は pending のまま）

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

## 3 回目の投入（2026-10-19T16:54Z、run 01M5AH9HM76KCHTC8171EGQE45）
- 人の決定 sirius-full-vnode-case は approve-full-vnode に訂正された（defer は取り消し）。
- case の固定条件を 1 vnode（ncpus=24・mem 124gb・ngpus 1）に改訂して pin を更新した。対象は case.json の params、contract.json の limits と pins、grade-case の conditions、job.pbs、task.md。job.py は変えていない。BLAS の thread 数は 1 に固定し、GPU は使わない。K2/K4・arm は変えていない。
- 合わせて、試験 fixture の ncpus を 24 に揃えた（short_ml_assets.rs・short_ml_observation.rs・test_short_ml_acceptance.py）。
- `qsub -A NBBG job.pbs` は exit 0 で、job id は 46039.pbs（投入 1/2）。直後の `qstat -x` では F だった。
- result.json に wait(cluster_job, 46039.pbs) を書いて終えた。collect・grader は再開 run で行う。
- 記録は外部資料の `exercise.attempts[2]` にある。

## 回収と grader（2026-10-19T17:05–17:15Z、run 01M5AHZ9RMX4G9HPC9C17F6MRC）
- `qstat -x -f 46039`: F、Exit_status 0、exec_host a02。raw 3 file・空の .o/.e・qstat 記録を外部の `ml-case-run-46039/artifact/` に回収した。
- 評価側 collect の限定配線を追加した。`crates/campaign/src/ssh_qstat.rs`（`SshQstat`、`ssh -o BatchMode=yes <cluster> qstat -x -f <id>`。cluster 名と id は検査してから渡す）と `observe::cli`（`celeris-campaign collect-observed`）。live の observed は exit 0 で作れた。
- 通常 grader の正例は exit 1: recompute は pass、evidence は fail（`log is empty`。job.py が stdout に何も書かない）。
- job の numpy は 2.0.2（compute node a02）で、env.lock の 1.23.5（login node で測定）と違う。contract の stop_on_mismatch に従い、ここで止めた。負例は未実行。
- 試験: 子 process 起動の許可リスト（runner_checker.rs）に ssh_qstat.rs を加え、SshQstat の拒否試験を追加した。

## 証拠コマンド
- `python3 crates/campaign/scripts/verify-short-ml-case.py --check` → exit 0（状態整合のみ）
- `python3 crates/campaign/scripts/verify-short-ml-case.py --acceptance --evidence <上記 json>` → exit 1（pending なので非 0 が正しい）
- `python3 crates/campaign/scripts/verify-short-ml-case.py --placement-check` → exit 0

## 未解決事項
- 人の決定 sirius-env-lock-rerun: env.lock を compute node 実測に改訂し、job.pbs に版と完了の stdout 記録を足して 2 本目（上限 2 の最後）を投入するか、延期するか。
