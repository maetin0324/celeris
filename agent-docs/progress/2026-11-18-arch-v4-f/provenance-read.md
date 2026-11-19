---
title: アーキ v4 F provenance-read — 受信箱の provenance を store の読み取りの接続で読み、open_attachment_index を消す
tasks: [01M7MZTKB8A4NJQEEA35Q4G5JV]
status: done
updated: 2026-11-19
completed: 2026-11-19
---
# provenance-read（WorkUnit）

完了日: 2026-11-19。ADR [2026-11-18-knowledge-lifecycle-tx-boundary](../../adr/2026-11-18-knowledge-lifecycle-tx-boundary.md) の D2 を実装した。
基準は `$CELERIS_WU_BASE`（件名「WU tracked scope base」。tree は「integrate wu/contract-tests (phase prep)」と同じ）。

## D2 との違い（レビューで見てほしい点）

D2 は `SqliteStore` に `pub fn attachment_provenance(&self, ..)` を足し、`pub` method が 1 増えると書いた。
その形で実装すると `bash scripts/dev/test-affected.sh` の repo health が落ちた:
`FAIL size.responsibility-growth crates/task-core/src/chat/attachments.rs:695: 型の method の数 115 が閾値 60 を超えた（単位 crates/task-core/src/chat::SqliteStore）`
（base の 114 から 1 増えて worsened）。

この unit の範囲（`attachments.rs`・`knowledge.rs`・3 つの試験・cos-chat-home の ADR・この dir）では、`chat` の単位の他の method を消して相殺できない。
そこで `attachment_provenance` を `SqliteStore` の method ではなく、`store: &SqliteStore` を受ける `pub fn`（`crates/task-core/src/chat/attachments.rs`）にした。
- 同じ形の前例: `crates/task-core/src/route_extract.rs` の `features_for_subject(store: &SqliteStore, ..)`（`store.with_read_conn` で読む）、
  `crates/task-core/src/completion_outcome.rs` の `refresh_routing_evidence(store: &SqliteStore, ..)`。
- D2 の目的は変わらない。task-api は SQLite の接続を持たない。読み取りは store の読み取りの接続（`with_read_conn`）と計数（`lock_counts().reader`）を通る。
  任意の SQL を走らせる口（ADR §4 の案 F で退けた `with_read_conn` の `pub` 化）は作っていない。
- 結果として `SqliteStore` の `pub` method は増えない（下の C6。D2 の見込みの 631 → 632 にはならず 631 のまま）。
- 他の unit との関係: F の ADR 本文はこの unit の範囲の外なので書き換えていない。D2 の文面の直しは「提案」に書いた。

## 変更（commit の件名）

- `arch-v4 F provenance-read: 受信箱の provenance を SqliteStore::attachment_provenance で読み、open_attachment_index を消す`
- `arch-v4 F provenance-read: attachment_provenance を SqliteStore の method でなく &SqliteStore を受ける関数にする（chat::SqliteStore の method 数を増やさない）`
- `arch-v4 F provenance-read: 進捗を書く`（この file）

### 消したもの

- `crates/task-api/src/knowledge.rs`: `open_attachment_index`（添付の DB の file を読み取り専用で自分で開き、busy_timeout 10 秒を置く関数）。
- `knowledge.rs` の `candidate_provenance` の `conn: Option<&rusqlite::Connection>` の引数。
- `inbox`・`inbox_detail` の `state.chat.attachment_db_path.clone()` と、`state.blocking(move |_| ..)` の中の接続の取得。
- `crates/task-core/src/chat/attachments.rs`: `provenance_for_owner` の `pub`（crate の外の呼び手は `knowledge.rs` だけだった。今は同じ file の `attachment_provenance` だけが呼ぶ private の関数）。

### 足したもの

- `crates/task-core/src/chat/attachments.rs`: `pub fn attachment_provenance(store: &SqliteStore, owner_kind: &str, owner_id: &str) -> Result<Vec<AttachmentProvenance>, StoreError>`。
  本体は `store.with_read_conn(|conn| provenance_for_owner(conn, ..))`。`AttachmentError::Db` は `StoreError::Sqlite`、それ以外（`InvalidPath`）は `StoreError::Invalid` に写す。
- `knowledge.rs` の `candidate_provenance(store, attachments, id)`。`attachments` は `state.chat.attachment_db_path.is_some()`（bool）で、偽なら今と同じく空の provenance を返す。
- 新しい trait・Port・flag・fallback は無い。`SqliteStore` の method も増えない。

### 応答の違い

- 成功の応答（`provenance[]` の欄と値）は変わらない。添付の保存が無効なら空なのも変わらない。
- 読み取りの失敗: 旧は 500 `internal`（detail `attachment index: …` / `attachment provenance: …`）。新は `From<StoreError>` の写像で、`SQLITE_BUSY` は 503 `db_busy`、それ以外は 500 `internal`。
  他の store の読み取りの route と同じ扱い。
- busy の待ち: 旧は自前の接続の busy_timeout 10 秒。新は store の読み取りの接続の設定に従う。

### ADR

- `agent-docs/adr/2026-10-05-cos-chat-home.md` の D4 の実装の行の `provenance_for_owner(conn, "knowledge_inbox", id)` を
  `task_core::chat::attachments::attachment_provenance(store, "knowledge_inbox", id)` に書き換えた。
- 同じ ADR の末尾に「付記: 受信箱の provenance の読み取りを store へ移す（2026-11-18）」を足し、F の ADR へ link した。method にしなかった理由も書いた。

## 数値（before = `$CELERIS_WU_BASE`、after = 提出時の HEAD。数え方は A の baseline.md §7 の C4・C6 の command と python の snippet）

| 指標 | before | after |
|---|---|---|
| `knowledge.rs` の `rusqlite` の出現（行） | 8（7 行） | 4（4 行。残りは record の pin の 4 つで、knowledge-pin の範囲） |
| `knowledge.rs` の `rusqlite::Connection` | 4 | 1 |
| `knowledge.rs` の `attachment_db_path.clone()` | 3 | 1（`record`。knowledge-pin の範囲） |
| `knowledge.rs` の `open_attachment_index`・`open_with_flags`・`OpenFlags` | 3・1・1 | 0・0・0 |
| C6 `SqliteStore` の pub method（snippet、path 無しの block） | block 102、`pub` 631、`pub(crate..)` 67、private 41 | 同じ |
| C6 同じ（path 付きの block も数える版） | block 105、`pub` 634 | 同じ |
| task-core の `chat/attachments.rs` の provenance の `pub fn` | `provenance_for_owner`（`&Connection` を取る） | `attachment_provenance`（`&SqliteStore` を取る） |
| C6 `&Connection`・`&Transaction` の引数の行 | 347（task-api 15） | 347（同じ） |
| C6 `Fn(&Connection)` の引数の行 | 22 | 22 |
| C4 `rusqlite` を書く production file（task-api） | 17 | 17 |
| C4 SQL 文字列の production file | 50 | 50 |

- C6 の `&Connection` の行が減らないのは、`provenance_for_owner(conn: &Connection, ..)` は private になったが行は残り、§7 の正規表現は可視性を見ないから。
  消した `conn: Option<&rusqlite::Connection>` は `Option<` が前に付くので、もとから §7 の正規表現に当たっていなかった。
  task-core の外から `&Connection` を渡す公開の関数が 1 つ減ったことは、上の `pub fn` の行で示す。
- C4 の file 数が減らないのは、`knowledge.rs` に record の pin の `rusqlite` が残るから（knowledge-pin が消す見込み）。
- 行数（`git diff --numstat $CELERIS_WU_BASE HEAD`、この file を除く）: `knowledge.rs` +14 −29、`attachments.rs` +18 −1、cos-chat-home の ADR +7 −1。
  production の Rust は +32 −30（純増 2 行）。試験の行の差は 0。純減はこの unit の目標ではない（ADR §7）。

## 証拠（提出時の HEAD で実行）

- `cargo nextest run -p task-api --test cos_delivery_suite --test knowledge --test cos_chat_attach_pin --test cos_live_fix_d2 --test cos_knowledge_use --test cos_delivery_suite_a -E 'test(tx_boundary_contract) | binary(knowledge) | binary(cos_chat_attach_pin) | binary(cos_live_fix_d2) | binary(cos_knowledge_use) | test(cos_chat_attach_handoff_kb)'`
  → exit 0、27 件 pass（tx_boundary_contract 8、knowledge 10、cos_chat_attach_pin 1、cos_live_fix_d2 3、cos_knowledge_use 3、cos_chat_attach_handoff_kb 2）。
- `git diff --quiet $CELERIS_WU_BASE -- crates/task-api/tests/cos_delivery_suite/tx_boundary_contract.rs` → exit 0（契約試験は変えていない。`crates/task-api/tests` の差分 0）。
- `cargo fmt --all -- --check` → exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0（real 16m20s）。repo health `verdict=pass`（`size.responsibility-growth` pass、worsened 0）、
  nextest 8479 件 pass・22 件 skip、web の unit 705 件 pass（101 file）、test-ts-verify 20 件 pass。
  method の形の commit の時点の 1 回目は上の `size.responsibility-growth` の fail で exit 1 だった。
- 試験は足していない。守る不変条件（provenance の欄と値、pin の後の一覧と詳細、添付の無い候補）は K1 と `cos_chat_attach_handoff_kb_candidate_keeps_provenance`・
  `cos_live_fix_d2` が既に確かめている。

## 未解決事項

- 添付の保存が無効なときの「provenance を空にする」分岐を直接見る試験は無い（今回も足していない）。`ApiState::new` は `attachment_db_path` を必ず `Some` にするので、
  この分岐は `ChatState::default()` を使う構成でだけ通る。

## 提案

- F の ADR の D2 の「`SqliteStore` に `pub fn attachment_provenance(&self, ..)` を足す」「`pub` method は 1 増える（631 → 632）」を、実装の形
  （`chat::attachments::attachment_provenance(store: &SqliteStore, ..)`、`pub` は 631 のまま）と理由（`size.responsibility-growth` の `chat::SqliteStore` 114 → 115）に
  合わせて直す。close の unit（ADR に実測の付記を書く）が付記として書くのがよい。
- `inbox`・`inbox_detail` の `attachment_db_path.is_some()` の分岐は、knowledge-pin が record の `attachment_db_path` を消した後も残る。
  `ChatState` の欄を消すときに一緒に消せる（D5 の範囲外の判断）。
