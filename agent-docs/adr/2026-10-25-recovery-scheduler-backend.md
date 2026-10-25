# 復旧比較 harness の PBS 接続契約

---
tasks: [01M5SRQ6DAEA8RMHFENY699T5B]
---

- 日付: 2026-10-25
- 状態: 接続契約を採用。実装・実ジョブ測定は後続段。本書は投入承認ではない。
- 範囲: P0-3 の contract。P0-1/P0-2 の task 完了をこの契約の依存にしない。

## 根拠と維持する成果

[SPEC](../../docs/SPEC.md) §3.6–3.7 に従い、既存認可内で手元を編集し、クラスタは実行・観測に使う。
[現行 harness 手順](../../docs/ops/dag-fault-comparison.md)、[cluster wait](0090-durable-wait-for-cluster-jobs.md)、
[submit audit](2026-10-23-a-submit-audit-and-collect-classes.md)、
[成功契約](2026-10-23-campaign-success-contract.md) を置き換えない。
K4 のセル採点、成功/human-op 集計、A の job ID/collector、graded attempts、独立 process の durable A2 の5成果は維持する。
過去の raw・grade・bundle は書き換えず、実 scheduler の結果を stub の過去測定へ混ぜない。

## D1. 再利用面と最小変更範囲

| 現行資産 | 再利用する契約 | 後続実装に必要な差分 |
|---|---|---|
| `crates/campaign/src/dagbench/ext.rs` の `ExtBackend` | 両 arm 共通の submit/lookup/wait/consume | PBS adapter と永続 submit intent。`Lookup::Unknown` を再送許可にしない境界を追加 |
| `crates/campaign/tests/dagbench_harness.rs`、`tests/common/dagbench_durable*.rs` | 共通 Params、親による wait 読み直し、controller 再起動、grading pin | 実 scheduler の opt-in 入口と adapter 注入。既定の offline 試験はネットワークを呼ばない |
| `crates/campaign/src/dagbench/{baseline,celeris_side,durable,graded,metrics}.rs` | attempt の永続消費、validated_completed、A2 証跡検証 | 実時計・実 scheduler raw の明示的な読取分岐。論理時計の完了推定を使わない |
| `crates/campaign/src/observe.rs`、`ssh_qstat.rs` | `pbs:sirius:<job_id>`、scheduler 原票、trial marker、raw hash、収集失敗分類 | 未知 job ID の marker 検索は新規。SshQstat の既知 ID 照会には全体 timeout がなく、有界 transport を用意 |
| `crates/campaign/src/harness_exec.rs`、`scripts/campaign/celeris-qsub` | `celeris.submit-audit/1` → track → 台帳、受理済み参照 | 現行 wrapper は応答喪失を照合しない。これを exactly-once 実装として再利用しない |
| `crates/task-dispatch/src/dispatcher/cluster_job_wait.rs` | 通常 SQLite wait、slot 解放、再開、notice 重複除去、PBS/Slurm poller | 原則変更なし。測定用 dispatcher の poller 注入点を使う。`gone` による wait 解放と研究の成功を分離 |
| `crates/task-worker/src/ssh.rs` | BatchMode、ControlMaster、setup/env、timeout、手元編集 | 原則変更なし。測定専用 path の同期だけ。既存 remote worktree を直接編集しない |

主な変更先を campaign の adapter・正式 harness・raw 検証・対応する fixture 試験に限定する。
daemon/store に LLM を入れず、一般 task 状態機械・本番 DB schema の refactor は行わない。
新接続で旧 submit 経路を置き換える箇所には二重投入経路や互換 flag を残さない。
ExtSim は offline 試験の別 backend として存続する（PBS の fallback にはしない）。

## D2. 対象 scheduler と両 arm 共通条件

最初の対象は **PBS / Sirius / gen / NBBG**。理由は既存 PBS collector・wrapper と既存会計群の運用実績。
Slurm 対応や複数クラスタ比較は今回含めない。認証・残枠が未確認なら offline 接続準備までとする。
選定は利用可能性の保証ではない。現時点の認証と承認不足は[進捗](../progress/2026-10-25-ieee-recovery-scheduler.md)参照。

最初の接続確認は無 LLM の小さな決定的 job とし、baseline / celeris に同一 payload bytes、入力 hash、
出力 hash 検査、grader binary/版、module/env、queue/account、select/place、walltime、poll/deadline を渡す。
job は測定専用ディレクトリに固定入力のコピーと digest を出す。CPU stress・BenchFS の危険な既知 hang 再現はしない。
過去 K3 の解析結果を新 job の成果物として採点しない。独立 grader は新 job の出力と scheduler 原票を照合する。

接続確認の上限契約（承認額ではない）:

- 1 vnode = `select=1:ncpus=24:mem=124gb:ngpus=1`、物理 host を占有する `place=exclhost`、walltime 300 秒。
  GPU は使用しない。queue がこの条件を拒否すれば縮退・別 queue への自動変更はせず停止する。
- 外部 job 同時枠 1、worker 枠 1、poll 30 秒、試行の実経過 deadline 1800 秒、SSH 1 回の全体 timeout 60 秒。
- seed 17、rep 1、normal と A2 の各条件を両 arm に1回ずつ、合計最大4 job。
  最大4物理 host × 300秒 = 1/3 host-hour の予約上限（PBS課金単位との換算は投入前に照合）。
- workload attempt 上限1。再起動・poll・回収・再採点は同じ attempt/job を継続し、枠を補充しない。
  新 payload の実行を必要とする失敗は停止。後続の retry 比較は別の固定条件・承認枠とする。
- A2 は同一 arm 内で同じ job ID を再開する。両 arm が物理 job を共有する意味ではない。
  両 arm に同じ資源条件を与え、別々の job を逐次実行する。

実行前にこの条件の payload・case・grader・環境・資源記述を hash で pin する。
値を変えるときは新 measurement_id と事前の契約更新が必要。既存主比較 A0/B0 の model/harness 比較とは別 dataset とする。

## D3. identity と raw の境界

P0-2 の最終 freeze は未取得。暫定契約の正本は `observe::trial_identity`、`TrialKey::idempotency_key()`、
`HarnessExecutor::TrialMarker` と[human-op 定義](../../docs/ops/eval-raw-record-definitions.md)。
次の対応を保存し、別 arm・別試行との衝突を拒否する。

| 意味 | 固定する値/対応 |
|---|---|
| 研究資産 | campaign_id、正規化 manifest hash、case/grader/input/payload/env の完全 SHA256、採用 source と binary hash |
| collector identity | `campaign_sha256, config, problem_id, rep, trial_no`。config は既存 `ConfigId` 表記 |
| harness marker | `campaign_id, manifest_sha256, arm, case, rep, account_id`。collector の case=problem_id、manifest_sha256=campaign_sha256 を照合 |
| 復旧比較 | 一意の measurement_id、arm=`baseline` または `celeris`、scenario/point/seed/condition、controller epoch |
| 結合 | trial_key は既存 TrialKey から生成。baseline→Config A、celeris→Config B を recovery 専用 manifest に明示。主比較と同じ campaign_id/out は使わない |
| 外部操作 | logical_job_id、node、workload_attempt、submit_intent_id、`job_ref=pbs:sirius:<完全なPBS ID>`、wait_id。PIDをjob IDにしない |

controller epoch は再起動で増えるが trial_key・logical_job_id・attempt・job_ref は変えない。
複数外部 node を区別する submit_intent_id は上記 identity と node/attempt から決定的に作り、PBS に
`CELERIS_TRIAL_KEY` と `CELERIS_SUBMIT_INTENT` を渡す。job名の短縮・部分一致を照合根拠にしない。

新しい実 scheduler raw は **`celeris.dagbench-scheduler-raw/1` / dataset=`scheduler_measured`** とする（未実装）。
`run.json` に上記 identity/pins、clock=`wall`、資源、各予算、LLM calls=0、実 job 数を記録する。
同時に `submit-intents.jsonl`、`scheduler-events.jsonl`、`process-events.jsonl`、`durable-wait.json`、
既存の worker/control 記録、attempt 別成果物・grade、取得時刻付き qstat 原票と hash 一覧を保存する。
JSONL は seq・UTC・observer の単調時計/epoch・identity・根拠参照を持つ追記専用。欠測は null と理由を残す。
出力先は新規のみ。集計は raw から再生成し、旧 `celeris.dagbench-durable-raw/1` の論理秒を実秒と読み替えない。
既存 `celeris.observed-jobs/1` と `celeris.submit-audit/1` は副記録としてそのまま結合し、collector 不一致を拒否する。
新 raw の reader/validator が完成するまでは現行 metrics CLI に渡さない。

独立採点成功と実行終了を分け、完遂は `validated_completed` のみ。
qstat の `F/Exit_status=0` だけでは完遂にならず、原票・identity・成果物 hash・独立 grade の全てが必要。
`gone`、取得失敗、履歴消失、採点入力欠落は未観測であり、成功/重複0/介入0へ補完しない。
共通 human-op schema の enum は今回は拡張せず、主比較専用の experiment_set を復旧比較に偽装しない。
共有作業は一度だけ、費用/時刻/操作の欠測は null。P0-2 と共通化するのは identity と記録の意味で、結果の分母は分ける。

## D4. unknown submit: 照合できるまで送らない

1. 単一 submit owner が identity・資源/payload hash・intent を durable に確定してから qsub を1回だけ送る。
   同じ intent の並行実行を排他し、送信前後のどちらで停止しても intent から再開する。
2. 完全な PBS ID を受け取れば受理記録を sync し、`qstat -x -f <id>` の marker/所有者/資源/投入先を照合する。
3. timeout、切断、ID 不正、受理後の台帳書込み失敗は **unknown_submit**。単なる `NoSlot` や未投入として再送しない。
4. operator が許可した自分の job に限定し、PBS の実行中と履歴から trial marker + intent marker を完全一致検索する。
   具体的な検索 capability（qselect/履歴保持/Variable_List 可視性）は接続後に read-only で確認する。
   **現行 SshQstat は既知 ID のみで、この検索は未実装**。
5. 候補1件かつ owner/cluster/account/queue/資源/payload 対応が一致すれば、その job ID を台帳へ追記して再追跡する。
   0件は「未投入」の証拠ではない。2件以上・不一致・履歴欠落・権限不足・検索timeoutは未解決として停止する。
   30秒間隔、試行 deadline 内でのみ再照合。deadline 到達後は raw を保存し、人の解決を待つ。
6. 自動 qsub 再送、qdel、一括取消は禁止。確実な送信前拒否だけは未投入として記録できるが、この接続確認の失敗時は停止する。
   未解決 job の予約資源は残枠へ戻さない。人が解決しても旧試行は書き換えず、判断証跡を追記する。

PBS に一般の冪等投入保証があるとは仮定しない。wrapper の stdout だけを成功根拠にせず、
unknown submit を `not_submitted`（モデル失敗）に誤分類しない。これは接続 adapter の観測不確定として停止させる。

## D5. 所有権と A2 の停止境界

測定 runner は自身が起動した controller child（PID＋starttime/epoch を確認）だけを停止できる。
本番 daemon、SSH master、PBS server、compute host、他者 job は停止対象でない。
PBS が外部 job を所有し、独立 observer が停止中にも qstat 原票を取得する。controller 自身の自己申告では証明しない。
submit owner の intent 台帳と observer の証跡は controller の寿命から独立させる。
Celeris の task/wait の真実は測定専用 SQLite、baseline は既存の sync/rename state。

A2 は親が SQLite/state から **job ID付き Waiting の保存済み状態**を読み直した後にだけ発火する。
順序は保存確認→所有 child の停止/回収→停止中の同一job観測→再開→同一job成果物回収→独立採点。
再開時の新 qsub は不合格。同一jobの投入/実行・終了・回収の証跡を別々に保存し、単なるpoll回数を起動数に数えない。
PBS rerun の有無も観測し、job ID が同じだけで物理実行1回とは主張しない（投入は rerunnable=false を要求）。
停止中に job が終わった証拠が取れなければ A2 成立を主張しない。故障注入は出来事待ちを使い、busy loop を置かない。

## 後続段の受入境界

offline では応答喪失・0/1/複数候補・履歴消失・不一致・timeout・再起動・二重submit競合・grade失敗の fixture を検証する。
実接続は認証/残枠確認後にのみ実行し、normal/A2 両 arm の同じ資源/pins、永続 wait、停止中原票、同一job回収、
grade、raw 再集計一致が揃って初めて実HPC復旧の証拠とする。この contract 完了は実測完了を意味しない。
