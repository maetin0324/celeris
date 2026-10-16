//! 独立採点（grades.jsonl）を試行記録へ結合する。実行終了状態（outcome）と成果物の採点は別々に持ち、
//! 論文用の成功は「outcome が passed かつ、結合キーの合う有効な grade が final_pass」のときだけ。
//! 結合キーは campaign hash・case id・arm・problem・rep・trial_no、固定するのは grader 定義 hash と採点対象成果物 hash。
use std::{collections::BTreeMap, fs, path::Path};

use serde_json::Value;

use crate::{
    grader::CaseSplit,
    manifest::{CampaignManifest, ConfigId, Split},
    record::TrialRecord,
};

pub const GRADES_FILE: &str = "grades.jsonl";
pub const GRADE_LINE_SCHEMA: &str = "celeris.campaign-grade/1";

pub type TrialKey = (ConfigId, String, u32, u32);

pub fn key_of(r: &TrialRecord) -> TrialKey {
    (r.config, r.problem_id.clone(), r.rep, r.trial_no)
}

/// 取り込めた grade（結合検査を通ったもの）と、拒否した理由。
#[derive(Debug, Default)]
pub struct GradeBinding {
    /// 試行 → 成果物の独立採点が final_pass か。
    pub valid: BTreeMap<TrialKey, bool>,
    pub violations: Vec<String>,
}

/// grades.jsonl が無ければ空（全て未採点）。1 行でも壊れていれば違反にする（黙って捨てない）。
pub fn bind(m: &CampaignManifest, recs: &[TrialRecord], out: &Path) -> GradeBinding {
    let mut b = GradeBinding::default();
    let path = out.join(GRADES_FILE);
    if !path.exists() {
        return b;
    }
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            b.violations.push(format!("{}: {e}", path.display()));
            return b;
        }
    };
    let hash = match m.compute_sha256() {
        Ok(h) => h,
        Err(e) => {
            b.violations.push(e);
            return b;
        }
    };
    let by_key: BTreeMap<TrialKey, &TrialRecord> = recs.iter().map(|r| (key_of(r), r)).collect();
    let mut rejected: Vec<TrialKey> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let tag = format!("grades.jsonl line {}", i + 1);
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                b.violations.push(format!("{tag}: unparsable: {e}"));
                continue;
            }
        };
        let t = &v["trial"];
        let Some(cfg) = serde_json::from_value::<ConfigId>(t["config"].clone()).ok() else {
            b.violations.push(format!("{tag}: unknown config"));
            continue;
        };
        let (Some(pid), Some(rep), Some(no)) = (
            t["problem_id"].as_str(),
            t["rep"].as_u64(),
            t["trial_no"].as_u64(),
        ) else {
            b.violations.push(format!(
                "{tag}: trial key needs config/problem_id/rep/trial_no"
            ));
            continue;
        };
        let key: TrialKey = (cfg, pid.to_string(), rep as u32, no as u32);
        let tag = format!("{tag} ({:?}/{pid}/rep{rep}#{no})", cfg);
        if v["graded"] != true {
            continue; // 採点していない行は成功に数えない（未採点のまま）。
        }
        let mut bad: Vec<String> = Vec::new();
        if v["schema"] != GRADE_LINE_SCHEMA {
            bad.push("unknown grade schema".into());
        }
        if v["campaign_sha256"] != hash {
            bad.push("grade belongs to another campaign (manifest hash mismatch)".into());
        }
        if serde_json::from_value::<Split>(v["split"].clone()).ok() != Some(m.split) {
            bad.push("grade split differs from the campaign split".into());
        }
        if v["case_id"] != pid || v["grade"]["case_id"] != pid {
            bad.push("grade case id differs from the trial's problem id".into());
        }
        // control case は最終合格にならない。dev/final は campaign の split と一致させる。
        let case_split = serde_json::from_value::<CaseSplit>(v["grade"]["split"].clone()).ok();
        let want = match m.split {
            Split::Dev => CaseSplit::Dev,
            Split::Final => CaseSplit::Final,
        };
        if !matches!(case_split, Some(s) if s == want || s == CaseSplit::Control) {
            bad.push("grade-case split differs from the campaign split".into());
        }
        if v["grader_sha256"].as_str().is_none_or(str::is_empty) {
            bad.push("grader definition hash is missing".into());
        }
        match by_key.get(&key) {
            None => bad.push("no such trial record".into()),
            Some(r) => {
                if v["artifact_sha256"] != r.workspace_sha256_final.as_str()
                    || r.workspace_sha256_final.is_empty()
                {
                    bad.push(
                        "graded artifact hash differs from the trial's final workspace (stale or altered artifact)"
                            .into(),
                    );
                }
            }
        }
        if rejected.contains(&key) || b.valid.contains_key(&key) {
            bad.push("duplicate grade for the trial".into());
            // 重複は先に取り込んだ分も無効にする（どちらが正かを選ばない）。
            b.valid.remove(&key);
            rejected.push(key.clone());
        }
        if !bad.is_empty() {
            for s in bad {
                b.violations.push(format!("{tag}: {s}"));
            }
            rejected.push(key);
            continue;
        }
        let Some(fp) = v["grade"]["final_pass"].as_bool() else {
            b.violations
                .push(format!("{tag}: grade has no boolean final_pass"));
            rejected.push(key);
            continue;
        };
        b.valid.insert(key, fp);
    }
    b
}
