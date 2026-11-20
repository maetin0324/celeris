---
title: アーキ v4 H R5 knowledge — knowledge.rs の accept・reject・page_put を plan と効果に分け、CoS の包みを cos/ops/decisions.rs に置く
tasks: [01M7VJY8NZ5G84ACBNQTPVV3R4]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# knowledge（WorkUnit）

完了日: 2026-11-20

## 0. 結論

- `crates/task-api/src/knowledge.rs` の `put_page_op`・`accept_op`・`reject_op`（`Option<&OperationAudit>` を受けて `Applied` を返す形）を消し、
  ADR 2026-11-20 applied-shape-migration D1 の plan と効果に分けた。`knowledge.rs` から `use crate::cos::operations::{Applied, OperationAudit};` と
  `put_page_op` の完全修飾の参照（`crate::cos::operations::…`）が消えた。
- HTTP の `put_page`・`accept`・`reject` は plan → 効果 → tracing の順に呼ぶ（D2）。`.direct()` は使わない。
- CoS の `knowledge.page_put`・`knowledge.accept`・`knowledge.reject`（`crates/task-api/src/cos/ops/decisions.rs`）は plan の拒否を `audit.reject`、
  効果を `audit.external`（page_put）・`audit.apply`（accept・reject）で包む（D3）。`audited(crate::knowledge::…)` は使わない。
- 変えた file は `crates/task-api/src/knowledge.rs`・`crates/task-api/src/cos/ops/decisions.rs` とこの note だけ。`crates/task-api/tests/`・
  `cos/operations.rs`・policy の file は変えていない。`record_candidate`・`pin_candidate_tx`・`discard_candidate` と `knowledge.record` の腕も変えていない。

## 1. 実装前 brief（要点）

1. 根本原因: 3 関数が呼び出し元（HTTP か CoS か）で `audit` の有無の腕に分かれ、`reject_op` は `InboxOutcome` の写しを 2 回書き、
   HTTP の腕だけの tracing と `cos = audit.is_some()` を共有の関数が持っていた。その分岐のために `knowledge.rs` が CoS の型を import していた。
2. 責務 owner: plan と効果は `knowledge.rs`、監査の包みは CoS adapter の `cos/ops/decisions.rs`。
3. 再利用先: `OperationAudit::{reject, apply, external}`（変えない）、`task_ops::knowledge::{commit_page, reindex, inbox_accept, inbox_reject}`、
   `knowledge.rs` の `require_kb`・`page_path`・`path_problem`・`candidate_not_found`・`etag_mismatch`・`page_not_found`・`knowledge_unavailable`。
4. 選択肢: ADR の選択肢 1（共有の file は plan と効果、包みは adapter）を採った。
5. 新しい状態・分岐・fallback・port: 無し。新しい定数 `CANDIDATE_NOT_FOUND`（`pub(crate)`。`candidate_not_found` と CoS の reject の腕が使う）。
6. 撤去対象: 3 関数、その `audit` の分岐 3 か所、HTTP の `.direct()` 3 か所、CoS の `audited(crate::knowledge::…)` 3 か所、`use crate::cos::operations`、
   旧い形の doc comment（「handler（`audit = None`）と CoS … が共有する」「shared by the handler and CoS」）。
7. 不変条件: ADR §8 の `cos-audit-same-tx`（accept・reject の git の効果は `cos_operation_apply` の transaction の中）、外部の効果の 1 回と監査の意味
   （page_put は `external` の中で 1 回、etag 不一致・missing は rejected、git の失敗は pending）、拒否理由の文言。試験は 3 節。

## 2. 移した後の関数（ADR D1 の名前のまま）

| 旧い関数 | plan | 効果 | HTTP の包み | CoS の包み（decisions.rs） |
|---|---|---|---|---|
| `put_page_op` | `plan_page_put(root, &request) -> Result<PageEdit, ApiProblem>` | `page_put_effect(root, &edit) -> Result<Result<KnowledgePageResult, ApiProblem>, ApiProblem>` | plan → `page_put_effect(..)??` → `tracing::info!(op = "knowledge_put", cos = false, ..)` | `reject("knowledge_page", 正規化の前の input.path)` → `external("knowledge_page", edit.path, "knowledge.page_put")` の中で効果 → 成功なら `tracing::info!(op = "knowledge_put", cos = true, ..)` |
| `accept_op` | `plan_accept(root, &id, &request) -> Result<(), ApiProblem>` | `accept_effect(root, &id, &request) -> Result<KnowledgePageResult, ApiProblem>` | plan → 効果 → `tracing::info!(op = "knowledge_accept", ..)` | `reject("knowledge", id)` → `apply("knowledge.accept")` の中で効果。失敗は problem を控えて `ChatError::Conflict(detail)`、応答は控えた problem |
| `reject_op` | `plan_reject(root, &id) -> Result<(), ApiProblem>` | `reject_effect(root, &id) -> Result<KnowledgeRejectResult, ApiProblem>` | plan → 効果 → `tracing::info!(op = "knowledge_reject", ..)` | `reject("knowledge", id)` → `apply("knowledge.reject")` の中で効果。code が `CANDIDATE_NOT_FOUND` なら `ChatError::NotFound { kind: "knowledge candidate", id }`、ほかは `ChatError::Conflict(detail)` |

行数（`git diff --numstat`、この WorkUnit の base から）: `knowledge.rs` +124 −183、`decisions.rs` +70 −21（合計 +194 −204）。
行数の純減はこの WorkUnit の目標ではない（数えるのは WorkUnit `after` の `applied_wide`・`applied_narrow`）。

## 3. 試験の結果（提出時の HEAD で実行）

| 条件 | コマンド | 結果 |
|---|---|---|
| 0. knowledge.rs に `OperationAudit`・`Applied` の語が無く、cos/ops から `audited(crate::knowledge::…)` を呼ばない | 計画の否定の grep（`git grep -nwE 'OperationAudit\|Applied'`・`'audit = None\|audit\.is_some\(\)\|shared by the handler and CoS'`・`'audited\(crate::knowledge::'`） | exit 0（該当なし） |
| 0'. knowledge.rs に新しい `crate::cos`・`crate::handlers` の path が無い | 計画の path の比較 | exit 0。base は `CosCaller`・`knowledge_use::note`・`operations`・`operations::Applied`・`operations::OperationAudit`、HEAD は `CosCaller`・`knowledge_use::note` だけ |
| 1. `applied_contract_knowledge_` が変更なしで通る | `cargo nextest run --no-tests=fail -p task-api -E 'test(applied_contract_knowledge_)'` | 6 本 pass（accept・reject・page_put × HTTP・CoS） |
| 関係する試験の binary | `cargo nextest run -p task-api --no-tests=fail -E 'binary(=knowledge) \| binary(/^cos_delivery_suite/) \| binary(=cos_knowledge_use) \| test(/tx_boundary_contract\|org_write_contract/)'` | 213 本 pass |
| 2. fmt・clippy | `cargo fmt --all -- --check`・`cargo clippy --workspace -- -D warnings` | どちらも exit 0 |
| 2. test-affected | `bash scripts/dev/test-affected.sh`（対象 celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web の tsc・unit） | exit 0。nextest 6158 本 pass・21 skip、web の unit 705 本 pass、`repo_health_gate: all checks passed` |
| 3. 範囲 | 計画の範囲 check（`$CELERIS_WU_SCOPE_PATHS`） | 範囲外の path なし |

### 反例の確認（CoS の `knowledge.reject` の `NotFound` の写し）

`applied_contract_knowledge_reject_cos_rejections_leave_kb_unchanged` は、知らない候補の rejected の行の `result.error` が `nope` を含むことだけを見る。
`ChatError::NotFound`（表示 `knowledge candidate nope not found`）と `ChatError::Conflict`（表示 `conflict: knowledge candidate not found: nope`）の
どちらでも通るので、新しい code の分岐を試験が守っていない。次の一時的な確認をして、どちらも元に戻した（`git status --short` で src の 2 file だけが差分）:

1. 試験に `assert_eq!(op_row(..,"rej-missing").result["error"], "knowledge candidate nope not found")` を一時的に足し、今のコードで pass した
   （旧コードの `InboxOutcome::Missing` → `ChatError::NotFound` と同じ文言）。
2. 同じ一時的な試験のまま、`decisions.rs` の分岐を `if false && problem.code() == …` に壊すと、`left: "conflict: knowledge candidate not found: nope"` で fail した。

accept の missing は試験が `knowledge candidate not found: nope` を含むことを見るので、`Conflict` の写しは既存の試験が守る。

## 4. ADR との違い

- `plan_page_put` は `request` を参照（`&KnowledgePagePutBody`）で受ける。CoS の腕は `input.path`（正規化の前の path）を `audit.reject` に渡すため、
  request を plan に渡した後も読む。ADR D1 の表の引数の書き方（`request`）からの違いはこれだけで、意味は同じ。
- HTTP の 3 handler の tracing は `state.blocking` の closure の中（効果の直後）に置いた。ADR D2 の順（plan → 効果 → tracing → 200）と同じ。
  `state.blocking` の store は使わない（`_store`）が、git の操作を blocking の thread で走らせるために `blocking` を残した。
- CoS の `knowledge.page_put` の tracing の `path` は `result.path`（= `edit.path`）。
- 定数の名前は `CANDIDATE_NOT_FOUND`（ADR は「`pub(crate) const`」とだけ書く）。WorkUnit `after` が付記の表に書くなら、この名前を使う。

## 5. 未解決事項

- 3 節の反例の確認のとおり、CoS の `knowledge.reject` の transaction の中の拒否の文言（`NotFound` の表示）は、どの試験も完全一致では見ていない。
  この WorkUnit は `crates/task-api/tests/` を変えられないので試験を足していない。

## 6. 提案

- `applied_contract_knowledge_reject_cos_rejections_leave_kb_unchanged` の `assert_apply_rejected(.., "knowledge.reject", "nope")` の第 4 引数を
  `"knowledge candidate nope not found"` にする（`NotFound` の写しを守る。3 節の 1 で今のコードが通ることを確かめた）。担当は WorkUnit `after` か後続の task。
