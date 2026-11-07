//! ADR 2026-11-07-health-review-integration D3.1〜D3.3 の試験（prefix `design_brief_`）。

use super::super::*;

fn fixture(name: &str) -> ExecutionPlanSpec {
    let path = format!(
        "{}/testdata/execution-plan/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn tree_on() -> ExecutionLimits {
    ExecutionLimits {
        tree: crate::tree::TreeLimits {
            enabled: true,
            ..crate::tree::TreeLimits::default()
        },
        ..ExecutionLimits::default()
    }
}

fn good_brief() -> DesignBrief {
    DesignBrief {
        root_cause: "retry state lives in two places".into(),
        owner: "crates/task-core/src/store".into(),
        reuse: vec!["existing replan trigger".into()],
        options: vec![
            DesignOption {
                name: "move to store".into(),
                chosen: true,
                reason: "single source of truth".into(),
            },
            DesignOption {
                name: "sync both".into(),
                chosen: false,
                reason: "keeps the dual state".into(),
            },
        ],
        removals: vec!["dispatcher in-memory retry map".into()],
        invariants: vec!["state and event commit atomically".into()],
        ..DesignBrief::default()
    }
}

fn brief_errors(spec: &ExecutionPlanSpec) -> Vec<PlanValidationError> {
    match validate(spec, tree_on(), &[]) {
        Ok(_) => Vec::new(),
        Err(errs) => errs
            .into_iter()
            .filter(|e| matches!(e, PlanValidationError::DesignBrief { .. }))
            .collect(),
    }
}

/// 正しい brief は検証を通り、JSON の往復で変わらない。brief の無い /3 は今までどおり通り、JSON に欄を出さない。
#[test]
fn design_brief_valid_brief_is_accepted_and_round_trips() {
    let mut spec = fixture("v3-browser.json");
    assert!(
        !serde_json::to_string(&spec)
            .unwrap()
            .contains("design_brief")
    );
    validate(&spec, tree_on(), &[]).expect("plan without brief still validates");
    spec.design_brief = Some(good_brief());
    let validated = validate(&spec, tree_on(), &[]).expect("valid brief");
    assert_eq!(validated.spec.design_brief, Some(good_brief()));
    let json = serde_json::to_string(&spec).unwrap();
    let back: ExecutionPlanSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(back, spec);
    // 未知の欄は読み込みで拒否する（deny_unknown_fields）。
    let bad = json.replace("\"root_cause\"", "\"risk\":\"low\",\"root_cause\"");
    assert!(serde_json::from_str::<ExecutionPlanSpec>(&bad).is_err());
}

/// 必須項目が空・選択肢の誤りは `DesignBrief { field }` で拒否する。
#[test]
fn design_brief_empty_required_fields_are_rejected() {
    let mut spec = fixture("v3-browser.json");
    let mut brief = good_brief();
    brief.root_cause = "  ".into();
    brief.owner = String::new();
    brief.removals = vec![String::new()];
    spec.design_brief = Some(brief);
    let fields: Vec<_> = brief_errors(&spec)
        .into_iter()
        .map(|e| match e {
            PlanValidationError::DesignBrief { field, .. } => field,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(fields, vec!["root_cause", "owner", "removals"]);

    let mut brief = good_brief();
    brief.options.truncate(1);
    spec.design_brief = Some(brief);
    let errs = brief_errors(&spec);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].to_string().contains("at least 2 options"));
}

#[test]
fn design_brief_requires_one_chosen_option() {
    let mut spec = fixture("v3-browser.json");
    for chosen in [[false, false], [true, true]] {
        let mut brief = good_brief();
        for (o, c) in brief.options.iter_mut().zip(chosen) {
            o.chosen = c;
        }
        spec.design_brief = Some(brief);
        let errs = brief_errors(&spec);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(
            errs[0].to_string().contains("exactly one option"),
            "{errs:?}"
        );
    }
}

/// 欠けた brief: 高 risk の class に触れる planner の計画は `DesignBriefMissing` で差し戻し、人の計画と
/// class の無い計画は brief を求めない（D3.3）。
#[test]
fn design_brief_missing_on_high_risk_path_is_rejected() {
    let mut spec = fixture("v3-browser.json");
    let classes = vec!["dispatch-state".to_string(), "dispatch-state".to_string()];
    let err = require_design_brief(&spec, PlanOrigin::Planner, &classes).unwrap_err();
    assert_eq!(
        err,
        PlanValidationError::DesignBriefMissing {
            classes: vec!["dispatch-state".into()]
        }
    );
    assert!(err.to_string().contains("dispatch-state"));
    require_design_brief(&spec, PlanOrigin::Human, &classes).expect("human plan");
    require_design_brief(&spec, PlanOrigin::Planner, &[]).expect("no high-risk class");
    spec.design_brief = Some(good_brief());
    require_design_brief(&spec, PlanOrigin::Planner, &classes).expect("brief present");
}

/// 高 risk 宣言: `new_state`・`new_fallback` などを宣言した brief は形として通り、`declared_new` が
/// 宣言の種類を返す（risk 判定の入力。D3.3）。宣言の無い brief は空。
#[test]
fn design_brief_high_risk_declaration_is_accepted_and_reported() {
    let mut spec = fixture("v3-browser.json");
    assert!(good_brief().declared_new().is_empty());
    let mut brief = good_brief();
    brief.new_state = vec!["events: DesignReviewRequested".into()];
    brief.new_fallback = vec!["fall back to the old path when policy is missing".into()];
    brief.new_port = vec!["CLI --print-review-policy".into()];
    assert_eq!(
        brief.declared_new(),
        vec!["new_state", "new_fallback", "new_port"]
    );
    spec.design_brief = Some(brief.clone());
    let validated = validate(&spec, tree_on(), &[]).expect("declaration is valid shape");
    assert_eq!(validated.spec.design_brief, Some(brief));
}

/// /1・/2 に brief を書けば `V3FieldNotAllowed`。
#[test]
fn design_brief_is_rejected_in_v1_and_v2() {
    for name in ["v1-basic.json", "v2-phases.json"] {
        let mut spec = fixture(name);
        spec.design_brief = Some(good_brief());
        let errs = validate(&spec, tree_on(), &[]).unwrap_err();
        assert!(
            errs.contains(&PlanValidationError::V3FieldNotAllowed {
                field: "design_brief"
            }),
            "{name}: {errs:?}"
        );
    }
}

/// brief は planner の JSON 上限（`max_plan_json_bytes_v3`）に数える。brief の分だけ上限を超えれば
/// `PlanTooLarge` で、brief の無い同じ計画は通る。
#[test]
fn design_brief_counts_toward_plan_json_limit() {
    let mut spec = fixture("v3-browser.json");
    let without = serde_json::to_vec(&spec).unwrap().len();
    let limits = ExecutionLimits {
        max_plan_json_bytes_v3: without + 16,
        ..tree_on()
    };
    validate(&spec, limits, &[]).expect("fits without a brief");
    spec.design_brief = Some(good_brief());
    let errs = validate(&spec, limits, &[]).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, PlanValidationError::PlanTooLarge { .. })),
        "{errs:?}"
    );
}
