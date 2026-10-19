# 短い ML 研究キャンペーン case: 運用手順（状態 pending）

---
tasks: [01M59XY99GX1R6HCPS1H06N2KZ, 01M59KY5FTRJTJ80PACNW3K533]
---

状態: **pending**（採用でも延期でもない）。ラベル dev、確認点 **2026-10-22T00:00:00Z**。
判断材料・比較・旧判断の撤回履歴は repo に置かない。外部資料を参照する:

- `/local/celeris/data/workspaces/01M59KY5FTRJTJ80PACNW3K533/artifacts/ml-case-reassessment.json`（機械向け正本）
- `/local/celeris/data/workspaces/01M59KY5FTRJTJ80PACNW3K533/artifacts/ml-case-review.md`

この文書は決定後の操作だけを書く。効果・final 汎化・実 pilot は主張しない（status.json の `claims` は全て false）。

## 実装済・検査済・実観測未実施・承認待ち

| 区分 | 内容 |
|---|---|
| 実装済 | status.json（pending）と状態整合検査 `--check`、受入検査 `--acceptance`、dev case `cases/dev/short-ml-campaign/`、固定入力、環境 lock、grader pin（contract.json）、評価側観測 `observe::collect_for_trial` |
| 検査済 | 決定的な試験のみ（資産・pin・入力隔離・観測の拒否・受入 CLI の期限境界／tamper／未実行拒否）。時計を注入した試験は synthetic 証拠で、本番の延期証拠ではない |
| 実観測未実施 | sirius の実 job、実 PBS 終了記録、環境の実測（python 3.9.25 / numpy 1.23.5 は `requested_unverified`）、正例 1・独立負例 3 の実 job に対する通常 grader 実行 |
| 承認待ち | 実行前の人の承認（下記）。承認と現在残枠の再確認が済むまで実 job を投入しない |
| 実装済（評価側 collect） | read-only `ssh_qstat::SshQstat`（`ssh -o BatchMode=yes <cluster> qstat -x -f <id>`）と `celeris-campaign collect-observed`（cluster は case.json、条件は grade case から取る） |

## 所在

- 状態の正本: `crates/campaign/optional/short-ml-campaign/status.json`（`cases/` の外）
- 検査: `crates/campaign/scripts/verify-short-ml-case.py`
- dev case: `crates/campaign/cases/dev/short-ml-campaign/`（case.json・contract.json・env.lock.json・materials・kb）
- 固定入力: `crates/campaign/fixtures/inputs/short-ml-campaign/`（train.csv・eval-features.csv。sha256 は case.json の `inputs[]`）
- 評価側だけが持つ資料: `crates/campaign/fixtures/grader-private/dev/short-ml-campaign/`（trial に渡さない）
- grade case: `crates/campaign/fixtures/grade-cases/dev/short-ml-campaign/`（既存の `experiment` と `recompute`。grader の種別は増やさない）
- 経過: `agent-docs/progress/2026-10-19-short-ml-case/`

## 検査コマンドの意味

```
python3 crates/campaign/scripts/verify-short-ml-case.py --check
python3 crates/campaign/scripts/verify-short-ml-case.py --acceptance --evidence /local/celeris/data/workspaces/01M59KY5FTRJTJ80PACNW3K533/artifacts/ml-case-reassessment.json
```

標準ライブラリのみ、LLM・ネットワーク不使用。

- `--check`: **状態整合の検査**。pending / deferred / adopted のどれでも status.json の構造・外部参照・claims・配置を見る。通過は採用でも延期でもなく、**採用の合格に読み替えない**。pending のままでも整合を検査できる。
- `--acceptance --evidence <file>`: **受入の検査**。evidence の status で分岐する。
  - `adopted`: case・入力・環境 lock・grader の pin を実ファイルと照合し、評価側が実観測した job 記録（origin=live）と raw hash を確かめ、通常の grader CLI を新しい workdir で正例 1 と独立な負例 3（誤計算／比較条件違い／未実行）について再実行する。
  - `deferred`: 実時計が確認点以後で、期限後の再確認（実日時・実コマンド・exit・出力 hash）が外部ファイルと一致し、なお不成立であること。
  - `pending`・承認不足・未検証・未知の status は **非 0**（現在の pending 証拠は非 0 が正しい）。
  - 実時計は上書きできない。時計注入は test 専用で synthetic 証拠にしか使えない。

## 固定した条件（実行前契約）

- LLM 0 token、主比較の試行 0、sirius の debug queue で queue 下限の 1 vnode（select=1:ncpus=24:mem=124gb:ngpus=1、-A NBBG）・walltime 600 s の job 1 本（硬い上限 2）。job.py は単一スレッドのままで GPU は使わない（2026-10-19 人の決定 sirius-full-vnode-case = approve-full-vnode）、disk 100 MiB、case wall 1800 s。
- 失敗しても再投入せず、失敗を報告して止まる。
- 入力の隔離: eval の目的変数は評価側だけが持つ。trial は `train.csv` と `eval-features.csv` だけを受ける。
- 終了条件: `qstat -x -f` の `Exit_status=0` と `results/` の raw 3 file。
- 環境 lock と grader pin は contract.json / env.lock.json にある。実測値が lock の要求値と違えば停止する。

## 実行前の承認と残枠の再確認

実 job を投入する前に次を満たす。満たさなければ投入しない。

1. 人が sirius の 1 vnode job 1 本（-A NBBG）の投入を承認している。
2. token 枠の**現在の残り**を、調査時点の数字ではなく現在の出典（台帳）で再確認する。本 case の消費は 0 token・0 試行。
3. 環境を評価側が測定し、lock と一致する:
   `ssh sirius python3 -c 'import sys,numpy;print(sys.version_info[:3],numpy.__version__)'`

## 実 job の標準手順（ここでは実行しない）

編集は手元の worktree で行い、sirius では実行と検証だけを行う。

1. **提出**: case の materials（`job.py`・`job.pbs`）と `inputs/` を sirius の作業 dir に置き（`/work/NBB/rmaeda` 配下）、`qsub -A NBBG job.pbs` を 1 本だけ投入する。返る数値[.host] を job id として Celeris 台帳の `cluster_job_ids`（`scheduler:cluster:job_id`）に記録する。PID は job id にしない。
2. **終了観測**: ADR-0090 の `cluster_job` wait で終了を待つ。一般の日時待ちは未対応で、`sleep` や偽 job で待たない。
3. **収集（評価側）**: `qstat -x -f <job id>` の出力を `evidence/qstat-<job id>.txt` に保存し、`observe::collect_for_trial` で `<out>/observed/<problem>/<Config>-rep<N>.json`（`celeris.observed-jobs/1`、origin=live）を作る。refs は台帳の job id、条件（queue・ncpus）は case の固定値を使う。未実行・未終了・Exit_status 欠落・raw 欠落・投入先違いは記録されない。
4. **grade**:
   ```
   celeris-campaign grade --case crates/campaign/fixtures/grade-cases/dev/short-ml-campaign/short-ml-campaign.json \
     --artifact <artifact dir> --observed <observed json>
   ```
   これが通常 grader（`experiment` と `recompute`）の正例。独立な負例 3（誤計算・条件違い・未実行）は `--acceptance` が同じ観測から新しい workdir で再実行する。
5. **受入**: 実観測・承認・現在残枠・環境測定を evidence に反映し、`--acceptance --evidence <file>` が exit 0 になって初めて採用とする。

## 確認点 2026-10-22T00:00Z 以降

- 正例と独立負例が実 job で成立していれば、上記の受入で採用にする。
- 成立していなければ、確認点以後に実コマンドで再確認し、その実日時・コマンド・exit・出力 hash を evidence に記録して deferred とする。確認点より前に延期を断定しない。
- 早期に不成立となり確認点まで待つ必要が出た場合は、一般の日時待ちが未対応なので plan_issue を出す。

## 範囲の区別

- 本 case は dev ラベルで、final 汎化を意味しない。実 pilot でもない。
- 主比較（A0/B0）と Jev 副比較（B1/C1）は別測定 ID。既存 arm の実行器は保持し、本 case は触らない。
- K2 / K4、final case、manifest、grader の種別、loader は変更しない。
