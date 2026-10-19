---
task: 01M5AFX5TQKG3WAEYD1E0XQNMS
wu: exercise-case
status: done
completed: 2026-10-19
---
# exercise-case: 46040.pbs で正例 1・独立負例 3 が成立し adopted（dev 専用）

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

## 2 本目投入前の再確認（2026-10-19T17:30Z、run 01M5AK2AF1A6FG41N0FN1A0PH6）
- 決定 sirius-env-lock-rerun は承認された。投入前に compute node の環境を確かめたが、python 版は測れなかった。
- login sirius02: `/usr/bin/python3 -> python3.9`（3.9.25、numpy 1.23.5 は RHEL rpm）、`python3.12`（3.12.14、numpy 2.1.3）。compute a02 の numpy 2.0.2 はどちらとも合わない。
- node への ssh は不可で、過去の job log にも compute の python 版は無い。承認文は「login の値は使わない」なので、lock の python を投入前に実測で埋められない。
- 最後の 1 本を推測値で賭けず、python の固定方法を人の決定 sirius-compute-python-lock に上げた。repo の case・lock・pin・job.pbs は変えていない。投入は 1/2 のまま。記録は外部資料の `exercise.attempts[3]` にある。

## 2 本目の投入（2026-10-19T17:33Z、run 01M5AKGJB8DX5DBRHFC29Q7S1J）
- 人の決定 sirius-compute-python-lock は measure-in-job。env.lock の requested を numpy 2.0.2（a02 の job 46039 が記録）・python null（job が記録）に改訂し、`measure_in_job` 欄を足した。login node の値は使っていない。
- job.pbs は hostname・python 完全版・numpy を stdout に出し、numpy が 2.0.2 でなければ exit 1。job.py の後に完了行を出す。job.py は不変。
- pin を更新した（materials efa7308c…、case.json 2c18e541…、env_lock 2ca5663b…）。試験は short_ml_assets.rs の lock 期待値、test_short_ml_acceptance.py の env_measured を lock から読む形に直した。
- 実行直前の再確認: debug Que 0 Run 0、自分の job なし。staging の sha256 は repo と一致。
- `qsub -A NBBG job.pbs` → exit 0、46040.pbs（投入 2/2、上限到達）。17:35:17Z の `qstat -x` は F。
- result.json に wait(cluster_job, sirius, 46040.pbs) を書いて終えた。回収・lock の python 完全版への書き直し・pin 更新・grader は再開 run で行う。記録は外部資料の `exercise.attempts[4]`。

## 回収・grader・採用（2026-10-19T17:40–17:55Z、run 01M5AM6665YYYESWWSFTZBCFB5）
- `qstat -x -f 46040`: F、Exit_status 0、exec_host a02/2*24、queue debug、select 1:ncpus=24:mem=124gb:ngpus=1、walltime 1 s。
- job の stdout（`.o46040`）: `host a02` / `python 3.9.25` / `numpy 2.0.2` / `done job.py exit 0`。raw 3 file は sirius 側と sha256 一致（coefficients ce9fd626…、metrics d2f612aa…、predictions 23578cb2…。46039 と同値）。
- env.lock を job の実測（python 3.9.25・numpy 2.0.2、`measured_in_job` に job id・host・log hash）に書き直し、contract の env_lock pin を 6f181132… に更新。short_ml_assets.rs の期待値も合わせた。
- 残枠の再確認: main b67bab68 の mlsys 進捗（2026-10-18〜19）に本測定・LLM 呼出しの消費なし → 残り 2,512,372 token。本 case は 0 token、job 2/2（NBBG、計 21 vnode 秒）。
- `collect-observed --job pbs:sirius:46040.pbs` → exit 0、origin live、条件 queue debug・ncpus 24。
- 通常 grader: 正例 exit 0（final_pass、evidence・recompute pass）。miscalc exit 1（recompute fail: metric coef_x1）、wrong-condition exit 1（evidence fail: condition）、unexecuted exit 1（evidence fail: not recorded by the evaluator）。各負例は他方の check が pass。出力は外部 `ml-case-run-46040/trials/`。
- status.json を adopted（decided_at 2026-10-19、case_dir short-ml-campaign、claims.adopted true のみ）。外部 evidence に `adoption` 節を追加。docs/ops/short-ml-campaign.md と test_short_ml_state.py を adopted に合わせた。K2/K4・arm・grader・loader・manifest は不変。

## 証拠コマンド
- `python3 crates/campaign/scripts/verify-short-ml-case.py --check` → exit 0（状態整合のみ）
- `python3 crates/campaign/scripts/verify-short-ml-case.py --acceptance --evidence <上記 json>` → exit 0（`ok: acceptance passed (4 grader runs re-executed)`）
- `python3 -m unittest crates/campaign/scripts/test_short_ml_acceptance.py crates/campaign/scripts/test_short_ml_state.py` → 46 OK
- `cargo fmt --all -- --check` → exit 0、`cargo clippy --workspace -- -D warnings` → exit 0
- `python3 crates/campaign/scripts/verify-short-ml-case.py --placement-check` → exit 0
- `cargo test -p celeris-campaign --test short_ml_assets` → 11 passed

## 未解決事項
- env.lock の `state` は schema 上 `requested_unverified` のまま（試験と verify script がこの値を前提にする）。値は job 内実測で、評価側が `.o` と照合済み。state 名の変更は提案に回す。
- 承認上限 2 本を使い切った。再実行には新しい承認が要る。

## 提案
- env.lock に `state: measured_in_job` を正式な状態として足し、verify script の env 照合を `.o` の版行から直接読む形にする（今は adoption.env_measured を人手で写す）。
