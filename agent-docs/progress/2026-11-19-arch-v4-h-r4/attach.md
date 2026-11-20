---
title: アーキ v4 H R4 attach — chat/attachments.rs の 3 つの Connection::open を task-core の store 読み取りの関数へ移す
tasks: [01M7TWBX8YYDKHV6C2JWDJS1VR]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# attach（WorkUnit）

完了日: 2026-11-19

## 0. 結論

- `crates/task-api/src/chat/attachments.rs` の 3 つの `rusqlite::Connection::open` と SQL 文字列 3 つを、
  `crates/task-core/src/chat/attachments.rs` の読み取りの自由関数 3 つ（`&SqliteStore` を受け `with_read_conn` を使う。F の
  `attachment_provenance` と同じ形）へ移した（[ADR](../../adr/2026-11-19-chat-attachment-store-connection.md) D1）。
- task-api の `chat/attachments.rs` は `rusqlite` を使わず、`attachment_db_path` も読まない。`open(state)` は
  `ChatAttachmentStore::open` に `state.inner.db_path`（store を開いた path）を渡す。`ChatState.attachment_db_path` の欄そのものは
  WorkUnit `flag` が消す。
- 応答の JSON・status・error code は変えていない。読み取りの失敗はどの `StoreError` でも移す前と同じ 500 `attachment_store_error`
  に写す（共通の写像の 503 `db_busy` は使わない）。
- 反例試験 `attach_conn_` を 3 件足し、3 件とも pass。既存の `chat_attach_api_*` 12 件も pass。
- `crates/task-core/src/store/`・`cos/`・`knowledge.rs`・policy の file は変えていない。

## 1. 消したもの（WorkUnit の base `$CELERIS_WU_BASE` の行）

| 場所 | 消した行 |
|---|---|
| `representation`（129 行） | `rusqlite::Connection::open(db)` と `SELECT expires_at FROM chat_attachments WHERE id=?1` |
| `upload`（250 行） | `rusqlite::Connection::open(db_path)` と `SELECT EXISTS(SELECT 1 FROM chat_client_requests WHERE kind='upload' ...)` |
| `add_reference`（394 行） | `rusqlite::Connection::open(db)` と `SELECT request_hash FROM chat_client_requests WHERE kind='attachment_ref' ...` |
| `open`・`representation`・`upload`・`add_reference`（80・122・243・387 行） | `state.chat.attachment_db_path` の 4 回の読み取りと、それぞれの 503 `attachments_unavailable` の写し（`attachment_data_dir` の 503 は残した） |
| 8 行 | `use rusqlite::{OptionalExtension, params};` |
| `upload` | `tokio::task::spawn_blocking` の直書き（`state.blocking` に替えた） |

## 2. 足したもの

- task-core（`chat/attachments.rs`、`attachment_provenance` の後）:
  - `attachment_expires_at(store, attachment_id) -> Result<Option<String>, StoreError>`（行が無ければ今と同じく error）
  - `upload_replayed(store, thread_id, client_upload_id) -> Result<bool, StoreError>`
  - `attachment_ref_request_hash(store, attachment_id, idempotency_key) -> Result<Option<String>, StoreError>`
  - 各関数の doc に「`write_tx`・`cos_operation_apply` の closure の中で呼ばない」と書いた（ADR §3）。SQL の文は移す前と同じ。
- task-api（`chat/attachments.rs`）:
  - `read_error(StoreError) -> ApiProblem`: `StoreError::Sqlite(e)` は `error(AttachmentError::Db(e))`、他の variant も 500
    `attachment_store_error`。
  - `representation` は `state` の代わりに `&SqliteStore` を受ける。`upload` は replay の読み取り・`ChatAttachmentStore::upload`・
    `representation` を 1 つの `state.blocking` の中で順に呼ぶ。`get_metadata` は `store.get` と `representation` を `state.blocking`
    の中で呼ぶ。`add_reference` は `request_hash` の読み取り（違えば 409）と `owner_checked` を 1 つの `state.blocking` の中で呼ぶ。
    `upload_effect` は受け取った `db` を渡す。呼ぶ順（replay → upload、hash → owner → pin）は移す前と同じ。
  - `state.rs` の `Inner.db_path` の doc に、`ChatAttachmentStore::open` にも渡すことを書いた（欄・型は変えていない）。
- 試験の補助: `cos_delivery_suite_a/chat_attachments.rs` の `upload` を `pub(crate)` にし、新しい試験から使う。

## 3. 反例試験（`crates/task-api/tests/cos_delivery_suite_a/attach_conn_contract.rs`）

| 試験 | 守る不変条件 | 既存の試験で検出できない理由 |
|---|---|---|
| `attach_conn_rollback_leaves_no_request_row` | `write_tx` の closure が upload・attachment_ref の要求の行を入れ `expires_at` を消してから `Err` を返した後、3 つの関数は前の値を返す。HTTP でも upload は 201、別の owner の pin は 200、metadata の `expires_at` は元の値。commit の後は新しい値 | 既存の添付の試験は rollback を起こさない |
| `attach_conn_reads_committed_rows_only` | 別の接続が `BEGIN IMMEDIATE` で未 commit の行を持つ間、3 つの関数は止まらずに commit 済みの値を返し、`lock_counts().reader` が 3 増え `writer` は増えない。metadata も 200。commit の後は新しい値 | 既存の添付の試験は他の接続の lock を持たない |
| `attach_conn_busy_keeps_attachment_store_error` | 別の接続が `BEGIN IMMEDIATE` で同じ key の別の owner の行を未 commit で持つ間の pin は、409 にならず、書き込みの段の busy を 500 `attachment_store_error` に写す（panic なら 500 `internal`）。lock を外した後、要求の行・pin の行は増えていない。再試行は 200 で 1 行ずつ増える | 既存の添付の試験は busy を起こさない |

試験の API の store は busy_timeout 300 ms で開く。待ちは lock の保持だけで作り、sleep のループは使わない。

## 4. 検査（提出時の HEAD で実行）

| 条件 | コマンド | 結果 |
|---|---|---|
| 0. attachments.rs に Connection::open・busy_timeout・TransactionBehavior・attachment_db_path が無い | `grep -nE "Connection::open\|busy_timeout\|TransactionBehavior\|attachment_db_path" crates/task-api/src/chat/attachments.rs` | 0 件（exit 1） |
| 1. attach_conn_ の試験が pass | `cargo nextest run -p task-api --test cos_delivery_suite_a --no-tests=fail -E 'test(attach_conn_)'` | 3 tests run: 3 passed |
| (c) 既存の chat_attachments の試験と attach_conn_ | `cargo nextest run -p task-api --test cos_delivery_suite_a --no-tests=fail -E 'test(attach_conn_) \| test(chat_attach_)'` | 15 tests run: 15 passed（`chat_attach_api_*` 12 件と `attach_conn_*` 3 件） |
| 2. fmt | `cargo fmt --all -- --check` | exit 0 |
| 2. clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| 2. test-affected | `bash scripts/dev/test-affected.sh` | exit 0。nextest 8494 tests run: 8494 passed（22 skipped）。web: tsc -b と vitest 101 files・705 tests passed。ts-verify 20 passed。repo_health_gate: all checks passed |
| 範囲 | `sh "$CELERIS_WU_SCOPE_PATHS"` | task-api の `chat/attachments.rs`・`state.rs`・試験 3 file、task-core の `chat/attachments.rs`、この file |

測定（参考。正の値は WorkUnit `after`）: `measure.sh <base.txt>` は 75・50・47、この WorkUnit の HEAD は 74・49・46。3 つの指標とも
`crates/task-api/src/chat/attachments.rs` だけが外れた（`measure.sh files <metric>` の diff）。SQL を移した先の task-core の
`chat/attachments.rs` は before から 3 つの指標に入っている。外れた理由は接続を開く行と SQL 文字列を消したことで、SQL の移動の分は
成果に数えない（ADR §7）。

## 5. 未解決事項

- ADR §5 の busy の起こし方（別の接続の `PRAGMA locking_mode=EXCLUSIVE` と `BEGIN EXCLUSIVE`）は、この試験の env では使えなかった。
  他の接続（試験の store と API の store の読み取りの接続）が一度でも読んだ WAL の DB では、`BEGIN EXCLUSIVE` 自体が
  `database is locked` で落ちる（試験の最初の版で確かめ、python の sqlite3 でも同じ結果を再現した）。WAL の読み取りは他の接続の lock で
  busy にならないので、`attach_conn_busy_*` は busy を書き込みの段（`ChatAttachmentStore` の接続の `BEGIN IMMEDIATE`）で起こす。
  この段の busy_timeout は `ChatAttachmentStore::open` の固定の 10 秒なので、試験は約 10 秒かかる。読み取りの関数の busy の写像
  （`read_error`）は、WAL では決定的に通す方法が無く、試験で通していない。WorkUnit `triage` の busy の試験も同じ制約を受ける。

## 6. 提案

- `ChatAttachmentStore` の書き込みの接続の busy_timeout（固定 10 秒）を store の `busy_timeout` に揃えるかは、ADR §6 の案 B
  （添付の書き込みを store の接続へ寄せる）と一緒に別の task で決める。この WorkUnit では変えていない。
