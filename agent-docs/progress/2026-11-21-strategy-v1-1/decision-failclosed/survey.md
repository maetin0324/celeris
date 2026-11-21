---
title: 決定の要求が人に届かないまま run が成功する経路の調査（decision-failclosed / survey）
tasks: [01M7XSGW4FH258N1K3AMW76Y6F]
status: done
updated: 2026-11-21
---
# 決定の要求が人に届かないまま run が成功する経路の調査（survey）

完了日: 2026-11-21

## 結論

- 直す経路は 3 つある。owner の中（`crates/task-core/src/decision.rs`・`crates/task-dispatch/src/dispatcher/tree_units.rs`）で直せるのは
  `decisions-not-array` だけで、次の unit `fix` がこれを直す。
- `continuable-end`（yield・予算切れ・クラスタ job の wait）と `failed-end`（checks の不合格を含む失敗）は、直す owner が
  `run_settlement`・`worker_finish.rs` にしかない。unit `fix` の対象のパスの外なので、**unit `fix` は replan を求める**
  （直し方の案は ADR の D3）。
- 残りの経路は fail-closed（人に届くか、仕事が人の答えまで止まる）か、問題が無い。
- 設計判断は [ADR 2026-11-21-worker-decisions-fail-closed](../../../adr/2026-11-21-worker-decisions-fail-closed.md) に書いた。コードは変えていない。

## 経路の表

file:line は提出時の HEAD（基準は `$CELERIS_WU_BASE`。この unit は crates/ を変えていないので base と同じ行）。

| id | 経路 | file:line〈HEAD〉 | 判定 | 理由 |
|---|---|---|---|---|
| planner-plan | planner の計画（`execution-plan.json`）の `decisions` の形・key の重複・`needed_before` の誤り | `crates/task-core/src/execution_plan/validation.rs:1739`（`validate_shape`）・`:1747`（`needed_before` の指す先）、`crates/task-dispatch/src/dispatcher/planner_flow.rs:81`（`parse_planner_output`）・`:283`（読めない・不正な計画 → `give_up_or_retry_planner`）・`:338`（再試行）・`:359`（`plan_invalid_decision`）、`crates/task-dispatch/src/dispatcher/tree_units.rs:531`（`decision_self_hold`） | fail-closed | 誤りは計画の検証の誤り（`InvalidDecision`・`DuplicateDecisionKey`・`UnknownNeededBefore`）になり、計画は採用されない。planner をもう 1 回走らせ、2 回とも不正なら `kind: plan_invalid`（`needed_before: [self]`）の決定を出し、答えるまで節点の run を起こさない。計画の `decisions` が配列でなければ計画の JSON の読み込みが失敗し、同じ経路に入る。integrate の unit は計画の unit の key に無いので、指せば `UnknownNeededBefore` になる。 |
| decisions-not-array | worker の `result.json` の `decisions` が object・文字列・数（配列でない） | `crates/task-core/src/decision.rs:875`（`worker_decisions_from_json` の `as_array()`）、`crates/task-dispatch/src/dispatcher/tree_units.rs:379`〜`:381`（`raw.is_empty()` で `Ok(None)`） | to-fix | `worker_decisions_from_json` が配列でない値を空にし、`Dispatcher::worker_decisions` は `None` を返す。決定の行も、`worker の決定の要求を記録できませんでした` の進行の行も残らない。leaf は done になり、run は成功のまま進む。worker の前置きは「受け付けられない要求は 1 件の決定にまとめて人に出され、この仕事は止まる」と約束している（`crates/task-worker/src/preamble.rs:43` の `decision_requests_section`）が、この形の誤りには当たらない。 |
| result-json-broken | `result.json` が JSON として壊れている、または `summary`・`question`・`yield` の型が違う | `crates/task-worker/src/result_report.rs:218`・`:227`（`not_valid_json` → `Terminal::Error { retryable: true }`）、`crates/task-dispatch/src/execution_scheduler.rs:147`（`RunEnd::Failed` → `failed`） | fail-closed | run は `error(retryable=true)` で失敗になり、完了しない。WU は再試行に回り、上限を使い切れば失敗（replan か人への質問）になる。`decisions` は読まれないが、run が成功する経路は無い。`decisions` だけの型の誤りは `ResultFile` が `decisions` を読まないので run の失敗にならず、`decisions-not-array` の行に入る。 |
| question-and-decisions | `question` と `decisions` を同じ `result.json` に書いた | `crates/task-worker/src/result_report.rs:229`（`question` が最優先 → `Terminal::Question`）、`crates/task-dispatch/src/execution_scheduler.rs:146`（`blocked(question)`）、`crates/task-dispatch/src/run_settlement/work_unit.rs:108`（`decisions` を読むのは `completed` だけ）、`crates/task-dispatch/src/run_settlement/task_run.rs:153`（atomic も `completed` だけ） | fail-closed | run は質問で終わり、WU は `blocked(question)`（atomic は質問の待ち）になって、人が質問に答えるまで進まない。`decisions` の中身は記録されず進行の行も残らないが、仕事は止まり、人には質問が届く。中身が人に届かない点は「提案」に残す（直すなら owner は `run_settlement`。`continuable-end` と同じ replan の unit で扱える）。 |
| atomic-run | WU の無い run（木の子の atomic の節点）の `decisions` | `crates/task-dispatch/src/dispatcher/tree_units.rs:415`（`self_key = None`）、`crates/task-core/src/decision.rs:666`（`self` のまま）・`:716`（まとめの `needed_before` も `self`）、`crates/task-dispatch/src/dispatcher/worker_finish.rs:684`（`hold_task_for_worker_decisions`）、`crates/task-dispatch/src/dispatcher/tree_units.rs:531`（`decision_self_hold`） | fail-closed | atomic の run の unit・段階の集合は空なので、`self` 以外を指す要求は拒否され、key `rejected`・`needed_before: [self]` の決定になる。`self_hold` が立ち、task は最終レビューに進まず `ready` に戻り、答えるまで run を起こさない。単体試験 `rejected_worker_decisions_become_one_self_decision` が atomic の batch（`self_key = None`）を確かめている。dispatcher を通した atomic の拒否の試験は無い（未確認の事項に書く）。`decisions` が配列でない場合は `decisions-not-array` と同じ穴があり、同じ修正で塞がる。 |
| continuable-end | leaf の run が `yield`（`Terminal::Yielded`）・予算切れ・クラスタ job の wait で終わり、同じ `result.json` に `decisions` を書いた | `crates/task-dispatch/src/run_settlement/work_unit.rs:108`（`reason == "completed"` だけ）、`crates/task-dispatch/src/execution_scheduler.rs:143`・`:155`、`crates/task-dispatch/src/dispatcher/worker_finish.rs:595` | to-fix（owner 外。replan を求める） | `decisions` を読まないので、決定の行も進行の行も残らない。WU は continuation（または job の終了待ち）で同じ unit の次の run に進み、次の run が `decisions` を書かずに完了すれば leaf は done になる。人は要求を知らない。直す owner は `run_settlement`（`WorkUnitSettlement::reads_worker_decisions`・`task_reads_worker_decisions`）と `worker_finish.rs`（高 risk の path class `worker-finish`）で、unit `fix` の対象のパスの外にある。 |
| failed-end | leaf の run が `summary` と `decisions` を書いたが、WU の checks が落ちて `Failed { retryable: true }` に替えられた（または worker が失敗を返した） | `crates/task-dispatch/src/run_settlement.rs:260`（checks の不合格の取り込み）、`crates/task-dispatch/src/execution_scheduler.rs:147`、`crates/task-dispatch/src/run_settlement/work_unit.rs:108` | to-fix（owner 外。replan を求める） | run は失敗だが、読める `result.json` の `decisions` は痕跡なく捨てられる。WU は再試行し、次の試行が `decisions` を書かずに checks を通せば leaf は done になる。直す owner は `continuable-end` と同じで、unit `fix` の対象のパスの外にある。 |
| non-tree-node | 木の節点でない task（木の子でも /3 の計画を持つ root でもない）の run の `decisions` | `crates/task-dispatch/src/dispatcher/tree_units.rs:370`（`is_tree_node` でなければ `None`）、`crates/task-dispatch/src/dispatcher/dispatch_run.rs:876`（書き方の節を出すのは木の節点の worker の run だけ） | no-problem | この run の前置きには決定の要求の書き方の節が出ない（`RunContext.decision_requests = false`）ので、`decisions` は契約の外の欄で、`summary` などと同じく読まない欄として扱われる。ADR-0079 R3a 11. の「木の節点の worker の run だけ」のとおり。 |
| tree-disabled | `[execution.limits.tree] enabled = false` のときの `decisions` | `crates/task-dispatch/src/dispatcher/tree_units.rs:370`、`crates/task-dispatch/src/dispatcher/dispatch_run.rs:877` | no-problem | 書き方の節は出ず、読まない。既存の試験 `tree_disabled_ignores_worker_decisions` が、前置きに節が出ないこと・決定が 0 件で task が done になることを確かめている。 |
| planner-result-decisions | planner の run の `result.json` に worker の `decisions` を書いた | `crates/task-dispatch/src/dispatcher/dispatch_run.rs:876`（`!is_planner_dispatch`） | no-problem | planner の run には worker の決定の書き方の節を出さない。planner が人に選ばせる点は計画の `decisions` に書く（`planner-plan` の行）。 |
| empty-or-null | `decisions` が無い・`null`・空の配列 | `crates/task-core/src/decision.rs:875`、`crates/task-dispatch/src/dispatcher/tree_units.rs:380` | no-problem | 要求が 0 件なので、捨てる要求が無い。修正の後も `None`（今と 1 バイトも変わらない）のままにする（ADR の不変条件 I3）。 |
| reread-same-question | 答えた後の run が、同じ問いの要求（または同じ拒否のまとめ）をもう一度書いた | `crates/task-core/src/decision.rs:719`（同じ問いのまとめが記録済みなら出さない）、同じ key・同じ問いの重複の扱い（`prepare_worker_decisions` の `used`） | no-problem | ADR 2026-11-18 D3 の「同じ要求の読み直し」。その問いには人が答えた（または未回答で開いている）ので、出し直さないのが正しい。問いが違えば新しい拒否のまとめになる。 |
| needed-before-started-unit | `needed_before` が今 `running`・`done`・`blocked` の unit を指した | `crates/task-dispatch/src/dispatcher/tree_units.rs:486`（止めるのは `pending`・`ready` の unit だけ） | no-problem（人に届く） | 決定は記録され、受信箱と通知に出るので、人に届かない経路ではない。指した unit がもう走っている・終わっているときに止まらない点は、この調査の基準（人に届かないまま成功する）の外で、「提案」に残す。 |
| harness-error | run が harness の誤りか取り消しで終わった | `crates/task-dispatch/src/run_settlement/work_unit.rs:129`（`reset_only`）、`crates/task-dispatch/src/execution_scheduler.rs:152` | no-problem | WU は `ready` に戻り、run をやり直す。完了した run ではなく、次の run の `result.json` が要求を出し直す。 |

## to-fix の経路の直し方

### decisions-not-array（unit `fix` が直す）

- owner: `task_core::decision`（`crates/task-core/src/decision.rs`）と `Dispatcher::worker_decisions`（`crates/task-dispatch/src/dispatcher/tree_units.rs`）。
- 変える関数:
  - `task_core::decision::worker_decisions_from_json`: 「欄が無い・`null`・JSON でない」と「欄が配列でない」を分ける。欄の値をそのまま返す
    （無い・`null`・JSON でないときは `None`）。
  - `task_core::decision::prepare_worker_decisions`: 入力を `decisions` の欄の値にする。配列でなければ、要素の拒否と同じく
    `RejectedRequest`（要約は欄の型、理由は「decisions must be an array of decision objects (got <型>)」、後戻りは分からないので `high`）を
    1 件足し、既存の `rejected_decision` で key `rejected`・`needed_before: [self]`（leaf の run なら leaf の key）の決定にする。
    配列でない object が決定 1 件の形をしていても受け付けない（読み替えない）。
  - `Dispatcher::worker_decisions`: 欄が無い・`null`・空の配列なら今と同じく `None`。それ以外は `prepare_worker_decisions` に渡し、
    既存の `WorkerDecisionBatch::recorded` → `DecisionRequested`・`self_hold` → `hold_for_worker_decisions`（atomic は
    `hold_task_for_worker_decisions`）→ `decision_self_hold` の経路で人に届け、leaf を止める。
- 足す試験（名前は `decision_fail_closed_` で始める。修正前に落ちることを出力で記録する）:
  - `decision_fail_closed_non_array_decisions_become_one_rejected_decision`（task-core の単体試験）
  - `decision_fail_closed_non_array_decisions_hold_the_leaf`（`crates/task-dispatch/src/dispatcher/tests/tree_decisions.rs`）
  - `decision_fail_closed_non_array_decisions_hold_the_atomic_node`（同じ file）
- 新しい通知の仕組み・Manager・DB 表・run の失敗の種類・決定の kind は作らない。

### continuable-end・failed-end（unit `fix` は replan を求める）

- owner: `run_settlement`（`WorkUnitSettlement::reads_worker_decisions`〈`crates/task-dispatch/src/run_settlement/work_unit.rs:103`〉・
  `task_reads_worker_decisions`〈`crates/task-dispatch/src/run_settlement/task_run.rs:153`〉）と `worker_finish.rs`（呼び出し側、`:595`・`:684`）。
  `decision.rs`・`tree_units.rs` の側は変えなくてよい（`Dispatcher::worker_decisions` をそのまま使える）。
- unit `fix` の対象のパス（`tree_units.rs`・`decision.rs`・`tree_decisions.rs`）の外なので、unit `fix` は `decisions-not-array` を
  直したうえで `plan_issue` を返し、`run_settlement`・`worker_finish.rs` を範囲に入れた unit を足す replan を求める。
  `worker_finish.rs` は高 risk の path class `worker-finish` なので、その unit の計画には設計 brief が要る。
- 直し方の案と試験名は ADR の D3。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `git diff --name-only "$CELERIS_WU_BASE" HEAD -- crates/` → 出力なし（crates/ を変えていない）
- `sh scripts/dev/check-adr-numbers.sh` → exit 0
- `sh scripts/dev/check-doc-links.sh` → exit 0
- `sh scripts/dev/progress-index.sh --check` → exit 0

## 未確認の事項

- dispatcher を通した atomic の run の拒否（`needed_before` に unit 名を書いた atomic の木の子）の試験は無い。コードの読みと単体試験だけで
  fail-closed と判定した。unit `fix` の `decision_fail_closed_non_array_decisions_hold_the_atomic_node` が同じ止めの経路を通る。
- `continuable-end`・`failed-end` を本番の events で数えていない（`decisions` を書いた yield・失敗の run があったかは未確認）。
  本番の再監査は unit `repro` の範囲。
- `failed-end` で再試行の run の前置きに前の試行の `result.json` が入るかは確かめていない（入らないと考えて to-fix とした）。

## 提案

- `question-and-decisions`: 質問で止まる run の `decisions` の中身が人に届かない。`continuable-end` の replan の unit で、質問で終わった run
  の `decisions` も記録する（質問は今のまま最優先）か、少なくとも進行の 1 行を残すかを決める。
- `needed-before-started-unit`: 指した unit がもう走っている・終わっているとき、決定は人に届くが unit は止まらない。止められない理由を
  決定の問いに書き足すかを別の task で決める。
