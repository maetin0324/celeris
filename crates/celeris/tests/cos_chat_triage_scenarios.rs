//! ADR 2026-10-05 D6 の通し試験（送信側）: 通知の一本化と一次対応 B の Discord 送信。daemon の
//! tick（`tick_loop`）と同じ順（`select_routes_batch` → `withdraw_if_resolved` → `triage::render` →
//! `post_webhook` → `record`）を偽 webhook（127.0.0.1 の TCP）に向けて回す。外部ネットワークに出ず、
//! 時刻は固定値を渡す。待ちは webhook の受信を出来事として待ち、sleep しない。
use std::sync::{Arc, Mutex};

use celeris::notify::{self, NotifyConfig, SendResult};
use task_core::chat::triage::CosTriageSource;
use task_core::feed::{NoticeEvent, NoticeKind, NoticeStore};
use task_core::model::{Budget, Check, Criterion, WorkerHint, WorkspaceSpec};
use task_core::{
    Event, NotificationKind, NotificationStore, SqliteStore, Status, Task, TaskId, TaskKind,
    TaskStore, Tier,
};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn at() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock")
}

fn config() -> NotifyConfig {
    NotifyConfig {
        gui_base_url: Some("http://127.0.0.1:7700/".into()),
        ..Default::default()
    }
}

/// Every non-CoS kind of ADR-0037 / ADR-0050 / ADR-0133 that used to reach Discord directly.
const LEGACY: [NotificationKind; 13] = [
    NotificationKind::InboxNew,
    NotificationKind::Digest,
    NotificationKind::MilestoneReady,
    NotificationKind::ApprovalPending,
    NotificationKind::QuestionBlocked,
    NotificationKind::BadNews,
    NotificationKind::SecretaryReply,
    NotificationKind::TaskReady,
    NotificationKind::ClusterLoginNeeded,
    NotificationKind::TaskFailed,
    NotificationKind::PhaseCheckpoint,
    NotificationKind::DecisionRequested,
    NotificationKind::PlanApproval,
];

/// A fake Discord webhook: answers 204 and keeps each request body.
struct Hook {
    url: String,
    bodies: Arc<Mutex<Vec<String>>>,
    seen: tokio::sync::mpsc::UnboundedReceiver<()>,
}

async fn hook() -> Hook {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let url = format!("http://{}/hook", listener.local_addr().expect("addr"));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let (tx, seen) = tokio::sync::mpsc::unbounded_channel();
    let kept = bodies.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut raw = Vec::new();
            let mut buf = vec![0; 8192];
            // Read the head, then exactly Content-Length bytes of body.
            let body = loop {
                let n = stream.read(&mut buf).await.expect("read");
                if n == 0 {
                    break String::new();
                }
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).to_string();
                if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if rest.len() >= len {
                        break rest.to_string();
                    }
                }
            };
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await
                .expect("reply");
            kept.lock().expect("bodies").push(body);
            let _ = tx.send(());
        }
    });
    Hook { url, bodies, seen }
}

fn view(dir: &std::path::Path) -> task_ops::view::ViewContext {
    task_ops::view::ViewContext {
        workspace_root: dir.into(),
        retry_backoff_base: std::time::Duration::from_secs(1),
        retry_backoff_max: std::time::Duration::from_secs(60),
        max_requeues: 3,
        clusters: Default::default(),
    }
}

/// One notifier tick, as `tick_loop` does it. Returns whether a webhook POST was made.
async fn tick(
    store: &SqliteStore,
    db: &std::path::Path,
    view: &task_ops::view::ViewContext,
    hook: &mut Hook,
) -> bool {
    let pending = store.notification_pending().expect("pending");
    let Some(mut batch) = notify::select_routes_batch(&pending) else {
        return false;
    };
    let row = pending
        .iter()
        .find(|n| batch.ids.contains(&n.id))
        .expect("row")
        .clone();
    if !notify::triage::withdraw_if_resolved(store, db, view, &row, at()).expect("recheck") {
        return false;
    }
    batch.content = notify::triage::render(&row, &config()).expect("render");
    let client = notify::client().expect("client");
    let outcome = notify::post_webhook(&client, &hook.url, &batch.content).await;
    hook.seen.recv().await.expect("the fake webhook saw the post");
    notify::record(
        store,
        &pending,
        &SendResult {
            ids: batch.ids,
            outcome,
        },
        at(),
    )
    .expect("record");
    true
}

fn question_task(store: &SqliteStore, title: &str) -> Task {
    let id = TaskId::new();
    let now = OffsetDateTime::now_utc();
    let task = Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: title.into(),
        objective: "make it work".into(),
        acceptance: vec![Criterion {
            text: "a human is happy".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Blocked,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: std::path::PathBuf::from(id.to_string()),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    };
    store
        .create_task(
            &task,
            vec![Event::QuestionRaised {
                run_id: "r0".into(),
                text: format!("{title}?"),
            }],
        )
        .expect("task");
    task
}

/// Ingest the derived `question-<task>` wait and claim its escalation outbox row.
fn escalate(store: &SqliteStore, db: &std::path::Path, task: &Task, summary: &str) -> String {
    // The revision is the derived item's `created_at`, as the dispatcher's intake keys it.
    let dir = db.parent().expect("dir");
    let derived = task_ops::human_inbox::human_inbox(
        store,
        None,
        &view(dir),
        at(),
        &|_, _| Vec::new(),
        None,
    )
    .expect("inbox")
    .items
    .into_iter()
    .find(|i| i.id == format!("question-{}", task.id))
    .expect("derived question item");
    let source = CosTriageSource {
        source_kind: "question".into(),
        source_key: derived.id.clone(),
        source_revision: derived.created_at.clone(),
        source_event_id: None,
        operation_id: None,
        summary: summary.into(),
        policy_version: "1".into(),
    };
    store
        .cos_triage_ingest_batch("events", &task.id.to_string(), &[source], at())
        .expect("ingest");
    let item: String = rusqlite::Connection::open(db)
        .expect("db")
        .query_row(
            "SELECT id FROM cos_inbox_items WHERE source_key=?1",
            [format!("question-{}", task.id)],
            |r| r.get(0),
        )
        .expect("item");
    let packet = serde_json::json!({
        "summary": summary,
        "options": [{"key": "reply", "label": "web で回答"}],
        "recommended": null,
        "recommendation_reason": "外部への push は人の認可が要る",
        "blocking": task.title,
        "web_path": format!("/tasks/{}", task.id),
    });
    store
        .cos_triage_outbox_claim(&item, "escalation", &packet.to_string(), at())
        .expect("outbox");
    item
}

/// D6 通知の一本化: 旧来の全通知 kind を pending に入れ、全 notice kind を記録しても、偽 webhook への
/// 直接送信は 0 通。CoS の escalation を 1 件足すと、その 1 通だけが送られる。
#[tokio::test]
async fn cos_chat_triage_unified_only_cos_escalation_reaches_the_webhook() {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("celeris.db");
    let store = SqliteStore::open(&db).expect("db");
    let view = view(dir.path());
    let mut hook = hook().await;
    for (i, kind) in LEGACY.iter().enumerate() {
        store
            .notification_upsert_pending(*kind, &format!("legacy-{i}"), "old body", None, at())
            .expect("insert")
            .expect("new row");
    }
    for kind in NoticeKind::ALL {
        store
            .notice_record(&NoticeEvent {
                source_key: format!("notice-{kind:?}"),
                kind,
                group_key: format!("g:{kind:?}"),
                title: format!("{kind:?}"),
                summary: "notice".into(),
                project_id: None,
                task_id: None,
                target: None,
                links: Vec::new(),
                at: at(),
            })
            .expect("notice");
    }
    for _ in 0..(LEGACY.len() + NoticeKind::ALL.len()) {
        assert!(!tick(&store, &db, &view, &mut hook).await, "a legacy kind was sent");
    }
    assert!(hook.bodies.lock().expect("bodies").is_empty());

    let task = question_task(&store, "外部への push");
    escalate(&store, &db, &task, "外部 repo への push の可否");
    assert!(tick(&store, &db, &view, &mut hook).await);
    assert!(!tick(&store, &db, &view, &mut hook).await, "sent once");
    let bodies = hook.bodies.lock().expect("bodies").clone();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].contains("CoS から判断のお願い"), "{}", bodies[0]);
    assert!(
        bodies[0].contains("\"allowed_mentions\":{\"parse\":[]}"),
        "{}",
        bodies[0]
    );
}

/// D6 一次対応 B（送信）: escalate の outbox は要点・選択肢・推奨の理由・止まっている範囲・web の
/// 絶対 link と「回答はリンク先で」を 1,900 字以内で 1 通送る。人が web で答えた後に残った
/// escalation は送信前の再照合で取り下げられ、通知が止まる。
#[tokio::test]
async fn cos_chat_triage_b_webhook_packet_and_stop_after_human_answer() {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("celeris.db");
    let store = SqliteStore::open(&db).expect("db");
    let view = view(dir.path());
    let mut hook = hook().await;
    let first = question_task(&store, "push の確認");
    escalate(&store, &db, &first, "外部 repo への push の可否");
    assert!(tick(&store, &db, &view, &mut hook).await);
    let sent = hook.bodies.lock().expect("bodies")[0].clone();
    let payload: serde_json::Value = serde_json::from_str(&sent).expect("json");
    let content = payload["content"].as_str().expect("content");
    for required in [
        "CoS から判断のお願い",
        "外部 repo への push の可否",
        "web で回答",
        "外部への push は人の認可が要る",
        "push の確認",
        &format!("http://127.0.0.1:7700/tasks/{}", first.id),
        "回答はリンク先で",
    ] {
        assert!(content.contains(required), "missing {required}: {content}");
    }
    assert!(content.chars().count() <= 1900);

    // A second escalation is pending; the human answers on the web before the next tick.
    let second = question_task(&store, "設計の変更");
    escalate(&store, &db, &second, "設計の根本変更");
    task_ops::gate::answer(&store, second.id, "web で答えた".into(), None).expect("human answer");
    assert!(!tick(&store, &db, &view, &mut hook).await);
    assert!(store.notification_pending().expect("pending").is_empty());
    assert_eq!(hook.bodies.lock().expect("bodies").len(), 1);
}
