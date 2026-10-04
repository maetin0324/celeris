---
title: drop-symlink: docs/progress の symlink を消し、main 90e3ca26 を取り込む
tasks: [01M41QGGB7XGFEH4ARE7XJJ38E]
status: done
updated: 2026-10-03
---

# drop-symlink 修復記録

完了日: 2026-10-03

## 実施内容

- `git rm docs/progress/review-sync-main.md` で symlink を削除した。`docs/progress/` の追跡ファイルは `README.md` のみ。
- `agent-docs/progress/2026-10-03-review-sync-main.md` 内の旧 path 参照を除去し、正本の場所への参照に更新した。末尾の `merged-main` は `90e3ca26ce1412e0cc2d4fcdc4613717df12973a`。
- `agent-docs/progress/2026-10-03-records-fix/move-records.md` に symlink を drop-symlink で削除したことを記録した。
- `git merge --no-ff main`（main = `90e3ca26`）で統合した（HEAD `13cb6990`）。`docs/architecture-map.md` の衝突では新しい ownerless-runs 行と既存 orphan-run 行の両方を保持した。
- `agent-docs/progress/2026-10-03-ownerless-running-runs.md` は main 由来の文書で front matter が 1 行目に無かったため、title・tasks・status・updated を先頭へ移した（`progress-index.sh --check` の指摘への対応）。

## 検査

| コマンド | 結果 | exit |
|---|---|---:|
| `test ! -e docs/progress/review-sync-main.md && test ! -L … && test "$(git ls-files docs/progress)" = docs/progress/README.md && ! grep -q 'docs/progress/review-sync-main' agent-docs/progress/2026-10-03-review-sync-main.md && test -s agent-docs/progress/2026-10-03-records-fix/drop-symlink.md` | 成功 | 0 |
| `sh scripts/dev/check-doc-links.sh` | `check-doc-links: ok` | 0 |
| `sh scripts/dev/check-adr-numbers.sh` | `check-adr-numbers: ok (134 files)` | 0 |
| `sh scripts/dev/progress-index.sh --check` | `progress-index --check: ok` | 0 |
| `python3 scripts/dev/check-architecture-map.py` | `OK: 232 件のパスを確認した` | 0 |
| `git merge-base --is-ancestor 90e3ca26ce1412e0cc2d4fcdc4613717df12973a HEAD` | 祖先 | 0 |
| `git grep -n '^<<<<<<< \|^>>>>>>> ' -- .` | 衝突マーカーなし | 1 |
| `cargo clippy --workspace -- -D warnings` | `Finished dev profile` | 0 |

`cargo test --workspace` はこの WorkUnit では走らせていない（統合の段で走る）。コードは変えていない。

## 未解決・提案

- なし。
