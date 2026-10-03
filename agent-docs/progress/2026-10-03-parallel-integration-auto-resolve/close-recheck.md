---
title: 差し戻し 2 点の解消を最新 main 上で再確認し、ゲート結果と対応表を記録する
tasks: [01M3ZCXNXTRTA9GNJ52Q36ZFCP]
status: done
updated: 2026-10-03
---

# 差し戻し 2 点の解消を最新 main 上で再確認し、ゲート結果と対応表を記録する

merged-main: 0225c752ef0cceca723694d17cbbbcda2ed780a7

実装は足していない。確認と記録だけの葉（ADR-0079 D4 の unit close-recheck）。

## 取り込み

- `git merge-base --is-ancestor main HEAD` → exit 0。main（`0225c752`）は HEAD（`f4b1d28d`）の祖先のため、`git merge --no-ff main` は不要だった。
- `git merge-base --is-ancestor 0225c752 HEAD` と `git rev-parse main` → 同じ `0225c752ef0cceca723694d17cbbbcda2ed780a7`。
- 取り込みによる衝突はない（取り込み対象が無い）。

## 差し戻し 2 点の対応表

| # | 指摘 | 修正 commit | 確認コマンド | 結果 |
|---|---|---|---|---|
| 1 | `.gitattributes` の `merge=union` が `agent-docs/progress/**/*.md` にも効き、front matter の両側変更（`status:`・`updated:`）が黙って統合される | `7effd2f8`（`.gitattributes` を 2 行に限定） | `grep -c 'merge=union' .gitattributes` → 2。`git check-attr merge` で `agent-docs/progress/…md`・`agent-docs/progress/x/y.md`・`crates/a.rs`・`docs/api/v1/a.schema.json` → いずれも `unspecified`。`sh scripts/dev/tests/progress_union_merge.sh` → exit 0 | 解消。front matter 衝突は黙って統合されず、union は凍結 PROGRESS 2 本だけ |
| 2 | `artifacts/review.json`（Celeris の判定材料）が commit `e49b57cb` で repo に追跡されていた | `7effd2f8`（`git rm --cached artifacts/review.json`、`.gitignore` に `/artifacts/`） | `git ls-files artifacts` → 空。`git ls-files \| grep -c 'artifacts/review.json'` → 0。`git check-ignore -v --no-index artifacts/review.json` → `.gitignore:40:/artifacts/` | 解消。追跡されず、無視規則で再発を防いでいる |

## 属性と union の範囲（criterion 1）

- `.gitattributes` の `merge=union` は `agent-docs/PROGRESS.md` と `docs/PROGRESS.md` の 2 行だけ。
- `git check-attr merge` の出力（対象 4 件）はすべて `unspecified`。agent-docs/progress の進捗ファイル・crates の .rs・docs の生成物（schema.json）に merge 属性は付いていない。
- `sh scripts/dev/tests/progress_union_merge.sh` → exit 0。出力の要点: `OK: .gitattributes limits merge=union to agent-docs/PROGRESS.md and docs/PROGRESS.md`、`OK: git merge kept both sections` が 2 件、`OK: front matter conflict is not silently merged`、`progress_union_merge: all checks passed`。同じ試験を 2 回目に単独で再実行して exit 0。

## ゲート結果（criterion 2）

| 検査 | コマンド | exit | 要点 |
|---|---|---|---|
| 書式 | `cargo fmt --all -- --check` | 0 | 差分なし |
| 静的解析 | `cargo clippy --workspace -- -D warnings` | 0 | `Finished dev profile`、警告なし |
| 試験 | `cargo test --workspace` | 0 | `test result` 行の集計: passed 3664 / failed 0 / ignored 13。断続 flaky は出ず、単体の再実行は不要 |
| 文書 | `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok` |
| 文書 | `sh scripts/dev/check-adr-numbers.sh` | 0 | `check-adr-numbers: ok (127 files)` |
| 文書 | `sh scripts/dev/progress-index.sh --check` | 0 | `progress-index --check: ok` |
| 衝突 | `git grep -nE '^(<<<<<<<\|>>>>>>>) ' -- crates config scripts docs gui web .gitattributes` | 1（一致なし） | 衝突マーカーなし |

補足: `cargo test --workspace` のログには `Automatic merge failed` の行が出るが、これは `auto_resolve::renumber` 試験が一時 repo で衝突を作った際の出力で、試験は pass している。作業ツリーは試験後も clean のまま（`git status --short` が空）。

## 範囲（criterion 3・4）

- この葉が commit する変更は `agent-docs/progress/2026-10-03-parallel-integration-auto-resolve/close-recheck.md` と親の進捗ファイル `agent-docs/progress/2026-10-03-parallel-integration-auto-resolve.md` の 1 行追記だけ。
- `agent-docs/PROGRESS.md` は変更していない（`git diff --name-only f4b1d28d -- agent-docs/PROGRESS.md` → 空）。
- main 取り込み分はなし（main は既に祖先）。

## 未解決事項

無し。

## 提案

無し（本葉は確認と記録のみ）。
