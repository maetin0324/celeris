---
title: クラスタ job の失敗・長い Q/H による早期再開
tasks: [01M752V5864CMYZZ2QHYAJ4X0E]
status: done
updated: 2026-11-11
---
# クラスタ job の失敗・長い Q/H による早期再開

## 設計 brief

1. 根本原因: wait の再開判定が全終了だけを条件とし、部分失敗と長い Q/H を worker が調べる機会が無かった。
2. 責務 owner: 地図の durable wait 行にある task-core::cluster_job（状態解析・起床条件）、task-dispatch::dispatcher::cluster_job_wait（poll と再開）、task-worker（前置き）。
3. 既存の再利用先: ClusterJobWaitFinished event、ClusterJobResume、needs_continuation と既存の偽 poller・注入時計。
4. 選択肢: worker を動かしたまま監視する案は採らず、既存 durable wait の再開条件を拡張する。監視で LLM と run の枠を消費しないため。
5. 新しい状態・分岐・fallback・port: status JSON に Q/H の初回観測時刻と通知済み情報を追加。新しい wait 状態・DB 列・fallback・port は無し。
6. 撤去対象: 全終了だけに限定する再開判定と説明を置き換える。前置き組立ては worker の共通経路へ移し、dispatcher の旧本体を削除。
7. 不変条件: events 追記専用、task の正本は SQLite、自動 qdel/scancel 禁止、LLM を dispatcher に置かない。全成功・期限超過・計数除外の既存試験と、下記の追加試験で確認する。

完了日: 2026-11-11

## 変更

- commit 件名: `phase 1: wake cluster job waits on failure and queue stalls`

- 部分失敗と Q/H の観測時間を再開条件に追加。既存の wait 終了 event、task/WU continuation を使う。
- `queued_wake_secs`（既定 3600 秒）と検証を追加。通知済み情報を status JSON に保存し、Q/H の起床は同じ job の待ち直しでも繰り返さない。失敗の起床は抑制しない。
- PBS の資源指定・comment・estimated.start_time を折返しも含めて引き継ぐ。
- continuation の組立てを task-worker に移動。worker の指示と設定例を更新。
- ADR: `2026-11-11-cluster-job-early-wakeup.md`。

## 試験の不変条件と追加理由

全終了の既存試験では、失敗の後も他 job を待ってしまう問題、Q/H の長期化と待ち直し時の通知重複を検出できなかった。
偽 poller と注入時計で、atomic・v2/v3 WU の早期起床、全 job と理由の前置き、
dispatcher 再作成、Q→H、再待機、continuation 回数不変を確かめる。
Slurm は状態が FAILED 等で exit 0 の場合と、exit 0 だが signal 非ゼロの場合も失敗とする。

## 検査環境と補修

- sandbox では共有 runner cache が書込不可のため `CELERIS_SCRIPT_RUNNER_CACHE=$TMPDIR/script-runner-cache` を使う。
- pnpm 12 の offline store は `PNPM_CONFIG_STORE_DIR=$PWD/.pnpm-store` で検証用の写しにも引き継ぐ。
  offline install の不足を復元後、小さな別ディレクトリで offline install が通ることを確認した。
- 既存の起動通知試験は Codex stub の設定に一時ホームを渡すよう修正。呼出元のアカウントの書込権限への依存を除き、通知の順序・回数・失敗時の assertion は維持した。
- EventRow・API・worker protocol の schema と web の生成型を更新。

## 証拠

提出時の HEAD で実行。基準は `git merge-base HEAD main`。最後の同期後にソースを変更していない。

- 条件 0・2: `bash scripts/dev/test-affected.sh` → exit 0。偽 poller の `partial_failure_resumes_atomic_with_every_job_and_can_wait_again` と `partial_failure_resumes_v2_and_v3_work_units` が合格。3 job の部分失敗・Q/R・理由の前置き、再待機、計数除外を確認。
- 条件 1: 同コマンド → exit 0。`queued_job_wakes_once_across_rewait_with_scheduling_evidence` と従来の成功・上限試験が合格。資源指定・comment・開始見込み、Q/H の通知一度、dispatcher 再作成後の継承を確認。
- 条件 3・4: worker の共通指示、継続の前置き、ADR と設定例を確認。文書検査 4 本は exit 0。
- 条件 5: `bash scripts/dev/test-affected.sh` → exit 0。Rust 8224 passed / 0 failed / 23 skipped（907.019 秒）、対象 doc test 合格、web の型検査・unit 712 件合格、test-ts-verify 20 passed / 0 failed、script bundle SKIP 0。
  sandbox の検証では `NEXTEST_TEST_THREADS=8`、`PNPM_CONFIG_STORE_DIR=$PWD/.pnpm-store`、`CELERIS_SCRIPT_RUNNER_CACHE=$TMPDIR/script-runner-cache` を指定。ログは run artifacts の `acceptance-submit.log`。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/progress-index.sh --check`、`python3 scripts/dev/check-architecture-map.py` → exit 0。
- `git diff --check` → exit 0。

## attempt 2: review の時間切れの調査

attempt 1 の review では、条件 5 と workspace.toml の `bash scripts/dev/test-affected.sh` が 600 秒と再試行の 1200 秒の両方で時間切れになった。
コードの不具合ではなく、検査の量と host の負荷が原因と判断し、コードは変えていない。

- 検査の量: task-core を変えたため、逆依存が jev-classifier 以外の 14 crate（nextest 8224 件）になる。加えて test-ts-verify（runner の cache key の入力の変更）と web の段も走る。
- 負荷: 実測時の load average は 64〜88（24 core）。同じ時間に別の task の test-affected が 2 本走っていた。
- 提出時の HEAD での実測（2026-11-11 13:14〜13:50 UTC）は合計 35 分 33 秒。check-health が 4 分 9 秒、検査器の自己試験が 4 分 19 秒、web が 18 秒、test-ts-verify が 6 分 27 秒、nextest の build が 3 分 36 秒、nextest が 971 秒。
- この実測では変更範囲外の 3 件が負荷で落ちた。`dispatcher::tests::git_workspace::review_records_each_repositories_own_definition_and_worktree_results`（each repository must have its own evidence）、`browser::post_login_tests::daemon_post_login_waits_through_slow_shibboleth_hops`（target_changed）、`celeris-campaign::k4_cells_validity real_k4_archive_grades_thirty_of_thirty_two`（600 秒で時間切れ）。
- 同じ HEAD で単独に再実行すると 3 件とも合格した。前 2 件と cluster_job の試験 28 件の 30 件が 24.5 秒で合格、k4 の試験は 36.6 秒で合格。
- doc test（同じ 14 crate）は exit 0（45 秒）。
- 2 回目の実測（提出時の HEAD、13:59〜14:22 UTC、load average は 47 から 89〜102 に上がった）は 22 分 27 秒、exit 100。nextest は 899.6 秒で 8222 件合格、2 件が 600 秒で時間切れ（`celeris-campaign::k4_cells_validity real_k4_archive_grades_thirty_of_thirty_two`、`celeris-campaign::k_cases_grade k3_k4_recompute_rejects_a_wrong_report_and_accepts_the_raw_derived_one`）。
- 時間切れの 2 件は原票の tgz を展開して採点する重い試験で、変更範囲外。単独の再実行では合格した（k4 は 36.6 秒、k3_k4_recompute は 137.2 秒、`dagbench_graded real_k3_case_is_graded_independently_on_both_arms` は 126.4 秒）。
- 低い負荷では同じ検査が 1200 秒に収まりうる。attempt 1 の nextest は 421 秒の回があった。
- `cargo fmt --all -- --check` と `cargo clippy --workspace -- -D warnings` は提出時の HEAD で exit 0。
- test-affected が 3 回続けて通らなかった（attempt 1 の review、attempt 2 の実測 2 回）ため、CLAUDE.md の規則に従い、進め方を人に質問する。

## attempt 3: 人の回答に従った提出

人の回答は「再検査」だった。host の負荷が下がった（load average 20 前後）ので、operator が 1 件ずつ順に再検査に回す。
回答の指示は「コードを変えず、提出時の HEAD の検査結果が進捗にあるなら、負荷を増やさないため test-affected を走らせ直さずに提出してよい（review が走らせる）」。

- コードは変えていない。commit『phase 1: wake cluster job waits on failure and queue stalls』の後の commit は、この進捗ファイルだけを変えている（`git diff --stat` で 1 file を確認）。
- test-affected は、回答に従ってこの run では走らせていない。同じコードでの合格の記録は上の「証拠」節（attempt 1、8224 passed、exit 0）にある。
- 提出時の HEAD で実行（2026-11-11 19:11 UTC、load average 11〜20）:
  - `cargo fmt --all -- --check` → exit 0。
  - `cargo clippy --workspace -- -D warnings` → exit 0（55 秒）。
  - `sh scripts/dev/progress-index.sh --check` → exit 0。

## attempt 4: 失敗の起床を失敗の発生ごとに 1 回にする

final review の欠陥: `wakeup::observe` は失敗した job を見るたびに `job_failed` を返していた。
起床の後の続きの run が失敗済みの job を含めたまま待ち直すと（例: `[1(F, Exit_status -14), 2(Q), 3(R)]`）、
次の poll でまた起き、`poll_secs`（sirius は 300 秒）ごとに LLM の run が起き続ける。D1 によりこの run は
continuation の回数にも進捗なしの窓にも数えられず、止まるのは `max_wait_secs` の質問だけだった。

### 実装前 brief

1. 根本原因: `wakeup::observe` の失敗判定が今回の観測だけを見て、同じ job の直近の観測（durable wait の `last_status`）を見ていない。
   Q/H の起床は `queue_notified` で 1 回にしていたが、失敗は「同じ id の rerun がある」として毎回起こしていた。症状は dispatcher の poll（`apply_cluster_job_statuses`）で出る。
2. 責務 owner: `task_core::cluster_job::wakeup::observe`（起床条件の唯一の判定）。dispatcher は理由を受け取るだけなので変えない。
3. 既存の再利用先: `observe` が `queued_since` のために集めている `previous`（今の wait の `last_status`、無ければ同じ task・WorkUnit・cluster・scheduler の過去の wait の `last_status`、新しい順）と `failed()`。
   試験は task-core の `wait_for`・`parse_pbs_qstat_xf`、dispatcher の偽 poller と注入時計（`cluster_job_wait.rs` の試験の helper）。
4. 選択肢: (a) `previous.first()` が失敗の終了なら起こさない（採用。追加の状態なしで「失敗の発生ごとに 1 回」になり、Q/R/H を観測した後の再失敗は起こす）。
   (b) status に「失敗を通知した」欄を足す（不採用。`previous` から導けるので正本が 2 つになる）。(c) dispatcher で失敗済み job を待ち直しから外す（不採用。owner の外の特例で、worker の待ち直しの自由を奪う）。
5. 新しい状態・分岐・fallback・port: 無し（DB 列・status の欄も足さない）。`observe` の失敗判定に直近の観測の条件を 1 つ足すだけ。
6. 撤去対象: 試験 `failure_is_not_suppressed_when_the_same_job_is_waited_again`、`observe` の doc の「a failed observation always wakes the worker」、ADR 2026-11-11-cluster-job-early-wakeup の「失敗の起床は抑制しない」の文。
   module doc（`cluster_job.rs`・dispatcher の `cluster_job_wait.rs`）の全終了だけの説明も書き直す。
7. 不変条件: 失敗の発生 1 回につき起床は高々 1 回。待ち直しでは失敗済み job は終わった job として扱い、残りが終われば satisfied で再開する。
   全成功の再開・上限切れの質問・D1・D6 は変えない。task-core の試験 2 件と dispatcher の試験 1 件で確かめ、既存の cluster_job 系の試験も通す。

### 変更（attempt 4）

- `task_core::cluster_job::wakeup::observe`: 失敗した job は、直近の観測（`queued_since` と同じ `previous`。
  今の wait の `last_status`、無ければ同じ task・WorkUnit・cluster・scheduler の過去の wait の新しい順）のうち
  `Unknown`（出力に job が無い）でない最初のものが既に失敗の終了なら `job_failed` にしない。DB 列・status の欄は足していない。
- 試験: `failure_is_not_suppressed_when_the_same_job_is_waited_again` を消し、task-core に
  `the_same_failure_does_not_wake_again_when_waited_again`（待ち直し・同じ wait の次の poll・`Unknown` を挟んでも起きない、
  別の WorkUnit では起きる）と `a_rerun_seen_queued_that_fails_again_wakes_again`（Q・R・H の後の再失敗で起きる）を足した。
  dispatcher に `rewaiting_with_an_inspected_failure_does_not_wake_again_until_the_rest_finish`（起床の後に `[1(F,-14),2(Q),3(R)]`
  をそのまま待ち直し、注入時計で 3 回 poll しても run は 2 本のまま・wait は waiting・`ClusterJobWaitFinished` は 1 件、
  残りが終わると satisfied で 3 本目の run が起き done、D1 の計数は 0）を足した。
- 既存の試験で検出できない理由: 既存の部分失敗の試験は失敗した job を外して待ち直すので、残した失敗による起床の連続を見ない。
  旧い規則（失敗を見るたびに起こす）に戻すと、task-core の 1 件と dispatcher の 1 件が落ちることを確かめた
  （rerun の試験は旧い規則でも通る。旧い規則でも起きる場面の試験なので）。
- ADR 2026-11-11-cluster-job-early-wakeup: 「失敗の起床は抑制しない」を、失敗の発生ごとに 1 回の規則と限界
  （`poll_secs` 以内に rerun が始まって再び失敗した場合は検出しない）に書き換えた。
- `CLUSTER_JOB_RECOVERY_GUIDANCE`（task-worker `ssh.rs`）: 待ち直しでは調べ終えた終了済みの job を外し、まだ Q/R の job と
  再投入した job だけを書くこと、失敗済みの job を残しても再び起こされないことを足した。
- module doc（`task-core/src/cluster_job.rs`、`task-dispatch/src/dispatcher/cluster_job_wait.rs`）を、全終了に加えて
  `job_failed`・`queue_stalled` でも再開すると書き直した。

### 検査（attempt 4、提出時の HEAD で実行）

commit『phase 1: wake once per cluster job failure so re-waits with a failed job do not loop』の後に実行した。
その後の commit『phase 1: 進捗に attempt 4 の検査結果を書く』はこの進捗ファイルだけを変える。

- `cargo nextest run -p task-core -p task-dispatch -p task-worker -p celeris --lib -E 'test(cluster_job) | test(job_wait) | test(preamble) | test(rerun) | test(failure)'` → 247 passed（exit 0）。
- 新しい 3 件を旧い規則（失敗を見るたびに起こす）に一時的に戻して実行 → task-core の `the_same_failure_does_not_wake_again_when_waited_again` と dispatcher の `rewaiting_with_an_inspected_failure_does_not_wake_again_until_the_rest_finish` が落ちた（コードは戻してある）。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（53 秒）。
- `bash scripts/dev/test-affected.sh` → exit 0（812 秒、20:05〜20:19 UTC、開始時 load average 3.9、終了時 43.6）。
  14 crate の nextest 8235 passed・23 skipped、web の tsc と vitest 712 passed・server の node test、test-ts-verify 20 passed、repo_health_gate all checks passed。
- `sh scripts/dev/progress-index.sh --check` → exit 0。

## 本番への移行

`queued_wake_secs` は、そのキーを知る release の昇格後に人が本番設定へ追加する。
省略時は 3600 秒。BenchFS の task、本番 job、本番設定は操作しない。

## 未解決

- 実クラスタの job 操作は実行していない。

## 提案

- task-core を変える task の test-affected は全 crate とほぼ同じ量になり、load average 60 以上の host では review の上限（`review_timeout_secs` = 600 秒、再試行 1200 秒）に収まらない。review の決定的検査の同時実行数を host の core 数で制限するか、重い検査の review を負荷の低い時に回す仕組みを別 task で検討することを提案する。
- celeris-campaign の原票 tgz を使う試験（k4_cells_validity・k_cases_grade・dagbench_graded の real 系）は単独でも 36〜137 秒かかり、負荷の下で 600 秒の上限に達する。全体の試験の中での扱い（slow 枠の分離など）を別 task で検討することを提案する。

変更範囲外の `celeris-credentiald::injection_ipc::concurrent_first_request_streams_over_the_limit_are_denied_unread`
が全体実行で一度 `call: Io` になった。コードを変えない単独再実行は 1 件合格（0.03 秒）。
待機枠を解放した後の接続の拒否と client の読み方は別 task で調べる候補とする。原因は未確定。
credentiald のコード・試験は変更せず、最終の影響範囲試験は `NEXTEST_TEST_THREADS=8` で再実行し、8224 件すべて合格した。

