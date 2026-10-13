---
title: api-errors — API 文書に default_branch_unresolvable・checkout_busy と取り込み操作の 409 を記載する
updated: 2026-10-11
status: done
tasks: [01M4NM5720BYCYHXT02297GFKK]
---

# api-errors

コードは変えず、`docs/api/v1/api.md` を実装に合わせた。`crates/`・`web/` は差分ゼロ（`git diff --quiet "$CELERIS_WU_BASE" -- crates/ web/` exit 0）。

## 追加・修正した行と根拠

| 箇所 | 内容 | 根拠（実装） |
|---|---|---|
| §1.5 エラー表（`default_branch_busy` の次） | `default_branch_unresolvable` \| 409。`detail` は `"<repo>: <理由>"`、git にも記録にも触らない、`discard` は影響なし | `crates/task-api/src/changes.rs` `RepoTarget::default_branch()`（409・`format!("{}: {error}", self.name)`）。呼び出しは `prepare_integrate`（`discard` 以外）、`run_integrate` の merge と pr、`diff`。`crates/task-api/src/integrations.rs` `mutate_simple` の merge は `resolve_default_branch` の Err を同じコードで返し、`stranded::merge` より前 |
| §1.5 エラー表 | `checkout_busy` \| 409。`detail` は理由の一行。git は触らないが記録は残る（`unintegrated` のまま reason を更新し integration request を 1 件起票） | `crates/task-api/src/integrations.rs` `invalid_problem`（`StoreError::Invalid` の `checkout_busy:` 接頭辞 → 409 `checkout_busy`）と `mutate_simple` の `MergeOutcome::Busy` → 409 `checkout_busy`。`crates/task-ops/src/stranded.rs` `merge`（dirty な既定 branch の checkout で `record_failure` を呼んでから `Busy` を返す。`record_failure` は upsert と `integration_request_record`）。既定 branch が detached/別 branch のときは Busy にならない |
| §2 表 行 73（`GET /integrations?state=`） | state の値 `unintegrated` / `integrated` / `discarded` / `held` / `not_applicable` を明記（値一覧は api.md に他に無かったので、この説明欄に足した） | `crates/task-core/src/integrations.rs` `TaskRepoIntegrationStatus`（`NotApplicable` = `not_applicable`）、`crates/celerisctl/src/commands/integration.rs` の state 検査文言 |
| §2 表 行 75（merge） | 409 の 3 種: `default_branch_unresolvable`、`checkout_busy`、`invalid_transition`（`not_applicable`）。解決が先に走るので、遠隔 repo の not_applicable でも解決できなければ `default_branch_unresolvable` が先に返る | `integrations.rs` `mutate_simple`（Merge の順: 解決 → `stranded::merge`） |
| §2 表 行 76・77・78（discard / hold / reopen） | `not_applicable` の行は 409 `invalid_transition`（遷移不可） | `crates/task-ops/src/stranded.rs` `reopen`（`not-applicable integrations cannot be changed locally`）、`set_disposition`（同文言）。`invalid_problem` は `Invalid` を `invalid_transition` の 409 に写す（`not found` を含むものだけ 404） |
| §3.80（diff） | 既定 branch を解決できないと 409 `default_branch_unresolvable` | `changes.rs` `diff` が `target.default_branch()?` を `file_diff` の基点に使う（行 321） |
| §3.81（integrate） | 既定 branch 解決不能（`merge` と `pr`、`discard` は解決しない）は git 前の 409 `default_branch_unresolvable`。「409 になるのは上の 2 つだけ」を 3 つ（`default_branch_busy`・`pr_unavailable`・`default_branch_unresolvable`）に直した | `changes.rs` `prepare_integrate`（`request.method != Discard` で `default_branch()?`）、`run_integrate` |

## 文書と実装の食い違い（直した）

- §3.81 の「409 になるのは上の 2 つだけ」は、解決不能の 409 を足したので誤りになっていた。3 つに直した。
- 行 75 の merge の説明に、`not_applicable` の行の 409 と `checkout_busy` の記録の扱いを書いた（以前の表にはなかった）。

## 確認したが書かなかったこと

- `POST /tasks/{id}/changes/{repo}/pr/merge`（§3.82）は既定 branch を使わないので、`default_branch_unresolvable` は出ない。
- `docs/api/v1/api-v1.schema.json` は `not_applicable` を既に含む（`grep -c` 4 件）。生成物なので変更していない。

## 検査

- `sh scripts/dev/check-doc-links.sh docs/api/v1/api.md` → `check-doc-links: ok`（exit 0）
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` → `check-doc-layout: ok`（exit 0）
- `grep -c "default_branch_unresolvable\|checkout_busy" docs/api/v1/api.md` → 6
- `git diff --quiet "${CELERIS_WU_BASE:-HEAD}" -- crates/ web/` → exit 0

## 未解決・提案

- 行 75 の `not_applicable` merge は、解決できない遠隔 repo では `default_branch_unresolvable` が先に返る。文書はそのとおり書いた。順序を変える（`not_applicable` を先に見る）のは実装の変更なので、この WU では触らない。必要なら別の決定として扱う。
- 文書の検査は `check-doc-links` と `check-doc-layout` だけ。API 文書の表とエラーコードの突き合わせを自動で見る検査は無い。
