---
title: ブラウザ実行課と web の Live View・操作・承認画面 — 総括
tasks: [01M46W97H391DSFW1XJ745W0G9]
status: running
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

統合後の HEAD（`a0927c8b`。文書 commit `eb3c4e60` はその上に乗り、コード検査には影響しない）で
全項目を再実行した結果。最初の close run（`eb3c4e60`）の結果を、close の 2 回目
（run `01M47HVMHSX9MW1S9EFDPS8TMV`、2026-10-06）でも全項目再実行し、件数・結果が一致すること
を確認した。後者のログは run の `artifacts/`
（test-parallel-close2.log・clippy-close2.log・web-typecheck-close2.log・web-test-close2.log・
web-build-close2.log・web-boundaries-close2.log・web-secrets-close2.log・mobile-audit-close2.log・
web-parity-close2.log・web-e2e-close2.log）。

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

## 実機確認: review-fix と launcher-fix の結果、rerun-evidence の回答（2026-10-06）

- **review-fix**: final review 指摘の 2 件を修正した。`/browser/settings` を画面台帳
  （`web/e2e/support/screens.ts`）に fixture 付きで追加（`e2e:nfr` 103 件成功・exit 0）、
  `config/org.example.toml:136` の参照を実在の手順 `docs/ops/browser-department.md` と
  実際の API `PATCH /api/v1/org/browser-execution/browser-settings` に直した。
  記録は [review-fix](2026-10-05-browser-web-live-view/review-fix.md)。
- **launcher-fix**: 実機確認（attempts 1〜8、tree `8aecea65`）で見つかった launcher 経路の
  欠陥（D4 action socket path 107 byte 超過・D6 shim policy / `policy_sha256` 欠落・egress の
  origin 照合）と確認台本の欠陥（D1〜D3・D5・D7）を、統合 commit `d8f3e5e6`（`0e167295` で
  統合）で修正した。記録は [launcher-fix 進捗](2026-10-06-browser-launcher-fix.md)。
- **rerun-evidence（人の決定、回答 `fable-rerun`）**: launcher-fix 統合後、Fable が
  tree `0e167295`（`d8f3e5e6` を祖先に持つ。証跡の `versions.txt`・`git merge-base --is-ancestor`
  で確認済み）で `scripts/dev/browser-web-live-check.sh` を再実行し、証跡を
  `real-check-evidence/rerun-2026-10-06/` に commit（`3680b70e`）。**結果は不合格（FAIL）**:
  D4・D6 修正は実機で効いていた（run 起動・`policy_sha256` あり・`launch` 許可）が、
  loopback の試験ページが launcher egress の設計（IP literal・private/loopback 拒否）で
  開けず、許可 origin の navigate 成功・範囲外 origin の egress 拒否の証跡・screenshot を
  取得できなかった。再実行で R1（台本 settings URL）・R2（gateway の拒否 upgrade 時に
  未処理 ECONNRESET で node 全体が落ちる）・R3（loopback 試験ページを egress で開けない、
  egress が拒否を記録しない）・R4（cleanup が socket を残す）と、別 session の Live View
  拒否・lease 保持時の入力転送のカバー欠けが見つかった。close せず、R1〜R4 の修正後に
  Fable が 3 回目の再実行を行う。各段の結果と経緯（attempts 1〜8 → launcher-fix → 再実行）は
  [real-check.md](2026-10-05-browser-web-live-view/real-check.md) に記録する。
- **rerun 3・4（人の replan で追加の修正葉の統合後）**: 3 回目（tree `d750b121`、証跡
  `real-check-evidence/rerun3-2026-10-06/`、commit `c2696fee`）は FAIL — R1〜R4 は直っていたが
  R5（launcher の session `policy.json` に `launch` が無く全 action 拒否）・R6（egress が
  CONNECT のみ）・R7（拒否記録の session id 探索）・R8（禁止 origin が egress に届かない）
  が見つかり、修正葉（[2026-10-06-browser-r5-r8-fix](2026-10-06-browser-r5-r8-fix.md)）を
  統合。4 回目（tree `bee1c2e4`、証跡 `real-check-evidence/rerun4-2026-10-06/`、commit
  `884c7baf`）は FAIL — R5〜R8 は直っていたが F1（shared CDP relay が idle 中に
  `Page.loadEventFired` を転送せず `open` が全件 25 s タイムアウト）・F2（拒否収集が
  pause/deny 後に走る）・F3（`ip_literal`/`private_address` の誤り）・F4（session dir に
  subuid 所有の `.cache`/`.config` が残る）が残る。原因調査は
  [f1-diagnosis-2026-10-06/](2026-10-05-browser-web-live-view/real-check-evidence/f1-diagnosis-2026-10-06/)
  （commit `4cc2b535`）、修正は [2026-10-06-browser-cdp-fix](2026-10-06-browser-cdp-fix.md)。
- **rerun 5（5 回目、2026-10-06、commit `15c23817`）**: tree `f5b9dc4e`（cdp-fix 統合後、
  `versions.txt` の binary・script・doc sha256 で確認）で Fable が再実行し、
  **Result: PASS**（コード・台本・手順書とも未パッチ）。open 0.45 s・snapshot 0.10 s・click
  0.10 s（F1 修正）、`ip_literal` の egress 拒否記録（F2/F3）、session dir 削除（F4）、
  Live View 200/401/403・RST 耐性（R2）・lease・入力の転送と拒否・waits・settings・cleanup
  すべて合格。証跡は `real-check-evidence/rerun5-2026-10-06/`（本 WU branch の merge
  `0b582b8d` で取り込み済み）。軽微な残りは 2 件: 手順書の HOME 上書き時の
  `PLAYWRIGHT_BROWSERS_PATH` 注記（`docs/ops/browser-web-live-check.md` に反映済み）と、task
  文言の『read #inside』が wrapper の interactive-only snapshot では参照が付かない点（台本は
  検査せず、Playwright が確認するため無害）。1〜5 回の各段の結果と欠陥の要約は
  [record6.md](2026-10-05-browser-web-live-view/record6.md) と [real-check.md](2026-10-05-browser-web-live-view/real-check.md) に記録する。

## 未解決事項

- ~~**実機再確認が不合格（rerun-evidence 回答 `fable-rerun`）**~~: **rerun 5（commit `15c23817`）で
  PASS 済み**。R1〜R4・R5〜R8・F1〜F4 はすべて修正・統合済み（上「rerun 3・4」・「rerun 5」）。
  残る軽微な 2 件のうち `PLAYWRIGHT_BROWSERS_PATH` 注記は
  `docs/ops/browser-web-live-check.md` に反映済み。詳細は
  [record6.md](2026-10-05-browser-web-live-view/record6.md)。
- ~~`check:parity` が exit 1（`/browser/settings: missing V3 screen`）~~: **review-fix で修正済み**
  （`web/e2e/support/screens.ts` に `/browser/settings` を fixture 付きで追加、rich fixture に
  `browser-execution` node を加え `e2e:nfr` 103 件成功・exit 0。記録は [review-fix](2026-10-05-browser-web-live-view/review-fix.md)）。
- **functional e2e の home 360px 失敗 2 件は main 由来の既知の失敗**: `home-layout.spec.ts:26`
  （360x800）・`home-stale-viewport.spec.ts`（stale の / 360）。この branch の web 差分は
  browser 画面のみ。web-ui 葉と同様に除外して残りを 0 failed で確認済み。main 側の修正を待つ。
- **本番の org 投入は未実施・実機確認は合格（rerun 5）**: 本番 org への投入（`docs/ops/browser-department.md`
  で `POST /api/v1/org`）は配送後に Fable が行う。実機確認は attempts 1〜8（tree `8aecea65`）から
  5 回（rerun 5、tree `f5b9dc4e`、commit `15c23817`）まで実施済みで、5 回目は PASS
  （上「rerun 5」。欠陥 R1〜R4・R5〜R8・F1〜F4 の全修正が統合済み）。
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

- ~~**実機再確認の R1〜R4 修正（次段・最優先）**~~: **完了（rerun 5 で PASS）**。R1〜R4・R5〜R8・
  F1〜F4 の全修正が統合済みで、5 回目の実機確認が合格（上「rerun 5」）。
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
