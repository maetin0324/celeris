use super::*;

fn node(id: &str, parent: Option<&str>, kind: OrgKind) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        id: id.to_string(),
        parent_id: parent.map(str::to_string),
        name: id.to_string(),
        kind,
        genre: None,
        brief: String::new(),
        profile: crate::profile::Profile::default(),
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

#[test]
fn a_second_secretary_is_rejected() {
    let existing = vec![node("secretary", None, OrgKind::Secretary)];
    let err =
        validate_upsert(&existing, &node("boss", None, OrgKind::Secretary)).expect_err("rejected");
    assert!(matches!(err, OrgError::DuplicateSecretary { .. }), "{err}");
    // 同じ id の更新は「1 つだけ」に反しない。
    validate_upsert(&existing, &node("secretary", None, OrgKind::Secretary))
        .expect("update is fine");
}

#[test]
fn a_department_needs_an_existing_parent_and_the_root_must_be_the_secretary() {
    let existing = vec![node("secretary", None, OrgKind::Secretary)];
    assert!(matches!(
        validate_upsert(&existing, &node("coding", None, OrgKind::Department)),
        Err(OrgError::MissingParent { .. })
    ));
    assert!(matches!(
        validate_upsert(
            &existing,
            &node("coding", Some("ghost"), OrgKind::Department)
        ),
        Err(OrgError::UnknownParent { .. })
    ));
    validate_upsert(
        &existing,
        &node("coding", Some("secretary"), OrgKind::Department),
    )
    .expect("ok");
}

#[test]
fn nesting_must_go_secretary_then_department_then_section() {
    let existing = vec![
        node("secretary", None, OrgKind::Secretary),
        node("coding", Some("secretary"), OrgKind::Department),
        node("frontend", Some("coding"), OrgKind::Section),
    ];
    assert!(matches!(
        validate_upsert(&existing, &node("sub", Some("frontend"), OrgKind::Section)),
        Err(OrgError::BadNesting { .. })
    ));
    assert!(matches!(
        validate_upsert(
            &existing,
            &node("dept", Some("frontend"), OrgKind::Department)
        ),
        Err(OrgError::BadNesting { .. })
    ));
    validate_upsert(&existing, &node("perf", Some("coding"), OrgKind::Section)).expect("ok");
}

/// 監査 D-1: `PATCH /org/{id}` で `kind` を変える更新は、既にぶら下がっている子とも整合しなければ
/// ならない。子を持つ `department` を `section` に変えようとすると「section の下に section」になり
/// 拒否される。子が無ければ通る。
#[test]
fn changing_kind_is_rejected_if_it_would_break_an_existing_childs_nesting() {
    let existing = vec![
        node("secretary", None, OrgKind::Secretary),
        node("coding", Some("secretary"), OrgKind::Department),
        node("frontend", Some("coding"), OrgKind::Section),
    ];
    // coding（部）を section に変えると、既にぶら下がる frontend（課）が「section の下の section」になる。
    assert!(matches!(
        validate_upsert(
            &existing,
            &node("coding", Some("secretary"), OrgKind::Section)
        ),
        Err(OrgError::BadNesting { .. })
    ));
    // 子が無ければ kind を変えても通る。
    let childless = vec![
        node("secretary", None, OrgKind::Secretary),
        node("infra", Some("secretary"), OrgKind::Department),
    ];
    validate_upsert(
        &childless,
        &node("infra", Some("secretary"), OrgKind::Section),
    )
    .expect("no children, ok");
}

#[test]
fn a_node_cannot_become_its_own_ancestor() {
    let existing = vec![
        node("secretary", None, OrgKind::Secretary),
        node("coding", Some("secretary"), OrgKind::Department),
        node("frontend", Some("coding"), OrgKind::Section),
    ];
    // coding の親を自分の子（frontend）にしようとする。
    assert!(matches!(
        validate_upsert(
            &existing,
            &node("coding", Some("frontend"), OrgKind::Department)
        ),
        Err(OrgError::Cycle { .. })
    ));
    // 自分自身を親にするのも循環。
    assert!(matches!(
        validate_upsert(
            &existing,
            &node("coding", Some("coding"), OrgKind::Department)
        ),
        Err(OrgError::Cycle { .. })
    ));
    // 既存データの親の連鎖が閉じていても（壊れた DB）、無限に辿らずに拒否する。
    let broken = vec![
        node("a", Some("b"), OrgKind::Department),
        node("b", Some("a"), OrgKind::Department),
    ];
    assert!(matches!(
        validate_upsert(&broken, &node("c", Some("a"), OrgKind::Section)),
        Err(OrgError::Cycle { .. })
    ));
}

#[test]
fn ids_are_lowercase_kebab_and_names_are_not_blank() {
    let existing: Vec<OrgNode> = vec![];
    assert!(matches!(
        validate_upsert(&existing, &node("Secretary", None, OrgKind::Secretary)),
        Err(OrgError::InvalidId(_))
    ));
    let mut blank = node("secretary", None, OrgKind::Secretary);
    blank.name = "   ".into();
    assert!(matches!(
        validate_upsert(&existing, &blank),
        Err(OrgError::BlankName)
    ));
}

#[test]
fn department_of_walks_up_to_the_first_department() {
    let org = vec![
        node("secretary", None, OrgKind::Secretary),
        node("coding", Some("secretary"), OrgKind::Department),
        node("coding-poc", Some("coding"), OrgKind::Section),
        node("research", Some("secretary"), OrgKind::Department),
        node("research-survey", Some("research"), OrgKind::Section),
    ];
    assert_eq!(department_of(&org, "coding-poc").as_deref(), Some("coding"));
    assert_eq!(department_of(&org, "coding").as_deref(), Some("coding"));
    assert_eq!(
        department_of(&org, "research-survey").as_deref(),
        Some("research")
    );
    assert_eq!(department_of(&org, "secretary"), None);
    assert_eq!(department_of(&org, "ghost"), None);
    // 親の連鎖が閉じた壊れたデータでも止まる。
    let broken = vec![
        node("a", Some("b"), OrgKind::Section),
        node("b", Some("a"), OrgKind::Section),
    ];
    assert_eq!(department_of(&broken, "a"), None);
}

#[test]
fn assignee_defaults_walk_the_node_genre_then_the_genre_default_role() {
    let roles = vec![RoleSpec {
        id: "implementer".into(),
        tier: Some(crate::model::Tier::Cheap),
        ..RoleSpec::default()
    }];
    let genres = vec![GenreSpec {
        id: "coding".into(),
        default_role: Some("implementer".into()),
        ..GenreSpec::default()
    }];
    let mut org = vec![node("coding-poc", Some("coding"), OrgKind::Section)];
    org[0].genre = Some("coding".into());
    let (genre, role) = assignee_defaults(&org, "coding-poc", &roles, &genres);
    assert_eq!(genre.as_deref(), Some("coding"));
    assert_eq!(role.map(|r| r.id.as_str()), Some("implementer"));
    // 知らない assignee・分野を持たないノードは何も返さない。
    assert_eq!(
        assignee_defaults(&org, "ghost", &roles, &genres),
        (None, None)
    );
    let no_genre = vec![node("infra", Some("secretary"), OrgKind::Department)];
    assert_eq!(
        assignee_defaults(&no_genre, "infra", &roles, &genres),
        (None, None)
    );
}

/// ADR-0079 D13（Phase R5a）: `is_root_task` は「案件直下という位置」だけで決まる。
fn plain_task(kind: crate::model::TaskKind) -> crate::model::Task {
    use crate::model::{
        Budget, Check, Criterion, Status, Task, TaskId, Tier, WorkerHint, WorkspaceSpec,
    };
    let now = OffsetDateTime::now_utc();
    Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Draft,
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
            max_retries: 0,
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

#[test]
fn is_root_task_requires_project_root_position() {
    // 案件が無ければ root task ではない。
    let mut t = plain_task(crate::model::TaskKind::Execute);
    assert!(!is_root_task(&t));

    // 案件直下の Execute タスクは root task。
    t.project_id = Some(ProjectId::new());
    assert!(is_root_task(&t));

    // 親を持つ（委譲・計画の子）なら root task ではない。
    let mut child = t.clone();
    child.parent_id = Some(crate::model::TaskId::new());
    assert!(!is_root_task(&child));

    // 木の子（採用で `parent_id` を持たない子も）は root task ではない。
    let mut tree_child = t.clone();
    tree_child.tree = Some(crate::tree::TreeInfo {
        root_id: crate::model::TaskId::new(),
        depth: 2,
        parent_unit: Some(crate::tree::ParentUnit {
            task_id: crate::model::TaskId::new(),
            plan_id: "p".into(),
            unit_key: "u".into(),
            stage: "s".into(),
            attempt: 1,
        }),
        base_commit: None,
    });
    assert!(!is_root_task(&tree_child));
    // 木の root 自身（`parent_unit` なし）は root task。
    let mut tree_root = t.clone();
    tree_root.tree = Some(crate::tree::TreeInfo::root(tree_root.id));
    assert!(is_root_task(&tree_root));

    // 対話（`conversation` あり）は root task ではない。
    let mut conv = t.clone();
    conv.conversation = Some(crate::message::MessageId::new());
    assert!(!is_root_task(&conv));

    // `kind = plan`（既存の分解タスク）は裏方（`support_kind` = plan）なので root task ではない。
    let mut plan = t.clone();
    plan.kind = crate::model::TaskKind::Plan;
    assert!(!is_root_task(&plan));

    // `kind = approval` / `review` は裏方なので root task ではない。
    let mut approval = t.clone();
    approval.kind = crate::model::TaskKind::Approval;
    assert!(!is_root_task(&approval));
    let mut review = t.clone();
    review.kind = crate::model::TaskKind::Review;
    assert!(!is_root_task(&review));

    // 裏方（圧縮・知識整理）の役割も root task ではない。
    let mut compaction = t.clone();
    compaction.role = Some(crate::report::COMPACTION_ROLE.to_string());
    assert!(!is_root_task(&compaction));
}
