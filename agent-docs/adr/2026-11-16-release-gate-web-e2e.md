---
tasks: [01M7H1RNG5C5AA76HNB2M9M098]
status: accepted
---
# ADR 2026-11-16: release gate の web 段で e2e（functional）を流す

- 日付: 2026-11-16
- 状態: 採用（実装は task 01M7H1RNG5C5AA76HNB2M9M098）
- 決定者: 人（2026-11-16、運用 session の問いへの答え「起票し gate に e2e も足す」）
- 関係: [ADR 2026-10-20 全体の試験は release gate だけ](2026-10-20-full-tests-at-release-only.md) の D1・D4
  （本 ADR が D4 の段の記述を正す。同 ADR に付記 A1 を足した）、[ADR-0081](0081-web-spa-frontend.md)（web が唯一の画面）、
  `agent-docs/guides/testing.md`、`docs/ops/selfdeploy.md`

## 背景

ADR 2026-10-20 は D1 で web の e2e を release gate だけで流すとし、D4 で gate の段を「fmt → test-parallel → clippy →
web build・unit・e2e → browser ledger 等」と書いた。しかし `scripts/selfdeploy/release.sh` の web の段は
`web-config`・`web-pnpm-install`・`web-pnpm-typecheck`・`web-pnpm-test`・`web-pnpm-release` の 5 段で、e2e が無かった。
WU は自分の spec だけを流し、gate は Playwright を流さないので、どこも e2e の全体を流していなかった。

2026-11-16 に運用 session が本番の sha の作業ツリーで `pnpm -C web e2e` を流すと 19 件が落ち（18 件は再実行でも落ち、
1 件は揺れ）、18 件は 10-10〜11-04 の変更から落ち続けていた。どれも画面の不具合ではなく、古い試験・古い fixture・
試験の道具の不具合だった。

## 決定

D1. **gate の web 段に e2e（functional）を足す。** 段の順は次のとおり。どれも必須で、飛ばす切替を置かない。
    `web-config` → `web-pnpm-install` → `web-pnpm-typecheck` → `web-pnpm-test` → `web-pnpm-release` →
    `web-e2e-chromium` → `web-pnpm-e2e`
    - `web-pnpm-e2e` は `corepack pnpm@<版> e2e`（= `WEB_E2E_SCOPE=functional playwright test`）。retries は 0
      （`web/playwright.config.ts` のまま）。1 件でも落ちれば段が非 0 で終わり、gate.json の `ok` が false、
      `failed_step` と `web.failed_step` に `web-pnpm-e2e` が出て、`.gate-web-pnpm-e2e.log` が残る。release は作らない。
    - e2e は `web-pnpm-release` の後に流す。release の `vite build` が作った `dist/` は入力より新しいので、
      webServer の `ensure-dist.mjs` は build し直さず、配る `dist/` をそのまま試す（二重に build しない）。
    - e2e は偽 daemon と `vite preview` だけを使う（試験ごとに loopback の空き port）。本番の API・DB・process に触れない。

D2. **chromium の確認を別の段にし、download しない。** `web-e2e-chromium` は `corepack pnpm@<版> e2e:chromium`
    （`web/scripts/check-e2e-chromium.mjs`）で、Playwright が e2e で使う chromium を実際に 1 回起動して閉じる。
    browser は host の `~/.cache/ms-playwright`（`PLAYWRIGHT_BROWSERS_PATH` があればそこ）にあるものだけを使う。
    - 無い・起動できない host では、この段が exit 1 で落ち、`failed_step` は `web-e2e-chromium` になる。log には
      Playwright の error（探した path）と、人が実行する入れ方（`pnpm -C web exec playwright install chromium`。
      `@playwright/test` の版に合う chromium を入れる）を出す。
    - SKIP にしない。gate が自分で download しない（release gate は外部 network に依存しない）。
      chromium が無いことを「e2e の不合格」と区別できるよう、段を分ける。

D3. **`e2e:nfr`・`e2e:all` は本 ADR では gate に入れない。** 所要時間を測って task の進捗ファイルの「提案」に書く。
    入れるなら別の task で決める。

D4. **ADR 2026-10-20 の D4 の段の記述を正す。** 同 ADR に付記 A1 を足し、「gate の段は現状のまま」と書いた時点で
    e2e が無かったこと、本 ADR で e2e を足したことを書く。`agent-docs/guides/testing.md` と `docs/ops/selfdeploy.md`
    の gate の段の記述も同じ変更で直す。WU・段の統合・task の受け入れ・review で e2e の全体を流さない規則
    （ADR 2026-10-20 D1・D2）は変えない。WU は直した spec file を名指しで流す。

D5. **gate の self-test。** `scripts/selfdeploy/tests/release_web_required.sh` が偽の corepack で、
    (a) 合格の release で `web-e2e-chromium`・`web-pnpm-e2e` が skip されず exit 0 で走ること、
    (b) e2e が落ちると gate.json の `ok` が false・`failed_step` が `web-pnpm-e2e`・release dir が作られないこと、
    (c) chromium の確認が落ちると `failed_step` が `web-e2e-chromium` で e2e が走らないこと、を確かめる。

## 結果

- 次の release から gate の時間が e2e（functional）の分だけ増える。所要時間と本番への移行は task の進捗ファイルに書く。
- release を作る host の、release を作る user の `~/.cache/ms-playwright` に、`web/package.json` の `@playwright/test` の版に合う
  chromium が要る。無ければ gate が `web-e2e-chromium` で落ちる。
- e2e の退行は WU・統合では見つからず、gate で初めて見つかることがある。そのときは release を出さず、原因を直す task を立てる
  （ADR 2026-10-20 D4 の「gate が最後の砦」のまま）。
