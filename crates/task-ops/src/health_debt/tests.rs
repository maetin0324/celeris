//! D6.6・D6.7 の起票の試験。既存の試験は task の作成の経路（`add`）と冪等キーの store（`idempotency`）を
//! 別々に見るだけで、棚卸しの「blocking だけ・Fail だけ・終端でない debt があれば省く・1 回 3 件まで・draft だけ」
//! の契約を持たないので、ここで固定する。
use super::*;
use task_core::{
    Project, ProjectRepo, ProjectStatus, RepoId, RepoKind, RepoRun, Task, Trigger, WorkspaceSpec,
};

struct Fixture {
    store: SqliteStore,
    project: ProjectId,
    repo: ProjectRepo,
}

fn fixture() -> Fixture {
    let store = SqliteStore::open_in_memory().expect("store");
    let now = OffsetDateTime::now_utc();
    let project = Project {
        auto_advance: false,
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
    let repo = ProjectRepo {
        auto_integrate: false,
        id: RepoId::new(),
        project_id: project.id,
        name: "code".into(),
        kind: RepoKind::Git,
        location: WorkspaceSpec::local("/srv/code"),
        default_branch: None,
        sync: None,
        run: RepoRun::Auto,
        is_primary: true,
        created_at: now,
    };
    store.repo_create(&repo).expect("repo");
    Fixture {
        store,
        project: project.id,
        repo,
    }
}

fn evidence(repo: RepoId, head: &str, verdict: HealthVerdict, blocking: &[&str]) -> HealthEvidence {
    HealthEvidence {
        repo_id: repo,
        base_sha: "b".repeat(40),
        head_sha: head.to_owned(),
        head_tree: "t".repeat(40),
        policy_sha256: "p".repeat(64),
        checker: None,
        report_sha256: "r".repeat(64),
        tool_version: "1".into(),
        verdict,
        blocking: blocking.iter().map(|s| (*s).to_owned()).collect(),
        reason: None,
        reused_from: None,
        reviewable: true,
    }
}

fn detail(fp: &str) -> HealthBlockingDetail {
    HealthBlockingDetail {
        fingerprint: fp.to_owned(),
        rule_id: "size.responsibility-growth".into(),
        paths: vec![format!("src/{fp}.rs")],
        message: format!("{fp} grew"),
    }
}

fn file(
    f: &Fixture,
    ev: &HealthEvidence,
    details: &[HealthBlockingDetail],
    remaining: usize,
) -> FiledHealthDebts {
    let input = HealthDebtInput {
        project_id: f.project,
        evidence: ev,
        details,
        remaining,
    };
    file_health_debts(&f.store, &input, &[], &[], OffsetDateTime::now_utc()).expect("file")
}

fn all_tasks(f: &Fixture) -> Vec<Task> {
    f.store.list(None).expect("list")
}

const H1: &str = "1111111111111111111111111111111111111111";
const H2: &str = "2222222222222222222222222222222222222222";

/// 同じ fingerprint は、同じ head の再実行でも別の head でも、終端でない debt がある限り 2 件目を作らない。
#[test]
fn health_inventory_does_not_file_same_debt_twice() {
    let f = fixture();
    let ev = evidence(f.repo.id, H1, HealthVerdict::Fail, &["fp-a"]);
    let first = file(&f, &ev, &[detail("fp-a")], MAX_DEBTS_PER_FIRE);
    assert_eq!(first.task_ids.len(), 1);
    let again = file(&f, &ev, &[detail("fp-a")], MAX_DEBTS_PER_FIRE);
    assert!(again.task_ids.is_empty());
    assert_eq!(again.existing_skipped, 1);
    let later = evidence(f.repo.id, H2, HealthVerdict::Fail, &["fp-a"]);
    let other_head = file(&f, &later, &[detail("fp-a")], MAX_DEBTS_PER_FIRE);
    assert!(other_head.task_ids.is_empty());
    assert_eq!(other_head.existing_skipped, 1);
    assert_eq!(all_tasks(&f).len(), 1);
    let keys = f
        .store
        .idempotency_keys_with_prefix(DEBT_KEY_PREFIX)
        .expect("keys");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].0, debt_key(&f.repo.id.to_string(), "fp-a", H1));
    assert_eq!(keys[0].1, first.task_ids[0].to_string());
}

/// 起票は draft の task 1 件だけで、計画・子 task・依存を持たず、objective はその finding の path だけを挙げる。
#[test]
fn health_inventory_files_drafts_only() {
    let f = fixture();
    let ev = evidence(f.repo.id, H1, HealthVerdict::Fail, &["fp-a", "fp-b"]);
    let filed = file(
        &f,
        &ev,
        &[detail("fp-a"), detail("fp-b")],
        MAX_DEBTS_PER_FIRE,
    );
    assert_eq!(filed.task_ids.len(), 2);
    let tasks = all_tasks(&f);
    assert_eq!(tasks.len(), 2, "1 件の finding に 1 件だけ作る");
    for task in &tasks {
        assert_eq!(task.status, Status::Draft);
        assert_eq!(task.parent_id, None);
        assert!(task.depends_on.is_empty());
        assert!(
            task.routing
                .as_ref()
                .is_none_or(|r| r.stages_hint.is_empty())
        );
        assert!(task.labels.iter().any(|l| l == DEBT_LABEL));
        assert_eq!(task.project_id, Some(f.project));
        assert_eq!(task.repos.len(), 1);
        assert_eq!(task.repos[0].repo_id, f.repo.id);
        assert!(!tasks.iter().any(|t| t.parent_id == Some(task.id)));
    }
    let a = tasks
        .iter()
        .find(|t| t.id == filed.task_ids[0])
        .expect("fp-a");
    assert!(a.objective.contains("src/fp-a.rs"));
    assert!(
        !a.objective.contains("src/fp-b.rs"),
        "ほかの debt を混ぜない"
    );
    assert!(a.title.contains("src/fp-a.rs"));
}

/// 1 回の発火の起票は 3 件まで。残りは数だけを返し、枠を超える値は 3 に切り詰める。
#[test]
fn health_inventory_caps_filing_at_three() {
    let f = fixture();
    let fps = ["fp-1", "fp-2", "fp-3", "fp-4", "fp-5"];
    let ev = evidence(f.repo.id, H1, HealthVerdict::Fail, &fps);
    let details: Vec<_> = fps.iter().map(|fp| detail(fp)).collect();
    let filed = file(&f, &ev, &details, 10);
    assert_eq!(filed.task_ids.len(), MAX_DEBTS_PER_FIRE);
    assert_eq!(filed.limit_remaining, 2);
    assert_eq!(all_tasks(&f).len(), 3);
    // 同じ発火の次の repo は残りの枠（0）を受ける。
    let other = fixture();
    let ev = evidence(other.repo.id, H1, HealthVerdict::Fail, &["fp-x", "fp-y"]);
    let none = file(&other, &ev, &[detail("fp-x"), detail("fp-y")], 0);
    assert!(none.task_ids.is_empty());
    assert_eq!(none.limit_remaining, 2);
    assert!(all_tasks(&other).is_empty());
}

/// 起票の対象は blocking の fingerprint だけ。detail にだけある fingerprint は起票せず、blocking にあって detail の
/// 無い fingerprint は起票せずに数える。
#[test]
fn health_inventory_files_only_checker_blocking() {
    let f = fixture();
    let ev = evidence(f.repo.id, H1, HealthVerdict::Fail, &["fp-a", "fp-missing"]);
    let mut no_path = detail("fp-missing");
    no_path.paths.clear();
    let details = [detail("fp-a"), detail("fp-extra"), no_path];
    let filed = file(&f, &ev, &details, MAX_DEBTS_PER_FIRE);
    assert_eq!(filed.task_ids.len(), 1);
    assert_eq!(filed.missing_detail, 1);
    let tasks = all_tasks(&f);
    assert_eq!(tasks.len(), 1);
    assert!(tasks[0].objective.contains("fp-a"));
    assert!(!tasks.iter().any(|t| t.objective.contains("fp-extra")));
}

/// verdict が Fail でなければ、blocking があっても何も起票しない。
#[test]
fn health_inventory_files_nothing_unless_fail() {
    for verdict in [
        HealthVerdict::Pass,
        HealthVerdict::PolicyOnly,
        HealthVerdict::Error,
        HealthVerdict::NotApplicable,
    ] {
        let f = fixture();
        let ev = evidence(f.repo.id, H1, verdict, &["fp-a"]);
        let filed = file(&f, &ev, &[detail("fp-a")], MAX_DEBTS_PER_FIRE);
        assert_eq!(filed, FiledHealthDebts::default(), "{verdict:?}");
        assert!(all_tasks(&f).is_empty(), "{verdict:?}");
        assert!(
            f.store
                .idempotency_keys_with_prefix(DEBT_KEY_PREFIX)
                .expect("keys")
                .is_empty()
        );
    }
}

/// debt の task が終端になった後、同じ fingerprint が別の head で blocking に出れば新しいキーで起票する。同じ head の
/// キーは既に使ったので作らない。接頭辞が同じ別の fingerprint の task は「同じ debt」に数えない。
#[test]
fn health_inventory_refiles_after_debt_task_closed() {
    let f = fixture();
    let ev = evidence(f.repo.id, H1, HealthVerdict::Fail, &["fp-a"]);
    let first = file(&f, &ev, &[detail("fp-a")], MAX_DEBTS_PER_FIRE);
    assert_eq!(first.task_ids.len(), 1);
    f.store
        .apply_transition(first.task_ids[0], Trigger::Cancel, None)
        .expect("cancel");
    let same_head = file(&f, &ev, &[detail("fp-a")], MAX_DEBTS_PER_FIRE);
    assert!(same_head.task_ids.is_empty());
    assert_eq!(same_head.existing_skipped, 1);
    let later = evidence(f.repo.id, H2, HealthVerdict::Fail, &["fp-a"]);
    let refiled = file(&f, &later, &[detail("fp-a")], MAX_DEBTS_PER_FIRE);
    assert_eq!(refiled.task_ids.len(), 1);
    assert_ne!(refiled.task_ids[0], first.task_ids[0]);
    let keys = f
        .store
        .idempotency_keys_with_prefix(DEBT_KEY_PREFIX)
        .expect("keys");
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[1].0, debt_key(&f.repo.id.to_string(), "fp-a", H2));
    // 開いている `fp-a:x` の debt は `fp-a` の debt ではない。
    let prefixed = evidence(f.repo.id, H1, HealthVerdict::Fail, &["fp-a:x"]);
    let filed = file(&f, &prefixed, &[detail("fp-a:x")], MAX_DEBTS_PER_FIRE);
    assert_eq!(filed.task_ids.len(), 1);
    f.store
        .apply_transition(refiled.task_ids[0], Trigger::Cancel, None)
        .expect("cancel");
    let third = evidence(
        f.repo.id,
        "3".repeat(40).as_str(),
        HealthVerdict::Fail,
        &["fp-a"],
    );
    let refiled_again = file(&f, &third, &[detail("fp-a")], MAX_DEBTS_PER_FIRE);
    assert_eq!(
        refiled_again.task_ids.len(),
        1,
        "fp-a:x の task は fp-a を塞がない"
    );
}
