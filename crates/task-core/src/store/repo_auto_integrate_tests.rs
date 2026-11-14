//! ADR 2026-11-14-repo-auto-integrate: `project_repos.auto_integrate`（D1・D2）と migration 0126（D7）。

use rusqlite::Connection;
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, TaskStore};
use crate::model::WorkspaceSpec;
use crate::org::ProjectId;
use crate::repos::{ProjectRepo, RepoError, RepoId, RepoKind, RepoRun, RepoSync};

fn repo(project_id: ProjectId, name: &str, kind: RepoKind, location: WorkspaceSpec) -> ProjectRepo {
    let sync = matches!(location, WorkspaceSpec::Remote { .. }).then_some(RepoSync::Worktree);
    ProjectRepo {
        id: RepoId::new(),
        project_id,
        name: name.into(),
        kind,
        location,
        default_branch: None,
        sync,
        run: RepoRun::Auto,
        is_primary: false,
        auto_integrate: false,
        created_at: OffsetDateTime::now_utc(),
    }
}

fn remote(path: &str) -> WorkspaceSpec {
    WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: path.into(),
        mode: None,
    }
}

fn is_auto_integrate_refusal(err: &StoreError, name: &str) -> bool {
    matches!(
        err,
        StoreError::Repo(RepoError::AutoIntegrateNotLocalGit(n)) if n == name
    )
}

fn store_with_project() -> (SqliteStore, ProjectId) {
    let store = SqliteStore::open_in_memory().unwrap();
    let project = crate::Project {
        id: ProjectId::new(),
        title: "p".into(),
        request: "r".into(),
        status: crate::ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        auto_advance: false,
        slug: None,
        created_at: OffsetDateTime::now_utc(),
        updated_at: OffsetDateTime::now_utc(),
    };
    store.project_create(&project).unwrap();
    (store, project.id)
}

/// 不変条件: 取り込めない行（remote・dir）は auto_integrate = true を持てない。黙って false に落とさない。
/// store の検証が API・CoS・CLI のどの経路にも効く正本なので、ここで直接確かめる。
#[test]
fn store_refuses_auto_integrate_on_remote_and_dir_repos() {
    let (store, pid) = store_with_project();

    let mut remote_repo = repo(pid, "code", RepoKind::Git, remote("/work/code"));
    remote_repo.auto_integrate = true;
    let err = store.repo_create(&remote_repo).unwrap_err();
    assert!(is_auto_integrate_refusal(&err, "code"), "{err:?}");

    let mut dir_repo = repo(
        pid,
        "notes",
        RepoKind::Dir,
        WorkspaceSpec::local("/srv/notes"),
    );
    dir_repo.auto_integrate = true;
    let err = store.repo_create(&dir_repo).unwrap_err();
    assert!(is_auto_integrate_refusal(&err, "notes"), "{err:?}");
    assert!(
        store.repo_list(pid).unwrap().is_empty(),
        "拒否は何も書かない"
    );

    // local git は有効にでき、値が往復する。
    let mut paper = repo(
        pid,
        "paper",
        RepoKind::Git,
        WorkspaceSpec::local("/srv/paper"),
    );
    paper.auto_integrate = true;
    store.repo_create(&paper).unwrap();
    assert!(store.repo_get(paper.id).unwrap().unwrap().auto_integrate);

    // 更新: 有効な local git を remote に変える要求は、有効のままなら拒否（既存の値も拒否の対象）。
    let moved = ProjectRepo {
        location: remote("/work/paper"),
        ..paper.clone()
    };
    let err = store.repo_update(&moved).unwrap_err();
    assert!(is_auto_integrate_refusal(&err, "paper"), "{err:?}");
    assert!(
        matches!(
            store.repo_get(paper.id).unwrap().unwrap().location,
            WorkspaceSpec::Local { .. }
        ),
        "拒否した更新は書かれない"
    );
    // 同じ要求で無効にすれば通る。
    let moved_off = ProjectRepo {
        auto_integrate: false,
        ..moved
    };
    assert!(store.repo_update(&moved_off).unwrap());

    // 更新: remote の行を有効にする要求も拒否。
    let err = store
        .repo_update(&ProjectRepo {
            auto_integrate: true,
            ..moved_off
        })
        .unwrap_err();
    assert!(is_auto_integrate_refusal(&err, "paper"), "{err:?}");
}

/// 不変条件（D7）: auto_integrate = 1 の案件の local git の行だけが有効になり、remote・dir は 0 のまま、
/// 案件の列は消える。案件が無効なら全部 0。
#[test]
fn migration_0126_moves_project_flag_to_local_git_repos_and_drops_column() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pre126.sqlite3");
    let mut conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
    )
    .unwrap();
    for version in 1..=125 {
        if crate::migration_registry::migration_sql(version).is_none() {
            continue; // 予約された飛び番号
        }
        SqliteStore::apply_migration_version(&mut conn, version).unwrap();
    }
    let now = "2026-11-14T00:00:00Z";
    for (id, flag) in [("PON", 1), ("POFF", 0)] {
        conn.execute(
            "INSERT INTO projects (id, title, request, status, created_at, updated_at, auto_integrate) \
             VALUES (?1, 't', 'r', 'active', ?2, ?2, ?3)",
            rusqlite::params![id, now, flag],
        )
        .unwrap();
    }
    let rows = [
        (
            "R-on-local",
            "PON",
            "git",
            r#"{"kind":"local","path":"/srv/a"}"#,
        ),
        (
            "R-on-remote",
            "PON",
            "git",
            r#"{"kind":"remote","cluster":"c","path":"/w/a"}"#,
        ),
        (
            "R-on-dir",
            "PON",
            "dir",
            r#"{"kind":"local","path":"/srv/d"}"#,
        ),
        (
            "R-off-local",
            "POFF",
            "git",
            r#"{"kind":"local","path":"/srv/b"}"#,
        ),
    ];
    for (id, project, kind, location) in rows {
        conn.execute(
            "INSERT INTO project_repos (id, project_id, name, kind, location_json, run, is_primary, created_at) \
             VALUES (?1, ?2, ?1, ?3, ?4, 'auto', 0, ?5)",
            rusqlite::params![id, project, kind, location, now],
        )
        .unwrap();
    }
    conn.execute_batch("DELETE FROM schema_migrations WHERE version = 126;")
        .unwrap();
    drop(conn);

    let store = SqliteStore::open(&path).unwrap();
    assert_eq!(store.schema_version().unwrap(), crate::SCHEMA_VERSION);
    let conn = Connection::open(&path).unwrap();
    let flag = |id: &str| -> i64 {
        conn.query_row(
            "SELECT auto_integrate FROM project_repos WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(flag("R-on-local"), 1);
    assert_eq!(flag("R-on-remote"), 0);
    assert_eq!(flag("R-on-dir"), 0);
    assert_eq!(flag("R-off-local"), 0);
    let has_project_column: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('projects') WHERE name = 'auto_integrate'")
        .unwrap()
        .exists([])
        .unwrap();
    assert!(!has_project_column, "projects.auto_integrate は消える");
}

/// 不変条件（D4）: 旧い event の `project_not_enabled` は `repo_not_enabled` として読める（events は書き換えない）。
#[test]
fn old_project_not_enabled_reason_reads_as_repo_not_enabled() {
    use crate::DeliverySkipReason;
    let old: DeliverySkipReason = serde_json::from_str("\"project_not_enabled\"").unwrap();
    assert_eq!(old, DeliverySkipReason::RepoNotEnabled);
    let new: DeliverySkipReason = serde_json::from_str("\"repo_not_enabled\"").unwrap();
    assert_eq!(new, DeliverySkipReason::RepoNotEnabled);
    assert_eq!(serde_json::to_string(&new).unwrap(), "\"repo_not_enabled\"");
    assert_eq!(
        DeliverySkipReason::RepoNotEnabled.code(),
        "repo_not_enabled"
    );
}
