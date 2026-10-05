# planner と CoS の prompt に最小 origin の規則を入れる

---
tasks: [01M470CJRXMPWS39S14PN7XPFP]
---

## 対象（D2.0 (e)）

agent-docs/adr/2026-10-05-browser-department-web-live-view.md D2.0 (e) の文面を、browser 子 task を
提案しうる 2 つの prompt 生成箇所に足した。両方とも `requirements.browser.allowed_domains`
（task-req 葉が決めた実際の欄名）を使い、例 `https://billing.example.com` と「親を超えない」規則を
出す。

- planner（execution plan）: `crates/task-worker/src/claude_code/prompt.rs`
  - `tree_plan_shape_section`（`celeris.execution-plan/3`。木の子 task unit。`PlanUnitSpec.requirements`
    が既に欄を持つ）。
  - `parallel_phases_section`（`celeris.execution-plan/2`。`children` 配列。`ExecutionChildSpec.requirements`
    が既に欄を持つ）。
  - 共通の文面は `pub const BROWSER_ALLOWED_DOMAINS_GUIDANCE`（同ファイル先頭）に 1 本化し、
    両方の節から `out.push_str` で差し込む。
- CoS の起票: `crates/task-worker/src/preamble.rs::actions_instructions`（`conversation_instructions`
  経由で `ConversationAddressee::Secretary` の対話 run にだけ出る）。`create_task` の JSON 例に
  `"requirements": {"browser": {"allowed_domains": [...]}}` を足し、直後に同じ規則を日本語で書いた
  （`ConsoleAction::CreateTask.requirements` が既に欄を持つ）。

v1（`celeris.execution-plan/1`。`WorkUnitSpec` に `requirements` 欄が無い）には足していない。leaf は
task を作らないので、ADR の対象（「browser task の `allowed_domains`」）に当たらない。

## 試験

`browser_allowed_domains_prompt_` で始まる試験 3 本（acceptance の下限 2 本を超える）:

- `claude_code::tests::browser_allowed_domains_prompt_tree_planner_has_minimal_origin_rule`（/3）
- `claude_code::tests::browser_allowed_domains_prompt_v2_planner_has_minimal_origin_rule`（/2）
- `preamble::tests::browser_allowed_domains_prompt_cos_create_task_has_minimal_origin_rule`（CoS）

いずれも `requirements.browser.allowed_domains` と `https://billing.example.com` が出力に含まれる
ことを確かめる。既存の prompt snapshot 試験（`planner_prompt_carries_depth_and_leaf_criteria` 等）は
内容の追加だけなので変更不要で、そのまま通った。

## 証拠

- `cargo test -p task-worker --lib browser_allowed_domains_prompt_` → `3 passed; 0 failed`
- `cargo test -p task-worker --lib` → `769 passed; 0 failed; 4 ignored`（既存試験に退行なし）
- `cargo clippy -p task-worker --lib -- -D warnings` → exit 0（警告なし）
- `cargo fmt --package task-worker -- --check` → exit 0

## 未解決事項・提案

- CoS の skill（config/skills/ 下）は grep したが、CoS 専用の外部 skill ファイルは見つからなかった
  （CoS の指示は `preamble.rs` の Rust 文字列に直書きで、config/skills/ は worker 側の mount skill の
  置き場所）。ADR の「CoS の起票の prompt・skill」は `preamble.rs` のこの箇所で満たしたと判断した。
