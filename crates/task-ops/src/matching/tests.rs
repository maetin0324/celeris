#[cfg(test)]
mod tests {
    use super::*;
    use task_core::{HarnessPrefs, Profile};
    use time::OffsetDateTime;

    fn node(
        id: &str,
        parent: Option<&str>,
        allowed: &[&str],
        default: Option<&str>,
        skills: &[&str],
    ) -> OrgNode {
        let now = OffsetDateTime::now_utc();
        OrgNode {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            name: id.into(),
            kind: if parent.is_none() {
                OrgKind::Secretary
            } else {
                OrgKind::Department
            },
            genre: None,
            brief: String::new(),
            profile: Profile {
                skills: skills.iter().map(|s| s.to_string()).collect(),
                harnesses: HarnessPrefs {
                    allowed: allowed.iter().map(|s| s.to_string()).collect(),
                    default: default.map(str::to_string),
                },
                ..Profile::default()
            },
            position: 0,
            created_at: now,
            updated_at: now,
        }
    }

    fn org() -> Vec<OrgNode> {
        vec![
            node(
                "cos",
                None,
                &["conversation", "plan"],
                Some("conversation"),
                &[],
            ),
            node("engineering", Some("cos"), &["coding"], None, &["software"]),
            node(
                "software-engineering",
                Some("engineering"),
                &[],
                Some("coding"),
                &["rust", "sqlite"],
            ),
            node(
                "systems-performance",
                Some("engineering"),
                &["data-analysis"],
                None,
                &["hpc", "benchmark", "rust"],
            ),
        ]
    }

    #[test]
    fn browser_matching_requires_administrator_capability_grant() {
        let mut nodes = org();
        let browser_task = task(Some("coding"), &["browser-enabled", "rust"]);
        assert!(matches!(
            decide(&nodes, &browser_task),
            Assignment::Unroutable { .. }
        ));
        nodes[3].profile.browser = Some(task_core::BrowserCapability {
            allowed_domains: vec!["example.com".into()],
            live_view_url: None,
            allowed_actions: None,
            credential_policy_ids: vec![],
        });
        assert!(
            matches!(decide(&nodes, &browser_task), Assignment::Assigned { ref node, .. } if node == "systems-performance")
        );
        nodes[3]
            .profile
            .browser
            .as_mut()
            .unwrap()
            .allowed_domains
            .clear();
        assert!(matches!(
            decide(&nodes, &browser_task),
            Assignment::Unroutable { .. }
        ));
    }

    fn task(harness: Option<&str>, skills: &[&str]) -> Task {
        let mut t = sample_task();
        t.genre = harness.map(str::to_string);
        t.skills = skills.iter().map(|s| s.to_string()).collect();
        t
    }

    fn sample_task() -> Task {
        use task_core::{
            Budget, Status, TaskId, TaskKind, TaskMode, Tier, WorkerHint, WorkspaceSpec,
        };
        let now = OffsetDateTime::now_utc();
        Task {
            tree: None,
            paused_at: None,
            routing: None,
            id: TaskId::new(),
            parent_id: None,
            kind: TaskKind::Execute,
            title: "t".into(),
            objective: "o".into(),
            acceptance: vec![],
            inputs: vec![],
            depends_on: vec![],
            status: Status::Ready,
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
            category: Default::default(),
            skills: vec![],
            mode: TaskMode::Production,
            conversation: None,
        }
    }

    /// ADR-0046 §4-4: スコアは skill の重なり。最大スコアが勝つ。
    #[test]
    fn the_node_with_the_most_overlapping_skills_wins() {
        let org = org();
        let assignment = decide(&org, &task(Some("coding"), &["hpc", "benchmark"]));
        assert_eq!(
            assignment,
            Assignment::Assigned {
                node: "systems-performance".into(),
                score: 2,
                reason: "harness coding / skill の重なり 2 件（hpc, benchmark）".into(),
            }
        );
        // rust は両方が持つが、software-engineering の方が sqlite も持つ。
        let assignment = decide(&org, &task(Some("coding"), &["rust", "sqlite"]));
        let Assignment::Assigned { node, score, .. } = assignment else {
            panic!("assigned");
        };
        assert_eq!((node.as_str(), score), ("software-engineering", 2));
    }

    /// 同点は浅い方、さらに同点は id の辞書順。
    #[test]
    fn ties_go_to_the_shallower_node_then_to_the_lexicographically_smaller_id() {
        let org = org();
        // `rust` は engineering の子 2 つが両方持つ（スコア 1）。深さは同じなので id 順。
        let Assignment::Assigned { node, .. } = decide(&org, &task(Some("coding"), &["rust"]))
        else {
            panic!("assigned");
        };
        assert_eq!(node, "software-engineering");
        // `software` は engineering（深さ 2）だけが持つ。
        let Assignment::Assigned { node, score, .. } =
            decide(&org, &task(Some("coding"), &["software"]))
        else {
            panic!("assigned");
        };
        assert_eq!((node.as_str(), score), ("engineering", 1));
    }

    /// skills が空なら、その harness を `default` に持つノードを優先する。
    #[test]
    fn without_skills_the_default_harness_node_wins_then_the_shallowest() {
        let org = org();
        let Assignment::Assigned {
            node,
            score,
            reason,
        } = decide(&org, &task(Some("coding"), &[]))
        else {
            panic!("assigned");
        };
        assert_eq!((node.as_str(), score), ("software-engineering", 0));
        assert_eq!(reason, "harness coding を既定に持つ最も浅いノード");
        // `data-analysis` を default に持つノードは無いので、allowed を持つ最も浅いノード。
        let Assignment::Assigned { node, .. } = decide(&org, &task(Some("data-analysis"), &[]))
        else {
            panic!("assigned");
        };
        assert_eq!(node, "systems-performance");
    }

    /// 根（CoS）は候補に入らない。`harnesses.allowed` は親と和なので、根だけに書いた harness も子は実効的に
    /// 継ぐ——それでも根自身が担当に選ばれることは無い（例は組み込みでない harness `triage`。組み込みの
    /// `conversation` などはそもそも matching に掛からない）。
    #[test]
    fn the_root_is_never_a_candidate() {
        let org = vec![
            node("cos", None, &["triage"], Some("triage"), &[]),
            node("engineering", Some("cos"), &[], None, &[]),
        ];
        let Assignment::Assigned { node: assigned, .. } = decide(&org, &task(Some("triage"), &[]))
        else {
            panic!("engineering が継いでいるので Assigned のはず");
        };
        assert_eq!(assigned, "engineering", "根ではなく、継いだ子");

        // 根しか無い組織では、根だけが持つ harness は誰にも継がれず候補が無い。
        let root_only = vec![node("cos", None, &["triage"], Some("triage"), &[])];
        assert!(matches!(
            decide(&root_only, &task(Some("triage"), &[])),
            Assignment::Unroutable { .. }
        ));
    }

    /// 候補が無ければ `Unroutable`（呼び出し側が `blocked` にして人に聞く）。
    #[test]
    fn no_candidate_is_unroutable_with_a_question() {
        let org = org();
        let Assignment::Unroutable { question } =
            decide(&org, &task(Some("literature"), &["paper-writing"]))
        else {
            panic!("unroutable");
        };
        assert!(
            question.starts_with("担当が見つからない: harness literature / skills paper-writing"),
            "{question}"
        );
    }

    /// ADR-0062 B1（Phase 107）: remote な作業場所は `cluster:<id>` を持つノードだけを候補にする。
    /// `systems-performance`（`cluster:sirius` を持つ）だけが候補になり、`software-engineering`
    /// （coding を許すが `cluster:sirius` を持たない）は候補から外れる。tools が空でも例外は無い。
    #[test]
    fn remote_workspace_only_matches_nodes_with_the_cluster_tool() {
        let mut org = org();
        // systems-performance に cluster:sirius を足す。
        org.iter_mut()
            .find(|n| n.id == "systems-performance")
            .unwrap()
            .profile
            .tools = vec!["cluster:sirius".to_string()];
        // coding を許す software-engineering には cluster:sirius が無い（tools は空のまま）。
        let mut t = task(Some("coding"), &[]);
        t.workspace = task_core::WorkspaceSpec::Remote {
            cluster: "sirius".into(),
            path: std::path::PathBuf::from("~"),
            mode: None,
        };
        match decide(&org, &t) {
            Assignment::Assigned { node, .. } => assert_eq!(node, "systems-performance"),
            other => panic!("expected Assigned(systems-performance), got {other:?}"),
        }
    }

    /// クラスタを持つノードが 1 つも無ければ blocked + 質問（文言に道具名が入る）。
    #[test]
    fn remote_workspace_with_no_cluster_tool_holder_is_unroutable() {
        let org = org();
        let mut t = task(Some("coding"), &[]);
        t.workspace = task_core::WorkspaceSpec::Remote {
            cluster: "sirius".into(),
            path: std::path::PathBuf::from("~"),
            mode: None,
        };
        let Assignment::Unroutable { question } = decide(&org, &t) else {
            panic!("expected Unroutable");
        };
        assert!(question.contains("cluster:sirius"), "{question}");
    }

    /// 既に担当が居る・ハーネスが無いタスクは対象外（何もしない）。
    #[test]
    fn a_task_with_an_assignee_or_without_a_harness_is_not_applicable() {
        let org = org();
        let mut t = task(Some("coding"), &["rust"]);
        t.assignee = Some("engineering".into());
        assert_eq!(decide(&org, &t), Assignment::NotApplicable);
        assert_eq!(
            decide(&org, &task(None, &["rust"])),
            Assignment::NotApplicable
        );
        assert_eq!(
            decide(&org, &task(Some(""), &[])),
            Assignment::NotApplicable
        );
    }

    /// Phase 59 より前の組織（`org_nodes` が空。組織そのものを使っていない構成）では、genre 付きの
    /// タスクでも matching を走らせない（さもないと ADR-0041 D5 の `smoke` 煙試験のような、組織を
    /// 使わない既存のタスクが軒並み `blocked` になってしまう）。
    #[test]
    fn an_empty_org_never_runs_matching() {
        assert_eq!(
            decide(&[], &task(Some("smoke"), &[])),
            Assignment::NotApplicable
        );
        assert_eq!(
            decide(&[], &task(Some("coding"), &["rust"])),
            Assignment::NotApplicable
        );
    }

    /// 実機 2026-09-20: 組織が profile を持っていても、組み込みの裏方ハーネスは matching に掛けない
    /// （掛けると verify の煙試験が `blocked` になり、検査 6 が落ちる）。
    #[test]
    fn builtin_support_harnesses_are_never_routed_through_the_org() {
        let org = org();
        for harness in task_core::harness::BUILTIN_HARNESSES {
            assert_eq!(
                decide(&org, &task(Some(harness), &[])),
                Assignment::NotApplicable,
                "{harness}"
            );
        }
        // ふつうのハーネスは従来どおり担当が決まる。
        assert!(matches!(
            decide(&org, &task(Some("coding"), &["rust"])),
            Assignment::Assigned { .. }
        ));
    }

    /// ADR-0046 D5: 明示の `assignee` が受けられないハーネスは弾く（API は 422）。
    #[test]
    fn an_explicit_assignee_must_allow_the_harness() {
        let org = org();
        assignee_accepts(&org, "software-engineering", Some("coding")).expect("allowed");
        let err = assignee_accepts(&org, "software-engineering", Some("literature"))
            .expect_err("rejected");
        assert!(err.contains("cannot take harness"), "{err}");
        // ハーネスを書かないタスクは通す。
        assignee_accepts(&org, "software-engineering", None).expect("no harness");
        // profile を持たないノード（Phase 59 より前の組織）は従来どおり通す。
        let bare = vec![node("infra", Some("cos"), &[], None, &[])];
        assignee_accepts(&bare, "infra", Some("coding")).expect("no profile: allowed");
        // 知らないノードも通す（存在の検査は呼び出し側）。
        assignee_accepts(&org, "ghost", Some("coding")).expect("unknown node");
    }
}
