---
tasks: [01M4TJFJFTA037YD6D3AGZ8SXJ]
unit: web-audit
status: done
date: 2026-10-13
---
# web-audit: web の予約監査欄と routing catalog 画面を消す

## したこと
- `web/features/ops/routing-catalog.ts`・`routing-catalog-section.tsx`・`routing-catalog.test.tsx` を削除（`GET /llm/routing/catalog` は消えた）。
- `UNKNOWN`（欠測の表示）を `web/features/ops/routing-state.ts` へ移した。
- `routing-state.ts` から `excludedLabel`・`scoreLabel`・`auditIncompleteLabel` を消した（型 `ExcludedReason`・`ScoreTrace` と `audit_incomplete` が生成型から消えたため）。対応する試験も消した。
- `web/features/tasks/routing-audit-view.tsx` を再生成型に合わせた。消した表示: 予約・上限消費の欄、estimator shadow（要約と詳細）、optimizer の比較、候補ごとの除外理由・score・費用内訳、実際に使った source、監査の不完全の注記。残した表示: run の実行枠・lane・選定理由・実測の請求・escalation・outcome・shadow 一覧（kind・status・候補・差・token）・結べない要求。新しい表示は足していない。
- `routing-audit-view.test.tsx` を新しい形に合わせて書き直した。予約欄が出ないことを確かめる試験を 1 件置いた。
- `web/e2e/support/fake-daemon.mjs`・`.d.mts`: routing catalog の fixture と応答、`reservation` 欄、`estimator`・`estimator_shadow` の fixture（`routingEstimatorShadowFixture`）、`requests`・`audit_incomplete`・`actual_sources` などの消えた欄を消した。

## 試験
- `corepack pnpm@12.6.0 -C web typecheck` → exit 0（修正前は 42 件のエラー）
- `corepack pnpm@12.6.0 -C web test` → exit 0。vitest 93 files / 661 tests passed。node --test fail 0。
- `corepack pnpm@12.6.0 -C web exec biome check <変更した 5 file>` → exit 0。

## 未解決事項・提案
- 生成型（`web/api/generated`）から、request 単位の routing trace（候補の除外理由・score・費用 4 成分）と task 横断の estimator 要約が消えた。`RequestRoutingAudit` は `decision_id`・`request_id` だけになった。この画面は候補を出せなくなった。`routing_request_decided` event 自体は残っている。survey は trace を「selector-wire の wire 型へ移す」としていた。API から候補の trace を消すのが意図通りかは人が確認する（rm-core の Rust 側の判断。web では直せない）。
- 本葉は web/ だけを変えた。e2e（Playwright）は走らせていない。
