//! ADR 2026-10-15-credential-wait-skip-triage: credential 待ちの受信箱 item は CoS triage（LLM run）を経ず、決定的に escalation outbox へ
//! 入り（`cos_triage_direct_escalate`）、既存の通知経路（outbox → Discord webhook）で 1 件だけ送られる。登録されて解決したら送る前に取り下げられる。
//! 送信先は偽の HTTP サーバ（外部ネットワークに出ない）。

use celeris::notify::{self, SendResult};
use task_core::NotificationStore;
use task_core::chat::triage::CosTriageSource;
use task_core::{
    Budget, Check, CredentialWaitEnter, Criterion, SqliteStore, Status, Task, TaskId, TaskKind,
    TaskStore, Tier, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

const SECRET: &str = "jev_api_key";
const DUMMY: &str = "sk-dummy-notify-never-leak-9911";

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).expect("clock")
}

fn task(title: &str) -> Task {
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: title.into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: "ws".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 0,
        },
        attempts: 0,
        lease: None,
        created_at: at(0),
        updated_at: at(0),
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn view(dir: &std::path::Path) -> task_ops::view::ViewContext {
    task_ops::view::ViewContext {
        workspace_roots: task_core::local_machine::WorkspaceRoots::new(dir.into(), None),
        retry_backoff_base: std::time::Duration::from_secs(1),
        retry_backoff_max: std::time::Duration::from_secs(60),
        max_requeues: 3,
        clusters: Default::default(),
    }
}

fn wait_for(store: &SqliteStore, t: &Task, unit: &str) {
    store.insert(t).expect("task");
    store
        .credential_wait_enter(
            &CredentialWaitEnter {
                work_unit_id: unit,
                task_id: t.id,
                secret_id: SECRET,
                provider: "jev",
                purpose: "Jev で依頼を分類する",
            },
            at(1),
        )
        .expect("enter");
}

async fn fake_webhook(max: usize) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = tokio::spawn(async move {
        let mut bodies = Vec::new();
        for _ in 0..max {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 8192];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            bodies.push(String::from_utf8_lossy(&buf[..n]).to_string());
            let _ = socket
                .write_all(b"HTTP/1.1 204 X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .await;
            let _ = socket.shutdown().await;
        }
        bodies
    });
    (format!("http://{addr}/hook"), handle)
}

#[tokio::test]
async fn one_credential_wait_item_is_sent_to_discord_once_and_resolved_before_a_second_post() {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("test.db");
    let store = SqliteStore::open(&db).expect("db");
    let view = view(dir.path());

    // 2 つの task が同じ鍵を待つ → 受信箱 item は 1 件。
    let (t1, t2) = (task("分類 A"), task("分類 B"));
    wait_for(&store, &t1, "wu-1");
    wait_for(&store, &t2, "wu-2");
    let inbox = task_ops::human_inbox::human_inbox(&store, None, &view, at(2), &|_, _| Vec::new())
        .expect("inbox");
    let items: Vec<_> = inbox
        .items
        .iter()
        .filter(|i| i.kind == task_ops::human_inbox::InboxKind::CredentialWait)
        .collect();
    assert_eq!(items.len(), 1);
    let item = items[0];

    // 直経路: triage の待ち（pending）を作らず outbox に 1 件。2 回目以降・task の増加では増えない。
    let source = CosTriageSource {
        source_kind: item.kind.as_str().into(),
        source_key: item.id.clone(),
        source_revision: item.created_at.clone(),
        source_event_id: None,
        operation_id: None,
        summary: item.title.clone(),
        policy_version: "1".into(),
    };
    let payload = serde_json::json!({
        "headline": "API キーの登録のお願い（自動通知）",
        "summary": "Jev の API キーの登録をお願いします（登録すれば自動で続行されます）",
        "blocking": "分類 A・分類 B", "web_path": "/accounts", "options": [],
    })
    .to_string();
    assert!(
        store
            .cos_triage_direct_escalate(&source, &payload, at(2))
            .expect("direct")
            .is_some()
    );
    assert!(
        store
            .cos_triage_direct_escalate(&source, &payload, at(3))
            .expect("again")
            .is_none()
    );
    let conn = rusqlite::Connection::open(&db).expect("conn");
    let pending_items: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cos_inbox_items WHERE state IN ('pending','running')",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(pending_items, 0, "CoS triage の対象にならない");
    let row = store.notification_pending().expect("pending").remove(0);
    // まだ解決していないので取り下げない。
    assert!(notify::triage::withdraw_if_resolved(&store, &db, &view, &row, at(5)).expect("live"));

    let config = notify::NotifyConfig {
        web_base_url: Some("http://127.0.0.1:7720/".into()),
        ..Default::default()
    };
    let body = notify::triage::render(&row, &config).expect("render");
    assert!(body.contains("http://127.0.0.1:7720/accounts"), "{body}");
    assert!(!body.contains(DUMMY));

    let (url, server) = fake_webhook(1).await;
    let client = notify::client().expect("client");
    let batch = notify::select_routes_batch(std::slice::from_ref(&row)).expect("batch");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<SendResult>(4);
    notify::spawn_send(client, url, &batch, tx);
    let result = rx.recv().await.expect("result");
    assert_eq!(result.outcome, notify::SendOutcome::Sent, "{result:?}");
    let bodies = server.await.expect("server");
    assert_eq!(bodies.len(), 1);
    assert!(bodies[0].contains("/accounts"), "{}", bodies[0]);
    assert!(bodies[0].contains("自動で続行"), "{}", bodies[0]);
    assert!(!bodies[0].contains(DUMMY));
}

#[test]
fn a_registered_and_resumed_wait_withdraws_its_pending_notification_before_the_post() {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("test.db");
    let store = SqliteStore::open(&db).expect("db");
    let view = view(dir.path());
    let t = task("分類 A");
    wait_for(&store, &t, "wu-1");
    let item = task_ops::human_inbox::human_inbox(&store, None, &view, at(2), &|_, _| Vec::new())
        .expect("inbox")
        .items
        .into_iter()
        .find(|i| i.kind == task_ops::human_inbox::InboxKind::CredentialWait)
        .expect("item");
    store
        .cos_triage_direct_escalate(
            &CosTriageSource {
                source_kind: item.kind.as_str().into(),
                source_key: item.id.clone(),
                source_revision: item.created_at.clone(),
                source_event_id: None,
                operation_id: None,
                summary: item.title.clone(),
                policy_version: "1".into(),
            },
            "{\"web_path\":\"/accounts\"}",
            at(3),
        )
        .expect("direct");
    let row = store.notification_pending().expect("pending").remove(0);

    // 登録 → 検証済み → 再開（待ちが閉じる）。送る前に取り下げられ、未送信の通知は残らない。
    let fp = "fp-0123456789ab";
    store
        .credential_record_secret_saved(SECRET, fp, at(4))
        .expect("event");
    assert!(
        store
            .credential_wait_mark_verified("wu-1", SECRET, fp, at(5))
            .expect("verified")
    );
    assert!(
        store
            .credential_wait_claim_resume("wu-1", SECRET, at(6))
            .expect("resume")
    );
    assert!(!notify::triage::withdraw_if_resolved(&store, &db, &view, &row, at(7)).expect("gone"));
    assert!(store.notification_pending().expect("pending").is_empty());
}
