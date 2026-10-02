use super::*;
use task_core::{Budget, Check, Criterion, SqliteStore, Task, Tier, WorkerHint, WorkspaceSpec};
use time::OffsetDateTime;

fn sample_task(kind: TaskKind, status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        expected_write_paths: None,
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind,
        title: "t".to_string(),
        objective: "o".to_string(),
        acceptance: vec![Criterion {
            text: "tests pass".to_string(),
            check: Check::Command {
                cmd: "true".to_string(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "workspace".into(),
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
    }
}

#[test]
fn graph_without_root_includes_all_tasks_and_depends_on_edges() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let a = sample_task(TaskKind::Execute, Status::Done);
    store.insert(&a).expect("insert a");
    let mut b = sample_task(TaskKind::Execute, Status::Ready);
    b.depends_on = vec![a.id];
    store.insert(&b).expect("insert b");

    let g = graph(&store, None, None, true).expect("graph");
    assert_eq!(g.nodes.len(), 2);
    assert_eq!(g.edges.len(), 1);
    assert_eq!(g.edges[0].from, a.id);
    assert_eq!(g.edges[0].to, b.id);
    assert_eq!(g.edges[0].kind, "depends_on");
}

/// GUI-R2（ADR-0016 D1）: DAG のノードにも `role` が出る（詳細を N+1 で引かなくてよい）。
#[test]
fn graph_nodes_carry_the_role() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let mut lead = sample_task(TaskKind::Execute, Status::Running);
    lead.role = Some("lead".to_string());
    store.insert(&lead).expect("insert lead");
    let plain = sample_task(TaskKind::Execute, Status::Ready);
    store.insert(&plain).expect("insert plain");

    let g = graph(&store, None, None, true).expect("graph");
    let role_of = |id| {
        g.nodes
            .iter()
            .find(|n| n.id == id)
            .expect("in graph")
            .role
            .clone()
    };
    assert_eq!(role_of(lead.id).as_deref(), Some("lead"));
    assert_eq!(role_of(plain.id), None);
}

#[test]
fn graph_root_and_depth_limits_traversal() {
    let store = SqliteStore::open_in_memory().expect("open store");
    // chain: a <- b <- c <- d  (b depends_on a, c depends_on b, d depends_on c)
    let a = sample_task(TaskKind::Execute, Status::Done);
    store.insert(&a).expect("insert a");
    let mut b = sample_task(TaskKind::Execute, Status::Done);
    b.depends_on = vec![a.id];
    store.insert(&b).expect("insert b");
    let mut c = sample_task(TaskKind::Execute, Status::Ready);
    c.depends_on = vec![b.id];
    store.insert(&c).expect("insert c");
    let mut d = sample_task(TaskKind::Execute, Status::Draft);
    d.depends_on = vec![c.id];
    store.insert(&d).expect("insert d");

    // From c with depth 1: only b and d (1 hop away) plus c itself.
    let g = graph(&store, Some(c.id), Some(1), true).expect("graph");
    let ids: HashSet<TaskId> = g.nodes.iter().map(|n| n.id).collect();
    assert_eq!(ids, HashSet::from([b.id, c.id, d.id]));

    // Unlimited depth from c reaches the whole chain.
    let g_full = graph(&store, Some(c.id), None, true).expect("graph");
    let ids_full: HashSet<TaskId> = g_full.nodes.iter().map(|n| n.id).collect();
    assert_eq!(ids_full, HashSet::from([a.id, b.id, c.id, d.id]));
}

#[test]
fn graph_root_follows_parent_child_relation_too() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let parent = sample_task(TaskKind::Plan, Status::Done);
    store.insert(&parent).expect("insert parent");
    let mut child = sample_task(TaskKind::Execute, Status::Ready);
    child.parent_id = Some(parent.id);
    store.insert(&child).expect("insert child");

    let g = graph(&store, Some(parent.id), Some(1), true).expect("graph");
    let ids: HashSet<TaskId> = g.nodes.iter().map(|n| n.id).collect();
    assert_eq!(ids, HashSet::from([parent.id, child.id]));
}

#[test]
fn graph_include_terminal_false_drops_terminal_nodes_and_their_edges() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let done = sample_task(TaskKind::Execute, Status::Done);
    store.insert(&done).expect("insert done");
    let mut ready = sample_task(TaskKind::Execute, Status::Ready);
    ready.depends_on = vec![done.id];
    store.insert(&ready).expect("insert ready");

    let g = graph(&store, None, None, false).expect("graph");
    assert_eq!(g.nodes.len(), 1);
    assert_eq!(g.nodes[0].id, ready.id);
    assert!(
        g.edges.is_empty(),
        "edge to the excluded terminal node must be dropped too"
    );
}

#[test]
fn graph_root_not_found_errors() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let missing = TaskId::new();
    let result = graph(&store, Some(missing), None, true);
    assert!(matches!(result, Err(OpsError::NotFound(id)) if id == missing));
}

#[test]
fn graph_over_node_limit_is_a_validation_error() {
    let store = SqliteStore::open_in_memory().expect("open store");
    for _ in 0..(MAX_GRAPH_NODES + 1) {
        store
            .insert(&sample_task(TaskKind::Execute, Status::Draft))
            .expect("insert");
    }
    let result = graph(&store, None, None, true);
    assert!(matches!(result, Err(OpsError::Validation(_))));
}
