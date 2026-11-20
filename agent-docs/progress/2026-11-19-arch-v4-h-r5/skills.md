---
title: アーキ v4 H R5 skills — skills.rs の put_skill_op・delete_skill_op を plan と効果へ移し、CoS の包みを cos/ops/admin.rs に置く
tasks: [01M7VJY8NZ5G84ACBNQTPVV3R4]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# skills（WorkUnit）

完了日: 2026-11-20

## 0. 結論

- `crates/task-api/src/skills.rs` の `put_skill_op`・`delete_skill_op` を消し、plan と効果の 4 関数に分けた。
  `Option<&OperationAudit>` の引数・`Applied` の戻り値・`audit` の有無の分岐・`.direct()` の 2 か所・
  `use crate::cos::operations::{Applied, OperationAudit};` の行が無くなった。
- CoS の `skill.put`・`skill.delete`（`crates/task-api/src/cos/ops/admin.rs`）は、plan の拒否を `audit.reject` で、効果を
  `admin.rs` の既存の `external`（`audit.external` と `effect_value`）で包む。`audited(crate::skills::…)` の 2 か所が無くなった。
- 契約試験 `applied_contract_skill_` の 4 本は、試験を変えずに通った。API・CLI・DB の契約は変えていない。
- 変えた file は `skills.rs`・`cos/ops/admin.rs` とこの note だけ。`cos/operations.rs`・`crates/task-api/tests/`・policy の file は変えていない。

## 1. 変更

| 旧 | 新（`skills.rs`） | 中身 |
|---|---|---|
| `put_skill_op` の検査 | `plan_skill_put(root)` | `require_kb`。何も書かない |
| `put_skill_op` の 2 つの腕の `skills_put` | `skill_put_effect(root, name, request, source) -> Effect<SkillPutResult>` | files を `(path, content)` に写し、`skills_put(.., Some(source))`。source は HTTP が `gui`、CoS が `cos` |
| `delete_skill_op` の `checked` の closure | `plan_skill_delete(store, root, name)` | `require_kb` → `skills_get` が無ければ 404 `skill_not_found` → `org_list`（`store_problem`）→ mount 中は 409 `skill_mounted` |
| `delete_skill_op` の 2 つの腕の `skills_delete` | `skill_delete_effect(root, name) -> Effect<()>` | `skills_delete`（KB の commit 1 つ） |
| `skill_effect<T: Serialize>(..) -> Effect<Value>`（CoS 専用。値を JSON に写す） | `skill_effect<T>(Result<T, SkillError>) -> Effect<T>`（private。2 つの効果が共有する） | `Failed` は外側の `Err(internal)`、ほかの `SkillError` は内側の `Err(skill_error_to_problem)`（422 `validation`）。JSON への写しは CoS の `external` が `effect_value` で行う |

`Effect<T>` は既存の `crate::problem::Effect`（`Result<Result<T, ApiProblem>, ApiProblem>`）を使った。新しい型は足していない。

- HTTP の `put_skill`: `state.blocking` の中で `plan_skill_put` → `skill_put_effect(.., "gui")??` → `tracing::info!(op = "skill_put")` → 200。
- HTTP の `delete_skill`: `plan_skill_delete` → `skill_delete_effect(..)??` → `tracing::info!(op = "skill_delete")` → 204。
  `require_admin`・`no_query`・`read_json`・`root_of` の順は変えていない。
- CoS の `skill.put`: `kb_root` の拒否（今のまま）→ 本文の decode（今のまま）→ `plan_skill_put` の拒否を `audit.reject(store, "skill", name, p)` →
  `external(store, audit, ("skill", name), "skill.put", || skill_put_effect(.., "cos"))`。
- CoS の `skill.delete`: `require_empty_body`（今のまま）→ `plan_skill_delete` の拒否を `audit.reject` →
  `external(.., "skill.delete", || skill_delete_effect(..) を {"name", "deleted": true} に写す)`。
- 旧い形を説明する doc comment（`shared by the handler and CoS`）は消し、plan と効果の doc comment を新しく書いた。`skills.rs` の
  doc comment に `OperationAudit`・`Applied` の語は無い。
- 依存: `skills.rs` から `crate::cos` への参照が無くなった（`skills → cos` の組は消える見込み。C2 での確認は WorkUnit `after`）。
  `admin.rs` の `use crate::skills::{..}` に `self` を足しただけで、`cos → skills` は既にある組。`use crate::cos`・`crate::handlers` は足していない。

挙動の同一性（旧コードと比べた点）:

- CoS の効果の時刻（`OffsetDateTime::now_utc()`）・action の文字列（`matched.action` は `skill.put`・`skill.delete`）・target（`skill`/<name>）・
  JSON への写し（旧 `skill_effect` の `to_value` と `effect_value` は同じ処理）は変わらない。
- 拒否の順（decode → `require_kb`、`require_empty_body` → `require_kb` → 404 → 409）も変わらない。
- HTTP の `??` は、旧い `skill_error_to_problem`（`Failed` は 500、ほかは 422）と同じ problem を返す。

## 2. ADR との違い

- ADR 2026-11-20-applied-shape-migration の §7 は「`skills.rs` の `skill_effect`（CoS 専用の `SkillError` の写し）を消す」と書く。
  この WorkUnit の objective は「効果（skills_put・skills_delete と skill_effect）を置く」と書く。両方に合わせ、名前 `skill_effect` は残し、
  中身を「`Result<T, SkillError>` を `Effect<T>` に写す、2 つの効果が共有する private の関数」に変えた。CoS 専用の JSON への写しは消え、
  `admin.rs` の既存の `external` の `effect_value` が代わる。WorkUnit `after` は付記の表と ADR §7 をこの形に合わせる。
- D1 の関数名（`plan_skill_put`・`skill_put_effect`・`plan_skill_delete`・`skill_delete_effect`）と署名は ADR のとおり
  （戻り値は ADR の `Result<Result<T, ApiProblem>, ApiProblem>` と同じ意味の `Effect<T>`）。
- D3 の CoS の包みは `audit.external` を直接呼ばず、`admin.rs` に既にある `external` の helper（`audit.external` と `effect_value` を呼ぶ）を使った。
  監査の意味は同じ。

## 3. 証拠（提出時の HEAD で実行）

- `cargo nextest run --no-tests=fail -p task-api -E 'test(applied_contract_skill_)'` → exit 0、`4 tests run: 4 passed, 904 skipped`。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → 4 節。
- 否定の grep（`OperationAudit|Applied`・`audit = None|audit.is_some()|shared by the handler and CoS` が `skills.rs` に無い、
  `audited(crate::skills::` が `cos/` に無い）→ exit 0。
- 新しい `crate::(cos|handlers)` の path が `skills.rs` に無い（base は `crate::cos::operations` の 1 つ、HEAD は 0）→ exit 0。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 範囲外の path なし。
- 参考（この WorkUnit だけの値。正式の after は WorkUnit `after`）: 広い command の file 数 18 → 17、狭い command 13（14 → 13）。
  `skills.rs` は広い command に当たらない。

## 4. test-affected

（下に追記）
