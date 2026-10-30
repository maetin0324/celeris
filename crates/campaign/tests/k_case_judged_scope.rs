//! Public case scope must describe the stages actually checked by the evaluator.
#[path = "support_cases/mod.rs"]
#[allow(dead_code)]
mod support_cases;
use std::path::Path;

use celeris_campaign::{
    cases::{CaseKind, CaseSet},
    grader::GradeCase,
    manifest::{CampaignManifest, Split},
    report::{campaign_scope, scope_line},
};

#[test]
fn all_cases_match_their_evaluator_checks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cases = CaseSet::load(&support_cases::cases_root(), &root.join("fixtures/grader-private"))
        .expect("load all public cases");
    let mut checked = 0;
    for loaded in &cases.cases {
        let case = &loaded.case;
        let Some(pin) = &case.grade_case_ref else {
            // The mock control has no grade-case and cannot enter a measurement.
            assert_eq!(case.kind, CaseKind::Control, "{}", case.id);
            assert_eq!(case.id, "k0-fixture-control");
            continue;
        };
        let grader = GradeCase::load(&root.join("fixtures/grade-cases").join(&pin.path))
            .expect("load pinned grade-case");
        let actual: Vec<String> = [
            ("implementation", grader.implementation.is_some()),
            ("experiment", grader.experiment.is_some()),
            ("analysis", grader.recompute.is_some()),
            ("review", grader.review.is_some()),
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(stage, _)| stage.to_owned())
        .collect();
        assert_eq!(case.judged_scope(), actual, "{}", case.id);
        if let Some(declared) = grader.judged_scope {
            assert_eq!(declared, actual, "{} grader declaration", case.id);
        }
        checked += 1;
    }
    assert_eq!(checked, cases.cases.len() - 1);
}

#[test]
fn k4_campaign_report_judges_analysis_and_excludes_review() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut manifest =
        CampaignManifest::load(&root.join("fixtures/pilot.json")).expect("load manifest fixture");
    manifest.split = Split::Final;
    manifest.problems = vec!["k4-cmp4-validity-analysis".into()];
    let (judged, not_judged) = campaign_scope(&manifest, &support_cases::cases_root()).expect("scope");
    assert_eq!(judged, ["analysis"]);
    assert_eq!(not_judged, ["review"]);
    assert_eq!(
        scope_line(&judged, &not_judged),
        "judged_scope stages=analysis not_judged=review"
    );
}
