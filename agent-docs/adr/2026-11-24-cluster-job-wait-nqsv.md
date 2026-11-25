# クラスタ job の wait を NEC NQSV（Pegasus）でも job の終了で解く

- 日付: 2026-11-24
- 状態: Accepted
- 関連: [ADR-0090](0090-durable-wait-for-cluster-jobs.md) D1・D2「読み方」・「残したもの」、
  [失敗・長い Q/H での早期起床](2026-11-11-cluster-job-early-wakeup.md)
- task: 01M86R8E4JRK12W0PKPQR7FAWZ

## 文脈（事象）

Pegasus の scheduler は NEC の NQSV で、PBS Pro ではない。ADR-0090 の wait は Pegasus で一度も `satisfied` にならず、
期限切れ（`timed_out`）でしか解けなかった（2026-11-24 時点で Pegasus の wait は 3 件。`timed_out` 2・`waiting` 1、どれも
`last_status_json` が `[]`。Sirius（PBS Pro）は `satisfied` 186 件）。MemT の E1（task 01M85D8FNK2AT54GKQJX5A3GJV）は job が
09:42Z に終わったのに 12:30Z の期限まで約 2 時間 50 分止まった。

原因は 2 つある（2026-11-24 12:27Z〜12:28Z に `ssh -o BatchMode=yes pegasus` で読み取りだけの `qstat` を流して確認）。

1. poll は `ssh -o BatchMode=yes pegasus -- <script>` で非ログインのシェルに流す。Pegasus の `qstat` は
   `/opt/nec/nqsv/bin/qstat` にあり、非ログインの PATH（`/usr/local/sbin:…:/snap/bin`）には無い。
   `ssh pegasus 'qstat -f 1'` は `bash: 行 1: qstat: コマンドが見つかりません` で exit 127。`bash -lc` では `command -v qstat`
   が `/opt/nec/nqsv/bin/qstat` を返す。
2. NQSV の `qstat` には `-x` が無い。`qstat -xf 315477.nqsv` は `qstat: invalid option -- 'x'` と usage を出す。

終わった request は `qstat -f 315477.nqsv` が **stdout** に `Batch Request: 315477.nqsv does not exist on nqsv.` を出し、exit 0
（stderr は空）。複数の id を渡すと 1 行ずつ出る。`qstat -f 315477`（server 名の無い id）も `315477.nqsv` の形で答える。
`/opt/nec/nqsv/bin` には `racctreq`・`racctjob`（accounting）・`qwait` もある。キューの一覧（`qstat -Q`）の状態の列は
`ARR WAI QUE PRR RUN POR EXT HLD HOL SUS MIG STG`。一般の利用者の `qstat` には自分の request しか出ず、確認の時点で自分の
request が無かったので、**Q・R の request の `qstat -f` の書式は未取得**。

## 決定

### D1. 見分け方: wait の申告の `scheduler` に `nqsv` を足す

`ClusterScheduler` を `pbs | slurm | nqsv` にする。worker は Pegasus の job を `"scheduler": "nqsv"` で待つ。worker の案内に
NQSV の見分け方（`qstat` が `/opt/nec/nqsv/bin` にある、`qstat -x` が `invalid option` になる、job id が `<番号>.nqsv`）を書く。

採らなかった案: クラスタの設定（`[[clusters]] scheduler`）で決める。wait の申告にも `scheduler` があるので、両方を持つと正本が 2 つに
なる。申告の欄を消すと worker の protocol・`cluster_job_waits.scheduler`・campaign / dagbench の wait の組み立てを一度に変える
破壊的変更になり、この task の範囲（NQSV の wait が解けない）を超える。クラスタの設定に移すかは進捗の「提案」に書く。

### D2. qstat の PATH: NQSV の poll はログインシェルの中で流す

NQSV の poll の script は `bash -lc '<env の export> && <setup> && qstat -f <ids>'` の 1 行にする（`cluster_job::poll_script`）。
ログインシェルの profile を先に読み、その後でクラスタの `env`・`setup` を当てる。profile が PATH を置き直す形でも、`env`・`setup`
の変更が消えない。

採らなかった案:
- 絶対 path（`/opt/nec/nqsv/bin/qstat`）: install 先を code に固定する。site の profile が設定する NQSV の環境変数も読まない。
- クラスタの `setup` に PATH を足す: 本番の config の書き換えが要り、設定を忘れたクラスタで同じ事象が再び起きる。`setup` は
  worker の remote-exec にも効く。

PBS（`qstat -xf`）と Slurm（`sacct`）の poll の script は変えない（従来どおり `env`・`setup` の後に直接流す）。

### D3. 終わった request の読み方: `does not exist` を `gone` にする

NQSV の `qstat` は終わった request を返さない（PBS の `-x` に当たる履歴が無い）。stdout と stderr の行
`Batch Request: <id> does not exist on <server>.` の `<id>` を [`ClusterJobState::Gone`]（終わったものとして扱う）にし、
`raw_state` にその行を残す。正常終了も失敗も同じ `gone` になる。

**終了コードは取らない（不明のまま）**。accounting（`racctreq`）の出力の書式・権限・反映の遅れを確かめていないので、poll には
使わない。このため NQSV では次のようになる。

- 早期起床の `job_failed`（ADR 2026-11-11）は起きない（終了コードが無い）。全 request が `gone` になった時点で `satisfied` で再開する。
- 続きの run の前置きは「exit status unknown」と書く。worker は job の出力・log（必要なら `racctreq`）で成否を確かめる（D5）。

### D4. Q・R の request の読み方（書式は未取得）

`qstat -f` の出力を request ごとの段落（要求した id を含み、`does not exist` でない行から、次の request の行まで）に分け、
段落の中の `<key>: <value>` / `<key> = <value>` の行のうち key が `State` で終わる行（例 `Current State`）の値の先頭の語を
NQSV の状態名で正規化する。

| NQSV の状態 | 正規化 |
|---|---|
| `ARR` `WAI` `QUE` `STG`（`ARRIVING` `WAITING` `QUEUED` `STAGING`） | queued |
| `HLD` `HOL` `SUS`（`HELD` `HOLDING` `SUSPENDED`） | held |
| `PRR` `RUN` `MIG`（`PRE-RUNNING` `RUNNING` `MIGRATING`） | running |
| `POR` `EXT`（`POST-RUNNING` `EXITING`） | exiting |

段落はあるが状態を読めない request は `unknown`（終わったとは扱わない）。出力に現れない request も `unknown`。
**`does not exist` の行が無い request で wait が解けることは無い**。この性質は Q・R の書式が上の想定と違っても保たれる。
想定と違えば Q/H の連続時間（`queued_wake_secs`）は進まない（`unknown` は観測に数えない）ので、`queue_stalled` の起床は
書式を確かめるまで起きないことがある。

### D5. worker への案内

`qstat -x` は PBS Pro だけの道具として書く。NQSV では「終わった request は `qstat -f` に出ない（`does not exist`）。成否は job の
出力・log（必要なら `racctreq`）で確かめる」と書く。対象は `ssh::CLUSTER_JOB_RECOVERY_GUIDANCE`・
`ssh::cluster_job_wait_instructions`・`CLUSTER_JOB_PLANNER_GUIDANCE`・続きの run の前置き（`preamble::cluster_jobs_section`）。

### D6. DB と events

`cluster_job_waits.scheduler` の CHECK は `('pbs', 'slurm')` なので、migration 0127（schema 127）で表を作り直し、CHECK を
`('pbs', 'slurm', 'nqsv')` にする。行はそのまま写し（消さない）、index を作り直す。events の `ClusterJobWaitStarted` 等の
`scheduler` に `nqsv` が入りうる（旧い event はそのまま読める）。旧いバイナリはこの DB を開けない（`SchemaTooNew`）。

## ADR-0090 との関係

- D2「読み方」に NQSV を足す（PBS・Slurm の読み方は変えない）。
- 「残したもの」の「job history（`qstat -x`）が無効なクラスタでは終わった job は `gone`（終了コード不明）」は、NQSV では常にこの
  形になる。終了コードを accounting から取る仕組みは置かない（D3）。

## 試験

- `cluster_job` の単体試験: 2026-11-24 に Pegasus で取った `does not exist` の出力（stdout・exit 0、1 件と 2 件）を fixture にして
  `gone` と `all_finished` を確かめる。Q・R は D4 の想定の段落（**合成。実機の書式ではない**）と、段落はあるが状態の無い出力で
  `queued`・`running`・`unknown` になり `all_finished` が偽になることを確かめる。`qstat -xf` の usage（`invalid option -- 'x'`）は
  どの request も `gone` にしない。poll の script が `bash -lc` で包まれ `-x` を含まないことを確かめる。
- dispatcher の試験: 偽の poller が NQSV の出力を返すと wait が `satisfied` になり、Q・R の出力では `waiting` のままになる。
  poll の要求の script が `bash -lc` の中に `env`・`setup`・`qstat -f` を持つ。
- migration の試験: schema 126 の DB の `cluster_job_waits` の行が 127 で残り、`nqsv` の行が入る。
- PBS・Slurm の既存の試験の期待値は変えない。

## 本番への移行

config の書き換えは要らない（クラスタの設定は変えない）。昇格は schema 127 を含むので stop → start。昇格の後、worker が
Pegasus の job を `"scheduler": "nqsv"` で待てば解ける。昇格の前に作られた Pegasus の `waiting`（`scheduler = pbs`）の wait は
`qstat -xf` を流し続けて期限切れで解ける（自動では書き換えない）。

> 付記（2026-11-25）: D1 は [scheduler をクラスタの設定で決める](2026-11-25-cluster-scheduler-from-config.md) で置き換えた。D1 の「採らなかった案」（クラスタの設定で決める）を採り、申告の `scheduler` の欄を消した（書くと不正な wait）。正本は `[[clusters]] scheduler` で、`cluster_job_waits.scheduler` は登録の時点の設定の写し（migration なし）。2026-11-25 に worker が Pegasus の job を `pbs` で申告し、`qstat -xf` が exit 127 を返し続けて wait が期限まで解けない事象が起きたため。D5 の worker への案内の NQSV の見分け方の文は消した（`CLUSTER_JOB_RECOVERY_GUIDANCE` の NQSV の道具の説明は残す）。D2〜D4・D6 は変えない。
