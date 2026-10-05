# 既定 fixture と状態変種の案内（states-r6）

## 修正

- rich fixture に schema 準拠の組織、案件の途中目標、文書一覧・ページ、文書保守の監査結果と整理案を追加した。既定の撮影で取得失敗や「取得中…」のまま残らない。
- T25 を reviewing の判断対象として追加した。判断パネルに承認・却下を表示し、`screenshots.mjs --states --only-state reviewing` でスマホ幅の「判断」区画を開いて撮る。
- 空の依存グラフに、依存関係が無いこととタスク一覧への操作を表示した。
- 権限が無いタスク一覧の復帰先をホームにした。SSE 再接続中はタスク一覧と run ログの本文にも、更新されていない旨を表示した。
- fixture 増加で 25 件となった一覧試験とスクロール復元試験を追従させ、変更点を e2e で固定した。

## 表示確認

旧 after-r4 の対応画像を run 成果物の `before/` に保存し、修正後の `after/` を 360・390・412・1440px で撮影した。組織の木、案件の途中目標 1/1 達成、文書一覧・本文、文書保守の指摘・整理案、reviewing の判断、空 graph の案内、forbidden のホームリンク、stale の本文内警告を目視確認した。幅 390px の各画面で横方向のはみ出しは見られなかった。

撮影には `node web/scripts/screenshots.mjs --out <run artifacts>/after --only <route>` と `--states --only-state <state>` を使用。reviewing は `reviewing-_tasks_T25-*.png`、文書本文は `_projects_P1_docs_path_docs_2Fguide_md-*.png` にある。

## 検証

- `corepack pnpm@12.6.0 -C web typecheck`
- `corepack pnpm@12.6.0 -C web lint`
- `corepack pnpm@12.6.0 -C web test`
- `corepack pnpm@12.6.0 -C web build`
- `corepack pnpm@12.6.0 -C web mobile-audit`
- `corepack pnpm@12.6.0 -C web e2e --grep-invert 'S1 /' --retries=0`

最終結果: 上記 6 コマンドはすべて exit 0。unit test は 352 件、mobile audit は 31 画面 × 4 幅、functional e2e は 188 pass・既存 8 skip（retries 0）。試験の skip・削除はしていない。
