---
title: web-ui fixture — 偽 browser backend と browser e2e 用 gateway 起動補助
tasks: [01M470CJT10MV8XVFEGSMCXHYS]
status: done
updated: 2026-10-05
completed: 2026-10-05
---

# web-ui fixture — 偽 browser backend と browser e2e 用 gateway 起動補助

WorkUnit `fixture`。[ADR 2026-10-05-browser-department-web-live-view](../../../adr/2026-10-05-browser-department-web-live-view.md) の D2.4・D3 の画面試験に使う偽物を作った。
gui/・crates/・web/server/ は変えていない。画面（色・token）は作っていない。

## 変更点

- `web/e2e/support/fake-daemon.mjs`: `createBrowserBackend()` を追加。`createFakeDaemon({ browser: true | {...} })` のときだけ動く（既定は無効。既存の fixture・受信箱の件数は変わらない）。
  - T1（P1）・T2（P2）。どちらも `skills: ["browser-enabled"]`・running。T1 に R0（COMPLETED）・R1（S1）、T2 に R2（S2）。
    `browser_updated` は raw の `live_view_url`（`BROWSER_RAW_LIVE_VIEW_URL`）を持つ。T1 の履歴には別 task（T2/R9）の行を 1 つ混ぜた（gateway が捨てることを確かめる用）。
  - `GET/POST /api/v1/tasks/{id}/browser/control/{run}/{session}`（+ `/disconnect`）: `agent_running → (pausing) → paused → human_control`、
    `expected_version` の不一致 409、holder 違いの renew 403、lease 期限で `paused`、disconnect で `paused`（agent は再開しない）、
    `resume` は `fresh_snapshot`・`policy_origin_ok` が両方 true のときだけ `agent_running`、`stop`、`auth_section` で 409、idempotency key の再送は同じ応答。
  - waits: W1（T1/R1 の decision 待ち）・W2（T2/R2 の credential 待ち。`credentialWait: false` で外せる）。`GET …/browser/waits`・`GET /api/v1/browser/waits`・
    `POST …/decision|credential`。受けた body（attestation・password を含む）は `daemon.browser.records.waits` に残る。解決すると受信箱の項目が消え `inbox_changed` を送る。
  - live: `grant`（60 秒）・`check`・`read?after=`（task-api の `resume_plan` と同じ replay/reset。scrub 済みの status・tabs・url・console）。`appendLive()` で足せる。
  - `/api/v1/browser/identities`: 一覧（project 別）・登録（201、重複 409）・revoke（generation+1）・restore（204）・delete。
  - 受信箱 `/inbox/items` に `browser_wait:W1`・`browser_wait:W2`、`/inbox` の `browser_waits` も埋める。
  - 時計は `now` で差し替える。`publicKey` を渡すと assertion の Ed25519 署名を確かめる。task 一覧に T1・T2 を前置きし、rich profile の他の task は待ちなし・履歴なしで答える（gateway の namespace 走査が 503 にならない）。
- `web/e2e/support/fake-daemon.d.mts`: 上の型。
- `web/e2e/support/gateway.ts`: 返り値に `server` を足した（WS upgrade を付けるため。既存の呼び出しは変わらない）。
- `web/e2e/support/browser-gateway.ts`（新規）: 一時 dir（0700）に password file・0600 の Ed25519 鍵・owner socket を置いて `startGateway` を起こす。
  偽 dashboard（loopback の http+ws。`/`・`/_next/static/x.js`・`/api/sessions`・`/api/chat/status`・`/api/session/{port}/status|tabs`・WS `/api/session/{port}/stream`）を
  `liveUpstream` に渡す。`loginAsOwner(page)`（login → challenge → socket で approve → CSRF token を返す）・`loginAsOther(page)`・`approveChallenge()` を公開。
  `dashboard.inputs`（WS で届いた message）・`dashboard.requests`（Host/Origin）・`broadcast()` を持つ。
- `web/e2e/support/fake-daemon.test.ts`: browser backend の試験 6 本（既定で無効・履歴と schema・control の状態機械・waits の記録・live grant/read・identities）。
- `web/e2e/browser/fixture-smoke.spec.ts`（新規）: owner の `/browser/runs` 取得（raw URL を含まない）と Live View の中継、他の session の 403 `not_owner`。

## 実行したコマンドと結果

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` → 成功
- `pnpm test`（vitest + node --test）→ exit 0、Test Files 59 passed / Tests 367 passed（browser backend 6 本を含む）
- `pnpm typecheck` → exit 0
- `pnpm lint` → exit 0（既存の warning のみ）
- `pnpm check:boundaries` / `check:secrets` / `check:parity` → 各 exit 0
- `WEB_E2E_SCOPE=functional pnpm exec playwright test e2e/browser/fixture-smoke.spec.ts` → 1 passed
- `WEB_E2E_WORKERS=4 pnpm e2e`（functional 全体）→ 204 passed / 3 failed / 8 skipped。落ちた 3 件は本変更と無関係:
  `shell/home-layout.spec.ts:26`・`work/home-stale-viewport.spec.ts:15` は単独でも落ちる既存の失敗（main で既知）、
  `work/narrow-r6.spec.ts:173` は負荷時の揺れで単独では pass。

## 未解決事項

- gateway（`web/server/browser-live.js`、本 WU の範囲外）の `isAuthWait` は wait の `state` を見ない。task-api の `GET …/browser/waits` は解決済みの wait も返すので、
  credential を登録した後も namespace の全 Live View が 409 `auth_interval` のままになる。偽 daemon も実物どおり解決済みを返す。
  Live View を試す spec は `backend: { credentialWait: false }` を使う。

## 提案

- `isAuthWait` を開いている wait（`pending`・`approved`）だけに絞る修正を gateway の後続（authz-e2e か close）で行う。
