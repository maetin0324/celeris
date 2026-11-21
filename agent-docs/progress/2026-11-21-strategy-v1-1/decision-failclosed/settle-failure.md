---
title: 戦略層 v1 子 1 decision-failclosed / settle-failure — 失敗・plan_issue・質問・replan の使い切りで終わった leaf の run でも決定が先に止めることを試験で確かめる
tasks: [01M7XSGW4FH258N1K3AMW76Y6F]
status: done
updated: 2026-11-21
---
# decision-failclosed / settle-failure — 失敗の側の経路で決定が先に止めることを dispatcher の試験で確かめる

完了日: 2026-11-21

## 結論

- `dispatcher/tests/tree_decisions.rs` に `decision_fail_closed_*` の試験 4 件を足した。WU の check の不合格・`plan_issue`・
  replan を使い切った木の失敗・質問のどれで終わった leaf の run でも、`needed_before: self` の決定が受信箱に出て、leaf は
  `blocked(decision)` 1 つで止まる。
- settle-leaf の production の変更を作業ツリーで一時的に戻すと、4 件とも assertion で落ちた（before.log、exit=100、
  4 tests run: 0 passed, 4 failed。コンパイル error は無い）。戻した後は 4 件とも通る（after.log、exit=0、4 passed）。
- 直した所: 無し。settle-leaf の修正（`run_settlement/work_unit.rs`・`dispatcher/worker_finish.rs`）で 4 経路とも
  fail-closed になっていた。production の file は変えていない。
- 数えと checks の不合格の記録は変えていない。試行・replan・予算の数えの code に差分は無い。既存の試験の期待値も変えていない。

## 実装前 brief

1. 根本原因: settle-leaf の修正の前は、WU の run の `decisions` を遷移 `completed` だけで読み、WU の失敗の処理（再試行・
   自動 replan・質問・replan の使い切りの決定）を決定より先に走らせていた。settle-leaf がこれを直した。この unit は
   失敗の側の 4 経路を dispatcher の経路全体で確かめる。
2. 責務 owner: `run_settlement/work_unit.rs`（`reads_worker_decisions`・`hold_for_worker_decisions`）と
   `dispatcher/worker_finish.rs`（`finish_worker_result_with` の読む順）。欠けが見つかればここを直す予定だった（欠けは無かった）。
3. 既存の再利用先: `tree_decisions.rs` の `DecisionAdapter`・`ScriptedRun`・`assert_leaf_held_by_worker_decision`・
   `answer_and_finish`・`run_until_quiet`。check の不合格の組み立ては `work_unit_check_failures.rs`（`WorkUnitChecksFailed`・
   `previous_check_failures`）、replan の使い切りの組み立ては `human_gates.rs` の
   `a_tree_node_leaf_failure_after_replans_are_exhausted_raises_the_limit_decision`（`max_retries = 0`・`max_replans = 0`・
   check `false`）に合わせた。
4. 選択肢: (a) 4 経路を dispatcher の試験で通す（採用。settlement の関数の試験では `QuestionRaised`・`AutoReplanRequested`・
   `limit:max_replans` の決定・`WorkUnitChecksFailed`・再試行の run の抑止を確かめられない）。(b) settlement の関数の
   試験を足す（settle-leaf の `decision_fail_closed_hold_clears_failure_side_effects` が既に持つので採らない）。
   質問で終わる run は、`ScriptedRun` に欄を足さず `DecisionAdapter::asking` で表した。同じ段で並行する settle-atomic が
   `ScriptedRun` を組み立てても、merge の後にコンパイルが壊れないようにするため。
5. 新しい状態・分岐・fallback・port: 無し（試験の adapter の分岐 1 つだけ。下の「試験の adapter の変更」）。
6. 撤去対象: 無し。
7. 不変条件: I1（読む）・I2（止める）・I7（leaf を止めるのは決定 1 か所）、数え（試行・replan・予算）と checks の
   不合格の記録を変えない。確かめる試験は下の表。各試験の doc comment に、守る不変条件と既存の試験で検出できない理由を書いた。

## 足した試験（`crates/task-dispatch/src/dispatcher/tests/tree_decisions.rs`）

| 試験 | 組み立て | 確かめること |
|---|---|---|
| `decision_fail_closed_check_failed_leaf_records_decisions` | leaf a の check は印の file が無いと落ちる。`max_retries = 2`。run 1 は `summary` だけ、run 2 は `summary` と self の決定 | run 1 は check の不合格で再試行（遷移 retry）。run 2 は決定が open で受信箱に出て a は `blocked(decision)`（遷移は dispatch→retry→dispatch→decision）。run 2 の outcome は決定の文。10 tick 回しても再試行の run は起きない（a の run は 2 本のまま）。retries は 1 のまま。`WorkUnitChecksFailed` は 2 run とも残り、2 件目は止めた run の分。`AutoReplanRequested` は無い。答えると a が走り直し、その run の `previous_check_failures` に止めた run の不合格が載る。check が通ると a は done（run 3 本、retries 1）、木が done、replay が一致する |
| `decision_fail_closed_plan_issue_leaf_records_decisions_before_replan` | run 1 は `yield` の `plan_issue` と self の決定。`max_replans` は既定値（3。replan の余地がある） | `AutoReplanRequested` が 0 件、planner の run は 1 本、計画は版 1 だけ。決定は `vault` の 1 件で、a は `blocked(decision)`（遷移 dispatch→decision）。outcome は決定の文で、申告の文を含む。答えると a が最初から走って木が done になり、その後も `AutoReplanRequested` は 0 件、計画は版 1 だけ |
| `decision_fail_closed_replan_exhausted_leaf_records_one_decision` | `max_replans = 0`・`max_retries = 0`・check `false`。run 1 は self の決定 | 決定の行は `vault` だけで、`limit:max_replans` は無い。outcome に `max_replans` が無い。`WorkUnitChecksFailed` は 1 件残る。答えた後の run は決定を書かずにまた失敗し、そこで `limit:max_replans` の決定が出る（受信箱にはそれだけ。root は `ready` で failed にならない。質問は無い）。replay が一致する |
| `decision_fail_closed_question_leaf_records_decisions` | run 1 は `result.json` に `question` と self の決定を併記し、質問で終わる | `QuestionRaised` と承認の行は無い。決定 1 件が受信箱に出て、a は `blocked(decision)`（遷移 dispatch→decision）。outcome は `decision: WorkUnit a は人の決定を待ちます（決定の要求 1 件。ADR-0079 D7）。止めた質問: which vault holds the deploy key?`。答えると a が最初から走って木が done、replay が一致する |

足した helper: `finished_outcome`（run の `WorkerFinished.outcome`）、`last_run_of_a`、`decision_keys`、`auto_replans`。

### 試験の adapter の変更

`DecisionAdapter` に `ask`（key → 質問の文）と builder の `asking(key, question)` を足した。`ask` に key があるとき、その key の
最初の run は `result.json` に `{"question": …}` と（`declare`・台本の）`decisions` を書いて `Terminal::Question` で終わる。
`ask` に key が無い run は今までどおり動くので、既存の試験の挙動は変わらない。

### integration repair の打ち切りより先に読むことの dispatcher の試験は足さない

`finish_worker_result_with` は決定を読んで `hold_for_worker_decisions` を呼んだ後に `settled.integration_repair_exhaust` を見る。
hold はこの欄を `None` にするので、打ち切りの処理は走らない。hold がこの欄を空にし、その後の `settle_work_unit_failure` が
何もしないことは、settle-leaf の `decision_fail_closed_hold_clears_failure_side_effects`（`run_settlement/work_unit/tests.rs`）が
integration repair の失敗の遷移で確かめる。dispatcher の試験にすると、integration repair の unit と repair の前の HEAD を持つ
git の作業場所を組み立てることになるが、確かめるのは同じ欄なので、Objective の指示どおり足さない。

## before / after

- before: settle-leaf の production の変更を作業ツリーで一時的に戻した。commit「decision-failclosed settle-leaf:
  decision_fail_closed_* の試験を先に書き、修正前に落ちる before.log を取る」（修正の直前で、production は修正前のまま）の
  版を `git show <その commit>:<path> > <path>` で次の 3 file に書いた。
  - `crates/task-dispatch/src/run_settlement/work_unit.rs`（production）
  - `crates/task-dispatch/src/dispatcher/worker_finish.rs`（production）
  - `crates/task-dispatch/src/run_settlement/work_unit/tests.rs`（試験。今の版は新しい署名
    `hold_for_worker_decisions(s, wu: &WorkUnitRow, count)` を呼ぶ。production だけを戻すと lib の試験 binary がコンパイル
    できないので、同じ commit の版に戻した。この 3 file の差分が settle-leaf の code の変更の全部だった）

  `git stash` は使っていない。その状態で
  `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/decision_fail_closed_check_failed_leaf/) | test(/decision_fail_closed_plan_issue_leaf/) | test(/decision_fail_closed_replan_exhausted_leaf/) | test(/decision_fail_closed_question_leaf/)'`
  を走らせた → [settle-failure/before.log](settle-failure/before.log)、exit=100、4 tests run: 0 passed, 4 failed。
  コンパイル error は無い。
  - check_failed: `assert!(blocked_by_decision(&units, "a"))`（`assert_leaf_held_by_worker_decision` の中）が落ちた。a は
    `Failed`・runs 3・retries 2（決定を読まず再試行の run に進み、失敗した）。
  - plan_issue: `assertion left == right failed: no AutoReplanRequested`（left 2、right 0）。
  - replan_exhausted: `blocked_by_decision` の assertion が落ちた。a は `Failed`・runs 1。
  - question: `blocked_by_decision` の assertion が落ちた。a は `blocked(Question)`・runs 1。
- 戻し: `git checkout HEAD -- <上の 3 path>` で 3 file を戻した。`git diff HEAD --stat -- crates/` は空。
- after（提出時の HEAD）: 同じ command を `NEXTEST_STATUS_LEVEL=pass` で走らせた → [settle-failure/after.log](settle-failure/after.log)、
  exit=0、4 tests run: 4 passed（4 件の PASS の行がある）。
- before の skip の数（3302）が after（3301）より 1 多いのは、戻した `work_unit/tests.rs` に旧い試験
  `worker_decisions_hold_a_completed_leaf_only`（settle-leaf が消した試験）が入っていたため。

## 数えと記録の扱い（変えていないことの確かめ）

- retries: 試験 0 では、run 1 の check の不合格で数えた retries 1 が、止めた後も答えの後も 1 のまま残る（下げない）。止めた
  run 2 の不合格は retries に足されない。止めた遷移は `decision` で、replay は `decision` の遷移で数えを動かさないため
  （settle-leaf の「数えの扱い」と同じ）。Acceptance の「retries は数えたまま」は「記録済みの retries を下げない」として確かめた。
- checks の不合格の記録: 止めた run の分も `WorkUnitChecksFailed` が残り、答えの後の run の `previous_check_failures` に渡る。
- replan の上限: 試験 2 では、答えた後の失敗で今までどおり `limit:max_replans` の決定が出る。試験 1 では、止めた run で
  `AutoReplanRequested`（replan の数え）を残さない。
- production の code に差分は無い（`git diff --numstat "$CELERIS_WU_BASE" HEAD -- crates/` は試験の file 1 件だけ）。

## commit（件名）

- decision-failclosed settle-failure: check の不合格・plan_issue・replan の使い切り・質問で終わる leaf の run の decision_fail_closed_* の試験 4 件を足す
- decision-failclosed settle-failure: settle-failure.md に試験・before/after・一時的な戻し・LoC を書く

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- after の command（上）→ exit 0、4 tests run: 4 passed
- `cargo nextest run -p task-core -p task-dispatch --no-tests=fail -E 'test(/rejected_worker_decision/) | test(/decision_fail_closed/)'`
  → exit 0、14 tests run: 14 passed（この unit の 4 件、settle-leaf の 5 件、fix の 3 件、`rejected_worker_decision*` の 2 件）
- `bash scripts/dev/test-affected.sh` → exit 0。repo-health: verdict=pass（fail_grade=0、new=0、worsened=0）。nextest は
  3951 passed・11 skipped（crates: celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch）。doc test は通過。script bundle の SKIP は 0 件。
- `cargo clippy --workspace -- -D warnings` → exit 0（外部の crate `deno_core` の deprecated の warning 1 件だけ）
- `cargo fmt --all -- --check` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0
- 範囲の確認（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変えた path は `crates/task-dispatch/src/dispatcher/tests/tree_decisions.rs`、
  この md、`settle-failure/` の 2 つの log だけ。

## LoC の差分（`git diff --numstat "$CELERIS_WU_BASE" HEAD`、追加/削除）

- production: 0/0（変更無し）
- test: `crates/task-dispatch/src/dispatcher/tests/tree_decisions.rs` +319/−4（試験 4 件・helper 4 個・`DecisionAdapter::asking`）
- 文書: この md と 2 つの log。この task は行数の純減を目的にしていない。

## 未解決

- 止めた run の check の不合格は retries に足されない（上の「数えと記録の扱い」）。足すには replay（task-ops）を変えるか
  遷移の event を足す必要があり、settle-leaf の設計 brief の不変条件（足す event は `DecisionRequested` と進行の行だけ、
  task-ops に差分が無い）に反する。この unit の範囲では変えない。答えの後の run で同じ失敗が起きれば、そこで再試行と上限が効く。
- atomic の節点の run は unit settle-atomic が扱う。WU のクラスタ job の wait の run（`cluster_jobs`）は読まない（ADR の付記）。
- 本番での確認は未実施（release の昇格の後に unit close と親の (d) で行う）。

## 提案

- 無し。
