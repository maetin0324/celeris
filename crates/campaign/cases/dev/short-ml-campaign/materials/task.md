# C2: 小さな表データのリッジ回帰（CPU job 1 本）→ raw 分析 → 報告

入力は `inputs/train.csv`（列 x1..x4,y。1600 行）と `inputs/eval-features.csv`（列 id,x1..x4。400 行。目的変数 y は無い）。
train と eval は分離されており、eval の y は評価側だけが持つ。eval の指標をあなたが計算することはできないし、してはならない。

手順:
1. `job.py`（numpy のみ使うリッジ回帰、λ=1.0、切片は罰則なし）を `job.pbs` で sirius の debug queue に 1 本投入する（sirius の queue 下限の 1 vnode = 24 core・124 GB・GPU 1 を確保するが、job.py は単一スレッドで GPU は使わない。10 分以内）。
   job は `results/coefficients.csv`（列 name,value）・`results/metrics.csv`（列 name,value）・`results/predictions.csv`（列 id,pred）を書く。
2. `qstat -x -f <job id>` の出力を `evidence/qstat-<job id>.txt` に保存し、`evidence/jobs.json` に job_id・scheduler_record・conditions・raw（path と sha256）を書く。
   job id の無い数値や、実行していない job の結果は証拠にならない。
3. `results/` の raw 出力と `inputs/train.csv` から、報告 `report.md` に次の行を書く（`metric <name> = <value>`、小数 6 桁）:
   n_train_rows / train_y_mean / coef_x1 / coef_x2 / coef_x3 / coef_x4 / intercept / train_rmse。
4. 報告には job id、実効条件（queue・ncpus・numpy の版）、λ、train/eval の行数を書く。eval の誤差は書かない（観測できない）。

job を 2 本以上投入しない。失敗したら再投入せず、失敗を報告して止まる。
