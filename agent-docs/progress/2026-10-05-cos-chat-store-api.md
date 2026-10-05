---
title: store-api 全体検査（統合後 HEAD）
tasks: [01M46XAR6KD97KMWBX2D6WW6TJ]
status: done
updated: 2026-10-05
---
# store-api 全体検査（統合後 HEAD）

統合後 HEAD（`celeris-wu/01M46XAR6KD97KMWBX2D6WW6TJ/close`、base `fc13a52e`）で全体検査 6 本を実行した。いずれも exit 0。実装の修正はしていない。

## 証拠コマンドと結果

| コマンド | exit | 要点 |
|---|---|---|
| `bash scripts/dev/test-parallel.sh` | 0 | nextest: 4030 passed / 0 failed / 12 skipped（120 バイナリ）。doctest: 全 crate 0 passed（task-worker 1 ignored）。`CELERIS_TEST_SUMMARY {"passed": 4030, "failed": 0, "ignored": 13, "nextest_exit": 0, "doctest_exit": 0, "summary_parsed": true}` |
| `cargo clippy --workspace -- -D warnings` | 0 | 警告ゼロで `Finished` |
| `node web/scripts/gen-types.mjs --check` | 0 | 差分なし（事前に `pnpm -C web install --offline` が必要。lockfile から復元、ネットワーク未使用） |
| `sh scripts/dev/check-doc-links.sh` | 0 | `check-doc-links: ok` |
| `sh scripts/dev/check-adr-numbers.sh` | 0 | `check-adr-numbers: ok (141 files)` |
| `sh scripts/dev/progress-index.sh --check` | 0 | `progress-index --check: ok` |

## ADR D6「保存・API」行と chat_ 試験名の対応

ADR 本文の該当行（`agent-docs/adr/2026-10-05-cos-chat-home.md` の D6 導入順・受け入れ条件の表、「保存・API」行、担当 store-api）:

> 旧 messages の移行/再実行/非 CoS 保全、FTS、client id 再送、同時 claim、snapshot→SSE、再接続重複排除、cursor GC の 410、stop の run id 競合、添付上限/../MIME/GC/pin

| 項目 | 対応する chat_ 試験 |
|---|---|
| 旧 messages の移行 | `chat_legacy_one_thread_per_project_and_global`、`chat_legacy_seq_follows_created_at_then_id_and_maps_roles`（task-core/src/store/legacy_tests.rs）、`chat_migration_upgrade_from_47_preserves_existing_rows`（task-core/src/chat/tests.rs） |
| 再実行（冪等な re-open） | `chat_legacy_reopen_is_idempotent_and_catches_up_late_rows` |
| 非 CoS 保全 | `chat_legacy_keeps_old_and_non_cos_messages`、`chat_legacy_retires_only_cos_sessions` |
| FTS | `chat_legacy_fts_finds_imported_bodies`、`chat_store_search_takes_fts_syntax_literally_and_dedups_threads` |
| client id 再送 | `chat_store_thread_create_is_idempotent_per_client_key`、`chat_store_message_replay_returns_same_message_and_conflicts_on_change`、`chat_api_post_replay_limits_and_cancel`、`chat_attach_idempotent_client_upload_id`、`chat_attach_api_replay_different_bytes_conflicts` |
| 同時 claim | `chat_store_concurrent_claim_has_one_winner`、`chat_store_claim_is_fifo_with_one_live_run`、`chat_attach_concurrent_reservation_counts_in_flight` |
| snapshot→SSE | `chat_stream_snapshot_then_live_has_no_gap`、`chat_store_message_pages_with_snapshot_event_id` |
| 再接続重複排除 | `chat_stream_last_event_id_reconnect_has_no_duplicate`、`chat_stream_replay_crosses_store_page_boundary` |
| cursor GC の 410 | `chat_stream_expired_cursor_is_410_and_future_is_400`、`chat_store_event_retention_and_cursor_expiry_use_injected_clock`、`chat_store_future_and_malformed_cursors_are_bad_requests` |
| stop の run id 競合 | `chat_store_stop_pauses_queue_and_late_stop_is_noop`（旧 run_id での late stop が新 run に影響しない／別 thread・存在しない run_id は 404）、`chat_api_queue_limit_and_run_stop_resume`（実行中の run の message 削除は 409、stop は run_id 指定で 202、終端後の再送は 200） |
| 添付上限/../MIME/GC/pin | 上限: `chat_attach_file_limit_exact_and_over`、`chat_attach_message_byte_limit_exact_and_over`、`chat_attach_message_count_and_thread_checks`。`../`: `chat_attach_rejects_parent_symlink_and_cross_thread_reuse`。MIME: `chat_attach_unknown_magic_is_octet_stream`。GC: `chat_attach_gc_releases_expired_reservation_and_staging`、`chat_attach_refs_block_delete_and_gc_by_injected_clock`、`chat_gc_removes_unsent_upload_after_orphan_ttl`、`chat_gc_keeps_referenced_blob_and_deletes_after_unreferenced_retention`、`chat_gc_expires_upload_reservations`、`chat_gc_applies_stream_retention_after_run_end`。pin（KB inbox 参照）: `chat_attach_api_knowledge_inbox_reference` |

上記以外にも `chat_` 試験は計 79 件ある（`chat_api_*`・`chat_model_*`・`chat_config_*`・`chat_wiring_*` 等。REST の権限・validation・query・SSE の heartbeat/マルチスレッド分離などを含む）。列挙は `grep -rn "fn chat_" crates/ tests/` で確認できる。

## 未解決事項（cos-run への申し送り）

- **CoS 無効時の 503 判定点**: `cos.enabled=false` のときの send 系 endpoint の挙動は `chat_api_disabled_send_is_unavailable` で確認済み（unavailable 応答）。CoS run 自体の起動・triage・escalation の 503/フォールバック判定は本 WorkUnit の範囲外（偽 run までしか実装していない）。cos-run 側で ADR D6「一次対応 C: CoS 不在」の表（run 失敗・quota 切れ・`cos.enabled=false`・`unavailable_after_secs` 超過）を参照して実装すること。
- **旧 CoS run の drain**: ADR 48 行目「稼働中の旧 CoS run は切替前に drain/stop して最終返事を取り込み、その後に migration/backfill」は本番 DB の切替手順であり、このリポジトリの migration 自体（`chat_migration_upgrade_from_47_preserves_existing_rows` 等）は新規 DB・既存行の両方を試験している。実際の drain 手順（稼働中 daemon の停止・確認）は本番操作であり人の手順として ops-docs 段で書くこと（本 Celeris からは本番 host を操作しない）。
- **Console 互換 facade**: `POST /console/instruct`・`POST /org/cos/messages`・MCP の CoS 入力を legacy thread へ投入する互換 facade は、ADR 本文（D6 移行節）に設計はあるが、本 WorkUnit の実装・試験には facade 自体の endpoint 変更は含まれていない（`chat_legacy_*` は移行後のデータ形だけを検証する）。cos-run か web-chat 側で `/console/instruct` 等のハンドラを legacy thread の `chat_message_post` へ向ける実装が必要。
- **preview の状態**: 添付の `preview` は `chat_attach_api_preview_delete_and_references`・`chat_attach_api_metadata_exposes_download_url` で upload 直後の preview 生成と削除時の扱いを確認済み。画像以外（PDF 等）の preview 非対応は `chat_attach_unknown_magic_is_octet_stream` の MIME 判定止まりで、cos-run 側の「添付の引渡し」（screenshot→画像入力、PDF→path→KB inbox pin）で実際に使う形まではここでは検証していない。

## 提案

- `grep -rn "fn chat_"` は production 関数（store/run_store の public API）も拾う。次回同種の棚卸しをするときは `#[test]`/`#[tokio::test]` の直後業の行だけを拾うスクリプト化を検討してよい（ここでは手作業で絞り込んだ）。
