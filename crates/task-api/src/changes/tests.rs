use super::*;
use task_core::{
    Budget, Check, Criterion, Report, ReportKind, Status, TaskKind, Tier, WorkerHint, WorkspaceSpec,
};

fn task() -> Task {
    let id = TaskId::new();
    let now = OffsetDateTime::now_utc();
    Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: "ワークスペース A2".into(),
        objective: "変更の取り込みを作る".into(),
        acceptance: vec![
            Criterion {
                text: "テストが通る".into(),
                check: Check::Human,
            },
            Criterion {
                text: "`cargo test` が exit 0".into(),
                check: Check::Command {
                    cmd: "cargo test".into(),
                    expect_exit: 0,
                },
            },
        ],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Done,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from("/srv/repo"),
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
        labels: Vec::new(),
        category: task_core::TaskCategory::default(),
        conversation: None,
    }
}

/// ADR-0043 D5: PR の本文は決定的（同じタスクからは同じ文字列）。
#[test]
fn the_pull_request_body_is_deterministic_and_links_back_to_celeris() {
    let task = task();
    let body = pr_body(&task, None, Some("http://192.168.1.103:7700/"));
    assert_eq!(
        body,
        pr_body(&task, None, Some("http://192.168.1.103:7700/"))
    );
    assert!(body.contains("## 目的"), "{body}");
    assert!(body.contains("変更の取り込みを作る"), "{body}");
    assert!(body.contains("- テストが通る"), "{body}");
    assert!(
        body.contains(&format!(
            "Celeris task {id}: http://192.168.1.103:7700/tasks/{id}",
            id = task.id
        )),
        "{body}"
    );
    // `gui_base_url` が無ければリンクは付けない。
    let plain = pr_body(&task, None, None);
    assert!(
        plain.contains(&format!("Celeris task {}", task.id)),
        "{plain}"
    );
    assert!(!plain.contains("http"), "{plain}");
}

/// 報告があれば見出しと本文の先頭が入る（長い本文は切る）。
#[test]
fn the_pull_request_body_summarises_the_latest_report() {
    let task = task();
    let report = Report {
        id: task_core::ReportId::new(),
        project_id: None,
        node_id: "impl".into(),
        task_id: Some(task.id),
        kind: ReportKind::Result,
        level: 1,
        headline: "取り込みを実装した".into(),
        body: (1..=20)
            .map(|i| format!("行 {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
        sources: vec![],
        read_at: None,
        created_at: OffsetDateTime::now_utc(),
    };
    let body = pr_body(&task, Some(&report), None);
    assert!(body.contains("**取り込みを実装した**"), "{body}");
    assert!(body.contains("行 1"), "{body}");
    assert!(body.contains(&format!("行 {REPORT_BODY_LINES}")), "{body}");
    assert!(
        !body.contains(&format!("行 {}", REPORT_BODY_LINES + 1)),
        "{body}"
    );
    assert!(
        body.contains("（報告の続きは Celeris で読めます）"),
        "{body}"
    );
}
