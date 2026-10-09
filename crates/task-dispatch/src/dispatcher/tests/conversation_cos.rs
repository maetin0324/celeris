use super::*;

/// ADR-0033 D4: 話しかけると対話用タスクができ、run の `summary` が `role = node` の行になる
/// （`run_id` 付き）。前置きには役職と brief・記憶・直近のやり取りが載る。
/// ADR-0033 D6: 結果ファイルの `memory` が日付付きの箇条書きで追記される。
#[tokio::test]
async fn a_conversation_run_answers_in_messages_and_writes_its_memory() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    let memory_root = dir.path().join("memory");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());
    // 先週覚えたこと。
    task_worker::MemoryDir::new(&memory_root)
        .append(
            "secretary",
            None,
            &task_worker::MemoryUpdate {
                notes: vec!["人は図より表が好き".into()],
                project: vec![],
            },
            "2026-09-10",
        )
        .unwrap();

    let started = task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "先週の続きを教えて",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let seen = Arc::new(StdMutex::new(None));
    let adapter = Arc::new(PersonAdapter {
        terminal: Terminal::Done {
            summary: "3 本の候補が出ています".into(),
            evidence: vec![],
            usage: None,
        },
        seen: seen.clone(),
        memory: Some(r#"{"summary":"ok","evidence":[],"memory":{"notes":["pegasus は pjsub"]}}"#),
        proposals: Vec::new(),
    });
    let mut d = person_dispatcher(
        store.clone(),
        adapter,
        workspace_root,
        Some(memory_root.clone()),
    );
    run_until_idle(&mut d, 40).await;

    // 1. 返事が `role = node` の行になり、run_id が付く。
    let thread = store.message_list("secretary", None, 20).unwrap();
    assert_eq!(thread.len(), 2, "{thread:?}");
    assert_eq!(thread[0].role, MessageRole::User);
    assert_eq!(thread[1].role, MessageRole::Node);
    assert_eq!(thread[1].text, "3 本の候補が出ています");
    assert!(thread[1].run_id.is_some(), "返事には run_id が付く");
    assert_eq!(
        store.get(started.task.id).unwrap().unwrap().status,
        Status::Done
    );

    // 2. 前置きに役職・brief・記憶・直近のやり取りが載っている。
    let context = seen.lock().unwrap().clone().expect("the run happened");
    let node = context.node.clone().expect("node context");
    assert_eq!(node.id, "secretary");
    assert_eq!(node.brief, "secretary の担当");
    assert_eq!(
        context.memory.clone().expect("memory").notes,
        "- 2026-09-10: 人は図より表が好き\n"
    );
    // 監査 L-6: 今回の本文は `objective` に載るので、直近のやり取りには**入れない**（二重に載せない）。
    assert!(
        context.conversation.is_empty(),
        "{:?}",
        context.conversation
    );
    let preamble = task_worker::preamble::render(&context, "artifacts");
    assert!(
        preamble.contains("## あなた: secretary 課 (secretary)"),
        "{preamble}"
    );
    assert!(
        preamble.contains("- 2026-09-10: 人は図より表が好き"),
        "{preamble}"
    );
    assert!(!preamble.contains("## 直近のやり取り"), "{preamble}");

    // 3. 結果ファイルの `memory` が追記されている（古い記憶の後ろに）。
    let notes = std::fs::read_to_string(memory_root.join("secretary/notes.md")).unwrap();
    assert_eq!(notes.lines().count(), 2, "{notes}");
    assert!(
        notes.lines().next().unwrap().contains("人は図より表が好き"),
        "{notes}"
    );
    assert!(
        notes
            .lines()
            .next_back()
            .unwrap()
            .contains("pegasus は pjsub"),
        "{notes}"
    );
}

/// ADR-0033 D6: `[memory]` を設定していない構成では記憶を読まないし書かない。
#[tokio::test]
async fn without_a_memory_dir_nothing_is_read_or_written() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());
    task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "やあ",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let seen = Arc::new(StdMutex::new(None));
    let adapter = Arc::new(PersonAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        seen: seen.clone(),
        memory: Some(r#"{"summary":"ok","memory":{"notes":["覚えて"]}}"#),
        proposals: Vec::new(),
    });
    let mut d = person_dispatcher(store.clone(), adapter, workspace_root, None);
    run_until_idle(&mut d, 40).await;

    let context = seen.lock().unwrap().clone().expect("the run happened");
    assert!(context.memory.is_none(), "記憶を渡さない");
    assert!(context.node.is_some(), "役職は記憶とは別に渡る");
    assert!(!dir.path().join("memory").exists(), "書きもしない");
}

/// ADR-0033 D4: run が `Error` に終わったら「返事できませんでした: …」を返事にする。
#[tokio::test]
async fn a_failed_conversation_run_says_it_could_not_answer() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());
    task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "調子はどう",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let adapter = Arc::new(person_adapter(Terminal::Error {
        message: "harness died".into(),
        retryable: false,
    }));
    let mut d = person_dispatcher(store.clone(), adapter, workspace_root, None);
    run_until_idle(&mut d, 40).await;

    let thread = store.message_list("secretary", None, 20).unwrap();
    let reply = thread.last().expect("a reply");
    assert_eq!(reply.role, MessageRole::Node);
    assert!(
        reply.text.starts_with("返事できませんでした: "),
        "{}",
        reply.text
    );
    assert!(reply.text.contains("harness died"), "{}", reply.text);
}

/// 監査 L-6（Phase 27）: 直近のやり取りには**前回まで**が載り、今回の本文（`objective` と同じ最後の
/// `user` の行）は落ちる。
#[tokio::test]
async fn the_recent_turns_keep_the_past_but_drop_this_very_message() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());
    let first = task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "先週の続きを教えて",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    task_ops::conversation::record_reply(
        store.as_ref(),
        &first.task,
        "run-1",
        "承知しました",
        OffsetDateTime::now_utc(),
    )
    .unwrap();
    let second = task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "その後どう",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let adapter = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let d = person_dispatcher(store.clone(), adapter, workspace_root, None);
    let extras = d
        .run_extras(&second.task, None, None, "claude-code")
        .unwrap();
    let texts: Vec<&str> = extras
        .conversation
        .iter()
        .map(|t| t.text.as_str())
        .collect();
    assert_eq!(
        texts,
        vec!["先週の続きを教えて", "承知しました"],
        "{texts:?}"
    );
}

/// 監査 M-5（Phase 27）: 途中の失敗（retryable でまだ試行が残る）では返事を書かない。
/// 失敗して `Failed` に落ちたときだけ「返事できませんでした」を 1 行書く。
#[tokio::test]
async fn a_retried_conversation_run_answers_only_once() {
    struct FlakyPersonAdapter {
        failures_left: AtomicUsize,
    }
    #[async_trait]
    impl WorkerAdapter for FlakyPersonAdapter {
        fn id(&self) -> &str {
            "instant"
        }
        async fn run(
            &self,
            _req: RunRequest,
            _run_id: &str,
            _limits: RunLimits,
            _sink: &dyn EventSink,
        ) -> Result<RunOutcome, AdapterError> {
            if self.failures_left.load(Ordering::SeqCst) > 0 {
                self.failures_left.fetch_sub(1, Ordering::SeqCst);
                return Ok(RunOutcome {
                    terminal: Terminal::Error {
                        message: "harness hiccup".into(),
                        retryable: true,
                    },
                    exit_code: Some(1),
                });
            }
            Ok(RunOutcome {
                terminal: Terminal::Done {
                    summary: "順調です".into(),
                    evidence: vec![],
                    usage: None,
                },
                exit_code: Some(0),
            })
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());
    let started = task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "調子はどう",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let adapter = Arc::new(FlakyPersonAdapter {
        failures_left: AtomicUsize::new(1),
    });
    let mut d = person_dispatcher(store.clone(), adapter, workspace_root, None);
    run_until_idle(&mut d, 60).await;

    assert_eq!(
        store.get(started.task.id).unwrap().unwrap().status,
        Status::Done
    );
    let thread = store.message_list("secretary", None, 20).unwrap();
    let replies: Vec<&Message> = thread
        .iter()
        .filter(|m| m.role == MessageRole::Node)
        .collect();
    assert_eq!(replies.len(), 1, "返事は 1 行だけ: {replies:?}");
    assert_eq!(replies[0].text, "順調です");
    assert!(
        !thread
            .iter()
            .any(|m| m.text.starts_with("返事できませんでした")),
        "途中の失敗は返事にしない: {thread:?}"
    );
}

/// ADR-0033 D4 / SPEC §3.1 → ADR-0069 D1（Phase 114）: 委譲（LLM）が別の部の課を `assignee` に
/// 書いても、その担当は使わない（捨てて進行に残す）。担当を名指ししないので部をまたぐ認可の質問も
/// 起きず、子はその場で作られ、担当は matching が決める。以前はここで子を作らずに秘書へ質問し、
/// 人が `once` / `standing` で認めると次の run で通っていた（その 2 本のテストはこの挙動変更で
/// 役目を終えたので、1 本にまとめて新しい規則を確かめる。`split_delegation` 自体の単体テストは
/// `task-ops` に残る）。
#[tokio::test]
async fn a_delegation_naming_other_departments_creates_children_and_drops_the_assignees() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());
    let task = assigned_task(&workspace_root, "t1", "research-survey");
    store.create_task(&task, vec![]).unwrap();

    let adapter = Arc::new(PersonAdapter {
        terminal: Terminal::Done {
            summary: "delegated".into(),
            evidence: vec![],
            usage: None,
        },
        seen: Arc::new(StdMutex::new(None)),
        memory: None,
        proposals: vec![delegate_to("research-data"), delegate_to("coding-poc")],
    });
    let mut d = person_dispatcher(store.clone(), adapter, workspace_root, None);
    run_until_idle(&mut d, 40).await;

    let children: Vec<Task> = store
        .children(task.id)
        .unwrap()
        .into_iter()
        .filter(|c| c.kind == TaskKind::Execute)
        .collect();
    assert_eq!(children.len(), 2, "部をまたいでも止めない: {children:?}");
    assert!(
        children.iter().all(|c| c.assignee.is_none()),
        "{children:?}"
    );
    assert!(
        store
            .approval_list(Some(true), None, None)
            .unwrap()
            .is_empty(),
        "担当を名指ししないので部をまたぐ認可は起きない"
    );
    assert_ne!(store.get(task.id).unwrap().unwrap().status, Status::Blocked);
    let notes: Vec<String> = store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerProgress { msg, .. } => Some(msg),
            _ => None,
        })
        .collect();
    for dropped in ["research-data", "coding-poc"] {
        assert!(
            notes
                .iter()
                .any(|m| m.contains(&format!("担当の指定 {dropped} は使わない"))),
            "{notes:?}"
        );
    }
}

/// 同じ部の中の委譲は、これまでどおり子タスクになる（規則が効きすぎないこと）。ADR-0069 D1 以降、
/// 委譲が書いた担当は子に残らない。
#[tokio::test]
async fn a_delegation_inside_the_same_department_still_creates_children() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let task = assigned_task(&workspace_root, "t2", "research-survey");
    store.create_task(&task, vec![]).unwrap();

    let adapter = Arc::new(PersonAdapter {
        terminal: Terminal::Done {
            summary: "delegated".into(),
            evidence: vec![],
            usage: None,
        },
        seen: Arc::new(StdMutex::new(None)),
        memory: None,
        proposals: vec![delegate_to("research-data")],
    });
    let mut d = person_dispatcher(store.clone(), adapter, workspace_root, None);
    run_until_idle(&mut d, 10).await;

    // Human check の承認用の子（`kind = approval`）は数に入れない。
    let children: Vec<Task> = store
        .children(task.id)
        .unwrap()
        .into_iter()
        .filter(|c| c.kind == TaskKind::Execute)
        .collect();
    assert_eq!(children.len(), 1, "同じ部の中なら子ができる");
    // ADR-0069 D1: 委譲が書いた担当は使わない（matching が決める）。
    assert_eq!(children[0].assignee, None);
    assert_eq!(children[0].title, "任せたい仕事");
}

/// Phase 28（ADR-0033 D4 追記）: 対話 run は委譲できない。実機で秘書が返事の代わりに research-survey へ
/// 委譲し、対話タスクが `blocked` に落ちた事故の再発防止。`delegate` は子を作らず、理由が
/// `WorkerProgress` に残る。
#[tokio::test]
async fn a_conversation_run_cannot_delegate() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let started = task_ops::conversation::start(
        store.as_ref(),
        "research-survey",
        None,
        "調べて",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let adapter = Arc::new(PersonAdapter {
        terminal: Terminal::Done {
            summary: "やっておきます".into(),
            evidence: vec![],
            usage: None,
        },
        seen: Arc::new(StdMutex::new(None)),
        memory: None,
        proposals: vec![delegate_to("research-data")],
    });
    let mut d = person_dispatcher(store.clone(), adapter, workspace_root, None);
    run_until_idle(&mut d, 40).await;

    let children: Vec<Task> = store.children(started.task.id).unwrap();
    assert!(children.is_empty(), "対話 run は委譲できない: {children:?}");
    let notes: Vec<String> = store
        .events_for(started.task.id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerProgress { msg, .. } => Some(msg),
            _ => None,
        })
        .collect();
    assert!(
        notes.iter().any(|m| m.contains("対話では委譲できない")),
        "{notes:?}"
    );
    assert_eq!(
        store.get(started.task.id).unwrap().unwrap().status,
        Status::Done
    );
}

/// Phase 28（ADR-0033 D4 追記）: 対話 run は `Question` を出さない。そのまま `Done` の返事になり、
/// `approvals` の行はできない（実機で「最終試行なので自分の一般知識で答えた」まま走った事故の反省）。
#[tokio::test]
async fn a_conversation_run_turns_a_question_into_a_reply_without_an_approval() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let started = task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "この案件をお願いします",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap();

    let adapter = Arc::new(person_adapter(Terminal::Question {
        text: "予算とクラスタを教えてください".into(),
    }));
    let mut d = person_dispatcher(store.clone(), adapter, workspace_root, None);
    run_until_idle(&mut d, 40).await;

    assert_eq!(
        store.get(started.task.id).unwrap().unwrap().status,
        Status::Done,
        "Question は Done 扱い"
    );
    let thread = store.message_list("secretary", None, 20).unwrap();
    let reply = thread.last().expect("a reply");
    assert_eq!(reply.role, MessageRole::Node);
    assert_eq!(reply.text, "予算とクラスタを教えてください");
    assert!(
        store.approval_list(None, None, None).unwrap().is_empty(),
        "approvals の行はできない"
    );
}

/// Phase 28（ADR-0033 D4 追記）: 対話 run には委譲の道具（`available_genres`）を渡さない。
/// 相手が秘書かそれ以外かで `conversation_addressee` を出し分ける。通常タスクには付かない。
/// ADR-0046 D6（Phase 59 追記）: **CoS（根）の対話 run** にだけ、誰が何をできるかの組織図
/// （`organization`）を渡す（人選はしない。matching が決める）。CoS 以外の対話 run には渡さない。
#[test]
fn conversation_runs_get_no_delegation_tools_but_get_the_addressee() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let to_secretary = task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "hi",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .task;
    let to_survey = task_ops::conversation::start(
        store.as_ref(),
        "research-survey",
        None,
        "hi",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .task;

    let adapter = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let d = person_dispatcher(store.clone(), adapter, workspace_root.clone(), None);

    let extras = d
        .run_extras(&to_secretary, None, None, "claude-code")
        .unwrap();
    assert!(
        extras.available_genres.is_empty(),
        "対話 run は委譲できない: {extras:?}"
    );
    // ADR-0046 D6: CoS（根 = `secretary`。`OrgKind::Secretary`）宛ての対話には組織の一覧が付く。
    assert!(
        !extras.organization.is_empty(),
        "CoS 宛ての対話には組織の一覧が付く: {extras:?}"
    );
    assert!(
        extras
            .organization
            .iter()
            .any(|n| n.id == "research-survey")
    );
    assert_eq!(
        extras.conversation_addressee,
        Some(ConversationAddressee::Secretary)
    );

    let extras = d.run_extras(&to_survey, None, None, "claude-code").unwrap();
    assert_eq!(
        extras.conversation_addressee,
        Some(ConversationAddressee::Other)
    );
    // CoS 以外（`research-survey`）宛ての対話には組織の一覧を付けない。
    assert!(
        extras.organization.is_empty(),
        "CoS 以外の対話には付けない: {extras:?}"
    );

    // 通常タスク（対話由来でない）には付かない。
    let ordinary = assigned_task(&workspace_root, "ordinary", "research-survey");
    let extras = d.run_extras(&ordinary, None, None, "claude-code").unwrap();
    assert_eq!(extras.conversation_addressee, None);
}

/// ADR-0048 D3（Phase 60b）: CoS の対話 run にだけ、進行中の案件（`proposed` / `active`）と
/// その途中目標を渡す。`done` / `cancelled` の案件は出さない。CoS 以外の対話・通常タスクには付かない。
#[test]
fn cos_conversations_carry_active_projects_but_not_frozen_milestones() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let mut active = titled_project("進行中の案件");
    active.workspace = Some(WorkspaceSpec::Local {
        path: dir.path().join("agent-platform"),
        mode: Default::default(),
    });
    active.status = ProjectStatus::Active;
    store.project_create(&active).unwrap();
    let milestone = store
        .milestone_create(
            active.id,
            "最初の途中目標",
            "d",
            MilestoneStatus::InProgress,
        )
        .unwrap();

    let mut done = titled_project("終わった案件");
    done.status = ProjectStatus::Done;
    store.project_create(&done).unwrap();

    let to_secretary = task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "hi",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .task;
    let to_survey = task_ops::conversation::start(
        store.as_ref(),
        "research-survey",
        None,
        "hi",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .task;

    let adapter = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let d = person_dispatcher(store.clone(), adapter, workspace_root.clone(), None);

    let extras = d
        .run_extras(&to_secretary, None, None, "claude-code")
        .unwrap();
    assert_eq!(
        extras.active_projects.len(),
        1,
        "{:?}",
        extras.active_projects
    );
    let project = &extras.active_projects[0];
    assert_eq!(project.id, active.id.to_string());
    assert_eq!(project.title, "進行中の案件");
    assert_eq!(project.status, "active");
    assert_eq!(project.repos, vec!["agent-platform"]);
    // ADR-0079 D12 / D13（Phase R5a）: 凍結した途中目標は CoS に渡さない。
    assert!(project.milestones.is_empty(), "{:?}", project.milestones);
    let _ = &milestone;

    // CoS 以外の対話には渡さない。
    let extras = d.run_extras(&to_survey, None, None, "claude-code").unwrap();
    assert!(extras.active_projects.is_empty());

    // 通常タスクにも渡さない。
    let ordinary = assigned_task(&workspace_root, "ordinary", "research-survey");
    let extras = d.run_extras(&ordinary, None, None, "claude-code").unwrap();
    assert!(extras.active_projects.is_empty());
}

/// ADR-0054 D1（Phase 67）: 継続中（`resume = true`）の CoS 対話 run は、brief・記憶・組織の一覧・
/// 進行中の案件を**流し直さない**（前置きは差分だけ）。1 本目（新規セッション）はこれまでどおり全量を
/// 渡す。CoS 以外への対話にはそもそもセッションが付かないので、毎回全量のまま（対象外）。
#[test]
fn a_continuing_cos_session_drops_the_full_preamble_and_a_fresh_one_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let mut active = titled_project("進行中の案件");
    active.status = ProjectStatus::Active;
    store.project_create(&active).unwrap();

    let to_secretary = task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "hi",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .task;

    let adapter = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let d = person_dispatcher(store.clone(), adapter, workspace_root.clone(), None);

    // 1 本目: 現役セッションが無いので新規（全量。Phase 66 までと同じ振る舞い）。
    let first = d
        .run_extras(&to_secretary, None, None, "claude-code")
        .unwrap();
    let session = first
        .session
        .as_ref()
        .expect("claude-code supports continuous sessions");
    assert!(
        !session.resume,
        "the first run of a session is never a resume"
    );
    assert!(first.node.is_some(), "fresh session: brief を渡す");
    assert!(
        !first.organization.is_empty(),
        "fresh session: 組織の一覧を渡す"
    );
    assert_eq!(
        first.active_projects.len(),
        1,
        "fresh session: 進行中の案件を渡す"
    );

    // 2 本目（同じノード）: 続く（resume）。前置きは差分だけになる。
    let second = d
        .run_extras(&to_secretary, None, None, "claude-code")
        .unwrap();
    let session2 = second.session.as_ref().expect("still claude-code");
    assert!(session2.resume, "the second run resumes the same session");
    assert_eq!(session2.session_id, session.session_id);
    assert!(
        second.node.is_none(),
        "continuing session: brief を流し直さない: {:?}",
        second.node
    );
    assert!(
        second.organization.is_empty(),
        "continuing session: 組織の一覧を流し直さない: {:?}",
        second.organization
    );
    assert!(
        second.active_projects.is_empty(),
        "continuing session: 進行中の案件を流し直さない（差分に「新しい案件」が乗る）: {:?}",
        second.active_projects
    );

    // CoS 以外への対話にはセッションが付かない（`is_cos_conversation` でなければ常に `None`）ので、
    // 何度呼んでも全量のまま（対象外）。
    let to_survey = task_ops::conversation::start(
        store.as_ref(),
        "research-survey",
        None,
        "hi",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .task;
    let other = d.run_extras(&to_survey, None, None, "claude-code").unwrap();
    assert!(other.session.is_none());
    assert!(
        other.node.is_some(),
        "CoS 以外は継続の対象外なので brief は毎回渡す"
    );
}

/// Phase 43（ADR-0039 D3）: 案件が作業場所を決めていれば、その run の前置きに出す 1 行が `RunExtras` に
/// 入る。決めていない案件・案件に属さないタスク・対話 run には入らない（従来どおりのプロンプト）。
#[test]
fn run_extras_carry_the_projects_workspace_note() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let now = OffsetDateTime::now_utc();
    let mut with_workspace = titled_project("Pluvio の PoC");
    with_workspace.workspace = Some(WorkspaceSpec::Remote {
        cluster: "pegasus".into(),
        path: PathBuf::from("/work/NBB/rmaeda/workspace/rust/benchfs"),
        mode: None,
    });
    store.project_create(&with_workspace).unwrap();
    let plain = titled_project("作業場所なし");
    store.project_create(&plain).unwrap();

    let adapter = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let d = person_dispatcher(store.clone(), adapter, workspace_root.clone(), None);

    let mut task = assigned_task(&workspace_root, "poc", "research-survey");
    task.project_id = Some(with_workspace.id);
    let extras = d.run_extras(&task, None, None, "claude-code").unwrap();
    assert_eq!(
        extras.workspace_note.as_deref(),
        Some(
            "この案件のコードはクラスタ pegasus の `/work/NBB/rmaeda/workspace/rust/benchfs` にある。\
             いまのカレントディレクトリはその写しで、celeris が run の前後で同期する。"
        )
    );

    task.project_id = Some(plain.id);
    assert_eq!(
        d.run_extras(&task, None, None, "claude-code")
            .unwrap()
            .workspace_note,
        None
    );
    task.project_id = None;
    assert_eq!(
        d.run_extras(&task, None, None, "claude-code")
            .unwrap()
            .workspace_note,
        None
    );

    // 対話 run には出さない（会話は編集をしない。ADR-0039 D2）。
    let conversation = task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        Some(with_workspace.id),
        "状況を教えて",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        now,
    )
    .unwrap()
    .task;
    assert_eq!(
        d.run_extras(&conversation, None, None, "claude-code")
            .unwrap()
            .workspace_note,
        None
    );
}

/// Phase 30（ADR-0033 D4 追記）: 対話は**ノードの `genre`（仕事のハーネス）に関係なく**常に対話用分野
/// （`task.genre`）で走る。実機の事故: 関連研究調査課（`genre = web-research` = LDR）に話しかけたら
/// 検索ハーネスが会話しようとして証拠ゲートで落ちた。ただし「人」らしさは保つため、対話 run にだけ、
/// 担当ノードが自分の仕事の分野を持てば `context.work_genre` として前置きに渡す（分野を持たない
/// ノードや通常タスクには乗らない）。
#[test]
fn conversation_runs_get_the_nodes_own_work_genre_when_it_has_one() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    for n in [
        org_node_of("secretary", None, OrgKind::Secretary, Some("secretary")),
        org_node_of("research", Some("secretary"), OrgKind::Department, None),
        org_node_of(
            "research-survey",
            Some("research"),
            OrgKind::Section,
            Some("literature"),
        ),
        org_node_of("research-data", Some("research"), OrgKind::Section, None),
    ] {
        store.org_upsert(&n).unwrap();
    }

    let to_survey = task_ops::conversation::start(
        store.as_ref(),
        "research-survey",
        None,
        "なぜ web search に失敗しているのでしょうか？",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .task;
    let to_data = task_ops::conversation::start(
        store.as_ref(),
        "research-data",
        None,
        "図表の相談",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        OffsetDateTime::now_utc(),
    )
    .unwrap()
    .task;

    // 対話そのものは常に対話用分野で走る（ノードの genre = literature ではない。分野の解決は
    // `task_ops::conversation` 側のテストで見ているのでここでは genre 未指定 = `None` のまま）。
    assert_eq!(to_survey.genre, None);

    let adapter = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let mut d = person_dispatcher(store.clone(), adapter, workspace_root.clone(), None);
    d.config.genres.push(GenreSpec {
        id: "literature".into(),
        description: "関連研究の調査".into(),
        capabilities: vec!["学術文献の検索".into(), "引用グラフの探索".into()],
        default_role: Some("literature-reader".into()),
        roles: vec!["literature-reader".into()],
        ..GenreSpec::default()
    });

    let extras = d.run_extras(&to_survey, None, None, "claude-code").unwrap();
    let work_genre = extras
        .work_genre
        .expect("research-survey has its own genre");
    assert_eq!(work_genre.id, "literature");
    assert_eq!(work_genre.description, "関連研究の調査");
    assert_eq!(
        work_genre.capabilities,
        vec!["学術文献の検索".to_string(), "引用グラフの探索".to_string()]
    );

    // 分野を持たないノードには `work_genre` が乗らない。
    let extras = d.run_extras(&to_data, None, None, "claude-code").unwrap();
    assert!(extras.work_genre.is_none());

    // 通常タスク（対話由来でない）には、担当が genre を持っていても乗らない
    // （`work_genre` は対話専用。仕事の run は `task.genre` 自体がその分野になる）。
    let ordinary = assigned_task(&workspace_root, "ordinary", "research-survey");
    let extras = d.run_extras(&ordinary, None, None, "claude-code").unwrap();
    assert!(extras.work_genre.is_none());
}

/// 担当のいないタスクの質問は秘書へ回る（フォールバック。ADR-0033 D5）。
#[tokio::test]
async fn a_question_without_an_assignee_goes_to_the_secretary() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());
    let q = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&q).unwrap();
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Question {
            text: "続けますか".into(),
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);
    run_until_idle(&mut d, 40).await;

    let pending = store.approval_list(Some(true), None, None).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].node_id, "secretary");
}

/// ADR-0033 D5（Phase 26）: 全員向け + そのノード向けの永続の認可が run の前置きに載る。
#[tokio::test]
async fn standing_rules_for_everyone_and_the_assignee_reach_the_preamble() {
    let dir = tempfile::tempdir().unwrap();
    let workspace_root = dir.path().join("workspaces");
    std::fs::create_dir_all(&workspace_root).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let now = OffsetDateTime::now_utc();
    store
        .standing_rule_append(&StandingRule {
            id: StandingRuleId::new(),
            node_id: None,
            rule: "深夜は連絡しない".into(),
            created_at: now - time::Duration::minutes(2),
        })
        .unwrap();
    store
        .standing_rule_append(&StandingRule {
            id: StandingRuleId::new(),
            node_id: Some("secretary".into()),
            rule: "pegasus のジョブは 1 ノードで始めてよい".into(),
            created_at: now - time::Duration::minutes(1),
        })
        .unwrap();
    // 他ノード宛ての規則は混ざらない。
    store
        .standing_rule_append(&StandingRule {
            id: StandingRuleId::new(),
            node_id: Some("coding-poc".into()),
            rule: "coding-poc だけの規則".into(),
            created_at: now,
        })
        .unwrap();

    task_ops::conversation::start(
        store.as_ref(),
        "secretary",
        None,
        "やあ",
        &[],
        &[],
        task_core::CONVERSATION_GENRE,
        now,
    )
    .unwrap();

    let seen = Arc::new(StdMutex::new(None));
    let adapter = Arc::new(PersonAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        seen: seen.clone(),
        memory: None,
        proposals: Vec::new(),
    });
    let mut d = person_dispatcher(store.clone(), adapter, workspace_root, None);
    run_until_idle(&mut d, 40).await;

    let context = seen.lock().unwrap().clone().expect("the run happened");
    assert_eq!(
        context.standing_rules,
        vec![
            "深夜は連絡しない".to_string(),
            "pegasus のジョブは 1 ノードで始めてよい".to_string(),
        ],
        "全員向け + secretary 向けだけ（他ノード宛ては混ざらない）"
    );
    let preamble = task_worker::preamble::render(&context, "artifacts");
    assert!(preamble.contains("永続の認可"), "{preamble}");
    assert!(preamble.contains("深夜は連絡しない"), "{preamble}");
    assert!(
        preamble.contains("pegasus のジョブは 1 ノードで始めてよい"),
        "{preamble}"
    );
    assert!(!preamble.contains("coding-poc だけの規則"), "{preamble}");
}

/// Phase 33 受け入れ 1: `run_extras` は対話 run にだけ、担当の直近の仕事を
/// 「更新の新しい順・案件優先・裏方（対話・まとめ・承認・レビュー）除外・最大 10 件」で集める。
#[test]
fn run_extras_recent_work_orders_by_project_then_recency_excludes_support_and_caps_at_ten() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());
    let base = OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap();

    let project_a = titled_project("案件A");
    store.project_create(&project_a).unwrap();
    let project_b = titled_project("案件B");
    store.project_create(&project_b).unwrap();

    // 案件 A の仕事 6 件（t=100..105。新しい順は A5, A4, ..., A0）。
    let mut a_ids = Vec::new();
    for i in 0..6i64 {
        let t = work_task(
            &format!("A{i}"),
            Status::Done,
            "research-survey",
            Some(project_a.id),
            base + time::Duration::seconds(100 + i),
        );
        store.insert(&t).unwrap();
        a_ids.push(t.id);
    }
    // 案件 B の仕事 6 件（t=200..205）。
    let mut b_ids = Vec::new();
    for i in 0..6i64 {
        let t = work_task(
            &format!("B{i}"),
            Status::Done,
            "research-survey",
            Some(project_b.id),
            base + time::Duration::seconds(200 + i),
        );
        store.insert(&t).unwrap();
        b_ids.push(t.id);
    }
    // 裏方タスク（承認）は一番新しいが除外される。
    let mut support = work_task(
        "approval",
        Status::Ready,
        "research-survey",
        Some(project_a.id),
        base + time::Duration::seconds(999),
    );
    support.kind = TaskKind::Approval;
    store.insert(&support).unwrap();
    // まとめ（圧縮）役割も裏方として除外される。
    let mut compaction = work_task(
        "compaction",
        Status::Done,
        "research-survey",
        Some(project_a.id),
        base + time::Duration::seconds(998),
    );
    compaction.role = Some(task_core::COMPACTION_ROLE.to_string());
    store.insert(&compaction).unwrap();
    // 他の担当の仕事は一番新しいが除外される。
    let other_assignee = work_task(
        "other",
        Status::Done,
        "research-data",
        None,
        base + time::Duration::seconds(999),
    );
    store.insert(&other_assignee).unwrap();

    let adapter: Arc<dyn WorkerAdapter> = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let dir = tempfile::tempdir().unwrap();
    let d = person_dispatcher(store.clone(), adapter, dir.path().to_path_buf(), None);

    // 案件 A を選んでいる対話。
    let mut conv = work_task(
        "conversation",
        Status::Ready,
        "research-survey",
        Some(project_a.id),
        base + time::Duration::seconds(1000),
    );
    conv.conversation = Some(task_core::MessageId::new());
    store.insert(&conv).unwrap();

    let extras = d.run_extras(&conv, None, None, "claude-code").unwrap();
    assert_eq!(
        extras.recent_work.len(),
        10,
        "capped at 10: {:?}",
        extras.recent_work
    );
    let ids: Vec<TaskId> = extras.recent_work.iter().map(|w| w.task_id).collect();
    let mut expected_a = a_ids.clone();
    expected_a.reverse();
    let mut expected_b_top4 = b_ids.clone();
    expected_b_top4.reverse();
    expected_b_top4.truncate(4);
    let mut expected = expected_a;
    expected.extend(expected_b_top4);
    assert_eq!(ids, expected, "案件 A が先、残りは更新の新しい順");
    assert!(!ids.contains(&support.id), "承認は裏方なので除外");
    assert!(!ids.contains(&compaction.id), "まとめは裏方なので除外");
    assert!(
        !ids.contains(&other_assignee.id),
        "他の担当の仕事は含めない"
    );
}

// ---- Phase 41（ADR-0038 D1）: 途中目標レビューの対話 run ----

/// ADR-0048 D3（Phase 60b）: CoS（`secretary` = `OrgKind::Secretary`）の対話 run の結果ファイルの
/// `actions` から `create_task` が実行され、担当なし・`ready` のタスクができる。実行できなかった
/// action があれば理由が `failed` に残り、`ActionsOutcome::to_metadata` が `Some` になる。
/// CoS 以外の対話・対話でない run では何もしない。
#[test]
fn absorb_console_actions_executes_the_declared_actions_for_the_cos_only() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("console");
    std::fs::create_dir_all(ws.join("artifacts")).unwrap();
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let mut d = person_dispatcher(store.clone(), adapter, dir.path().to_path_buf(), None);
    d.config.genres.push(GenreSpec {
        id: "coding".into(),
        description: "コードを直す".into(),
        ..GenreSpec::default()
    });

    let mut task = work_task(
        "対話",
        Status::Done,
        "secretary",
        None,
        OffsetDateTime::now_utc(),
    );
    task.workspace = WorkspaceSpec::Local {
        path: ws.clone(),
        mode: None,
    };
    task.conversation = Some(task_core::MessageId::new());
    store.insert(&task).unwrap();

    // 結果ファイルが無ければ何もしない。
    assert!(d.absorb_console_actions(&task, "run-1").is_none());
    assert_eq!(store.list(None).unwrap().len(), 1, "対話タスク自身だけ");

    // 有効な action ＋ 検証に落ちる action の混在。
    std::fs::write(
        ws.join("artifacts/result.json"),
        coding_result(&store, dir.path(), r#"{"summary":"やります","actions":[
            {"type":"create_task","title":"直す","objective":"直して","acceptance":["直った"],"harness":"coding",
             "stages_hint":[{"title":"Phase 1","scope":"MVP"}]},
            {"type":"add_milestone","project":"01ZZZZZZZZZZZZZZZZZZZZZZZZ","title":"廃止された action"}
        ]}"#),
    )
    .unwrap();
    let outcome = d
        .absorb_console_actions(&task, "run-1")
        .expect("actions were declared");
    assert_eq!(outcome.executed.len(), 1);
    assert_eq!(outcome.failed.len(), 1);
    let created = outcome.executed[0].task_id.expect("task id");
    let stored = store.get(created).unwrap().expect("task");
    assert_eq!(stored.status, Status::Ready);
    assert_eq!(stored.assignee, None, "matching は別経路（次 tick）");
    // ADR-0079 D12（Phase R5a）: CoS の `create_task.stages_hint` は `Task.routing.stages_hint` に入る。
    let hints = stored
        .routing
        .as_ref()
        .expect("routing")
        .stages_hint
        .clone();
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].title, "Phase 1");
    assert_eq!(hints[0].scope, "MVP");
    assert!(outcome.to_metadata().is_some());
    let note = outcome.failure_note().expect("failure note");
    assert!(note.contains("実行できなかった action"));
    // `add_milestone` は廃止: 理由付きで落ち（人に見える）、途中目標の行は作られない。
    assert!(note.contains("add_milestone は廃止（ADR-0079）"), "{note}");
    assert!(note.contains("root task の段階"), "{note}");

    // 同じ run の 2 回目は何もしない（冪等）。
    assert!(d.absorb_console_actions(&task, "run-1").is_none());
    assert_eq!(
        store.list(None).unwrap().len(),
        2,
        "重複してタスクが増えない"
    );

    // CoS 以外（`research-survey`）宛ての対話には何もしない。
    let mut other = work_task(
        "対話",
        Status::Done,
        "research-survey",
        None,
        OffsetDateTime::now_utc(),
    );
    other.workspace = WorkspaceSpec::Local {
        path: ws.clone(),
        mode: None,
    };
    other.conversation = Some(task_core::MessageId::new());
    assert!(d.absorb_console_actions(&other, "run-2").is_none());

    // 対話でない run には何もしない。
    let mut ordinary = other.clone();
    ordinary.conversation = None;
    assert!(d.absorb_console_actions(&ordinary, "run-3").is_none());
}

/// ADR-0054 Phase 112 D3（D4(c)）: `task-worker::codex` の「result.json が無いが最終メッセージが
/// その形（`summary`+`actions`）なら回収する」救済（`final_message_is_recoverable_result`）は、
/// 回収したテキストをそのまま `<artifacts_dir>/result.json` として disk に書く（task-worker 側の
/// 単体テスト `codex::tests::phase_112_a_recoverable_final_message_is_written_as_result_json_and_its_summary_is_used`
/// で確認済み）。ここでは「書かれた後」を検査する: 書かれた `result.json` に対して
/// `absorb_console_actions` が通常どおり動き、`create_task` action が実行されてタスクが `ready` で
/// 作られることを確認する（`absorb_console_actions` は disk を読むだけで、その内容が本来モデルが
/// 書くはずだったものか、D3 の救済で回収されたものかを区別しない — それがこの救済の狙いそのもの）。
#[test]
fn absorb_console_actions_executes_actions_recovered_from_the_final_message() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("console");
    std::fs::create_dir_all(ws.join("artifacts")).unwrap();
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let mut d = person_dispatcher(store.clone(), adapter, dir.path().to_path_buf(), None);
    d.config.genres.push(GenreSpec {
        id: "coding".into(),
        description: "コードを直す".into(),
        ..GenreSpec::default()
    });

    let mut task = work_task(
        "対話",
        Status::Done,
        "secretary",
        None,
        OffsetDateTime::now_utc(),
    );
    task.workspace = WorkspaceSpec::Local {
        path: ws.clone(),
        mode: None,
    };
    task.conversation = Some(task_core::MessageId::new());
    store.insert(&task).unwrap();

    // 最終メッセージから回収された `result.json`（codex.rs が `final_message_is_recoverable_result`
    // で判定してそのまま書き込んだものと同じ形）。
    std::fs::write(
        ws.join("artifacts/result.json"),
        coding_result(&store, dir.path(), r#"{"summary":"直すタスクを作りました","actions":[
            {"type":"create_task","title":"直す","objective":"直して","acceptance":["直った"],"harness":"coding"}
        ]}"#),
    )
    .unwrap();
    let outcome = d
        .absorb_console_actions(&task, "run-1")
        .expect("actions were declared");
    assert_eq!(outcome.executed.len(), 1, "{:?}", outcome.executed);
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    let created = outcome.executed[0].task_id.expect("task id");
    let stored = store.get(created).unwrap().expect("task");
    assert_eq!(stored.status, Status::Ready);
    assert_eq!(stored.title, "直す");
}

/// ADR-0048 D3: `record_conversation_reply` は `done` な CoS の返事に actions を実行し、
/// 実行できなかった action を本文に足し、実行結果を `Message.metadata` に残す。
#[test]
fn record_conversation_reply_runs_actions_and_attaches_the_result() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("console");
    std::fs::create_dir_all(ws.join("artifacts")).unwrap();
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let mut d = person_dispatcher(store.clone(), adapter, dir.path().to_path_buf(), None);
    d.config.genres.push(GenreSpec {
        id: "coding".into(),
        description: "コードを直す".into(),
        ..GenreSpec::default()
    });

    let mut task = work_task(
        "対話",
        Status::Done,
        "secretary",
        None,
        OffsetDateTime::now_utc(),
    );
    task.workspace = WorkspaceSpec::Local {
        path: ws.clone(),
        mode: None,
    };
    task.conversation = Some(task_core::MessageId::new());
    store.insert(&task).unwrap();

    std::fs::write(
        ws.join("artifacts/result.json"),
        coding_result(&store, dir.path(), r#"{"summary":"やります","actions":[{"type":"create_task","title":"直す","objective":"直して","acceptance":["直った"],"harness":"coding"}]}"#),
    )
    .unwrap();

    d.record_conversation_reply(&task, "run-1", "done: やります", Status::Done);
    let messages = store.message_list("secretary", None, 10).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].text.starts_with("やります"));
    let metadata = messages[0].metadata.as_ref().expect("metadata");
    assert_eq!(metadata.actions_executed.len(), 1);
    assert!(metadata.actions_executed[0].summary.contains("直す"));
    assert!(metadata.actions_failed.is_empty());
}

/// Phase 33 受け入れ 1: 通常の run（対話でない）では `recent_work` は常に空
/// （担当がいても、他に仕事があっても）。
#[test]
fn run_extras_recent_work_is_empty_for_ordinary_runs() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());
    let base = OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap();
    let done = work_task("done work", Status::Done, "research-survey", None, base);
    store.insert(&done).unwrap();

    let adapter: Arc<dyn WorkerAdapter> = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let dir = tempfile::tempdir().unwrap();
    let d = person_dispatcher(store.clone(), adapter, dir.path().to_path_buf(), None);

    let ordinary = work_task(
        "ordinary",
        Status::Ready,
        "research-survey",
        None,
        base + time::Duration::seconds(1),
    );
    store.insert(&ordinary).unwrap();
    let extras = d.run_extras(&ordinary, None, None, "claude-code").unwrap();
    assert!(extras.recent_work.is_empty(), "{:?}", extras.recent_work);
}

/// Phase 33 受け入れ 2: `outcome` は Phase 25 の報告の組み立て（`task_core::report`）を流用して
/// 決定的に作る — `done` は summary の 1 行目、`failed` は直近のレビュー不合格の理由かワーカーの
/// エラー（`web search returned nothing` / `idle timeout` を含む。より後のイベントが勝つ）、
/// それも無ければ `Failed` への遷移理由、`blocked` は直近の質問。
#[test]
fn recent_work_outcome_reuses_the_report_wording_for_done_failed_and_blocked() {
    let done_events = vec![(
        0u64,
        Event::WorkerFinished {
            run_id: "r1".into(),
            outcome: "done: Pluvio と比較可能な非同期ランタイムを 3 件確認した\n詳細は成果物を参照"
                .into(),
            usage: None,
            role: None,
            metrics: None,
            end: None,
        },
    )];
    let done_task = Task {
        status: Status::Done,
        ..new_task(std::path::Path::new("/nonexistent"), Check::Human, 0)
    };
    assert_eq!(
        recent_work_outcome(&done_task, &done_events),
        Some("Pluvio と比較可能な非同期ランタイムを 3 件確認した".to_string())
    );

    let idle_timeout_events = vec![(
        0u64,
        Event::WorkerFinished {
            run_id: "r1".into(),
            outcome: "error(retryable=true): idle timeout".into(),
            usage: None,
            role: None,
            metrics: None,
            end: None,
        },
    )];
    let failed_task = Task {
        status: Status::Failed,
        ..new_task(std::path::Path::new("/nonexistent"), Check::Human, 0)
    };
    assert_eq!(
        recent_work_outcome(&failed_task, &idle_timeout_events),
        Some("idle timeout".to_string())
    );

    let search_nothing_events = vec![(
        0u64,
        Event::WorkerFinished {
            run_id: "r1".into(),
            outcome:
                "error(retryable=true): web search returned nothing (possible search path failure: \
                      expired key, CAPTCHA, or network block)"
                    .into(),
            usage: None,
            role: None,
            metrics: None,
            end: None,
        },
    )];
    assert_eq!(
        recent_work_outcome(&failed_task, &search_nothing_events),
        Some(
            "web search returned nothing (possible search path failure: expired key, CAPTCHA, or network block)"
                .to_string()
        )
    );

    // レビュー不合格は、その前のワーカーの `done` より優先される（より後のイベントだから）。
    let review_failed_events = vec![
        (
            0u64,
            Event::WorkerFinished {
                run_id: "r1".into(),
                outcome: "done: 一見よさそう".into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        ),
        (
            1u64,
            Event::ReviewVerdict {
                run_id: "r1".into(),
                criterion_idx: 0,
                pass: false,
                reason: "rejected: evidence missing".into(),
            },
        ),
    ];
    assert_eq!(
        recent_work_outcome(&failed_task, &review_failed_events),
        Some("rejected: evidence missing".to_string())
    );

    // どちらも無ければ `Failed` への遷移理由にフォールバックする。
    let fallback_events = vec![(
        0u64,
        Event::Transitioned {
            from: Status::Reviewing,
            to: Status::Failed,
            reason: "child_failed".into(),
        },
    )];
    assert_eq!(
        recent_work_outcome(&failed_task, &fallback_events),
        Some("child_failed".to_string())
    );

    let question_events = vec![(
        0u64,
        Event::QuestionRaised {
            run_id: "r1".into(),
            text: "どちらの案で進めますか？".into(),
        },
    )];
    let blocked_task = Task {
        status: Status::Blocked,
        ..new_task(std::path::Path::new("/nonexistent"), Check::Human, 0)
    };
    assert_eq!(
        recent_work_outcome(&blocked_task, &question_events),
        Some("どちらの案で進めますか？".to_string())
    );

    // 進行中のタスクには要約を出さない。
    let running_task = Task {
        status: Status::Running,
        ..new_task(std::path::Path::new("/nonexistent"), Check::Human, 0)
    };
    assert_eq!(recent_work_outcome(&running_task, &done_events), None);
}

/// Phase 33 受け入れ 3: `recent_work` の各行に案件名・成果物名・終了時刻も乗る。
#[test]
fn run_extras_recent_work_carries_project_title_and_artifacts() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    seed_conversation_org(store.as_ref());
    let base = OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap();
    let project = titled_project("Pluvio の新テーマ");
    store.project_create(&project).unwrap();

    let done = work_task(
        "先行研究のまとめ",
        Status::Done,
        "research-survey",
        Some(project.id),
        base,
    );
    store.insert(&done).unwrap();
    store
        .append_event(
            done.id,
            &Event::WorkerFinished {
                run_id: "r1".into(),
                outcome: "done: Pluvio と比較可能な非同期ランタイムを 3 件確認した".into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        )
        .unwrap();
    store
        .append_event(
            done.id,
            &Event::ArtifactProduced {
                run_id: "r1".into(),
                artifact: ArtifactRef {
                    name: "survey.md".into(),
                    path: "artifacts/survey.md".into(),
                    sha256: String::new(),
                    kind: "text".into(),
                    declared: true,
                },
            },
        )
        .unwrap();

    let adapter: Arc<dyn WorkerAdapter> = Arc::new(person_adapter(Terminal::Done {
        summary: "ok".into(),
        evidence: vec![],
        usage: None,
    }));
    let dir = tempfile::tempdir().unwrap();
    let d = person_dispatcher(store.clone(), adapter, dir.path().to_path_buf(), None);

    let mut conv = work_task(
        "conversation",
        Status::Ready,
        "research-survey",
        Some(project.id),
        base + time::Duration::seconds(1),
    );
    conv.conversation = Some(task_core::MessageId::new());
    store.insert(&conv).unwrap();

    let extras = d.run_extras(&conv, None, None, "claude-code").unwrap();
    assert_eq!(extras.recent_work.len(), 1);
    let w = &extras.recent_work[0];
    assert_eq!(w.task_id, done.id);
    assert_eq!(w.title, "先行研究のまとめ");
    assert_eq!(w.project_title.as_deref(), Some("Pluvio の新テーマ"));
    assert_eq!(w.status, Status::Done);
    assert!(w.finished_at.is_some());
    assert_eq!(
        w.outcome.as_deref(),
        Some("Pluvio と比較可能な非同期ランタイムを 3 件確認した")
    );
    assert_eq!(w.artifacts, vec!["survey.md".to_string()]);
}

fn coding_result(store: &Arc<dyn TaskStore>, dir: &std::path::Path, result: &str) -> String {
    let (project_id, _) = project_with_repos(store, &[("code", dir, task_core::RepoKind::Dir)]);
    let mut result: serde_json::Value = serde_json::from_str(result).unwrap();
    for action in result["actions"].as_array_mut().unwrap() {
        if action["type"] == "create_task" {
            action["project"] = serde_json::json!(project_id);
            action["repos"] = serde_json::json!(["code"]);
        }
    }
    result.to_string()
}
