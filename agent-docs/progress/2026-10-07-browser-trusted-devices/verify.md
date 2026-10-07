---
title: 葉 verify — 統合後 HEAD の全体検証
tasks: [01M4ADMXWYHPSVEJBJ5JPCR6PS]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 verify: 全体検証（base 0612bf24）

コード・設定は変更していない（修正なし）。全コマンドが exit 0。

| # | コマンド | exit | 要点 |
|---|---|---|---|
| 1 | `bash scripts/dev/test-parallel.sh` | 0 | nextest 4320 件 passed / 0 failed（skip 13）、binaries 133。doctest 10 binaries 合格（ignored 1）。`test-parallel: ok` |
| 2 | `cargo clippy --workspace -- -D warnings` | 0 | 警告なし |
| 3 | `cargo fmt --all -- --check` | 0 | 差分なし |
| 4a | `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` | 0 | 完了 |
| 4b | `… -C web test`（vitest + node --test） | 0 | vitest 72 files / 491 tests passed、node:test 67 pass / 0 fail |
| 4c | `… -C web typecheck`（tsc -b） | 0 | エラーなし |
| 4d | `… -C web lint`（biome check） | 0 | 391 files、警告 4 件（styles.css:220-223 の noImportantStyles、既存。error なし） |
| 4e | `… -C web e2e`（functional） | 0 | 275 passed / 8 skipped / 0 failed（1.3m） |
| 5a | `bash scripts/selfdeploy/tests/web_probe_owner_isolation.sh` | 0 | `all ok` |
| 5b | `bash scripts/selfdeploy/tests/verify_web_app_start.sh` | 0 | `ok` |
| 5c | `bash scripts/selfdeploy/tests/web_follow_health_gate.sh` | 0 | `all ok` |

## 未解決事項

なし。（lint 警告 4 件は prefers-reduced-motion 用の `!important` で、本件の変更ではない。）
