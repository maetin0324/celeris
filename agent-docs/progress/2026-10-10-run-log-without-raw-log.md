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

## 再検証（flaky 修正後）2026-10-10

proxy-authz-flake / write-set-flake の flaky 2 件修正後の統合 HEAD `66c99fce7407f6bd3b0849684684708a697c75c4` で実行。実装は変更していない（記録のみ）。詳細は `agent-docs/progress/2026-10-10-run-log-without-raw-log/reverify-flake.md`。

- web 全体試験（`cd web && corepack pnpm@12.6.0 run test`）: Vitest 90 files / 652 tests passed、Node `server/*.test.mjs` 78 passed / 0 failed、exit 0。
- lint / typecheck: `pnpm run lint` exit 0（461 files、既存 `web/styles.css` の 4 warnings）、`pnpm run typecheck`（`tsc -b`）exit 0。
- e2e functional（`TMPDIR=/tmp WEB_E2E_SCOPE=functional pnpm exec playwright test e2e/runs e2e/browser`）: 28 passed、exit 0。
- 文書検査: `check-doc-links.sh` / `check-adr-numbers.sh`（ADR 177 files）/ `progress-index.sh --check` 各 exit 0。crates 差分ゼロ、`git diff --check` クリーン、WU 範囲 check クリーン。
- cargo の全体試験は daemon の統合検査が流すため、この葉では回していない。

## 経緯: flaky 2 件と修正（2026-10-10）

- integrate-reclose で 2 件失敗: (1) e2e `browser/proxy-authz.spec.ts` の SSE 時間切れ — spec が「評価対象 fetch が購読した」ことを daemon の stream 要求数増加で推定していたが、`/browser` 自身の EventSource が先に数を増やしたため評価対象 fetch の購読開始前に event が送られ、fake daemon は過去 event を再送しないため待ちが時間切れ。修正: 評価対象 fetch の response header 到着後にフラグを立ててから送る（commit `2bc8cb8f`）。(2) `phase_effect_ab` write_set — `run_variant` が `next_end()` が Some になった時点で試験時計を進めており、全 run の開始登録より早いため off 変種で run B が run A が書いた版を読み `(0,0,2)` に。修正: `registered_runs() == running.len()` になってから時計を進める条件を追加し回帰試験を追加（commit `171d1b14`）。

## 未解決事項

- なし。flaky 修正後の上記全検査が exit 0。
## 本番反映

本番の反映は人が行う。`web/` の変更を取り込んだ後、既存の celeris-web release 追従手順で release を作成・反映し、GUI で `files.stdout=false` の run の案内・進捗・結果表示と browser イベント欄を確認する。release / daemon / service の操作はこの run では実行していない。
