//! ops-r1 trace の fixture（actor_trace flow1 相当）。本物の store で案件・案件 CoS thread・CoS operation
//! （`task.create`）・子 task・event を書き、chat message・chat run・run・root_done_summaries の行を足して、
//! 単一ファイルの DB の写しを作る。LLM は呼ばない。
#![allow(dead_code)]
use serde_json::json;
use std::path::{Path, PathBuf};
use task_core::chat::{AuditContext, ChatActor, ChatOpenThreadRequest};
use task_core::{
    Budget, Check, Criterion, Event, SqliteStore, Status, Task, TaskId, TaskKind, TaskStore, Tier,
    WorkerHint, WorkspaceSpec,
};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

pub const TRIAL: &str = "ops-r1-b-1";
pub const COS_RUN: &str = "cos-intake-1";
pub const REPORT_RUN: &str = "cos-report-1";

pub fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::parse("2026-10-29T01:00:00Z", &Rfc3339).unwrap() + Duration::seconds(seconds)
}
pub fn ts(seconds: i64) -> String {
    at(seconds).format(&Rfc3339).unwrap()
}
/// 出力の時刻の表記（UTC・小数 3 桁）。
pub fn out_ts(seconds: i64) -> String {
    assert!((0..3600).contains(&seconds));
    format!("2026-10-29T01:{:02}:{:02}.000Z", seconds / 60, seconds % 60)
}

/// どこまで書くか。
#[derive(Clone, Copy)]
pub struct Shape {
    /// 報告の CoS run と assistant message を書く（false なら報告の依頼だけが積まれた状態）。
    pub report: bool,
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    /// 単一ファイルの写し（`-wal` なし）。
    pub copy: PathBuf,
    pub thread: String,
    pub project: String,
    pub root: TaskId,
    pub child: TaskId,
    pub operation_id: String,
    pub intake_message: String,
    pub report_input: String,
    pub report_message: Option<String>,
}

fn task(id: TaskId, parent: Option<TaskId>, project: task_core::ProjectId) -> Task {
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: parent,
        kind: TaskKind::Execute,
        title: "ops-r1 trial".into(),
        objective: "solve the case".into(),
        acceptance: vec![Criterion {
            text: "reviewer passes".into(),
            check: Check::Reviewer,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from(id.to_string()),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
            max_tokens: None,
        },
        attempts: 0,
        lease: None,
        created_at: at(20),
        updated_at: at(20),
        role: None,
        genre: None,
        aggregate: false,
        project_id: Some(project),
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn message(
    conn: &rusqlite::Connection,
    thread: &str,
    seq: i64,
    role: &str,
    client: Option<&str>,
    author: Option<&str>,
    seconds: i64,
) -> String {
    let id = ulid::Ulid::new().to_string();
    let meta = match author {
        Some(a) => json!({ "author": a }),
        None => json!({}),
    };
    conn.execute(
        "INSERT INTO chat_messages(id,thread_id,seq,role,text,state,client_message_id,metadata_json,created_at,updated_at) \
         VALUES(?1,?2,?3,?4,'text','completed',?5,?6,?7,?7)",
        rusqlite::params![id, thread, seq, role, client, meta.to_string(), ts(seconds)],
    )
    .unwrap();
    conn.execute(
        "UPDATE chat_threads SET next_seq=?2 WHERE id=?1",
        rusqlite::params![thread, seq + 1],
    )
    .unwrap();
    id
}

#[allow(clippy::too_many_arguments)]
fn chat_run(
    conn: &rusqlite::Connection,
    run: &str,
    thread: &str,
    input: &str,
    output: Option<&str>,
    state: &str,
    from: i64,
    to: i64,
) {
    conn.execute(
        "INSERT INTO chat_runs(run_id,thread_id,input_message_id,output_message_id,state,started_at,finished_at,usage_json) \
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        rusqlite::params![
            run,
            thread,
            input,
            output,
            state,
            ts(from),
            ts(to),
            json!({"input_tokens":120,"output_tokens":30,"cache_read_tokens":40,"cache_creation_tokens":10,"cost_usd":0.002}).to_string()
        ],
    )
    .unwrap();
}

fn run_row(conn: &rusqlite::Connection, run: &str, task: TaskId, role: &str, from: i64, to: i64) {
    conn.execute(
        "INSERT INTO runs(run_id,task_id,role,seq,status,usage_json,started_at,finished_at) \
         VALUES(?1,?2,?3,1,'succeeded',?4,?5,?6)",
        rusqlite::params![
            run,
            task.to_string(),
            role,
            json!({"input_tokens":1000,"output_tokens":200,"cache_read_tokens":0,"cache_creation_tokens":0}).to_string(),
            ts(from),
            ts(to)
        ],
    )
    .unwrap();
}

fn append(db: &Path, task: TaskId, event: &Event, seconds: i64) -> i64 {
    let mut conn = rusqlite::Connection::open(db).unwrap();
    let tx = conn.transaction().unwrap();
    SqliteStore::append_event_tx_at(&tx, task, event, at(seconds)).unwrap();
    let seq: i64 = tx
        .query_row(
            "SELECT MAX(seq) FROM events WHERE task_id=?1",
            [task.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    tx.commit().unwrap();
    seq
}

impl Fixture {
    pub fn new(shape: Shape) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("measure.db");
        let store = SqliteStore::open(&db).unwrap();
        let project = task_core::Project {
            id: task_core::ProjectId::new(),
            title: "ops-r1".into(),
            request: "ops-r1 B".into(),
            status: task_core::ProjectStatus::Active,
            workspace: None,
            archived_at: None,
            paused_from: None,
            auto_advance: false,
            auto_integrate: false,
            slug: None,
            created_at: at(0),
            updated_at: at(0),
        };
        store.project_create(&project).unwrap();
        let thread = store
            .cos_thread_open(
                &ChatOpenThreadRequest {
                    project_id: Some(project.id.to_string()),
                },
                at(0),
            )
            .unwrap()
            .thread
            .id;
        let conn = rusqlite::Connection::open(&db).unwrap();
        // 受付: 人の依頼の message と、それを取り込んだ案件 CoS の run。
        let intake_message = message(
            &conn,
            &thread,
            1,
            "user",
            Some(&format!("ops-r1-intake:{TRIAL}")),
            Some("human:tester"),
            10,
        );
        // 起票: 案件 CoS の task.create operation（task・cos_operations・監査 event を同じ transaction で書く）。
        let root = TaskId::new();
        let operation_id = ulid::Ulid::new().to_string();
        let by = format!("cos:{thread}");
        let ctx = AuditContext {
            actor: ChatActor::Cos,
            thread_id: thread.clone(),
            run_id: COS_RUN.into(),
            operation_id: operation_id.clone(),
            reason: "人の依頼を起票する".into(),
            policy_version: "6".into(),
        };
        let root_task = task(root, None, project.id);
        store
            .cos_operation_apply_at(
                &ctx,
                "ops-r1-create",
                "hash-1",
                "task",
                &root.to_string(),
                None,
                "task.create",
                &json!({"title": "ops-r1 trial"}),
                at(20),
                |tx, _| {
                    SqliteStore::create_task_tx(tx, &root_task, None, &by, vec![])?;
                    Ok(json!({"task_id": root.to_string()}))
                },
            )
            .unwrap();
        // 子 task（計画から daemon が作る）。
        let child = TaskId::new();
        store
            .create_task_by(&task(child, Some(root), project.id), "daemon", vec![])
            .unwrap();
        // task と run: planner・worker・reviewer。
        run_row(&conn, "run-planner-1", root, "planner", 40, 60);
        run_row(&conn, "run-worker-1", child, "worker", 70, 120);
        run_row(&conn, "run-reviewer-1", root, "reviewer", 130, 150);
        // 成果確定: review の判定と done への遷移。
        append(
            &db,
            root,
            &Event::ReviewVerdict {
                run_id: "run-reviewer-1".into(),
                criterion_idx: 0,
                pass: true,
                reason: "ok".into(),
            },
            150,
        );
        append(
            &db,
            root,
            &Event::ReviewerPassed {
                review_run: "run-reviewer-1".into(),
                repo_id: task_core::RepoId::new(),
                target_sha: "a".repeat(40),
                reviewed_sha: "a".repeat(40),
                criteria: vec![0],
            },
            151,
        );
        let done_seq = append(
            &db,
            root,
            &Event::Transitioned {
                from: Status::Reviewing,
                to: Status::Done,
                reason: "reviewer passed".into(),
            },
            160,
        );
        // 報告: daemon が積む root done の依頼と、それへの案件 CoS の返答。
        let report_input = message(&conn, &thread, 2, "system", None, None, 161);
        conn.execute(
            "INSERT INTO root_done_summaries(task_id,done_seq,state,thread_id,input_message_id,created_at,updated_at) \
             VALUES(?1,?2,?3,?4,?5,?6,?6)",
            rusqlite::params![
                root.to_string(),
                done_seq,
                if shape.report { "notified" } else { "queued" },
                thread,
                report_input,
                ts(161)
            ],
        )
        .unwrap();
        let report_message = shape.report.then(|| {
            let m = message(&conn, &thread, 3, "assistant", None, None, 170);
            chat_run(
                &conn,
                REPORT_RUN,
                &thread,
                &report_input,
                Some(&m),
                "completed",
                162,
                171,
            );
            m
        });
        let intake_reply = message(&conn, &thread, 4, "assistant", None, None, 30);
        chat_run(
            &conn,
            COS_RUN,
            &thread,
            &intake_message,
            Some(&intake_reply),
            "completed",
            11,
            30,
        );
        // 単一ファイルの写し（sqlite3 .backup と同じく -wal を伴わない）。
        let copy = dir.path().join("copy.db");
        conn.execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
            .unwrap();
        drop(conn);
        drop(store);
        let c = rusqlite::Connection::open(&copy).unwrap();
        c.query_row("PRAGMA journal_mode=DELETE", [], |_| Ok(()))
            .unwrap();
        drop(c);
        Self {
            dir,
            copy,
            thread,
            project: project.id.to_string(),
            root,
            child,
            operation_id,
            intake_message,
            report_input,
            report_message,
        }
    }

    pub fn args(&self, out: &Path, cutoff: &str) -> Vec<String> {
        [
            "ops-r1-trace",
            "--db",
            self.copy.to_str().unwrap(),
            "--thread",
            &self.thread,
            "--trial-id",
            TRIAL,
            "--cutoff",
            cutoff,
            "--out",
            out.to_str().unwrap(),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}
