//! ADR 2026-11-02-ts-control-layer-boundary (f)・付記 S2-8・S5-7: 停止基準の計数と判定。数値は (f) のまま。
//!
//! I/O も時計の読み出しも持たない。計数の永続化と戻しの書き込みは `store::script_guard`、時計は呼び出し側が渡す。

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::script_compare::{CompareOutcome, ScriptDomain, ScriptMode};
use crate::script_exhaustive::ScriptExhaustiveStanding;

/// 窓の長さ（(f) の「直近 1000 呼び出し」）。
pub const SCRIPT_GUARD_WINDOW: usize = 1000;
/// timeout の件数の窓（(f) の「10 分間」）。
pub const SCRIPT_GUARD_TIMEOUT_WINDOW_SECS: i64 = 600;
/// 10 分間の timeout の件数の上限（(f) の「3 件」で戻す）。
pub const SCRIPT_GUARD_TIMEOUT_BURST: usize = 3;
/// 連続の失敗の上限（(f) の「連続 3 件」で戻す）。
pub const SCRIPT_GUARD_FAILURE_STREAK: u64 = 3;
/// 純粋判定の昇格に要る連続の呼び出し数（(f)）。
pub const SCRIPT_GUARD_READY_PURE: u64 = 1000;
/// `lane_rules` で網羅の証拠が合格のときの純粋判定の件数（ADR 2026-11-04-lane-rules-exhaustive-evidence D6）。
pub const SCRIPT_GUARD_READY_PURE_EXHAUSTIVE: u64 = 100;
/// CoS の経路の昇格に要る件数（(f)）。
pub const SCRIPT_GUARD_READY_COS: u64 = 200;

/// `actor` の `system:script-guard`（S5-7）。
pub const SCRIPT_GUARD_ACTOR: &str = "system:script-guard";

/// 呼び出し 1 件の結果（S2-8 の分類。`unavailable` は分母に入れないので無い）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptGuardCall {
    Ok,
    Mismatch,
    /// 承認した改善仕様が説明する差分（ADR 2026-11-03-ts-intended-behavior-change D2）。戻す条件に入らない。
    Explained,
    Timeout,
    Failure,
}

impl ScriptGuardCall {
    /// S2-8 の分類。`unavailable` は `None`（tick の予算切れを含む。S5-8）。
    pub fn classify(outcome: CompareOutcome, error_kind: Option<&str>) -> Option<Self> {
        match outcome {
            CompareOutcome::Match | CompareOutcome::ProposalOnly => Some(Self::Ok),
            CompareOutcome::Mismatch => Some(Self::Mismatch),
            CompareOutcome::Explained => Some(Self::Explained),
            CompareOutcome::TsError if matches!(error_kind, Some("timeout" | "terminated")) => {
                Some(Self::Timeout)
            }
            CompareOutcome::TsError | CompareOutcome::InvalidOutput => Some(Self::Failure),
            CompareOutcome::Unavailable => None,
        }
    }

    fn letter(self) -> char {
        match self {
            Self::Ok => 'o',
            Self::Mismatch => 'm',
            Self::Explained => 'e',
            Self::Timeout => 't',
            Self::Failure => 'f',
        }
    }
}

/// 呼び出し 1 件の観測。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptGuardObservation {
    pub call: ScriptGuardCall,
    /// `safety_violations` が空でない（1 件の記録に code が複数あっても 1 件。S2-8）。
    pub safety_violation: bool,
    /// canary の TS 側の単位か（Rust 側の単位の影の呼び出しは偽）。`canary→ts` の前提はこの側だけで数える（S5-4）。
    pub ts_side: bool,
}

/// `script_guard_state` の計数（(domain, generation_id) ごとの 1 行。S5-7）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScriptGuardCounters {
    /// 直近の結果（古い順の `o`・`m`・`e`・`t`・`f`。最大 [`SCRIPT_GUARD_WINDOW`] 文字）。
    pub window: String,
    pub consecutive_failures: u64,
    /// 直近 10 分の timeout の時刻（unix 秒、古い順）。
    pub timeouts_10min: Vec<i64>,
    /// 説明のつかない不一致の無い連続の呼び出し数。
    pub clean_streak: u64,
    /// `clean_streak` のうち TS 側の単位の数。不一致で 0 に戻る。
    pub ts_side_clean_streak: u64,
    pub calls: u64,
    pub safety_violations: u64,
}

impl ScriptGuardCounters {
    /// 1 件を数える。古い timeout の時刻はここで捨てる。
    pub fn observe(&mut self, obs: ScriptGuardObservation, now: OffsetDateTime) {
        self.calls += 1;
        if self.window.len() >= SCRIPT_GUARD_WINDOW {
            self.window.remove(0);
        }
        self.window.push(obs.call.letter());
        self.consecutive_failures = match obs.call {
            ScriptGuardCall::Failure => self.consecutive_failures + 1,
            _ => 0,
        };
        let at = now.unix_timestamp();
        self.timeouts_10min
            .retain(|t| at - *t <= SCRIPT_GUARD_TIMEOUT_WINDOW_SECS);
        if obs.call == ScriptGuardCall::Timeout {
            self.timeouts_10min.push(at);
        }
        if obs.call == ScriptGuardCall::Mismatch {
            self.clean_streak = 0;
            self.ts_side_clean_streak = 0;
        } else {
            self.clean_streak += 1;
            if obs.ts_side {
                self.ts_side_clean_streak += 1;
            }
        }
        if obs.safety_violation {
            self.safety_violations += 1;
        }
    }

    fn count(&self, call: ScriptGuardCall) -> usize {
        self.window.chars().filter(|c| *c == call.letter()).count()
    }

    pub fn timeouts_in_window(&self) -> usize {
        self.count(ScriptGuardCall::Timeout)
    }

    pub fn failures_in_window(&self) -> usize {
        self.count(ScriptGuardCall::Failure)
    }

    pub fn mismatches_in_window(&self) -> usize {
        self.count(ScriptGuardCall::Mismatch)
    }

    /// 窓の中の説明済みの差分の件数（D2。戻す条件には使わない）。
    pub fn explained_in_window(&self) -> usize {
        self.count(ScriptGuardCall::Explained)
    }

    /// 窓の中の件数が 0.1% を超えるか（窓の長さ `len` に対して）。
    fn rate_exceeded(count: usize, len: usize) -> bool {
        count * 1000 > len
    }

    fn recent_timeouts(&self, now: OffsetDateTime) -> usize {
        let at = now.unix_timestamp();
        self.timeouts_10min
            .iter()
            .filter(|t| at - **t <= SCRIPT_GUARD_TIMEOUT_WINDOW_SECS)
            .count()
    }
}

/// `canary`・`ts` を `rust` へ戻す理由（S5-7 の code）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScriptGuardStop {
    GuardMismatch,
    GuardTimeoutRate,
    GuardTimeoutBurst,
    GuardFailureRate,
    GuardFailureStreak,
    GuardSafetyViolation,
}

impl ScriptGuardStop {
    pub fn code(self) -> &'static str {
        match self {
            Self::GuardMismatch => "guard_mismatch",
            Self::GuardTimeoutRate => "guard_timeout_rate",
            Self::GuardTimeoutBurst => "guard_timeout_burst",
            Self::GuardFailureRate => "guard_failure_rate",
            Self::GuardFailureStreak => "guard_failure_streak",
            Self::GuardSafetyViolation => "guard_safety_violation",
        }
    }
}

/// (f) の「即 `rust` へ戻す条件」を当てる。率は 1000 件に届いた窓で判定する（S5-7）。
pub fn evaluate(counters: &ScriptGuardCounters, now: OffsetDateTime) -> Option<ScriptGuardStop> {
    if counters.safety_violations > 0 {
        return Some(ScriptGuardStop::GuardSafetyViolation);
    }
    if counters.mismatches_in_window() > 0 {
        return Some(ScriptGuardStop::GuardMismatch);
    }
    if counters.recent_timeouts(now) >= SCRIPT_GUARD_TIMEOUT_BURST {
        return Some(ScriptGuardStop::GuardTimeoutBurst);
    }
    if counters.consecutive_failures >= SCRIPT_GUARD_FAILURE_STREAK {
        return Some(ScriptGuardStop::GuardFailureStreak);
    }
    let len = counters.window.len();
    if len >= SCRIPT_GUARD_WINDOW {
        if ScriptGuardCounters::rate_exceeded(counters.timeouts_in_window(), len) {
            return Some(ScriptGuardStop::GuardTimeoutRate);
        }
        if ScriptGuardCounters::rate_exceeded(counters.failures_in_window(), len) {
            return Some(ScriptGuardStop::GuardFailureRate);
        }
    }
    None
}

/// 昇格の前提の計数（S5-4 の 422 `script_canary_not_ready` の本文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptGuardReadiness {
    pub ready: bool,
    /// 数えた連続の呼び出し数（`compare` は `clean_streak`、`canary` は `ts_side_clean_streak`）。
    pub clean_streak: u64,
    /// 要る件数（純粋判定 1000、CoS の経路 200）。
    pub required: u64,
    pub window_len: usize,
    pub timeouts_in_window: usize,
    pub failures_in_window: usize,
    pub safety_violations: u64,
    /// 判定に使った網羅の証拠の立場（D6）。
    pub exhaustive: ScriptExhaustiveStanding,
}

impl fmt::Display for ScriptGuardReadiness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "clean_streak {}/{}, window {}, timeouts {}, failures {}, safety_violations {}, exhaustive {}",
            self.clean_streak,
            self.required,
            self.window_len,
            self.timeouts_in_window,
            self.failures_in_window,
            self.safety_violations,
            self.exhaustive.as_str()
        )
    }
}

/// (f) の「`compare`→`canary` へ進む条件」。`from` が `canary` なら TS 側の単位で数える（S5-4 の `canary→ts`）。
/// 窓が 1000 件に届かない間の率は、届いた分の長さで割る（CoS の経路は 200 件で判定するため）。
///
/// D6: `lane_rules` で網羅の証拠が `Passed` なら、要る件数は 100 で、窓の timeout・失敗は 0 件。
/// それ以外（証拠なし・不合格・他の領域）は従来どおり。
pub fn readiness(
    domain: ScriptDomain,
    from: ScriptMode,
    counters: &ScriptGuardCounters,
    exhaustive: ScriptExhaustiveStanding,
) -> ScriptGuardReadiness {
    let evidence_passed =
        domain == ScriptDomain::LaneRules && exhaustive == ScriptExhaustiveStanding::Passed;
    let required = match domain {
        ScriptDomain::LaneRules if evidence_passed => SCRIPT_GUARD_READY_PURE_EXHAUSTIVE,
        ScriptDomain::LaneRules
        | ScriptDomain::RoutePriority
        | ScriptDomain::ExecutionGate
        | ScriptDomain::ProjectController => SCRIPT_GUARD_READY_PURE,
        ScriptDomain::CosDigest => SCRIPT_GUARD_READY_COS,
    };
    let clean_streak = match from {
        ScriptMode::Canary => counters.ts_side_clean_streak,
        _ => counters.clean_streak,
    };
    let window_len = counters.window.len();
    let (timeouts, failures) = (counters.timeouts_in_window(), counters.failures_in_window());
    let rates_ok = if evidence_passed {
        timeouts == 0 && failures == 0
    } else {
        !ScriptGuardCounters::rate_exceeded(timeouts, window_len)
            && !ScriptGuardCounters::rate_exceeded(failures, window_len)
    };
    ScriptGuardReadiness {
        ready: clean_streak >= required && counters.safety_violations == 0 && rates_ok,
        clean_streak,
        required,
        window_len,
        timeouts_in_window: timeouts,
        failures_in_window: failures,
        safety_violations: counters.safety_violations,
        exhaustive,
    }
}

/// `script_guard_observe` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptGuardOutcome {
    pub counters: ScriptGuardCounters,
    /// (f) の条件に当たったか（`compare` でも返す。mode を変えるのは `canary`・`ts` だけ）。
    pub stop: Option<ScriptGuardStop>,
    /// `rust` へ戻した領域。
    pub rolled_back: Vec<ScriptDomain>,
    /// この観測で generation を `invalid` にしたか（安全違反）。
    pub invalidated: bool,
}

#[cfg(test)]
mod exhaustive_readiness_tests {
    use super::*;
    use ScriptExhaustiveStanding as S;

    fn counters(n: u64, window: &str) -> ScriptGuardCounters {
        ScriptGuardCounters {
            window: window.to_owned(),
            clean_streak: n,
            ts_side_clean_streak: n,
            calls: n,
            ..Default::default()
        }
    }

    fn ready(domain: ScriptDomain, from: ScriptMode, c: &ScriptGuardCounters, s: S) -> bool {
        readiness(domain, from, c, s).ready
    }

    #[test]
    fn readiness_lane_rules_exhaustive_required_100() {
        for from in [ScriptMode::Compare, ScriptMode::Canary] {
            let r = readiness(
                ScriptDomain::LaneRules,
                from,
                &counters(100, &"o".repeat(100)),
                S::Passed,
            );
            assert!(r.ready, "{from:?}");
            assert_eq!(r.required, 100);
            assert_eq!(r.exhaustive, S::Passed);
            // 境界 99 件。
            let r = readiness(
                ScriptDomain::LaneRules,
                from,
                &counters(99, &"o".repeat(99)),
                S::Passed,
            );
            assert!(!r.ready);
            assert_eq!(r.required, 100);
        }
    }

    #[test]
    fn readiness_without_evidence_stays_1000() {
        let c = counters(100, &"o".repeat(100));
        assert!(!ready(
            ScriptDomain::LaneRules,
            ScriptMode::Compare,
            &c,
            S::Missing
        ));
        let r = readiness(
            ScriptDomain::LaneRules,
            ScriptMode::Compare,
            &counters(999, &"o".repeat(999)),
            S::Missing,
        );
        assert!(!r.ready);
        assert_eq!(r.required, 1000);
        let c = counters(1000, &"o".repeat(1000));
        assert!(ready(
            ScriptDomain::LaneRules,
            ScriptMode::Compare,
            &c,
            S::Missing
        ));
        // 窓が 1000 件で timeout 1 件は 0.1% で通る（従来）。
        let w = format!("t{}", "o".repeat(999));
        assert!(ready(
            ScriptDomain::LaneRules,
            ScriptMode::Compare,
            &counters(1000, &w),
            S::Missing
        ));
    }

    #[test]
    fn readiness_failed_evidence_stays_1000() {
        let c = counters(100, &"o".repeat(100));
        assert!(!ready(
            ScriptDomain::LaneRules,
            ScriptMode::Canary,
            &c,
            S::Failed
        ));
        assert_eq!(
            readiness(ScriptDomain::LaneRules, ScriptMode::Canary, &c, S::Failed).required,
            1000
        );
    }

    #[test]
    fn readiness_other_domains_unchanged() {
        let c = counters(100, &"o".repeat(100));
        // 証拠が Passed でも route_priority は 1000、cos_digest は 200 のまま。
        for s in [S::Passed, S::Missing, S::NotApplicable] {
            let r = readiness(ScriptDomain::RoutePriority, ScriptMode::Compare, &c, s);
            assert!(!r.ready);
            assert_eq!(r.required, 1000);
            let r = readiness(ScriptDomain::CosDigest, ScriptMode::Compare, &c, s);
            assert!(!r.ready);
            assert_eq!(r.required, 200);
        }
    }

    #[test]
    fn readiness_exhaustive_zero_timeouts_and_failures() {
        // 100 件の窓に timeout 1 件（1%）、失敗 1 件でも、証拠付きは 0 件でなければ ready にしない。
        for bad in ['t', 'f'] {
            let w = format!("{bad}{}", "o".repeat(99));
            assert!(!ready(
                ScriptDomain::LaneRules,
                ScriptMode::Compare,
                &counters(100, &w),
                S::Passed
            ));
        }
        // 安全違反。
        let mut c = counters(100, &"o".repeat(100));
        c.safety_violations = 1;
        assert!(!ready(
            ScriptDomain::LaneRules,
            ScriptMode::Compare,
            &c,
            S::Passed
        ));
        // canary は TS 側の連続で数える。
        let mut c = counters(500, &"o".repeat(500));
        c.ts_side_clean_streak = 99;
        assert!(!ready(
            ScriptDomain::LaneRules,
            ScriptMode::Canary,
            &c,
            S::Passed
        ));
        assert!(ready(
            ScriptDomain::LaneRules,
            ScriptMode::Compare,
            &c,
            S::Passed
        ));
    }
}
