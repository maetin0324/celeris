---
title: "Live View の Playwright 実証（pw-live）"
tasks: [01M4JAK3MY5G1K8T66Q4RTWSS5]
status: done
updated: 2026-10-10
---

# Live View の Playwright 実証（pw-live）

final review 基準 2 の不足（有効ケースが 4 バイトの不完全な JPEG で blob: src を見るだけ、無効理由の表示が未検証）を埋めた。

## 変更

- `web/e2e/browser/live-view-frames.spec.ts`: Chromium の canvas で作った 16x16 の単色 JPEG（SOI/EOI を確認）を WebSocket で送り、`img.decode()` 成功・`naturalWidth` 16・中央 1 px の色で表示を確かめる。2 枚目（青）を送り、`src` が変わり decode 後の色が青になることを確かめる。非 owner 拒否と読み取り専用文言・引き継ぐボタン無しは従来どおり残す。
- `web/e2e/browser/live-view-disabled.spec.ts`: 新ケース。grant が launcher 由来の `live_reason: launcher_protocol_no_live_frames` を返すとき、`browser-live-unavailable`（`data-reason` = その reason）・「映像なし — イベントで監視中」・launcher の理由文言を表示し、relay 由来の文言は出ないことを確かめる。
- `web/e2e/support/fake-daemon.mjs` / `fake-daemon.d.mts`: `createBrowserBackend` に `liveReason` option を足す（`framesAvailable` が false のとき grant に `live_reason` を返す）。既定は null で既存の試験の挙動は変わらない。
- 本体（`web/features`・`web/server`・`crates/`）は変更していない。安全境界（本人のみ・保存しない・input 拒否）も変えていない。

## 証拠

- 依存: `corepack pnpm@12.6.0 -C web install --offline` → `Done in 496ms`（exit 0）。
- 対象 e2e:
  - 実行: `cd web && TMPDIR=/tmp WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 exec playwright test e2e/browser/live-view-frames.spec.ts e2e/browser/live-view-disabled.spec.ts`
  - 結果: `4 passed (3.8s)`、exit 0。
    - live-view-frames: owner sees a decoded launcher frame, a newer frame replaces it, and non-owner is refused ✓
    - live-view-disabled: a launcher without live frames shows its own reason and event monitoring ✓
    - live-view-disabled: gateway without live relay reports every run unavailable ✓（既存）
    - live-view-disabled: a human-waiting run also shows event monitoring ✓（既存）
- lint: `corepack pnpm@12.6.0 run lint`（`biome check .`）→ exit 0。警告 4 件はすべて `web/styles.css:220-223` の `noImportantStyles`（今回の変更外）。変更ファイルは `biome check e2e/browser e2e/support/fake-daemon.mjs e2e/support/fake-daemon.d.mts` で `No fixes applied`、exit 0。
- typecheck: `corepack pnpm@12.6.0 run typecheck`（`tsc -b`）→ exit 0。
- 単体: `npx --no-install vitest run e2e/support/fake-daemon.test.ts` → exit 0。

## 基準との対応

- 基準 0（実 JPEG が decode され naturalWidth>0 で表示）: `decodedFrame` が `img.decode()` を await し、`naturalWidth` 16 と中央画素の赤（R>200）を確かめる。PASS。
- 基準 1（2 枚目の frame への差し替え）: 青の frame 送信後、`src` が最初の値と変わり、decode 後の中央画素が青（B>200）。PASS。
- 基準 2（launcher 由来の無効理由の文言表示）: `data-reason=launcher_protocol_no_live_frames`、理由文言、「映像なし — イベントで監視中」を確認。PASS。
- 基準 3（web test・lint・対象 e2e が exit 0、pw-live.md に結果）: 上記の通り。PASS（web の全 vitest / 全 e2e は本 unit の範囲外で未実行）。

## 未解決事項

- 実 JPEG は Chromium の canvas で生成している（試験の時点で Chromium が必ず使える前提）。固定の JPEG バイト列を fixture にしなかった理由は、手で書いた JPEG の正しさを本ツールで検証できないため。
- 本番 launcher の実 frame（CDP screencast 由来）での表示は、運用の確認手順（`docs/ops/browser-launcher-live-view.md`）で人が確かめる。本 unit では行っていない。
