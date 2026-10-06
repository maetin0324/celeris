---
title: web gateway — チャット添付の streaming 中継・チャット SSE・添付ダウンロードの中継
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: blocked
updated: 2026-10-06
---
# web-chat / gateway WorkUnit

ADR [2026-10-05-cos-chat-home](../../../adr/2026-10-05-cos-chat-home.md) の D2（チャット SSE）と D4（添付）を web gateway で中継する。

## 完了内容
- `web/server/chat.js`（新規）: `createChat()`。JSON relay（relay.js）より前に `/api/chat` へ登録し、次の 3 種だけを扱う。他の `/api/chat/*` は `next()` で既存の JSON relay へ（挙動不変）。
  - POST `/api/chat/threads/{t}/attachments`: 本文を buffer に貯めず `node:http` の request で daemon へ流す（upstream の write が詰まったら受信を pause）。受信バイト数を数え、上限を超えた時点で upstream を abort して 413 `request_too_large`（`Connection: close`）。宣言の Content-Length が上限超なら daemon へ送らず 413。timeout は本文を送り終えてから応答まで。fetch を使わないのは、stream の本文で daemon が 401 を返すと undici が応答を返さず失敗する（`expected non-null body source`）ため。
  - GET `/api/chat/threads/{t}/stream`: timeout 無しの SSE 中継。`after` と `Last-Event-ID`（10 進の event id）だけを通す（不一致の判定は daemon）。切断で upstream を abort。200 以外（410 `chat-cursor-expired` の problem+json など）は status・Content-Type・本文をそのまま返す。
  - GET/HEAD `/api/chat/attachments/{a}/content|preview`: 本文を流す。Content-Type・Content-Disposition・Content-Length・ETag を保ち、`X-Content-Type-Options: nosniff`・CSP sandbox・no-store を付ける。content は Disposition が無ければ `attachment`、HTML/SVG 等は files.js の `dispositionFor` で attachment に直す。
  - daemon の 401/403 は JSON relay と同じく 502 `daemon_auth`、redirect は 502 `daemon_redirect`。JSON 応答は Buffer で送り、problem+json に charset を足さない。
- 上限: 既定 `DEFAULT_CHAT_UPLOAD_LIMIT_BYTES` = 100 MiB（D4 の 1 メッセージ合計）。`createApp({ chatUploadLimitBytes })`、起動時は環境変数 `CELERIS_WEB_CHAT_UPLOAD_LIMIT_BYTES`（index.js）。正の整数以外は起動失敗。
- `web/server/app.js`・`app.d.ts`・`index.js`: 配線のみ。

## 証拠
- `node --test server/chat.test.mjs`（web/ で）→ tests 10 / pass 10 / fail 0。
  - 上限ちょうど（1024 B）: 201、daemon は全バイトと boundary 付き Content-Type・daemon token を受け、cookie は受けない。daemon が先頭を受けた時点でブラウザ側は送信途中（貯めていない証拠）。
  - 上限超過（chunked で 1026 B）: 413 `request_too_large`、daemon の request は未完了のまま close（受信 ≤ 1024 B）。
  - 宣言 Content-Length 超過: 413、daemon に届かない。
  - upload 中のブラウザ切断: daemon の request が未完了で close。
  - SSE: `after`・`Last-Event-ID` が届き cookie は届かない。次の event を daemon から送るまで stream を保持し、`text/event-stream`・no-store・`X-Accel-Buffering: no`、切断で daemon 側 close。時間待ちは使わない。
  - 410 problem+json はそのまま（Content-Type も一致）、不正 cursor は 400 `invalid_query` で daemon に届かない。
  - content/preview の Content-Type・Content-Disposition（filename* 込み）・nosniff・HEAD・Disposition 無し SVG → attachment、preview 404 problem+json の素通し。
  - 他の `/api/chat/*`（一覧・送信）は JSON relay のまま、1mb 上限も 413 のまま。
- `node --test server/*.test.mjs` → tests 57 / pass 57 / fail 0（既存の relay・files・events・console・auth を含む）。
- `corepack pnpm@12.6.0 -C web exec biome check server` → exit 0。
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0。
- `corepack pnpm@12.6.0 -C web test` → exit 0（vitest 391、node 57）。
- 指定 check `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile >/dev/null && corepack pnpm@12.6.0 -C web typecheck && corepack pnpm@12.6.0 -C web lint && corepack pnpm@12.6.0 -C web test` → exit 1。lint の 4 件は開始 commit `c40b3669` に存在する `web/components/content/artifact-preview.test.tsx`、`web/components/ui/{confirm-dialog,drawer,gallery}.test.tsx` の import 順。4 ファイルはこの WU の objective（`web/server/`）の外。`web lint` を server の lint に絞るか、先行の別 WU で 4 ファイルを直す必要がある。
- crates/・gui/ は変えていないので cargo の検査は対象外。

## 未解決
- 上記の全域 lint check が開始 commit にある 4 件で失敗する。計画の check を修正する必要がある。
- `check:secrets` は `web/dist` を要するため build 後の統合段で確認する。

## 提案
- `docs/ops/web-parallel-operation.md` §2.3 の web.env の例に `CELERIS_WEB_CHAT_UPLOAD_LIMIT_BYTES`（既定 104857600、daemon の `[cos.attachments] max_message_bytes` に合わせる）を 1 行足す。この WU の範囲（web/server）外なので close 段で扱うのがよい。
