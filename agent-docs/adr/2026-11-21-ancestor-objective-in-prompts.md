---
tasks: [01M7XQ9Q4P4ZQ38Y509J9XYAQ0]
---
# ADR 2026-11-21: 子 task の worker・planner・WU・reviewer の prompt に、祖先の task の目的と受入条件の全文を出す

- 日付: 2026-11-21
- 状態: Accepted
- 関係: 戦略層 v1 子 1（task 01M7XQ9Q4P4ZQ38Y509J9XYAQ0）の WU prompt-audit の「欠けの一覧」
  （`agent-docs/progress/2026-11-21-strategy-v1-1/prompt-audit.md`）。[ADR-0079](0079-recursive-task-decomposition.md) D4 (2)（木の planner に
  祖先の path と各祖先の目的の先頭 300 文字を渡す）、[ADR 2026-11-16-reduction-goal-delivery](2026-11-16-reduction-goal-delivery.md) D1（WU の
  run に task 全体の目的の全文）、[ADR 2026-11-19-planner-reviewer-prompt-coverage](2026-11-19-planner-reviewer-prompt-coverage.md) D1（`acceptance_lines`）。

## 事象

prompt-audit は、修正後の release（2026-11-19T15:39Z 以降の昇格）で始まった本番の 17 run の prompt を読み、次の欠けを見つけた。

1. 祖先（親・root）の受入条件が、どの role の prompt にも入らない（worker 0/4・reviewer 0/4・planner 0/18 件）。
2. planner の prompt で、祖先の objective が先頭 300 字で切れる（2 件）。
3. planner の prompt で、木の外の祖先（CoS が `parent_id` で作った親）が祖先として出ない（2 件）。
4. worker・reviewer の prompt に祖先の objective を出す節が無い。found の 4 件は、CoS が親の objective の文を子の objective に写した
   ために入っただけで、写さない子では届かない。
5. Project.request が、どの prompt にも入らない（0/15）。

## 原因

- `RunContext`（`crates/task-worker/src/protocol.rs`）に祖先の objective と受入条件を運ぶ欄が無い。`prompt_header`（`crates/task-worker/src/claude_code/prompt.rs`）は
  run 自身の task の objective（WU の run では WU と task の objective）しか出さない。
- 木の planner の `TreeAncestorContext` は `objective_excerpt`（`tree_planner_context` が `t.objective.chars().take(300)` で切る）だけを持ち、
  受入条件の欄が無い。祖先は `task_ops::tree::ancestors_with_self` が `tree.parent_unit` だけを辿って作るので、木の root の上にある
  `parent_id` の親は出ない。

## 決定

- D1: `RunContext.children` を `RunContext.lineage: TaskLineageContext { ancestors, children }` に置き換える（wire は
  `context.lineage.ancestors[]`・`context.lineage.children[]`。`context.children` は消す）。どちらも store の task の木の上下で、dispatcher が
  組む。`AncestorContext` は `{title, objective, acceptance}` で、どれも全文で切らない。`RunContext` の欄の数は増やさない（repo health の
  `size.responsibility-growth` の型の欄の上限 40。base で 40）。
  - 祖先の列は `task_dispatch::prompt_ancestors::chain_to_root` の 1 か所で辿る: 対象 task から、木の親（`tree_parent`）、無ければ
    `parent_id` を root まで（循環と 32 hop で止める）。これは ADR-0117 D1 の人の決定・回答の収集（`Dispatcher::review_human_inputs`）が
    持っていた走査そのもので、その走査をこの関数に移して両方が使う。木の root の上にある CoS が `parent_id` で作った親も祖先に入る
    （欠け 3）。`task_ops::delegate::ancestors`（`parent_id` だけ。委譲の循環の検査）と `task_ops::tree::ancestors_with_self`（木の親だけ。
    木の上限と段階）は目的が違うので使わない。
  - worker・planner・WU の run は `Dispatcher::run_extras` が `prompt_ancestors::lineage` で組む（子の要約は従来どおり `run_extras` が組む）。
    親の無い task では空で、wire（`skip_serializing_if = "TaskLineageContext::is_empty"`）にも prompt にも出ない。
  - prompt は `prompt_header` の 1 か所（task の目的の直後。WU の run では「このタスク全体の目的」の直後）で、見出し
    「祖先の task の目的と受け入れ条件（全文。root が先頭、親が末尾。…）」の節に出す。受入条件の行は `acceptance_lines` を使う
    （受入条件が空なら「無し」）。`prompt_header` を全 builder（worker・WU・`TaskKind::Plan`・案件の計画と replan・木と木でない
    execution planner・reviewer）と全 harness（`build_prompt`・`build_prompt_with_skill_list`）が使うので、harness ごとの分岐は足さない。
  - 祖先を読めなければ（store の失敗）、run の文脈を作れない store の失敗として run を起こさない（`run_extras` の他の store の読み取り
    と同じ扱い）。人の元の目的の無い prompt で走らせない。
  - CoS の対話の run（`cos_context::cos_run_context`）は木の中に無いので空。`celerisctl worker run`（手動の run）と eval の trial は、
    役職・記憶などと同じく最小の context で起こす既存の方針のままで、祖先を載せない。
- D2: `TreeAncestorContext.objective_excerpt` を消す（目的の全文は D1 の節にあるので、同じ text を 2 回出さない）。木の planner の
  祖先の行は題名と段階だけにする。ADR-0079 D4 (2) の「各祖先の目的の先頭 300 文字を渡す」はこの ADR で廃止する。
- D3: reviewer の run の祖先。
  - final review: `review_human_inputs` が D1 と同じ列から人の決定・回答と祖先を 1 つの `review::ReviewHumanInputs { decisions, answers,
    ancestors }` で返し、`ReviewExtras.human_inputs`（従来の `decisions`・`answers` の 2 欄を置き換える）→ `ReviewHumanContext.inputs` →
    `run_reviewer_inner` の `RunContext.lineage`（祖先だけ）に写す。人の決定・回答は従来どおり `ReviewRequest` に載る。
  - 設計 review: `start_design_review` が `prompt_ancestors::lineage` で組み、`run_design_reviewer` の `RunContext.lineage` に渡す。読み取りは
    予算の watch（`token_budget::run_watch` の `?`）の直後、`pick_reviewer` と `Event::WorkerStarted` の append より前に置く。読めなければ
    `?` で返し、`WorkerStarted`・runs の行（`run_index_start`）・`quota_begin` のどれも作らず、run を起こさない（次の tick でやり直す）。
- D3 の付記（読み取りの位置。3 つの経路）:
  - final review（`Dispatcher::spawn_review`）: `review_human_inputs(&task)?` は `Event::WorkerStarted`（role reviewer）の append より前に
    ある。読めなければ WorkerStarted を残さずに返る（base と同じ位置。この ADR は位置を変えない）。
  - 設計 review: 上のとおり WorkerStarted より前（final review と同じ扱い）。
  - worker・planner・WU の run（`dispatch_run`）: 祖先は `run_extras(...)?` の中で読む。`run_extras` は `Event::WorkerStarted` の append の
    後に呼ばれ、base でも同じ所で `?` で store を読む（`store.children`・`events_for`・`comments_for`・`get` など）。祖先の読み取りはこの既存の
    扱いに加わるだけで、位置と失敗時の挙動（WorkerStarted の後の store の失敗）を変えない。
  - `pick_reviewer` と `spawn_review` は変えない（repo health の関数の行の上限 200 を base で超えている関数を伸ばさない）。
- D5: wire の版。D1 で `context.children` を消し、D2 で `objective_excerpt` を消して既存の形を変えるので、`PROTOCOL_VERSION`
  （`crates/task-worker/src/protocol.rs`）を 4 から 5 に上げる（`docs/protocol/worker-protocol.md` の版の行・例、schema も合わせる）。
  daemon とワーカーの production のコードは版を数値で比べない（`git grep -n PROTOCOL_VERSION` で代入だけ）。版を 4 と固定していた
  試験の行（`subprocess/tests.rs` の comment 行の試験の末尾の `assert_eq!(PROTOCOL_VERSION, 4)`）は消す。その試験が守る comment 行の
  契約（非終端で sink の `comment` に渡る）は残りの assert が守る。`tests/e2e/tests/scenarios.rs` の `"protocol":4` の literal も 5 に
  直す必要がある（release gate の全体の試験だけが走らせる）。
- D4: 計画の無い root の task（祖先の無い task）の prompt は 1 byte も変えない。`ancestors_section` は空の祖先で空文字列を返し、
  木の planner の祖先の行の変更は木の祖先を持つ task にしか出ない。

## 欠け 5（Project.request）を扱わない理由

- この WU の目的は祖先の task の objective と受入条件を出すことで、root の task の prompt を変えないことを求める。Project.request を
  出すと、案件に属する root の task の prompt がすべて変わる。
- prompt-audit の対象の案件の Project.request は 345 字の案件の説明で、依頼の原文ではない。依頼の原文は CoS が root の task の objective
  に写しており、D1 でその全文が子 task の全 prompt に届く。
- Project.request を prompt に出すかは別の判断として、子 1 の進捗の提案節に書く。

## 試験（prefix `prompt_ancestor_objective_`）

- `task-dispatch` の `dispatcher::tests::prompt_ancestor_objective` の 2 件: 3,000 文字の objective と 3,000 文字の受入条件を持つ親を
  store に置き、`parent_id` の子の worker・final review の reviewer・木の planner・WU の run の `RunRequest` から `claude_code::build_prompt`
  で組んだ prompt に、末尾の印がちょうど 1 回ずつ載ることを確かめる。この変更の前の production の tree では 4 経路とも x0 で落ちる。
- `task-dispatch` の `health_review::design::tests::prompt_ancestor_objective_design_review_carries_the_parent`: 設計 review の prompt。
- `task-worker` の `claude_code::tests::ancestor_prompt_tests` の 2 件: 全 builder・2 つの harness の builder で祖先の節が 1 回出て root が
  親の前に来ること、祖先の無い context では節が無く、節を除いた prompt が祖先の無い prompt と 1 byte も違わないこと、空の
  `lineage` が wire に出ないこと。
- 既存の `review_human_inputs_collect_answered_decisions_and_answers_of_the_task_and_its_ancestors` が、走査を移した後も人の決定・回答の
  集め方が変わらないことを守る。

## 守る不変条件

- 子 task（`parent_id` を持つ task）の worker・planner・WU・reviewer の run の prompt に、祖先のすべての objective と受入条件の全文が
  1 回ずつ載る。CoS が子の objective に親の文を写したかによらない。
- 祖先の無い task の prompt と wire は変わらない（`context.children` を `context.lineage.children` に移した点を除く。子の要約を持つ run の
  wire だけが変わり、prompt の文は変わらない）。
