use super::*;
use std::time::Duration as StdDuration;
use task_core::{
    Budget, Check, Criterion, SqliteStore, Status, Task, TaskId, TaskKind, Tier, WorkspaceSpec,
};

use crate::inbox::{ApprovalItem, InboxCounts, PlanApprovalStage, ProjectPlanRef, QuestionItem};

fn view_ctx() -> ViewContext {
    ViewContext {
        workspace_root: std::path::PathBuf::from("/tmp/workspaces"),
        retry_backoff_base: StdDuration::from_secs(10),
        retry_backoff_max: StdDuration::from_secs(300),
        max_requeues: 5,
        clusters: Default::default(),
    }
}

fn now() -> OffsetDateTime {
    OffsetDateTime::parse("2026-10-02T15:00:00Z", &Rfc3339).expect("parse now")
}

fn sample_task(kind: TaskKind, status: Status) -> Task {
    let now = now();
    Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind,
        title: "do something".to_string(),
        objective: "make it work".to_string(),
        acceptance: vec![Criterion {
            text: "tests pass".to_string(),
            check: Check::Command {
                cmd: "true".to_string(),
                expect_exit: 0,
            },
        }],
        inputs: vec![],
        depends_on: vec![],
        status,
        priority: 0,
        worker_hint: task_core::WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: "workspace".into(),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
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
    }
}

fn no_evidence(_task: &Task, _run_id: &str) -> Vec<EvidenceView> {
    Vec::new()
}

fn empty_inbox() -> Inbox {
    Inbox {
        approvals: Vec::new(),
        questions: Vec::new(),
        drafts: Vec::new(),
        attention: Vec::new(),
        suppressed: Default::default(),
        browser_waits: Vec::new(),
        decisions: Vec::new(),
        counts: InboxCounts {
            approvals: 0,
            questions: 0,
            drafts: 0,
            attention: 0,
            browser_waits: 0,
            decisions: 0,
            by_status: Default::default(),
        },
        disk_full: Vec::new(),
    }
}

const AT: &str = "2026-10-02T13:10:00Z";

fn browser_wait_item(task: &Task, reason: &str) -> crate::browser::BrowserWaitItem {
    let wait: task_core::browser_wait::BrowserWait = serde_json::from_value(serde_json::json!({
        "wait_id": "w1",
        "task_id": task.id,
        "run_id": "r1",
        "session_id": "s1",
        "reason": reason,
        "origin": "https://example.com",
        "purpose": "ログインして一覧を取る",
        "policy_revision": 1,
        "policy_hash": "h",
        "deadline": "2026-10-02T16:00:00Z",
        "resume_key": "k",
        "version": 1,
        "state": "pending",
        "created_at": AT,
    }))
    .expect("browser wait json");
    crate::browser::BrowserWaitItem {
        task: view::task_ref(task),
        run_state: wait.reason.run_state(),
        wait,
    }
}

/// D1 の受信箱側の全種類と、通知側の `requeue_limit_near` を 1 件ずつ持つ `Inbox`。
struct Fixture {
    inbox: Inbox,
    by_id: HashMap<TaskId, Task>,
    task: Task,
    approval_id: task_core::approval::ApprovalId,
}

fn fixture() -> Fixture {
    let mut task = sample_task(TaskKind::Execute, Status::Blocked);
    task.title = "api 葉".to_string();
    let draft = sample_task(TaskKind::Execute, Status::Draft);
    let approval_task = sample_task(TaskKind::Approval, Status::Ready);
    let tref = view::task_ref(&task);
    let approval_id = task_core::approval::ApprovalId::new();
    let project_id = task_core::ProjectId::new();
    let ctx = view_ctx();

    let mut inbox = empty_inbox();
    inbox.decisions.push(crate::decision::DecisionInboxItem {
        id: "01M3YG7Q2Z9X4K8N5T0A1B2C3D".to_string(),
        key: "digest-interval".to_string(),
        kind: task_core::DecisionKind::Choice,
        task_id: task.id,
        root_id: task.id,
        path: Vec::new(),
        question: "通知の要約の既定間隔をどうするか".to_string(),
        options: vec![
            task_core::DecisionOption {
                key: "1h".to_string(),
                label: "1 時間".to_string(),
                consequence: None,
            },
            task_core::DecisionOption {
                key: "6h".to_string(),
                label: "6 時間".to_string(),
                consequence: None,
            },
        ],
        recommended: "1h".to_string(),
        cost_of_reversal: task_core::CostOfReversal::Low,
        cost_note: None,
        needed_before: vec!["outbound".to_string()],
        origin: task_core::DecisionOrigin::Planner,
        created_at: AT.to_string(),
        age_secs: 0,
    });
    inbox.approvals.push(ApprovalItem {
        approval: view::task_ref(&approval_task),
        parent: Some(tref.clone()),
        criterion_text: "画面を目で見て確かめる".to_string(),
        criterion_idx: Some(0),
        attempt: Some(1),
        requested_at: AT.to_string(),
        last_run: None,
        evidence: Vec::new(),
        other_verdicts: Vec::new(),
        artifacts: Vec::new(),
        knowledge_pages: Vec::new(),
        previous_decisions: Vec::new(),
    });
    inbox.questions.push(QuestionItem {
        task: tref.clone(),
        question: "どちらの API を使うか".to_string(),
        asked_at: Some(AT.to_string()),
        run_id: None,
        previous: Vec::new(),
        approval_id: None,
    });
    let mut authz_task = sample_task(TaskKind::Execute, Status::Blocked);
    authz_task.title = "cluster への委譲".to_string();
    inbox.questions.push(QuestionItem {
        task: view::task_ref(&authz_task),
        question: "cluster-hpc へ委譲してよいか".to_string(),
        asked_at: Some(AT.to_string()),
        run_id: None,
        previous: Vec::new(),
        approval_id: Some(approval_id),
    });
    inbox.drafts.push(DraftGroup {
        parent: Some(tref.clone()),
        plan_summary: Some("子を 1 つ作る".to_string()),
        drafts: vec![view::build_task_summary(&draft, 0, 0, &ctx, now())],
        project_plan: None,
    });
    inbox.drafts.push(DraftGroup {
        parent: None,
        plan_summary: Some("途中目標 2 つ".to_string()),
        drafts: Vec::new(),
        project_plan: Some(ProjectPlanRef {
            project_id,
            version: 2,
            supersedes: Some(1),
        }),
    });
    inbox.attention = vec![
        AttentionItem::Failed {
            task: tref.clone(),
            reason: "review 不合格".to_string(),
            at: AT.to_string(),
            class: FailureClass::Infra,
            delivered_release: None,
            integration_repair: None,
        },
        AttentionItem::RequeueLimitNear {
            task: tref.clone(),
            count: 4,
            max: 5,
            at: AT.to_string(),
        },
        AttentionItem::Unroutable {
            task: tref.clone(),
            hint: task.worker_hint.clone(),
            at: AT.to_string(),
        },
        AttentionItem::ClusterUnavailable {
            cluster: "pegasus".to_string(),
            host: "pegasus.example".to_string(),
            at: AT.to_string(),
            tasks: 2,
        },
        AttentionItem::PhaseCheckpoint {
            task: tref.clone(),
            phase: "core".to_string(),
            phase_title: "中核".to_string(),
            phases_done: 1,
            phases_total: 3,
            report_idx: Some(0),
            next_phase: Some("wire".to_string()),
            at: AT.to_string(),
        },
        AttentionItem::PlanApproval {
            task: tref.clone(),
            plan_id: "p1".to_string(),
            plan_version: 2,
            reasons: vec!["decisions:1".to_string()],
            summary: "決定の要求が 1 件ある".to_string(),
            stages: vec![PlanApprovalStage {
                key: "design".to_string(),
                title: "設計".to_string(),
                review_human: false,
                units: vec!["adr: ADR（leaf）".to_string()],
            }],
            decision_ids: vec!["01M3YG7Q2Z9X4K8N5T0A1B2C3D".to_string()],
            at: AT.to_string(),
        },
        AttentionItem::DeliverySkipped {
            task: tref.clone(),
            reason: task_core::DeliverySkipReason::NoMarker,
            summary: "完了しましたが main への取り込みを開始できませんでした".to_string(),
            detail: "marker が無い".to_string(),
            head: None,
            at: AT.to_string(),
        },
        AttentionItem::IntegrationRequest {
            task: tref.clone(),
            request_id: format!("{}:aaa111:bbb222", task.id),
            request: Box::new(task_core::integration_request::IntegrationRequest {
                target_branch: "main".into(),
                target_sha: "aaa111".into(),
                source_branch: "feature".into(),
                source_sha: "bbb222".into(),
                merge_base: None,
                conflict_files: vec!["src/lib.rs".into()],
                intent: Vec::new(),
                reason: "conflict".into(),
                recommendation: "review".into(),
                actions: Vec::new(),
                candidate_sha: None,
            }),
            at: AT.to_string(),
        },
    ];
    inbox
        .browser_waits
        .push(browser_wait_item(&task, "waiting_for_auth"));
    // ADR 2026-10-07-build-tmp-hygiene D4.3: 使用率が critical の path。
    inbox.disk_full.push(task_core::DiskWatchState {
        path: "/local".to_string(),
        level: task_core::DiskLevel::Critical,
        since: now() - time::Duration::minutes(10),
        last_pct: Some(96.2),
        last_notified_at: Some(now() - time::Duration::minutes(10)),
        updated_at: now(),
    });

    let by_id: HashMap<TaskId, Task> = [task.clone(), draft, approval_task, authz_task]
        .into_iter()
        .map(|t| (t.id, t))
        .collect();
    Fixture {
        inbox,
        by_id,
        task,
        approval_id,
    }
}

fn build() -> (Fixture, HumanInbox) {
    let f = fixture();
    let out = from_inbox(&f.inbox, &f.by_id, now());
    (f, out)
}

/// D2 の共通形が埋まっている（何を決めるか・選択肢・答え方・id の形）。
fn assert_common(item: &InboxItem) {
    let prefix = item.kind.as_str();
    assert!(
        item.id.starts_with(prefix),
        "id {} must start with kind {prefix}",
        item.id
    );
    assert!(
        item.id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')),
        "id must be URL-safe: {}",
        item.id
    );
    assert!(!item.title.trim().is_empty(), "title: {item:?}");
    assert!(!item.options.is_empty(), "options: {item:?}");
    if let Some(r) = &item.recommended {
        assert!(
            item.options.iter().any(|o| &o.key == r),
            "recommended must be an option key: {item:?}"
        );
    }
    assert!(!item.blocking.summary.is_empty(), "blocking: {item:?}");
    assert_eq!(item.answer.method, "POST");
    assert_eq!(
        item.answer.path,
        format!("/api/v1/inbox/items/{}/answer", item.id)
    );
    assert!(!item.created_at.is_empty());
}

fn one(out: &HumanInbox, kind: InboxKind) -> &InboxItem {
    let found: Vec<&InboxItem> = out.items.iter().filter(|i| i.kind == kind).collect();
    assert_eq!(found.len(), 1, "exactly one {kind:?}: {:?}", out.items);
    assert_common(found[0]);
    found[0]
}

#[test]
fn human_inbox_every_inbox_kind_appears_in_common_form() {
    let (_, out) = build();
    for kind in InboxKind::ALL {
        one(&out, kind);
    }
    assert_eq!(out.counts.total as usize, out.items.len());
    assert_eq!(out.counts.total as usize, InboxKind::ALL.len());
    assert!(out.counts.by_kind.values().all(|n| *n == 1));
}

#[test]
fn human_inbox_notice_kinds_do_not_appear() {
    let (f, out) = build();
    // requeue_limit_near は通知側（D1.1）。
    assert!(route_attention(&f.inbox.attention[1]).is_none());
    assert!(
        out.items
            .iter()
            .all(|i| !i.id.starts_with("requeue_limit_near")),
        "{:?}",
        out.items
    );
    // 通知側だけの Inbox からは何も出ない。
    let mut only_notice = empty_inbox();
    only_notice.attention.push(f.inbox.attention[1].clone());
    let out = from_inbox(&only_notice, &f.by_id, now());
    assert!(out.items.is_empty(), "{:?}", out.items);
    assert_eq!(out.counts.total, 0);
}

#[test]
fn human_inbox_decision_keeps_options_recommended_and_blocked_units() {
    let (f, out) = build();
    let d = one(&out, InboxKind::Decision);
    assert_eq!(d.id, "decision-01M3YG7Q2Z9X4K8N5T0A1B2C3D");
    let keys: Vec<&str> = d.options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["1h", "6h"]);
    assert_eq!(d.recommended.as_deref(), Some("1h"));
    assert_eq!(d.blocking.units, ["outbound"]);
    assert_eq!(d.due_at.as_deref(), Some("2026-10-03T13:10:00Z"));
    assert_eq!(
        d.answer.native.as_ref().map(|n| n.path.as_str()),
        Some("/api/v1/decisions/01M3YG7Q2Z9X4K8N5T0A1B2C3D/answer")
    );
    assert_eq!(d.task.as_ref().map(|t| t.id), Some(f.task.id));
}

#[test]
fn human_inbox_plan_gate_recommends_approve_and_is_blocked_by_its_decision() {
    let (f, out) = build();
    let p = one(&out, InboxKind::PlanGate);
    assert_eq!(p.id, format!("plan_gate-{}-v2", f.task.id));
    let keys: Vec<&str> = p.options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["approve", "replan", "withdraw"]);
    assert!(p.options[1].needs_note);
    assert_eq!(p.recommended.as_deref(), Some("approve"));
    assert_eq!(p.blocked_by, ["decision-01M3YG7Q2Z9X4K8N5T0A1B2C3D"]);
    assert_eq!(p.blocking.root.as_ref().map(|r| r.id), Some(f.task.id));
}

#[test]
fn human_inbox_phase_gate_options_and_report_link() {
    let (f, out) = build();
    let p = one(&out, InboxKind::PhaseGate);
    assert_eq!(p.id, format!("phase_gate-{}-core", f.task.id));
    let keys: Vec<&str> = p.options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["continue", "replan", "withdraw"]);
    assert_eq!(
        p.links[0].href,
        format!("/api/v1/tasks/{}/artifacts/0", f.task.id)
    );
}

#[test]
fn human_inbox_authorization_absorbs_question_with_approval_id() {
    let (f, out) = build();
    let a = one(&out, InboxKind::Authorization);
    assert_eq!(a.id, format!("authorization-{}", f.approval_id));
    let keys: Vec<&str> = a.options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["once", "standing", "denied"]);
    // 同じ止まりは question として二重に出ない。
    let q = one(&out, InboxKind::Question);
    assert_ne!(q.task.as_ref().map(|t| t.id), a.task.as_ref().map(|t| t.id));
}

#[test]
fn human_inbox_browser_wait_has_deadline_and_native_credential_path() {
    let (f, out) = build();
    let b = one(&out, InboxKind::BrowserWait);
    assert_eq!(b.id, "browser_wait-w1");
    assert_eq!(b.due_at.as_deref(), Some("2026-10-02T16:00:00Z"));
    assert_eq!(
        b.answer.native.as_ref().map(|n| n.path.clone()),
        Some(format!(
            "/api/v1/tasks/{}/browser/waits/w1/credential",
            f.task.id
        ))
    );
    // 承認待ちは approve / deny。
    let mut inbox = empty_inbox();
    inbox
        .browser_waits
        .push(browser_wait_item(&f.task, "waiting_for_approval"));
    let out = from_inbox(&inbox, &f.by_id, now());
    let keys: Vec<&str> = out.items[0]
        .options
        .iter()
        .map(|o| o.key.as_str())
        .collect();
    assert_eq!(keys, ["approve", "deny"]);
}

#[test]
fn human_inbox_question_needs_note_answer() {
    let (f, out) = build();
    let q = one(&out, InboxKind::Question);
    assert_eq!(q.id, format!("question-{}", f.task.id));
    assert!(q.options[0].needs_note);
    assert_eq!(q.detail.as_deref(), Some("どちらの API を使うか"));
}

#[test]
fn human_inbox_acceptance_check_approve_or_reject() {
    let (f, out) = build();
    let a = one(&out, InboxKind::AcceptanceCheck);
    let keys: Vec<&str> = a.options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["approve", "reject"]);
    assert_eq!(a.task.as_ref().map(|t| t.id), Some(f.task.id));
    assert_eq!(a.detail.as_deref(), Some("画面を目で見て確かめる"));
}

#[test]
fn human_inbox_draft_accept_one_item_per_parent() {
    let (f, out) = build();
    let d = one(&out, InboxKind::DraftAccept);
    assert_eq!(d.id, format!("draft_accept-{}", f.task.id));
    assert_eq!(d.blocking.tasks.len(), 1);
    assert!(d.answer.native.is_some());
}

#[test]
fn human_inbox_project_plan_points_to_version() {
    let (f, out) = build();
    let p = one(&out, InboxKind::ProjectPlan);
    let pp = f.inbox.drafts[1].project_plan.as_ref().expect("plan");
    assert_eq!(p.id, format!("project_plan-{}-v2", pp.project_id));
    assert_eq!(
        p.answer.native.as_ref().map(|n| n.path.clone()),
        Some(format!(
            "/api/v1/projects/{}/project-plan/2/decide",
            pp.project_id
        ))
    );
}

#[test]
fn human_inbox_failed_recommends_retry_only_for_infra() {
    let (f, out) = build();
    let item = one(&out, InboxKind::Failed);
    assert_eq!(item.id, format!("failed-{}-20261002131000", f.task.id));
    let keys: Vec<&str> = item.options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["retry", "reopen", "cancel"]);
    assert_eq!(item.recommended.as_deref(), Some("retry"));

    let mut inbox = empty_inbox();
    inbox.attention.push(AttentionItem::Failed {
        task: view::task_ref(&f.task),
        reason: "review 不合格".to_string(),
        at: AT.to_string(),
        class: FailureClass::Work,
        delivered_release: None,
        integration_repair: None,
    });
    let out = from_inbox(&inbox, &f.by_id, now());
    assert_eq!(out.items[0].recommended, None);
}

#[test]
fn human_inbox_unroutable_reassign_or_cancel() {
    let (_, out) = build();
    let u = one(&out, InboxKind::Unroutable);
    let keys: Vec<&str> = u.options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["reassign", "cancel"]);
}

#[test]
fn human_inbox_cluster_login_names_cluster_and_scope() {
    let (_, out) = build();
    let c = one(&out, InboxKind::ClusterLogin);
    assert_eq!(c.id, "cluster_login-pegasus");
    assert!(c.blocking.summary.contains("2 件"));
}

#[test]
fn human_inbox_delivery_skipped_assign_or_skip() {
    let (_, out) = build();
    let d = one(&out, InboxKind::DeliverySkipped);
    let keys: Vec<&str> = d.options.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(keys, ["assign", "skip"]);
    assert!(d.detail.as_deref().is_some_and(|s| s.contains("marker")));
}

#[test]
fn human_inbox_integration_request_preserves_decision_material() {
    let task = sample_task(TaskKind::Execute, Status::Done);
    let request = task_core::integration_request::IntegrationRequest {
        target_branch: "main".into(),
        target_sha: "aaa111".into(),
        source_branch: "feature".into(),
        source_sha: "bbb222".into(),
        merge_base: Some("base".into()),
        conflict_files: vec!["src/lib.rs".into()],
        intent: vec![task_core::integration_request::FileIntent {
            path: "src/lib.rs".into(),
            target: task_core::integration_request::SideIntent {
                branch: "main".into(),
                path: "src/lib.rs".into(),
                commits: vec![task_core::integration_request::CommitIntent {
                    sha: "aaa111".into(),
                    subject: "keep target behavior".into(),
                }],
                diffstat: None,
                unavailable: None,
            },
            source: task_core::integration_request::SideIntent {
                branch: "feature".into(),
                path: "src/lib.rs".into(),
                commits: vec![task_core::integration_request::CommitIntent {
                    sha: "bbb222".into(),
                    subject: "add source behavior".into(),
                }],
                diffstat: None,
                unavailable: None,
            },
        }],
        reason: "content conflict".into(),
        recommendation: "combine both".into(),
        actions: Vec::new(),
        candidate_sha: Some("ccc333".into()),
    };
    let mut inbox = empty_inbox();
    inbox.attention.push(AttentionItem::IntegrationRequest {
        task: view::task_ref(&task),
        request_id: request.id_for(task.id),
        request: Box::new(request),
        at: AT.into(),
    });
    let by_id = [(task.id, task.clone())].into();
    let out = from_inbox(&inbox, &by_id, now());
    let item = one(&out, InboxKind::IntegrationRequest);
    assert_eq!(
        item.id,
        format!("integration_request-{}-aaa111-bbb222", task.id)
    );
    assert_eq!(
        item.options
            .iter()
            .map(|o| o.key.as_str())
            .collect::<Vec<_>>(),
        ["integrated", "declined", "retry"]
    );
    let detail = item.detail.as_deref().unwrap();
    for expected in [
        "main",
        "feature",
        "aaa111",
        "bbb222",
        "src/lib.rs",
        "keep target behavior",
        "add source behavior",
        "combine both",
        "ccc333",
    ] {
        assert!(detail.contains(expected), "missing {expected}");
    }
    assert_eq!(
        item.answer.path,
        format!("/api/v1/inbox/items/{}/answer", item.id)
    );
}

#[test]
fn human_inbox_never_has_knowledge_review() {
    // 人の決定 2026-10-08: KB の取り込み待ちは受信箱に出さない（種類ごと無い）。
    assert!(
        InboxKind::ALL
            .iter()
            .all(|k| k.as_str() != "knowledge_review")
    );
    let (_, out) = build();
    assert!(
        out.items
            .iter()
            .all(|i| !i.id.starts_with("knowledge_review")
                && i.kind.as_str() != "knowledge_review"),
        "{:?}",
        out.items
    );
}

#[test]
fn human_inbox_order_is_due_then_kind_then_created() {
    let (_, out) = build();
    // 期限のある decision（翌日）・browser_wait（今日 16:00）が先。近い順。
    assert_eq!(out.items[0].kind, InboxKind::BrowserWait);
    assert_eq!(out.items[1].kind, InboxKind::Decision);
    let rest: Vec<InboxKind> = out.items[2..].iter().map(|i| i.kind).collect();
    let mut sorted = rest.clone();
    sorted.sort();
    assert_eq!(rest, sorted);
}

#[test]
fn human_inbox_answered_items_disappear() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let ctx = view_ctx();

    // 下書き: 受け入れると消える。
    let draft = sample_task(TaskKind::Execute, Status::Draft);
    store.insert(&draft).expect("insert draft");
    // 質問: 答えると消える。
    let blocked = sample_task(TaskKind::Execute, Status::Blocked);
    store.insert(&blocked).expect("insert blocked");

    let before = human_inbox(&store, None, &ctx, now(), &no_evidence).expect("inbox before");
    let draft_id = "draft_accept-root".to_string();
    let question_id = format!("question-{}", blocked.id);
    assert!(
        before.items.iter().any(|i| i.id == draft_id),
        "{:?}",
        before.items
    );
    assert!(
        before.items.iter().any(|i| i.id == question_id),
        "{:?}",
        before.items
    );

    crate::gate::accept(&store, draft.id, None).expect("accept");
    crate::gate::answer(&store, blocked.id, "A を使う".to_string(), None).expect("answer");

    let after = human_inbox(&store, None, &ctx, now(), &no_evidence).expect("inbox after");
    assert!(
        after.items.iter().all(|i| i.id != draft_id),
        "{:?}",
        after.items
    );
    assert!(
        after.items.iter().all(|i| i.id != question_id),
        "{:?}",
        after.items
    );
    assert_eq!(after.counts.total, 0);
}
