//! 第 4 フェーズ P4-B（ADR 2026-11-05-phase4-p4b-evidence-revocation）: 達成の後の根拠の失効（接頭辞
//! `project_revocation_`）。
//!
//! phase `achieved` の案件で根拠の root が reopen・再 review の不合格・取り込みの reopen になると、event から
//! wake（`evidence_changed`・`review_failed`）が積まれ、Rust が達成を数え直して phase を `waiting` に戻す。旧い達成の
//! 判断の行と Roadmap の `reached` の記録は消さない。LLM は 0、時計は注入する。cron の wake は入れない（event だけで
//! 起きることを確かめる）。

use super::*;

const REVOKED: &str = "pc/guard/achievement-revoked";

impl Case {
    /// cron の wake を入れずに controller の tick を 1 回流す（event から積まれた wake・timer だけで評価する）。
    fn tick_only(&mut self) {
        self.env.advance(1);
        tick(&mut self.d);
    }

    fn settle_events(&mut self) {
        for _ in 0..6 {
            self.tick_only();
        }
    }

    /// done の root を ready に戻す（`Transitioned` done → ready）。
    fn reopen(&self, t: &Task) {
        let conn = rusqlite::Connection::open(self.env.db()).unwrap();
        conn.execute(
            "UPDATE tasks SET status = 'ready', json = json_set(json, '$.status', 'ready') WHERE id = ?1",
            [t.id.to_string()],
        )
        .unwrap();
        self.event(
            t,
            Event::Transitioned {
                from: Status::Done,
                to: Status::Ready,
                reason: "reopen".into(),
            },
        );
    }

    fn integration_reopened(&self, t: &Task) {
        self.event(
            t,
            Event::IntegrationReopened {
                repo_id: task_core::RepoId::new(),
                branch: "b".into(),
                head_sha: "0".repeat(40),
                reason: Some("reverted".into()),
                actor: "human:test".into(),
            },
        );
    }

    fn validations(&self) -> Vec<String> {
        self.decisions()
            .into_iter()
            .rev()
            .map(|d| d.validation)
            .collect()
    }

    fn count_rule(&self, prefix: &str) -> usize {
        self.validations()
            .iter()
            .filter(|v| v.starts_with(prefix))
            .count()
    }

    /// 人が Roadmap の版を提案して承認する（`checks` 付き・acceptance の差し替えができる形）。
    fn approve_specs(&self, specs: Vec<RoadmapOutcomeSpec>) -> i64 {
        let pid = self.pid();
        let now = self.env.now();
        let base = self
            .store
            .roadmap_approved(&pid)
            .unwrap()
            .map(|v| v.version);
        let input = RoadmapProposalInput {
            base_version: base,
            change_reason: "試験".into(),
            outcomes: specs,
        };
        let v = self
            .store
            .write_tx(|tx| {
                SqliteStore::roadmap_propose_tx(tx, &pid, &input, "sha", "human:test", now)
            })
            .unwrap();
        self.decide_roadmap(v.version).unwrap()
    }

    fn task(&self, id: TaskId) -> Task {
        self.store.get(id).unwrap().unwrap()
    }

    /// 今の snapshot（root の行と Outcome だけを見る。controller の値は使わない）。
    fn snapshot(&self) -> ProjectSnapshot {
        use task_core::project_proposal::{SnapshotController, SnapshotLimits};
        task_ops::project_snapshot::build(
            &self.store,
            self.p.id,
            task_ops::project_snapshot::SnapshotContext {
                controller: SnapshotController {
                    mode: "enabled".into(),
                    phase: "active".into(),
                    revision: 0,
                    human_epoch: 0,
                    retry_count: 0,
                    progress_epoch: 0,
                },
                decisions: Vec::new(),
                wakes: Vec::new(),
                limits: SnapshotLimits::default(),
                now: self.env.now(),
            },
        )
        .unwrap()
    }

    /// controller が起票した root（古い順）。
    fn controller_roots(&self) -> Vec<Task> {
        let mut v: Vec<Task> = self
            .store
            .list(None)
            .unwrap()
            .into_iter()
            .filter(|t| {
                t.project_id == Some(self.p.id)
                    && t.labels
                        .iter()
                        .any(|l| l == crate::project_controller::CONTROLLER_LABEL)
            })
            .collect();
        v.sort_by_key(|t| t.created_at);
        v
    }
}

fn spec(id: &str, acceptance: &str, checks: &[&str]) -> RoadmapOutcomeSpec {
    RoadmapOutcomeSpec {
        id: id.into(),
        title: format!("{id} を仕上げる"),
        acceptance: vec![acceptance.into()],
        priority: 0,
        depends_on: Vec::new(),
        dropped: false,
        checks: checks
            .iter()
            .map(|c| task_core::project_proposal::OutcomeCheck {
                cmd: (*c).into(),
                expect_exit: 0,
            })
            .collect(),
    }
}

/// 達成まで進めた案件（根拠の root を返す）。
fn achieved_case(roadmap: bool, repo: bool) -> (Case, Task) {
    let mut c = lying_case(&["o1"]);
    if roadmap {
        c.approve_roadmap(&["o1"]).unwrap();
    }
    let r = c.root(c.p.id, "o1", repo);
    c.review_done(&r, true);
    if repo {
        c.merged(&r);
    } else {
        c.deliver(&r);
    }
    c.settle();
    assert_eq!(c.phase(), ControllerPhase::Achieved);
    (c, r)
}

fn assert_revoked(c: &Case) {
    assert_ne!(c.phase(), ControllerPhase::Achieved);
    assert_eq!(
        c.count_rule(&format!("{REVOKED}:o1")),
        1,
        "{:?}",
        c.validations()
    );
    // 旧い達成の判断の行は残る（監査）。
    assert!(c.count_rule("pc/achieved") >= 1);
}

/// G1: 達成の後に根拠の root が reopen されると、event の wake（`evidence_changed`）で Rust が数え直し、phase を
/// waiting に戻す。旧い達成の判断と Roadmap の `reached` の記録は残る。cron の wake は使わない。
#[test]
fn project_revocation_reopen_after_achieved_returns_to_waiting() {
    let (mut c, r) = achieved_case(true, false);
    let reached_before = c.roadmap_reached("o1");
    c.reopen(&r);
    c.settle_events();
    assert_revoked(&c);
    assert_eq!(c.roadmap_reached("o1"), reached_before);
    let revoked = c
        .decisions()
        .into_iter()
        .find(|d| d.validation.starts_with(REVOKED))
        .unwrap();
    let rec: Value = serde_json::from_str(&revoked.proposal_json).unwrap();
    assert_eq!(rec["guard"], "achievement-revoked");
    assert_eq!(rec["detail"]["unverified"], json!(["o1"]));
    assert!(
        rec["detail"]["wakes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().starts_with("evidence_changed:")),
        "{rec}"
    );
    assert_eq!(revoked.effect_state, EffectState::None);
    // 直ちに判定（TS）に回り、TS が達成と偽っても Rust の検証で waiting のまま。
    assert!(
        c.validations()
            .iter()
            .any(|v| v.starts_with(ACHIEVED_UNVERIFIED_RULE))
    );
}

/// G1（再 review の不合格）: 達成の後に root が reopen され、2 回目の review で command の検査が落ちて failed に
/// なっても、達成に戻らない。
#[test]
fn project_revocation_review_fail_after_achieved() {
    let (mut c, r) = achieved_case(false, false);
    c.reopen(&r);
    c.review_done(&r, false);
    c.set_status(&r, Status::Failed, "review_fail");
    c.settle_events();
    assert_revoked(&c);
    assert_eq!(c.records_applied(), 1);
}

/// G1（取り込みの reopen）: repo 付きの root の取り込みが取り消されたら waiting に戻り、取り込み直すと再達成する。
#[test]
fn project_revocation_integration_reopened_after_achieved() {
    let (mut c, r) = achieved_case(true, true);
    c.integration_reopened(&r);
    c.settle_events();
    assert_revoked(&c);
    c.merged(&r);
    c.settle_events();
    assert_eq!(c.phase(), ControllerPhase::Achieved);
    assert_eq!(c.roadmap_reached("o1"), 1);
}

/// G1（陰性）: 根拠が残っていれば、cron の wake でも別の案件の root の reopen でも achieved のまま（静けさを保つ）。
#[test]
fn project_revocation_intact_evidence_and_other_project_stay_achieved() {
    let (mut c, _r) = achieved_case(false, false);
    let foreign_project = pc_project(&c.store, Some(ControllerMode::Enabled));
    let foreign = c.root(foreign_project.id, "o1", false);
    c.review_done(&foreign, true);
    c.deliver(&foreign);
    c.reopen(&foreign);
    c.settle_events();
    c.settle();
    assert_eq!(c.phase(), ControllerPhase::Achieved);
    assert_eq!(c.count_rule(REVOKED), 0);
    assert!(c.count_rule("pc/guard/achieved-quiet") >= 1);
}

/// G1（人の Roadmap の改版）: 達成の後に人が o1 の acceptance を変えた版を承認すると、人の変更の wake で
/// 数え直し、旧い root は新しい条件の根拠にならないので waiting に戻る。旧い `reached` の記録は残る。
#[test]
fn project_revocation_human_roadmap_change_reopens() {
    let (mut c, _r) = achieved_case(true, false);
    c.approve_specs(vec![spec("o1", "o1 の新しい条件が通る", &[])]);
    c.settle();
    assert_ne!(c.phase(), ControllerPhase::Achieved);
    assert!(
        c.validations()
            .last()
            .unwrap()
            .starts_with(ACHIEVED_UNVERIFIED_RULE)
    );
    assert_eq!(c.roadmap_reached("o1"), 1);
}

/// G1（crash と競合）: 失効の数え直しの commit の直前で daemon が落ちても、作り直した daemon が同じ wake で数え
/// 直して waiting にする。同じ窓で人が mode を置き直す（`revision` の前進）と commit は stale になり、次の評価が
/// 決める。どちらの順でも achieved を書き直さない。
#[test]
fn project_revocation_crash_and_human_race_never_rewrite_achieved() {
    let (mut c, r) = achieved_case(false, false);
    let achieved_before = achieved_decisions(&c);
    // 1 回目: BeforeCommit で panic（crash）。
    c.d.set_project_controller_hook(Box::new(|b| {
        if b == Boundary::BeforeCommit {
            panic!("crash before commit");
        }
    }));
    c.reopen(&r);
    c.env.advance(1);
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tick(&mut c.d)));
    assert!(crashed.is_err());
    assert_eq!(
        c.phase(),
        ControllerPhase::Achieved,
        "nothing was committed"
    );
    let decider = crate::project_decider_ts::TsProjectDecider::with_fixed_script(
        crate::project_decider_ts::DeciderLimits::default(),
        Arc::new(FakeTs {
            f: Arc::new(lying_ts),
            seen: c.seen.clone(),
        }),
    );
    c.d = ts_daemon(
        &c.env,
        &c.store,
        decider,
        ProjectControllerSettings::default(),
    );
    // 2 回目: 人が mode を置き直す（revision の前進）を BeforeCommit に 1 回だけ割り込ませる。
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let store = Arc::clone(&c.store);
        let pid = c.pid();
        let clock = c.env.now();
        let fired = Arc::clone(&fired);
        c.d.set_project_controller_hook(Box::new(move |b| {
            use std::sync::atomic::Ordering::SeqCst;
            if b != Boundary::BeforeCommit || fired.swap(true, SeqCst) {
                return;
            }
            store
                .project_controller_set_mode(
                    &pid,
                    ControllerMode::Enabled,
                    None,
                    "human:test",
                    clock,
                )
                .unwrap();
        }));
    }
    // 前の daemon の lease が切れるまで進める。
    c.env.advance(3600);
    let mut stale = Vec::new();
    for _ in 0..6 {
        c.tick_only();
        stale.extend(c.d.project_controller_last().unwrap().stale);
    }
    assert!(fired.load(std::sync::atomic::Ordering::SeqCst));
    assert!(!stale.is_empty(), "the human race made a commit stale");
    assert_ne!(c.phase(), ControllerPhase::Achieved);
    assert_eq!(achieved_decisions(&c), achieved_before);
}

/// Roadmap の Outcome の root を起票し、根拠が消えたら直す root を起票する TS（嘘はつかない）。
fn planner_ts(s: &ProjectSnapshot) -> Value {
    let Some(o) = s.outcomes.first() else {
        return proposal(
            s,
            json!({"kind": "wait", "waiting_on": [], "next_wake_after_secs": 60, "phase": "waiting"}),
            "pc/test-no-outcomes",
            &[],
        );
    };
    let marked = s.decisions.iter().any(|d| {
        d.kind == "mark_milestone"
            && d.effect_state != "failed"
            && d.outcome_id.as_deref() == Some(o.id.as_str())
    });
    if let Some(r) = s
        .roots
        .iter()
        .find(|r| task_core::project_proposal::root_satisfies(r, o))
    {
        if !marked {
            return proposal(
                s,
                json!({"kind": "mark_milestone", "outcome_id": o.id, "evidence_refs": [format!("task:{}", r.id)]}),
                "pc/milestone-reached",
                &[],
            );
        }
        return achieved(s);
    }
    let in_flight: Vec<&str> = s
        .roots
        .iter()
        .filter(|r| !task_core::project_proposal::is_terminal_status(&r.status))
        .map(|r| r.id.as_str())
        .collect();
    if !in_flight.is_empty() {
        return proposal(
            s,
            json!({"kind": "continue", "watch_root_ids": in_flight, "next_wake_after_secs": 60}),
            "pc/root-in-flight",
            &[],
        );
    }
    let n = s
        .roots
        .iter()
        .filter(|r| r.controller_key.is_some())
        .count()
        + 1;
    proposal(
        s,
        json!({"kind": "launch_root", "root": {
            "key": format!("outcome:{}:{n}", o.id), "title": "o1 を仕上げる", "objective": "途中目標 o1 を達成する",
            "acceptance": o.acceptance.iter().map(|t| json!({"text": t, "check": {"type": "reviewer"}})).collect::<Vec<_>>(),
            "stages_hint": [], "continues_root_ids": [], "outcome_ids": [o.id]}}),
        "pc/outcome-open",
        &[],
    )
}

/// 起票された root を走らせた体にする（running → review → done、成果物あり）。
fn run_root(c: &Case, t: &Task, command_pass: bool) {
    let conn = rusqlite::Connection::open(c.env.db()).unwrap();
    conn.execute(
        "UPDATE tasks SET status = 'running', json = json_set(json, '$.status', 'running') WHERE id = ?1",
        [t.id.to_string()],
    )
    .unwrap();
    c.deliver(t);
    c.review_done(t, command_pass);
}

fn snap(c: &Case, step: &str) -> Value {
    json!({
        "step": step,
        "phase": c.phase().as_str(),
        "last_rule": c.last().validation,
        "controller_roots": c.controller_roots().iter().map(|t| serde_json::to_value(t.status).unwrap()).collect::<Vec<_>>(),
        "roadmap_reached_records": c.roadmap_reached("o1"),
    })
}

/// G1・G2 の e2e: Roadmap の Outcome の `checks` が controller の root の `Check::Command` になる → 到達 → 根拠の
/// root が reopen され再 review の command の検査が落ちる（失効）→ waiting に戻り、controller が直す root を起票 →
/// その root が合格して再達成。旧い達成の判断・`reached` の記録は残る。`PROJECT_REVOCATION_RAW` があれば各段の
/// 状態を JSON で書く。
#[test]
fn project_revocation_e2e_reach_invalidate_fix_reachieve() {
    let mut c = case(Some(ControllerMode::Enabled), planner_ts);
    c.set_request(&["o1"]);
    c.approve_specs(vec![spec("o1", &acc("o1"), &["test -f out/o1.txt"])]);
    let mut steps = Vec::new();
    c.settle_events();
    c.settle();
    let roots = c.controller_roots();
    assert_eq!(roots.len(), 1);
    let r1 = c.task(roots[0].id);
    // G2: Roadmap の checks が root の Check::Command になる。
    assert!(r1.acceptance.iter().any(|a| matches!(
        &a.check,
        Check::Command { cmd, expect_exit: 0 } if cmd == "test -f out/o1.txt"
    )));
    steps.push(snap(&c, "launched"));
    run_root(&c, &r1, true);
    c.settle_events();
    c.settle();
    assert_eq!(c.phase(), ControllerPhase::Achieved);
    assert_eq!(c.roadmap_reached("o1"), 1);
    steps.push(snap(&c, "achieved"));
    // 失効: reopen して 2 回目の review で command の検査が落ちる。
    c.reopen(&r1);
    c.review_done(&r1, false);
    c.set_status(&r1, Status::Failed, "review_fail");
    c.settle_events();
    assert_ne!(c.phase(), ControllerPhase::Achieved);
    assert_eq!(c.count_rule(&format!("{REVOKED}:o1")), 1);
    steps.push(snap(&c, "revoked"));
    // 直す root（2 本目）が起票される。
    c.settle_events();
    let roots = c.controller_roots();
    assert_eq!(roots.len(), 2, "{:?}", c.validations());
    let r2 = c.task(roots[1].id);
    assert!(
        r2.acceptance
            .iter()
            .any(|a| matches!(&a.check, Check::Command { cmd, .. } if cmd == "test -f out/o1.txt"))
    );
    steps.push(snap(&c, "fix_launched"));
    run_root(&c, &r2, true);
    c.settle_events();
    c.settle();
    assert_eq!(c.phase(), ControllerPhase::Achieved);
    assert_eq!(
        c.roadmap_reached("o1"),
        1,
        "the old record stays; not doubled"
    );
    assert_eq!(c.count_rule(&format!("{REVOKED}:o1")), 1);
    steps.push(snap(&c, "reachieved"));
    if let Some(dir) = std::env::var_os("PROJECT_REVOCATION_RAW") {
        let out = json!({
            "schema": "celeris.p4b-revocation-e2e/1",
            "steps": steps,
            "decisions": c.validations(),
        });
        std::fs::write(
            std::path::Path::new(&dir).join("e2e.json"),
            serde_json::to_string_pretty(&out).unwrap(),
        )
        .unwrap();
    }
}

/// G2: Roadmap の Outcome の checks の検証（数・空の command）は依頼文の block と同じ。checks だけを変えた版も
/// 新しい版で、人の承認（decide）までは approved の版は変わらない。
#[test]
fn project_revocation_roadmap_checks_validation_and_human_gate() {
    let c = lying_case(&["o1"]);
    let pid = c.pid();
    let now = c.env.now();
    let propose = |specs: Vec<RoadmapOutcomeSpec>, base: Option<i64>| {
        let input = RoadmapProposalInput {
            base_version: base,
            change_reason: "試験".into(),
            outcomes: specs,
        };
        c.store
            .write_tx(|tx| SqliteStore::roadmap_propose_tx(tx, &pid, &input, "sha", "cos", now))
    };
    let blank = propose(vec![spec("o1", &acc("o1"), &[" "])], None);
    assert!(
        matches!(blank, Err(task_core::store::RoadmapWriteError::Invalid(ref i)) if i.code == "roadmap_invalid"),
        "{blank:?}"
    );
    let nine: Vec<&str> = vec!["true"; 9];
    assert!(propose(vec![spec("o1", &acc("o1"), &nine)], None).is_err());
    let v1 = c.approve_specs(vec![spec("o1", &acc("o1"), &[])]);
    // checks だけを変えた提案は「変化なし」ではない。承認までは approved の版の checks は空のまま。
    let v2 = propose(vec![spec("o1", &acc("o1"), &["true"])], Some(v1)).unwrap();
    let approved = c.store.roadmap_approved(&pid).unwrap().unwrap();
    assert_eq!(approved.version, v1);
    assert!(approved.outcomes[0].checks.is_empty());
    c.decide_roadmap(v2.version).unwrap();
    let approved = c.store.roadmap_approved(&pid).unwrap().unwrap();
    assert_eq!(approved.outcomes[0].checks.len(), 1);
    // checks の無い版の JSON は前と同じ形（欄を出さない）。
    let v1_row = c.store.roadmap_version(&pid, v1).unwrap().unwrap();
    assert!(
        !serde_json::to_string(&v1_row.outcomes)
            .unwrap()
            .contains("checks")
    );
}

/// controller が起票した root で達成まで進めた案件（Roadmap の o1 は acceptance `acc("o1")`・checks `test -f out/o1.txt`）。
fn controller_achieved_case() -> (Case, Task) {
    let mut c = case(Some(ControllerMode::Enabled), planner_ts);
    c.set_request(&["o1"]);
    c.approve_specs(vec![spec("o1", &acc("o1"), &["test -f out/o1.txt"])]);
    c.settle_events();
    c.settle();
    let roots = c.controller_roots();
    assert_eq!(roots.len(), 1);
    let r1 = c.task(roots[0].id);
    run_root(&c, &r1, true);
    c.settle_events();
    c.settle();
    assert_eq!(c.phase(), ControllerPhase::Achieved);
    (c, r1)
}

/// 人の改版の後: 旧い root は o1 を名指していても根拠にならず（`stale_outcome_ids`）、達成は戻る。controller は
/// 新しい条件の root（2 本目）を起票し、その root の合格で再達成する。旧い達成の判断と `reached` の記録は残る。
fn assert_human_change_reopens_and_reachieves(
    mut c: Case,
    r1: &Task,
    new: RoadmapOutcomeSpec,
    expect_cmd: &str,
) {
    c.approve_specs(vec![new.clone()]);
    c.settle_events();
    c.settle();
    assert_ne!(
        c.phase(),
        ControllerPhase::Achieved,
        "{:?}",
        c.validations()
    );
    // 人の改版の wake で TS が評価され、o1 を未達と見て直す root を起票する（達成の後の判断）。
    let after = c.validations();
    let achieved_at = after.iter().rposition(|v| v == "pc/achieved").unwrap();
    assert!(
        after[achieved_at..].iter().any(|v| v == "pc/outcome-open"),
        "{after:?}"
    );
    assert!(c.count_rule("pc/achieved") >= 1, "the old decision stays");
    assert_eq!(c.roadmap_reached("o1"), 1);
    // 旧い root は done・合格のままだが、snapshot では o1 に対して stale。
    let snap = c.snapshot();
    let old = snap
        .roots
        .iter()
        .find(|r| r.id == r1.id.to_string())
        .unwrap();
    assert_eq!(old.status, "done");
    assert_eq!(old.stale_outcome_ids, vec!["o1".to_owned()]);
    let o1 = snap.outcomes.iter().find(|o| o.id == "o1").unwrap();
    assert!(!task_core::project_proposal::root_satisfies(old, o1));
    // TS が嘘をついて達成と申告しても、Rust の数え直し（到達の記録はあっても根拠の root が無い）は o1 を未達とする。
    let recorded: std::collections::BTreeSet<String> = ["o1".to_owned()].into();
    assert_eq!(
        task_core::project_proposal::unverified_outcomes(&snap, &recorded),
        vec!["o1".to_owned()]
    );
    // 新しい条件の root が起票される。
    let roots = c.controller_roots();
    assert_eq!(roots.len(), 2, "{:?}", c.validations());
    let r2 = c.task(roots[1].id);
    assert!(r2.acceptance.iter().any(|a| a.text == new.acceptance[0]));
    assert!(
        r2.acceptance
            .iter()
            .any(|a| matches!(&a.check, Check::Command { cmd, .. } if cmd == expect_cmd))
    );
    run_root(&c, &r2, true);
    c.settle_events();
    c.settle();
    assert_eq!(c.phase(), ControllerPhase::Achieved);
    let snap = c.snapshot();
    let fresh = snap
        .roots
        .iter()
        .find(|r| r.id == r2.id.to_string())
        .unwrap();
    assert!(fresh.stale_outcome_ids.is_empty());
}

/// 第 4 フェーズ P4-B（差し戻しの修正）: controller が起票した root（`outcome_ids` に o1）で達成の後、人が o1 の
/// **acceptance** を変えた版を承認すると達成は戻る（起票時の条件の digest が今の版と違う）。
#[test]
fn project_revocation_controller_root_acceptance_change_reopens() {
    let (c, r1) = controller_achieved_case();
    let new = spec("o1", "o1 の新しい条件が通る", &["test -f out/o1.txt"]);
    assert_human_change_reopens_and_reachieves(c, &r1, new, "test -f out/o1.txt");
}

/// 同じく、人が o1 の **checks だけ**を変えた版を承認すると達成は戻り、新しい command の root で再達成する。
#[test]
fn project_revocation_controller_root_checks_change_reopens() {
    let (c, r1) = controller_achieved_case();
    let new = spec("o1", &acc("o1"), &["test -f out/o1-v2.txt"]);
    assert_human_change_reopens_and_reachieves(c, &r1, new, "test -f out/o1-v2.txt");
}

/// 条件の変わらない改版（title・priority だけ）では、controller の root は根拠のままで達成は戻らない。
#[test]
fn project_revocation_controller_root_title_change_stays_achieved() {
    let (mut c, r1) = controller_achieved_case();
    let mut same = spec("o1", &acc("o1"), &["test -f out/o1.txt"]);
    same.title = "o1 の題を変えた".into();
    same.priority = 3;
    c.approve_specs(vec![same]);
    c.settle_events();
    c.settle();
    assert_eq!(
        c.phase(),
        ControllerPhase::Achieved,
        "{:?}",
        c.validations()
    );
    assert_eq!(c.count_rule(REVOKED), 0);
    let snap = c.snapshot();
    let old = snap
        .roots
        .iter()
        .find(|r| r.id == r1.id.to_string())
        .unwrap();
    assert!(old.stale_outcome_ids.is_empty());
    assert_eq!(c.controller_roots().len(), 1);
}
