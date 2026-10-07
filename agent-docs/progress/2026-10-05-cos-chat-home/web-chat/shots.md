---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-07
---
# web-chat / shots（見た目の再撮影）

既存8枚を fix-visual 適用済みの build で撮り直し、streaming・320px drawer・送信後の画像添付の3枚を追加した。全11枚、最大127,836 bytesで、各300 KB以下。

白い thumbnail の原因は二つあった。mobile.spec.ts の upload は1pxの白いPNGを使い、fake-daemon の preview endpoint も固定1px PNGを返していた。両方を青・黄色の96×64格子に変更した。preview_url は設定済みで、UI側の変更は不要だった。送信前と送信後に naturalWidth=96（>0）、送信後の会話内画像の可視性、横溢れ0・44px以上の操作領域を検証する。

streaming は hold 制御 endpoint で run を保持し、text_delta を3回流す。本文・caret・停止ボタンを検証してから撮り、撮影後に完了 event を流す。drawer は320pxで dialogと会話項目の可視性を検証してから撮る。sleep・負荷試験・fixme の追加なし。

## 画像の目視確認

全画像を view_image（Read相当）で開いて確認した。旧会話 badge は横一行、題名は省略されている。tool の成功は濃い緑で読め、添付は青と黄色の格子で白い空白ではない。

| 画像 | 写っている状態・確認内容 |
| --- | --- |
| [chat-threads-1440.png](screenshots/chat-threads-1440.png) | 空の会話と新規・検索・編集・保管。旧会話badgeが一行。 |
| [chat-resume-1440.png](screenshots/chat-resume-1440.png) | 再読込後の長い会話の末尾（履歴226〜240）とcomposer。 |
| [chat-content-1440.png](screenshots/chat-content-1440.png) | Markdownの表・太字・code、折りたたんだreadとgrepの両方の濃い緑の成功表示。 |
| [chat-cards-1440.png](screenshots/chat-cards-1440.png) | タスク・決定・質問の人待ち、回答ボタン・詳細link。 |
| [chat-queue-paused-1440.png](screenshots/chat-queue-paused-1440.png) | 順番待ちメッセージ・キュー停止中・取消・キュー再開。 |
| [chat-jump-latest-1440.png](screenshots/chat-jump-latest-1440.png) | 過去の履歴を読んでいる状態と「最新へ 2」。 |
| [chat-resync-1440.png](screenshots/chat-resync-1440.png) | snapshot再取得後の認可・計画承認・通知カードと「最新へ 2」。 |
| [chat-mobile-320.png](screenshots/chat-mobile-320.png) | CoS代答カードの取消・差し戻しと送信前の色付きthumbnail、composerと下部タブバー。 |
| [chat-streaming-1440.png](screenshots/chat-streaming-1440.png) | 生成途中の「進捗はこうです。」「2 段落」、考え中、停止・割り込み送信。 |
| [chat-drawer-320.png](screenshots/chat-drawer-320.png) | 会話一覧drawerの新規・検索・会話項目・編集・保管・閉じる。badgeが縦積みにならない。 |
| [chat-attachment-sent-320.png](screenshots/chat-attachment-sent-320.png) | 送信済み・順番待ちメッセージ内の色付きpreview、208 B、送信待ちの取消、composerと下部タブバー。 |

## 証拠コマンドと結果

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: 初回は不足tarballで失敗。`install --frozen-lockfile` で取得後、offline再実行はexit 0。lockfile変更なし。
- `corepack pnpm@12.6.0 -C web build`: exit 0。既存chunk size warningのみ。
- `corepack pnpm@12.6.0 -C web exec biome format --write e2e features`: exit 0。
- `CHAT_SHOT_DIR=../agent-docs/progress/2026-10-05-cos-chat-home/web-chat/screenshots WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat`: exit 0、31 passed。pnpm -C webの実行cwdに合わせ、指定したrepo内ディレクトリへ撮影する。
- 反復は `e2e/chat/content.spec.ts` と `e2e/chat/mobile.spec.ts` のみ実行。mobile初回は送信後previewが1pxで失敗し、偽daemon fixture修正後は8 passed。content最終は2 passed。
- `corepack pnpm@12.6.0 -C web mobile-audit`: exit 0、32 paths × 4 widths ok。
- `corepack pnpm@12.6.0 -C web typecheck`・`lint`: exit 0。既存styles.cssのwarning 4件のみ。
- `corepack pnpm@12.6.0 -C web test`: exit 0、vitest 70 files / 490 tests、server 57 tests passed。
- `git diff --quiet "$CELERIS_WU_BASE" -- crates/ gui/`: exit 0。
- `git diff --check`: exit 0。WU baseからの差分・untrackedをweb/e2e/chat、fake-daemon.mjs、この進捗、screenshotsに限定して確認。

## 未解決・提案

このWUの未解決なし。recloseで一覧・ADR付記を更新する際は、この11枚を参照する。
