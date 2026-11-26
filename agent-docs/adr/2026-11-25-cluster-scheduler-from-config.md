# クラスタ job の wait の scheduler をクラスタの設定で決め、poll の失敗が続いたら run を起こす

- 日付: 2026-11-25
- 状態: Accepted
- 関連: [ADR-0090](0090-durable-wait-for-cluster-jobs.md) D1・D2・D7、
  [NQSV の wait](2026-11-24-cluster-job-wait-nqsv.md) D1、
  [失敗・長い Q/H での早期起床](2026-11-11-cluster-job-early-wakeup.md)
- task: 01M8AJ2STJRB0VQCCFRXQQ3R2P

## 文脈（事象）

2026-11-25、本番で確認した。MemT の E2（task 01M884MKGET79G27ABC01DP9AF、run 01M89PTDW4KY1KA5MD4M5ASFWT）は、Pegasus（NEC NQSV）の
job 318985〜318988 を `"scheduler": "pbs"` で待つ wait（01M89PTZ9YVYY5C0NMR1H9E364、15:57:52Z 登録、期限 2026-11-26T15:57:52Z）を申告した。
同じ task の前の 6 件の wait は `nqsv` で申告され、どれも `satisfied` で解けている。

- PBS の poll（非ログインのシェルで `qstat -xf`）は Pegasus では 5 分ごとに失敗する。本番の log は
  `cluster job poll returned no job states; retrying after poll_secs (ADR-0090)`、`exit: Some(127)`、
  `stderr: bash: 行 1: qstat: コマンドが見つかりません` を出し続け、`last_status_json` は `[]` のまま。
- 23:5xZ に運用者が Pegasus のログインシェルで `qstat 318985 318986 318987 318988` を流すと、4 件とも
  `Batch Request: <id>.nqsv does not exist on nqsv.`（終わっている）だった。
- 影響: job は終わっているのに wait は期限まで（最大 24 時間）解けない。早く解く API は無い。利用者に見えるのは task が
  ready のまま動かないことだけで、受信箱にも出ない。

原因は 2 つある。

1. クラスタの scheduler の正本が worker の申告だけで、申告を誤るとクラスタに無い command で poll し続ける。
2. poll が command の失敗（exit 127・usage）を続けて返しても、dispatcher は `poll_secs` ごとに再試行するだけで、期限まで黙って待つ。

## 決定

### D1. scheduler の正本は `[[clusters]] scheduler`

`[[clusters]]` に必須の欄 `scheduler = "pbs" | "slurm" | "nqsv" | "none"` を足す。既定値は置かない。欄が無い config、値が
この 4 つ以外の config は読み込みエラーにする（serde の必須欄と `celeris::config::cluster::ClusterSchedulerSetting` の enum で
弾く）。設定は `ClusterSpec.scheduler: Option<ClusterScheduler>` に写す（`none` は `None`）。

`"none"` は job scheduler の無いノード（remote-exec だけに使うワークステーション。example の fern03）の値。そのクラスタへの
wait は、登録（`Dispatcher::cluster_job_wait_for`）で `cluster "<id>" has no job scheduler ([[clusters]] scheduler = "none")` として
断る。run は `error(retryable)`（`invalid cluster job wait: …`、attempts を 1 使う）で終わり、行は作らず poll もしない。
次の試行の run は wait を使わずに終わりを確かめる。`none` を置く理由: 欄を必須にすると、scheduler の無いノードにも値が要る。
`pbs` 等を仮に書かせると、そのノードへの wait がクラスタに無い command で poll され、今回の事象（exit 127 で期限まで解けない）を
設定の側で作り直す。`none` なら誤った wait は登録の時点で run に返る。

wait の登録は scheduler を申告からではなく、wait のクラスタの `ClusterSpec.scheduler` から決める。poll の command
（`cluster_job::poll_script`）と読み方（`cluster_job::parse_poll_output`）は wait の行の `scheduler` を使う（従来どおり）。
`cluster_job_waits.scheduler` の CHECK は `pbs | slurm | nqsv` のままで、`none` は行に現れない。

### D2. 申告の `scheduler` の欄は消す（申告に書くと wait を断る）

`ClusterJobWaitRequest.scheduler` と `WorkerMessage::Wait.scheduler` を消す。worker の `result.json` の wait に `scheduler` の
キーがあれば、`parse_wait_request` は不正な wait（`invalid cluster job wait: …`、`error(retryable)`）にする。理由の文は
「scheduler は `[[clusters]] scheduler` で決まるので申告から外す」と書き、次の試行の run は欄を外して申告し直す。

採らなかった案:
- 申告の欄を残し、設定と食い違う申告を登録時に断る: 申告の値は設定と同じか誤りかのどちらかで、情報を足さない。欄を残すと
  worker の案内に「scheduler を書く」が残り、今回と同じ誤りの入口が残る。
- 申告の欄を黙って無視する: CLAUDE.md の方針（警告だけ出して無視する互換分岐を残さない）に反する。消した欄は読み込みエラー
  （config の欄と同じ扱い）にする。

### D3. 前の ADR（2026-11-24 D1）の「採らなかった案」への答え

| 前の ADR の理由 | この ADR の答え |
|---|---|
| 申告にも `scheduler` があるので正本が 2 つになる | 申告の欄を消す（D2）。入力は設定だけになる。 |
| worker の protocol の破壊的変更になる | 破壊的変更として行う（`agent-docs/adr/2026-10-11-breaking-changes-by-default.md`）。`PROTOCOL_VERSION` は変えない（欄を消すだけで、残る欄の意味は同じ）。旧い形の申告は D2 の文で断る。 |
| `cluster_job_waits.scheduler` を変える | 列は残す。値は登録の時点の設定から写す（`poll_secs`・`timeout_secs` を設定から丸めて行に書くのと同じ）。wait の行は「この wait をどの command で poll するか」の記録で、設定と並ぶ正本ではない。migration は要らない（CHECK は 2026-11-24 の 127 のまま）。既存の行は書き換えない。 |
| campaign / dagbench の wait の組み立てを変える | `campaign::dagbench::celeris_side` の申告から `scheduler` を外し、その `ClusterSpec` に `scheduler = pbs` を置く。`campaign::celeris_exec` は task の詳細の `cluster_job_wait.scheduler`（wait の行の値）を読むだけなので変えない。 |

登録の後に設定の `scheduler` を書き換えても、開いている wait は登録の時点の値で poll し続ける。その wait の poll が失敗し続ければ
D4 で run が起きる。

### D4. poll が job の状態を返さない失敗が続いたら run を起こす（detail `poll_failed`）

**失敗の poll**: poll の結果が `Err`（ssh を起こせない・60 秒の打ち切り）か、exit ≠ 0 でどの job の状態も分からない
（`Unknown` だけ）もの。command が無い（exit 127）・usage（`invalid option`）・ssh の失敗（exit 255）がこれに当たる。exit 0 で
どの job も出力に現れない poll は失敗に数えない（NQSV の Q・R の `qstat -f` の書式は未取得で、書式の違いで Q・R の job を
`Unknown` と読む恐れがあるため。2026-11-24 D4）。

**記録**: 失敗の poll ごとに、wait の `last_status`（無ければ申告の job ごとの `Unknown`）の各 job に
`poll_failed_since`（連続した失敗の最初の時刻、RFC 3339）と `poll_error`（最後の失敗の要約。`exit 127: <stderr の末尾>` の形、
300 字まで）を書き、`last_status_json` に残す。job の状態の JSON が変わった失敗の poll（連続の最初・要約が変わったとき）は
`ClusterJobWaitPolled` を出し、同じ失敗の繰り返しは `cluster_job_wait_touch` だけにする（ADR-0090 D2 の「状態が変わった poll
だけ event」と同じ規則）。daemon の再起動をまたいでも連続の時刻は残る。前の観測（`job_state` 等）は消さない。
失敗の分け方は `task_core::cluster_job::read_poll_output`、記録と起床の判定は `cluster_job::wakeup::evaluate`
（job の状態は従来の `observe`、失敗の poll は `observe_poll_failure`）が持ち、dispatcher は従来の判定の経路
（`apply_cluster_job_statuses`）に結果を渡すだけにする。

**起床の条件**: `now - poll_failed_since >= job_wait.poll_failure_wake_secs` になった失敗の poll で、wait を
`ClusterJobWaitFinished{satisfied, detail: "poll_failed"}` で閉じ、既存の再開（atomic は `ClusterJobResume`、unit は
`needs_continuation`）で続きの run を起こす。新しい wait の状態・DB の列は作らない（2026-11-11 と同じ置き方）。

`[[clusters]] job_wait.poll_failure_wake_secs` は既定 1800 秒（`poll_secs = 300` なら失敗の poll がおよそ 7 回続いたとき）、正の
整数（0 は読み込みエラー）。最初の失敗では `now - since = 0` なので起きない。起床には少なくとも 2 回の連続した失敗が要る。

**Q・R の間に早く起こさない**: job の状態を 1 つでも返した poll（Q・R・H・E・F・`gone`）は失敗に数えず、その poll の結果には
`poll_failed_since` が無いので連続が切れる。job が Q・R と読める間は、この起床は起きない。exit 0 で `Unknown` だけの poll も
連続を進めない（上の理由）。

**続きの run**: 前置きの「クラスタ job の結果」節は「celeris の poll が job の状態を返さない失敗を続けたため再開した」と書き、
job ごとの行に最後の失敗（`poll_error`）を出す。worker は remote-exec で scheduler に job の状態を問い合わせ、終わっていれば回収し、
Q・R なら待ち直す。失敗の原因がクラスタの設定（`[[clusters]] scheduler`・PATH）なら、worker は直せないので人に質問する。

採らなかった案:
- 人に知らせるだけ（受信箱の質問）: 多くの場合、worker が remote-exec で状態を確かめれば自分で先へ進める（job が終わっていれば
  回収、Q・R なら待ち直し）。人が要るのは設定の誤りのときだけで、その判断は続きの run が質問で上げる。
- 回数の閾値: 連続の回数を残すには行に新しい値が要る。時刻なら `queued_since` と同じ形で job の状態の JSON に残せる。
- exit 0 の `Unknown` だけの poll も数える: NQSV の Q・R の書式を確かめるまで、Q・R の間に起こす恐れがある。

### D5. worker への案内

`ssh::cluster_job_wait_instructions`（wait の申告の説明）と `CLUSTER_JOB_PLANNER_GUIDANCE` から `scheduler` の申告の説明と NQSV の
見分け方の文を消し、「scheduler はクラスタの設定（`[[clusters]] scheduler`）で決まるので wait に書かない（書くと断られる）」と書く。
`CLUSTER_JOB_RECOVERY_GUIDANCE` の scheduler ごとの道具（PBS Pro は `qstat -x`、NQSV は `does not exist`・`racctreq`）は、
続きの run が自分で調べるための説明なので残す。

## 試験

- `cluster_job` の単体試験: wait の申告に `scheduler` があると不正な wait になる。申告の形から `scheduler` を読まない。
  `poll_failed_since` を持つ状態の JSON が読み書きできる。
- dispatcher の試験（偽の poller と差し替えた時計）:
  - 設定で NQSV のクラスタに対し、申告なしの wait を登録すると行の `scheduler` は `nqsv` で、poll の要求は `bash -lc` の中の
    `qstat -f`（`-x` を含まない）。
  - 偽の poller が exit 127 を返し続けると、`poll_failure_wake_secs` に達する前は `waiting`、達した poll で
    `satisfied`（detail `poll_failed`）になり task が `ready` に戻る。
  - 失敗の間に Q・R を返す poll が挟まると連続が切れ、閾値を超えても起きない。Q・R を返し続ける間は起きない。
- worker の試験: `scheduler` を含む `result.json` の wait は不正な wait になる。前置きの `poll_failed` の文。
- config の試験: `scheduler` の無い `[[clusters]]`、値が不正な `[[clusters]]`、`poll_failure_wake_secs = 0` は読み込みエラー。
  `scheduler = "none"` は読めて `ClusterSpec.scheduler` が `None` になる。
- dispatcher の試験: `scheduler` が `None` のクラスタへの wait は retryable な失敗（`has no job scheduler`）で、行も poll も無い。
- PBS Pro・Slurm・NQSV の既存の poll の読み方の試験の期待値は変えない。試験は外部ネットワークに出ない。

## 本番への移行

- config: 全ての `[[clusters]]` に `scheduler` を足す。**この欄を知る release の昇格と同時に**書き換える（旧い版は未知の欄で、
  新しい版は欄が無いと起動に失敗する）。pegasus = `"nqsv"`、sirius = `"pbs"`。fern03 等ほかのクラスタは scheduler を確かめて書く
  （job scheduler が無ければ `"none"`。手順は task の進捗ファイルの「本番への移行」節）。
- DB: migration は無い。既存の wait の行の `scheduler` は書き換えない。
- 昇格の前に `pbs` で登録された Pegasus の `waiting` の wait（01M89PTZ9YVYY5C0NMR1H9E364 など）は、昇格の後も `qstat -xf` で
  poll し続けて失敗するが、D4 により `poll_failure_wake_secs`（既定 30 分）の後に `poll_failed` で続きの run が起きる。

> 付記（2026-11-26）: D1 の値 `"none"` と、そのクラスタへの wait を登録時に断る扱いを書き足した。実装（attempt 1）は `none` を
> 受けていたが、この ADR の D1 は 3 つの値しか書いておらず、review で「設計の記録に無い」と指摘されたため。決定の中身
> （scheduler の正本は設定だけ・申告の欄は消す・poll の失敗で起こす）は変えない。
