use super::*;

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().expect("open")
}

#[test]
fn create_then_active_round_trips() {
    let store = store();
    let now = OffsetDateTime::now_utc();
    assert_eq!(
        store
            .node_session_active("cos", SessionKind::Conversation, None)
            .expect("active"),
        None
    );
    let session = NodeSession::new(
        "cos",
        SessionKind::Conversation,
        None,
        "claude-code",
        Some("acct-a".to_string()),
        "sess-1",
        now,
    );
    store.node_session_create(&session).expect("create");
    let active = store
        .node_session_active("cos", SessionKind::Conversation, None)
        .expect("active")
        .expect("some");
    assert_eq!(active.session_id, "sess-1");
    assert_eq!(active.account_id.as_deref(), Some("acct-a"));
    assert_eq!(active.turns, 0);
    assert_eq!(active.approx_tokens, 0);
    assert!(active.retired_at.is_none());
}

#[test]
fn touch_bumps_turns_and_tokens_and_set_id_updates_the_session_id_only() {
    let store = store();
    let now = OffsetDateTime::now_utc();
    let session = NodeSession::new(
        "cos",
        SessionKind::Conversation,
        None,
        "codex",
        None,
        "", // codex: 確定するまで空。
        now,
    );
    store.node_session_create(&session).expect("create");

    // `EventSink::session_established` 相当: run の途中で id が確定する（turns/tokens は動かない）。
    let set = store
        .node_session_set_id("cos", SessionKind::Conversation, None, "thread-abc")
        .expect("set_id");
    assert!(set);
    let active = store
        .node_session_active("cos", SessionKind::Conversation, None)
        .expect("active")
        .expect("some");
    assert_eq!(active.session_id, "thread-abc");
    assert_eq!(active.turns, 0);
    assert_eq!(active.approx_tokens, 0);

    // run 完了後、usage が分かった時点で touch する。
    let touched = store
        .node_session_touch(
            "cos",
            SessionKind::Conversation,
            None,
            1234,
            now + time::Duration::seconds(5),
        )
        .expect("touch");
    assert!(touched);

    let active = store
        .node_session_active("cos", SessionKind::Conversation, None)
        .expect("active")
        .expect("some");
    assert_eq!(active.turns, 1);
    assert_eq!(active.approx_tokens, 1234);
    assert_eq!(active.session_id, "thread-abc");

    // 2 回目の touch: 累積だけ増える。
    store
        .node_session_touch(
            "cos",
            SessionKind::Conversation,
            None,
            100,
            now + time::Duration::seconds(10),
        )
        .expect("touch 2");
    let active = store
        .node_session_active("cos", SessionKind::Conversation, None)
        .expect("active")
        .expect("some");
    assert_eq!(active.turns, 2);
    assert_eq!(active.approx_tokens, 1334);
    assert_eq!(active.session_id, "thread-abc");
}

#[test]
fn set_id_and_touch_are_no_ops_when_there_is_no_active_session() {
    let store = store();
    let now = OffsetDateTime::now_utc();
    assert!(
        !store
            .node_session_set_id("cos", SessionKind::Conversation, None, "x")
            .expect("set_id")
    );
    assert!(
        !store
            .node_session_touch("cos", SessionKind::Conversation, None, 10, now)
            .expect("touch")
    );
}

#[test]
fn retire_then_create_replaces_the_active_session() {
    let store = store();
    let now = OffsetDateTime::now_utc();
    let first = NodeSession::new(
        "engineering",
        SessionKind::Lead,
        None,
        "claude-code",
        Some("acct-a".to_string()),
        "sess-1",
        now,
    );
    store.node_session_create(&first).expect("create 1");
    let retired = store
        .node_session_retire(
            "engineering",
            SessionKind::Lead,
            None,
            now + time::Duration::seconds(1),
        )
        .expect("retire");
    assert!(retired);
    // 2 回目の retire は何もしない（もう有効な行が無い）。
    assert!(
        !store
            .node_session_retire(
                "engineering",
                SessionKind::Lead,
                None,
                now + time::Duration::seconds(2)
            )
            .expect("retire again")
    );
    assert_eq!(
        store
            .node_session_active("engineering", SessionKind::Lead, None)
            .expect("active"),
        None
    );

    let second = NodeSession::new(
        "engineering",
        SessionKind::Lead,
        None,
        "claude-code",
        Some("acct-b".to_string()),
        "sess-2",
        now + time::Duration::seconds(2),
    );
    store.node_session_create(&second).expect("create 2");
    let active = store
        .node_session_active("engineering", SessionKind::Lead, None)
        .expect("active")
        .expect("some");
    assert_eq!(active.session_id, "sess-2");
    assert_eq!(active.account_id.as_deref(), Some("acct-b"));
}

#[test]
fn conversation_and_lead_kinds_and_different_nodes_are_independent() {
    let store = store();
    let now = OffsetDateTime::now_utc();
    store
        .node_session_create(&NodeSession::new(
            "cos",
            SessionKind::Conversation,
            None,
            "claude-code",
            None,
            "cos-sess",
            now,
        ))
        .expect("create cos");
    store
        .node_session_create(&NodeSession::new(
            "engineering",
            SessionKind::Lead,
            None,
            "claude-code",
            None,
            "eng-sess",
            now,
        ))
        .expect("create eng");
    store
        .node_session_create(&NodeSession::new(
            "research",
            SessionKind::Lead,
            None,
            "codex",
            None,
            "res-sess",
            now,
        ))
        .expect("create res");

    assert_eq!(
        store
            .node_session_active("cos", SessionKind::Conversation, None)
            .expect("active")
            .expect("some")
            .session_id,
        "cos-sess"
    );
    assert_eq!(
        store
            .node_session_active("engineering", SessionKind::Lead, None)
            .expect("active")
            .expect("some")
            .session_id,
        "eng-sess"
    );
    assert_eq!(
        store
            .node_session_active("research", SessionKind::Lead, None)
            .expect("active")
            .expect("some")
            .session_id,
        "res-sess"
    );
    // 部署が無いノードの Lead セッションを引いても None（他の部署の行に混ざらない）。
    assert_eq!(
        store
            .node_session_active("ops", SessionKind::Lead, None)
            .expect("active"),
        None
    );
}

#[test]
fn kind_as_str_and_parse_round_trip() {
    for kind in [
        SessionKind::Conversation,
        SessionKind::Lead,
        SessionKind::Continuation,
    ] {
        assert_eq!(SessionKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(SessionKind::parse("bogus"), None);
}

// --- ADR-0140 D2: WU 単位の継続セッション（work_unit_session_*） ---

fn wu_session(
    task_id: TaskId,
    wu: Option<&str>,
    account: Option<&str>,
    session_id: &str,
    now: OffsetDateTime,
) -> WorkUnitSession {
    WorkUnitSession::new(
        "software-engineering",
        task_id,
        wu.map(str::to_string),
        "claude-code",
        account.map(str::to_string),
        Some("anthropic".to_string()),
        Some("/ws/wu/session-key".to_string()),
        session_id,
        now,
    )
}

#[test]
fn work_unit_session_create_then_active_round_trips() {
    let store = store();
    let now = OffsetDateTime::now_utc();
    let task = TaskId::new();
    assert_eq!(
        store
            .work_unit_session_active(task, Some("wu-a"), "claude-code", Some("acct-a"))
            .expect("active"),
        None
    );
    let created = wu_session(task, Some("wu-a"), Some("acct-a"), "uuid-1", now);
    store.work_unit_session_create(&created).expect("create");
    let active = store
        .work_unit_session_active(task, Some("wu-a"), "claude-code", Some("acct-a"))
        .expect("active")
        .expect("some");
    assert_eq!(active.id, created.id);
    assert_eq!(active.task_id, task);
    assert_eq!(active.work_unit_id.as_deref(), Some("wu-a"));
    assert_eq!(active.session_id, "uuid-1");
    assert_eq!(active.provider.as_deref(), Some("anthropic"));
    assert_eq!(active.cwd.as_deref(), Some("/ws/wu/session-key"));
    assert_eq!(active.turns, 0);
    assert!(active.retired_at.is_none());

    // touch は key が一致する行だけを進める。
    assert!(
        store
            .work_unit_session_touch(
                task,
                Some("wu-a"),
                "claude-code",
                Some("acct-a"),
                500,
                now + time::Duration::seconds(3),
            )
            .expect("touch")
    );
    assert!(
        !store
            .work_unit_session_touch(task, Some("wu-a"), "claude-code", Some("acct-b"), 9, now)
            .expect("touch other account")
    );
    let active = store
        .work_unit_session_active(task, Some("wu-a"), "claude-code", Some("acct-a"))
        .expect("active")
        .expect("some");
    assert_eq!(active.turns, 1);
    assert_eq!(active.approx_tokens, 500);
}

#[test]
fn work_unit_session_other_account_or_adapter_is_not_returned() {
    let store = store();
    let now = OffsetDateTime::now_utc();
    let task = TaskId::new();
    store
        .work_unit_session_create(&wu_session(task, Some("wu-a"), Some("acct-a"), "u1", now))
        .expect("create");
    // 別 account の session は返さない（account isolation、ADR-0140 D3）。
    assert_eq!(
        store
            .work_unit_session_active(task, Some("wu-a"), "claude-code", Some("acct-b"))
            .expect("active"),
        None
    );
    // account 無し（None）とも一致しない。
    assert_eq!(
        store
            .work_unit_session_active(task, Some("wu-a"), "claude-code", None)
            .expect("active"),
        None
    );
    // 別 adapter とも一致しない。
    assert_eq!(
        store
            .work_unit_session_active(task, Some("wu-a"), "codex", Some("acct-a"))
            .expect("active"),
        None
    );
    // current は判定用に adapter・account を問わず返す（dispatcher が account_changed を見て retire する）。
    let current = store
        .work_unit_session_current(task, Some("wu-a"))
        .expect("current")
        .expect("some");
    assert_eq!(current.account_id.as_deref(), Some("acct-a"));
}

#[test]
fn work_unit_session_is_not_returned_after_retire() {
    let store = store();
    let now = OffsetDateTime::now_utc();
    let task = TaskId::new();
    store
        .work_unit_session_create(&wu_session(task, Some("wu-a"), Some("acct-a"), "u1", now))
        .expect("create");
    assert!(
        store
            .work_unit_session_retire(task, Some("wu-a"), now + time::Duration::seconds(1))
            .expect("retire")
    );
    assert!(
        !store
            .work_unit_session_retire(task, Some("wu-a"), now + time::Duration::seconds(2))
            .expect("retire again")
    );
    assert_eq!(
        store
            .work_unit_session_active(task, Some("wu-a"), "claude-code", Some("acct-a"))
            .expect("active"),
        None
    );
    assert_eq!(
        store
            .work_unit_session_current(task, Some("wu-a"))
            .expect("current"),
        None
    );
    assert!(
        !store
            .work_unit_session_touch(task, Some("wu-a"), "claude-code", Some("acct-a"), 1, now)
            .expect("touch")
    );
    // retire → create で新しい session が現役になる。
    store
        .work_unit_session_create(&wu_session(
            task,
            Some("wu-a"),
            Some("acct-a"),
            "u2",
            now + time::Duration::seconds(3),
        ))
        .expect("create 2");
    assert_eq!(
        store
            .work_unit_session_active(task, Some("wu-a"), "claude-code", Some("acct-a"))
            .expect("active")
            .expect("some")
            .session_id,
        "u2"
    );
}

#[test]
fn work_unit_session_other_work_unit_or_task_is_not_returned() {
    let store = store();
    let now = OffsetDateTime::now_utc();
    let task = TaskId::new();
    let other_task = TaskId::new();
    store
        .work_unit_session_create(&wu_session(task, Some("wu-a"), Some("acct-a"), "u1", now))
        .expect("create");
    // 別 WU（独立 WU）は他 WU の session を引き継がない。
    assert_eq!(
        store
            .work_unit_session_active(task, Some("wu-b"), "claude-code", Some("acct-a"))
            .expect("active"),
        None
    );
    // atomic（work_unit_id = None）とも混ざらない。
    assert_eq!(
        store
            .work_unit_session_active(task, None, "claude-code", Some("acct-a"))
            .expect("active"),
        None
    );
    // 別 Task の同名 WU とも混ざらない。
    assert_eq!(
        store
            .work_unit_session_active(other_task, Some("wu-a"), "claude-code", Some("acct-a"))
            .expect("active"),
        None
    );
    // 別 WU の retire はこの WU の session に触れない。
    assert!(
        !store
            .work_unit_session_retire(task, Some("wu-b"), now)
            .expect("retire other")
    );
    assert!(
        store
            .work_unit_session_active(task, Some("wu-a"), "claude-code", Some("acct-a"))
            .expect("active")
            .is_some()
    );
}

#[test]
fn work_unit_session_atomic_task_and_node_sessions_are_independent() {
    let store = store();
    let now = OffsetDateTime::now_utc();
    let task = TaskId::new();
    // atomic な Task: work_unit_id = None が「Task 全体で 1 本」。
    store
        .work_unit_session_create(&wu_session(task, None, Some("acct-a"), "atomic", now))
        .expect("create");
    assert_eq!(
        store
            .work_unit_session_active(task, None, "claude-code", Some("acct-a"))
            .expect("active")
            .expect("some")
            .session_id,
        "atomic"
    );
    // 既存の Lead session（ADR-0054）は continuation 行と混ざらない。
    store
        .node_session_create(&NodeSession::new(
            "software-engineering",
            SessionKind::Lead,
            None,
            "claude-code",
            Some("acct-a".to_string()),
            "lead-sess",
            now,
        ))
        .expect("create lead");
    assert_eq!(
        store
            .node_session_active("software-engineering", SessionKind::Lead, None)
            .expect("lead")
            .expect("some")
            .session_id,
        "lead-sess"
    );
    assert!(
        store
            .work_unit_session_retire(task, None, now)
            .expect("retire")
    );
    assert!(
        store
            .node_session_active("software-engineering", SessionKind::Lead, None)
            .expect("lead")
            .is_some()
    );
}
