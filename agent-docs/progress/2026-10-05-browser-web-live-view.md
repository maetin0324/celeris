---
title: ブラウザ実行課と web の Live View・操作・承認画面 — 総括
tasks: [01M46W97H391DSFW1XJ745W0G9]
status: done
updated: 2026-10-06
---

# ブラウザ実行課と web の Live View・操作・承認画面 — 総括（close-out）

[ADR 2026-10-05-browser-department-web-live-view](../adr/2026-10-05-browser-department-web-live-view.md)
に従い、ブラウザ実行の部署（browser-specialist profile）の定義と、web の Live View 監視・操作画面・
本人承認・identity 画面、および 2026-10-06 の人の追加要望（web からの browser 設定編集、task ごとの
`allowed_domains`）までを実装・統合した。本ファイルは統合後の HEAD（`a0927c8b`）での全体検査の結果、
兄弟 WU（domain-policy・web-settings）の結果の突き合わせ、未解決事項と提案をまとめる。
各 WU の詳細は `2026-10-05-browser-web-live-view/` 配下（adr・org-node・org-tests・gateway・
web-ui/・domain-policy/・web-settings・real-check）。ADR 末尾の「付記: 実装との突き合わせ」が
決定→実装の対応の正本。

## やったこと（工程の要約）

- **部署（org-node・org-tests）**: `browser-execution`（ブラウザ実行課、section、親 `engineering`、
  genre `coding`）を `config/org.example.toml` と投入 JSON（`docs/ops/browser-department-org.json`）・
  手順（`docs/ops/browser-department.md`）に足した。matching 試験
  `browser_specialist_node_receives_browser_enabled_tasks_and_ungranted_nodes_are_excluded`
  で browser-enabled task が grant を持つ node にだけ割り当たることを固定。`crates/celeris` の
  org 試験 3 件（seed 件数・id 一覧・既定 routing）を更新。
- **gateway（gateway）**: `web/server/browser-live.js` に認可付きの live proxy（HTTP・WebSocket）と
  owner session・control・waits・identities の中継を足した。宛先は起動設定の loopback だけ。
  入力は lease holder・`human_control`・期限内・認証区間外のときだけ転送。generic relay は browser の
  変更経路・live・control・identity を拒否し、JSON と SSE から `live_view_url` を除く。
- **web 画面（web-ui/）**: `web/features/browser/` に run 一覧（`/browser`）・run 画面
  （`/browser/runs/$taskId/$runId`、Live View・control bar・lease・待ち・イベント）・
  identity（`/projects/$id/browser-identities`）を足し、nav・task 詳細・受信箱・承認から辿れるようにした。
  Live View は同一 origin の `/browser/live/...` だけで、raw `live_view_url` は出さない。
- **domain-policy（domain-policy/）**: `allowed_domains` を origin 形式に（ADR
  [2026-10-05-browser-allowed-origins](../adr/2026-10-05-browser-allowed-origins.md)）。
  browser-enabled 新規 task への `requirements.browser.allowed_domains` を必須化し、全作成経路で
  欠落・空・不正を拒否、子 task は親の部分集合。実効許可は task ∩ grant で、broker・egress・credential
  判定・shim が共有。grant 縮小は次の run から。planner・CoS の prompt に最小 origin の規則と例。
- **web-settings（web-settings）**: `/browser/settings` で browser-execution の grant・harness・予算・
  credential/identity 対応を web から編集。`PATCH /api/v1/org/{id}/browser-settings`（検証済み。
  失敗時は DB と event を変えない。成功時は `org_browser_events` に actor と変更前後）。
- **実機確認台本（real-check）**: `scripts/dev/browser-web-live-check.sh` と
  `docs/ops/browser-web-live-check.md`。試験用 DB daemon・別 UID の launcher・loopback 試験ページ・
  web gateway を起動する host 専用 opt-in 台本（未 opt-in で exit 2）。

## 全体検査（2026-10-06、統合後の HEAD `a0927c8b`）

この run（close、attempt 1）で再実行した結果。ログは run の `artifacts/`（test-parallel.log・
clippy.log・web-e2e.log）。

- `bash scripts/dev/test-parallel.sh`: **exit 0（失敗 0）** —
  `CELERIS_TEST_SUMMARY {"passed": 3976, "failed": 0, "ignored": 13, "nextest_exit": 0, "doctest_exit": 0}`
  （nextest 117 binaries + doc-test 10 = 127）。
- `cargo clippy --workspace -- -D warnings`: exit 0（warning 0）。
- `sh scripts/dev/progress-index.sh --check`: exit 0。
- 文書検査: `sh scripts/dev/check-adr-numbers.sh`（ok, 142 files）・
  `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv`（ok）・
  `sh scripts/dev/check-doc-links.sh`（ok） いずれも exit 0。
- `pnpm -C web typecheck`（`tsc -b`）: exit 0。
- `pnpm -C web test`（`vitest run && node --test server/*.test.mjs`）: vitest
  `Test Files 66 passed (66)`・`Tests 428 passed (428)`、node:test `tests 52 pass 52 fail 0`。
- web の e2e（Playwright）を再実行した。
  `WEB_E2E_SCOPE=functional pnpm exec playwright test`（全体）: 225 passed / 8 skipped /
  **2 failed**。failed 2 件は `e2e/shell/home-layout.spec.ts:26`（360x800「ページが viewport より
  高くない」）と `e2e/work/home-stale-viewport.spec.ts`（stale の / 360）で、**main 由来の既知の
  失敗**（`web-ui.md`「未解決事項」で記録済み。この branch の web 差分は browser 画面のみで、
  home の 360px レイアウトに差分が無い）。web-ui 葉と同様の `--grep-invert` で両方を除外すると
  **222 passed / 8 skipped / failed 0**。
  `pnpm exec playwright test e2e/browser`: **21 passed**（settings 2 件を含む。web-ui 葉記録の
  19 件より settings.spec.ts の 2 件が増えたもの）。
- `pnpm -C web build`: exit 0（JS chunk >500 kB の warning は既存）。
- `pnpm -C web check:boundaries`: exit 0。`pnpm -C web check:secrets`: exit 0
  （build 後で「token absent from build output, HTML, /api responses, errors and logs」）。
- `pnpm -C web mobile-audit`: `mobile-audit: 34 path(s) x 4 widths ok`、exit 0。
- `pnpm -C web check:parity`: **exit 1 — `/browser/settings: missing V3 screen`**。
  web-settings 葉（7f91202d）が `web/server/spa-routes.js` に `/browser/settings` を足したが、
  V3 画面台帳 `web/e2e/support/screens.ts` の対応行を足していないため。この close run は
  文書のみを編集する範囲のため未修正（「未解決事項」・「提案」へ）。

## 兄弟 WU の結果の突き合わせ（domain-policy・web-settings）

両 WU は本 task の plan 内で別の子 task として走ったため、この close で結果を突き合わせる。

- **domain-policy**: 要求→実装・試験の対応表を
  [domain-policy.md](2026-10-05-browser-web-live-view/domain-policy.md) に残した。
  試験は `browser_allowed_domains_` 23 件・`browser_settings_`（task-api）9 件、いずれも確定系。
  全体検査 `test-parallel.sh` を domain-policy の close 葉で 0 failed（3976 passed / 13 ignored）を
  確認済みで、本 run の再実行（上の 3976 passed）と件数・結果が一致する。申し送りを 2 点だけ
  反映した: (1) `crates/celeris` の org 試験 3 件は org-tests 葉で修正済み（上の 0 failed に含む）。
  (2) 管理 API の経路は `PATCH /api/v1/org/{id}/browser-settings`（D5.1 の「browser 専用 API は
  新設しない」に対する逸脱。理由と実装は ADR 付記 D2.0/D5 節）。
- **web-settings**: `/browser/settings` と e2e（`web/e2e/browser/settings.spec.ts`）を実装。
  `org_browser_events` の読み取り API が無いため、画面は直前の保存の actor・`updated_at` だけを出す
  （履歴一覧は未実装、別 task に提案）。e2e の拒否系（scheme・`*`・public suffix wildcard）と
  event 不変の検証を domain-policy の API 試験と併せて確定系でカバーしている。
  突き合わせで `check:parity` の漏れ（V3 台帳行の未追加）を発見（上の「未解決事項」・「提案」）。

## 未解決事項

- **`check:parity` が exit 1（`/browser/settings: missing V3 screen`）**: web-settings 葉
  （7f91202d）が route だけ足して V3 台帳行（`web/e2e/support/screens.ts`）を足していない。
  修正は 1 行（`{ path: "/browser/settings", fixture: "/browser/settings", heading:
  "ブラウザ実行課の設定" }`、`v3: true` は browser backend 対応があるまで付けない）。ただし
  `mobile-gate`（nfr project・axe 全画面）が fixture gateway（browser backend 無し）で h1 を
  期待するため、追加時は fixture 側の browser grant 付き org と `mobile-audit` の再実行が要る
  （web-settings 葉の再計画で直す）。本 close run（文書のみ）では直していない。
- **functional e2e の home 360px 失敗 2 件は main 由来の既知の失敗**: `home-layout.spec.ts:26`
  （360x800）・`home-stale-viewport.spec.ts`（stale の / 360）。この branch の web 差分は
  browser 画面のみ。web-ui 葉と同様に除外して残りを 0 failed で確認済み。main 側の修正を待つ。
- **本番の org 投入と実機確認は未実施**: 配送後に Fable が `docs/ops/browser-department.md` で
  `POST /api/v1/org` し、`scripts/dev/browser-web-live-check.sh`（opt-in）で実機確認して証跡を
  ここへ転記する。本 run は本番 DB・daemon には触れていない。
- **egress は `host:port` の完全一致**: wildcard origin（`*.example.com`）は egress で一致せず
  fail-closed に拒否される（以前からの挙動）。実運用では wildcard ではなく単一 origin を指定する。
- **launcher 経路（`browser_launcher`）は prepared policy を受け取るだけ**: 追加の origin 検証は
  本 task の範囲外。
- **`org_browser_events` の読み取り API が無い**: 設定画面は直前の保存の actor・時刻を出すだけ。
  履歴一覧を出すには読み取り API を足す（別 task）。
- **migration 0048〜0050 はこの branch では空番**: 本 branch は 0051 のみを追加した。統合時に
  他 branch 側の 0048〜0050 実体が入れば番号の重複検査で確認する。
- **mobile-audit の browser fixture は owner 状態を持たない**: owner 状態の control bar・待ち form の
  44 px は `web/e2e/browser/narrow-a11y.spec.ts`（360px）が確かめる。
- **web 画面に `v3: true` 未付**: S1 latency・S2 realtime の browser backend 対応が別途要る。
- **CoS 専用の外部 skill file は無い**: 最小 origin 規則は `preamble.rs` の Rust 文字列 1 箇所に
  入っている（prompts 葉の判断）。
- **旧 GUI（`gui/`）の browser 画面の撤去は本 ADR の範囲外**。

## 提案

- **`check:parity` の修復（最優先・小さく確定）**: `web/e2e/support/screens.ts` に
  `{ path: "/browser/settings", fixture: "/browser/settings", heading: "ブラウザ実行課の設定" }`
  （`v3: true` 無し）を 1 行足し、`mobile-gate`・`check:parity`・`check:boundaries`・`mobile-audit`
  を再実行して 0 failed を確かめる。fixture gateway が `browser-execution` node（browser grant 付き）
  を持たない場合は、org fixture に node を足すか settings 画面の fetch 失敗表示で h1 が出ることを
  先に確認する。web-settings 葉の再計画か、次の close で直す。
- **`org_browser_events` の読み取り API**（`GET /api/v1/org/{id}/browser-events`）を足し、設定画面に
  変更履歴一覧を出す（actor・時刻・変更欄の要約のみ）。
- **launcher 経路の origin 検証**: `browser_launcher` が prepared policy をそのまま egress へ渡す
  前提の再確認と、必要なら launcher 側の reject 試験を足す。
- **e2e の browser backend を S1/S2 に使う**: `v3: true` を browser 画面に付け、latency・realtime の
  掃引に入る（fixture 整備が先）。
- **mobile-audit の owner fixture**: `scripts/mobile-audit.mjs` に owner 承認付きの gateway 起動を
  入れ、owner 状態の狭幅画面を自動監査する。
- **実機確認の定期化**: 配送後の Fable 実行を一度きりにせず、release gate 前の opt-in check として
  定期化する（台本は既定で走らないまま）。
