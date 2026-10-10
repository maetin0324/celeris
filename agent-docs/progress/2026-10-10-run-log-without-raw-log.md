---
tasks: [01M4HSR9B6NT3Z3SZWVH8ZH1C1]
title: 生ログを保存しない run の進捗と結果表示
status: done
updated: 2026-10-10
---

# 生ログを保存しない run の進捗と結果表示

## 完了

`files.stdout === false` の run は stdout を要求せず、機密保護の案内・worker_progress の進捗・利用可能な result summary/question を表示する。browser 画面のイベント欄では browser.* の ToolResult も表示する。stdout が保存される run の既存経路は維持した。

- `agent-docs/progress/2026-10-10-run-log-without-raw-log/runlog-view.md` — run ログ画面の実装・単体試験記録
- `agent-docs/progress/2026-10-10-run-log-without-raw-log/live-events.md` — browser イベント欄の実装・単体試験記録
- `agent-docs/adr/2026-10-10-run-log-without-raw-log.md` — files 判定、表示内容、未取得時の互換動作の決定

## 再検証 2026-10-10

統合後 HEAD `258be014739e5a1326788cf5dbb2148aec8f87a9` で実行した。

- 条件: web 全体試験が完走し exit 0。コマンド: `cd web && corepack pnpm@12.6.0 run test`。出力: Vitest 90 files / 652 tests passed、Node `server/*.test.mjs` 78 passed / 0 failed、全体 exit 0。前回の node test 停止・未完了は解決。
- 条件: lint と tsc が成功。コマンド: `cd web && corepack pnpm@12.6.0 run lint`。出力: exit 0、461 files checked、既存 `web/styles.css` の `!important` に 4 warnings。コマンド: `cd web && corepack pnpm@12.6.0 run typecheck`。出力: `tsc -b` exit 0。
- 条件: runs/browser functional e2e が成功。コマンド: `cd web && TMPDIR=/tmp corepack pnpm@12.6.0 exec playwright test e2e/runs e2e/browser --project=functional`。出力: 28 passed、exit 0（no-raw-log fixture を含む）。
- 条件: crates に差分がない。コマンド: `git diff --name-only "$CELERIS_WU_BASE"...HEAD -- crates`。出力なし。
- 条件: 文書検査 3 本が成功。コマンド: `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/progress-index.sh --check`。出力: 各 exit 0（ADR 177 files）。
- 条件: WU の範囲外 path がない。コマンド: `sh "$CELERIS_WU_SCOPE_PATHS"`。出力なし、exit 0。
- 条件: 差分に whitespace error がない。コマンド: `git diff --check`。出力なし、exit 0。

lint の 4 warnings は今回の差分に含まれない `web/styles.css` の reduced-motion 規則に対するもの。

## 本番反映

本番の反映は人が行う。`web/` の変更を取り込んだ後、既存の celeris-web release 追従手順で release を作成・反映し、GUI で `files.stdout=false` の run の案内・進捗・結果表示と browser イベント欄を確認する。release / daemon / service の操作はこの run では実行していない。
