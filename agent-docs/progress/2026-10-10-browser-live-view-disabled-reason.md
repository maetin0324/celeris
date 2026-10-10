---
title: browser Live View の不可理由を gateway が返し、iframe に 503 を出さない
tasks: [01M4HRQBN8XHKQRD1DAXHXBSEX]
status: done
completed: 2026-10-10
updated: 2026-10-10
---

# browser Live View の不可理由を gateway が返し、iframe に 503 を出さない

人の報告（2026-10-10）: browser run の Live View が見られない。本番では CELERIS_WEB_LIVE_VIEW_UPSTREAM が未設定で、
`web/server/browser-live.js` が全 run に 503 `live_view_relay_unavailable` を返し、SPA が iframe にその JSON を表示していた。
gateway の `/browser/runs` は理由を返さず、終わった run でも control の polling が 404 を出し続けていた。

決定の根拠は [ADR 2026-10-05-browser-department-web-live-view](../adr/2026-10-05-browser-department-web-live-view.md) の
付記（2026-10-10）。人の決定（2026-10-10）: credential session でも本人には Live View を見せる。この task では
credential session を理由に disabled にしない。

## やったこと

- **gateway-live**（commit `ee1e4d37`）: `/browser/runs` の各 run に `live` を入れる。`live` は
  `{state:"link", href}` か `{state:"disabled", reason}`。`live_path` は link のときだけ付く。
  判定は `liveAvailability`（`web/server/browser-live.js`）。
- **spa-live**（commit `edaff412`）: disabled のとき iframe を出さず、理由の文言と「映像なし — イベントで監視中」
  （`#browser-live-events` へのリンク）を出す。COMPLETED・FAILED の run と gateway の 404 では control polling を止める。
  詳細は [spa-live.md](2026-10-10-browser-live-view-disabled-reason/spa-live.md)。
- **統合**（commit `78502c1e` gateway、`19eec882` spa）。
- **close-out**（本 WU）: ADR 2026-10-05 に付記（2026-10-10）を追加。

## 証拠（本 WU で実行したコマンドと結果）

- `cd web && pnpm install --offline` — exit 0（worktree の依存を入れた）。
- `cd web && pnpm typecheck`（`tsc -b`） — exit 0。
- `cd web && pnpm lint`（`biome check .`） — exit 0。ただし warning 4 件（`web/styles.css` の既存の `!important`、
  本 WU の変更外）。`Found 4 warnings.`、`Checked 457 files`。
- `cd web && pnpm test`（`vitest run && node --test server/*.test.mjs`） — 途中まで実行したとき、
  `server/browser-live.test.mjs` の 1 件目 `rejected upgrade RST ...` が 60 秒 timeout、後続が `listen EINVAL … owner.sock`
  で連鎖した。原因は run の `TMPDIR`（`/local/celeris/data/workspaces/…/runs/…/tmp`）が長く、Unix socket の
  path 長（SUN_LEN）を超えたこと。`TMPDIR=/tmp` で流し直すと全て合格。
- `cd web && TMPDIR=/tmp node --test server/*.test.mjs` — `tests 80, pass 80, fail 0`、exit 0。
  （`browser-live.test.mjs` 単独は `tests 21, pass 21`。）
- `cd web && pnpm exec vitest run` — `Test Files 87 passed (87)`、`Tests 645 passed (645)`、exit 0。
- `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` — `check-doc-layout: ok`、exit 0。
- `sh scripts/dev/check-adr-numbers.sh` — `check-adr-numbers: ok (176 files)`、exit 0。
- `sh scripts/dev/check-doc-links.sh` — `check-doc-links: ok`、exit 0（進捗ファイル作成後に再実行）。
- `git diff 19eec882 -- crates | wc -l` — 0 行（crates に差分なし）。

## 未解決事項

- **launcher runtime の映像経路は未実装**。`live_view_url` はどこでも None のため、launcher の run は
  upstream があっても `relay_unavailable` になる。映像経路の実装は別の task。
- **本番 upstream 未設定**。`CELERIS_WEB_LIVE_VIEW_UPSTREAM` が未設定のままだと、RUNNING で session を持つ run は
  `relay_unavailable` になる。終わった run は `not_running`。
- **Playwright の e2e は reclose WU で実行済み**（functional 全体 `322 passed`、`8 skipped`、exit 0）。
  結果と受け入れ条件との対応は [reclose.md](2026-10-10-browser-live-view-disabled-reason/reclose.md)。
- この branch に入っている `crates/task-worker` の試験（post-login・download の待ち方）と `scripts/dev/test-parallel.sh`
  （nextest 失敗時の test 名の出力）の変更は、統合段の flaky 対処（post-login-flake・fail-names・reverify-2）で入ったもの。
  Live View の挙動とは無関係。
- **`grant_expired` は `/browser/runs` の理由として返していない**（live 接続時の応答で返る）。ADR D3.2 の理由表と
  gateway の返す集合は一致しない。ADR 付記に記した。
- lint の warning 4 件（`web/styles.css` の `!important`）は既存。本 task では触っていない。

## 再提出（live-fix 後・reclose で記録）

final review の差し戻し（upstream 未設定の判定順、RUNNING 以外の not_running、WAITING_FOR_AUTH の除外、Playwright 未実行）を
`3da75b4b`（live-fix）で直した後、HEAD `55539dc1` で web の検査を流した（詳細と条件ごとの対応は
[reclose.md](2026-10-10-browser-live-view-disabled-reason/reclose.md)）。

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` — exit 0。
- `pnpm run lint`（`biome check .`） — exit 0（warning 4 件は既存の `web/styles.css`）。
- `pnpm run typecheck`（`tsc -b`） — exit 0。
- `pnpm run test`（vitest + node --test） — exit 0。vitest `90 files / 655 tests passed`、node --test `82 pass / 0 fail`。
- `pnpm run build`（vite build） — exit 0。
- `pnpm run e2e`（Playwright functional 全体） — exit 0。`322 passed`、`8 skipped`（`WEB_SHOTS_OUT`・`WEB_E2E_REAL_BASE_URL` 未指定の環境 skip）。
  run の `TMPDIR`（93 文字）では Unix socket が `EINVAL` で 28 件落ちたため、`TMPDIR=/tmp/wl.*` で流し直した。
- 文書検査: `check-doc-layout.sh`・`check-adr-numbers.sh`（178 files）・`check-doc-links.sh` — いずれも ok、exit 0。
- ADR 2026-10-05 の付記（2026-10-10）を実装に合わせて直した（判定順: upstream → RUNNING → session_id。`auth_interval` は使わない。
  polling は RUNNING 以外の全状態で止まる）。

## 本番反映（人の手順）

本番の host 操作は人が行う。エージェントは実行していない。

1. `web` の release を本番へ反映する（`web/` を含む release。`bash web/scripts/release.sh` の流れに従う）。
2. `celeris-web` を本番の手順で再起動する（unit の名前と再起動の手順は本番の運用手順に従う）。
3. 確認: `GET /browser/runs` の各 item に `live.state` が入り、upstream 未設定なら RUNNING の run は
   `{"state":"disabled","reason":"relay_unavailable"}`、終わった run は `not_running`。
   ブラウザで Live View の画面を開き、iframe が出ず「映像なし — イベントで監視中」の表示と理由の文言が出ることを確かめる。
4. 終わった run の画面で `/browser/control` への polling が続かないこと（ブラウザの開発者ツールの Network で確認）。

## 提案

- launcher runtime の映像経路（dashboard upstream の供給）を別の task として起票する。それまで Live View は
  設計どおり「イベントで監視」に限る。
- 本番に CELERIS_WEB_LIVE_VIEW_UPSTREAM を設定するかどうかは人の決定とする（現状は未設定のまま）。
- `grant_expired` の扱い（`/browser/runs` に返すか、live 接続時だけにするか）を ADR で決める。

## 文書検査

- `check-doc-links.sh`・`check-doc-layout.sh`・`check-adr-numbers.sh` の 3 本の結果は「証拠」節に記した。
