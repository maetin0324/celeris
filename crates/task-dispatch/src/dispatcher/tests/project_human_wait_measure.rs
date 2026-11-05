//! K38: 人の回答待ちだけの案件を仮想時刻で 24 時間流し、評価の回数を数える台本。修正前の tree でも同じ file を
//! 動かして比べる（`scripts/dev/test-project-recovery.sh` の手順）。現行の store の新しい口を使わない。
//! 本物の TS（bundle）が無ければ何もしない（`real_ts` の SKIP の規則）。

use super::*;

fn raw(value: Value) {
    let Some(dir) = std::env::var_os("PROJECT_RECOVERY_RAW_DIR") else {
        return;
    };
    let mut v = value;
    v["schema"] = json!("celeris.long-horizon-record/1");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join("human_wait_measure.jsonl"))
        .unwrap();
    use std::io::Write as _;
    f.write_all(format!("{v}\n").as_bytes()).unwrap();
}

fn evaluations(c: &Env, store: &SqliteStore, p: &Project) -> (usize, usize) {
    let _ = c;
    let rows = store
        .project_controller_decisions(&p.id.to_string(), None, 1000)
        .unwrap();
    let cron = store
        .project_wakes_all(&p.id.to_string())
        .unwrap()
        .iter()
        .filter(|w| w.kind == ProjectWakeKind::Cron)
        .count();
    (rows.len(), cron)
}

/// 開いた問いのある root を持つ案件（本物の TS は `pc/human-wait`）と、人が答えた問いの待ち（`pc/authority-answered`）。
#[tokio::test]
async fn project_recovery_k38_human_wait_evaluations_in_24_hours() {
    let Some(ts) = super::super::long_horizon_bench::real_ts("project_recovery_k38") else {
        return;
    };
    // 1. 本物の TS: 人の質問が開いた root。
    let env = Env::new();
    let store = env.open();
    let p = pc_project(&store, Some(ControllerMode::Enabled));
    reconcile_job(&store, &p);
    let decider = TsProjectDecider::with_fixed_script(DeciderLimits::default(), ts);
    let mut d = ts_daemon(&env, &store, decider, ProjectControllerSettings::default());
    prime(&mut d);
    let root = root_task(&env, &store, &p, Status::Running);
    store
        .append_event(
            root.id,
            &Event::QuestionRaised {
                run_id: "k38".into(),
                text: "人の判断を待つ".into(),
            },
        )
        .unwrap();
    d.tick().unwrap();
    env.advance(31);
    d.tick().unwrap();
    let base = evaluations(&env, &store, &p);
    let start = env.now();
    while (env.now() - start).whole_seconds() < 24 * 3600 {
        env.advance(300);
        d.tick().unwrap();
    }
    let (rows, cron) = evaluations(&env, &store, &p);
    let human_wait = rows - base.0;
    let last = store
        .project_controller_decisions(&p.id.to_string(), None, 1)
        .unwrap();
    assert_eq!(last[0].validation, "pc/human-wait");
    raw(
        json!({"test": "project_recovery_k38", "case": "open_question_real_ts",
               "virtual_hours": 24, "evaluations": human_wait, "cron_wakes": cron - base.1,
               "rule": last[0].validation, "cos_inputs_and_roots": controller_roots(&store, &p)}),
    );
    // 2. 答えのある問い（fake の script、Rust の `ANSWERED_RECHECK_SECS`）。cron なしで timer だけを数える。
    let mut c = case(Some(ControllerMode::Enabled), |s: &ProjectSnapshot| {
        escalate(s, "answered-question")
    });
    let project = c.p.id.to_string();
    let now = c.env.now();
    c.store
        .write_tx::<_, task_core::StoreError, _>(|tx| {
            use task_core::authority as a;
            SqliteStore::authority_grant_insert_tx(
                tx,
                &a::NewAuthorityGrant {
                    id: ulid::Ulid::new().to_string(),
                    project_id: project.clone(),
                    scope: a::GrantScope {
                        kind: a::ScopeKind::Project,
                        reference: project.clone(),
                    },
                    action_class: a::ActionClass::LaunchRoot,
                    source: a::GrantSource {
                        kind: a::SourceKind::HumanOperation,
                        reference: format!("project.controller_put:{project}:1"),
                    },
                    granted_by: "human:test".into(),
                    question_key: Some("answered-question".into()),
                    note: "答え".into(),
                    expires_at: (now + time::Duration::days(30)).format(&Rfc3339).unwrap(),
                    budget: 5,
                },
                now,
            )?
            .unwrap();
            Ok(())
        })
        .unwrap();
    c.wake();
    let before = c.calls();
    let start = c.env.now();
    while (c.env.now() - start).whole_seconds() < 24 * 3600 {
        c.env.advance(300);
        tick(&mut c.d);
    }
    let answered = c.calls() - before;
    raw(
        json!({"test": "project_recovery_k38", "case": "answered_question_fake_ts",
               "virtual_hours": 24, "evaluations": answered}),
    );
    assert!(
        human_wait <= 1,
        "real TS human wait evaluated {human_wait} times in 24 h"
    );
    assert!(
        answered <= 1,
        "answered wait evaluated {answered} times in 24 h"
    );
}
