---
title: "reclose-2: adr-note 統合後 HEAD の全検査（Live View protocol 9）"
tasks: [01M4JAK3MY5G1K8T66Q4RTWSS5]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

tested_sha: 7ce751166b49b3e7c1b65bcc9488f6ef37561cf9

# 全検査の取り直し（tested_sha）

## 対象

- tested_sha: `7ce751166b49b3e7c1b65bcc9488f6ef37561cf9`（integrate wu/adr-note (phase record)。作業開始時の HEAD）。
- この commit の時点で作業ツリーは clean。検査はコード・ADR を一切変えずに流した。
- protocol: Live View は `LIVE_FRAME_PROTOCOL = 9`、artifact transfer は `ARTIFACT_PROTOCOL = 8`。決定の記録は ADR 付記 2026-10-10d（[ADR](../../adr/2026-10-10-browser-launcher-live-view-frames.md)）。

## 実行結果

| 検査 | 実行したコマンド | exit | 結果 |
|---|---|---|---|
| 全体試験（release gate と同じ env） | `TMPDIR=/tmp CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` | 0 | `CELERIS_TEST_SUMMARY`: passed 5090 / failed 0 / ignored 14、nextest 167 binaries・doc 10 binaries、`userns: true`。Operation not permitted の失敗 0 件。 |
| 全体試験（release env なし） | `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` | 0 | `CELERIS_TEST_SUMMARY`: passed 5090 / failed 0 / ignored 14、`userns: null`（release env を付けていないため未判定）。 |
| clippy | `cargo clippy --workspace -- -D warnings` | 0 | 警告・エラーなし。 |
| fmt | `cargo fmt --all -- --check` | 0 | 差分なし。 |
| web install | `corepack pnpm@12.6.0 -C web install --prefer-offline` | 0 | 依存は offline store から解決。 |
| web test | `corepack pnpm@12.6.0 -C web test` | 0 | Vitest 91 files / 657 tests passed。node --test 86 pass / 0 fail。 |
| web lint | `corepack pnpm@12.6.0 -C web lint` | 0 | 4 warnings（`web/styles.css` の `noImportantStyles`、既存）、エラーなし。 |
| Live View e2e（2 spec） | `TMPDIR=/tmp WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/browser/live-view-frames.spec.ts e2e/browser/live-view-disabled.spec.ts` | 0 | 4 passed（frames 1、disabled 3）。 |
| 文書検査 1 | `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok` |
| 文書検査 2 | `sh scripts/dev/check-adr-numbers.sh` | 0 | `check-adr-numbers: ok (179 files)` |
| 文書検査 3 | `sh scripts/dev/progress-index.sh --check` | 0 | `progress-index --check: ok` |

## 補足

- **e2e の初回失敗（環境由来）**: `TMPDIR` を run の作業領域（`/local/celeris/data/workspaces/.../runs/.../tmp`）にしたまま e2e を流すと、4 件とも `listen EINVAL: invalid argument .../owner.sock` で失敗した。Unix socket の path が長すぎる（SUN_LEN）のが原因で、コードの失敗ではない。`TMPDIR=/tmp` で流し直して 4 件とも通った。初回ログは `artifacts/e2e-live-view.log` を上書きしたため残っていない。
- test-parallel は両方とも `tests left N entries in TMPDIR (removed on exit)` の warning を出したが、exit 0 で終了時に削除された。
- userns 欄の意味: release env（`CELERIS_USERNS_TESTS=1` `CELERIS_ISOLATION_TESTS=require`）では `true`。この sandbox では userns が使えたため、`Operation not permitted` の件数は 0。

## 未解決事項

- ADR 付記 2026-10-10d の見出しは「人の決定」だが、本文の注記は「この回答は CoS の代答であり、人の決定そのものではない」と書く。見出しと本文が食い違う。この unit では ADR を変えていない（コード・ADR 不変の方針）。
- 実 Chrome と本番 launcher での Live View 映像の確認は未実施。手順は `docs/ops/browser-launcher-live-view.md` にある（人の運用）。

## 提案

- ADR 付記 2026-10-10d の見出しを「（CoS 代答: Live View は protocol 9）」に改めるか、人が実際に承認したなら注記を消すかを決める。どちらかに揃えてから、その commit で本節の検査を取り直す。
