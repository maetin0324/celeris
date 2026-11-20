//! アーキv4 H R4（WorkUnit attach。ADR 2026-11-19-chat-attachment-store-connection D1・§5）の反例試験。chat 添付の
//! route の 3 つの読み取り（`representation` の `expires_at`・`upload` の replay・`add_reference` の `request_hash`）は、
//! route が要求ごとに開いた自前の `rusqlite::Connection::open` から、task-core の `attachment_expires_at`・
//! `upload_replayed`・`attachment_ref_request_hash`（`SqliteStore::with_read_conn`）へ移った。ここでは移した後の
//! 読み取りが commit 済みの行だけを返すことと、busy の error の status・code が移す前と同じこと（500
//! `attachment_store_error`）を確かめる。合否は commit の有無と lock の有無だけで決まり、待ち時間に依らない（API の
//! busy_timeout は、lock を持つ間の要求を早く諦めさせるためだけに短くする）。
//!
//! 既存の試験（`chat_attachments.rs` の `chat_attach_api_*`、`tx_boundary_contract.rs` の K1〜K4）は、どれも読み取りを
//! commit 済みの行だけがある DB に当て、busy を起こすのは知識の record の書き込みだけなので、次の 3 つの崩れを検出できない:
//! 読み取りが失敗した transaction の行を返す、読み取りが他の接続の commit 前の行を返す、読み取りの busy の code が
//! 共通の写像（503 `db_busy`）に変わる。
use crate::common;

use std::time::Duration;

use common::*;
use serde_json::json;
use task_core::chat::attachments::{
    ChatAttachmentLimits, attachment_expires_at, attachment_ref_request_hash, upload_replayed,
};
use task_core::store::StoreError;
use task_core::{Status, TaskKind};

use crate::chat_attachments::upload;

const BASE: &str = "/api/v1/chat";
/// lock を持つ間の要求が早く諦めるための API の busy_timeout（合否は lock の有無で決まり、この値に依らない）。
const SHORT_BUSY: Duration = Duration::from_millis(300);
const STAMP: &str = "2026-11-19T00:00:00Z";

fn admin_options() -> EnvOptions {
    EnvOptions {
        token: Some(TOKEN.to_string()),
        ..EnvOptions::default()
    }
}

/// 添付の保存を有効にし、API の state を短い busy_timeout で作り直した env と、その thread の id。
async fn attach_env() -> (TestEnv, String) {
    let mut env = TestEnv::with(admin_options());
    let mut settings = settings(
        &env.db_path,
        &env.workspace_root,
        &env.docs_repo_root,
        &env.knowledge_root,
        admin_options(),
    );
    settings.busy_timeout = SHORT_BUSY;
    let (daemon_tx, daemon_rx) = tokio::sync::watch::channel(None);
    env.state = task_api::ApiState::new(settings, daemon_rx)
        .expect("api state")
        .with_chat_attachments(
            env.dir.path().to_path_buf(),
            ChatAttachmentLimits::default(),
        );
    env.daemon_tx = daemon_tx;
    let created = send(
        &env.router(),
        post_admin(&format!("{BASE}/threads"), &json!({"project_id": null})),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let thread = created.json()["thread"]["id"]
        .as_str()
        .expect("thread id")
        .to_owned();
    (env, thread)
}

/// 1 件の添付を上げ、その id と（pin の前の）`expires_at` を返す。
async fn uploaded(env: &TestEnv, thread: &str) -> (String, String) {
    let resp = upload(&env.router(), thread, "up-1", "first.txt", b"first").await;
    assert_eq!(resp.status.as_u16(), 201, "{}", resp.text());
    let attachment = &resp.json()["attachment"];
    (
        attachment["id"].as_str().expect("id").to_owned(),
        attachment["expires_at"]
            .as_str()
            .expect("an unpinned attachment expires")
            .to_owned(),
    )
}

fn seed_task(env: &TestEnv) -> String {
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    task.id.to_string()
}

/// 3 つの読み取りが返さないはずの行（upload の key `up-2`、attachment_ref の key `ref-1`、`expires_at` を消す）を
/// 渡された接続（transaction の中）に書く。
fn write_unseen_rows(conn: &rusqlite::Connection, thread: &str, attachment: &str) {
    conn.execute(
        "INSERT INTO chat_client_requests(kind,scope_id,key,request_hash,result_id,created_at) \
         VALUES('upload',?1,'up-2','unseen-hash','01UNSEENUPLOAD0000000000000',?2)",
        [thread, STAMP],
    )
    .expect("upload request row");
    conn.execute(
        "INSERT INTO chat_client_requests(kind,scope_id,key,request_hash,result_id,created_at) \
         VALUES('attachment_ref',?1,'ref-1','task:unseen-owner','unseen-owner',?2)",
        [attachment, STAMP],
    )
    .expect("attachment_ref request row");
    conn.execute(
        "UPDATE chat_attachments SET expires_at=NULL WHERE id=?1",
        [attachment],
    )
    .expect("clear expires_at");
}

/// 添付の要求の行と pin の行の数（`chat_client_requests` の添付の 2 種、`chat_attachment_refs`）。
fn request_and_ref_rows(env: &TestEnv) -> (i64, i64) {
    let conn = rusqlite::Connection::open(&env.db_path).expect("open db");
    let requests = conn
        .query_row(
            "SELECT COUNT(*) FROM chat_client_requests WHERE kind IN ('upload','attachment_ref')",
            [],
            |r| r.get(0),
        )
        .expect("count requests");
    let refs = conn
        .query_row("SELECT COUNT(*) FROM chat_attachment_refs", [], |r| {
            r.get(0)
        })
        .expect("count refs");
    (requests, refs)
}

/// 守る不変条件（`task-state-in-sqlite`）: 3 つの読み取りは失敗した transaction の行を返さない。`write_tx` の closure が
/// upload と attachment_ref の要求の行を入れ、`expires_at` を消してから `Err` を返した後、`upload_replayed` は偽、
/// `attachment_ref_request_hash` は無し、`attachment_expires_at` は元の値を返す。API の store を通る HTTP でも同じで、
/// 同じ key の upload は 201（replay でない）、同じ key の別の owner の pin は 409 でなく 200、metadata の `expires_at` は
/// 元の値になる。pin の commit の後は読み取りが新しい値（hash・`expires_at` 無し・replay の 200）を返す。
/// 既存の試験で検出できない理由: 既存の添付の試験は rollback を起こさず、commit 済みの行しか無い DB で読み取りを見る。
/// 読み取りが store の書き込み中の transaction の中身を見る形（同じ接続の未 commit の行を読むなど）に変わっても落ちない。
#[tokio::test]
async fn attach_conn_rollback_leaves_no_request_row() {
    let (env, thread) = attach_env().await;
    let app = env.router();
    let (id, expires) = uploaded(&env, &thread).await;
    let owner = seed_task(&env);
    assert_eq!(
        attachment_expires_at(&env.store, &id).expect("expires_at"),
        Some(expires.clone())
    );

    let rolled: Result<(), StoreError> = env.store.write_tx(|tx| {
        write_unseen_rows(tx, &thread, &id);
        Err(StoreError::Invalid("attach_conn rollback".to_owned()))
    });
    assert!(matches!(rolled, Err(StoreError::Invalid(_))), "{rolled:?}");

    assert!(!upload_replayed(&env.store, &thread, "up-2").expect("replay"));
    assert_eq!(
        attachment_ref_request_hash(&env.store, &id, "ref-1").expect("hash"),
        None
    );
    assert_eq!(
        attachment_expires_at(&env.store, &id).expect("expires_at"),
        Some(expires.clone())
    );
    let meta = send(&app, get_admin(&format!("{BASE}/attachments/{id}"))).await;
    assert_eq!(meta.status.as_u16(), 200, "{}", meta.text());
    assert_eq!(meta.json()["attachment"]["expires_at"], json!(expires));
    let second = upload(&app, &thread, "up-2", "second.txt", b"second").await;
    assert_eq!(second.status.as_u16(), 201, "{}", second.text());
    let pin = send(
        &app,
        post_admin(
            &format!("{BASE}/attachments/{id}/references"),
            &json!({"owner_kind": "task", "owner_id": owner, "idempotency_key": "ref-1"}),
        ),
    )
    .await;
    assert_eq!(pin.status.as_u16(), 200, "{}", pin.text());

    assert_eq!(
        attachment_ref_request_hash(&env.store, &id, "ref-1").expect("hash"),
        Some(format!("task:{owner}"))
    );
    assert_eq!(
        attachment_expires_at(&env.store, &id).expect("expires_at"),
        None
    );
    assert!(upload_replayed(&env.store, &thread, "up-2").expect("replay"));
    let replay = upload(&app, &thread, "up-2", "second.txt", b"second").await;
    assert_eq!(replay.status.as_u16(), 200, "{}", replay.text());
}

/// 守る不変条件（`task-state-in-sqlite`、ADR-0064 D4）: 別の接続が `BEGIN IMMEDIATE` で書き込みの lock を持ち、
/// 要求の行と `expires_at` の変更を commit していない間も、3 つの読み取りは止まらずに commit 済みの値（replay は偽、
/// hash は無し、`expires_at` は元の値）を返し、どれも store の読み取りの接続（`lock_counts().reader`）を通る。API の
/// store を通る metadata も 200 で元の `expires_at` を返す。commit の後は新しい値を返す。
/// 既存の試験で検出できない理由: 既存の添付の試験は他の接続の書き込みの lock を持たない。読み取りが書き込みの接続
/// （`lock_counts().writer`）や書き込みの lock を待つ経路に変わっても、commit 前の行が見える形に変わっても落ちない。
#[tokio::test]
async fn attach_conn_reads_committed_rows_only() {
    let (env, thread) = attach_env().await;
    let app = env.router();
    let (id, expires) = uploaded(&env, &thread).await;

    let holder = rusqlite::Connection::open(&env.db_path).expect("holder");
    holder.execute_batch("BEGIN IMMEDIATE").expect("write lock");
    write_unseen_rows(&holder, &thread, &id);

    let before = env.store.lock_counts();
    assert!(!upload_replayed(&env.store, &thread, "up-2").expect("replay"));
    assert_eq!(
        attachment_ref_request_hash(&env.store, &id, "ref-1").expect("hash"),
        None
    );
    assert_eq!(
        attachment_expires_at(&env.store, &id).expect("expires_at"),
        Some(expires.clone())
    );
    let after = env.store.lock_counts();
    assert_eq!(after.reader, before.reader + 3, "{before:?} -> {after:?}");
    assert_eq!(after.writer, before.writer, "{before:?} -> {after:?}");
    let meta = send(&app, get_admin(&format!("{BASE}/attachments/{id}"))).await;
    assert_eq!(meta.status.as_u16(), 200, "{}", meta.text());
    assert_eq!(meta.json()["attachment"]["expires_at"], json!(expires));

    holder.execute_batch("COMMIT").expect("commit");
    assert!(upload_replayed(&env.store, &thread, "up-2").expect("replay"));
    assert_eq!(
        attachment_ref_request_hash(&env.store, &id, "ref-1").expect("hash"),
        Some("task:unseen-owner".to_owned())
    );
    assert_eq!(
        attachment_expires_at(&env.store, &id).expect("expires_at"),
        None
    );
}

/// 守る不変条件（API の契約・`no-data-loss`）: 別の接続が `BEGIN IMMEDIATE` で書き込みの lock を持ち、同じ添付と key に
/// 別の owner の `attachment_ref` の要求の行を commit せずに入れている間の pin（`POST /chat/attachments/{a}/references`）は、
/// `request_hash` の読み取りが commit 済みの値（無し）を返すので 409 `chat_conflict` にならず、書き込みの段
/// （`ChatAttachmentStore` の接続の `BEGIN IMMEDIATE`）の busy を移す前と同じ 500 `attachment_store_error` の problem に写す
/// （panic なら 500 `internal`、共通の `StoreError` の写像なら 503 `db_busy` になる）。lock を外した後、要求の行も pin の行も
/// 増えていない。同じ要求の再試行は 200 で、要求の行と pin の行が 1 行ずつ増える。
/// WAL の読み取りは他の接続の lock で busy にならない（`PRAGMA locking_mode=EXCLUSIVE` の `BEGIN EXCLUSIVE` も、他の接続が
/// 一度でも読んだ DB では取れない）ので、busy は書き込みの段で起こす。待ちは `ChatAttachmentStore` の固定の
/// busy_timeout（10 秒）だけで、合否は lock を持つ間か外した後かで決まる。
/// 既存の試験で検出できない理由: 既存の添付の試験は他の接続の lock を持たない。読み取りが commit 前の行を見る変更（409 に
/// なる）、busy で panic する変更、busy を別の code に写す変更のどれでも落ちない。
#[tokio::test]
async fn attach_conn_busy_keeps_attachment_store_error() {
    let (env, thread) = attach_env().await;
    let app = env.router();
    let (id, _) = uploaded(&env, &thread).await;
    let owner = seed_task(&env);
    let pin_request = || {
        post_admin(
            &format!("{BASE}/attachments/{id}/references"),
            &json!({"owner_kind": "task", "owner_id": owner, "idempotency_key": "ref-1"}),
        )
    };
    let rows_before = request_and_ref_rows(&env);

    let holder = rusqlite::Connection::open(&env.db_path).expect("holder");
    holder.execute_batch("BEGIN IMMEDIATE").expect("write lock");
    write_unseen_rows(&holder, &thread, &id);
    let busy = send(&app, pin_request()).await;
    assert_problem(&busy, 500, "attachment_store_error");
    holder.execute_batch("ROLLBACK").expect("unlock");

    assert_eq!(request_and_ref_rows(&env), rows_before);
    let pinned = send(&app, pin_request()).await;
    assert_eq!(pinned.status.as_u16(), 200, "{}", pinned.text());
    assert_eq!(
        request_and_ref_rows(&env),
        (rows_before.0 + 1, rows_before.1 + 1)
    );
}
