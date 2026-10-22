---
title: Web の設定画面・routing 画面の API path 二重前置（404）
status: done
updated: 2026-10-22
completed: 2026-10-22
tasks: [01M5JAYKJ5W80FD5954CXHDNEX]
---
# Web の設定画面・routing 画面の API path 二重前置（404）

- 原因: web の api client の規約は `/api/x`（gateway の server/relay.js が daemon の `/api/v1/x` へ前置する）。設定画面・routing 画面が `/api/v1/...` を渡していたので daemon には `/api/v1/v1/...` が届き 404 になった。
- 点検（web 全体）: 同じ誤りが他に 2 か所あった。
  - `api/realtime/transport.ts` の接続確認 `/api/v1/health`（404 でも "ok" 扱いなので表に出ていなかった）→ `/api/health`。
  - 統合操作 `features/tasks/task-integration.tsx`（task 詳細の `integrationActionPath` と、受信箱が daemon の `answer.native.path`＝`/api/v1/tasks/…/merge` をそのまま使っていた）→ `/api/tasks/…`、daemon 由来の path は `apiPathFromDaemon` で直す。
- 再発防止: `assertApiPath` が `/api/v1`（`/api/v1/`・`/api/v1?`・`/api/v1` 単独）で始まる path を TypeError で拒む。`api/client.test.ts` の source scan が client code（試験・生成物以外）に `"/api/v1` の文字列 literal が無いことを確かめる。
- 試験: 設定・routing 画面は query を `configFieldsQuery` などに切り出し、`api/fake-gateway.ts`（relay と同じ前置で fixture を引く）で要求 path と画面の表示を単体試験。e2e `e2e/admin/settings-routing.spec.ts` は本物の gateway＋fake daemon で値の表示と `/api/v1/v1/` が無いことを確かめる。

## 証拠
- `pnpm exec vitest run`（web）: 96 files・676 tests pass。
- `pnpm typecheck`・`pnpm exec biome check .`: exit 0（warning 4 件は既存）。
- `playwright test e2e/admin/settings-routing.spec.ts`: 2 passed。shell/realtime/help/gateway-relay: 15 passed。
- attempt 2（HEAD 0d2987b7 の上で再確認）: `bash scripts/dev/test-affected.sh` exit 0・約 11 秒（web unit: vitest 96 files・676 tests、node --test server 93 tests）。`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` exit 0。
- 前回 attempt の決定的 check で test-affected が 1200 秒で時間切れになった。同じ HEAD で再実行すると 11 秒で通り、原因は再現しなかった。cargo 試験は走らず（crates の差分なし）、web の unit 試験と `cargo metadata` だけが走る。check の環境側で詰まった可能性がある（未特定）。

- attempt 3: daemon の決定的 check は 2 回とも test-affected が止まった（events: 18:17:31 開始 → 600 秒で時間切れ → 1200 秒の再実行も時間切れ → 18:47:43。reviewer run は起動前で並走なし、main は動いていない）。手元では pipe・新 session・env -i・`ulimit -n 1024`（daemon の soft 上限）・2 本並走のどれでも 10〜23 秒で exit 0、原因は未特定。
  - 対処: `scripts/dev/test-affected.sh` の web 分岐を corepack＋pnpm 経由から、`web/node_modules/.bin` を PATH に足して test script を `setsid sh -c` で直に走らせる形に変えた（pnpm run と同じ解決。止まりうる層を 1 つ減らす）。watchdog（`TEST_AFFECTED_WEB_TIMEOUT`、既定 300 秒）が時間切れで止まっている process の木を stderr に出して session ごと止める。check の 600 秒より先に失敗し、理由の stderr_tail に証拠が残る。
  - 証拠: `bash scripts/dev/test-affected.sh` exit 0・12.6 秒（vitest 96 files・676 tests、node --test 93 pass。全出力は artifacts/test-affected-attempt3.log）。`TEST_AFFECTED_WEB_TIMEOUT=3` で exit 143・3.4 秒、process の木を出し、vitest の残りなし。`bash scripts/dev/tests/test-affected-list.sh` ok。`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` exit 0。

- attempt 4（daemon でだけ止まる原因の修正。operator の特定による）: daemon は TMPDIR を `<ws>/runs/exec-tmp-*`（79 byte）に向ける。`web/server/browser-live.test.mjs` の socket dir は `owner.sock` が sun_path（107 byte）に収まるかだけで場所を決めていたが、startWeb は `device-owner-<n>.sock` を作るので 109 byte になり listen が EINVAL。旧 startWeb は owner socket の失敗で reject しても http server を開いたまま残し、node --test が 'Promise resolution is still pending' のまま止まった。手元の TMPDIR（93 byte）では `owner.sock` も収まらず /tmp に逃げていたので再現しなかった。
  - 対処: `web/server/socket-path.js` に `assertSocketPath`（107 byte を超える path を path と byte 数入りの error にする）と `makeSocketDir(prefix, longestName)`（最長の socket 名が収まらなければ /tmp 直下に私有 dir）を置いた。本体の `startSocket` は listen 前に `assertSocketPath` で明示の error にする（`server/index.js` の catch が exit 2 にする）。試験 2 本と e2e の browser gateway は `makeSocketDir` を使う。startWeb は http server の error でも reject し、owner socket が開けなければ http server を閉じて reject する。frames-prod 試験は一時 dir と socket dir を after で消す（以前は /tmp/cbp-* が残っていた）。
  - 試験: `web/server/socket-path.test.mjs` が 107/108 byte の境界、長い base での /tmp への退避と 0700、startSocket の明示の error、TMPDIR が 80 byte（owner.sock は収まり device-owner-N.sock は収まらない幅）と 110 byte のときに子の `node --test server/browser-live.test.mjs server/browser-live-frames-prod.test.mjs` が全件通ることを確かめる。修正前の browser-live.test.mjs を 80 byte の TMPDIR で走らせると 1 件失敗して宙づり（40 秒で kill）になることを手元で確かめた。
  - 証拠: 79 byte の TMPDIR・stdin null で `bash scripts/dev/test-affected.sh` exit 0（vitest 96 files・676 tests、node --test 98 pass。artifacts/test-affected-tmpdir79.log）。通常の環境で exit 0・10.8 秒（artifacts/test-affected-attempt4.log）。`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` exit 0。`tsc -b`・`biome check .` exit 0（warning 4 件は既存）。e2e は release gate だけで走る（型検査のみ）。

- delivery-repair（a51b48fc の release 準備が cargo-test で失敗）: release gate の全体試験 6318 件中 `task-worker browser_egress::tests::egress_test_loopback_connect_allowed_only_when_listed` だけが 403 で落ちた（2.05 秒、tests.rs:654 の「listed なら 200」）。
  - 原因: 試験の `echo_listener` は accept を 1 回しかしない。他の試験・process が自分の閉じた port（bind して drop した「閉じた resolver」など）へ接続すると、kernel が同じ port 番号をこの listener に渡していた場合、その接続が accept を使い切って listener が消え、proxy 自身の connect が拒否されて `Transport` → 403 になる。stray 接続を 1 本先に入れると旧 listener で同じく 2.01 秒・403 で落ちることを確かめた。
  - 対処: `echo_listener` は 1 byte を echo できた接続が来るまで accept を続ける（接続ごとに task）。試験の冒頭で stray 接続を 2 本（すぐ閉じるもの・開いたまま黙るもの）入れて決定的に再現する。
  - test-affected の実行中に `celeris-campaign::arm_set_cli_normal` の 3 件が `bad_auth=2` で落ちた（以前 attempt で再現しなかったもの）。stub が受けた 2 件は token の無い `GET /` で、送り手は `User-Agent: Go-http-client/1.1`。host 上の試験外の Go の process が loopback の port を probe している（単独の実行でも時々来る）。CLI は常に token 付きで `/api/` 配下だけを叩くので、stub は「token 無しかつ `/api/` 外」の要求を 404 で返して数えない（user-agent を stderr に出す）。`flow` の冒頭で同じ `GET /` を送り、フィルタを外すと 4 件落ちることを確かめた。
  - 証拠: `bash scripts/dev/test-affected.sh` exit 0（nextest 4450 passed・15 skipped、artifacts/test-affected.log）。egress 試験を単独で 10 回 pass。`cargo clippy --workspace -- -D warnings`・`cargo fmt --all -- --check` exit 0。

## 本番への移行
- 無し（web の release で反映）。
