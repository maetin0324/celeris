use super::*;
use crate::TaskStore;
use crate::model::{Budget, Task, TaskKind, Tier, WorkerHint, WorkspaceSpec};

const SENTINEL: &str = "SENTINEL-p4ssw0rd-9f3a";

fn task(status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "browser".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "ws".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 1,
            max_retries: 2,
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
        tree: None,
        paused_at: None,
    }
}

fn running(store: &SqliteStore) -> TaskId {
    let t = task(Status::Ready);
    store.insert(&t).expect("insert");
    assert!(
        store
            .acquire_lease(t.id, "run-1", std::time::Duration::from_secs(60))
            .expect("lease")
    );
    t.id
}

fn auth_request(key: &str) -> NewBrowserWait {
    NewBrowserWait {
        work_unit_id: None,
        run_id: "run-1".into(),
        session_id: "sess-1".into(),
        reason: BrowserWaitReason::WaitingForAuth,
        origin: "https://login.example.com".into(),
        purpose: "Sign in to read the build dashboard".into(),
        credential_policy_id: Some("pol-example".into()),
        credential: None,
        operation: None,
        policy_revision: 3,
        policy_hash: "sha256-abc".into(),
        owner_id: Some("owner".into()),
        ttl_secs: None,
        resume_key: key.into(),
    }
}

fn approval_request(key: &str) -> NewBrowserWait {
    NewBrowserWait {
        reason: BrowserWaitReason::WaitingForApproval,
        credential_policy_id: None,
        credential: Some(CredentialRef {
            credential_id: "cred-1".into(),
            provider: "manual".into(),
            policy_id: "pol-example".into(),
        }),
        operation: Some(OperationIntent {
            intent_id: "intent-1".into(),
            action: "credential_use".into(),
            args_digest: Some("sha256-args".into()),
        }),
        ttl_secs: Some(10_000),
        ..auth_request(key)
    }
}

fn decision(kind: BrowserDecision, version: u64, nonce: &str) -> HumanDecision {
    HumanDecision {
        decision: kind,
        expected_version: version,
        actor_id: "owner".into(),
        owner_session_hash: "sess-hash".into(),
        policy_hash: "sha256-abc".into(),
        nonce: nonce.into(),
        idempotency_key: format!("idem-{nonce}"),
    }
}

fn status(store: &SqliteStore, id: TaskId) -> Status {
    store.get(id).expect("get").expect("task").status
}

fn record() -> CredentialRecord {
    CredentialRecord {
        credential_id: "cred-1".into(),
        provider: "manual".into(),
        policy_id: "pol-example".into(),
        credential_revision: 1,
        origin: "https://login.example.com".into(),
        receipt_id: "rcpt-1".into(),
    }
}

#[test]
fn open_blocks_task_releases_lease_and_is_idempotent_by_resume_key() {
    let store = SqliteStore::open_in_memory().expect("open");
    let id = running(&store);
    let now = OffsetDateTime::now_utc();
    let open = store
        .browser_wait_open(id, &auth_request("rk-1"), now)
        .expect("open");
    assert!(open.created);
    assert_eq!(open.wait.state, BrowserWaitState::Pending);
    assert_eq!(open.wait.deadline, now + Duration::hours(24));
    let t = store.get(id).expect("get").expect("task");
    assert_eq!(t.status, Status::Blocked);
    assert!(t.lease.is_none(), "worker slot (lease) must be released");
    // 同じ resume_key の再送は同じ wait。
    let again = store
        .browser_wait_open(id, &auth_request("rk-1"), now)
        .expect("again");
    assert!(!again.created);
    assert_eq!(again.wait.wait_id, open.wait.wait_id);
    assert_eq!(store.browser_waits_for_task(id).expect("list").len(), 1);
    // 別の task が同じ resume_key を使うのは拒否。
    let other = running(&store);
    assert!(matches!(
        store.browser_wait_open(other, &auth_request("rk-1"), now),
        Err(BrowserWaitError::Invalid {
            field: "resume_key"
        })
    ));
    // events: transitioned(waiting_for_auth) + browser_wait_opened が同じ遷移で入る。
    let events: Vec<Event> = store
        .events_for(id)
        .expect("events")
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert!(events.iter().any(|e| matches!(e, Event::Transitioned { to: Status::Blocked, reason, .. } if reason == "waiting_for_auth")));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::BrowserWaitOpened { .. }))
    );
}

#[test]
fn open_rejects_non_running_task_and_bad_fields() {
    let store = SqliteStore::open_in_memory().expect("open");
    let t = task(Status::Ready);
    store.insert(&t).expect("insert");
    let now = OffsetDateTime::now_utc();
    assert!(matches!(
        store.browser_wait_open(t.id, &auth_request("rk"), now),
        Err(BrowserWaitError::TaskNotRunning)
    ));
    let id = running(&store);
    for (mutate, field) in [
        (
            Box::new(|r: &mut NewBrowserWait| r.origin = "https://*.example.com".into())
                as Box<dyn Fn(&mut NewBrowserWait)>,
            "origin",
        ),
        (
            Box::new(|r: &mut NewBrowserWait| r.origin = "https://u:p@example.com".into()),
            "origin",
        ),
        (
            Box::new(|r: &mut NewBrowserWait| r.origin = "https://example.com:443".into()),
            "origin",
        ),
        (
            Box::new(|r: &mut NewBrowserWait| r.origin = "https://example.com/login".into()),
            "origin",
        ),
        (
            Box::new(|r: &mut NewBrowserWait| r.origin = "http://example.com".into()),
            "origin",
        ),
        (
            Box::new(|r: &mut NewBrowserWait| r.purpose = "x".repeat(501)),
            "purpose",
        ),
        (
            Box::new(|r: &mut NewBrowserWait| r.purpose = "a\nb".into()),
            "purpose",
        ),
        (
            Box::new(|r: &mut NewBrowserWait| r.credential_policy_id = None),
            "credential_policy_id",
        ),
    ] {
        let mut r = auth_request("rk-bad");
        mutate(&mut r);
        match store.browser_wait_open(id, &r, now) {
            Err(BrowserWaitError::Invalid { field: got }) => assert_eq!(got, field),
            other => panic!("expected invalid {field}: {other:?}"),
        }
    }
    assert!(valid_exact_origin("https://example.com:8443"));
    assert_eq!(status(&store, id), Status::Running);
}

#[test]
fn register_resolves_once_and_does_not_approve_use() {
    let store = SqliteStore::open_in_memory().expect("open");
    let id = running(&store);
    let now = OffsetDateTime::now_utc();
    let wait = store
        .browser_wait_open(id, &auth_request("rk-1"), now)
        .expect("open")
        .wait;
    // 一般の回答・途中確認の再開では ready に戻せない。
    assert!(store.apply_transition(id, Trigger::Answer, None).is_err());
    assert_eq!(status(&store, id), Status::Blocked);
    // origin の差し替え・古い version は拒否。
    let mut wrong = record();
    wrong.origin = "https://evil.example.com".into();
    assert!(matches!(
        store.browser_wait_register(id, &wait.wait_id, wait.version, &wrong, "owner", now),
        Err(BrowserWaitError::Invalid { field: "origin" })
    ));
    assert!(matches!(
        store.browser_wait_register(id, &wait.wait_id, 99, &record(), "owner", now),
        Err(BrowserWaitError::VersionConflict)
    ));
    // 登録依頼は approve_once できない。
    assert!(matches!(
        store.browser_wait_decide(
            id,
            &wait.wait_id,
            &decision(BrowserDecision::ApproveOnce, wait.version, "n0"),
            now
        ),
        Err(BrowserWaitError::InvalidState { .. })
    ));
    let r = store
        .browser_wait_register(id, &wait.wait_id, wait.version, &record(), "owner", now)
        .expect("register");
    assert!(!r.replayed);
    assert_eq!(r.task_status, Status::Ready);
    assert_eq!(r.wait.state, BrowserWaitState::Registered);
    assert_eq!(r.wait.version, 2);
    assert!(r.wait.approval_id.is_none(), "registration is not approval");
    // 再送は冪等。
    let again = store
        .browser_wait_register(id, &wait.wait_id, wait.version, &record(), "owner", now)
        .expect("replay");
    assert!(again.replayed);
    assert_eq!(
        store
            .browser_credential_get("cred-1")
            .expect("get")
            .expect("credential")
            .origin,
        "https://login.example.com"
    );
}

#[test]
fn approve_once_consumes_once_and_deny_fails_task() {
    let store = SqliteStore::open_in_memory().expect("open");
    let id = running(&store);
    let now = OffsetDateTime::now_utc();
    let wait = store
        .browser_wait_open(id, &approval_request("rk-a"), now)
        .expect("open")
        .wait;
    // 承認待ちは既定・上限 5 分（要求の 10000 秒は丸める）。
    assert_eq!(wait.deadline, now + Duration::minutes(5));
    assert!(matches!(
        store.browser_wait_decide(
            id,
            &wait.wait_id,
            &decision(BrowserDecision::ApproveOnce, 7, "n1"),
            now
        ),
        Err(BrowserWaitError::VersionConflict)
    ));
    let mut bad_policy = decision(BrowserDecision::ApproveOnce, wait.version, "n1");
    bad_policy.policy_hash = "sha256-other".into();
    assert!(matches!(
        store.browser_wait_decide(id, &wait.wait_id, &bad_policy, now),
        Err(BrowserWaitError::Invalid {
            field: "policy_hash"
        })
    ));
    let d = decision(BrowserDecision::ApproveOnce, wait.version, "n1");
    let r = store
        .browser_wait_decide(id, &wait.wait_id, &d, now)
        .expect("approve");
    assert_eq!(r.task_status, Status::Ready);
    assert_eq!(r.wait.state, BrowserWaitState::Approved);
    // 同じ idempotency_key の再送は冪等、別の決定で同じ nonce は replay。
    assert!(
        store
            .browser_wait_decide(id, &wait.wait_id, &d, now)
            .expect("replay")
            .replayed
    );
    let mut reuse = decision(BrowserDecision::Deny, r.wait.version, "n1");
    reuse.idempotency_key = "idem-other".into();
    assert!(matches!(
        store.browser_wait_decide(id, &wait.wait_id, &reuse, now),
        Err(BrowserWaitError::Replay)
    ));
    let consumed = store
        .browser_wait_consume(id, &wait.wait_id, "rk-a", "run-1", "sess-1", now)
        .expect("consume");
    assert_eq!(consumed.state, BrowserWaitState::Resumed);
    assert!(matches!(
        store.browser_wait_consume(id, &wait.wait_id, "rk-a", "run-1", "sess-1", now),
        Err(BrowserWaitError::InvalidState { op: "consume" })
    ));
    let approvals = store
        .browser_approvals_for_wait(&wait.wait_id)
        .expect("approvals");
    assert_eq!(approvals.len(), 1);
    assert!(approvals[0].consumed_at.is_some());

    // 拒否: task は failed（自動 retry なし）。
    let id2 = running(&store);
    let w2 = store
        .browser_wait_open(id2, &approval_request("rk-b"), now)
        .expect("open")
        .wait;
    let r2 = store
        .browser_wait_decide(
            id2,
            &w2.wait_id,
            &decision(BrowserDecision::Deny, w2.version, "n2"),
            now,
        )
        .expect("deny");
    assert_eq!(r2.task_status, Status::Failed);
    assert_eq!(r2.wait.state, BrowserWaitState::Denied);
    assert_eq!(r2.wait.resolution_code.as_deref(), Some("approval_denied"));
    let t2 = store.get(id2).expect("get").expect("task");
    assert_eq!(t2.attempts, 0);
}

#[test]
fn consume_in_other_session_invalidates_and_revoke_blocks_consume() {
    let store = SqliteStore::open_in_memory().expect("open");
    let now = OffsetDateTime::now_utc();
    let id = running(&store);
    let w = store
        .browser_wait_open(id, &approval_request("rk-s"), now)
        .expect("open")
        .wait;
    store
        .browser_wait_decide(
            id,
            &w.wait_id,
            &decision(BrowserDecision::ApproveOnce, w.version, "n1"),
            now,
        )
        .expect("approve");
    assert!(matches!(
        store.browser_wait_consume(id, &w.wait_id, "rk-s", "run-1", "sess-2", now),
        Err(BrowserWaitError::Gone)
    ));
    assert_eq!(
        store
            .browser_wait_get(&w.wait_id)
            .expect("get")
            .expect("wait")
            .state,
        BrowserWaitState::Invalidated
    );

    let id2 = running(&store);
    let w2 = store
        .browser_wait_open(id2, &approval_request("rk-r"), now)
        .expect("open")
        .wait;
    let approved = store
        .browser_wait_decide(
            id2,
            &w2.wait_id,
            &decision(BrowserDecision::ApproveOnce, w2.version, "n2"),
            now,
        )
        .expect("approve");
    let revoked = store
        .browser_wait_decide(
            id2,
            &w2.wait_id,
            &decision(BrowserDecision::Revoke, approved.wait.version, "n3"),
            now,
        )
        .expect("revoke");
    assert_eq!(revoked.wait.state, BrowserWaitState::Revoked);
    assert!(matches!(
        store.browser_wait_consume(id2, &w2.wait_id, "rk-r", "run-1", "sess-1", now),
        Err(BrowserWaitError::Gone)
    ));
}

#[test]
fn expiry_terminates_exactly_once_and_cancel_closes_waits() {
    let store = SqliteStore::open_in_memory().expect("open");
    let now = OffsetDateTime::now_utc();
    let id = running(&store);
    let w = store
        .browser_wait_open(id, &approval_request("rk-e"), now)
        .expect("open")
        .wait;
    assert!(
        store
            .browser_waits_expire(now + Duration::minutes(4))
            .expect("not yet")
            .is_empty()
    );
    let later = now + Duration::minutes(6);
    // 期限後の承認は 410 相当で、wait は期限切れとして終端化される。
    assert!(matches!(
        store.browser_wait_decide(
            id,
            &w.wait_id,
            &decision(BrowserDecision::ApproveOnce, w.version, "n1"),
            later
        ),
        Err(BrowserWaitError::Gone)
    ));
    assert_eq!(status(&store, id), Status::Failed);
    // 照合でもう一度終端化しない。
    assert!(
        store
            .browser_waits_expire(later)
            .expect("expire")
            .is_empty()
    );
    let resolved = store
            .events_for(id)
            .expect("events")
            .into_iter()
            .filter(|(_, e)| matches!(e, Event::BrowserWaitResolved { code, .. } if code == "browser_wait_expired"))
            .count();
    assert_eq!(resolved, 1);

    // 照合（tick）による期限切れ。
    let id2 = running(&store);
    store
        .browser_wait_open(id2, &auth_request("rk-e2"), now)
        .expect("open");
    let expired = store
        .browser_waits_expire(now + Duration::hours(25))
        .expect("expire");
    assert_eq!(expired.len(), 1);
    assert_eq!(status(&store, id2), Status::Failed);
    assert!(
        store
            .browser_waits_expire(now + Duration::hours(26))
            .expect("again")
            .is_empty()
    );

    // cancel: 開いている wait は cancelled。
    let id3 = running(&store);
    let w3 = store
        .browser_wait_open(id3, &auth_request("rk-c"), now)
        .expect("open")
        .wait;
    store
        .apply_transition(id3, Trigger::Cancel, None)
        .expect("cancel");
    let w3 = store
        .browser_wait_get(&w3.wait_id)
        .expect("get")
        .expect("wait");
    assert_eq!(w3.state, BrowserWaitState::Cancelled);
    assert!(store.browser_waits_pending().expect("pending").is_empty());
}

#[test]
fn waits_survive_restart_and_resume() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("celeris.db");
    let now = OffsetDateTime::now_utc();
    let (id, wait) = {
        let store = SqliteStore::open(&path).expect("open");
        let id = running(&store);
        let wait = store
            .browser_wait_open(id, &approval_request("rk-restart"), now)
            .expect("open")
            .wait;
        (id, wait)
    };
    let store = SqliteStore::open(&path).expect("reopen");
    // 再起動時の照合: 期限内なので残る。
    assert!(store.browser_waits_expire(now).expect("expire").is_empty());
    let pending = store.browser_waits_pending().expect("pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].wait_id, wait.wait_id);
    let r = store
        .browser_wait_decide(
            id,
            &wait.wait_id,
            &decision(BrowserDecision::ApproveOnce, wait.version, "n-restart"),
            now,
        )
        .expect("approve after restart");
    assert_eq!(r.task_status, Status::Ready);
    store
        .browser_wait_consume(id, &wait.wait_id, "rk-restart", "run-1", "sess-1", now)
        .expect("consume after restart");
}

/// 秘密を受け取る型がここには無いこと、DB の全行・event に秘密が混ざらないこと（API 層の試験と
/// 対になる）: 秘密っぽい値を他の欄に入れようとしても検証で落ち、どこにも書かれない。
#[test]
fn secret_like_values_never_reach_rows_or_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("celeris.db");
    let store = SqliteStore::open(&path).expect("open");
    let now = OffsetDateTime::now_utc();
    let id = running(&store);
    let mut r = auth_request("rk-x");
    r.policy_hash = format!("{SENTINEL}!");
    assert!(store.browser_wait_open(id, &r, now).is_err());
    let w = store
        .browser_wait_open(id, &auth_request("rk-x"), now)
        .expect("open")
        .wait;
    store
        .browser_wait_register(id, &w.wait_id, w.version, &record(), "owner", now)
        .expect("register");
    drop(store);
    let raw = std::fs::read(&path).expect("db");
    let hay = String::from_utf8_lossy(&raw);
    assert!(!hay.contains(SENTINEL));
}
