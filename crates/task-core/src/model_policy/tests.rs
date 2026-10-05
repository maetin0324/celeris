use super::*;
use crate::model::{Budget, Criterion, Status, TaskId, TaskRouting, WorkerHint};

fn f(
    judgment: Level,
    ambiguity: Level,
    verifiability: Level,
    reversibility: Level,
    consequence: Level,
) -> TaskFeatures {
    TaskFeatures {
        judgment,
        ambiguity,
        verifiability,
        reversibility,
        consequence,
        context_size: Level::Low,
        tool_intensity: Level::Medium,
        expected_length: Level::Low,
        cross_cutting: Level::Low,
    }
}

pub(crate) fn task(objective: &str, acceptance: Vec<Criterion>) -> Task {
    let now = time::OffsetDateTime::UNIX_EPOCH;
    Task {
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: "t".into(),
        objective: objective.into(),
        acceptance,
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 10,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::local("/tmp/x"),
        repos: vec![],
        budget: Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: Some("coding".into()),
        aggregate: false,
        project_id: None,
        milestone_id: None,
        assignee: None,
        labels: vec![],
        category: TaskCategory::Other,
        skills: vec![],
        mode: TaskMode::Production,
        conversation: None,
        routing: Some(TaskRouting::default()),
    }
}

fn cmd() -> Criterion {
    Criterion {
        text: "tests pass".into(),
        check: Check::Command {
            cmd: "cargo test".into(),
            expect_exit: 0,
        },
    }
}

fn reviewer() -> Criterion {
    Criterion {
        text: "looks right".into(),
        check: Check::Reviewer,
    }
}

/// 表駆動: 3 つの基本ケースと境界の組み合わせ。
#[test]
fn rule_table_maps_features_to_lanes() {
    use Level::*;
    let open = LaneCeiling::default();
    let cases: &[(TaskFeatures, Tier, &str)] = &[
        // 基本 1: 機械的 + 検証しやすい + 戻せる → cheap
        (
            f(Low, Low, High, High, Low),
            Tier::Cheap,
            "cheap/mechanical-verifiable-reversible",
        ),
        // 基本 2: 判断が重い + 曖昧 → frontier
        (
            f(High, High, Medium, High, Medium),
            Tier::Frontier,
            "frontier/judgment-under-uncertainty",
        ),
        // 基本 3: どちらでもない → standard
        (
            f(Medium, Medium, Medium, High, Medium),
            Tier::Standard,
            "standard/default",
        ),
        // 判断が重く、検証できない → frontier
        (
            f(High, Low, Low, High, Low),
            Tier::Frontier,
            "frontier/judgment-under-uncertainty",
        ),
        // 損失が大きく検証できない → frontier（判断は中）
        (
            f(Medium, Medium, Low, Medium, High),
            Tier::Frontier,
            "frontier/costly-and-unverifiable",
        ),
        // 機械的だが損失が大きい → cheap にしない
        (
            f(Low, Low, High, High, High),
            Tier::Standard,
            "standard/default",
        ),
        // 機械的だが戻せない → cheap にしない
        (
            f(Low, Low, High, Low, Low),
            Tier::Standard,
            "standard/default",
        ),
        // 判断が重いが検証でき、曖昧でもない → standard（単一スコアなら frontier 寄りになり得る）
        (
            f(High, Low, High, High, Medium),
            Tier::Standard,
            "standard/default",
        ),
    ];
    for (features, lane, rule) in cases {
        let d = ModelPolicy.decide(features, &open);
        assert_eq!(d.lane, *lane, "{features:?}");
        assert_eq!(d.rule_id, *rule, "{features:?}");
        assert_eq!(d.policy_version, LANE_POLICY_VERSION);
        assert!(d.clamped_by.is_none());
    }
    // 判断が重く横断的 → frontier/broad-judgment
    let mut broad = f(High, Low, High, High, Medium);
    broad.cross_cutting = High;
    assert_eq!(
        ModelPolicy.decide(&broad, &open).rule_id,
        "frontier/broad-judgment"
    );
}

#[test]
fn org_ceiling_clamps_the_lane_and_records_why() {
    use Level::*;
    let hard = f(High, High, Low, Low, High);
    let ceiling = LaneCeiling {
        allowed: vec![Tier::Standard, Tier::Cheap],
        max_lane: None,
    };
    let d = ModelPolicy.decide(&hard, &ceiling);
    assert_eq!(d.proposed, Tier::Frontier);
    assert_eq!(d.lane, Tier::Standard);
    assert!(
        d.clamped_by
            .as_deref()
            .is_some_and(|c| c.contains("frontier -> standard"))
    );
    // max_lane は allowed より厳しければ勝つ
    let ceiling = LaneCeiling {
        allowed: vec![],
        max_lane: Some(Tier::Cheap),
    };
    assert_eq!(ModelPolicy.decide(&hard, &ceiling).lane, Tier::Cheap);
    // 下に許される lane が無ければ上の最も近いもの
    let ceiling = LaneCeiling {
        allowed: vec![Tier::Frontier],
        max_lane: None,
    };
    let easy = f(Low, Low, High, High, Low);
    assert_eq!(ModelPolicy.decide(&easy, &ceiling).lane, Tier::Frontier);
    assert_eq!(ceiling.top(), Some(Tier::Frontier));
}

#[test]
fn infer_is_deterministic_and_reads_the_task() {
    let mechanical = task("crates/task-core/src/model.rs の typo を直す", vec![cmd()]);
    let d = decide_for_task(&mechanical, &LaneCeiling::default()).unwrap();
    assert_eq!(d.lane, Tier::Cheap, "{:?}", d.features);

    let mut design = task(
        "ルーティングの方針を設計し、トレードオフを比較検討する",
        vec![reviewer()],
    );
    design.genre = Some("writing".into());
    let d = decide_for_task(&design, &LaneCeiling::default()).unwrap();
    assert_eq!(d.lane, Tier::Frontier, "{:?}", d.features);

    let normal = task(
        "API に新しいエンドポイントを足してテストを書く。既存のハンドラと同じ形にする。",
        vec![cmd(), reviewer()],
    );
    let d = decide_for_task(&normal, &LaneCeiling::default()).unwrap();
    assert_eq!(d.lane, Tier::Standard, "{:?}", d.features);
    assert_eq!(TaskFeatures::infer(&normal), TaskFeatures::infer(&normal));
}

#[test]
fn hints_override_axes_and_explicit_tiers_skip_the_policy() {
    let mut t = task("crates/x.rs の typo を直す", vec![cmd()]);
    t.routing = Some(TaskRouting {
        tier_source: TierSource::Hint,
        features: Some(TaskFeatureHints {
            judgment: Some(Level::High),
            ambiguity: Some(Level::High),
            ..TaskFeatureHints::default()
        }),
        ..TaskRouting::default()
    });
    t.worker_hint.tier = Tier::Cheap;
    let d = decide_for_task(&t, &LaneCeiling::default()).unwrap();
    assert_eq!(d.lane, Tier::Frontier);
    assert_eq!(d.hint, Some(Tier::Cheap));
    assert!(d.reasons.iter().any(|r| r.contains("judgment, ambiguity")));

    // 人の明示は policy も天井も当てない
    t.routing = Some(TaskRouting {
        tier_source: TierSource::Human,
        ..TaskRouting::default()
    });
    let ceiling = LaneCeiling {
        allowed: vec![Tier::Frontier],
        max_lane: None,
    };
    let d = decide_for_task(&t, &ceiling).unwrap();
    assert_eq!(
        (d.lane, d.rule_id.as_str()),
        (Tier::Cheap, "explicit/human")
    );

    // routing の無い既存タスク・execute 以外は対象外
    t.routing = None;
    assert!(decide_for_task(&t, &ceiling).is_none());
    t.routing = Some(TaskRouting::default());
    t.kind = TaskKind::Plan;
    assert!(decide_for_task(&t, &ceiling).is_none());
}

#[test]
fn decision_serializes_with_the_audit_fields_and_old_tasks_still_parse() {
    let t = task("x", vec![cmd()]);
    let d = decide_for_task(&t, &LaneCeiling::default()).unwrap();
    let v = serde_json::to_value(&d).unwrap();
    for key in ["lane", "rule_id", "policy_version", "features", "reasons"] {
        assert!(v.get(key).is_some(), "{key} missing in {v}");
    }
    assert!(v.get("shadow").is_none());
    // 導入前のタスク（`routing` 無し）はそのまま読める
    let mut old = serde_json::to_value(&t).unwrap();
    old.as_object_mut().unwrap().remove("routing");
    let back: Task = serde_json::from_value(old).unwrap();
    assert!(back.routing.is_none());
}

// ---- ADR-0074 D5.1/D5.2（Phase F1）: WU ごとの features と lane の上限 ----

fn wu_row(features: Option<serde_json::Value>, checks: Vec<Criterion>) -> crate::WorkUnitRow {
    use crate::execution_plan::{
        WorkUnitCheck, WorkUnitContext, WorkUnitKind, WorkUnitSpec, WorkUnitStatus,
    };
    let spec = WorkUnitSpec {
        key: "a".to_string(),
        kind: WorkUnitKind::Implement,
        title: "a".to_string(),
        objective: "do a".to_string(),
        depends_on: vec![],
        done_when: vec![],
        checks: checks
            .into_iter()
            .filter_map(|c| match c.check {
                Check::Command { cmd, expect_exit } => Some(WorkUnitCheck {
                    cmd,
                    expect_exit,
                    scope: false,
                }),
                _ => None,
            })
            .collect(),
        context: WorkUnitContext::default(),
        harness: None,
        features,
        budget: None,
        outputs: vec![],
        phase: None,
    };
    crate::WorkUnitRow::new(
        "wu-a".to_string(),
        "task".to_string(),
        "plan".to_string(),
        0,
        spec,
        WorkUnitStatus::Ready,
        "2026-09-26T00:00:00Z".to_string(),
    )
}

/// ADR-0074 §6 F1 (c): judgment=low/ambiguity=low/verifiability=high/reversibility=high と
/// `checks` を持つ WU は cheap になる。
#[test]
fn work_unit_features_lower_a_mechanical_unit_to_cheap() {
    let t = task(
        "設計方針を比較検討する（Task 自体は判断が重い）",
        vec![reviewer()],
    );
    let wu = wu_row(
        Some(serde_json::json!({
            "judgment": "low", "ambiguity": "low", "verifiability": "high",
            "reversibility": "high", "consequence": "low"
        })),
        vec![cmd()],
    );
    let d = decide_for_work_unit(&t, &wu, &LaneCeiling::default(), None)
        .expect("decision for execute task");
    assert_eq!(d.lane, Tier::Cheap, "{d:?}");
}

/// features が無い WU は、Task の `routing.features`（明示のヒント）をそのまま継ぐ
/// （WU 自身の `features` が上書きするのは、WU にそれがあるときだけ）。
#[test]
fn work_unit_without_features_inherits_the_task_hints() {
    let mut t = task("do a", vec![cmd()]);
    t.routing = Some(TaskRouting {
        features: Some(TaskFeatureHints {
            judgment: Some(Level::High),
            ambiguity: Some(Level::High),
            ..TaskFeatureHints::default()
        }),
        ..TaskRouting::default()
    });
    let wu = wu_row(None, vec![cmd()]);
    let d = decide_for_work_unit(&t, &wu, &LaneCeiling::default(), None).unwrap();
    assert_eq!(d.lane, Tier::Frontier, "{d:?}");
}

/// (c): Task が standard のとき、WU の features が frontier を示しても standard に丸まり、
/// `clamped_by` に理由が残る（D5.2 の上限 = max(Task の lane, standard)）。
#[test]
fn work_unit_lane_is_capped_by_the_task_lane() {
    let t = task(
        "API に新しいエンドポイントを足してテストを書く。既存のハンドラと同じ形にする。",
        vec![cmd(), reviewer()],
    );
    // Task 自身は standard（判断も曖昧さも中程度）。
    let task_decision = decide_for_task(&t, &LaneCeiling::default()).unwrap();
    assert_eq!(task_decision.lane, Tier::Standard, "{task_decision:?}");

    let wu = wu_row(
        Some(serde_json::json!({
            "judgment": "high", "ambiguity": "high", "verifiability": "medium",
            "reversibility": "high", "consequence": "medium"
        })),
        vec![],
    );
    // 上限を掛けない（`work_unit_lane_cap = "none"` に相当）: そのまま frontier。
    let uncapped = decide_for_work_unit(&t, &wu, &LaneCeiling::default(), None).unwrap();
    assert_eq!(uncapped.lane, Tier::Frontier, "{uncapped:?}");
    assert!(uncapped.clamped_by.is_none());

    // 上限を掛ける（既定 `work_unit_lane_cap = "task"`）: standard に丸まる。
    let capped =
        decide_for_work_unit(&t, &wu, &LaneCeiling::default(), Some(task_decision.lane)).unwrap();
    assert_eq!(capped.lane, Tier::Standard, "{capped:?}");
    assert!(
        capped
            .clamped_by
            .as_deref()
            .is_some_and(|c| c.contains("work-unit lane cap")),
        "{capped:?}"
    );
}

/// D5.2: 上限は「下げる」方向には効かない。Task が standard でも、WU の features が cheap を
/// 示せば cheap のまま。
#[test]
fn work_unit_lane_cap_does_not_prevent_routing_cheaper_than_the_task() {
    let t = task(
        "API に新しいエンドポイントを足してテストを書く。既存のハンドラと同じ形にする。",
        vec![cmd(), reviewer()],
    );
    let task_decision = decide_for_task(&t, &LaneCeiling::default()).unwrap();
    assert_eq!(task_decision.lane, Tier::Standard, "{task_decision:?}");
    let wu = wu_row(
        Some(serde_json::json!({
            "judgment": "low", "ambiguity": "low", "verifiability": "high",
            "reversibility": "high", "consequence": "low"
        })),
        vec![cmd()],
    );
    let d =
        decide_for_work_unit(&t, &wu, &LaneCeiling::default(), Some(task_decision.lane)).unwrap();
    assert_eq!(d.lane, Tier::Cheap, "{d:?}");
}

/// Task 自身が frontier のとき、WU の上限は `max(frontier, standard) = frontier` になる
/// （frontier の WU をさらに下げない）。
#[test]
fn work_unit_lane_cap_follows_a_frontier_task() {
    let mut t = task(
        "ルーティングの方針を設計し、トレードオフを比較検討する",
        vec![reviewer()],
    );
    t.genre = Some("writing".into());
    let task_decision = decide_for_task(&t, &LaneCeiling::default()).unwrap();
    assert_eq!(task_decision.lane, Tier::Frontier, "{task_decision:?}");
    let wu = wu_row(
        Some(serde_json::json!({
            "judgment": "high", "ambiguity": "high", "verifiability": "low",
            "reversibility": "high", "consequence": "medium"
        })),
        vec![],
    );
    let d =
        decide_for_work_unit(&t, &wu, &LaneCeiling::default(), Some(task_decision.lane)).unwrap();
    assert_eq!(d.lane, Tier::Frontier, "{d:?}");
}

/// D5.1: 5 軸のうち一部だけを書いた WU は拒否されず（`execution_plan::validate` の役目とは別）、
/// `reasons` に `features_source: work_unit(partial)` が残る。
#[test]
fn partial_work_unit_features_are_noted_as_partial() {
    let t = task("x", vec![cmd()]);
    let wu = wu_row(Some(serde_json::json!({"judgment": "low"})), vec![cmd()]);
    let d = decide_for_work_unit(&t, &wu, &LaneCeiling::default(), None).unwrap();
    assert!(
        d.reasons
            .iter()
            .any(|r| r == "features_source: work_unit(partial)"),
        "{d:?}"
    );
}
