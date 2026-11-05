//! P1-G（ADR 2026-11-05-phase3-p1g、P0-A の K29〜K40）: 自律復帰の確定試験（接頭辞 `project_recovery_`）。
//!
//! 一時 DB・注入した時計・fake の script だけで動く（LLM なし、外部ネットワークなし、本番に触れない）。
//! `PROJECT_RECOVERY_RAW_DIR` があれば `recovery.jsonl`（`celeris.long-horizon-record/1`）を書く
//! （`scripts/dev/test-project-recovery.sh` が検査する）。

use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};
use task_core::authority as a;
use task_core::project_wake::ProjectControllerDecision;
use task_core::{ProjectBrief, ProjectBriefInput};
use task_ops::project_budget::BudgetLimits;

fn raw(value: Value) {
    let Some(dir) = std::env::var_os("PROJECT_RECOVERY_RAW_DIR") else {
        return;
    };
    let mut v = value;
    v["schema"] = json!("celeris.long-horizon-record/1");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join("recovery.jsonl"))
        .unwrap();
    use std::io::Write as _;
    f.write_all(format!("{v}\n").as_bytes()).unwrap();
}

fn settings(f: impl FnOnce(&mut BudgetLimits)) -> ProjectControllerSettings {
    let mut s = ProjectControllerSettings::default();
    f(&mut s.budget);
    s
}

fn wait_action(s: &ProjectSnapshot, kind: &str) -> Value {
    proposal(
        s,
        json!({"kind": "wait", "waiting_on": [{"kind": kind, "note": "待ち"}],
               "next_wake_after_secs": 86400, "phase": "waiting"}),
        "pc/human-wait",
        &[],
    )
}

fn human_wait(s: &ProjectSnapshot) -> Value {
    wait_action(s, "human")
}

fn external_wait(s: &ProjectSnapshot) -> Value {
    wait_action(s, "external")
}

/// 呼ぶたびに別の CoS 入力（replan）。回数の上限の試験。
fn replan_each(s: &ProjectSnapshot) -> Value {
    replan(s, "next", &[format!("project:{}", s.project.id)], "unknown")
}

fn brief_put(c: &Case, state: &str) {
    brief_put_on(&c.store, c.p.id, state, c.env.now());
}

fn brief_put_on(store: &SqliteStore, id: ProjectId, state: &str, now: OffsetDateTime) {
    store
        .project_brief_put(&ProjectBrief::from_input(
            id,
            ProjectBriefInput {
                objective: "目的".into(),
                state: state.into(),
                open_issues: Vec::new(),
                next_actions: Vec::new(),
                waiting_on: Vec::new(),
                evidence_refs: Vec::new(),
            },
            "cos",
            now,
        ))
        .unwrap();
}

fn external(c: &Case, key: &str) {
    c.store
        .project_wake_ingest(
            &[NewProjectWake {
                project_id: c.p.id.to_string(),
                wake_key: format!("external:{key}"),
                kind: ProjectWakeKind::ExternalReleased,
                source_event_id: None,
                source_ref: Some(key.into()),
            }],
            None,
            0,
            c.env.now(),
        )
        .unwrap();
}

fn state(c: &Case) -> task_core::project_wake::ProjectControllerState {
    c.store
        .project_controller_state(&c.p.id.to_string())
        .unwrap()
        .unwrap()
}

fn phase(c: &Case) -> ControllerPhase {
    state(c).phase
}

fn spin(c: &mut Case, ticks: usize) {
    for _ in 0..ticks {
        c.env.advance(1);
        tick(&mut c.d);
    }
}

/// 時計を `hours` 時間、1 時間ずつ進める。止まった案件の時間経過の試験。
fn hours(c: &mut Case, hours: i64) {
    for _ in 0..hours {
        c.env.advance(3600);
        tick(&mut c.d);
    }
}

fn fail_root(c: &Case, reason: &str) -> Task {
    let t = root_task(&c.env, &c.store, &c.p, Status::Running);
    transition(&c.store, &t, Status::Failed, reason);
    t
}

fn rule_count(c: &Case, rule: &str) -> usize {
    c.store
        .project_controller_decisions(&c.p.id.to_string(), None, 1000)
        .unwrap()
        .iter()
        .filter(|d| d.validation == rule)
        .count()
}

fn resumes(c: &Case, rule: &str) -> Vec<ProjectControllerDecision> {
    c.store
        .project_controller_decisions(&c.p.id.to_string(), None, 1000)
        .unwrap()
        .into_iter()
        .filter(|d| d.validation == rule)
        .collect()
}

fn human(c: &Case) {
    human_bump(&c.store, &c.p.id.to_string(), c.env.now());
}

fn notices_like(c: &Case, pattern: &str) -> i64 {
    rusqlite::Connection::open(c.env.db())
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM feed_sources WHERE source_key LIKE ?1",
            [pattern],
            |r| r.get(0),
        )
        .unwrap()
}

fn grant(c: &Case, question: Option<&str>, class: a::ActionClass) {
    let project = c.p.id.to_string();
    let now = c.env.now();
    c.store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            SqliteStore::authority_grant_insert_tx(
                tx,
                &a::NewAuthorityGrant {
                    id: ulid::Ulid::new().to_string(),
                    project_id: project.clone(),
                    scope: a::GrantScope {
                        kind: a::ScopeKind::Project,
                        reference: project.clone(),
                    },
                    action_class: class,
                    source: a::GrantSource {
                        kind: a::SourceKind::HumanOperation,
                        reference: format!("project.controller_put:{project}:1"),
                    },
                    granted_by: "human:test".into(),
                    question_key: question.map(str::to_owned),
                    note: "人の許可".into(),
                    expires_at: (now + time::Duration::days(30)).format(&Rfc3339).unwrap(),
                    budget: 5,
                },
                now,
            )?
            .unwrap();
            SqliteStore::authority_unblock_tx(tx, &project, now, 0)?;
            Ok(())
        })
        .unwrap();
}

/// 人が問いの提案を断った状態（`POST …/dismiss` と同じ行と wake。API の経路は task-api の試験が見る）。
fn dismissed(c: &Case, question: &str) {
    let project = c.p.id.to_string();
    let now = c.env.now();
    let at = now.format(&Rfc3339).unwrap();
    c.store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            tx.execute(
                "INSERT INTO authority_proposals (id, project_id, question_key, scope_kind, scope_ref, \
                   action_class, source_kind, source_ref, expires_at, budget, proposed_by, state, \
                   resolved_by, resolved_at, created_at) \
                 VALUES (?1, ?2, ?3, 'project', ?2, 'launch_root', 'human_message', 'm', ?4, 1, 'cos', \
                   'dismissed', 'human:test', ?4, ?4)",
                rusqlite::params![ulid::Ulid::new().to_string(), project, question, at],
            )?;
            SqliteStore::project_controller_human_bump_tx(tx, &project, now, 0)?;
            Ok(())
        })
        .unwrap();
}

fn without_grant(f: impl Fn(&ProjectSnapshot) -> Value + Send + Sync + 'static) -> Case {
    let c = case(Some(ControllerMode::Shadow), f);
    c.store
        .project_controller_set_mode(
            &c.p.id.to_string(),
            ControllerMode::Enabled,
            None,
            "human:test",
            c.env.now(),
        )
        .unwrap();
    c
}

fn seed_tokens(c: &Case, t: &Task, run: &str, tokens: u64) {
    let at = c.env.now().format(&Rfc3339).unwrap();
    rusqlite::Connection::open(c.env.db())
        .unwrap()
        .execute(
            "INSERT INTO runs (run_id, task_id, role, seq, status, usage_json, started_at) \
             VALUES (?1, ?2, 'worker', 1, 'completed', ?3, ?4)",
            rusqlite::params![
                run,
                t.id.to_string(),
                format!("{{\"input_tokens\":{tokens},\"output_tokens\":0}}"),
                at
            ],
        )
        .unwrap();
}

/// 次の UTC 00:00 の 1 秒後へ時計を進める。
fn next_day(c: &mut Case) {
    let secs = task_ops::project_budget::secs_until_next_day(c.env.now());
    c.env.advance(secs as i64 + 1);
}

// ---- K29・K30: crash と再配送 ----

#[tokio::test]
async fn project_recovery_k29_brief_wake_survives_crash_at_each_stage() {
    let stages = [
        Boundary::BeforeIngest,
        Boundary::AfterIngest,
        Boundary::AfterResume,
        Boundary::AfterLease,
        Boundary::BeforeCommit,
        Boundary::AfterCommit,
        Boundary::AfterEffect,
        Boundary::AfterApplied,
    ];
    for boundary in stages {
        let env = Env::new();
        let p;
        {
            let store = env.open();
            p = pc_project(&store, Some(ControllerMode::Enabled));
            let script = Arc::new(FakeTs {
                f: Arc::new(launch),
                seen: Arc::new(StdMutex::new(Vec::new())),
            });
            let mut d = ts_daemon(
                &env,
                &store,
                TsProjectDecider::with_fixed_script(DeciderLimits::default(), script),
                ProjectControllerSettings::default(),
            );
            prime(&mut d);
            brief_put_on(&store, p.id, "1", env.now());
            env.advance(31);
            d.set_project_controller_hook(Box::new(move |b| {
                if b == boundary {
                    panic!("crash at {b:?}");
                }
            }));
            assert!(
                catch_unwind(AssertUnwindSafe(|| tick(&mut d))).is_err(),
                "{boundary:?}: the hook did not fire"
            );
        }
        let store = env.open();
        let script = Arc::new(FakeTs {
            f: Arc::new(launch),
            seen: Arc::new(StdMutex::new(Vec::new())),
        });
        let mut d = ts_daemon(
            &env,
            &store,
            TsProjectDecider::with_fixed_script(DeciderLimits::default(), script),
            ProjectControllerSettings::default(),
        );
        for _ in 0..4 {
            env.advance(121);
            tick(&mut d);
        }
        let wakes = store.project_wakes_all(&p.id.to_string()).unwrap();
        let brief_wakes = wakes
            .iter()
            .filter(|w| w.wake_key.starts_with("brief:"))
            .count();
        assert_eq!(brief_wakes, 1, "{boundary:?}");
        assert_eq!(controller_roots(&store, &p), 1, "{boundary:?}");
        assert_eq!(pending_wakes(&store, &p), 0, "{boundary:?}");
        assert_eq!(store.project_controller_pending_effects().unwrap().len(), 0);
        raw(
            json!({"test": "project_recovery_k29", "boundary": format!("{boundary:?}"),
                   "brief_wakes": brief_wakes, "roots": 1, "pending": 0}),
        );
    }
}

#[tokio::test]
async fn project_recovery_k29_resume_survives_crash_right_after_its_transaction() {
    let mut c = case(Some(ControllerMode::Enabled), |s: &ProjectSnapshot| {
        escalate(s, "scope")
    });
    c.wake();
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    grant(&c, Some("scope"), a::ActionClass::LaunchRoot);
    c.d.set_project_controller_hook(Box::new(|b| {
        if b == Boundary::AfterResume {
            panic!("crash after resume");
        }
    }));
    c.env.advance(31);
    assert!(catch_unwind(AssertUnwindSafe(|| tick(&mut c.d))).is_err());
    // 再起動（新しい instance）: 再開の行は 1 件だけで、再起動の後の評価は 1 回。
    let script = Arc::new(FakeTs {
        f: Arc::new(|s: &ProjectSnapshot| escalate(s, "scope")),
        seen: c.seen.clone(),
    });
    c.d = ts_daemon(
        &c.env,
        &c.store,
        TsProjectDecider::with_fixed_script(DeciderLimits::default(), script),
        ProjectControllerSettings::default(),
    );
    let calls = c.calls();
    spin(&mut c, 3);
    assert_eq!(resumes(&c, "pc/resume/question").len(), 1);
    assert_eq!(c.calls(), calls + 1);
    assert_eq!(c.escalations(), 1);
    raw(
        json!({"test": "project_recovery_k29_resume", "resume_rows": 1, "evaluations_after_restart": 1}),
    );
}

#[tokio::test]
async fn project_recovery_k30_same_event_twice_is_one_wake_and_one_evaluation() {
    let mut c = case(Some(ControllerMode::Enabled), human_wait);
    let before = c.store.latest_event_id().unwrap();
    let t = root_task(&c.env, &c.store, &c.p, Status::Running);
    transition(&c.store, &t, Status::Failed, "worker_error");
    spin(&mut c, 2);
    let calls = c.calls();
    let wakes = c
        .store
        .project_wakes_all(&c.p.id.to_string())
        .unwrap()
        .len();
    // 全体の cursor を戻して同じ event をもう一度配る。
    c.store
        .feed_cursor_set(
            crate::dispatcher::cron_tick::project_wake::PROJECT_WAKE_CURSOR,
            &before.to_string(),
        )
        .unwrap();
    spin(&mut c, 3);
    assert_eq!(
        c.store
            .project_wakes_all(&c.p.id.to_string())
            .unwrap()
            .len(),
        wakes
    );
    assert_eq!(c.calls(), calls);
    // brief の同じ内容の再送も同じ（時刻だけが違う）。
    brief_put(&c, "同じ");
    spin(&mut c, 2);
    let after_brief = c.calls();
    c.env.advance(3 * 86400);
    brief_put(&c, "同じ");
    assert_eq!(pending_wakes(&c.store, &c.p), 0);
    raw(
        json!({"test": "project_recovery_k30", "wakes": wakes, "evaluations_for_event": calls,
               "evaluations_with_brief": after_brief}),
    );
}

// ---- K31: 3 日の時計飛び ----

#[tokio::test]
async fn project_recovery_k31_three_day_clock_jump_folds_into_one_evaluation() {
    let mut c = case(Some(ControllerMode::Enabled), human_wait);
    reconcile_job(&c.store, &c.p);
    c.wake();
    assert_eq!(c.calls(), 1);
    // suspend の間に root の終わり・brief・外の応答が積もる。
    for n in 0..5 {
        let t = root_task(&c.env, &c.store, &c.p, Status::Done);
        transition(&c.store, &t, Status::Done, "review_pass");
        brief_put(&c, &format!("{n}"));
        external(&c, &format!("e{n}"));
    }
    c.env.advance(3 * 86400);
    let cron_before = c
        .store
        .project_wakes_all(&c.p.id.to_string())
        .unwrap()
        .len();
    for _ in 0..4 {
        c.d.tick().unwrap();
        c.env.advance(31);
    }
    assert_eq!(c.calls(), 2, "the jump is folded into one evaluation");
    assert_eq!(c.escalations(), 0);
    assert_eq!(c.roots(), 0);
    assert_eq!(pending_wakes(&c.store, &c.p), 0);
    assert_eq!(rule_count(&c, "pc/guard/ttl"), 0);
    raw(
        json!({"test": "project_recovery_k31", "virtual_hours_jumped": 72,
               "evaluations_after_jump": 1, "wakes_before_cron": cron_before,
               "questions": 0, "roots": 0}),
    );
}

// ---- K32: 同じ失敗 ----

#[tokio::test]
async fn project_recovery_k32_same_failure_stops_again_after_human_clear() {
    let mut c = case(Some(ControllerMode::Enabled), human_wait);
    for n in 0..3 {
        fail_root(&c, &format!("check failed: exit 101 after {n}2s"));
        c.env.advance(60);
        tick(&mut c.d);
    }
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(c.escalations(), 1);
    let calls = c.calls();
    // 止まっている間の同じ失敗・時間経過・brief・外の入力では解けない。
    fail_root(&c, "check failed: exit 101 after 99s");
    brief_put(&c, "CoS の要約");
    external(&c, "unrelated");
    hours(&mut c, 48);
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(c.calls(), calls);
    assert_eq!(c.escalations(), 1, "the question is asked once");
    // 人が解消する: 根拠の行を残して 1 回だけ再評価する。
    human(&c);
    spin(&mut c, 3);
    assert_eq!(c.calls(), calls + 1);
    let r = resumes(&c, "pc/resume/same_failure");
    assert_eq!(r.len(), 1);
    let rec: Value = serde_json::from_str(&r[0].proposal_json).unwrap();
    assert_eq!(rec["basis"]["reason"], "cause_resolved");
    assert_eq!(rec["schema"], "celeris.project-resume/1");
    assert_ne!(phase(&c), ControllerPhase::Blocked);
    // また同じ失敗が上限まで続けば、もう一度止まり、新しい問いを 1 回だけ出す。
    for n in 0..3 {
        fail_root(&c, &format!("check failed: exit 101 after {n}7s"));
        c.env.advance(60);
        tick(&mut c.d);
    }
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(c.escalations(), 2);
    assert_eq!(c.roots(), 0, "no controller root grew");
    raw(
        json!({"test": "project_recovery_k32", "failures": 7, "questions": c.escalations(),
               "resumes": 1, "roots": 0, "evaluations": c.calls()}),
    );
}

#[tokio::test]
async fn project_recovery_k32_failed_root_completing_clears_the_block() {
    let mut c = case(Some(ControllerMode::Enabled), human_wait);
    let mut last = None;
    for n in 0..3 {
        last = Some(fail_root(&c, &format!("check failed: exit 1 after {n}s")));
        c.env.advance(60);
        tick(&mut c.d);
    }
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    // 別の root の完了は原因の解消ではない。
    let other = root_task(&c.env, &c.store, &c.p, Status::Done);
    transition(&c.store, &other, Status::Done, "review_pass");
    spin(&mut c, 3);
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    // 失敗した root 自身がやり直しで完了した。
    let failed = last.unwrap();
    rusqlite::Connection::open(c.env.db())
        .unwrap()
        .execute(
            "UPDATE tasks SET status = 'done', json = json_set(json, '$.status', 'done') WHERE id = ?1",
            [failed.id.to_string()],
        )
        .unwrap();
    transition(&c.store, &failed, Status::Done, "review_pass");
    spin(&mut c, 3);
    assert_eq!(resumes(&c, "pc/resume/same_failure").len(), 1);
    assert_ne!(phase(&c), ControllerPhase::Blocked);
    raw(json!({"test": "project_recovery_k32_root_done", "resumes": 1}));
}

// ---- K33: brief と authority の同時編集 ----

#[tokio::test]
async fn project_recovery_k33_concurrent_brief_and_authority_edits_invalidate_the_decision() {
    for boundary in [Boundary::BeforeCommit, Boundary::AfterCommit] {
        let mut c = case(Some(ControllerMode::Enabled), launch);
        let store = c.store.clone();
        let project = c.p.clone();
        let clock = c.env.clock.clone();
        let mut done = false;
        c.d.set_project_controller_hook(Box::new(move |b| {
            if b != boundary || done {
                return;
            }
            done = true;
            let now = *clock.lock().unwrap();
            let id = project.id.to_string();
            brief_put_on(&store, project.id, "人が編集した", now);
            store
                .write_tx::<_, task_core::StoreError, _>(|tx| {
                    SqliteStore::authority_unblock_tx(tx, &id, now, 0)
                })
                .unwrap();
        }));
        let epoch = state(&c).human_epoch;
        c.wake();
        // 古い判断の効果は 1 つも通らない。
        assert_eq!(c.roots(), 0, "{boundary:?}");
        assert_eq!(state(&c).human_epoch, epoch + 2, "{boundary:?}");
        let wakes = c.store.project_wakes_all(&c.p.id.to_string()).unwrap();
        assert_eq!(
            wakes
                .iter()
                .filter(|w| w.wake_key.starts_with("brief:"))
                .count(),
            1
        );
        assert_eq!(
            wakes
                .iter()
                .filter(|w| w.wake_key.starts_with("human:"))
                .count(),
            1
        );
        // 次の評価は最新の版を読み、root を 1 件だけ作る。
        spin(&mut c, 3);
        assert_eq!(c.roots(), 1, "{boundary:?}");
        assert_eq!(pending_wakes(&c.store, &c.p), 0, "{boundary:?}");
        let seen = c
            .seen
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .controller
            .human_epoch;
        assert_eq!(seen as i64, epoch + 2, "{boundary:?}");
        raw(
            json!({"test": "project_recovery_k33", "boundary": format!("{boundary:?}"),
                   "roots_from_old_decision": 0, "roots_after_reevaluation": 1, "wakes_per_edit": 1}),
        );
    }
}

// ---- K34・K35: clear と deny ----

#[tokio::test]
async fn project_recovery_k34_answer_clears_once_and_records_the_basis() {
    let mut c = case(Some(ControllerMode::Enabled), |s: &ProjectSnapshot| {
        escalate(s, "deploy-window")
    });
    c.wake();
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    let calls = c.calls();
    hours(&mut c, 30);
    assert_eq!(c.calls(), calls, "time alone does not clear");
    grant(&c, Some("deploy-window"), a::ActionClass::LaunchRoot);
    spin(&mut c, 4);
    assert_eq!(c.calls(), calls + 1, "one re-evaluation");
    let r = resumes(&c, "pc/resume/question");
    assert_eq!(r.len(), 1);
    let rec: Value = serde_json::from_str(&r[0].proposal_json).unwrap();
    assert_eq!(rec["basis"]["reason"], "authority_answered");
    assert_eq!(rec["basis"]["question_key"], "deploy-window");
    assert!(rec["basis"]["grant_id"].is_string());
    assert_eq!(r[0].effect_state, EffectState::None);
    assert_eq!(c.escalations(), 1);
    assert_eq!(c.decisions()[0].validation, "pc/authority-answered");
    assert_eq!(phase(&c), ControllerPhase::Waiting);
    raw(
        json!({"test": "project_recovery_k34_clear", "evaluations_after_answer": 1,
               "basis": rec["basis"]["reason"], "questions": 1}),
    );
}

#[tokio::test]
async fn project_recovery_k34_request_change_clears_objective_question() {
    let mut c = case(Some(ControllerMode::Enabled), |s: &ProjectSnapshot| {
        proposal(
            s,
            json!({"kind": "escalate", "category": "objective_change",
                   "question_key": format!("outcomes_invalid:{}", &s.project.request_sha256[..12]),
                   "summary": "依頼文を直す", "options": [], "evidence_refs": []}),
            "pc/outcomes-invalid",
            &[],
        )
    });
    c.wake();
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    let calls = c.calls();
    spin(&mut c, 3);
    assert_eq!(c.calls(), calls);
    rusqlite::Connection::open(c.env.db())
        .unwrap()
        .execute(
            "UPDATE projects SET request = ?1 WHERE id = ?2",
            rusqlite::params![format!("{REQUEST}\n直した"), c.p.id.to_string()],
        )
        .unwrap();
    human(&c);
    spin(&mut c, 3);
    let r = resumes(&c, "pc/resume/question");
    assert_eq!(r.len(), 1);
    let rec: Value = serde_json::from_str(&r[0].proposal_json).unwrap();
    assert_eq!(rec["basis"]["reason"], "request_changed");
    assert_eq!(c.calls(), calls + 1);
    assert_eq!(
        c.escalations(),
        2,
        "the changed request is a new question key"
    );
    raw(json!({"test": "project_recovery_k34_request", "basis": "request_changed"}));
}

#[tokio::test]
async fn project_recovery_k35_deny_has_no_effect_and_is_not_asked_again() {
    // TS の問いを断る。
    let mut c = case(Some(ControllerMode::Enabled), |s: &ProjectSnapshot| {
        escalate(s, "deploy-window")
    });
    c.wake();
    let calls = c.calls();
    dismissed(&c, "deploy-window");
    spin(&mut c, 3);
    assert_eq!(c.calls(), calls + 1);
    assert_eq!(c.escalations(), 1, "not asked again");
    assert_eq!(c.roots(), 0);
    assert_eq!(c.decisions()[0].validation, "pc/authority-answered");
    // 許可の不足の問いを断る: 再評価しても root は 0、問いは増えない。
    let mut c = without_grant(launch);
    c.wake();
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(c.escalations(), 1);
    dismissed(&c, "authority:launch_root:g0");
    spin(&mut c, 3);
    assert_eq!(c.roots(), 0);
    assert_eq!(c.escalations(), 1);
    assert_eq!(phase(&c), ControllerPhase::Waiting);
    assert_eq!(resumes(&c, "pc/resume/question").len(), 1);
    // 時間が過ぎて再評価しても同じ。
    hours(&mut c, 26);
    assert_eq!(c.roots(), 0);
    assert_eq!(c.escalations(), 1);
    raw(
        json!({"test": "project_recovery_k35_deny", "roots": 0, "questions": 1,
               "evaluations": c.calls()}),
    );
}

// ---- K37: 自動で解かないもの ----

#[tokio::test]
async fn project_recovery_k37_pause_and_cancel_are_not_lifted_by_time_or_answers() {
    for (status, expect) in [
        (ProjectStatus::Paused, ControllerPhase::Paused),
        (ProjectStatus::Cancelled, ControllerPhase::Cancelled),
    ] {
        let mut c = case(Some(ControllerMode::Enabled), launch);
        assert!(c.store.project_set_status(c.p.id, status).unwrap());
        let calls = c.calls();
        for day in 0..8 {
            brief_put(&c, &format!("{day}"));
            external(&c, &format!("e{day}"));
            hours(&mut c, 24);
        }
        assert_eq!(phase(&c), expect);
        assert_eq!(c.calls(), calls);
        assert_eq!(c.roots(), 0);
        raw(
            json!({"test": "project_recovery_k37_stop", "status": format!("{status:?}"),
                   "virtual_days": 8, "evaluations": 0, "roots": 0}),
        );
    }
}

#[tokio::test]
async fn project_recovery_k37_unresolved_human_required_stays_blocked() {
    for category in ["explicit_human", "objective_change", "standing_permission"] {
        let category_owned = category.to_owned();
        let mut c = case(Some(ControllerMode::Enabled), move |s: &ProjectSnapshot| {
            proposal(
                s,
                json!({"kind": "escalate", "category": category_owned, "question_key": "ask",
                       "summary": "人が決める", "options": [], "evidence_refs": []}),
                "pc/root-ended",
                &[],
            )
        });
        c.wake();
        let calls = c.calls();
        for day in 0..8 {
            brief_put(&c, &format!("{day}"));
            external(&c, &format!("e{day}"));
            let t = root_task(&c.env, &c.store, &c.p, Status::Done);
            transition(&c.store, &t, Status::Done, "review_pass");
            hours(&mut c, 24);
        }
        // 問いと関係のない key の許可でも解けない。
        grant(&c, Some("other-question"), a::ActionClass::LaunchRoot);
        spin(&mut c, 3);
        assert_eq!(phase(&c), ControllerPhase::Blocked, "{category}");
        assert_eq!(c.calls(), calls, "{category}");
        assert_eq!(c.escalations(), 1, "{category}");
        assert_eq!(c.roots(), 0, "{category}");
        raw(
            json!({"test": "project_recovery_k37_human_required", "category": category,
                   "virtual_days": 8, "evaluations_after_question": 0, "questions": 1}),
        );
    }
}

#[tokio::test]
async fn project_recovery_k37_dangerous_budget_overrun_stays_blocked() {
    let mut c = case_with(
        Some(ControllerMode::Enabled),
        launch,
        settings(|b| b.max_tokens_per_day = 1_000),
    );
    let t = root_task(&c.env, &c.store, &c.p, Status::Done);
    seed_tokens(&c, &t, "r1", 2_500);
    c.wake();
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(
        state(&c).next_wake_at,
        None,
        "no expiry for a dangerous overrun"
    );
    let calls = c.calls();
    for day in 0..8 {
        external(&c, &format!("e{day}"));
        brief_put(&c, &format!("{day}"));
        hours(&mut c, 24);
    }
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(c.calls(), calls);
    assert_eq!(c.roots(), 0);
    // 止めた時は上限内でも、止めた後に流れ込んだ run で危険になったら翌日に戻さず、1 回だけ知らせる。
    let mut c = case_with(
        Some(ControllerMode::Enabled),
        launch,
        settings(|b| b.max_tokens_per_day = 1_000),
    );
    let t = root_task(&c.env, &c.store, &c.p, Status::Done);
    seed_tokens(&c, &t, "r1", 1_200);
    c.wake();
    assert!(state(&c).next_wake_at.is_some());
    seed_tokens(&c, &t, "r2", 1_200);
    next_day(&mut c);
    spin(&mut c, 3);
    hours(&mut c, 48);
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(state(&c).next_wake_at, None);
    assert_eq!(rule_count(&c, "pc/resume/budget"), 0);
    assert_eq!(
        notices_like(&c, &format!("project_controller:budget_hold:{}:%", c.p.id)),
        1
    );
    raw(
        json!({"test": "project_recovery_k37_dangerous_budget", "virtual_days": 8,
               "evaluations_after_stop": 0, "hold_notices": 1, "roots": 0}),
    );
}

// ---- K39: CoS chat の token と起動 ----

#[tokio::test]
async fn project_recovery_k39_chat_tokens_stop_the_guard_and_are_measured() {
    let mut c = case_with(
        Some(ControllerMode::Enabled),
        launch,
        settings(|b| b.max_tokens_per_day = 1_000),
    );
    let conn = rusqlite::Connection::open(c.env.db()).unwrap();
    let at = c.env.now().format(&Rfc3339).unwrap();
    conn.execute(
        "INSERT INTO chat_threads (id,kind,title,project_id,status,created_at,updated_at) \
         VALUES ('pc','project','project',?1,'open',?2,?2)",
        rusqlite::params![c.p.id.to_string(), at],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chat_runs (run_id,thread_id,input_message_id,state,started_at,usage_json) \
         VALUES ('chat-1','pc','input','completed',?1,'{\"input_tokens\":1100,\"output_tokens\":100}')",
        [at],
    )
    .unwrap();
    for n in 0..3 {
        external(&c, &format!("e{n}"));
        spin(&mut c, 2);
    }
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(c.calls(), 0, "the guard stops before the decider");
    assert_eq!(rule_count(&c, "pc/guard/budget"), 1);
    assert_eq!(c.escalations(), 1);
    assert_eq!(c.roots(), 0);
    let usage =
        task_ops::project_budget::usage(&c.store, &c.p.id.to_string(), c.env.now()).unwrap();
    assert_eq!(usage.tokens, 1_200);
    assert_eq!(usage.chat_starts, 1);
    let rec: Value =
        serde_json::from_str(&resumes(&c, "pc/guard/budget")[0].proposal_json).unwrap();
    assert_eq!(rec["detail"]["usage"]["chat_starts"], 1);
    raw(
        json!({"test": "project_recovery_k39", "tokens": usage.tokens, "chat_starts": usage.chat_starts,
               "questions": 1, "evaluations": 0}),
    );
}

// ---- K40: 翌日の再評価 ----

#[tokio::test]
async fn project_recovery_k40_token_budget_expires_at_next_utc_day_once() {
    let mut c = case_with(
        Some(ControllerMode::Enabled),
        human_wait,
        settings(|b| b.max_tokens_per_day = 1_000),
    );
    let t = root_task(&c.env, &c.store, &c.p, Status::Done);
    seed_tokens(&c, &t, "r1", 1_200);
    c.wake();
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(c.calls(), 0);
    assert_eq!(
        state(&c).next_wake_at.as_deref(),
        Some("2026-11-02T00:00:00Z"),
        "the expiry is the next UTC midnight"
    );
    // 当日は外の入力があっても再開しない。
    external(&c, "same-day");
    spin(&mut c, 3);
    assert_eq!(c.calls(), 0);
    next_day(&mut c);
    spin(&mut c, 3);
    assert_eq!(c.calls(), 1, "one re-evaluation on the next day");
    let r = resumes(&c, "pc/resume/budget");
    assert_eq!(r.len(), 1);
    let rec: Value = serde_json::from_str(&r[0].proposal_json).unwrap();
    assert_eq!(rec["basis"]["reason"], "next_day");
    assert_eq!(rec["basis"]["day"], "2026-11-01");
    assert_eq!(c.escalations(), 1);
    assert_eq!(pending_wakes(&c.store, &c.p), 0);
    raw(
        json!({"test": "project_recovery_k40_tokens", "expiry": "2026-11-02T00:00:00Z",
               "evaluations_same_day": 0, "evaluations_next_day": 1, "questions": 1}),
    );
}

#[tokio::test]
async fn project_recovery_k40_root_budget_needs_a_live_grant() {
    // 許可が切れている: 翌日も再評価せず、root は増えない。
    let mut c = case_with(
        Some(ControllerMode::Enabled),
        launch,
        settings(|b| b.max_roots_per_day = 1),
    );
    c.wake();
    assert_eq!(c.roots(), 1);
    external(&c, "again");
    spin(&mut c, 2);
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(rule_count(&c, "pc/guard/budget"), 1);
    let project = c.p.id.to_string();
    let now = c.env.now();
    let live = c.store.authority_grants(&project).unwrap();
    c.store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            for g in &live {
                SqliteStore::authority_grant_revoke_tx(tx, &project, &g.id, "human:test", now)?;
            }
            Ok(())
        })
        .unwrap();
    next_day(&mut c);
    hours(&mut c, 26);
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(c.roots(), 1, "no root without a standing grant");
    assert_eq!(rule_count(&c, "pc/resume/budget"), 0);
    assert_eq!(
        notices_like(&c, &format!("project_controller:budget_hold:{project}:%")),
        1
    );
    // 許可が生きている: 翌日に 1 回再評価する（root は同じ key で増えない）。
    let mut c = case_with(
        Some(ControllerMode::Enabled),
        launch,
        settings(|b| b.max_roots_per_day = 1),
    );
    c.wake();
    external(&c, "again");
    spin(&mut c, 2);
    let calls = c.calls();
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    next_day(&mut c);
    spin(&mut c, 3);
    assert_eq!(rule_count(&c, "pc/resume/budget"), 1);
    assert_eq!(c.calls(), calls + 1);
    assert_eq!(c.roots(), 1);
    raw(
        json!({"test": "project_recovery_k40_roots", "without_grant_resumes": 0, "with_grant_resumes": 1,
               "roots": 1}),
    );
}

#[tokio::test]
async fn project_recovery_k40_auto_resume_is_limited_without_a_person() {
    let mut c = case_with(
        Some(ControllerMode::Enabled),
        human_wait,
        settings(|b| b.max_tokens_per_day = 1_000),
    );
    let t = root_task(&c.env, &c.store, &c.p, Status::Done);
    let mut seq = 0;
    for _ in 0..6 {
        seq += 1;
        seed_tokens(&c, &t, &format!("r{seq}"), 1_200);
        external(&c, &format!("e{seq}"));
        spin(&mut c, 3);
        next_day(&mut c);
        spin(&mut c, 3);
    }
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(rule_count(&c, "pc/resume/budget"), 3);
    assert_eq!(c.escalations(), 4, "one question per stopped day");
    assert_eq!(
        notices_like(&c, &format!("project_controller:budget_hold:{}:%", c.p.id)),
        1
    );
    raw(
        json!({"test": "project_recovery_k40_limit", "days": 6, "auto_resumes": 3,
               "questions": c.escalations(), "hold_notices": 1}),
    );
}

#[tokio::test]
async fn project_recovery_k40_same_day_reset_blocks_again_without_a_new_question() {
    let mut c = case_with(
        Some(ControllerMode::Enabled),
        launch,
        settings(|b| b.max_tokens_per_day = 1_000),
    );
    let t = root_task(&c.env, &c.store, &c.p, Status::Done);
    seed_tokens(&c, &t, "r1", 1_200);
    c.wake();
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    c.set_mode(ControllerMode::Enabled);
    assert_eq!(phase(&c), ControllerPhase::Active);
    c.wake();
    spin(&mut c, 2);
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(c.escalations(), 1);
    assert_eq!(c.roots(), 0);
    raw(json!({"test": "project_recovery_k40_same_day_reset", "questions": 1, "roots": 0}));
}

// ---- 検出した懸念: CoS の brief で human wait の TTL が延びない ----

#[tokio::test]
async fn project_recovery_cos_brief_does_not_extend_the_human_wait_ttl() {
    let ttl = 3 * 86400;
    let mut c = case(Some(ControllerMode::Enabled), human_wait);
    c.wake();
    let start = c.env.now();
    let mut updates = 0;
    while (c.env.now() - start).whole_seconds() < ttl + 86400 {
        c.env.advance(12 * 3600);
        updates += 1;
        brief_put(&c, &format!("要約 {updates}"));
        spin(&mut c, 2);
    }
    let evaluations = c.calls();
    assert_eq!(phase(&c), ControllerPhase::Blocked);
    assert_eq!(
        rule_count(&c, "pc/guard/ttl"),
        1,
        "the summary is not an answer"
    );
    assert_eq!(c.escalations(), 1);
    assert_eq!(c.roots(), 0);
    // 外の応答で再開する。
    external(&c, "reply");
    spin(&mut c, 2);
    assert_eq!(resumes(&c, "pc/resume/ttl").len(), 1);
    assert_eq!(c.calls(), evaluations + 1);
    raw(
        json!({"test": "project_recovery_cos_brief_ttl", "brief_updates": updates,
               "evaluations": evaluations, "ttl_stops": 1, "questions": 1}),
    );
}

// ---- K36: 7 日以上の複合 ----

/// 8 日の仮想時刻に、K29〜K35 の出来事を順に流す。件数の上限を確かめる。
#[tokio::test]
async fn project_recovery_k36_eight_day_composite_keeps_the_limits() {
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let phase_of_script = counter.clone();
    let mut c = case_with(
        Some(ControllerMode::Enabled),
        move |s: &ProjectSnapshot| match phase_of_script.load(std::sync::atomic::Ordering::SeqCst) {
            0 => human_wait(s),
            1 => escalate(s, "scope-day3"),
            _ => human_wait(s),
        },
        settings(|b| {
            b.max_tokens_per_day = 10_000;
            b.max_same_failure_repeats = 3;
        }),
    );
    let job = reconcile_job(&c.store, &c.p);
    let _ = job;
    let t0 = c.env.now();
    let mut stats = Vec::new();
    for hour in 1..=(8 * 24) {
        *c.env.clock.lock().unwrap() = t0 + time::Duration::hours(hour);
        match hour {
            // 1 日目: CoS の要約と外の応答。
            5 => brief_put(&c, "day1"),
            6 => external(&c, "day1"),
            // 2 日目: 同じ失敗が 3 回で止まる。人が解消する。
            30 | 31 | 32 => {
                fail_root(&c, "check failed: exit 3");
            }
            40 => human(&c),
            // 3 日目: 時計飛び（72 時間ぶんの cron は 1 件に畳まれる）。
            // 4 日目: 問い。止まったまま 1 日。人が答える。
            72 => {
                counter.store(1, std::sync::atomic::Ordering::SeqCst);
                external(&c, "ask");
            }
            96 => grant(&c, Some("scope-day3"), a::ActionClass::LaunchRoot),
            // 5 日目: 予算で止まり、翌日に戻る。
            100 => {
                counter.store(2, std::sync::atomic::Ordering::SeqCst);
                let t = root_task(&c.env, &c.store, &c.p, Status::Done);
                seed_tokens(&c, &t, "r-day5", 12_000);
                external(&c, "budget");
            }
            // 6 日目: 同じ tick に brief と authority の編集。
            130 => {
                brief_put(&c, "day6");
                grant(&c, None, a::ActionClass::LaunchRoot);
            }
            _ => {}
        }
        c.d.tick().unwrap();
        c.env.advance(31);
        c.d.tick().unwrap();
        if hour % 24 == 0 {
            stats.push(json!({
                "day": hour / 24,
                "evaluations": c.calls(),
                "decisions": c.decisions().len(),
                "wakes": c.store.project_wakes_all(&c.p.id.to_string()).unwrap().len(),
                "questions": c.escalations(),
                "phase": phase(&c).as_str(),
            }));
        }
    }
    let wakes = c.store.project_wakes_all(&c.p.id.to_string()).unwrap();
    let cron = wakes
        .iter()
        .filter(|w| w.kind == ProjectWakeKind::Cron)
        .count();
    assert_eq!(c.roots(), 0, "the script never launches");
    assert_eq!(c.escalations(), 3, "same failure, the question, the budget");
    assert_eq!(resumes(&c, "pc/resume/same_failure").len(), 1);
    assert_eq!(resumes(&c, "pc/resume/question").len(), 1);
    assert_eq!(resumes(&c, "pc/resume/budget").len(), 1);
    assert_eq!(pending_wakes(&c.store, &c.p), 0);
    assert_eq!(
        c.store.project_controller_pending_effects().unwrap().len(),
        0
    );
    assert!(c.calls() <= 20, "evaluations {} {stats:?}", c.calls());
    assert!(cron <= 10, "cron wakes {cron} of 192 hours");
    assert_eq!(
        c.count(
            "SELECT COUNT(*) FROM chat_messages WHERE client_message_id LIKE 'pc-input:%' AND ?1 IS NOT NULL"
        ),
        0,
        "no CoS input from the controller"
    );
    // 予算の試験の run の行（seed）は数えない: LLM の起動は CoS の chat run で数える。
    assert_eq!(
        c.count("SELECT COUNT(*) FROM chat_runs WHERE ?1 IS NOT NULL"),
        0
    );
    raw(
        json!({"test": "project_recovery_k36", "virtual_days": 8, "evaluations": c.calls(),
               "wakes": wakes.len(), "cron_wakes": cron, "questions": c.escalations(), "roots": 0,
               "chat_runs": 0, "by_day": stats}),
    );
}
