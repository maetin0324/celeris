//! ADR-0095（Phase R7-6）: worker の run から本番 DB は読み取り専用。本番 2026-09-30 22:37:52Z の事故
//! （codex の run が `celerisctl add --db <本番 DB>` を escalation で sandbox の外で走らせ、DB を migrate した）を、
//! 実バイナリ `celeris` + `adapter = "codex"`（スタブの `codex` は `sh` スクリプト）+ 実バイナリ `celerisctl` で再現する。
//! スタブはエージェントの代わりに、daemon が開いている WAL の DB に対して `celerisctl ls` / `celerisctl add` /
//! 生の書き込みを試み、結果を artifacts に書く。外部ネットワークに出ない。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use task_core::{
    Budget, Check, Criterion, Event, SCHEMA_VERSION, SqliteStore, Status, Task, TaskId, TaskKind,
    TaskStore, Tier, Trigger, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

fn bin(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug_dir = exe.parent().unwrap().parent().unwrap();
    let path = debug_dir.join(name);
    assert!(
        path.exists(),
        "{} not found; run `cargo test --workspace`",
        path.display()
    );
    path
}

fn ready_task(store: &SqliteStore, dir: &Path) -> TaskId {
    let now = OffsetDateTime::now_utc();
    let task = Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "worker tries to write the db".into(),
        objective: "e2e: ADR-0095".into(),
        acceptance: vec![Criterion {
            text: "probe ran".into(),
            check: Check::Command {
                cmd: "test -f artifacts/probe.txt".into(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Draft,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: dir.to_path_buf(),
            mode: None,
        },
        budget: Budget {
            max_turns: 5,
            max_wall_secs: 60,
            max_retries: 0,
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
    store.insert(&task).unwrap();
    store
        .append_event(
            task.id,
            &Event::Created {
                task: Box::new(task.clone()),
                origin: None,
            },
        )
        .unwrap();
    store
        .apply_transition(task.id, Trigger::Accept, None)
        .unwrap();
    task.id
}

#[test]
fn a_codex_worker_run_cannot_write_the_daemon_db_but_can_read_it_with_celerisctl() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let db = root.join("celeris.sqlite3");
    let store = SqliteStore::open(&db).unwrap();
    let ws = root.join("workspaces").join("probe");
    std::fs::create_dir_all(&ws).unwrap();
    let task_id = ready_task(&store, &ws);
    let tasks_before = store.list(None).unwrap().len();

    // スタブの codex: 事故の run と同じく `celerisctl add --db <daemon の DB>` を打つ（ここでは
    // escalation の有無に関係なく、celeris が起動したプロセスの子なので namespace の中）。
    let stub = root.join("codex-stub.sh");
    std::fs::write(
        &stub,
        format!(
            r#"#!/bin/sh
set -u
CTL='{ctl}'
DB='{db}'
mkdir -p artifacts
"$CTL" --db "$DB" ls > artifacts/ls.out 2>&1; echo "ls_exit=$?" >> artifacts/probe.txt
"$CTL" --db "$DB" show {task_id} > artifacts/show.out 2>&1; echo "show_exit=$?" >> artifacts/probe.txt
"$CTL" --db "$DB" add --title injected --objective x --check-cmd true > artifacts/add.out 2>&1; echo "add_exit=$?" >> artifacts/probe.txt
(echo garbage >> "$DB") 2>/dev/null; echo "raw_db_write_exit=$?" >> artifacts/probe.txt
(true >> "$DB-wal") 2>/dev/null; echo "raw_wal_write_exit=$?" >> artifacts/probe.txt
printf '%s' '{{"summary":"probed","evidence":[]}}' > artifacts/result.json
echo '{{"type":"turn.completed"}}'
"#,
            ctl = bin("celerisctl").display(),
            db = db.display(),
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let config = root.join("config.toml");
    std::fs::write(
        &config,
        format!(
            r#"db = "celeris.sqlite3"
workspace_root = "workspaces"
tick_ms = 50
max_concurrency = 1
lease_grace_secs = 60
idle_timeout_secs = 30
kill_grace_secs = 1
review_timeout_secs = 30
retry_backoff_base_secs = 0

[adapters.codex]
command = "{stub}"
extra_args = ["--approve-for-me"]

[[providers]]
id = "codex-local"
adapter = "codex"
tiers = ["frontier", "standard", "cheap"]
concurrency = 1
"#,
            stub = stub.display()
        ),
    )
    .unwrap();

    let log = root.join("celeris.log");
    let mut child = Command::new(bin("celeris"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "--until-idle",
            "--max-ticks",
            "2000",
            "--log-format",
            "text",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            panic!(
                "celeris did not reach idle\n{}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let log_text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(status.success(), "celeris exited with {status}\n{log_text}");

    let artifacts = ws.join("artifacts");
    let probe = std::fs::read_to_string(artifacts.join("probe.txt"))
        .unwrap_or_else(|e| panic!("probe.txt: {e}\n{log_text}"));
    let add_out = std::fs::read_to_string(artifacts.join("add.out")).unwrap_or_default();
    let ls_out = std::fs::read_to_string(artifacts.join("ls.out")).unwrap_or_default();
    // 読み取り（`ls` / `show`）は worker から使える。
    assert!(probe.contains("ls_exit=0"), "{probe}\n{ls_out}");
    assert!(probe.contains("show_exit=0"), "{probe}");
    // 書き込みは失敗する（celerisctl も生の書き込みも）。
    assert!(probe.contains("add_exit=1"), "{probe}\n{add_out}");
    assert!(
        add_out.contains("attempt to write a readonly database"),
        "{add_out}"
    );
    assert!(add_out.contains("ADR-0095"), "{add_out}");
    assert!(!probe.contains("raw_db_write_exit=0"), "{probe}");
    assert!(!probe.contains("raw_wal_write_exit=0"), "{probe}");
    assert!(
        log_text.contains("worker runs see the db directory read-only"),
        "{log_text}"
    );

    // DB は変わっていない（起票されていない・schema はそのまま・壊れていない）。
    let tasks = store.list(None).unwrap();
    assert_eq!(tasks.len(), tasks_before, "no task was injected");
    assert!(tasks.iter().all(|t| t.title != "injected"));
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert!(task_core::integrity_check(&db).unwrap());
    assert_eq!(
        store.get(task_id).unwrap().map(|t| t.status),
        Some(Status::Done),
        "{log_text}"
    );
}
