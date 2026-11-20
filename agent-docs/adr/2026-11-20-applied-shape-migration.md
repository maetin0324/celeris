---
tasks: [01M7VJY8NZ5G84ACBNQTPVV3R4]
---
# ADR 2026-11-20: knowledge・lifecycle・skills の 6 関数を `Applied` の形から org_write の形（共有の plan と効果、HTTP と CoS の adapter）へ移す（アーキv4 H R5）

- 日付: 2026-11-20
- 状態: Accepted（実装前。この ADR を書いた WorkUnit `adr` は `crates/` を変えていない。旧コードに当てる契約試験は WorkUnit `contract`、
  測定の道具と before は `measure`、knowledge.rs は `knowledge`、lifecycle.rs は `pause`、skills.rs は `skills`、after と付記の表と進捗の完了は `after` が行う）
- task: 01M7VJY8NZ5G84ACBNQTPVV3R4（アーキv4 H 01M7MZTKBPZPQE4XN451F8X84F の子、人の決定 unmet b の R5。H の final-report の提案 15・16・17）
- 進捗: `agent-docs/progress/2026-11-19-arch-v4-h-r5.md`（WorkUnit `measure` が置く）、この WorkUnit の要点は [adr.md](../progress/2026-11-19-arch-v4-h-r5/adr.md)、
  測定値は同じ dir の `metrics.tsv`
- 関係: [ADR 2026-11-18 org-write-shared-use-case](2026-11-18-org-write-shared-use-case.md)（D。付記「2 つの形の使い分けと撤去の条件」が移し先の形と撤去の条件）、
  [ADR 2026-10-09 CoS の全変更操作](2026-10-09-cos-operations-all-mutations.md)（`cos-audit-same-tx`、`Applied` の形の D5）、
  [ADR 2026-11-18 knowledge-lifecycle-tx-boundary](2026-11-18-knowledge-lifecycle-tx-boundary.md)（F。`knowledge.rs` の transaction の境界）、
  [ADR 2026-11-04 project-controller](2026-11-04-project-controller.md)（`lifecycle-human-epoch-same-tx`）、
  [ADR 2026-10-27 API の主体](2026-10-27-api-principals.md)（`api-principal-from-token`）、
  [ADR-0056](0056-mcp-server.md) D3 と P-82-d（skill の frontmatter の `source`）、
  [ADR 2026-11-17 境界の意味を変える改修の risk](2026-11-17-boundary-change-risk-review.md)（`transaction` class の設計 review）、
  [2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。
- 行番号は、この ADR を書いた時点の HEAD（WorkUnit の base と同じ内容。件名は `integrate wu/r7-rules (phase fix1)`）の行。

## 1. 根本原因（brief 1）

症状: `crates/task-api/src` の `Applied` の形の file が広い command で 18、狭い command で 14 残る（H の final-report の `applied_wide`・`applied_narrow`）。
そのうち `knowledge.rs`・`lifecycle.rs`・`skills.rs` の 3 file は、HTTP と CoS が共有する 6 関数が `Option<&OperationAudit>` を受け、
`Applied::Direct`・`Applied::Audited` を返す。

| file:行 | 関数 | CoS の呼び出し元 |
|---|---|---|
| `knowledge.rs:548` | `put_page_op`（完全修飾 `Option<&crate::cos::operations::OperationAudit>`。狭い command に当たらない） | `cos/ops/decisions.rs:102`（`knowledge.page_put`） |
| `knowledge.rs:921` | `accept_op` | `cos/ops/decisions.rs:116`（`knowledge.accept`） |
| `knowledge.rs:1005` | `reject_op` | `cos/ops/decisions.rs:190`（`knowledge.reject`） |
| `lifecycle.rs:134` | `task_pause_op` | `cos/ops/tasks.rs:315`（`task.pause`・`task.resume`） |
| `skills.rs:286` | `put_skill_op` | `cos/ops/admin.rs:354`（`skill.put`） |
| `skills.rs:349` | `delete_skill_op` | `cos/ops/admin.rs:363`（`skill.delete`） |

原因は 2 つある。

1. 共有の関数が「誰が呼んだか」で 2 つに分かれている。`audit = None`（HTTP）と `audit = Some`（CoS）で別の腕を持ち、腕ごとに git・DB の効果を
   書き直している（`reject_op` は `InboxOutcome` の写しを 2 回、`put_skill_op` は `skills_put` の呼び出しを 2 回、`task_pause_op` は
   `plan_set_paused` の呼び出しを 2 回書く）。HTTP の腕だけの tracing（`op = "knowledge_accept"` など）も共有の関数の中にある。
2. 分岐のために、業務の file が CoS adapter の型（`cos::operations::{Applied, OperationAudit}`）を import する。`lifecycle.rs:23` と
   `skills.rs:28` は `crate::cos` への参照がこの 1 行だけで、これが `lifecycle → cos`・`skills → cos` の依存の組を作る。
   HTTP の 6 か所は `.direct()` で「監査が無いのに `Audited` が返った」という起きない場合を internal error に写す。

## 2. 責務の owner（brief 2）

- 業務の検証（plan）と効果: `crates/task-api/src/knowledge.rs`・`lifecycle.rs`・`skills.rs`（[architecture-map](../../docs/architecture-map.md) の
  「案件の lifecycle …・task の subtree の一時停止」と「知識ベース操作（KB・skill）」の行。git の書き込みの本体は `task_ops::knowledge`、
  task の行の書き込みは `task_core::store::SqliteStore::edit_task_tx`）。
- 監査の包み: CoS adapter（`crates/task-api/src/cos/ops/decisions.rs`・`tasks.rs`・`admin.rs` の各腕）と `cos/operations.rs` の `OperationAudit`。
- HTTP の包み: 同じ 3 file の async handler（`put_page`・`accept`・`reject`・`task_action`・`put_skill`・`delete_skill`）。

## 3. 再利用先（brief 3）

- 形: D の `org_write.rs`（plan は `ApiProblem` で拒否、効果は呼び出し元の transaction か外部の系に書く）と、その HTTP adapter
  （`handlers/org.rs`）・CoS adapter（`cos/ops/admin.rs` の `org.*`）。
- 監査の包み: `OperationAudit::reject`・`apply`・`external`（`cos/operations.rs`。変えない）。
- 効果の本体: `task_ops::knowledge::{commit_page, reindex, inbox_accept, inbox_reject, skills_put, skills_get, skills_delete}`、
  `task_ops::lifecycle::{plan_set_paused, pause_result}`、`SqliteStore::{update_task, edit_task_tx}`（`update_task` は
  `write_tx(|tx| edit_task_tx(tx, ..))`。`crates/task-core/src/store/tasks.rs:724`）。
- 拒否の写し: `knowledge.rs` の `require_kb`・`page_path`・`path_problem`・`candidate_not_found`・`knowledge_unavailable`・`etag_mismatch`・
  `page_not_found`、`skills.rs` の `skill_not_found`・`skill_mounted`・`skill_error_to_problem`、`problem::ops_problem`・`store_problem`。
- 試験: 下の §8 の既存の試験と、WorkUnit `contract` が旧コードで固定する `applied_contract_<op>_(http|cos)_*`。

## 4. 決定

### D1. 共有の file には plan と効果だけを置く

6 関数を消し、同じ file に次を置く。どれも `OperationAudit` を受けず、`Applied` を返さず、`crate::cos` を参照しない。
名前は下のとおり（実装の WorkUnit が同じ file の既存の名前と衝突して変えるなら、進捗に書き、WorkUnit `after` が付記の表に今の名前を書く）。

| 旧い関数 | 共有の plan（読み取りと検証。拒否は `ApiProblem`。DB も git も書かない） | 共有の効果 |
|---|---|---|
| `knowledge::put_page_op` | `plan_page_put(root, request) -> Result<PageEdit, ApiProblem>`: `require_kb` → `page_path` → `_inbox/` は 403 `path_forbidden` → message の既定 `knowledge: <path>` → `mark_human_authored` → 空の etag を捨てる | `page_put_effect(root, &PageEdit) -> Result<Result<KnowledgePageResult, ApiProblem>, ApiProblem>`: `commit_page`。`Written` は `reindex` して `Ok(Ok(..))`、`EtagMismatch` は `Ok(Err(etag_mismatch))`、`Missing` は `Ok(Err(page_not_found))`、`Failed` は `Err(knowledge_unavailable)`。tracing を持たない |
| `knowledge::accept_op` | `plan_accept(root, &id, &request) -> Result<(), ApiProblem>`: `require_kb` → `inbox_path`（`path_problem`）→ `request.path` が空でなければ `page_path` | `accept_effect(root, &id, &request) -> Result<KnowledgePageResult, ApiProblem>`: `inbox_accept`。`Missing` は 404 `candidate_not_found`、`Exists` は 409 `page_exists`、`Failed` は 409 `knowledge_unavailable`、`Rejected` は `knowledge_unavailable("unexpected outcome")` |
| `knowledge::reject_op` | `plan_reject(root, &id) -> Result<(), ApiProblem>`: `require_kb` → `inbox_path` | `reject_effect(root, &id) -> Result<KnowledgeRejectResult, ApiProblem>`: `inbox_reject`。`Missing` は 404 `candidate_not_found`、`Failed` は `knowledge_unavailable(detail)`、ほかは `knowledge_unavailable("unexpected outcome: {other:?}")` |
| `lifecycle::task_pause_op` | `plan_task_pause(store, task_id, pause, by, now) -> Result<(Task, Event), ApiProblem>`: `task_ops::lifecycle::plan_set_paused` を `ops_problem(store, e, Some("task_pause"｜"task_resume"))` で写す | 共有の効果は task-core の `SqliteStore::edit_task_tx`（task-api に新しい関数を置かない） |
| `skills::put_skill_op` | `plan_skill_put(root) -> Result<(), ApiProblem>`: `require_kb` | `skill_put_effect(root, name, request, source) -> Result<Result<SkillPutResult, ApiProblem>, ApiProblem>`: files を `(path, content)` に写して `skills_put(.., Some(source))`。`SkillError::Failed` は `Err(ApiProblem::internal(detail))`、ほかの `SkillError` は `Ok(Err(skill_error_to_problem(e)))`（422 `validation`） |
| `skills::delete_skill_op` | `plan_skill_delete(store, root, name) -> Result<(), ApiProblem>`: `require_kb` → `skills_get` が無ければ 404 `skill_not_found` → `org_list`（`store_problem`）→ mount されていれば 409 `skill_mounted` | `skill_delete_effect(root, name) -> Result<Result<(), ApiProblem>, ApiProblem>`: `skills_delete`。`Failed` は外側の `Err`、ほかは内側の `Err` |

外部の系（git）の効果の戻り値 `Result<Result<T, ApiProblem>, ApiProblem>` は、`OperationAudit::external` の closure と同じ意味にそろえる:
内側の `Err` は「外部の系が変更の前に断った」（CoS では `rejected`）、外側の `Err` は「結果が分からない」（CoS では pending のまま remediation）。
HTTP は両方をそのまま応答の problem にする（`effect()??`）。これで `skills.rs` の CoS 専用の写し `skill_effect` は要らなくなる（§7）。

### D2. HTTP adapter（plan → 効果）

各 async handler が `state.blocking` の中で plan → 効果の順に呼ぶ。`.direct()` を使わない。

- `put_page`: `plan_page_put` → `page_put_effect(..)??` → `tracing::info!(op = "knowledge_put", path, unchanged, cos = false, "knowledge page committed")` → 200。
- `accept`: `plan_accept` → `accept_effect` → `tracing::info!(who = "admin", op = "knowledge_accept", id, path, ..)` → 200。
- `reject`: `plan_reject` → `reject_effect` → `tracing::info!(who = "admin", op = "knowledge_reject", id, ..)` → 200。
- `task_action`: `plan_task_pause(store, task_id, pause, &caller.actor(), now)` → `store.update_task(&next, event)`（`store_problem`）→
  `task_ops::lifecycle::pause_result(store, &updated)`（`ops_problem`）→ 今の tracing（`op = task_pause｜task_resume`、subtree の数）→ 200。
  今の HTTP の腕と同じ書き込み（`update_task` = `write_tx(edit_task_tx)`）で、書き直さない。
- `put_skill`: `plan_skill_put` → `skill_put_effect(.., "gui")??` → `tracing::info!(who = "admin", op = "skill_put", ..)` → 200。
- `delete_skill`: `plan_skill_delete` → `skill_delete_effect(..)??` → `tracing::info!(who = "admin", op = "skill_delete", ..)` → 204。

`require_admin`・`no_query`・`read_json`・`root_of` の順は変えない（`api-principal-from-token`: HTTP の `by` は token の principal の `caller.actor()` のまま）。

### D3. CoS adapter（cos/ops の呼び出し元。plan の拒否は `audit.reject`、効果は今と同じ監査の意味で包む）

各腕が plan → 包みの順に呼ぶ（今の 6 関数も plan の検査を包みの前に行う。冪等の再送でも plan が先に走る順は変えない）。
`audited(..)` を使わない（`audited` 関数は他の file の操作が使うので残す）。

- `cos/ops/decisions.rs` の `knowledge.page_put`: `request.path` を写してから `plan_page_put`。拒否は `audit.reject(store, "knowledge_page", <写した request.path>, p)`
  （今と同じく正規化の前の path）。効果は `audit.external(store, "knowledge_page", &edit.path, "knowledge.page_put", now, || ..)` の中で
  `page_put_effect` を呼び、`Ok(Ok(result))` なら `tracing::info!(op = "knowledge_put", .., cos = true, ..)` をして `serde_json::to_value`、
  `Ok(Err(refused))` はそのまま、`Err` は `?` で外へ返す。
- `knowledge.accept`: `plan_accept` の拒否は `audit.reject(store, "knowledge", &id, p)`。効果は `audit.apply(store, "knowledge", &id, "knowledge.accept", |_tx| ..)`
  の transaction の中で `accept_effect` を呼ぶ（git の取り込みは SQLite の transaction の中。今と同じ）。成功は `{"id", "path", "sha"}`。
  失敗の problem は控えてから `ChatError::Conflict(problem.detail())` を返し、腕は `failure.unwrap_or(problem)` を返す（今と同じ）。
- `knowledge.reject`: `plan_reject` の拒否は `audit.reject(store, "knowledge", &id, p)`。効果は `audit.apply(.., "knowledge.reject", |_tx| ..)` の中で
  `reject_effect` を呼ぶ。成功は `{"id", "sha"}`。失敗の problem は控え、code が `candidate_not_found` なら
  `ChatError::NotFound { kind: "knowledge candidate", id }`、ほかは `ChatError::Conflict(problem.detail())` を返す（今の `InboxOutcome` ごとの写しと同じ値。
  `knowledge_unavailable(detail).detail()` は `detail` そのもの）。code の文字列は `knowledge.rs` の `pub(crate) const` にして、`candidate_not_found` と
  この腕の両方が同じ定数を使う。
- `cos/ops/tasks.rs` の `task.pause`・`task.resume`: `plan_task_pause(store, id, pause, &audit.by(), now)` の拒否は `audit.reject(store, "task", &id, p)`。
  効果は `audit.apply(store, "task", &id, "task.pause"｜"task.resume", |tx| ..)` の中で `SqliteStore::edit_task_tx(tx, &next, &event)?` を呼び、
  `{"task_id", "paused"}` を返す。task の行・`Edited` event・監査の行は同じ transaction（`cos-audit-same-tx`）。
  objective の文面は `apply_checked` だが、今の腕は `apply` で、`edit_task_tx` の `StoreError` を `ChatError::Store`（表示は `StoreError` のまま）に写す。
  `apply_checked` にすると、同じ失敗の監査の行の理由が `invalid: <store_problem の detail>` に変わり、応答も `chat_problem` から `store_problem` の写しに変わる。
  「拒否理由の文言を変えない」を優先して `apply` を残す。同じ transaction の性質はどちらでも同じ。
- `cos/ops/admin.rs` の `skill.put`: `plan_skill_put` の拒否は `audit.reject(store, "skill", &name, p)`。効果は
  `audit.external(store, "skill", &name, "skill.put", now, || ..)` の中で `skill_put_effect(.., "cos")` を呼び、成功を `serde_json::to_value`。
- `skill.delete`: `require_empty_body`（今のまま）→ `plan_skill_delete` の拒否は `audit.reject(store, "skill", &name, p)`。効果は
  `audit.external(.., "skill.delete", ..)` の中で `skill_delete_effect` を呼び、成功を `{"name", "deleted": true}` にする。

### D4. 6 op の今の形・移し先・包み・拒否理由・監査の意味

`audit.reject` の監査の行の理由は `rejected: <problem.detail()>; requested reason: <依頼の reason>`（`cos/operations.rs:325`）。
`audit.apply` の closure の失敗は `cos_operation_apply` が `ChatError` の表示文で rejected の行を書く（`crates/task-core/src/chat/operations.rs` の
`error.to_string()`）。`audit.external` の内側の `Err` は `fail_external` が rejected に、外側の `Err` は pending のまま残す。

| op（CoS の action） | 今の形（HTTP ／ CoS） | 移し先（D1） | HTTP の包み | CoS の包み | 拒否理由（CoS の監査の行） | 監査の意味 |
|---|---|---|---|---|---|---|
| `knowledge.page_put` | `put_page_op(.., None)?.direct()` ／ `decisions.rs:102` の `audited(put_page_op(.., Some(audit)))` | `plan_page_put`・`page_put_effect` | plan → effect `??` → tracing `cos = false` | `reject`（正規化の前の path）→ `external`（正規化した path）→ tracing `cos = true` | plan: `rejected: <detail>; ..`（403 `path_forbidden`・422・400・409 `knowledge_unavailable`）。etag 不一致: 409 `etag_mismatch` の detail、missing: `knowledge page not found: <path>` | 成功 applied。plan の拒否・etag 不一致・missing は rejected。git の失敗（`Failed`）は pending。再送は効果を走らせず記録を返す |
| `knowledge.accept` | `accept_op(.., None)?.direct()` ／ `decisions.rs:116` の `audited(accept_op(.., Some(audit)))` | `plan_accept`・`accept_effect` | plan → effect → tracing `knowledge_accept` | `reject` → `apply`（git は transaction の中）。失敗は `ChatError::Conflict(detail)`、応答は控えた problem | plan: `rejected: <detail>; ..`。効果: `conflict: knowledge candidate not found: <id>`・`conflict: page already exists: ..`・`conflict: <git の detail>` | 成功 applied（`cos_operations`・監査 event・card と同じ transaction）。plan と効果の失敗は rejected で、取り込みは起きない。pending は無い |
| `knowledge.reject` | `reject_op(.., None)?.direct()` ／ `decisions.rs:190` の `audited(reject_op(.., Some(audit)))` | `plan_reject`・`reject_effect` | plan → effect → tracing `knowledge_reject` | `reject` → `apply`。missing は `ChatError::NotFound { kind: "knowledge candidate", id }`、ほかは `ChatError::Conflict(detail)` | plan: `rejected: <detail>; ..`。効果: `knowledge candidate <id> not found`・`conflict: <git の detail>`・`conflict: unexpected outcome: ..` | 成功 applied（同じ transaction）。失敗は rejected。git の commit の後に SQLite の commit が落ちたときだけ監査の無い取り下げが残る（今と同じ。ADR 2026-10-05 D3 の注記） |
| `task.pause`・`task.resume` | `task_pause_op(.., &caller.actor(), None)?.direct()` ／ `tasks.rs:315` の `audited(task_pause_op(.., &audit.by(), Some(audit)))` | `plan_task_pause`・`edit_task_tx` | plan（`by` = token の principal）→ `update_task`（= `write_tx(edit_task_tx)`）→ `pause_result` | `reject` → `apply`（`edit_task_tx` と監査の行が同じ transaction。`by` = `cos:<thread_id>`） | plan: `rejected: <ops_problem の detail>; ..`。`edit_task_tx` の失敗: `StoreError` の表示文 | 成功 applied（task の行・`Edited` event・監査）。拒否と書き込みの失敗は rejected で task は変わらない（rollback）。pending は無い |
| `skill.put` | `put_skill_op(.., None)?.direct()` ／ `admin.rs:354` の `audited(put_skill_op(.., Some(audit)))` | `plan_skill_put`・`skill_put_effect` | plan → effect(`"gui"`) `??` → tracing `skill_put` | `reject` → `external` → effect(`"cos"`) | plan: `rejected: <knowledge_unavailable の detail>; ..`。効果の入力の誤り: 422 `validation` の detail | 成功 applied。入力の誤りは rejected。`SkillError::Failed` は pending。frontmatter の `source` は HTTP `gui`・CoS `cos`（既に `source:` があれば `skills_put` が触らない） |
| `skill.delete` | `delete_skill_op(.., None)?.direct()` ／ `admin.rs:363` の `audited(delete_skill_op(.., Some(audit)))` | `plan_skill_delete`・`skill_delete_effect` | plan → effect `??` → tracing `skill_delete` → 204 | `require_empty_body` → `reject` → `external` | plan: 404 `skill_not_found`・409 `skill_mounted`・409 `knowledge_unavailable` の detail。効果の入力の誤り: 422 の detail | 成功 applied（`{"name", "deleted": true}`）。plan の拒否（KB に触れる前）と KB が断った入力の誤りは rejected。`Failed` は pending |

### D5. 変えないもの

- API（HTTP の path・status・body・problem の code と detail）、CLI、DB（`cos_operations` の列と値、`events`、migration）。
- op ごとの包み（`apply`・`external`・`reject`）と、上の表の拒否理由の文言と監査の意味。
- skill の frontmatter の `source`（HTTP `gui`・CoS `cos`）。`source` は frontmatter に書くデータで、`task_ops::knowledge::skills_put` が既に引数に取る。
  adapter が定数を渡す（呼び出し元で分岐する旗ではない）。
- tracing の field と文言。`op = "knowledge_put"` の `cos` は共有の効果から外し、HTTP adapter の `false`・CoS adapter の `true` の定数にする
  （共有の効果に呼び出し元の種別の引数を足さない）。
- `Applied` 型・`audited`・`OperationAudit`（`cos/operations.rs` は変えない。広い command に残る 15 file が使う）。
- `lifecycle.rs` の案件の操作（`project_action` と `HumanEpoch`）。task の一時停止は今も human epoch を増やさない。

## 5. 選択肢（brief 4）

1. **採用: D1〜D3（共有の file は plan と効果、包みは各 adapter）。** D の付記の形そのもので、CoS の監査の意味を各腕が 1 か所で持つ。
   共有の file から `crate::cos` の型が消え、`lifecycle → cos`・`skills → cos` の組が消える。
2. 共有の関数に「包みの closure」を渡す（`FnOnce(effect) -> Result<T, ApiProblem>` を引数にする）。HTTP と CoS の違いを引数で吸収できるが、
   共有の interface に呼び出し元ごとの hook を足す形（規約 3）で、`Applied` の代わりの別の抽象になる。不採用。
3. `task_pause` の CoS を `apply_checked` にする。D の付記の新しい操作の既定に合うが、§D3 のとおり `edit_task_tx` の失敗の監査の理由と応答が変わる。不採用。
4. 6 関数を CoS adapter の側（`cos/ops`）へ丸ごと移し、HTTP から `cos` を呼ぶ。`handlers`・業務の file から `cos` への新しい依存の組になり、
   `deps.cycle` が落ちる。不採用。

## 6. 新しい状態・分岐・fallback・port（brief 5）

- 新しい状態・fallback・port: 無し。
- 新しい関数: D1 の plan と効果（11 個。`task_pause` は plan だけ）。旧い 6 関数の 2 つの腕を分けたもので、分岐は増えない
  （`Option<&OperationAudit>` の `match` と `let Some(audit) = audit else` の 6 か所が消える）。
- 新しい定数: `knowledge.rs` の `candidate_not_found` の code（CoS の `knowledge.reject` の写しが使う）。

## 7. 撤去対象（brief 6）

- 6 関数（`put_page_op`・`accept_op`・`reject_op`・`task_pause_op`・`put_skill_op`・`delete_skill_op`）と、その `Option<&OperationAudit>` 引数・
  `Applied` の戻り値・`audit` の有無の分岐。
- HTTP の 6 か所の `.direct()` と、CoS の 6 か所の `audited(..)` の呼び出し。
- `knowledge.rs:34`・`lifecycle.rs:23`・`skills.rs:28` の `use crate::cos::operations::{Applied, OperationAudit};` と、`knowledge.rs:552-554` の完全修飾の参照。
- `skills.rs` の `skill_effect`（CoS 専用の `SkillError` の写し。D1 の効果の 2 段の戻り値が代わる）。`skill_error_to_problem` は D1 の効果と `skills.rs:450` の route が使うので残す。
- 6 関数の doc comment の「handler（`audit = None`）と CoS が共有する」の記述（各 plan・効果・adapter の doc comment に書き直す）。
- 文書: [org-write の ADR](2026-11-18-org-write-shared-use-case.md) の付記「2 つの形の使い分けと撤去の条件」の表から 3 file の行を消し、数を今の値にする
  （WorkUnit `after`）。[architecture-map](../../docs/architecture-map.md) の lifecycle の行（task の一時停止と CoS の腕 `cos/ops/tasks.rs`）は WorkUnit `pause` が直す。
- 残すもの: `Applied` 型・`audited`・`cos/operations.rs`（撤去の条件は D の付記のまま。広い command が `cos/operations.rs` だけになったとき）。

## 8. 不変条件と試験（brief 7）

| 不変条件 | この変更で守ること | 試験 |
|---|---|---|
| `cos-audit-same-tx` | accept・reject の git の効果と `task.pause` の `edit_task_tx` は `cos_operation_apply` の transaction の中。失敗すれば業務の変更と applied の行は残らず、rejected の行が今の文言で残る | `applied_contract_knowledge_accept_cos_*`・`applied_contract_knowledge_reject_cos_*`・`applied_contract_task_pause_cos_*`、既存の `cos_delivery_suite_a::cos_ops_mutations` |
| 外部の効果の 1 回と監査の意味 | page_put・skill の効果は `external` の中で 1 回。etag 不一致・missing・入力の誤りは rejected、git の失敗は pending、再送は効果を走らせない | `applied_contract_knowledge_page_put_cos_*`・`applied_contract_skill_put_cos_*`・`applied_contract_skill_delete_cos_*`、既存の `cos_ops_projects_cron_knowledge_page_put_is_external_once`・`cos_delivery_suite::cos_ops_admin_config` |
| `task-state-in-sqlite` | task の一時停止の真実は SQLite の task の行と `Edited` event。KB は git（今と同じ） | `applied_contract_task_pause_(http\|cos)_*`、既存の `project_plan::task_pause_and_resume_cover_the_subtree`・`tests/lifecycle.rs` |
| `lifecycle-human-epoch-same-tx` | `knowledge.rs` は `transaction` class。この変更は transaction を開く場所を増やさず、HTTP の `update_task`・CoS の `apply` の境界を変えない。案件の操作の `HumanEpoch` の経路は触らない | 既存の `tests/lifecycle.rs`・`tests/knowledge.rs`、`tx_boundary_contract`・`org_write_contract` の試験 |
| `api-principal-from-token` | HTTP の `by` は token の principal（`caller.actor()`）、CoS は `audit.by()`（`cos:<thread_id>`） | `applied_contract_task_pause_http_*`（`Edited` event の actor）・`applied_contract_task_pause_cos_*` |

契約試験の名前は `applied_contract_<op>_(http|cos)_<内容>`、`<op>` は `knowledge_page_put`・`knowledge_accept`・`knowledge_reject`・`task_pause`・
`skill_put`・`skill_delete`。WorkUnit `contract` が旧コードで先に通し、実装の WorkUnit は変えずに通す。各 op で少なくとも次を固定する:
HTTP の成功の status と body、拒否の status と code、CoS の operation の status（applied・rejected・pending のうち作れるもの）と rejected の行の理由の文言、
拒否・失敗の後に業務の変更が残らないこと（rollback）、skill の frontmatter の `source`。既存の試験が同じことを固定していれば、`contract` は
進捗にその試験名を書き、重ねない（規約 10。既存の試験が見ていないのは主に rejected の行の文言と HTTP の `source`・actor）。

## 9. 計測と回帰条件

数え方は H の final-report の再現コマンド節のまま（`metrics.tsv` の key・scope・checker も H の `struct-*.tsv` と同じ文字列）。

| metric | before（H の値） | 目標の after | 減る分 |
|---|---|---|---|
| `applied_wide`（`git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" crates/task-api/src` の file 数） | 18 | 15 | `knowledge.rs`・`lifecycle.rs`・`skills.rs` |
| `applied_narrow`（`git grep -l 'Option<&OperationAudit>' crates/task-api/src` の file 数） | 14 | 11 | 同じ 3 file |
| `applied_functions`（広い command の file で該当の引数を受ける関数。H の補助の awk） | 55 | 49 | 6 関数 |

回帰条件: `applied_wide` ≤ 18。移動・改名・抽象の追加だけ、分母の増加は成果に数えない（この変更は分母の file を増やさない）。

依存の向き（`deps.cycle`）: 新しい依存の組を足さない。`cos/ops/*` → `knowledge`・`lifecycle`・`skills` は既にある組。`knowledge.rs` は
`crate::cos::CosCaller`・`crate::cos::knowledge_use` を使い続けるので `knowledge → cos` の組は残る。`lifecycle.rs`・`skills.rs` は
`crate::cos` への参照が `use crate::cos::operations` の 1 行だけなので、`lifecycle → cos`・`skills → cos` の組は消える見込み（未確認。WorkUnit `after` が C2 で確かめる）。

## 後続 WorkUnit への引き継ぎ

- `contract`: §8 の試験を旧コードで通す。
- `measure`: §9 の 3 指標の before を `metrics.tsv` に置く。
- `knowledge`・`pause`・`skills`: D1〜D3 を自分の file と CoS の腕だけで行う（file が重ならない）。`pause` は architecture-map の lifecycle の行も直す。
  D1 の名前を変えたら進捗に書く。
- `after`: §9 の after を測り、org-write の ADR の付記の表を今の関数に合わせ、進捗を done にする。
