---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-07
---
# web-chat / e2e-attach（添付の入力経路・upload queue）

ADR 2026-10-05-cos-chat-home D6 の添付 UI 行を `web/e2e/chat/attachments.spec.ts` の 2 試験で検証した。前葉の下書きは入力経路の参考にし、存在しない制御 API・本文ありの「画像だけ」試験・即時失敗中の進捗待ちを現行 fixture に合わせて書き直した。

## 検証内容

- 添付ボタンで filechooser を開き setFiles（Playwright の setInputFiles 経路）を使う。実 DataTransfer を持つ DragEvent と ClipboardEvent でも画像を加え、同じキューに 3 件並ぶことを確認。
- daemon の応答を保留し、XHR の送信済み bytes に対応した進捗（100/100）を表示していても、upload の成功応答前には送信できないことを確認。本文あり・なしとも送信ボタンを無効にし、Enter でも message 要求を送らない。
- drop・paste の画像 preview が実際に decode できることを naturalWidth で確認。
- upload 中の取消でキューから消え、gateway 経由の upstream も中断され、daemon の保留一覧から消える。
- drop・paste の成功後は本文と 2 つの attachment_ids を送信し、キューが空になる。
- upload を明示的に失敗させると失敗表示と再試行ボタンが出て、本文を入れても送信できない。再試行は同じ client_upload_id を使い、成功後は本文空の画像 1 件だけを送れる。要求 body と daemon の受理済み message の双方で空本文・添付 id を確認。

## 実装と修正

`fake-daemon.mjs` に次の token 保護された制御 endpoint を追加し、共通 chat ヘルパーと型コメントも更新した。状態は試験ごとの daemon 内で完結する。

- `POST /__fixture/chat/upload-state`: 新規 upload を hold / succeed / fail に切り替える。
- `GET /__fixture/chat/uploads`: 保留中の client upload id と名前を取得する。
- `POST /__fixture/chat/uploads/{id}/release`: 特定 upload を succeed / fail で解放する。

切断で保留を除去し、close 時にも待機を解く。単体試験では不正状態の 422、解放前の応答保留、失敗、再試行、成功済み id の冪等性、即時失敗、abort 後の除去と 404 を検証した。進行は endpoint と expect.poll / DOM の出来事待ちで制御し、sleep は追加していない。

初回の添付 spec は 1 passed / 1 failed。composer の `URL.createObjectURL` による preview が gateway の CSP に拒否されていたため、`web/server/app.js` の document CSP の img-src にだけ blob: を追加した。`app.test.mjs` に CSP の回帰検査を追加し、Playwright で画像 decode まで通った。添付処理本体の変更は不要だった。

## 証拠コマンドと結果

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: 初回は @types/express の cache 不足で exit 1。`install --frozen-lockfile` で取得後、最終 offline install は exit 0。lockfile 変更なし。
- `corepack pnpm@12.6.0 -C web exec biome format --write e2e features`: exit 0。server/app.js・app.test.mjs も整形済み。import 並べ替え 1 件は biome check --write で修正した。
- `corepack pnpm@12.6.0 -C web typecheck`: exit 0。
- `corepack pnpm@12.6.0 -C web lint`: 最終 exit 0（既存 styles.css の !important warning 4 件）。
- `corepack pnpm@12.6.0 -C web exec vitest run e2e/support/fake-daemon.test.ts`: exit 0、11 passed。
- `corepack pnpm@12.6.0 -C web test`: exit 0（Vitest 70 files / 486 tests、node server 57 tests）。
- build: 初回 Playwright 起動時に `e2e/support/ensure-dist.mjs` が dist 不在を検知し Vite build を実行、成功。以後 UI の変更はなく dist を再利用した。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat/attachments.spec.ts`: 最終 exit 0、2 passed。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat`: exit 0、22 passed（添付 2 + 既存 20）。
- `git diff --check`: exit 0。
- 前 run は server の 2 ファイルを独自に許可して範囲検査したが、計画の check はこれらを許可していない。この検査では計画の check を満たしたことにならない。gui/・crates/ の差分 0 件。

## 未解決

人の replan v7 で server 2 ファイルが追加許可され、範囲 check も成功した。未解決なし。fixme・skip なし。通信進捗の数値は実 XHR の byte 送信で確認し、daemon の制御は応答待ちから失敗・成功への遷移を決定的に進める。

## 提案

後続 e2e-narrow で狭い幅・mobile-audit・screenshot を検証する。

## 再試行（run 01M4A34V9E35TSK5EYWQAWYEVC）

変更前に計画と同じ範囲 check を実行し、exit 1、`web/server/app.js` と `web/server/app.test.mjs` が範囲外になることを再現した。前 run の修正は document CSP の img-src に blob: を許可し、composer が URL.createObjectURL で作る画像 preview を表示するためのもの。テストだけを変更して隠すことや composer の preview 方式を置き換えることは、この不具合への最小修正にならないため、修正を保持して plan_issue を返す。

再確認:

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: exit 0。
- `corepack pnpm@12.6.0 -C web exec biome format --write e2e features`: exit 0、変更なし。
- `node --test web/server/app.test.mjs`: exit 0、6 passed（CSP 回帰検査を含む）。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat`: exit 0、22 passed（添付 2 試験を含む）、5.0 秒。

計画修正の提案: e2e-attach の範囲 check に `web/server/app.js` と `web/server/app.test.mjs` の 2 パスを明示的に追加する。範囲修正後、同じ check を再実行する。受け入れ条件や E2E の assertion を弱める必要はない。

## 許可更新後の完了確認（run 01M4A3869X2GGSRH6WSHPSFQ6Y）

人の回答に従い、CSP 修正と回帰試験を保持して再検証した。以前の未解決事項は解消。コード・テストの追加変更は不要だった。gui/・crates/ の差分はない。

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: exit 0。
- `corepack pnpm@12.6.0 -C web exec biome format --write e2e features`: exit 0、変更なし。
- `corepack pnpm@12.6.0 -C web typecheck` / `lint` / `test`: すべて exit 0。Vitest 70 files / 486 tests、server 57 tests。lint は既存 styles.css の warning 4 件。
- `node --test web/server/app.test.mjs`: exit 0、6 passed。
- `corepack pnpm@12.6.0 -C web build`: exit 0（bundle size warning のみ）。
- `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat`: exit 0、添付 2 件を含む 22 passed。再 build 後も成功。
- `test -s web/e2e/chat/attachments.spec.ts` / `git diff --check`: exit 0。

範囲検査は同じ cwd と CELERIS_WU_BASE を使い、従来の許可に人が指定した server 2 パスだけを追加して実行し exit 0。保存された旧 execution-plan.json の check は更新前のため、実行した許可更新後の式を以下に記録する。

```sh
out=$({ git diff --name-only "${CELERIS_WU_BASE:-HEAD}"; git ls-files --others --exclude-standard; } | sort -u | grep -vE '^(web/e2e/chat/|web/e2e/support/|web/features/chat/|web/server/app\.js$|web/server/app\.test\.mjs$|agent-docs/progress/2026-10-05-cos-chat-home/web-chat/)'); [ -z "$out" ] || { echo "out of scope:"; echo "$out"; exit 1; }
```
