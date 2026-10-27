//! ADR 2026-10-27-done-task-auto-reintegrate の試験。実 git の一時 repo と注入した時刻で決定的に走る。
use super::*;
use std::fs;
use task_core::{
    ArtifactRef, Budget, Check, Criterion, Project, ProjectId, ProjectStatus, SqliteStore, Task,
    TaskKind, TaskRepoIntegrationState, Tier, WorkerHint, WorkspaceSpec,
};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn lines(first: &str, last: &str) -> String {
    let mut lines: Vec<String> = (1..=30).map(|n| format!("line{n}")).collect();
    lines[0] = first.into();
    lines[29] = last.into();
    lines.join("\n") + "\n"
}

fn commit(dir: &Path, path: &str, content: &str) -> String {
    fs::write(dir.join(path), content).expect("write");
    git(dir, &["add", path]);
    git(dir, &["commit", "-q", "-m", path]);
    git(dir, &["rev-parse", "HEAD"])
}

/// main だけに commit を置く（作業ツリーと index は触らない。GIT_INDEX_FILE を一時 file にして commit-tree）。
fn advance_main(dir: &Path, path: &str, content: &str) -> String {
    let index = dir.join(".git").join("advance-main.idx");
    let run = |args: &[&str], stdin: Option<&str>| -> String {
        use std::io::Write;
        let mut child = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("git");
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(input.as_bytes())
                .expect("stdin write");
        }
        let out = child.wait_with_output().expect("git wait");
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    run(&["read-tree", "refs/heads/main"], None);
    let blob = run(&["hash-object", "-w", "--stdin"], Some(content));
    run(
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{blob},{path}"),
        ],
        None,
    );
    let tree = run(&["write-tree"], None);
    let sha = run(
        &["commit-tree", &tree, "-p", "refs/heads/main", "-m", "other"],
        None,
    );
    run(&["update-ref", "refs/heads/main", &sha], None);
    let _ = fs::remove_file(index);
    sha
}

struct Fixture {
    _tmp: tempfile::TempDir,
    dir: std::path::PathBuf,
    store: SqliteStore,
    project: ProjectId,
    now: OffsetDateTime,
}

fn fixture(auto_integrate: bool) -> Fixture {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().to_path_buf();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "test@example.invalid"]);
    git(&dir, &["config", "user.name", "test"]);
    commit(&dir, "shared.txt", &lines("line1", "line30"));
    let store = SqliteStore::open_in_memory().expect("store");
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock");
    let project = Project {
        auto_advance: false,
        auto_integrate,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "p".into(),
        request: "r".into(),
        status: ProjectStatus::Active,
        workspace: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&project).expect("project");
    Fixture {
        _tmp: tmp,
        dir,
        store,
        project: project.id,
        now,
    }
}

fn done_task(project: ProjectId, done_at: OffsetDateTime) -> Task {
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Reviewer,
        }],
        inputs: Vec::<ArtifactRef>::new(),
        depends_on: vec![],
        status: Status::Done,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::local("/nonexistent/ws"),
        repos: Vec::new(),
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 2,
            max_tokens: None,
        },
        attempts: 1,
        lease: None,
        created_at: done_at,
        updated_at: done_at,
        role: None,
        genre: None,
        aggregate: false,
        project_id: Some(project),
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

impl Fixture {
    /// done の task と、その task branch（`path` を `content` に変える 1 commit）を作る。
    fn task(&self, path: &str, content: &str, repo: RepoId) -> (TaskId, BranchRecord) {
        let task = done_task(self.project, self.now);
        self.store.insert(&task).expect("insert");
        let branch = format!("celeris/{}", task.id);
        git(&self.dir, &["switch", "-q", "-c", &branch, "main"]);
        let head = commit(&self.dir, path, content);
        git(&self.dir, &["switch", "-q", "main"]);
        let record = BranchRecord {
            task_id: task.id,
            repo_id: repo,
            repo_path: Some(self.dir.clone()),
            default_branch: Ok("main".into()),
            branch: Some(branch),
            head: Some(head),
            done: true,
            root: true,
            changed_repo: true,
            not_applicable: false,
        };
        (task.id, record)
    }

    fn sweep_and_run(&self, records: &[BranchRecord]) -> Vec<Outcome> {
        super::super::sweep(&self.store, records, self.now).expect("sweep");
        run(&self.store, records, self.now).expect("reintegrate")
    }

    fn status(&self, task: TaskId) -> Status {
        self.store.get(task).expect("get").expect("task").status
    }

    fn row(&self, task: TaskId, repo: RepoId) -> TaskRepoIntegrationState {
        self.store
            .task_integration_get(task, repo)
            .expect("row")
            .expect("row exists")
    }

    fn events(&self, task: TaskId) -> Vec<Event> {
        self.store
            .events_for(task)
            .expect("events")
            .into_iter()
            .map(|(_, e)| e)
            .collect()
    }
}

fn count<F: Fn(&Event) -> bool>(events: &[Event], f: F) -> usize {
    events.iter().filter(|e| f(e)).count()
}

/// (a) main が task の差分と重ならない file だけ進んだ: 再 review（Rereview + DoneReintegrationStarted{disjoint}）に戻り、
/// review 前同期と配送の ff を経て head が main の祖先になり、sweep が integrated にする。
#[test]
fn disjoint_advance_reenters_review_and_lands_as_ancestor() {
    let f = fixture(true);
    let repo = RepoId::new();
    let (task, record) = f.task("task.txt", "task\n", repo);
    advance_main(&f.dir, "other.txt", "other\n");
    let outcomes = f.sweep_and_run(std::slice::from_ref(&record));
    assert_eq!(
        outcomes,
        vec![Outcome::Started {
            task_id: task,
            repo_id: repo,
            class: "disjoint",
            attempt: 1
        }]
    );
    assert_eq!(f.status(task), Status::Reviewing);
    let events = f.events(task);
    // 遷移と開始の記録は同じ transaction で隣り合う。
    let at = events
        .iter()
        .position(|e| matches!(e, Event::DoneReintegrationStarted { .. }))
        .expect("started");
    assert!(matches!(&events[at - 1], Event::Transitioned { reason, .. } if reason == "rereview"));
    assert!(
        matches!(&events[at], Event::DoneReintegrationStarted { class, attempt: 1, head_sha, .. }
        if class == "disjoint" && Some(head_sha) == record.head.as_ref())
    );
    // 再 review に乗った task は次の周期で触らない（done でない）。
    assert!(
        run(&f.store, std::slice::from_ref(&record), f.now)
            .expect("again")
            .is_empty()
    );

    // 以後は既存の経路: review 前同期が main を task branch へ merge し、合格後に配送が main を ff する。
    let branch = record.branch.clone().expect("branch");
    git(&f.dir, &["switch", "-q", &branch]);
    git(&f.dir, &["merge", "-q", "--no-edit", "main"]);
    let synced = git(&f.dir, &["rev-parse", "HEAD"]);
    git(&f.dir, &["switch", "-q", "main"]);
    git(&f.dir, &["merge", "-q", "--ff-only", &synced]);
    f.store
        .apply_transition(task, Trigger::ReviewPass, None)
        .expect("review pass");
    let landed = BranchRecord {
        head: Some(synced.clone()),
        ..record
    };
    assert!(f.sweep_and_run(std::slice::from_ref(&landed)).is_empty());
    assert_eq!(
        f.row(task, repo).state,
        TaskRepoIntegrationStatus::Integrated
    );
    assert!(super::super::ancestor(&f.dir, &synced, "main"));
}

/// main が task と同じ file の別の行を変えた（clean だが overlap）: reviewer もやり直す再 review に戻す。
#[test]
fn overlapping_clean_advance_reenters_review_as_overlap() {
    let f = fixture(true);
    let repo = RepoId::new();
    let (task, record) = f.task("shared.txt", &lines("task", "line30"), repo);
    advance_main(&f.dir, "shared.txt", &lines("line1", "main"));
    assert_eq!(
        classify(&f.dir, record.head.as_deref().expect("head"), "main"),
        Class::Overlap {
            target_sha: git(&f.dir, &["rev-parse", "main"]),
            files: vec!["shared.txt".into()],
        }
    );
    let outcomes = f.sweep_and_run(std::slice::from_ref(&record));
    assert!(matches!(
        outcomes.as_slice(),
        [Outcome::Started {
            class: "overlap",
            ..
        }]
    ));
    assert_eq!(f.status(task), Status::Reviewing);
}

/// (b) 衝突する: 取り込まず done のまま、受信箱 `unintegrated` の理由と統合依頼に出す。見送りの event は 1 回だけ。
#[test]
fn conflicting_advance_stays_done_and_surfaces_in_inbox() {
    let f = fixture(true);
    let repo = RepoId::new();
    let (task, record) = f.task("shared.txt", &lines("task", "line30"), repo);
    let target = advance_main(&f.dir, "shared.txt", &lines("main", "line30"));
    let main_before = git(&f.dir, &["rev-parse", "main"]);
    let outcomes = f.sweep_and_run(std::slice::from_ref(&record));
    assert_eq!(
        outcomes,
        vec![Outcome::Deferred {
            task_id: task,
            repo_id: repo,
            reason: "auto reintegrate: merge conflict in shared.txt".into()
        }]
    );
    assert_eq!(f.status(task), Status::Done);
    assert_eq!(git(&f.dir, &["rev-parse", "main"]), main_before);
    let row = f.row(task, repo);
    assert_eq!(row.state, TaskRepoIntegrationStatus::Unintegrated);
    assert_eq!(
        row.reason.as_deref(),
        Some("auto reintegrate: merge conflict in shared.txt")
    );
    assert_eq!(row.actor, ACTOR);
    let events = f.events(task);
    assert!(events.iter().any(|e| matches!(e,
        Event::IntegrationRequested { request, .. }
            if request.conflict_files == vec!["shared.txt".to_string()] && request.target_sha == target)));
    assert_eq!(
        count(&events, |e| matches!(
            e,
            Event::DoneReintegrationDeferred { .. }
        )),
        1
    );
    assert_eq!(
        count(&events, |e| matches!(
            e,
            Event::DoneReintegrationStarted { .. }
        )),
        0
    );
    // 次の周期: sweep は理由を上書きせず、同じ見送りを重ねて記録しない。
    let before = f.events(task).len();
    f.sweep_and_run(std::slice::from_ref(&record));
    assert_eq!(
        f.row(task, repo).reason.as_deref(),
        Some("auto reintegrate: merge conflict in shared.txt")
    );
    assert_eq!(f.events(task).len(), before);
}

/// (c) 人が hold / discard した行、自動取り込みが無効な案件は触らない。
#[test]
fn held_discarded_and_disabled_projects_are_left_alone() {
    let f = fixture(true);
    let repo = RepoId::new();
    let (held, held_record) = f.task("held.txt", "held\n", repo);
    let (discarded, discarded_record) = f.task("discarded.txt", "discarded\n", repo);
    advance_main(&f.dir, "other.txt", "other\n");
    let records = vec![held_record, discarded_record];
    super::super::sweep(&f.store, &records, f.now).expect("sweep");
    super::super::hold(&f.store, held, repo, "人が確認する", "human", f.now).expect("hold");
    super::super::discard(&f.store, discarded, repo, "不要", "human", f.now).expect("discard");
    assert!(f.sweep_and_run(&records).is_empty());
    for task in [held, discarded] {
        assert_eq!(f.status(task), Status::Done);
        assert!(!f.events(task).iter().any(|e| matches!(
            e,
            Event::DoneReintegrationStarted { .. } | Event::DoneReintegrationDeferred { .. }
        )));
    }
    assert_eq!(f.row(held, repo).state, TaskRepoIntegrationStatus::Held);
    assert_eq!(
        f.row(discarded, repo).state,
        TaskRepoIntegrationStatus::Discarded
    );

    let off = fixture(false);
    let (task, record) = off.task("task.txt", "task\n", repo);
    advance_main(&off.dir, "other.txt", "other\n");
    assert!(off.sweep_and_run(std::slice::from_ref(&record)).is_empty());
    assert_eq!(off.status(task), Status::Done);
    assert_eq!(off.row(task, repo).reason.as_deref(), Some(NOT_ANCESTOR));
}

/// D5: 1 周期で再 review を始めるのは repo ごとに 1 件。task あたり 2 回で止め、理由つきで受信箱に残す。
#[test]
fn one_start_per_repo_per_sweep_and_two_attempts_per_task() {
    let f = fixture(true);
    let repo = RepoId::new();
    let (first, first_record) = f.task("a.txt", "a\n", repo);
    let (second, second_record) = f.task("b.txt", "b\n", repo);
    advance_main(&f.dir, "other.txt", "other\n");
    let records = vec![second_record, first_record.clone()];
    let outcomes = f.sweep_and_run(&records);
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    // 同じ時刻なら id 順で 1 件だけ（どちらか一方）。
    let started = match &outcomes[0] {
        Outcome::Started { task_id, .. } => *task_id,
        other => panic!("{other:?}"),
    };
    let waiting = if started == first { second } else { first };
    assert_eq!(f.status(waiting), Status::Done);

    // `first_record` の task で上限を試す: 2 回目まで始め、3 回目は上限で見送る。
    let target = first;
    let target_record = first_record;
    if started != target {
        // もう一方を done に戻して、この task を始めさせる。
        f.store
            .apply_transition(started, Trigger::ReviewPass, None)
            .expect("pass");
        super::super::hold(&f.store, started, repo, "試験", "human", f.now).expect("hold");
        assert_eq!(f.sweep_and_run(&records).len(), 1);
    }
    for attempt in 2..=MAX_AUTO_REINTEGRATIONS {
        // 再 review の後も main が先へ進み、done のまま head が祖先でない。
        f.store
            .apply_transition(target, Trigger::ReviewPass, None)
            .expect("pass");
        advance_main(&f.dir, &format!("other{attempt}.txt"), "x\n");
        let outcomes = f.sweep_and_run(std::slice::from_ref(&target_record));
        assert!(
            matches!(outcomes.as_slice(), [Outcome::Started { attempt: a, .. }] if *a == attempt)
        );
    }
    f.store
        .apply_transition(target, Trigger::ReviewPass, None)
        .expect("pass");
    advance_main(&f.dir, "last.txt", "x\n");
    f.sweep_and_run(std::slice::from_ref(&target_record));
    f.sweep_and_run(std::slice::from_ref(&target_record));
    assert_eq!(f.status(target), Status::Done);
    assert_eq!(
        f.row(target, repo).reason.as_deref(),
        Some("auto reintegrate: limit reached after 2 attempts")
    );
    let events = f.events(target);
    assert_eq!(
        count(&events, |e| matches!(
            e,
            Event::DoneReintegrationDeferred { .. }
        )),
        1
    );
    // 人の rereview で数え直す。
    assert_eq!(attempts(&events, repo), MAX_AUTO_REINTEGRATIONS);
    f.store
        .apply_transition(target, Trigger::Rereview, None)
        .expect("human rereview");
    assert_eq!(attempts(&f.events(target), repo), 0);
}
