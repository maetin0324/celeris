//! ADR 2026-11-08-health-enforcement-loop D6.1・D6.3〜D6.5 の runner の試験（接頭辞 `health_inventory_`）。
//! 検査器は `gate_tests` と同じ stub（一時 git repo の base に commit した `repo_health.py`。`count` の file に
//! 起動ごとに 1 行足す）で、外部ネットワークに出ず、LLM を呼ばない。既存の試験は cron の種類・表・起票を別々に
//! 見るだけで、runner の base の選び方・省略・検査器の選択・説明の出どころを検出できないので、ここで固定する。

use std::path::Path;

use serde_json::{Value, json};
use task_core::cron::{CronCatchUp, CronOverlap, CronTaskTemplate};
use task_core::required_checks::health::CheckerSource;
use task_core::{Project, ProjectStatus, RepoRun};
use task_ops::cron_jobs::{CronFireContext, NewCronJob, create_job};

use super::*;
use crate::health_review::gate::tests::{
    SRC, TIMEOUT, applied, base_policy, checker_files, commit, finding, init_repo, report, rule,
    sh_git,
};

fn at(minute: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000) + time::Duration::minutes(minute)
}

struct Fixture {
    store: Arc<SqliteStore>,
    project: ProjectId,
}

fn fixture() -> Fixture {
    let store = SqliteStore::open_in_memory().expect("store");
    let now = at(0);
    let project = Project {
        auto_advance: false,
        auto_integrate: false,
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
        store: Arc::new(store),
        project: project.id,
    }
}

impl Fixture {
    fn repo(&self, name: &str, location: WorkspaceSpec) -> ProjectRepo {
        let repo = ProjectRepo {
            id: RepoId::new(),
            project_id: self.project,
            name: name.into(),
            kind: RepoKind::Git,
            location,
            default_branch: None,
            sync: None,
            run: RepoRun::Auto,
            is_primary: false,
            created_at: at(0),
        };
        self.store.repo_create(&repo).expect("repo");
        repo
    }
}

fn fire(job_id: CronJobId, minute: i64, bundled: Option<CheckerId>) -> InventoryFire {
    InventoryFire {
        job_id,
        scheduled_for: at(minute),
        now: at(minute),
        bundled,
        timeout: TIMEOUT,
    }
}

/// stub の設定: `exit` で終わり、`report` を `--out` に書き、起動ごとに `count` に 1 行足す。
fn counted_stub(exit: i32, report: &str, count: &Path) -> String {
    json!({"exit": exit, "report": report, "count": count.to_string_lossy()}).to_string()
}

/// base に検査器を commit した repo（`count` は repo の外の file）。
fn checked_repo(exit: i32, report: &str) -> (tempfile::TempDir, tempfile::TempDir, String) {
    let dir = init_repo();
    let counts = tempfile::tempdir().expect("count dir");
    let stub = counted_stub(exit, report, &counts.path().join("count"));
    let files = checker_files(&stub, true);
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, b)| (*p, b.as_str())).collect();
    let head = commit(dir.path(), &refs, "checker");
    (dir, counts, head)
}

fn runs_of(counts: &tempfile::TempDir) -> usize {
    std::fs::read_to_string(counts.path().join("count"))
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

fn pass_report() -> String {
    report("pass", 0, Vec::new(), base_policy(false), Vec::new())
}

fn local(dir: &Path) -> WorkspaceSpec {
    WorkspaceSpec::local(dir.to_string_lossy().as_ref())
}

fn latest(store: &SqliteStore, job: CronJobId, repo: RepoId) -> HealthEvidence {
    store
        .health_inventory_latest_evaluated(job, repo)
        .expect("read")
        .and_then(|row| row.evidence)
        .expect("an evaluated row")
}

/// 不変条件（D6.3）: 初回（評価できた行が無い）は base = head で評価し、行を 1 件追記するだけで task を作らない。
#[test]
fn health_inventory_first_run_records_without_filing() {
    let f = fixture();
    let (dir, counts, head) = checked_repo(0, &pass_report());
    let repo = f.repo("code", local(dir.path()));
    let job = CronJobId(Ulid::new());

    fire(job, 1, None).run(&f.store, std::slice::from_ref(&repo));

    let ev = latest(&f.store, job, repo.id);
    assert_eq!(ev.verdict, HealthVerdict::Pass, "{ev:?}");
    assert_eq!(
        (ev.base_sha.as_str(), ev.head_sha.as_str()),
        (head.as_str(), head.as_str())
    );
    assert_eq!(runs_of(&counts), 1);
    let counts = f
        .store
        .health_inventory_fire_counts(job, at(1))
        .expect("counts");
    assert_eq!(
        (counts.repos, counts.evaluated, counts.tasks_created),
        (1, 1, 0)
    );
    assert!(f.store.list(None).expect("tasks").is_empty());
}

/// 不変条件（D6.4）: `(head_tree, checker.sha256)` が最後に評価できた行と同じなら検査器を走らせず、`unchanged` の
/// 行だけを足す。tree が同じ別の commit も省略する。tree が変われば評価する。
#[test]
fn health_inventory_skips_unchanged_repo() {
    let f = fixture();
    let (dir, counts, first) = checked_repo(0, &pass_report());
    let repo = f.repo("code", local(dir.path()));
    let job = CronJobId(Ulid::new());
    fire(job, 1, None).run(&f.store, std::slice::from_ref(&repo));
    assert_eq!(runs_of(&counts), 1);

    sh_git(
        dir.path(),
        &["commit", "-q", "--allow-empty", "-m", "same tree"],
    );
    fire(job, 2, None).run(&f.store, std::slice::from_ref(&repo));

    assert_eq!(runs_of(&counts), 1, "the checker ran for an unchanged tree");
    let skipped = f
        .store
        .health_inventory_fire_counts(job, at(2))
        .expect("counts");
    assert_eq!(
        (skipped.repos, skipped.unchanged, skipped.evaluated),
        (1, 1, 0)
    );
    assert_eq!(latest(&f.store, job, repo.id).head_sha, first);

    let changed = commit(dir.path(), &[(SRC, "fn changed() {}\n")], "change");
    fire(job, 3, None).run(&f.store, std::slice::from_ref(&repo));

    assert_eq!(runs_of(&counts), 2);
    let ev = latest(&f.store, job, repo.id);
    assert_eq!(
        (ev.base_sha.as_str(), ev.head_sha.as_str()),
        (first.as_str(), changed.as_str())
    );
}

/// 不変条件（D6.3）: base は最後に評価できた行の head_sha。verdict が Error の行は飛ばす。
#[test]
fn health_inventory_evaluates_previous_head_as_base() {
    let f = fixture();
    let (dir, _counts, first) = checked_repo(0, &pass_report());
    let repo = f.repo("code", local(dir.path()));
    let job = CronJobId(Ulid::new());
    let at_one = fire(job, 1, None);
    at_one.run(&f.store, std::slice::from_ref(&repo));

    let broken = commit(dir.path(), &[(SRC, "fn broken() {}\n")], "broken");
    let mut error = latest(&f.store, job, repo.id);
    error.head_sha = broken;
    error.verdict = HealthVerdict::Error;
    let error_row = RepoInventory {
        outcome: HealthInventoryOutcome::Evaluated,
        evidence: Some(error),
        details: Vec::new(),
    };
    fire(job, 2, None)
        .record(&f.store, repo.id, error_row)
        .expect("error row");

    let head = commit(dir.path(), &[(SRC, "fn later() {}\n")], "later");
    fire(job, 3, None).run(&f.store, std::slice::from_ref(&repo));

    let ev = latest(&f.store, job, repo.id);
    assert_eq!(ev.verdict, HealthVerdict::Pass, "{ev:?}");
    assert_eq!(
        (ev.base_sha.as_str(), ev.head_sha.as_str()),
        (first.as_str(), head.as_str())
    );
}

/// 不変条件（D6.1）: 同じ job の棚卸しが走っている間の発火は Busy で、印は `in_flight` に数える。終われば次の発火が
/// 始まり、repo ごとの行が表に残る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_inventory_busy_while_running() {
    let f = fixture();
    let (dir, _counts, _head) = checked_repo(0, &pass_report());
    f.repo("code", local(dir.path()));
    let job = inventory_job(&f.store, f.project);
    let mut runs = InventoryRuns::default();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    runs.start(job.id, move || {
        let _ = wait.recv();
    })
    .expect("start");

    let busy = start_inventory(
        Some(&f.store),
        &mut runs,
        &job,
        at(1),
        &f.project.to_string(),
        at(1),
    );
    assert!(matches!(busy, Ok(CronInputPost::Busy(_))), "{busy:?}");
    assert_eq!(runs.in_flight(), 1);

    release.send(()).expect("release");
    runs.join_all().await;
    assert_eq!(runs.in_flight(), 0);
    let posted = start_inventory(
        Some(&f.store),
        &mut runs,
        &job,
        at(2),
        &f.project.to_string(),
        at(2),
    );
    match posted {
        Ok(CronInputPost::Posted(detail)) => assert!(detail.contains("1 git repo(s)"), "{detail}"),
        other => panic!("{other:?}"),
    }
    runs.join_all().await;
    let counts = f
        .store
        .health_inventory_fire_counts(job.id, at(2))
        .expect("counts");
    assert_eq!((counts.repos, counts.evaluated), (1, 1));
    assert!(start_inventory(None, &mut runs, &job, at(3), &f.project.to_string(), at(3)).is_err());
}

fn inventory_job(store: &SqliteStore, project: ProjectId) -> CronJob {
    let mut template = CronTaskTemplate {
        title: "棚卸し".into(),
        ..CronTaskTemplate::default()
    };
    template
        .extra
        .insert("kind".into(), json!("health_inventory"));
    template
        .extra
        .insert("project_id".into(), json!(project.to_string()));
    create_job(
        store,
        &CronFireContext::default(),
        NewCronJob {
            name: "inventory".into(),
            schedule: "0 0 * * 1".into(),
            timezone: "UTC".into(),
            overlap: CronOverlap::Skip,
            catch_up: CronCatchUp::Latest,
            enabled: true,
            template,
        },
        at(0),
    )
    .expect("job")
}

/// 不変条件: 検査器は 1 repo につき 1 回走り、blocking の説明は同じ report の finding から作る（`blocking` と同じ
/// fingerprint・同じ順、path は evidence の path、message は rationale）。
#[test]
fn health_inventory_details_come_from_same_run() {
    let with_rationale = |mut v: Value, why: &str| {
        v["rationale"] = json!(why);
        v
    };
    let findings = vec![
        with_rationale(finding("size.y", 2, "error", "new", false), "second"),
        with_rationale(finding("size.x", 1, "error", "worsened", false), "first"),
        finding("size.x", 3, "warning", "new", false),
        finding("size.x", 4, "error", "existing", false),
    ];
    let text = report(
        "fail",
        1,
        findings,
        base_policy(false),
        vec![rule("size.x", "fail", "")],
    );
    let (dir, counts, _head) = checked_repo(1, &text);
    let f = fixture();
    let repo = f.repo("code", local(dir.path()));

    let got = fire(CronJobId(Ulid::new()), 1, None)
        .inventory_repo(&f.store, &repo)
        .expect("inventory");

    assert_eq!(runs_of(&counts), 1);
    let ev = got.evidence.expect("evidence");
    assert_eq!(ev.verdict, HealthVerdict::Fail, "{ev:?}");
    let fingerprints: Vec<&str> = got.details.iter().map(|d| d.fingerprint.as_str()).collect();
    assert_eq!(
        fingerprints,
        ev.blocking.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(
        fingerprints,
        [format!("size.x:{:016x}", 1), format!("size.y:{:016x}", 2)]
    );
    assert_eq!(got.details[0].rule_id, "size.x");
    assert_eq!(got.details[0].paths, [SRC]);
    assert_eq!(got.details[0].message, "first");
    assert_eq!(got.details[1].message, "second");
}

/// 不変条件（D1.1・D6.3）: 検査器は `repo_facts` → `applicability` だけで選ぶ。base の検査器があれば同梱より
/// base、base に無ければ同梱、どちらも無ければ Error の行（`checker_missing`）、remote workspace は検査器を
/// 走らせず `not_applicable` の行。
#[test]
fn health_inventory_selects_checker_by_applicability() {
    let f = fixture();
    let bundled = CheckerId {
        source: CheckerSource::Bundled,
        sha256: "b".repeat(64),
    };
    let job = CronJobId(Ulid::new());
    let (with_checker, counts, head) = checked_repo(0, &pass_report());
    let based = f.repo("based", local(with_checker.path()));
    let plain_dir = init_repo();
    let plain = f.repo("plain", local(plain_dir.path()));
    let remote = f.repo(
        "remote",
        WorkspaceSpec::Remote {
            cluster: "pegasus".into(),
            path: "/work/x".into(),
            mode: None,
        },
    );

    let base_first = fire(job, 1, Some(bundled.clone()))
        .inventory_repo(&f.store, &based)
        .expect("based");
    let ev = base_first.evidence.expect("evidence");
    assert_eq!(
        ev.checker,
        Some(applied(with_checker.path(), &head, Some(&bundled)))
    );
    assert_eq!(
        ev.checker.as_ref().map(|c| c.source),
        Some(CheckerSource::Base)
    );
    assert_eq!(runs_of(&counts), 1);

    let to_bundled = fire(job, 1, Some(bundled.clone()))
        .inventory_repo(&f.store, &plain)
        .expect("plain");
    assert_eq!(
        to_bundled.evidence.expect("evidence").checker,
        Some(bundled.clone())
    );

    let missing = fire(job, 1, None)
        .inventory_repo(&f.store, &plain)
        .expect("missing");
    let ev = missing.evidence.expect("evidence");
    assert_eq!(
        (ev.verdict, ev.checker.clone()),
        (HealthVerdict::Error, None)
    );
    assert!(
        ev.reason
            .as_deref()
            .unwrap_or("")
            .contains("checker_missing"),
        "{ev:?}"
    );

    let skipped = fire(job, 1, Some(bundled))
        .inventory_repo(&f.store, &remote)
        .expect("remote");
    assert_eq!(skipped.outcome, HealthInventoryOutcome::NotApplicable);
    assert_eq!(skipped.evidence, None);
}
