---
title: web の e2e:nfr・nfr-latency の既存の不合格 7 件を直す
tasks: [01M7HKKB3ZKPFEGYQ6P0J36KTC]
status: done
updated: 2026-11-16
---
# web の e2e:nfr・nfr-latency の既存の不合格 7 件を直す

完了日: 2026-11-16

人の決定（2026-11-16、運用 session の問いへの答え）による task。task 01M7H1RNG5C5AA76HNB2M9M098 の計測
（[進捗](2026-11-16-web-e2e-release-gate.md) の「未解決」）で見つかった、release gate に入っていない nfr・nfr-latency の
不合格 7 件を、いまの意図した挙動に合わせて直した。assert は弱めておらず、skip も足していない。

## 変更（commit の件名）
- `phase 1: e2e の nfr・nfr-latency の既存の不合格 7 件を直す`（spec・probe・cron 詳細の見出し・単体試験・この進捗・
  `agent-docs/web/gates/p5-01-latency.md` の付記）

## 7 件の原因と直し方

| 件 | 原因 | 直し方 | いま確かめていること |
|---|---|---|---|
| `parity/mobile-gate.spec.ts:27`（axe `/browser/settings`） | 設定 form は本人（owner）の session にだけ出る（ADR 2026-10-08-browser-prod-enablement D3「web の扱い」、10-08 から）。試験は owner の無い gateway で開くので、本人確認の案内だけが出て form（許可 origin 2 件）が無かった | この画面だけ owner socket を持つ browser gateway（`startBrowserGateway`）で本人として login して開く | 4 幅で form の許可 origin が 2 件あり、axe の critical/serious 0、横溢れ 0（変更前と同じ assert） |
| `realtime/refetch-scope.spec.ts:31`（S2 `/accounts`） | `/accounts` の「登録待ちの work unit」（`features/ops/credential-waits.tsx`、10-15 の cw-web）は受信箱の `kind=credential_wait` を 5 s ごとに読み直す（cw-web の進捗に「item の再取得は 5 秒間隔」と書かれた設計）。probe は path だけを記録するので、shell の受信箱（15 s の poll）と別の query の timer を区別できず、20 s の仮想時間で 5 回（timer 4 + shell 1）と数えていた。SSE による取り直しではない | probe（`e2e/support/realtime-probe.ts`）が path と query を記録する。画面の timer の query を表（`SCREEN_TIMER_POLLS`）に持ち、進めた仮想時間の分（20 s / 5 s = 4 回）までを許す。表の query 以外は従来どおり path で数える | shell の受信箱 ≤ 2、`/api/v1/tasks` 0、画面の query 0 は変更前と同じ。加えて credential_wait の読み直しが timer の 4 回以下（上限を 1 減らすと 4 > 3 で落ちることを確かめた） |
| `latency/transition.spec.ts:24` S1 `/` | ホームは統括 CoS を開き URL を `/?thread=chat-main` に置き換える（ADR 2026-10-28-single-human-window-general-cos D5）。試験は `/` を期待し、しかもホームからホームの link を押していた | URL の終点を `/?thread=chat-main` にし、遷移は `/help` から始める（現在地と同じ link は遷移しない。a11y/axe.spec.ts と同じ） | `/help` → ホームの click から `/?thread=chat-main` と見出しまで 300 ms 以内、10 s−0 s 差 100 ms 以内（遅延 0/5/10 s） |
| S1 `/knowledge`・`/knowledge/inbox`・`/tasks/$id/runs/$runId` | 失敗は `toHaveURL` ではなく「画面データ」の locator（h1 の親の h1 以外の子）だった。page header（パンくず・主操作）を持ち説明の無い画面では h1 の親に h1 しかない | 画面データを、画面の枠（`[data-screen]`）の子のうち h1 と page header を除いた本文で測る | 予算・差・retries と URL・見出しの測り方は変えていない |
| S1 `/cron/$id` | 上の locator に加え、詳細の見出しが job 名で、詳細の JSON を待っていた（10-11 の追加時から。遅延 5 s で見出しが出ない）。一覧の job 名は詳細への SPA のリンクなのに in-app route の表に無く、pushState で測っていた | 画面: 一覧で取得済みの名前を先に見出しに出す（`features/cron/cron-screen.tsx`、ADR-0081 D7）。試験: `in-app-routes.ts` に `/cron` → `/cron/daily` を足した | 一覧の click から見出し「知識ベースの日次整理」まで 300 ms 以内。単体試験 `cron_detail_heading_uses_the_listed_name_before_the_detail_arrives`（変更を外すと落ちる） |

task の objective は S1 の 5 件を「どれも `toHaveURL('/')` に対して `/?thread=chat-main`」としていたが、URL で落ちていたのは `/` だけで、
残り 4 件は画面データの locator（`/cron/$id` は加えて見出しが JSON 待ち）だった。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- 受け入れ 0: `pnpm -C web install --frozen-lockfile --offline && pnpm -C web exec playwright test parity/mobile-gate.spec.ts realtime/refetch-scope.spec.ts && pnpm -C web exec playwright test latency/transition.spec.ts`
  → exit 0。nfr 81 passed（20.3 s）、nfr-latency 37 passed（37.8 s）。全体 59 s
- 受け入れ 1: `pnpm -C web typecheck && pnpm -C web test` → exit 0。vitest 103 files / 716 tests passed、node test 113 pass / 0 fail
- 名指しの追加確認: `pnpm -C web exec playwright test parity/latency-gate.spec.ts` → exit 0（3 passed、21 s）、
  `WEB_E2E_SCOPE=functional pnpm -C web exec playwright test parity/cron-check-logs.spec.ts` → exit 0（3 passed）
- `pnpm -C web exec biome check`（変えた file）→ 0 error
- S1 の 5 件の URL / 見出しの最大（遅延 0/5/10 s）: `/` 70 / 86 ms、`/knowledge` 39 / 47 ms、`/knowledge/inbox` 20 / 39 ms、
  `/tasks/$id/runs/$runId` 30 / 38 ms、`/cron/$id` 26 / 59 ms
- e2e の全体・`e2e:nfr`・`e2e:all` は流していない（task の指示。直した spec と影響のある spec を名指しで流した）。
  Rust は変えていないので test-affected・clippy・fmt は対象外

## 未解決
- 偽 daemon の chat API（`/api/v1/chat/`）は JSON の遅延（`setDelay`）の対象外。S1 `/` の `/?thread=chat-main` への置き換えは
  統括 CoS の一覧の応答を待つので、本物の daemon が遅いと URL の置き換えも遅れる。S1 はこの遅れを測っていない。
- `/cron/$id` を一覧を経ずに開く（chat の札・URL の直打ち）と、見出しは詳細の JSON が来るまで「定期実行の詳細」のまま。
- `biome format` を web の e2e・features 全体に掛けると、この task の外の 4 file（`e2e/chat/queue.spec.ts`・
  `e2e/shell/home-layout.spec.ts`・`e2e/support/fake-daemon.mjs`・`features/tasks/wait-reason.tsx`）に既存の整形差分が出る。
  この task では直していない。

## 提案
- **e2e:nfr を release gate に入れる（ADR 2026-11-16-release-gate-web-e2e D3）。** 所要時間は task 01M7H1RNG5C5AA76HNB2M9M098 の
  計測で 39 秒（125 件）。うち、この task で直した 2 spec は 81 件・20 秒（受け入れ 0）で、残りの a11y/axe は 44 件。不合格は
  この 2 件だけだったので、直した後は全件が通る見込み（e2e:nfr の一括はこの task では流していないので未確認）。
  時間の閾値を持たない（axe・横溢れ・仮想時間の再取得の数）ので、gate の host の負荷では揺れにくい。
  入れるなら `web-pnpm-e2e` の後に `web-pnpm-e2e-nfr`（`corepack pnpm@<版> e2e:nfr`）を足し、
  `scripts/selfdeploy/tests/release_web_required.sh` に e2e:nfr の失敗で `failed_step` が出る組を足す（別の task）。
- **nfr-latency（`latency/transition`・`parity/latency-gate`）は gate に入れない。** 所要時間は 37 件・38 秒と 3 件・21 秒で短いが、
  300 ms・100 ms の時間の閾値を持ち、release gate の host の負荷（並走する task の cargo）で揺れる。retries 0 のまま gate に
  入れると負荷による不合格で release が止まる。入れるなら gate の host の負荷を測ってから別に決める。
- 偽 daemon の chat API にも JSON の遅延を掛けるかは別に決める（上の「未解決」の 1 件目）。掛けると S1 `/` は統括 CoS の
  一覧を待つので、ホームが統括 CoS の thread id を一覧の取得前に決められるようにする（画面の変更）か、S1 の `/` の終点を
  分けるかの判断が要る。
