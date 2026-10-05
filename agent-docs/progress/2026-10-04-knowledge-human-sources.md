---
title: 知識ベースの sources の human を『人が書いた』と『人の指示由来』に分け、整理の保護を前者だけにする
tasks: [01M44BFF0EDZZQ4MAPQZB1GARV]
status: done
updated: 2026-10-04
---

# PROGRESS — sources の human の分離

正本: [ADR-0047](../adr/0047-knowledge-base.md) の「付記（2026-10-04、sources の human を『人が書いた』と『人の指示由来』に分ける）」
（H1〜H4）。[ADR-0131](../adr/0131-cron-jobs.md) 末尾に保護の範囲の付記。運用手順は
[docs/ops/knowledge-human-sources-migration.md](../../docs/ops/knowledge-human-sources-migration.md)。

## Phase 1（完了 2026-10-04、単一 phase）

### やったこと

- **印と判定**（task-core `knowledge/human_marks.rs`）: `SOURCE_HUMAN_AUTHORED`（`human:authored`）・
  `SOURCE_HUMAN_INSTRUCTION`（`human:instruction`）・`SOURCE_HUMAN_LEGACY`（`human`）、`human_authored`
  （`author: human` 行も）・`legacy_human`（単数形 `source: human` も）・`protected_page`・
  `normalize_agent_sources`・`replace_sources`（`sources` 行だけを書き換える）。
- **validator**（task-ops `knowledge_curation::human_page`）: `protected_page` に委ねる。保護は `user/`・
  `human:authored`・`author: human`・未判別の旧形だけ。計画の本文が『人が書いた』印を付け外しするときも
  `human_decisions`（理由「人が書いた印の付け外し: …」）。
- **knowledge run の印**: `apply_candidates_in`（直接コミット・`_inbox/` 行きの両方）と `record_in` が
  `normalize_agent_sources` を通す（`human`/`human:authored` → `human:instruction`、task id を添える。対象ページが
  既に持つ印は保つ）。依頼文（`maintenance_objective`・`langmem_run.py`・GC の INSTRUCTIONS）も新しい印を指示。
  `init` の雛形は `human:authored` で書く。GC の「shared source」判定は `human*` 全部を無視。
- **移行**: `task_ops::knowledge::migrate_human_sources(root, apply)` と
  `celerisctl knowledge migrate-human-sources [--apply] [--json]`（既定 dry-run）。git 履歴の author と件名で判別。

### 証拠

- 受け入れ 0（ADR・保護の試験）: ADR-0047 付記 H1〜H3。試験 `task-ops knowledge_curation::tests::only_authored_marks_and_user_pages_are_protected`
  （`human:authored`・`author: human`・旧形 `human`・`user/` は `human_decisions`、`human:instruction` だけのページは削除が適用される）、
  `curation_cannot_add_or_drop_the_authored_mark`、`task-core knowledge::tests::human_marks_protect_only_authored_and_legacy_and_user_pages`。
- 受け入れ 1（knowledge run の新しい印）: `task-ops knowledge::human_sources::tests::knowledge_run_marks_human_instruction_on_both_write_paths`、
  `knowledge_run_keeps_an_existing_authored_mark_on_update`、`record_marks_human_instruction`、`maintenance_objective_asks_for_human_instruction`、
  `task-core knowledge::tests::normalize_agent_sources_turns_human_marks_into_instruction`。
- 受け入れ 2（移行台本）: `task-ops knowledge::human_sources::tests::migration_classifies_from_git_history_and_lists_undetermined`
  （一時 KB に author・件名の違う commit を積み、GUI 編集・人の直接 git → authored、run の直接コミット・候補の取り込み・`_inbox/` → instruction、
  雛形・未コミット → undetermined で書き換えない、dry-run は HEAD・ファイル不変、apply は 1 commit、2 回目は commit しない）。
  `cargo nextest run -p task-core -p task-ops -E 'test(human) | …'` → 86 passed。
  本番 KB の写し（`git clone` した一時ディレクトリ。本番には書いていない）で dry-run → 人が書いた 9・指示由来 6・判別できない 3、
  `--apply` → 15 ファイルの `sources` 行だけ変わる 1 commit、2 回目の dry-run は判別できない 3 件だけ。
- 受け入れ 3:
  - `bash scripts/dev/test-parallel.sh` → exit 0、`3903 tests run: 3903 passed (1 slow), 12 skipped`。
    （途中の 1 回は無関係の `task-dispatch dispatcher::tests::planner_budget::review_fail_replan_planner_gets_the_planner_budget` が
    全体負荷で 1 回落ちた。単独 3/3 pass、task-dispatch は未変更、再実行で全 pass）
  - `cargo clippy --workspace -- -D warnings` → exit 0（`--all-targets` も警告なし）。`cargo fmt --all -- --check` → exit 0。
  - 文書検査: `check-doc-links` ok、`check-doc-layout` ok、`check-adr-numbers` ok、`check-architecture-map` OK。

### 未解決事項

- 本番 KB への移行は未実施（人の承認で 1 回。手順書の通り。この版が current になってから `celerisctl` に入る）。
- 判別できない 3 件（`environment/clusters/{fern03,sirius}.md`・`experience/README.md`）は人が印を決めるまで保護のまま（手順書 7）。
- 「Phase K-1 整理」の `Celeris (human)` commit は人の編集として数えた（API の人の編集と区別できないため保護側）。
- task-api の `KnowledgePage.sources` の doc comment（schema に流れる）は旧表記のまま。schema と gui/web 型の再生成を避けた。

### 提案

- （attempt 2 で実装済み）`PUT /knowledge/page` で人が保存したページに `human:authored` を自動で足す。

## Phase 1 attempt 2（完了 2026-10-04、review 差し戻しの修正）

差し戻し: ADR H1 の「`human:authored` は人の GUI 編集（`PUT /knowledge/page`）で付く」が未実装で、移行の後に人が
GUI で直したページ（例: `human:instruction` だけのページ）が保護されなかった。

### やったこと

- task-core `knowledge::mark_human_authored(path, raw)`: 印が無ければ `sources` に `human:authored` を足す（旧形
  `human` は置き換え、`human:instruction`・`task:<id>` は残す）。front matter が無ければ `sources` だけの front matter
  を先頭に足す。`user/` 配下・既に印のあるページ・BOM/CRLF の front matter は原文のまま。冪等。
- task-api `put_page` が保存する本文にこれを通す（author `Celeris (human)` は従来どおり）。schema の変更は無い。
- ADR-0047 付記 H1 に PUT の振る舞いと「GUI で印を外しても次の保存で付く。外すのは直接の git 編集」を明記。
  手順書 7 を合わせた（GUI 保存 = authored、instruction にするのは直接 git）。

### 証拠

- `task-api knowledge::a_gui_edit_marks_the_page_as_human_authored`（一時 KB。新規ページ・`human:instruction` だけの
  ページを PUT すると `human:authored` が付き `protected_page` が真、同じ本文の再保存は `unchanged`）、
  `task-core knowledge::tests::mark_human_authored_adds_the_authored_mark_for_gui_edits`。既存
  `a_page_is_rendered_with_its_front_matter_history_and_etag` は PUT の旧形 `human` が `human:authored` になる期待に更新。
  `cargo nextest run -p task-api --test knowledge` → 8 passed。`cargo nextest run -p task-core -p task-ops -p celerisctl human` → 64 passed。

- 受け入れ 3（attempt 2）: `bash scripts/dev/test-parallel.sh` → exit 0、`3905 tests run: 3905 passed (1 slow), 12 skipped`。
  `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。

### 未解決事項

- 本番 KB への移行と昇格は未実施（attempt 1 と同じ。人の承認で 1 回）。
