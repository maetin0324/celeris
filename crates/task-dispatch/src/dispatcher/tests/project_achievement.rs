//! 第 3 フェーズ P0-B（ADR 2026-11-05-phase3-achievement-invariants・付記 2026-11-05-phase3-p0b）: 達成判定の
//! Rust の検証（接頭辞 `project_achievement_`）。
//!
//! TS の代わりに fake の script（`project_cos` の `FakeTs`）を置き、**TS は嘘をつく**（`gap` を偽って全 Outcome を
//! 到達と申告し、`wait(achieved)` を返す）。それでも Rust が snapshot と到達の記録から数え直し、根拠の無い達成を
//! 拒否することを確かめる（K1〜K11）。LLM は 0、時計は注入する。

use super::*;
use task_core::Criterion;
use task_core::project_wake::ProjectControllerDecision;
use task_core::repos::RepoRef;
use task_core::roadmap::{OutcomeStatus, RoadmapOutcomeSpec, RoadmapProposalInput};
use task_dispatch_achievement::*;

/// 第 4 フェーズ P4-B: 達成の後の根拠の失効（この module の嘘をつく TS と Case の helper を使う）。
#[path = "project_revocation.rs"]
mod project_revocation;

/// 試験の中だけで使う名前（`crate::project_decider_ts` の公開物）。
mod task_dispatch_achievement {
    pub(super) use crate::project_decider_ts::{
        ACHIEVED_UNVERIFIED_RULE, MILESTONE_RECORDED_RULE, OutcomeRecord, outcome_in_place,
        record_outcome,
    };
}

fn acc(id: &str) -> String {
    format!("{id} の検査が通る")
}

fn request(ids: &[&str]) -> String {
    let rows: Vec<Value> = ids
        .iter()
        .map(|id| json!({"id": id, "title": format!("{id} を仕上げる"), "acceptance": [acc(id)]}))
        .collect();
    format!(
        "達成判定の試験。\n```celeris-outcomes\n{}\n```\n",
        serde_json::to_string(&rows).unwrap()
    )
}

/// 嘘をつく TS: 根拠のある未記録の Outcome があれば `mark_milestone`、無ければ `gap` を偽って `wait(achieved)`。
fn lying_ts(s: &ProjectSnapshot) -> Value {
    for o in &s.outcomes {
        let marked = s.decisions.iter().any(|d| {
            d.kind == "mark_milestone"
                && d.effect_state != "failed"
                && d.outcome_id.as_deref() == Some(o.id.as_str())
        });
        if marked {
            continue;
        }
        if let Some(r) = s
            .roots
            .iter()
            .find(|r| task_core::project_proposal::root_satisfies(r, o))
        {
            return forged(
                s,
                json!({"kind": "mark_milestone", "outcome_id": o.id, "evidence_refs": [format!("task:{}", r.id)]}),
                "pc/milestone-reached",
            );
        }
    }
    achieved(s)
}

/// `gap` を偽る（全 Outcome を reached、open・unknown は空）。
fn forged(s: &ProjectSnapshot, action: Value, rule: &str) -> Value {
    let mut p = proposal(s, action, rule, &[]);
    p["gap"]["reached"] = json!(s.outcomes.iter().map(|o| o.id.clone()).collect::<Vec<_>>());
    p["gap"]["open"] = json!([]);
    p["gap"]["unknown"] = json!([]);
    p
}

fn achieved(s: &ProjectSnapshot) -> Value {
    forged(
        s,
        json!({"kind": "wait", "waiting_on": [], "next_wake_after_secs": 0, "phase": "achieved"}),
        "pc/achieved",
    )
}

impl Case {
    fn set_request(&mut self, ids: &[&str]) {
        let text = request(ids);
        self.store
            .write_tx::<_, task_core::StoreError, _>(|tx| {
                SqliteStore::project_set_text_tx(tx, self.p.id, None, Some(&text))
            })
            .unwrap();
    }

    fn pid(&self) -> String {
        self.p.id.to_string()
    }

    fn phase(&self) -> ControllerPhase {
        self.store
            .project_controller_state(&self.pid())
            .unwrap()
            .unwrap()
            .phase
    }

    fn last(&self) -> ProjectControllerDecision {
        self.decisions().into_iter().next().unwrap()
    }

    /// `outcome` の acceptance を持つ root を案件に置く（人の起票と同じ。controller の key は無い）。
    fn root(&self, project: ProjectId, outcome: &str, repo: bool) -> Task {
        let mut t = new_task(self.env.dir.path(), Check::Reviewer, 0);
        t.project_id = Some(project);
        t.acceptance = vec![
            Criterion {
                text: acc(outcome),
                check: Check::Reviewer,
            },
            Criterion {
                text: "公開の検査 script が 0 で終わる".into(),
                check: Check::Command {
                    cmd: "true".into(),
                    expect_exit: 0,
                },
            },
        ];
        if repo {
            t.repos = vec![RepoRef {
                repo_id: task_core::RepoId::new(),
                name: "r".into(),
            }];
        }
        t.status = Status::Running;
        self.store.create_task(&t, vec![]).unwrap();
        t
    }

    fn event(&self, t: &Task, e: Event) {
        self.store.append_event(t.id, &e).unwrap();
    }

    fn set_status(&self, t: &Task, to: Status, reason: &str) {
        let s = serde_json::to_value(to).unwrap();
        let conn = rusqlite::Connection::open(self.env.db()).unwrap();
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

    /// review を通して done にする（`command_pass` が偽なら command の検査は落ちたのに reviewer は合格）。
    fn review_done(&self, t: &Task, command_pass: bool) {
        self.set_status(t, Status::Reviewing, "worker_done");
        for (i, pass) in [(0usize, true), (1usize, command_pass)] {
            self.event(
                t,
                Event::ReviewVerdict {
                    run_id: "rev".into(),
                    criterion_idx: i,
                    pass,
                    reason: String::new(),
                },
            );
        }
        self.set_status(t, Status::Done, "review_pass");
    }

    fn deliver(&self, t: &Task) {
        self.event(
            t,
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
    }

    fn merged(&self, t: &Task) {
        self.event(
            t,
            Event::IntegrationMerged {
                repo_id: task_core::RepoId::new(),
                branch: "b".into(),
                head_sha: "0".repeat(40),
                actor: "test".into(),
            },
        );
    }

    /// 十分に評価させる（mark → 記録 → 次の評価、の連鎖を流す）。
    fn settle(&mut self) {
        for _ in 0..8 {
            self.wake();
        }
    }

    /// 到達の記録の効果（`RecordOutcome`）が applied の判断の数。
    fn records_applied(&self) -> usize {
        self.decisions()
            .iter()
            .filter(|d| {
                d.effect_state == EffectState::Applied && d.effect_json.contains("record_outcome")
            })
            .count()
    }

    /// Roadmap の `reached` の状態の記録の数。
    fn roadmap_reached(&self, outcome: &str) -> usize {
        self.store
            .roadmap_outcome_events(&self.pid())
            .unwrap()
            .iter()
            .filter(|e| {
                e.outcome_id == outcome
                    && e.kind == "status"
                    && e.status == Some(OutcomeStatus::Reached)
            })
            .count()
    }

    /// 人が Roadmap の版を提案して承認する（API の `post_decide` と同じく同じ tx で human_epoch を上げる）。
    fn approve_roadmap(&self, ids: &[&str]) -> Result<i64, task_core::store::RoadmapWriteError> {
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
        let v = self.store.write_tx(|tx| {
            SqliteStore::roadmap_propose_tx(tx, &pid, &input, "sha", "human:test", now)
        })?;
        self.decide_roadmap(v.version)
    }

    fn decide_roadmap(&self, version: i64) -> Result<i64, task_core::store::RoadmapWriteError> {
        let pid = self.pid();
        let now = self.env.now();
        self.store.write_tx(|tx| {
            let row =
                SqliteStore::roadmap_decide_tx(tx, &pid, version, true, None, "human:test", now)?;
            SqliteStore::project_controller_human_bump_tx(tx, &pid, now, 0)?;
            Ok(row.version)
        })
    }
}

fn lying_case(ids: &[&str]) -> Case {
    let mut c = case(Some(ControllerMode::Enabled), lying_ts);
    c.set_request(ids);
    c
}

fn assert_unverified(c: &Case, ids: &str) {
    assert_ne!(c.phase(), ControllerPhase::Achieved);
    let last = c.last();
    assert_eq!(last.validation, format!("{ACHIEVED_UNVERIFIED_RULE}:{ids}"));
    let rec: Value = serde_json::from_str(&last.proposal_json).unwrap();
    assert_eq!(
        rec["achievement"]["unverified"],
        json!(ids.split(',').collect::<Vec<_>>())
    );
}

/// K1・K12: repo 付きの root が review に合格して done でも、`IntegrationMerged` が無ければ達成しない。
/// 取り込まれたら mark が 1 件記録され、その後に達成する。
#[test]
fn project_achievement_unintegrated_root_is_not_achieved() {
    let mut c = lying_case(&["o1"]);
    let r = c.root(c.p.id, "o1", true);
    c.review_done(&r, true);
    c.event(
        &r,
        Event::DeliverySkipped {
            reason: task_core::DeliverySkipReason::NoCommits,
            detail: String::new(),
            head: None,
        },
    );
    c.settle();
    assert_unverified(&c, "o1");
    assert_eq!(c.records_applied(), 0);
    c.merged(&r);
    c.settle();
    assert_eq!(c.records_applied(), 1);
    assert_eq!(c.phase(), ControllerPhase::Achieved);
}

/// K1b・K2: TS が `gap` を偽って全 Outcome を到達と申告しても、根拠の root が無ければ達成しない。
#[test]
fn project_achievement_forged_gap_without_root_is_refused() {
    let mut c = lying_case(&["o1", "o2"]);
    c.settle();
    assert_unverified(&c, "o1,o2");
    assert_eq!(c.roots(), 0);
}

/// K3: reviewer は合格でも command の検査が落ちた root は根拠にならない（done でも合格に数えない）。
#[test]
fn project_achievement_command_check_fail_is_not_reached() {
    let mut c = lying_case(&["o1"]);
    let r = c.root(c.p.id, "o1", false);
    c.review_done(&r, false);
    c.deliver(&r);
    c.settle();
    assert_unverified(&c, "o1");
    assert_eq!(c.records_applied(), 0);
}

/// K4: 違う案件の root を根拠に指す mark は検証で落ち、状態を書かない。Roadmap の案件で効果を直接呼んでも
/// `roadmap_evidence_unknown` で拒否する。
#[test]
fn project_achievement_other_project_root_is_refused() {
    let mut c = lying_case(&["o1"]);
    c.approve_roadmap(&["o1"]).unwrap();
    // 別の案件（同じ DB）の root を作る。
    let foreign_project = pc_project(&c.store, None);
    let foreign = c.root(foreign_project.id, "o1", false);
    c.review_done(&foreign, true);
    c.deliver(&foreign);
    c.settle();
    assert_ne!(c.phase(), ControllerPhase::Achieved);
    assert_eq!(c.roadmap_reached("o1"), 0);
    // TS の検証を通らない経路（効果の DTO を直接）でも Rust が拒否する。
    let mut d = c.last();
    d.id = "forged-decision".into();
    let got = record_outcome(
        &c.store,
        &d,
        "o1",
        &[format!("task:{}", foreign.id)],
        c.env.now(),
    )
    .unwrap();
    assert_eq!(
        got,
        OutcomeRecord::Refused("refused:roadmap_evidence_unknown".into())
    );
    assert_eq!(c.roadmap_reached("o1"), 0);
}

/// K5: Outcome 3 件のうち 2 件だけ到達なら waiting（未達の id を残す）。
#[test]
fn project_achievement_partial_outcomes_wait() {
    let mut c = lying_case(&["o1", "o2", "o3"]);
    for o in ["o1", "o2"] {
        let r = c.root(c.p.id, o, false);
        c.review_done(&r, true);
        c.deliver(&r);
    }
    c.settle();
    assert_eq!(c.records_applied(), 2);
    assert_unverified(&c, "o3");
}

/// K6: 全 Outcome が I3 を満たせば、mark ごとに到達が 1 件ずつ記録され、最後に達成する（Roadmap の案件）。
/// K41: repo の無い調査の root は成果物（`ArtifactProduced`）で根拠になる。成果物が無ければ到達しない。
#[test]
fn project_achievement_all_outcomes_reached_roadmap() {
    let mut c = lying_case(&["o1", "o2"]);
    c.approve_roadmap(&["o1", "o2"]).unwrap();
    let r1 = c.root(c.p.id, "o1", false);
    c.review_done(&r1, true);
    c.deliver(&r1);
    let r2 = c.root(c.p.id, "o2", false);
    c.review_done(&r2, true);
    c.settle();
    // r2 は成果物が無い（K41 の陰性）: o2 は到達しない。
    assert_eq!(c.roadmap_reached("o1"), 1);
    assert_eq!(c.roadmap_reached("o2"), 0);
    assert_unverified(&c, "o2");
    c.deliver(&r2);
    c.settle();
    assert_eq!(c.roadmap_reached("o1"), 1);
    assert_eq!(c.roadmap_reached("o2"), 1);
    assert_eq!(c.records_applied(), 2);
    assert_eq!(c.phase(), ControllerPhase::Achieved);
    let view = task_ops::roadmap::view(&c.store, c.p.id, None)
        .unwrap()
        .unwrap();
    assert!(
        view.outcomes
            .iter()
            .all(|o| o.status == OutcomeStatus::Reached)
    );
}

/// K6（Roadmap の無い案件）: 適用済みの mark の判断の行が到達の正本。
#[test]
fn project_achievement_all_outcomes_reached_request_block() {
    let mut c = lying_case(&["o1", "o2"]);
    for o in ["o1", "o2"] {
        let r = c.root(c.p.id, o, false);
        c.review_done(&r, true);
        c.deliver(&r);
    }
    c.settle();
    assert_eq!(c.records_applied(), 2);
    assert_eq!(c.phase(), ControllerPhase::Achieved);
}

/// K7: 達成の後に人が approved の版を差し替えて Outcome を 1 件足すと、人の変更の wake で waiting に戻る。
/// 前の Outcome の到達の記録は 1 件のまま。
#[test]
fn project_achievement_roadmap_version_swap_reopens() {
    let mut c = lying_case(&["o1"]);
    c.approve_roadmap(&["o1"]).unwrap();
    let r = c.root(c.p.id, "o1", false);
    c.review_done(&r, true);
    c.deliver(&r);
    c.settle();
    assert_eq!(c.phase(), ControllerPhase::Achieved);
    c.approve_roadmap(&["o1", "o2"]).unwrap();
    c.settle();
    assert_unverified(&c, "o2");
    assert_eq!(c.roadmap_reached("o1"), 1);
    assert_eq!(c.records_applied(), 1);
}

/// K8: 同じ `IntegrationMerged`・`ReviewerPassed` が重複して届き、TS が同じ mark を繰り返しても、到達の記録は 1 件。
#[test]
fn project_achievement_duplicate_events_record_once() {
    // 毎回同じ mark を返す TS（`decided` を見ない）。
    let mut c = case(Some(ControllerMode::Enabled), |s| {
        match s.roots.iter().find(|r| {
            s.outcomes
                .iter()
                .any(|o| task_core::project_proposal::root_satisfies(r, o))
        }) {
            Some(r) => forged(
                s,
                json!({"kind": "mark_milestone", "outcome_id": "o1", "evidence_refs": [format!("task:{}", r.id)]}),
                "pc/milestone-reached",
            ),
            None => achieved(s),
        }
    });
    c.set_request(&["o1"]);
    c.approve_roadmap(&["o1"]).unwrap();
    let r = c.root(c.p.id, "o1", true);
    c.review_done(&r, true);
    for _ in 0..3 {
        c.merged(&r);
        c.event(
            &r,
            Event::ReviewerPassed {
                review_run: "rev".into(),
                repo_id: task_core::RepoId::new(),
                target_sha: "0".repeat(40),
                reviewed_sha: "0".repeat(40),
                criteria: vec![0, 1],
            },
        );
        c.settle();
    }
    assert_eq!(c.roadmap_reached("o1"), 1);
    assert_eq!(c.records_applied(), 1);
    assert!(
        c.decisions()
            .iter()
            .filter(|d| d.validation == MILESTONE_RECORDED_RULE)
            .count()
            >= 2
    );
}

/// K9（dispatcher まで）: done の root を reopen し 2 回目の review が落ちたら、前の合格を根拠に使わない。
#[test]
fn project_achievement_reopened_root_is_not_evidence() {
    let mut c = lying_case(&["o1"]);
    let r = c.root(c.p.id, "o1", false);
    c.deliver(&r);
    c.review_done(&r, true);
    c.set_status(&r, Status::Ready, "reopen");
    c.review_done(&r, false);
    c.set_status(&r, Status::Failed, "review_fail");
    c.settle();
    assert_unverified(&c, "o1");
    assert_eq!(c.records_applied(), 0);
}

/// K10: 同じ版を土台にした 2 つの承認は CAS で 1 つだけ通る。承認（人の操作）の前の判断の効果は、書き込みの tx の
/// 門で止まり状態を書かない。
#[test]
fn project_achievement_concurrent_approval_cas() {
    let mut c = lying_case(&["o1"]);
    c.approve_roadmap(&["o1"]).unwrap();
    let pid = c.pid();
    let now = c.env.now();
    let base = c.store.roadmap_approved(&pid).unwrap().map(|v| v.version);
    let propose = |reason: &str| {
        let input = RoadmapProposalInput {
            base_version: base,
            change_reason: reason.into(),
            outcomes: vec![RoadmapOutcomeSpec {
                id: "o1".into(),
                title: "o1".into(),
                acceptance: vec![acc("o1")],
                priority: 0,
                depends_on: Vec::new(),
                dropped: false,
                checks: Vec::new(),
            }],
        };
        c.store
            .write_tx(|tx| SqliteStore::roadmap_propose_tx(tx, &pid, &input, "sha", "human:a", now))
            .unwrap()
            .version
    };
    let (a, b) = (propose("a"), propose("b"));
    let r = c.root(c.p.id, "o1", false);
    c.review_done(&r, true);
    c.deliver(&r);
    // controller の判断（mark）を記録だけ作る: 1 回評価して、その判断を取り出す。
    c.wake();
    let mark = c
        .decisions()
        .into_iter()
        .find(|d| d.effect_json.contains("record_outcome"))
        .unwrap();
    assert!(c.decide_roadmap(a).is_ok());
    assert!(matches!(
        c.decide_roadmap(b),
        Err(task_core::store::RoadmapWriteError::Stale { .. })
    ));
    // 承認で human_epoch が進んだ後に古い判断の効果をもう一度当てても書かない（門は同じ tx で閉じている）。
    let before = c.roadmap_reached("o1");
    let mut stale = mark.clone();
    stale.id = "stale-decision".into();
    let got = record_outcome(&c.store, &stale, "o1", &[format!("task:{}", r.id)], now).unwrap();
    assert_eq!(got, OutcomeRecord::GateClosed);
    assert_eq!(c.roadmap_reached("o1"), before);
    assert!(before <= 1);
}

/// K11: 効果の途中（記録の後、判断を applied にする前）で daemon が止まって同じ判断を当て直しても、記録は 1 件。
/// 再起動の後は `outcome_in_place` が記録を見つけて applied にする。
#[test]
fn project_achievement_restart_records_once() {
    let mut c = lying_case(&["o1"]);
    c.approve_roadmap(&["o1"]).unwrap();
    let r = c.root(c.p.id, "o1", false);
    c.review_done(&r, true);
    c.deliver(&r);
    // 判断だけを作る（効果を当てる前の状態を模すため、記録の後に判断の行を pending のまま読む）。
    c.wake();
    let mark = c
        .decisions()
        .into_iter()
        .find(|d| d.effect_json.contains("record_outcome"))
        .unwrap();
    let evidence = [format!("task:{}", r.id)];
    let first = record_outcome(&c.store, &mark, "o1", &evidence, c.env.now()).unwrap();
    let again = record_outcome(&c.store, &mark, "o1", &evidence, c.env.now()).unwrap();
    assert!(matches!(first, OutcomeRecord::Recorded(_)));
    assert!(matches!(again, OutcomeRecord::Recorded(ref r) if r.starts_with("roadmap:already:")));
    assert_eq!(c.roadmap_reached("o1"), 1);
    assert!(
        outcome_in_place(&c.store, &c.pid(), &mark.id)
            .unwrap()
            .is_some()
    );
    // daemon を作り直して（別の owner）評価を続けても増えない。
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
    c.settle();
    assert_eq!(c.roadmap_reached("o1"), 1);
    assert_eq!(c.phase(), ControllerPhase::Achieved);
}

/// decide と commit の間に起きる、根拠の root への割り込み。
#[derive(Clone, Copy)]
enum Interrupt {
    /// done の root を ready に戻す（`Transitioned` done → ready）。
    Reopen,
    /// 取り込みを取り消す（`IntegrationReopened`。状態は done のまま）。
    IntegrationReopened,
}

/// 到達の記録（mark の効果）が applied になった後の最初の `BeforeCommit`（= `wait(achieved)` の判断の後・tx1 の前）で、
/// 根拠の root に `what` を 1 回だけ起こす。8 回評価させ、tick ごとの `stale` を集めて返す。
fn race_achieved(c: &mut Case, r: &Task, what: Interrupt) -> Vec<String> {
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let store = Arc::clone(&c.store);
        let db = c.env.db();
        let pid = c.pid();
        let tid = r.id;
        let fired = Arc::clone(&fired);
        c.d.set_project_controller_hook(Box::new(move |b| {
            use std::sync::atomic::Ordering::SeqCst;
            if b != Boundary::BeforeCommit || fired.load(SeqCst) {
                return;
            }
            let recorded = store
                .project_controller_decisions(&pid, None, 100)
                .unwrap()
                .iter()
                .any(|d| {
                    d.effect_state == EffectState::Applied
                        && d.effect_json.contains("record_outcome")
                });
            if !recorded {
                return;
            }
            fired.store(true, SeqCst);
            match what {
                Interrupt::Reopen => {
                    rusqlite::Connection::open(&db)
                        .unwrap()
                        .execute(
                            "UPDATE tasks SET status = 'ready', \
                             json = json_set(json, '$.status', 'ready') WHERE id = ?1",
                            [tid.to_string()],
                        )
                        .unwrap();
                    store
                        .append_event(
                            tid,
                            &Event::Transitioned {
                                from: Status::Done,
                                to: Status::Ready,
                                reason: "reopen".into(),
                            },
                        )
                        .unwrap();
                }
                Interrupt::IntegrationReopened => {
                    store
                        .append_event(
                            tid,
                            &Event::IntegrationReopened {
                                repo_id: task_core::RepoId::new(),
                                branch: "b".into(),
                                head_sha: "0".repeat(40),
                                reason: Some("reverted".into()),
                                actor: "human:test".into(),
                            },
                        )
                        .unwrap();
                }
            }
        }));
    }
    let mut stale = Vec::new();
    for _ in 0..8 {
        c.wake();
        stale.extend(c.d.project_controller_last().unwrap().stale);
    }
    assert!(
        fired.load(std::sync::atomic::Ordering::SeqCst),
        "the race was injected"
    );
    stale
}

/// achieved を書いた判断（TS の `pc/achieved` がそのまま commit された判断）の数。
fn achieved_decisions(c: &Case) -> usize {
    c.decisions()
        .iter()
        .filter(|d| d.validation == "pc/achieved")
        .count()
}

/// K9（競合）: `wait(achieved)` の判断の後・commit の前に根拠の root が reopen されたら、commit の tx が root の
/// event の位置を照合して `Stale(Evidence)` にし、達成を書かない。次の評価は reopen を見て waiting にする。
#[test]
fn project_achievement_reopen_between_decide_and_commit_is_stale() {
    let mut c = lying_case(&["o1"]);
    let r = c.root(c.p.id, "o1", false);
    c.review_done(&r, true);
    c.deliver(&r);
    let stale = race_achieved(&mut c, &r, Interrupt::Reopen);
    assert!(stale.contains(&"rejected:stale_evidence".to_string()));
    assert_eq!(achieved_decisions(&c), 0);
    assert_unverified(&c, "o1");
    assert_eq!(c.records_applied(), 1);
}

/// K9（競合・取り込み）: 同じ窓で `IntegrationReopened` が来ても達成を書かない。取り込み直したら達成し、
/// 到達の記録は 1 件のまま。
#[test]
fn project_achievement_integration_reopened_between_decide_and_commit_is_stale() {
    let mut c = lying_case(&["o1"]);
    let r = c.root(c.p.id, "o1", true);
    c.review_done(&r, true);
    c.merged(&r);
    let stale = race_achieved(&mut c, &r, Interrupt::IntegrationReopened);
    assert!(stale.contains(&"rejected:stale_evidence".to_string()));
    assert_eq!(achieved_decisions(&c), 0);
    assert_unverified(&c, "o1");
    c.merged(&r);
    c.settle();
    assert_eq!(c.phase(), ControllerPhase::Achieved);
    assert_eq!(achieved_decisions(&c), 1);
    assert_eq!(c.records_applied(), 1);
}

/// 第 3 フェーズ P2-J: `GET …/controller` の `summary`（`task_ops::project_controller_view::summary`）を読む。
fn summary_of(c: &Case) -> task_core::project_wake::ProjectControllerSummary {
    let state = c.store.project_controller_state(&c.pid()).unwrap().unwrap();
    let last = state
        .last_decision_id
        .as_deref()
        .and_then(|id| c.store.project_controller_decision(id).unwrap());
    let pending = c.store.project_wakes_pending(&c.pid()).unwrap();
    task_ops::project_controller_view::summary(
        &c.store,
        c.p.id,
        &state,
        last.as_ref(),
        &pending,
        &task_ops::project_budget::BudgetLimits::default(),
        c.env.now(),
    )
    .unwrap()
}

/// P2-J: 一部の Outcome だけ根拠と記録がそろった案件では、summary が達成（o1・o2）と未達（o3）を分け、
/// 待機の理由に判定と同じ規則（`pc/achieved-unverified:o3`）と未達の id を出す。trace は最後の判断の写し。
#[test]
fn project_controller_summary_shows_reached_unreached_and_wait_reason() {
    let mut c = lying_case(&["o1", "o2", "o3"]);
    let mut roots = Vec::new();
    for o in ["o1", "o2"] {
        let r = c.root(c.p.id, o, false);
        c.review_done(&r, true);
        c.deliver(&r);
        roots.push(r.id.to_string());
    }
    c.settle();
    assert_unverified(&c, "o3");
    let s = summary_of(&c);
    let by_id = |id: &str| s.outcomes.iter().find(|o| o.id == id).unwrap().clone();
    for (i, id) in ["o1", "o2"].into_iter().enumerate() {
        let o = by_id(id);
        assert!(o.reached && o.recorded, "{o:?}");
        assert_eq!(o.evidence_root_ids, vec![roots[i].clone()]);
        assert_eq!(o.source, "request_block");
    }
    let o3 = by_id("o3");
    assert!(!o3.reached && !o3.recorded && o3.evidence_root_ids.is_empty());
    assert!(!s.outcomes_invalid);
    let w = s.waiting.expect("waiting reason");
    assert_eq!(w.phase, ControllerPhase::Waiting);
    assert_eq!(w.kind, "achievement_unverified");
    assert_eq!(w.rule_id.as_deref(), Some("pc/achieved-unverified:o3"));
    assert!(
        w.details.iter().any(|d| d.contains("\"o3\"")),
        "{:?}",
        w.details
    );
    let last = c.last();
    let t = s.trace.expect("trace");
    assert_eq!(t.decision_id, last.id);
    assert_eq!(t.rule_id, last.validation);
    assert_eq!(w.decision_id.as_deref(), Some(last.id.as_str()));
    // 次の再評価は判断が置いた timer（state の next_wake_at）と同じ。
    let state = c.store.project_controller_state(&c.pid()).unwrap().unwrap();
    assert_eq!(s.next_wake.at, state.next_wake_at);
    assert!(s.next_wake.at.is_some());
    assert_eq!(
        s.budget_limits,
        task_core::project_wake::ControllerBudgetLimits::from(
            &task_ops::project_budget::BudgetLimits::default()
        )
    );
}

/// P2-J: 全 Outcome が達成した案件（phase `achieved`）では、全件 `reached` で待機の理由は無い。
#[test]
fn project_controller_summary_all_reached_has_no_wait_reason() {
    let mut c = lying_case(&["o1", "o2"]);
    for o in ["o1", "o2"] {
        let r = c.root(c.p.id, o, false);
        c.review_done(&r, true);
        c.deliver(&r);
    }
    c.settle();
    assert_eq!(c.phase(), ControllerPhase::Achieved);
    let s = summary_of(&c);
    assert_eq!(s.outcomes.len(), 2);
    assert!(s.outcomes.iter().all(|o| o.reached));
    assert!(s.waiting.is_none());
    assert!(s.trace.is_some());
}
