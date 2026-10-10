---
task: 01M4HRQBNJCHA35KZP78WW8MM1
wu: reverify-flake
status: done
completed: 2026-10-10
---
# flaky 修正後の再検証（通し）

## 実行条件

proxy-authz-flake（`2bc8cb8f`）と write-set-flake（`171d1b14`）の修正統合後 HEAD `66c99fce7407f6bd3b0849684684708a697c75c4` で実行。実装は変更していない（記録のみ）。cargo の全体試験は daemon の統合検査が流すので、この葉では回していない。

## 結果

- 条件: web 全体試験が完走し exit 0。コマンド: `cd web && corepack pnpm@12.6.0 run test`。出力: Vitest 90 files / 652 tests passed、Node `server/*.test.mjs` 78 passed / 0 failed、exit 0。
- 条件: lint と tsc が成功。コマンド: `cd web && corepack pnpm@12.6.0 run lint` → exit 0、461 files checked、既存 `web/styles.css` の reduced-motion 規則に 4 warnings。`cd web && corepack pnpm@12.6.0 run typecheck` → `tsc -b` exit 0。
- 条件: runs/browser functional e2e が成功。コマンド: `cd web && TMPDIR=/tmp WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 exec playwright test e2e/runs e2e/browser`。出力: 28 passed (12.9s)、exit 0、retry なし。
- 条件: 文書検査 3 本が成功。コマンド: `sh scripts/dev/check-doc-links.sh` → exit 0。`sh scripts/dev/check-adr-numbers.sh` → exit 0（ADR 177 files）。`sh scripts/dev/progress-index.sh --check` → exit 0。
- 条件: crates に差分がない。コマンド: `git diff --name-only "$CELERIS_WU_BASE"...HEAD -- crates`。出力なし。
- 条件: 差分に whitespace error がない。コマンド: `git diff --check`。出力なし。
- 条件: WU の範囲外 path がない。コマンド: `sh "$CELERIS_WU_SCOPE_PATHS"`。出力: 本葉の `agent-docs/progress/2026-10-10-run-log-without-raw-log/` 配下のみ。
