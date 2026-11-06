//! ADR 2026-11-04-lane-rules-exhaustive-evidence D2・D4: `lane_rules` の全入力を本番の isolate で流した証拠の型と合否判定。
//!
//! I/O も時計も持たない純粋な型と関数だけを置く。永続は `store`、流す側は daemon の runner が担う。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::script_compare::ScriptDomain;

/// `TaskFeatures` の 9 軸 × 3 段階（D2）。
pub const LANE_RULES_EXHAUSTIVE_INPUTS: u64 = 19_683;
/// 正順と逆順の 2 回（D2）。
pub const EXHAUSTIVE_PASSES: u64 = 2;
/// 1 回の網羅の呼び出し数（D2）。
pub const LANE_RULES_EXHAUSTIVE_CALLS: u64 = LANE_RULES_EXHAUSTIVE_INPUTS * EXHAUSTIVE_PASSES;

/// 参照（`lane_fixture_jsonl()` の byte 列）の sha256（D2）。今の binary の規則表を表す。
pub fn lane_rules_reference_sha256() -> Result<String, serde_json::Error> {
    let jsonl = crate::model_policy::lane_fixture_jsonl()?;
    Ok(format!("{:x}", Sha256::digest(jsonl.as_bytes())))
}

/// 網羅 1 回の状態（表 `script_exhaustive_runs.state`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScriptExhaustiveState {
    Running,
    Passed,
    Failed,
    Aborted,
}

impl ScriptExhaustiveState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
        }
    }

    /// `passed`・`failed`（最後まで、または不合格の確定まで流して判定した行）。
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Passed | Self::Failed)
    }
}

/// 途中で止めた理由（D4 の `stop_code`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScriptExhaustiveStopCode {
    Mismatch,
    SafetyViolation,
    ConsecutiveErrors,
    RateExceeded,
    GenerationGone,
    WallLimit,
    DaemonStopped,
    DaemonRestarted,
}

impl ScriptExhaustiveStopCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mismatch => "mismatch",
            Self::SafetyViolation => "safety_violation",
            Self::ConsecutiveErrors => "consecutive_errors",
            Self::RateExceeded => "rate_exceeded",
            Self::GenerationGone => "generation_gone",
            Self::WallLimit => "wall_limit",
            Self::DaemonStopped => "daemon_stopped",
            Self::DaemonRestarted => "daemon_restarted",
        }
    }

    /// `failed` で終える理由か（それ以外は `aborted`）。
    pub fn is_failure(self) -> bool {
        matches!(
            self,
            Self::Mismatch | Self::SafetyViolation | Self::ConsecutiveErrors | Self::RateExceeded
        )
    }
}

/// 網羅の件数と latency 統計（D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ScriptExhaustiveCounts {
    pub calls: u64,
    pub matches: u64,
    pub mismatches: u64,
    pub timeouts: u64,
    pub failures: u64,
    pub safety_violations: u64,
    /// 2 回の中で一度も `match` しなかった入力の数。
    pub uncovered_inputs: u64,
    pub latency_p50_us: u64,
    pub latency_p95_us: u64,
    pub latency_max_us: u64,
}

impl ScriptExhaustiveCounts {
    /// 合格か（D4）。呼び出し数が 39,366、不一致・安全違反・未網羅が 0、
    /// timeout と失敗がそれぞれ `calls` の 0.1% 以下（`count * 1000 <= calls`）。
    pub fn verdict(&self) -> bool {
        self.calls == LANE_RULES_EXHAUSTIVE_CALLS
            && self.mismatches == 0
            && self.safety_violations == 0
            && self.uncovered_inputs == 0
            && self.timeouts * 1000 <= self.calls
            && self.failures * 1000 <= self.calls
    }
}

/// 表 `script_exhaustive_runs` の 1 行（D4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptExhaustiveRun {
    pub id: String,
    pub domain: ScriptDomain,
    pub generation_id: String,
    pub bundle_sha256: String,
    /// 始めたときの mode（`compare` か `canary`）。
    pub mode_at_start: String,
    pub daemon_instance_id: String,
    pub requested_by: String,
    pub reason: String,
    pub state: ScriptExhaustiveState,
    pub stop_code: Option<ScriptExhaustiveStopCode>,
    pub reference_sha256: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub counts: ScriptExhaustiveCounts,
    pub first_problem_json: Option<String>,
}

/// 開始時に書く行（`store` の `script_exhaustive_begin` の入力）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewScriptExhaustiveRun {
    pub domain: ScriptDomain,
    pub bundle_sha256: String,
    pub daemon_instance_id: String,
    pub requested_by: String,
    pub reason: String,
    pub reference_sha256: String,
}

/// 領域・generation ごとの証拠の立場（D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScriptExhaustiveStanding {
    /// 網羅の対象でない領域（`lane_rules` 以外）。
    NotApplicable,
    /// 今の generation・参照に対する終わった証拠が無い。
    Missing,
    Passed,
    Failed,
}

impl ScriptExhaustiveStanding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Missing => "missing",
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }
}

/// (domain, generation) の立場を行から決める（D4）。`reference` は今の binary の `reference_sha256`。
/// `domain` と `generation_id` が違う行、`reference_sha256` が違う行は数えない。
pub fn exhaustive_standing(
    domain: ScriptDomain,
    generation_id: &str,
    runs: &[ScriptExhaustiveRun],
    reference: &str,
) -> ScriptExhaustiveStanding {
    if domain != ScriptDomain::LaneRules {
        return ScriptExhaustiveStanding::NotApplicable;
    }
    let relevant: Vec<&ScriptExhaustiveRun> = runs
        .iter()
        .filter(|r| {
            r.domain == domain
                && r.generation_id == generation_id
                && r.reference_sha256 == reference
        })
        .collect();
    // 不一致・安全違反は、その generation の間は消えない。
    if relevant
        .iter()
        .any(|r| r.counts.mismatches > 0 || r.counts.safety_violations > 0)
    {
        return ScriptExhaustiveStanding::Failed;
    }
    let latest = relevant
        .iter()
        .filter(|r| r.state.is_finished())
        .max_by(|a, b| (&a.started_at, &a.id).cmp(&(&b.started_at, &b.id)));
    match latest {
        Some(r) if r.state == ScriptExhaustiveState::Passed && r.counts.verdict() => {
            ScriptExhaustiveStanding::Passed
        }
        Some(_) => ScriptExhaustiveStanding::Failed,
        None => ScriptExhaustiveStanding::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REF: &str = "ref-a";

    fn good() -> ScriptExhaustiveCounts {
        ScriptExhaustiveCounts {
            calls: LANE_RULES_EXHAUSTIVE_CALLS,
            matches: LANE_RULES_EXHAUSTIVE_CALLS,
            ..Default::default()
        }
    }

    fn run(
        id: &str,
        gen_id: &str,
        state: ScriptExhaustiveState,
        counts: ScriptExhaustiveCounts,
    ) -> ScriptExhaustiveRun {
        ScriptExhaustiveRun {
            id: id.into(),
            domain: ScriptDomain::LaneRules,
            generation_id: gen_id.into(),
            bundle_sha256: "b".into(),
            mode_at_start: "compare".into(),
            daemon_instance_id: "i".into(),
            requested_by: "human:a".into(),
            reason: "r".into(),
            state,
            stop_code: None,
            reference_sha256: REF.into(),
            started_at: format!("2026-11-04T00:00:{id}Z"),
            finished_at: None,
            counts,
            first_problem_json: None,
        }
    }

    fn standing(runs: &[ScriptExhaustiveRun]) -> ScriptExhaustiveStanding {
        exhaustive_standing(ScriptDomain::LaneRules, "g1", runs, REF)
    }

    #[test]
    fn constants_match_adr() {
        assert_eq!(LANE_RULES_EXHAUSTIVE_CALLS, 39_366);
        assert_eq!(
            crate::model_policy::all_features().len() as u64,
            LANE_RULES_EXHAUSTIVE_INPUTS
        );
    }

    #[test]
    fn exhaustive_verdict_each_condition() {
        assert!(good().verdict());
        let mut c = good();
        c.calls -= 1;
        assert!(!c.verdict(), "件数不足");
        let mut c = good();
        c.mismatches = 1;
        assert!(!c.verdict());
        let mut c = good();
        c.safety_violations = 1;
        assert!(!c.verdict());
        let mut c = good();
        c.uncovered_inputs = 1;
        assert!(!c.verdict());
        // 0.1%: 39,366 の 0.1% = 39.366 なので 39 件まで。
        let mut c = good();
        c.timeouts = 39;
        assert!(c.verdict());
        c.timeouts = 40;
        assert!(!c.verdict());
        let mut c = good();
        c.failures = 39;
        assert!(c.verdict());
        c.failures = 40;
        assert!(!c.verdict());
    }

    #[test]
    fn exhaustive_standing_rules() {
        use ScriptExhaustiveState::*;
        assert_eq!(standing(&[]), ScriptExhaustiveStanding::Missing);
        // 合格。
        let p = run("01", "g1", Passed, good());
        assert_eq!(
            standing(std::slice::from_ref(&p)),
            ScriptExhaustiveStanding::Passed
        );
        // running・aborted は数えない。
        let r = run("02", "g1", Running, ScriptExhaustiveCounts::default());
        let a = run("03", "g1", Aborted, ScriptExhaustiveCounts::default());
        assert_eq!(standing(&[r, a]), ScriptExhaustiveStanding::Missing);
        // generation 違い・参照違いは数えない。
        let other = run("04", "g2", Passed, good());
        assert_eq!(standing(&[other]), ScriptExhaustiveStanding::Missing);
        let mut stale = run("05", "g1", Passed, good());
        stale.reference_sha256 = "ref-old".into();
        assert_eq!(standing(&[stale]), ScriptExhaustiveStanding::Missing);
        // 不一致の行は、後の合格でも消えない。
        let mut bad_counts = good();
        bad_counts.mismatches = 1;
        let bad = run("06", "g1", Failed, bad_counts);
        let later = run("07", "g1", Passed, good());
        assert_eq!(
            standing(&[bad, later.clone()]),
            ScriptExhaustiveStanding::Failed
        );
        // 率だけで落ちた行は、後の合格で Passed に戻れる。
        let mut rate = good();
        rate.timeouts = 100;
        let slow = run("08", "g1", Failed, rate);
        let again = run("09", "g1", Passed, good());
        assert_eq!(
            standing(&[slow.clone(), again]),
            ScriptExhaustiveStanding::Passed
        );
        assert_eq!(standing(&[slow]), ScriptExhaustiveStanding::Failed);
        // lane_rules 以外。
        assert_eq!(
            exhaustive_standing(ScriptDomain::CosDigest, "g1", &[p], REF),
            ScriptExhaustiveStanding::NotApplicable
        );
    }

    #[test]
    fn exhaustive_actor_gate() {
        use crate::script_compare::actor_may_run_exhaustive as ok;
        assert!(ok("human:ryo") && ok("operator:claude-code"));
        assert!(!ok("cos:a") && !ok("mcp:x") && !ok("system:script-guard"));
        assert!(!ok("human:") && !ok("operator: "));
    }
}
