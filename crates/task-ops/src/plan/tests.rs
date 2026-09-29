use super::*;
use task_core::{Event, SqliteStore};

const DEFAULT_MAX_TURNS: u32 = 30;
const DEFAULT_MAX_WALL_SECS: u64 = 900;
const DEFAULT_MAX_RETRIES: u32 = 1;

fn base_spec(goal: &str) -> NewPlanSpec {
    NewPlanSpec {
        goal: goal.to_string(),
        workspace: Some(PathBuf::from("/tmp/workspace")),
        tier: Tier::Frontier,
        priority: 0,
        max_turns: DEFAULT_MAX_TURNS,
        max_wall_secs: DEFAULT_MAX_WALL_SECS,
        max_retries: DEFAULT_MAX_RETRIES,
    }
}

#[test]
fn create_plan_inserts_plan_task_and_created_event() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let goal = "add CLI argument parsing to hello-crate".to_string();
    let spec = base_spec(&goal);

    let task = create_plan(&store, spec, OffsetDateTime::now_utc()).expect("create_plan");

    assert_eq!(task.kind, TaskKind::Plan);
    assert_eq!(task.status, Status::Draft);
    assert!(task.acceptance.is_empty());
    assert_eq!(task.worker_hint.tier, Tier::Frontier);
    assert_eq!(task.worker_hint.adapter, None);
    assert_eq!(task.budget.max_turns, DEFAULT_MAX_TURNS);
    assert_eq!(task.budget.max_wall_secs, DEFAULT_MAX_WALL_SECS);
    assert_eq!(task.budget.max_retries, DEFAULT_MAX_RETRIES);
    assert_eq!(task.objective, goal);
    assert_eq!(task.title, goal);
    assert!(task.parent_id.is_none());

    let events = store.events_for(task.id).expect("events_for");
    assert_eq!(events.len(), 1);
    match &events[0].1 {
        Event::Created { task: created, .. } => assert_eq!(created.id, task.id),
        other => panic!("expected Created event, got {other:?}"),
    }
}

#[test]
fn create_plan_truncates_multiline_goal_title_but_keeps_full_objective() {
    let store = SqliteStore::open_in_memory().expect("open store");
    // 1行目にマルチバイト文字を含み、80文字を超える長さにする。
    let first_line: String = "目".repeat(90);
    let goal = format!("{first_line}\nsecond line\nthird line");
    let spec = base_spec(&goal);

    let task = create_plan(&store, spec, OffsetDateTime::now_utc()).expect("create_plan");

    let expected_title: String = first_line.chars().take(TITLE_MAX_CHARS).collect();
    assert_eq!(task.title, expected_title);
    assert_eq!(task.title.chars().count(), TITLE_MAX_CHARS);
    assert_eq!(task.objective, goal);
}

#[test]
fn create_plan_with_blank_goal_returns_error() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let spec = base_spec("   \n  \t ");

    let result = create_plan(&store, spec, OffsetDateTime::now_utc());
    assert!(matches!(result, Err(OpsError::Validation(_))));
}

#[test]
fn create_plan_with_custom_tier_priority_and_retries() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec("do something useful");
    spec.tier = Tier::Standard;
    spec.max_retries = 0;
    spec.priority = 5;

    let task = create_plan(&store, spec, OffsetDateTime::now_utc()).expect("create_plan");
    assert_eq!(task.worker_hint.tier, Tier::Standard);
    assert_eq!(task.budget.max_retries, 0);
    assert_eq!(task.priority, 5);
}

/// P-19: `workspace` 省略時は `<task_id>`（相対パス）になる。
#[test]
fn create_plan_without_workspace_defaults_to_relative_task_id_path() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut spec = base_spec("do something useful");
    spec.workspace = None;

    let task = create_plan(&store, spec, OffsetDateTime::now_utc()).expect("create_plan");
    assert_eq!(
        task.workspace,
        WorkspaceSpec::Local {
            path: PathBuf::from(task.id.to_string()),
            mode: None
        }
    );
}
