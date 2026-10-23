//! Research fault exposure is independent of executor outcome and artifact grade.
//! The injector writes one observation per attempted trial to faults.jsonl; declarations
//! alone never count as injection evidence. This module does not inject faults.
use std::{collections::BTreeMap, fs, path::Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    aggregate::Outcome,
    checker::{Recomputed, expected},
    grades::{GradeOutcome, TrialKey, key_of},
    manifest::{CampaignManifest, ConfigId},
    record::TrialRecord,
};

pub const FAULTS_FILE: &str = "faults.jsonl";
pub const FAULT_SCHEMA: &str = "celeris.campaign-fault/1";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FaultStatus {
    Injected,
    NotInjected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaultObservation {
    pub schema: String,
    pub campaign_sha256: String,
    pub config: ConfigId,
    pub problem_id: String,
    pub rep: u32,
    pub trial_no: u32,
    pub scenario: String,
    pub status: FaultStatus,
    /// Injector log relative to the campaign output directory (not the model workspace).
    pub evidence: String,
    pub evidence_sha256: String,
}

impl FaultObservation {
    fn key(&self) -> TrialKey {
        (
            self.config,
            self.problem_id.clone(),
            self.rep,
            self.trial_no,
        )
    }
}

fn load(
    m: &CampaignManifest,
    recs: &[TrialRecord],
    out: &Path,
) -> Result<Vec<FaultObservation>, String> {
    let text = match fs::read_to_string(out.join(FAULTS_FILE)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("faults: {e}")),
    };
    let sha = m.compute_sha256()?;
    let root = out.canonicalize().map_err(|e| format!("faults: {e}"))?;
    let mut seen = std::collections::BTreeSet::new();
    let mut observations = Vec::new();
    for line in text.lines() {
        let f: FaultObservation = serde_json::from_str(line).map_err(|e| format!("faults: {e}"))?;
        if f.schema != FAULT_SCHEMA
            || f.campaign_sha256 != sha
            || m.fault_scenarios.get(&f.problem_id) != Some(&f.scenario)
            || !recs.iter().any(|r| key_of(r) == f.key() && r.ran())
            || !seen.insert(f.key())
        {
            return Err("faults: invalid binding, undeclared scenario, unexecuted trial or duplicate observation".into());
        }
        let rel = Path::new(&f.evidence);
        if rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || f.evidence.is_empty()
        {
            return Err("faults: evidence must be a relative path within output".into());
        }
        let path = out
            .join(rel)
            .canonicalize()
            .map_err(|e| format!("faults: evidence: {e}"))?;
        if !path.starts_with(&root) {
            return Err("faults: evidence escapes output".into());
        }
        let bytes = fs::read(path).map_err(|e| format!("faults: evidence: {e}"))?;
        if bytes.is_empty() || format!("{:x}", Sha256::digest(&bytes)) != f.evidence_sha256 {
            return Err("faults: empty or changed evidence".into());
        }
        observations.push(f);
    }
    Ok(observations)
}

/// Cross-tabulate exposure and outcome over the same fixed denominator as completion.
/// All retry observations are preserved, but only the first attempt determines a cell.
pub(crate) fn reconcile(
    m: &CampaignManifest,
    recs: &[TrialRecord],
    out: &Path,
    grades: &BTreeMap<TrialKey, GradeOutcome>,
    rc: &mut Recomputed,
    violations: &mut Vec<String>,
    reasons: &mut Vec<String>,
) {
    let observations = match load(m, recs, out) {
        Ok(o) => o,
        Err(e) => {
            violations.push(e);
            vec![]
        }
    };
    for (c, problems) in expected(m) {
        for p in problems {
            for rep in 1..=m.repeats {
                let first = recs
                    .iter()
                    .filter(|r| r.config == c && r.problem_id == p && r.rep == rep)
                    .min_by_key(|r| r.trial_no);
                let observation =
                    first.and_then(|r| observations.iter().find(|f| f.key() == key_of(r)));
                let exposure = if !m.fault_scenarios.contains_key(&p) {
                    "not_planned"
                } else {
                    match observation.map(|f| f.status) {
                        Some(FaultStatus::Injected) => "injected",
                        Some(FaultStatus::NotInjected) => "not_injected",
                        None => "unobserved",
                    }
                };
                if matches!(exposure, "not_injected" | "unobserved") {
                    reasons.push(format!("{c:?}/{p}/rep{rep}: research fault {exposure}; injection evidence unresolved"));
                }
                let outcome = match first.map(|r| r.outcome) {
                    None => "missing",
                    Some(Outcome::Passed) => match first.and_then(|r| grades.get(&key_of(r))) {
                        Some(GradeOutcome::Passed) => "final_passed",
                        Some(GradeOutcome::ModelFailure) => "model_failure",
                        Some(GradeOutcome::EnvFailure) => "grader_env_failure",
                        Some(GradeOutcome::Undetermined) => "grader_undetermined",
                        Some(GradeOutcome::ControlOnly) => "grader_control_only",
                        Some(GradeOutcome::ObservationFailure(_)) => "observation_failed",
                        None => "ungraded",
                    },
                    Some(Outcome::Failed) => "executor_failed",
                    Some(Outcome::Error) => "executor_error",
                    Some(Outcome::Timeout) => "executor_timeout",
                    Some(Outcome::Cancelled) => "executor_cancelled",
                    Some(Outcome::Blocked) => "executor_blocked",
                };
                *rc.totals
                    .entry(c)
                    .or_default()
                    .fault_breakdown
                    .entry(exposure.into())
                    .or_default()
                    .entry(outcome.into())
                    .or_default() += 1;
            }
        }
    }
    rc.fault_observations = observations;
}
