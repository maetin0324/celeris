---
title: "launcher: protocol v8 と CDP screencast（WorkUnit launcher）"
tasks: [01M4JAK3MY5G1K8T66Q4RTWSS5]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# launcher: protocol v8 と CDP screencast

[ADR 付記 2026-10-10b](../../adr/2026-10-10-browser-launcher-live-view-frames.md) の launcher unit。
変更は `crates/task-worker/src/browser_launcher/` だけ（`browser_runtime.rs` は変えていない）。

## 実装

- `protocol.rs`: `PROTOCOL_VERSION = 8`、`LIVE_FRAME_PROTOCOL = 8`、`MAX_LIVE_BODY = 2 MiB`、`LiveEncoding`。
  要求 `live_start` / `live_stop`、応答 `live_started` / `live_frame`（session_id・seq・width・height・encoding・body_len だけ）/ `live_stopped`。どれも `deny_unknown_fields`。input の verb は無い（D4）。
- `live.rs`（新規）:
  - `LiveTap::interpose` が Chrome の CDP 出力 pipe と `CdpController` の間に入る demux。launcher 自身の screencast CDP session の event（`Page.screencastFrame`、attach/detach の告知、その session の他 event）は controller に渡さない。よって agent の event queue に frame も session id も入らない。他は全部そのまま controller へ（応答は常に controller へ）。容量 1 の最新 frame slot。転送待ちが 4 MiB を超えたら Chrome の読みを止める（以前と同じ背圧）。転送した buffer は zeroize する。
  - `ScreencastFeed` は controller（pipe の唯一の書き手）を通して `Target.getTargets` → `Target.attachToTarget` → `Page.startScreencast`、frame ごとに `Page.screencastFrameAck`、drop で `Page.stopScreencast` + `Target.detachFromTarget` を送る。auth section の login target を優先して映す（D3、`auth_begin` が `set_preferred_target`）。auth section 中も frame は本人向けに流れ、agent の観測停止（H3）はそのまま。
  - `LiveImage` は `Debug`・`Serialize`・`Clone` を持たず、drop で bytes を消す。
- `server.rs`: `live_start` が来た接続は、session・lease・期限が合い、session を作った daemon process（SO_PEERCRED の pid + starttime）が、session の control 接続とは別の接続で開いたときだけ frame 接続になる。1 session 1 stream。frame は metadata frame の後に長さ前置の binary frame（≤ 2 MiB）で書く。`live_stop`（別 session 名は `lease_mismatch`）・切断・session 終了・lease 失効・shutdown で終わる。frame 接続での他の要求（action・未知の input verb）は `bad_request` で閉じ、session に届かない。control 接続での `live_start` は `unauthorized` で答え、接続も session も壊さない。`BackendSession::live` の既定は拒否。
- `client.rs`: `LauncherClient::open_live(path, timeout, launcher_protocol, session_id, lease_id)`。版が 8 未満なら接続せずに `ClientError::NoLiveFrames`（`launcher_protocol_no_live_frames`）。`LiveStream::next_frame` は別 session・seq の巻き戻り・2 MiB 超・metadata と長さが違う body を protocol error にする。`stop` は `live_stop` を送って `live_stopped` まで読む。
- `backend.rs`: 実 runtime の CDP pipe に `LiveTap` を入れ、`RuntimeSession::live` が `ScreencastFeed` を返す。
- `tests.rs`: v7 の等式 `PROTOCOL_VERSION == CONSENT_PROTOCOL` を `CONSENT_PROTOCOL(7) < PROTOCOL_VERSION` に直した。

## 試験（`live_tests.rs`、userns・実 browser 不要）

- `browser_launcher_live_frame_cdp_screencast_start_ack_stop_on_fake_cdp`: pipe 上の偽 Chrome と実 `CdpController`。start/ack/stop の CDP command、frame の decode、2 MiB 超の frame は捨てる、agent event に launcher session・frame・attach 告知が出ない、agent 自身の session の event は残る、screencast 以外の command（`Input.*` 等）を送らない。
- `browser_launcher_live_frame_follows_preferred_target`
- `browser_launcher_protocol_v8_frames`、`browser_launcher_live_frame_unknown_fields_and_verbs_rejected`
- `browser_launcher_live_frame_stream_roundtrip`、`browser_launcher_v7_continues_without_live_view`、`browser_launcher_live_frame_other_session_rejected`、`browser_launcher_live_view_rejects_input`、`browser_launcher_live_frame_client_enforces_bounds_and_binding`

## 証拠

- `TMPDIR=/tmp cargo nextest run -p task-worker --lib browser_launcher` → 49 passed（既存 40 + 新規 9）、exit 0。
- live 系 10 件を 3 回 → 毎回 10 passed。
- `TMPDIR=/tmp cargo nextest run -p celeris browser_doctor` → 6 passed。
- `cargo fmt --all -- --check` exit 0、`cargo clippy --workspace -- -D warnings` exit 0。
- `TMPDIR=/tmp bash scripts/dev/test-parallel.sh` → exit 0、5036 passed / 13 skipped（run の TMPDIR のままだと Unix socket の SUN_LEN で既存試験が落ちるため /tmp）。

## 未解決事項

- 実 Chrome での screencast は確かめていない（実 browser の試験は opt-in、後続の cross-tests / 運用確認）。
- daemon 側の版確認と `LatestFrameSlot` への中継は daemon unit。`celeris browser doctor` は launcher 版を `PROTOCOL_VERSION`（8）と等しいかで見るので、v7 の本番 launcher は差し替えまで NG と出る（ops-doc で手順に書く）。

## 提案

- daemon unit は `LauncherClient::hello` の版をそのまま `open_live` に渡す。`ClientError::NO_LIVE_FRAMES` を Live View の reason に使える。
