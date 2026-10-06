---
tasks: [01M44H0SRV70E32AQ6C5N37MSK]
status: done
updated: 2026-10-06
---
# main（46f985c5 以降）の取り込みと全体検査

## 経緯

前の葉 `main-sync-3` は `scripts/dev/progress-index.sh --check` が main 由来の 2 file（`agent-docs/progress/2026-10-05-web-artifact-viewer.md`、`agent-docs/progress/2026-10-06-web-tabbar-first-screen.md`）の front matter 不備で blocked した。その後 Fable が main `d1e6a35f` でこの 2 file を修正し、`46f985c5` で gui の mobile-audit（`TaskRoutingPanel` の表の overflow-x-auto 包み、task-tabs の touch scroll）も直した。本葉はこの `46f985c5` 以降を取り込み、全体検査を記録する。また前回の `main-sync-4`（同じ objective）は daemon 再起動で孤児になったため key を変えて `main-sync-5` として置き直した。

## 手順と結果

1. `git merge --ff-only celeris-wu/01M44H0SRV70E32AQ6C5N37MSK/main-sync-3`
   - fast-forward: `da1ae386..4d07e2a7`（`crates/celeris/tests/instance_handoff.rs` の修正のみ、main `67abf760` 由来）
2. `git merge --no-ff --no-edit main`
   - main 先端 `46f985c5` を merge。コミット `19638c2e`。衝突なし。取り込んだ差分: `agent-docs/progress/2026-10-05-web-artifact-viewer.md`・`agent-docs/progress/2026-10-06-web-tabbar-first-screen.md`（front matter 修正）、`gui/app/app.css`・`gui/app/components/RoutingSourceState.tsx`・`gui/app/components/TaskRoutingPanel.tsx`（mobile-audit 修正）。
3. `git diff main -- crates/celeris/tests/instance_handoff.rs | wc -l` → `0`（main と差分ゼロ）
4. `bash scripts/dev/test-parallel.sh` → exit 0。`CELERIS_TEST_SUMMARY {"passed": 4058, "failed": 0, "ignored": 14, "nextest_exit": 0, "doctest_exit": 0}`
5. `cargo clippy --workspace -- -D warnings` → exit 0（警告なし）
6. `bash scripts/dev/check-doc-links.sh` → `check-doc-links: ok`（exit 0）
7. `bash scripts/dev/check-adr-numbers.sh` → `check-adr-numbers: ok (143 files)`（exit 0）
8. `bash scripts/dev/progress-index.sh --check` → `progress-index --check: ok`（exit 0、main-sync-3 が blocked した 2 file の指摘は main `d1e6a35f` の修正で解消済み。本葉の時点で既知の指摘は無い）

## 取り込んだ sha

- HEAD（merge 後）: `19638c2e`
- main 先端: `46f985c5`
- 経由した main 側 commit: `67abf760`（instance_handoff の出来事待ち化）→ `d1e6a35f`（進捗 2 file の front matter 修正）→ `46f985c5`（gui mobile-audit 修正）

## 未解決事項

なし。main `67abf760` 以降が HEAD の祖先になり、`instance_handoff.rs` は main と差分ゼロ、test-parallel・clippy・doc-links・adr-numbers・progress-index の全検査が exit 0。本番の config・daemon は変更していない。
