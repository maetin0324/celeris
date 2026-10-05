---
tasks: [01M46XAR6KD97KMWBX2D6WW6TJ]
unit: store-chat
status: done
completed: 2026-10-05
---
# CoS chat の thread/message/run/event store 操作

- WorkUnit: `store-chat`
- ADR: `agent-docs/adr/2026-10-05-cos-chat-home.md` D1/D2
- 置き場所: `crates/task-core/src/chat/store.rs`（thread・message・共通部品）、`chat/run_store.rs`（run・stop/interrupt・event cursor・retention）、試験 `chat/store_tests.rs`。`chat/mod.rs` は module 宣言と re-export の追加だけ。
- 形: `impl SqliteStore` の inherent method（`TaskStore` の supertrait にはしない。`ApiState` は `Arc<SqliteStore>` を持つので API から直接呼べる）。書込みは writer lock + IMMEDIATE tx 1 本、読取りは read 接続の 1 transaction（`snapshot_event_id` と items が同じ snapshot）。時計は全部 `now: OffsetDateTime` 引数で注入。LLM 呼び出しは無い。
- 時刻の保存形式は固定幅の `YYYY-MM-DDTHH:MM:SS.mmmZ`（文字列順 = 時刻順。一覧の keyset と retention の比較に使う）。

## 操作と規則
- thread: `chat_thread_create(scope_id, req, now)`（`chat_client_requests` kind=`thread` で冪等、同 key 同内容は `created=false`、内容違いは 409、project 参照は tx 内で検査）、`chat_thread_get`/`chat_thread_detail`（active_run と last_event_id）、`chat_thread_list`（status・`updated_at|id` 降順 cursor、limit 1..=100、`q` は FTS5 literal で thread 単位に重複排除、空白だけの q は一覧）、`chat_thread_patch`（expected_revision、inbox の archive・run/queue 残りの archive は 409）、`chat_thread_resume_queue`。
- message: `chat_message_post`（本文 64 KiB 超 413、空白のみは添付ありだけ、添付 10 件超 413、同 thread の ready 添付だけ、待機 100 件で 429、client_message_id 再送は同じ応答・本文/添付/mode/reply 違いは 409、seq 採番＋queue 投入＋`message`/`queue` event を 1 tx、`resume_queue=true` で pause 解除）、`chat_message_list`（before_seq/after_seq 排他、limit 1..=200、items は seq 昇順）、`chat_message_cancel`（queued の user 入力だけ、開始済みは 409、取消済みへの再送はそのまま返す）、`chat_system_message_add`（カード付き system 発言）。
- run: `chat_run_claim_next(thread, run_id, resolved_config, now)`（interrupt 発言を先頭に seq 順、pause 中は interrupt 発言だけ、live run があれば None、部分 UNIQUE が接続をまたいだ二重を防ぐ。assistant の出力 message を running で作る）、`chat_run_append_text`（offset は追記前の UTF-8 byte 長）、`chat_run_status`、`chat_run_tool`（detail 4 KiB で切り truncated）、`chat_run_finish`（終端状態だけ、同じ終端への再送は冪等・別終端は 409、入出力 message を completed/failed/interrupted に）、`chat_run_stop`（running→stopping＋queue_paused、stopping への再送は accepted、終端への再送・古い run_id の遅延 stop は `accepted=false` で何も変えない）。interrupt 送信は live run を stopping にし queue_paused は変えない。
- event: `chat_events_page(thread, {after, run_id, limit 1..=500})`、`chat_event_cursor_check`（未来の cursor・不正な cursor は 400、retention の watermark 未満は 410）、`chat_events_retention(now, retention)`（終端 run の `text_delta`/`tool` を `finished_at <= now-retention` で削除、既定 `CHAT_EVENT_RETENTION_DAYS=30`）。watermark は既存 `feed_cursor` 表の `chat_events_purged:<thread_id>`（削除した最大 id）。
- エラー: `ChatError::http_status()` が 400/404/409/410/413/422/429/500 を返す（API の ApiProblem へ写す。410 は code=chat-cursor-expired）。

## 証拠
- `cargo test -p task-core chat_` → 22 passed（chat_store_* 16 件＋model 葉の 6 件）。
- `bash scripts/dev/test-parallel.sh` → exit 0、passed 3964 / failed 0 / ignored 13（nextest 160.5s）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- base からの差分: `crates/task-core/src/chat/{mod.rs,store.rs,run_store.rs,store_tests.rs}` と本ファイルだけ。

## 試験（chat_store_*）
thread 冪等作成・一覧 cursor/status/limit・FTS literal（`OR`/`col:`/`NEAR`/`*`/`"`/`(` が演算子として働かない）と重複排除・patch/archive 規則・送信上限（64 KiB 丁度は可）・再送・ページと snapshot_event_id・取消・FIFO claim・2 接続 4 thread の同時 claim で勝者 1・run の event と終端・stop/遅延 stop/resume・resume_queue 送信・interrupt（pause 有無の両方）・retention の境界 1 秒前/丁度と 410・未来 cursor 400。待ちは Barrier だけで sleep は無い。

## 未解決・申し送り
- API 葉: thread 作成の `scope_id` は認証済み利用者/インスタンスを渡す（現行は単一管理者なので固定値でよい）。stop の 202/200 は `ChatStopOutcome.accepted`、thread 作成の 201/200 は `ChatThreadCreated.created`。SSE は `chat_event_cursor_check` → `chat_events_page` で再生してから live に移る。
- wire 葉: `chat_events_retention` の定期起動（既定 30 日）を daemon に配線する。
- cos-run: claim に渡す `run_id` は既存 runs 行の id。`resolved_config` の `harness/llm_source/provider/account_id/model/tier/session_mode` がそのまま R に出る。再起動時の回収（running の chat_runs を interrupted にする）は cos-run 側。

## 提案
- FTS5 の既定 tokenizer（unicode61）は日本語の連続した文字列を 1 語として扱うので、「画面の修正」の中の「修正」では当たらない。日本語の部分一致が要るなら `chat_search` を `tokenize='trigram'` に変える（0050 が main に入る前なら 0050 の修正で、後なら新 migration で作り直し＋backfill）。検索語の literal 化（`chat_fts_literal`）はそのまま使える。
