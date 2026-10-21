//! guardrail tick の試験（ADR 2026-10-11-llm-source-model-routing-evaluation D3）。
//! run 結果と時計は注入する（sleep しない）。store は in-memory。

use std::collections::BTreeMap;

use super::*;
use task_core::feed::{NoticeQuery, NoticeStore};
use task_core::llm_route::{CandidateKey, HarnessId, Registry, RouteSelectionSettings};
use task_core::model::{Budget, Check, Criterion, WorkerHint, WorkspaceSpec};
use task_core::model_router::feedback::RoutingOutcome;
use task_core::routing_evidence::{EvidenceKey, EvidenceRow};
use task_core::{Event, SqliteStore, Status, Task, TaskId, TaskKind, TaskStore, Tier};
use task_ops::routing_outcome::RouteWindow;
use time::OffsetDateTime;

fn at() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("clock")
}

fn settings() -> GuardrailSettings {
    GuardrailSettings {
        window: 50,
        quota_floor: 0.1,
    }
}

fn window(runs: u64, successes: u64, failures: u64, wall_median_s: f64) -> RouteWindow {
    RouteWindow {
        genre: "coding".into(),
        harness_id: "codex".into(),
        task_id: None,
        runs,
        successes,
        failures,
        wall_median_s,
        last_at: Some(at()),
    }
}

fn evidence(harness_version: &str, pass_lower: f64, wall_median_s: f64) -> EvidenceRow {
    EvidenceRow {
        key: EvidenceKey {
            genre: "coding".into(),
            harness_id: "codex".into(),
            harness_version: harness_version.into(),
            source_id: "s".into(),
            model_id: "m".into(),
        },
        evidence_version: "v1".into(),
        n: 30,
        pass: 27,
        pass_lower,
        pass_upper: 0.97,
        pass_boot_lower: pass_lower,
        pass_boot_upper: 0.97,
        wall_median_s: Some(wall_median_s),
        usd_list_median: Some(0.2),
        quota_per_run: [("7d".to_string(), 0.01)].into(),
        timeout_rate: 0.0,
        low_confidence: false,
    }
}

fn input(stage: RoutingStage, window: RouteWindow, evidence: Vec<EvidenceRow>) -> PairInput {
    PairInput {
        window,
        stage,
        evidence,
        quota_remaining_fraction: Some(0.5),
        live_harness_version: Some("1.2.0".into()),
    }
}

fn healthy() -> RouteWindow {
    window(30, 30, 0, 10.0)
}

fn reasons(action: &GuardrailAction) -> String {
    match action {
        GuardrailAction::Revert { reason, .. } => reason.clone(),
        GuardrailAction::QuotaNotice { .. } => "quota".into(),
    }
}

#[test]
fn plan_pair_reverts_on_each_stop_condition() {
    let base = evidence("1.2.0", 0.5, 10.0);
    // 合格率: 直近 run の Wilson 上限が根拠表の pass_lower を下回る。
    let action = plan_pair(
        &input(
            RoutingStage::Enforce,
            window(10, 0, 0, 10.0),
            vec![base.clone()],
        ),
        &settings(),
        &[],
    )
    .expect("revert");
    assert!(reasons(&action).contains("pass_rate"), "{action:?}");
    // 失敗率: 区間下限が shadow 期間の失敗率（根拠表の timeout_rate）の 2 倍を超える。
    let action = plan_pair(
        &input(
            RoutingStage::Enforce,
            window(10, 10, 10, 10.0),
            vec![base.clone()],
        ),
        &settings(),
        &[],
    )
    .expect("revert");
    assert!(reasons(&action).contains("failure_rate"), "{action:?}");
    // 壁時計: 中央値が根拠表の 2 倍を超える。
    let action = plan_pair(
        &input(
            RoutingStage::Enforce,
            window(30, 30, 0, 21.0),
            vec![base.clone()],
        ),
        &settings(),
        &[],
    )
    .expect("revert");
    assert!(reasons(&action).contains("wall_clock"), "{action:?}");
    // 根拠表の harness の major が本番の版と違う。
    let action = plan_pair(
        &input(
            RoutingStage::Enforce,
            healthy(),
            vec![evidence("2.0.0", 0.5, 10.0)],
        ),
        &settings(),
        &[],
    )
    .expect("revert");
    assert!(
        reasons(&action).contains("evidence_harness_major_mismatch"),
        "{action:?}"
    );
}

#[test]
fn plan_pair_keeps_enforce_below_thresholds_and_skips_shadow() {
    let base = vec![evidence("1.2.0", 0.5, 10.0)];
    assert_eq!(
        plan_pair(
            &input(RoutingStage::Enforce, healthy(), base.clone()),
            &settings(),
            &[]
        ),
        None
    );
    // 既に shadow なら（条件を満たしていても）何もしない。
    assert_eq!(
        plan_pair(
            &input(RoutingStage::Shadow, window(10, 0, 10, 99.0), base.clone()),
            &settings(),
            &[]
        ),
        None
    );
    // 根拠表の行が無ければ判定できない（`enforce` のまま、値を捏造しない）。
    assert_eq!(
        plan_pair(
            &input(RoutingStage::Enforce, healthy(), vec![]),
            &settings(),
            &[]
        ),
        None
    );
    // 本番の harness の版が未知なら版の食い違いは判定しない（他の条件は効く）。
    let mut unknown = input(
        RoutingStage::Enforce,
        healthy(),
        vec![evidence("9.9.9", 0.5, 10.0)],
    );
    unknown.live_harness_version = None;
    assert_eq!(plan_pair(&unknown, &settings(), &[]), None);
}

#[test]
fn plan_pair_quota_low_only_notifies_and_never_promotes() {
    let base = vec![evidence("1.2.0", 0.5, 10.0)];
    let mut low = input(RoutingStage::Enforce, healthy(), base.clone());
    low.quota_remaining_fraction = Some(0.09);
    let action = plan_pair(&low, &settings(), &[]).expect("notice");
    assert!(
        matches!(&action, GuardrailAction::QuotaNotice { remaining, .. } if (*remaining - 0.09).abs() < f64::EPSILON),
        "{action:?}"
    );
    // 測定できない quota は「低い」と扱わない。
    let mut unknown = input(RoutingStage::Enforce, healthy(), base.clone());
    unknown.quota_remaining_fraction = None;
    assert_eq!(plan_pair(&unknown, &settings(), &[]), None);
    // shadow から enforce へは自動的に進まない（shadow は判定しない）。
    let mut shadow = input(RoutingStage::Shadow, healthy(), base);
    shadow.quota_remaining_fraction = Some(0.09);
    assert_eq!(plan_pair(&shadow, &settings(), &[]), None);
}

#[test]
fn select_evidence_follows_route_defaults_then_largest_n() {
    let mut a = evidence("1.2.0", 0.5, 10.0);
    a.key.model_id = "m-a".into();
    a.n = 5;
    let mut b = evidence("1.2.0", 0.9, 10.0);
    b.key.model_id = "m-b".into();
    b.n = 30;
    let rows = vec![a.clone(), b.clone()];
    let priority = vec![CandidateKey {
        source: "s".into(),
        model: "m-a".into(),
    }];
    assert_eq!(select_evidence(&rows, &priority), Some(&a));
    assert_eq!(select_evidence(&rows, &[]), Some(&b));
    assert_eq!(select_evidence(&[], &priority), None);
}

#[test]
fn ticker_runs_on_the_injected_clock_only_after_the_interval() {
    let mut ticker = Ticker::default();
    let start = at();
    assert!(ticker.due(INTERVAL, start));
    assert!(!ticker.due(INTERVAL, start));
    assert!(!ticker.due(INTERVAL, start + Duration::from_secs(899)));
    assert!(ticker.due(INTERVAL, start + INTERVAL));
}

struct FixedQuota(Option<f64>);

impl QuotaProbe for FixedQuota {
    fn min_remaining(&self, _sources: &[String], _now: OffsetDateTime) -> Option<f64> {
        self.0
    }
}

fn registry() -> Registry {
    let mut registry = Registry {
        route_selection: RouteSelectionSettings {
            exploration_epsilon: 0.1,
            guardrail_window: 50,
            guardrail_quota_floor: 0.1,
            evaluation_reservations: BTreeMap::new(),
        },
        ..Registry::default()
    };
    registry.route_defaults.insert(
        HarnessId::from("codex"),
        vec![CandidateKey {
            source: "s".into(),
            model: "m".into(),
        }],
    );
    registry
}

fn sample_task(genre: &str, harness: &str, now: OffsetDateTime) -> Task {
    let id = TaskId::new();
    Task {
        harness_id: Some(HarnessId::from(harness)),
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
        title: "guardrail fixture".into(),
        objective: "run".into(),
        acceptance: vec![Criterion {
            text: "checks pass".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Done,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: std::path::PathBuf::from(id.to_string()),
            mode: None,
        },
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
            max_tokens: None,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: Some(genre.into()),
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn outcome(index: usize, passed: bool, wall_ms: u64) -> RoutingOutcome {
    RoutingOutcome {
        outcome_id: format!("o{index}"),
        decision_id: format!("d{index}"),
        run_id: Some(format!("r{index}")),
        request_id: None,
        evaluation_version: "v1".into(),
        supersedes: None,
        acceptance_passed: Some(passed),
        review_passed: Some(passed),
        failed_criterion_ids: Vec::new(),
        failure_class: if passed { None } else { Some("error".into()) },
        cash_usd: None,
        tokens: None,
        wall_ms: Some(wall_ms),
        retries: None,
        reward: None,
    }
}

/// store を組む: task（genre×harness）＋根拠表＋`enforce`＋run 結果の events。
/// events の `ts` は store の時計（実時刻）なので、`now` は実時刻を注入する（sleep はしない）。
fn fixture(runs: usize, passed: bool) -> (SqliteStore, TaskId, OffsetDateTime) {
    fixture_with_evidence(runs, passed, true)
}

fn fixture_with_evidence(
    runs: usize,
    passed: bool,
    with_evidence: bool,
) -> (SqliteStore, TaskId, OffsetDateTime) {
    let store = SqliteStore::open_in_memory().expect("store");
    let now = OffsetDateTime::now_utc();
    let task = sample_task("coding", "codex", now);
    store.create_task(&task, Vec::new()).expect("task");
    if with_evidence {
        store
            .replace_routing_evidence("v1", &[evidence("1.2.0", 0.5, 10.0)])
            .expect("evidence");
    }
    store
        .set_routing_stage(
            task.id,
            &RoutingStageChange {
                genre: "coding".into(),
                harness_id: "codex".into(),
                stage: RoutingStage::Enforce,
                since: "now".into(),
                evidence_version: Some("v1".into()),
                reason: "human approved".into(),
                window: serde_json::json!({}),
                actor: "human".into(),
            },
        )
        .expect("stage");
    for index in 0..runs {
        store
            .append_event(
                task.id,
                &Event::RoutingOutcomeRecorded {
                    outcome: Box::new(outcome(index, passed, 10_000)),
                },
            )
            .expect("event");
    }
    (store, task.id, now)
}

fn notices(store: &SqliteStore) -> Vec<String> {
    store
        .notice_list(&NoticeQuery {
            unread_only: false,
            kinds: Vec::new(),
            limit: 50,
            offset: 0,
        })
        .expect("notices")
        .items
        .into_iter()
        .map(|notice| notice.title)
        .collect()
}

#[test]
fn run_tick_reverts_to_shadow_with_event_and_notice_once() {
    let (store, task_id, now) = fixture(10, false);
    let versions = BTreeMap::from([("codex".to_owned(), "1.2.0".to_owned())]);
    let report =
        run_tick(&store, &registry(), &FixedQuota(Some(0.5)), &versions, now).expect("tick");
    assert_eq!(report.evaluated, 1);
    assert_eq!(report.reverted, 1);
    assert_eq!(report.notices, 1);
    assert_eq!(report.harness_version_unknown, 0);
    assert_eq!(
        store.routing_stage("coding", "codex").expect("stage"),
        RoutingStage::Shadow
    );
    let events = store.events_for(task_id).expect("events");
    let reverted: Vec<_> = events
        .iter()
        .filter_map(|(_, event)| match event {
            Event::RoutingStageChanged {
                to,
                actor,
                reason,
                window,
                ..
            } if to == "shadow" => Some((actor.clone(), reason.clone(), window.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(reverted.len(), 1, "{reverted:?}");
    let (actor, reason, window) = &reverted[0];
    assert_eq!(actor, "daemon");
    assert!(reason.contains("pass_rate"), "{reason}");
    assert_eq!(window["runs"], serde_json::json!(10));
    assert_eq!(window["window"], serde_json::json!(50));
    assert!(window["reasons"].as_array().is_some_and(|r| !r.is_empty()));
    let titles = notices(&store);
    assert_eq!(titles.len(), 1, "{titles:?}");
    assert!(titles[0].contains("shadow"), "{titles:?}");

    // 同じ判定を次の tick で繰り返さない（既に shadow なので何もしない）。
    let again =
        run_tick(&store, &registry(), &FixedQuota(Some(0.5)), &versions, now).expect("tick again");
    assert_eq!(again.evaluated, 0);
    assert_eq!(again.reverted, 0);
    assert_eq!(again.notices, 0);
    assert_eq!(notices(&store).len(), 1);
    assert_eq!(
        store.events_for(task_id).expect("events").len(),
        events.len()
    );
}

#[test]
fn run_tick_quota_low_only_notifies_and_keeps_enforce() {
    let (store, task_id, now) = fixture(10, true);
    let versions = BTreeMap::from([("codex".to_owned(), "1.2.0".to_owned())]);
    let report =
        run_tick(&store, &registry(), &FixedQuota(Some(0.05)), &versions, now).expect("tick");
    assert_eq!(report.evaluated, 1);
    assert_eq!(report.reverted, 0);
    assert_eq!(report.notices, 1);
    assert_eq!(
        store.routing_stage("coding", "codex").expect("stage"),
        RoutingStage::Enforce
    );
    let events = store.events_for(task_id).expect("events");
    assert!(
        !events.iter().any(|(_, event)| matches!(
            event,
            Event::RoutingStageChanged { to, .. } if to == "shadow"
        )),
        "quota alone must not revert the stage"
    );
    let titles = notices(&store);
    assert_eq!(titles.len(), 1, "{titles:?}");
    assert!(titles[0].contains("quota"), "{titles:?}");

    // 同じ日の同じ通知は書き直さない（source_key が同じ）。
    let again =
        run_tick(&store, &registry(), &FixedQuota(Some(0.05)), &versions, now).expect("tick again");
    assert_eq!(again.notices, 0);
    assert_eq!(notices(&store).len(), 1);
    assert_eq!(
        store.routing_stage("coding", "codex").expect("stage"),
        RoutingStage::Enforce
    );
}

#[test]
fn run_tick_skips_pairs_without_evidence_rows() {
    let (store, _task_id, now) = fixture_with_evidence(10, false, false);
    let report = run_tick(
        &store,
        &registry(),
        &FixedQuota(Some(0.5)),
        &BTreeMap::new(),
        now,
    )
    .expect("tick");
    assert_eq!(report.evaluated, 0);
    assert_eq!(report.skipped_no_evidence, 1);
    assert_eq!(report.reverted, 0);
    assert_eq!(
        store.routing_stage("coding", "codex").expect("stage"),
        RoutingStage::Enforce
    );
}
