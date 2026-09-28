//! ADR-0079 §7 R1c / D6: 子 task のブランチ `celeris/<child_id>` は親の段階の基点（同じ段階の依存先が
//! あればその HEAD）から切られ、親の段階末尾の統合 WU が親のブランチへ merge する（`PhaseIntegrated.merged`
//! に子が残る）。子の最終レビューは main ではなく親のブランチと比べる。孫（深さ 3）は子へ、子は root へ。
//! すべて一時ディレクトリの git と偽のアダプタだけで、外部ネットワークに出ない。

use super::*;

/// planner run には計画の列を順に書く。WU の run は `<key>.txt` を、子 task の run（計画を持たない task の
/// run = `work_unit` の無い run）は `<題名の Child の後>.txt` を作業ツリーに書く（commit はしない: 子の done で
/// daemon が決定的に commit する）。`child_actions` は子の run の中で走らせる作業（main を動かすなど）。
struct GitTreeAdapter {
    plans: StdMutex<std::collections::VecDeque<String>>,
    /// `(key, cwd)`（WU は unit の key、子は題名の key）。
    seen: StdMutex<Vec<(String, PathBuf)>>,
    child_actions: HashMap<String, WuAction>,
}

impl GitTreeAdapter {
    fn new(plans: Vec<String>) -> Self {
        GitTreeAdapter {
            plans: StdMutex::new(plans.into_iter().collect()),
            seen: StdMutex::new(Vec::new()),
            child_actions: HashMap::new(),
        }
    }

    fn with_child_action(
        mut self,
        key: &str,
        action: impl Fn(&std::path::Path) + Send + Sync + 'static,
    ) -> Self {
        self.child_actions.insert(key.to_string(), Box::new(action));
        self
    }

    fn cwd_of(&self, key: &str) -> PathBuf {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, cwd)| cwd.clone())
            .unwrap_or_else(|| panic!("{key} never ran"))
    }
}

#[async_trait]
impl WorkerAdapter for GitTreeAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let cwd = req
            .work_dir
            .clone()
            .unwrap_or_else(|| req.workspace.clone());
        if req.context.execution_planner.is_some() {
            if let Some(json) = self.plans.lock().unwrap().pop_front() {
                std::fs::create_dir_all(&req.artifacts_dir).unwrap();
                std::fs::write(req.artifacts_dir.join("execution-plan.json"), json).unwrap();
            }
        } else {
            let key = match &req.context.work_unit {
                Some(w) => w.key.clone(),
                None => req
                    .task
                    .title
                    .strip_prefix("Child ")
                    .unwrap_or("atomic")
                    .to_string(),
            };
            std::fs::write(cwd.join(format!("{key}.txt")), &key).unwrap();
            if req.context.work_unit.is_none()
                && let Some(action) = self.child_actions.get(&key)
            {
                action(&cwd);
            }
            self.seen.lock().unwrap().push((key, cwd));
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

fn leaf(key: &str, stage: &str, deps: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "stage": stage,
        "kind": "implement",
        "title": format!("Leaf {key}"),
        "objective": format!("Implement {key} thoroughly and completely"),
        "depends_on": deps,
        "done_when": [format!("{key} is done")],
        "checks": [{"cmd": format!("test -f {key}.txt"), "expect_exit": 0}],
    })
}

fn task_unit(key: &str, stage: &str, deps: &[&str], acceptance_cmd: &str) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "stage": stage,
        "kind": "task",
        "title": format!("Child {key}"),
        "objective": format!("Deliver the {key} part as its own reviewed task"),
        "depends_on": deps,
        "acceptance": [serde_json::to_value(task_core::Criterion {
            text: format!("{acceptance_cmd} passes"),
            check: Check::Command {
                cmd: acceptance_cmd.into(),
                expect_exit: 0,
            },
        })
        .unwrap()],
    })
}

/// 子 task の gate を compound に倒す（ADR-0072 D13 の強制規則 `expected_length=high` かつ
/// `cross_cutting=high`）。孫を持つ子（深さ 2 の計画）の試験用。
fn compound_task_unit(key: &str, stage: &str) -> serde_json::Value {
    let mut unit = task_unit(key, stage, &[], "true");
    unit["features"] = serde_json::json!({"expected_length": "high", "cross_cutting": "high"});
    unit
}

fn stage(key: &str) -> serde_json::Value {
    serde_json::json!({"key": key, "kind": "implement", "title": format!("Stage {key}")})
}

fn v3_plan(stages: Vec<serde_json::Value>, units: Vec<serde_json::Value>) -> String {
    serde_json::json!({
        "schema": task_core::EXECUTION_PLAN_SCHEMA_V3,
        "rationale": "the child part is reviewed on its own",
        "stages": stages,
        "units": units,
    })
    .to_string()
}

/// 一時 git リポジトリの上の compound の root task（ADR-0041 D1 の 1 リポジトリの worktree
/// `<workspace_root>/<id>/tree`、ブランチ `celeris/<id>`）。
fn git_root(repo: &std::path::Path) -> Task {
    let mut task = git_task(
        repo,
        None,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
    );
    task.routing = Some(task_core::TaskRouting {
        execution_hint: Some(task_core::ExecutionHintSpec {
            mode: task_core::ExecutionMode::Compound,
            explicit: true,
        }),
        ..Default::default()
    });
    task
}

fn git_tree_dispatcher(
    store: &Arc<dyn TaskStore>,
    adapter: Arc<dyn WorkerAdapter>,
    workspace_root: &std::path::Path,
) -> Dispatcher {
    let mut d = parallel_dispatcher(store.clone(), adapter, workspace_root, 3, 3, 3);
    d.config.execution.gate = task_core::GateMode::On;
    d.config.execution.planner.adapter = "instant".to_string();
    let mut limits = task_core::ExecutionLimits::default();
    limits.tree.enabled = true;
    d.config.execution.limits = limits;
    d
}

fn unit<'a>(units: &'a [task_core::WorkUnitRow], key: &str) -> &'a task_core::WorkUnitRow {
    units
        .iter()
        .find(|u| u.key == key)
        .unwrap_or_else(|| panic!("no unit {key}: {units:?}"))
}

fn child_of(store: &Arc<dyn TaskStore>, parent: TaskId, key: &str) -> Task {
    let units = store.work_units_for(parent).unwrap();
    let id: TaskId = unit(&units, key)
        .child_task_id
        .as_deref()
        .unwrap_or_else(|| panic!("unit {key} has no child"))
        .parse()
        .unwrap();
    store.get(id).unwrap().unwrap()
}

/// `PhaseIntegrated{phase}` の `merged`（`(key, commit, skipped)`）。
fn merged_of(store: &Arc<dyn TaskStore>, task: TaskId, phase: &str) -> Vec<(String, String, bool)> {
    store
        .events_for(task)
        .unwrap()
        .into_iter()
        .find_map(|(_, e)| match e {
            Event::PhaseIntegrated {
                phase: p, merged, ..
            } if p == phase => Some(
                merged
                    .into_iter()
                    .map(|m| (m.key, m.commit, m.skipped))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{task} never integrated {phase}"))
}

fn branch_head(repo: &std::path::Path, branch: &str) -> String {
    crate::integration::rev_parse(repo, &format!("refs/heads/{branch}"))
        .unwrap_or_else(|| panic!("no branch {branch}"))
}

/// `branch` の木に `path` があるか（`git cat-file -e <branch>:<path>`）。
fn branch_has(repo: &std::path::Path, branch: &str, path: &str) -> bool {
    git_ok(repo, &["cat-file", "-e", &format!("{branch}:{path}")])
}

fn assert_replay_is_clean(store: &Arc<dyn TaskStore>) {
    let report = task_ops::replay::replay(store.as_ref()).unwrap();
    assert!(report.mismatches.is_empty(), "{:?}", report.mismatches);
    let (wu, _runs, plans, _) =
        task_ops::replay::check_and_apply_execution(store.as_ref(), false).unwrap();
    assert!(wu.is_empty(), "{wu:?}");
    assert!(plans.is_empty(), "{plans:?}");
}

/// R1c (a): 段階 s1 = leaf a、段階 s2 = leaf b + 子 c。c のブランチ `celeris/<c>` は s2 の基点（s1 の統合後の
/// 親ブランチの HEAD）から切られ、`integrate-s2` が b と c を親ブランチに merge し、`PhaseIntegrated.merged`
/// に c と子のブランチの HEAD が残る。子の worktree は統合の後で消え、ブランチは残る。root だけが done で
/// 終わり（子の done で main は動かない）、replay は unit の `head_commit` / `base_commit` を同じに作り直す。
#[tokio::test]
async fn integration_merges_child_task_branch() {
    let repo = tempfile::tempdir().unwrap();
    let main_sha = init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = git_root(repo.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1"), stage("s2")],
        vec![
            leaf("a", "s1", &[]),
            leaf("b", "s2", &[]),
            task_unit("c", "s2", &[], "test -f a.txt && test -f c.txt"),
        ],
    );
    let adapter = Arc::new(GitTreeAdapter::new(vec![plan]));
    let mut d = git_tree_dispatcher(&store, adapter.clone(), ws.path());
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    let stored = store.get(root_id).unwrap().unwrap();
    assert_eq!(
        stored.status,
        Status::Done,
        "{:?}",
        store.events_for(root_id).unwrap()
    );

    let root_branch = format!("celeris/{root_id}");
    let units = store.work_units_for(root_id).unwrap();
    let s1_head = unit(&units, "integrate-s1")
        .integrated_commit
        .clone()
        .expect("s1 integrated");
    let child = child_of(&store, root_id, "c");
    assert_eq!(child.status, Status::Done);
    let tree = child.tree.as_ref().expect("tree");
    assert_eq!(
        tree.base_commit.as_deref(),
        Some(s1_head.as_str()),
        "the child is cut from the stage base (the parent's branch after s1)"
    );
    // 子のブランチは子自身のもの（`celeris/<child_id>`）で、段階の基点の上にある。
    let child_branch = format!("celeris/{}", child.id);
    let child_head = branch_head(repo.path(), &child_branch);
    assert!(crate::integration::is_ancestor(
        repo.path(),
        &s1_head,
        &child_head
    ));
    assert!(branch_has(repo.path(), &child_branch, "c.txt"));
    assert!(
        branch_has(repo.path(), &child_branch, "a.txt"),
        "s1's result is in the child's base"
    );
    // 子は自分の作業場所（task の作業場所の規則 `<workspace_root>/<child_id>/tree`）で走った。
    assert_eq!(
        adapter.cwd_of("c"),
        ws.path().join(child.id.to_string()).join("tree")
    );
    let c = unit(&units, "c");
    assert_eq!(c.head_commit.as_deref(), Some(child_head.as_str()));
    assert_eq!(c.base_commit.as_deref(), Some(s1_head.as_str()));
    assert_eq!(c.branch, None, "a task unit has no WU worktree");

    // integrate-s2 が b と c を merge した（seq 順）。c の commit は子のブランチの HEAD。
    let merged = merged_of(&store, root_id, "s2");
    assert_eq!(
        merged
            .iter()
            .map(|(k, _, skipped)| (k.as_str(), *skipped))
            .collect::<Vec<_>>(),
        vec![("b", false), ("c", false)],
        "{merged:?}"
    );
    assert_eq!(merged[1].1, child_head);
    for f in ["a.txt", "b.txt", "c.txt"] {
        assert!(branch_has(repo.path(), &root_branch, f), "{f}");
    }
    assert!(crate::integration::is_ancestor(
        repo.path(),
        &child_head,
        &root_branch
    ));
    // 子の worktree は消え、ブランチは残る（監査のため root の終端まで）。
    assert!(!adapter.cwd_of("c").exists());
    assert!(
        crate::integration::rev_parse(repo.path(), &format!("refs/heads/{child_branch}")).is_some()
    );
    // main は動かない（子も root も main に取り込まない: 取り込みは人か ADR-0051）。
    assert_eq!(branch_head(repo.path(), "main"), main_sha);
    // 子の done の commit の記録（`WorkUnitCommitted`）。
    assert!(
        store
            .events_for(root_id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(
                e,
                Event::WorkUnitCommitted { key, branch, commit, base, .. }
                    if key == "c" && *branch == child_branch && *commit == child_head
                        && base.as_deref() == Some(s1_head.as_str())
            ))
    );
    assert_replay_is_clean(&store);

    // replay: 子の done の記録（unit の `head_commit` / `base_commit`）は events だけから作り直せる。
    let original = store.work_units_for(root_id).unwrap();
    let broken: Vec<task_core::WorkUnitRow> = original
        .iter()
        .cloned()
        .map(|mut u| {
            if u.key == "c" {
                u.head_commit = None;
                u.base_commit = None;
            }
            u
        })
        .collect();
    store.work_units_replace(root_id, broken).unwrap();
    let (wu, _, _, _) = task_ops::replay::check_and_apply_execution(store.as_ref(), false).unwrap();
    assert!(
        wu.iter().any(|m| m.key == "c" && m.field == "head_commit"),
        "{wu:?}"
    );
    task_ops::replay::check_and_apply_execution(store.as_ref(), true).unwrap();
    let restored = store.work_units_for(root_id).unwrap();
    let c = unit(&restored, "c");
    assert_eq!(c.head_commit.as_deref(), Some(child_head.as_str()));
    assert_eq!(c.base_commit.as_deref(), Some(s1_head.as_str()));
    assert_replay_is_clean(&store);
}

/// R1c (b): 同じ段階で子 c に依存する leaf b は、c のブランチの HEAD から切られる（`dependency_base` を
/// 子のブランチに広げた）。統合は c と b の両方を `merged` に残す（b が c を含むので c の commit は 1 度だけ入る）。
#[tokio::test]
async fn same_stage_dependency_on_child_bases_on_child_head() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = git_root(repo.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1")],
        vec![
            task_unit("c", "s1", &[], "test -f c.txt"),
            leaf("b", "s1", &["c"]),
        ],
    );
    let adapter = Arc::new(GitTreeAdapter::new(vec![plan]));
    let mut d = git_tree_dispatcher(&store, adapter.clone(), ws.path());
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    let units = store.work_units_for(root_id).unwrap();
    let c = unit(&units, "c");
    let b = unit(&units, "b");
    let child_head = c.head_commit.clone().expect("child head recorded");
    assert_eq!(
        b.base_commit.as_deref(),
        Some(child_head.as_str()),
        "b stacks on the child's HEAD"
    );
    let merged = merged_of(&store, root_id, "s1");
    let keys: Vec<(&str, bool)> = merged
        .iter()
        .map(|(k, _, skipped)| (k.as_str(), *skipped))
        .collect();
    assert_eq!(keys, vec![("c", false), ("b", false)], "{merged:?}");
    let root_branch = format!("celeris/{root_id}");
    assert!(branch_has(repo.path(), &root_branch, "c.txt"));
    assert!(branch_has(repo.path(), &root_branch, "b.txt"));
    assert_replay_is_clean(&store);
}

/// R1c (c): 子の最終レビュー中に main が進んでも、子の `git merge-base --is-ancestor main HEAD` は
/// 親のブランチと比べるので落ちず、main の commit は子にも親にも入らない（今日の merge_base のずれの形）。
/// root は今どおり（main とは比べない検査なので main は動かない）。
#[tokio::test]
async fn child_review_ignores_main_moving() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = git_root(repo.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let plan = v3_plan(
        vec![stage("s1")],
        vec![task_unit(
            "c",
            "s1",
            &[],
            "git merge-base --is-ancestor main HEAD",
        )],
    );
    let source = repo.path().to_path_buf();
    let adapter = Arc::new(
        GitTreeAdapter::new(vec![plan]).with_child_action("c", move |_cwd| {
            // 子が走っている間に main が進む（人が別の変更を main に入れた）。
            std::fs::write(source.join("elsewhere.txt"), "moved").unwrap();
            assert!(git_ok(&source, &["add", "elsewhere.txt"]));
            assert!(git_ok(&source, &["commit", "-q", "-m", "main moves"]));
        }),
    );
    let mut d = git_tree_dispatcher(&store, adapter, ws.path());
    let report = run_until_idle(&mut d, 1500).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(store.get(root_id).unwrap().unwrap().status, Status::Done);
    let child = child_of(&store, root_id, "c");
    assert_eq!(child.status, Status::Done);
    let main_now = branch_head(repo.path(), "main");
    let child_branch = format!("celeris/{}", child.id);
    let root_branch = format!("celeris/{root_id}");
    // 子の検査は親のブランチと比べた（判定の理由に親のブランチ）。1 回目で合格（repair も merge も無い）。
    let verdicts: Vec<(bool, String)> = store
        .events_for(child.id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::ReviewVerdict { pass, reason, .. } => Some((pass, reason)),
            _ => None,
        })
        .collect();
    assert_eq!(verdicts.len(), 1, "{verdicts:?}");
    assert!(verdicts[0].0, "{verdicts:?}");
    assert!(
        verdicts[0]
            .1
            .contains(&format!("--is-ancestor {root_branch} HEAD")),
        "{verdicts:?}"
    );
    // main の新しい commit は子にも親にも入っていない（子は main を merge していない）。
    assert!(!crate::integration::is_ancestor(
        repo.path(),
        &main_now,
        &child_branch
    ));
    assert!(!crate::integration::is_ancestor(
        repo.path(),
        &main_now,
        &root_branch
    ));
    assert!(branch_has(repo.path(), &root_branch, "c.txt"));
    // 子の元の条件（task の JSON）は書き換えない（レビューの view だけ）。
    assert_eq!(
        child.acceptance[0].check,
        Check::Command {
            cmd: "git merge-base --is-ancestor main HEAD".into(),
            expect_exit: 0,
        }
    );
    assert_replay_is_clean(&store);
}

/// R1c: 孫（深さ 3）は子（深さ 2）のブランチへ、子は root のブランチへ取り込まれる。孫の基点は子の段階の
/// 基点（子のブランチの HEAD）、子の統合の `merged` に孫、root の統合の `merged` に子。
#[tokio::test]
async fn grandchild_integrates_into_child_which_integrates_into_root() {
    let repo = tempfile::tempdir().unwrap();
    let main_sha = init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let root = git_root(repo.path());
    let root_id = root.id;
    store.create_task(&root, vec![]).unwrap();
    let root_plan = v3_plan(vec![stage("s1")], vec![compound_task_unit("c", "s1")]);
    let child_plan = v3_plan(
        vec![stage("t1")],
        vec![
            leaf("l", "t1", &[]),
            task_unit("g", "t1", &[], "test -f g.txt"),
        ],
    );
    let adapter = Arc::new(GitTreeAdapter::new(vec![root_plan, child_plan]));
    let mut d = git_tree_dispatcher(&store, adapter, ws.path());
    let report = run_until_idle(&mut d, 3000).await;
    assert!(report.idle, "{report:?}");
    assert_eq!(
        store.get(root_id).unwrap().unwrap().status,
        Status::Done,
        "{:?}",
        store.events_for(root_id).unwrap()
    );
    let child = child_of(&store, root_id, "c");
    assert_eq!(child.status, Status::Done);
    assert!(
        store.execution_plan_active(child.id).unwrap().is_some(),
        "the child is compound (its own plan)"
    );
    let grandchild = child_of(&store, child.id, "g");
    assert_eq!(grandchild.status, Status::Done);
    let g_tree = grandchild.tree.as_ref().unwrap();
    assert_eq!(g_tree.depth, 3);
    assert_eq!(g_tree.root_id, root_id);
    let child_branch = format!("celeris/{}", child.id);
    let root_branch = format!("celeris/{root_id}");
    let g_branch = format!("celeris/{}", grandchild.id);
    // 孫の基点は子の段階の基点（子のブランチの HEAD。子のブランチは root の段階の基点 = main から）。
    let child_base = child.tree.as_ref().unwrap().base_commit.clone().unwrap();
    assert_eq!(
        child_base, main_sha,
        "root's first stage base is its branch head = main"
    );
    let g_base = g_tree.base_commit.clone().unwrap();
    assert!(crate::integration::is_ancestor(
        repo.path(),
        &child_base,
        &g_base
    ));
    // 子の統合の merged に孫、root の統合の merged に子。
    let child_merged = merged_of(&store, child.id, "t1");
    assert!(
        child_merged
            .iter()
            .any(|(k, c, _)| k == "g" && *c == branch_head(repo.path(), &g_branch)),
        "{child_merged:?}"
    );
    assert!(
        child_merged.iter().any(|(k, _, _)| k == "l"),
        "{child_merged:?}"
    );
    let root_merged = merged_of(&store, root_id, "s1");
    assert_eq!(root_merged.len(), 1, "{root_merged:?}");
    assert_eq!(root_merged[0].0, "c");
    assert_eq!(root_merged[0].1, branch_head(repo.path(), &child_branch));
    for f in ["g.txt", "l.txt"] {
        assert!(branch_has(repo.path(), &child_branch, f), "{f}");
        assert!(branch_has(repo.path(), &root_branch, f), "{f}");
    }
    assert!(crate::integration::is_ancestor(
        repo.path(),
        &g_branch,
        &child_branch
    ));
    assert!(crate::integration::is_ancestor(
        repo.path(),
        &child_branch,
        &root_branch
    ));
    assert_eq!(
        branch_head(repo.path(), "main"),
        main_sha,
        "only the human / ADR-0051 moves main"
    );
    assert_replay_is_clean(&store);
}

/// R1c: 木の子の worktree の base は `BaseKind::Parent`（`tree.base_commit`）、root と木でない task は今どおり
/// main（`BaseKind::Main`）。base_commit がこのリポジトリに無ければ親のブランチの HEAD。
#[test]
fn worktree_base_of_a_tree_child_is_the_parent_base() {
    let repo = tempfile::tempdir().unwrap();
    let main_sha = init_test_repo(repo.path());
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(GitTreeAdapter::new(Vec::new()));
    let d = git_tree_dispatcher(&store, adapter, ws.path());
    let root = git_root(repo.path());
    let base = d.worktree_base_for(&root, repo.path()).unwrap();
    assert_eq!(
        (base.kind, base.sha.as_str()),
        (task_worker::BaseKind::Main, main_sha.as_str())
    );
    // 親のブランチに 1 commit 足したものを子の基点にする。
    assert!(git_ok(
        repo.path(),
        &["branch", &format!("celeris/{}", root.id)]
    ));
    let wt = repo.path().join("parent-wt");
    assert!(git_ok(
        repo.path(),
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            &format!("celeris/{}", root.id)
        ]
    ));
    std::fs::write(wt.join("p.txt"), "p").unwrap();
    let (parent_head, _) = crate::integration::commit_all(&wt, "parent work").unwrap();
    let mut child = git_root(repo.path());
    child.tree = Some(task_core::TreeInfo::child_of(
        &root,
        task_core::ParentUnit {
            task_id: root.id,
            plan_id: "plan".into(),
            unit_key: "c".into(),
            stage: "s1".into(),
        },
        Some(main_sha.clone()),
    ));
    let base = d.worktree_base_for(&child, repo.path()).unwrap();
    assert_eq!(
        (base.kind, base.sha.as_str()),
        (task_worker::BaseKind::Parent, main_sha.as_str())
    );
    // 基点がこのリポジトリで解決できない（別のリポジトリの sha）: 親のブランチの HEAD。
    child.tree.as_mut().unwrap().base_commit =
        Some("0123456789abcdef0123456789abcdef01234567".into());
    let base = d.worktree_base_for(&child, repo.path()).unwrap();
    assert_eq!(
        (base.kind, base.sha.as_str()),
        (task_worker::BaseKind::Parent, parent_head.as_str())
    );
}
