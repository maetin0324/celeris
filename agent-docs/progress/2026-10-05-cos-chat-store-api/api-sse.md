---
tasks: [01M46XAR6KD97KMWBX2D6WW6TJ]
unit: api-sse
status: done
completed: 2026-10-05
---
# CoS chat の SSE stream と run events

`crates/task-api/src/chat/stream.rs` に ADR D2 の `GET /chat/threads/{t}/stream` と `GET /chat/threads/{t}/runs/{r}/events` を実装した。

- cursor は `after` か `Last-Event-ID`。両方あって不一致なら 400、どちらも無ければ `0`（残っている全 event）。200 を返す前に `chat_event_cursor_check` で 404・400（不正・将来）・410 を決める。410 は code `chat-cursor-expired`（REST 共通の `chat_problem` は `chat_cursor_expired` なので stream.rs 側で写し替えた）。
- 接続後は cursor 以後を `chat_events_page`（500 件ずつ）で読み、送った id まで cursor を進め、その後も同じ cursor から読み続ける。replay と live の境界は id そのものなので gap・重複が無い。
- live の起点は store への追記。REST の書込みは `ChatState::events`（`Notify`、読む前に `enable` して取りこぼさない）で即起き、別接続の書込み（cos-run）は既存 `sse.rs` と同じ `poll_interval` の再読取りで拾う。
- `event:` は type、`id:` は chat_events.id、`data:` は envelope E。heartbeat は `heartbeat_interval`（既定 15 秒）の SSE コメント `: heartbeat` で id を持たない。切断はループを終えるだけで run には触れない。stream 中に retention が cursor を越えたら stream を閉じ、再接続が 410 を受ける。
- 接続数は既存の stream slot（`try_open_stream`）を共用する。
- run events は `after`・`limit`（既定 100、最大 500 に丸め、0 は 400）。

試験のため `crates/task-api/Cargo.toml` の dev-dependency tokio に `test-util` を足した（`tokio::time::pause` に要る）。

## 確認

- `cargo nextest run -p task-api --test chat_stream --lib chat_stream`: 6 passed（試験 5 件＋cursor 解決の単体 1 件）。すべて `start_paused`、待ちは出来事待ち＋120 秒の保険。
  - `chat_stream_snapshot_then_live_has_no_gap`: snapshot_event_id から接続し、接続前の claim は replay、接続後の偽 run（text_delta・tool×2・text_delta・run 終端）は live で届く。id 昇順で run events ページと一致、offset 0/3。
  - `chat_stream_last_event_id_reconnect_has_no_duplicate`: 途中で切断し run を完了、Last-Event-ID で再接続して重複・欠落なし。切断後も run は completed。
  - `chat_stream_heartbeat_does_not_advance_the_id`: heartbeat は id/event/data 無し、その後の最初の id は snapshot からの再生と同じ。
  - `chat_stream_expired_cursor_is_410_and_future_is_400`: retention 後の cursor が stream（query・header）と run events で 410 chat-cursor-expired。after=0 は残りを再生。将来・不一致・不正は 400、無い thread は 404。
  - `chat_stream_run_events_page`: limit=2 でページ送りし全件一致、limit 上限の丸め、0 は 400、無い run は 404、未知 query は 400。
- `bash scripts/dev/test-parallel.sh`: exit 0、4003 passed / 0 failed / 13 ignored。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo clippy -p task-api --all-targets -- -D warnings`: exit 0。

## 再試行での補強

前回の WU check は `chat_stream_` が 6 件で下限 8 件に届かず失敗した。既存の実装と試験はそのまま使い、`chat_stream_replay_crosses_store_page_boundary`（500 件を超える再生）と `chat_stream_does_not_mix_threads`（スレッド分離）を追加した。`cargo test -p task-api --test chat_stream chat_stream_ -- --quiet` は 7 passed、単体試験を含む計画の件数 check は `8 tests`、exit 0。`cargo clippy -p task-api --all-targets -- -D warnings` と `cargo fmt --all -- --check` も exit 0。

## 未解決・申し送り

- `crates/task-api/Cargo.toml` を api-attach 葉も触る可能性がある（multer 等）。統合で衝突したら両方の行を残す。
- 410 の code は stream 系が `chat-cursor-expired`（ADR どおり）、REST 共通写像は `chat_cursor_expired` のまま。schema 葉で語彙をそろえるか確認してほしい。
