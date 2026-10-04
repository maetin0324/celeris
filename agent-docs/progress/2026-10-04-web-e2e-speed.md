---
title: web の e2e を速くする（refetch-scope の決定化・functional/nfr 分割・並列化・二重 build の廃止）
tasks: [01M43WFDREMHYKE45ND6YVH10W]
status: done
updated: 2026-10-04
---

# PROGRESS — web の e2e の高速化

## したこと

- `web/e2e/realtime/refetch-scope.spec.ts`（S2、30 画面）の 1 画面 20 s の固定待ちをやめた。
  - page の時計を Playwright の clock に差し替え、画面が出て取得が落ち着いたら止める（`clock.pauseAt`）。
    束ね窓 250 ms・受信箱の 15 s poll・2 s の daemon tick は `clock.runFor(2000)` × 10（仮想 20 s）で進める。
  - 試験側の観測点 `web/e2e/support/realtime-probe.ts`（addInitScript。EventSource の listener と window.fetch を
    包む。画面の実装は変えない）で「app が SSE を処理し終えた件数」と「呼んだ取得（daemon の path に揃える）」を読む。
    関係のない task.event 20 件・daemon 10 件を処理し終えたことを待ってから時計を進め、取得の進行中が 0 に
    なるのを待つ。
  - 最後に受信箱を取り直す関係のある event（`created`）を目印に流し、取得が呼ばれることを確かめる
    （観測の空振りでないことの証拠）。目印を処理し終えた時点までの記録で、画面の取得 0 本・/tasks 0 本・
    受信箱 2 本以下を確かめる（検査内容は修正前と同じ）。
- `web/playwright.config.ts` を project に分けた（`WEB_E2E_SCOPE` で選ぶ）。
  - `functional`（80 件）: parity・shell・scaffold の機能。`pnpm -C web e2e`。
  - `nfr`（92 件）: `a11y/axe`・`parity/mobile-gate`・`realtime/refetch-scope`。
  - `nfr-latency`（33 件）: `latency/transition`（S1）・`parity/latency-gate`。他の project の後に既定 4 worker
    （`WEB_E2E_LATENCY_WORKERS`）。retries 0 のまま。
  - `release`（1 件）: `parity/cutover`。`scripts/release.sh` が dist/ を build し直すので、他の project の後に 1 worker。
  - `pnpm -C web e2e:nfr` は nfr + nfr-latency、`pnpm -C web e2e:all` は全部。
- 並列化: `fullyParallel: true`、workers は CPU 数の半分（上限 8）。`WEB_E2E_WORKERS` で上書き（数か "50%"）。
  偽 daemon・gateway は試験ごとに loopback の port 0 で起こしていて worker 間で共有しない。module で作る
  一時 dir と beforeAll の server を file 内で共有する 12 file（parity の console・org・gateway-relay・
  gateway-auth・reports・realtime・help・gateway・knowledge・shell・inbox・ops）は
  `test.describe.configure({ mode: "default" })` で file 内を 1 worker・順に流す（file どうしは並列）。
- 二重 build の廃止: webServer は `web/e2e/support/ensure-dist.mjs` を通し、dist/ が無いか build の入力
  （e2e・server・scripts・tsbuildinfo などを除く）が dist/index.html より新しいときだけ `vite build` する。
  `pnpm build && pnpm e2e` では `e2e: dist/ は最新なので build しない` と出る。
- `agent-docs/guides/testing.md` に例（出来事待ち＋時計）と WU の check の書き方（対象画面の functional spec、
  nfr は visual-qa・最終の受け入れ・release gate）を足した。

## 所要時間（同じ host、24 CPU、2026-10-04）

| 区分 | 修正前（HEAD 08c66fa6、workers 1） | 修正後（workers 8） |
| --- | --- | --- |
| functional | 80 件、試験時間の和 170 s | 80 件（72 passed・8 skipped）、81 s（壁時計、pnpm 起動込み） |
| nfr | 125 件、試験時間の和 816 s（うち refetch-scope 606 s、S1 141 s） | 125 passed、74 s |
| all | 205 件、1034 s（17.2 m）。196 passed・1 failed・8 skipped | 205 件（197 passed・8 skipped）、134 s（2.2 m） |

- 修正前は分割が無いので functional・nfr は 1 回の全体実行の試験ごとの時間を file で振り分けた和。
  修正前の 1 failed は計測用に `web/` と `scripts/` だけを写した場所で走らせたため `gen-types --check` が
  `docs/api` を読めなかったもの（環境要因。修正後は同じ試験が通る）。8 skipped は両方とも staging 指定時だけの試験。
- 全体は修正前 17.2 m の 13%、人の指摘の 11.6 m の 19%（目標 1/3 以下を満たす）。
- refetch-scope 単体: 30 件 4.6〜4.8 s（修正前 606 s）。

## 証拠

- 修正前: `corepack pnpm@12.6.0 build && corepack pnpm@12.6.0 e2e`（HEAD の web/・scripts/ の写し）→ `Running 205 tests using 1 worker`、
  `196 passed (17.2m)`、e2e 1034 s。
- `corepack pnpm@12.6.0 -C web e2e` → exit 0、81 s、72 passed・8 skipped。
- `corepack pnpm@12.6.0 -C web e2e:nfr` → exit 0、74 s、125 passed。
- `corepack pnpm@12.6.0 -C web install --frozen-lockfile && … typecheck && … e2e:all` → exit 0、136 s、197 passed・8 skipped。
- 反例（わざと関係のない SSE で取り直す実装）: `web/api/realtime/invalidation-map.ts` の `worker_progress` を
  `sets: ["L", "N", "P"]` にして `e2e:nfr realtime/refetch-scope.spec.ts` → exit 1、7 failed・23 passed
  （/tasks・/board・/projects・/projects/$id・/reports・/approvals・/artifacts。例 `/reports: unrelated SSE refetched
  /api/v1/reports`）。変更は戻した。
- dist/ を退けて `pnpm -C web e2e scaffold.spec.ts` → `e2e: dist/ が無いので build する`、1 passed。

## 未解決事項

- `pnpm -C web e2e <nfr の file>` は functional に無いので「No tests found」で落ちる。nfr の file は `e2e:nfr` で指定する。
- 古い gate 文書（`agent-docs/web/gates/p5-01-latency.md` など）の実行記録は当時のコマンドのまま残した。

## 提案

- 12 file の module 共有状態（一時 dir・server）を試験ごとの fixture にすれば file 内も並列にでき、
  最長の `parity/gateway-relay`（約 60 s）が短くなる。画面改修の task が終わった後に行う。
