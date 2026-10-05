---
tasks: [01M46XAR6KD97KMWBX2D6WW6TJ]
unit: wire
status: done
completed: 2026-10-05
---
# daemon 配線: [cos.attachments] config・ApiState の data dir・GC と legacy 移行の起動

- `crates/celeris/src/config/cos.rs`: `[cos] stream_retention_days`（既定 30）と `[cos.attachments]` の 6 鍵（既定は ADR D4: 25 MiB / 100 MiB / 10 件 / 10 GiB / 24 h / 30 日）。他の節と同じく `deny_unknown_fields`。cos-run は `CosConfig` に欄を足せばよい。`Config::validate` が 0・大小の逆転（file ≤ message ≤ storage）・i64 超えを弾く。
- data dir は DB のディレクトリ（`CosConfig::attachment_data_dir`）。blob は `<db dir>/chat/attachments/<id>/blob`。
- `daemon/api.rs` の `with_chat_wiring` が `ApiState::with_chat_attachments` に data dir と上限を渡す。daemon の upload body limit（middleware）はこの `max_file_bytes` を使う。
- `crates/celeris/src/chat_gc.rs`: 1 時間ごと（起動直後にも 1 回）に添付 GC（未送信・未参照）と upload 予約の期限回収（`ChatAttachmentStore::gc`）、chat_events の retention（`chat_events_retention`）を回す。`db_maintenance` と同じ背景タスクの流儀で、時計と実行可否（active のときだけ。standby・verify では回さない）を注入する。LLM は使わない。
- legacy 移行は daemon の `build_dispatcher` の store open で走る（試験で確認）。
- `config/celeris.example.toml` に `[cos]`・`[cos.attachments]` の例。

## 証拠

- `cargo test -p celeris --lib chat_` → 15 passed（chat_config_* 7、chat_gc_* 6、chat_wiring_* 2）
- `bash scripts/dev/test-parallel.sh` → exit 0、4030 passed / 0 failed / 13 ignored
- `cargo clippy --workspace -- -D warnings` と `--all-targets` → exit 0。`cargo fmt --all -- --check` → exit 0

## 未解決・申し送り

- web gateway（`web/server/relay.js`）は本文を `express.raw` の 1 MB 上限でメモリに載せ、応答を text として読む。このため添付の upload（>1 MB）と binary の取得は gateway 経由では通らない。ADR D4 の「gateway の body limit も同じ上限・全体をメモリに載せない」には、添付の route だけ stream で中継する変更が要る。web のチャット画面の葉で扱う（ここでは daemon 側だけ合わせた）。
- `celerisctl` の `worker_tests.rs` の `Config` 直書きに `cos` の欄を足した（compile のため）。

## 提案

- gateway の添付中継は daemon の `GET /api/v1/config` 等から上限を読み、route ごとに上限を変えるのがよい（gateway に別の設定を持たせない）。
