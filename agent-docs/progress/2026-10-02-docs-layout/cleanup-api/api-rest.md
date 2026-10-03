---
title: docs 再構成 — gui-api.md §4〜§10 を実装と照らして直し、経緯の節を畳む（cleanup-api / api-rest）
tasks: [01M3YNFVFZG5AFFM0RS05X22ZT]
status: done
updated: 2026-10-02
---
# docs 再構成 — gui-api.md §4〜§10 を実装と照らして直し、経緯の節を畳む（cleanup-api / api-rest）

完了日: 2026-10-02。担当は `docs/api/v1/gui-api.md` の §4 以降だけ（§1〜§3 は兄弟 leaf）。crates/・web/・gui/・scripts/・
CLAUDE.md・.claude/・`scripts/dev/docs-layout.tsv`・`*.schema.json` は読んだだけで変えていない。

## 処理した文書

| 処理(削除/統合/移動/修正) | 旧パス | 新パス or - | 理由 | 最後の commit |
|---|---|---|---|---|
| 修正 | `docs/api/v1/gui-api.md` §4〜§10 | - | sse.rs・types.rs・schema.rs・task-ops の計算と照らして食い違いを直し、手書きの Rust 型の写し（旧 §6.2、約 600 行）を削って型の出所の索引にした。§9『未決・確認事項』を本文へ畳み、§10 の経緯の題を外した | `d9524c0f` |

## 直した点

- **§4 SSE**（`crates/task-api/src/sse.rs`・`lib.rs` の定数）: `reset.reason` に `cursor_ahead` を追加（旧文は `cursor_too_old` だけ）。`daemon` は値が `null` の間は送らない。1,000 件読めたら追いつくまで続けて読む。クエリは `after_id` / `task_id` だけ。定数名（`MAX_STREAMS` ほか）を併記。`/console/stream` は §3.99 へ案内。
- **§5 派生値**: 関数の経路を実在のもの（`task_ops::inbox::inbox` / `view::runs` / `view::timers(…, ctx: &ViewContext, …)` / `view::actions`・`actions_with_events` / `derive::latest_question`）に直した。
  - 5.1: `browser_waits` / `decisions` 区画、`counts.by_status`、attention の `phase_checkpoint` / `plan_approval` / `delivery_skipped` と `failed.class` / `delivered_release`、approvals の `knowledge_pages` と `ApprovalArtifact`（flatten）、questions の除外条件と `approval_id`、案件計画の提案グループを追加。
  - 5.2: `continued`（`continue: ` / `end` の Yielded・BudgetExhausted）と `infra_requeue: ` を追加。
  - 5.4: `rereview` / `phase_gate` / `plan_gate`（`actions_with_events`）を追加し、`TaskRef` / `TaskSummary` は `actions(task)` だけと正した。
  - 5.7: `parent_cancelled` を追加。5.8: 走査は起動時ではなく最初の `GET /providers`（`/accounts`）からの遅延（旧文は SSE ループで更新と誤記）。
- **§6 型**: 旧 6.1 の Phase 別の表と旧 6.2 の手書き Rust 定義（実装と大きくずれていた）を削り、ファイル別の型の出所の索引と表記の約束にした。欄の正は `api-v1.schema.json` と Rust の定義。見出し `6.2` は `scripts/dev/source-size-report.toml` が参照するので残した。
- **§7**: event.schema.json の一致試験が task-core の `store/tests.rs` であること、`schema_uses_defs_once_for_shared_types`、`GET /schema` が `include_str!` の写しを返すこと、web/ の `gen-types.mjs`（`web/api/generated/`、`--check`）と `scripts/sync-gui-docs.sh` を追加。G0 の draft07 切替提案など古い記述を削除。
- **§8**: 11 項目の試験観点（agent 向け）を削り、試験の置き場所と `GET /tasks/{id}` の byte 一致の契約だけ残した。
- **§9 削除**: 決定済み 6 件を本文へ: 1→§5.2/§5.8（reviewer run を集計に含む）、2→§5.4（一括承認 API 無し）、3→§10（`q` は title・objective・コメント、ADR-0014 D2 / ADR-0044 D4）、4→削除（`CooldownView.reason` は必須の文字列）、5→§10（`/health` は無認証）、6→§5.6（空白・存在の検査、ADR-0014 D3）。
- **§10** を「要求の検査と細部の挙動」に改題: Origin は POST/PUT/PATCH/DELETE、Content-Type・サイズは POST/PUT/PATCH。ファイルの `offset > size` は 416、`Range` と `offset` の併用は 400、複数範囲等は 416、クエリは `offset` / `length` / `download` だけ。rereview は review_fail の failed も受け、`expected_status` は任意の Status（不一致 409）。delivery に `pushed_at` / `push_error`、routing の `runs[]` に `task_id` / `run_id`。§10 の番号は §3（兄弟の範囲）の参照「§10」を壊さないため残した（§9 は欠番）。

## 証拠

- `grep -nE '^#+ .*(未決|Phase 9b)' docs/api/v1/gui-api.md` → 出力なし（exit 1）。
- `grep -n '^## ' docs/api/v1/gui-api.md` → §4 SSE、§5、§6、§7、§8 試験の置き場所、§10 要求の検査と細部の挙動。
- `sh scripts/dev/check-doc-links.sh` → exit 1（43 件）。gui-api.md の行は 0 件。残りは他の文書（`docs/ops/web-parallel-operation.md`、`gui/docs/celeris-api-v1.md` の overview.md など）で、この leaf の範囲外。
- 照合は sse.rs・lib.rs・schema.rs・types.rs・task-ops の inbox.rs / view.rs / derive.rs / gate.rs・task-api の stats.rs / middleware.rs / files.rs を読んで行った（file:line は作業中に確認）。
- `git diff --stat` → `docs/api/v1/gui-api.md | 829 +++---…`（106 行追加・723 行削除）。この記録以外のファイルは変えていない。
- cargo test / clippy は実行していない（変更は Markdown だけで、crates から gui-api.md を `include_str!` / 読み込みしている箇所は無い: `grep -rn 'gui-api.md' crates --include=*.rs | grep -E 'include_str|read'` → 0 件）。

## 未解決

- `gui/docs/celeris-api-v1.md`（`scripts/sync-gui-docs.sh` の写し）は gui/ を触れないので更新していない。`sync-gui-docs.sh --check` はずれを報告する（兄弟 leaf の変更でも同じ）。
- §9 が欠番になる。

## 提案

- land-verify か gui/ を触れる task で `scripts/sync-gui-docs.sh` を走らせて写しを更新する。旧 gui/ を廃止するなら写しと script ごと退役させる。
- §3 の「§10」参照（3.39〜3.41 の近く）を直せる統合段で、§10 を §9 に詰める。
