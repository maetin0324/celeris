use super::*;

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = git(dir, args).expect("git");
    assert!(out.ok, "git {args:?}: {}", out.stderr);
    out.stdout.trim().to_string()
}

/// 元のリポジトリ（main に 1 commit）と、Task の worktree（`celeris/<task>`）を作る。
fn setup(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    sh(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    std::fs::write(repo.join("shared.txt"), "line1\nline2\nline3\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-q", "-m", "init"]);
    let task_dir = root.join("ws").join("T1");
    let task_tree = task_dir.join("repos").join("repo");
    let base = rev_parse(&repo, "main").unwrap();
    let lwt = LocalWorktree {
        repo: repo.clone(),
        task_dir: task_dir.clone(),
        dir: task_tree.clone(),
        branch: "celeris/T1".into(),
        base: BaseRef {
            kind: BaseKind::Main,
            sha: base,
        },
    };
    lwt.ensure_blocking().unwrap();
    (repo, task_dir, task_tree)
}

fn make_wu(
    repo: &Path,
    task_dir: &Path,
    task_tree: &Path,
    key: &str,
    base_rev: &str,
) -> LocalWorktree {
    let base = rev_parse(task_tree, base_rev).unwrap();
    let wt = wu_worktree(task_dir, "T1", key, "repo", repo, &base);
    ensure_wu_worktree(&wt).unwrap();
    wt
}

#[test]
fn wu_worktree_is_created_on_its_own_branch_from_the_base_and_is_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let (repo, task_dir, task_tree) = setup(root.path());
    let wt = make_wu(&repo, &task_dir, &task_tree, "a", "HEAD");
    assert_eq!(wt.dir, task_dir.join("wu/a/repos/repo"));
    assert!(wt.dir.join("README.md").is_file());
    assert_eq!(
        sh(&wt.dir, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "celeris-wu/T1/a"
    );
    // 2 回目は何もしない（使い回す）。
    ensure_wu_worktree(&wt).unwrap();
    // 変更が無ければ commit しない。
    let (head0, committed) = commit_all(&wt.dir, "wu/a: nothing").unwrap();
    assert!(!committed);
    assert_eq!(head0, wt.base.sha);
    std::fs::write(wt.dir.join("a.txt"), "a\n").unwrap();
    let (head1, committed) = commit_all(&wt.dir, &commit_message("a", "Title A")).unwrap();
    assert!(committed);
    assert_ne!(head1, head0);
    assert_eq!(
        sh(&wt.dir, &["log", "-1", "--format=%an %s"]),
        "celeris wu/a: Title A"
    );
}

#[test]
fn integrate_merges_leaves_deterministically_and_skips_ones_already_in() {
    let root = tempfile::tempdir().unwrap();
    let (repo, task_dir, task_tree) = setup(root.path());
    let mut items = Vec::new();
    for key in ["a", "b", "c"] {
        let wt = make_wu(&repo, &task_dir, &task_tree, key, "HEAD");
        std::fs::write(wt.dir.join(format!("{key}.txt")), key).unwrap();
        commit_all(&wt.dir, &commit_message(key, key)).unwrap();
        items.push(MergeItem::work_unit(key, wu_branch("T1", key)));
    }
    let out = integrate(&task_tree, &items, "build").unwrap();
    assert!(out.conflict.is_none());
    assert_eq!(out.merged.len(), 3);
    assert!(out.merged.iter().all(|m| !m.skipped));
    for key in ["a", "b", "c"] {
        assert!(task_tree.join(format!("{key}.txt")).is_file());
    }
    let subjects = sh(&task_tree, &["log", "--first-parent", "--format=%s", "-3"]);
    assert_eq!(
        subjects.lines().collect::<Vec<_>>(),
        vec![
            "integrate wu/c (phase build)",
            "integrate wu/b (phase build)",
            "integrate wu/a (phase build)"
        ]
    );
    assert_eq!(out.head, rev_parse(&task_tree, "HEAD").unwrap());
}

/// ADR-0074 §6 F2 のテスト表: 再起動後のやり直しは冪等（済んだ merge は飛ばし、途中の
/// `MERGE_HEAD` は abort してからやり直す）。
#[test]
fn merge_is_idempotent_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let (repo, task_dir, task_tree) = setup(root.path());
    let mut items = Vec::new();
    for key in ["a", "b"] {
        let wt = make_wu(&repo, &task_dir, &task_tree, key, "HEAD");
        std::fs::write(wt.dir.join(format!("{key}.txt")), key).unwrap();
        commit_all(&wt.dir, &commit_message(key, key)).unwrap();
        items.push(MergeItem::work_unit(key, wu_branch("T1", key)));
    }
    // 1 件目だけ入った状態で「落ちた」ことにする。さらに 2 件目の merge を途中で止める
    // （`--no-commit` で MERGE_HEAD を残す）。
    let first = integrate(&task_tree, &items[..1], "build").unwrap();
    assert_eq!(first.merged.len(), 1);
    sh(
        &task_tree,
        &["merge", "--no-ff", "--no-commit", &wu_branch("T1", "b")],
    );
    assert!(rev_parse(&task_tree, "MERGE_HEAD").is_some());
    // やり直し: a は飛ばし、MERGE_HEAD を abort してから b を入れる。
    let again = integrate(&task_tree, &items, "build").unwrap();
    assert!(again.conflict.is_none());
    assert_eq!(
        again
            .merged
            .iter()
            .map(|m| (m.key.as_str(), m.skipped))
            .collect::<Vec<_>>(),
        vec![("a", true), ("b", false)]
    );
    // 3 回目: 何も変わらない（HEAD 同じ、全部 skipped）。
    let head = rev_parse(&task_tree, "HEAD").unwrap();
    let third = integrate(&task_tree, &items, "build").unwrap();
    assert_eq!(third.head, head);
    assert!(third.merged.iter().all(|m| m.skipped));
    let merges = sh(&task_tree, &["log", "--merges", "--format=%s"]);
    assert_eq!(merges.lines().count(), 2, "{merges}");
}

#[test]
fn a_conflict_is_aborted_and_reported_and_resumes_after_the_repair() {
    let root = tempfile::tempdir().unwrap();
    let (repo, task_dir, task_tree) = setup(root.path());
    let mut items = Vec::new();
    for (key, text) in [("a", "line1\nA\nline3\n"), ("b", "line1\nB\nline3\n")] {
        let wt = make_wu(&repo, &task_dir, &task_tree, key, "HEAD");
        std::fs::write(wt.dir.join("shared.txt"), text).unwrap();
        commit_all(&wt.dir, &commit_message(key, key)).unwrap();
        items.push(MergeItem::work_unit(key, wu_branch("T1", key)));
    }
    let before = rev_parse(&task_tree, "HEAD").unwrap();
    let out = integrate(&task_tree, &items, "build").unwrap();
    let conflict = out.conflict.expect("conflict");
    assert_eq!(conflict.key, "b");
    assert_eq!(conflict.files, vec!["shared.txt".to_string()]);
    assert_eq!(out.merged.len(), 1);
    assert_ne!(out.head, before, "a は入っている");
    assert!(rev_parse(&task_tree, "MERGE_HEAD").is_none(), "abort 済み");
    assert!(status_clean(&task_tree));
    // repair WU の代わり: Task の worktree で b を merge し、衝突を解消して commit する。
    let _ = git(
        &task_tree,
        &["merge", "--no-ff", "--no-edit", &wu_branch("T1", "b")],
    );
    std::fs::write(task_tree.join("shared.txt"), "line1\nA\nB\nline3\n").unwrap();
    sh(&task_tree, &["add", "-A"]);
    sh(&task_tree, &["commit", "-q", "--no-edit"]);
    // 続きから: a も b も既に入っているので飛ばす。
    let resumed = integrate(&task_tree, &items, "build").unwrap();
    assert!(resumed.conflict.is_none());
    assert!(resumed.merged.iter().all(|m| m.skipped));
}

#[test]
fn a_stacked_unit_branches_from_its_dependency_and_only_the_leaf_is_merged() {
    let root = tempfile::tempdir().unwrap();
    let (repo, task_dir, task_tree) = setup(root.path());
    let a = make_wu(&repo, &task_dir, &task_tree, "a", "HEAD");
    std::fs::write(a.dir.join("a.txt"), "a").unwrap();
    let (a_head, _) = commit_all(&a.dir, "wu/a: a").unwrap();
    // b は a のブランチの HEAD から切る（積み上げ）。
    let b_base = rev_parse(&repo, &format!("refs/heads/{}", wu_branch("T1", "a"))).unwrap();
    assert_eq!(b_base, a_head);
    let b = wu_worktree(&task_dir, "T1", "b", "repo", &repo, &b_base);
    ensure_wu_worktree(&b).unwrap();
    assert!(b.dir.join("a.txt").is_file(), "a の成果の上で始まる");
    std::fs::write(b.dir.join("b.txt"), "b").unwrap();
    commit_all(&b.dir, "wu/b: b").unwrap();
    let out = integrate(
        &task_tree,
        &[MergeItem::work_unit("b", wu_branch("T1", "b"))],
        "build",
    )
    .unwrap();
    assert!(out.conflict.is_none());
    assert!(task_tree.join("a.txt").is_file() && task_tree.join("b.txt").is_file());
    assert!(is_ancestor(&task_tree, &a_head, "HEAD"));
    // worktree を消してもブランチは残る。
    remove_wu_worktree(&a).unwrap();
    remove_wu_worktree(&b).unwrap();
    assert!(!a.dir.exists() && !b.dir.exists());
    assert!(rev_parse(&repo, &format!("refs/heads/{}", wu_branch("T1", "a"))).is_some());
}

fn status_clean(dir: &Path) -> bool {
    sh(dir, &["status", "--porcelain"]).is_empty()
}

fn dep_row(key: &str, kind: task_core::WorkUnitKind) -> task_core::WorkUnitRow {
    let spec = task_core::WorkUnitSpec {
        expected_write_paths: None,
        key: key.to_string(),
        kind,
        title: key.to_string(),
        objective: key.to_string(),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: task_core::WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: Some("p".into()),
    };
    let mut row = task_core::WorkUnitRow::new(
        format!("id-{key}"),
        "T1".into(),
        "plan".into(),
        0,
        spec,
        task_core::WorkUnitStatus::Done,
        "2026-09-28T00:00:00Z".into(),
    );
    row.phase = Some("p".into());
    row
}

/// 1 回 commit して、その sha を返す。
fn commit_file(dir: &Path, name: &str) -> String {
    std::fs::write(dir.join(name), name).unwrap();
    commit_all(dir, &format!("add {name}")).unwrap().0
}

/// ADR-0074「Phase F5-fix7 実装時の明確化」: 依存先の基点の決め方（ブランチ → 記録した commit →
/// Task ブランチ）。
#[test]
fn dependency_base_falls_back_to_the_recorded_commits_and_the_task_branch() {
    let root = tempfile::tempdir().unwrap();
    let (repo, task_dir, task_tree) = setup(root.path());
    let main = rev_parse(&repo, "main").unwrap();
    let bogus = "0123456789abcdef0123456789abcdef01234567".to_string();

    // 1. WU ブランチがあればその HEAD。
    let wt = make_wu(&repo, &task_dir, &task_tree, "a", "HEAD");
    let a_head = commit_file(&wt.dir, "a.txt");
    let mut a = dep_row("a", task_core::WorkUnitKind::Implement);
    a.branch = Some(wu_branch("T1", "a"));
    a.head_commit = Some(bogus.clone());
    assert_eq!(
        dependency_base(&repo, "T1", &a, "celeris/T1", "celeris/").unwrap(),
        a_head
    );

    // 2. Task の worktree で走った repair WU（本番の remerge）: Task ブランチに commit した head。
    let repair_head = commit_file(&task_tree, "remerge.txt");
    let mut remerge = dep_row("remerge", task_core::WorkUnitKind::Repair);
    remerge.head_commit = Some(repair_head.clone());
    assert_eq!(
        dependency_base(&repo, "T1", &remerge, "celeris/T1", "celeris/").unwrap(),
        repair_head
    );
    // commit しなかった（head も base も無い）repair WU: Task ブランチの HEAD。
    let task_head = commit_file(&task_tree, "later.txt");
    let nothing = dep_row("nothing", task_core::WorkUnitKind::Repair);
    assert_eq!(
        dependency_base(&repo, "T1", &nothing, "celeris/T1", "celeris/").unwrap(),
        task_head
    );
    // 記録した head がこのリポジトリに無ければ Task ブランチの HEAD。
    let mut stale = dep_row("stale", task_core::WorkUnitKind::Repair);
    stale.head_commit = Some(bogus.clone());
    assert_eq!(
        dependency_base(&repo, "T1", &stale, "celeris/T1", "celeris/").unwrap(),
        task_head
    );

    // 統合 WU: integrated_commit。
    let mut integ = dep_row("integrate-p", task_core::WorkUnitKind::Integrate);
    integ.integrated_commit = Some(repair_head.clone());
    assert_eq!(
        dependency_base(&repo, "T1", &integ, "celeris/T1", "celeris/").unwrap(),
        repair_head
    );

    // 3. WU ブランチを持っていたのに ref が無い: head_commit → base_commit（commit の無い done）。
    let mut gone = dep_row("gone", task_core::WorkUnitKind::Implement);
    gone.branch = Some(wu_branch("T1", "gone"));
    gone.head_commit = Some(repair_head.clone());
    assert_eq!(
        dependency_base(&repo, "T1", &gone, "celeris/T1", "celeris/").unwrap(),
        repair_head
    );
    gone.head_commit = None;
    gone.base_commit = Some(main.clone());
    assert_eq!(
        dependency_base(&repo, "T1", &gone, "celeris/T1", "celeris/").unwrap(),
        main
    );
    // どれも解決できない: Err（呼び出し側は blocked にする）。
    gone.head_commit = Some(bogus.clone());
    gone.base_commit = None;
    let err = dependency_base(&repo, "T1", &gone, "celeris/T1", "celeris/").unwrap_err();
    assert!(err.contains("dependency branch of gone"), "{err}");
    // Task の worktree で走った WU でも、Task ブランチまで無ければ Err。
    let err = dependency_base(&repo, "T1", &nothing, "celeris/missing", "celeris/").unwrap_err();
    assert!(err.contains("ran in the task worktree"), "{err}");
}

/// ADR-0079 D6（Phase R1c）: 子 task のブランチ（`optional`）は、このリポジトリに無ければ飛ばし
/// （`merged` に出さない）、既に入っていれば `skipped`（採用した done の子の成果が既に親ブランチに
/// ある場合の冪等）。WU のブランチが無ければ従来どおり `Err`。
#[test]
fn child_task_branches_are_optional_and_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let (repo, _task_dir, task_tree) = setup(root.path());
    // 子のブランチ（親ブランチの HEAD から切った別の worktree で commit）。
    let child_dir = root.path().join("ws").join("C1").join("tree");
    let child = LocalWorktree {
        repo: repo.clone(),
        task_dir: root.path().join("ws").join("C1"),
        dir: child_dir.clone(),
        branch: "celeris/C1".into(),
        base: BaseRef {
            kind: BaseKind::Parent,
            sha: rev_parse(&task_tree, "HEAD").unwrap(),
        },
    };
    child.ensure_blocking().unwrap();
    let child_head = commit_file(&child_dir, "c.txt");
    let items = vec![
        MergeItem::child_task("missing", "celeris/NOPE"),
        MergeItem::child_task("c", "celeris/C1"),
    ];
    let out = integrate(&task_tree, &items, "s1").unwrap();
    assert!(out.conflict.is_none());
    assert_eq!(
        out.merged,
        vec![Merged {
            key: "c".into(),
            commit: child_head.clone(),
            skipped: false,
            target_sha: None,
            parent_head: None,
        }]
    );
    assert!(task_tree.join("c.txt").is_file());
    // もう一度: 既に入っているので飛ばす。
    let again = integrate(&task_tree, &items, "s1").unwrap();
    assert_eq!(again.merged.len(), 1);
    assert!(again.merged[0].skipped);
    // WU のブランチが無ければ Err（従来どおり）。
    let err = integrate(
        &task_tree,
        &[MergeItem::work_unit("x", "celeris-wu/T1/x")],
        "s1",
    )
    .unwrap_err();
    assert!(err.contains("does not exist"), "{err}");

    // `dependency_base`: 依存先が kind task の unit なら子のブランチの HEAD → 記録した head → Task ブランチ。
    let mut dep = dep_row("c", task_core::WorkUnitKind::Task);
    dep.child_task_id = Some("C1".into());
    assert_eq!(
        dependency_base(&repo, "T1", &dep, "celeris/T1", "celeris/").unwrap(),
        child_head
    );
    dep.child_task_id = Some("GONE".into());
    dep.head_commit = Some(child_head.clone());
    assert_eq!(
        dependency_base(&repo, "T1", &dep, "celeris/T1", "celeris/").unwrap(),
        child_head
    );
    dep.head_commit = None;
    assert_eq!(
        dependency_base(&repo, "T1", &dep, "celeris/T1", "celeris/").unwrap(),
        rev_parse(&repo, "refs/heads/celeris/T1").unwrap()
    );
    let err = dependency_base(&repo, "T1", &dep, "celeris/missing", "celeris/").unwrap_err();
    assert!(err.contains("child task unit"), "{err}");
}

/// 子の worktree（親ブランチの HEAD から切る）を作る。
fn make_child(root: &Path, repo: &Path, task_tree: &Path, id: &str) -> PathBuf {
    let child_dir = root.join("ws").join(id).join("tree");
    let child = LocalWorktree {
        repo: repo.to_path_buf(),
        task_dir: root.join("ws").join(id),
        dir: child_dir.clone(),
        branch: format!("celeris/{id}"),
        base: BaseRef {
            kind: BaseKind::Parent,
            sha: rev_parse(task_tree, "HEAD").unwrap(),
        },
    };
    child.ensure_blocking().unwrap();
    child_dir
}

/// ADR-0118 D5: 子のブランチが review 後に動いた（HEAD ≠ merge candidate）なら merge せずに止める。
#[test]
fn merge_candidate_mismatch_stops_without_merging() {
    let root = tempfile::tempdir().unwrap();
    let (repo, _task_dir, task_tree) = setup(root.path());
    let child_dir = make_child(root.path(), &repo, &task_tree, "C1");
    let reviewed = commit_file(&child_dir, "c.txt");
    let target = rev_parse(&task_tree, "HEAD").unwrap();
    // review の後に子のブランチへ commit が足された。
    let moved = commit_file(&child_dir, "late.txt");
    let before = rev_parse(&task_tree, "HEAD").unwrap();
    let items = vec![
        MergeItem::child_task("c", "celeris/C1").with_candidate(Some(MergeCandidate {
            target_sha: target.clone(),
            merge_candidate_sha: reviewed.clone(),
        })),
    ];
    let out = integrate(&task_tree, &items, "s1").unwrap();
    assert!(out.conflict.is_none());
    assert!(out.merged.is_empty());
    assert_eq!(
        out.stale,
        Some(StaleCandidate {
            key: "c".into(),
            branch: "celeris/C1".into(),
            merge_candidate_sha: reviewed,
            head_sha: moved,
            target_sha: target,
        })
    );
    assert_eq!(out.head, before);
    assert_eq!(rev_parse(&task_tree, "HEAD").unwrap(), before);
    assert!(!task_tree.join("c.txt").exists());
}

/// ADR-0118 D5 / ADR-0079 D6: 親ブランチが子の review 後に進んだだけなら従来どおり merge し、
/// 記録の target SHA と merge 直前の親 HEAD を返す。記録の無い子は照合しない（移行前）。
#[test]
fn merge_candidate_match_merges_after_parent_advanced_and_records_parent_head() {
    let root = tempfile::tempdir().unwrap();
    let (repo, _task_dir, task_tree) = setup(root.path());
    let child_dir = make_child(root.path(), &repo, &task_tree, "C1");
    let reviewed = commit_file(&child_dir, "c.txt");
    let target = rev_parse(&task_tree, "HEAD").unwrap();
    // 子の review の後に親ブランチが進んだ（同じ段階の別の merge など）。
    let parent_head = commit_file(&task_tree, "p.txt");
    let legacy_dir = make_child(root.path(), &repo, &task_tree, "C2");
    let legacy = commit_file(&legacy_dir, "legacy.txt");
    let items = vec![
        MergeItem::child_task("c", "celeris/C1").with_candidate(Some(MergeCandidate {
            target_sha: target.clone(),
            merge_candidate_sha: reviewed.clone(),
        })),
        MergeItem::child_task("legacy", "celeris/C2"),
    ];
    let out = integrate(&task_tree, &items, "s1").unwrap();
    assert!(out.conflict.is_none());
    assert!(out.stale.is_none());
    assert_eq!(out.merged.len(), 2);
    assert_eq!(out.merged[0].commit, reviewed);
    assert_eq!(out.merged[0].target_sha.as_deref(), Some(target.as_str()));
    assert_eq!(
        out.merged[0].parent_head.as_deref(),
        Some(parent_head.as_str())
    );
    assert_eq!(out.merged[1].commit, legacy);
    assert_eq!(out.merged[1].target_sha, None);
    assert_eq!(out.merged[1].parent_head, None);
    assert!(task_tree.join("c.txt").is_file());
    assert!(task_tree.join("legacy.txt").is_file());
    assert!(is_ancestor(&task_tree, &parent_head, "HEAD"));
}
