use std::path::PathBuf;

use task_core::model_policy::{LaneCeiling, RoutingRecord, decide_for_task};
use task_core::model_router::feedback::{
    RequestSourceAttempt, RoutingOutcome, RoutingRequestRecord,
};
use task_core::{Event, RunMetrics, SqliteStore, Status, TaskId, TaskStore, Usage};
use time::OffsetDateTime;

use super::record_routing_outcomes;
use crate::add::{CriterionSpec, NewTaskSpec, PriorityInput, SpecProvenance, create_task};

fn spec() -> NewTaskSpec {
    NewTaskSpec {
        mode: Default::default(),
        skills: Vec::new(),
        requirements: Default::default(),
        repos: Vec::new(),
        title: "routing outcome".to_string(),
        objective: "record the run outcome".to_string(),
        acceptance: vec![CriterionSpec::ArtifactExists {
            name: "result.md".to_string(),
        }],
        kind: task_core::TaskKind::Execute,
        tier: None,
        priority: Some(PriorityInput::Number(0)),
        parent: None,
        depends_on: vec![],
        max_turns: None,
        max_wall_secs: None,
        max_tokens: None,
        max_retries: 2,
        role: None,
        genre: None,
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        workspace: Some(PathBuf::from("/tmp/workspace")),
        cluster: None,
        workspace_mode: None,
        harness: None,
        labels: Vec::new(),
        category: None,
        status: None,
        features: None,
        execution: None,
        pause_after: None,
        stages_hint: Vec::new(),
        provenance: SpecProvenance::default(),
    }
}

/// run r1 の開始・routing 決定・完了（コスト 0.5 USD・30 分・retries 1）を書く。
fn run_events(task: &task_core::Task) -> Vec<Event> {
    let decision = decide_for_task(task, &LaneCeiling::default()).expect("lane decision");
    vec![
        Event::WorkerStarted {
            run_id: "r1".into(),
            adapter: "claude-code".into(),
            model: "model-cheap".into(),
            provider: Some("cc-1".into()),
            account: None,
            role: None,
            task_role: None,
        },
        Event::RoutingDecided {
            run_id: "r1".into(),
            record: Box::new(RoutingRecord {
                features: None,
                org_node: Some("software-engineering".into()),
                harness: Some("coding".into()),
                decision,
                quota_reason: None,
                work_unit_id: None,
                escalation: None,
            }),
        },
        Event::WorkerFinished {
            run_id: "r1".into(),
            outcome: "done: ok".into(),
            usage: Some(Usage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                cost_usd: Some(0.5),
                ..Usage::default()
            }),
            role: None,
            metrics: Some(RunMetrics {
                wall_ms: 1_800_000,
                retries: 1,
                peak_context_tokens: None,
                turns: None,
            }),
            end: Some(task_core::RunEnd::Completed),
        },
    ]
}

fn append_all(store: &SqliteStore, id: TaskId, events: &[Event]) {
    for event in events {
        store.append_event(id, event).expect("append event");
    }
}

fn outcomes(
    store: &SqliteStore,
    id: TaskId,
) -> Vec<task_core::model_router::feedback::RoutingOutcome> {
    store
        .events_for(id)
        .expect("events")
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::RoutingOutcomeRecorded { outcome } => Some(*outcome),
            _ => None,
        })
        .collect()
}

/// ADR 2026-10-04 §6・§10 Phase 3: review 未到着は reward=None、到着後に 1 件、再投影は同じ、訂正は旧 outcome を
/// supersede。run の合否を request に複写しない。
#[test]
fn routing_reward_waits_for_review_and_supersedes_idempotently() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = create_task(&store, spec(), OffsetDateTime::now_utc()).expect("create task");
    let id = task.id;
    append_all(&store, id, &run_events(&task));

    // review 未到着: 1 件。review も受け入れ検査も未判定なので reward は None（false や 0 ではない）。
    assert_eq!(record_routing_outcomes(&store, id).unwrap(), 1);
    let first = outcomes(&store, id);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].review_passed, None);
    assert_eq!(first[0].reward, None);
    assert_eq!(first[0].cash_usd, Some(0.5));
    assert_eq!(first[0].wall_ms, Some(1_800_000));
    assert_eq!(first[0].retries, Some(1));
    // 同じ入力の再投影は追記しない（冪等）。
    assert_eq!(record_routing_outcomes(&store, id).unwrap(), 0);
    assert_eq!(outcomes(&store, id).len(), 1);

    // review 到着（合格）: 新しい 1 件。reward = 1 - 0.2*0.5 - 0.1*0.5 - 0.1*0.25 = 0.825。
    append_all(
        &store,
        id,
        &[Event::Transitioned {
            from: Status::Reviewing,
            to: Status::Done,
            reason: "review_pass".into(),
        }],
    );
    assert_eq!(record_routing_outcomes(&store, id).unwrap(), 1);
    let passed = outcomes(&store, id);
    assert_eq!(passed.len(), 2);
    assert_eq!(passed[1].review_passed, Some(true));
    let reward = passed[1].reward.expect("reward after review");
    assert!((reward - 0.825).abs() < 1e-9, "reward={reward}");
    // review 前の outcome（reward None）を、review 後の結果が supersede する。
    assert_eq!(
        passed[1].supersedes.as_deref(),
        Some(first[0].outcome_id.as_str())
    );
    assert_eq!(record_routing_outcomes(&store, id).unwrap(), 0);

    // proxy の要求を結んでも、run の outcome は要求に複写されない（request_id は常に None）。
    append_all(
        &store,
        id,
        &[Event::RoutingRequestDecided {
            record: Box::new(RoutingRequestRecord {
                request_id: "req-1".into(),
                decision_id: "proxy:req-1".into(),
                parent_decision_id: Some("run:r1".into()),
                run_id: Some("r1".into()),
                attempts: vec![RequestSourceAttempt {
                    source_id: "qwen-local".into(),
                    model: Some("qwen".into()),
                    account_id: None,
                    fallback_reason: None,
                }],
                fallback_reason: None,
            }),
        }],
    );
    assert_eq!(record_routing_outcomes(&store, id).unwrap(), 0);
    assert!(outcomes(&store, id).iter().all(|o| o.request_id.is_none()));

    // 後の review 訂正（不合格）: 旧 outcome を supersede する新しい 1 件。旧 outcome は書き換えない。
    append_all(
        &store,
        id,
        &[Event::Transitioned {
            from: Status::Done,
            to: Status::Ready,
            reason: "review_fail".into(),
        }],
    );
    assert_eq!(record_routing_outcomes(&store, id).unwrap(), 1);
    let all = outcomes(&store, id);
    assert_eq!(all.len(), 3);
    assert_eq!(all[2].review_passed, Some(false));
    assert_eq!(all[2].failure_class.as_deref(), Some("review_failed"));
    assert_eq!(
        all[2].supersedes.as_deref(),
        Some(all[1].outcome_id.as_str())
    );
    assert_ne!(all[2].outcome_id, all[1].outcome_id);
    assert_eq!(
        all[1].review_passed,
        Some(true),
        "the old outcome stays as recorded"
    );
    assert_eq!(record_routing_outcomes(&store, id).unwrap(), 0);
}

#[test]
fn routing_outcome_sums_finished_fragments_and_estimates_known_prices() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let task = create_task(&store, spec(), OffsetDateTime::now_utc()).expect("create task");
    let id = task.id;
    let mut events = run_events(&task);
    if let Event::WorkerStarted { model, .. } = &mut events[0] {
        *model = "gpt-5-codex".into();
    }
    let Event::WorkerFinished { usage, metrics, .. } = &mut events[2] else {
        panic!("worker finish");
    };
    usage.as_mut().unwrap().cost_usd = None;
    let price =
        task_core::estimate_cost_usd("gpt-5-codex", usage.as_ref().unwrap()).expect("known price");
    metrics.as_mut().unwrap().wall_ms = 1_000;
    let mut second = events[2].clone();
    if let Event::WorkerFinished { usage, metrics, .. } = &mut second {
        *usage = Some(Usage {
            cost_usd: Some(0.25),
            ..Usage::default()
        });
        *metrics = Some(RunMetrics {
            wall_ms: 2_000,
            retries: 2,
            peak_context_tokens: None,
            turns: None,
        });
    }
    events.push(second);
    append_all(&store, id, &events);

    assert_eq!(record_routing_outcomes(&store, id).unwrap(), 1);
    let result = outcomes(&store, id);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].cash_usd, Some(price + 0.25));
    assert_eq!(result[0].tokens, Some(120));
    assert_eq!(result[0].wall_ms, Some(3_000));
    assert_eq!(result[0].retries, Some(3));
    assert_eq!(record_routing_outcomes(&store, id).unwrap(), 0);
}

/// guardrail の窓（ADR 2026-10-11-llm-source-model-routing-evaluation D3）: run ごとの最新 outcome を
/// genre×harness に集計する。訂正（supersede）された outcome は二度数えず、genre の無い task は数えない。
#[test]
fn route_windows_projects_the_latest_outcome_per_run() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let now = OffsetDateTime::now_utc();
    let mut spec = spec();
    spec.genre = Some("coding".to_string());
    let task = create_task(&store, spec, now).expect("create task");
    let id = task.id;
    append_all(&store, id, &run_events(&task));
    assert_eq!(record_routing_outcomes(&store, id).unwrap(), 1);

    let since = now - time::Duration::days(1);
    let windows = super::route_windows(&store, since, 50).expect("windows");
    assert_eq!(windows.len(), 1);
    let window = &windows[0];
    assert_eq!(window.genre, "coding");
    assert_eq!(window.harness_id, "coding");
    assert_eq!(window.task_id, Some(id));
    // review 未到着: 合格でも失敗でもない（未判定は `runs` だけに数える）。
    assert_eq!(
        (window.runs, window.successes, window.failures),
        (1, 0, 0),
        "{window:?}"
    );
    assert!(
        (window.wall_median_s - 1800.0).abs() < 1e-9,
        "wall={}",
        window.wall_median_s
    );
    assert!(window.last_at.is_some_and(|at| at >= since));

    // review 合格: 訂正された旧 outcome は数えず、同じ 1 run が合格になる。
    append_all(
        &store,
        id,
        &[Event::Transitioned {
            from: Status::Reviewing,
            to: Status::Done,
            reason: "review_pass".into(),
        }],
    );
    assert_eq!(record_routing_outcomes(&store, id).unwrap(), 1);
    let windows = super::route_windows(&store, since, 50).expect("windows");
    assert_eq!(
        (windows[0].runs, windows[0].successes, windows[0].failures),
        (1, 1, 0),
        "{:?}",
        windows[0]
    );

    // genre が無ければ集計しない（無い値を捏造しない）。
    let other = create_task(&store, self::spec(), now).expect("create task");
    append_all(&store, other.id, &run_events(&other));
    assert_eq!(record_routing_outcomes(&store, other.id).unwrap(), 1);
    let windows = super::route_windows(&store, since, 50).expect("windows");
    assert_eq!(windows.len(), 1, "{windows:?}");
}

/// run の `routing_decided` が無いときは task の `harness_id` で帰属させる。
#[test]
fn route_windows_falls_back_to_the_task_harness() {
    let store = SqliteStore::open_in_memory().expect("open store");
    let now = OffsetDateTime::now_utc();
    let mut spec = spec();
    spec.genre = Some("coding".to_string());
    spec.harness = Some("codex".to_string());
    let task = create_task(&store, spec, now).expect("create task");
    store
        .append_event(
            task.id,
            &Event::RoutingOutcomeRecorded {
                outcome: Box::new(RoutingOutcome {
                    outcome_id: "o1".into(),
                    decision_id: "d1".into(),
                    run_id: Some("r9".into()),
                    request_id: None,
                    evaluation_version: "v1".into(),
                    supersedes: None,
                    acceptance_passed: Some(false),
                    review_passed: None,
                    failed_criterion_ids: vec!["c1".into()],
                    failure_class: Some("check".into()),
                    cash_usd: None,
                    tokens: None,
                    wall_ms: Some(500),
                    retries: None,
                    reward: None,
                }),
            },
        )
        .expect("append");
    let windows = super::route_windows(&store, now - time::Duration::days(1), 50).expect("windows");
    assert_eq!(windows.len(), 1, "{windows:?}");
    assert_eq!(windows[0].harness_id, "codex");
    assert_eq!(
        (windows[0].runs, windows[0].successes, windows[0].failures,),
        (1, 0, 1)
    );
    assert!((windows[0].wall_median_s - 0.5).abs() < 1e-9);
}
