---
tasks: [01M7XSGW4FH258N1K3AMW76Y6F]
---
# ADR 2026-11-21: worker の決定の要求は、読める `result.json` に書かれていれば記録するか拒否のまとめにする（fail-closed）

- 日付: 2026-11-21
- 状態: Accepted
- 関係: [ADR-0079](0079-recursive-task-decomposition.md) D7・§7 R3a 11.（worker の決定）、
  [ADR 2026-11-18-rejected-worker-decisions-reach-human](2026-11-18-rejected-worker-decisions-reach-human.md)（拒否した要求を 1 件の決定にする。
  以下「11-18 の ADR」）。この ADR は 11-18 の ADR の D1 を、要素の誤りから `decisions` の欄そのものの型の誤りに広げる。束ね・要素の拒否・
  D3（読み直し）・D4（`needed_before` を読み替えない）は変えない。
- task: 01M7XSGW4FH258N1K3AMW76Y6F（戦略層 v1 子 1 の unit decision-failclosed）
- 調査の表: [survey.md](../progress/2026-11-21-strategy-v1-1/decision-failclosed/survey.md)

## 背景

ADR-0079 R3a 11. は「捨てた要素は進行の 1 行に理由を残し、run は失敗させない」とした。2026-11-18 の事象（task 01M7Q5ZABN9NFQCH6QBTTP3EZ8、
`needed_before` に integrate の unit 名）でこの規則が人への要求を黙って捨てたので、11-18 の ADR は拒否した要素を key `rejected`・
`needed_before: [self]` の 1 件の決定にして、要求を出した仕事を止めるように改めた。

11-18 の ADR が扱ったのは `decisions` の**配列の要素**の誤りだけだった。HEAD のコードを読むと、要求が人に届かないまま仕事が進む経路が
まだ 3 つある（調査の表の to-fix の行）。

1. `decisions-not-array`: `decisions` が object・文字列・数のとき、`task_core::decision::worker_decisions_from_json` が空の配列を返し、
   `Dispatcher::worker_decisions` が `None` を返す。決定の行も進行の行も残らず、leaf は done になる。
2. `continuable-end`: leaf の run が yield・予算切れ・クラスタ job の wait で終わったとき、`run_settlement` は `decisions` を読まない
   （読むのは WU の遷移の理由が `completed` の run と、atomic の `WorkerDone` かつ `RunEnd::Completed` の run だけ）。
3. `failed-end`: leaf の run が `summary` と `decisions` を書き、checks が落ちて失敗に替えられたとき（または worker が失敗を返したとき）も読まない。

worker の前置き（`task_worker::preamble::decision_requests_section`）は「受け付けられない要求は理由を添えた 1 件の決定にまとめて人に出され、
この仕事は答えが出るまで止まります」と約束している。上の 3 つの経路はこの約束を破る。

## 決定

- D1（守る規則）: 木の節点の worker の run が読める `result.json` に `decisions` の欄を書いたら、daemon はその欄を次のどれかにする。
  (a) 受け付けた決定（束ねを含む）として記録する。(b) 11-18 の ADR D1 の拒否のまとめ（key `rejected`、`needed_before: [self]`〈leaf の run
  なら leaf の key〉）にする。(c) 欄が無い・`null`・空の配列なら何もしない。進行の 1 行だけ・何も残さない、のどちらにもしない。
  run が完了したかどうかは、この規則に関係しない（要求した仕事を止めるのは `needed_before` の効き目で、run の終わり方ではない）。
- D2（`decisions-not-array` を owner で直す）: owner は `task_core::decision` と `Dispatcher::worker_decisions`
  （`crates/task-dispatch/src/dispatcher/tree_units.rs`）。
  - `worker_decisions_from_json` は欄の値をそのまま返す（無い・`null`・JSON でないときは `None`）。配列でない値を空の配列にしない。
  - `prepare_worker_decisions` の入力を欄の値にする。配列でなければ、要素の拒否と同じ `RejectedRequest` を 1 件足す（要約は欄の型、理由は
    `decisions must be an array of decision objects (got <型>)`、後戻りは分からないので `high`）。まとめは既存の `rejected_decision` で作る。
  - 配列でない object が決定 1 件の形をしていても受け付けない。1 件の配列として読み替える案は採らない（承認の無い読み替えを増やさない。
    worker は答えの後の run の前置きで誤りを知る）。
  - `Dispatcher::worker_decisions` は、欄が無い・`null`・空の配列なら今と同じく `None` を返す。それ以外は `prepare_worker_decisions` に渡し、
    既存の経路（`WorkerDecisionBatch::recorded` → `DecisionRequested`、`self_hold` → `hold_for_worker_decisions`〈atomic は
    `hold_task_for_worker_decisions`〉、答えるまでは `decision_self_hold`）で人に届けて、要求を出した仕事を止める。
- D3（`continuable-end`・`failed-end` は owner の外。replan で直す）: 直す owner は `run_settlement`
  （`WorkUnitSettlement::reads_worker_decisions`・`task_reads_worker_decisions`）と、その呼び出し側の `worker_finish.rs`（高 risk の path class
  `worker-finish`）で、unit `fix` の対象のパス（`decision.rs`・`tree_units.rs`・`tree_decisions.rs`）の外にある。unit `fix` は D2 を直したうえで
  `plan_issue` を返し、`run_settlement`・`worker_finish.rs` を範囲に入れた unit を足す replan を求める。その unit の計画には設計 brief を書く。
  直し方の案（その unit の設計 brief で確かめる）:
  - leaf の run が `Yielded`・`BudgetExhausted`・`Waiting` で終わったときも `Dispatcher::worker_decisions` を呼ぶ。`self_hold` なら
    continuation・job の終了待ちの代わりに `blocked(decision)` にする（答えの後の run が checkpoint から続ける）。
  - checks の不合格・worker の失敗で終わった run も同じく読む。`self_hold` なら再試行の代わりに `blocked(decision)` にし、答えの後の run が
    再試行になる。
  - atomic の run も同じ規則にする（`task_reads_worker_decisions` を `RunEnd::Completed` だけにしない）。
  - `harness_error`・取り消しの run は読まない（run をやり直し、次の run の `result.json` が要求を出し直す）。
- D4（fail-closed と判定し、変えない経路）: 調査の表の判定のとおり。計画の `decisions` の誤り（`plan_invalid` の決定と `decision_self_hold`）、
  壊れた `result.json`（run が `error(retryable=true)` で失敗する）、atomic の run（`self` のまま節点の止め）、質問と `decisions` の併記
  （質問が最優先で WU は `blocked(question)`）は、人に届くか仕事が人の答えまで止まる。木の節点でない run・木が無効な run・planner の run は
  書き方の節を前置きに出さないので、`decisions` は契約の外の欄として読まない。
- D5（作らないもの）: 新しい通知の仕組み・Manager・DB 表・run の失敗の種類・決定の kind は作らない。11-18 の ADR D2 と同じ理由で、run の完了を
  拒んで worker に書き直させる方式も採らない。

## 試験の計画と守る不変条件

不変条件:

- I1: 木の節点の worker の run の読める `result.json` に、`null` でも空の配列でもない `decisions` の欄があれば、決定が 1 件以上記録される
  （受け付けた決定か `rejected` のまとめ）。
- I2: `rejected` のまとめの `needed_before` は要求を出した仕事（leaf の key、atomic なら `self`）で、答えるまでその leaf は done にならず、
  それを待つ unit（同じ段階の integrate を含む）も進まない。atomic の節点は最終レビューに進まない。
- I3: 欄が無い・`null`・空の配列の run は、今と 1 バイトも変わらない（`Dispatcher::worker_decisions` が `None`）。
- I4: 欄の値の内容によって受け付けを変えない（配列でなければ、決定 1 件の形の object でも拒否する）。

unit `fix` が足す試験（名前は `decision_fail_closed_` で始める。修正の前に落ちる出力を進捗に記録する）:

- `decision_fail_closed_non_array_decisions_become_one_rejected_decision`（task-core の `decision` の単体試験）: `decisions` が object（決定 1 件の
  形のものを含む）・文字列・数のとき、`rejected` のまとめが 1 件でき、`needed_before` が leaf の key（`self_key = None` なら `self`）、後戻りが
  `high`、問いに型の誤りの理由が入る（I1・I2・I4）。無い・`null`・空の配列は何もできない（I3）。修正前は `worker_decisions_from_json` が空を
  返すので落ちる。
- `decision_fail_closed_non_array_decisions_hold_the_leaf`（`crates/task-dispatch/src/dispatcher/tests/tree_decisions.rs`）: leaf が
  `decisions` に object を書くと、`rejected` の行（open・origin worker・`needed_before = [leaf]`）ができ、受信箱の未回答の決定
  （`task_ops::decision::open_rows`）に出て、leaf は `blocked(decision)`・integrate は done にならない。答えると leaf が走り直し、木が終わる
  （I1・I2）。修正前は leaf が done になり決定が 0 件なので落ちる。
- `decision_fail_closed_non_array_decisions_hold_the_atomic_node`（同じ file）: 木の子の atomic の節点が `decisions` に文字列を書くと、
  `needed_before = [self]` の `rejected` の行ができ、答えるまで節点が done にならない（I1・I2）。修正前は節点が done になるので落ちる。

D3 の replan の unit が足す試験の名前の案: `decision_fail_closed_yielded_leaf_records_decisions`、
`decision_fail_closed_check_failed_leaf_records_decisions`、`decision_fail_closed_yielded_atomic_node_records_decisions`。

既存の試験で検出できない理由:

- `rejected_worker_decisions_become_one_self_decision`・`rejected_worker_decision_reaches_the_inbox_and_holds_the_leaf`・
  `worker_declared_decisions`・`worker_decisions_are_validated_and_self_is_resolved` は、どれも `decisions` に配列を渡す。
  `worker_decisions_from_json` の確かめ（`worker_decisions_are_validated_and_self_is_resolved` の末尾）は、欄が無いときの 0 件と配列の 1 件しか
  見ないので、配列でない欄が空になることを検出できない。
- dispatcher の試験の adapter（`tree_decisions.rs` の `DecisionAdapter`）は常に `summary` 付きの完了で終わる。yield・失敗・checks の不合格の
  run に `decisions` を書く場面が無いので、`continuable-end`・`failed-end` を検出できない。

試験の変更: `worker_decisions_from_json` の戻り値を欄の値にするので、`worker_decisions_are_validated_and_self_is_resolved` の末尾の 2 つの
確かめは新しい戻り値に合わせて書き換える（守る契約〈無いときは何もしない・配列は要素を読む〉は変えず、配列でない欄の確かめは
`decision_fail_closed_non_array_decisions_become_one_rejected_decision` が足す）。

## 付記 2026-11-21（読む規則と止め方の順）

unit settle-leaf（WU の run）で D3 の直し方を確かめ、次のとおりに決めた。owner は `run_settlement`（`work_unit.rs`）と
読む順を持つ `worker_finish.rs`。atomic の節点の run は unit settle-atomic が同じ規則で直す（下の「atomic の規則」）。

- 読む規則（WU の run）: `WorkUnitSettlement::reads_worker_decisions` は「状態だけを戻す run（harness_error・取り消し。
  `reset_only`）でなく、WU の遷移が `cluster_jobs` でない」とき true。遷移の理由は列挙しない。completed・continue・
  auto_leaf_budget・limit・continuation_cap・plan_issue・question・retry・failed・merge_repair_failed の全てで読む。
  旧い規則（done の遷移 `completed` だけで読む）は廃止した。
- 外す経路と理由:
  - harness_error・取り消し: run をやり直すだけで、WU は `ready` に戻る。次の run の `result.json` が要求を出し直す。
  - WU のクラスタ job の wait（`RunEnd::Waiting`、遷移 `cluster_jobs`）: 待ちの正本は WU の行の `blocked(cluster_jobs)`
    と wait の行で、job の終了による再開（`cluster_job_wait.rs`）は `blocked(cluster_jobs)` の行からだけ動く。待ちの理由の
    欄は 1 つなので、`blocked(decision)` に置き換えると job の待ちの正本が 2 つになり、job が終わっても再開できない。
    直すには `cluster_job_wait.rs`・`task_ops::decision` の変更が要り、この段の範囲の外なので読まない（要求は job の後の
    run の `result.json` が出し直す。close の未解決に入れる）。
- 止め方の順（`worker_finish.rs`）: `settle_work_unit` の直後に決定を読み、`needed_before: self` の要求があれば
  `hold_for_worker_decisions` で止める（決定の要求の数には settlement の auto-leaf の要求を含める）。その後に integration repair の打ち切り（`integration_repair_exhaustion`）と WU の
  失敗の処理（`settle_work_unit_failure`。自動 replan・質問・replan の使い切りの `limit:max_replans` の決定）を回すが、
  止めた settlement では `failure`・`integration_repair_exhaust`・`auto_replan` が空なので何も起きない。1 つの leaf を
  止めるのは決定 1 か所になる。auto-leaf の上限の決定の要求と worker の決定は 1 つの `WorkerDecisions` で両方記録する
  （旧い「worker の決定を読まないときだけ auto-leaf の要求を記録する」分岐は消した）。self でない決定だけの run は
  決定を記録し、遷移と失敗の処理は今のまま。
- 止めた leaf の行: どの遷移でも `blocked(decision)`（`WorkUnitTransitioned.reason = "decision"`）、task は
  `Continue{advance}`、待つ unit は ready にせず失敗も伝播しない。行は `running` の行を `blocked(decision)` にしたもので、
  この run の遷移の数え（`continue` の continuation・`retry` の再試行）は動かさない。replay（`task_ops::replay`）は
  WU の数えを遷移の理由から復元し、`decision` では数えを動かさないので、行と events が一致する。足す event は
  `DecisionRequested` と進行の行だけで、遷移の event は 1 本のまま。auto-leaf の上限の遷移（`auto_leaf_budget`）は
  理由と continuation の数えを残す（replay はこの理由で continuation を数えて `blocked(decision)` にする）。task の
  試行・replan の数え・予算の記録・checkpoint の記録は変えない。質問で終わる提案（trigger が `WorkerQuestion`。worker の
  質問・計画の問題・進捗なし・固い上限）を止めたときは、質問の文を outcome の末尾（`。止めた質問: …`）に残す。
- 答えの後の run: 答えは既存の解放（`task_ops::decision` の `decision_answered`）で leaf を `pending`/`ready` に戻す。
  `needs_continuation` ではないので、次の run は continuation でなく最初から走り、前置きの「人の決定」節に答えが入る。
  同じ条件（進捗なし・上限・失敗）なら、その run の終わりで数えと上限・replan が効く。
- atomic の規則（unit settle-atomic が入れる）: atomic の節点の run は、終わり方が分かり（`Some`）harness_error・取り消し・
  `Waiting` でなければ読む（trigger を列挙しない）。self の要求なら `WorkerDone`・`WorkerQuestion`・`WorkerError` を
  `Continue{advance}` に置き換え、`Continue{continue}`・`Continue{children_wait}` は残す。次の run は
  `dispatch_admission` の `decision_self_hold` が答えまで止める（止めの正本は decisions の行 1 つ）。
- 置き換えた試験: `worker_decisions_hold_a_completed_leaf_only`（`run_settlement/work_unit/tests.rs`）は「done の遷移
  だけで読む」旧い契約を確かめていたので消し、`decision_fail_closed_reads_every_end_but_reset_and_cluster_jobs`（読む
  規則。状態だけを戻す run の trigger・outcome を残す確かめを引き継いだ）と `decision_fail_closed_hold_clears_failure_side_effects`
  （止め方。done の止めの outcome・待つ unit・trigger の確かめを引き継いだ）に置き換えた。auto-leaf の試験
  `continuation_disabled_retries_the_unit_unless_the_auto_leaf_limit_stops_it` の `assert!(!s.reads_worker_decisions())`
  は、auto-leaf の上限の run も読む新しい規則に合わせて `assert!(s.reads_worker_decisions())` にした（`auto_leaf_request`
  の確かめは残した）。auto-leaf の要求が worker の決定と並んでも記録されることは
  `decision_fail_closed_auto_leaf_request_recorded_with_and_without_worker_decisions`（`dispatcher/tests/tree_gate.rs`）が守る。
