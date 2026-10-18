---
status: done
completed: 2026-10-18
title: 親の continuation で子が重複起票される問題
updated: 2026-10-18
---
# no-duplicate-child-delegation
- ADR agent-docs/adr/2026-10-18-no-duplicate-child-delegation.md。
- `task_ops::delegate::plan_delegation` が既存の子（failed/cancelled 以外）と title 正規化一致・objective Jaccard>=0.9 の提案を拒否（delegate.json・plan children 両経路）。
- 子を起票できる run の prompt に「Existing child tasks (do not duplicate)」節（run_context.rs / prompt.rs）。
- 進捗判定は子の起票を入力に持たないので変更なし（重複は起票時点で拒否される）。
- 試験: task-ops `rejects_reproposing_the_scope_of_an_existing_child`、task-worker `build_prompt_includes_children_section_only_when_present`。旧挙動の試験は無かった。
- 未解決: 意味的に同じだが表現が違う子は Jaccard で拾えない。提案: 範囲（write-set）での重複判定。
