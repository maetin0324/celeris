---
title: union 修正後の最新 main 取り込みと workspace ゲート
tasks: [01M3ZCXNXTRTA9GNJ52Q36ZFCP]
status: done
updated: 2026-10-03
---

# union 修正後の最新 main 取り込みと workspace ゲート

merged-main: 0225c752ef0cceca723694d17cbbbcda2ed780a7

## 取り込み

`git fetch origin main` → `origin/main` は `0225c752ef0cceca723694d17cbbbcda2ed780a7`（sync-gate 時点と同じ、main は進んでいない）。`git merge-base --is-ancestor main HEAD` が exit 0 のため main はこのブランチ（HEAD `7effd2f8`、union-scope 済み）の祖先。`git merge-tree --write-tree --name-only HEAD main` も衝突ファイル名を 1 件も出さず（tree を書けた）、merge は不要なのでそのまま main の sha を記録した。衝突マーカーは `git grep -n '^<<<<<<<\|^=======$\|^>>>>>>>' -- crates docs agent-docs gui web scripts` で該当なし。

## 証拠

- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0（警告なし、`Finished dev profile`）。
- `cargo test --workspace` → exit 0。集計（全テストバイナリの `test result:` 行の合計）: passed 3664 / failed 0。断続 flaky は今回の実行では発生せず、単体再実行は不要だった。
- `sh scripts/dev/check-doc-links.sh` → exit 0（`check-doc-links: ok`）。
- `sh scripts/dev/check-adr-numbers.sh` → exit 0（`check-adr-numbers: ok (127 files)`）。
- `sh scripts/dev/progress-index.sh --check` → exit 0（`progress-index --check: ok`）。
- `sh scripts/dev/tests/progress_union_merge.sh` → exit 0（union 範囲が agent-docs/PROGRESS.md と docs/PROGRESS.md の2行に限られていること、front matter の衝突が黙って統合されないことを再確認）。

## 未解決事項

無し。union-scope（commit `7effd2f8`）で直した前回レビューの2点（`.gitattributes` の union 範囲、`artifacts/review.json` の誤追跡）はこの取り込み後の tree でも再発していないことを確認した。
