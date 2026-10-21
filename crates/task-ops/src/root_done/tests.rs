use super::*;
use task_core::{
    ArtifactRef, Budget, DeliverySkipReason, ParentUnit, ProjectId, RepoId, TaskKind, Tier,
    TreeInfo, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

fn task(title: &str, status: Status, parent: Option<(&Task, &str)>) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
        paused_at: None,
        tree: parent.map(|(p, unit)| TreeInfo {
            root_id: task_core::tree::root_id_of(p),
            depth: task_core::tree::depth_of(p) + 1,
            parent_unit: Some(ParentUnit {
                task_id: p.id,
                plan_id: "p".into(),
                unit_key: unit.into(),
                stage: "s1".into(),
                attempt: 1,
            }),
            base_commit: None,
        }),
        routing: None,
        mode: Default::default(),
        skills: vec![],
        repos: vec![],
        id: TaskId::new(),
        parent_id: parent.map(|(p, _)| p.id),
        kind: TaskKind::Execute,
        title: title.into(),
        objective: "o".into(),
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 2,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: "/tmp/ws".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 30,
            max_wall_secs: 900,
            max_retries: 2,
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

fn transitioned(to: Status) -> Event {
    Event::Transitioned {
        from: Status::Reviewing,
        to,
        reason: "review_pass".into(),
    }
}

fn artifact(path: &str) -> Event {
    Event::ArtifactProduced {
        run_id: "r".into(),
        artifact: ArtifactRef {
            name: path.into(),
            path: path.into(),
            sha256: "0".into(),
            kind: "file".into(),
            declared: true,
        },
    }
}

fn delivery(task: &Task, state: DeliveryState, release: Option<&str>) -> Delivery {
    Delivery {
        task_id: task.id,
        project_id: ProjectId::new(),
        repo_id: RepoId::new(),
        repo: "/r".into(),
        branch: format!("celeris/{}", task.id),
        base: "base".into(),
        head: "abcdef0123456789".into(),
        target_sha: None,
        reviewed_sha: None,
        merge_candidate_sha: None,
        default_branch: "main".into(),
        department: "d".into(),
        review_run: "rv".into(),
        worker_run: "w".into(),
        criterion_idx: 0,
        decision: Some(true),
        state,
        detail: String::new(),
        release: release.map(Into::into),
        prepare_pid: None,
        notification: None,
        pushed_at: None,
        push_error: None,
    }
}

const PROGRESS: &str = "---\ntasks: [ID]\n---\n# 進捗\n\n## 未解決事項\n\n- なし\n\n## 提案\n\n- X を足す\n\n## 本番への移行\n\n```sh\nsystemctl --user restart celeris\n## これは見出しではない\n```\n";

#[test]
fn is_root_done_only_for_plain_root_done() {
    let root = task("root", Status::Done, None);
    assert!(is_root_done(&root));
    // done でない root
    assert!(!is_root_done(&task("r", Status::Failed, None)));
    assert!(!is_root_done(&task("r", Status::Reviewing, None)));
    // 木の子（parent_id と tree.parent_unit）
    let child = task("c", Status::Done, Some((&root, "u")));
    assert!(!is_root_done(&child));
    // parent_id だけを持つ子（委譲の子）
    let mut delegated = task("d", Status::Done, None);
    delegated.parent_id = Some(root.id);
    assert!(!is_root_done(&delegated));
    // 採用（adopt）で parent_id を持たないが木の子
    let mut adopted = task("a", Status::Done, Some((&root, "u")));
    adopted.parent_id = None;
    assert!(!is_root_done(&adopted));
    // support task（計画 run）
    let mut plan = task("p", Status::Done, None);
    plan.kind = TaskKind::Plan;
    assert!(!is_root_done(&plan));
}

#[test]
fn done_seq_is_the_last_done_transition_and_keys_differ_after_reopen() {
    let t = task("root", Status::Done, None);
    assert_eq!(done_transition_seq(&[]), None);
    let events = vec![
        (1, transitioned(Status::Running)),
        (4, transitioned(Status::Done)),
        (6, transitioned(Status::Ready)),
        (9, transitioned(Status::Done)),
        (10, artifact("a")),
    ];
    assert_eq!(done_transition_seq(&events), Some(9));
    assert_eq!(summary_key(t.id, 9), format!("{}:9", t.id));
    assert_ne!(summary_key(t.id, 4), summary_key(t.id, 9));
}

#[test]
fn collect_reads_integration_release_artifacts_and_verdicts() {
    let t = task("root の題名", Status::Done, None);
    let child = ChildFact {
        title: "子 A".into(),
        status: Status::Done,
    };
    let events = vec![
        (
            1,
            Event::PhaseIntegrated {
                phase: "design".into(),
                work_unit_id: "w".into(),
                merged: vec![],
                head: "1111111111111111".into(),
                checks: vec![],
            },
        ),
        (2, artifact("artifacts/report.md")),
        (3, artifact("artifacts/report.md")),
        (4, artifact("out/x.md")),
        (
            5,
            Event::ReviewVerdict {
                run_id: "rv".into(),
                criterion_idx: 1,
                pass: false,
                reason: "no".into(),
            },
        ),
        (
            6,
            Event::ReviewVerdict {
                run_id: "rv".into(),
                criterion_idx: 1,
                pass: true,
                reason: "ok".into(),
            },
        ),
        (
            7,
            Event::ReviewVerdict {
                run_id: "rv".into(),
                criterion_idx: 0,
                pass: true,
                reason: "ok".into(),
            },
        ),
        (
            8,
            Event::WorkerFinished {
                run_id: "w".into(),
                outcome: "done: 全部やった".into(),
                usage: None,
                role: None,
                metrics: None,
                end: None,
            },
        ),
        (9, transitioned(Status::Done)),
        (
            10,
            Event::IntegrationMerged {
                repo_id: RepoId::new(),
                branch: "b".into(),
                head_sha: "2222222222222222".into(),
                actor: "celeris".into(),
            },
        ),
    ];
    let d = delivery(&t, DeliveryState::Ready, Some("333333333333"));
    let progress = PROGRESS.replace("ID", &t.id.to_string());
    let facts = collect(RootDoneInputs {
        task: &t,
        events: &events,
        delivery: Some(&d),
        children: std::slice::from_ref(&child),
        progress_markdown: Some(&progress),
    });
    assert_eq!(facts.done_seq, Some(9));
    assert_eq!(
        facts.integration_commit.as_deref(),
        Some("2222222222222222")
    );
    assert_eq!(facts.release.as_deref(), Some("333333333333"));
    assert_eq!(facts.delivery_state, Some(DeliveryState::Ready));
    assert_eq!(facts.artifacts, vec!["artifacts/report.md", "out/x.md"]);
    assert_eq!(facts.acceptance, vec![(0, true), (1, true)]);
    assert_eq!(
        facts.phases,
        vec![PhaseFact {
            phase: "design".into(),
            head: "1111111111111111".into()
        }]
    );
    assert_eq!(facts.children, vec![child]);
    assert_eq!(facts.worker_outcome.as_deref(), Some("全部やった"));
    assert_eq!(
        facts.progress,
        Some(ProgressSections {
            migration_required: true,
            has_unresolved: false,
            has_proposal: true,
        })
    );

    let block = facts_block(&facts, Some("https://c.example/"));
    assert!(
        block.contains("root task『root の題名』が完了しました"),
        "{block}"
    );
    assert!(
        block.contains(&format!("task: https://c.example/tasks/{}", t.id)),
        "{block}"
    );
    assert!(block.contains("- design → 111111111111"), "{block}");
    assert!(block.contains("- 子 A（done）"), "{block}");
    assert!(
        block.contains("統合 commit: 222222222222（release 333333333333）"),
        "{block}"
    );
    assert!(block.contains("- artifacts/report.md"), "{block}");
    assert!(block.contains("受け入れ判定: 0:合格 1:合格"), "{block}");
    assert!(block.contains("本番への移行: あり"), "{block}");
    assert!(block.contains("提案: あり"), "{block}");
    assert!(!block.contains("未解決事項: あり"), "{block}");
    assert!(
        block.contains("（参考・worker の申告）全部やった"),
        "{block}"
    );
    // 決定的: 同じ入力から同じ文
    assert_eq!(block, facts_block(&facts, Some("https://c.example/")));
}

#[test]
fn collect_falls_back_to_ready_delivery_head_and_reports_skips() {
    let t = task("t", Status::Done, None);
    let d = delivery(&t, DeliveryState::Ready, Some("444444444444"));
    let facts = collect(RootDoneInputs {
        task: &t,
        events: &[(1, transitioned(Status::Done))],
        delivery: Some(&d),
        children: &[],
        progress_markdown: None,
    });
    assert_eq!(
        facts.integration_commit.as_deref(),
        Some("abcdef0123456789")
    );

    // 取り込み途中の delivery は統合 commit にしない
    let pending = delivery(&t, DeliveryState::Reviewing, None);
    let facts = collect(RootDoneInputs {
        task: &t,
        events: &[],
        delivery: Some(&pending),
        children: &[],
        progress_markdown: None,
    });
    assert_eq!(facts.integration_commit, None);
    assert!(facts_block(&facts, None).contains("統合 commit: 取り込み途中（delivery reviewing）"));

    let skipped = vec![(
        1,
        Event::DeliverySkipped {
            reason: DeliverySkipReason::NoProject,
            detail: " 案件なし ".into(),
            head: None,
        },
    )];
    let facts = collect(RootDoneInputs {
        task: &t,
        events: &skipped,
        delivery: None,
        children: &[],
        progress_markdown: None,
    });
    assert_eq!(
        facts.delivery_skipped.as_deref(),
        Some("no_project: 案件なし")
    );
    let block = facts_block(&facts, None);
    assert!(
        block.contains("統合 commit: 取り込みなし（no_project: 案件なし）"),
        "{block}"
    );
    assert!(block.contains("成果物: なし"), "{block}");
    assert!(block.contains("受け入れ判定: 記録なし"), "{block}");
    assert!(
        block.contains("本番への移行: 不明（進捗ファイルなし）"),
        "{block}"
    );
    assert!(!block.contains("task: "), "{block}");
}

#[test]
fn progress_sections_and_task_front_matter() {
    let id = TaskId::new();
    let text = PROGRESS.replace("ID", &id.to_string());
    assert!(progress_mentions_task(&text, id));
    assert!(!progress_mentions_task(&text, TaskId::new()));
    assert!(progress_mentions_task(
        &format!("---\ntitle: x\ntasks: [\"{}\", other]\n---\n", id),
        id
    ));
    assert!(!progress_mentions_task(&format!("tasks: [{id}]\n"), id));

    let none = "## 本番への移行\n\nなし（コードのみ）\n\n## 未解決事項\n\n- 特になし\n";
    assert_eq!(progress_sections(none), ProgressSections::default());
    let empty = "## 本番への移行\n\n## 提案\n";
    assert_eq!(progress_sections(empty), ProgressSections::default());
    let some = "## 本番への移行\n\n- なし、ただし config を書き換える\n- 再起動\n\n## 未解決事項\n\n- flaky\n";
    assert_eq!(
        progress_sections(some),
        ProgressSections {
            migration_required: true,
            has_unresolved: true,
            has_proposal: false,
        }
    );
}

#[test]
fn fallback_body_is_short_deterministic_and_needs_a_web_url() {
    let t = task("題名".repeat(100).as_str(), Status::Done, None);
    let events = vec![
        (1, transitioned(Status::Done)),
        (2, artifact("a")),
        (3, artifact("b")),
    ];
    let progress = "## 本番への移行\n\n- config を書き換える\n";
    let facts = collect(RootDoneInputs {
        task: &t,
        events: &events,
        delivery: None,
        children: &[],
        progress_markdown: Some(progress),
    });
    assert_eq!(
        fallback_body(&facts, "  ", None),
        Err("notify.web_base_url is missing or invalid")
    );
    let body = fallback_body(&facts, "https://c.example/", Some("TH1")).expect("body");
    assert_eq!(
        body,
        fallback_body(&facts, "https://c.example/", Some("TH1")).expect("body")
    );
    let lines: Vec<&str> = body.lines().collect();
    assert!(
        lines[0].starts_with("完了: 題名") && lines[0].ends_with('…'),
        "{body}"
    );
    assert_eq!(lines[1], "統合 commit: 取り込みなし");
    assert_eq!(lines[2], "成果物: 2 件");
    assert!(lines[3].starts_with("本番への移行: あり"), "{body}");
    assert_eq!(lines[5], format!("https://c.example/tasks/{}", t.id));
    assert_eq!(lines[6], "https://c.example/?thread=TH1");
    assert!(body.chars().count() < 1900, "{body}");

    let no_thread = fallback_body(&facts, "https://c.example", None).expect("body");
    assert!(!no_thread.contains("?thread="), "{no_thread}");
    assert!(no_thread.ends_with(&format!("https://c.example/tasks/{}", t.id)));
}
