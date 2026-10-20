use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

use task_core::{
    ArtifactRef, Budget, Check, CredentialWaitEnter, Criterion, Status, Task, TaskId, TaskKind,
    Tier, WorkUnitContext, WorkUnitKind, WorkUnitRow, WorkUnitSpec, WorkerHint, WorkspaceSpec,
};

use super::*;

const SECRET: &str = "jev_api_key";
const DUMMY: &str = "sk-dummy-do-not-leak-0001";

struct Scripted {
    outcomes: Mutex<VecDeque<VerifyOutcome>>,
    calls: AtomicUsize,
}

impl Scripted {
    fn new(outcomes: &[VerifyOutcome]) -> Arc<Self> {
        Arc::new(Self {
            outcomes: Mutex::new(outcomes.iter().copied().collect()),
            calls: AtomicUsize::new(0),
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CredentialVerifier for Scripted {
    fn verify(&self, _secret_id: &str, _value: &str) -> VerifyOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(VerifyOutcome::Transient)
    }
}

struct Fixture {
    store: Arc<SqliteStore>,
    dir: tempfile::TempDir,
    clock: Arc<Mutex<OffsetDateTime>>,
    task: TaskId,
}

fn task(status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
        }],
        inputs: Vec::<ArtifactRef>::new(),
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: std::env::temp_dir(),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 30,
            max_retries: 0,
            max_tokens: None,
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

fn blocked_unit(task_id: TaskId, key: &str) -> WorkUnitRow {
    let spec = WorkUnitSpec {
        key: key.to_string(),
        kind: WorkUnitKind::Implement,
        title: key.to_string(),
        objective: "o".into(),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    };
    WorkUnitRow::new(
        format!("wu-{key}"),
        task_id.to_string(),
        "plan".into(),
        0,
        spec,
        WorkUnitStatus::Blocked,
        "2026-10-15T00:00:00Z".into(),
    )
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("t.db");
        let store = Arc::new(SqliteStore::open(&db).unwrap());
        let t = task(Status::Blocked);
        store.insert(&t).unwrap();
        store
            .work_units_replace(t.id, vec![blocked_unit(t.id, "a")])
            .unwrap();
        let f = Self {
            store,
            dir,
            clock: Arc::new(Mutex::new(
                OffsetDateTime::parse(
                    "2026-10-15T00:00:00Z",
                    &time::format_description::well_known::Rfc3339,
                )
                .unwrap(),
            )),
            task: t.id,
        };
        f.enter("wu-a", f.task);
        f
    }

    fn raw(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.dir.path().join("t.db")).unwrap()
    }

    fn enter(&self, wu: &str, task: TaskId) {
        self.store
            .credential_wait_enter(
                &CredentialWaitEnter {
                    work_unit_id: wu,
                    task_id: task,
                    secret_id: SECRET,
                    provider: "jev",
                    purpose: "request classification",
                },
                *self.clock.lock().unwrap(),
            )
            .unwrap();
    }

    fn put_secret(&self, value: &str) {
        std::fs::write(self.dir.path().join(SECRET), format!("{value}\n")).unwrap();
    }

    fn resumer(&self, v: Arc<Scripted>) -> CredentialResumer {
        let clock = self.clock.clone();
        CredentialResumer::new(
            self.store.clone(),
            Arc::new(FileSecretSource {
                dir: self.dir.path().to_path_buf(),
            }),
            Arc::new(move || *clock.lock().unwrap()),
        )
        .with_verifier("jev", v)
    }

    fn advance(&self, secs: i64) {
        let mut c = self.clock.lock().unwrap();
        *c += time::Duration::seconds(secs);
    }

    fn wu_status(&self) -> WorkUnitStatus {
        self.store.work_units_for(self.task).unwrap()[0].status
    }

    fn wait_state(&self) -> CredentialWaitState {
        self.store
            .credential_wait_get("wu-a", SECRET)
            .unwrap()
            .unwrap()
            .state
    }

    fn resumed_events(&self) -> usize {
        self.store
            .events_for(self.task)
            .unwrap()
            .iter()
            .filter(|(_, e)| {
                matches!(e, Event::WorkUnitTransitioned { reason, .. } if reason == "credential_resumed")
            })
            .count()
    }
}

#[test]
fn valid_key_requeues_the_same_work_unit_exactly_once() {
    let f = Fixture::new();
    f.put_secret(DUMMY);
    let v = Scripted::new(&[VerifyOutcome::Valid]);
    let r = f.resumer(v.clone());
    let rep = r.process().unwrap();
    assert_eq!((rep.verified_calls, rep.resumed), (1, 1));
    assert_eq!(f.wu_status(), WorkUnitStatus::Ready);
    assert_eq!(f.wait_state(), CredentialWaitState::Resumed);
    // もう一度回しても何も起きない（待ちは閉じた）。
    let rep = r.process().unwrap();
    assert_eq!((rep.verified_calls, rep.resumed), (0, 0));
    assert_eq!(v.calls(), 1);
    assert_eq!(f.resumed_events(), 1);
}

#[test]
fn wrong_key_keeps_waiting_without_retry_then_update_resumes() {
    let f = Fixture::new();
    f.put_secret("bad-key");
    let v = Scripted::new(&[VerifyOutcome::InvalidKey, VerifyOutcome::Valid]);
    let r = f.resumer(v.clone());
    assert_eq!(r.process().unwrap().rejected, 1);
    assert_eq!(f.wait_state(), CredentialWaitState::Rejected);
    assert_eq!(f.wu_status(), WorkUnitStatus::Blocked);
    // 同じ鍵のまま何度 tick しても再検証しない（無限再試行しない）。
    for _ in 0..3 {
        f.advance(3600);
        assert_eq!(r.process().unwrap().verified_calls, 0);
    }
    assert_eq!(v.calls(), 1);
    assert_eq!(f.wu_status(), WorkUnitStatus::Blocked);
    // 更新（新しい fingerprint）で再検証し、再開する。
    f.put_secret(DUMMY);
    assert_eq!(r.process().unwrap().resumed, 1);
    assert_eq!(f.wu_status(), WorkUnitStatus::Ready);
    assert_eq!(v.calls(), 2);
}

#[test]
fn insufficient_permission_is_rejected_with_its_own_kind() {
    let f = Fixture::new();
    f.put_secret(DUMMY);
    let r = f.resumer(Scripted::new(&[VerifyOutcome::InsufficientPermission]));
    assert_eq!(r.process().unwrap().rejected, 1);
    let w = f
        .store
        .credential_wait_get("wu-a", SECRET)
        .unwrap()
        .unwrap();
    assert_eq!(w.state, CredentialWaitState::Rejected);
    assert_eq!(
        w.last_verify_kind,
        Some(CredentialVerifyKind::InsufficientPermission)
    );
    assert_eq!(f.wu_status(), WorkUnitStatus::Blocked);
}

#[test]
fn transient_failure_keeps_waiting_and_backs_off_with_the_injected_clock() {
    let f = Fixture::new();
    f.put_secret(DUMMY);
    let v = Scripted::new(&[
        VerifyOutcome::Transient,
        VerifyOutcome::Transient,
        VerifyOutcome::Valid,
    ]);
    let r = f.resumer(v.clone());
    assert_eq!(r.process().unwrap().transient, 1);
    assert_eq!(f.wait_state(), CredentialWaitState::Waiting);
    assert_eq!(f.wu_status(), WorkUnitStatus::Blocked);
    // 30 秒経つまでは呼ばない。
    f.advance(29);
    assert_eq!(r.process().unwrap().verified_calls, 0);
    f.advance(1);
    assert_eq!(r.process().unwrap().transient, 1);
    // 2 回目の後は 60 秒。
    f.advance(59);
    assert_eq!(r.process().unwrap().verified_calls, 0);
    f.advance(1);
    assert_eq!(r.process().unwrap().resumed, 1);
    assert_eq!(v.calls(), 3);
    // 一時障害では受信箱を「再登録待ち」へ変えない（rejected にしていない）。
}

#[test]
fn backoff_is_capped() {
    assert_eq!(backoff_delay(1), Duration::from_secs(30));
    assert_eq!(backoff_delay(2), Duration::from_secs(60));
    assert_eq!(backoff_delay(30), BACKOFF_CAP);
    assert_eq!(backoff_delay(u32::MAX), BACKOFF_CAP);
}

#[test]
fn duplicate_events_and_two_resumers_do_not_resume_twice() {
    let f = Fixture::new();
    f.put_secret(DUMMY);
    let fp = secret_fingerprint(DUMMY);
    // 同じ secret_stored が複数回届いても記録は 1 件。
    for _ in 0..3 {
        f.store
            .credential_record_secret_saved(SECRET, &fp, *f.clock.lock().unwrap())
            .unwrap();
    }
    let a = f.resumer(Scripted::new(&[VerifyOutcome::Valid, VerifyOutcome::Valid]));
    let b = f.resumer(Scripted::new(&[VerifyOutcome::Valid, VerifyOutcome::Valid]));
    let total = a.process().unwrap().resumed + b.process().unwrap().resumed;
    assert_eq!(total, 1);
    assert_eq!(f.resumed_events(), 1);
}

#[test]
fn cancelled_task_is_not_resumed() {
    let f = Fixture::new();
    f.put_secret(DUMMY);
    f.raw()
        .execute(
            "UPDATE tasks SET status = 'cancelled' WHERE id = ?1",
            [f.task.to_string()],
        )
        .unwrap();
    let r = f.resumer(Scripted::new(&[VerifyOutcome::Valid]));
    let rep = r.process().unwrap();
    assert_eq!((rep.resumed, rep.cancelled), (0, 1));
    assert_eq!(f.wu_status(), WorkUnitStatus::Blocked);
    assert_eq!(f.wait_state(), CredentialWaitState::Cancelled);
}

#[test]
fn reconcile_picks_up_a_missed_secret_stored_event() {
    let f = Fixture::new();
    // PUT は成功したが event を取り逃した: 記録は無く、ファイルだけがある。
    assert!(
        f.store
            .credential_wait_get("wu-a", SECRET)
            .unwrap()
            .unwrap()
            .seen_fingerprint
            .is_none()
    );
    f.put_secret(DUMMY);
    let r = f.resumer(Scripted::new(&[VerifyOutcome::Valid]));
    assert_eq!(r.process().unwrap().resumed, 1);
    assert_eq!(f.wu_status(), WorkUnitStatus::Ready);
}

#[test]
fn restart_restores_the_wait_from_the_db() {
    let f = Fixture::new();
    f.put_secret(DUMMY);
    // 検証前に daemon が落ちる（resumer を作り直す）。待ちは DB にあり、新しい resumer がそのまま続ける。
    drop(f.resumer(Scripted::new(&[])));
    let r = f.resumer(Scripted::new(&[VerifyOutcome::Valid]));
    assert!(r.has_open_waits());
    assert_eq!(r.process().unwrap().resumed, 1);
    assert!(!r.has_open_waits());
}

#[test]
fn no_secret_value_reaches_events_or_the_db() {
    let f = Fixture::new();
    f.put_secret(DUMMY);
    let r = f.resumer(Scripted::new(&[VerifyOutcome::Valid]));
    r.process().unwrap();
    let dump = {
        let conn = f.raw();
        let mut out = String::new();
        for table in [
            "events",
            "credential_waits",
            "credential_wait_events",
            "work_units",
        ] {
            let mut stmt = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
            let n = stmt.column_count();
            let mut rows = stmt.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for i in 0..n {
                    if let Ok(v) = row.get::<_, rusqlite::types::Value>(i) {
                        out.push_str(&format!("{v:?} "));
                    }
                }
            }
        }
        out
    };
    assert!(!dump.contains(DUMMY));
    assert!(dump.contains(&secret_fingerprint(DUMMY)));
}

#[test]
fn http_status_classification_never_asks_for_reregistration_on_429_or_budget() {
    assert_eq!(classify_http_status(200), VerifyOutcome::Valid);
    assert_eq!(classify_http_status(401), VerifyOutcome::InvalidKey);
    assert_eq!(
        classify_http_status(403),
        VerifyOutcome::InsufficientPermission
    );
    for s in [402, 429, 500, 503] {
        assert_eq!(classify_http_status(s), VerifyOutcome::Transient, "{s}");
    }
    // 400 は要求の schema 不備: 再試行しても直らない。402（残高不足）とは区別する。
    assert_eq!(classify_http_status(400), VerifyOutcome::RequestRejected);
}
