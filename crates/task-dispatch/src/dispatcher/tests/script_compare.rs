//! 付記 S2: `decide_lane` の `lane_rules` の compare。偽の `PolicyScript` だけを使い、V8 は起こさない。
//! どの場合も `routing_decided` の lane の判定は mode rust・PolicyScript 無しのときと同じであることを確かめる。

use super::lane_assist::{hinted_task, question_adapter, routing_decided};
use super::*;
use crate::script_policy::{
    LaneRulesOutput, PolicyFuture, PolicyReply, PolicyScript, PolicyScriptBundle,
    PolicyScriptError, SCRIPT_CALL_DEADLINE,
};
use task_core::script_compare::{
    CompareOutcome, LaneRulesInput, NewScriptGeneration, RoutePriorityInput, RoutePriorityOutput,
    ScriptCompareRecord, ScriptGenerationState, ScriptMode, ScriptModeChange,
};

const BUNDLE: &str = "ab";

pub(super) fn bundle_sha() -> String {
    BUNDLE.repeat(32)
}

/// 偽の TS の振る舞い（両領域に同じ振る舞いを当てる）。
#[derive(Clone)]
pub(super) enum Behavior {
    /// Rust と同じ答え（`lane_rules` は `match_rule`、`route_priority` は付記 S2-10 の規則を入力 DTO に当てる）。
    Echo,
    /// 決まった値を返す。
    Fixed(serde_json::Value),
    Fail(PolicyScriptError),
    /// 返らない（dispatcher の deadline で打ち切られる）。
    Hang,
}

pub(super) struct FakeScript {
    pub(super) sha: String,
    pub(super) behavior: Behavior,
    /// `lane_rules` の呼び出し回数。
    pub(super) calls: AtomicUsize,
    /// `route_priority` の呼び出し回数と受けた入力。
    pub(super) route_inputs: StdMutex<Vec<RoutePriorityInput>>,
    /// `cos_digest_pass` の呼び出しの `pass_id`（付記 S4-12）。
    pub(super) digest_passes: StdMutex<Vec<String>>,
}

impl FakeScript {
    pub(super) fn new(behavior: Behavior) -> Arc<Self> {
        Self::with_sha(bundle_sha(), behavior)
    }

    pub(super) fn with_sha(sha: String, behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            sha,
            behavior,
            calls: AtomicUsize::new(0),
            route_inputs: StdMutex::new(Vec::new()),
            digest_passes: StdMutex::new(Vec::new()),
        })
    }

    pub(super) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub(super) fn route_calls(&self) -> usize {
        self.route_inputs.lock().unwrap().len()
    }
}

/// 付記 S2-10 の規則を入力 DTO だけから当てる（偽の TS の「Rust と同じ答え」）。
fn echo_route_priority(input: &RoutePriorityInput) -> RoutePriorityOutput {
    use task_core::script_compare::RoutePriorityBasis as B;
    let t = &input.table;
    let lane = input.lane.clone().unwrap_or_default();
    let class = input
        .execution_class
        .as_ref()
        .and_then(|c| t.class_defaults.iter().find(|x| &x.execution_class == c));
    let (basis, candidates) = if input.cos {
        (B::Cos, t.cos_route_candidates.clone())
    } else if let Some(class) = class {
        (B::ExecutionClass, class.candidates.clone())
    } else if let Some(l) = t.lane_route_defaults.iter().find(|x| x.lane == lane) {
        (B::Lane, l.candidates.clone())
    } else {
        (B::Harness, t.route_defaults.clone())
    };
    let basis = if candidates.is_empty() {
        B::None
    } else {
        basis
    };
    RoutePriorityOutput { basis, candidates }
}

impl PolicyScript for FakeScript {
    fn bundle(&self) -> PolicyScriptBundle {
        PolicyScriptBundle {
            bundle_sha256: self.sha.clone(),
            host_api: "1.0.0".into(),
        }
    }

    fn lane_rules(&self, input: LaneRulesInput, _deadline: Duration) -> PolicyFuture<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let behavior = self.behavior.clone();
        Box::pin(async move {
            match behavior {
                Behavior::Echo => Ok(PolicyReply {
                    output: serde_json::to_value(LaneRulesOutput::from(
                        task_core::model_policy::match_rule(&input.features),
                    ))
                    .unwrap(),
                    isolate_epoch: 1,
                }),
                Behavior::Fixed(output) => Ok(PolicyReply {
                    output,
                    isolate_epoch: 1,
                }),
                Behavior::Fail(e) => Err(e),
                Behavior::Hang => {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Err(PolicyScriptError::new("exception", "woke up"))
                }
            }
        })
    }

    fn route_priority(&self, input: RoutePriorityInput, _deadline: Duration) -> PolicyFuture<'_> {
        self.route_inputs.lock().unwrap().push(input.clone());
        let behavior = self.behavior.clone();
        Box::pin(async move {
            match behavior {
                Behavior::Echo => Ok(PolicyReply {
                    output: serde_json::to_value(echo_route_priority(&input)).unwrap(),
                    isolate_epoch: 1,
                }),
                Behavior::Fixed(output) => Ok(PolicyReply {
                    output,
                    isolate_epoch: 1,
                }),
                Behavior::Fail(e) => Err(e),
                Behavior::Hang => {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Err(PolicyScriptError::new("exception", "woke up"))
                }
            }
        })
    }

    fn cos_digest_pass(
        &self,
        call: crate::script_policy::CosDigestPassCall,
        _deadline: Duration,
    ) -> PolicyFuture<'_> {
        self.digest_passes
            .lock()
            .unwrap()
            .push(call.pass_id.clone());
        let behavior = self.behavior.clone();
        Box::pin(async move {
            match behavior {
                Behavior::Echo => crate::script_policy::test_pass::reference_reply(&call).await,
                Behavior::Fixed(output) => Ok(PolicyReply {
                    output,
                    isolate_epoch: 1,
                }),
                Behavior::Fail(e) => Err(e),
                Behavior::Hang => {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Err(PolicyScriptError::new("exception", "woke up"))
                }
            }
        })
    }
}

struct Outcome {
    decision: task_core::LaneDecision,
    records: Vec<ScriptCompareRecord>,
    task: Task,
    _ws: tempfile::TempDir,
}

/// 1 件の task を 1 tick だけ dispatch し、走った compare を待つ。`compare` が真なら `lane_rules` を compare にする。
async fn run_once(
    compare: bool,
    script: Option<Arc<FakeScript>>,
    source: task_core::TierSource,
    edit: impl FnOnce(&mut Task, &SqliteStore),
) -> Outcome {
    let ws = tempfile::tempdir().unwrap();
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    let mut task = hinted_task(ws.path(), source);
    edit(&mut task, &sqlite);
    store.insert(&task).unwrap();
    let now = OffsetDateTime::now_utc();
    let generation = sqlite
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: bundle_sha(),
                host_api: "1.0.0".into(),
                schema_sha256: "0".repeat(64),
                manifest_json: r#"{"domains":["lane_rules"]}"#.into(),
                state: ScriptGenerationState::Loaded,
            },
            now,
        )
        .unwrap();
    if compare {
        sqlite
            .script_domain_set_mode(
                &ScriptModeChange {
                    domain: "lane_rules".into(),
                    to_mode: ScriptMode::Compare,
                    to_generation: Some(generation.id.clone()),
                    expected_version: 0,
                    actor: "human:test".into(),
                    reason: "script_compare test".into(),
                },
                now,
            )
            .unwrap();
    }
    let mut d = dispatcher(store.clone(), question_adapter(), 1);
    d.set_script_domain_store(sqlite.clone());
    d.set_policy_script(script.map(|s| s as Arc<dyn PolicyScript>));
    d.tick().unwrap();
    d.wait_script_compares().await;
    let records = store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::ScriptCompareRecorded { record } => Some(*record),
            _ => None,
        })
        .collect();
    Outcome {
        decision: routing_decided(&store, &task).decision,
        records,
        task,
        _ws: ws,
    }
}

/// mode rust・PolicyScript 無しの判定（比べる基準）。
async fn baseline(
    source: task_core::TierSource,
    edit: impl FnOnce(&mut Task, &SqliteStore),
) -> task_core::LaneDecision {
    let out = run_once(false, None, source, edit).await;
    assert!(out.records.is_empty());
    out.decision
}

fn only(records: &[ScriptCompareRecord]) -> &ScriptCompareRecord {
    assert_eq!(records.len(), 1, "{records:?}");
    &records[0]
}

#[tokio::test]
async fn script_compare_match_records_and_keeps_rust_lane() {
    let base = baseline(task_core::TierSource::Hint, |_, _| {}).await;
    let script = FakeScript::new(Behavior::Echo);
    let out = run_once(
        true,
        Some(script.clone()),
        task_core::TierSource::Hint,
        |_, _| {},
    )
    .await;
    assert_eq!(script.calls(), 1);
    assert_eq!(out.decision, base);
    let r = only(&out.records);
    assert_eq!(r.outcome, CompareOutcome::Match);
    assert_eq!(r.call_site, "dispatch_run.decide_lane");
    assert_eq!(r.bundle_sha256, bundle_sha());
    assert_eq!(r.isolate_epoch, 1);
    assert_eq!(r.run_id, None);
    assert_eq!(r.rust, r.ts.clone().unwrap());
    assert_eq!(r.rust["rule_id"], serde_json::json!(base.rule_id));
    // 記録した入力は 9 軸の features だけ（S2-3）。
    let input = r.input.as_ref().unwrap();
    assert_eq!(input.as_object().unwrap().len(), 1);
    assert_eq!(input["features"].as_object().unwrap().len(), 9);
    assert!(r.safety_violations.is_empty() && r.normalized.is_empty());
    assert!(r.error.is_none());
}

#[tokio::test]
async fn script_compare_mismatch_records_and_keeps_rust_lane() {
    let base = baseline(task_core::TierSource::Hint, |_, _| {}).await;
    let ts = serde_json::json!({
        "rule_id": "frontier/always",
        "proposed": "frontier",
        "condition": "always frontier",
    });
    let script = FakeScript::new(Behavior::Fixed(ts.clone()));
    let out = run_once(
        true,
        Some(script.clone()),
        task_core::TierSource::Hint,
        |_, _| {},
    )
    .await;
    assert_eq!(script.calls(), 1);
    assert_eq!(out.decision, base);
    assert_ne!(base.lane, Tier::Frontier, "fixture must differ from TS");
    let r = only(&out.records);
    assert_eq!(r.outcome, CompareOutcome::Mismatch);
    assert_eq!(r.ts.as_ref(), Some(&ts));
}

#[tokio::test]
async fn script_compare_ts_error_and_invalid_output_keep_rust_lane() {
    let base = baseline(task_core::TierSource::Hint, |_, _| {}).await;
    let cases = [
        (
            Behavior::Fail(PolicyScriptError::new("exception", "boom")),
            CompareOutcome::TsError,
            "exception",
        ),
        (
            Behavior::Fixed(
                serde_json::json!({"rule_id": "x", "proposed": "huge", "condition": ""}),
            ),
            CompareOutcome::InvalidOutput,
            "invalid_output",
        ),
        (
            Behavior::Fixed(
                serde_json::json!({"rule_id": "x", "proposed": "cheap", "condition": "", "extra": 1}),
            ),
            CompareOutcome::InvalidOutput,
            "invalid_output",
        ),
        (
            Behavior::Fixed(serde_json::Value::Null),
            CompareOutcome::InvalidOutput,
            "invalid_output",
        ),
    ];
    for (behavior, outcome, kind) in cases {
        let script = FakeScript::new(behavior);
        let out = run_once(
            true,
            Some(script.clone()),
            task_core::TierSource::Hint,
            |_, _| {},
        )
        .await;
        assert_eq!(script.calls(), 1);
        assert_eq!(out.decision, base, "lane must stay Rust's for {outcome:?}");
        let r = only(&out.records);
        assert_eq!(r.outcome, outcome);
        assert_eq!(r.error.as_ref().unwrap().kind, kind);
    }
}

#[tokio::test(start_paused = true)]
async fn script_compare_timeout_records_ts_error_and_keeps_rust_lane() {
    let base = baseline(task_core::TierSource::Hint, |_, _| {}).await;
    let script = FakeScript::new(Behavior::Hang);
    let out = run_once(
        true,
        Some(script.clone()),
        task_core::TierSource::Hint,
        |_, _| {},
    )
    .await;
    assert_eq!(script.calls(), 1);
    assert_eq!(out.decision, base);
    let r = only(&out.records);
    assert_eq!(r.outcome, CompareOutcome::TsError);
    assert_eq!(r.error.as_ref().unwrap().kind, "timeout");
    let deadline_us = u64::try_from(SCRIPT_CALL_DEADLINE.as_micros()).unwrap();
    assert!(
        r.elapsed_us >= deadline_us && r.elapsed_us < 2 * deadline_us,
        "elapsed {} us",
        r.elapsed_us
    );
}

#[tokio::test]
async fn script_compare_mode_rust_never_calls_ts() {
    let base = baseline(task_core::TierSource::Hint, |_, _| {}).await;
    let script = FakeScript::new(Behavior::Echo);
    let out = run_once(
        false,
        Some(script.clone()),
        task_core::TierSource::Hint,
        |_, _| {},
    )
    .await;
    assert_eq!(script.calls(), 0);
    assert!(out.records.is_empty());
    assert_eq!(out.decision, base);
}

#[tokio::test]
async fn script_compare_without_policy_script_records_unavailable() {
    let base = baseline(task_core::TierSource::Hint, |_, _| {}).await;
    let out = run_once(true, None, task_core::TierSource::Hint, |_, _| {}).await;
    assert_eq!(out.decision, base);
    let r = only(&out.records);
    assert_eq!(r.outcome, CompareOutcome::Unavailable);
    assert!(r.ts.is_none());
}

#[tokio::test]
async fn script_compare_other_bundle_is_unavailable_and_not_called() {
    let base = baseline(task_core::TierSource::Hint, |_, _| {}).await;
    let script = FakeScript::with_sha("cd".repeat(32), Behavior::Echo);
    let out = run_once(
        true,
        Some(script.clone()),
        task_core::TierSource::Hint,
        |_, _| {},
    )
    .await;
    assert_eq!(script.calls(), 0);
    assert_eq!(out.decision, base);
    assert_eq!(only(&out.records).outcome, CompareOutcome::Unavailable);
}

#[tokio::test]
async fn script_compare_human_tier_never_calls_ts() {
    let edit = |t: &mut Task, _: &SqliteStore| t.worker_hint.tier = Tier::Cheap;
    let base = baseline(task_core::TierSource::Human, edit).await;
    let script = FakeScript::new(Behavior::Fixed(serde_json::json!({
        "rule_id": "frontier/always", "proposed": "frontier", "condition": "always",
    })));
    let out = run_once(
        true,
        Some(script.clone()),
        task_core::TierSource::Human,
        edit,
    )
    .await;
    assert_eq!(script.calls(), 0);
    assert!(out.records.is_empty());
    assert_eq!(out.decision, base);
    assert_eq!(out.decision.lane, Tier::Cheap);
    assert_eq!(out.decision.rule_id, "explicit/human");
}

/// 担当の天井（max_lane = standard）で丸めた後の lane は、TS が何を返しても Rust と同じ。
#[tokio::test]
async fn script_compare_lane_after_ceiling_is_rust() {
    let edit = |t: &mut Task, sqlite: &SqliteStore| {
        let now = OffsetDateTime::now_utc();
        for (id, parent, kind, max_lane) in [
            ("secretary", None, OrgKind::Secretary, None),
            (
                "coding",
                Some("secretary"),
                OrgKind::Department,
                Some(Tier::Standard),
            ),
        ] {
            let mut profile = task_core::profile::Profile::default();
            profile.budget.max_lane = max_lane;
            sqlite
                .org_upsert(&OrgNode {
                    profile,
                    id: id.into(),
                    parent_id: parent.map(str::to_string),
                    name: id.into(),
                    kind,
                    genre: None,
                    brief: String::new(),
                    position: 0,
                    created_at: now,
                    updated_at: now,
                })
                .unwrap();
        }
        t.assignee = Some("coding".into());
        if let Some(routing) = t.routing.as_mut() {
            routing.features = Some(task_core::model_policy::TaskFeatureHints {
                consequence: Some(task_core::model_policy::Level::High),
                verifiability: Some(task_core::model_policy::Level::Low),
                ..Default::default()
            });
        }
    };
    let base = baseline(task_core::TierSource::Hint, edit).await;
    assert_eq!(base.proposed, Tier::Frontier);
    assert_eq!(base.lane, Tier::Standard);
    assert!(base.clamped_by.is_some());
    for ts in [
        serde_json::json!({"rule_id": "cheap/always", "proposed": "cheap", "condition": "c"}),
        serde_json::json!({"rule_id": "frontier/always", "proposed": "frontier", "condition": "f"}),
    ] {
        let script = FakeScript::new(Behavior::Fixed(ts));
        let out = run_once(
            true,
            Some(script.clone()),
            task_core::TierSource::Hint,
            edit,
        )
        .await;
        assert_eq!(script.calls(), 1);
        assert_eq!(out.decision, base);
        assert_eq!(out.task.assignee.as_deref(), Some("coding"));
        let r = only(&out.records);
        assert_eq!(r.outcome, CompareOutcome::Mismatch);
        assert!(r.safety_violations.is_empty());
    }
}

/// 付記 S2-11: `route_priority` の compare の試験（`tests/mod.rs` を変えずに足すため子 module にした）。
#[path = "script_compare_route.rs"]
mod script_compare_route;
