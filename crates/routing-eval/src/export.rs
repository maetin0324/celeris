//! Trial-level JSONL export with frozen problem features.
use crate::{
    aggregate,
    manifest::{Manifest, Problem},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};

pub fn result_row(manifest: &Manifest, problem: &Problem, trial: &aggregate::Trial) -> Value {
    json!({"schema":"celeris.routing-eval-result/1","manifest_sha256":manifest.sha256,"decision_id":trial.decision_id,"problem":{"id":problem.id,"task_id":problem.task_id,"work_unit_id":problem.work_unit_id,"genre":problem.genre,"kind":problem.kind,"split":problem.split,"stratum":problem.stratum,"features_sha256":problem.features_sha256,"features":problem.features},"candidate":{"source_id":trial.key.source_id,"model_id":trial.key.model_id,"harness_id":trial.key.harness_id,"harness_version":trial.key.harness_version},"trial_no":trial.trial_no,"result":{"passed":trial.passed,"timed_out":trial.timed_out,"status":trial.status,"wall_secs":trial.wall_secs,"usd_list":trial.usd_list,"quota":trial.quota}})
}

pub fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let value = |flag: &str| -> Result<String, String> {
        args.iter()
            .position(|x| x == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or_else(|| format!("missing required option {flag}"))
    };
    let db = value("--db")?;
    let manifest_path = PathBuf::from(value("--manifest")?);
    let output = PathBuf::from(value("--out")?);
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(manifest_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let trials = aggregate::collect(&db, &manifest)?;
    let problems: BTreeMap<&str, &Problem> = manifest
        .problems
        .iter()
        .map(|p| (p.id.as_str(), p))
        .collect();
    let mut file = fs::File::create(output).map_err(|e| e.to_string())?;
    for t in &trials {
        let problem = problems
            .get(t.problem_id.as_str())
            .ok_or_else(|| format!("unknown problem {}", t.problem_id))?;
        let row = result_row(&manifest, problem, t);
        serde_json::to_writer(&mut file, &row).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    println!("exported_trials={}", trials.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Problem;
    use task_core::routing_evidence::EvidenceKey;
    #[test]
    fn export_row_has_versioned_schema_and_features() {
        let p = Problem {
            id: "p".into(),
            task_id: "t".into(),
            work_unit_id: "w".into(),
            created_at: "".into(),
            genre: "coding".into(),
            kind: "task".into(),
            command: "secret check".into(),
            split: "dev".into(),
            stratum: "coding:task".into(),
            subset_12: false,
            head_commit: None,
            base_commit: None,
            features_sha256: "fh".into(),
            features: json!({"brief":{"len":3},"aux":{"x":1}}),
            sha256: "ph".into(),
        };
        let m = Manifest {
            schema: "manifest".into(),
            version: "1".into(),
            cutoff: "".into(),
            seed: 0,
            problems: vec![p.clone()],
            sha256: "mh".into(),
        };
        let t = aggregate::Trial {
            decision_id: "d".into(),
            problem_id: "p".into(),
            trial_no: 1,
            key: EvidenceKey {
                genre: "coding".into(),
                harness_id: "claude-code".into(),
                harness_version: "1".into(),
                source_id: "claude-oauth".into(),
                model_id: "m".into(),
            },
            passed: true,
            timed_out: false,
            wall_secs: Some(2.0),
            usd_list: Some(0.2),
            quota: BTreeMap::new(),
            status: "passed".into(),
        };
        let v = result_row(&m, &p, &t);
        assert_eq!(v["schema"], "celeris.routing-eval-result/1");
        assert_eq!(v["problem"]["features"]["brief"]["len"], 3);
        assert_eq!(v["result"]["passed"], true);
    }
}
