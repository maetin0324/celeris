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

## 統合後の検査

統合ブランチ `celeris-wu/01M4HRQBNJCHA35KZP78WW8MM1/close-out` の HEAD `2069a241267f527af28807987abe240c09ac84d3` で実行した。

| 検査 | 結果 |
| --- | --- |
| `cd web && pnpm run typecheck` | 合格（exit 0） |
| `cd web && pnpm run lint` | 合格（exit 0）、459 files checked、4 warnings（既存 `styles.css` reduced-motion 規則の `!important`） |
| `cd web && pnpm run test` | vitest 88 files / 648 tests 合格。`node --test server/*.test.mjs` は 17 件を出力後に無出力で停止したため中断（exit 130）。全体コマンドは未完了 |
| `cd web && TMPDIR=/tmp pnpm exec playwright test e2e/runs e2e/browser --project=functional` | 合格、28 tests passed（run log 2 件 + browser 26 件） |
| `sh scripts/dev/check-doc-links.sh && sh scripts/dev/check-adr-numbers.sh && sh scripts/dev/progress-index.sh --check` | 合格（exit 0、ADR 177 files） |
| `sh "$CELERIS_WU_SCOPE_PATHS"` | 合格、開始時 snapshot から範囲外 path なし |
| `git diff --check "$CELERIS_WU_BASE"...HEAD` | 合格 |
| `git diff --name-only "$CELERIS_WU_BASE"...HEAD -- crates gui` | 出力なし。crates/ と gui/ に差分なし |
| `git status --short` | 出力なし。統合 worktree clean |

最初の e2e 失敗の原因は run の長い TMPDIR 以下に置かれた `owner.sock` の AF_UNIX path 長制限だった。短い `/tmp` を TMPDIR に指定すると全対象 e2e が通った。Playwright 設定に `chromium` project は存在せず、実際の project 名 `functional` で実行した。

lint の warning は今回の差分に含まれない `web/styles.css` の reduced-motion 規則に対するもの。

## 未解決事項

- `pnpm run test` の gateway node test 部分は 17 件を出力後に無出力で停止し、再実行でも同じ症状だった。Vitest は全件合格。Node test の hang 原因調査が必要。
- 全体検査コマンド `bash scripts/dev/test-parallel.sh` と `cargo clippy --workspace -- -D warnings` はこの close-out の acceptance 範囲に含めず実行していない。

## 本番反映

本番の反映は人が行う。`web/` の変更を取り込んだ後、既存の celeris-web release 追従手順で release を作成・反映し、GUI で `files.stdout=false` の run の案内・進捗・結果表示と browser イベント欄を確認する。release / daemon / service の操作はこの run では実行していない。

## 提案

`node --test server/*.test.mjs` の停止原因を調査する。実行環境の TMPDIR が長い場合は `TMPDIR=/tmp` を指定して runs/browser e2e を実行する。
