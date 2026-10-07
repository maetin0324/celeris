---
title: 葉 web-server — web gateway の端末登録・再起動後の owner session 自動復帰・失効の即時反映
tasks: [01M4ADMXWYHPSVEJBJ5JPCR6PS]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 web-server: web gateway の信頼端末

## やったこと（`web/server/browser-live.js`、`auth.js`、`app.js`）

- 端末 cookie `__celeris_web_device` = `<device_id>.<secret>`（secret は `randomBytes(32)` の base64url）。
  属性は httpOnly・SameSite=Strict・https のときだけ Secure・`Path=/browser/owner-session`・Max-Age = daemon の `expires_at` まで。
  daemon には `sha256("celeris-device\0" + secret)` だけを送る。形の不正な cookie は daemon に送らず消す。
- 登録: `POST /browser/trusted-devices`（と ADR D3 の名前 `POST /browser/owner-session/device`）。owner・CSRF（本文 `csrf` か header `x-csrf-token`）・Origin を要する。
  名前は 1〜64 文字。daemon の `POST /api/v1/browser/trusted-devices` を `DeviceClaims`（`device_register`）の assertion 付きで呼ぶ。
- 復帰: `POST /browser/owner-session/resume`。password login（`auth.sessionKey`）＋ Origin ＋ 端末 cookie のときだけ daemon の `…/verify` を呼び、
  成功なら回転後の秘密で cookie を置き直し、`owner = { session, deviceId, expires }` を作る（challenge なし）。
  `expires = min(now+24h, login cookie の期限, 端末の expires_at)`（`auth.sessionExpiresAt` を足した）。
  同じ端末・同じ秘密の同時 resume は in-flight を共有して daemon を 1 回だけ呼ぶ。
- `GET /browser/owner-session` に `trustedDevice`（cookie を受けたか）・`resumable`・`deviceId` を足した。
- 一覧 `GET /browser/trusted-devices`（`currentDeviceId` 付き。hash は返さない）、失効 `DELETE /browser/trusted-devices/:id`
  （と `POST …/:id/revoke`）。失効した端末から作った owner はメモリから即時に落とす（live の WebSocket も閉じる）。
  resume が `device_rejected` のときも、その端末由来の owner を落とす（旧秘密の再提示で daemon が失効させた場合）。
- probe: `createApp({ probe })`（既定 `CELERIS_WEB_PROBE === "1"`）。`startSocket()` が null を返し、端末系の端点は 503 `probe_mode`、
  daemon には何も送らない。`createApp` に注入の時計 `now` を足し、auth と browser-live に渡す。
- 秘密は log・応答・daemon への要求のどれにも出ない（試験で確認）。

## 証拠

- `node --test server/browser-live.test.mjs`: 19 本合格（新しい `trusted_device:` 8 本: 再起動後の復帰、同時 resume の集約、失効と owner の即時失効、
  90 日 sliding 期限（偽の時計）、誤った秘密・旧秘密の使い回し・形の不正、未 login の拒否、probe モード、登録の owner/CSRF/Origin）。
- `node --test server/*.test.mjs`: 67 本合格。`pnpm lint`: exit 0（既存の warning 4 件）。
- `git diff --name-only $CELERIS_WU_BASE -- crates/`: 0 件。
- `cargo clippy --workspace -- -D warnings`: exit 0。`bash scripts/dev/test-parallel.sh`: exit 0（passed 4320、failed 0、ignored 14）。

## 未解決事項

- （attempt 2 で解消）schema の Event 4 種（`trusted_device_*`）を `web/api/realtime/event-kinds.ts`・`invalidation-map.ts` に足した
  （store 葉 b1e99155 が戻した変更の再適用。sets は空、一覧の query 無効化は web-ui 葉で足す）。
  `pnpm -C web test && typecheck && lint` は exit 0（vitest 71 files / 479 tests 合格）。web-ui 葉が同じ 9 行を足すと merge で重なる。
- 不一致の秘密による拒否でも、その端末由来の owner を落とす（web は daemon の拒否理由を区別できないため、fail closed）。
  password と device id を持つ者が owner を落とせるが、owner にはなれない。

## 提案

- web-ui 葉は上の web 端点（登録・resume・一覧・失効、`trustedDevice`/`resumable`）を使う。
