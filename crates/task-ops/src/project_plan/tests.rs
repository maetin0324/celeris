use super::*;
use task_core::SqliteStore;
use time::OffsetDateTime;

/// ADR-0079 D13（Phase R5a）: 案件計画の版が無い案件（本番のすべて）では DAG は `None`、版の一覧は空。
#[test]
fn a_project_without_plan_versions_has_no_dag() {
    let store = SqliteStore::open_in_memory().expect("open");
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "案件".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).expect("create");
    assert!(
        plan_state(&store, project.id)
            .expect("state")
            .versions
            .is_empty()
    );
    assert!(dag_view(&store, &project).expect("dag").is_none());
}
