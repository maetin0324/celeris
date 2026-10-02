use super::*;
use crate::org::OrgKind;
use time::OffsetDateTime;

fn node(id: &str, parent: Option<&str>, profile: Profile) -> OrgNode {
    let now = OffsetDateTime::now_utc();
    OrgNode {
        id: id.to_string(),
        parent_id: parent.map(str::to_string),
        name: id.to_string(),
        kind: if parent.is_none() {
            OrgKind::Secretary
        } else {
            OrgKind::Department
        },
        genre: None,
        brief: String::new(),
        profile,
        position: 0,
        created_at: now,
        updated_at: now,
    }
}

fn tree() -> Vec<OrgNode> {
    vec![
        node(
            "cos",
            None,
            Profile {
                skills: vec!["coordination".into()],
                tools: vec!["gh".into(), "docker".into()],
                policy: vec!["根の方針".into()],
                model: ModelPrefs {
                    tier: Some(Tier::Standard),
                    allowed_tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
                },
                harnesses: HarnessPrefs {
                    allowed: vec!["conversation".into(), "plan".into()],
                    default: Some("conversation".into()),
                },
                permissions: Permissions {
                    approvals: vec!["external-post".into()],
                },
                run: Some(ProfileRun::Host),
                ..Profile::default()
            },
        ),
        node(
            "engineering",
            Some("cos"),
            Profile {
                skills: vec!["software".into(), "coordination".into()],
                harnesses: HarnessPrefs {
                    allowed: vec!["coding".into()],
                    default: None,
                },
                policy: vec!["部の方針".into()],
                model: ModelPrefs {
                    tier: None,
                    allowed_tiers: vec![Tier::Standard, Tier::Cheap],
                },
                ..Profile::default()
            },
        ),
        node(
            "software-engineering",
            Some("engineering"),
            Profile {
                skills: vec!["rust".into()],
                harnesses: HarnessPrefs {
                    allowed: vec![],
                    default: Some("coding".into()),
                },
                deny_tools: vec!["docker".into()],
                tools: vec!["tavily".into()],
                policy: vec!["課の方針".into()],
                model: ModelPrefs {
                    tier: Some(Tier::Cheap),
                    allowed_tiers: vec![],
                },
                run: Some(ProfileRun::Container),
                review: ReviewPrefs {
                    escalate_on_fail: None,
                    harness: Some("reviewer".into()),
                    tier: Some(Tier::Cheap),
                },
                permissions: Permissions {
                    approvals: vec!["cluster-write".into()],
                },
                ..Profile::default()
            },
        ),
    ]
}

/// ADR-0046 §4-1: 和・子勝ち・deny 勝ち・交わり・連結。
#[test]
fn resolve_merges_lists_by_union_scalars_by_child_and_tiers_by_intersection() {
    let org = tree();
    let eff = resolve(&org, "software-engineering");
    assert_eq!(eff.node_id, "software-engineering");
    assert_eq!(
        eff.chain,
        vec!["cos", "engineering", "software-engineering"]
    );
    // 和（根→葉の順、重複は落ちる）。
    assert_eq!(eff.skills, vec!["coordination", "software", "rust"]);
    assert_eq!(
        eff.harnesses_allowed,
        vec!["conversation", "plan", "coding"]
    );
    assert_eq!(eff.approvals, vec!["external-post", "cluster-write"]);
    // deny が常に勝つ（`docker` は根で与えられていても消える）。
    assert_eq!(eff.tools, vec!["gh", "tavily"]);
    assert_eq!(eff.deny_tools, vec!["docker"]);
    // 子が勝つ。
    assert_eq!(eff.harness_default.as_deref(), Some("coding"));
    assert_eq!(eff.run, Some(ProfileRun::Container));
    assert_eq!(eff.tier, Some(Tier::Cheap));
    assert_eq!(eff.review_harness.as_deref(), Some("reviewer"));
    assert_eq!(eff.review_tier, Some(Tier::Cheap));
    // 交わり（葉の空は「制限なし」なので親の交わりがそのまま残る）。
    assert_eq!(eff.allowed_tiers, vec![Tier::Standard, Tier::Cheap]);
    // 連結（根→葉。重複も残す）。
    assert_eq!(eff.policy, vec!["根の方針", "部の方針", "課の方針"]);
}

/// ADR-0056 D3（Phase 78）: `skills_mounts` は `knowledge` と同じ和の規則（親と子の重複は落ちる）。
#[test]
fn skills_mounts_are_unioned_like_knowledge_mounts() {
    let org = vec![
        node(
            "cos",
            None,
            Profile {
                skills_mounts: vec!["writing".to_string()],
                ..Profile::default()
            },
        ),
        node(
            "engineering",
            Some("cos"),
            Profile {
                skills_mounts: vec!["rust-review".to_string(), "writing".to_string()],
                ..Profile::default()
            },
        ),
    ];
    assert_eq!(
        resolve(&org, "cos").skills_mounts,
        vec!["writing".to_string()]
    );
    assert_eq!(
        resolve(&org, "engineering").skills_mounts,
        vec!["writing".to_string(), "rust-review".to_string()]
    );
}

/// 親が空の `allowed_tiers`（制限なし）でも、子の制限はそのまま効く。
#[test]
fn an_empty_parent_allowed_tiers_means_no_restriction() {
    let org = vec![
        node("cos", None, Profile::default()),
        node(
            "engineering",
            Some("cos"),
            Profile {
                model: ModelPrefs {
                    tier: None,
                    allowed_tiers: vec![Tier::Cheap],
                },
                ..Profile::default()
            },
        ),
    ];
    assert_eq!(
        resolve(&org, "engineering").allowed_tiers,
        vec![Tier::Cheap]
    );
    assert!(resolve(&org, "cos").allowed_tiers.is_empty());
}

#[test]
fn resolve_of_an_unknown_node_is_empty_and_a_broken_chain_terminates() {
    let org = tree();
    let eff = resolve(&org, "ghost");
    assert_eq!(eff, EffectiveProfile::default());
    // 親の連鎖が閉じた壊れたデータでも止まる。
    let broken = vec![
        node("a", Some("b"), Profile::default()),
        node("b", Some("a"), Profile::default()),
    ];
    let eff = resolve(&broken, "a");
    assert!(eff.chain.len() <= broken.len() + 1, "{:?}", eff.chain);
}

#[test]
fn effective_tools_expose_the_clusters_and_the_allows_helpers() {
    let org = vec![node(
        "cluster-hpc",
        None,
        Profile {
            tools: vec![
                "cluster:pegasus".into(),
                "cluster:sirius".into(),
                "gh".into(),
            ],
            harnesses: HarnessPrefs {
                allowed: vec!["coding".into()],
                default: None,
            },
            ..Profile::default()
        },
    )];
    let eff = resolve(&org, "cluster-hpc");
    assert_eq!(eff.clusters(), vec!["pegasus", "sirius"]);
    assert!(eff.has_tool("gh"));
    assert!(!eff.has_tool("docker"));
    assert!(eff.allows_harness("coding"));
    assert!(!eff.allows_harness("literature"));
}

/// ADR-0046 D1: タスクの上書きが最後に効く。
#[test]
fn with_task_overrides_harness_tier_and_skills() {
    let org = tree();
    let eff = resolve(&org, "software-engineering");
    let mut task = crate::model::Task {
        expected_write_paths: None,
        genre: Some("literature".into()),
        skills: vec!["benchmark".into()],
        ..sample_task()
    };
    task.worker_hint.tier = Tier::Frontier;
    let out = eff.clone().with_task(&task);
    assert_eq!(out.harness_default.as_deref(), Some("literature"));
    assert_eq!(out.tier, Some(Tier::Frontier));
    assert_eq!(out.skills, vec!["benchmark"]);
    // `harnesses_allowed` は増えない（受けられるかは別に判定する）。
    assert_eq!(out.harnesses_allowed, eff.harnesses_allowed);
    // skills が空のタスクはノードの skills をそのまま残す。
    let bare = crate::model::Task {
        expected_write_paths: None,
        genre: None,
        skills: vec![],
        ..sample_task()
    };
    assert_eq!(eff.clone().with_task(&bare).skills, eff.skills);
}

#[test]
fn browser_grant_is_inherited_but_task_skills_cannot_create_it() {
    let mut org = tree();
    let grant = crate::BrowserCapability {
        allowed_domains: vec!["example.com".into()],
        live_view_url: Some("https://browser.example.com/live".into()),
        allowed_actions: None,
        credential_policy_ids: vec![],
    };
    org[1].profile.browser = Some(grant.clone());
    let effective = resolve(&org, "software-engineering");
    assert_eq!(effective.browser, Some(grant));
    let mut task = sample_task();
    task.skills = vec![crate::browser::BROWSER_SKILL.into()];
    assert_eq!(
        effective.clone().with_task(&task).browser,
        effective.browser
    );
    assert!(
        EffectiveProfile::default()
            .with_task(&task)
            .browser
            .is_none()
    );
    let invalid = Profile {
        browser: Some(crate::BrowserCapability::default()),
        ..Profile::default()
    };
    assert!(matches!(
        validate_profile(&invalid, &[]),
        Err(ProfileError::InvalidBrowser(_))
    ));
}

fn sample_task() -> crate::model::Task {
    use crate::model::{Budget, Status, TaskKind, WorkerHint, WorkspaceSpec};
    let now = OffsetDateTime::now_utc();
    crate::model::Task {
        expected_write_paths: None,
        tree: None,
        paused_at: None,
        routing: None,
        id: crate::model::TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Draft,
        priority: 10,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::local("/tmp"),
        repos: vec![],
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
        labels: vec![],
        category: crate::model::TaskCategory::Other,
        skills: vec![],
        mode: crate::model::TaskMode::Production,
        conversation: None,
    }
}

#[test]
fn validate_profile_rejects_unknown_tools_harnesses_and_skills() {
    let known = vec!["coding".to_string(), "conversation".to_string()];
    let ok = Profile {
        skills: vec!["io_uring".into(), "rust".into()],
        tools: vec!["gh".into(), "cluster:pegasus".into()],
        harnesses: HarnessPrefs {
            allowed: vec!["coding".into()],
            default: Some("coding".into()),
        },
        review: ReviewPrefs {
            escalate_on_fail: None,
            harness: Some("conversation".into()),
            tier: None,
        },
        ..Profile::default()
    };
    validate_profile(&ok, &known).expect("ok");

    let bad_tool = Profile {
        tools: vec!["kubectl".into()],
        ..Profile::default()
    };
    assert!(matches!(
        validate_profile(&bad_tool, &known),
        Err(ProfileError::UnknownTool { .. })
    ));
    // `cluster:` だけ（id が空）も駄目。
    let empty_cluster = Profile {
        tools: vec!["cluster:".into()],
        ..Profile::default()
    };
    assert!(validate_profile(&empty_cluster, &known).is_err());
    let bad_harness = Profile {
        harnesses: HarnessPrefs {
            allowed: vec!["ghost".into()],
            default: None,
        },
        ..Profile::default()
    };
    assert!(matches!(
        validate_profile(&bad_harness, &known),
        Err(ProfileError::UnknownHarness {
            field: "harnesses.allowed",
            ..
        })
    ));
    let bad_review = Profile {
        review: ReviewPrefs {
            escalate_on_fail: None,
            harness: Some("ghost".into()),
            tier: None,
        },
        ..Profile::default()
    };
    assert!(matches!(
        validate_profile(&bad_review, &known),
        Err(ProfileError::UnknownHarness {
            field: "review.harness",
            ..
        })
    ));
    let bad_skill = Profile {
        skills: vec!["IO_Uring".into()],
        ..Profile::default()
    };
    assert!(matches!(
        validate_profile(&bad_skill, &known),
        Err(ProfileError::InvalidSkill { .. })
    ));
    // `known_harnesses` が空なら harness は検査しない（最小構成）。
    validate_profile(&bad_harness, &[]).expect("no registry: skip");

    // ADR-0056 D3: `skills_mounts` は `[a-z0-9-]{1,64}`。
    let bad_skill_mount = Profile {
        skills_mounts: vec!["Rust Review".into()],
        ..Profile::default()
    };
    assert!(matches!(
        validate_profile(&bad_skill_mount, &known),
        Err(ProfileError::InvalidSkillMount { .. })
    ));
    let ok_skill_mount = Profile {
        skills_mounts: vec!["rust-review".into()],
        ..Profile::default()
    };
    validate_profile(&ok_skill_mount, &known).expect("ok skill mount");
}

/// 空の profile は JSON に何も出さない（導入前のノードと 1 バイトも変わらない）。
#[test]
fn an_empty_profile_serializes_to_an_empty_object() {
    assert!(Profile::default().is_empty());
    assert_eq!(
        serde_json::to_string(&Profile::default()).expect("json"),
        "{}"
    );
    let round: Profile = serde_json::from_str("{}").expect("parse");
    assert_eq!(round, Profile::default());
    // 知らない項目は拒否する（綴り間違いの検出）。
    assert!(serde_json::from_str::<Profile>(r#"{"skils":[]}"#).is_err());
    // `run` の enum は綴りを見る（API は 422 にする）。
    assert!(serde_json::from_str::<Profile>(r#"{"run":"bogus"}"#).is_err());
    assert!(serde_json::from_str::<Profile>(r#"{"run":"container"}"#).is_ok());
    assert!(serde_json::from_str::<Profile>(r#"{"model":{"tier":"bogus"}}"#).is_err());
}

/// ADR-0069 D2（Phase 114）: 予算の天井は最も厳しい値が勝ち、`allowed_tiers` と合わせて
/// lane の天井になる。導入前の profile（`budget` / `escalate_on_fail` 無し）はそのまま読める。
#[test]
fn budget_ceiling_takes_the_strictest_value_and_old_profiles_parse() {
    let mut nodes = tree();
    nodes[0].profile.budget = BudgetPrefs {
        max_lane: Some(Tier::Standard),
        max_attempts: Some(3),
    };
    // 子が緩めようとしても効かない
    nodes[1].profile.budget = BudgetPrefs {
        max_lane: Some(Tier::Frontier),
        max_attempts: Some(5),
    };
    nodes[1].profile.review.escalate_on_fail = Some(false);
    let eff = resolve(&nodes, "engineering");
    assert_eq!(eff.max_lane, Some(Tier::Standard));
    assert_eq!(eff.max_attempts, Some(3));
    assert_eq!(eff.review_escalate_on_fail, Some(false));
    let ceiling = eff.lane_ceiling();
    assert!(!ceiling.permits(Tier::Frontier));
    assert_eq!(ceiling.clamp(Tier::Frontier).0, Tier::Standard);
    // 子がさらに厳しくするのは効く
    nodes[1].profile.budget.max_lane = Some(Tier::Cheap);
    assert_eq!(resolve(&nodes, "engineering").max_lane, Some(Tier::Cheap));

    let old: Profile =
        serde_json::from_str(r#"{"model":{"tier":"cheap"},"review":{"tier":"cheap"}}"#).unwrap();
    assert!(old.budget.is_empty());
    assert_eq!(old.review.escalate_on_fail, None);
    // 空の budget は JSON に出ない（導入前と 1 バイトも変わらない）
    assert_eq!(
        serde_json::to_string(&old).unwrap(),
        r#"{"model":{"tier":"cheap"},"review":{"tier":"cheap"}}"#
    );
}
