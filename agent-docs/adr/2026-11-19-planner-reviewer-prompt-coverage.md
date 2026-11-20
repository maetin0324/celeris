---
tasks: [01M7T5KM1VYFDW1RD0B90YHAZ6]
---
# ADR 2026-11-19: planner の prompt に task の受入条件を出し、planner と reviewer の prompt が長い objective の末尾まで載ることを試験で守る

- 日付: 2026-11-19
- 状態: Accepted
- 関係: アーキv4 H（task 01M7MZTKBPZPQE4XN451F8X84F）の `agent-docs/progress/2026-11-19-arch-v4-h/final-report.md` の提案 6・8、
  同 dir の `prompt-audit.md`「planner の受入条件（0/126）」と `gate.md` §2・§8。[ADR-0007](0007-phase5-planner-and-reviewer.md) D7（kind 別の
  prompt）、[ADR-0072](0072-task-execution-decomposition.md) D9・D14（WU の run の参考の節・task-local の planner run）。
- task: 01M7T5KM1VYFDW1RD0B90YHAZ6（H R1 の子）

## 事象

H の実 prompt の監査（`prompt-audit.md`）で、本番の planner の 24 run の prompt に task の受入条件の text が 0/126 件しか入って
いなかった（objective の末尾は 24/24 run で入った）。planner は task の受入条件を見ずに WorkUnit の `done_when` と check を書く。
受入条件の text にだけある条件（例: 「test-affected・clippy・fmt が通る」「進捗が done」）は、objective が同じ文を含まない限り
planner に届かない。

また、3,000 文字の objective の末尾が prompt に載ることを確かめる試験（`prompt_boundary_worker_and_planner_get_class_rule_invariants_and_dependency_rule`）
は worker の prompt にしか末尾の印を当てていない。planner・design review・final review の prompt が objective を途中で切っても、
今の試験は落ちない（`gate.md` §2・§8）。

## 原因

`crates/task-worker/src/claude_code/prompt.rs` の planner の builder は、どれも `prompt_header`（objective の全文を含む）に計画の
指示を足すだけで、`task.acceptance` を出さない。

- `build_execution_plan_prompt`: 実際の木の planner（`context.execution_planner` がある run。`tree` があれば /3、無ければ /1・/2）。
- `build_plan_prompt`: `TaskKind::Plan`（`plan.json`）。
- `build_project_plan_prompt`・`build_project_replan_prompt`: 案件の計画と replan（`project-plan.json`）。

`task.acceptance` を出すのは worker の `build_execute_prompt`（WU の run では「最終レビューで確かめる Task の受け入れ条件（参考）」の節）と
`build_review_prompt`（「Acceptance criteria of the task under review」）だけで、2 か所に同じ loop（`{i}. {text}`）が書かれていた。

## 決定

- D1: planner の 4 builder（`build_execution_plan_prompt` の木と木でない形、`build_plan_prompt`、`build_project_plan_prompt`、
  `build_project_replan_prompt`）の prompt に、task の `acceptance[].text` を全部、見出し
  `## Task acceptance criteria (checked by the final review)` の節に `{i}. {text}` の行で出す。受入条件が空なら「無し」の 1 行を出す
  （節を黙って消さない）。
  - 行の組み立ては `prompt.rs` の helper 1 つ（`acceptance_lines`）にし、`build_review_prompt` の受入条件の節と `build_execute_prompt`
    の WU の run の参考の節の同じ loop をそれで置き換える。この 2 節の出力は 1 byte も変えない（同じ書式 `{i}. {text}\n`）。
  - 木でない WU の run の `## Acceptance criteria`（check ごとの説明を足す形）は書式が違うので helper に寄せない（見た目が似ているだけの
    ものをまとめない）。
  - `prompt_header` には入れない。worker と review の prompt は既に受入条件の節を持つので、header に入れると同じ text が 2 回出る。
  - 置き場所は各 builder の「## Instructions」の前（objective の直後。planner が分解の前に読む）。
- D2: 3,000 文字の objective の末尾の印が planner・design review・final review の prompt に載ることを試験で守る（prefix
  `prompt_full_objective_`）。
  - planner の分（この task の WU `planner-prompt`）: `prompt_full_objective_planner_*` が木の planner と `TaskKind::Plan` の prompt に
    objective の末尾の印と、3,000 文字の受入条件の text の末尾の印が載ることを確かめる。
  - design review と final review の分は WU `review-tests` が足す（この ADR の決定に従う）。
  - 新しい試験は、守る不変条件と、今の `planner_prompt_*`・`prompt_boundary_` で検出できない理由を doc comment に書く。
- D3: ほかの harness も同じ builder を通る。codex は `build_prompt`（`crates/task-worker/src/codex.rs:723`）、acp は
  `build_prompt_with_skill_list`（`crates/task-worker/src/acp.rs:849`・`:1034`）、pi は `build_prompt_with_skill_list`
  （`crates/task-worker/src/pi.rs:199`）を呼び、どれも CoS chat でない run では `build_prompt_inner` に入る（Claude Code は
  `crates/task-worker/src/claude_code.rs:471` の `build_prompt`）。harness ごとの分岐を足さず、試験
  `planner_prompt_acceptance_reaches_every_harness_input` が `build_prompt` と `build_prompt_with_skill_list` の両方の出力に全 text が
  入ることを確かめる。
- D4: 本番の実 prompt（`/local/celeris/data/workspaces/<task id>/runs/<run id>/prompt.txt`）での再監査は、この変更を含む release の
  後の回帰条件にする。release の後の planner の run で、prompt に `## Task acceptance criteria (checked by the final review)` の節があり、
  task の受入条件の text が全部入ること（H の監査の 0/126 が全件になること）を `prompt-audit.md` と同じ数え方で確かめる。この task
  では本番へ昇格しない。

## 変えないもの

- task-dispatch・repo-health の policy の file・API・CLI・DB の契約。planner の出力の形（`plan.json`・`execution-plan.json`・
  `project-plan.json`）と検証。
- worker・review の prompt の受入条件の節の出力（D1 の helper への置き換えは同じ byte を出す）。

## 試験

- `planner_prompt_acceptance_tree_planner_lists_every_criterion`（木の planner）
- `planner_prompt_acceptance_flat_planner_and_plan_kinds_list_every_criterion`（木でない execution planner の /1・/2、
  `TaskKind::Plan`、案件の計画・replan、空の受入条件）
- `planner_prompt_acceptance_reaches_every_harness_input`（`build_prompt` と `build_prompt_with_skill_list`）
- `prompt_full_objective_planner_keeps_objective_and_acceptance_tails`（木の planner と `TaskKind::Plan`）
- `acceptance_lines_keep_review_and_work_unit_sections`（review と WU の run の節の byte を固定）
