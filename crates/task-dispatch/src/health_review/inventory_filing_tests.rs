//! ADR 2026-11-08-health-enforcement-loop D6.5〜D6.7 の通しの試験（接頭辞 `health_inventory_`）: runner の評価と
//! 起票（`task_ops::health_debt::file_health_debts`）の接続。検査器は `gate_tests` と同じ stub（一時 git repo の
//! base に commit した `repo_health.py`）で、外部ネットワークに出ず、LLM を呼ばない。
//!
//! 既存の試験では検出できない理由: runner の試験（`inventory_tests`）は起票の欄が 0 の行だけを見て、起票の試験
//! （`task_ops::health_debt::tests`）は手で組んだ `HealthEvidence` を渡すだけなので、同じ検査器の実行の証跡と説明が
//! 起票に渡ること・行に起票の結果が 1 回で入ること・発火をまたぐ重複の防止・repo をまたぐ枠を通しては見ない。

use std::path::Path;

use serde_json::Value;
use task_core::{Project, ProjectStatus, RepoRun, Status, Task, TaskId};
use task_ops::health_debt::{DEBT_KEY_PREFIX, DEBT_LABEL};

use super::*;
use crate::health_review::gate::tests::{
    SRC, TIMEOUT, base_policy, checker_files, commit, finding, init_repo, report, rule, sh_git,
};

fn at(minute: i64) -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000) + time::Duration::minutes(minute)
}

/// file の DB（試験の外から行を数える）と案件 1 件。
struct Fixture {
    dir: crate::test_support::WritableTempDir,
    store: Arc<SqliteStore>,
    project: ProjectId,
    job: CronJobId,
}

fn fixture() -> Fixture {
    let dir = crate::test_support::WritableTempDir::new();
    let store = SqliteStore::open(&dir.path().join("celeris.db")).expect("store");
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
        created_at: at(0),
        updated_at: at(0),
    };
    store.project_create(&project).expect("project");
    Fixture {
        dir,
        store: Arc::new(store),
        project: project.id,
        job: CronJobId(Ulid::new()),
    }
}

impl Fixture {
    /// base に stub の検査器（exit 1・`report`）を commit した repo を案件に足す。
    fn failing_repo(&self, name: &str, findings: Vec<Value>) -> (tempfile::TempDir, ProjectRepo) {
        let text = report(
            "fail",
            1,
            findings,
            base_policy(false),
            vec![rule("size.x", "fail", "")],
        );
        let stub = serde_json::json!({"exit": 1, "report": text}).to_string();
        let dir = init_repo();
        let files = checker_files(&stub, true);
        let refs: Vec<(&str, &str)> = files.iter().map(|(p, b)| (*p, b.as_str())).collect();
        commit(dir.path(), &refs, "checker");
        let repo = ProjectRepo {
            auto_integrate: false,
            id: RepoId::new(),
            project_id: self.project,
            name: name.into(),
            kind: RepoKind::Git,
            location: WorkspaceSpec::local(dir.path().to_string_lossy().as_ref()),
            default_branch: None,
            sync: None,
            run: RepoRun::Auto,
            is_primary: false,
            created_at: at(0),
        };
        self.store.repo_create(&repo).expect("repo");
        (dir, repo)
    }

    fn fire(&self, minute: i64) -> InventoryFire {
        InventoryFire {
            job_id: self.job,
            project_id: self.project,
            scheduled_for: at(minute),
            now: at(minute),
            bundled: None,
            timeout: TIMEOUT,
            roles: Vec::new(),
            genres: Vec::new(),
        }
    }

    fn debts(&self) -> Vec<Task> {
        self.store
            .list(None)
            .expect("tasks")
            .into_iter()
            .filter(|t| t.labels.iter().any(|l| l == DEBT_LABEL))
            .collect()
    }

    fn count(&self, table: &str) -> i64 {
        rusqlite::Connection::open(self.dir.path().join("celeris.db"))
            .expect("db")
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .expect("count")
    }

    /// 発火の行（`scheduled_for` が `minute` のもの。追記の順）を表から直接読む。
    fn rows(&self, minute: i64) -> Vec<Row> {
        let conn = rusqlite::Connection::open(self.dir.path().join("celeris.db")).expect("db");
        let mut stmt = conn
            .prepare(
                "SELECT scheduled_for, repo_id, evidence_json, task_ids_json, existing_skipped, \
                 limit_remaining FROM health_inventory_runs WHERE job_id = ?1 ORDER BY rowid",
            )
            .expect("prepare");
        let raw = stmt
            .query_map([self.job.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, u64>(4)?,
                    r.get::<_, u64>(5)?,
                ))
            })
            .expect("query");
        let rfc3339 = &time::format_description::well_known::Rfc3339;
        raw.map(|row| row.expect("row"))
            .filter(|row| OffsetDateTime::parse(&row.0, rfc3339).expect("time") == at(minute))
            .map(|(_, repo, evidence, tasks, existing, remaining)| Row {
                repo_id: repo.parse().expect("repo id"),
                evidence: evidence.map(|e| serde_json::from_str(&e).expect("evidence")),
                task_ids: serde_json::from_str(&tasks).expect("task ids"),
                existing_skipped: existing,
                limit_remaining: remaining,
            })
            .collect()
    }
}

/// 表 `health_inventory_runs` の 1 行（試験で見る列）。
#[derive(Debug)]
struct Row {
    repo_id: RepoId,
    evidence: Option<HealthEvidence>,
    task_ids: Vec<TaskId>,
    existing_skipped: u64,
    limit_remaining: u64,
}

/// `size.x` の error・new の finding（`n` で fingerprint を変える）。
fn blocking(n: usize) -> Value {
    finding("size.x", n, "error", "new", false)
}

fn fp(n: usize) -> String {
    format!("size.x:{n:016x}")
}

/// 作業ツリーを変えて tree を進める（同じ tree は `unchanged` で省略されるので）。
fn advance(dir: &Path, body: &str) -> String {
    commit(dir, &[(SRC, body)], "advance")
}

/// ref・HEAD・作業ツリー・worktree・stash の写し。
fn git_state(dir: &Path) -> Vec<String> {
    [
        &["for-each-ref", "--format=%(refname) %(objectname)"][..],
        &["rev-parse", "HEAD"],
        &["symbolic-ref", "HEAD"],
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignored",
        ],
        &["worktree", "list", "--porcelain"],
        &["stash", "list"],
    ]
    .iter()
    .map(|args| sh_git(dir, args))
    .collect()
}

/// 不変条件（D6.6）: debt が残ったまま head が進んでも、同じ repo・fingerprint の 2 件目を作らない。2 回目の発火の
/// 行は評価済み（head が進んだ）で、起票 0・既存で省いた 1 を持つ。
#[test]
fn health_inventory_does_not_file_same_debt_twice_across_fires() {
    let f = fixture();
    let (dir, repo) = f.failing_repo("code", vec![blocking(1)]);
    f.fire(1).run(&f.store, std::slice::from_ref(&repo));
    let first = f.debts();
    assert_eq!(first.len(), 1, "{first:?}");

    for (minute, body) in [(2, "fn b() {}\n"), (3, "fn c() {}\n")] {
        let head = advance(dir.path(), body);
        f.fire(minute).run(&f.store, std::slice::from_ref(&repo));
        let rows = f.rows(minute);
        assert_eq!(rows.len(), 1, "{rows:?}");
        let ev = rows[0].evidence.as_ref().expect("evaluated");
        assert_eq!(
            (ev.head_sha.as_str(), ev.blocking.clone()),
            (head.as_str(), vec![fp(1)])
        );
        assert_eq!(
            (rows[0].task_ids.len(), rows[0].existing_skipped),
            (0, 1),
            "{rows:?}"
        );
    }
    let debts = f.debts();
    assert_eq!(
        debts.iter().map(|t| t.id).collect::<Vec<_>>(),
        [first[0].id]
    );
    let keys = f
        .store
        .idempotency_keys_with_prefix(DEBT_KEY_PREFIX)
        .expect("keys");
    assert_eq!(keys.len(), 1, "{keys:?}");
}

/// 不変条件（D6.7）: 起票は draft の task だけ。親・依存・子 task・計画・WorkUnit・run・chat を作らず、repo の ref・
/// HEAD・作業ツリーを変えない。
#[test]
fn health_inventory_files_drafts_only_without_rewrite() {
    let f = fixture();
    let (dir, repo) = f.failing_repo("code", vec![blocking(1), blocking(2)]);
    let before = git_state(dir.path());

    f.fire(1).run(&f.store, std::slice::from_ref(&repo));

    let debts = f.debts();
    assert_eq!(debts.len(), 2, "{debts:?}");
    for task in &debts {
        assert_eq!(task.status, Status::Draft, "{task:?}");
        assert_eq!(task.parent_id, None);
        assert!(task.depends_on.is_empty());
        assert!(f.store.children(task.id).expect("children").is_empty());
        assert!(task.objective.contains(SRC), "{}", task.objective);
    }
    assert_eq!(f.store.list(None).expect("tasks").len(), 2);
    for table in [
        "execution_plans",
        "work_units",
        "runs",
        "chat_runs",
        "chat_messages",
    ] {
        assert_eq!(f.count(table), 0, "{table}");
    }
    assert_eq!(git_state(dir.path()), before);
}

/// 不変条件（D6.5・D6.6）: 発火の数は表の行の集計（`health_inventory_fire_counts`）だけから求まり、起票した task・
/// 既存で省いた数・枠で残した数と一致する。起票の枠 3 件は repo をまたいで数える。
#[test]
fn health_inventory_records_counts() {
    let f = fixture();
    let (dir_a, a) = f.failing_repo("a", vec![blocking(1), blocking(2)]);
    let (dir_b, b) = f.failing_repo("b", vec![blocking(3), blocking(4)]);
    let repos = [a.clone(), b.clone()];

    f.fire(1).run(&f.store, &repos);

    let counts = f
        .store
        .health_inventory_fire_counts(f.job, at(1))
        .expect("counts");
    let expected = task_core::health_inventory::HealthInventoryFireCounts {
        repos: 2,
        evaluated: 2,
        tasks_created: 3,
        limit_remaining: 1,
        ..Default::default()
    };
    assert_eq!(counts, expected);
    let filed: Vec<TaskId> = f.rows(1).iter().flat_map(|r| r.task_ids.clone()).collect();
    let mut stored: Vec<TaskId> = f.debts().iter().map(|t| t.id).collect();
    let mut sorted = filed.clone();
    sorted.sort();
    stored.sort();
    assert_eq!(sorted, stored);
    let by_repo: Vec<(RepoId, usize, u64)> = f
        .rows(1)
        .iter()
        .map(|r| (r.repo_id, r.task_ids.len(), r.limit_remaining))
        .collect();
    assert_eq!(by_repo, [(a.id, 2, 0), (b.id, 1, 1)]);

    advance(dir_a.path(), "fn a2() {}\n");
    advance(dir_b.path(), "fn b2() {}\n");
    f.fire(2).run(&f.store, &repos);

    let counts = f
        .store
        .health_inventory_fire_counts(f.job, at(2))
        .expect("counts");
    let expected = task_core::health_inventory::HealthInventoryFireCounts {
        repos: 2,
        evaluated: 2,
        tasks_created: 1,
        existing_skipped: 3,
        ..Default::default()
    };
    assert_eq!(counts, expected);
    assert_eq!(f.debts().len(), 4);

    f.fire(3).run(&f.store, &repos);
    let counts = f
        .store
        .health_inventory_fire_counts(f.job, at(3))
        .expect("counts");
    assert_eq!(
        (counts.repos, counts.unchanged, counts.tasks_created),
        (2, 2, 0)
    );
}

/// 不変条件（D6.6・D6.7）: report で severity error・delta new でも、検査器の例外（`exception`）で `blocking` に
/// 入らない finding は起票しない。同じ report の blocking の finding だけが起票される。
#[test]
fn health_inventory_exception_finding_not_filed() {
    let f = fixture();
    let excepted = finding("size.x", 2, "error", "new", true);
    let (_dir, repo) = f.failing_repo("code", vec![blocking(1), excepted]);

    f.fire(1).run(&f.store, std::slice::from_ref(&repo));

    let rows = f.rows(1);
    let ev = rows[0].evidence.as_ref().expect("evaluated");
    assert_eq!(ev.blocking, [fp(1)], "{ev:?}");
    let debts = f.debts();
    assert_eq!(debts.len(), 1, "{debts:?}");
    assert_eq!(rows[0].task_ids, [debts[0].id]);
    assert!(
        debts[0].objective.contains(&fp(1)),
        "{}",
        debts[0].objective
    );
    assert!(!debts.iter().any(|t| t.objective.contains(&fp(2))));
    assert_eq!((rows[0].existing_skipped, rows[0].limit_remaining), (0, 0));
}
