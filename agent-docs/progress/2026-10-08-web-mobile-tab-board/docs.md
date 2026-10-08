---
title: web 下部タブの 3 列目をボードに — docs 葉
tasks: [01M4CRGVA0Z9T2SQ2QR4XFR6H5]
status: done
updated: 2026-10-08
completed: 2026-10-08
---

# web 下部タブの 3 列目をボードに — docs 葉

人の決定 2026-10-08（CoS チャット「スマホ版の下のメニューバーの真ん中のボタンはタスク一覧ではなくボードが見れるように」）を文書に反映した。コードは触っていない（shell.tsx・nav-items.ts のコメントと実装は tabbar 葉）。

## 変更

- `agent-docs/adr/2026-10-06-web-bottom-tabbar-first-screen.md`: 状態行に改訂を記し、D2 のボードの項に改訂の注を付け（本文は残す）、末尾に「付記 2026-10-08」を追加。並び ホーム・受信箱・ボード・案件・その他、タスク /tasks は「その他」シート（navItems の順）、理由に人の依頼、変えないこと（列数 5・44px・1 行・safe-area・md 以上の側面 nav・navItems の順・メニューボタン）、試験の置き場。
- `docs/frontend/DESIGN.md`「幅ごとの規則」: 下部タブの並び（ホーム・受信箱・ボード・案件・その他）と規則の段落を追加（従来 DESIGN.md には並びの記述が無かったため新設）。

## 証拠

- `grep -n '付記 2026-10-08' agent-docs/adr/2026-10-06-web-bottom-tabbar-first-screen.md` → 該当あり
- `grep -n 'ホーム・受信箱・ボード・案件・その他' docs/frontend/DESIGN.md` → 該当あり
- 範囲: `git diff --name-only $CELERIS_WU_BASE` は上の 2 文書とこの進捗だけ

## 未解決事項

- なし（実装・e2e は tabbar 葉、検証は web-verify 葉）。

## 提案

- なし。
