---
title: cron-jobs gate — main 再取り込みの要否と close の検査
tasks: [01M3YF3NS2EGTZD2BBWNPG1K28]
status: done
updated: 2026-10-03
---
# cron-jobs gate — main 再取り込みの要否と close の検査

## main の取り込み

- 手元の `main` = `af71d4e9d0e2705e692d644564bcd676b83f0e3e`。`sync-main.md` の最後の `merged-main:` 行と同じなので、
  再取り込みはしていない（新しい `merged-main` 行も足していない）。
- `git merge-base --is-ancestor main HEAD` → exit 0（HEAD `93abbd39` は main を祖先に含む）。

## 差分の範囲（基点 merged-main `af71d4e9`）

- `git diff --name-only af71d4e9 HEAD` の上位ディレクトリ: crates 45、gui 11、agent-docs 5、docs 4、scripts 2、
  config 1、tests 1、Cargo.lock 1。
- `git diff --stat af71d4e9 HEAD -- web/` → 空（web/ は main と同一）。
- scripts/ の 2 件は `scripts/dev/check-adr-numbers.sh` と `scripts/dev/check-doc-links.sh`。後者は objective の
  範囲の列挙に無いが、docs-move 葉（`34d5edcc`）が `FOREIGN_DOCS` に `crates/task-api/src/cron_jobs.rs` を 1 行足した
  だけ（利用側リポジトリの API 文書参照を live link と誤認しないため。docs-move.md 参照）。
- main 側の変更の取りこぼし: `git diff --name-status af71d4e9 HEAD` で D（削除）は 0 件。変更（M）は cron 機能の追加と、
  sync-main.md に記録した衝突解決（config・migration 版数・`knowledge_maint` から cron への移し替え）に限られる。

## 検査（exit code と試験数）

| コマンド | exit | 試験数 |
|---|---|---|
| `cargo fmt --all -- --check` | 0 | — |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | — |
| `cargo test -p task-core cron` | 0 | 18 passed / 0 failed |
| `cargo test -p task-ops cron` | 0 | 14 passed / 0 failed |
| `cargo test -p task-ops inbox` | 0 | 46 passed / 0 failed |
| `cargo test -p task-ops attention` | 0 | 7 passed / 0 failed |
| `cargo test -p task-dispatch cron` | 0 | 2 passed / 0 failed |
| `cargo test -p task-api --test cron_jobs` | 0 | 7 passed / 0 failed |
| `cargo test -p celerisctl cron` | 0 | 9 passed / 0 failed |
| `cargo test -p celeris cron` | 0 | 7 passed / 0 failed |
| `cargo test -p celeris knowledge_maint` | 0 | 5 passed / 0 failed |
| `cargo test -p celeris knowledge_curation`（知識整理 job） | 0 | 16 passed / 0 failed |
| `cargo test -p celeris --test daily_curation`（1 日 1 件の要約） | 0 | 3 passed / 0 failed |
| `cargo test -p task-ops knowledge_curation` | 0 | 10 passed / 0 failed |
| `cargo test -p task-api --lib schema`（schema drift 検査） | 0 | 3 passed / 0 failed |
| `cargo test -p task-api --test docs` | 0 | 11 passed / 0 failed |
| `cd gui && pnpm install --offline --frozen-lockfile && pnpm typecheck` | 0 | — |
| `cd gui && pnpm test` | 0 | 89 files / 1276 passed |
| `sh scripts/dev/check-doc-links.sh` | 0 | — |
| `sh scripts/dev/check-adr-numbers.sh` | 0 | — |
| `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | 0 | — |
| `python3 scripts/dev/check-architecture-map.py` | 0 | — |
| `sh scripts/dev/progress-index.sh --check` | 0 | — |

修正は不要だった（この葉でのコード変更なし）。

## 未解決事項

- `cargo test --workspace` の全体実行はこの葉では回していない（ops-verify の記録を参照。flake 1 件は範囲外）。
