---
title: cron-jobs gate — main 再取り込みの要否と close の検査
tasks: [01M3YF3NS2EGTZD2BBWNPG1K28]
status: done
updated: 2026-10-03
---
# cron-jobs gate — main 再取り込みの要否と close の検査

## main の取り込み（attempt 2、2026-10-03）

- 前回の範囲 check が exit 1。原因は `scripts/dev/check-doc-links.sh`（範囲外）への `FOREIGN_DOCS` 1 行追加。
  `crates/task-api/src/cron_jobs.rs` 先頭の doc comment が実在しない `docs/celeris-api-v1.md` を指していたのが元なので、
  参照を実在の `docs/api/cron-jobs.md` に直し、script は merged-main の版へ戻した（`check-doc-links.sh` exit 0）。
- main が `af71d4e9` から `f8a89553a1ad738b3832138a6e62b7eedebb0b2c` に進んでいたので `git merge --no-ff main` で取り込んだ
  （衝突なし）。`sync-main.md` の末尾に `merged-main: f8a89553…` を足した。
- `git merge-base --is-ancestor main HEAD` → exit 0。
- migration: main は `0041_feed_notices` まで、cron は `0046_cron_jobs` のまま後ろ（`git diff --name-status main HEAD -- crates/task-core/migrations` は A 0046 の 1 件だけ）。

## 差分の範囲（基点 merged-main `f8a89553`）

- 計画の範囲 check（web/ 同一 + 許可 path 以外なし）→ exit 0。
- `git diff --name-only f8a89553 HEAD` の上位ディレクトリ: crates 45、gui 11、agent-docs 6、docs 4、config 1、tests 1、
  Cargo.lock 1、scripts 1（`scripts/dev/check-adr-numbers.sh`）。web/ の差分は空。
- main 側の変更の取りこぼし: `git diff --name-status f8a89553 HEAD` で D（削除）は 0 件。

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

修正は doc comment の参照 1 行だけ（上記）。検査は取り込み後の HEAD で全部回し直した。

## 未解決事項

- `cargo test --workspace` の全体実行はこの葉では回していない（ops-verify の記録を参照。flake 1 件は範囲外）。
