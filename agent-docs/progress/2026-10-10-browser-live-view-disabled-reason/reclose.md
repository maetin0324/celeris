---
title: web の lint・Vitest・Playwright の全体記録（live-fix 後の再提出）
tasks: [01M4HRQBN8XHKQRD1DAXHXBSEX]
status: done
completed: 2026-10-10
---

# web の lint・Vitest・Playwright の全体記録（live-fix 後の再提出）

本 WU は実装しない。`3da75b4b`（live-fix）と統合 `55539dc1` の HEAD で web の検査を全部流し、結果を記録する。
前回の final review は Playwright 未実行を差し戻した。本 WU でその穴を埋める。

## 環境

- 作業場所: `repos/agent-platform`（branch `celeris-wu/01M4HRQBN8XHKQRD1DAXHXBSEX/reclose`、base `55539dc1`）。
- run の `TMPDIR` は 93 文字あり、browser gateway の Unix socket（`owner.sock`）が `listen EINVAL`（SUN_LEN 超過）で落ちる。
  Playwright は `TMPDIR` を短い `/tmp/wl.*` に差し替えて流した（socket のための一時 directory のみで、repo の写しは置いていない）。
  最初に run の `TMPDIR` で流した e2e は 28 件が全てこの socket 起因で落ちた（後述「失敗した試行」）。

## 実行結果（条件ごと）

- 条件: 依存の導入
  - 実行: `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`
  - 結果: exit 0（`Done in 513ms`）。
- 条件: lint
  - 実行: `cd web && corepack pnpm@12.6.0 run lint`（`biome check .`）
  - 結果: exit 0。`Checked 462 files`、`Found 4 warnings.`。warning 4 件は `web/styles.css` の既存の `!important`（本 WU・live-fix の変更外）。
- 条件: typecheck
  - 実行: `cd web && corepack pnpm@12.6.0 run typecheck`（`tsc -b`）
  - 結果: exit 0。
- 条件: Vitest と node --test（`pnpm run test` = `vitest run && node --test server/*.test.mjs`）
  - 実行: `cd web && corepack pnpm@12.6.0 run test`
  - 結果: exit 0。
    - vitest: `Test Files 90 passed (90)`、`Tests 655 passed (655)`。
    - node --test: `tests 82`、`pass 82`、`fail 0`。
- 条件: build
  - 実行: `cd web && corepack pnpm@12.6.0 run build`（`vite build`）
  - 結果: exit 0（`built in 421ms`）。chunk サイズの警告のみ。
- 条件: Playwright functional（全体）
  - 実行: `cd web && TMPDIR=/tmp/wl.UBXsQN corepack pnpm@12.6.0 run e2e`（`WEB_E2E_SCOPE=functional playwright test`）
  - 結果: exit 0。`322 passed (2.1m)`、`8 skipped`。
  - skip 8 件は `test.skip` の環境条件によるもの（`WEB_SHOTS_OUT` 未指定の parity 画面撮影 7 件、`WEB_E2E_REAL_BASE_URL` 未指定の staging 読み取り 1 件）。
    これは試験の不具合ではなく、既定の run で走らない試験。
  - 本 WU の受け入れに関わる spec は `e2e/browser/live-view-disabled.spec.ts` の 2 件とも passed（upstream 未設定の全 run が `relay_unavailable`、WAITING_FOR_HUMAN で iframe を出さない）。
- 条件: 文書検査 3 本
  - `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — `check-doc-layout: ok`、exit 0。
  - `sh scripts/dev/check-adr-numbers.sh` — `check-adr-numbers: ok (178 files)`、exit 0。
  - `sh scripts/dev/check-doc-links.sh` — `check-doc-links: ok`、exit 0。

## 受け入れ条件との対応

- 条件 1（upstream 未設定なら全 run が `relay_unavailable`、iframe は出ない）
  - 試験: vitest の `server/browser-live.test.mjs`（`liveAvailability(run, null)` の `relay_unavailable`、RUNNING 以外でも upstream 未設定なら `relay_unavailable`）、
    Playwright `live-view-disabled.spec.ts` の 1 件目（gateway 全 run の応答が `disabled` / `relay_unavailable`、画面に理由と「映像なし — イベントで監視中」）。
  - 結果: 合格。
- 条件 2（upstream ありで RUNNING 以外は `not_running`、WAITING_FOR_AUTH を独自に除外しない、polling 停止）
  - 試験: `server/browser-live.test.mjs`（WAITING_FOR_AUTH 等の run が `not_running`）、`features/browser/control-bar.test.tsx` の `liveViewState`（認証区間フラグが true でも RUNNING なら frame。SPA は認証区間で除外しない）、`features/browser/browser-query.test.ts`（`stops control polling for every non-running run and 404 responses`：WAITING_FOR_AUTH・WAITING_FOR_HUMAN・WAITING_FOR_APPROVAL・FAILED・その他で `refetchInterval` が false）。
  - 結果: 合格。
- 条件 3（web 試験）: lint・typecheck・vitest・node --test・build・Playwright functional 全体がいずれも exit 0。

## 失敗した試行（記録）

- run の `TMPDIR`（93 文字）で `pnpm run e2e` を流した → exit 1、`28 failed / 294 passed / 8 skipped`。
  28 件の失敗は全て `listen EINVAL: invalid argument …/owner.sock`（SUN_LEN 超過）による。試験の不具合ではない。
  `TMPDIR` を短くして流し直すと `322 passed`、exit 0。

## 未解決事項

- なし（本 WU の範囲で）。
- Playwright の skip 8 件（parity の画面撮影と staging 読み取り）は、`WEB_SHOTS_OUT`・`WEB_E2E_REAL_BASE_URL` を指定した別の run で流す必要がある。本 WU の受け入れには含まない。
- lint の warning 4 件（`web/styles.css` の `!important`）は既存。

## 提案

- browser gateway の Unix socket は run の `TMPDIR` が長いと必ず落ちる。`e2e` の全体 run は短い `TMPDIR` で流す手順を運用文書に書く（別の task で扱う）。
