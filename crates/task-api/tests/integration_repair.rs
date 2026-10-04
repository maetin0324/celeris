//! ADR-0120 D5: `TaskDetail.integration_repair` / 受信箱 `AttentionItem::Failed.integration_repair`
//! が review 前同期の衝突解消（IntegrationRepair）の状況を出し、実装失敗（レビュー不合格・
//! `ReviewFail`）とは別の欄であることを確かめる。

mod common;

use common::*;
use serde_json::Value;
use task_core::{Event, RepoId, Status, TaskKind};

#[tokio::test]
async fn task_detail_shows_integration_repair_scheduled_state() {
    let env = TestEnv::new();
    let app = env.router();
    let repo_id = RepoId::new();

    let scheduled = new_task(TaskKind::Execute, Status::Running);
    env.seed_with(
        &scheduled,
        vec![Event::IntegrationRepairScheduled {
            work_unit_id: "wu-1".into(),
            key: "integration-repair-1".into(),
            repo_id,
            target_ref: "refs/heads/main".into(),
            target_sha: "abc123".into(),
            before_sha: "def456".into(),
            conflict_files: vec!["src/a.rs".into(), "src/b.rs".into()],
            attempt: 1,
        }],
    );

    let resp = send(&app, get(&format!("/api/v1/tasks/{}", scheduled.id))).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body: Value = resp.json();
    let repair = &body["integration_repair"];
    assert_eq!(repair["state"], "scheduled");
    assert_eq!(repair["work_unit_id"], "wu-1");
    assert_eq!(repair["attempt"], 1);
    assert_eq!(repair["max_attempts"], 2);
    assert_eq!(repair["target_ref"], "refs/heads/main");
    assert_eq!(repair["target_sha"], "abc123");
    assert_eq!(repair["before_sha"], "def456");
    assert_eq!(
        repair["conflict_files"],
        serde_json::json!(["src/a.rs", "src/b.rs"])
    );
    assert!(
        repair.get("reason").is_none(),
        "scheduled has no exhaust reason"
    );
    // 走っているだけの task は失敗欄を持たない（integration_repair とは別の欄）。
    assert!(body["failure"].is_null());
}

#[tokio::test]
async fn task_detail_review_fail_has_no_integration_repair_history() {
    let env = TestEnv::new();
    let app = env.router();

    let failed = new_task(TaskKind::Execute, Status::Failed);
    env.seed_with(
        &failed,
        vec![
            Event::ReviewVerdict {
                run_id: "rev-1".into(),
                criterion_idx: 0,
                pass: false,
                reason: "テストが落ちている".into(),
            },
            Event::Transitioned {
                from: Status::Reviewing,
                to: Status::Failed,
                reason: "review_fail".into(),
            },
        ],
    );

    let resp = send(&app, get(&format!("/api/v1/tasks/{}", failed.id))).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body: Value = resp.json();
    assert!(
        body.get("integration_repair").is_none(),
        "a plain review-fail task has no integration repair history to project"
    );
    assert_eq!(body["failure"]["class"], "work");
    assert_eq!(body["failure"]["reason"], "テストが落ちている");
}

#[tokio::test]
async fn inbox_attention_failed_carries_integration_repair_separately_from_review_fail() {
    let env = TestEnv::new();
    let app = env.router();
    let repo_id = RepoId::new();

    // 従来経路へ落ちた（上限超過）: 成果は保持し fallback で未同期 HEAD を review に回したが、
    // reviewer 自身はそれとは無関係な理由で不合格にした。task の失敗理由は「レビュー不合格」のまま、
    // integration_repair 欄だけが exhausted の履歴を別途示す。
    let failed = new_task(TaskKind::Execute, Status::Failed);
    env.seed_with(
        &failed,
        vec![
            Event::IntegrationRepairScheduled {
                work_unit_id: "wu-1".into(),
                key: "integration-repair-1".into(),
                repo_id,
                target_ref: "refs/heads/main".into(),
                target_sha: "target-sha".into(),
                before_sha: "before-sha".into(),
                conflict_files: vec!["src/a.rs".into()],
                attempt: 2,
            },
            Event::IntegrationRepairExhausted {
                work_unit_id: None,
                repo_id,
                target_sha: "target-sha".into(),
                before_sha: "before-sha".into(),
                attempt: 2,
                reason: task_core::IntegrationRepairExhaustReason::LimitReached,
                rollback_to_sha: None,
                fallback: true,
            },
            Event::WorkerStarted {
                run_id: "rev-1".into(),
                adapter: "fake".into(),
                model: "m".into(),
                provider: None,
                account: None,
                role: None,
                task_role: None,
            },
            Event::ReviewVerdict {
                run_id: "rev-1".into(),
                criterion_idx: 0,
                pass: false,
                reason: "テストが落ちている".into(),
            },
            Event::Transitioned {
                from: Status::Reviewing,
                to: Status::Failed,
                reason: "review_fail".into(),
            },
        ],
    );

    let resp = send(&app, get("/api/v1/inbox")).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    let body: Value = resp.json();
    let item = body["attention"]
        .as_array()
        .expect("attention")
        .iter()
        .find(|a| a["task"]["id"] == failed.id.to_string())
        .expect("failed task in attention");
    assert_eq!(item["type"], "failed");
    assert_eq!(item["class"], "work");
    assert_eq!(item["reason"], "テストが落ちている");
    let repair = &item["integration_repair"];
    assert_eq!(repair["state"], "exhausted");
    assert_eq!(repair["reason"], "limit_reached");
    assert_eq!(repair["fallback"], true);
    assert_eq!(repair["attempt"], 2);
    assert_eq!(repair["max_attempts"], 2);
}
