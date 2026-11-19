---
title: アーキ v4 H R1 planner-prompt — planner の prompt に task の受入条件の節を足し、planner の試験を足す
tasks: [01M7T5KM1VYFDW1RD0B90YHAZ6]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# アーキ v4 H R1 planner-prompt

H R1（task 01M7T5KM1VYFDW1RD0B90YHAZ6）の WorkUnit `planner-prompt` の記録。材料は
[final-report.md](../2026-11-19-arch-v4-h/final-report.md) の提案 6・8 と [prompt-audit.md](../2026-11-19-arch-v4-h/prompt-audit.md)
「planner の受入条件（0/126）」。設計は [ADR 2026-11-19-planner-reviewer-prompt-coverage](../../adr/2026-11-19-planner-reviewer-prompt-coverage.md)。

## 結論

- planner の 4 builder（`build_execution_plan_prompt` の木と木でない形、`build_plan_prompt`、`build_project_plan_prompt`、
  `build_project_replan_prompt`）の prompt に、`## Task acceptance criteria (checked by the final review)` の節で task の
  `acceptance[].text` を全部出すようにした。空なら「無し」の 1 行を出す。節は objective の後・`## Instructions` の前に 1 回だけ出る。
- 新しい試験 5 件のうち、対象の 4 件（`planner_prompt_acceptance_` 3 件、`prompt_full_objective_planner` 1 件）は修正の前の
  code で落ち、修正の後に通った。review と WU の run の受入条件の節の byte を固定する 1 件は前後とも通った（出力が変わっていない）。
- design review と final review の `prompt_full_objective_*` は WU `review-tests` の担当で、この WU では足していない。

## 変更

- `crates/task-worker/src/claude_code/prompt.rs`
  - `acceptance_lines`（task の受入条件を `{i}. {text}\n` で並べる helper）を足し、`build_review_prompt` の
    「Acceptance criteria of the task under review」と `build_execute_prompt` の WU の run の
    「最終レビューで確かめる Task の受け入れ条件（参考）」の同じ loop をそれで置き換えた（出力は同じ byte）。
  - `planner_acceptance_section` を足し、planner の 4 builder の `prompt_header` の直後に出した。`prompt_header` には入れていない。
  - 木でない WU の run の `## Acceptance criteria`（check ごとの説明を足す形）は書式が違うので変えていない。
- `crates/task-worker/src/claude_code/planner_prompt_tests.rs`（新規）: 下の 5 件。`claude_code/tests.rs` から `#[path]` で読む。
- `crates/task-worker/src/change_rules.rs`・`change_rules/tests.rs`: fixture（`planner_context`・`tree_planner`・`prompt_for`）を
  `claude_code` の試験から使うため、試験 module と 3 関数の可視性を `pub(super)` から `pub(crate)` にした（試験の code だけ）。
- `agent-docs/adr/2026-11-19-planner-reviewer-prompt-coverage.md`（新規）: D1〜D4。

harness の経路（ADR D3）: codex は `build_prompt`（`crates/task-worker/src/codex.rs:723`）、acp は `build_prompt_with_skill_list`
（`crates/task-worker/src/acp.rs:849`・`:1034`）、pi は `build_prompt_with_skill_list`（`crates/task-worker/src/pi.rs:199`）、
Claude Code は `build_prompt`（`crates/task-worker/src/claude_code.rs:471`）。CoS chat でない run はどれも `build_prompt_inner` を通る
（grep で確かめた）。

## 試験

| 試験 | 見ること | 修正前 | 修正後 |
|---|---|---|---|
| `planner_prompt_acceptance_tree_planner_lists_every_criterion` | 木の planner（/3）に 3 件の受入条件の節が objective と Instructions の間に 1 回 | FAIL（`tree planner: acceptance heading must appear exactly once`） | PASS |
| `planner_prompt_acceptance_flat_planner_and_plan_kinds_list_every_criterion` | /1・/2・`TaskKind::Plan`・案件の計画・replan に同じ節、空なら「無し」 | FAIL（`execution planner /1: acceptance heading must appear exactly once`） | PASS |
| `planner_prompt_acceptance_reaches_every_harness_input` | `build_prompt` と `build_prompt_with_skill_list` の両方に全 text | FAIL（`tree planner/build_prompt: missing \`ACC-ZERO ...\``） | PASS |
| `prompt_full_objective_planner_keeps_objective_and_acceptance_tails` | 3,000 文字の objective の末尾の印と 3,000 文字の受入条件の末尾の印が木の planner と `TaskKind::Plan` に載る | FAIL（`tree planner: acceptance criterion missing or truncated`。objective の末尾の確認はその前の行で通った） | PASS |
| `acceptance_lines_keep_review_and_work_unit_sections` | review と WU の run の受入条件の節の byte を固定 | PASS | PASS |

各試験の doc comment に、守る不変条件と、既存の `planner_prompt_*`・`prompt_boundary_` で検出できない理由を書いた。
`prompt_full_objective_planner_*` の objective の末尾の部分は修正前も通る（objective の全文配信は既に入っている）。修正前に落ちたのは
受入条件の末尾の部分で、この試験は planner の prompt の objective と受入条件の両方の切れを検出する回帰の試験として置いた。

## 証拠（提出時の HEAD で実行）

基準は `$CELERIS_WU_BASE`（この WU の開始時の base）。

- 修正前: `cargo nextest run -p task-worker --lib -E 'test(/planner_prompt_acceptance_|prompt_full_objective_planner|acceptance_lines_keep/)' --no-fail-fast`
  → `error: test run failed`、`5 tests run: 1 passed, 4 failed`（落ちたのは上の表の 4 件）。
- 修正後: `cargo nextest run -p task-worker -p task-dispatch --status-level pass -E 'test(/planner_prompt_|prompt_full_objective_|prompt_boundary_|boundary_case_|acceptance_lines_keep/)'`
  → exit 0、`20 tests run: 20 passed`（`prompt_boundary_` 3 件、C の `boundary_case_` 6 件、既存の `planner_prompt_*` を含む）。
- `bash scripts/dev/test-affected.sh` → exit 0、`6127 tests run: 6127 passed (9 slow), 21 skipped`（対象 crate: celeris・
  celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。

## 未解決事項

- `cargo clippy -p task-worker --tests -- -D warnings` は `crates/task-worker/src/adapter.rs:561` の `clippy::type_complexity` で落ちる。
  この WU の変更より前からある指摘で、release gate の clippy（`--workspace`、試験を含まない）は通る。この WU では直していない。
- 本番の実 prompt の再監査（ADR D4）は release の後の回帰条件。この WU では本番へ昇格していない。

## 提案

- 無し。
