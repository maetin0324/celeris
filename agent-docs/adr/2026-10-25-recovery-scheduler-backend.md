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
短いjobが保存確認前に終了しないよう、両armで同じ完了解放barrierを使う。A2では親のcontroller終了確認後、
normalでは親のwait保存確認後にobserverが解放する。期限付きの出来事待ちとし、解放時刻と理由を原票に残す。
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
実測指標も `celeris.dagbench-scheduler-metrics/1` に分離する。queue待ち・外部実行・停止・回収・採点の
実時間を個別に出し、強制barrier待ちを自然なscheduler待ちとして解釈しない。CPU/RSSはcontrollerの観測範囲を明記する。

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

## 実装付記 2026-10-25

実装前に形だけを固める付記。上の D1–D5 は変えない。食い違えば D1–D5 が正で、この付記を直す。
対象は PBS / Sirius の1 scheduler・1 cluster のみ。Slurm の型・分岐は作らない。

### I1. module 配置

新規は `crates/campaign/src/dagbench/pbs/` の4 file と `mod.rs`（`dagbench/mod.rs` に `pub mod pbs;`）。

| file | 持つもの | 再利用 |
|---|---|---|
| `pbs/transport.rs` | `PbsTransport` trait、`SshPbs`（実装）、`PbsCall`/`PbsReply` 原票型 | `observe::SchedulerTransport`・`Origin`・`SchedulerReply`、`observe::is_scheduler_job_id`、`observe` の PBS `key = value` パーサ（`pub(crate)` に上げて共用。複製しない） |
| `pbs/intent.rs` | submit intent 台帳（`submit-intents.jsonl`）、排他、`submit_intent_id` の生成、状態の再生 | `ext.rs` の `drop_torn_tail` 方式（壊れた末尾行の切り捨て）と seq 復元 |
| `pbs/backend.rs` | `PbsExt`（`ExtBackend` の PBS 実装）、`RecoverySpec`、marker 照合 | `observe::Expect`/`observe_job` の照合規則（cluster・job id・`CELERIS_TRIAL_KEY`・conditions） |
| `pbs/raw.rs` | `celeris.dagbench-scheduler-raw/1` の writer と validator、trial 結合 | `observe::trial_identity`・`TrialKey`・`CollectClass`・`record_failure`、`harness_exec::SUBMIT_AUDIT_SCHEMA` |

子 process（ssh）を起こすのは従来どおり `ssh_qstat.rs` と新しい `pbs/transport.rs` の `SshPbs` だけ。
`dagbench/{baseline,celeris_side,durable,graded,metrics}.rs` は I4 の境界追加以外は変えない。
`Lookup`/`SubmitError` の variant 追加に伴い `PROTOCOL_VERSION` を上げる（実装は 2）。`ipc.rs` の server を
`Box<dyn ExtBackend + Send>` の所有へ広げる案は I12 で取りやめた（server は `ExtSim` 固定のまま。PbsExt は入口から
同じ process で使い、真実は file の intent 台帳・消費台帳に置く）。

### I2. `PbsTransport`

```text
trait PbsTransport {
    fn origin(&self) -> Origin;                       // Live / Synthetic を raw に残す
    fn qsub(&self, req: &QsubRequest) -> PbsReply;    // 1 回だけ実行。再試行しない
    fn qstat_full(&self, job_id: &str) -> PbsReply;   // qstat -x -f <完全なID>
    fn find_by_marker(&self, m: &MarkerQuery) -> PbsReply; // 自分の job のみ、実行中＋履歴
}
PbsReply = { call, started_utc, finished_utc, exit: Option<i32>, stdout, stderr, outcome }
outcome  = Ok | Timeout | SpawnFailed | NonZero | Unsupported
```

- 全 call は transport 側で全体 timeout（D2 の 60 秒、`RecoverySpec` から渡す）を持つ。子 process は timeout で
  SIGKILL して回収し、`Timeout` を返す。読み取り途中の stdout も原票に残す（推定で補わない）。
- `SshPbs` は `ssh -o BatchMode=yes -o ConnectTimeout=<n> -o ControlMaster=no -- <cluster> <cmd>`。利用者の ssh 設定の
  ControlPath にある既存 master は使うが、transport 側で master の起動・停止をしない（D5）。引数は既存 `SshQstat` と同じ文字種検査を通す。
- `qsub` の引数は `QsubRequest`（spec 由来の `-q -A -l select/place/walltime -r n -N -v CELERIS_TRIAL_KEY=..,CELERIS_SUBMIT_INTENT=..` と
  script path）から組み立て、自由文字列を受けない。
- `find_by_marker` の具体 command（`qselect -x -u <user> -v ...` か `qstat -x -f -u` の走査か）は D4.4 のとおり接続後の
  read-only 確認で決める。それまでは trait と偽実装だけで、`SshPbs::find_by_marker` は「未確認」を返す
  （`NonZero` 扱いではなく専用の `outcome = Unsupported` とし、照合不能として停止する）。
- 既存 `SshQstat` は評価側 collector（`observe::collect`）の既知 ID 照会として残す。recovery の収集は
  `PbsTransport::qstat_full` を `SchedulerTransport` に写す薄い adapter で `observe_job` に渡し、パーサと照合規則を共有する。

### I3. submit intent 台帳と状態遷移

`submit_intent_id = "si-" + hex(sha256(canonical JSON {schema:"celeris.submit-intent-id/1", measurement_id, arm,
trial_key, logical_job_id, node, workload_attempt}))`（64 桁全体）。再起動・controller epoch で変わらない。
PBS に渡す marker も完全な値で、job 名（`-N`）は表示用であり照合に使わない。

台帳は raw の `submit-intents.jsonl`（追記専用、1 行 = 1 遷移、`seq`・UTC・observer epoch・`submit_intent_id`・
`state`・根拠の原票参照）。書くたびに `sync_data`。排他は同じ dir の `submit-intents.lock` に
`std::fs::File::lock`（排他 lock）を取り、qsub の前後の記録を同じ lock 区間で行う。lock は投入する process
（`PbsExt` を持つ入口）が取り、別 process・別 thread からの同じ intent の投入も lock で直列になる（I12）。
`resolve` の再照合の待ちは lock の外で行い、照会のたびに lock を取り直して台帳を読み直す。

```text
intent_recorded ──(lock 内で qsub を 1 回)──> sent
sent ──(完全な PBS ID を受け、accepted を sync)──> accepted ──> 以降は wait へ
sent ──(起動していないことが確実)──> rejected_before_send（停止扱い）
sent ──(timeout・切断・非0・ID 不正)──> unknown_submit
（accepted の書込み失敗は Err で抜け、台帳は sent で終わる。次の owner が再開時に unknown_submit へ進める）
unknown_submit ──(marker 検索で候補1件・全項目一致)──> reconciled（job_ref を追記）
unknown_submit ──(0件・検索/照会の timeout・切断)──> reconcile_retry（原票を保存、poll 間隔後に再照合）
unknown_submit ──(2件以上・不一致・履歴欠落・権限不足・Unsupported・応答不正・deadline 到達)──> unresolved_stop
accepted/reconciled の job の qstat_full が marker/資源で不一致 ──> 台帳には書かず、PbsExt が照会のたびに
  Lookup::Unresolved / SubmitError::Unresolved を返して停止（I13）
```

- 再起動時は台帳を再生する。最後が `intent_recorded` なら「送ったか不明」なので `sent` と同じく unknown_submit へ
  進める（送信前停止と送信後停止を区別できないため。qsub を送り直さない）。
- `sent` の行は qsub を起動する直前に sync する。qsub の起動自体が失敗した（`SpawnFailed`、PBS に届いていない）
  ことが transport で確実なときだけ `rejected_before_send` を記録できるが、D4.6 によりこの接続確認では停止する。
- `unknown_submit` からの再照合は poll 間隔（30 秒）ごと、試行 deadline 内だけ。`unresolved_stop` は終端で、
  以後この intent に qsub も qdel も出さない。人の解決を表す `human_resolution` 行は未実装（I13。既存行は書き換えない）。
- 同じ `submit_intent_id` で 2 本目の `intent_recorded` は書かない（lock 内で既存状態を見て、既にあれば再生へ回す）。
  これで二重 submit 競合（2 controller・再起動直後の重複呼び出し）は物理 qsub 1 回に収まる。

### I4. `ExtBackend` の境界

- `Lookup` に `Unresolved { submit_intent_id, reason }`、`SubmitError` に `Unresolved { .. }` を足す。
  PbsExt は unknown_submit/unresolved_stop の intent について **`Lookup::Unknown` を返さない**（`Unknown` は
  台帳に intent が無い＝まだ一度も投入していない、の意味に限る）。
- baseline の `reconcile`/再開照会と celeris_side の応答喪失分岐は、`Unresolved` を受けたら `SubmitPending`
  （再投入）へ戻さず `Error` で停止し、raw に停止理由を残す。ExtSim はこの variant を返さないので既存の
  offline 試験・成功契約は変わらない。
- `advance(now)` は PbsExt では実時計で qstat_full を poll し、終了を `Notice` にする。`due_secs` は `None`
  （論理時計で完了を推定しない、D1）。`now` は observer の単調時計秒。
- `record_consumed` は台帳と別の `consumed.jsonl` に `(job_ref, workload_attempt)` を1回だけ書き、2 回目は
  `false`（既存の重複消費 0 判定と同じ意味）。
- unknown な投入を `CollectClass::NotSubmitted`（model_failure）に写さない。trial の収集分類は
  `Unobserved`（照合不能）か `IdentityMismatch`（不一致）で、`failure_class` は `Undetermined`。

### I5. 両 arm 共通の spec

`RecoverySpec`（serde、`deny_unknown_fields`）を1つの JSON から読み、その sha256 を `run.json` の pin にする。

```text
RecoverySpec {
  measurement_id, scheduler:"pbs", cluster:"sirius", queue:"gen", account:"NBBG",
  resources { select:"1:ncpus=24:mem=124gb:ngpus=1", place:"exclhost", walltime_secs:300 },
  rerunnable:false, payload { script_sha256, input_sha256, expected_output_sha256 },
  grader { binary_sha256, version }, env { modules, env_sha256 },
  limits { external_slots:1, worker_slots:1, poll_secs:30, trial_deadline_secs:1800,
           ssh_timeout_secs:60, max_jobs:4, workload_attempts:1 },
  conditions:[normal, a2], seed:17, rep:1 }
```

- `RecoverySpec::qsub_request(arm, trial_key, logical) -> QsubRequest` と `RecoverySpec::intent_spec(arm, trial_key, logical)
  -> IntentSpec`（照合は `IntentSpec::check_record`。`observe::Expect` は作らない）が baseline/celeris の両方に同じ値を返す。arm で変わるのは identity（arm・trial_key・intent）だけで、資源・
  終了（walltime・deadline）・attempt 上限は arm を引数に取らない関数で作る。
- 既定値は持たない（欄の欠落は読み込みエラー）。値の変更は新しい measurement_id（D2）。

### I6. trial への結合と再収集

- 1 trial の結合行（raw の `trials.jsonl`）は `trial_key`、collector identity（`observe::trial_identity`）、
  `submit_intent_id`、`job_ref`、qstat 原票の file 名・sha256・取得 UTC、投入先（cluster/queue/account）、
  attempt 別成果物の sha256、grade 参照、`CollectClass`（成功時は null）を持つ。
- validator（`raw.rs`）は、`run.json` の pin と spec hash、intent 台帳の再生結果、`scheduler-events.jsonl`、
  原票 hash 一覧、成果物 hash、grade を突き合わせ、job ref と trial の不一致・原票欠落・hash 改変・
  `unresolved_stop` を含む trial の成功扱い・`validated_completed` 以外の完遂を拒否する。旧 `celeris.dagbench-durable-raw/1`
  は受けない（schema で分岐し、読み替えない）。
- 再収集は read-only: qstat_full と成果物の読み直しだけで、`PbsTransport::qsub` も `record_consumed` も呼ばない
  （collector は transport を `qstat_full` だけの view で受け取り、型で qsub を呼べないようにする）。
  取得は毎回新しい原票として追記し、前回の原票・結合行を上書きしない。結合行は最新の取得を参照する新行を足す。

### I7. `celeris-qsub` との関係

- recovery harness の投入経路は PbsExt → `PbsTransport::qsub` の1本だけ。payload の job script は qsub を呼ばず、
  recovery 入口は `CELERIS_SUBMIT_CMD`/`CELERIS_SUBMIT_AUDIT` を payload 環境へ渡さない（設定されていれば起動を拒否）。
- `scripts/campaign/celeris-qsub` は主比較 A arm（LLM 試行内の投入）の wrapper として変えずに残す。recovery の
  exactly-once 投入には使わず、共有もしない（応答喪失の照合を持たないため、D1）。wire-entry unit で wrapper 冒頭の
  注記に「recovery 比較の投入には使わない」を加える。
- recovery の受理記録は intent 台帳が正本で、`celeris.submit-audit/1` 形式の副記録は台帳の `accepted`/`reconciled`
  から導出して書く（D3 の副記録の結合を保つ）。wrapper と台帳の両方が同じ job を記録する経路は作らない。

### I8. offline fixture（偽 transport）

偽 `FakePbs` は台本（呼び出し順の応答列）と物理投入数・消費数のカウンタを持ち、試験は数を assert する。
ssh・qsub・外部ネットワークは使わない。時間は注入時計で進め sleep しない。

| fixture | 台本 | 期待 |
|---|---|---|
| 正常 | qsub → `123.sirius`、qstat 実行中→`F`/Exit 0、成果物 hash 一致、grade 合格 | `accepted`、`validated_completed`、qsub 1・消費 1 |
| 応答喪失→1候補 | qsub `Timeout`、marker 検索 1 件一致 | `unknown_submit`→`reconciled`、qsub 1・同じ job を回収 |
| 応答喪失→0候補 | qsub `Timeout`、検索 0 件 | `unresolved_stop`、qsub 1、not_submitted にしない |
| 複数候補 | 検索 2 件 | `unresolved_stop`、qsub 1 |
| 不一致 | 候補1件だが queue/資源/account/owner が違う | `unresolved_stop` |
| 履歴消失 | accepted 後 qstat が unknown job | `Unobserved`、成功・重複0 に補完しない |
| ID 不正 | qsub stdout が PID 形・空 | `unknown_submit` |
| timeout（照会） | qstat/検索が `Timeout` 連続し deadline 到達 | raw 保存・`unresolved_stop` |
| Unsupported | 検索 capability 未確認 | 照合不能で停止 |
| 再起動（送信前後） | `intent_recorded`/`sent` 行で process 停止→再生 | qsub を送り直さず unknown_submit へ |
| A2 再開 | Waiting 保存後に controller 停止→再開 | 同じ job_ref を回収、新 qsub 0 |
| 二重 submit 競合 | 同じ intent を 2 経路から同時に submit（lock 待ちを出来事で同期） | qsub 1、2 本目は再生 |
| 原票欠落・hash 改変 | raw の qstat 原票を消す/1 byte 変える | validator が拒否 |
| job ID/trial 不一致 | 原票の Variable_List が別 trial_key | `IdentityMismatch` |
| collector 障害 | qstat_full `SpawnFailed`/`NonZero` | `FetchFailed`（env_failure） |
| grade 失敗 | 成果物 hash が expected と違う/grader 非0 | 実行終了でも `validated_completed` でない |
| 再収集 | 収集を2回 | qsub・消費の数が増えず、原票行が追記で増える |
| arm 共通 spec | 同じ spec から baseline/celeris の QsubRequest | identity 以外の欄が一致 |

### I9. この付記で決めないこと

実 command の検索手段（D4.4）、PBS 課金単位の換算、実接続の承認は接続後・人の決定に残す。

### I10. ext-pbs 実装での補足（2026-10-25）

I4・I5 の実装（`pbs/backend.rs`）で、付記に書いていなかった形を次のとおり決めた。D1–D5 は変えない。

- `RecoverySpec` に `owner`（`Job_Owner` の user・marker 検索の範囲）と `payload.script`（cluster 上の job script の絶対 path）を足す。
  intent の照合（`IntentSpec::check_record`）に要るため。`limits.workload_attempts` は 1 以外を読み込みエラーにし、`rerunnable=true` も拒否する。
- spec の hash は測定 dir の `recovery-spec.json`（pin、hard link で 1 回だけ作る）と照合する。両 arm は同じ測定 dir を使い、
  別 hash の spec では `PbsExt::open` が失敗する。pin の `start_unix` を `now`/`advance` の秒の起点にし、`advance(t)` は wall clock が
  開始 + t に届くまで注入時計で待ってから poll 間隔ごとに qstat する。
- identity の `node` は論理 job ID（scenario の外部 node ごとに一意）。`max_jobs` と `external_slots` は測定 dir 全体（両 arm）で数え、
  未解決の intent は枠を戻さない（D4.6）。
- 受理後の履歴消失（`Unknown Job Id`）・照会失敗は `Lookup` ではなく `io::Error`（未観測で停止）。原票が intent と食い違えば
  `Lookup::Unresolved`。`Lookup::Unknown` は台帳に intent が無いときだけ。
- 消費台帳は `consumed.jsonl`（lock 内で読み直し、`(job_ref, workload_attempt)` の 2 回目は `false`）。
- ipc server を `Box<dyn ExtBackend + Send>` に広げる変更（I1）は入口を作る wire-entry に残す。`PROTOCOL_VERSION` は variant 追加に合わせて 2 にした。

### I11. raw-collect 実装での補足（2026-10-25）

I6 の実装（`pbs/raw.rs`）で、付記に書いていなかった形を次のとおり決めた。D1–D5 は変えない。

- raw は `PbsExt` と同じ測定 dir に置く。`RawWriter::create` は `run.json`・`scheduler-events.jsonl`・`trials.jsonl` が
  まだ無い dir だけを受け（出力先は新規のみ）、`RawWriter::reopen` は本 raw の `run.json` がある dir だけを受ける（再収集）。
  `run.json` は収集後に `write_run` が1回だけ新規作成し、`real_jobs` は intent 台帳の受理・照合済み job の数、`llm_calls` は 0。
  spec に無い研究資産の pin（campaign_id・manifest・case・source・binary）は `StudyPins` で渡す。
- PBS 呼び出し 1 回 = `scheduler-events.jsonl` の 1 行。stdout は `scheduler/<seq>-<call>.txt` に新規作成し sha256 を残す。
  stdout が空なら file/sha256 は null、outcome が Ok 以外なら `missing_reason` 必須。
- trial_key は collector identity から既存 `TrialKey::idempotency_key()` で作り、arm→Config は baseline→A・celeris→B。
  結合行の collector 収集は既存 `observe::collect` を `QstatView` 越しに呼び、成功時の `celeris.observed-jobs/1` は
  `observed/<seq>.json`（collector の控えは `observed/c<seq>/`）に新規作成する。`submit-audit.jsonl` は台帳から導く。
- 結果（`status`）は `derive_status` が欄から導き、validator は同じ規則で再計算して行の値と食い違えば拒否する。
  `validated_completed` は受理/照合済み・収集成功・Exit_status 0・attempt 成果物 hash 全部・期待出力 hash 一致・
  grade 合格の全てが揃うときだけ。intent 無し・送信前拒否は `not_submitted`、それ以外の未照合は `unresolved`
  （収集分類は `Unobserved`。`NotSubmitted` にしない）。
- `metrics::compute` と `durable::verify` は本 raw の `run.json` を版で拒否する。

### I12. wire-entry 実装での補足（2026-10-25）

入口（`pbs/entry.rs`）で、付記に書いていなかった形を次のとおり決めた。D1–D5 は変えない。

- backend の選択は `entry::BackendChoice`（`ExtSim` が既定、`Pbs` は transport・時計を注入したときだけ）。
  `dagbench run` は `BackendChoice::ExtSim` で開き、`--backend` の既定は `ext-sim`。`--backend pbs` は CLI では
  実 transport を組まずに拒否する（Celeris arm は dev-dependency の task-api router が要り、実接続は人の承認後）。
  recovery の harness は `entry::run_trial` に `BackendChoice::Pbs` 相当の注入物（`TrialRun`）を渡して使う。
- I1 の ipc server の `Box<dyn ExtBackend + Send>` 化はしない。PbsExt の真実（intent 台帳・消費台帳）は file にあり、
  controller の寿命から独立している（D5）。A2 の「所有 controller の停止」は controller の incarnation を終える
  （baseline は `Baseline::run` を backend の誤りで抜け、Celeris は drive の future を捨てる）ことで表し、
  保存済み Waiting は controller の保存先（`state.json` / 測定専用 SQLite の cluster job wait）を読み直して確かめる。
  別 process での停止（PID・starttime の確認と SIGKILL）は実接続の段で `dagbench::process` の Supervisor に載せる。
- 境界は `entry::Gate`: controller が job ID を受けた後の次の backend 呼び出しで保存済み Waiting を確かめ、normal は
  完了解放 barrier を外し、A2 は止める。barrier と順序は trial dir の `barriers.jsonl`（`celeris.recovery-barrier/1`）、
  A2 の証跡は `a2.json`（`celeris.recovery-a2/1`: 保存済み Waiting・停止中の同 job 終了原票・再開前後の intent 行数・
  消費した job_ref）。停止中の観測原票は raw の `scheduler-events.jsonl` に入る。
- DAG は `fixtures/dagbench/recovery-scenarios.json` の `rcv-1`（外部 job 1 本、worker/外部枠 1）で、両 arm 共通。
  測定 dir 1 つに 2 arm × 2 condition = 4 job（D2 の上限）。
- 独立 grader（`entry::grade`）は attempt の成果物がちょうど 1 つで sha256 が spec の期待出力と一致すれば合格とし、
  `grade.json`（`celeris.recovery-grade/1`）を書く。
- 投入経路は intent 台帳 1 本: 入口は `CELERIS_SUBMIT_CMD`・`CELERIS_SUBMIT_AUDIT` があれば拒否し、
  `scripts/campaign/celeris-qsub` は `CELERIS_SUBMIT_INTENT` のある環境（recovery の job）では何も投入せず exit 4。
  A arm（印なし）の挙動は変えない。

### I13. close-progress での実装との突き合わせ（2026-10-25）

最終 unit で付記と実装（`crates/campaign/src/dagbench/pbs/`）を突き合わせ、食い違った箇所を上で直した。D1–D5 は変えない。

- I1: ipc server の `Box<dyn ExtBackend + Send>` 化はしていない（I12 の判断）。I1 の本文をそれに合わせた。
- I2: `SshPbs` の argv は `-o ControlMaster=no` を付ける（既存 master は ControlPath で使い、起動しない）。outcome に `Unsupported` を明記した。
- I3: 0 件と照会の timeout・切断は即停止ではなく `reconcile_retry`（試行 deadline まで poll 間隔で再照合）。
  `accepted` 後の qstat 原票の不一致は台帳の `unresolved_stop` 行にせず、`PbsExt` の `Lookup::Unresolved` /
  `SubmitError::Unresolved` で停止する（照会のたびに同じ判定で、再投入はしない）。`human_resolution` 行は未実装で、
  人の解決の記録形式は実接続前に決める。lock は入口の process が持つ（IPC の独立 owner ではない）。
- I5: spec の関数は `qsub_request`/`intent_spec`（arm・trial_key・logical job を取る）で、`observe::Expect` は作らない。
