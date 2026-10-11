---
title: "旧 GUI を廃止し web に一本化する"
tasks: [01M4NAKDCESYK36PNY7TTCTJ5E]
status: running
updated: 2026-10-11
---

# 旧 GUI の廃止

## 現在地

2026-10-11: 人が必要とした2機能を web に実装した。旧 GUI の削除と selfdeploy の変更は未着手。
全受け入れ条件の完了ではない。本番の unit・config・DB には触れていない。
方針は [廃止 ADR](../adr/2026-10-11-retire-gui.md) と
[破壊的変更を既定にする ADR](../adr/2026-10-11-breaking-changes-by-default.md)。

## 実装済み（Phase 1）

- `/cron` と `/cron/:id`: 一覧・次回・前回結果・雛形・履歴・task リンク、停止/再開/手動実行。
  `web/features/cron/` が既存 API relay `/api/cron-jobs` を使う。送信後に一覧・詳細・履歴を再取得。
  読み込み・空・取得失敗・古いデータ・権限不足を区別し、操作の二重送信を防ぐ。
- タスク詳細の「実行」に WorkUnit 検査ログを追加。展開時のみ取得し、実行中は5秒更新。
  現在/最後の検査の番号・command・exit・末尾・省略情報を表示。失敗時は直前のログを保持。
  木の実装は変更していない（並行 task 01M4NAKDCC との境界）。
- API・生成型は既存のものだけを使用。旧 server 処理の移植、Rust/API/schema の変更はない。
- cron を画面台帳・gateway の SPA route・ナビへ登録。既存 `spa-routes.test.mjs` の古い固定画面数は削除し、
  route tree との集合一致検査を維持した。

## 依存調査

web gateway は `web/server/index.js` の daemon URL（既定7710）から API・SSE・Console・file を配線する。
`auth.js` は独自 cookie/鍵、`browser-live.js` は本人認証・承認・credential・identity・Live View と
frames/WS を提供する。旧 GUI プロセスを経由する必須の経路は確認されていない。
本番の環境変数で upstream が上書きされている可能性は、移行時に運用セッションが確認する。

旧 `gui/app/routes.ts` の画面と resource を web の各 feature と対応表に照合。
旧画面限定として確認した cron と検査ログは今回追加済み。それ以外の限定機能は確認されていない。
詳細な対応表・調査コマンドは task artifacts の `gui-removal-audit.md` と `gui-removal-audit-checks.json`。

## 検証（Phase 1）

ログの根: `/local/celeris/data/workspaces/01M4NAKDCESYK36PNY7TTCTJ5E/artifacts`。

| コマンド | 結果・ログ |
|---|---|
| `corepack pnpm@12.6.0 -C web build` | 合格。既存の chunk size 警告あり。`web-build.log` |
| `corepack pnpm@12.6.0 -C web typecheck` | 合格。`web-typecheck.log` |
| `corepack pnpm@12.6.0 -C web lint` | 合格。既存 CSS の警告5件。`web-lint.log` |
| `corepack pnpm@12.6.0 -C web test` | vitest 93 files / 678件、gateway 93件合格。`web-test.log` |
| `corepack pnpm@12.6.0 -C web check:boundaries` | 合格。`web-boundaries.log` |
| `corepack pnpm@12.6.0 -C web e2e e2e/parity/cron-check-logs.spec.ts e2e/parity/task-detail.spec.ts` | 追加3件＋既存7件、計10件合格。`web-cron-check-e2e.log` |
| `corepack pnpm@12.6.0 -C web e2e e2e/parity/cron-check-logs.spec.ts` | ログの4幅・axe・更新失敗時の保持を追加後も3件合格。`web-cron-check-final-e2e.log` |
| `corepack pnpm@12.6.0 -C web e2e a11y/axe.spec.ts --grep 'S4 /cron'` | 一覧/詳細の2件合格。`web-cron-axe.log` |
| `corepack pnpm@12.6.0 -C web mobile-audit --only /cron` と `--only /cron/daily` | 各360/390/412/1440pxで合格。`mobile-cron.log` / `mobile-cron-detail.log` |

画像は `cron-screenshots/`（8枚）と `check-log-screenshots/`（4枚）。新設画面の変更前は404
（初回調査 JSON の isSpaRoute=false）。検査ログは上記 e2e で4幅と axe を確認。
依存の offline install は tarball が不足したため、同一 lockfile の既存 checkout から依存だけを
worktree の `web/node_modules` へコピーして検証した。コピー元は変更していない。

Rust の指定検査も完了:

- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh`: exit 0、5,131 passed / 0 failed / 14 ignored
  （nextest 5,128件＋doctest 3件）。`test-parallel.log`。残った一時 fixture 9件は script の EXIT cleanup で削除。
- `cargo clippy --workspace -- -D warnings`: exit 0。`clippy.log`。
- `sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-adr-numbers.sh`、
  `sh scripts/dev/progress-index.sh --check`: 全て exit 0（`doc-links.log` / `adr-numbers.log` / `progress-check.log`）。

通常の run TMPDIR は Unix socket の長さ上限を超えた。短い symlink での再試行は canonical path
比較/安全検査に影響した（`test-parallel-short-path.log`）。`/tmp/ct.XXXXXX` でも script がさらに作る
階層により4件が readiness timeout（`test-parallel-final.log`）となり、最終的に試験 fixture に限って
`TMPDIR=/tmp` を指定して全件合格した。source/依存のコピーや build 出力を /tmp に置いていない。
CARGO_TARGET_DIR・compiler wrapper・build 設定は渡されたまま。失敗した版と正常結果を取り違えないこと。

## 未解決・次の工程

1. `gui/`、専用 script/unit、build/deploy/verify 段と試験を撤去する。
2. web の release/verify を必須 gate とし、promote/rollback/status/migrate から旧 GUI を外す。
   現在の `web-follow.sh` は失敗しても exit 0 なので、唯一の画面として必要な成功判定を検討・検査する。
3. README・SPEC・architecture-map・現行 guide を web に合わせる。過去 ADR は改変しない。
4. 削除後の参照走査、selfdeploy の試験、workspace/clippy と web 検査を再実行する。
   削除前の走査 `gui-removal-remaining.txt` は416行（scripts/deploy/web/README/docs/guides の限定走査）。
5. 削除と運用経路の変更が揃ってから本番への移行手順を確定する。

## 本番への移行（撤去実装後に確定）

**本番操作は未実施。現時点で旧 GUI の unit を止めない。** 次工程で新 release の build/verify/promote
が web のみで動くことを確かめた後、運用セッションが以下を実行する具体的手順を確定する。

- `celeris-gui@` の active/enabled instance、unit 本文と drop-in、旧 port（既定7700）を使う proxy を棚卸し・保存。
- web の login、本人承認、Live View、SSE、file 配信、cron、検査ログを確認。
  `CELERIS_API_URL` と `CELERIS_WEB_LIVE_VIEW_UPSTREAM` が旧 GUI を向いていないことを確認。
- gui 無しの release を build/verify/promote し、web の healthz release と current の一致を確認。
- 旧 instance を停止・無効化し、template/drop-in の退避と除去、daemon-reload、不要 listener/proxy 設定の解除。
- DB・events・credential・既存 release・rollback に必要な依存/鍵は消さない。旧版へ戻す際は保存した unit と
  旧 release の手順を使用し、新コードに互換分岐を残さない。

## 提案

なし。人から承認済みの2機能と廃止だけを対象とする。
