---
title: 戦略層 v1 子 1 decision-failclosed / settle-atomic — atomic の節点の run の decisions を D1 の規則で読み、self の要求なら決定で止める
tasks: [01M7XSGW4FH258N1K3AMW76Y6F]
status: done
updated: 2026-11-21
---
# decision-failclosed / settle-atomic — atomic の節点の run の `decisions` を D1 の規則で読む

完了日: 2026-11-21

## 結論

- atomic の節点の run の `decisions` は、終わり方が分かり（`Some`）harness_error・取り消し・`Waiting`（クラスタ job の
  wait）でない全ての run で読む（`task_reads_worker_decisions(end)`。trigger は見ない）。旧い規則は done の run だけだった。
- `needed_before: self` の要求なら `hold_task_for_worker_decisions` が `WorkerDone`・`WorkerQuestion`・`WorkerError` を
  `Continue{advance}` に替える。`Continue{continue}`・`Continue{children_wait}` は trigger を残す。どちらでも次の run は
  `dispatch_admission` の `decision_self_hold` が答えまで止める（止めの正本は decisions の行 1 つ）。
- 3 件の試験を先に書き、修正前は 3 件とも assertion で落ちた（before.log、exit=100）。修正後は通る（after.log、exit=0）。
- 旧い契約の試験 `worker_decisions_hold_an_atomic_run_only_when_done` を置き換え、理由を ADR
  2026-11-21-worker-decisions-fail-closed の「付記 2026-11-21（atomic の規則の実装）」に書いた。

## 実装前 brief

1. 根本原因: `task_reads_worker_decisions` が `WorkerDone` かつ `RunEnd::Completed` のときだけ true だった。atomic の節点の
   run が yield・予算切れ・質問・失敗で終わると、`result.json` の `decisions` は決定の行も進行の行も残さずに捨てられ、
   節点は continuation・再試行・質問に進んだ（unit survey の `continuable-end`・`failed-end` の atomic の分）。
2. 責務 owner: `run_settlement/task_run.rs`（`task_reads_worker_decisions`・`hold_task_for_worker_decisions`）と、
   その呼び出し側の `dispatcher/worker_finish.rs`（`finish_worker_result_with` の atomic の決定の読み取り）。
3. 既存の再利用先: `Dispatcher::worker_decisions`（受け付けと `rejected` のまとめ）、`Dispatcher::decision_self_hold`
   （`dispatch_admission` の止め）、答えの解放（`task_ops::decision::answer`）、試験は `tree_decisions.rs` の
   `DecisionAdapter`（`scripting`・`declaring`）と helper（`child_named`・`decision`・`finished_outcome`・`no_questions`）。
4. 選択肢: ADR の付記の「atomic の規則」をそのまま採った。continuation と子 task 待ちも `Continue{advance}` にする案は
   採らない（continuation の数え・checkpoint の続き・子の待ちを壊す。付記の規則が残すと決めている）。止めた
   continuation の outcome を `decision: …` に替える案も採らない（run の一覧の分類〈`classify_outcome`〉が
   continuation から error に変わる）。文の末尾に決定を待つことを足す案を採った。
5. 新しい状態・分岐・fallback・port: 無し。`hold_task_for_worker_decisions` に trigger による分岐（替える 3 種と残す
   trigger）と、質問の文を残す分岐を足した（どちらも付記の止め方）。通知・Manager・DB 表・失敗の種類・決定の kind・
   fallback は足していない。
6. 撤去対象: `task_reads_worker_decisions` の trigger の引数と「done の run だけ」の規則、`hold_task_for_worker_decisions`
   の「常に固定の文」の挙動、旧い契約の試験 `worker_decisions_hold_an_atomic_run_only_when_done`、`worker_finish.rs` の
   旧い説明の comment。
7. 不変条件: I1（読む）・I2（止める。答えまで done にならず、次の run が起きない）・I3（決定の無い run は変わらない。
   `worker_decisions` が `None` なら何も変えない）・数え（continuation の遷移と checkpoint を残す。予算の記録と木の
   token 予算の止めは変えない）。試験は下の表。replay の一致は `assert_replay_is_clean` で確かめる。

## 変えた関数

- `task_reads_worker_decisions`（`crates/task-dispatch/src/run_settlement/task_run.rs`）: 引数を `end: Option<RunEnd>`
  だけにし、`end` が `Some` で `HarnessError`・`Cancelled`・`Waiting` でなければ true にした（旧: `WorkerDone` かつ
  `Completed`）。
- `hold_task_for_worker_decisions`（同じ file）: 引数を `(trigger, outcome, count)` にした。
  - `WorkerDone`・`WorkerQuestion`・`WorkerError` → `Continue{advance}`、outcome は
    `decision: 人の決定を待ちます（決定の要求 N 件。ADR-0079 D7）`。`WorkerQuestion` なら末尾に `。止めた質問: <質問の文>`
    （`question: ` の接頭辞を除いた文。worker の質問・進捗なし・固い上限の質問）。
  - それ以外（`Continue{continue}`・`Continue{children_wait}`）→ trigger を残し、outcome は元の文の末尾に
    `。人の決定を待ちます（決定の要求 N 件。ADR-0079 D7）` を足したもの（`continue: ` の接頭辞が残る）。
- `Dispatcher::finish_worker_result_with`（`crates/task-dispatch/src/dispatcher/worker_finish.rs`）: 新しい引数で呼び、
  `worker_decisions` を直接受けて `self_hold` のときだけ trigger と outcome を替える形にした。関数の行数は 769 → 766
  （`$CELERIS_WU_BASE` 比。repo health の `size.responsibility-growth` は worsened=0）。

## 数えの扱い

- continuation（`Continue{continue}`）を止めた run は、遷移 `continue` と `CheckpointSaved` をそのまま残す。continuation の
  数え・進捗なしの窓は変わらず、答えの後の run は checkpoint の続き（continuation の文脈あり）として走る。
- 失敗（`WorkerError`）を `Continue{advance}` に替えた run は試行（`attempts`）を消費しない（`Continue` の遷移は試行を
  据え置く）。答えの後の run がまた失敗すれば、そこで数える。記録済みの数えは下げていない。
- 予算と usage の記録、木の token 予算の止め（`stop_at_token_budget`）は決定の読み取りの後にあり、変えていない。止めた
  trigger（`Continue{..}`）も「次の run を起こす」trigger なので、上限に達していれば今までどおり止める。

## 足した試験（守る不変条件と、既存の試験で検出できない理由は各試験のコメントに書いた）

| 試験 | 場所 | 確かめること |
|---|---|---|
| `decision_fail_closed_task_reads_every_end_but_reset_and_waiting` | `run_settlement/task_run/tests.rs` | Completed・Yielded・BudgetExhausted（turns・wall_clock）・Question・Failed（retryable の true・false）で読み、harness_error（infra・lease_expired）・取り消し・Waiting・終わり方不明（`None`）で読まない。止め方は done・失敗（retryable の true・false）を `Continue{advance}` と固定の文、質問（worker の質問・進捗なしの質問）を `Continue{advance}` と質問の文付きの文にし、`Continue{continue}`・`Continue{children_wait}` は trigger を残して文の末尾に足す |
| `decision_fail_closed_yielded_atomic_node_records_decisions` | `dispatcher/tests/tree_decisions.rs` | 木の子の atomic の節点 k が yield で終わり self の決定を書くと、決定（open・origin worker・`needed_before = [self]`・止めた run）が受信箱に出て、k は run 1 本で `ready` のまま（10 tick 回しても次の run が起きない）、unit k と root は done にならず、質問も出ない。遷移 `continue` と checkpoint が残り、outcome は `continue: …。人の決定を待ちます（決定の要求 1 件。ADR-0079 D7）`。答えると k の次の run が continuation として走り（前置きの `answers` に答え）、k・unit k・root が done、replay が一致する |
| `decision_fail_closed_failed_atomic_node_records_decisions` | 同上 | k が self の決定を書いて失敗（`Terminal::Error{retryable: true}`）で終わると、同じく決定が出て k は止まる。遷移は `advance`、試行は 0 のまま、outcome は `decision: 人の決定を待ちます（決定の要求 1 件。ADR-0079 D7）`。答えると k の次の run が最初から走り（continuation の文脈なし）、木が done、replay が一致する |

試験の adapter の変更: `DecisionAdapter` に key ごとの最初の run を失敗で終える `failing`（`fail` の map）を足した。
指定しない key の挙動は変わらない。

## 置き換えた既存の試験（期待値は弱めていない）

- `worker_decisions_hold_an_atomic_run_only_when_done` を消した。「done の run だけで読む」「止めた trigger は常に
  `Continue{advance}`、outcome は固定の文」の旧い契約を確かめていたため（ADR の付記で廃止）。確かめのうち、done の run を
  読むこと・終わり方不明の run を読まないこと・done の止め（`Continue{advance}` と固定の文）は
  `decision_fail_closed_task_reads_every_end_but_reset_and_waiting` に引き継いだ。「質問の trigger の後で読まない」の
  確かめは、trigger を見ない新しい規則で廃止した。
- 子 task 待ちの trigger（`Continue{children_wait}`）が残ることは同じ試験が確かめる。
- 他の既存の試験は変えていない（`rejected_worker_decision_reaches_the_inbox_and_holds_the_leaf`・
  `decision_fail_closed_non_array_decisions_hold_the_atomic_node`・`worker_declared_decisions` を含む）。

## before / after

- before（commit「decision-failclosed settle-atomic: decision_fail_closed_* の試験を先に書き、修正前に落ちる before.log を
  取る」）: この commit は 2 つの関数の引数だけを新しい形にし、挙動は base と同じにした（atomic の呼び出しの位置では
  `Completed` の run は必ず `WorkerDone` なので `end == Some(Completed)` は旧い条件と同じ。止め方は引数を使わず固定の
  文を返す）。`cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/decision_fail_closed_task_reads_every/) |
  test(/decision_fail_closed_yielded_atomic/) | test(/decision_fail_closed_failed_atomic/)'` →
  [settle-atomic/before.log](settle-atomic/before.log)、exit=100、3 tests run: 0 passed, 3 failed。コンパイル error は無い。
  - task_reads_every: `reads Yielded`（yield の run で読まなかった）
  - yielded_atomic・failed_atomic: `assertion left == right failed: no next run before the answer`（left: 2, right: 1。
    決定が無く、k は continuation・再試行の 2 本目の run に進んだ）
- after（提出時の HEAD）: 同じ command を `NEXTEST_STATUS_LEVEL=pass` で → [settle-atomic/after.log](settle-atomic/after.log)、
  exit=0、3 tests run: 3 passed（3 件の PASS の行がある）。

## commit（件名）

- decision-failclosed settle-atomic: decision_fail_closed_* の試験を先に書き、修正前に落ちる before.log を取る
- decision-failclosed settle-atomic: 修正前の before.log を置く
- decision-failclosed settle-atomic: atomic の節点の run の decisions を D1 の規則で読み、self の要求なら決定で止める
- decision-failclosed settle-atomic: ADR に付記 2026-11-21（atomic の規則の実装）と after.log を足す
- decision-failclosed settle-atomic: settle-atomic.md に変えた関数・試験・before/after・LoC を書く

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/rejected_worker_decision/) | test(/decision_fail_closed/)'`
  → exit 0、17 tests run: 17 passed（この unit の 3 件と、fix・settle-leaf・settle-failure の試験、`rejected_worker_decision*`）
- `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/decision/) | test(/auto_leaf/) | test(/run_settlement::/) |
  test(/tree/) | test(/continuation/) | test(/work_unit/) | test(/atomic/) | test(/children/) | test(/cluster_job/) | test(/yield/)'`
  → exit 0、540 tests run: 540 passed
- `python3 -I agent-docs/progress/2026-11-21-strategy-v1-1/decision-failclosed/reproduce.py verify` → exit 0
- `bash scripts/dev/test-affected.sh` → exit 0。repo-health: verdict=pass（fail_grade=0、new=0、worsened=0）、nextest 3953 passed・
  11 skipped（crates: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）、doc test 通過、script bundle の SKIP 0 件。
  末尾に scratch の古い trash dir を消せない `rm: Permission denied` が 3 行出たが、exit code は 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（外部の crate `deno_core` の deprecated の warning 1 件だけ）
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` → exit 0
- 範囲の確認（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変えた path は `run_settlement/task_run.rs`・`run_settlement/task_run/tests.rs`・
  `dispatcher/worker_finish.rs`・`dispatcher/tests/tree_decisions.rs`・ADR・この dir の settle-atomic.md と settle-atomic/ の
  2 つの log だけ。

## LoC の差分（`git diff --numstat "$CELERIS_WU_BASE" HEAD`、追加/削除）

- production: `crates/task-dispatch/src/run_settlement/task_run.rs` +40/−12、`crates/task-dispatch/src/dispatcher/worker_finish.rs`
  +9/−12（計 +49/−24、純増 25 行。説明の comment を含む）
- test: `crates/task-dispatch/src/run_settlement/task_run/tests.rs` +89/−22、`crates/task-dispatch/src/dispatcher/tests/tree_decisions.rs`
  +217/−0（計 +306/−22）
- 文書: ADR +34 行、この md と 2 つの log。この task は行数の純減を目的にしていない。

## 未解決

- atomic の節点のクラスタ job の wait の run（`Waiting`）は読まない（ADR の付記の理由。要求は job の後の run が出し直す）。
  WU の `cluster_jobs` と同じく close の未解決に入れる。
- 終わり方不明（`None`）の run は読まない。infra 再試行を使い切った run（`WorkerError{retryable:false}`、`None`）が
  `decisions` を書いていた場合も読まないが、adapter が結果を返していないので `result.json` が書かれたかは未確認の場面。
- 本番での確認は未実施（release の昇格の後に unit close と親の (d) で行う）。

## 提案

- 無し。
