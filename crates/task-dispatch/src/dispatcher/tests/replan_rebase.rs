//! ADR 2026-10-20-replan-dep-change-wu-rebase: replan で同じ段の依存先が変わった持ち越し WU は、次の
//! prepare で旧い WU ブランチを `-superseded-<sha12>` へ退避し、新しい起点（依存先の HEAD / Task ブランチの
//! HEAD）から作り直す。依存先が変わらない持ち越しはブランチと base を使い回す。実 git リポジトリだけを
//! 使い、LLM・外部ネットワーク・sleep に頼らない。

use super::*;

struct Fixture {
    _repo: tempfile::TempDir,
    _root: tempfile::TempDir,
    repo: std::path::PathBuf,
    store: Arc<dyn TaskStore>,
    task: Task,
    d: Dispatcher,
    ws: task_worker::TaskWorkspaces,
}

impl Fixture {
    /// 同じ段 `build` に a・b（依存なし）と c（`c_deps` に依存）を置く。
    fn new(c_deps: &[&str]) -> Self {
        let repo = tempfile::tempdir().unwrap();
        init_test_repo(repo.path());
        let root = tempfile::tempdir().unwrap();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let task = parallel_task(repo.path(), "true");
        store.insert(&task).unwrap();
        adopt_v2_plan(
            &store,
            task.id,
            &["build"],
            vec![
                v2_wu("a", "build", &[]),
                v2_wu("b", "build", &[]),
                v2_wu("c", "build", c_deps),
            ],
        );
        let adapter = Arc::new(ParallelWuAdapter::new(Duration::from_millis(20)));
        let d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
        let ws = d.task_workspaces_for(&task).expect("task workspaces");
        Fixture {
            repo: repo.path().to_path_buf(),
            _repo: repo,
            _root: root,
            store,
            task,
            d,
            ws,
        }
    }

    fn row(&self, key: &str) -> task_core::WorkUnitRow {
        self.store
            .work_units_for(self.task.id)
            .unwrap()
            .into_iter()
            .find(|u| u.key == key)
            .unwrap()
    }

    fn put(&self, row: task_core::WorkUnitRow) {
        let ev = Event::WorkUnitTransitioned {
            work_unit_id: row.id.clone(),
            key: row.key.clone(),
            from: row.status,
            to: row.status,
            reason: "fixture".into(),
            run_id: None,
        };
        self.store
            .work_unit_transition(self.task.id, row, ev)
            .unwrap();
    }

    fn worktree(&self, key: &str) -> std::path::PathBuf {
        crate::integration::wu_dir(&self.ws.task_dir, key)
            .join(task_worker::task_repos::REPOS_DIR_NAME)
            .join(&self.ws.repos[0].name)
    }

    fn branch(&self, key: &str) -> String {
        crate::integration::wu_branch(&self.task.id.to_string(), key)
    }

    /// prepare して、dispatch と同じく行に branch・base_commit を書く。戻りは base。
    fn prepare(&self, key: &str) -> String {
        let row = self.row(key);
        let prepared = self
            .d
            .prepare_work_unit_workspace(&self.task, &row, Some(&self.ws))
            .unwrap_or_else(|e| panic!("prepare {key}: {e:?}"))
            .expect("a work unit worktree");
        let mut row = self.row(key);
        row.branch = Some(prepared.branch.clone());
        row.base_commit = Some(prepared.base.clone());
        row.rebase_pending = false;
        self.put(row);
        prepared.base
    }

    /// WU を prepare し、worktree に 1 commit 積んで done にする。戻りは HEAD。
    fn run_with_commit(&self, key: &str) -> String {
        self.prepare(key);
        let wt = self.worktree(key);
        std::fs::write(wt.join(format!("{key}.txt")), key).unwrap();
        assert!(git_ok(&wt, &["add", "-A"]));
        assert!(git_ok(
            &wt,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-q",
                "-m",
                key
            ]
        ));
        let head = git_out(&wt, &["rev-parse", "HEAD"]);
        let mut row = self.row(key);
        row.status = task_core::WorkUnitStatus::Done;
        row.head_commit = Some(head.clone());
        self.put(row);
        head
    }

    /// replan の持ち越しと同じ形に c の行を書き換える（execution.rs: 同段依存が変われば branch・base・head を
    /// 消して `rebase_pending`）。
    fn replan_c(&self, new_deps: &[&str]) {
        let mut row = self.row("c");
        let old = row.depends_on.clone();
        let new: Vec<String> = new_deps.iter().map(|s| s.to_string()).collect();
        let phase_of = |k: &str| -> Option<String> {
            ["a", "b", "c"].contains(&k).then(|| "build".to_string())
        };
        let changed = task_core::same_phase_dependencies_changed(
            &old,
            Some("build"),
            &phase_of,
            &new,
            Some("build"),
            &phase_of,
        );
        row.depends_on = new;
        if changed {
            row.branch = None;
            row.base_commit = None;
            row.head_commit = None;
            row.rebase_pending = true;
        }
        self.put(row);
    }

    fn is_ancestor(&self, key: &str, commit: &str) -> bool {
        git_ok(
            &self.worktree(key),
            &["merge-base", "--is-ancestor", commit, "HEAD"],
        )
    }

    fn superseded_branches(&self, key: &str) -> Vec<String> {
        let out = git_out(
            &self.repo,
            &[
                "for-each-ref",
                "--format=%(refname:short)",
                &format!("refs/heads/{}-superseded-*", self.branch(key)),
            ],
        );
        out.lines().map(str::to_string).collect()
    }
}

/// 受け入れ条件 1: 同じ段の c の依存先を a から b へ変えると、次の prepare で c は b の HEAD から作り直され、
/// 旧い試行のブランチは `-superseded-<sha12>` に残る。
#[test]
fn a_changed_same_phase_dependency_rebuilds_the_unit_from_the_new_dependency() {
    let f = Fixture::new(&["a"]);
    let a_head = f.run_with_commit("a");
    let b_head = f.run_with_commit("b");
    let first = f.prepare("c");
    assert_eq!(first, a_head, "最初の c は a の HEAD から切られる");
    let old_c_head = git_out(&f.worktree("c"), &["rev-parse", "HEAD"]);

    f.replan_c(&["b"]);
    assert!(f.row("c").rebase_pending);
    let base = f.prepare("c");
    assert_eq!(base, b_head, "CELERIS_WU_BASE は新しい依存先 b の HEAD");
    let c = f.row("c");
    assert_eq!(c.base_commit.as_deref(), Some(b_head.as_str()));
    assert!(!c.rebase_pending);
    assert!(
        f.is_ancestor("c", &b_head),
        "c の worktree は b の HEAD を祖先に持つ"
    );
    assert!(
        !f.is_ancestor("c", &a_head),
        "旧い依存先 a の成果は入らない"
    );
    assert!(f.worktree("c").join("b.txt").is_file());
    let branch_head = git_out(
        &f.repo,
        &["rev-parse", &format!("refs/heads/{}", f.branch("c"))],
    );
    assert_eq!(branch_head, b_head);
    // 旧い試行は退避ブランチから追える。
    let archived = f.superseded_branches("c");
    assert_eq!(archived.len(), 1, "{archived:?}");
    assert_eq!(
        git_out(&f.repo, &["rev-parse", &archived[0]]),
        old_c_head,
        "退避ブランチは旧い c の HEAD を指す"
    );
}

/// 受け入れ条件 2: 依存先が変わらない持ち越しはブランチと base_commit を使い回す。
#[test]
fn an_unchanged_dependency_reuses_the_branch_and_base() {
    let f = Fixture::new(&["a"]);
    let a_head = f.run_with_commit("a");
    f.run_with_commit("b");
    assert_eq!(f.prepare("c"), a_head);
    // c の前の試行が commit を積んでいた。
    let wt = f.worktree("c");
    std::fs::write(wt.join("c.txt"), "c").unwrap();
    assert!(git_ok(&wt, &["add", "-A"]));
    assert!(git_ok(
        &wt,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "-m",
            "c"
        ]
    ));
    let c_head = git_out(&wt, &["rev-parse", "HEAD"]);

    f.replan_c(&["a"]);
    let c = f.row("c");
    assert!(!c.rebase_pending);
    assert_eq!(c.base_commit.as_deref(), Some(a_head.as_str()));
    assert_eq!(f.prepare("c"), a_head, "base は使い回す");
    assert_eq!(
        git_out(&wt, &["rev-parse", "HEAD"]),
        c_head,
        "前の試行の commit が残る"
    );
    assert!(f.superseded_branches("c").is_empty());
}

/// 受け入れ条件 3: 依存が外れた（あり → なし）WU は Task ブランチの HEAD から作り直される。
#[test]
fn a_dropped_dependency_rebuilds_the_unit_from_the_task_branch() {
    let f = Fixture::new(&["a"]);
    let a_head = f.run_with_commit("a");
    assert_eq!(f.prepare("c"), a_head);
    let task_branch = f.ws.repos[0].worktree.as_ref().unwrap().branch.clone();
    let task_head = git_out(
        &f.repo,
        &["rev-parse", &format!("refs/heads/{task_branch}")],
    );
    assert_ne!(task_head, a_head);

    f.replan_c(&[]);
    assert!(f.row("c").rebase_pending);
    let base = f.prepare("c");
    assert_eq!(base, task_head, "依存が無ければ Task ブランチの HEAD");
    assert_eq!(f.row("c").base_commit.as_deref(), Some(task_head.as_str()));
    assert!(!f.is_ancestor("c", &a_head));
    assert_eq!(git_out(&f.worktree("c"), &["rev-parse", "HEAD"]), task_head);
    assert_eq!(f.superseded_branches("c").len(), 1);
}

/// 判定関数: 同じ段の依存先の集合だけを比べる。別の段の依存の増減・順序の入れ替えは「変わらない」。
#[test]
fn same_phase_dependencies_changed_compares_only_same_phase_sets() {
    let phase_of = |k: &str| -> Option<String> {
        match k {
            "a" | "b" | "c" => Some("build".into()),
            "x" => Some("design".into()),
            _ => None,
        }
    };
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let changed = |old: &[&str], new: &[&str]| {
        task_core::same_phase_dependencies_changed(
            &s(old),
            Some("build"),
            &phase_of,
            &s(new),
            Some("build"),
            &phase_of,
        )
    };
    assert!(changed(&["a"], &["b"]));
    assert!(changed(&["a"], &[]));
    assert!(changed(&[], &["a"]));
    assert!(changed(&["a"], &["a", "b"]));
    assert!(!changed(&["a"], &["a"]));
    assert!(!changed(&["a", "b"], &["b", "a"]));
    assert!(
        !changed(&["a"], &["a", "x"]),
        "別の段の依存は起点に関わらない"
    );
    assert!(!task_core::same_phase_dependencies_changed(
        &s(&["a"]),
        None,
        &phase_of,
        &s(&["b"]),
        None,
        &phase_of,
    ));
}
