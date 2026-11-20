---
title: アーキ v4 H R4 triage — cos/triage_view.rs の Connection::open と cos/inbox.rs の triage_db を store の読み取りへ移す
tasks: [01M7TWBX8YYDKHV6C2JWDJS1VR]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# triage（WorkUnit）

完了日: 2026-11-19

## 0. 結論

- `crates/task-api/src/cos/triage_view.rs` の `open`（`Connection::open` と `busy_timeout` 5 秒）・4 つの読み取りの関数
  （`item_get`・`items`・`fallback_unsent`・`superseded`）・SQL の定数・`item_row` を消し、`CosInboxItem` と同じ SQL の
  読み取りの自由関数 4 つを task-core の `crates/task-core/src/cos_inbox.rs` に置いた（`&SqliteStore` を受け `with_read_conn` を使う。
  `cos_inbox_item`・`cos_inbox_items`・`cos_inbox_fallback_unsent`・`cos_inbox_superseded`）。
- `crates/task-api/src/cos/inbox.rs` の `triage_db` と 503 `cos_inbox_unavailable` の分岐を消した。`list` と `resolve` は
  `state.blocking(move |store| ..)` の `&SqliteStore` で読む。読み取りは今までどおり `audit.apply`・`dispatch` の前と後で呼び、
  closure の中では呼ばない（ADR §3）。
- 置き場所は ADR D2 の `chat::triage` から `task_core::cos_inbox` に変えた（ADR の付記 2026-11-19）。`chat` に置くと
  `size.responsibility-growth` の module の行（単位 `crates/task-core/src/chat`）が 8026 で閾値 8000 を超え、新しい fail になった。
- `cos/triage_view.rs` は消さず、接続も SQL も持たない `terminal_conflict`（resolve の 409 の判定）の置き場にした。policy の file
  `capability-map.json` がこの path を不変条件の根拠に挙げており、消すと `arch-map.missing_invariant` が fail になる（ADR の付記）。
- 応答の JSON（`CosInboxItem` の欄・順・serde 属性・doc）・status・error code は変えていない。schema の生成物の一致試験
  `committed_schema_matches_generated` は pass（`docs/api/v1/api-v1.schema.json` の差分なし）。
- `cos/operations.rs`・`cos/ops/`・`crates/task-core/src/store/`・policy の file は変えていない。

## 1. 消したもの・移したもの

| 場所 | 中身 |
|---|---|
| `crates/task-api/src/cos/triage_view.rs` | `open`（`Connection::open`・`busy_timeout`）、`item_get`・`items`・`fallback_unsent`・`superseded`、`ITEM_COLUMNS`・`ITEM_FROM`・`item_row`、`CosInboxItem` の定義、`use rusqlite` と module doc の「Reads open their own connection」 |
| `crates/task-api/src/cos/inbox.rs` | `triage_db`、503 `cos_inbox_unavailable`、`db`・`lookup_db` の `PathBuf`、`terminal_conflict` の `db: &Path` の引数（`terminal_conflict` は `triage_view.rs` へ移し `&SqliteStore` を受ける） |
| `crates/task-api/src/schema.rs` | `cos_inbox_item` の型の path を `task_core::cos_inbox::CosInboxItem` に |
| `docs/architecture-map.md` | CoS チャットの行に `crates/task-core/src/cos_inbox.rs` を足した |

足したもの: `crates/task-core/src/cos_inbox.rs`（`CosInboxItem`・`is_terminal`・`item_row`・4 つの関数）と `lib.rs` の `pub mod cos_inbox;`。

## 2. 反例試験（`triage_conn_`）

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `task-core chat::triage_tests::triage_conn_rollback_keeps_item_state` | `cos_operation_apply` の closure が mark の後に失敗すると、read pool の読み取りは `pending` と元の一覧を返し、操作の記録は `rejected` の 1 行だけ | 既存の試験は rollback の後を試験自身の接続で読む |
| `task-core chat::triage_tests::triage_conn_failed_write_and_reads_append_no_event` | 失敗した `write_tx`（mark・event・`cos_operations` の行を書いて `Err`）は `events`・`cos_operations` を増やさず、4 つの読み取りも増やさない | 読み取りの経路を移したときに書き込みが混ざらないことを数で見る試験が無い |
| `task-core chat::triage_tests::triage_conn_no_read_pool_store_answer_path_does_not_deadlock` | in-memory と `read_pool_size = 0` の store で answer の経路（読み取り → `cos_operation_apply` の中の mark → 応答の読み取り）が返り、読み取り 5 回が `with_read_conn` を通る。別 thread と channel の受信（上限 60 秒は guard） | API の store は常に read pool を持ち、書き込みの接続へ切り替える経路を通らない |
| `task-api tests/cos_triage.rs::triage_conn_http_reads_show_committed_rows_and_append_nothing` | `GET /api/v1/cos/inbox` は行を足さない。`applied` の監査行を trigger が拒む resolve（500）の後も一覧は `pending`、`applied` の行と event は 0、`rejected` は 1 | `cos_chat_triage_mark_and_audit_commit_together` は試験自身の接続で状態を読む |

deadlock の試験は、`triage_conn_answer_path` の `cos_operation_apply` の closure の中に `cos_inbox_item` を入れた写しで
`in-memory: the answer path did not return` の FAIL（60.5 秒）になることを確かめ、写しを戻した。

## 3. 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `grep -nE "Connection::open|busy_timeout|attachment_db_path|fn triage_db" crates/task-api/src/cos/inbox.rs crates/task-api/src/cos/triage_view.rs` → 該当なし（exit 1）
- `cargo nextest run -p task-core -p task-api --no-tests=fail -E 'test(triage_conn_) | binary(cos_triage) | test(committed_schema_matches_generated)'` → 28 passed
- `bash scripts/dev/test-affected.sh` → exit 0（nextest 8498 passed・22 skipped、web の tsc と vitest 705 passed、ts-verify 20 passed、
  repo-health verdict=pass fail_grade=0 new=0）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 4. commit（件名）

- arch-v4 H R4 triage: CoS 受信箱の 4 つの読み取りを task-core の store 読み取りの関数へ移し、triage_view.rs と triage_db を消す
- arch-v4 H R4 triage: 受信箱の読み取りを task_core::cos_inbox に置き（chat module の行の上限）、ADR に付記、architecture-map の triage_view を直す
- arch-v4 H R4 triage: triage_view.rs を接続を持たない resolve の判定（terminal_conflict）の置き場にし、capability の根拠 path を保つ
- arch-v4 H R4 triage: 進捗 triage.md と ADR の付記を書く

## 5. 未解決事項・提案

- 未解決（attempt 2、2026-11-20）: 計画の範囲 check（WorkUnit `triage` の 1 つ目の check）が `crates/task-core/src/cos_inbox.rs` を範囲の外として exit 1 にする。check が許す `crates/task-core/src/chat/` に置くと `size.responsibility-growth` の単位 `crates/task-core/src/chat` が閾値 8000 を超える（今 7944、空きは 56 行。読み取りを詰めた案でも 68 行で 8012）。`plan_issue` で計画に返し、check の正規表現に `crates/task-core/src/cos_inbox\.rs$` を足すよう申告した。
- 測定への影響（after の WorkUnit が数える）: `cos_inbox.rs` が新しく `rusqlite`・SQL 文字列の file になり、`cos/triage_view.rs` が
  外れるので、この WorkUnit の分の `rusqlite_prod_files`・`sql_string_prod_files` は増減 0 の見込み。`conn_open_files` は −1 の見込み（未測定）。
- 提案: `capability-map.json` の `cos-inbox-triage-operations-audit` の owner と根拠 path を、`cos/triage_view.rs` から
  `crates/task-core/src/cos_inbox.rs`・`chat/triage.rs`（`cos_triage_resolve_tx`）に直す policy の変更を人が起票する。直した後なら
  `terminal_conflict` を `cos/inbox.rs` に戻して `triage_view.rs` を消せる。
- 提案: `chat` module は module の行の閾値 8000 に近い（この WorkUnit の後 7944）。`chat::triage` の読み取りの method
  （`cos_triage_cursor`・`cos_triage_route_version` は `feed_cursor_get` と同じ SQL）を寄せる整理は別 task で行う。
