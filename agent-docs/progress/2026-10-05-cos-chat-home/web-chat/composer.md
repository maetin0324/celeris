---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-06
---

# web-chat composer

ADR `2026-10-05-cos-chat-home` D5 の入力欄と添付・キュー操作を `web/features/chat/composer/` に実装した。home の route への接続は後続の home unit が行う。

- Enter 送信、Shift+Enter 改行、composition 中と keyCode 229 の送信抑止、タッチ用送信ボタンを実装した。
- 添付ボタン、drop、paste を同一 upload queue に接続した。画像 preview、原名、容量、進捗、取消、失敗、再試行を表示する。未完了の添付がある間は送信できず、添付だけでも送信できる。
- run 中は通常送信を queue mode にし、明示の割込みは interrupt mode にした。待機中の発言と順序、DELETE 取消、run_id 指定の停止、paused queue の再開を出す。quota 待ち、停止要求中、切断、failed を区別する。
- `visualViewport` と shell の下部タブ inset、安全領域を使って入力欄を配置し、操作ボタンは共通 Button の 44 px target を使う。
- 送信中に下書きや添付が追加されても、送信対象だけを消す。同じ内容の失敗後の再送は同じ client_message_id を使う。upload の遅延完了が取消済みなら添付を削除する。

## 検証

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: exit 0
- `corepack pnpm@12.6.0 -C web typecheck`: exit 0
- `corepack pnpm@12.6.0 -C web exec vitest run features/chat/composer/chat-composer.test.tsx`: exit 0、6 passed
- `corepack pnpm@12.6.0 -C web test`: exit 0、vitest 427 passed、server 57 passed
- `corepack pnpm@12.6.0 -C web lint`: exit 0、既存 `styles.css` の `!important` に関する警告 4 件
- `corepack pnpm@12.6.0 -C web build`: exit 0、chunk size の警告あり

## 未解決と提案

- route へ組み込んだ状態の e2e と 320 px 実画面の確認は home・e2e-mobile unit で行う。
- data 層は変更していない。
