---
title: "本番で Live View の映像が出ない: gateway の guard が解決済みの credential 待ちを認証区間と取り違える"
tasks: [01M4GYJ3XGJNWZQDF35F1MDE0H, 01M4JAK3MY5G1K8T66Q4RTWSS5]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# Live View fix（2026-10-10）

- branch: `ops/live-view-fix`（main 342ec107 から）。決定は ADR 2026-10-10-browser-launcher-live-view-frames 付記 2026-10-10e。

## 原因（本番は読み取りだけで確認）

対象: task 01M4GYJ3XGJNWZQDF35F1MDE0H の run 01M4K442Q4D3ABPFC59N12ZB4R（session celeris-40b3106f…、launcher session
a0c6f68f…）。events: `browser_updated` RUNNING 14:40:24.927Z → WAITING_FOR_HUMAN 14:41:20.024Z、task は 14:40:24.8–14:41:21.9 running。

- (a) 署名鍵: web.env の `CELERIS_WEB_ATTESTATION_KEY_FILE` は設定済み、鍵は ed25519・0600・dir 0700（gateway の `privateKey()` の条件を満たす）。
  daemon の `browser_attestation_public_key_file` も設定済み。gateway の鍵で署名した assertion を終了済み run の grant に送ると
  `410 run_ended`（署名検証は通り、`authorize_live` の run_active で止まる。grant は作られない）、壊した署名は `403 not_owner_session`。
  鍵の組は合っている。
- (b) registry: `live_registration` は `browser.session_id` を鍵に登録し、grant も同じ鍵で引く。relay を開けないときの
  info log（"Live View frames unavailable"）は daemon journal に無い → v9 の `live_start` は受理されている。
- (c) RUNNING は 56 秒あり、その間に gateway の `/browser/runs` は 14:40:25・14:40:42・14:40:55・14:40:56 に呼ばれた。
- (d) 順序: 14:40:25 の 1 回は登録前の可能性があるが、14:40:42 以降は登録後。
- (e) route: grant の route は存在する（空 body で 422 `browser_body_invalid`）。
- **原因**: frame の WebSocket の upgrade（`upgradeFrames` → `checked` → `guard`）は、終端でない全 task の `/browser/waits` を
  見て `isAuthWait` に当たる待ちが 1 件でもあれば 409 `auth_interval` を返す。`/browser/waits` は解決済みの待ちも返し、
  `isAuthWait` は state を見ない。この task には解決済み（registered・resumed）の credential 待ちが 19 件あり、未決は 0 件
  （本番 API を GET だけで同じ判定を再現: `[["01M4GYJ3XGJNWZQDF35F1MDE0H","blocked",19,0]]`）。よって `/browser/runs` は
  grant 成立で link を出し、SPA が開いた WebSocket は毎回 409 で拒否され、映像の枠だけが残った。
  upgrade は express の access log を通らないので journal に `/frames` が出なかった（要求が無かったのではない）。
- 付随: grant が拒否された run は `/browser/runs` で理由が捨てられ、upstream 未設定のため `relay_unavailable` に落ちていた。

## 直したこと

- `web/server/browser-live.js`: 認証区間は未決（pending・approved）の待ちだけ（`isOpenAuthWait`）。`/browser/runs` は grant の後に
  upgrade と同じ `guard` を通し、通らなければ link を出さない。frame 経路の不可理由を固定語彙で返す（`liveDisabledReason`）。
  frame upgrade の 101 / 拒否と `/browser/runs` の不可理由を log に 1 行（id と code だけ）。
- 同じ経路の 2 つ目の不具合: frame の HTTP 要求の `setTimeout(10000)` が応答後も socket の無通信 timeout として残り、画面が
  10 秒変わらないと stream が切れていた（SPA は再接続しない）。応答後に `setTimeout(0)` で外す。
- `web/server/app.js`: `log` と（試験用の）`liveFrameConnectTimeoutMs` を browser live に渡す。
- `web/features/browser/browser-model.ts`: 新しい理由の文。
- 試験: `web/server/browser-live-frames-prod.test.mjs`（本番の構成を再現。修正前は 4 件とも失敗: 409 `auth_interval`、
  link のまま、`relay_unavailable`、無通信 200ms で stream 切断）、`browser-model.test.ts`・`browser-runs-model.test.ts` の理由。
- ADR 付記 2026-10-10e、`docs/ops/browser-launcher-live-view.md` §5 に log の見方。

## 証拠

- 修正前の gateway で新試験: 3 件 fail（`409 Rejected {"code":"auth_interval"}`、`{state:'link'}` ≠ `auth_interval`、`relay_unavailable` ≠ `run_ended`）。
  `setTimeout(0)` だけを外すと `browser_live_frame_stream_survives_idle_longer_than_connect_timeout` が fail（210ms で切断）。
- 修正後: `node --test server/*.test.mjs` 90 pass / 0 fail、`vitest run` 91 files / 657 tests pass、`biome check .` exit 0（既存 warning 5）、`tsc -b` exit 0。
- Rust は変更なし。`cargo fmt --all -- --check` exit 0。`bash scripts/dev/test-parallel.sh` exit 0（nextest 5099 passed / 0 failed / 14 ignored、
  doctest ok）。`CELERIS_USERNS_TESTS=1 CELERIS_ISOLATION_TESTS=require bash scripts/dev/test-parallel.sh` exit 0（5099 / 0 / 14、userns=true）。
  `cargo clippy --workspace -- -D warnings` exit 0。

## 本番での確認（運用者）

release 後、credential を使う browser task を 1 件走らせ、owner の web で run 画面を開く。`journalctl --user -u celeris-web@<sha12>` に
`{"path":"/browser/live/frames",...,"status":101}` が出て映像が更新されること。出ないときは同じ log の `code` / `browser_live_unavailable` の `reason` を見る。

## 未解決事項

- 映像の中身（screencast が実際に frame を出すか）は本番 run で未確認（この run は upgrade の手前で止まっていた）。

## 提案

- ADR D3 は auth section 中も本人に frame を出すとするが、gateway の guard は未決の auth 待ちがある間（他 task のものでも）frame を止める。
  frame 経路は session 単位なので、frame の upgrade だけは dashboard 共有名前空間の auth 判定を外してよいか、人の判断を求める。
- SPA の LiveViewFrame は WebSocket が閉じても再接続しない（grant 失効・一時的な切断の後は画面の再読み込みが要る）。再接続を検討する。
