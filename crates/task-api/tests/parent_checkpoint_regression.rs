//! ADR 2026-11-22-parent-checkpoint-evaluator D9.2: 回帰（接頭辞 `parent_checkpoint_regression_`）。従来の低抽象度の
//! task（strategy の無い root）・controller が無効な案件・shadow の案件で、評価の判断と reviewer run が増えず、
//! 節の無い review の wire と prompt が変わらない。
//!
//! 本物の snapshot（`task_ops::project_snapshot::build`・`checkpoints`）と本物の Rust の検証
//! （`project_proposal::parse_and_validate`）を通す。TS の規則は D5.1 の 1 をこの file が書く（task-api は TS の host に依存しない）。
//! reviewer run の数は一時 DB の `runs` の `role = 'reviewer'` の行と `WorkerStarted` の event で数える。
//!
//! 守る不変条件と、既存の試験で検出できない理由: task-dispatch の `project_controller_checkpoint_no_strategy_root_or_disabled_`
//! は合成の子 2 件で起動の段だけを見る。ここでは子 4 件が done の strategy 無しの root で snapshot の JSON に
//! `checkpoints` の key が出ないこと、mode `off` と行の無い案件で controller が判断の対象にならないこと、
//! final review の `request.json` と prompt に節が出ないことを、task-api の側から確かめる。

mod common;

use common::*;
use serde_json::{Value, json};
use task_core::chat::{AuditContext, ChatActor};
use task_core::project_proposal::{
    self as pp, ProjectSnapshot, SnapshotController, SnapshotDecision, SnapshotLimits,
};
use task_core::project_wake::ControllerMode;
use task_core::{Event, ProjectId, Status, Task, TaskKind};
use task_ops::project_snapshot::{self as ps, CheckpointDecisionRow, SnapshotContext};
use task_worker::protocol::ReviewRequest;
use time::OffsetDateTime;

const REQUEST: &str = "低抽象度の回帰の案件。\n\
```celeris-outcomes\n\
[{\"id\":\"o1\",\"title\":\"修正\",\"acceptance\":[\"不具合が直る\"]}]\n\
```\n";

struct Case {
    env: TestEnv,
    project: ProjectId,
    root: Task,
}

/// root 1 件と、done の直下の子 `done` 件。`strategy` が真なら root を strategy root にする（D1.1）。
fn case(done: usize, strategy: bool) -> Case {
    let env = admin_env();
    let now = OffsetDateTime::now_utc();
    let project = task_core::Project {
        id: ProjectId::new(),
        title: "regression".into(),
        request: REQUEST.into(),
        status: task_core::ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        auto_advance: false,
        slug: None,
        created_at: now,
        updated_at: now,
    };
    env.store.project_create(&project).expect("project");
    let mut root = new_task(TaskKind::Execute, Status::Running);
    root.project_id = Some(project.id);
    env.store.create_task(&root, vec![]).expect("root");
    for _ in 0..done {
        let mut t = new_task(TaskKind::Execute, Status::Done);
        t.project_id = Some(project.id);
        t.parent_id = Some(root.id);
        env.store.create_task(&t, vec![]).expect("child");
        env.store
            .append_event(
                t.id,
                &Event::Transitioned {
                    from: Status::Reviewing,
                    to: Status::Done,
                    reason: "review_pass".into(),
                },
            )
            .expect("done");
    }
    if strategy {
        let op = ulid::Ulid::new().to_string();
        let ctx = AuditContext {
            actor: ChatActor::Cos,
            thread_id: "thread".into(),
            run_id: "cos-run".into(),
            operation_id: op.clone(),
            reason: "regression".into(),
            policy_version: "v1".into(),
        };
        let payload = json!({
            "method": "POST", "path": "/api/v1/tasks", "body": {"title": "root"},
            "strategy": {"task_id": root.id.to_string(), "name": "strategy/s1/v1.json",
                         "sha256": format!("sha256:{}", "a".repeat(64)), "candidate": null}
        });
        env.store
            .cos_operation_apply_at(
                &ctx,
                &op,
                "hash",
                "task",
                &root.id.to_string(),
                None,
                "task.create",
                &payload,
                now,
                |_, _| Ok(json!({})),
            )
            .expect("cos operation");
    }
    Case {
        env,
        project: project.id,
        root,
    }
}

impl Case {
    fn snapshot(
        &self,
        mode: &str,
        decisions: &[(SnapshotDecision, pp::ProjectProposal)],
    ) -> ProjectSnapshot {
        let mut s = ps::build(
            &self.env.store,
            self.project,
            SnapshotContext {
                controller: SnapshotController {
                    mode: mode.into(),
                    phase: "active".into(),
                    revision: decisions.len() as u64,
                    human_epoch: 0,
                    retry_count: 0,
                    progress_epoch: 0,
                },
                decisions: decisions.iter().map(|(d, _)| d.clone()).collect(),
                wakes: Vec::new(),
                limits: SnapshotLimits::default(),
                now: OffsetDateTime::now_utc(),
            },
        )
        .expect("snapshot");
        let rows: Vec<CheckpointDecisionRow<'_>> = decisions
            .iter()
            .map(|(d, p)| CheckpointDecisionRow {
                id: &d.id,
                effect_state: &d.effect_state,
                proposal: p,
            })
            .collect();
        s.checkpoints = ps::checkpoints(
            &self.env.store,
            &self.env.view_context().workspace_roots,
            &s.roots,
            &rows,
            &Default::default(),
            &Default::default(),
            &mut |_| Ok(0),
        )
        .expect("checkpoints");
        s
    }

    /// reviewer run の数（`runs` の reviewer の行と `WorkerStarted{role: reviewer}` の event）。
    fn reviewer_runs(&self) -> i64 {
        let rows = count(
            &self.env,
            "SELECT COUNT(*) FROM runs WHERE role = 'reviewer'",
        );
        let started = self
            .env
            .store
            .events_for(self.root.id)
            .expect("events")
            .into_iter()
            .filter(|(_, e)| matches!(e, Event::WorkerStarted { .. }))
            .count() as i64;
        rows + started
    }
}

fn evaluate(s: &ProjectSnapshot, root_id: &str, triggers: &[&str]) -> Value {
    json!({
        "action": {"kind": "evaluate", "root_id": root_id, "triggers": triggers},
        "gap": {"reached": [], "open": s.outcomes.iter().map(|o| o.id.clone()).collect::<Vec<_>>(),
                "unknown": [], "progress_epoch": s.controller.progress_epoch},
        "explanation": {"rule_id": "pc/checkpoint/evaluate", "signals": []},
        "based_on": {"revision": s.controller.revision, "human_epoch": s.controller.human_epoch,
                     "request_sha256": s.project.request_sha256},
    })
}

fn codes(s: &ProjectSnapshot, raw: &Value) -> Vec<String> {
    match pp::parse_and_validate(s, raw) {
        Ok(_) => Vec::new(),
        Err(v) => v.into_iter().map(|v| v.code).collect(),
    }
}

/// controller が enabled の案件で strategy 無しの root の子が 4 件 done でも、checkpoint は無く、snapshot の JSON に
/// `checkpoints` の key が無く、`evaluate` は検証で落ち（判断 0 件）、reviewer run は 0。
#[test]
fn parent_checkpoint_regression_no_strategy_root_no_reviewer_run() {
    let c = case(4, false);
    let s = c.snapshot("enabled", &[]);
    assert!(s.checkpoints.is_empty());
    let v = serde_json::to_value(&s).expect("snapshot json");
    assert!(v.get("checkpoints").is_none(), "{v}");
    let got = codes(&s, &evaluate(&s, &c.root.id.to_string(), &["children:4"]));
    assert!(
        got.contains(&"checkpoint_root_unknown".to_string()),
        "{got:?}"
    );
    assert_eq!(c.reviewer_runs(), 0);
    println!(
        "no_strategy_root evaluate_decisions=0 reviewer_runs={}",
        c.reviewer_runs()
    );
}

/// strategy root がある案件で controller の行が無い・`off` のとき、案件は判断の対象（`project_controller_active`）に
/// 入らず、判断 0 件・reviewer run 0。
#[test]
fn parent_checkpoint_regression_controller_off_no_reviewer_run() {
    let c = case(4, true);
    let pid = c.project.to_string();
    let active = |c: &Case| {
        c.env
            .store
            .project_controller_active()
            .expect("active")
            .into_iter()
            .any(|st| st.project_id == pid)
    };
    assert!(
        c.env
            .store
            .project_controller_state(&pid)
            .expect("state")
            .is_none()
    );
    assert!(!active(&c), "行の無い案件は判断しない");
    c.env
        .store
        .project_controller_set_mode(
            &pid,
            ControllerMode::Off,
            None,
            "human:test",
            OffsetDateTime::now_utc(),
        )
        .expect("mode off");
    assert!(!active(&c), "off の案件は判断しない");
    // 判断が無いので evaluate は 1 件も記録されない（snapshot の checkpoint は due でも起動しない）。
    let s = c.snapshot("off", &[]);
    assert_eq!(s.checkpoints.len(), 1);
    assert_eq!(c.reviewer_runs(), 0);
    println!(
        "controller_off evaluate_decisions=0 reviewer_runs={}",
        c.reviewer_runs()
    );
}

/// mode `shadow` で `evaluate` は `suppressed` で記録され、reviewer run は 0。同じ trigger の 2 回目の判断は無い。
#[test]
fn parent_checkpoint_regression_shadow_no_reviewer_run() {
    let c = case(2, true);
    let s = c.snapshot("shadow", &[]);
    let due: Vec<String> = s.checkpoints[0].due.iter().map(|t| t.key.clone()).collect();
    assert_eq!(due, vec!["children:2".to_string()]);
    let raw = evaluate(&s, &c.root.id.to_string(), &["children:2"]);
    let p = pp::parse_and_validate(&s, &raw).expect("valid evaluate");
    let d = SnapshotDecision {
        id: "dec-001".into(),
        kind: "evaluate".into(),
        rule_id: "pc/checkpoint/evaluate".into(),
        effect_state: "suppressed".into(),
        root_key: None,
        reason_code: None,
        outcome_id: None,
        request_sha256: s.project.request_sha256.clone(),
        progress_epoch: 0,
        at: s.now.clone(),
    };
    let decisions = vec![(d, p)];
    let s2 = c.snapshot("shadow", &decisions);
    assert!(s2.checkpoints[0].due.is_empty(), "suppressed は発火済み");
    assert!(
        s2.checkpoints[0].running.is_none(),
        "suppressed は実行中でない"
    );
    let again = codes(&s2, &evaluate(&s2, &c.root.id.to_string(), &["children:2"]));
    assert!(
        again.contains(&"checkpoint_not_due".to_string()),
        "{again:?}"
    );
    assert_eq!(c.reviewer_runs(), 0);
    println!(
        "shadow evaluate_decisions=1 suppressed reviewer_runs={}",
        c.reviewer_runs()
    );
}

/// strategy root の子の final review の `request.json` に `parent_checkpoint` の key が無く、prompt に節が無い
/// （D3.1・D3.3。`parent_checkpoint` の無い review は base と同じ）。
#[test]
fn parent_checkpoint_regression_final_review_unchanged() {
    let c = case(1, true);
    let review = ReviewRequest {
        summary: "did the work".into(),
        criteria: vec![0],
        ..ReviewRequest::default()
    };
    let v = serde_json::to_value(&review).expect("request json");
    assert!(v.get("parent_checkpoint").is_none(), "{v}");
    assert_eq!(
        task_worker::change_rules::review::parent_checkpoint_section(Some(&review)),
        ""
    );
    assert_eq!(c.reviewer_runs(), 0);
}
