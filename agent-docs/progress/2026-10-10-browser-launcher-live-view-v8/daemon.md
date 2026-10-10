---
title: "daemon: v8 版確認・frame 接続・LatestFrameSlot への中継（WorkUnit daemon）"
tasks: [01M4JAK3MY5G1K8T66Q4RTWSS5]
status: done
updated: 2026-10-10
completed: 2026-10-10
---

# daemon: v8 版確認・frame 接続・LatestFrameSlot への中継

[ADR 付記 2026-10-10b](../../adr/2026-10-10-browser-launcher-live-view-frames.md) の daemon unit。
変更は `crates/task-worker/src/browser_launcher_run.rs`・`browser_live.rs`・`browser.rs` だけ。

## 実装

- `browser_live.rs`
  - `FrameRelay`: frame 源を別 thread で読み、`task_core::browser_live_frame::LatestFrameSlot`（容量 1）に `publish` するだけ。終わる（`End`・slot が閉じた・停止）と必ず slot を閉じる。drop / `close` は slot を閉じて停止を知らせ、thread を待たない。frame は slot 以外のどこ（`LiveEmitter`・`LiveSink`・`EventSink`・tracing）にも渡さない。
  - `LauncherLiveEntry`（`LiveSessionEntry`）: `kind = Isolated`、`accepts_state = false`（launcher 経路は IdentityRestore を拒否のまま）、`live_key = (task_id, run_id)`、`live_frames` は v8 の frame 接続があり session が続いている間だけ `Some`。`current_attestation` は起動時に `verify_isolation` を通した attestation を session の間だけ返す（daemon は launcher の process を観測し直せない）。`unavailable_reason()` は `launcher_protocol_no_live_frames`（v7 以下）か `launcher_live_stream_unavailable`（v8 だが開けない）。
  - `LiveFrameRegistration`: `LiveSessions` への登録の持ち主。drop で registry から外し、entry を閉じ、slot を閉じ、relay を止める（run の終わり・どの早期 return でも同じ）。
- `browser_launcher_run.rs`
  - `open_frame_relay(socket, launcher_protocol, session_id, lease_id)`: `LauncherClient::open_live` を使い、版が 8 未満なら接続せず（`live_start` を送らず）`launcher_protocol_no_live_frames`。frame は `LiveFrame::new` で opaque に包んで slot へ。別 session・seq の巻き戻り・上限超過・`live_stopped`・切断は stream の終わり（slot を閉じる）。読みの期限（10 秒、`MSG_PEEK` の段なので区切りは崩れない）ごとに停止の旗を見直す。
  - `LauncherRuntime::open_live_frames`: 同じ control 接続の `hello` で版を確かめてから `open_frame_relay`。`LauncherRuntime` は起動時の attestation を持つ。
  - `run_registered`（本番の入口。`browser.rs` が `IsolatedBrowserConfig.live_sessions` を渡す）: session が Running になった直後、credential login の**前**に `live_registration` で frame 接続を開き `browser.session_id` で登録する。auth section 中も slot は流れ、H3 の観測停止（`LiveEmitter` の auth guard・store の auth section）は frame と無関係にそのまま。run の終わりは session の stop の**前**に登録を drop。session stop・lease 失効は launcher 側の stream 終了で slot が閉じる。grant 失効は task-api の stream 側（api-stream unit）。
  - `run`（registry なし）は試験専用の入口として残した（既存試験の呼び出しを変えない）。

## 試験（`browser_launcher_run.rs` の `live_frame_tests`、userns・実 browser 不要）

偽 launcher = 実の `LauncherServer` + channel で frame を流す偽 backend。v7 launcher = その前に置いた `hello` の `protocol_version` を 7 に書き換え、全接続の要求の種類を記録する proxy。

- `browser_live_frame_session_mismatch_rejected`: 生の frame 接続で 1 枚目（正しい session）は slot に届き、別 session の 2 枚目で slot が閉じる。
- `browser_live_frame_cleanup_closes_slot_on_drop_and_stop`: 登録の drop で registry から消え slot・entry が閉じ attestation も失効、後から来た frame は捨てる。session stop で slot が閉じる。
- `browser_live_frame_auth_section_owner_only`: `auth_begin` 後・auth guard と観測停止の旗が立った状態で本人向け slot に frame が届き、`LiveEmitter` は event を捨て、frame の後も観測停止は解けず、sink は空。
- `browser_launcher_v7_continues_without_live_view`: v7 proxy 越しの launcher run が Done・session stop・Running→Completed、`live_start` が launcher に届かず backend の `live()` も 0 回、entry は frame 購読口なし・理由 `launcher_protocol_no_live_frames`、同じ v7 launcher で action が通る。
- `browser_launcher_daemon_checks_live_protocol_before_enable`: v8 では `hello` の後に `live_start` が 1 回、v7 では `hello` だけ。
- `launcher_credential_input_not_in_events_cdp_response_or_logs`: login 入力が写った frame（MARKER）が run 中の本人向け slot に届き、sink の progress・browser 更新・live event・comment・artifact、outcome、run の作業場所と launcher dir の全 file、action の receipt/観測、slot の Debug のどこにも出ない。

## 証拠

- `TMPDIR=/tmp cargo nextest run -p task-worker --lib live_frame_tests` → 6 passed（3 回連続）。
- `TMPDIR=/tmp cargo nextest run -p task-worker -E 'test(browser_launcher) | test(browser_live) | test(launcher_)'` → 137 passed（3 回連続）。
- `cargo clippy --workspace -- -D warnings` exit 0、`cargo clippy -p task-worker --all-targets -- -D warnings` exit 0、`cargo fmt --all -- --check` exit 0。
- 全体試験は下の「全体試験」に記録。

## 未解決事項

- run の TMPDIR（長い path）のままだと Unix socket の SUN_LEN で偽 launcher の bind が落ちる（既存の launcher 試験と同じ環境要因）。試験は `TMPDIR=/tmp` で流す。
- v7 の理由は `LauncherLiveEntry::unavailable_reason()` にあるが、`LiveSessionEntry` trait（task-core）には理由の口が無いので、task-api からは `live_frames() == None` しか見えない。
- `LiveSessions::remove` は session id だけで外す（同じ logical session を次の run が登録し直す前に前の run の登録は drop 済みなので現状は問題ない）。

## 提案

- task-core の `LiveSessionEntry` に `live_unavailable_reason() -> Option<&'static str>`（既定 `None`）を足し、`LauncherLiveEntry` が返すようにすれば、task-api の run 一覧が v7 launcher の理由 `launcher_protocol_no_live_frames` をそのまま返せる（integrate-relay か cross-tests で）。
