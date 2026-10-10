---
title: "reclose: 統合後 HEAD の全体 gate 取り直し（Live View v9）"
tasks: [01M4JAK3MY5G1K8T66Q4RTWSS5]
status: done
updated: 2026-10-10
---

# 全体 gate の取り直し（統合後 HEAD）

## 対象

- base: `651a36cb`（integrate wu/docs-v9 (phase refix)）。ここから作業ツリーに変更なし（検査のみ）。
- protocol: Live View は `LIVE_FRAME_PROTOCOL = 9`、artifact transfer は `ARTIFACT_PROTOCOL = 8`（`crates/task-worker/src/browser_launcher/protocol.rs`）。版の正は ADR 付記 2026-10-10c。

## 実行結果

| 検査 | 実行したコマンド | 結果 |
|---|---|---|
| 全体試験（release 相当 env なし） | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | exit 0。nextest 5067 run / 5067 passed / 13 skipped（167 binaries）。doc tests 全 pass（task_core の compile fail 3 件含む）。`CELERIS_TEST_SUMMARY`: passed 5070 / failed 0 / ignored 14、`userns: null`（release env を付けていないため未判定）。 |
| 全体試験（release gate と同じ env） | `TMPDIR=/tmp CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` | exit 0。passed 5070 / failed 0 / ignored 14、`userns: true`。Operation not permitted の失敗 0 件。 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0（警告なし） |
| fmt | `cargo fmt --all -- --check` | exit 0（差分なし） |
| web test | `corepack pnpm@12.6.0 -C web test`（script は `vitest run && node --test server/*.test.mjs`） | exit 0。Vitest 91 files / 657 tests passed。Node test 86 pass / 0 fail。 |
| web lint | `corepack pnpm@12.6.0 -C web exec biome check .` | exit 0。warning 4 件（`web/styles.css` 220–223 行の `noImportantStyles`）。エラーなし。 |
| 文書検査 1 | `sh scripts/dev/check-doc-links.sh docs/ops` | exit 0（`check-doc-links: ok`） |
| 文書検査 2 | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0（`check-doc-layout: ok`） |
| 文書検査 3 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（`check-adr-numbers: ok (178 files)`） |

ログ: `artifacts/test-parallel.log`、`artifacts/test-parallel-release-env.log`、`artifacts/clippy.log`、`artifacts/fmt.log`、`artifacts/web-test.log`、`artifacts/web-lint.log`。

## 補足

- test-parallel は `tests left 2 entries in TMPDIR (removed on exit)` の warning を 2 回出した。exit 0 で、終了時に削除された。
- 失敗した試験名はなし。範囲内の修正も不要だった（merge 崩れ由来の compile error は merge-fix 段で直済み）。
- web lint の `!important` 4 件は既存の警告で、この unit では触っていない（exit 0 のため gate は通る）。

## 未解決事項

- 実 Chrome と本番 launcher での映像確認は未実施。手順は `docs/ops/browser-launcher-live-view.md` にある（人の運用）。
