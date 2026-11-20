---
tasks: [01M7TWBX8YYDKHV6C2JWDJS1VR]
---
# ADR 2026-11-19: task-api の chat 添付と CoS triage の自前の SQLite 接続を SqliteStore の読み取りへ移し、ChatState の attachment_db_path を消す（アーキv4 H R4）

- 日付: 2026-11-19
- 状態: Accepted（実装前。この ADR を書いた WorkUnit `adr` は `crates/` を変えていない。D1 は WorkUnit `attach`、D2 は `triage`、
  D4 は `flag`、測定の道具と before は `measure`、after と進捗の完了は `after` が行う）
- task: 01M7TWBX8YYDKHV6C2JWDJS1VR（アーキv4 H 01M7MZTKBPZPQE4XN451F8X84F の子、人の決定 unmet b の R4）
- 進捗: `agent-docs/progress/2026-11-19-arch-v4-h-r4.md`（WorkUnit `measure` が置く）、測定値は同じ名の dir の `metrics.tsv`
- 関係: [ADR 2026-11-18 knowledge-lifecycle-tx-boundary](2026-11-18-knowledge-lifecycle-tx-boundary.md)（F。`attachment_provenance` の形と、
  `attachment_db_path` を有無の旗として残した付記）、
  [ADR 2026-10-05 cos-chat-home](2026-10-05-cos-chat-home.md) D4（添付の保存・pin・provenance）、
  [ADR 2026-10-06 cos-inbox-triage](2026-10-06-cos-inbox-triage.md)（triage の表）、
  [ADR 2026-11-11 cos-fallback-timing](2026-11-11-cos-fallback-timing.md) D2（`fallback_unsent`）、
  [ADR-0064](0064-db-local-disk-and-store-resilience.md) D4（SELECT は読み取りの接続）、
  [ADR 2026-11-10 taskstore-consumer-ports](2026-11-10-taskstore-consumer-ports.md) D5（triage の書き込みは `cos_operation_apply` の中）、
  [ADR 2026-11-16 taskstore-trait-removal](2026-11-16-taskstore-trait-removal.md)（巨大 trait を戻さない）、
  [ADR 2026-10-09 CoS の全変更操作](2026-10-09-cos-operations-all-mutations.md)・[同 external-effects](2026-10-09-cos-operations-external-effects.md)（`cos-audit-same-tx`）、
  [ADR 2026-11-17 境界の意味を変える改修の risk](2026-11-17-boundary-change-risk-review.md)（`transaction` class の設計 review）、
  [2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。
- 材料: アーキv4 H の [final-report.md](../progress/2026-11-19-arch-v4-h/final-report.md) の「提案」の 4・5・12。
- 行番号は、この ADR を書いた時点の HEAD（WorkUnit の base `$CELERIS_WU_BASE` と同じ内容。件名「integrate wu/boundary (phase boundary)」）の行。
- 契約: API・CLI・DB の契約（応答の JSON の形・status・error code・表）は変えない。変える必要が出たら実装を止め、result.json の `question` で人に聞く。
  policy の file は変えない。`TaskStore` の巨大 trait を戻さない。`crates/task-core/src/store/` の file は変えない（`with_read_conn` は `pub(crate)` のまま）。

## 1. 実装前 brief

### 1.1 根本原因

`crates/task-api` は store と同じ SQLite の file を、`SqliteStore` を通らない自前の接続で 5 か所から開く。開く path の正本も 2 つある。

1. **chat 添付の 3 つの読み取り。** `crates/task-api/src/chat/attachments.rs` は要求ごとに `rusqlite::Connection::open` で
   `state.chat.attachment_db_path` を開いて 1 行を読む。
   - `representation`（117-148 行。129 行で open）: `chat_attachments.expires_at`。
   - `upload`（243-254 行。250 行で open）: `chat_client_requests` に `kind='upload'` の同じ key があるか（replay なら 200、無ければ 201）。
   - `add_reference`（387-399 行。394 行で open）: `chat_client_requests` の `kind='attachment_ref'` の `request_hash`（違えば 409 `chat_conflict`）。

   この接続は store の読み取りの接続（`with_read_conn`、ADR-0064 D4）を通らず、`SqliteStore::lock_counts` にも出ない。
   SQL の文字列が task-api に残り、同じ表の SQL の owner である `task_core::chat::attachments` と 2 か所に分かれる。
2. **CoS triage の読み取り。** `crates/task-api/src/cos/triage_view.rs` の `open`（70-74 行）が `Connection::open` と `busy_timeout` 5 秒で接続を作り、
   `item_get`（76 行）・`items`（87 行）・`fallback_unsent`（104 行）・`superseded`（111 行）が使う。path は
   `cos/inbox.rs` の `triage_db`（885-893 行）が `state.chat.attachment_db_path` から取る。値が `None` なら 503 `cos_inbox_unavailable` を返す分岐を持つが、
   `ApiState::new`（`state.rs:178-181`）が必ず `Some(settings.db_path)` を入れるので、この分岐は通らない。
3. **同じ DB の path の正本が 2 つ。** `ApiState::new` は `settings.db_path` を `Inner.db_path`（`state.rs:194`、store を開いた path）と
   `ChatState.attachment_db_path`（`state.rs:179`）の両方に写す。`ChatState` は `pub` の欄（`chat/mod.rs:34-36`）で、
   `attachment_data_dir`（添付の blob の置き場。`with_chat_attachments` が入れる）と別に、同じ DB を指す 2 つ目の値を持つ（`dual_source_of_truth`）。
4. **knowledge.rs の旗が意味の違う値から導かれる。** `knowledge.rs` の `inbox`（696 行）・`record`（735 行）・`inbox_detail`（882 行）は
   `state.chat.attachment_db_path.is_some()` を「添付の保存が有効か」の旗にする。この値は常に `Some` なので、旗は常に真になる。
   [api.md](../../docs/api/v1/api.md) は §3.104 の `provenance[]` に「添付の保存が無効なら空」、`POST /knowledge/inbox` に
   「添付の保存が無効で `attachment_ids` を渡せば 503 `attachments_unavailable`」と書く。「添付の保存が無効」は `attachment_data_dir` が無いこと
   （`chat/attachments.rs:72-87` の `open` が 503 `attachments_unavailable` を返す条件）なので、今の旗は文書の契約と別の値を見ている。

症状は final-report.md の (2) の行に出る: `rusqlite`・SQL 文字列を書く production file（A の C4）が task-api に残り、F が knowledge.rs の自前の接続を消した後も
`attachment_db_path` が「有無の旗」と「開く path」の 2 役で残った（F の ADR の付記の表、D1・D5 の行）。

### 1.2 責務の owner

- chat 添付の表（`chat_attachments`・`chat_attachment_refs`・`chat_client_requests` の添付の行）の SQL: `crates/task-core/src/chat/attachments.rs`
  （`ChatAttachmentStore`・`add_ref_tx`・`attachment_provenance` の module）。
- CoS triage の表（`cos_inbox_items`・`cos_notification_routes`）の SQL: `crates/task-core/src/chat/triage.rs`（`cos_triage_resolve_tx` の module）。
  `cos_fallback::unsent`・`cos_revision::superseded` は `crates/task-core/src/cos_fallback.rs`・`cos_revision.rs` のまま使う。
- 読み取りの接続の選び方（read pool か書き込みの接続か）: `SqliteStore::with_read_conn`（`crates/task-core/src/store/mod.rs:911`）。
- HTTP の status・code への写像と、添付の保存の有無の判定: task-api の `chat/attachments.rs`（`error`・`open`）・`chat/mod.rs`（`chat_problem`）。
- DB の path の正本: `ApiState` の `Inner.db_path`（store を開いた path）。

### 1.3 既存の再利用先

- 形の手本: F の `task_core::chat::attachments::attachment_provenance(store: &SqliteStore, ..)`（`task-core/src/chat/attachments.rs:699`）。
  `&SqliteStore` を受け、`store.with_read_conn` の中で SQL を走らせる自由関数で、`SqliteStore` の method を増やさない。
- 読み取りの接続: `SqliteStore::with_read_conn`（ADR-0064 D4。file DB では read pool、`read_pool_size = 0` と in-memory では書き込みの接続）。
- 書き込み: `SqliteStore::write_tx`（knowledge の pin）、`OperationAudit::apply` → `cos_operation_apply`（triage の resolve）、
  `ChatAttachmentStore` の `upload`・`add_ref_request`・`delete_unreferenced`（添付の blob と行）。どれも今のまま使う。
- 試験の手本: `crates/task-api/tests/cos_delivery_suite/tx_boundary_contract.rs`（別接続が持つ lock で busy を決定的に起こす。合否は lock の有無だけで決まる）、
  `crates/task-api/tests/common/mod.rs` の `admin_env`（添付の保存なし）と `env_with_attachments`（あり）。
- 応答の型: `CosInboxItem` の derive（`serde::Serialize`・`schemars::JsonSchema`）は task-core でも使える（task-core は `schemars` に依存し、
  `chat/triage.rs` に既に `JsonSchema` の derive がある）。

### 1.4 選択肢

| 案 | 中身 | 判定 |
|---|---|---|
| A（採る） | 5 つの接続点の SQL を task-core の `chat::attachments`・`chat::triage` に `&SqliteStore` を受ける読み取りの自由関数として置き、`with_read_conn` を使う。task-api は関数を呼ぶだけにする。`attachment_db_path` は欄ごと消す | F と同じ形で、公開面は読み取りの関数 7 つだけ。store の型も trait も増えない |
| B | `ChatAttachmentStore` を `Arc<SqliteStore>` の上に作り直し、添付の書き込みも store の書き込みの接続に寄せる | 採らない（§6） |
| C | `with_read_conn` を `pub` にし、task-api が SQL を持ったまま store の接続を借りる | 採らない（§6） |
| D | `SqliteStore` に `cos_inbox_items` などの method を足す | 採らない。store の型が育ち、ADR 2026-11-16 taskstore-trait-removal が避けた形に寄る。A の自由関数で足りる |

### 1.5 新しい状態・分岐・fallback・port

- 新しい状態: 無し。`ChatState` の欄は 1 つ減る。
- 新しい分岐: 無し。503 `cos_inbox_unavailable` の分岐（通らない分岐）を消す。knowledge.rs の 3 つの旗は分岐の数を変えず、導く値だけを替える。
- 新しい fallback: 無し。`with_read_conn` の書き込みの接続への切り替えは既存（ADR-0064 D4）。
- 新しい port: 無し。足す公開面は task-core の読み取りの自由関数 7 つ（D1 の 3 つ、D2 の 4 つ）と、`CosInboxItem` の型の置き場所の移動。

### 1.6 撤去対象

- `crates/task-api/src/chat/attachments.rs`: 3 つの `rusqlite::Connection::open`、その中の SQL 文字列 3 つ、`use rusqlite::{OptionalExtension, params}`、
  `attachment_db_path` を読む 4 か所（80・122・243・387 行）と、それぞれの 503 `attachments_unavailable` の写し（`attachment_data_dir` の 503 は残す）。
- `crates/task-api/src/cos/triage_view.rs`: `open`（`Connection::open`・`busy_timeout`）と 4 つの読み取りの関数、SQL の定数 `ITEM_COLUMNS`・`ITEM_FROM`、
  `item_row`。`CosInboxItem` は task-core へ移す。file に残る物が無ければ file を消し、`cos/mod.rs:17` の `pub mod triage_view;` も消す。
- `crates/task-api/src/cos/inbox.rs`: `triage_db`（885-893 行）、503 `cos_inbox_unavailable`、`db`・`lookup_db` の `PathBuf` の受け渡し
  （187・523-524 行）と `terminal_conflict` の `db: &Path` の引数（707 行）。
- `crates/task-api/src/chat/mod.rs`: `ChatState.attachment_db_path` の欄と doc comment（35-36 行）、`Default` の `attachment_db_path: None`（47 行）。
- `crates/task-api/src/state.rs`: `ChatState { attachment_db_path: Some(..), ..Default::default() }`（178-181 行）。
- `crates/task-api/src/knowledge.rs`: 3 か所の `state.chat.attachment_db_path.is_some()`（D4 で置き換える）。
- 説明: `cos/triage_view.rs` の module doc の「Reads open their own connection on the daemon DB (same as chat attachments)」、
  `cos/inbox.rs` の `triage_db` の doc。F の ADR の付記の表の「`attachment_db_path` は有無の旗として残る」は歴史の記録なので書き換えない
  （この ADR が後を引き継ぐ）。

### 1.7 不変条件と確かめる試験

| 不変条件 | 確かめる試験 |
|---|---|
| `task-state-in-sqlite`: 読み取りは commit 済みの行だけを返し、失敗した transaction の行を見せない | `attach_conn_*`・`triage_conn_*` の rollback と commit 済みの試験（§5） |
| `no-data-loss`・`cos-audit-same-tx`: 書き込みの経路（`write_tx`・`cos_operation_apply`・`ChatAttachmentStore`）は変えず、読み取りは書き込みの transaction の外にある | `triage_conn_*` の event の試験と既存の `tx_boundary_contract`・triage の試験 |
| `events-append-only`: 読み取りの関数は `events` に行を足さない | `triage_conn_reads_append_no_event`（§5） |
| `no-unwrap-outside-tests` | clippy と review |
| API の契約（status・code・JSON の形）が変わらない | 既存の添付・pin・triage・chat の試験、`attach_conn_busy_*`（busy の code）、`attachment_flag_*`（api.md の 2 つの文） |
| deadlock しない（§3） | `triage_conn_no_read_pool_store_does_not_deadlock`（§5） |

## 2. 5 つの接続点の移し先（D1・D2）

### D1. chat/attachments.rs の 3 つの `Connection::open`

`crates/task-core/src/chat/attachments.rs` に、`attachment_provenance` と同じ形の読み取りの自由関数を 3 つ置く。どれも `&SqliteStore` を受け、
`store.with_read_conn` の中で SQL を 1 文走らせ、`Result<_, StoreError>` を返す。SQL の文は今の task-api の文と同じにする（条件・列を変えない）。

| 接続点（今） | task-core の関数（名は実装の WorkUnit が決めてよい。目安） | SQL |
|---|---|---|
| `representation`（129 行） | `attachment_expires_at(store, attachment_id) -> Result<Option<String>, StoreError>` | `SELECT expires_at FROM chat_attachments WHERE id=?1`（行が無ければ今と同じく error） |
| `upload` の replay（250 行） | `upload_replayed(store, thread_id, client_upload_id) -> Result<bool, StoreError>` | `SELECT EXISTS(SELECT 1 FROM chat_client_requests WHERE kind='upload' AND scope_id=?1 AND key=?2)` |
| `add_reference` の `request_hash`（394 行） | `attachment_ref_request_hash(store, attachment_id, idempotency_key) -> Result<Option<String>, StoreError>` | `SELECT request_hash FROM chat_client_requests WHERE kind='attachment_ref' AND scope_id=?1 AND key=?2`（`optional()`） |

- task-api の呼び手は `state.inner.store`（`Arc<SqliteStore>`）を渡す。呼ぶ場所（`representation` の中、`upload` の `store.upload` の前、
  `add_reference` の `owner_checked` の前）と同期・非同期の別は今と同じにする。
- error の写像は今の code を保つ。今の 3 か所は `error(AttachmentError::Db(e))` を通り、`error` の `_` の腕で **500 `attachment_store_error`** になる
  （busy も含む）。移した後も 3 つの関数の `StoreError` は、どの variant でも 500 `attachment_store_error` に写す
  （`StoreError::Sqlite(e)` は `AttachmentError::Db(e)` にして `error` に渡し、他の variant も同じ status・code にする）。
  共通の `StoreError` → `ApiProblem` の写像（busy は 503 `db_busy`）は使わない。使うと busy の code が変わり、契約の変更になる。
- busy の待ち時間は、今の自前の接続の rusqlite の既定（5 秒）から API の store の `busy_timeout`（`ApiSettings.busy_timeout`）に変わる。
  status・code は変わらない。
- `ChatAttachmentStore::open`（`task-core/src/chat/attachments.rs:85`）に渡す DB の path は `state.inner.db_path`（store を開いた path）から取る。
  `ChatAttachmentStore` は自分の書き込みの接続（`open_with_flags(.., READ_WRITE)`・`busy_timeout` 10 秒）を今のまま持つ（§6 の案 B を採らない）。
- task-api の `chat/attachments.rs` は `rusqlite` を使わなくなる（`use rusqlite::..` を消す）。

### D2. cos/triage_view.rs と cos/inbox.rs の `triage_db`

`crates/task-core/src/chat/triage.rs`（または同じ `chat` の下の読み取りの module。新しい file にするなら `chat/triage_read.rs` など 1 つ）に、
`&SqliteStore` を受けて `with_read_conn` を使う読み取りの自由関数を 4 つ置く。

| 今の task-api の関数 | task-core の関数（目安） | 中身 |
|---|---|---|
| `triage_view::item_get(db, id)` | `cos_inbox_item(store, id) -> Result<Option<CosInboxItem>, ChatError>` | 今の `ITEM_COLUMNS`・`ITEM_FROM` の SELECT と `item_row` |
| `triage_view::items(db, state, limit)` | `cos_inbox_items(store, state, limit) -> Result<Vec<CosInboxItem>, ChatError>` | 同じ。新しい順と `LIMIT` を変えない |
| `triage_view::fallback_unsent(db, id)` | `cos_inbox_fallback_unsent(store, id) -> Result<bool, ChatError>` | `cos_fallback::unsent(conn, id)` を `with_read_conn` の中で呼ぶ |
| `triage_view::superseded(db, item)` | `cos_inbox_superseded(store, item_id) -> Result<bool, ChatError>` | `cos_revision::superseded(conn, id)` を同じく |

- `CosInboxItem`（型・`is_terminal`・derive・欄の順・`serde` の属性・doc comment）を task-core の同じ module へ移す。JSON の形と
  `crates/task-api/src/schema.rs:377` の schema の出力を変えない（schema の生成物に差分が無いことを実装の WorkUnit が確かめる）。
  `item_row` の `addressee` の写像（`OPERATOR_ROUTE` なら `operator`、他は `human`）も同じにする。
- error は今と同じく `ChatError` で返す（`StoreError` は `ChatError::Store` に入る）。task-api は今の `chat_problem` で写すので、
  status・code（`ChatError::Store` は 500 `chat_store_error`）は変わらない。
- `cos/inbox.rs` は `triage_db` と 503 `cos_inbox_unavailable` の分岐を消し、`state.blocking(move |store| ..)` が渡す `&SqliteStore` で 4 つの関数を呼ぶ。
  `list`（187-190 行）の `blocking` は `|_|` を `|store|` にする。`resolve` の読み取り（544・550・559 行と `terminal_conflict`）と、応答の
  `item_get`（690 行）も同じ。`terminal_conflict` は `db: &Path` の代わりに `store: &SqliteStore` を受ける。
  503 `cos_inbox_unavailable` は通らない分岐で、[api.md](../../docs/api/v1/api.md) と web/ に記述が無いので、消しても文書の契約は変わらない。

### 書き込みの置き場所（変えない）

- triage の結果の書き込みは今どおり `OperationAudit::apply` → `cos_operation_apply` の transaction の中の `cos_triage_resolve_tx`。
- knowledge の pin は今どおり `SqliteStore::write_tx` の中の `pin_candidate_tx`、CoS の `attachment.reference` は `audit.apply` の中の `add_ref_tx`。
- 添付の blob と行（`upload`・`add_ref_request`・`delete_unreferenced`）は今どおり `ChatAttachmentStore`（task-core）の書き込みの接続。
- この変更の後、task-api には自前の書き込みの接続が無い。task-api に残る自前の接続は `state.rs` の `measure_journal_mode`（起動時に
  `PRAGMA journal_mode` を 1 回読む）と `routing_stage.rs` の読み取りの view（`task_ops::routing_stage` に `Inner.db_path` を渡す）で、どちらも R4 の範囲の外。
  残す理由と撤去の条件は進捗の「提案」に書く（撤去の責任者は人が起票する task）。

## 3. 読み取りを書き込みの closure の中で呼ばない規則

D1・D2 の読み取りの関数は、`write_tx`・`cos_operation_apply`（`OperationAudit::apply`）・`*_tx` の closure の中で呼ばない。
`with_read_conn` は read pool が無い構成（in-memory の store、`read_pool_size = 0`）では書き込みの接続の `Mutex` を取る
（`store/mod.rs:925-927`）。`write_tx` はその `Mutex` を closure が終わるまで持つ（`store/mod.rs:1016-1028`）ので、closure の中の読み取りは
同じ thread で同じ `Mutex` を待ち、止まる（`std::sync::Mutex` は再入できない）。read pool がある構成（`ApiState::new` の file DB、既定の
`read_pool_size = 2`）では止まらないので、普段の試験では見つからない。

- 今の呼び手はどれも closure の外にある: `resolve` の読み取りは `audit.apply` の前、`upload_effect` の `representation` は
  `OperationAudit::external` の effect（pending の記録の commit の後、transaction の外）で走る。実装の WorkUnit はこの位置を保つ。
- transaction の中で同じ行を読む必要が出たら、読み取りの関数を呼ばず、closure が受ける `&Transaction` で読む `*_tx` の関数を task-core に置く。
- この規則は `write_tx` の doc（「`write` の中で `self` を読まない」）と同じ内容を、読み取りの自由関数の側にも書く: 各関数の doc に
  「`write_tx`・`cos_operation_apply` の closure の中で呼ばない」と 1 行書く。

## 4. ChatState.attachment_db_path の撤去と knowledge.rs の旗（D4）

- `ChatState.attachment_db_path` を欄ごと消す（§1.6）。`ChatState` は `pub` の型だが、crate の外で欄を読む・作る所は無い
  （`crates/` の grep で task-api の `src` だけ）。互換のための別名や deprecated の欄は置かない。
- knowledge.rs の 3 か所の旗を `state.chat.attachment_data_dir.is_some()` から導く:
  - `inbox`（696 行）・`inbox_detail`（882 行）: 偽なら `candidate_provenance` が空を返す（`provenance[]` が空）。
  - `record`（735 行）: 偽で `attachment_ids` があれば `record_candidate` が 503 `attachments_unavailable` を返す。CoS の腕は今どおり `true` を渡す。
- この導き方は [api.md](../../docs/api/v1/api.md) の 2 つの文と一致する: §3.104 の `provenance[]` の「添付の保存が無効なら空」と、
  `POST /knowledge/inbox` の「添付の保存が無効で `attachment_ids` を渡せば 503 `attachments_unavailable`」。「添付の保存が無効」は
  `chat/attachments.rs` の `open` が 503 `attachments_unavailable` を返す条件（`attachment_data_dir` が無い）と同じ値になる。
- 挙動の差: 本番の daemon は `with_chat_wiring`（`crates/celeris/src/daemon/api.rs:436-441`、622 行で呼ぶ）が必ず `attachment_data_dir` を入れるので、
  本番の応答は変わらない。`with_chat_attachments` を呼ばない `ApiState`（試験の `admin_env` など）では、今は旗が常に真で provenance を出し、
  `attachment_ids` の pin を試みる。移した後は api.md のとおり provenance が空、record は 503 になる。文書の契約に実装を合わせる変更で、契約の変更ではない。
  既存の試験がこの差で落ちたら、その試験が添付の保存を前提にしているかを見て `env_with_attachments` に替える（期待値を弱めない）。

## 5. 反例試験（接頭辞と中身）

どの試験も lock の有無・commit の有無だけで合否が決まり、待ち時間や CPU の負荷に依らない（`agent-docs/guides/testing.md`）。busy は別接続の lock で起こす。
この ADR を書くときに SQLite（WAL）で確かめた: 別接続が `PRAGMA locking_mode=EXCLUSIVE` の後に `BEGIN EXCLUSIVE` と 1 行の書き込みをすると、
他の接続の SELECT は `database is locked`（`SQLITE_BUSY`）になる。`BEGIN IMMEDIATE` だけでは WAL の読み取りは止まらない。lock は lock を持つ接続を閉じて外す。

### `attach_conn_`（WorkUnit `attach`）

| 試験（目安の名） | 中身 | 今の試験で検出できない理由 |
|---|---|---|
| `attach_conn_rollback_leaves_no_request_row` | `write_tx` の closure が `chat_client_requests` に `kind='upload'`・`kind='attachment_ref'` の行を入れ、`chat_attachments.expires_at` を書き換えてから `Err` を返す。その後 D1 の 3 つの関数は前の値（replay は偽、`request_hash` は無し、`expires_at` は元の値）を返す。HTTP の upload は 201、references は 200 になる | 今の試験は読み取りを自前の接続で行う経路しか通らず、store の読み取りが rollback した行を返さないことを見ない |
| `attach_conn_busy_keeps_attachment_store_error` | API の `busy_timeout` を短くした env で、別接続が上の方法で lock を持つ間の `POST /chat/attachments/{a}/references` は 500 `attachment_store_error`（今と同じ code）で、`chat_attachment_refs`・`chat_client_requests` の行は増えない。lock の接続を閉じた後の同じ要求は 200 で、pin が 1 行増える | 今の添付の試験は busy を起こさない。共通の写像（503 `db_busy`）に変わっても検出できない |
| `attach_conn_reads_committed_rows_only` | 別接続が `BEGIN IMMEDIATE` で `chat_client_requests` の upload の行と `attachment_ref` の行を入れて commit しない間、`upload_replayed` は止まらずに偽を返し、`attachment_ref_request_hash` は無しを返す（WAL の読み取りは書き込みの lock を待たない）。commit の後は真と hash を返す。関数の単位で見る | 読み取りが commit 前の行を見ないこと（snapshot）を確かめる試験が無い |

### `triage_conn_`（WorkUnit `triage`）

| 試験（目安の名） | 中身 | 今の試験で検出できない理由 |
|---|---|---|
| `triage_conn_rollback_keeps_item_state` | `cos_operation_apply`（または `write_tx`）の closure が `cos_triage_resolve_tx` で item を `answered` にしてから `Err` を返す。その後 `cos_inbox_item` は `pending` を返し、`cos_inbox_items` の一覧も変わらない | triage の試験は成功の経路と拒否の記録を見るが、rollback した結果を store の読み取りが見せないことを見ない |
| `triage_conn_reads_append_no_event` | 4 つの読み取りの関数と `GET /api/v1/cos/inbox`・`GET /api/v1/cos/inbox/{i}` の後、`events` と `cos_operations` の行数が変わらない | 読み取りの経路を移すときに書き込みが混ざらないことを数で見る試験が無い |
| `triage_conn_no_read_pool_store_does_not_deadlock` | `SqliteStore::open_with(.., StoreOptions { read_pool_size: 0, .. })`（または `open_in_memory`）の store に item を入れ、4 つの関数を書き込みの transaction の外で呼ぶと返る（別 thread で走らせ、channel の受信を時間の上限つきで待つ。上限は止まったときに試験を落とすためだけの guard）。`lock_counts().reader` が 4 増える（`with_read_conn` を通った証拠） | 普段の API の store は read pool を持つので、書き込みの接続への切り替えの経路を通らない |

### `attachment_flag_`（WorkUnit `flag`）

| 試験（目安の名） | 中身 | 今の試験で検出できない理由 |
|---|---|---|
| `attachment_flag_no_data_dir_hides_provenance_and_refuses_ids` | `with_chat_attachments` を呼ばない env（`admin_env` と初期化した KB）で、`knowledge_inbox` の pin の行を持つ候補の `GET /knowledge/inbox` と `GET /knowledge/inbox/{id}` の `provenance` が空。`attachment_ids` を渡す `POST /knowledge/inbox` は 503 `attachments_unavailable` で、`_inbox` に候補が残らず pin の行も増えない | 今の旗は常に真なので、添付の保存が無効なときの 2 つの文（api.md）を通る試験が無い |
| `attachment_flag_data_dir_shows_provenance` | `env_with_attachments` で同じ候補の `provenance` が 1 件出て、`attachment_ids` つきの record が 200 で pin を作る | 旗を `attachment_data_dir` から導いた後も、有効な側の経路が変わらないことを同じ試験の組で見る |

既存の試験（添付の pin・triage・chat・`tx_boundary_contract`）は弱めない。消す試験は無い。

## 6. 採らない案

- **`ChatAttachmentStore` を `Arc<SqliteStore>` の上に作り直す（案 B）。** 採らない。`ChatAttachmentStore` の書き込み（`upload` の予約・blob の
  rename・行の確定、`delete_unreferenced`、`gc`）は blob の file 操作と行の書き込みを 1 つの手順で扱い、固有の `busy_timeout` 10 秒を持つ。
  store の書き込みの `Mutex` に載せると、大きな file の upload の間に store の他の書き込みを止めうる。手順と lock の範囲の設計が別に要り、
  R4 の目的（task-api の自前の接続をなくし、同じ DB を指す 2 つ目の値を消す）には要らない。task-api が持つ接続点は D1・D2 で 0 になる。
- **`with_read_conn` を `pub` にする（案 C）。** 採らない。task-api が `&Connection` を受けて SQL を書けるようになり、SQL の owner が task-core と
  task-api に割れたままになる（A の C4 の `rusqlite`・SQL 文字列の file も減らない）。§3 の deadlock を起こしうる呼び手が crate の外に増え、
  F が `provenance_for_owner` を `pub` から外した方針とも逆になる。

## 7. 測定

before は開始時の `git merge-base HEAD main`、after は HEAD。A の規則（final-report.md の「再現コマンド」の節）で数え、
script と設定は base の commit から `git show <base>:<path>` で取り出して両方に当てる。値は `agent-docs/progress/2026-11-19-arch-v4-h-r4/metrics.tsv`
（列 `metric scope before after checker note`。H の struct-*.tsv と同じ key・scope・checker の文字列）に置く。道具と before は WorkUnit `measure`、after は `after`。

| metric | scope | checker | 中身 |
|---|---|---|---|
| `rusqlite_prod_files` | `prod-excl-task-core-store` | `a-c4` | `rusqlite` を書く production file（H の値 75） |
| `sql_string_prod_files` | `prod-excl-task-core-store` | `a-c4` | SQL の文字列を持つ production file（H の値 50） |
| `conn_open_files` | 測定の WorkUnit が決める（同じ scope を推す） | 測定の WorkUnit が決める | 補助の指標（final-report.md の提案 12）。`Connection::open`・`TransactionBehavior`・`busy_timeout` の行を持つ production file |

- 見込み（未測定。正は after の WorkUnit の値）: task-api の `chat/attachments.rs` と `cos/triage_view.rs` が 3 つの指標から外れる。
  SQL を移す先の `task-core/src/chat/attachments.rs`・`chat/triage.rs` は既に 3 つの指標に入っているので増えない（新しい file を作れば増える）。
- 成果に数える物: task-api の自前の接続の撤去（`conn_open_files` の減り。`Connection::open`・`busy_timeout` の行は消え、置き換えの `with_read_conn` は既存）と、
  `ChatState.attachment_db_path` の撤去（同じ DB を指す値が 1 つになる）。
- 成果に数えない物: SQL の文と `CosInboxItem` の task-core への移動。`rusqlite_prod_files`・`sql_string_prod_files` の減りは記録するが、
  note に内訳（接続を開く行を消して外れた file と、SQL を既に数えられた task-core の file へ移して外れた file）を分けて書き、移動の分を成果に入れない。
  新しい file・型・trait の追加と分母の増加も成果に数えない。

## 付記 2026-11-19（WorkUnit `triage`）: D2 の置き場所を `task_core::cos_inbox` にする

- 事実: D2 の 4 つの読み取りの関数と `CosInboxItem` を `chat/triage.rs` に置くと、repo health の `size.responsibility-growth` の
  module の本体の行（単位 `crates/task-core/src/chat`）が 8026 になり、閾値 8000 を超えて新しい fail になった
  （base 7904、WorkUnit `attach` の後 7944、D2 で +82）。`chat` の中で file を分けても同じ単位に数えられる。
- 決定: `CosInboxItem`・`item_row`・`cos_inbox_item`・`cos_inbox_items`・`cos_inbox_fallback_unsent`・`cos_inbox_superseded` を
  新しい top-level module `crates/task-core/src/cos_inbox.rs` に置く。`cos_fallback`・`cos_revision` と同じく `cos_inbox_items`・
  `cos_notification_routes` の SQL だけを持つ module で、書き込み（`cos_triage_resolve_tx`）は `chat::triage` に残る。
  error は D2 のとおり `ChatError`、関数の中身・§3 の規則・§5 の試験は変えない。
- 採らなかった案: `chat` の既存の読み取り（`cos_triage_cursor`・`cos_triage_route_version` を `feed_cursor_get` に寄せる等）を
  削って 8000 に収める案。D2 の範囲の外の dispatcher の読み取りの接続を変え、それでも余裕が数行しか残らない。
- 測定への影響: `rusqlite_prod_files`・`sql_string_prod_files` は `cos/triage_view.rs` の −1 と `cos_inbox.rs` の +1 で
  この WorkUnit の分は増減 0。`conn_open_files` は `cos/triage_view.rs` の −1（`cos_inbox.rs` は `Connection::open`・
  `busy_timeout` を持たない）。§7 の見込みのうち「新しい file を作れば増える」に当たる。
- `crates/task-api/src/cos/triage_view.rs` は消さない。repo health の地図の対照（policy の file
  `agent-docs/progress/2026-11-07-health-v2-b0/capability-map.json`）が capability `cos-inbox-triage-operations-audit` の
  不変条件「既に解決済みなら Conflict にする」の根拠にこの path を挙げ、消すと `arch-map.missing_invariant` が fail になる。
  この file には接続も SQL も持たない resolve の判定 `terminal_conflict`（`cos/inbox.rs` から移した 409 の判定）だけを置く。
  §1.6 の撤去（`open`・`Connection::open`・`busy_timeout`・4 つの読み取りの関数・SQL の定数・`item_row`）はそのまま行った。

## 付記 2026-11-20（WorkUnit `flag`）: D4 の実装

- 消した物: `crates/task-api/src/chat/mod.rs` の `ChatState.attachment_db_path`（欄・doc comment・`Default` の行）と、
  `crates/task-api/src/state.rs` の `ApiState::new` の `attachment_db_path: Some(settings.db_path.clone())` の代入。
  `ApiState::new` は `ChatState::default()` を入れる。同じ DB を指す値は `Inner.db_path` の 1 つになった。
- 置き換えた判定: `ChatState::attachments_enabled()`（`pub(crate)`、`attachment_data_dir.is_some()`）を 1 つ置き、
  `crates/task-api/src/knowledge.rs` の `inbox`・`record`・`inbox_detail` の 3 か所の `state.chat.attachment_db_path.is_some()` を
  これに替えた。`chat/attachments.rs` は dir の値そのものが要るので、今どおり `attachment_data_dir` の `Option` を読んで 503 を返す
  （判定の条件は同じ）。
- 変えなかった物: `record` の `write_tx`（`pin_candidate_tx`）と `discard_candidate` の境界、CoS の `knowledge.record` が
  `record_candidate` に渡す `true`。knowledge.rs に `Connection::open`・`TransactionBehavior`・`busy_timeout` を足していない。
  API・CLI・DB の契約は変えていない。本番の daemon は `with_chat_wiring` が `attachment_data_dir` を必ず入れるので応答は変わらない。
- 試験: §5 の 2 件を `crates/task-api/tests/cos_delivery_suite/attachment_flag_contract.rs` に置いた。env は `admin_env` の
  state（`with_chat_attachments` を呼ばない）と、その clone に `with_chat_attachments` を足した state の 2 つの router で、
  同じ DB・同じ KB を読む。upload・record の body・`_inbox` と参照行の読み取りは F の `tx_boundary_contract.rs` の helper を
  `pub(crate)` にして使った（試験の中身は変えていない）。既存の試験で `env_with_attachments` へ替える必要があった物は無かった。
