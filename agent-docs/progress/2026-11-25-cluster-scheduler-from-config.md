---
title: cluster job の wait の scheduler をクラスタの設定で決め、poll の失敗が続いたら run を起こす
tasks: [01M8AJ2STJRB0VQCCFRXQQ3R2P]
status: done
updated: 2026-11-26
---
# cluster job の wait の scheduler をクラスタの設定で決め、poll の失敗が続いたら run を起こす

完了日: 2026-11-26

状態: done（ADR・実装・試験・失敗注入は済んだ。本番の config の書き換えと昇格は運用者が 2026-11-26T03:10Z〜03:14Z に済ませた。
下の「本番への移行」）。attempt 3（取り込みの後のリリース準備の差し戻し）の対応は「attempt 3」節。
基準は `git merge-base HEAD main`。検査は提出時の HEAD で実行した。

## commit（件名）

- ADR: クラスタ job の wait の scheduler をクラスタの設定で決め、poll の失敗が続いたら run を起こす
- cluster job wait: scheduler を [[clusters]] scheduler で決め、poll の失敗が続いたら poll_failed で run を起こす
- progress: cluster job の wait の scheduler をクラスタの設定で決める task の証拠と本番への移行
- cluster job wait: 失敗の poll の分け方を task_core::cluster_job::read_poll_output に移し、dispatcher の本体を増やさない
- cluster job wait: 失敗の poll を wakeup::evaluate で既存の判定の経路に通し、dispatcher の method を増やさない
- cluster job wait: 試験用の [[clusters]] の config に必須になった scheduler を足す（cluster_admin・e2e の cluster_scenarios）
- progress: 検査の結果（test-affected・nextest の絞り込み・clippy・fmt）
- ADR: [[clusters]] scheduler の値 none と、そのクラスタへの wait を登録時に断る扱いを D1 に書き足す
- cluster job wait: scheduler = none のクラスタへの wait を断る試験を足し、example・protocol・API の文書から申告の scheduler を消す
- progress: attempt 2 の review への対応と検査の結果
- ADR: [[clusters]] scheduler を 2 段で必須にする（1 段目は欄が無ければ未設定として wait を断る。verify が本番の config を新旧の版に読ませるため）
- cluster job wait: [[clusters]] scheduler の無い config を 1 段目では読み、未設定として none と同じく wait を断る（本番の config のまま verify を通す）
- cluster job wait: 未設定・none の wait を断る文を 1 行に収め、dispatcher の本体の行を増やさない（ADR の文も合わせる）
- cluster job wait: celerisctl の試験の [[clusters]] を 1 段目の Option の scheduler に合わせる
- ADR: [[clusters]] scheduler の 2 段の移行を取り消し、必須のままにする（本番は config を書き換えて c7863978ccb5 を昇格した）
- cluster job wait: 2 段の移行の 1 段目（未設定の [[clusters]] scheduler を none と同じに扱う）を取り消す
- progress: attempt 3（リリース準備の差し戻し）の対応と検査の結果

上の 4 件（2 段の移行）は下の 2 件の取り消しで打ち消した。`git merge-base HEAD main` からの差分は ADR の付記とこの進捗ファイルだけ。

## 実装前 brief

1. 根本原因: wait の scheduler の正本が worker の申告だけだった（申告を誤ると、クラスタに無い command で poll し続ける）。
   poll が job の状態を返さない失敗を続けても、`on_cluster_job_poll` は `poll_secs` ごとに再試行するだけで期限まで待った。
   症状は task が ready / blocked のまま動かないこと（受信箱にも出ない）。
2. 責務 owner: scheduler の決定は wait の登録（`Dispatcher::cluster_job_wait_for`）とクラスタの設定（`celeris::config::cluster`
   → `ClusterSpec`）。申告の解釈は `task_core::cluster_job::parse_wait_request`。poll の失敗の扱いは
   `Dispatcher::on_cluster_job_poll` と起床の方針の `task_core::cluster_job::wakeup`。
3. 既存の再利用先: 早期起床（ADR 2026-11-11）の `satisfied` + detail と `finish_cluster_job_wait`（`ClusterJobResume` /
   `needs_continuation`）、状態の JSON に時刻を残す `queued_since` の置き方、`cluster_job_wait_touch`、`poll_secs` を設定から
   行に写す登録の形。試験は `dispatcher/tests/cluster_job_wait.rs` の偽の poller と注入時計。
4. 選択肢: (a) 申告の欄を残し食い違いを断る、(b) 申告の欄を黙って無視する、(c) 欄を消し、書けば断る。(c) を採った
   （(a) は誤りの入口を残す、(b) は CLAUDE.md の「警告だけ出して無視」に当たる）。poll の失敗は「run を起こす」を採った
   （人に知らせるだけより、worker が remote-exec で確かめて先へ進める場合が多い。設定の誤りは続きの run が質問で上げる）。
5. 新しい状態・分岐・fallback・port: 新しい wait の状態・DB の列・migration は無い。足したのは config の必須の欄
   `[[clusters]] scheduler`（`pbs | slurm | nqsv | none`）、`job_wait.poll_failure_wake_secs`（既定 1800）、job の状態の JSON の
   `poll_failure`（`since`・`error`）、早期起床の detail `poll_failed`。`none` は job scheduler の無いノード（fern03）のための値で、
   そのクラスタへの wait は登録時に断る。
6. 撤去対象: `ClusterJobWaitRequest.scheduler`、`WorkerMessage::Wait.scheduler`、申告の `scheduler` の解釈（既定 `pbs`）、
   worker の案内の `scheduler` の申告と NQSV の見分け方の文（`ssh::cluster_job_wait_instructions`）、
   `CLUSTER_JOB_PLANNER_GUIDANCE` の旧い前提、試験の `scheduler` を含む申告。
7. 不変条件: scheduler はクラスタの設定だけから決まる（`an_nqsv_wait_polls_in_a_login_shell_…`・
   `wait_request_naming_a_scheduler_is_rejected`・`result_contract_wait_naming_a_scheduler_is_retryable_error`・
   `cluster_scheduler_is_required_and_copied_into_the_spec`）。poll の失敗が閾値を超えて続くと続きの run が起き、Q・R を読めた
   poll は連続を切る（`polls_that_keep_failing_…`・`polls_that_report_q_or_r_never_count_…`・
   `poll_failures_wake_only_after_the_threshold_from_the_first_failure`）。PBS・Slurm・NQSV の読み方の既存の試験の期待値は変えない。

## 受け入れ条件ごとの証拠

### 0. 実装の前の commit で ADR を足し、前の 2 つの ADR に付記した

- `agent-docs/adr/2026-11-25-cluster-scheduler-from-config.md`（事象、D1 scheduler の正本、D2 申告の欄、D3 前の ADR の D1 の
  「採らなかった案」への答えの表、D4 poll の失敗が続くときの扱いと閾値、D5 worker への案内、試験、本番への移行）。
- 付記: `agent-docs/adr/0090-durable-wait-for-cluster-jobs.md`（2026-11-25）、`agent-docs/adr/2026-11-24-cluster-job-wait-nqsv.md`（2026-11-25）。
- ADR の commit（件名「ADR: クラスタ job の wait の scheduler をクラスタの設定で決め、…」）は実装の commit の前にある
  （`git log --oneline $(git merge-base HEAD main)..HEAD` で確認）。

### 1. NQSV のクラスタに `pbs` の申告・申告なしの wait

- 申告なし: `task-dispatch dispatcher::tests::cluster_job_wait::an_nqsv_wait_polls_in_a_login_shell_and_resumes_when_the_request_no_longer_exists`。
  申告（`wait_terminal`）は scheduler を持たず、クラスタの設定を `Some(Nqsv)` にすると、行の `scheduler` は `nqsv`、poll の script は
  `bash -lc '{ module load openpbs; } && qstat -f 315477'`、`does not exist` で `satisfied` → 続きの run → done。
- `pbs` の申告（2026-11-25 の事象の形）: `task-core cluster_job::tests::wait_request_naming_a_scheduler_is_rejected` と
  `task-worker result_report::result_contract_tests::result_contract_wait_naming_a_scheduler_is_retryable_error`。
  `"scheduler": "pbs"`（Pegasus の job 318985・318986）・`"nqsv"`・`"slurm"`（入れ子）・`"lsf"` の申告は
  `Error{retryable: true}`（`result.json has an invalid cluster job wait: … [[clusters]] scheduler …`）になり、run は欄を外して申告し直す。
- 失敗注入（`artifacts/failure-injection.txt`。注入の後 `git checkout -- crates` で戻した）:
  - A: 登録の scheduler を設定でなく旧い既定の `Pbs` にする → `an_nqsv_wait_polls_in_a_login_shell_…` が FAIL（1 run、0 passed、1 failed）。
  - B: 申告の `scheduler` を断らない → `wait_request_naming_a_scheduler_is_rejected`・`result_contract_wait_naming_a_scheduler_is_retryable_error` が FAIL（2 run、0 passed、2 failed）。

### 2. poll の失敗が閾値を超えて続くと run が起き、Q・R の間は起きない

- 注入した poll の結果（偽の poller）と注入時計（`advance`）で試験した。sleep の待ちは tick の駆動の 20 ms の間だけで、判定は時計の差し替えで決まる。
- `polls_that_keep_failing_wake_the_run_after_poll_failure_wake_secs`: poll が exit 127・`bash: 行 1: qstat: コマンドが見つかりません`
  を返し続ける。`poll_failure_wake_secs = 90`、`poll_secs = 30`。1〜3 回目（t0〜t0+62 秒）は `waiting` で `last_status` に
  `poll_failure`（`exit 127: …`）が残る。4 回目（t0+93 秒）で `ClusterJobWaitFinished{satisfied, detail: "poll_failed"}`、続きの run の
  前置きは「poll が job の状態を返さない失敗を続けたため再開しました」と `exit 127` を含み、task は done。replay も一致。
- `polls_that_report_q_or_r_never_count_towards_the_poll_failure_wake`: R を 4 回（93 秒、閾値を超える）返しても起きない。失敗 2 回の後に
  Q を返す poll で連続が切れ（`poll_failure` が消える）、最初の失敗から 155 秒経っても起きない。切れた後の失敗が 93 秒続いた 11 回目で初めて起きる。
- `task-core cluster_job::tests::poll_failures_wake_only_after_the_threshold_from_the_first_failure`: 最初の失敗では起きない、
  `since` は最初の失敗の時刻のまま、閾値の時刻で起きる、失敗の poll は前の観測（Q・`queued_since`）を消さない、状態の JSON の往復。
- 失敗注入:
  - C: 起床を止める（旧い「期限まで再試行」）→ 2 件とも FAIL（60 秒の状態待ちの保険で止まる。2 run、0 passed、2 failed）。
  - D: Q・R を読めた poll で連続を切らない → `polls_that_report_q_or_r_never_count_…` が FAIL（2 run、1 passed、1 failed）。

### attempt 2: 前の review の指摘への対応

前の review（attempt 1）は 2 点を不備とした。どちらも直した。

- ADR の D1 が `pbs | slurm | nqsv` の 3 値だけを書き、実装の 4 つ目の値 `"none"`（job scheduler の無いノード。そのクラスタへの
  wait は登録時に断る）が設計の記録に無かった。→ ADR の D1 に `none` の意味・登録時の拒否（`error(retryable)`、行も poll も無し）・
  置く理由（必須の欄に scheduler の無いノードが `pbs` 等を仮に書くと、exit 127 で期限まで解けない事象を設定の側で作り直す）を
  書き足し、試験と本番への移行の節にも足した。末尾に付記（2026-11-26）で書き足した経緯を残した。ADR-0090 の付記も
  `pbs | slurm | nqsv | none` に直した。決定の中身（D2〜D5）は変えていない。
- `config/celeris.clusters.example.toml` の `job_wait` の説明が、worker の wait の例に `"scheduler": "pbs"` を書いていた。→ 例から
  外し、poll の command は上の `scheduler` で決まること（pbs・nqsv・slurm の command）、wait に `scheduler` を書くと断ること、
  `poll_failure_wake_secs` の意味を書いた。
- 同じ種類の残りを探して直した（`grep -rn '"scheduler"\s*:'`）: `docs/protocol/worker-protocol.md` §4.8 の wait の例と欄の表が
  `scheduler`（省略時は `pbs`）を書いていた → 例と表の行を消し、「`scheduler` の欄は無い。書くと不正な wait。`[[clusters]] scheduler`
  で決まり、`none` のクラスタへの wait も `error{retryable:true}`」と書いた。`docs/api/v1/api.md` の `cluster_job_wait.scheduler` の値が
  `pbs | slurm` だった → `pbs | slurm | nqsv`（登録の時点の `[[clusters]] scheduler` の写し）に直した。
  残りの `"scheduler":` は、dagbench の submit spec・submit-audit の台帳・task の詳細の `cluster_job_wait`（wait の行の値）・
  過去の ADR の本文と過去の進捗で、wait の申告ではないので変えない。
- 試験を 1 件足した: `task-dispatch dispatcher::tests::cluster_job_wait::a_wait_on_a_cluster_without_a_job_scheduler_is_a_retryable_failure`。
  守る不変条件: 設定に scheduler の無いクラスタ（`scheduler = "none"`）で poll の command を推測しない（行を作らず poll もせず、
  run は `invalid cluster job wait: … has no job scheduler` の retryable な失敗）。既存の試験はどれも scheduler のあるクラスタで
  登録するので、この経路を検出できない（`none` の config の読み込みは `cluster_scheduler_is_required_and_copied_into_the_spec` が見る）。
- 失敗注入 E（`artifacts/failure-injection.txt`）: 登録で `none` を断らず旧い既定の `Pbs` に落とす → 足した試験が FAIL
  （1 run、0 passed、1 failed。wait が登録されて attempts が 1 にならず、`tick_until` の 60 秒の打ち切りで止まる）。
  注入を外す（`git checkout -- crates/task-dispatch/src/dispatcher/cluster_job_wait.rs`、diff 空を確認）と 2 件（足した試験と
  `a_wait_on_an_unknown_cluster_is_a_retryable_failure`）とも pass。

### 3. 検査

提出時の HEAD で実行した結果は「検査の結果」節に書く。

### attempt 3: 取り込みの後のリリース準備の差し戻し（2026-11-26）

差し戻しの理由: 取り込みの後のリリース準備（release c7863978ccb5、`prepare.log`）は build・試験・web を通ったが、verify の検査 1 が
``failed to parse config: missing field `scheduler` in `clusters` `` で落ちた。`verify.sh` は本番の `config.toml` をそのまま新しい版
（検査 1）と旧い版 N-1（検査 5）に読ませる（`NEW_CMD`・`OLD_CMD` がどちらも `--config "$SD_CONFIG"`）。欄を必須にした新しい版は欄の
無い config で起動せず、`deny_unknown_fields` の旧い版は欄のある config で起動しない。ADR の「昇格と同時に書き換える」は、書き換える
前の config で verify する自動の経路を通れなかった。

対応の経過:

1. この run の前の run（同じ attempt。03:13Z の昇格の daemon の再起動で打ち切られた）は、欄を一時的に任意にし「未設定」を `none`
   と同じに扱う 2 段の移行を ADR に書いて実装した（上の commit の 4 件）。失敗注入 F・G（`artifacts/failure-injection.txt`）:
   必須の形に戻すと config の試験が `missing field scheduler` で落ち、03:10Z より前の本番の config は必須の形で exit 1、任意の形で
   exit 0 だった。この run で、`celerisctl` の試験の `ClusterConfig` が `Option` に合わずコンパイルできないことを
   test-affected で見つけて直した。
2. その後で本番の状態を読み取りだけで確かめた。運用者が 03:10:57Z に本番の config の 3 つの `[[clusters]]` に `scheduler` を書き、
   03:12:18Z に verify を流し直し（`verify.json`: `ok = true`、検査 1・2・3・4d・6 が真、検査 5 だけ偽で `live_ok = false`）、03:13:55Z に
   c7863978ccb5 を停止 → 起動で昇格していた（`promote-20261126-031328.log`: 「new celeris is healthy: schema_version=127」、
   current → c7863978ccb5、previous → 3c7fa588c844）。
3. 判断: 2 段の移行を取り消し、欄は必須のままにした。昇格の後は次の verify の N-1（c7863978ccb5）が欄を知り、本番の config に欄が
   あるので、必須の欄の release も検査 1 と 5 を通る。任意の欄を残すと、撤去を待つだけの未設定の分岐が増える（CLAUDE.md の互換分岐の
   禁止。汎用規約 5）。ADR 2026-11-25-cluster-scheduler-from-config に付記（2026-11-26「リリース準備の失敗と昇格」）を足し、「本番への
   移行」の節に、書き換えの後に verify を流し直して停止 → 起動で昇格する手順を書いた。コードは `main`（c7863978）と同じに戻った。
4. 確認（読み取りだけ。本番の config・DB・daemon は変えていない）: 提出時の HEAD の `celerisctl config to-harnesses --config
   ~/.config/celeris/config.toml`（標準出力は捨てた）→ exit 0。

## 検査の結果（提出時の HEAD）

### attempt 3（2026-11-26）

提出時の HEAD（2 段の移行を取り消した commit の後。main は `git merge-base HEAD main` から動いておらず、取り込み・rebase はしていない。
この後の commit はこの進捗ファイルだけを変える）で実行した。

- `bash scripts/dev/test-affected.sh` → exit 0。`git merge-base HEAD main` からの差分は ADR と進捗だけなので「試験対象の変更なし（docs
  のみ等）」で、repo-health は `verdict=pass exit=0 fail_grade=0 new=0 worsened=0`。
  - 取り消す前の HEAD（2 段の移行と celerisctl の試験の直しの後）でも exit 0 だった: crates `celeris celeris-campaign celerisctl
    llm-proxy task-dispatch`、`Summary [431.141s] 4021 tests run: 4021 passed (9 slow), 11 skipped`。その前の 1 回目は
    `crates/celerisctl/src/commands/worker_tests.rs:168` の型の不一致（`expected Option<ClusterSchedulerSetting>`）で build が落ちた。
- `cargo nextest run -p task-core -p task-dispatch -p task-worker -p celeris -p celerisctl -E 'test(/cluster_job|job_wait|poll_fail|scheduler/)'`
  → exit 0、`72 tests run: 72 passed, 5769 skipped`。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `celerisctl config to-harnesses --config ~/.config/celeris/config.toml`（本番の config の読み込みだけ）→ exit 0。

### attempt 2

提出時の HEAD（attempt 2 の最後のコードの commit の後。main は `git merge-base HEAD main` から動いておらず、取り込み・rebase は
していない）で実行した。この後の commit はこの進捗ファイルだけを変える。

- `bash scripts/dev/test-affected.sh` → exit 0（2026-11-26、attempt 2）。repo-health は verdict=pass（`fail_grade=0`）、web の型検査
  （tsc -b）は通り、web の unit は `Test Files 108 passed (108)`・`Tests 774 passed (774)`、Rust は
  `Summary [604.432s] 8930 tests run: 8930 passed (12 slow), 34 skipped`（attempt 1 の 8929 件に足した試験 1 件）。
  - repo-health の warning（advisory、合否に入らない）が 2 件ある。`dup.contract`: `celeris::config::cluster::ClusterJobWaitConfig`
    （config の欄の読み込みと検証）と `task_core::cluster_job::ClusterJobWaitLimits`（dispatcher が使う値）が似ている。前者が後者を
    作る層の違いで、`poll_failure_wake_secs` を両方に足したので似た形が増えた。`dup.public-surface`: `cluster_job::wakeup::evaluate` が
    ほかの module の `evaluate` と同名（名前だけの一致）。どちらも attempt 1 の変更から出ており、attempt 2 では変えていない。
  - attempt 1 の 1 回目は、`[[clusters]] scheduler` を必須にしたことで欄の無い試験用の config が読み込みエラーになり、
    `celeris cluster_admin::tests` の 4 件と `e2e::cluster_scenarios` の 2 件
    （`missing_control_master_records_cluster_unavailable_and_does_not_block_idle`・`worker_run_cluster_without_control_master_exits_4`）
    が落ちた（`missing field scheduler`）。これらの config は job scheduler を使わない（wait を申告しない）ので
    `scheduler = "none"` を足した。試験の期待値は変えていない。
- `cargo nextest run -p task-core -p task-dispatch -p task-worker -p celeris -E 'test(/cluster_job|job_wait|poll_fail|scheduler/)'`
  → exit 0、`72 tests run: 72 passed, 5568 skipped`。PBS Pro・Slurm・NQSV の poll の読み方の既存の試験
  （`task-core cluster_job::tests::*`）はこの 72 件に入り、期待値は変えていない。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 本番への移行

済んだ（運用者が実行。この run は読み取りで確かめただけ）:

- 2026-11-26T03:10:57Z: `~/.config/celeris/config.toml` の全ての `[[clusters]]` に `scheduler` を書いた
  （pegasus = `"nqsv"`、sirius = `"pbs"`、fern03 = `"none"`。fern03 の値は未確認のまま置いた仮の値）。
- 03:12:18Z: release c7863978ccb5 の verify を流し直した（`ok = true`、`live_ok = false`。検査 5 は旧い版 3c7fa588c844 が `scheduler` の欄を
  知らないために落ちる。想定どおり）。
- 03:13:55Z: c7863978ccb5 を停止 → 起動で昇格した（`schema_version` 127、healthy）。

```toml
[[clusters]]
id = "pegasus"
scheduler = "nqsv"   # NEC NQSV（2026-11-24 に bash -lc の qstat が /opt/nec/nqsv/bin/qstat であることを確認）

[[clusters]]
id = "sirius"
scheduler = "pbs"    # PBS Pro（qstat -xf で Sirius の wait が satisfied 186 件）

[[clusters]]
id = "fern03"
scheduler = "none"   # 未確認。下の確認の後に決める
```

残り（人が行う）:

- fern03 の scheduler を確かめる。2026-11-25 にこの task の run から `ssh -o BatchMode=yes fern03` で読み取りだけの確認を試みたが、
  `REMOTE HOST IDENTIFICATION HAS CHANGED`（ED25519 `SHA256:1X4VOFiPleLPTH5hMRT3LclpyG4lto4OFvsrwqPk/zw`、
  `~/.ssh/known_hosts:3` の ECDSA 鍵と食い違い）で接続されなかった。host key の検査は迂回していない。人が host key の変更の理由を
  確かめてから、`ssh fern03 'for c in qstat sbatch; do command -v $c; done; bash -lc "command -v qstat sbatch"'` で scheduler の有無を
  見る。何も無ければ `"none"` のまま、`qstat` が PBS Pro なら `"pbs"`、`sbatch`/`sacct` なら `"slurm"` に直し、daemon を再起動する
  （`clusters` は起動時にだけ読む。ADR 2026-10-22-dynamic-config）。
- c7863978ccb5 より前の release（3c7fa588c844 など）へ戻すときは、config の `scheduler` の行も消す（旧い版は未知の欄で起動しない）。
- 確認: Pegasus の wait を待つ task で、新しい wait の行の `scheduler` が `nqsv` であること（`GET /api/v1/tasks/{id}` の
  `cluster_job_wait.scheduler`）。
- `job_wait.poll_failure_wake_secs` は任意（既定 1800 秒）。
- DB: migration は無い。既存の wait の行は書き換えない。昇格の前に `pbs` で登録された Pegasus の wait
  01M89PTZ9YVYY5C0NMR1H9E364（期限 2026-11-26T15:57:52Z）は、昇格の後も `qstat -xf` で失敗している（本番の log: 03:15:42Z から
  5 分ごとに `cluster job poll returned no job states …`、`exit 127`）。最初の失敗から `poll_failure_wake_secs`（1800 秒）を過ぎた最初の
  poll（03:45:42Z の後）で `poll_failed` で続きの run が起きるはず。この run の 03:33Z の時点では、まだ起きていない（未確認）。

## 未解決事項

- fern03 の scheduler は未確認（上の「本番への移行」）。本番の config は仮に `"none"`。
- wait 01M89PTZ9YVYY5C0NMR1H9E364 が `poll_failed` で起きることは、この run の時点（03:33Z）では未確認。
- NQSV の Q・R の `qstat -f` の書式は未取得のまま（2026-11-24 D4）。exit 0 で `Unknown` だけの poll は失敗に数えないので、
  書式の違いで状態を読めないときは poll の失敗の起床も起きない（期限か全 request の `does not exist` で解ける）。
- 起床の後の続きの run が待ち直し、poll の失敗が続くと、`poll_failure_wake_secs` ごとに run が起きる。設定の誤りなら worker は人に
  質問する（前置きの指示）。この繰り返しの回数の上限は置いていない（continuation の回数には数えない。ADR-0090 D1）。

## 提案

- 既存の config の型に必須の欄を足す release は、自動の取り込みのリリース準備（verify が本番の config を新旧の版に読ませる）を
  通らない。ADR の「本番への移行」に、人が config を書き換えて verify を流し直し停止 → 起動で昇格する手順を書くか、ADR-0075 D7 の
  順序（欄を任意で足し、昇格の後に本番の config に書き、次の release で必須にする）を選ぶかを、設計の時点で決める規則を
  `agent-docs/guides/` か ADR-0075 の付記に置く。今回はこの規則が無く、取り込みの後に差し戻しになった。
- worker の前置きに、そのクラスタの scheduler（`ClusterSpec.scheduler`）を書くと、続きの run が使う道具（PBS Pro の `qstat -x`、
  NQSV の `racctreq`）を選びやすい。今回は案内を「scheduler はクラスタの設定で決まる」に留めた。
