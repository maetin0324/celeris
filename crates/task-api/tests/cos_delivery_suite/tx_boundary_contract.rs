//! アーキv4 F（contract-tests）: 知識の候補の記録（`POST /knowledge/inbox` と CoS の `knowledge.record`。添付の
//! pin）と案件の中止（`POST /projects/{id}/cancel` と CoS の `project.cancel`）の transaction 境界の契約を、
//! refactor（`knowledge.rs` の自前の SQLite 接続と `lifecycle.rs` の Connection を渡す closure を、既存の
//! `SqliteStore::write_tx`・読み取り接続へ寄せる）の前のコードに当てて固定する。書き込みは公開の経路（HTTP と
//! `/api/v1/cos/operations`）だけで行い、結果は応答と DB の行（rusqlite で読むだけ）と `_inbox/` の file で確かめる。
//! 内部の関数は呼ばない（refactor の後もこの file を変えずに通すため）。tx の途中の失敗は試験が置く SQLite の
//! trigger（`contract_fault`）で、busy は試験の別接続が持つ `BEGIN IMMEDIATE` の lock で決定的に起こす。busy の
//! 合否は lock を持つ間か外した後かだけで決まり、待ち時間には依らない（API の busy_timeout は短くして待ちを縮める
//! だけ）。
//!
//! 試験ごとに守る不変条件と、既存の試験（cos_live_fix_d2.rs・cos_chat_attach_pin.rs・lifecycle.rs・
//! cos_delivery_suite/cos_ops_projects_cron.rs・routing_budget_suite/project_wake_human.rs）で検出できない理由:
//!
//! (K1) `admin_record_pins_attachments_and_serves_provenance`
//!      admin の HTTP の record（添付 2 件。1 件は chat の発言に載せたもの）が 201 と応答の欄（id・path・target・scope・
//!      attachment_ids・sha）を返し、`chat_attachment_refs` に owner_kind `knowledge_inbox` の行を添付ごとに 1 行書き、
//!      添付の `expires_at` を消す（pin した添付は掃除されない）。`GET /knowledge/inbox/{id}` と一覧の provenance は
//!      添付ごとに name・media_type・size_bytes・sha256（中身の sha256）・thread_id・pinned_at（参照行の created_at）・
//!      発言の message_id と request_text を返す。
//!      既存で検出できない理由: `cos_live_fix_d2_api_creates_candidate_with_provenance` は発言に載せない添付 1 件で
//!      provenance の attachment_id・thread_id・name だけを見る。参照行・`expires_at`・発言の結合（message_id・
//!      request_text）・sha256・pinned_at を見ないので、provenance の読み取りを別の接続に移す変更や、pin を別の
//!      書き込みの経路に移す変更でそれらが崩れても落ちない。
//!
//! (K2) `invalid_attachment_record_leaves_no_candidate_and_no_reference`
//!      正しい添付と不正な添付（存在しない id・同じ id の重複・ULID でない id）を並べた HTTP の record は 422
//!      `invalid_attachment`（problem の type は `urn:celeris:problem:invalid_attachment`）で、`_inbox/` に候補を残さず、
//!      正しい添付にも参照行を書かず、その `expires_at` も変えない。
//!      既存で検出できない理由: `cos_live_fix_d2_invalid_scope_is_422_and_writes_nothing` は存在しない id 1 件だけを送り、
//!      `_inbox/` の file だけを見る。並べた正しい添付を先に pin してから残りを検査する変更（参照行と `expires_at` が
//!      残る）を検出できない。
//!
//! (K3) `cos_record_writes_audit_and_reference_in_one_transaction`
//!      CoS の `knowledge.record` の成功で、cos_operations の applied の行・監査 event・参照行が揃う。applied の行の
//!      挿入が落ちると参照行も `expires_at` の変更も残らず、候補は `_inbox/` から消え、rejected の行だけが残る（= pin と
//!      監査が同じ transaction）。他の thread の添付の pin は 422 `invalid_attachment` で rejected の行が残り、その添付の
//!      参照行も候補も残らない。
//!      既存で検出できない理由: `cos_live_fix_d2_cos_operation_records_audit_row` は成功の applied の行の数と、他の
//!      thread の添付の 422・rejected の行・`_inbox/` の file を見るが、参照行を見ず、監査の行を書く段の失敗を起こさない。
//!      pin を監査と別の transaction で先に commit する変更はどの既存の試験でも落ちない。
//!
//! (K4) `busy_store_fails_record_without_residue_and_retry_succeeds`
//!      試験の別接続が `BEGIN IMMEDIATE` で書き込みの lock を持つ間の HTTP の record（添付つき）は 5xx の problem を返し、
//!      候補・参照行・`expires_at` の変更を残さない。lock を外した後の同じ要求は 201 で、参照行が 1 行入る。
//!      既存で検出できない理由: record の busy を起こす試験が無い。pin の書き込みが失敗したときに git に書いた候補を
//!      消す経路（`inbox_reject`）は、不正な添付の 422 でしか通っていない。
//!
//! (L1) `http_project_cancel_response_and_transition_events`
//!      HTTP の project cancel の応答の欄がちょうど `project`・`cancelled_tasks`・`cancelled_milestones` で、
//!      `cancelled_tasks` は非終端だった task（ready・running）だけを TaskRef の形（id・title・kind・status・actions、
//!      status は cancelled）で返し、`cancelled_milestones` は非終端の途中目標だけ。event は cancel した task ごとに
//!      ちょうど 1 件（`Transitioned`、to cancelled、reason `project_cancelled`）増え、終端の task と他の行には増えない。
//!      既存で検出できない理由: `cancelling_a_project_cascades_to_its_tasks_and_milestones` は `contains` と最後の
//!      遷移の reason だけを見る。遷移 event の重複・余計な event・応答の余計な欄や TaskRef の形の崩れを検出できない。
//!      応答を HTTP の adapter で組み直す変更で崩れうる。
//!
//! (L2) `busy_store_fails_project_cancel_without_change_and_retry_succeeds`
//!      別接続が lock を持つ間の HTTP の project cancel（人の token、controller が shadow の案件）は 503 `db_busy` で、
//!      task の状態・event の数・`human_epoch`・案件と途中目標の状態を変えない。lock を外した後の再試行は 200 で、
//!      `human_epoch` は 1 回だけ増える。
//!      既存で検出できない理由: `project_wake_api_human_write_failure_keeps_the_epoch` と
//!      `project_wake_api_human_bump_failure_rolls_the_write_back` は trigger の RAISE(ABORT) による tx の中の失敗だけを
//!      起こす。transaction を始められない busy と、その error の status・code、lock を外した後の再試行を見ない。
//!
//! (L3) `cos_project_cancel_writes_audit_and_cascade_in_one_transaction`
//!      CoS の `project.cancel` の applied の行の挿入が落ちると、task・途中目標・案件の状態も event の数も変わらず、
//!      rejected の行だけが残る（= cascade と監査が同じ transaction）。別の key の再送は applied になり、result の
//!      `cancelled_tasks` は非終端だった task だけ、監査 event が 1 件残る。既に cancelled の案件では 409
//!      `invalid_transition` と rejected の行が残り、何も変わらない。
//!      既存で検出できない理由: `cos_ops_projects_cron_project_lifecycle_is_audited` は成功と 409 の rejected の行を見るが、
//!      監査の行を書く段の失敗を起こさない。cascade を監査と別の transaction で先に commit する変更を検出できない。
//!
//! (L4) `non_admin_record_and_project_cancel_are_refused_without_writes`
//!      MCP の token（読むだけの主体）の HTTP の record と project cancel は 403 `forbidden` で、候補・参照行・task・
//!      event・案件を変えない。token を設定しない構成（guard が全てを通す）でも、bearer の無い record と project cancel は
//!      handler の `require_admin` で 401 `unauthorized` になり、何も書かない。
//!      既存で検出できない理由: record の経路の 403・401 を見る試験が無い。project cancel の MCP の 403 は
//!      `project_wake_api_human_mcp_is_not_counted` が status と `human_epoch` だけを見て、task・event・案件が変わらない
//!      ことを見ない。handler を書き直す変更で `require_admin` が書き込みの後ろに回っても、既存の試験は落ちない。
//!
//! 重ねて書かないもの（既存の試験が確かめる）: trigger の RAISE(ABORT) による HTTP の project の書き込みと
//! `human_epoch` の rollback、`human_epoch` の加算が 1 要求 1 回であること、CoS の経路を数えないことは
//! routing_budget_suite/project_wake_human.rs の `project_wake_api_human_write_failure_keeps_the_epoch`・
//! `project_wake_api_human_bump_failure_rolls_the_write_back`・`project_wake_api_human_cos_is_not_counted`・
//! `project_wake_api_human_each_route_bumps_once`。
use crate::common;

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use common::cos_ops::{OPS, audit_events, cos_bearer, cos_run_bearer, db, op_body};
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::chat::attachments::ChatAttachmentLimits;
use task_core::chat::{ChatPostMessageRequest, ChatSendMode};
use task_core::{MilestoneStatus, ProjectId, ProjectStatus, Status, TaskId, TaskKind};

const PDF: &[u8] = b"%PDF-1.4 tx boundary contract";
const MCP_TOKEN: &str = "tx-boundary-mcp-token";
const MCP_CLIENT: &str = "01MCPCLIENTA";
/// lock を持つ間の要求が早く諦めるための API の busy_timeout（合否は lock の有無で決まり、この値に依らない）。
const SHORT_BUSY: Duration = Duration::from_millis(300);
pub(crate) const INBOX: &str = "/api/v1/knowledge/inbox";
/// task の行に属する event の数（CoS の監査 event は数えない）。
const TASK_EVENTS: &str = "SELECT COUNT(*) FROM events WHERE task_id IN (SELECT id FROM tasks)";

fn admin_options() -> EnvOptions {
    EnvOptions {
        token: Some(TOKEN.to_string()),
        ..EnvOptions::default()
    }
}

fn mcp_options() -> EnvOptions {
    EnvOptions {
        token: Some(TOKEN.to_string()),
        mcp_token: Some(MCP_TOKEN.to_string()),
        ..EnvOptions::default()
    }
}

/// chat 添付と初期化した KB を持つ env。`busy` を渡すと API の state をその busy_timeout で作り直す。
fn contract_env(options: fn() -> EnvOptions, busy: Option<Duration>) -> TestEnv {
    let mut env = TestEnv::with(options());
    if let Some(busy) = busy {
        let mut settings = settings(
            &env.db_path,
            &env.workspace_root,
            &env.docs_repo_root,
            &env.knowledge_root,
            options(),
        );
        settings.busy_timeout = busy;
        let (daemon_tx, daemon_rx) = tokio::sync::watch::channel(None);
        env.state = task_api::ApiState::new(settings, daemon_rx).expect("api state");
        env.daemon_tx = daemon_tx;
    }
    env.state = env.state.clone().with_chat_attachments(
        env.dir.path().to_path_buf(),
        ChatAttachmentLimits::default(),
    );
    task_ops::knowledge::init(&env.knowledge_root).expect("knowledge init");
    env
}

fn admin() -> String {
    format!("Bearer {TOKEN}")
}

pub(crate) async fn upload(app: &axum::Router, thread: &str, upload_id: &str) -> String {
    let mut body = Vec::new();
    body.extend_from_slice(b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"manual.pdf\"\r\nContent-Type: application/pdf\r\n\r\n");
    body.extend_from_slice(PDF);
    body.extend_from_slice(
        format!("\r\n--boundary\r\nContent-Disposition: form-data; name=\"client_upload_id\"\r\n\r\n{upload_id}\r\n--boundary--\r\n")
            .as_bytes(),
    );
    let request = Request::post(format!("/api/v1/chat/threads/{thread}/attachments"))
        .header("host", HOST)
        .header("authorization", admin())
        .header("content-type", "multipart/form-data; boundary=boundary")
        .body(Body::from(body))
        .expect("multipart");
    let uploaded = send(app, request).await;
    assert!(uploaded.status.is_success(), "{}", uploaded.text());
    uploaded.json()["attachment"]["id"]
        .as_str()
        .expect("attachment id")
        .to_owned()
}

pub(crate) fn record_body(attachment_ids: &[&str]) -> Value {
    json!({
        "title": "fern03 の使い方",
        "scope": "environment",
        "body": "添付の PDF の要点。",
        "sources": ["message:01J00000000000000000000000"],
        "path": "environment/servers/fern03.md",
        "attachment_ids": attachment_ids,
    })
}

pub(crate) fn inbox_files(env: &TestEnv) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(env.knowledge_root.join("_inbox"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name != ".gitkeep")
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// `knowledge_inbox` の参照行 `(attachment_id, owner_id, created_at)`（attachment_id 順）。
pub(crate) fn inbox_refs(env: &TestEnv) -> Vec<(String, String, String)> {
    let conn = db(env);
    let mut stmt = conn
        .prepare(
            "SELECT attachment_id, owner_id, created_at FROM chat_attachment_refs \
             WHERE owner_kind='knowledge_inbox' ORDER BY attachment_id, owner_id",
        )
        .expect("prepare");
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

pub(crate) fn expires_at(env: &TestEnv, attachment: &str) -> Option<String> {
    db(env)
        .query_row(
            "SELECT expires_at FROM chat_attachments WHERE id=?1",
            [attachment],
            |r| r.get(0),
        )
        .expect("attachment row")
}

/// その idempotency key の cos_operations の state（古い順）。
fn op_states(env: &TestEnv, key: &str) -> Vec<String> {
    let conn = db(env);
    let mut stmt = conn
        .prepare(
            "SELECT state FROM cos_operations WHERE idempotency_key=?1 ORDER BY created_at, id",
        )
        .expect("prepare");
    stmt.query_map([key], |r| r.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

/// applied の cos_operations の行の挿入を落とす trigger（rejected の行は通す）。
fn fail_applied_operation_row(env: &TestEnv) {
    db(env)
        .execute_batch(
            "CREATE TRIGGER contract_fault BEFORE INSERT ON cos_operations WHEN NEW.state='applied' \
             BEGIN SELECT RAISE(ABORT, 'contract_fault'); END;",
        )
        .expect("trigger");
}

fn drop_fault(env: &TestEnv) {
    db(env)
        .execute_batch("DROP TRIGGER contract_fault")
        .expect("drop trigger");
}

/// 試験の別接続で書き込みの lock（`BEGIN IMMEDIATE`）を取る。[`release`] で外す。
fn hold_write_lock(env: &TestEnv) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(&env.db_path).expect("open");
    conn.execute_batch("BEGIN IMMEDIATE").expect("lock");
    conn
}

fn release(conn: rusqlite::Connection) {
    conn.execute_batch("ROLLBACK").expect("unlock");
}

#[track_caller]
fn assert_server_problem(resp: &Resp) -> Value {
    assert!(
        resp.status.is_server_error(),
        "{}: {}",
        resp.status,
        resp.text()
    );
    assert_eq!(
        resp.header("content-type"),
        Some("application/problem+json")
    );
    resp.json()
}

fn seed_project(env: &TestEnv, title: &str) -> ProjectId {
    let now = time::OffsetDateTime::now_utc();
    let project = task_core::Project {
        id: ProjectId::new(),
        title: title.into(),
        request: "request".into(),
        status: ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        auto_advance: false,
        slug: None,
        created_at: now,
        updated_at: now,
    };
    env.store.project_create(&project).expect("project");
    project.id
}

fn seed_task(env: &TestEnv, project: ProjectId, status: Status) -> task_core::Task {
    let mut task = new_task(TaskKind::Execute, status);
    task.project_id = Some(project);
    if status == Status::Running {
        task.lease = Some(task_core::Lease {
            worker_run_id: "run-1".into(),
            expires_at: time::OffsetDateTime::now_utc() + time::Duration::seconds(60),
        });
    }
    env.seed(&task);
    task
}

fn milestone(env: &TestEnv, project: ProjectId) -> String {
    env.store
        .milestone_create(project, "途中目標", "", MilestoneStatus::Proposed)
        .expect("milestone")
        .id
        .to_string()
}

fn events_of(env: &TestEnv, task: TaskId) -> Vec<task_core::Event> {
    env.store
        .events_for(task)
        .expect("events")
        .into_iter()
        .map(|(_, e)| e)
        .collect()
}

/// 案件・途中目標・task の行と task の event の数（失敗した要求が何も変えていないことを比べる。CoS の監査
/// event も `events` 表に入るので、task の行に属する event だけを数える）。
fn domain_rows(env: &TestEnv) -> String {
    db(env)
        .query_row(
            "SELECT (SELECT json_group_array(json_array(id, status, paused_from, archived_at, updated_at)) \
             FROM (SELECT * FROM projects ORDER BY id)), \
             (SELECT json_group_array(json_array(id, status, updated_at)) FROM (SELECT * FROM milestones ORDER BY id)), \
             (SELECT json_group_array(json) FROM (SELECT json FROM tasks ORDER BY id)), \
             (SELECT COUNT(*) FROM events WHERE task_id IN (SELECT id FROM tasks))",
            [],
            |row| {
                Ok(format!(
                    "{}|{}|{}|{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?
                ))
            },
        )
        .expect("rows")
}

fn sorted_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().expect("object").keys().cloned().collect();
    keys.sort();
    keys
}

// ---------------------------------------------------------------------------
// 知識の候補の記録（添付の pin）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_record_pins_attachments_and_serves_provenance() {
    let env = contract_env(admin_options, None);
    let app = env.router();
    let thread = project_cos_thread(&env.store, "k1-human").id;
    let sent = upload(&app, &thread, "up-sent").await;
    let loose = upload(&app, &thread, "up-loose").await;
    let message = env
        .store
        .chat_message_post(
            &thread,
            &ChatPostMessageRequest {
                client_message_id: "k1-message".into(),
                text: "この PDF を KB に入れておいて".into(),
                attachment_ids: vec![sent.clone()],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
                discussion_mode: None,
            },
            time::OffsetDateTime::now_utc(),
        )
        .expect("message");
    let message_id = message.response.message.id.clone();
    assert!(
        expires_at(&env, &loose).is_some(),
        "an unpinned upload expires"
    );

    let created = send(&app, post_admin(INBOX, &record_body(&[&sent, &loose]))).await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let created = created.json();
    assert_eq!(
        sorted_keys(&created),
        ["attachment_ids", "id", "path", "scope", "sha", "target"],
        "{created}"
    );
    let id = created["id"].as_str().expect("id").to_owned();
    assert_eq!(created["path"], format!("_inbox/{id}.md"));
    assert_eq!(created["target"], "environment/servers/fern03.md");
    assert_eq!(created["scope"], "environment");
    assert_eq!(created["attachment_ids"], json!([sent, loose]));
    assert!(created["sha"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(inbox_files(&env), vec![format!("{id}.md")]);

    let refs = inbox_refs(&env);
    let mut want: Vec<&str> = vec![&sent, &loose];
    want.sort();
    assert_eq!(
        refs.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        want,
        "{refs:?}"
    );
    assert!(refs.iter().all(|r| r.1 == id), "{refs:?}");
    assert_eq!(expires_at(&env, &sent), None);
    assert_eq!(
        expires_at(&env, &loose),
        None,
        "a pinned attachment is kept"
    );

    let detail = send(&app, get_admin(&format!("{INBOX}/{id}"))).await;
    assert_eq!(detail.status.as_u16(), 200, "{}", detail.text());
    let detail = detail.json();
    let provenance = detail["provenance"].as_array().expect("provenance").clone();
    assert_eq!(provenance.len(), 2, "{provenance:?}");
    let sha = format!("{:x}", Sha256::digest(PDF));
    for (attachment, message) in [(&sent, Some(&message_id)), (&loose, None)] {
        let entry = provenance
            .iter()
            .find(|p| p["attachment_id"] == attachment.as_str())
            .unwrap_or_else(|| panic!("provenance of {attachment}: {provenance:?}"));
        let pinned_at = &refs.iter().find(|r| &r.0 == attachment).expect("ref row").2;
        assert_eq!(entry["name"], "manual.pdf");
        assert_eq!(entry["media_type"], "application/pdf");
        assert_eq!(entry["size_bytes"], PDF.len());
        assert_eq!(entry["sha256"], sha.as_str());
        assert_eq!(entry["thread_id"], thread.as_str());
        assert_eq!(entry["pinned_at"], pinned_at.as_str());
        match message {
            Some(message) => {
                assert_eq!(entry["message_id"], message.as_str(), "{entry}");
                assert_eq!(entry["request_text"], "この PDF を KB に入れておいて");
            }
            None => {
                assert!(entry.get("message_id").is_none(), "{entry}");
                assert!(entry.get("request_text").is_none(), "{entry}");
            }
        }
    }

    let list = send(&app, get_admin(INBOX)).await;
    assert_eq!(list.status.as_u16(), 200, "{}", list.text());
    let list = list.json();
    let item = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["id"] == id.as_str())
        .expect("listed")
        .clone();
    assert_eq!(item["provenance"], detail["provenance"]);
}

#[tokio::test]
async fn invalid_attachment_record_leaves_no_candidate_and_no_reference() {
    let env = contract_env(admin_options, None);
    let app = env.router();
    let thread = project_cos_thread(&env.store, "k2-human").id;
    let valid = upload(&app, &thread, "up-valid").await;
    let expiry = expires_at(&env, &valid);
    assert!(expiry.is_some());

    let unknown = "01J00000000000000000000000";
    for ids in [
        vec![valid.as_str(), unknown],
        vec![valid.as_str(), valid.as_str()],
        vec![valid.as_str(), "not-a-ulid"],
    ] {
        let resp = send(&app, post_admin(INBOX, &record_body(&ids))).await;
        assert_problem(&resp, 422, "invalid_attachment");
        assert!(
            inbox_files(&env).is_empty(),
            "{ids:?}: {:?}",
            inbox_files(&env)
        );
        assert!(
            inbox_refs(&env).is_empty(),
            "{ids:?}: {:?}",
            inbox_refs(&env)
        );
        assert_eq!(expires_at(&env, &valid), expiry, "{ids:?}");
    }
    let list = send(&app, get_admin(INBOX)).await;
    assert_eq!(list.json()["items"], json!([]));
}

#[tokio::test]
async fn cos_record_writes_audit_and_reference_in_one_transaction() {
    let env = contract_env(admin_options, None);
    let app = env.router();
    let thread = project_cos_thread(&env.store, "k3-cos").id;
    let attachment = upload(&app, &thread, "up-1").await;
    let expiry = expires_at(&env, &attachment);
    let bearer = cos_run_bearer(&env, &thread, "k3");
    let headers = [("authorization", bearer.as_str())];
    let envelope = |key: &str, ids: &[&str]| op_body(key, "POST", INBOX, record_body(ids));

    // applied の行を書く段で落ちると、pin も候補も残らず rejected の行だけが残る。
    fail_applied_operation_row(&env);
    let failed = send(
        &app,
        post_json_with(OPS, &envelope("k3-fault", &[&attachment]), &headers),
    )
    .await;
    assert!(!failed.status.is_success(), "{}", failed.text());
    drop_fault(&env);
    assert_eq!(op_states(&env, "k3-fault"), ["rejected"]);
    assert!(inbox_refs(&env).is_empty(), "{:?}", inbox_refs(&env));
    assert_eq!(expires_at(&env, &attachment), expiry);
    assert!(inbox_files(&env).is_empty(), "{:?}", inbox_files(&env));

    // 成功: applied の行・監査 event・参照行が揃う。
    let resp = send(
        &app,
        post_json_with(OPS, &envelope("k3-ok", &[&attachment]), &headers),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied", "{op}");
    assert_eq!(op["action"], "knowledge.record");
    let id = op["result"]["id"]
        .as_str()
        .expect("candidate id")
        .to_owned();
    assert_eq!(op["target_id"], id.as_str());
    assert_eq!(op_states(&env, "k3-ok"), ["applied"]);
    let audits = audit_events(&env, op["id"].as_str().expect("operation id"));
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(audits[0]["state"], "applied");
    let refs = inbox_refs(&env);
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(
        (refs[0].0.as_str(), refs[0].1.as_str()),
        (attachment.as_str(), id.as_str())
    );
    assert_eq!(expires_at(&env, &attachment), None);
    assert_eq!(inbox_files(&env), vec![format!("{id}.md")]);

    // 他の thread の添付: 422 と rejected の行。参照行も候補も増えない。
    let other_thread = project_cos_thread(&env.store, "k3-other").id;
    let foreign = upload(&app, &other_thread, "up-other").await;
    let foreign_expiry = expires_at(&env, &foreign);
    let refused = send(
        &app,
        post_json_with(OPS, &envelope("k3-foreign", &[&foreign]), &headers),
    )
    .await;
    assert_problem(&refused, 422, "invalid_attachment");
    assert_eq!(op_states(&env, "k3-foreign"), ["rejected"]);
    assert_eq!(inbox_refs(&env), refs);
    assert_eq!(expires_at(&env, &foreign), foreign_expiry);
    assert_eq!(inbox_files(&env), vec![format!("{id}.md")]);
}

#[tokio::test]
async fn busy_store_fails_record_without_residue_and_retry_succeeds() {
    let env = contract_env(admin_options, Some(SHORT_BUSY));
    let app = env.router();
    let thread = project_cos_thread(&env.store, "k4-human").id;
    let attachment = upload(&app, &thread, "up-1").await;
    let expiry = expires_at(&env, &attachment);
    let body = record_body(&[&attachment]);

    let lock = hold_write_lock(&env);
    let busy = send(&app, post_admin(INBOX, &body)).await;
    release(lock);
    assert_server_problem(&busy);
    assert!(inbox_files(&env).is_empty(), "{:?}", inbox_files(&env));
    assert!(inbox_refs(&env).is_empty(), "{:?}", inbox_refs(&env));
    assert_eq!(expires_at(&env, &attachment), expiry);

    let retried = send(&app, post_admin(INBOX, &body)).await;
    assert_eq!(retried.status.as_u16(), 201, "{}", retried.text());
    let id = retried.json()["id"].as_str().expect("id").to_owned();
    assert_eq!(inbox_files(&env), vec![format!("{id}.md")]);
    let refs = inbox_refs(&env);
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].1, id);
    assert_eq!(expires_at(&env, &attachment), None);
}

// ---------------------------------------------------------------------------
// 案件の中止
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_project_cancel_response_and_transition_events() {
    let env = contract_env(admin_options, None);
    let app = env.router();
    let project = seed_project(&env, "l1");
    let open_milestone = milestone(&env, project);
    let done_milestone = milestone(&env, project);
    env.store
        .write_tx(|tx| {
            task_core::SqliteStore::milestone_set_lifecycle_tx(
                tx,
                done_milestone.parse().expect("milestone id"),
                MilestoneStatus::Reached,
                None,
            )
        })
        .expect("achieved");
    let ready = seed_task(&env, project, Status::Ready);
    let running = seed_task(&env, project, Status::Running);
    let done = seed_task(&env, project, Status::Done);
    let other_project = seed_project(&env, "l1-other");
    let outside = seed_task(&env, other_project, Status::Ready);
    let before: Vec<usize> = [&ready, &running, &done, &outside]
        .iter()
        .map(|t| events_of(&env, t.id).len())
        .collect();
    let total_before = count(&env, TASK_EVENTS);

    let resp = send(
        &app,
        post_admin(&format!("/api/v1/projects/{project}/cancel"), &json!({})),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let body = resp.json();
    assert_eq!(
        sorted_keys(&body),
        ["cancelled_milestones", "cancelled_tasks", "project"],
        "{body}"
    );
    assert_eq!(body["project"]["id"], project.to_string());
    assert_eq!(body["project"]["status"], "cancelled");
    let mut cancelled: Vec<String> = body["cancelled_tasks"]
        .as_array()
        .expect("cancelled_tasks")
        .iter()
        .map(|t| {
            assert_eq!(
                sorted_keys(t),
                ["actions", "id", "kind", "status", "title"],
                "{t}"
            );
            assert_eq!(t["status"], "cancelled", "{t}");
            assert_eq!(t["kind"], "execute", "{t}");
            t["id"].as_str().expect("id").to_string()
        })
        .collect();
    cancelled.sort();
    let mut want = vec![ready.id.to_string(), running.id.to_string()];
    want.sort();
    assert_eq!(cancelled, want);
    assert_eq!(body["cancelled_milestones"], json!([open_milestone]));

    for (task, before) in [&ready, &running].into_iter().zip(&before) {
        let events = events_of(&env, task.id);
        assert_eq!(events.len(), before + 1, "{}: {events:?}", task.id);
        match events.last() {
            Some(task_core::Event::Transitioned { to, reason, .. }) => {
                assert_eq!(*to, Status::Cancelled);
                assert_eq!(reason, "project_cancelled");
            }
            other => panic!("{}: last event {other:?}", task.id),
        }
        assert_eq!(env.status_of(task.id), Status::Cancelled);
    }
    assert_eq!(events_of(&env, done.id).len(), before[2]);
    assert_eq!(events_of(&env, outside.id).len(), before[3]);
    assert_eq!(env.status_of(outside.id), Status::Ready);
    assert_eq!(count(&env, TASK_EVENTS), total_before + 2);
}

#[tokio::test]
async fn busy_store_fails_project_cancel_without_change_and_retry_succeeds() {
    let env = contract_env(admin_options, Some(SHORT_BUSY));
    let project = seed_project(&env, "l2");
    let app = task_api::router(
        env.state
            .clone()
            .with_project_controller(true, vec![project.to_string()]),
    );
    let shadow = send(
        &app,
        put_json_with(
            &format!("/api/v1/projects/{project}/controller"),
            &json!({"mode": "shadow", "expected_revision": 0, "reason": "人が試す"}),
            &admin_headers(),
        ),
    )
    .await;
    assert_eq!(shadow.status.as_u16(), 200, "{}", shadow.text());
    milestone(&env, project);
    let ready = seed_task(&env, project, Status::Ready);
    let epoch = || {
        env.store
            .project_controller_state(&project.to_string())
            .expect("state")
            .map_or(0, |state| state.human_epoch)
    };
    assert_eq!(epoch(), 0);
    let before = domain_rows(&env);
    let cancel = format!("/api/v1/projects/{project}/cancel");

    let lock = hold_write_lock(&env);
    let busy = send(&app, post_admin(&cancel, &json!({}))).await;
    release(lock);
    let problem = assert_server_problem(&busy);
    assert_eq!(busy.status.as_u16(), 503, "{problem}");
    assert_eq!(problem["code"], "db_busy", "{problem}");
    assert_eq!(domain_rows(&env), before);
    assert_eq!(epoch(), 0);
    assert_eq!(env.status_of(ready.id), Status::Ready);

    let retried = send(&app, post_admin(&cancel, &json!({}))).await;
    assert_eq!(retried.status.as_u16(), 200, "{}", retried.text());
    assert_eq!(retried.json()["project"]["status"], "cancelled");
    assert_eq!(env.status_of(ready.id), Status::Cancelled);
    assert_eq!(epoch(), 1);
}

#[tokio::test]
async fn cos_project_cancel_writes_audit_and_cascade_in_one_transaction() {
    let env = contract_env(admin_options, None);
    let app = env.router();
    let project = seed_project(&env, "l3");
    let open_milestone = milestone(&env, project);
    let ready = seed_task(&env, project, Status::Ready);
    let done = seed_task(&env, project, Status::Done);
    let (_, _, bearer) = cos_bearer(&env, "l3");
    let headers = [("authorization", bearer.as_str())];
    let cancel = format!("/api/v1/projects/{project}/cancel");
    let envelope = |key: &str| op_body(key, "POST", &cancel, json!({}));

    // applied の行を書く段で落ちると、cascade も残らず rejected の行だけが残る。
    let before = domain_rows(&env);
    fail_applied_operation_row(&env);
    let failed = send(&app, post_json_with(OPS, &envelope("l3-fault"), &headers)).await;
    assert!(!failed.status.is_success(), "{}", failed.text());
    drop_fault(&env);
    assert_eq!(op_states(&env, "l3-fault"), ["rejected"]);
    assert_eq!(domain_rows(&env), before);
    assert_eq!(env.status_of(ready.id), Status::Ready);

    // 成功: cascade と applied の行・監査 event。
    let ready_events = events_of(&env, ready.id).len();
    let resp = send(&app, post_json_with(OPS, &envelope("l3-ok"), &headers)).await;
    assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    let op = resp.json()["operation"].clone();
    assert_eq!(op["state"], "applied", "{op}");
    assert_eq!(op["action"], "project.cancel");
    assert_eq!(op["target_id"], project.to_string());
    assert_eq!(
        op["result"]["cancelled_tasks"],
        json!([ready.id.to_string()])
    );
    assert_eq!(op_states(&env, "l3-ok"), ["applied"]);
    let audits = audit_events(&env, op["id"].as_str().expect("operation id"));
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert_eq!(env.status_of(ready.id), Status::Cancelled);
    assert_eq!(events_of(&env, ready.id).len(), ready_events + 1);
    assert_eq!(env.status_of(done.id), Status::Done);
    let project_row = env
        .store
        .project_get(project)
        .expect("get")
        .expect("project");
    assert_eq!(project_row.status, ProjectStatus::Cancelled);
    let milestones = env.store.milestone_list(project).expect("milestones");
    assert!(
        milestones
            .iter()
            .any(|m| m.id.to_string() == open_milestone && m.status == MilestoneStatus::Cancelled),
        "{milestones:?}"
    );

    // 既に cancelled: 409 と rejected の行。何も変わらない。
    let after = domain_rows(&env);
    let again = send(&app, post_json_with(OPS, &envelope("l3-again"), &headers)).await;
    assert_problem(&again, 409, "invalid_transition");
    assert_eq!(op_states(&env, "l3-again"), ["rejected"]);
    assert_eq!(domain_rows(&env), after);
}

#[tokio::test]
async fn non_admin_record_and_project_cancel_are_refused_without_writes() {
    // MCP の token は読むだけ（guard が 403）。
    let env = contract_env(mcp_options, None);
    let app = env.router();
    let thread = project_cos_thread(&env.store, "l4-human").id;
    let attachment = upload(&app, &thread, "up-1").await;
    let expiry = expires_at(&env, &attachment);
    let project = seed_project(&env, "l4");
    let ready = seed_task(&env, project, Status::Ready);
    let before = domain_rows(&env);
    let mcp = format!("Bearer {MCP_TOKEN}");
    let headers = [
        ("authorization", mcp.as_str()),
        ("x-celeris-mcp-client", MCP_CLIENT),
    ];
    let cancel = format!("/api/v1/projects/{project}/cancel");
    let record = send(
        &app,
        post_json_with(INBOX, &record_body(&[&attachment]), &headers),
    )
    .await;
    assert_problem(&record, 403, "forbidden");
    let cancelled = send(&app, post_json_with(&cancel, &json!({}), &headers)).await;
    assert_problem(&cancelled, 403, "forbidden");
    assert!(inbox_files(&env).is_empty(), "{:?}", inbox_files(&env));
    assert!(inbox_refs(&env).is_empty());
    assert_eq!(expires_at(&env, &attachment), expiry);
    assert_eq!(domain_rows(&env), before);
    assert_eq!(env.status_of(ready.id), Status::Ready);

    // token を設定しない構成: guard は通すが handler の `require_admin` が 401 にする。
    let open = contract_env(EnvOptions::default, None);
    let app = open.router();
    let project = seed_project(&open, "l4-open");
    let ready = seed_task(&open, project, Status::Ready);
    let before = domain_rows(&open);
    let record = send(&app, post_json(INBOX, &record_body(&[]))).await;
    assert_problem(&record, 401, "unauthorized");
    let cancelled = send(
        &app,
        post_json(&format!("/api/v1/projects/{project}/cancel"), &json!({})),
    )
    .await;
    assert_problem(&cancelled, 401, "unauthorized");
    assert!(inbox_files(&open).is_empty(), "{:?}", inbox_files(&open));
    assert_eq!(domain_rows(&open), before);
    assert_eq!(open.status_of(ready.id), Status::Ready);
}
