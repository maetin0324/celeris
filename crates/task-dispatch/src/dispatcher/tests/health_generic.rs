//! ADR 2026-11-10-health-generic-fixtures (3)・D3.1〜D3.3（unit rs-paths）: Celeris でない isolated Git fixture と本物の
//! 検査器で、dispatcher の経路（WU の check・atomic の direct review・final review の材料・複数 repo）が repo health を
//! 当て、Celeris 固有の欄（案件名・repo 名・task の mode・selfdeploy・案件の flag）で結果が変わらないことを確かめる。
//!
//! fixture は builder（`scripts/tests/repo_health_fixtures.py`）の CLI で `$TMPDIR` の一時 repo に作る（[`build_fixture`]。
//! 内容をここに写さない。ADR の K1・K2）。検査器は作業ツリーの `scripts/dev/repo-health/` を release と同じ配置
//! （`profiles/` を除く。`scripts/selfdeploy/lib.sh` の `sd_bundle_repo_health`）で写した同梱の検査器
//! （[`configure_real_bundled_checker`]）で、stub の検査器は使わない。fixture の base は検査器を持たないので、適用判定は
//! 同梱の検査器を選ぶ（G3 の D1.4）。nextest は試験ごとに process を分けるので、process に 1 回の同梱の設定は他の試験と
//! ぶつからない。外部ネットワークに出ず、LLM を呼ばない。
//!
//! 既存の試験で足りない理由: `health_enforce_non_celeris_*` は stub の検査器（決まった finding を返す）で経路を通し、
//! `scripts/tests/test_repo_health_generic*.py` は検査器だけを fixture に当てる。本物の検査器の判定（層の宣言・
//! 候補の finding・evidence の path）が dispatcher の経路の結果（repair・review の材料・統合の停止）に届くことは、
//! どちらも確かめない。
//!
//! `health_generic_stale.rs`・`health_generic_limits.rs` は [`configure_real_bundled_checker`]・[`build_fixture`]・
//! [`apply_candidate`]・[`restore_base`] を `pub(super)` で使う。
use super::health_enforce::{
    RepairActionAdapter, health_items, non_celeris_dispatcher, unit, with_check,
};
use super::health_review_scenario::copy_checker;
use super::target_sync::reviewing_task;
use super::*;
use crate::health_review::gate::{CHECKER_TREE_FILE, configure_bundled_checker};
use task_core::required_checks::health::{
    CHECKER_DIR, CheckerId, CheckerSource, HEALTH_CHECK_ID, HealthEvidence, HealthVerdict,
    PROFILES_DIR, SOURCE_SIZE_REPORT,
};
use task_worker::protocol::{
    HealthFindingKind, HealthRepoInput, HealthReviewInput, HealthSeverity,
};

/// builder の出力の末尾の 1 行（ADR D1.1）。`base`・`head` は `non-git` で `None`。`multi` は先頭の repo の SHA。
#[derive(Debug, Clone, serde::Deserialize)]
pub(super) struct Fixture {
    pub(super) base: Option<String>,
    pub(super) head: Option<String>,
    pub(super) repos: Vec<String>,
    pub(super) expected: Vec<String>,
}

impl Fixture {
    pub(super) fn base(&self) -> &str {
        self.base.as_deref().expect("the fixture has a base commit")
    }

    pub(super) fn head(&self) -> &str {
        self.head.as_deref().expect("the fixture has a head commit")
    }
}

/// この worktree の root（`scripts/` の親）。
fn source_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// builder の CLI `python3 -I <root>/scripts/tests/repo_health_fixtures.py build <kind> <variant> <dest>` で fixture を
/// 作る（ADR D3.2）。python3 が無ければ失敗にする（skip にしない。検査器も python3 を要する）。
pub(super) fn build_fixture(kind: &str, variant: &str, dest: &std::path::Path) -> Fixture {
    let builder = source_root().join("scripts/tests/repo_health_fixtures.py");
    let mut cmd = std::process::Command::new("python3");
    cmd.arg("-I")
        .arg(&builder)
        .args(["build", kind, variant])
        .arg(dest);
    run_builder(cmd, kind, variant)
}

/// [`build_fixture`] の profile を置かない形（base に `profiles/default.toml` が無く、検査器は組み込み generic を使う。
/// 契約 ADR 2026-11-07-repo-health-contract の D2）。builder の CLI は profile の有無を選べないので、同じ builder の
/// `build(..., with_profile=False)` を呼ぶ（fixture の内容は写さない）。
fn build_fixture_without_profile(kind: &str, variant: &str, dest: &std::path::Path) -> Fixture {
    const CALL: &str = "import json, sys\nfrom dataclasses import asdict\nfrom pathlib import Path\n\
        sys.path.insert(0, sys.argv[1])\nimport repo_health_fixtures as f\n\
        print(json.dumps(asdict(f.build(sys.argv[2], sys.argv[3], Path(sys.argv[4]), with_profile=False))))\n";
    let mut cmd = std::process::Command::new("python3");
    cmd.arg("-I")
        .arg("-c")
        .arg(CALL)
        .arg(source_root().join("scripts/tests"))
        .args([kind, variant])
        .arg(dest);
    run_builder(cmd, kind, variant)
}

/// builder を走らせ、出力の末尾の 1 行を [`Fixture`] として読む。
fn run_builder(mut cmd: std::process::Command, kind: &str, variant: &str) -> Fixture {
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("python3 を起動できない（builder と検査器が要る）: {e}"));
    assert!(
        out.status.success(),
        "builder {kind}/{variant}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_else(|| panic!("builder {kind}/{variant} printed nothing"));
    serde_json::from_str(last).unwrap_or_else(|e| panic!("builder の JSON {last:?}: {e}"))
}

/// 作業ツリーの本物の検査器（`scripts/dev/repo-health/` と side file）を release の同梱の配置に写し（`profiles/` は
/// release と同じく除く）、`CHECKER_TREE` に HEAD の tree id と blob id を書いて process の同梱の検査器に設定する。
/// 返す dir は試験の間持ち続ける（process の設定がこの dir を指す）。
pub(super) fn configure_real_bundled_checker() -> (tempfile::TempDir, CheckerId) {
    let root = source_root();
    let bundle = tempfile::tempdir().unwrap();
    copy_checker(&root, bundle.path());
    std::fs::remove_dir_all(bundle.path().join(PROFILES_DIR)).unwrap();
    let tree = git_out(&root, &["rev-parse", &format!("HEAD:{CHECKER_DIR}")]);
    let blob = git_out(&root, &["rev-parse", &format!("HEAD:{SOURCE_SIZE_REPORT}")]);
    std::fs::write(
        bundle.path().join(CHECKER_TREE_FILE),
        format!("repo-health {tree}\nsource-size-report {blob}\n"),
    )
    .unwrap();
    let configured = configure_bundled_checker(bundle.path())
        .unwrap_or_else(|why| panic!("同梱の検査器を設定できない: {why}"));
    assert_eq!(configured.id.source, CheckerSource::Bundled);
    let id = configured.id.clone();
    (bundle, id)
}

/// 作業ツリー `dir`（fixture の base から切った branch）を fixture の候補の commit まで進める（worker の変更の代わり）。
pub(super) fn apply_candidate(dir: &std::path::Path, head: &str) {
    git_out(dir, &["merge", "-q", "--ff-only", head]);
}

/// 作業ツリー `dir` の tree を fixture の base に戻して commit する（修復の代わり。候補の変更を取り消す）。
pub(super) fn restore_base(dir: &std::path::Path, base: &str) {
    git_out(dir, &["read-tree", "-u", "--reset", base]);
    git_out(
        dir,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "undo the candidate change",
        ],
    );
}

/// fixture の base → head で変わった path（evidence の path の照合に使う。fixture の内容を写さない）。
pub(super) fn changed_paths(repo: &std::path::Path, fx: &Fixture) -> Vec<String> {
    git_out(repo, &["diff", "--name-only", fx.base(), fx.head()])
        .lines()
        .map(str::to_string)
        .collect()
}

/// review の health の項目（`RequiredChecksEvaluated.results[]` の `HEALTH_CHECK_ID`）を `(pass, evidence)` で順に。
pub(super) fn review_health(events: &[Event]) -> Vec<(bool, HealthEvidence)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::RequiredChecksEvaluated { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .filter(|r| r.cmd == HEALTH_CHECK_ID)
        .filter_map(|r| Some((r.pass, (**r.health.as_ref()?).clone())))
        .collect()
}

/// dep-inversion の fail 級の finding の rule（ADR (2) の表）。層の逆向きは `deps.violation`。rust は core の
/// `Cargo.toml` が app に依存し、app が core に依存するので Cargo の循環 `deps.cycle` も fail 級になる（検査器の実測）。
pub(super) fn dep_inversion_rules(kind: &str) -> Vec<&'static str> {
    match kind {
        "rust" => vec!["deps.cycle", "deps.violation"],
        _ => vec!["deps.violation"],
    }
}

/// fail 級の finding の rule の集合（fingerprint `<rule_id>:<hash>` の rule の部分）。
pub(super) fn blocking_rules(evidence: &HealthEvidence) -> Vec<&str> {
    let mut rules: Vec<&str> = evidence
        .blocking
        .iter()
        .map(|fp| fp.rsplit_once(':').map_or(fp.as_str(), |(rule, _)| rule))
        .collect();
    rules.sort_unstable();
    rules.dedup();
    rules
}

/// 本物の検査器（同梱）が dep-inversion の候補に出した Fail（fail 級の rule は [`dep_inversion_rules`] だけ）。
pub(super) fn assert_dep_inversion_fail(
    kind: &str,
    evidence: &HealthEvidence,
    bundled: &CheckerId,
    head: &str,
) {
    assert_eq!(evidence.verdict, HealthVerdict::Fail, "{evidence:?}");
    assert_eq!(evidence.checker.as_ref(), Some(bundled), "{evidence:?}");
    assert_eq!(evidence.head_sha, head);
    assert_eq!(
        blocking_rules(evidence),
        dep_inversion_rules(kind),
        "{evidence:?}"
    );
}

/// 本物の検査器の Pass（fail 級 0）。
pub(super) fn assert_pass(evidence: &HealthEvidence, bundled: &CheckerId) {
    assert_eq!(evidence.verdict, HealthVerdict::Pass, "{evidence:?}");
    assert_eq!(evidence.checker.as_ref(), Some(bundled), "{evidence:?}");
    assert!(evidence.blocking.is_empty(), "{evidence:?}");
}

/// dep-inversion の review の repair の class（fail 級の rule の bucket `health:<rule_id>` を `,` で並べたもの）。
fn repair_class(kind: &str) -> String {
    dep_inversion_rules(kind)
        .iter()
        .map(|rule| format!("health:{rule}"))
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn repair_classes(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::RepairScheduled { class, origin, .. }
                if *origin == task_core::execution::RepairOrigin::Review =>
            {
                Some(class.clone())
            }
            _ => None,
        })
        .collect()
}

fn no_replan_or_question(events: &[Event]) -> bool {
    !events.iter().any(|e| {
        matches!(
            e,
            Event::AutoReplanRequested { .. } | Event::QuestionRaised { .. }
        )
    })
}

// ---- WU の check ----

/// WU の check を流した結果（fixture と同梱の検査器の dir は試験の終わりまで持つ）。
struct WuFlow {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    fx: Fixture,
    changed: Vec<String>,
    bundled: CheckerId,
    unit: task_core::WorkUnitRow,
    events: Vec<Event>,
}

/// 1 段 1 WU の計画で、WU の 1 回目の run が fixture の候補を入れ、2 回目以降（WU の repair = retry）が base に戻す。
async fn wu_check_flow(bundled: &CheckerId, kind: &str, variant: &str) -> WuFlow {
    let fixtures = tempfile::tempdir().unwrap();
    let repo = fixtures.path().join(kind);
    let fx = build_fixture(kind, variant, &repo);
    let changed = changed_paths(&repo, &fx);
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(&repo, "true");
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["build"],
        vec![with_check(v2_wu("a", "build", &[]))],
    );
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (base, head) = (fx.base().to_string(), fx.head().to_string());
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(5)).with_action("a", move |dir| {
            if runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                apply_candidate(dir, &head);
            } else {
                restore_base(dir, &base);
            }
        }),
    );
    let mut d = non_celeris_dispatcher(store.clone(), adapter, root.path(), 2);
    run_until_idle(&mut d, 800).await;
    let a = unit(&store, task.id, "a");
    let events = events_of(&store, task.id);
    WuFlow {
        _dirs: (fixtures, root),
        fx,
        changed,
        bundled: bundled.clone(),
        unit: a,
        events,
    }
}

/// WU の health の項目（`WorkUnitCheckFinished.health`）を順に。
fn wu_health(flow: &WuFlow) -> Vec<(bool, HealthEvidence)> {
    health_items(&flow.events)
        .into_iter()
        .filter(|(id, ..)| *id == flow.unit.id)
        .map(|(_, pass, h)| (pass, h))
        .collect()
}

/// 不変条件 (a): 良い変更の WU の check は本物の検査器で health を評価し（base = WU の base = fixture の base、head =
/// 候補）、Pass にして WU を done にする。checks の不合格（retry）にしない。
async fn wu_check_good_passes(kind: &str) {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let flow = wu_check_flow(&bundled, kind, "good").await;
    let items = wu_health(&flow);
    let (pass, first) = items
        .first()
        .unwrap_or_else(|| panic!("no health item for the work unit: {:?}", flow.events));
    assert!(*pass, "{first:?}");
    assert_pass(first, &flow.bundled);
    assert_eq!(first.base_sha, flow.fx.base());
    assert_eq!(Some(&first.base_sha), flow.unit.base_commit.as_ref());
    assert_eq!(first.head_sha, flow.fx.head());
    assert!(
        !flow
            .events
            .iter()
            .any(|e| matches!(e, Event::WorkUnitChecksFailed { .. })),
        "a good change must not fail the checks: {:?}",
        flow.events
    );
    assert_eq!(
        flow.unit.status,
        task_core::WorkUnitStatus::Done,
        "{:?}",
        flow.unit
    );
}

#[tokio::test]
async fn health_generic_wu_check_ts_good_passes() {
    wu_check_good_passes("ts").await;
}

#[tokio::test]
async fn health_generic_wu_check_rust_good_passes() {
    wu_check_good_passes("rust").await;
}

/// 不変条件 (b)(g): base の profile が層を宣言した repo で、層の逆向きの依存を入れた WU の check は、本物の検査器の
/// fail 級の `deps.violation` で Fail になり、checks の不合格（WU の repair = retry）に回る。不合格の記録は class
/// `health:deps.violation` の bucket の形（`health:<fingerprint>（evidence: <候補が変えた path>）`）。retry が直した
/// HEAD は評価し直して Pass にし、replan・人への質問にしない。
async fn wu_check_dep_inversion_routes_to_repair(kind: &str) {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let flow = wu_check_flow(&bundled, kind, "dep-inversion").await;
    let items = wu_health(&flow);
    let (failed, passed) = match items.as_slice() {
        [first, .., last] => (first, last),
        _ => panic!("the work unit check did not evaluate health twice: {items:?}"),
    };
    assert!(!failed.0, "{failed:?}");
    assert_dep_inversion_fail(kind, &failed.1, &flow.bundled, flow.fx.head());
    assert_eq!(failed.1.base_sha, flow.fx.base());
    assert!(passed.0, "{passed:?}");
    assert_pass(&passed.1, &flow.bundled);
    assert_ne!(passed.1.head_sha, failed.1.head_sha);
    let failed_checks: Vec<&task_core::FailedWorkUnitCheck> = flow
        .events
        .iter()
        .filter_map(|e| match e {
            Event::WorkUnitChecksFailed {
                work_unit_id,
                failed,
                ..
            } if *work_unit_id == flow.unit.id => Some(failed),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(failed_checks.len(), 1, "{failed_checks:?}");
    let detail = &failed_checks[0].detail;
    assert_eq!(failed_checks[0].cmd, HEALTH_CHECK_ID);
    let prefix = format!("health:{}（evidence: ", failed.1.blocking[0]);
    assert!(detail.starts_with(&prefix), "{detail}");
    assert!(
        flow.changed.iter().any(|p| detail.contains(p.as_str())),
        "the evidence must point at the candidate's change {:?}: {detail}",
        flow.changed
    );
    assert!(
        no_replan_or_question(&flow.events),
        "a deterministic health Fail must go to the work unit repair: {:?}",
        flow.events
    );
    assert_eq!(
        flow.unit.status,
        task_core::WorkUnitStatus::Done,
        "{:?}",
        flow.unit
    );
}

#[tokio::test]
async fn health_generic_wu_check_ts_dep_inversion_routes_to_repair() {
    wu_check_dep_inversion_routes_to_repair("ts").await;
}

#[tokio::test]
async fn health_generic_wu_check_rust_dep_inversion_routes_to_repair() {
    wu_check_dep_inversion_routes_to_repair("rust").await;
}

// ---- atomic の direct review ----

/// direct review を流した結果。
struct ReviewFlow {
    _dirs: Vec<tempfile::TempDir>,
    fx: Fixture,
    changed: Vec<String>,
    bundled: CheckerId,
    task: Task,
    ws: PathBuf,
    events: Vec<Event>,
    status: Status,
    /// [`Reviewer::Candidates`] の adapter が reviewer run ごとに受けた `ReviewRequest.health`（順に）。ほかの
    /// reviewer では空。
    requests: Vec<Option<HealthReviewInput>>,
}

/// final review の reviewer run（[`review_flow`]）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Reviewer {
    /// reviewer run を起こさない（acceptance に `Check::Reviewer` を足さない）。review の repair は worktree を
    /// fixture の base に戻す。
    #[default]
    None,
    /// acceptance に `Check::Reviewer` を足し、reviewer は合格の `review.json`（[`REVIEW_PASS`]）を書く。
    Pass,
    /// acceptance に `Check::Reviewer` を足し、reviewer は受けた `ReviewRequest.health` を記録して、その候補の
    /// finding から判定を書く（[`CandidateReviewer`]）。
    Candidates,
}

/// review の作り方（[`review_flow`]）。
#[derive(Debug, Clone, Copy, Default)]
struct ReviewSetup {
    reviewer: Reviewer,
    /// Celeris 固有の欄（[`Flags`]）。`None` は既定の task（`target_sync::reviewing_task`）。
    flags: Option<Flags>,
    /// fixture の base に profile を置かない（検査器は組み込み generic を使う。[`build_fixture_without_profile`]）。
    without_profile: bool,
}

/// Celeris 固有の欄（ADR D3.3）。どれを立てても health の結果は変わらない（J1）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Flags {
    /// 案件の題名（`Project.title`）と slug（`Project.slug`）を `celeris` にする。
    project_named_celeris: bool,
    /// 案件の repo の名前（`ProjectRepo.name`・`RepoRef.name`）を `celeris` にする（`RepoId` は ULID で名前を持たない）。
    repo_named_celeris: bool,
    /// task の mode（`Task.mode`）を `TaskMode::Prototype` にする。
    prototype: bool,
    /// selfdeploy の欄（`DispatchConfig.releases_dir` = `[selfdeploy] releases_dir`）を設定し、その親の
    /// `current/manifest.json` に fixture の base の sha を書く（base は変えない）。
    selfdeploy: bool,
    /// 案件の自動統合（`Project.auto_integrate`）を有効にする。`self_improve` という欄はコードに無い（進捗の記録）。
    auto_integrate: bool,
}

/// atomic の task（計画なし、reviewing）を fixture の repo 1 つで作る。`flags` が `None` なら既定の
/// `target_sync::reviewing_task`。案件の題名は `TaskStore` に書き込みの method が無いので、`sqlite`（`store` と
/// 同じ DB の handle）の transaction で書く。
fn review_task(
    repo: &std::path::Path,
    store: &Arc<dyn TaskStore>,
    sqlite: &SqliteStore,
    flags: Option<Flags>,
) -> Task {
    let Some(flags) = flags else {
        return reviewing_task(repo, store);
    };
    let name = if flags.repo_named_celeris {
        "celeris"
    } else {
        "code"
    };
    let (project, repos) = project_with_repos(store, &[(name, repo, task_core::RepoKind::Git)]);
    if flags.project_named_celeris {
        assert!(
            sqlite
                .write_tx::<_, task_core::StoreError, _>(|tx| {
                    SqliteStore::project_set_text_tx(tx, project, Some("celeris"), None)
                })
                .unwrap()
        );
        assert!(store.project_set_slug(project, "celeris").unwrap());
    }
    if flags.auto_integrate {
        assert!(store.project_set_auto_integrate(project, true).unwrap());
    }
    let mut task = git_task(
        repo,
        None,
        Check::Command {
            cmd: "git merge-base --is-ancestor main HEAD".into(),
            expect_exit: 0,
        },
    );
    task.project_id = Some(project);
    task.repos = repos.iter().map(task_core::RepoRef::of).collect();
    task.status = Status::Reviewing;
    if flags.prototype {
        task.mode = task_core::TaskMode::Prototype;
    }
    task
}

/// reviewer の合格の `review.json`（acceptance[1] が `Check::Reviewer`）。
pub(super) const REVIEW_PASS: &str =
    r#"{"verdicts":[{"criterion":1,"pass":true,"reason":"fine"}],"health":{"findings":[]}}"#;

/// fixture の候補を task の worktree に入れて review を起こし、止まるまで流す。review の repair（key `repair-*`）は
/// worktree を fixture の base に戻す。
async fn review_flow(
    bundled: &CheckerId,
    kind: &str,
    variant: &str,
    setup: ReviewSetup,
) -> ReviewFlow {
    let fixtures = tempfile::tempdir().unwrap();
    let repo = fixtures.path().join(kind);
    let fx = if setup.without_profile {
        build_fixture_without_profile(kind, variant, &repo)
    } else {
        build_fixture(kind, variant, &repo)
    };
    let changed = changed_paths(&repo, &fx);
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let mut task = review_task(&repo, &store, &sqlite, setup.flags);
    if setup.reviewer != Reviewer::None {
        task.acceptance.push(Criterion {
            text: "reviewer".into(),
            check: Check::Reviewer,
        });
    }
    store.insert(&task).unwrap();
    let base = fx.base().to_string();
    let requests = Arc::new(StdMutex::new(Vec::new()));
    let adapter: Arc<dyn WorkerAdapter> = match setup.reviewer {
        Reviewer::None => Arc::new(RepairActionAdapter::new(move |dir, _| {
            restore_base(dir, &base)
        })),
        Reviewer::Pass => {
            Arc::new(ParallelWuAdapter::new(Duration::from_millis(5)).with_review(REVIEW_PASS))
        }
        Reviewer::Candidates => Arc::new(CandidateReviewer {
            requests: requests.clone(),
        }),
    };
    let mut d = non_celeris_dispatcher(store.clone(), adapter, ws.path(), 1);
    let releases = tempfile::tempdir().unwrap();
    if setup.flags.is_some_and(|f| f.selfdeploy) {
        let current = releases.path().join("current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(
            current.join("manifest.json"),
            serde_json::json!({"sha": fx.base()}).to_string(),
        )
        .unwrap();
        let dir = releases.path().join("releases");
        std::fs::create_dir_all(&dir).unwrap();
        d.config.releases_dir = Some(dir);
    }
    let wt = d.local_worktree_for(&task).unwrap();
    wt.ensure_blocking().unwrap();
    assert_eq!(git_out(&wt.dir, &["rev-parse", "HEAD"]), fx.base());
    apply_candidate(&wt.dir, fx.head());
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert!(run_until_idle(&mut d, 800).await.idle);
    let events = events_of(&store, task.id);
    let status = store.get(task.id).unwrap().unwrap().status;
    let ws_path = ws.path().to_path_buf();
    let requests = requests.lock().unwrap().clone();
    ReviewFlow {
        _dirs: vec![fixtures, ws, releases],
        fx,
        changed,
        bundled: bundled.clone(),
        task,
        ws: ws_path,
        events,
        status,
        requests,
    }
}

/// 不変条件 (a)(g): atomic の task（計画なし）の direct review でも本物の検査器で health を当て（base = review の target
/// = fixture の base、head = 候補）、良い変更は Pass で task を done にする。repair にしない。
async fn direct_review_good_passes(kind: &str) {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let flow = review_flow(&bundled, kind, "good", ReviewSetup::default()).await;
    let items = review_health(&flow.events);
    let (pass, first) = items
        .first()
        .unwrap_or_else(|| panic!("the review did not evaluate health: {:?}", flow.events));
    assert!(*pass, "{first:?}");
    assert_pass(first, &flow.bundled);
    assert_eq!(first.base_sha, flow.fx.base());
    assert_eq!(first.head_sha, flow.fx.head());
    assert!(repair_classes(&flow.events).is_empty(), "{:?}", flow.events);
    assert_eq!(flow.status, Status::Done, "{:?}", flow.events);
}

#[tokio::test]
async fn health_generic_direct_review_ts_good_passes() {
    direct_review_good_passes("ts").await;
}

#[tokio::test]
async fn health_generic_direct_review_rust_good_passes() {
    direct_review_good_passes("rust").await;
}

/// 不変条件 (b)(g): atomic の direct review で、層の逆向きの依存は本物の検査器の fail 級の `deps.violation` で Fail に
/// なり、review の repair（`RepairScheduled` の class `health:deps.violation`、origin `Review`）に回る。修復の後の
/// review は新しい候補で評価し直して Pass にし、task を done にする。replan・人への質問にしない。
async fn direct_review_dep_inversion_routes_to_repair(kind: &str) {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let flow = review_flow(&bundled, kind, "dep-inversion", ReviewSetup::default()).await;
    let items = review_health(&flow.events);
    let (failed, passed) = match items.as_slice() {
        [first, .., last] => (first, last),
        _ => panic!("the review did not evaluate health twice: {items:?}"),
    };
    assert!(!failed.0, "{failed:?}");
    assert_dep_inversion_fail(kind, &failed.1, &flow.bundled, flow.fx.head());
    assert_eq!(failed.1.base_sha, flow.fx.base());
    assert!(passed.0, "{passed:?}");
    assert_pass(&passed.1, &flow.bundled);
    assert_ne!(passed.1.head_sha, flow.fx.head());
    assert_eq!(repair_classes(&flow.events), vec![repair_class(kind)]);
    assert!(
        flow.events
            .iter()
            .any(|e| matches!(e, Event::Transitioned { reason, .. } if reason == "review_repair")),
        "{:?}",
        flow.events
    );
    assert!(
        !flow
            .events
            .iter()
            .any(|e| matches!(e, Event::AutoReplanRequested { .. })),
        "a repairable health Fail must not replan"
    );
    assert!(!flow.changed.is_empty());
    assert_eq!(flow.status, Status::Done, "{:?}", flow.events);
}

#[tokio::test]
async fn health_generic_direct_review_ts_dep_inversion_routes_to_repair() {
    direct_review_dep_inversion_routes_to_repair("ts").await;
}

#[tokio::test]
async fn health_generic_direct_review_rust_dep_inversion_routes_to_repair() {
    direct_review_dep_inversion_routes_to_repair("rust").await;
}

// ---- final review の材料（候補の finding） ----

/// fixture の期待のラベル（`<rule_id>:<鍵>`）の rule。
fn expected_rule(fx: &Fixture) -> String {
    let label = fx
        .expected
        .first()
        .unwrap_or_else(|| panic!("the fixture has no expected label: {fx:?}"));
    label
        .split_once(':')
        .map_or(label.as_str(), |(rule, _)| rule)
        .to_string()
}

/// review が task dir の `runs/health/` に書いた report の、`rule` の new の finding。
fn reported_findings(flow: &ReviewFlow, rule: &str) -> Vec<serde_json::Value> {
    let dir = flow.ws.join(flow.task.id.to_string()).join("runs/health");
    let mut findings = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        findings.extend(
            report["findings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|f| f["rule_id"] == rule && f["delta"] == "new")
                .cloned(),
        );
    }
    findings
}

/// final review（reviewer run あり）を候補の finding の fixture で流す。
async fn review_input_flow(bundled: &CheckerId, kind: &str, variant: &str) -> ReviewFlow {
    let setup = ReviewSetup {
        reviewer: Reviewer::Pass,
        ..ReviewSetup::default()
    };
    review_flow(bundled, kind, variant, setup).await
}

/// 不変条件 (b): 名前の違う同じ契約（dup-contract）・同じ表の書き手の追加（dup-state）を、本物の検査器は候補の
/// finding（candidate=true・severity warning・delta new・exception null）として、候補が変えた file を evidence に
/// 出す。候補だけでは health は Fail にならず（verdict pass、fail 級 0）、final review は reviewer の合格で task を
/// done にする（候補は review の材料で、関門ではない。契約 ADR の S4）。
async fn review_input_candidate_does_not_fail(kind: &str, variant: &str) {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let flow = review_input_flow(&bundled, kind, variant).await;
    let items = review_health(&flow.events);
    assert!(!items.is_empty(), "the review did not evaluate health");
    for (pass, evidence) in &items {
        assert!(*pass, "{evidence:?}");
        assert_pass(evidence, &flow.bundled);
    }
    assert!(repair_classes(&flow.events).is_empty());
    assert_eq!(flow.status, Status::Done);
    let rule = expected_rule(&flow.fx);
    let findings = reported_findings(&flow, &rule);
    assert_eq!(findings.len(), 1, "{rule}: {findings:?}");
    let f = &findings[0];
    assert_eq!(f["candidate"], true, "{f}");
    assert_eq!(f["severity"], "warning", "{f}");
    assert!(f["exception"].is_null(), "{f}");
    let paths: Vec<&str> = f["evidence"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["path"].as_str())
        .collect();
    assert!(
        paths.iter().any(|p| flow.changed.iter().any(|c| c == p)),
        "the evidence {paths:?} must point at the candidate's change {:?}",
        flow.changed
    );
}

#[tokio::test]
async fn health_generic_review_input_ts_dup_contract_does_not_fail() {
    review_input_candidate_does_not_fail("ts", "dup-contract").await;
}

#[tokio::test]
async fn health_generic_review_input_ts_dup_state_does_not_fail() {
    review_input_candidate_does_not_fail("ts", "dup-state").await;
}

#[tokio::test]
async fn health_generic_review_input_rust_dup_contract_does_not_fail() {
    review_input_candidate_does_not_fail("rust", "dup-contract").await;
}

#[tokio::test]
async fn health_generic_review_input_rust_dup_state_does_not_fail() {
    review_input_candidate_does_not_fail("rust", "dup-state").await;
}

/// 不変条件 (b)（ADR (3) の `review_input_`）: 候補の finding は final review の材料（`HealthReviewInputBuilt` の
/// `input.findings.new`、= reviewer の `ReviewRequest.health`）に evidence の path 付きで入る。fixture の base の
/// profile は `[review]` 節を持たない（review 方針は `Absent`）ので、材料は `Present` と同じ構築で作られ、方針の値
/// （`owner_rows`・`invariants`）だけが空になる（ADR 2026-11-11-health-generic-review-input R1）。
/// 既存の試験で足りない理由: `health_review_input_` の材料の試験は stub の検査器だけで、本物の検査器の
/// `Absent` の profile と候補の finding（dup-contract・dup-state）を見ない。
async fn review_input_candidate_reaches_reviewer(kind: &str, variant: &str) {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let flow = review_input_flow(&bundled, kind, variant).await;
    let rule = expected_rule(&flow.fx);
    let inputs: Vec<&serde_json::Value> = flow
        .events
        .iter()
        .filter_map(|e| match e {
            Event::HealthReviewInputBuilt {
                reviewed_sha,
                input,
                ..
            } if reviewed_sha == flow.fx.head() => Some(input),
            _ => None,
        })
        .collect();
    let skipped: Vec<&String> = flow
        .events
        .iter()
        .filter_map(|e| match e {
            Event::HealthReviewInputSkipped { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(
        inputs.len(),
        1,
        "no HealthReviewInputBuilt for the candidate (skipped: {skipped:?})"
    );
    assert!(skipped.is_empty(), "{skipped:?}");
    let empty = |key: &str| match &inputs[0][key] {
        serde_json::Value::Array(a) => a.is_empty(),
        serde_json::Value::Object(o) => o.is_empty(),
        _ => false,
    };
    assert!(
        empty("owner_rows") && empty("invariants"),
        "no [review] in the base profile: {}",
        inputs[0]
    );
    let new: Vec<&serde_json::Value> = inputs[0]["findings"]["new"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["rule_id"] == rule.as_str())
        .collect();
    assert_eq!(new.len(), 1, "{}", inputs[0]);
    let path = new[0]["path"].as_str().unwrap_or_default();
    assert!(
        flow.changed.iter().any(|c| c == path),
        "{path:?} not in {:?}",
        flow.changed
    );
}

#[tokio::test]
async fn health_generic_review_input_ts_dup_contract_reaches_reviewer() {
    review_input_candidate_reaches_reviewer("ts", "dup-contract").await;
}

#[tokio::test]
async fn health_generic_review_input_ts_dup_state_reaches_reviewer() {
    review_input_candidate_reaches_reviewer("ts", "dup-state").await;
}

#[tokio::test]
async fn health_generic_review_input_rust_dup_contract_reaches_reviewer() {
    review_input_candidate_reaches_reviewer("rust", "dup-contract").await;
}

#[tokio::test]
async fn health_generic_review_input_rust_dup_state_reaches_reviewer() {
    review_input_candidate_reaches_reviewer("rust", "dup-state").await;
}

// ---- G4-R1: adapter が受けた ReviewRequest.health と atomic の final review の合否 ----
//
// 既存の `…_reaches_reviewer` は `HealthReviewInputBuilt` の event だけを見る。event は dispatcher が材料を作った記録で、
// reviewer の run の `RunContext` に載ったこと（配送）を示さない。材料を作っても `ReviewRequest.health` に入れ損ねる・
// 別の repo の材料を入れる・reviewer の判定を合否に使わない、という壊れ方は event だけでは検出できない。ここでは
// 試験の中だけの reviewer（[`CandidateReviewer`]。本体に hook を足さない）が実際に受けた `ReviewRequest.health` を
// 記録し、その中身と、受けた候補を根拠にした reviewer の不合格が task の状態を決めることを確かめる。report・event は
// 受信の写しの照合だけに使い、配送の証拠にしない。

/// 候補の finding の rule と、reviewer がそれを根拠に返す blocking の health finding の種別（汎用規約の 2・4）。
/// `dup.contract` は同じ契約の二重実装、`dup.state` は同じ表の書き手（状態の正本）の二重化。
const CANDIDATE_KINDS: [(&str, HealthFindingKind); 2] = [
    ("dup.contract", HealthFindingKind::DuplicateImplementation),
    ("dup.state", HealthFindingKind::DualSourceOfTruth),
];

/// 試験の中だけの reviewer。reviewer run（`TaskKind::Review`）で受けた `ReviewRequest.health` を記録し、その
/// `findings.new` の候補（[`CANDIDATE_KINDS`] の rule）ごとに blocking の finding（path は候補の path、reason は候補の
/// fingerprint を引く）を `review.json` に書く。候補が無ければ空の health の判定で合格にする。fixture・作業ツリーは
/// 読まず、受けた材料だけで決める（材料が届かなければ合格を書くので、届かない不具合は Done で表に出る）。ほかの run
/// （worker のやり直し）は何も変えずに終わる。
struct CandidateReviewer {
    requests: Arc<StdMutex<Vec<Option<HealthReviewInput>>>>,
}

impl CandidateReviewer {
    /// 受けた材料 `health` への `review.json`（acceptance[1] が `Check::Reviewer`）。
    fn review_json(health: Option<&HealthReviewInput>) -> String {
        let findings: Vec<serde_json::Value> = health
            .into_iter()
            .flat_map(|h| &h.repos)
            .flat_map(|repo| &repo.findings.new)
            .filter_map(|f| {
                let (_, kind) = CANDIDATE_KINDS.iter().find(|(rule, _)| *rule == f.rule_id)?;
                let path = f.path.as_deref()?;
                Some(serde_json::json!({
                    "kind": kind,
                    "severity": HealthSeverity::Blocking,
                    "path": path,
                    "reason": format!("{} at {path}: the candidate adds a second owner", f.fingerprint),
                }))
            })
            .collect();
        serde_json::json!({
            "verdicts": [{"criterion": 1, "pass": true, "reason": "the acceptance holds"}],
            "health": {"findings": findings},
        })
        .to_string()
    }
}

#[async_trait]
impl WorkerAdapter for CandidateReviewer {
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
        if req.task.kind == TaskKind::Review {
            let health = req.context.review.as_ref().and_then(|r| r.health.clone());
            std::fs::create_dir_all(&req.artifacts_dir).unwrap();
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                Self::review_json(health.as_ref()),
            )
            .unwrap();
            self.requests.lock().unwrap().push(health);
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

/// adapter が受けた 1 回目の reviewer run の `ReviewRequest.health` から、task の唯一の repo の材料を返す。repo・
/// base（= fixture の base）・head（= 候補）・diff（= 候補が変えた path の全部）・空の `owner_rows`/`invariants`
/// （base の profile に `[review]` が無い）・policy の状態（`source` は `policy_source`、`changed = false`）・材料に
/// した report を確かめる。同じ候補の `HealthReviewInputBuilt` の `input` が受けた材料と同じことも確かめる（event は
/// 受信の写しとして照合するだけ）。
fn received_input<'a>(flow: &'a ReviewFlow, policy_source: &str) -> &'a HealthRepoInput {
    let skipped: Vec<&String> = flow
        .events
        .iter()
        .filter_map(|e| match e {
            Event::HealthReviewInputSkipped { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert!(
        !flow.requests.is_empty(),
        "no reviewer run: {:?}",
        flow.events
    );
    assert!(
        flow.requests.iter().all(Option::is_some),
        "every reviewer run gets ReviewRequest.health (skipped: {skipped:?})"
    );
    let health = flow.requests[0].as_ref().expect("checked above");
    assert!(health.brief.is_none(), "an atomic task has no plan");
    assert_eq!(health.repos.len(), 1, "{health:?}");
    let repo = &health.repos[0];
    assert_eq!(repo.repo_id, flow.task.repos[0].repo_id.to_string());
    assert_eq!(repo.target_sha, flow.fx.base());
    assert_eq!(repo.reviewed_sha, flow.fx.head());
    let mut diff: Vec<&str> = repo.diff.paths.iter().map(|p| p.path.as_str()).collect();
    diff.sort_unstable();
    let mut changed: Vec<&str> = flow.changed.iter().map(String::as_str).collect();
    changed.sort_unstable();
    assert!(!changed.is_empty());
    assert_eq!(diff, changed, "{:?}", repo.diff);
    assert_eq!(repo.diff.files, changed.len() as u64, "{:?}", repo.diff);
    assert!(
        repo.owner_rows.is_empty() && repo.invariants.is_empty(),
        "no [review] in the base profile: {repo:?}"
    );
    assert_eq!(repo.policy.source, policy_source, "{:?}", repo.policy);
    assert!(
        !repo.policy.changed && repo.policy.files.is_empty(),
        "{:?}",
        repo.policy
    );
    assert!(
        repo.findings.reviewable && repo.findings.reason.is_none(),
        "{:?}",
        repo.findings
    );
    assert_eq!(repo.findings.verdict, "pass", "{:?}", repo.findings);
    assert!(skipped.is_empty(), "{skipped:?}");
    let built: Vec<&serde_json::Value> = flow
        .events
        .iter()
        .filter_map(|e| match e {
            Event::HealthReviewInputBuilt {
                reviewed_sha,
                input,
                ..
            } if reviewed_sha == flow.fx.head() => Some(input),
            _ => None,
        })
        .collect();
    assert_eq!(built.len(), 1, "one material per candidate");
    assert_eq!(*built[0], serde_json::to_value(repo).unwrap());
    repo
}

/// review の不合格の verdict のうち reason に `tag` を含むもの。
fn failed_verdicts<'a>(events: &'a [Event], tag: &str) -> Vec<&'a str> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ReviewVerdict {
                pass: false,
                reason,
                ..
            } if reason.contains(tag) => Some(reason.as_str()),
            _ => None,
        })
        .collect()
}

/// review が task を Done にした（`review_pass` の遷移）か。
fn review_passed(events: &[Event]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, Event::Transitioned { reason, .. } if reason == "review_pass"))
}

/// 候補の reviewer（[`Reviewer::Candidates`]）で atomic の task の final review を流す。
async fn request_flow(
    bundled: &CheckerId,
    kind: &str,
    variant: &str,
    without_profile: bool,
) -> ReviewFlow {
    let setup = ReviewSetup {
        reviewer: Reviewer::Candidates,
        without_profile,
        ..ReviewSetup::default()
    };
    review_flow(bundled, kind, variant, setup).await
}

/// 不変条件（ADR 2026-11-11-health-generic-review-input R1・R5、ADR 2026-11-07-health-review-integration D5.3）:
/// review 方針の無い汎用 repo（fixture の profile か、profile の無い repo の組み込み generic）の atomic の final review で、
/// adapter が実際に受けた `ReviewRequest.health` に、その repo・base・head・diff と、本物の検査器が出した候補の
/// finding（rule は fixture の期待、evidence の path は候補が変えた file）が入り、`owner_rows`・`invariants` は空。
/// 受けた候補を根拠に reviewer が blocking の health finding を返すと、review は不合格になり task は Done にならない
/// （検査器の health の項目は合格のままで、不合格は reviewer の判定から来る）。
/// 既存の試験で足りない理由: `…_reaches_reviewer` は `HealthReviewInputBuilt` の event だけを見るので、材料が
/// reviewer の run に載ったこと（配送）と、受けた候補による reviewer の不合格が task の状態を止めることを確かめない。
/// `health_review_input_policy_absent_reaches_the_reviewer_request` は stub の検査器の固定の report で、本物の検査器の
/// 候補・組み込み generic を通らない。
async fn request_candidate_blocks_done(kind: &str, variant: &str, without_profile: bool) {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let flow = request_flow(&bundled, kind, variant, without_profile).await;
    let source = if without_profile { "builtin" } else { "base" };
    let repo = received_input(&flow, source);
    let rule = expected_rule(&flow.fx);
    let candidates: Vec<_> = repo
        .findings
        .new
        .iter()
        .filter(|f| f.rule_id == rule)
        .collect();
    assert_eq!(candidates.len(), 1, "{rule}: {:?}", repo.findings);
    let candidate = candidates[0];
    assert_eq!(
        (candidate.severity.as_str(), candidate.delta.as_str()),
        ("warning", "new"),
        "{candidate:?}"
    );
    let path = candidate.path.as_deref().unwrap_or_default();
    assert!(
        flow.changed.iter().any(|c| c == path),
        "the candidate's evidence {path:?} must be a changed file {:?}",
        flow.changed
    );
    let (_, finding_kind) = CANDIDATE_KINDS
        .iter()
        .find(|(r, _)| *r == rule)
        .unwrap_or_else(|| panic!("{rule} is not a candidate rule"));
    let tag = format!("health:{}", finding_kind.as_str());
    let failed = failed_verdicts(&flow.events, &tag);
    assert!(
        !failed.is_empty(),
        "the reviewer's blocking {tag} must fail the review: {:?}",
        flow.events
    );
    assert!(
        failed
            .iter()
            .all(|r| r.contains(&candidate.fingerprint) && !r.contains("ungrounded")),
        "the blocking finding is grounded on the received candidate: {failed:?}"
    );
    let items = review_health(&flow.events);
    assert!(
        !items.is_empty() && items.iter().all(|(pass, _)| *pass),
        "the checker's health item passes; the reviewer fails the review: {items:?}"
    );
    assert!(!review_passed(&flow.events), "{:?}", flow.events);
    // substantive の blocking は局所修復でなく `ReviewFail`。やり直しの無い task（`max_retries = 0`）は Failed で止まる。
    assert_eq!(flow.requests.len(), 1, "one reviewer run");
    assert_eq!(flow.status, Status::Failed, "{:?}", flow.events);
}

#[tokio::test]
async fn g4_r1_review_request_ts_dup_contract_fails_review() {
    request_candidate_blocks_done("ts", "dup-contract", false).await;
}

#[tokio::test]
async fn g4_r1_review_request_ts_dup_state_fails_review() {
    request_candidate_blocks_done("ts", "dup-state", false).await;
}

#[tokio::test]
async fn g4_r1_review_request_rust_dup_contract_fails_review() {
    request_candidate_blocks_done("rust", "dup-contract", false).await;
}

#[tokio::test]
async fn g4_r1_review_request_rust_dup_state_fails_review() {
    request_candidate_blocks_done("rust", "dup-state", false).await;
}

#[tokio::test]
async fn g4_r1_review_request_builtin_generic_dup_contract_fails_review() {
    request_candidate_blocks_done("ts", "dup-contract", true).await;
}

#[tokio::test]
async fn g4_r1_review_request_builtin_generic_dup_state_fails_review() {
    request_candidate_blocks_done("rust", "dup-state", true).await;
}

/// 不変条件（同じ ADR の R1・R5）: 良い変更（fixture の期待のラベルが無い）でも、adapter が受けた
/// `ReviewRequest.health` には同じ repo・base・head・diff の材料が入り、`owner_rows`・`invariants` は空で、二重化の
/// 候補（`dup.contract`・`dup.state`）は無い。同じ reviewer は不合格を返さず、task は 1 回の reviewer run で Done になる。
/// 既存の試験で足りない理由: 候補の試験（`…_fails_review`）だけでは、reviewer が材料に関係なく不合格にする・材料が
/// 届かない場合と区別できない。`health_generic_direct_review_*_good_passes` は reviewer run を起こさず、受信の材料を
/// 見ない。
async fn request_good_is_done(kind: &str, without_profile: bool) {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let flow = request_flow(&bundled, kind, "good", without_profile).await;
    assert!(flow.fx.expected.is_empty(), "{:?}", flow.fx);
    let source = if without_profile { "builtin" } else { "base" };
    let repo = received_input(&flow, source);
    assert!(
        repo.findings
            .new
            .iter()
            .all(|f| CANDIDATE_KINDS.iter().all(|(rule, _)| *rule != f.rule_id)),
        "no duplication candidate for a good change: {:?}",
        repo.findings
    );
    assert_eq!(flow.requests.len(), 1, "one reviewer run");
    assert!(
        flow.events
            .iter()
            .all(|e| !matches!(e, Event::ReviewVerdict { pass: false, .. })),
        "{:?}",
        flow.events
    );
    assert!(review_passed(&flow.events), "{:?}", flow.events);
    assert_eq!(flow.status, Status::Done, "{:?}", flow.events);
}

#[tokio::test]
async fn g4_r1_review_request_ts_good_is_done() {
    request_good_is_done("ts", false).await;
}

#[tokio::test]
async fn g4_r1_review_request_rust_good_is_done() {
    request_good_is_done("rust", false).await;
}

#[tokio::test]
async fn g4_r1_review_request_builtin_generic_good_is_done() {
    request_good_is_done("ts", true).await;
}

// ---- 複数 repo ----

/// `(repo_id, pass, evidence)` の review の health の項目。
fn review_health_by_repo(events: &[Event]) -> Vec<(task_core::RepoId, bool, HealthEvidence)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::RequiredChecksEvaluated {
                repo_id, results, ..
            } => Some((*repo_id, results)),
            _ => None,
        })
        .flat_map(|(repo_id, results)| {
            results
                .iter()
                .filter(|r| r.cmd == HEALTH_CHECK_ID)
                .filter_map(move |r| Some((repo_id, r.pass, (**r.health.as_ref()?).clone())))
        })
        .collect()
}

/// 2 repo（`ts`・`rust` の順）の atomic の task を reviewing で作り、各 repo の worktree に `fixtures` の候補を入れて
/// review を流す。review の repair は 2 つ目の repo の worktree を base に戻す。
async fn multi_repo_flow(
    fixtures: &[(&std::path::Path, &Fixture)],
) -> (Vec<Event>, Status, Vec<task_core::ProjectRepo>) {
    let ws = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let specs: Vec<(&str, &std::path::Path, task_core::RepoKind)> = ["ts", "rust"]
        .iter()
        .zip(fixtures)
        .map(|(name, (dir, _))| (*name, *dir, task_core::RepoKind::Git))
        .collect();
    let (project, repos) = project_with_repos(&store, &specs);
    let mut task = git_task(
        fixtures[0].0,
        None,
        Check::Command {
            cmd: "git merge-base --is-ancestor main HEAD".into(),
            expect_exit: 0,
        },
    );
    task.project_id = Some(project);
    task.repos = repos.iter().map(task_core::RepoRef::of).collect();
    task.status = Status::Reviewing;
    store.insert(&task).unwrap();
    let second_base = fixtures[1].1.base().to_string();
    let adapter = Arc::new(RepairActionAdapter::new(move |dir, _| {
        let second = dir.parent().expect("repos dir").join("rust");
        restore_base(&second, &second_base);
    }));
    let mut d = non_celeris_dispatcher(store.clone(), adapter, ws.path(), 1);
    let workspaces = d.task_workspaces_for(&task).expect("workspaces");
    assert_eq!(workspaces.repos.len(), 2);
    for (repo, (_, fx)) in workspaces.repos.iter().zip(fixtures) {
        repo.worktree
            .as_ref()
            .expect("a git worktree")
            .ensure_blocking()
            .unwrap();
        apply_candidate(&repo.dir, fx.head());
    }
    assert!(
        d.spawn_review(task.id, "worker".into(), &ReviewSubject::default())
            .unwrap()
    );
    assert!(run_until_idle(&mut d, 800).await.idle);
    let status = store.get(task.id).unwrap().unwrap().status;
    (events_of(&store, task.id), status, repos)
}

/// 不変条件 (g): builder の multi（ts と rust の 2 repo）の良い変更では、review は 2 つの repo のそれぞれで本物の
/// 検査器の health を評価し（repo ごとに base・head はその repo の fixture）、どちらも Pass で task を done にする。
#[tokio::test]
async fn health_generic_multi_repo_each_repo_checked() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let fixtures = tempfile::tempdir().unwrap();
    let fx = build_fixture("multi", "good", fixtures.path());
    assert_eq!(fx.repos, ["ts", "rust"]);
    let dirs: Vec<PathBuf> = fx.repos.iter().map(|r| fixtures.path().join(r)).collect();
    let per_repo: Vec<Fixture> = dirs
        .iter()
        .map(|dir| Fixture {
            base: Some(git_out(dir, &["rev-parse", "main"])),
            head: Some(git_out(dir, &["rev-parse", "candidate"])),
            repos: vec![".".into()],
            expected: Vec::new(),
        })
        .collect();
    let pairs: Vec<(&std::path::Path, &Fixture)> =
        dirs.iter().map(PathBuf::as_path).zip(&per_repo).collect();
    let (events, status, repos) = multi_repo_flow(&pairs).await;
    let items = review_health_by_repo(&events);
    for (repo, fx) in repos.iter().zip(&per_repo) {
        let (_, pass, evidence) = items
            .iter()
            .find(|(id, ..)| *id == repo.id)
            .unwrap_or_else(|| panic!("no health item for the repo {}: {items:?}", repo.name));
        assert!(*pass, "{evidence:?}");
        assert_pass(evidence, &bundled);
        assert_eq!(evidence.base_sha, fx.base(), "{}", repo.name);
        assert_eq!(evidence.head_sha, fx.head(), "{}", repo.name);
    }
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(status, Status::Done, "{events:?}");
}

/// 不変条件 (g): 2 repo の task で、1 つ目の repo（ts の良い変更）が Pass でも、2 つ目の repo（rust の層の逆向き）の
/// fail 級の finding が review を Fail にし、統合（done）を止めて review の repair に回す（class は 2 つ目の repo の
/// rule だけ）。修復の後は 2 つの repo とも評価し直して Pass にし、task を done にする。
#[tokio::test]
async fn health_generic_multi_repo_second_repo_violation_blocks_integration() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let fixtures = tempfile::tempdir().unwrap();
    let (ts, rust) = (fixtures.path().join("ts"), fixtures.path().join("rust"));
    let good = build_fixture("ts", "good", &ts);
    let inverted = build_fixture("rust", "dep-inversion", &rust);
    let (events, status, repos) =
        multi_repo_flow(&[(ts.as_path(), &good), (rust.as_path(), &inverted)]).await;
    let items = review_health_by_repo(&events);
    let of = |repo: &task_core::ProjectRepo| -> Vec<(bool, HealthEvidence)> {
        items
            .iter()
            .filter(|(id, ..)| *id == repo.id)
            .map(|(_, pass, h)| (*pass, h.clone()))
            .collect()
    };
    let (first, second) = (of(&repos[0]), of(&repos[1]));
    assert!(
        first.len() >= 2 && first.iter().all(|(pass, _)| *pass),
        "the first repo passes in every review: {first:?}"
    );
    assert_pass(&first[0].1, &bundled);
    assert_eq!(first[0].1.head_sha, good.head());
    let (failed, passed) = match second.as_slice() {
        [first, .., last] => (first, last),
        _ => panic!("the second repo was not evaluated twice: {second:?}"),
    };
    assert!(!failed.0, "{failed:?}");
    assert_dep_inversion_fail("rust", &failed.1, &bundled, inverted.head());
    assert_eq!(failed.1.base_sha, inverted.base());
    assert!(passed.0, "{passed:?}");
    assert_pass(&passed.1, &bundled);
    assert_eq!(repair_classes(&events), vec![repair_class("rust")]);
    let review_pass_after_repair = events
        .iter()
        .position(|e| matches!(e, Event::Transitioned { reason, .. } if reason == "review_pass"));
    let repair = events
        .iter()
        .position(|e| matches!(e, Event::Transitioned { reason, .. } if reason == "review_repair"));
    assert!(
        matches!((repair, review_pass_after_repair), (Some(r), Some(p)) if r < p),
        "the task must not pass review before the second repo is repaired: {events:?}"
    );
    assert_eq!(status, Status::Done);
}

// ---- Celeris 固有の欄 ----

/// review の結果のうち、Celeris 固有の欄で変わってはならないもの: health の評価ごとの判定と fail 級の finding、review
/// の repair の class、task の最後の状態。
/// `(評価ごとの (判定, fail 級の fingerprint), review の repair の class, 最後の状態)`。
type Outcome = (Vec<(HealthVerdict, Vec<String>)>, Vec<String>, Status);

fn outcome(flow: &ReviewFlow) -> Outcome {
    let verdicts = review_health(&flow.events)
        .into_iter()
        .map(|(_, h)| (h.verdict, h.blocking))
        .collect();
    (verdicts, repair_classes(&flow.events), flow.status)
}

/// 不変条件 K5・J1（ADR D3.3）: 案件名・repo 名を `celeris` に、task の mode を prototype に、selfdeploy の欄
/// （`releases_dir`）を設定し、案件の自動統合を有効にしても、同じ fixture の health の判定と fail 級の finding の
/// 集合（と repair・最後の状態）は既定の task と同じ。fail 級の出る fixture（ts の dep-inversion）と出ない fixture
/// （rust の good）の両方で確かめる。
#[tokio::test]
async fn health_generic_flags_do_not_change_outcome() {
    let (_bundle, bundled) = configure_real_bundled_checker();
    let all = Flags {
        project_named_celeris: true,
        repo_named_celeris: true,
        prototype: true,
        selfdeploy: true,
        auto_integrate: true,
    };
    let configs = [
        Flags {
            project_named_celeris: true,
            ..Flags::default()
        },
        Flags {
            repo_named_celeris: true,
            ..Flags::default()
        },
        Flags {
            prototype: true,
            ..Flags::default()
        },
        Flags {
            selfdeploy: true,
            ..Flags::default()
        },
        Flags {
            auto_integrate: true,
            ..Flags::default()
        },
        all,
    ];
    for (kind, variant, verdict) in [
        ("ts", "dep-inversion", HealthVerdict::Fail),
        ("rust", "good", HealthVerdict::Pass),
    ] {
        let baseline = outcome(&review_flow(&bundled, kind, variant, ReviewSetup::default()).await);
        assert_eq!(
            baseline.0.first().map(|(v, _)| *v),
            Some(verdict),
            "{kind}/{variant}: {baseline:?}"
        );
        for flags in configs {
            let setup = ReviewSetup {
                flags: Some(flags),
                ..ReviewSetup::default()
            };
            let flow = review_flow(&bundled, kind, variant, setup).await;
            assert_eq!(outcome(&flow), baseline, "{kind}/{variant} with {flags:?}");
        }
    }
}
