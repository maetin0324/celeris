use super::*;
use crate::TaskStore;
use crate::model::{Budget, Task, TaskKind, Tier, WorkerHint, WorkspaceSpec};

const SENTINEL: &str = "SENTINEL-p4ssw0rd-9f3a";

fn task(status: Status) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        requirements: Default::default(),
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
        trusted_login: None,
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
    // 承認待ちは既定・上限 30 分（要求の 10000 秒は丸める。ADR 2026-10-08 D3）。
    assert_eq!(wait.deadline, now + Duration::minutes(30));
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
    // ADR 2026-10-08 D3: the approval deadline is 30 minutes (was 5).
    assert_eq!(
        w.deadline,
        now + Duration::seconds(APPROVAL_WAIT_MAX_SECS as i64)
    );
    assert!(
        store
            .browser_waits_expire(now + Duration::minutes(29))
            .expect("not yet")
            .is_empty()
    );
    let later = now + Duration::minutes(31);
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

// ---- ADR-0110 D2: trusted login の形式検証・固定・照合 ----

const LOGIN_ORIGIN: &str = "https://login.example.com";

fn trusted() -> TrustedLogin {
    TrustedLogin {
        policy_id: "pol-example".into(),
        revision: 3,
        login_url: format!("{LOGIN_ORIGIN}/signin?next=%2F"),
        password_selector: "form#login > input[name=\"password\"]".into(),
        submit_selector: Some("button[type=submit]".into()),
        username_selector: None,
        post_login: None,
    }
}

/// ADR 2026-10-09 credential username / post-login D1-1・D2-1: 2 欄と post_login の TrustedLogin。
#[test]
fn trusted_login_with_username_and_post_login_validates_and_round_trips() {
    let mut t = trusted();
    t.username_selector = Some("input[name=j_username]".into());
    t.post_login = Some(PostLogin {
        read_origins: vec!["https://lms.example.com".into()],
        actions: vec![PostLoginAction::Snapshot, PostLoginAction::Download],
    });
    assert_eq!(t.validate(LOGIN_ORIGIN), Ok(()));
    let json = serde_json::to_string(&t).expect("json");
    assert_eq!(
        serde_json::from_str::<TrustedLogin>(&json).expect("back"),
        t
    );
    // 旧い wait の JSON（欄なし）はそのまま読め、比較では別物になる（policy_changed）。
    let old = serde_json::to_string(&trusted()).expect("json");
    assert!(!old.contains("username_selector") && !old.contains("post_login"));
    assert_ne!(serde_json::from_str::<TrustedLogin>(&old).expect("old"), t);
    let mut bad = t.clone();
    bad.username_selector = Some("input:focus".into());
    assert_eq!(bad.validate(LOGIN_ORIGIN), Err("username_selector"));
    let mut bad = t.clone();
    bad.post_login = Some(PostLogin {
        read_origins: vec![LOGIN_ORIGIN.into()],
        actions: vec![PostLoginAction::Snapshot],
    });
    assert_eq!(bad.validate(LOGIN_ORIGIN), Err("post_login_idp_origin"));
    assert_eq!(PostLoginAction::Extract.upstream_action(), "gettext");
    assert_eq!(PostLoginAction::Click.upstream_action(), "click");
}

#[test]
fn trusted_selector_grammar_accepts_only_the_adr_subset() {
    for ok in [
        "input",
        "#pass",
        ".login-form input[type=password]",
        "form#login>input[name=\"pass word\"]",
        "div.a.b > form > input[autocomplete=current-password]",
        "input[data-x]",
    ] {
        assert_eq!(validate_trusted_selector(ok), Ok(()), "{ok}");
    }
    for bad in [
        "",
        "input, #other",
        "input:not([type=text])",
        "input::after",
        "*",
        "a + input",
        "a ~ input",
        "iframe >>> input",
        "iframe /deep/ input",
        "#pa\\ss",
        "css=input",
        "xpath=//input",
        "text=Password",
        "internal:role=textbox",
        "frame=login >> input",
        "@e12",
        "input[name='p']",
        "input[name=\"p]",
        "input >",
        "> input",
        "input\n#p",
        "ｉnput",
        "INPUT",
        "#1abc",
    ] {
        assert!(validate_trusted_selector(bad).is_err(), "{bad:?}");
    }
    let long = format!("#{}", "a".repeat(TRUSTED_SELECTOR_MAX_LEN));
    assert_eq!(validate_trusted_selector(&long), Err("selector_length"));
    let deep = ["div"; TRUSTED_SELECTOR_MAX_COMPOUNDS + 1].join(" ");
    assert_eq!(
        validate_trusted_selector(&deep),
        Err("selector_too_complex")
    );
    let max = ["div"; TRUSTED_SELECTOR_MAX_COMPOUNDS].join(" > ");
    assert_eq!(validate_trusted_selector(&max), Ok(()));
}

#[test]
fn trusted_login_url_must_stay_on_the_exact_origin() {
    let o = LOGIN_ORIGIN;
    assert_eq!(validate_trusted_login_url(&format!("{o}/login"), o), Ok(()));
    for bad in [
        o.to_string(),
        format!("{o}.evil.test/login"),
        format!("{o}@evil.test/login"),
        format!("{o}:444/login"),
        format!("{o}/login#frag"),
        format!("{o}/lo gin"),
        format!("{o}/lo\\gin"),
        "http://login.example.com/login".to_string(),
        "https://other.example.com/login".to_string(),
        format!("{o}/{}", "a".repeat(TRUSTED_LOGIN_URL_MAX_LEN)),
    ] {
        assert!(validate_trusted_login_url(&bad, o).is_err(), "{bad}");
    }
    assert!(validate_trusted_login_url("http://x/login", "http://x").is_err());
}

#[test]
fn trusted_login_is_pinned_only_on_valid_credential_use_approvals() {
    let mut req = approval_request("rk-t");
    req.trusted_login = Some(trusted());
    assert!(req.validate().is_ok());
    let invalid = |t: TrustedLogin| {
        let mut r = approval_request("rk-t");
        r.trusted_login = Some(t);
        matches!(
            r.validate(),
            Err(BrowserWaitError::Invalid {
                field: "trusted_login"
            })
        )
    };
    assert!(invalid(TrustedLogin {
        password_selector: "input, #x".into(),
        ..trusted()
    }));
    assert!(invalid(TrustedLogin {
        submit_selector: Some("button:hover".into()),
        ..trusted()
    }));
    assert!(invalid(TrustedLogin {
        login_url: "https://evil.example.com/login".into(),
        ..trusted()
    }));
    assert!(invalid(TrustedLogin {
        policy_id: "pol-other".into(),
        ..trusted()
    }));
    assert!(invalid(TrustedLogin {
        revision: 4,
        ..trusted()
    }));
    // 登録依頼には固定しない。
    let mut auth = auth_request("rk-u");
    auth.trusted_login = Some(trusted());
    assert!(auth.validate().is_err());
}

#[test]
fn model_requests_cannot_carry_selectors() {
    // モデル・worker が組む操作 intent に selector・URL を混ぜても読まない（deny_unknown_fields）。
    for extra in [
        r##""selector":"#evil""##,
        r#""password_selector":"input""#,
        r#""login_url":"https://evil.example.com/""#,
        r#""trusted_login":{}"#,
    ] {
        let raw = format!(r#"{{"intent_id":"i-1","action":"credential_use",{extra}}}"#);
        assert!(
            serde_json::from_str::<OperationIntent>(&raw).is_err(),
            "{raw}"
        );
    }
    let raw = r##"{"credential_id":"c","provider":"manual","policy_id":"p","selector":"#x"}"##;
    assert!(serde_json::from_str::<CredentialRef>(raw).is_err());
}

/// 登録済み credential と承認済みの credential 使用 wait を用意する。
fn approved_credential_use(
    store: &SqliteStore,
    pinned: Option<TrustedLogin>,
) -> (TaskId, BrowserWait) {
    let now = OffsetDateTime::now_utc();
    let id = running(store);
    let auth = store
        .browser_wait_open(id, &auth_request("rk-reg"), now)
        .expect("open auth")
        .wait;
    store
        .browser_wait_register(
            id,
            &auth.wait_id,
            auth.version,
            &CredentialRecord {
                credential_id: "cred-1".into(),
                provider: "manual".into(),
                policy_id: "pol-example".into(),
                credential_revision: 1,
                origin: LOGIN_ORIGIN.into(),
                receipt_id: "rcpt-1".into(),
            },
            "human-1",
            now,
        )
        .expect("register");
    assert!(
        store
            .acquire_lease(id, "run-1", std::time::Duration::from_secs(60))
            .expect("lease")
    );
    let mut req = approval_request("rk-use");
    req.trusted_login = pinned;
    let wait = store.browser_wait_open(id, &req, now).expect("open").wait;
    let d = decision(BrowserDecision::ApproveOnce, wait.version, "n-use");
    let wait = store
        .browser_wait_decide(id, &wait.wait_id, &d, now)
        .expect("approve")
        .wait;
    (id, wait)
}

#[test]
fn consumed_approval_carries_the_pinned_selector_and_rejects_request_selectors() {
    let store = SqliteStore::open_in_memory().expect("open");
    let now = OffsetDateTime::now_utc();
    let (id, wait) = approved_credential_use(&store, Some(trusted()));
    // 耐久化した固定値は読み戻しても同じ。
    let stored = store
        .browser_wait_get(&wait.wait_id)
        .expect("get")
        .expect("wait");
    assert_eq!(stored.trusted_login, Some(trusted()));

    // 差し替え: 呼出し側の wait の selector を書き換えても採用せず、一回承認も消費しない。
    let mut swapped = wait.clone();
    swapped.trusted_login = Some(TrustedLogin {
        password_selector: "#attacker".into(),
        ..trusted()
    });
    assert_eq!(
        consume_credential_approval(&store, id, &swapped, now),
        Err("selector_mismatch")
    );
    let mut dropped = wait.clone();
    dropped.trusted_login = None;
    assert_eq!(
        consume_credential_approval(&store, id, &dropped, now),
        Err("selector_mismatch")
    );
    let still = store
        .browser_wait_get(&wait.wait_id)
        .expect("get")
        .expect("wait");
    assert_eq!(still.state, BrowserWaitState::Approved);

    let consumed = consume_credential_approval(&store, id, &wait, now).expect("consume");
    assert_eq!(consumed.trusted_login, Some(trusted()));
    let pinned = trusted().password_selector;
    assert_eq!(consumed.injection_selector(None), Ok(pinned.as_str()));
    assert_eq!(
        consumed.injection_selector(Some(&pinned)),
        Ok(pinned.as_str())
    );
    // 要求側の selector の不一致（1 byte 違い・空・別要素）は拒否。
    for other in [
        "#attacker",
        "",
        "form#login > input[name=\"password\"] ",
        "input",
    ] {
        assert_eq!(
            consumed.injection_selector(Some(other)),
            Err("selector_mismatch"),
            "{other:?}"
        );
    }
    // 固定後に値を書き換えた ConsumedBrowserApproval（出所が壊れている）も注入に使えない。
    let mut tampered = consumed.clone();
    if let Some(t) = tampered.trusted_login.as_mut() {
        t.password_selector = "input:not([x])".into();
    }
    assert_eq!(
        tampered.injection_selector(None),
        Err("trusted_selector_missing")
    );
}

#[test]
fn approval_without_pinned_selector_cannot_inject() {
    let store = SqliteStore::open_in_memory().expect("open");
    let now = OffsetDateTime::now_utc();
    let (id, wait) = approved_credential_use(&store, None);
    let consumed = consume_credential_approval(&store, id, &wait, now).expect("consume");
    assert_eq!(consumed.trusted_login, None);
    assert_eq!(
        consumed.injection_selector(None),
        Err("trusted_selector_missing")
    );
    assert_eq!(
        consumed.injection_selector(Some("#pass")),
        Err("trusted_selector_missing")
    );
}

/// ADR 2026-10-08-browser-click-download-approval-policy D3: 承認待ちの既定・上限は 30 分、登録待ちは 24 時間のまま。
/// 時計は `browser_wait_open` / `browser_waits_expire` の引数で差し替える（実時間は使わない）。
#[test]
fn approval_wait_defaults_and_caps_at_thirty_minutes_and_expires_once() {
    assert_eq!(APPROVAL_WAIT_MAX_SECS, 30 * 60);
    assert_eq!(AUTH_WAIT_MAX_SECS, 24 * 60 * 60);
    let store = SqliteStore::open_in_memory().expect("open");
    let now = OffsetDateTime::now_utc();

    // ttl_secs 無し → 既定 30 分。
    let id = running(&store);
    let mut request = approval_request("rk-30");
    request.ttl_secs = None;
    let w = store
        .browser_wait_open(id, &request, now)
        .expect("open")
        .wait;
    assert_eq!(w.deadline, now + Duration::minutes(30));

    // ttl_secs が上限を超える → 30 分に clamp（approval_request は 10_000 秒を頼む）。
    let id2 = running(&store);
    let w2 = store
        .browser_wait_open(id2, &approval_request("rk-cap"), now)
        .expect("open")
        .wait;
    assert_eq!(w2.deadline, now + Duration::minutes(30));

    // 29 分では期限切れにならず、30 分ちょうどから一度だけ終端化する。
    assert!(
        store
            .browser_waits_expire(now + Duration::minutes(29) + Duration::seconds(59))
            .expect("not yet")
            .is_empty()
    );
    assert_eq!(status(&store, id), Status::Blocked);
    let expired = store
        .browser_waits_expire(now + Duration::minutes(30))
        .expect("expire");
    assert_eq!(expired.len(), 2);
    assert_eq!(status(&store, id), Status::Failed);
    assert_eq!(status(&store, id2), Status::Failed);
    assert!(
        store
            .browser_waits_expire(now + Duration::minutes(31))
            .expect("again")
            .is_empty()
    );

    // 登録待ちは変わらず 24 時間。
    let id3 = running(&store);
    let w3 = store
        .browser_wait_open(id3, &auth_request("rk-auth"), now)
        .expect("open")
        .wait;
    assert_eq!(w3.deadline, now + Duration::hours(24));
}

fn operation_request(key: &str) -> NewBrowserWait {
    NewBrowserWait {
        reason: BrowserWaitReason::WaitingForApproval,
        credential_policy_id: None,
        credential: None,
        operation: Some(OperationIntent {
            intent_id: "intent-click-1".into(),
            action: "click".into(),
            args_digest: Some("sha256-click-e3".into()),
        }),
        origin: "https://example.com".into(),
        purpose: "Press the export button".into(),
        ttl_secs: None,
        ..auth_request(key)
    }
}

/// ADR 2026-10-08 D2: click / download を承認対象に戻したときの承認待ち。credential 無しの操作 intent で開き、
/// 承認後に同じ run/session で一度だけ消費し、30 分で期限切れになる（時計は引数）。
#[test]
fn operation_approval_opens_without_credential_consumes_once_and_expires_at_thirty_minutes() {
    let store = SqliteStore::open_in_memory().expect("open");
    let now = OffsetDateTime::now_utc();
    let id = running(&store);
    let opened = store
        .browser_wait_open(id, &operation_request("rk-op"), now)
        .expect("open")
        .wait;
    assert_eq!(opened.reason, BrowserWaitReason::WaitingForApproval);
    assert_eq!(opened.deadline, now + Duration::minutes(30));
    assert!(opened.credential.is_none());
    assert_eq!(
        opened.operation.as_ref().map(|o| o.action.as_str()),
        Some("click")
    );
    assert_eq!(status(&store, id), Status::Blocked);
    assert_eq!(store.browser_waits_pending().expect("pending").len(), 1);

    // 未承認では消費できない（credential 用の消費経路も操作 wait を受けない）。
    assert_eq!(
        consume_operation_approval(&store, id, &opened, now).unwrap_err(),
        "browser_wait_state"
    );
    assert_eq!(
        consume_credential_approval(&store, id, &opened, now).unwrap_err(),
        "browser approval is not a credential use"
    );

    let approved = store
        .browser_wait_decide(
            id,
            &opened.wait_id,
            &decision(BrowserDecision::ApproveOnce, opened.version, "n-op"),
            now,
        )
        .expect("approve");
    assert_eq!(approved.task_status, Status::Ready);
    assert_eq!(approved.wait.state, BrowserWaitState::Approved);

    // 呼出し側の intent が store と食い違えば消費しない。
    let mut swapped = approved.wait.clone();
    swapped.operation = Some(OperationIntent {
        intent_id: "intent-click-1".into(),
        action: "download".into(),
        args_digest: Some("sha256-click-e3".into()),
    });
    assert_eq!(
        consume_operation_approval(&store, id, &swapped, now).unwrap_err(),
        "browser approval intent mismatch"
    );
    assert_eq!(
        store
            .browser_wait_get(&opened.wait_id)
            .expect("get")
            .expect("wait")
            .state,
        BrowserWaitState::Approved
    );

    // 同じ run/session の continuation として一度だけ。
    let consumed = consume_operation_approval(&store, id, &approved.wait, now).expect("consume");
    assert_eq!(consumed.action, "click");
    assert_eq!(consumed.approved_by, "owner");
    assert_eq!(consumed.wait.state, BrowserWaitState::Resumed);
    assert_eq!(consumed.wait.session_id, "sess-1");
    assert_eq!(
        consume_operation_approval(&store, id, &approved.wait, now).unwrap_err(),
        "browser_wait_state"
    );

    // 期限: 29 分 59 秒では残り、30 分で一度だけ期限切れ（pending の task は failed）。
    let id2 = running(&store);
    let w2 = store
        .browser_wait_open(id2, &operation_request("rk-op-2"), now)
        .expect("open")
        .wait;
    assert!(
        store
            .browser_waits_expire(now + Duration::minutes(29) + Duration::seconds(59))
            .expect("not yet")
            .is_empty()
    );
    let expired = store
        .browser_waits_expire(now + Duration::minutes(30))
        .expect("expire");
    assert_eq!(
        expired
            .iter()
            .map(|w| w.wait_id.as_str())
            .collect::<Vec<_>>(),
        vec![w2.wait_id.as_str()]
    );
    assert_eq!(status(&store, id2), Status::Failed);
    assert!(
        store
            .browser_waits_expire(now + Duration::minutes(31))
            .expect("again")
            .is_empty()
    );
}
