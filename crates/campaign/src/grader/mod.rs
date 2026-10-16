//! 独立 grader（ADR 2026-10-16-mlsys-independent-grader）。
//! 入力は trial の成果物 directory と grader 側の case 定義だけ。成果物中の成功宣言・trial 記録の
//! outcome・mock の Passed は読まない。4 検査（実装テスト・実行証拠・独立再計算・レビュー）を独立に行う。
pub mod case;
pub mod evidence;
pub mod implementation;
pub mod recompute;
pub mod review;

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub use case::{CaseSplit, GradeCase};
pub use implementation::{CommandOutput, CommandRunner, ProcessRunner};

pub const GRADE_SCHEMA: &str = "celeris.grade/1";

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Fail,
    NotApplicable,
}

#[derive(Clone, Debug, Serialize)]
pub struct CheckResult {
    pub status: Status,
    pub reasons: Vec<String>,
    pub details: Value,
}

impl CheckResult {
    pub fn not_applicable() -> Self {
        CheckResult {
            status: Status::NotApplicable,
            reasons: vec!["case does not define this check".into()],
            details: Value::Null,
        }
    }

    /// 理由が空なら pass、あれば fail。
    pub fn from_reasons(reasons: Vec<String>, details: Value) -> Self {
        CheckResult {
            status: if reasons.is_empty() {
                Status::Pass
            } else {
                Status::Fail
            },
            reasons,
            details,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
    /// control（fixture/mock）case。検査が通っても最終合格ではない。
    ControlOnly,
}

#[derive(Clone, Debug, Serialize)]
pub struct Checks {
    pub implementation: CheckResult,
    pub evidence: CheckResult,
    pub recompute: CheckResult,
    pub review: CheckResult,
}

#[derive(Clone, Debug, Serialize)]
pub struct GradeReport {
    pub schema: &'static str,
    pub case_id: String,
    pub split: CaseSplit,
    pub verdict: Verdict,
    pub final_pass: bool,
    pub checks: Checks,
}

pub struct GradeOptions<'a> {
    /// case file の dir（pristine の基点）。
    pub case_dir: PathBuf,
    /// 実装テストの作業 dir の親（既定は `std::env::temp_dir()`、つまり $TMPDIR）。
    pub work_root: PathBuf,
    pub runner: &'a dyn CommandRunner,
}

pub fn grade(case: &GradeCase, artifact: &Path, opts: &GradeOptions) -> GradeReport {
    let control = case.split == CaseSplit::Control;
    let checks = Checks {
        implementation: match &case.implementation {
            Some(s) => implementation::check(s, artifact, opts),
            None => CheckResult::not_applicable(),
        },
        evidence: match &case.experiment {
            Some(s) => evidence::check(s, artifact, control),
            None => CheckResult::not_applicable(),
        },
        recompute: match &case.recompute {
            Some(s) => recompute::check(s, artifact),
            None => CheckResult::not_applicable(),
        },
        review: match &case.review {
            Some(s) => review::check(s, artifact),
            None => CheckResult::not_applicable(),
        },
    };
    let all = [
        &checks.implementation,
        &checks.evidence,
        &checks.recompute,
        &checks.review,
    ];
    let applicable = all
        .iter()
        .filter(|c| c.status != Status::NotApplicable)
        .count();
    let passed = applicable > 0 && all.iter().all(|c| c.status != Status::Fail);
    let verdict = match (passed, control) {
        (false, _) => Verdict::Fail,
        (true, true) => Verdict::ControlOnly,
        (true, false) => Verdict::Pass,
    };
    GradeReport {
        schema: GRADE_SCHEMA,
        case_id: case.id.clone(),
        split: case.split,
        verdict,
        final_pass: verdict == Verdict::Pass,
        checks,
    }
}

/// `celeris-campaign grade --case <file> --artifact <dir> [--work <dir>]`。合格で Ok、不合格で Err（どちらも判定 JSON）。
pub fn cli(args: &[String]) -> Result<String, String> {
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
    };
    let case_path = arg("--case").ok_or("--case <file> is required")?;
    let artifact = arg("--artifact").ok_or("--artifact <dir> is required")?;
    let case = GradeCase::load(&case_path)?;
    let opts = GradeOptions {
        case_dir: case_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
        work_root: arg("--work").unwrap_or_else(std::env::temp_dir),
        runner: &ProcessRunner,
    };
    let rep = grade(&case, &artifact, &opts);
    let s = serde_json::to_string_pretty(&rep).map_err(|e| e.to_string())?;
    if rep.final_pass { Ok(s) } else { Err(s) }
}

pub(crate) fn sha256_file(p: &Path) -> Result<String, String> {
    let b = fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(format!("{:x}", Sha256::digest(&b)))
}

/// 成果物 dir 内の相対 path を解決する。絶対 path・`..`・成果物外を指す symlink を拒否する。
pub(crate) fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    if rel.is_empty()
        || p.components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "path `{rel}` must be relative and stay inside the artifact"
        ));
    }
    let joined = root.join(p);
    if let (Ok(r), Ok(j)) = (root.canonicalize(), joined.canonicalize())
        && !j.starts_with(&r)
    {
        return Err(format!("path `{rel}` resolves outside the artifact"));
    }
    Ok(joined)
}

pub(crate) fn read_json(p: &Path) -> Result<Value, String> {
    let b = fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display()))
}

/// 出力の末尾だけを残す（秘密値の大量流出と判定 JSON の肥大を避ける）。
pub(crate) fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut i = s.len() - max;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    s[i..].to_string()
}
