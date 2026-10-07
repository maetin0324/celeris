---
title: 葉 web-ui — 端末の登録ボタン・自動復帰・登録端末の一覧と失効
tasks: [01M4ADMXWYHPSVEJBJ5JPCR6PS]
status: done
updated: 2026-10-07
completed: 2026-10-07
---
# 葉 web-ui: 信頼できる端末の画面

## やったこと（web/ だけ。crates/ は差分なし）

- 自動復帰（`features/browser/browser-query.ts`）: `ownerSessionQuery` の queryFn を `fetchOwnerSession` にした。
  `GET /browser/owner-session` が `isOwner: false` かつ `resumable: true`（端末 cookie あり）なら
  `POST /browser/owner-session/resume` を 1 度だけ呼び、状態を読み直す。同時の読み手は in-flight を共有。
  失敗したら `resumeError`（固定コード）を付けて返し、読み込み直すまで再試行しない（成功したら再び試せる）。
- `OwnerSessionNotice` は `resumeError` があれば理由（失効・期限切れ・不一致 / 接続不可）を alert で出し、従来の challenge 発行に戻る。
- 登録（`features/browser/trusted-devices.tsx` の `TrustedDeviceOffer`・`TrustDeviceForm`）: CLI 承認の owner で端末 cookie が無いとき
  「この端末を信頼する」form（名前入力、既定値は UA から）を出し、`POST /browser/owner-session/device` を CSRF 付きで呼ぶ。
  登録済みなら短い表示と一覧への link。live view（`/browser/runs/$taskId/$runId`）と `/browser` に置いた。
- 一覧と失効: 新画面 `/browser/devices`（「信頼できる端末」）。名前・登録日時・最終使用・期限・状態（有効 / 失効済み /
  使い回し検知 / 期限切れ）、「この端末」badge、有効な端末にだけ失効ボタン（ConfirmDialog）。
  SPA 経路は `/browser/trusted-devices`（gateway の JSON API）と衝突しないよう `/browser/devices` にした。
  `server/spa-routes.js`・`routeTree.gen.ts`・`e2e/support/screens.ts`（fixture `/browser/devices`）に追加。`/browser` から link。
- realtime: `trusted_device_*` 4 種で `["browser","trusted-devices"]` を、revoked では owner-session も stale にする。
- e2e の支え: 偽 daemon に `/api/v1/browser/trusted-devices`（登録・verify と回転・一覧・失効、上限 5・90 日・旧秘密の再提示で reuse 失効、
  assertion は既存の `claims()` で署名と期限を検証）。`startBrowserGateway` に固定の login 署名鍵と `restart()`（同じ port で web を起こし直す）。

## 証拠

- `pnpm -C web exec playwright test e2e/browser/trusted-devices.spec.ts`: 3 本合格
  （登録 → web 再起動 → 再読み込みで challenge なしに復帰・cookie 回転・axe 0 件 → 別端末の失効で状態が「失効済み」→ 今の端末の失効で即 challenge 表示 →
  再起動後も復帰しない／誤った秘密は拒否され理由を表示・cookie 消去／live view から登録）。`e2e/browser/` 全体 24 本合格。
- `pnpm -C web e2e`: 275 passed・8 skipped。`pnpm -C web e2e:nfr`: 108 passed（`/browser/devices` の axe と 4 幅 mobile-gate を含む）。
- `pnpm -C web test`: vitest 72 files / 491 tests 合格、`node --test server/*.test.mjs` 67 本合格（spa-routes の画面数 37→38 を更新）。
  新しい vitest `features/browser/trusted-devices.test.tsx`（`trusted_device:` 自動復帰 4・要求 3・状態と表示 5）。
- `pnpm -C web typecheck`: exit 0。`pnpm -C web lint`: exit 0（既存 warning 4 件）。
- `git diff --name-only $CELERIS_WU_BASE -- crates/`: 0 件。
- `cargo clippy --workspace -- -D warnings`: exit 0。`bash scripts/dev/test-parallel.sh`: exit 0（passed 4320、failed 0、ignored 14）。

## 未解決事項

- CLI 承認の owner が登録しても、gateway のメモリの owner に `deviceId` は付かない（gateway の仕様）。そのため登録直後の一覧には
  「この端末」badge が出ず、次の resume の後から出る。失効の即時反映は resume 由来の owner に効く。
- 自動復帰は画面の読み込みごとに 1 度だけ。失敗後は読み込み直すまで challenge 表示のまま。

## 提案

- gateway の登録（`registerDevice`）で、今の owner に登録した `deviceId` を結び付けると、登録直後から「この端末」表示と失効の即時反映が揃う（web/server は本葉の範囲外）。
