//! ADR-0095（Phase R7-6）: worker の run から本番 DB は読み取り専用。本番 2026-09-30 22:37:52Z の事故
//! （codex の run が `celerisctl add --db <本番 DB>` を escalation で sandbox の外で走らせ、DB を migrate した）を、
//! 実バイナリ `celeris` + `adapter = "codex"`（スタブの `codex` は `sh` スクリプト）+ 実バイナリ `celerisctl` で再現する。
//! スタブはエージェントの代わりに、daemon が開いている WAL の DB に対して `celerisctl ls` / `celerisctl add` /
//! 生の書き込みを試み、結果を artifacts に書く。外部ネットワークに出ない。
//!
//! ADR-0098（Phase R7-10）: 同じ `celerisctl add --db <daemon の DB>` は、run の中では DB を開かずに run の
//! `followups.json` への宣言になり、run の後に daemon が元の task の案件・primary のリポジトリで `draft` として作る
//! （出自は `worker_run`）。R7-10 の env を外した `add` は従来どおり読み取り専用で失敗する（ADR-0095 の保証）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use task_core::{
    Budget, Check, CreatedOrigin, Criterion, Event, Project, ProjectId, ProjectRepo, ProjectStatus,
    RepoId, RepoKind, RepoRun, SCHEMA_VERSION, SqliteStore, Status, Task, TaskId, TaskKind,
    TaskStore, Tier, Trigger, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

/// worker run の印（`task_worker::db_guard::WORKER_DB_GUARD_ENV`、ADR-0126 A1-1）。
const WORKER_DB_GUARD_ENV: &str = "CELERIS_WORKER_DB_GUARD";

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

/// ADR-0098: 元の task の案件（primary のリポジトリは `dir` 種別。worktree は作らない）。
fn project_with_primary(store: &SqliteStore, dir: &Path) -> (ProjectId, RepoId) {
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "agent-platform".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).unwrap();
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: project.id,
        name: "agent-platform".into(),
        kind: RepoKind::Dir,
        location: WorkspaceSpec::local(dir),
        default_branch: None,
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: now,
    };
    store.repo_create(&repo).unwrap();
    (project.id, repo.id)
}

fn ready_task(store: &SqliteStore, dir: &Path, project: Option<ProjectId>) -> TaskId {
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
        // ADR-0098: 案件に属す（repos は持たない → 後続は案件の primary を継ぐ）。
        project_id: project,
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
    // ADR-0126 A3: worker run の印があると試験用 DB の daemon は guard を入れない（免除）。この試験は
    // guard そのもの（実 namespace）を確かめるので、daemon から印を外して probe 経路を通す。印がある
    // （worker sandbox の中で userns を作れない）なら CELERIS_USERNS_TESTS=1 のときだけ走らせる。
    let in_worker_run = std::env::var_os(WORKER_DB_GUARD_ENV).is_some_and(|v| !v.is_empty());
    if in_worker_run && std::env::var("CELERIS_USERNS_TESTS").as_deref() != Ok("1") {
        eprintln!(
            "SKIPPED (userns test, not passed): inside a worker run ({WORKER_DB_GUARD_ENV} is set); \
             set CELERIS_USERNS_TESTS=1 to run (ADR-0126)"
        );
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let db = root.join("celeris.sqlite3");
    let store = SqliteStore::open(&db).unwrap();
    let ws = root.join("workspaces").join("probe");
    std::fs::create_dir_all(&ws).unwrap();
    let primary_dir = root.join("primary-repo");
    std::fs::create_dir_all(&primary_dir).unwrap();
    let (project, primary) = project_with_primary(&store, &primary_dir);
    let task_id = ready_task(&store, &ws, Some(project));
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
echo "run_db=${{CELERIS_RUN_DB:-}}" >> artifacts/probe.txt
"$CTL" --db "$DB" add --title injected --objective x --check-cmd true > artifacts/add.out 2>&1; echo "add_exit=$?" >> artifacts/probe.txt
env -u CELERIS_RUN_DB -u CELERIS_FOLLOWUPS_FILE "$CTL" --db "$DB" add --title direct --objective x --check-cmd true > artifacts/direct.out 2>&1; echo "direct_exit=$?" >> artifacts/probe.txt
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
        .env_remove(WORKER_DB_GUARD_ENV)
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
    let direct_out = std::fs::read_to_string(artifacts.join("direct.out")).unwrap_or_default();
    let ls_out = std::fs::read_to_string(artifacts.join("ls.out")).unwrap_or_default();
    // 読み取り（`ls` / `show`）は worker から使える。
    assert!(probe.contains("ls_exit=0"), "{probe}\n{ls_out}");
    assert!(probe.contains("show_exit=0"), "{probe}");
    // ADR-0098 D6: daemon の DB に向けた `add` は run の後続の宣言になる（DB は開かない）。
    assert!(
        probe.contains(&format!("run_db={}", db.display())),
        "{probe}"
    );
    assert!(probe.contains("add_exit=0"), "{probe}\n{add_out}");
    assert!(add_out.contains("queued follow-up #1"), "{add_out}");
    // 直接の書き込みは失敗する（R7-10 の env を外した celerisctl も生の書き込みも）。
    assert!(probe.contains("direct_exit=1"), "{probe}\n{direct_out}");
    assert!(
        direct_out.contains("attempt to write a readonly database"),
        "{direct_out}"
    );
    assert!(direct_out.contains("ADR-0095"), "{direct_out}");
    assert!(!probe.contains("raw_db_write_exit=0"), "{probe}");
    assert!(!probe.contains("raw_wal_write_exit=0"), "{probe}");
    assert!(
        log_text.contains("worker runs see the db directory read-only"),
        "{log_text}"
    );

    // worker は DB に直接書けていない（`direct` は無い・schema はそのまま・壊れていない）。宣言した後続だけを
    // daemon が元の task の案件・primary で `draft` として作り、出自を `worker_run` で残している（ADR-0098）。
    let tasks = store.list(None).unwrap();
    assert_eq!(tasks.len(), tasks_before + 1, "only the declared follow-up");
    assert!(tasks.iter().all(|t| t.title != "direct"));
    let followup = tasks
        .iter()
        .find(|t| t.title == "injected")
        .unwrap_or_else(|| panic!("follow-up not created\n{log_text}"));
    assert_eq!(followup.project_id, Some(project));
    assert_eq!(
        followup.repos.iter().map(|r| r.repo_id).collect::<Vec<_>>(),
        vec![primary]
    );
    assert_eq!(followup.status, Status::Draft);
    let origin = store
        .events_for(followup.id)
        .unwrap()
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::Created { origin, .. } => Some(origin),
            _ => None,
        })
        .flatten();
    assert!(
        matches!(&origin, Some(CreatedOrigin::WorkerRun { task_id: t, .. }) if *t == task_id),
        "{origin:?}"
    );
    assert_eq!(store.schema_version().unwrap(), SCHEMA_VERSION);
    assert!(task_core::integrity_check(&db).unwrap());
    assert_eq!(
        store.get(task_id).unwrap().map(|t| t.status),
        Some(Status::Done),
        "{log_text}"
    );
}
