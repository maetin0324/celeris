---
title: アーキ v4 H R5 adr — 6 関数を org_write の形へ移す設計（ADR applied-shape-migration）
tasks: [01M7VJY8NZ5G84ACBNQTPVV3R4]
status: done
updated: 2026-11-20
completed: 2026-11-20
---
# adr（WorkUnit）

完了日: 2026-11-20

## 変更

- `arch-v4 H R5 adr: …`（件名で参照。hash は書かない）— [ADR 2026-11-20 applied-shape-migration](../../adr/2026-11-20-applied-shape-migration.md) を足した
  （状態 Accepted。節: 根本原因・owner・再利用先・D1〜D5 の決定と 6 op の表・選択肢・新しい状態・撤去対象・不変条件と試験・計測と回帰条件・引き継ぎ）。
- task の進捗 `agent-docs/progress/2026-11-19-arch-v4-h-r5.md` と `metrics.tsv` は WorkUnit `measure` が置く（この WorkUnit は作らない）。
- `crates/`・`scripts/`・`web/` は変えていない。

## 要点

- 移し先: 共有の file（`knowledge.rs`・`lifecycle.rs`・`skills.rs`）には plan（`ApiProblem` で拒否）と効果だけを置く。HTTP adapter は plan → 効果、
  CoS adapter（`cos/ops/decisions.rs`・`tasks.rs`・`admin.rs`）は plan の拒否を `audit.reject`、効果を今と同じ包み（accept・reject は `apply`、
  page_put と skills は `external`、task_pause は `apply`）で包む。
- objective との差: task_pause の CoS は objective の文面の `apply_checked` ではなく、今と同じ `apply` を残す。`apply_checked` にすると
  `edit_task_tx` の失敗の監査の理由が `invalid: …` に、応答の写しが `store_problem` に変わり、「拒否理由の文言を変えない」に反する
  （`cos_operation_apply` は `ChatError` の表示文を理由に書く。`crates/task-core/src/chat/operations.rs`）。同じ transaction の性質はどちらでも同じ。
- `knowledge.reject` の CoS の写し: missing は `ChatError::NotFound { kind: "knowledge candidate", id }`、ほかは `Conflict(detail)`。共有の効果は
  `ApiProblem` を返し、CoS の腕が `candidate_not_found` の code（`knowledge.rs` の定数）で写し分ける。
- 外部の系の効果（page_put・skill_put・skill_delete）は `Result<Result<T, ApiProblem>, ApiProblem>`（内側 = 断った、外側 = 結果が分からない）にそろえ、
  `skills.rs` の `skill_effect` を消す。
- tracing `op = "knowledge_put"` の `cos` は adapter の定数（HTTP `false`・CoS `true`）。skill の `source` は adapter が渡すデータ（HTTP `gui`・CoS `cos`）。
- 目標: applied_wide 18→15、applied_narrow 14→11、applied_functions 55→49。回帰条件 applied_wide ≤ 18。
- 依存: 新しい組を足さない。`lifecycle → cos`・`skills → cos` は消える見込み（未確認。`after` が C2 で確かめる）、`knowledge → cos` は `CosCaller` で残る。

## 棚卸しの方法

- `git grep -n 'accept_op\|reject_op\|put_page_op\|task_pause_op\|put_skill_op\|delete_skill_op'` で 6 関数と CoS の呼び出し元 6 か所を出した
  （`docs::put_page_op` は別 file の別の関数で対象外）。
- 監査の理由の文言: `OperationAudit::reject`（`cos/operations.rs:325`）と `cos_operation_apply` の rejected の行（`error.to_string()`）を読んだ。
- `crate::cos` の参照: 3 file に `git grep -n 'crate::cos'` → `knowledge.rs` は operations のほかに `CosCaller`・`knowledge_use`、`lifecycle.rs`・`skills.rs` は operations の 1 行だけ。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `git grep -lE "Option<&('[a-z_]+ )?(crate::cos::operations::)?OperationAudit>" crates/task-api/src | wc -l` → 18。
  `git grep -l 'Option<&OperationAudit>' crates/task-api/src | wc -l` → 14（H の値と同じ）。

## 検査

検査の結果は下に追記する。
- `sh scripts/dev/check-doc-links.sh <ADR と adr.md>` → exit 0（`check-doc-links: ok`）。`sh scripts/dev/check-adr-numbers.sh` → exit 0（`ok (468 files)`）。
  `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。
- `git diff --name-only $CELERIS_WU_BASE -- crates scripts web | wc -l` → 0。`sh "$CELERIS_WU_SCOPE_PATHS"` → ADR と adr.md の 2 path だけ。
- `cargo fmt --all -- --check` → exit 0。`bash scripts/dev/test-affected.sh` → exit 0（`試験対象の変更なし（docs のみ等）`、repo-health は verdict=pass・new 0・worsened 0）。
- `cargo clippy --workspace -- -D warnings` は実行していない（`crates/` を変えていないため。実装の WorkUnit と統合で走る）。
