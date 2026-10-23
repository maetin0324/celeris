//! 成功契約（canonical success / failure 分類）。checker・export・human-op join/report が同じ関数で cell を判定する。
//! 定義は `docs/ops/eval-raw-record-definitions.md` §7 と ADR `2026-10-23-campaign-success-contract`。
//!
//! - cell = 計画された (arm, case, rep)。分母は manifest の計画数（有効 arm の問題数 × repeats）で、未実行・未採点・欠測も残す。
//! - 再試行の選択: cell の判定は trial_no 最小の記録（最初の試行）。後の trial_no は `recorded_trials` に数えるだけで成功にしない。
//! - executor status（実行器の終了状態）と validated success（executor passed かつ結合検査を通った独立採点 final_pass）は別の欄。
//! - 採点の不合格は failure_class を保つ。env_failure / undetermined は再採点まで未確定（`needs_regrade`）。
use std::{collections::BTreeMap, fs, path::Path};

use serde::Serialize;

use crate::{
    aggregate::Outcome,
    grades::{GradeOutcome, TrialKey, key_of},
    manifest::{CampaignManifest, ConfigId},
    record::{CAMPAIGN_FILE, REASON_BLOCKED, REASON_EXECUTOR_ERROR, TRIALS_FILE, TrialRecord},
};

pub const SUCCESS_CONTRACT: &str = "celeris.campaign-success/1";
/// 再試行の選択規則（cell を判定する試行）。
pub const RETRY_SELECTION: &str = "first_attempt: the record with the smallest trial_no decides the cell; later trial_no are counted, never promoted";
/// 分母の規則。
pub const DENOMINATOR: &str = "planned cells of the manifest (active arm problems x repeats); unexecuted, ungraded and missing cells stay in the denominator";
/// 人の介入・時間・再作業を数える観測窓。
/// 終端だけで切る: 同じ試行 identity の記録は試行開始前（準備・再起動）も数え、終端より後に始まったものは数えない。
pub const OBSERVATION_WINDOW: &str = "counted until the cutoff when one is recorded, else until the trial end (ledger start + wall); ops, attempts and intervals starting after that end are excluded (intervals crossing it are clipped); records of the same trial identity before the trial start (preparation, restart) are counted";

/// cell の結論。成功に数えるのは `ValidatedSuccess` だけ。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SuccessClass {
    ValidatedSuccess,
    /// executor passed、独立採点が成果物の不合格。
    ModelFailure,
    /// executor passed、採点基盤・初期資産の不良。再採点まで未確定。
    GraderEnvFailure,
    /// executor passed、不合格の原因が未確定。再採点まで未確定。
    GraderUndetermined,
    /// executor passed、control case（最終合格にならない）。
    ControlOnly,
    /// executor passed、収集（観測）の失敗で採点できなかった。再収集・再採点まで未確定。
    ObservationFailed,
    /// executor passed、有効な採点が無い（未採点・結合検査で拒否）。
    Ungraded,
    ExecutorFailed,
    ExecutorError,
    ExecutorTimeout,
    ExecutorCancelled,
    ExecutorBlocked,
    /// 記録が無い。
    Missing,
}

impl SuccessClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ValidatedSuccess => "validated_success",
            Self::ModelFailure => "model_failure",
            Self::GraderEnvFailure => "grader_env_failure",
            Self::GraderUndetermined => "grader_undetermined",
            Self::ControlOnly => "control_only",
            Self::ObservationFailed => "observation_failed",
            Self::Ungraded => "ungraded",
            Self::ExecutorFailed => "executor_failed",
            Self::ExecutorError => "executor_error",
            Self::ExecutorTimeout => "executor_timeout",
            Self::ExecutorCancelled => "executor_cancelled",
            Self::ExecutorBlocked => "executor_blocked",
            Self::Missing => "missing",
        }
    }

    pub fn is_success(self) -> bool {
        self == Self::ValidatedSuccess
    }

    /// 再採点（環境修復・初期 tree の指定）まで結論を出せない。
    pub fn needs_regrade(self) -> bool {
        matches!(
            self,
            Self::GraderEnvFailure | Self::GraderUndetermined | Self::ObservationFailed
        )
    }
}

/// 1 試行の記録と（あれば）結合済みの採点から結論を出す。
pub fn classify(r: Option<&TrialRecord>, grade: Option<&GradeOutcome>) -> SuccessClass {
    use SuccessClass as S;
    let Some(r) = r else { return S::Missing };
    match r.outcome {
        Outcome::Passed => match grade {
            Some(GradeOutcome::Passed) => S::ValidatedSuccess,
            Some(GradeOutcome::ModelFailure) => S::ModelFailure,
            Some(GradeOutcome::EnvFailure) => S::GraderEnvFailure,
            Some(GradeOutcome::Undetermined) => S::GraderUndetermined,
            Some(GradeOutcome::ControlOnly) => S::ControlOnly,
            Some(GradeOutcome::ObservationFailure(_)) => S::ObservationFailed,
            None => S::Ungraded,
        },
        Outcome::Failed => S::ExecutorFailed,
        Outcome::Error => S::ExecutorError,
        Outcome::Timeout => S::ExecutorTimeout,
        Outcome::Cancelled => S::ExecutorCancelled,
        Outcome::Blocked => S::ExecutorBlocked,
    }
}

pub type CellKey = (ConfigId, String, u32);

/// 計画された 1 cell の判定。
#[derive(Clone, Debug, Serialize)]
pub struct Cell {
    pub config: ConfigId,
    pub problem_id: String,
    pub rep: u32,
    /// 判定に使った試行（trial_no 最小）。記録が無ければ None。
    pub canonical_trial_no: Option<u32>,
    pub executor_status: Option<Outcome>,
    pub executor_reason: Option<String>,
    pub class: SuccessClass,
    /// 記録はあるが実行されなかった（task 作成失敗・予算で未投入）。依存で blocked は含めない。
    pub unexecuted: bool,
    /// この cell の記録の数（再試行を含む。判定には最初の 1 件だけを使う）。
    pub recorded_trials: u32,
}

/// cell ごとの判定。キーは manifest の計画 cell だけ（計画外の記録は checker が違反にする）。
pub fn cells(
    m: &CampaignManifest,
    recs: &[TrialRecord],
    grades: &BTreeMap<TrialKey, GradeOutcome>,
) -> BTreeMap<CellKey, Cell> {
    let mut first: BTreeMap<(ConfigId, &str, u32), &TrialRecord> = BTreeMap::new();
    let mut count: BTreeMap<(ConfigId, &str, u32), u32> = BTreeMap::new();
    for r in recs {
        let k = (r.config, r.problem_id.as_str(), r.rep);
        *count.entry(k).or_default() += 1;
        let e = first.entry(k).or_insert(r);
        if r.trial_no < e.trial_no {
            *e = r;
        }
    }
    let mut out = BTreeMap::new();
    for (c, probs) in crate::checker::expected(m) {
        for p in &probs {
            for rep in 1..=m.repeats {
                let r = first.get(&(c, p.as_str(), rep)).copied();
                let grade = r.and_then(|r| grades.get(&key_of(r)));
                out.insert(
                    (c, p.clone(), rep),
                    Cell {
                        config: c,
                        problem_id: p.clone(),
                        rep,
                        canonical_trial_no: r.map(|r| r.trial_no),
                        executor_status: r.map(|r| r.outcome),
                        executor_reason: r.and_then(|r| r.reason.clone()),
                        class: classify(r, grade),
                        unexecuted: r.is_some_and(|r| {
                            r.reason.as_deref() == Some(REASON_EXECUTOR_ERROR)
                                || (!r.ran() && r.reason.as_deref() != Some(REASON_BLOCKED))
                        }),
                        recorded_trials: count.get(&(c, p.as_str(), rep)).copied().unwrap_or(0),
                    },
                );
            }
        }
    }
    out
}

/// runner の out dir から読んだ canonical な判定（human-op join/report 用）。checker と同じ結合検査を通す。
pub struct Canonical {
    pub manifest_sha256: String,
    pub cells: BTreeMap<CellKey, Cell>,
    /// grades.jsonl の結合検査の違反（拒否した行は未採点のまま）。
    pub grade_violations: Vec<String>,
    pub grades_file: bool,
}

/// `out` に campaign.json が無ければ None（manifest が無いと分母も採点の結合も決められない）。
pub fn load(out: &Path) -> Result<Option<Canonical>, String> {
    let mp = out.join(CAMPAIGN_FILE);
    if !mp.is_file() {
        return Ok(None);
    }
    let m: CampaignManifest =
        serde_json::from_slice(&fs::read(&mp).map_err(|e| format!("{}: {e}", mp.display()))?)
            .map_err(|e| format!("{}: {e}", mp.display()))?;
    let sha = m.compute_sha256()?;
    let tp = out.join(TRIALS_FILE);
    let text = if tp.is_file() {
        fs::read_to_string(&tp).map_err(|e| format!("{}: {e}", tp.display()))?
    } else {
        String::new()
    };
    let mut recs: Vec<TrialRecord> = Vec::new();
    for (i, l) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let r: TrialRecord =
            serde_json::from_str(l).map_err(|e| format!("{}:{}: {e}", tp.display(), i + 1))?;
        // 再起動で同じ試行の行が重なっても 1 件（checker は重複を違反にする）。
        if !recs.iter().any(|x| key_of(x) == key_of(&r)) {
            recs.push(r);
        }
    }
    let mut violations = Vec::new();
    let lock = match crate::lock_exec::load_saved(&m, out, None) {
        Ok(l) => l,
        Err(e) => {
            violations.push(format!("lock: {e}"));
            None
        }
    };
    let mut b = crate::grades::bind(&m, &recs, out, lock.as_ref());
    if !violations.is_empty() {
        // checker と同じ: lock を信用できないときは grade を有効にしない。
        b.valid.clear();
    }
    violations.extend(b.violations);
    Ok(Some(Canonical {
        manifest_sha256: sha,
        cells: cells(&m, &recs, &b.valid),
        grade_violations: violations,
        grades_file: out.join(crate::grades::GRADES_FILE).is_file(),
    }))
}
