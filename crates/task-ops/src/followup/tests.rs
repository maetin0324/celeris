//! ADR-0098（Phase R7-10）: run が宣言した後続は元の task の案件とリポジトリを継ぐ。

use super::*;
use serde_json::json;
use task_core::{
    Project, ProjectId, ProjectRepo, ProjectStatus, RepoId, RepoKind, RepoRun, SqliteStore,
    WorkspaceSpec,
};

struct Fixture {
    store: SqliteStore,
    project: ProjectId,
    code: ProjectRepo,
    paper: ProjectRepo,
    dir: tempfile::TempDir,
}

fn project(store: &SqliteStore, title: &str) -> ProjectId {
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: title.into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).expect("project");
    project.id
}

fn repo(store: &SqliteStore, project: ProjectId, name: &str, primary: bool) -> ProjectRepo {
    let repo = ProjectRepo {
        id: RepoId::new(),
        project_id: project,
        name: name.into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local(format!("/srv/{name}")),
        default_branch: None,
        sync: None,
        run: RepoRun::Auto,
        is_primary: primary,
        created_at: OffsetDateTime::now_utc(),
    };
    store.repo_create(&repo).expect("repo");
    repo
}

fn fixture() -> Fixture {
    let store = SqliteStore::open_in_memory().expect("store");
    let project = project(&store, "agent-platform");
    let code = repo(&store, project, "agent-platform", true);
    let paper = repo(&store, project, "paper", false);
    Fixture {
        store,
        project,
        code,
        paper,
        dir: tempfile::tempdir().expect("tempdir"),
    }
}

fn spec(value: serde_json::Value) -> NewTaskSpec {
    serde_json::from_value(value).expect("spec")
}

fn minimal(title: &str) -> serde_json::Value {
    json!({"title": title, "objective": "o", "acceptance": [{"type": "command", "cmd": "true"}]})
}

/// 元の task（人が作った、案件 `project` の `repos`）。
fn origin(store: &SqliteStore, project: Option<ProjectId>, repos: &[&str]) -> Task {
    let mut body = minimal("origin");
    if let Some(p) = project {
        body["project_id"] = json!(p.to_string());
        body["repos"] = json!(repos);
    }
    crate::add::create_task(store, spec(body), OffsetDateTime::now_utc()).expect("origin")
}

fn write_followups(dir: &Path, tasks: serde_json::Value) {
    std::fs::write(
        dir.join(FOLLOWUPS_FILE_NAME),
        serde_json::to_string(&json!({"tasks": tasks})).expect("json"),
    )
    .expect("write");
}

fn absorb(f: &Fixture, origin: &Task, run_id: &str) -> Option<AbsorbOutcome> {
    absorb_followups_file(
        &f.store,
        origin.id,
        run_id,
        f.dir.path(),
        &[],
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("absorb")
}

fn created_origin(store: &SqliteStore, id: TaskId) -> Option<CreatedOrigin> {
    store
        .events_for(id)
        .expect("events")
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::Created { origin, .. } => Some(origin),
            _ => None,
        })
        .flatten()
}

fn progress(store: &SqliteStore, id: TaskId) -> Vec<String> {
    store
        .events_for(id)
        .expect("events")
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerProgress { msg, .. } => Some(msg),
            _ => None,
        })
        .collect()
}

/// D3-1/2・D5: 案件を省略した後続は X の案件と X の repos を継ぎ、出自が `worker_run` で残る。
#[test]
fn followup_inherits_the_origin_project_and_repos_and_records_provenance() {
    let f = fixture();
    let x = origin(&f.store, Some(f.project), &["paper"]);
    write_followups(f.dir.path(), json!([minimal("next step")]));

    let outcome = absorb(&f, &x, "run-1").expect("file present");
    assert_eq!(outcome.created.len(), 1, "{:?}", outcome.notes);
    let t = f.store.get(outcome.created[0]).expect("get").expect("task");
    assert_eq!(t.project_id, Some(f.project));
    assert_eq!(
        t.repos.iter().map(|r| r.repo_id).collect::<Vec<_>>(),
        vec![f.paper.id],
        "X の repos（primary ではない）"
    );
    assert_eq!(t.status, Status::Draft);
    assert_eq!(t.parent_id, None, "後続は X の子ではない");
    assert_eq!(
        created_origin(&f.store, t.id),
        Some(CreatedOrigin::WorkerRun {
            task_id: x.id,
            run_id: "run-1".into()
        })
    );
    assert!(
        progress(&f.store, x.id)
            .iter()
            .any(|m| m.starts_with("follow-up created: ") && m.contains(&t.id.to_string())),
        "X の timeline から辿れる"
    );
    // 適用済みの記録に改名され、二度目は何もしない。
    assert!(!f.dir.path().join(FOLLOWUPS_FILE_NAME).exists());
    assert!(f.dir.path().join(applied_file_name("run-1")).exists());
    assert_eq!(absorb(&f, &x, "run-1"), None);
}

/// D3-2: X が repos を持たなければ案件の primary。
#[test]
fn followup_of_an_origin_without_repos_gets_the_project_primary() {
    let f = fixture();
    let mut x = origin(&f.store, Some(f.project), &[]);
    x.repos.clear();
    f.store
        .update_task(
            &x,
            Event::Edited {
                fields: vec!["repos".into()],
                by: "human".into(),
            },
        )
        .expect("clear repos");
    let x = f.store.get(x.id).expect("get").expect("x");
    assert!(x.repos.is_empty());
    let (result, _) = create_followup(
        &f.store,
        &x,
        "run-1",
        spec(minimal("a")),
        &[],
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("create");
    let FollowupResult::Created(t) = result else {
        panic!("created")
    };
    assert_eq!(t.project_id, Some(f.project));
    assert_eq!(
        t.repos.iter().map(|r| r.repo_id).collect::<Vec<_>>(),
        vec![f.code.id]
    );
}

/// D3-2: 明示の repos は案件の中で解決し、知らない名前は拒否する。
#[test]
fn explicit_repos_resolve_inside_the_origin_project() {
    let f = fixture();
    let x = origin(&f.store, Some(f.project), &["agent-platform"]);
    let mut body = minimal("paper work");
    body["repos"] = json!(["paper"]);
    let (result, _) = create_followup(
        &f.store,
        &x,
        "r",
        spec(body),
        &[],
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("create");
    let FollowupResult::Created(t) = result else {
        panic!("created")
    };
    assert_eq!(t.repos[0].repo_id, f.paper.id);

    let mut body = minimal("elsewhere");
    body["repos"] = json!(["not-a-repo"]);
    assert!(matches!(
        create_followup(
            &f.store,
            &x,
            "r",
            spec(body),
            &[],
            &[],
            OffsetDateTime::now_utc()
        ),
        Err(OpsError::Validation(_))
    ));
}

/// D3-1: 別の案件を書いた 1 件は拒否し、他の件は作る。拒否の理由は X の進行に残る。
#[test]
fn a_different_project_is_rejected_without_blocking_the_others() {
    let f = fixture();
    let other = project(&f.store, "other");
    let x = origin(&f.store, Some(f.project), &["agent-platform"]);
    let mut foreign = minimal("foreign");
    foreign["project_id"] = json!(other.to_string());
    let mut same = minimal("same project");
    same["project_id"] = json!(f.project.to_string());
    write_followups(f.dir.path(), json!([foreign, same]));

    let outcome = absorb(&f, &x, "run-2").expect("file");
    assert_eq!(outcome.created.len(), 1);
    let t = f.store.get(outcome.created[0]).expect("get").expect("task");
    assert_eq!(t.title, "same project");
    assert_eq!(t.project_id, Some(f.project));
    let notes = progress(&f.store, x.id);
    assert!(
        notes
            .iter()
            .any(|m| m.contains("\"foreign\" rejected") && m.contains("its own project")),
        "{notes:?}"
    );
    assert!(
        f.store
            .list(None)
            .expect("list")
            .iter()
            .all(|t| t.project_id != Some(other)),
        "別の案件には何も作らない"
    );
}

/// D3-1: 案件の無い task の worker は案件を選べない。案件を書かなければ従来どおり案件無しで作る。
#[test]
fn an_origin_without_project_cannot_pick_one_and_creates_project_less_followups() {
    let f = fixture();
    let x = origin(&f.store, None, &[]);
    let mut picks = minimal("picks a project");
    picks["project_id"] = json!(f.project.to_string());
    write_followups(f.dir.path(), json!([picks, minimal("plain")]));
    let outcome = absorb(&f, &x, "run-3").expect("file");
    assert_eq!(outcome.created.len(), 1, "{:?}", outcome.notes);
    let t = f.store.get(outcome.created[0]).expect("get").expect("task");
    assert_eq!(t.title, "plain");
    assert_eq!(t.project_id, None);
    assert!(t.repos.is_empty());
    assert!(
        outcome
            .notes
            .iter()
            .any(|m| m.contains("belongs to no project"))
    );
}

/// D3-4/5: parent・assignee・adapter・workspace・cluster は使わず、status は常に draft。理由は進行に残す。
#[test]
fn worker_only_fields_are_dropped_and_status_is_always_draft() {
    let f = fixture();
    let x = origin(&f.store, Some(f.project), &["agent-platform"]);
    let mut body = minimal("over-specified");
    body["parent"] = json!(x.id.to_string());
    body["assignee"] = json!("dev");
    body["adapter"] = json!("codex");
    body["workspace"] = json!("/etc");
    body["status"] = json!("ready");
    let (result, notes) = create_followup(
        &f.store,
        &x,
        "r",
        spec(body),
        &[],
        &[],
        OffsetDateTime::now_utc(),
    )
    .expect("create");
    let FollowupResult::Created(t) = result else {
        panic!("created")
    };
    assert_eq!(t.parent_id, None);
    assert_eq!(t.assignee, None);
    assert_eq!(t.worker_hint.adapter, None);
    assert_eq!(t.status, Status::Draft);
    assert!(
        matches!(&t.workspace, WorkspaceSpec::Local { path, .. } if path == &PathBuf::from(t.id.to_string()))
    );
    assert_eq!(t.project_id, Some(f.project));
    for needle in ["parent", "assignee", "adapter", "workspace", "status"] {
        assert!(
            notes.iter().any(|n| n.starts_with(needle)),
            "{needle}: {notes:?}"
        );
    }
}

/// D4: 同じ案件に終端でない同じ題名の task があれば作らない（retry の再宣言）。
#[test]
fn a_redeclared_followup_with_the_same_title_is_not_duplicated() {
    let f = fixture();
    let x = origin(&f.store, Some(f.project), &["agent-platform"]);
    write_followups(f.dir.path(), json!([minimal("same")]));
    assert_eq!(absorb(&f, &x, "attempt-1").expect("file").created.len(), 1);
    write_followups(f.dir.path(), json!([minimal("  same ")]));
    let second = absorb(&f, &x, "attempt-2").expect("file");
    assert!(second.created.is_empty());
    assert!(second.notes.iter().any(|m| m.contains("same title")));
}

/// 壊れた要素は他を止めない。ファイルそのものが読めなければ 1 行残して何も作らない。
#[test]
fn malformed_entries_and_files_leave_a_note() {
    let f = fixture();
    let x = origin(&f.store, Some(f.project), &["agent-platform"]);
    write_followups(
        f.dir.path(),
        json!([{"title": "t", "bogus": 1}, minimal("good")]),
    );
    let outcome = absorb(&f, &x, "r1").expect("file");
    assert_eq!(outcome.created.len(), 1);
    assert!(outcome.notes.iter().any(|m| m.contains("not a task spec")));

    std::fs::write(f.dir.path().join(FOLLOWUPS_FILE_NAME), "not json").expect("write");
    let outcome = absorb(&f, &x, "r2").expect("file");
    assert!(outcome.created.is_empty());
    assert!(outcome.notes.iter().any(|m| m.contains("ignored")));
}

/// D6: `celerisctl add` の追記は既存の宣言を保ち、壊れたファイルは上書きしない。
#[test]
fn append_to_file_keeps_earlier_declarations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("artifacts").join(FOLLOWUPS_FILE_NAME);
    assert_eq!(append_to_file(&path, &spec(minimal("a"))).expect("1"), 1);
    assert_eq!(append_to_file(&path, &spec(minimal("b"))).expect("2"), 2);
    let file: FollowupsFile =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    let titles: Vec<String> = file.tasks.into_iter().map(|v| spec(v).title).collect();
    assert_eq!(titles, vec!["a", "b"]);

    std::fs::write(&path, "garbage").expect("write");
    assert!(append_to_file(&path, &spec(minimal("c"))).is_err());
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "garbage");
}
