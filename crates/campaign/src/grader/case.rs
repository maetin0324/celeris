//! grader 側の case 定義（`celeris.grade-case/1`）。試行の作業場所には渡さない。
use std::{collections::BTreeMap, fs, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const GRADE_CASE_SCHEMA: &str = "celeris.grade-case/1";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CaseSplit {
    Dev,
    Final,
    /// fixture/mock の制御試験。全検査が通っても最終合格にしない。
    Control,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GradeCase {
    pub schema: String,
    pub id: String,
    pub split: CaseSplit,
    #[serde(default)]
    pub implementation: Option<ImplementationSpec>,
    #[serde(default)]
    pub experiment: Option<ExperimentSpec>,
    #[serde(default)]
    pub recompute: Option<RecomputeSpec>,
    #[serde(default)]
    pub review: Option<ReviewSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PristineTest {
    /// 成果物 repo 内の path（重ねる先）。pristine の実体は `pristine_dir/<path>`。
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImplementationSpec {
    /// case file の dir からの相対 path。
    pub pristine_dir: String,
    /// 成果物 dir 内の repo の相対 path。
    pub repo: String,
    pub tests: Vec<PristineTest>,
    pub command: Vec<String>,
    /// 出力に `test <name> ... ok` が現れねばならない試験名。
    #[serde(default)]
    pub must_pass: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentSpec {
    /// 成果物 dir 内の証拠一覧（既定 `evidence/jobs.json`）。
    #[serde(default = "default_jobs_file")]
    pub jobs_file: String,
    pub min_jobs: usize,
    /// 各 job の `conditions` に要求する値（node 数等）。
    pub conditions: BTreeMap<String, Value>,
}

fn default_jobs_file() -> String {
    "evidence/jobs.json".into()
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Agg {
    Mean,
    Median,
    Sum,
    Min,
    Max,
    Count,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum Source {
    /// 各 file の JSON pointer の値。
    Json { files: Vec<String>, pointer: String },
    /// 各 file の CSV 列（header 行あり）。`filter` の列＝値の行だけ。
    Csv {
        files: Vec<String>,
        column: String,
        #[serde(default)]
        filter: BTreeMap<String, String>,
    },
    /// 他の指標の比。
    Ratio {
        numerator: String,
        denominator: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricSpec {
    pub name: String,
    pub source: Source,
    #[serde(default = "default_agg")]
    pub agg: Agg,
    /// 相対許容差（|reported - recomputed| <= tol * max(|recomputed|, 1e-12)）。
    pub tolerance_rel: f64,
}

fn default_agg() -> Agg {
    Agg::Mean
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecomputeSpec {
    /// 成果物 dir 内の報告（`metric <name> = <value>` 行）。
    pub report: String,
    /// 固定入力の sha256（成果物 dir 内の path → sha256）。
    #[serde(default)]
    pub pinned_raw: BTreeMap<String, String>,
    pub metrics: Vec<MetricSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefectSpec {
    pub id: String,
    pub file: String,
    /// 含む行範囲 [start, end]。無ければ file 全体。
    #[serde(default)]
    pub lines: Option<(u64, u64)>,
    /// 指摘本文に 1 つ以上含まれるべき語（大文字小文字を区別しない）。
    pub keywords: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSpec {
    #[serde(default = "default_findings_file")]
    pub findings: String,
    pub known_defects: Vec<DefectSpec>,
    /// list 外だが実在が確認済みの指摘（検出にも誤指摘にも数えない）。
    #[serde(default)]
    pub also_valid: Vec<DefectSpec>,
    pub min_detection_rate: f64,
    pub max_false_positives: usize,
}

fn default_findings_file() -> String {
    "review/findings.json".into()
}

impl GradeCase {
    pub fn load(path: &Path) -> Result<Self, String> {
        let b = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let c: GradeCase =
            serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display()))?;
        c.validate()?;
        Ok(c)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema != GRADE_CASE_SCHEMA {
            return Err(format!("unknown case schema {}", self.schema));
        }
        if self.id.is_empty() {
            return Err("case id is empty".into());
        }
        if let Some(i) = &self.implementation
            && (i.tests.is_empty() || i.command.is_empty())
        {
            return Err("implementation needs tests and a command".into());
        }
        if let Some(r) = &self.recompute
            && r.metrics
                .iter()
                .any(|m| m.tolerance_rel.is_nan() || m.tolerance_rel < 0.0)
        {
            return Err("metric tolerance_rel must be >= 0".into());
        }
        if let Some(r) = &self.review
            && (r.known_defects.is_empty() || !(0.0..=1.0).contains(&r.min_detection_rate))
        {
            return Err("review needs known_defects and min_detection_rate in 0..=1".into());
        }
        Ok(())
    }
}
