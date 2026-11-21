---
title: 戦略層 v1 子 1 decision-failclosed / settle-leaf — WU の run の decisions を D1 の規則で読み、self の要求なら失敗の処理より先に決定で止める
tasks: [01M7XSGW4FH258N1K3AMW76Y6F]
status: done
updated: 2026-11-21
---
# decision-failclosed / settle-leaf — WU の run の `decisions` を D1 の規則で読む

完了日: 2026-11-21

## 結論

- WU の run の `decisions` は、状態だけを戻す run（harness_error・取り消し）と WU のクラスタ job の wait（遷移 `cluster_jobs`）を
  除く全ての遷移で読む（`WorkUnitSettlement::reads_worker_decisions`。遷移の理由は列挙しない）。
- 読み取りは `worker_finish.rs` で `settle_work_unit` の直後、integration repair の打ち切りと WU の失敗の処理より前に置いた。
  `needed_before: self` の要求があれば `hold_for_worker_decisions` が leaf を `blocked(decision)`・task を `Continue{advance}` に
  して、失敗・integration repair の打ち切り・自動 replan の提案を消す。1 つの leaf を止めるのは決定 1 か所になる。
- auto-leaf の上限の決定の要求と worker の決定は 1 つの `WorkerDecisions` で両方記録する。
- `decision_fail_closed_*` の試験 4 件を先に書き、修正前は 4 件とも assertion で落ちた（before.log、exit=100）。修正後は通る
  （after.log、exit=0）。auto-leaf の守りの試験は修正の前も後も通る。
- ADR 2026-11-21-worker-decisions-fail-closed に「付記 2026-11-21（読む規則と止め方の順）」を足した。

## 実装前 brief

1. 根本原因: `reads_worker_decisions` が WU の遷移 `completed` だけで true だった。yield・予算切れ（continue・auto_leaf_budget・
   limit・continuation_cap・plan_issue）、質問、再試行・失敗、merge の repair の失敗で終わった run の要求は、決定の行も進行の
   行も残さずに捨てられた。さらに `worker_finish.rs` では WU の失敗の処理（integration repair の打ち切り・自動 replan・質問・
   replan の使い切りの決定）が決定の読み取りより先に走っていた。
2. 責務 owner: `run_settlement/work_unit.rs`（`reads_worker_decisions`・`hold_for_worker_decisions`）と、読む順を持つ
   `dispatcher/worker_finish.rs`（`finish_worker_result_with`）。
3. 既存の再利用先: `Dispatcher::worker_decisions`（受け付けた決定と `rejected` のまとめ）、`WorkerDecisions`、答えの後の解放
   （`task_ops::decision` の `decision_answered`）、試験は `tree_decisions.rs` の `DecisionAdapter`、`tree_gate.rs` の
   `AutoLeafAdapter`、`run_settlement/work_unit/tests.rs` の helper。
4. 選択肢: 設計 brief の採用案（D1 の規則で読み、決定を失敗の処理より先にする）を採った。遷移の理由を足す案・読む位置を後ろの
   まま広げる案・質問や replan を先にする案・cluster job の wait も読む案は、設計 brief と ADR の付記の理由で退けた。
5. 新しい状態・分岐・fallback・port: 無し。`hold_for_worker_decisions` の中に「auto-leaf の上限の遷移なら理由と数えを残す」
   「質問で終わる提案なら質問の文を outcome に残す」の分岐を足した（どちらも止め方の一部）。auto-leaf の要求の記録は旧い
   `else if` の分岐を消して、worker の決定と同じ記録に入れた。通知・Manager・DB 表・失敗の種類・決定の kind・fallback は足していない。
6. 撤去対象: `reads_worker_decisions` の「done にした WU だけ」の説明と挙動、`worker_finish.rs` の
   「`if reads_worker_decisions {…} else if auto_leaf_request`」の `else` と後置きの読み取り、旧い契約の試験
   `worker_decisions_hold_a_completed_leaf_only`、auto-leaf の試験の `assert!(!s.reads_worker_decisions())`。
7. 不変条件: I1（読む）・I2（止める）・I7（止めは 1 か所）・I3（決定が無い run は変わらない）・I5（cluster job の wait は読まない）。
   試験は下の表。replay の一致（行と events）は dispatcher の試験の `assert_replay_is_clean` で確かめる。

## 変えた関数

- `WorkUnitSettlement::reads_worker_decisions`（`crates/task-dispatch/src/run_settlement/work_unit.rs`）:
  `!reset_only && decision.reason != "cluster_jobs"` にした（旧: `decision.reason == "completed"`）。
- `hold_for_worker_decisions`（同じ file）: 引数を `key: &str` から `running` の行 `wu: &WorkUnitRow` にした。どの遷移でも
  - 行を `running` の行の `blocked(decision)` にし（lease を外す）、遷移の理由を `decision` にする。ただし `auto_leaf_budget` は
    理由と行（continuation を数えた行）を残す。
  - `plan_complete` を false にし、待つ unit の ready・失敗の伝播（`newly_ready`・`newly_blocked`）を空にする。
  - task の trigger を `Continue{advance}` にし、`failure`・`integration_repair_exhaust`・`auto_replan` を空にする。
  - outcome を `decision: WorkUnit <key> は人の決定を待ちます（決定の要求 N 件。ADR-0079 D7）` にする。N は worker の決定の数に、
    settlement に auto-leaf の要求があれば 1 を足す。trigger が `WorkerQuestion` だった（worker の質問・計画の問題・進捗なし・
    固い上限）なら、末尾に `。止めた質問: <質問の文>` を足す。
- `AUTO_LEAF_BUDGET_REASON`（同じ file）: `hold_for_auto_leaf` と `hold_for_worker_decisions` が同じ理由の文字列を使うための定数。
- `Dispatcher::finish_worker_result_with`（`crates/task-dispatch/src/dispatcher/worker_finish.rs`）: 決定の読み取りを
  `settle_work_unit` の直後（integration repair の打ち切り・`settle_work_unit_failure` の前）に移し、auto-leaf の要求を同じ
  `WorkerDecisions` に足す。関数の行数は base 以下（repo health の `size.responsibility-growth` が worsened=0）。

## 数えの扱い（数えを減らさない）

- `continue`（continuation +1）と `retry`（再試行 +1）の遷移を止めたとき、WU の行の数えは run の前の値のままにした。replay
  （`task_ops::replay`）は WU の数えを遷移の理由から復元し、`decision` では数えを動かさない（ADR-0072 D6 の表）。設計 brief の
  不変条件は「足す event は `DecisionRequested` と進行の行だけ」「task-ops に差分が無い」なので、遷移の event を 2 本にする案と
  replay を変える案は採れない。行に +1 を残すと replay が一致しない（`assert_replay_is_clean` が落ちる）。
- 記録済みの数えは下げない。task の試行（`Continue` は試行を消費しない）・replan の数え（`AutoReplanRequested` を数える）・予算と
  usage の記録・checkpoint の記録（`CheckpointSaved`）は変えない。auto-leaf の上限の遷移は `auto_leaf_budget` のまま continuation を
  数える。答えの後の run で同じ条件（進捗なし・上限・失敗）なら、そこで数えと上限・replan が効く。

## 足した試験（守る不変条件と、既存の試験で検出できない理由は各試験のコメントに書いた）

| 試験 | 場所 | 確かめること |
|---|---|---|
| `decision_fail_closed_reads_every_end_but_reset_and_cluster_jobs` | `run_settlement/work_unit/tests.rs` | completed・continue・auto_leaf_budget・limit・continuation_cap・plan_issue・question・retry・failed・merge_repair_failed で読み、harness_error・取り消し・cluster_jobs で読まない。状態だけを戻す run の trigger と outcome を残す（旧い試験から引き継いだ確かめ） |
| `decision_fail_closed_hold_clears_failure_side_effects` | 同上 | failed・plan_issue・question・integration repair の失敗・completed・retry・continue・auto_leaf_budget を止めると `blocked(decision)`・`Continue{advance}`、`failure`・`integration_repair_exhaust`・`auto_replan` が空、待つ unit の ready と失敗の伝播が空。止めた後の `settle_work_unit_failure` は何もしない。質問の文が outcome に残る。retry・continue の数えは run の前の値、auto_leaf_budget は理由と数えを残す |
| `decision_fail_closed_yielded_leaf_records_decisions` | `dispatcher/tests/tree_decisions.rs` | leaf が yield の run で self の決定を書くと、決定が `task_ops::decision::open_rows` に出て leaf は `blocked(decision)`（run 1 本、遷移は dispatch→decision）。continuation・質問にならず integrate と root は進まない。答えると leaf が最初から走り（continuation の文脈が無い、前置きに答え 1 件）、木が done、replay が一致する |
| `decision_fail_closed_no_progress_leaf_records_decisions` | 同上 | 2 回目の run が進捗なしの yield（`no_progress_limit = 1`、`max_replans = 0`）で self の決定を書くと、`blocked(limit)` と質問にならず、決定が受信箱に出て leaf は `blocked(decision)`（遷移は dispatch→continue→dispatch→decision）。答えると木が done、replay が一致する |
| `decision_fail_closed_auto_leaf_request_recorded_with_and_without_worker_decisions` | `dispatcher/tests/tree_gate.rs` | 修正の前も後も通る守り。auto-leaf の上限を超えた run に worker の決定があってもなくても、`auto_leaf_budget:…` の決定が 1 件 open で記録され、leaf は `blocked(decision)`、最後の遷移の理由は `auto_leaf_budget`、replay が一致する |

試験の adapter の変更: `DecisionAdapter` に key ごとの台本（`scripting`・`ScriptedRun`。yield の checkpoint と `decisions`）を
足した。台本が無い key は今までどおり `declare` を使うので、既存の試験の挙動は変わらない。`AutoLeafAdapter` に最後の台本の run
だけ `decisions` を書く `deciding_on_last_run` を足した（指定しなければ今までどおり）。

## 置き換えた・変えた既存の試験（期待値は弱めていない）

- `worker_decisions_hold_a_completed_leaf_only` を消した。「done の遷移だけで読む」旧い契約を確かめていたため（ADR の付記で廃止）。
  確かめのうち、done の止め（待つ unit を ready にしない・`WorkerDone` を `advance` に替える・outcome の文）は
  `decision_fail_closed_hold_clears_failure_side_effects` に、状態だけを戻す run で読まないことと trigger・outcome・`ready` を
  残すことは `decision_fail_closed_reads_every_end_but_reset_and_cluster_jobs` に移した。
- `continuation_disabled_retries_the_unit_unless_the_auto_leaf_limit_stops_it` の `assert!(!s.reads_worker_decisions())` を
  `assert!(s.reads_worker_decisions())` にした。auto-leaf の上限で止めた run も worker の決定を読む新しい規則のため。
  `assert!(s.auto_leaf_request.is_some())` は残した。auto-leaf の要求が記録されることは dispatcher の守りの試験が確かめる。
- 他の既存の試験は変えていない（`rejected_worker_decision_reaches_the_inbox_and_holds_the_leaf` の遷移 `["dispatch", "decision"]` を含む）。

## before / after

- before（commit「decision-failclosed settle-leaf: decision_fail_closed_* の試験を先に書き、修正前に落ちる before.log を取る」の
  production のまま）: `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/decision_fail_closed_reads_every/) |
  test(/decision_fail_closed_hold_clears/) | test(/decision_fail_closed_yielded_leaf/) | test(/decision_fail_closed_no_progress_leaf/)'`
  → [settle-leaf/before.log](settle-leaf/before.log)、exit=100、4 tests run: 0 passed, 4 failed。コンパイル error は無い。
  - reads_every: `assertion left == right failed: continue`（continue の遷移で読まなかった）
  - hold_clears: `assertion failed: s.failure.is_none() && s.integration_repair_exhaust.is_none()`
  - yielded: `assert!(blocked_by_decision(&units, "a"))` が落ちた（a は continuation の run で `Done`、integrate-s1 も `Done`）
  - no_progress: 同じ assertion が落ちた（a は `blocked(Limit)`、b は `Ready` のまま）
  - log の行番号は試験を書いた時点の file の行で、その commit の `cargo fmt` で少しずれる。
- after（提出時の HEAD）: 同じ command を `NEXTEST_STATUS_LEVEL=pass` で → [settle-leaf/after.log](settle-leaf/after.log)、exit=0、
  4 tests run: 4 passed（4 件の PASS の行がある）。

## commit（件名）

- decision-failclosed settle-leaf: decision_fail_closed_* の試験を先に書き、修正前に落ちる before.log を取る
- decision-failclosed settle-leaf: WU の run の decisions を D1 の規則で読み、self の要求なら失敗の処理より先に決定で止める
- decision-failclosed settle-leaf: ADR に付記 2026-11-21（読む規則と止め方の順）を足す
- decision-failclosed settle-leaf: 決定の読み取りを Dispatcher::work_unit_decisions に移し、finish_worker_result_with を大きくしない
- decision-failclosed settle-leaf: 決定の読み取りを finish_worker_result_with の中に戻し、行数を base 以下にする
- decision-failclosed settle-leaf: settle-leaf.md に変えた関数・試験・before/after・LoC を書く

4 件目は repo health の `size.responsibility-growth` が `finish_worker_result_with`（関数 774 行、閾値 200）の増加で落ちたので
helper に移したが、`Dispatcher` の method 数と dispatcher module の行の合計が増えて別の 2 件で落ちた。5 件目で helper を消し、
読み取りを関数の中に base 以下の行数で戻した（auto-leaf の要求の数えは `hold_for_worker_decisions` に入れた）。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/rejected_worker_decision/) | test(/decision_fail_closed/)'`
  → exit 0、10 tests run: 10 passed（この unit の 5 件と、fix の 3 件、`rejected_worker_decision*` 2 件）
- `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/decision/) | test(/auto_leaf/) | test(/run_settlement::/) |
  test(/tree/) | test(/continuation/) | test(/work_unit/)'` → exit 0、447 tests run: 447 passed
- `bash scripts/dev/test-affected.sh` → exit 0。repo-health: verdict=pass（fail_grade=0、new=0、worsened=0）、nextest 3947 passed・
  11 skipped（crates: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）、doc test 通過、script bundle の SKIP 0 件。
  末尾に scratch の古い trash dir を消せない `rm: Permission denied` が 3 行出たが、exit code は 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（外部の crate `deno_core` の deprecated の warning 1 件だけ）
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- 範囲の確認（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変えた path は `run_settlement/work_unit.rs`・`run_settlement/work_unit/tests.rs`・
  `dispatcher/worker_finish.rs`・`dispatcher/tests/tree_decisions.rs`・`dispatcher/tests/tree_gate.rs`・ADR・この dir の
  settle-leaf.md と settle-leaf/ の 2 つの log だけ。

## LoC の差分（`git diff --numstat "$CELERIS_WU_BASE" HEAD`、追加/削除）

- production: `crates/task-dispatch/src/run_settlement/work_unit.rs` +49/−14、`crates/task-dispatch/src/dispatcher/worker_finish.rs`
  +25/−27（計 +74/−41、純増 33 行。説明の comment を含む）
- test: `crates/task-dispatch/src/run_settlement/work_unit/tests.rs` +285/−41、`crates/task-dispatch/src/dispatcher/tests/tree_decisions.rs`
  +210/−8、`crates/task-dispatch/src/dispatcher/tests/tree_gate.rs` +88/−6（計 +583/−55）
- 文書: ADR +47 行、この md と 2 つの log。この task は行数の純減を目的にしていない。

## 未解決

- WU のクラスタ job の wait の run（遷移 `cluster_jobs`）は読まない（ADR の付記の理由）。読むには `cluster_job_wait.rs`・
  `task_ops::decision` の変更が要り、この段の範囲の外。close の未解決に入れる。
- atomic の節点の run は unit settle-atomic が同じ規則で直す（ADR の付記の「atomic の規則」）。
- unit settle-failure の done_when の「retries は数えたまま」: 止めた run の遷移は `decision` で、WU の行の retries は run の前の
  値のまま（上の「数えの扱い」）。settle-failure はこれを「記録済みの retries を下げない」と読む必要がある。retries に +1 を残すには
  replay（task-ops）を変えるか遷移の event を足す必要があり、設計 brief の不変条件に反する。
- 本番での確認は未実施（release の昇格の後に unit close と親の (d) で行う）。

## 提案

- 無し。
