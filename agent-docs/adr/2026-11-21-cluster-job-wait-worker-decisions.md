---
tasks: [01M7YWAC1JMG1Y87EA9XM1QSJ8]
---
# ADR 2026-11-21: クラスタ job の wait で終わった run の決定の要求は、wait を開くときに記録し、job の終了と答えの両方を待って再開する

- 日付: 2026-11-21
- 状態: Accepted
- 関係: [ADR-0079](0079-recursive-task-decomposition.md) D7（worker の決定。`needed_before` が要求を出した仕事を止める）、
  [ADR-0090](0090-cluster-job-wait.md) D2（WU の `blocked(cluster_jobs)` と wait の行、job の終了による再開）、
  [ADR 2026-11-21-worker-decisions-fail-closed](2026-11-21-worker-decisions-fail-closed.md)（以下「fail-closed の ADR」）。
  この ADR は fail-closed の ADR の付記 2026-11-21 が「外す経路」に残したクラスタ job の wait（WU の遷移 `cluster_jobs`、
  atomic の `RunEnd::Waiting`）を閉じる。fail-closed の ADR の D1（守る規則）をこの経路にも当て、D2〜D5 と付記の読む規則・
  止め方の順・atomic の規則は変えない。
- task: 01M7YWAC1JMG1Y87EA9XM1QSJ8

## 事象

fail-closed の ADR の付記は、木の節点の worker の run の `decisions` を終わり方によらず読むようにしたが、クラスタ job の wait で
終わった run は除いた。

- WU の run: `WorkUnitSettlement::reads_worker_decisions` が遷移 `cluster_jobs` で false。
- atomic の run: `task_reads_worker_decisions` が `RunEnd::Waiting` で false。

除いた理由は、待ちの正本が WU の行の `blocked(cluster_jobs)` と wait の行（`cluster_job_waits`）で、job の終了による再開
（`cluster_job_wait.rs` の `finish_cluster_job_wait`）が `blocked(cluster_jobs)` の行からだけ動くこと。settlement で
`hold_for_worker_decisions` を当てて `blocked(decision)` に置き換えると、待ちの理由の欄が 1 つなので job の待ちが行から消え、
wait の行だけが残る（正本が 2 つ）。job が終わっても `cluster_wait_unit_row` が行を見つけられず、unit は再開しない。

除いたままだと、その run が書いた決定の要求は決定の行にも受信箱にもならない。job が終わると unit は `needs_continuation` に戻り、
人の答えの前に leaf が進む。要求は job の後の run の `result.json` が出し直す前提で、出し直さなければ人に届かない。

## 選択肢

1. 待ちの理由を 2 つ持てるようにする（WU の行に `blocked_reason` を複数持つ、または `cluster_jobs` と `decision` を合わせた
   新しい理由を足す）。行・replay（`task_ops::replay` の遷移の理由から `blocked_reason` への写し）・liveness
   （`task_core::tree::liveness`）・store の列の全てを変える。新しい状態を足すので採らない。
2. settlement で `blocked(decision)` にし、job の終了を wait の行だけで覚える。`blocked(cluster_jobs)` の行が消え、
   `cluster_wait_unit_row` が再開できない（上の事象）。再開を wait の行から引くように変えると、unit の行の待ちと wait の行の
   待ちの正本が 2 つになる。採らない。
3. job が終わっても答えまで unit を `blocked(cluster_jobs)` のまま残し、答えの側で再開する。wait の行は閉じているのに行が
   `cluster_jobs` を待っていると言い、liveness は「クラスタ job の終了を待っている」と誤って示す。採らない。
4. （採用）待ちの理由は常に 1 つで、今の待ちを表す。run の終わりでは `blocked(cluster_jobs)` のままにし、決定の要求だけを
   記録する。job の終了による再開は、決定がまだ unit を止めていれば `needs_continuation` の代わりに `blocked(decision)` にする。
   答えによる解放は、最後の run が job の wait で終わった unit を `pending`/`ready` でなく `needs_continuation` に戻す。
   再開の条件は「job の終了」と「答え」の両方で、後に来た方が unit を `needs_continuation` にする。

## 決定

- D1（読む場所と記録）: クラスタ job の wait で終わった木の節点の run（WU の遷移 `cluster_jobs`、atomic の trigger
  `ClusterJobWait`。どちらも wait の行を開く run）の `result.json` の `decisions` は、wait を開くときに
  `Dispatcher::cluster_wait_worker_decisions`（`cluster_job_wait.rs`）が既存の `Dispatcher::worker_decisions` で読む。
  受け付けた決定・拒否のまとめ（fail-closed の ADR D1・11-18 の ADR D1）・他の unit の `blocked(decision)` は、既存の経路と
  同じく `WorkerFinished` の後の書き込み（`work_units_apply`）で記録する。決定の行と受信箱の項目はこの書き込みでできる。
  - settlement の読む規則（`reads_worker_decisions`・`task_reads_worker_decisions`）は変えない。どちらも「settlement で読み、
    その場で leaf を止める run か」を表し、クラスタ job の wait では false のまま（止めは wait の再開が持つ）。
  - run の遷移・trigger・wait の行は変えない。WU は `blocked(cluster_jobs)`、atomic の task は `ClusterJobWait` で
    `blocked` のまま。`needed_before: self` の要求なら outcome の文の末尾に「人の決定を待ちます（決定の要求 N 件。ADR-0079 D7）」を
    足す（atomic は既存の `hold_task_for_worker_decisions` が `ClusterJobWait` を残して文を足す）。
- D2（job の終了が先: WU）: `finish_cluster_job_wait`（satisfied と v2 の timed_out）と v1 の timed_out の unit の行は
  `cluster_wait_unit_row` が作る。この関数は、`task_ops::decision::holds_unit` が「この unit を止める決定がある」
  （未回答か、`replan` で答えて replan が済んでいない決定が `needed_before` で unit か `stage:<その段階>` を名指しする、または
  `needs_decisions` に未回答のものがある。`release_rows` と同じ規則）と答えたら、行を `blocked(decision)`
  （遷移の理由 `decision`。replay は `decision` を `Decision` に写す）にする。wait の行は今までどおり閉じる
  （`ClusterJobWaitFinished`。poll は止まる）。task の `ClusterJobResume` も今までどおり（v1 の task は `ready` に戻り、
  unit が `blocked(decision)` なので走る unit が無い）。
- D3（答え: WU）: `task_ops::decision::release_rows` は、解放する unit の最後の run（`runs` 索引の
  `runs_for_work_unit` の最後の行）が job の wait で終わっていれば（`RunIndexStatus::Waiting`）、`pending`/`ready` でなく
  `needs_continuation` に戻す。次の run は checkpoint の続きとして走り、前置きに job の最終状態
  （`work_unit_continuation_context` の `cluster_jobs`）と人の決定の答えが入る。その他の unit の解放は今のまま（fail-closed の
  ADR 付記「答えの後の run」）。
- D4（答えが先: WU）: unit は `blocked(cluster_jobs)` のままで、`release_rows` は `blocked(decision)` だけを解放するので触らない。
  job が終わると `holds_unit` が false を返し、今までどおり `needs_continuation`（理由 `cluster_jobs_finished` か
  `cluster_jobs_timed_out`）になる。取り下げ（`withdraw_rows`）は今までどおり `blocked` の unit を `cancelled` にし、job の
  終了は行を見つけずに wait だけを閉じる。
- D5（atomic）: 止めの正本は決定の行 1 つ（fail-closed の ADR 付記「atomic の規則」と同じ）。job の終了が先なら
  `ClusterJobResume` で task は `ready` になり、`dispatch_admission` の `decision_self_hold` が答えまで run を起こさない。
  答えが先なら（`Answered` の event は task の状態を変えない）、task は job の終了まで `blocked` のままで、job の終了の後の run は
  答えと job の状態を受け取る。`cluster_job_wait.rs` の atomic の再開は変えない。
- D6（作らないもの）: 新しい待ちの理由・WU の状態・DB の列・event の種類・決定の kind・設定の欄は作らない。

## 守る不変条件

- I1: クラスタ job の wait で終わった木の節点の run の `result.json` に `null` でも空の配列でもない `decisions` があれば、決定の行が
  1 件以上でき、受信箱（`task_ops::decision::open_rows`）に出る。
- I2: `needed_before` がその leaf（atomic なら `self`）を指す未回答の決定がある間、job が終わっても leaf は走らない
  （WU は `blocked(decision)`、atomic は `ready` で `decision_self_hold`）。
- I3: job の終了と答えのどちらが先でも、両方が揃った後の最初の run は job の wait の続き（WU は `needs_continuation`、
  atomic は checkpoint の続き）になる。
- I4: 待ちの理由は常に 1 つ。wait の行が `waiting` の間、WU の行は `blocked(cluster_jobs)` で、job の終了による再開は
  その行からだけ動く。
- I5: fail-closed の ADR が直した 5 経路と、fail-closed・問題無しとした経路の挙動と試験の期待値は変えない。
  `decision_fail_closed_reads_every_end_but_reset_and_cluster_jobs`・`decision_fail_closed_task_reads_every_end_but_reset_and_waiting`
  の settlement の読む規則の表もそのまま。

## 試験

`crates/task-dispatch/src/dispatcher/tests/cluster_job_wait.rs` に足す（名前は `cluster_wait_decisions_` で始める）。時計は
`FakeClock`、job の状態は既存の試験と同じ通知（`apply_cluster_job_notice`）で決定的に渡す。sleep と負荷は使わない。

- `cluster_wait_decisions_job_end_first_holds_the_leaf_until_the_answer`: 木の節点の v2 の WU の leaf が job の wait で終わり、
  `result.json` に `needed_before: [leaf]` の決定を書く。決定の行（open・origin worker）と受信箱の項目ができ、unit は
  `blocked(cluster_jobs)`。job の終了で unit は `blocked(decision)`（`needs_continuation` にならない）、wait は satisfied。
  答えると unit は `needs_continuation` になり、次の run の前置きに job の状態が入る（I1〜I4）。
- `cluster_wait_decisions_answer_first_resumes_on_the_job_end`: 同じ run で答えが先。答えの後も unit は `blocked(cluster_jobs)`。
  job の終了で `needs_continuation`（I3・I4）。
- `cluster_wait_decisions_atomic_node_waits_for_both`: 木の子の atomic の節点が job の wait で終わり、`needed_before: [self]` の
  決定を書く。決定の行と受信箱の項目ができ、task は `blocked`。job が先なら task は `ready` だが run は起きず、答えの後に
  走る（I1・I2・I3）。
- `task_ops::decision` の単体試験 `release_resumes_a_waited_unit_as_a_continuation`: 最後の run が wait で終わった unit は
  `needs_continuation`、そうでない unit は今のまま `pending`/`ready` に戻す（D3）。

既存の試験で検出できない理由: `tests/cluster_job_wait.rs` の試験は `result.json` に `decisions` を書かず、`tree_decisions.rs` の
adapter は wait で終わらない。fail-closed の ADR の 2 つの表の試験は settlement の読む規則を見るだけで、wait の経路の記録と
再開の条件を見ない。
