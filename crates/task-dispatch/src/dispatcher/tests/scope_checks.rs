//! ADR-0074 付記 2026-10-05（`WorkUnitCheck.scope`）: 範囲 check は WU の作業時（WU の作業ツリー、
//! `CELERIS_WU_BASE` / `CELERIS_WU_TARGET` 付き）だけで流し、段の統合の検査には入れない。落ちるときは範囲外の
//! path が判定文に載り、無言で落ちた範囲 check には一文が足される。一時 repository と偽のアダプタだけで、
//! CPU を焼く負荷も外部ネットワークも使わない。
use super::*;

/// ADR の既定の形の範囲 check（`allowed` は許可 path の正規表現）。
fn scope_cmd(allowed: &str) -> String {
    format!(
        "out=$({{ git diff --name-only \"${{CELERIS_WU_BASE:-HEAD}}\"; git ls-files --others --exclude-standard; }} | sort -u | grep -vE '{allowed}'); [ -z \"$out\" ] || {{ echo \"out of scope:\"; echo \"$out\"; exit 1; }}"
    )
}

/// 基点と統合先が env で渡り、どちらも commit / ブランチとして解けることを見る範囲 check。
const ENV_CHECK: &str = "git rev-parse --verify -q \"$CELERIS_WU_BASE^{commit}\" >/dev/null && git rev-parse --verify -q \"refs/heads/$CELERIS_WU_TARGET\" >/dev/null";

fn check(cmd: &str, scope: bool) -> task_core::WorkUnitCheck {
    task_core::WorkUnitCheck {
        cmd: cmd.to_string(),
        expect_exit: 0,
        scope,
    }
}

/// `files` を作業ツリーに書く（親ディレクトリも作る。commit はしない — commit は dispatcher が checks の後にする）。
fn write_files(
    files: &'static [(&'static str, &'static str)],
) -> impl Fn(&std::path::Path) + Send + Sync {
    move |cwd: &std::path::Path| {
        for (path, content) in files {
            let p = cwd.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
    }
}

fn work_unit_check_cmds(events: &[Event], key: &str) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::WorkUnitCheckFinished {
                key: k, cmd, pass, ..
            } if k == key => Some((cmd.clone(), *pass)),
            _ => None,
        })
        .collect()
}

fn integration_check_cmds(events: &[Event], key: &str) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::IntegrationCheckStarted { key: k, cmd, .. } if k == key => Some(cmd.clone()),
            _ => None,
        })
        .collect()
}

/// (a) 2 つの WU が別の範囲（`a/`・`b/`）を変え、それぞれ範囲 check を持つ。範囲 check は WU の作業時に流れて通り
/// （未追跡の file も見る・基点と統合先が env で渡る）、段の統合の検査には範囲 check が入らず普通の check だけが流れ、
/// task は done になる。
#[tokio::test]
async fn scope_checks_run_only_at_work_unit_time_and_not_after_integration() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let mut a = v2_wu("a", "build", &[]);
    a.checks = vec![
        check(&scope_cmd("^a/"), true),
        check(ENV_CHECK, true),
        check("test -f a/one.txt", false),
    ];
    let mut b = v2_wu("b", "build", &[]);
    b.checks = vec![
        check(&scope_cmd("^b/"), true),
        check("test -f b/one.txt", false),
    ];
    adopt_v2_plan(&store, task.id, &["build"], vec![a, b]);
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(20))
            .with_action("a", write_files(&[("a/one.txt", "a\n")]))
            .with_action("b", write_files(&[("b/one.txt", "b\n")])),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;
    let stored = store.get(task.id).unwrap().unwrap();
    let events = events_of(&store, task.id);
    assert_eq!(stored.status, Status::Done, "{stored:?}\n{events:#?}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::WorkUnitChecksFailed { .. })),
        "{events:#?}"
    );
    let a_checks = work_unit_check_cmds(&events, "a");
    assert!(
        a_checks.contains(&(scope_cmd("^a/"), true)),
        "the scope check ran and passed at WU time: {a_checks:?}"
    );
    assert!(
        a_checks.contains(&(ENV_CHECK.to_string(), true)),
        "CELERIS_WU_BASE / CELERIS_WU_TARGET reach the checks: {a_checks:?}"
    );
    let b_checks = work_unit_check_cmds(&events, "b");
    assert!(b_checks.contains(&(scope_cmd("^b/"), true)), "{b_checks:?}");
    let integ = integration_check_cmds(&events, "integrate-build");
    assert!(
        integ.contains(&"test -f a/one.txt".to_string()),
        "{integ:?}"
    );
    assert!(
        integ.contains(&"test -f b/one.txt".to_string()),
        "{integ:?}"
    );
    for scope in [scope_cmd("^a/"), scope_cmd("^b/"), ENV_CHECK.to_string()] {
        assert!(
            !integ.contains(&scope),
            "{scope} ran after integration: {integ:?}"
        );
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::PhaseIntegrated { .. })),
        "{events:#?}"
    );
}

/// (b) WU `a` が範囲外（`b/oops.txt` と追跡済みの `README.md`）を書くと、WU の作業時に `WorkUnitChecksFailed` になり、
/// 判定文に範囲外の path が載る（未追跡も差分も見る）。範囲 check は統合の検査では流れない。
#[tokio::test]
async fn an_out_of_scope_write_fails_the_work_unit_check_and_prints_the_paths() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let mut a = v2_wu("a", "build", &[]);
    a.checks = vec![check(&scope_cmd("^a/"), true)];
    adopt_v2_plan(&store, task.id, &["build"], vec![a]);
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(20)).with_action(
            "a",
            write_files(&[
                ("a/one.txt", "a\n"),
                ("b/oops.txt", "oops\n"),
                ("README.md", "changed\n"),
            ]),
        ),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;
    let events = events_of(&store, task.id);
    let failed: Vec<task_core::FailedWorkUnitCheck> = events
        .iter()
        .filter_map(|e| match e {
            Event::WorkUnitChecksFailed { key, failed, .. } if key == "a" => Some(failed.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(!failed.is_empty(), "{events:#?}");
    let first = &failed[0];
    assert_eq!(first.cmd, scope_cmd("^a/"));
    assert!(first.detail.contains("out of scope:"), "{}", first.detail);
    assert!(first.detail.contains("b/oops.txt"), "{}", first.detail);
    assert!(first.detail.contains("README.md"), "{}", first.detail);
    assert!(!first.detail.contains("a/one.txt"), "{}", first.detail);
    assert!(
        !first
            .detail
            .contains(super::super::work_units::SILENT_SCOPE_CHECK_HINT),
        "the paths were printed, so no hint: {}",
        first.detail
    );
    let integ = integration_check_cmds(&events, "integrate-build");
    assert!(!integ.contains(&scope_cmd("^a/")), "{integ:?}");
}

/// (c) 範囲 check が何も出さずに落ちたら（`exit 1`）判定文に一文が足される。普通の check の無言の不合格・
/// 出力のある範囲 check の不合格には足さない。判定（不合格）は変わらない。
#[tokio::test]
async fn a_silent_failing_scope_check_gets_a_hint_in_its_detail() {
    let ws_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(ws_dir.path(), "true");
    store.insert(&task).unwrap();
    let checks = vec![
        check("exit 1", true),
        check("exit 1", false),
        check("echo x/y.txt; exit 1", true),
    ];
    let observed = super::super::phase_integration::ObservedIntegration {
        work_unit_id: "wu-a".into(),
        key: "a".into(),
        run_id: Some("run-a".into()),
        log_dir: ws_dir.path().join("work-unit-checks").join("a"),
    };
    let ws = task_worker::LocalWorkspace::new(ws_dir.path());
    let results = super::super::phase_integration::run_integration_checks(
        store.as_ref(),
        task.id,
        &observed,
        &ws,
        &checks,
        Duration::from_secs(30),
    )
    .await;
    assert!(results.iter().all(|(pass, _)| !pass), "{results:?}");
    let run = super::super::work_units::WorkUnitCheckRun {
        checks,
        results,
        cwd: ws_dir.path().to_path_buf(),
    };
    let done: Result<RunOutcome, AdapterError> = Ok(RunOutcome {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        exit_code: Some(0),
    });
    let failure = run.failure(&done).expect("all three failed");
    let hint = super::super::work_units::SILENT_SCOPE_CHECK_HINT;
    let summary = failure.summary();
    assert_eq!(summary.matches(hint).count(), 1, "{summary}");
    assert!(summary.contains("x/y.txt"), "{summary}");
    // 判定文は check の順に `; ` で繋がる。足した一文は無言の範囲 check（先頭）の直後にある。
    let after_first = summary
        .split_once(&format!("; {hint}"))
        .map(|(before, _)| before)
        .unwrap_or_default();
    assert!(after_first.contains("cmd=\"exit 1\""), "{summary}");
    assert!(!after_first.contains("x/y.txt"), "{summary}");
}

/// D4: 一文を足すのは `scope` で stdout・stderr とも空のときだけ（純粋関数）。
#[test]
fn scope_check_detail_appends_the_hint_only_for_silent_scope_checks() {
    use super::super::work_units::{SILENT_SCOPE_CHECK_HINT, scope_check_detail};
    let silent = r#"cmd="exit 1" exit=Some(1) expected=0 stdout_tail="" stderr_tail="""#;
    assert_eq!(
        scope_check_detail(true, silent),
        format!("{silent}; {SILENT_SCOPE_CHECK_HINT}")
    );
    assert_eq!(scope_check_detail(false, silent), silent);
    let loud =
        r#"cmd="x" exit=Some(1) expected=0 stdout_tail="out of scope:\nb/x\n" stderr_tail="""#;
    assert_eq!(scope_check_detail(true, loud), loud);
    let timed_out = "command timed out after 30s: exit 1";
    assert_eq!(scope_check_detail(true, timed_out), timed_out);
}

/// D1.4 の 4: 統合の検査の一覧は、その段の生きた葉の WU の普通の check（重複を除く、計画の順）と
/// workspace の既定の check。範囲 check と他の段の check は入らない。
#[test]
fn integration_checks_for_phase_skips_scope_checks() {
    let repo = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(repo.path(), "true");
    store.insert(&task).unwrap();
    let mut a = v2_wu("a", "build", &[]);
    a.checks = vec![
        check(&scope_cmd("^a/"), true),
        check("test -f a/one.txt", false),
        check("cargo test", false),
    ];
    let mut b = v2_wu("b", "build", &[]);
    b.checks = vec![
        check(
            "git diff --name-only \"${{CELERIS_WU_BASE:-HEAD}}\" | grep -vE '^b/'",
            true,
        ),
        check("cargo test", false),
    ];
    let mut c = v2_wu("c", "verify", &["a", "b"]);
    c.checks = vec![check("test -f c.txt", false)];
    adopt_v2_plan(&store, task.id, &["build", "verify"], vec![a, b, c]);
    let units = store.work_units_for(task.id).unwrap();
    let checks = super::super::phase_integration::integration_checks_for_phase(
        &units,
        "build",
        vec!["cargo test".to_string(), "true".to_string()],
    );
    let cmds: Vec<(&str, bool)> = checks.iter().map(|c| (c.cmd.as_str(), c.scope)).collect();
    assert_eq!(
        cmds,
        vec![
            ("test -f a/one.txt", false),
            ("cargo test", false),
            ("true", false)
        ]
    );
}

/// 付記 2026-10-02 / 2026-10-05: repair に渡す範囲外差分の check は `scope: true` と、互換の `git diff` を含む cmd。
#[test]
fn repair_scope_checks_include_scope_flag_and_git_diff_heuristic() {
    use super::super::phase_integration::is_repair_scope_check;
    assert!(is_repair_scope_check(&check(
        "sh scripts/scope.sh a/",
        true
    )));
    assert!(is_repair_scope_check(&check(
        "git diff --name-only HEAD~1",
        false
    )));
    assert!(!is_repair_scope_check(&check("cargo test", false)));
}

type ScopeCheckEnvHistory = Arc<StdMutex<Vec<Vec<(String, String)>>>>;

#[derive(Clone)]
struct WuBaseAdapter {
    env: Vec<(String, String)>,
    seen: ScopeCheckEnvHistory,
    bad: bool,
    retry: bool,
}

#[async_trait]
impl WorkerAdapter for WuBaseAdapter {
    fn id(&self) -> &str {
        "instant"
    }
    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut next = self.clone();
        next.env.extend_from_slice(extra);
        Some(Arc::new(next))
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let cwd = req.work_dir.as_deref().unwrap_or(&req.workspace);
        let key = &req.context.work_unit.as_ref().unwrap().key;
        let mut terminal = Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        };
        if key == "a" {
            write_files(&[
                ("a/previous.txt", "untracked predecessor\n"),
                ("README.md", "tracked predecessor\n"),
            ])(cwd);
        } else if key == "b" {
            let env: Vec<_> = self
                .env
                .iter()
                .filter(|(k, _)| k.starts_with("CELERIS_WU_"))
                .cloned()
                .collect();
            let base = env
                .iter()
                .find(|(k, _)| k == "CELERIS_WU_BASE")
                .expect("worker gets base");
            let helper = env
                .iter()
                .find(|(k, _)| k == "CELERIS_WU_SCOPE_PATHS")
                .expect("worker gets helper");
            assert!(Path::new(&helper.1).is_file());
            std::fs::create_dir_all(cwd.join("b")).unwrap();
            std::fs::write(cwd.join("b/base"), &base.1).unwrap();
            std::fs::write(cwd.join("b/helper"), &helper.1).unwrap();
            if self.bad {
                std::fs::write(cwd.join("a/previous.txt"), "out of scope\n").unwrap();
            }
            let mut seen = self.seen.lock().unwrap();
            if self.retry && seen.is_empty() {
                terminal = Terminal::Error {
                    message: "retry fixture".into(),
                    retryable: true,
                };
            }
            seen.push(env);
        }
        Ok(RunOutcome {
            terminal,
            exit_code: Some(0),
        })
    }
}

fn wu_base_scope_cmd() -> &'static str {
    "paths=$(if [ -n \"${CELERIS_WU_SCOPE_PATHS:-}\" ]; then sh \"$CELERIS_WU_SCOPE_PATHS\"; else git diff --name-only \"${CELERIS_WU_BASE:-HEAD}\" && git ls-files --others --exclude-standard; fi) || exit 1; out=$(printf '%s\n' \"$paths\" | sort -u | grep -vE '^b/'); [ -z \"$out\" ] || { echo 'out of scope:'; echo \"$out\"; exit 1; }"
}

async fn wu_base_dispatch_case(shared: bool, bad: bool, retry: bool) {
    let repo = tempfile::tempdir().unwrap();
    let initial = init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = parallel_task(repo.path(), "true");
    if shared {
        task.workspace = WorkspaceSpec::Local {
            path: repo.path().to_path_buf(),
            mode: Some(task_core::WorkspaceMode::Shared),
        };
        // Runtime bookkeeping must not count as WU source changes.
        std::fs::write(
            repo.path().join(".gitignore"),
            ".taskd/\nruns/\ninputs/\nartifacts/\nwork-unit-checks/\n",
        )
        .unwrap();
        git_out(repo.path(), &["add", ".gitignore"]);
        git_out(repo.path(), &["commit", "-qm", "ignore runtime files"]);
    }
    let before = git_out(repo.path(), &["rev-parse", "HEAD"]);
    store.insert(&task).unwrap();
    let a = v2_wu("a", "build", &[]);
    let mut b = v2_wu("b", "build", &["a"]);
    b.checks = vec![
        check(
            "test \"$(cat b/base)\" = \"$CELERIS_WU_BASE\" && test \"$(cat b/helper)\" = \"$CELERIS_WU_SCOPE_PATHS\"",
            true,
        ),
        check(wu_base_scope_cmd(), true),
    ];
    adopt_v2_plan(&store, task.id, &["build"], vec![a, b]);
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let adapter = Arc::new(WuBaseAdapter {
        env: vec![],
        seen: seen.clone(),
        bad,
        retry,
    });
    let mut d = parallel_dispatcher(store.clone(), adapter, root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;
    let events = events_of(&store, task.id);
    let checks = work_unit_check_cmds(&events, "b");
    assert!(
        checks.contains(&(b_env_check().into(), true)),
        "{events:#?}"
    );
    assert!(
        checks.contains(&(wu_base_scope_cmd().into(), !bad)),
        "{events:#?}"
    );
    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty());
    if retry {
        assert!(seen.len() >= 2);
        assert_eq!(seen[0], seen[1], "retry must reuse base and helper");
    }
    if shared {
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::WorkUnitsSerialized { .. }))
        );
        assert_eq!(
            git_out(repo.path(), &["rev-parse", "HEAD"]),
            before,
            "predecessor remains uncommitted"
        );
    } else {
        assert_eq!(
            git_out(repo.path(), &["rev-parse", "HEAD"]),
            initial,
            "source repo untouched"
        );
    }
    if bad {
        assert!(events.iter().any(|e| matches!(e, Event::WorkUnitChecksFailed { key, failed, .. } if key == "b" && failed.iter().any(|f| f.detail.contains("a/previous.txt")))));
    } else {
        assert_eq!(
            store.get(task.id).unwrap().unwrap().status,
            Status::Done,
            "{events:#?}"
        );
    }
}

fn b_env_check() -> &'static str {
    "test \"$(cat b/base)\" = \"$CELERIS_WU_BASE\" && test \"$(cat b/helper)\" = \"$CELERIS_WU_SCOPE_PATHS\""
}

#[tokio::test]
async fn wu_base_serialized_ignores_uncommitted_predecessor_and_reuses_on_retry() {
    wu_base_dispatch_case(true, false, true).await;
}

#[tokio::test]
async fn wu_base_serialized_rejects_edit_to_predecessor_untracked_file() {
    wu_base_dispatch_case(true, true, false).await;
}

#[tokio::test]
async fn wu_base_own_worktree_worker_and_checks_get_same_snapshot() {
    wu_base_dispatch_case(false, false, true).await;
}

#[tokio::test]
async fn wu_base_snapshot_prepare_failures_accumulate_and_never_start_worker() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = parallel_task(repo.path(), "true");
    task.workspace = WorkspaceSpec::Local {
        path: repo.path().into(),
        mode: Some(WorkspaceMode::Shared),
    };
    store.insert(&task).unwrap();
    adopt_v2_plan(&store, task.id, &["build"], vec![v2_wu("a", "build", &[])]);
    let wu = store
        .work_units_for(task.id)
        .unwrap()
        .into_iter()
        .find(|w| w.key == "a")
        .unwrap();
    super::super::wu_base::scope_env(repo.path(), &wu.id, true).unwrap();
    std::fs::write(
        repo.path()
            .join(".git/celeris-wu-bases")
            .join(&wu.id)
            .join("snapshot.json"),
        "corrupt",
    )
    .unwrap();
    let adapter = Arc::new(WuScriptAdapter::new(HashMap::new()));
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    // Advance only the prepare retry deadline, without sleeps or clock races.
    for _ in 0..super::super::MAX_WU_PREPARE_ATTEMPTS {
        d.tick().unwrap();
        if let Some(f) = d.wu_prepare_failures.get_mut(&wu.id) {
            f.retry_at = OffsetDateTime::UNIX_EPOCH;
        }
    }
    assert!(adapter.seen.lock().unwrap().is_empty());
    let row = store
        .work_units_for(task.id)
        .unwrap()
        .into_iter()
        .find(|w| w.id == wu.id)
        .unwrap();
    assert_eq!(row.status, task_core::WorkUnitStatus::Blocked);
    assert_eq!(
        row.blocked_reason,
        Some(task_core::WorkUnitBlockedReason::Question)
    );
}
