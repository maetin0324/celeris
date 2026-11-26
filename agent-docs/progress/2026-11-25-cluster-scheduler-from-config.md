---
title: cluster job の wait の scheduler をクラスタの設定で決め、poll の失敗が続いたら run を起こす
tasks: [01M8AJ2STJRB0VQCCFRXQQ3R2P]
status: done
updated: 2026-11-26
---
# cluster job の wait の scheduler をクラスタの設定で決め、poll の失敗が続いたら run を起こす

完了日: 2026-11-26

状態: done（ADR・実装・試験・失敗注入は済んだ。本番の config の書き換えは人が昇格と同時に行う。下の「本番への移行」）。
基準は `git merge-base HEAD main`。検査は提出時の HEAD で実行した。

## commit（件名）

- ADR: クラスタ job の wait の scheduler をクラスタの設定で決め、poll の失敗が続いたら run を起こす
- cluster job wait: scheduler を [[clusters]] scheduler で決め、poll の失敗が続いたら poll_failed で run を起こす
- progress: cluster job の wait の scheduler をクラスタの設定で決める task の証拠と本番への移行
- cluster job wait: 失敗の poll の分け方を task_core::cluster_job::read_poll_output に移し、dispatcher の本体を増やさない
- cluster job wait: 失敗の poll を wakeup::evaluate で既存の判定の経路に通し、dispatcher の method を増やさない
- cluster job wait: 試験用の [[clusters]] の config に必須になった scheduler を足す（cluster_admin・e2e の cluster_scenarios）
- progress: 検査の結果（test-affected・nextest の絞り込み・clippy・fmt）

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

### 3. 検査

提出時の HEAD で実行した結果は「検査の結果」節に書く。

## 検査の結果（提出時の HEAD）

提出時の HEAD（最後のコードの commit と fmt の後。main の取り込み・rebase はしていない）で実行した。この後の commit は
この進捗ファイルだけを変える。

- `bash scripts/dev/test-affected.sh` → exit 0（2026-11-26T01:12Z〜01:24Z。fmt の折り返しの前の 00:55Z〜01:07Z の 1 回目も exit 0）。
  repo-health は verdict=pass、web の型検査（tsc -b）は通り、web の unit は `Test Files 108 passed (108)`・`Tests 774 passed (774)`、
  Rust は `Summary [498.698s] 8929 tests run: 8929 passed (7 slow), 34 skipped`。
  - 前の run の 1 回目は、`[[clusters]] scheduler` を必須にしたことで欄の無い試験用の config が読み込みエラーになり、
    `celeris cluster_admin::tests` の 4 件と `e2e::cluster_scenarios` の 2 件
    （`missing_control_master_records_cluster_unavailable_and_does_not_block_idle`・`worker_run_cluster_without_control_master_exits_4`）
    が落ちた（`missing field scheduler`）。これらの config は job scheduler を使わない（wait を申告しない）ので
    `scheduler = "none"` を足した。試験の期待値は変えていない。
- `cargo nextest run -p task-core -p task-dispatch -p task-worker -E 'test(/cluster_job|job_wait|poll_fail|scheduler/)'`
  → exit 0、`69 tests run: 69 passed, 4790 skipped`。PBS Pro・Slurm・NQSV の poll の読み方の既存の試験
  （`task-core cluster_job::tests::*`）はこの 69 件に入り、期待値は変えていない。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0（`cargo fmt --all` で試験用の config の 1 行を折り返した後）。

## 本番への移行

人が行う。**この欄を知る release の昇格と同時に** config を書き換える（旧い版は未知の欄 `scheduler` で起動に失敗し、新しい版は
欄が無いと起動に失敗する。順序: release を作る → `~/.config/celeris/celeris.toml` の全ての `[[clusters]]` に `scheduler` を足す →
昇格（stop → start）。昇格を戻すときは config の `scheduler` の行も消す）。

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

- fern03 は未確認。2026-11-25 にこの run から `ssh -o BatchMode=yes fern03` で読み取りだけの確認を試みたが、
  `REMOTE HOST IDENTIFICATION HAS CHANGED`（ED25519 `SHA256:1X4VOFiPleLPTH5hMRT3LclpyG4lto4OFvsrwqPk/zw`、
  `~/.ssh/known_hosts:3` の ECDSA 鍵と食い違い）で接続されなかった。host key の検査は迂回していない。人が host key の変更の理由を
  確かめてから、`ssh fern03 'for c in qstat sbatch; do command -v $c; done; bash -lc "command -v qstat sbatch"'` で scheduler の有無を
  見る。何も無ければ `"none"`（その上での wait は登録時に断る）、`qstat` が PBS Pro なら `"pbs"`、`sbatch`/`sacct` なら `"slurm"`。
  example（`config/celeris.clusters.example.toml`）は `"none"` にした（「クラスタでないノードでの実験用」と書かれているため）。
- ほかに `[[clusters]]` があれば同じく scheduler を確かめて書く。
- 確認: 昇格の後 `journalctl --user -u celeris@<sha12>` に config の読み込みエラーが無いこと。Pegasus の wait を待つ task で、
  新しい wait の行の `scheduler` が `nqsv` であること（`GET /api/v1/tasks/{id}` の `cluster_job_wait.scheduler`）。
- `job_wait.poll_failure_wake_secs` は任意（既定 1800 秒）。足すなら昇格の後。
- DB: migration は無い。既存の wait の行は書き換えない。昇格の前に `pbs` で登録された Pegasus の `waiting` の wait
  （01M89PTZ9YVYY5C0NMR1H9E364、期限 2026-11-26T15:57:52Z）は、昇格の後も `qstat -xf` で失敗するが、
  `poll_failure_wake_secs`（30 分）の後に `poll_failed` で続きの run が起きる（期限が先に来ればこれまでどおり期限切れの質問）。

## 未解決事項

- fern03 の scheduler は未確認（上の「本番への移行」）。
- NQSV の Q・R の `qstat -f` の書式は未取得のまま（2026-11-24 D4）。exit 0 で `Unknown` だけの poll は失敗に数えないので、
  書式の違いで状態を読めないときは poll の失敗の起床も起きない（期限か全 request の `does not exist` で解ける）。
- 起床の後の続きの run が待ち直し、poll の失敗が続くと、`poll_failure_wake_secs` ごとに run が起きる。設定の誤りなら worker は人に
  質問する（前置きの指示）。この繰り返しの回数の上限は置いていない（continuation の回数には数えない。ADR-0090 D1）。

## 提案

- worker の前置きに、そのクラスタの scheduler（`ClusterSpec.scheduler`）を書くと、続きの run が使う道具（PBS Pro の `qstat -x`、
  NQSV の `racctreq`）を選びやすい。今回は案内を「scheduler はクラスタの設定で決まる」に留めた。
