//! 独立採点（grades.jsonl）を試行記録へ結合する。実行終了状態（outcome）と成果物の採点は別々に持ち、
//! 論文用の成功は「outcome が passed かつ、結合キーの合う有効な grade が final_pass」のときだけ。
//! 結合キーは campaign hash・case id・arm・problem・rep・trial_no、固定するのは grader 定義 hash と採点対象成果物 hash。
use std::{collections::BTreeMap, fs, path::Path};

use serde_json::Value;

use crate::{
    grader::CaseSplit,
    lock::CampaignLock,
    manifest::{CampaignManifest, ConfigId, Split},
    record::TrialRecord,
};

pub const GRADES_FILE: &str = "grades.jsonl";
pub const GRADE_LINE_SCHEMA: &str = "celeris.campaign-grade/1";

pub type TrialKey = (ConfigId, String, u32, u32);

pub fn key_of(r: &TrialRecord) -> TrialKey {
    (r.config, r.problem_id.clone(), r.rep, r.trial_no)
}

/// 結合検査を通った独立採点の結果。`final_pass` の bool に潰さず、不合格の原因を保つ（checker・export まで届く）。
/// 成功に数えるのは `Passed` だけ。残りは分母に残り、原因別に数える。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GradeOutcome {
    Passed,
    /// 成果物の不合格（初期 tree が健全で、モデルの変更が壊した）。
    ModelFailure,
    /// 採点基盤・初期資産の不良（成果物の出来ではない）。環境を直して再採点するまで未確定。
    EnvFailure,
    /// 原因未確定（初期 tree 未指定で compile error の出どころが決められない）。再採点するまで未確定。
    Undetermined,
    /// control（fixture/mock）case。検査が通っても最終合格ではない。
    ControlOnly,
}

impl GradeOutcome {
    /// 再採点（環境修復・初期 tree の指定）まで結論を出せない結果か。
    pub fn needs_regrade(self) -> bool {
        matches!(self, Self::EnvFailure | Self::Undetermined)
    }
}

/// 取り込めた grade（結合検査を通ったもの）と、拒否した理由。
#[derive(Debug, Default)]
pub struct GradeBinding {
    /// 試行 → 成果物の独立採点の結果（合格と不合格の原因）。
    pub valid: BTreeMap<TrialKey, GradeOutcome>,
    pub violations: Vec<String>,
}

/// grades.jsonl が無ければ空（全て未採点）。1 行でも壊れていれば違反にする（黙って捨てない）。
///
/// `lock` は保存 lock（封印・manifest 結合は検証済み）。Some なら、各 grade 行を lock の固定値
/// （lock 封印・case の grade-case hash・公開/隠しテスト digest・case split）と照合する。非空検査では済ませない。
/// None は lock 導入前の legacy 記録（無課金の mock 実行など）で、従来の結合検査だけを行う（本測定の根拠にならない）。
pub fn bind(
    m: &CampaignManifest,
    recs: &[TrialRecord],
    out: &Path,
    lock: Option<&CampaignLock>,
) -> GradeBinding {
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
        if let Some(l) = lock {
            if v["lock_sha256"] != l.sha256.as_str() {
                bad.push("grade was not produced under this campaign lock".into());
            }
            match l.case(pid) {
                None => bad.push("case is not in the campaign lock".into()),
                Some(c) => {
                    if v["grader_sha256"] != c.grader.grade_case_sha256.as_str() {
                        bad.push("grader definition hash differs from the locked grader".into());
                    }
                    if v["tests_sha256"] != c.tests_sha256().as_str() {
                        bad.push("public/hidden test hash differs from the locked tests".into());
                    }
                    if serde_json::from_value::<Split>(v["grade"]["split"].clone()).ok()
                        != Some(c.split)
                        && case_split != Some(CaseSplit::Control)
                    {
                        bad.push("grade split differs from the locked case split".into());
                    }
                }
            }
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
        // 不合格は原因（failure_class）を必ず持つ。持たない行は原因を決められないので取り込まない。
        let outcome = if fp {
            GradeOutcome::Passed
        } else if v["grade"]["verdict"] == "control_only" {
            GradeOutcome::ControlOnly
        } else {
            match v["grade"]["failure_class"].as_str() {
                Some("model_failure") => GradeOutcome::ModelFailure,
                Some("env_failure") => GradeOutcome::EnvFailure,
                Some("undetermined") => GradeOutcome::Undetermined,
                other => {
                    b.violations.push(format!(
                        "{tag}: failed grade needs failure_class model_failure|env_failure|undetermined, got {other:?}"
                    ));
                    rejected.push(key);
                    continue;
                }
            }
        };
        b.valid.insert(key, outcome);
    }
    b
}
