---
tasks: [01M7MZTKB8A4NJQEEA35Q4G5JV]
---
# ADR 2026-11-18: knowledge.rs の自前の SQLite 接続と task-ops の lifecycle.rs の closure の transaction を、SqliteStore の write_tx・with_read_conn と adapter の側の境界へ移す（アーキ v4 F）

- 日付: 2026-11-18
- 状態: Accepted（本文は設計。この ADR を書いた WorkUnit `adr-doc` は `crates/`・`web/`・既存の ADR を変えていない。実装と実測の結果は、コードを変える unit と
  `close` が末尾に付記を足す）
- task: 01M7MZTKB8A4NJQEEA35Q4G5JV（依頼キー celeris-architecture-scalability-v4-20261117 の子 F、親 01M7MZT8KX547464YYS76QNP4G）
- 進捗: [2026-11-18-arch-v4-f.md](../progress/2026-11-18-arch-v4-f.md)、before の値と再現 command: [arch-v4-f/adr.md](../progress/2026-11-18-arch-v4-f/adr.md)
- 関係: [ADR 2026-11-18 org-write-shared-use-case](2026-11-18-org-write-shared-use-case.md)（D。付記「2 つの形の使い分けと撤去の条件」）、
  [ADR 2026-11-04 project-controller](2026-11-04-project-controller.md) D12 と付記 A1 の 7・9（`human_epoch` を書き込みと同じ transaction で増やす。不変条件 `lifecycle-human-epoch-same-tx`）、
  [ADR 2026-10-09 CoS の全変更操作](2026-10-09-cos-operations-all-mutations.md)（`cos-audit-same-tx`、`Applied` の形の D5）、
  [ADR 2026-10-07 cos-live-fixes](2026-10-07-cos-live-fixes.md) D2 と付記 D2-a（`POST /knowledge/inbox` と `knowledge.record`）、
  [ADR 2026-10-05 cos-chat-home](2026-10-05-cos-chat-home.md) D4（受信箱の候補の provenance）、
  [ADR-0064](0064-db-local-disk-and-store-resilience.md) D4（SELECT は読み取りの接続）、[ADR-0133](0133-inbox-and-notifications.md) の付記（`SqliteStore::lock_counts`）、
  [ADR 2026-11-17 境界の意味を変える改修の risk](2026-11-17-boundary-change-risk-review.md)（`transaction` class の設計 review）、
  [2026-10-11 破壊的変更を既定にする](2026-10-11-breaking-changes-by-default.md)。
- 行番号は、この ADR を書いた時点の HEAD（`git merge-base HEAD main` と同じ内容。件名は進捗の adr.md の「基準」）の行。

## 1. 根本原因

`crates/task-api/src/knowledge.rs` と `crates/task-ops/src/lifecycle.rs` は repo の profile の高 risk の `transaction` class
（不変条件 `task-state-in-sqlite`・`lifecycle-human-epoch-same-tx`）に入る。どちらも、transaction の境界を `SqliteStore` の外か、
2 つの crate にまたがって持つ。

1. **record の HTTP 経路が自前の接続で pin する。** `record_op`（`knowledge.rs:813-903`）の `audit = None` の腕は、
   `rusqlite::Connection::open(db)`（874 行）で `state.chat.attachment_db_path` を開き、`busy_timeout` 10 秒を置き、
   `transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)`（878 行）の自前の transaction で `pin_candidate_tx` を走らせて commit する。
   `attachment_db_path` は `ApiState::new` が store と同じ `settings.db_path` を入れる（`state.rs:176-178`）ので、同じ SQLite の file に
   2 本目の書き込みの接続を開いていることになる。この書き込みは `SqliteStore` の書き込みの Mutex を通らず、同じ process の store の書き込みと
   SQLite の lock で競る（待ちは busy_timeout の 10 秒、`lock_counts` にも出ない）。CoS の腕（`audit = Some`）は同じ pin を
   `OperationAudit::apply` の store の transaction で書くので、同じ書き込みが 2 つの接続の経路を持つ。
2. **受信箱の provenance を別の接続で読む。** `inbox`（703 行）と `inbox_detail`（909 行）は、要求ごとに `open_attachment_index`（654-666 行）で
   `rusqlite::Connection::open_with_flags(.., SQLITE_OPEN_READ_ONLY)` の接続を開き、`candidate_provenance`（668-678 行）が
   `task_core::chat::attachments::provenance_for_owner(conn, "knowledge_inbox", id)` を呼ぶ。store の読み取りの接続（`with_read_conn`、ADR-0064 D4）を通らない。
   `provenance_for_owner`（`task-core/src/chat/attachments.rs:698`）が `pub` で `&Connection` を取るので、task-api が SQL の接続を持つ形になっている。
3. **task-ops の `apply_project_action` が closure で加算を受け、transaction の境界の owner が割れる。** `apply_project_action`
   （`task-ops/src/lifecycle.rs:102-137`）は `also: F` where `F: FnOnce(&rusqlite::Connection) -> Result<(), StoreError>`（110 行）を受け、
   自分で `store.write_tx` を開いて `apply_project_change_tx` の後に `also(tx)` を呼ぶ。task-api の `project_action_op`（`task-api/src/lifecycle.rs:70-104`）は
   `audit = None` のとき `|tx| human.bump_tx(tx, Some(project_id))` を渡す（80-82 行）。transaction を開くのは task-ops、その中身の半分（human epoch の加算）を
   決めるのは task-api で、共有の関数に特定の呼び手（HTTP の human epoch）のための hook がある（変更の規約 3）。`apply_project_change_tx`（410 行）の
   `tx` は `&rusqlite::Connection` で、transaction の外の接続でも呼べる型になっている。
4. **HTTP と CoS は `Applied` の形の別経路を持つ。** `record_op` と `project_action_op` は `Option<&OperationAudit>` を受けて `Applied::Direct`・
   `Applied::Audited` を返す（ADR 2026-10-09 D5 の形）。1 つの関数の中で HTTP と CoS が別々の transaction の作り方（1 と 3 の自前の境界、
   CoS の `audit.apply`）に分かれる。`record_op` の CoS の腕は `audit.apply` の closure の失敗を `failure` に退避して取り出す（888-901 行）が、
   これは `OperationAudit::apply_checked`（`cos/operations.rs:307-330`）と同じ処理の書き直しである。CoS の `project.*` は
   `cos/ops/projects.rs:238-244` で `HumanEpoch::NotCounted` を渡すが、`project_action_op` の CoS の腕は `human` を使わない（渡す値に意味が無い）。

症状は、pin と provenance の読み書きが store の Mutex・読み取りの接続・計数の外にあること、task-ops の transaction に task-api の規則が
closure で入ること、同じ書き込みの経路が HTTP と CoS で 2 本あることである。

## 2. 決定

### D1. record の HTTP 経路の pin は `SqliteStore::write_tx` で書く

- HTTP の `record`（`knowledge.rs:739-754`）は、候補を書いた後、`attachment_ids` が空でなければ
  `store.write_tx(|tx| pin_candidate_tx(tx, &ids, &candidate_id, None))` で pin する。`ApiProblem` は `From<StoreError>` を持つ（`problem.rs:598`）ので
  `write_tx` の `E` にそのまま使う。`thread` は今と同じ `None`（HTTP は thread を限らない）。
- `write_tx` の closure の中で store を読まない（`write_tx` の doc の規則）。git の書き込み（候補の file と commit）は `write_tx` の前に終える。
  書き込みの Mutex を持ったまま git を呼ばない。
- `record_op` の `attachment_db: Option<&Path>` 引数、`rusqlite::Connection::open`・`busy_timeout`・`TransactionBehavior::Immediate` の自前の transaction、
  `record`・`inbox`・`inbox_detail` の `state.chat.attachment_db_path.clone()` を消す。
- `attachments_unavailable`（`knowledge.rs:801-807`）と、それを返す分岐（834-836 行。HTTP で `attachment_db` が `None` のとき 503）も消す。
  `ApiState::new` が `attachment_db_path` を必ず `Some(settings.db_path)` にするので、この分岐は今も通らない。添付の保存（`attachment_data_dir`）が
  無い daemon では `ready` の添付が無いので、`pin_candidate_tx` の検査（`task_pin_problem_tx`）が今と同じく 422 `invalid_attachment` を返す。
  `chat/attachments.rs` と `cos/inbox.rs` の `attachment_db_path` は D5 の範囲外で、`ChatState` の欄は残る。
- 応答の違い: 書き込みの接続を取れないときの失敗が、今の 500 `internal`（`attachment pin: …`）から `store_problem` の写像（`SQLITE_BUSY` は 503 `db_busy`）に変わる。
  `write_tx` を使う他の route と同じ扱いで、成功・検証の拒否（422 `invalid_attachment`）・pin の書き込みの失敗（500 `internal`）の応答は変わらない。

### D2. 受信箱の provenance は `SqliteStore::attachment_provenance`（`with_read_conn`）で読む

- task-core に `pub fn attachment_provenance(&self, owner_kind: &str, owner_id: &str) -> Result<Vec<AttachmentProvenance>, StoreError>` を足す。
  本体は `self.with_read_conn(|conn| provenance_for_owner(conn, owner_kind, owner_id).map_err(..))` で、`AttachmentError::Db` は `StoreError` の SQLite の variant、
  それ以外（`InvalidPath` など）は `StoreError::Invalid` に写す。置き場所は `provenance_for_owner` と同じ `crates/task-core/src/chat/attachments.rs` の
  `impl SqliteStore` の block（`chat/` の他の file と同じ置き方。`store/` の下ではない）。
- `provenance_for_owner` は crate の中に縮める（`pub` → `pub(crate)` 以下。呼び手が `attachment_provenance` だけなら private）。
  今の production の呼び手は `knowledge.rs:675` だけで、試験の呼び手は無い。
- `knowledge.rs` の `candidate_provenance` は `store.attachment_provenance("knowledge_inbox", id)` を呼び、`inbox`・`inbox_detail` の `state.blocking` は
  `move |store|` で store を受ける。`open_attachment_index` を消す。
- **`SqliteStore` の `pub` method は 1 増える**（C6 の pub method の数え方で 631 → 632。`impl SqliteStore` の block も 1 増える見込み）。
  task-api が `&Connection` を持たずに provenance を読むための入口で、読み取りの接続・計数（`lock_counts().reader`）を通る。

### D3. `record_op` を D の付記の形へ移す

[ADR 2026-11-18 org-write-shared-use-case](2026-11-18-org-write-shared-use-case.md) の付記「2 つの形の使い分けと撤去の条件」の形にする。
共有の操作に `Option<&OperationAudit>` を渡さず、`Applied` を返させない。

- 共有（`knowledge.rs`、`pub(crate)`）:
  - `record_candidate(store, root, body) -> Result<KnowledgeRecordResult, ApiProblem>`: 今の `record_op` の 820-868 行から `discard` の 853-858 行を除いた部分（`require_kb`・`page_path`・
    `project_refs`・`layout`・`task_ops::knowledge::record_in`・結果の組み立て）。候補の file を `_inbox/<id>.md` に書いて git commit する。SQLite は読むだけ。
  - `pin_candidate_tx(tx: &rusqlite::Transaction<'_>, ids, candidate, thread)`: 今のまま（773-799 行。private → `pub(crate)`）。
  - `discard_candidate(root, id, problem) -> ApiProblem`: 今の `discard`（853-858 行）。`inbox_reject` で候補を消し、消せなければ warn を残して `problem` を返す。
- HTTP adapter（`knowledge.rs` の `record`）: `record_candidate` → D1 の `write_tx` の pin → 失敗なら `discard_candidate`。`tracing::info!` の行は残す。201 と
  `KnowledgeRecordResult` の本文は変えない。
- CoS adapter（`cos/ops/decisions.rs:197-207` の `knowledge.record` の腕）: `record_candidate` の失敗は `audit.reject(store, "knowledge", "new", problem)`（今と同じ）。
  成功したら `audit.apply_checked(store, "knowledge", &id, "knowledge.record", |tx| { pin_candidate_tx(tx, .., Some(&audit.ctx.thread_id))?; 結果の JSON })` で、
  失敗なら `discard_candidate`。添付が無くても `apply_checked` を通す（監査の行と event は毎回書く）。`counted_record`（`cos/knowledge_use.rs:93`）で包むのは今と同じ。
  腕は `apply_checked` の `CosOperation` をそのまま返す（`audited(Applied)` を通さない）。
- 失敗時は候補を消す（[cos-live-fixes 付記 D2-a](2026-10-07-cos-live-fixes.md) の「どちらかが失敗したら候補ファイルを残さない」を保つ）。
  git の commit の後に process が落ちたときの隙（D2-a の「残る隙」）は変えない。
- `serde_json::to_value` の失敗（`KnowledgeRecordResult` では起きない）は `ApiProblem::internal` にする。今は `ChatError::Invalid` を経由する。

### D4. `apply_project_action` を closure ごと消す

- task-ops（`crates/task-ops/src/lifecycle.rs`）は次の pub fn だけを持つ。human epoch の加算・`Principal`・`HumanEpoch` を持たない。
  - `plan_project_action(store, id, action, now) -> Result<ProjectChange, OpsError>`（今のまま。読むだけ）。
  - `apply_project_change_tx(tx: &rusqlite::Transaction<'_>, id, &ProjectChange) -> Result<Vec<TaskId>, StoreError>`（引数の型を `&rusqlite::Connection` から
    `&rusqlite::Transaction<'_>` に変える。transaction の外の接続で呼べないようにする）。
  - 結果の組み立ての pub fn（名は `project_action_result(store, id, &ProjectChange, cancelled: Vec<TaskId>) -> Result<ProjectLifecycle, OpsError>` を推す。
    今の `apply_project_action` の 122-136 行。`pause_result` と同じく書いた後に読むだけ）。
- HTTP の `project_action`（`task-api/src/lifecycle.rs:106-126`）は `state.blocking` の中で、`plan_project_action` の後、`ProjectChange::Unchanged` でなければ
  `store.write_tx(|tx| { let c = apply_project_change_tx(tx, id, &change)?; human.bump_tx(tx, Some(id))?; Ok(c) })` を呼び、`project_action_result` で応答を作る。
  順序（変更 → 加算）と、Unchanged では書かず加算もしないことは今と同じ。加算の規則（`HumanEpoch::of`・`bump_tx`・`COALESCE_SECS`）は
  `crates/task-api/src/human_epoch.rs` の 1 つのまま。plan の失敗は `ops_problem(store, e, Some(trigger))`、`write_tx` の失敗は `store_problem`（今の
  `OpsError::Store` の写像と同じ）。
- CoS の `project.*`（`cos/ops/projects.rs:224-245`）は、腕の中で `plan_project_action` の失敗を `audit.reject(store, "project", &target_id, ops_problem(..))` にし、
  `audit.apply(store, "project", &target_id, matched.action, |tx| apply_project_change_tx(tx, ..) の後に {"project_id", "cancelled_tasks"} の JSON)` を直接呼ぶ。
  今の `HumanEpoch::NotCounted` のとおり加算しない（腕は `HumanEpoch` を参照しない）。`apply` を使う（`apply_checked` にしない）のは、`StoreError` の失敗の
  写像（`chat_problem`）を今と同じにするため。Unchanged でも operation の行を書くのは今と同じ。
- `project_action_op` を消す。`project_trigger` は HTTP と CoS の両方が使うので `pub(crate)` にする。`project_action_name` は CoS の腕が `matched.action`（同じ文字列）を
  使うので消す。

### D5. 範囲外と理由

| 範囲外 | 理由 |
|---|---|
| task-ops の他の closure の `also`（`comment.rs` の `reopen_with`、`decision.rs` の `apply_plan_also`、`gate.rs` の `cancel_with`、`op_writes.rs` の `WritesAlso` と `human_epoch.rs:88-93` の `writes`） | 同じ形だが、承認・判断・gate の別々の領域で、領域ごとに HTTP と CoS の契約試験を旧コードで先に固定する作業が要る。F の依頼は 2 file（`knowledge.rs`・task-ops の `lifecycle.rs`）の pilot。before の C6 の `Fn(&Connection)` は task-ops 5 行で、F は `lifecycle.rs:110` の 1 行だけを消す |
| `knowledge.rs` の `accept_op`・`reject_op`・`put_page_op`、task-api の `lifecycle.rs` の `task_pause_op` の `Applied` の形 | 自前の接続を持たず、transaction の境界は CoS の `audit.apply` と store の method にある。D の付記の撤去の条件（子 H の完了報告で残数を数える）に任せる |
| `chat/attachments.rs`（80・122・243・387 行）と `cos/inbox.rs:886` の `attachment_db_path` の接続 | `ChatAttachmentStore`（blob の file と DB の行を一緒に扱う添付の store）の経路で、knowledge の境界とは別の設計が要る |
| `cos/operations.rs` の `apply`・`apply_checked` の `Fn(&Transaction)`、`cos/ops/surface.rs:270`・`repos.rs:148` の closure | `apply`・`apply_checked` は CoS の監査の transaction の owner そのもの（`cos-audit-same-tx`）。他の 2 つは別の領域 |
| `cos/operations.rs`（`authz` class）の変更 | `apply_checked`・`reject` をそのまま使う。変える必要が無い |
| handlers ⇄ cos の循環（C2）の解消 | F の目的は transaction の境界の移動。`knowledge.rs`・task-api の `lifecycle.rs` は他の `*_op` のために `crate::cos::operations` を参照し続ける |
| 候補の git commit と SQLite の commit の非原子性 | D2-a の「残る隙」。`record_in` を書き込みと commit に割る別の設計が要る |
| 行数の純減 | F の受け入れ条件に無い（変更の規約 11）。source-size-report は増減の確認のために測る |

### D6. 試験と撤去の条件

- **契約試験を旧コードで先に固定する。** 実装の前の WorkUnit が、既存の suite に mod を 1 つ足して（新しい test binary を作らない。例
  `crates/task-api/tests/cos_delivery_suite/main.rs`）旧コードに当てて通し、実装の後に試験を変えずに通ることを確かめる。既存の試験で検出できないもの:
  1. HTTP の record で pin の書き込みが失敗したとき（`chat_attachment_refs` への INSERT を SQLite の trigger で断る）、応答の status と code、ref の行が 0、
     候補の file が `_inbox` に残らないこと、`chat_attachments` の行が残ること。既存の `cos_live_fix_d2_invalid_scope_is_422_and_writes_nothing` は
     書き込みの前の検査（422 `invalid_attachment`）しか見ない。
  2. CoS の `knowledge.record` で同じ trigger のとき、応答の status と code、operation が rejected で detail を持つこと、ref の行が 0、候補の file が残らないこと。
     既存の `cos_live_fix_d2_cos_operation_records_audit_row` は検査の拒否だけを見る。
  3. CoS の `project.archive` を 2 回（2 回目は Unchanged）: 2 回目も applied で `cancelled_tasks` が空、案件の行は変わらず、operation の行は 2 つ。
  4. CoS の `project.pause` で案件の UPDATE を trigger で断ったとき: 案件の状態が変わらず、応答の status と code、operation の行の state と detail。
     値は旧コードの結果をそのまま固定する（D4 の `apply` の写像を保つことの確認）。
- **既存の試験を変えずに通す。** `crates/task-api/tests/routing_budget_suite/project_wake_human.rs` の `project_wake_api_human_each_route_bumps_once`・
  `_write_failure_keeps_the_epoch`・`_bump_failure_rolls_the_write_back`・`_refused_or_unchanged_requests_do_not_bump`・`_cos_is_not_counted`
  （ROUTES に Pause・Resume・ProjectCancel・Archive・Unarchive を含む）、`cos_delivery_suite/cos_ops_projects_cron.rs` の
  `cos_ops_projects_cron_project_lifecycle_is_audited`、`crates/task-api/tests/cos_live_fix_d2.rs` の 3 件、
  `cos_delivery_suite_a/cos_chat_attach_handoff_kb.rs` の `cos_chat_attach_handoff_kb_candidate_keeps_provenance`・`_pin_missing_owner_404`、
  `crates/task-api/tests/cos_chat_attach_pin.rs`、`crates/task-api/tests/knowledge.rs`、`crates/task-api/tests/lifecycle.rs`。
- **呼び出しを移す試験（検査は弱めない）。** `apply_project_action` を呼ぶ試験は 2 か所ある。`crates/task-ops/src/lifecycle/tests.rs:13` の `act` は
  `plan_project_action` → `store.write_tx(apply_project_change_tx)` → `project_action_result` に移す。task-dispatch の
  `crates/task-dispatch/src/dispatcher/tests/project_wake.rs:814`（`project_wake_human_races_pause`）は同じ `write_tx` の中で
  `apply_project_change_tx` の後に `SqliteStore::project_controller_human_bump_tx` を呼ぶ形に移す。どちらも assert は変えない。
- **撤去の条件。** D1〜D4 の撤去（§5）は同じ変更で行い、旧新を並存させない（互換の分岐・切替の flag を置かない）。失敗したら git の revert で戻す。
  D の付記の `Applied` の形の撤去の条件・期限（子 H の完了報告）・責任者は変えない。F の後も `knowledge.rs` と task-api の `lifecycle.rs` は
  他の `*_op` のために付記の 2 本の command の一覧に残る（file 数は 14・18 のままの見込み）。F は関数の単位で 2 つ（`record_op`・`project_action_op`）を移す。

### D7. 旧い経路を書いた ADR の行は、コードを変える unit が付記で書き換える

この ADR を書く WorkUnit は既存の ADR を変えない。コードを変える unit が、同じ変更で次の ADR の末尾に付記を足し、旧い経路の行を新しい経路で言い直す
（本文の行は書き換えず、付記がそれを上書きすると書く）。

| ADR | 旧い経路を書いた所 | 付記で書くこと | 担当 |
|---|---|---|---|
| [2026-11-04-project-controller](2026-11-04-project-controller.md) | 付記 A1 の 7 の表の `POST /projects/{id}/{pause,resume,cancel,archive,unarchive}` の行（`lifecycle::project_action_op` → `task_ops::lifecycle::apply_project_action`）と、A1 の 7 の本文・9 の表（`human_epoch.rs` の module doc が「付記 A1 の 9」と呼ぶ表）の「`/cos/operations` の監査経路（`*_op` の `audit = Some`）は数えない」 | HTTP の `project_action` が `SqliteStore::write_tx` の中で `apply_project_change_tx` と `HumanEpoch::bump_tx` を呼ぶこと。CoS の `project.*` は `cos/ops/projects.rs` で `reject`・`apply` を直接呼び、加算しないこと | D4 の unit |
| [2026-10-07-cos-live-fixes](2026-10-07-cos-live-fixes.md) | D2 と付記 D2-a（「pin（と CoS の監査）を SQLite に commit」）、close の表の D2 の行（「本体 `record_op` を handler と CoS operation が共有」） | 共有は `record_candidate`・`pin_candidate_tx`・`discard_candidate`。HTTP は `write_tx`、CoS は `reject`・`apply_checked`。失敗時に候補を消すのは同じ | D1・D3 の unit |
| [2026-10-05-cos-chat-home](2026-10-05-cos-chat-home.md) | D4「PDF は KB inbox candidate を作って pin し…」の実装の行（`candidate_provenance`（`provenance_for_owner(conn, "knowledge_inbox", id)`）） | provenance は `SqliteStore::attachment_provenance`（`with_read_conn`）で読むこと | D2 の unit |

`crates/task-api/src/human_epoch.rs` の module doc（3-6 行）・task-ops の `lifecycle.rs` の doc comment（99-101・326-328 行）・task-api の `lifecycle.rs` の
doc comment（66-69 行）も、同じ変更で新しい経路に合わせる。`docs/architecture-map.md` の行は、owner の file が変わらないので変えない見込み
（変わるなら同じ unit が直す）。

## 3. task-ops に human epoch の加算を置かない理由

前回の設計 review が、加算を task-ops へ移す案を退けた（出典は親 task の計画の design_brief の options。review の本文そのものはこの WorkUnit の run からは
読めない）。理由:

1. **加算の要否は task-api の認証の結果で決まる。** `HumanEpoch::of(&Caller)`（`human_epoch.rs:27-35`）が `Principal::Human` だけを数え、`Operator`・`Mcp`・
   `Cos`・`Unauthenticated` を数えない。`Principal` と `Caller` は task-api の型で、task-ops は task-api に依存しない（依存の向きは task-api → task-ops）。
   task-ops が加算するには、呼び手の種類を知る引数（`bool`・enum）を共有の関数に足すことになる。これは特定の呼び手のための分岐を共有の interface に
   足すこと（変更の規約 3）で、今の closure の hook を名前を変えて残すだけになる。
2. **規則が 2 か所になる。** 加算の口は `bump_tx`・`bump_task_tx`・`bump_posted_tx` の 3 つで、project-controller 付記 A1 の 7 の表の 10 以上の経路が使う。
   `COALESCE_SECS = 30`（`human_epoch.rs:17`。API は daemon の設定を持たないので固定値）も human_epoch.rs にある。案件の lifecycle の 1 経路だけを
   task-ops で加算すると、加算の規則と `COALESCE_SECS` の正本が 2 つになる（規約 4）。
3. **CoS は加算しない。** CoS の `project.*` は `HumanEpoch::NotCounted` で、加算の有無は adapter（HTTP か CoS か）で決まる。adapter が transaction を開き、
   その中で業務の書き込み（task-ops の `apply_project_change_tx`）と adapter の規則（HTTP だけの `bump_tx`、CoS だけの監査の行）を並べれば、
   task-ops は transaction を開かず、どちらの規則も知らない。transaction の境界の owner は adapter の 1 つになる（D の付記の形と同じ）。

## 4. 比べた案

| 案 | 内容 | 判定と理由 |
|---|---|---|
| A（採用） | D1〜D4: pin は `write_tx`、provenance は `attachment_provenance`、record と project の操作は D の付記の形。task-ops は plan・`apply_project_change_tx`・結果の組み立てだけを持ち、HTTP adapter が `write_tx` の中で `bump_tx` を呼ぶ | transaction の境界は `SqliteStore`（HTTP）と `OperationAudit`（CoS）の 2 つの既存の owner だけになる。加算の規則は human_epoch.rs の 1 つのまま。新しい trait・Port・flag を足さない |
| B | task-ops に human epoch の加算を移す（`apply_project_action` が加算の要否を受ける） | 退けた。§3 |
| C | `apply_project_action` の closure を残し、型だけ `FnOnce(&rusqlite::Transaction<'_>)` に変える | 退けた。transaction を開くのは task-ops、中身の半分を決めるのは task-api という割れ方が残る。CoS の別経路（`Applied` の形）も残る |
| D | `record_op` の HTTP の pin だけを `write_tx` に変え、`Option<&OperationAudit>` と `Applied` の形は残す | 退けた。自前の接続は消えるが、CoS の腕の `apply_checked` の書き直し（`failure` の退避）と、1 つの関数が 2 つの transaction の作り方を持つ形が残る。D の付記は「その領域の共有の操作を変える task が同じ変更で移す」と決めている |
| E | provenance を task-api の中で `SqliteStore` の書き込みの接続から読む（`write_tx` の中で SELECT） | 退けた。読み取りに書き込みの Mutex と `BEGIN IMMEDIATE` を取る。ADR-0064 D4 の「SELECT は読み取りの接続」に反する |
| F | `with_read_conn` を `pub` にして、task-api が `provenance_for_owner` を直接呼ぶ | 退けた。`pub` の数は同じでも、任意の SQL を走らせる接続を task-api に渡す口になり、C6 の SQL の公開面が広がる |

## 5. 撤去対象

同じ変更で消す。旧新の並存は無い。

- `crates/task-api/src/knowledge.rs`: `record_op`（813-903 行）、その `attachment_db` 引数と `Option<&OperationAudit>`・`Applied` の使用、
  `rusqlite::Connection::open`・`busy_timeout`・`TransactionBehavior::Immediate` の自前の transaction（871-882 行）、`attachments_unavailable`（801-807 行）とその分岐、
  `open_attachment_index`（654-666 行）、`candidate_provenance` の `conn: Option<&rusqlite::Connection>` 引数、3 つの handler の `attachment_db_path` の clone。
- `crates/task-api/src/cos/ops/decisions.rs`: `knowledge.record` の腕の `record_op(.., None, input, Some(audit))` と `audited(..)` の包み。
- `crates/task-core/src/chat/attachments.rs`: `provenance_for_owner` の `pub`（crate の中へ縮める）。
- `crates/task-ops/src/lifecycle.rs`: `apply_project_action`（doc comment と本体の 99-137 行）と `also` の closure 引数（110 行の `FnOnce(&rusqlite::Connection)`）、
  `apply_project_change_tx` の `tx: &rusqlite::Connection`（`&rusqlite::Transaction<'_>` に変える）。
- `crates/task-api/src/lifecycle.rs`: `project_action_op`（66-104 行）と `project_action_name`（55-64 行）。
- `crates/task-api/src/cos/ops/projects.rs`: `project_action_op(.., HumanEpoch::NotCounted, Some(audit))` と `audited(..)` の包み（238-244 行）。
- 試験: `apply_project_action` の 2 つの呼び手（D6 のとおり移す。消さない）。

## 6. 不変条件と試験の対応

| 不変条件 | 守ること | 試験・検査 |
|---|---|---|
| `lifecycle-human-epoch-same-tx` | HTTP の案件の lifecycle で、変更と human epoch の加算が同じ `write_tx` の中に入り、片方だけ残らない。Unchanged・拒否では加算しない。CoS は加算しない | `project_wake_api_human_each_route_bumps_once`・`_write_failure_keeps_the_epoch`・`_bump_failure_rolls_the_write_back`・`_refused_or_unchanged_requests_do_not_bump`・`_cos_is_not_counted`（`routing_budget_suite/project_wake_human.rs`）、task-dispatch の `project_wake_human_races_pause`（D6 で呼び出しを移す） |
| `cos-audit-same-tx` | CoS の `knowledge.record`・`project.*` の書き込み（pin・案件の変更）と operation の行・監査 event が同じ transaction に入る。失敗は rollback と rejected の行 | `cos_live_fix_d2_cos_operation_records_audit_row`、`cos_ops_projects_cron_project_lifecycle_is_audited`、D6 の契約試験 2・3・4 |
| `task-state-in-sqlite` | pin と provenance の読み書きが `SqliteStore` の接続（書き込みの Mutex・読み取りの接続）だけを通る。task-api が SQLite の file を自分で開かない | `cos_live_fix_d2_api_creates_candidate_with_provenance`、`cos_chat_attach_handoff_kb_candidate_keeps_provenance`（一覧と詳細の provenance）、計測の 3 file の `rusqlite` の出現数と `git grep -n 'Connection::open' crates/task-api/src/knowledge.rs` が 0 |
| `no-data-loss` | pin・監査の失敗で消すのは今書いた候補の file だけ。添付の行・blob・events は消さない。git の履歴は残る | D6 の契約試験 1・2（ref の行が 0、`chat_attachments` の行が残る、候補の file が残らない）、`cos_live_fix_d2_invalid_scope_is_422_and_writes_nothing` |
| `api-principal-from-token` | HTTP の `require_admin`、CoS の直接の route の 422 `cos_audit_context_required` を変えない | `cos_live_fix_d2_cos_operation_records_audit_row`（直接の route）、`project_wake_api_human_operator_is_not_counted`・`_mcp_is_not_counted` |
| `no-unwrap-outside-tests`・`events-append-only` | 新しい code に `unwrap()` を置かない。events は既存の `*_tx`・`cos_operation_apply` が追記するだけ | `cargo clippy --workspace -- -D warnings` と review |

`transaction` class の 2 file（`knowledge.rs`・task-ops の `lifecycle.rs`）を変える unit は、この ADR を設計 brief として参照する。

## 7. 計測

before の値と再現 command は [arch-v4-f/adr.md](../progress/2026-11-18-arch-v4-f/adr.md)（基準は `git merge-base HEAD main`。A の baseline.md §6・§7 の
規則と command をそのまま当てた）。要約:

| 指標 | before |
|---|---|
| C2 循環の件数、単位・組・行の辺 | 17 件、198・756・1,815。task-api の SCC は 49・261・620 |
| C2 handlers → cos、cos → handlers | 19 本・12 file、50 本・9 file |
| C2 SCC の中の `knowledge`・`lifecycle` の単位の辺 | knowledge は出 10・入 18、lifecycle は出 5・入 9 |
| C4 `rusqlite` を書く production file、SQL 文字列の file | 合計 75・50（task-api 17・4、task-ops 8・3） |
| C5 `crate::cos::` を書く handlers の行、`handlers` を書く cos の行 | 19 行・50 行 |
| C6 `&Connection`・`&Transaction` の引数の行 | 347（path だけに除外を当て直すと 336） |
| C6 `Fn(&Connection)`・`Fn(&Transaction)` の引数の行 | 22（task-api 4、task-core 13、task-ops 5。task-ops の 1 つが `lifecycle.rs:110`） |
| C6 `impl SqliteStore` の block、pub method | block 102、`pub` 631、`pub(crate..)` 67、private 41 |
| 3 file の `rusqlite` の出現数 | `knowledge.rs` 8、task-ops の `lifecycle.rs` 2、task-api の `lifecycle.rs` 0 |
| `project_controller_human_bump_tx` の呼び手 file | 定義 1（`store/project_wake.rs`）、production 3（`human_epoch.rs`・`handlers/authority.rs`・`store/authority.rs`）、試験 5 |
| D の付記の 2 本の command（狭い形・広い形） | 14 file・18 file |
| source-size-report（production 全言語・production Rust・own_total_with_tests） | 375,818・331,127・1,030,359 行 |

after の見込み（約束ではない。F の measure が同じ command で測り、違えば理由を書く）:

- 3 file の `rusqlite`: `knowledge.rs` 8 → 1（`pin_candidate_tx` の `&rusqlite::Transaction<'_>` だけ）、task-ops の `lifecycle.rs` 2 → 1
  （`apply_project_change_tx` の `tx`）、task-api の `lifecycle.rs` 0 のまま。
- C6 の `Fn(&Connection)` は task-ops で 5 → 4。`&Connection`・`&Transaction` の引数は変わらない（`apply_project_change_tx` は型が変わるだけ）。
  pub method は D2 で 631 → 632、block は 102 → 103。
- C4 の file 数・C5・D の付記の 2 本の command の file 数・`project_controller_human_bump_tx` の呼び手 file は変わらない
  （`knowledge.rs`・task-ops の `lifecycle.rs` は `rusqlite` の型を書き続け、他の `*_op` が残る）。
- C2 の task-api の SCC の単位と組は変わらない見込み。行の辺は CoS の腕の参照が変わる分だけ増減しうる。
- source-size-report の production は、自前の接続・`record_op` の 2 分岐・`project_action_op`・`apply_project_action` が消え、
  `record_candidate`・`discard_candidate`・`attachment_provenance`・`project_action_result` と 2 つの CoS の腕が増える。純減は目標にしない。

## 後続 WorkUnit への引き継ぎ（推奨。F の計画が決める）

- 契約試験（D6 の 1〜4）を旧コードで固定する unit を、実装の unit の前に置く。
- knowledge の unit（D1〜D3、cos-live-fixes・cos-chat-home の付記）と lifecycle の unit（D4、project-controller の付記、試験の呼び出しの移し替え）。
  `cos/ops/decisions.rs` と `cos/ops/projects.rs` は別の file なので、2 つの unit は並列にできる。task-dispatch の試験の file を変えるので、
  lifecycle の unit の test-affected は task-dispatch を含む。
- measure は §7 の command を after に当て、close は付記で実装との違いを書く。
