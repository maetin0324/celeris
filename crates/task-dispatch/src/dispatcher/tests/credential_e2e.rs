//! ADR 2026-10-15-mlsys-min-decision-assisted D4 の通し試験（結合）: 未登録 → 待機 → 受信箱 item 1 件（複数 task 集約）→
//! secret 保存 → mock 検証 → 同じ work unit が一度だけ再開。分岐は誤キー・429・再起動・取り逃し reconcile・重複・cancel。
//! 一時 DB・mock 検証（`Scripted`）・注入時計。外部ネットワークと LLM は使わない。時間は注入時計で進める。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use task_core::{CredentialVerifyKind, CredentialWaitState};

use super::credential_gate::{SECRET, jev_pick, unit};
use super::*;
use crate::credential_resume::{
    CredentialResumer, CredentialVerifier, FileSecretSource, VerifyOutcome, secret_fingerprint,
};
use crate::dispatcher::{CredentialGate, CredentialGates};

const DUMMY: &str = "sk-dummy-e2e-never-leak-7731";
const WRONG: &str = "sk-wrong-e2e-never-leak-0042";

struct Scripted {
    outcomes: Mutex<VecDeque<VerifyOutcome>>,
    calls: AtomicUsize,
    /// 検証 adapter が受け取った値（mock の内側にだけ残る。外へは出ない）。
    seen: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(outcomes: &[VerifyOutcome]) -> Arc<Self> {
        Arc::new(Self {
            outcomes: Mutex::new(outcomes.iter().copied().collect()),
            calls: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
        })
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CredentialVerifier for Scripted {
    fn verify(&self, _secret_id: &str, value: &str) -> VerifyOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(value.to_string());
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(VerifyOutcome::Transient)
    }
}

struct E2e {
    dir: tempfile::TempDir,
    secrets: PathBuf,
    sqlite: Arc<SqliteStore>,
    clock: Arc<Mutex<OffsetDateTime>>,
}

impl E2e {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let secrets = dir.path().join("secrets");
        std::fs::create_dir_all(&secrets).unwrap();
        let sqlite = Arc::new(SqliteStore::open(&dir.path().join("db.sqlite")).unwrap());
        let start = OffsetDateTime::parse(
            "2026-10-15T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        Self {
            dir,
            secrets,
            sqlite,
            clock: Arc::new(Mutex::new(start)),
        }
    }

    fn store(&self) -> Arc<dyn TaskStore> {
        self.sqlite.clone()
    }

    /// daemon の（再）起動: 新しい Dispatcher（gate 配線）と新しい resumer を作る。
    fn dispatcher(&self) -> Dispatcher {
        let mut d = pool_dispatcher(
            self.store(),
            done_adapter(),
            None,
            PathBuf::from("/accounts/claude"),
            1,
            1,
        );
        d.set_credential_gates(CredentialGates {
            secrets_dir: Some(self.secrets.clone()),
            by_source: HashMap::from([(
                "jev".into(),
                CredentialGate {
                    secret_id: SECRET.into(),
                    provider: "jev".into(),
                    purpose: "Jev で依頼を分類する".into(),
                },
            )]),
        });
        d
    }

    fn resumer(&self, v: &Arc<Scripted>) -> CredentialResumer {
        let clock = self.clock.clone();
        CredentialResumer::new(
            self.sqlite.clone(),
            Arc::new(FileSecretSource {
                dir: self.secrets.clone(),
            }),
            Arc::new(move || *clock.lock().unwrap()),
        )
        .with_verifier("jev", v.clone())
    }

    fn advance(&self, secs: i64) {
        *self.clock.lock().unwrap() += time::Duration::seconds(secs);
    }

    /// 計画つき task（unit `a`: 待つ・`b`: 独立した兄弟）。
    fn task(&self) -> (Task, WorkUnitRow, WorkUnitRow) {
        let task = new_task(self.dir.path(), Check::Reviewer, 1);
        self.sqlite.insert(&task).unwrap();
        let a = unit(&task, "a", 0);
        let b = unit(&task, "b", 1);
        self.sqlite
            .work_units_apply(task.id, vec![a.clone(), b.clone()], Vec::new(), Vec::new())
            .unwrap();
        (task, a, b)
    }

    fn unit(&self, task: &Task, key: &str) -> (WorkUnitStatus, Option<WorkUnitBlockedReason>) {
        let u = self
            .sqlite
            .work_units_for(task.id)
            .unwrap()
            .into_iter()
            .find(|u| u.key == key)
            .unwrap();
        (u.status, u.blocked_reason)
    }

    /// PUT /api/v1/secrets/{id} の効果: 値を secrets_dir に置き、`secret_stored`（id と fingerprint のみ）を記録する。
    fn put_secret(&self, value: &str) {
        std::fs::write(self.secrets.join(SECRET), format!("{value}\n")).unwrap();
        self.sqlite
            .credential_record_secret_saved(
                SECRET,
                &secret_fingerprint(value),
                *self.clock.lock().unwrap(),
            )
            .unwrap();
    }

    /// event を取り逃した PUT（ファイルだけある）。
    fn put_secret_without_event(&self, value: &str) {
        std::fs::write(self.secrets.join(SECRET), format!("{value}\n")).unwrap();
    }

    fn inbox(&self) -> Vec<task_ops::human_inbox::InboxItem> {
        let ctx = task_ops::view::ViewContext {
            workspace_roots: task_core::local_machine::WorkspaceRoots::new(
                self.dir.path().to_path_buf(),
                None,
            ),
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            max_requeues: 5,
            clusters: Default::default(),
        };
        task_ops::human_inbox::human_inbox(
            self.sqlite.as_ref(),
            None,
            &ctx,
            *self.clock.lock().unwrap(),
            &|_, _| Vec::new(),
        )
        .unwrap()
        .items
        .into_iter()
        .filter(|i| i.kind == task_ops::human_inbox::InboxKind::CredentialWait)
        .collect()
    }

    fn resumed_events(&self, task: &Task) -> usize {
        self.sqlite
            .events_for(task.id)
            .unwrap()
            .iter()
            .filter(|(_, e)| {
                matches!(e, Event::WorkUnitTransitioned { reason, .. } if reason == "credential_resumed")
            })
            .count()
    }

    fn wait(&self, task: &Task, wu: &WorkUnitRow) -> task_core::CredentialWait {
        let _ = task;
        self.sqlite
            .credential_wait_get(&wu.id, SECRET)
            .unwrap()
            .unwrap()
    }

    /// 待機中に worker run を起こしていない。
    fn runs(&self, task: &Task) -> usize {
        self.sqlite.runs_for_task(task.id).unwrap().len()
    }
}

fn hold(d: &mut Dispatcher, t: &Task, wu: &WorkUnitRow) {
    assert!(d.credential_gate_hold(t, Some(wu), &jev_pick()).unwrap());
}

#[test]
fn unregistered_to_wait_one_item_for_many_tasks_to_a_single_resume() {
    let e = E2e::new();
    let mut d = e.dispatcher();
    let (t1, a1, _b1) = e.task();
    let (t2, a2, _b2) = e.task();

    // 未登録 → 該当 unit だけ待機。task の Status・兄弟は止めない。
    hold(&mut d, &t1, &a1);
    hold(&mut d, &t2, &a2);
    hold(&mut d, &t1, &a1); // 冪等
    let waiting = (
        WorkUnitStatus::Blocked,
        Some(WorkUnitBlockedReason::WaitingForAuth),
    );
    assert_eq!(e.unit(&t1, "a"), waiting);
    assert_eq!(e.unit(&t2, "a"), waiting);
    assert_eq!(e.unit(&t1, "b"), (WorkUnitStatus::Ready, None));
    assert_eq!(e.sqlite.get(t1.id).unwrap().unwrap().status, Status::Ready);
    assert_eq!(e.runs(&t1) + e.runs(&t2), 0);

    // 受信箱 item は 1 件（2 task を束ねる）。通知（triage）の入力も 1 件で、task が増えても新しい通知にならない。
    let items = e.inbox();
    assert_eq!(items.len(), 1, "{items:?}");
    let item = items[0].clone();
    assert_eq!(item.blocking.tasks.len(), 2);
    let detail = item.detail.clone().unwrap();
    for needle in ["Jev で依頼を分類する", "/accounts", "自動で続行"] {
        assert!(detail.contains(needle), "{needle}: {detail}");
    }
    assert!(item.links.iter().any(|l| l.href == "/accounts"));
    let (t3, a3, _) = e.task();
    hold(&mut d, &t3, &a3);
    let again = e.inbox();
    assert_eq!(again.len(), 1);
    assert_eq!(
        (&again[0].id, &again[0].created_at),
        (&item.id, &item.created_at)
    );
    assert_eq!(again[0].blocking.tasks.len(), 3);

    // 登録 → mock 検証成功（秘密 1 つ・fingerprint 1 つにつき 1 回）→ 3 つの unit が一度だけ再開。
    e.put_secret(DUMMY);
    let v = Scripted::new(&[VerifyOutcome::Valid]);
    let r = e.resumer(&v);
    let rep = r.process().unwrap();
    assert_eq!((rep.verified_calls, rep.resumed), (1, 3));
    for t in [&t1, &t2, &t3] {
        assert_eq!(e.unit(t, "a"), (WorkUnitStatus::Ready, None));
        assert_eq!(e.resumed_events(t), 1);
        assert_eq!(
            e.runs(t),
            0,
            "再開は再 queue だけで run は dispatch まで起きない"
        );
    }
    assert_eq!(e.unit(&t1, "b"), (WorkUnitStatus::Ready, None));
    assert!(e.inbox().is_empty());
    // 何度回しても再開は増えない。
    for _ in 0..3 {
        let rep = r.process().unwrap();
        assert_eq!((rep.verified_calls, rep.resumed), (0, 0));
    }
    assert_eq!(v.calls(), 1);
    assert_eq!(e.resumed_events(&t1), 1);
    // 検証 adapter は値を受け取るが、worker にも他にも値は渡らない。
    assert_eq!(*v.seen.lock().unwrap(), vec![DUMMY.to_string()]);
}

#[test]
fn wrong_key_updates_the_same_item_keeps_waiting_and_an_update_resumes() {
    let e = E2e::new();
    let mut d = e.dispatcher();
    let (t, a, _b) = e.task();
    hold(&mut d, &t, &a);
    let before = e.inbox().remove(0);

    e.put_secret(WRONG);
    let v = Scripted::new(&[VerifyOutcome::InvalidKey, VerifyOutcome::Valid]);
    let r = e.resumer(&v);
    assert_eq!(r.process().unwrap().rejected, 1);
    assert_eq!(e.wait(&t, &a).state, CredentialWaitState::Rejected);
    assert_eq!(
        e.wait(&t, &a).last_verify_kind,
        Some(CredentialVerifyKind::InvalidKey)
    );
    assert_eq!(e.unit(&t, "a").0, WorkUnitStatus::Blocked);
    assert_eq!(e.resumed_events(&t), 0);

    // 通知は同じ item の更新（id・created_at 不変 → triage は新規にしない）。文面は「更新してください」。
    let after = e.inbox();
    assert_eq!(after.len(), 1);
    assert_eq!(
        (&after[0].id, &after[0].created_at),
        (&before.id, &before.created_at)
    );
    assert_ne!(after[0].title, before.title);
    assert!(
        after[0]
            .detail
            .clone()
            .unwrap()
            .contains("更新してください")
    );

    // 同じ誤キーのまま何度 tick しても再検証しない（無限再試行しない）。
    for _ in 0..3 {
        e.advance(3600);
        assert_eq!(r.process().unwrap().verified_calls, 0);
    }
    assert_eq!(v.calls(), 1);

    // 更新で再開。
    e.put_secret(DUMMY);
    assert_eq!(r.process().unwrap().resumed, 1);
    assert_eq!(e.unit(&t, "a"), (WorkUnitStatus::Ready, None));
    assert_eq!(e.resumed_events(&t), 1);
    assert!(e.inbox().is_empty());
    assert_eq!(v.calls(), 2);
}

#[test]
fn rate_limit_keeps_waiting_without_asking_for_reregistration() {
    let e = E2e::new();
    let mut d = e.dispatcher();
    let (t, a, _b) = e.task();
    hold(&mut d, &t, &a);
    let before = e.inbox().remove(0);
    e.put_secret(DUMMY);
    let v = Scripted::new(&[VerifyOutcome::Transient, VerifyOutcome::Valid]);
    let r = e.resumer(&v);
    assert_eq!(r.process().unwrap().transient, 1);
    assert_eq!(e.wait(&t, &a).state, CredentialWaitState::Waiting);
    assert_eq!(e.unit(&t, "a").0, WorkUnitStatus::Blocked);
    // 再登録要求にしない: item の文面は「登録待ち」のまま。
    let mid = e.inbox().remove(0);
    assert_eq!(mid.title, before.title);
    assert!(!mid.detail.clone().unwrap().contains("更新してください"));
    // backoff の間は呼ばない。時計を進めると再検証して再開する。
    e.advance(29);
    assert_eq!(r.process().unwrap().verified_calls, 0);
    e.advance(1);
    assert_eq!(r.process().unwrap().resumed, 1);
    assert_eq!(e.resumed_events(&t), 1);
}

#[test]
fn daemon_restart_restores_the_wait_and_resumes_once() {
    let e = E2e::new();
    let (t, a, _b) = e.task();
    let (t2, a2, _b2) = e.task();
    {
        let mut d = e.dispatcher();
        hold(&mut d, &t, &a);
        // 入口の途中で落ちた task: wait だけ書かれ unit は ready のまま。
        e.sqlite
            .credential_wait_enter(
                &task_core::CredentialWaitEnter {
                    work_unit_id: &a2.id,
                    task_id: t2.id,
                    secret_id: SECRET,
                    provider: "jev",
                    purpose: "Jev で依頼を分類する",
                },
                *e.clock.lock().unwrap(),
            )
            .unwrap();
    }
    // 登録だけ済んで検証前に落ちた。再起動: 新しい Dispatcher と resumer。
    e.put_secret(DUMMY);
    let mut restarted = e.dispatcher();
    let restored = restarted.restore_credential_waits().unwrap();
    assert_eq!(restored.len(), 2);
    assert_eq!(e.unit(&t2, "a").0, WorkUnitStatus::Blocked);
    assert_eq!(e.inbox().len(), 1);

    let v = Scripted::new(&[VerifyOutcome::Valid]);
    let r = e.resumer(&v);
    assert!(r.has_open_waits());
    assert_eq!(r.process().unwrap().resumed, 2);
    assert_eq!(e.unit(&t, "a").0, WorkUnitStatus::Ready);
    assert_eq!(e.unit(&t2, "a").0, WorkUnitStatus::Ready);
    // もう一度再起動しても二重に再開しない。
    let r2 = e.resumer(&Scripted::new(&[VerifyOutcome::Valid]));
    assert_eq!(r2.process().unwrap().resumed, 0);
    assert_eq!((e.resumed_events(&t), e.resumed_events(&t2)), (1, 1));
}

#[test]
fn a_missed_secret_stored_event_is_reconciled_by_the_tick() {
    let e = E2e::new();
    let mut d = e.dispatcher();
    let (t, a, _b) = e.task();
    hold(&mut d, &t, &a);
    // PUT は成功したが event を取り逃した。記録は無く、ファイルだけがある。
    e.put_secret_without_event(DUMMY);
    assert!(e.wait(&t, &a).seen_fingerprint.is_none());
    let v = Scripted::new(&[VerifyOutcome::Valid]);
    let r = e.resumer(&v);
    assert_eq!(r.process().unwrap().resumed, 1);
    assert_eq!(e.unit(&t, "a"), (WorkUnitStatus::Ready, None));
    assert_eq!(e.resumed_events(&t), 1);
    // 遅れて event が届いても（重複）何も起きない。
    e.sqlite
        .credential_record_secret_saved(
            SECRET,
            &secret_fingerprint(DUMMY),
            *e.clock.lock().unwrap(),
        )
        .unwrap();
    assert_eq!(r.process().unwrap().resumed, 0);
    assert_eq!(e.resumed_events(&t), 1);
}

#[test]
fn duplicate_events_and_concurrent_resumers_resume_exactly_once() {
    let e = E2e::new();
    let mut d = e.dispatcher();
    let (t, a, _b) = e.task();
    hold(&mut d, &t, &a);
    e.put_secret(DUMMY);
    for _ in 0..4 {
        e.put_secret(DUMMY);
    }
    let v = Scripted::new(&[
        VerifyOutcome::Valid,
        VerifyOutcome::Valid,
        VerifyOutcome::Valid,
    ]);
    let rs: Vec<_> = (0..3).map(|_| e.resumer(&v)).collect();
    let total: usize = std::thread::scope(|s| {
        let hs: Vec<_> = rs
            .iter()
            .map(|r| s.spawn(|| r.process().unwrap().resumed))
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).sum()
    });
    assert_eq!(total, 1);
    assert_eq!(e.resumed_events(&t), 1);
    assert_eq!(e.unit(&t, "a"), (WorkUnitStatus::Ready, None));
}

#[test]
fn a_cancelled_task_is_not_resumed_when_the_key_arrives_later() {
    let e = E2e::new();
    let mut d = e.dispatcher();
    let (t, a, _b) = e.task();
    let (live, alive_a, _) = e.task();
    hold(&mut d, &t, &a);
    hold(&mut d, &live, &alive_a);
    e.sqlite
        .apply_transition(t.id, task_core::Trigger::Cancel, None)
        .unwrap();
    e.put_secret(DUMMY);
    let v = Scripted::new(&[VerifyOutcome::Valid]);
    let rep = e.resumer(&v).process().unwrap();
    assert_eq!((rep.resumed, rep.cancelled), (1, 1));
    assert_eq!(e.unit(&t, "a").0, WorkUnitStatus::Blocked);
    assert_eq!(e.resumed_events(&t), 0);
    assert_eq!(e.wait(&t, &a).state, CredentialWaitState::Cancelled);
    // 同じ鍵を待つ生きた task は止めずに再開する。
    assert_eq!(e.unit(&live, "a").0, WorkUnitStatus::Ready);
    assert_eq!(e.resumed_events(&live), 1);
}

/// 秘密値は wait 行・受信箱・triage・event・work unit・SQLite ファイル（WAL 含む）・report・error のどこにも出ない。
#[test]
fn secret_values_never_appear_in_db_inbox_events_reports_or_errors() {
    let e = E2e::new();
    let mut d = e.dispatcher();
    let (t, a, _b) = e.task();
    hold(&mut d, &t, &a);
    let v = Scripted::new(&[
        VerifyOutcome::InvalidKey,
        VerifyOutcome::InsufficientPermission,
        VerifyOutcome::Transient,
        VerifyOutcome::Valid,
    ]);
    let r = e.resumer(&v);
    let mut surface = String::new();
    for value in [
        WRONG,
        "sk-noperm-e2e-never-leak-5",
        "sk-transient-e2e-never-leak-6",
        DUMMY,
    ] {
        e.put_secret(value);
        surface.push_str(&format!("{:?}", r.process().unwrap()));
        e.advance(3600);
        surface.push_str(&format!(
            "{:?}{:?}",
            e.inbox(),
            e.sqlite.credential_waits_list_waiting().unwrap()
        ));
        surface.push_str(&format!("{:?}", e.sqlite.events_for(t.id).unwrap()));
    }
    assert_eq!(e.unit(&t, "a").0, WorkUnitStatus::Ready);
    surface.push_str(&format!("{:?}", e.sqlite.work_units_for(t.id).unwrap()));
    let secrets = [
        DUMMY,
        WRONG,
        "sk-noperm-e2e-never-leak-5",
        "sk-transient-e2e-never-leak-6",
    ];
    for s in secrets {
        assert!(!surface.contains(s), "{s} leaked into a visible surface");
    }
    // DB ファイル全体（-wal / -shm を含む）に値の文字列が無い。fingerprint は残ってよい。
    for entry in std::fs::read_dir(e.dir.path()).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            let bytes = std::fs::read(&p).unwrap();
            for s in secrets {
                assert!(
                    !bytes.windows(s.len()).any(|w| w == s.as_bytes()),
                    "{s} found in {}",
                    p.display()
                );
            }
        }
    }
}

/// 待機は CoS triage（LLM run）を経ない: 本物の `Dispatcher::tick` を回し、待機 item の作成〜通知 enqueue の後でも
/// runs 行・CoS triage の chat run・worker の spawn が 0 本で、escalation outbox が 1 件（task が増えても 1 件）。
#[tokio::test]
async fn credential_wait_notifies_through_the_outbox_without_any_run_over_full_ticks() {
    let e = E2e::new();
    let mut d = e.dispatcher();
    let clock = Arc::new(StdMutex::new(*e.clock.lock().unwrap()));
    d.test_now = Some(clock.clone());
    d.config.min_free_disk_mb = 0;
    d.set_cos_chat_launch(
        e.sqlite.clone(),
        crate::dispatcher::cos_chat::launch::CosChatLaunchConfig {
            worker_reserve_five_hour: 0.90,
            enabled: true,
            harness: "fake".into(),
            max_turns: 2,
            max_wall_secs: 30,
            unavailable_reason: None,
            data_dir: e.dir.path().to_path_buf(),
            db_path: e.dir.path().join("db.sqlite"),
            attachment_limits: Default::default(),
            api_base_url: "http://127.0.0.1:7700/api/v1".into(),
            web_base_url: Some("http://celeris.test".into()),
            triage: Default::default(),
        },
    );
    let tick = |d: &mut Dispatcher| {
        *clock.lock().unwrap() += time::Duration::seconds(1);
        d.tick().unwrap();
    };
    let count = |sql: &str| -> i64 {
        rusqlite::Connection::open(e.dir.path().join("db.sqlite"))
            .unwrap()
            .query_row(sql, [], |r| r.get(0))
            .unwrap()
    };
    let single_unit_task = || {
        let task = new_task(e.dir.path(), Check::Reviewer, 1);
        e.sqlite.insert(&task).unwrap();
        let a = unit(&task, "a", 0);
        e.sqlite
            .work_units_apply(task.id, vec![a.clone()], Vec::new(), Vec::new())
            .unwrap();
        (task, a)
    };

    tick(&mut d); // 導入: 空の受信箱
    let (t1, a1) = single_unit_task();
    hold(&mut d, &t1, &a1);
    for _ in 0..4 {
        tick(&mut d);
    }
    let pending = e.sqlite.notification_pending().unwrap();
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].kind, task_core::NotificationKind::CosEscalation);
    assert!(pending[0].body.contains("/accounts"), "{}", pending[0].body);
    assert!(pending[0].body.contains("自動で続行"), "{}", pending[0].body);
    assert!(!pending[0].body.contains(DUMMY));

    // 同じ秘密を待つ 2 つ目の task は通知を増やさない（再 tick・restart 相当の reconcile でも 1 件）。
    let (t2, a2) = single_unit_task();
    hold(&mut d, &t2, &a2);
    d.request_cos_triage_reconcile();
    for _ in 0..4 {
        tick(&mut d);
    }
    assert_eq!(e.sqlite.notification_pending().unwrap().len(), 1);

    // LLM run・worker run なし: runs 行・chat run・triage の system message・pending な triage item が 0。
    assert_eq!(e.runs(&t1) + e.runs(&t2), 0);
    assert_eq!(count("SELECT COUNT(*) FROM runs"), 0);
    assert_eq!(count("SELECT COUNT(*) FROM chat_runs"), 0);
    assert_eq!(
        count("SELECT COUNT(*) FROM chat_messages WHERE role='system'"),
        0
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM cos_inbox_items WHERE state IN ('pending','running')"),
        0
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM cos_inbox_items WHERE source_kind='credential_wait'"),
        1
    );
    assert_eq!(e.unit(&t1, "a").0, WorkUnitStatus::Blocked);
}
