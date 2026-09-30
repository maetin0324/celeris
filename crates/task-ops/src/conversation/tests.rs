use super::*;
use task_core::{
    CONVERSATION_GENRE, Check, Criterion, DelegateTask, OrgKind, Project, ProjectStatus,
    SqliteStore,
};

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn node(id: &str, parent: Option<&str>, kind: OrgKind, genre: Option<&str>) -> OrgNode {
    let t = now();
    OrgNode {
        profile: Default::default(),
        id: id.into(),
        parent_id: parent.map(str::to_string),
        name: format!("{id} さん"),
        kind,
        genre: genre.map(str::to_string),
        brief: format!("{id} の担当"),
        position: 0,
        created_at: t,
        updated_at: t,
    }
}

fn seed_org(store: &SqliteStore) {
    for n in [
        node("secretary", None, OrgKind::Secretary, Some("secretary")),
        node("research", Some("secretary"), OrgKind::Department, None),
        node(
            "research-survey",
            Some("research"),
            OrgKind::Section,
            Some("literature"),
        ),
        node("research-data", Some("research"), OrgKind::Section, None),
        node("coding", Some("secretary"), OrgKind::Department, None),
        node(
            "coding-poc",
            Some("coding"),
            OrgKind::Section,
            Some("coding"),
        ),
    ] {
        store.org_upsert(&n).expect("org upsert");
    }
}

fn specs() -> (Vec<RoleSpec>, Vec<GenreSpec>) {
    let roles = vec![
        RoleSpec {
            id: "secretary".into(),
            tier: Some(Tier::Standard),
            adapter: Some("claude-code".into()),
            max_turns: Some(40),
            ..RoleSpec::default()
        },
        RoleSpec {
            id: "literature-reader".into(),
            tier: Some(Tier::Cheap),
            adapter: Some("paperqa".into()),
            ..RoleSpec::default()
        },
    ];
    let genres = vec![
        GenreSpec {
            id: "secretary".into(),
            description: "人と話す".into(),
            default_role: Some("secretary".into()),
            roles: vec!["secretary".into()],
            ..GenreSpec::default()
        },
        GenreSpec {
            id: "literature".into(),
            description: "関連研究の調査".into(),
            default_role: Some("literature-reader".into()),
            roles: vec!["literature-reader".into()],
            ..GenreSpec::default()
        },
    ];
    (roles, genres)
}

#[test]
fn talking_to_a_node_records_the_message_and_makes_one_ready_conversation_task() {
    let store = SqliteStore::open_in_memory().expect("open");
    seed_org(&store);
    let (roles, genres) = specs();
    let project = Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "Pluvio".into(),
        request: "新テーマの模索".into(),
        status: ProjectStatus::Proposed,
        secretary_summary: None,
        workspace: None,
        created_at: now(),
        updated_at: now(),
    };
    store.project_create(&project).expect("project");

    let started = start(
        &store,
        "secretary",
        Some(project.id),
        "この案件をお願いします",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start");

    assert_eq!(started.message.role, MessageRole::User);
    assert_eq!(
        store
            .message_list("secretary", Some(project.id), 20)
            .expect("list")
            .len(),
        1
    );

    let task = store.get(started.task.id).expect("get").expect("task");
    assert_eq!(task.status, Status::Ready, "話しかけた時点で run できる");
    assert_eq!(task.kind, TaskKind::Execute);
    assert_eq!(task.title, "対話: この案件をお願いします");
    assert_eq!(task.objective, "この案件をお願いします");
    assert!(task.acceptance.is_empty());
    assert_eq!(task.assignee.as_deref(), Some("secretary"));
    assert_eq!(task.project_id, Some(project.id));
    assert_eq!(task.genre.as_deref(), Some("secretary"));
    assert_eq!(task.role.as_deref(), Some("secretary"));
    assert_eq!(task.worker_hint.adapter.as_deref(), Some("claude-code"));
    assert_eq!(
        task.budget.max_turns, CONVERSATION_MAX_TURNS,
        "役割の 40 ではなく対話用の予算"
    );
    assert_eq!(
        task_core::conversation_origin(&task),
        Some(started.message.id)
    );
}

/// Phase 30（ADR-0033 D4 追記）: 対話はノードの `genre`（仕事のハーネス）に関係なく、常に対話用分野
/// （`[conversation] genre`、既定 `secretary`）で走る。実機の事故: 関連研究調査課
/// （`genre = web-research` = Local Deep Research）に「なぜ web search に失敗しているのでしょうか？」
/// と話しかけたら、検索ハーネスが会話しようとして検索し、0 件 → 証拠ゲートで `failed` になった。
/// 検索ハーネスや PaperQA は会話ができない。
#[test]
fn conversation_always_uses_the_conversation_genre_regardless_of_the_nodes_own_genre() {
    let store = SqliteStore::open_in_memory().expect("open");
    seed_org(&store);
    let (roles, genres) = specs();

    // 分野を持たないノード（従来どおり対話用分野）。
    let started = start(
        &store,
        "research-data",
        None,
        "図表の体裁を相談したい",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start");
    assert_eq!(started.task.genre.as_deref(), Some(CONVERSATION_GENRE));
    assert_eq!(started.task.role.as_deref(), Some("secretary"));
    assert_eq!(started.task.project_id, None);

    // 分野を持つノード（`research-survey` = 関連研究調査課、`genre = literature`）に話しかけても、
    // その分野（実機では web-research = LDR）ではなく、常に対話用分野・役割・adapter で run する。
    let survey = start(
        &store,
        "research-survey",
        None,
        "先行研究の当て方",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start");
    assert_eq!(
        survey.task.genre.as_deref(),
        Some(CONVERSATION_GENRE),
        "ノードの genre ではなく対話用分野"
    );
    assert_eq!(survey.task.role.as_deref(), Some("secretary"));
    assert_eq!(
        survey.task.worker_hint.adapter.as_deref(),
        Some("claude-code"),
        "paperqa ではなく対話用の adapter"
    );
}

/// 完了条件: `research-survey`（`genre = web-research`）への対話タスクが `secretary` 分野の役割・adapter
/// で作られる（LDR ではない）。前置きにそのノードの `brief` と「仕事で使う道具」が入ることは
/// `task-worker::preamble` 側で検証する（ここは分野解決の決定的なロジックだけを見る）。
#[test]
fn a_web_research_node_still_talks_through_the_conversation_genre() {
    let store = SqliteStore::open_in_memory().expect("open");
    let t = now();
    store
        .org_upsert(&OrgNode {
            profile: Default::default(),
            id: "secretary".into(),
            parent_id: None,
            name: "秘書".into(),
            kind: OrgKind::Secretary,
            genre: Some("secretary".into()),
            brief: "秘書".into(),
            position: 0,
            created_at: t,
            updated_at: t,
        })
        .expect("org upsert");
    store
        .org_upsert(&OrgNode {
            profile: Default::default(),
            id: "research-survey".into(),
            parent_id: Some("secretary".into()),
            name: "関連研究調査課".into(),
            kind: OrgKind::Section,
            genre: Some("web-research".into()),
            brief: "関連研究を洗う。".into(),
            position: 0,
            created_at: t,
            updated_at: t,
        })
        .expect("org upsert");
    let (roles, mut genres) = specs();
    genres.push(GenreSpec {
        id: "web-research".into(),
        description: "web 検索で先行研究を洗う".into(),
        capabilities: vec!["web 検索".into()],
        default_role: None,
        roles: vec![],
        ..GenreSpec::default()
    });

    let started = start(
        &store,
        "research-survey",
        None,
        "なぜ web search に失敗しているのでしょうか？",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start");
    assert_eq!(
        started.task.genre.as_deref(),
        Some("secretary"),
        "web-research（LDR）ではない"
    );
    assert_eq!(started.task.role.as_deref(), Some("secretary"));
    assert_eq!(
        started.task.worker_hint.adapter.as_deref(),
        Some("claude-code")
    );
}

#[test]
fn unknown_nodes_projects_and_blank_text_are_rejected_without_writing_anything() {
    let store = SqliteStore::open_in_memory().expect("open");
    seed_org(&store);
    let (roles, genres) = specs();
    assert!(matches!(
        start(
            &store,
            "ghost",
            None,
            "hello",
            &roles,
            &genres,
            CONVERSATION_GENRE,
            now()
        ),
        Err(OpsError::Validation(_))
    ));
    assert!(matches!(
        start(
            &store,
            "secretary",
            Some(ProjectId::new()),
            "hello",
            &roles,
            &genres,
            CONVERSATION_GENRE,
            now()
        ),
        Err(OpsError::Validation(_))
    ));
    assert!(matches!(
        start(
            &store,
            "secretary",
            None,
            "   ",
            &roles,
            &genres,
            CONVERSATION_GENRE,
            now()
        ),
        Err(OpsError::Validation(_))
    ));
    assert!(
        store
            .message_list("secretary", None, 20)
            .expect("list")
            .is_empty()
    );
    assert!(
        store
            .message_list("ghost", None, 20)
            .expect("list")
            .is_empty()
    );
}

#[test]
fn the_reply_is_recorded_with_the_run_id_and_only_for_conversation_tasks() {
    let store = SqliteStore::open_in_memory().expect("open");
    seed_org(&store);
    let (roles, genres) = specs();
    let started = start(
        &store,
        "secretary",
        None,
        "状況を教えて",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start");

    let reply = record_reply(&store, &started.task, "run-1", "順調です", now())
        .expect("record")
        .expect("some");
    assert_eq!(reply.role, MessageRole::Node);
    assert_eq!(reply.run_id.as_deref(), Some("run-1"));
    let thread = store.message_list("secretary", None, 20).expect("list");
    assert_eq!(thread.len(), 2);
    assert_eq!(thread[1].text, "順調です");

    // 空の summary は「返事できませんでした」に寄せる（空行は残さない）。
    record_reply(&store, &started.task, "run-2", "  ", now()).expect("record");
    assert!(
        store.message_list("secretary", None, 20).expect("list")[2]
            .text
            .starts_with("返事できませんでした")
    );

    // 対話由来でないタスクは何も書かない。
    let mut plain = started.task.clone();
    plain.conversation = None;
    assert_eq!(
        record_reply(&store, &plain, "run-3", "x", now()).expect("record"),
        None
    );
    assert_eq!(
        store
            .message_list("secretary", None, 20)
            .expect("list")
            .len(),
        3
    );
}

fn proposal(assignee: Option<&str>, title: &str) -> DelegateTask {
    DelegateTask {
        title: title.into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        role: None,
        genre: None,
        depends_on: vec![],
        tier: None,
        assignee: assignee.map(str::to_string),
        workspace: None,
    }
}

/// ADR-0033 D4 / SPEC §3.1: 部をまたぐ委譲は秘書に聞く。同じ部の中なら聞かない。
/// Phase 27（監査 H-1 / H-2）: **バッチは分ける**（同じ部宛ての提案はその場で子にする）。
#[test]
fn a_delegation_to_another_department_becomes_a_question_and_the_batch_is_split() {
    let store = SqliteStore::open_in_memory().expect("open");
    seed_org(&store);
    let org = store.org_list().expect("org");
    let (roles, genres) = specs();
    let mut parent = start(
        &store,
        "research-survey",
        None,
        "調べて",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;

    // 同じ部（研究部）の課へ: 聞かない。
    let split = split_delegation(
        &store,
        &org,
        &parent,
        &[proposal(Some("research-data"), "整理")],
    )
    .expect("split");
    assert_eq!(split.allowed.len(), 1);
    assert!(split.pending.is_empty() && split.denied.is_empty());
    // 担当なしの提案: 従来どおり（聞かない）。
    let split = split_delegation(&store, &org, &parent, &[proposal(None, "t")]).expect("split");
    assert_eq!(split.allowed.len(), 1);
    assert!(split.pending.is_empty());

    // 部またぎと同じ部宛てが混ざったバッチ: 同じ部宛てだけ子になり、部またぎは認可待ちになる。
    let split = split_delegation(
        &store,
        &org,
        &parent,
        &[
            proposal(Some("research-data"), "整理"),
            proposal(Some("coding-poc"), "PoC を書く"),
        ],
    )
    .expect("split");
    assert_eq!(split.allowed.len(), 1, "同じ部宛ては止めない: {split:?}");
    assert_eq!(split.allowed[0].assignee.as_deref(), Some("research-data"));
    assert_eq!(split.pending.len(), 1);
    assert_eq!(
        split.pending[0],
        CrossDepartment {
            from: "research-survey".into(),
            to: "coding-poc".into(),
            title: "PoC を書く".into()
        }
    );
    assert_eq!(
        split.pending[0].key(),
        "cross-department: research-survey -> coding-poc"
    );
    assert_eq!(
        split.pending[0].question(),
        "cross-department: research-survey -> coding-poc: PoC を書く"
    );
    assert_eq!(
        cross_department_key(&split.pending[0].question()).as_deref(),
        Some(split.pending[0].key().as_str())
    );
    assert_eq!(cross_department_key("どのクラスタを使いますか"), None);

    // 委譲元が秘書（部に属さない）なら誰にでも振れる。
    parent.assignee = Some("secretary".into());
    let split = split_delegation(&store, &org, &parent, &[proposal(Some("coding-poc"), "t")])
        .expect("split");
    assert_eq!(split.allowed.len(), 1);
    // 担当を持たないタスクも従来どおり。
    parent.assignee = None;
    let split = split_delegation(&store, &org, &parent, &[proposal(Some("coding-poc"), "t")])
        .expect("split");
    assert_eq!(split.allowed.len(), 1);
}

/// Phase 27（監査 H-1）: 人が答えたら次の run で同じ委譲が通る。`once` はそのタスクだけ、
/// `standing` は以後ずっと、`denied` は通さない。照合は鍵の前方一致で決定的。
#[test]
fn once_standing_and_denied_decide_whether_the_next_run_may_delegate_across_departments() {
    use task_core::approval::{
        Approval, ApprovalId, ApprovalStore, Decision, StandingRule, StandingRuleId,
    };

    let store = SqliteStore::open_in_memory().expect("open");
    seed_org(&store);
    let org = store.org_list().expect("org");
    let (roles, genres) = specs();
    let parent = start(
        &store,
        "research-survey",
        None,
        "調べて",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    let proposals = [proposal(Some("coding-poc"), "PoC を書く")];
    let crossing = CrossDepartment {
        from: "research-survey".into(),
        to: "coding-poc".into(),
        title: "PoC を書く".into(),
    };

    // まだ聞いていない: 認可待ち。
    assert_eq!(
        split_delegation(&store, &org, &parent, &proposals)
            .expect("split")
            .pending,
        vec![crossing.clone()]
    );

    // `once`: 同じタスクの次の run では通る。
    let approval = Approval {
        id: ApprovalId::new(),
        project_id: None,
        node_id: "research-survey".into(),
        task_id: Some(parent.id),
        question: crossing.question(),
        decision: None,
        answer: None,
        created_at: now(),
        decided_at: None,
    };
    store.approval_append(&approval).expect("append");
    store
        .approval_decide(approval.id, Decision::Once, Some("認める".into()), now())
        .expect("decide");
    let split = split_delegation(&store, &org, &parent, &proposals).expect("split");
    assert_eq!(split.allowed.len(), 1, "once: 子を作る");
    assert!(split.pending.is_empty());

    // 別のタスクの同じ委譲は、`once` では通らない（今回だけ）。
    let other = start(
        &store,
        "research-survey",
        None,
        "別の件",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    assert_eq!(
        split_delegation(&store, &org, &other, &proposals)
            .expect("split")
            .pending,
        vec![crossing.clone()]
    );

    // `standing`: 鍵をそのまま規則にすると、以後どのタスクでも通る。
    store
        .standing_rule_append(&StandingRule {
            id: StandingRuleId::new(),
            node_id: Some("research-survey".into()),
            rule: crossing.key(),
            created_at: now(),
        })
        .expect("rule");
    let split = split_delegation(&store, &org, &other, &proposals).expect("split");
    assert_eq!(split.allowed.len(), 1, "standing: 以後ずっと通る");

    // `denied`: 子を作らず、もう聞かない（`answers[]` の「認めない」がワーカーに見える）。
    let store = SqliteStore::open_in_memory().expect("open");
    seed_org(&store);
    let parent = start(
        &store,
        "research-survey",
        None,
        "調べて",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    let approval = Approval {
        id: ApprovalId::new(),
        project_id: None,
        node_id: "research-survey".into(),
        task_id: Some(parent.id),
        question: crossing.question(),
        decision: None,
        answer: None,
        created_at: now(),
        decided_at: None,
    };
    store.approval_append(&approval).expect("append");
    store
        .approval_decide(
            approval.id,
            Decision::Denied,
            Some("認めない".into()),
            now(),
        )
        .expect("decide");
    let split = split_delegation(&store, &org, &parent, &proposals).expect("split");
    assert!(split.allowed.is_empty() && split.pending.is_empty());
    assert_eq!(split.denied, vec![crossing.clone()]);

    // Phase F7: `withdrawn`（認可元のタスクが終わって celeris が取り下げた）は人の決定ではない。
    // 「まだ決まっていない」（もう一度聞く）として読む。
    store
        .approval_decide(
            approval.id,
            Decision::Withdrawn,
            Some(task_core::approval::withdrawn_answer(
                task_core::Status::Cancelled,
            )),
            now(),
        )
        .expect("withdraw");
    let split = split_delegation(&store, &org, &parent, &proposals).expect("split");
    assert!(split.allowed.is_empty() && split.denied.is_empty());
    assert_eq!(split.pending, vec![crossing]);
}

/// 監査 M-3: 同じノード・同じ案件の対話は直列化する（2 通目は 1 通目が終わるまで `ready` にならない）。
#[test]
fn a_second_message_waits_for_the_first_reply() {
    let store = SqliteStore::open_in_memory().expect("open");
    seed_org(&store);
    let (roles, genres) = specs();
    let project = Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "Pluvio".into(),
        request: "r".into(),
        status: ProjectStatus::Proposed,
        secretary_summary: None,
        workspace: None,
        created_at: now(),
        updated_at: now(),
    };
    store.project_create(&project).expect("project");

    let first = start(
        &store,
        "secretary",
        Some(project.id),
        "1 通目",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    let second = start(
        &store,
        "secretary",
        Some(project.id),
        "2 通目",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    assert_eq!(second.depends_on, vec![first.id]);
    let ready: Vec<TaskId> = store
        .ready_tasks(10)
        .expect("ready")
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ready, vec![first.id], "2 通目はまだ run できない");

    // 別のノード宛て・別の案件（案件なし）は別の列（待たない）。
    let other_node = start(
        &store,
        "research-survey",
        Some(project.id),
        "別の人へ",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    assert!(other_node.depends_on.is_empty());
    let no_project = start(
        &store,
        "secretary",
        None,
        "雑談",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    assert!(no_project.depends_on.is_empty());

    // 1 通目が終われば 2 通目が run できる。
    store
        .acquire_lease(first.id, "run-1", std::time::Duration::from_secs(60))
        .expect("lease");
    store
        .apply_transition(first.id, Trigger::WorkerDone, None)
        .expect("done");
    store
        .apply_transition(first.id, Trigger::ReviewPass, None)
        .expect("pass");
    let ready: Vec<TaskId> = store
        .ready_tasks(10)
        .expect("ready")
        .iter()
        .map(|t| t.id)
        .collect();
    assert!(
        ready.contains(&second.id),
        "1 通目が done なら 2 通目が ready: {ready:?}"
    );
}

/// ADR-0054 D1/D2（Phase 68）: **CoS**（`task_core::COS_ID`）は継続セッションが全体で 1 本
/// （案件で分かれていない）ので、待ち行列も案件をまたいで直列化する。案件つきの一言のあと、
/// 案件なしの一言（`scope=all`）を打っても、CoS がまだ走っていれば 2 通目は `ready` にならない
/// （2 つの run が同じ継続セッションにぶつからない）。他のノードは従来どおり案件ごとに別の列。
#[test]
fn the_cos_serializes_across_projects_but_other_nodes_do_not() {
    let store = SqliteStore::open_in_memory().expect("open");
    store
        .org_upsert(&node("cos", None, OrgKind::Secretary, Some("secretary")))
        .expect("org upsert");
    store
        .org_upsert(&node("coding", Some("cos"), OrgKind::Department, None))
        .expect("org upsert");
    let (roles, genres) = specs();
    let project = Project {
        auto_advance: false,
        slug: None,
        archived_at: None,
        paused_from: None,
        id: ProjectId::new(),
        title: "Pluvio".into(),
        request: "r".into(),
        status: ProjectStatus::Proposed,
        secretary_summary: None,
        workspace: None,
        created_at: now(),
        updated_at: now(),
    };
    store.project_create(&project).expect("project");

    // 1 通目: 案件の文脈で CoS に話す。
    let first = start(
        &store,
        "cos",
        Some(project.id),
        "この案件どう？",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    // 2 通目: 案件なしで CoS に話す（違う `project_id` だが同じ CoS の継続セッション）。
    let second = start(
        &store,
        "cos",
        None,
        "雑談",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    assert_eq!(
        second.depends_on,
        vec![first.id],
        "CoS は案件が違っても 1 通目を待つ"
    );
    let ready: Vec<TaskId> = store
        .ready_tasks(10)
        .expect("ready")
        .iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(ready, vec![first.id], "2 通目はまだ run できない");

    // 対照: CoS 以外のノード（"coding"）は案件をまたいだら別の列（待たない）。
    let other_project_msg = start(
        &store,
        "coding",
        None,
        "別件",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    assert!(other_project_msg.depends_on.is_empty());

    // 1 通目が終われば 2 通目が run できる。
    store
        .acquire_lease(first.id, "run-1", std::time::Duration::from_secs(60))
        .expect("lease");
    store
        .apply_transition(first.id, Trigger::WorkerDone, None)
        .expect("done");
    store
        .apply_transition(first.id, Trigger::ReviewPass, None)
        .expect("pass");
    let ready: Vec<TaskId> = store
        .ready_tasks(10)
        .expect("ready")
        .iter()
        .map(|t| t.id)
        .collect();
    assert!(
        ready.contains(&second.id),
        "1 通目が done なら 2 通目が ready: {ready:?}"
    );
}

/// P-78（ADR-0033 D4 / Phase 28）: 対話タスクの `depends_on` は返事を送った順に返すための直列化だけが
/// 目的で、前の対話タスクの成否には意味が無い。前が `failed` に落ちても、次の対話タスクは
/// `cancelled`（`DependencyFailed`）にならず、`ready` になる。
#[test]
fn a_failed_conversation_task_does_not_cancel_the_next_one() {
    let store = SqliteStore::open_in_memory().expect("open");
    seed_org(&store);
    let (roles, genres) = specs();

    let first = start(
        &store,
        "secretary",
        None,
        "1 通目",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    let second = start(
        &store,
        "secretary",
        None,
        "2 通目",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start")
    .task;
    assert_eq!(second.depends_on, vec![first.id]);

    // 1 通目が失敗（非 retryable なので即 `Failed`）しても、2 通目は `cancelled` に連鎖しない。
    store
        .acquire_lease(first.id, "run-1", std::time::Duration::from_secs(60))
        .expect("lease");
    store
        .apply_transition(first.id, Trigger::WorkerError { retryable: false }, None)
        .expect("fail");
    assert_eq!(
        store.get(first.id).expect("get").expect("some").status,
        Status::Failed
    );
    assert_eq!(
        store.get(second.id).expect("get").expect("some").status,
        Status::Ready,
        "対話タスクは DependencyFailed の対象から外れる"
    );
    let ready: Vec<TaskId> = store
        .ready_tasks(10)
        .expect("ready")
        .iter()
        .map(|t| t.id)
        .collect();
    assert!(
        ready.contains(&second.id),
        "前が failed でも次は ready になる: {ready:?}"
    );
}

/// R4（migration 0007）: 1 往復の両方の行に、それを起こした対話用タスクの id が入る。
#[test]
fn both_sides_of_one_exchange_carry_the_conversation_task_id() {
    let store = SqliteStore::open_in_memory().expect("open");
    seed_org(&store);
    let (roles, genres) = specs();
    let started = start(
        &store,
        "secretary",
        None,
        "状況を教えて",
        &roles,
        &genres,
        CONVERSATION_GENRE,
        now(),
    )
    .expect("start");
    assert_eq!(started.message.task_id, Some(started.task.id));
    record_reply(&store, &started.task, "run-1", "順調です", now()).expect("record");
    let thread = store.message_list("secretary", None, 20).expect("list");
    assert!(
        thread.iter().all(|m| m.task_id == Some(started.task.id)),
        "{thread:?}"
    );
}
