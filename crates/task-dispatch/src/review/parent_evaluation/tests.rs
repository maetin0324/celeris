//! ADR 2026-11-22-parent-checkpoint-evaluator D3.1・D4 R4（付記 WU eval-input）: 親の途中評価の入力の試験（接頭辞
//! `parent_evaluation_input_`）。
//!
//! 守る不変条件:
//! - `request` の `goal.outcomes` は承認済み Roadmap の Outcome（id・title・acceptance）で、`reached` は D6 の読み口
//!   （`recorded_outcomes`・`root_satisfies`。controller の画面の `summary` と同じ値）。
//! - 子の `criteria` は子の受け入れ条件と今の試行の review の判定（snapshot の root の行と同じ組み立て）。
//! - `facts` の `goal_reached` は同じ D6 の値（対象の Outcome がすべて到達のときだけ真）。
//! - 評価器の prompt（`parent_checkpoint_section`）に「（Outcome の記録は無い）」が出ない。
//!
//! 既存の試験で検出できない理由: `project_controller_checkpoint_*` は Roadmap の無い案件で起動と決着の経路だけを見、
//! task-api の `parent_checkpoint_live_*`・`parent_checkpoint_blind_*` は request の欄を試験の中で組み立てる。
//! どれも `request` が `goal.outcomes`・`criteria` を空で渡し、`facts` が `goal_reached` を偽に固定しても通る。

use super::*;
use task_core::roadmap::{OutcomeStatus, RoadmapOutcomeSpec, RoadmapProposalInput};
use task_core::store::NewOutcomeEvent;
use task_core::{Check, Criterion, Project, ProjectId, ProjectStatus};
use time::OffsetDateTime;

const ABSENT: &str = "（Outcome の記録は無い）";

fn acc(id: &str) -> String {
    format!("{id} の検査が通る")
}

struct Case {
    _dir: crate::test_support::WritableTempDir,
    dir: std::path::PathBuf,
    store: SqliteStore,
    p: Project,
}

impl Case {
    fn new() -> Self {
        let dir = crate::test_support::WritableTempDir::new();
        let path = dir.path().to_path_buf();
        let store = SqliteStore::open(&path.join("celeris.db")).unwrap();
        let now = OffsetDateTime::now_utc();
        let p = Project {
            id: ProjectId::new(),
            title: "eval-input".into(),
            request: "親の目的".into(),
            status: ProjectStatus::Active,
            workspace: None,
            archived_at: None,
            paused_from: None,
            auto_advance: false,
            slug: None,
            created_at: now,
            updated_at: now,
        };
        store.project_create(&p).unwrap();
        store
            .project_controller_set_mode(
                &p.id.to_string(),
                ControllerMode::Enabled,
                None,
                "human:test",
                now,
            )
            .unwrap();
        Self {
            _dir: dir,
            dir: path,
            store,
            p,
        }
    }

    fn pid(&self) -> String {
        self.p.id.to_string()
    }

    /// 人が Roadmap の版（Outcome `ids`）を提案して承認する。
    fn approve_roadmap(&self, ids: &[&str]) {
        let pid = self.pid();
        let now = OffsetDateTime::now_utc();
        let input = RoadmapProposalInput {
            base_version: None,
            change_reason: "試験".into(),
            outcomes: ids
                .iter()
                .map(|id| RoadmapOutcomeSpec {
                    id: (*id).into(),
                    title: format!("{id} を仕上げる"),
                    acceptance: vec![acc(id)],
                    priority: 0,
                    depends_on: Vec::new(),
                    dropped: false,
                    checks: Vec::new(),
                })
                .collect(),
        };
        let v = self
            .store
            .write_tx(|tx| {
                SqliteStore::roadmap_propose_tx(tx, &pid, &input, "sha", "human:test", now)
            })
            .unwrap();
        self.store
            .write_tx(|tx| {
                SqliteStore::roadmap_decide_tx(tx, &pid, v.version, true, None, "human:test", now)
            })
            .unwrap();
    }

    /// 案件の task（`parent` が `None` なら root）。受け入れ条件は `(text, check)`。
    fn task(&self, parent: Option<&Task>, title: &str, criteria: Vec<(String, Check)>) -> Task {
        let mut t = crate::review::tests::task_with(Vec::new(), &self.dir);
        t.title = title.into();
        t.project_id = Some(self.p.id);
        t.parent_id = parent.map(|p| p.id);
        t.status = Status::Running;
        t.acceptance = criteria
            .into_iter()
            .map(|(text, check)| Criterion { text, check })
            .collect();
        self.store.create_task(&t, vec![]).unwrap();
        t
    }

    fn event(&self, t: &Task, e: Event) {
        self.store.append_event(t.id, &e).unwrap();
    }

    fn set_status(&self, t: &Task, to: Status, reason: &str) {
        let s = serde_json::to_value(to).unwrap();
        let conn = rusqlite::Connection::open(self.dir.join("celeris.db")).unwrap();
        conn.execute(
            "UPDATE tasks SET status = ?1, json = json_set(json, '$.status', ?1) WHERE id = ?2",
            rusqlite::params![s.as_str().unwrap(), t.id.to_string()],
        )
        .unwrap();
        self.event(
            t,
            Event::Transitioned {
                from: Status::Running,
                to,
                reason: reason.into(),
            },
        );
    }

    /// review の判定（`passes[i]` が条件 i の結果）を置いて、全部が合格なら done にする。
    fn review(&self, t: &Task, passes: &[bool]) {
        self.set_status(t, Status::Reviewing, "worker_done");
        for (i, pass) in passes.iter().enumerate() {
            self.event(
                t,
                Event::ReviewVerdict {
                    run_id: "rev".into(),
                    criterion_idx: i,
                    pass: *pass,
                    reason: String::new(),
                },
            );
        }
        if passes.iter().all(|p| *p) {
            self.set_status(t, Status::Done, "review_pass");
        }
    }

    /// Outcome `id` の成果を持つ root（done・合格・成果物あり）を置き、Roadmap に `reached` を記録する。
    fn reach(&self, id: &str) -> Task {
        let r = self.task(
            None,
            &format!("{id} の root"),
            vec![(acc(id), Check::Reviewer)],
        );
        self.review(&r, &[true]);
        self.event(
            &r,
            Event::ArtifactProduced {
                run_id: "run".into(),
                artifact: task_core::ArtifactRef {
                    name: "report.md".into(),
                    path: "artifacts/report.md".into(),
                    sha256: "0".repeat(64),
                    kind: "file".into(),
                    declared: true,
                },
            },
        );
        let pid = self.pid();
        let evidence = vec![format!("task:{}", r.id)];
        self.store
            .write_tx(|tx| {
                SqliteStore::roadmap_outcome_event_tx(
                    tx,
                    &NewOutcomeEvent {
                        project_id: &pid,
                        outcome_id: id,
                        status: Some(OutcomeStatus::Reached),
                        root_task_id: None,
                        evidence: &evidence,
                        reason: "試験",
                        actor: "controller",
                    },
                    OffsetDateTime::now_utc(),
                )
            })
            .unwrap();
        r
    }

    /// 戦略の文書を参照する `task.create` の CoS 操作（strategy root の正本。`task_core::strategy_reads`）。
    fn strategy_op(&self, root: &Task) {
        use task_core::chat::{AuditContext, ChatActor};
        let op = ulid::Ulid::new().to_string();
        let ctx = AuditContext {
            actor: ChatActor::Cos,
            thread_id: "thread".into(),
            run_id: "cos-run".into(),
            operation_id: op.clone(),
            reason: "eval-input test".into(),
            policy_version: "v1".into(),
        };
        let payload = serde_json::json!({
            "method": "POST", "path": "/api/v1/tasks", "body": {"title": "t"},
            "strategy": {"task_id": root.id.to_string(), "name": "strategy/s1/v1.json",
                         "sha256": format!("sha256:{}", "a".repeat(64)), "candidate": null}
        });
        self.store
            .cos_operation_apply_at(
                &ctx,
                &op,
                "hash",
                "task",
                &root.id.to_string(),
                None,
                "task.create",
                &payload,
                OffsetDateTime::now_utc(),
                |_, _| Ok(serde_json::json!({})),
            )
            .unwrap();
    }

    /// D6 の読み口の画面の値（`summary` の Outcome ごとの `reached`）。
    fn d6(&self) -> Vec<(String, bool)> {
        let state = self
            .store
            .project_controller_state(&self.pid())
            .unwrap()
            .unwrap();
        task_ops::project_controller_view::summary(
            &self.store,
            self.p.id,
            &state,
            None,
            &[],
            &task_ops::project_budget::BudgetLimits::default(),
            OffsetDateTime::now_utc(),
        )
        .unwrap()
        .outcomes
        .into_iter()
        .map(|o| (o.id, o.reached))
        .collect()
    }
}

fn no_tokens(
    _: &SqliteStore,
    _: TaskId,
    _: Option<&str>,
) -> Result<u64, task_core::store::StoreError> {
    Ok(0)
}

fn decision() -> RunningDecision {
    RunningDecision {
        id: "dec-1".into(),
        triggers: vec!["children:2".into()],
        request_sha256: sha256_hex("親の目的"),
        at: "9999-12-31T23:59:59Z".into(),
    }
}

/// 承認済み Roadmap（o1 到達・o2 未達）・strategy root・受け入れ条件つきの子 2 件。`request` の Outcome と子の
/// criteria が埋まり、`reached`・`goal_reached` が D6 と一致し、prompt に「（Outcome の記録は無い）」が出ない。
/// o2 も到達させると `goal_reached` が真になる。
#[test]
fn parent_evaluation_input_outcomes_criteria_and_goal_reached_follow_d6() {
    let c = Case::new();
    c.approve_roadmap(&["o1", "o2"]);
    c.reach("o1");
    let root = c.task(None, "strategy root", vec![(acc("o2"), Check::Reviewer)]);
    c.strategy_op(&root);
    let a = c.task(
        Some(&root),
        "子 A",
        vec![
            ("A の報告がある".into(), Check::Reviewer),
            (
                "A の検査が 0 で終わる".into(),
                Check::Command {
                    cmd: "false".into(),
                    expect_exit: 0,
                },
            ),
        ],
    );
    c.review(&a, &[true, false]);
    let b = c.task(
        Some(&root),
        "子 B",
        vec![("B の表がある".into(), Check::Human)],
    );

    let req = request(&c.store, &root, &decision(), no_tokens)
        .unwrap()
        .expect("strategy root");
    let got: Vec<(String, String, Vec<String>, bool)> = req
        .goal
        .outcomes
        .iter()
        .map(|o| {
            (
                o.id.clone(),
                o.title.clone(),
                o.acceptance.clone(),
                o.reached,
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![
            ("o1".into(), "o1 を仕上げる".into(), vec![acc("o1")], true),
            ("o2".into(), "o2 を仕上げる".into(), vec![acc("o2")], false),
        ]
    );
    let reached: Vec<(String, bool)> = req
        .goal
        .outcomes
        .iter()
        .map(|o| (o.id.clone(), o.reached))
        .collect();
    assert_eq!(reached, c.d6(), "reached は D6 の読み口と同じ値");

    let criteria = |id: TaskId| -> Vec<(String, String, String)> {
        req.children
            .iter()
            .find(|k| k.task_id == id)
            .expect("child")
            .criteria
            .iter()
            .map(|k| (k.text.clone(), k.kind.clone(), k.result.clone()))
            .collect()
    };
    assert_eq!(
        criteria(a.id),
        vec![
            ("A の報告がある".into(), "reviewer".into(), "pass".into()),
            (
                "A の検査が 0 で終わる".into(),
                "command".into(),
                "fail".into()
            ),
        ]
    );
    assert_eq!(
        criteria(b.id),
        vec![("B の表がある".into(), "human".into(), "unknown".into())]
    );

    assert!(!facts(&c.store, &root, None).unwrap().goal_reached);

    let review = task_worker::ReviewRequest {
        parent_checkpoint: Some(Box::new(req)),
        ..Default::default()
    };
    let prompt = task_worker::change_rules::review::parent_checkpoint_section(Some(&review));
    assert!(!prompt.contains(ABSENT), "{prompt}");
    assert!(
        prompt.contains("- outcome `o1`: o1 を仕上げる（reached=true）"),
        "{prompt}"
    );
    assert!(
        prompt.contains("  - [command] fail: A の検査が 0 で終わる"),
        "{prompt}"
    );

    // o2 も到達すると、対象の Outcome がすべて到達で goal_reached は真（D6 と同じ）。
    c.reach("o2");
    assert_eq!(c.d6(), vec![("o1".into(), true), ("o2".into(), true)]);
    let req = request(&c.store, &root, &decision(), no_tokens)
        .unwrap()
        .expect("strategy root");
    assert!(req.goal.outcomes.iter().all(|o| o.reached));
    assert!(facts(&c.store, &root, None).unwrap().goal_reached);
}

/// Outcome の無い案件（Roadmap も依頼文の Outcome block も無い）では `outcomes` は空で、`goal_reached` は偽
/// （D6 の `unverified_outcomes` は対象 0 件を達成にしない）。子の criteria は入る。
#[test]
fn parent_evaluation_input_no_outcomes_is_not_reached() {
    let c = Case::new();
    let root = c.task(None, "strategy root", Vec::new());
    c.strategy_op(&root);
    c.task(
        Some(&root),
        "子 A",
        vec![("A の報告がある".into(), Check::Reviewer)],
    );
    let req = request(&c.store, &root, &decision(), no_tokens)
        .unwrap()
        .expect("strategy root");
    assert!(req.goal.outcomes.is_empty());
    assert_eq!(req.children[0].criteria.len(), 1);
    assert_eq!(req.children[0].criteria[0].result, "unknown");
    assert!(!facts(&c.store, &root, None).unwrap().goal_reached);
}
