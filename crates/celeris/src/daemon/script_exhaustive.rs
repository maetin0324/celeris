//! ADR 2026-11-04-lane-rules-exhaustive-evidence D2・D3・D5・D8: `lane_rules` の全入力を、実行中の daemon の
//! generation の isolate へ流す runner。
//!
//! - 入力は `model_policy::all_features()`（9 軸 × 3 段階 = 19,683 通り）を Rust で列挙する。fixture file は読まない。
//!   正順の全件の次に逆順の全件を同じ isolate に流し、各入力を `match_rule` と `classify_lane_rules`
//!   （天井は `LaneCeiling::default()`）で比べる。
//! - dispatch の tick の外の tokio task で 1 件ずつ呼ぶ。`ScriptTickWait` を使わず、`ScriptGenerations.ops` の lock も
//!   持たない。各呼び出しの前に generation の in-flight を読み、live の要求が走っていれば呼ばずに待つ。
//!   1 件ごとに [`EXHAUSTIVE_CALL_GAP`] を空ける。deadline は live と同じ `SCRIPT_CALL_DEADLINE`。
//! - 結果は表 `script_exhaustive_runs` だけに書く（開始・[`EXHAUSTIVE_PROGRESS_EVERY`] ごとの進捗・終了）。
//!   live の `script_guard_state`・`script_compare_log`・`events` には書かない（D8）。
//! - 時刻は `tokio::time` だけで読む（試験は `tokio::time::pause` と [`ExhaustiveTuning::before_call`] の遅延フックで
//!   決定的に確かめる）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use task_core::SqliteStore;
use task_core::model_policy::{LaneCeiling, TaskFeatures, all_features, match_rule};
use task_core::script_compare::{CompareOutcome, LaneRulesInput};
use task_core::script_exhaustive::{
    LANE_RULES_EXHAUSTIVE_CALLS, ScriptExhaustiveCounts, ScriptExhaustiveState,
    ScriptExhaustiveStopCode,
};
use task_core::script_guard::ScriptGuardCall;
use task_dispatch::script_policy::{
    LaneRulesOutput, PolicyScript, SCRIPT_CALL_DEADLINE, TsResult, call_lane_rules,
    classify_lane_rules,
};
use time::OffsetDateTime;
use tokio::sync::watch;

/// 呼び出しの間隔（D3）。
pub const EXHAUSTIVE_CALL_GAP: Duration = Duration::from_millis(5);
/// timeout の後に待つ時間（D3）。
pub const EXHAUSTIVE_TIMEOUT_BACKOFF: Duration = Duration::from_secs(1);
/// 進捗を書く呼び出し数の間隔（D3）。
pub const EXHAUSTIVE_PROGRESS_EVERY: u64 = 1000;
/// 1 回の実行の上限（D3）。
pub const EXHAUSTIVE_WALL_LIMIT: Duration = Duration::from_secs(30 * 60);
/// timeout か失敗がこの件数続いたら打ち切る（D3）。
pub const EXHAUSTIVE_CONSECUTIVE_ERRORS: u32 = 3;
/// 終了・戻しの書き込みに失敗したときの書き直しの間隔（D5）。
pub const EXHAUSTIVE_FINISH_RETRY: Duration = Duration::from_secs(1);
/// live の要求を待つ間の最短の読み直し間隔（間隔が 0 の試験でも空回りしない）。
const LIVE_WAIT_MIN: Duration = Duration::from_millis(1);

/// 呼び出しの前に待つ遅延フック（試験だけ）。引数は 0 始まりの呼び出しの番号。
pub type BeforeCallHook =
    Arc<dyn Fn(u64) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// runner の時間の設定。本番は [`Default`]（ADR の値）を使い、試験だけが変える。
#[derive(Clone)]
pub struct ExhaustiveTuning {
    /// 1 呼び出しの deadline（本番は live と同じ `SCRIPT_CALL_DEADLINE`）。
    pub deadline: Duration,
    pub gap: Duration,
    pub timeout_backoff: Duration,
    pub wall_limit: Duration,
    pub progress_every: u64,
    /// 呼び出しの直前に待つフック（試験の出来事待ち）。本番は `None`。
    pub before_call: Option<BeforeCallHook>,
}

impl Default for ExhaustiveTuning {
    fn default() -> Self {
        Self {
            deadline: SCRIPT_CALL_DEADLINE,
            gap: EXHAUSTIVE_CALL_GAP,
            timeout_backoff: EXHAUSTIVE_TIMEOUT_BACKOFF,
            wall_limit: EXHAUSTIVE_WALL_LIMIT,
            progress_every: EXHAUSTIVE_PROGRESS_EVERY,
            before_call: None,
        }
    }
}

impl std::fmt::Debug for ExhaustiveTuning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExhaustiveTuning")
            .field("deadline", &self.deadline)
            .field("gap", &self.gap)
            .field("timeout_backoff", &self.timeout_backoff)
            .field("wall_limit", &self.wall_limit)
            .field("progress_every", &self.progress_every)
            .field("before_call", &self.before_call.is_some())
            .finish()
    }
}

/// 呼び出しの前に読む generation の様子（D3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetState {
    /// isolate が在り、in-flight が 0。
    Ready,
    /// live（dispatcher の compare・`cos_digest` の pass）の要求が走っている。呼ばずに待つ。
    LiveInFlight,
    /// isolate が無い・drain 中（generation が終わった）。
    Gone,
}

/// generation の様子を読む口。
pub type TargetProbe = Arc<dyn Fn() -> TargetState + Send + Sync>;

/// 1 回の実行の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct ExhaustiveOutcome {
    pub state: ScriptExhaustiveState,
    pub stop_code: Option<ScriptExhaustiveStopCode>,
    pub counts: ScriptExhaustiveCounts,
    /// 最初の不一致・安全違反・失敗の 1 件（D4 の `first_problem_json`）。
    pub first_problem: Option<serde_json::Value>,
}

/// 進捗の書き込み先（store の行）。`None` なら書かない（単体の試験）。
pub struct ProgressSink {
    pub db: Arc<SqliteStore>,
    pub run_id: String,
}

/// 集計の途中の値。
struct Tally {
    counts: ScriptExhaustiveCounts,
    latencies_us: Vec<u64>,
    covered: Vec<bool>,
    first_problem: Option<serde_json::Value>,
}

impl Tally {
    fn new(inputs: usize) -> Self {
        Self {
            counts: ScriptExhaustiveCounts::default(),
            latencies_us: Vec::with_capacity(LANE_RULES_EXHAUSTIVE_CALLS as usize),
            covered: vec![false; inputs],
            first_problem: None,
        }
    }

    /// 件数・未網羅・latency を埋めた値。
    fn snapshot(&self) -> ScriptExhaustiveCounts {
        let mut c = self.counts;
        c.uncovered_inputs = self.covered.iter().filter(|v| !**v).count() as u64;
        let mut sorted = self.latencies_us.clone();
        sorted.sort_unstable();
        c.latency_p50_us = nearest_rank(&sorted, 50);
        c.latency_p95_us = nearest_rank(&sorted, 95);
        c.latency_max_us = sorted.last().copied().unwrap_or(0);
        c
    }

    fn outcome(
        &self,
        state: ScriptExhaustiveState,
        stop_code: Option<ScriptExhaustiveStopCode>,
    ) -> ExhaustiveOutcome {
        ExhaustiveOutcome {
            state,
            stop_code,
            counts: self.snapshot(),
            first_problem: self.first_problem.clone(),
        }
    }

    fn stop(&self, code: ScriptExhaustiveStopCode) -> ExhaustiveOutcome {
        let state = if code.is_failure() {
            ScriptExhaustiveState::Failed
        } else {
            ScriptExhaustiveState::Aborted
        };
        self.outcome(state, Some(code))
    }
}

/// 最近傍順位の百分位（D4 の p95）。`sorted` は昇順。
fn nearest_rank(sorted: &[u64], pct: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (pct * sorted.len()).div_ceil(100).max(1);
    sorted[rank - 1]
}

/// `stop` が立ったか（送り手が消えたのも停止とみなす）。
fn stopped(stop: &watch::Receiver<bool>) -> bool {
    *stop.borrow() || stop.has_changed().is_err()
}

/// `d` だけ待つ。途中で `stop` が立てば `false`。
async fn sleep_or_stop(d: Duration, stop: &mut watch::Receiver<bool>) -> bool {
    if stopped(stop) {
        return false;
    }
    tokio::select! {
        biased;
        _ = stop.changed() => !stopped(stop),
        () = tokio::time::sleep(d) => true,
    }
}

/// 正順・逆順の (pass 名, 入力の番号) の列。
fn order(n: usize) -> impl Iterator<Item = (&'static str, usize)> {
    (0..n)
        .map(|i| ("forward", i))
        .chain((0..n).rev().map(|i| ("reverse", i)))
}

/// 全入力を流して結果を返す（D2・D3）。DB には `progress` へ進捗だけを書き、終了は呼び出し側が書く。
pub async fn drive(
    policy: &dyn PolicyScript,
    probe: &TargetProbe,
    tuning: &ExhaustiveTuning,
    stop: &mut watch::Receiver<bool>,
    progress: Option<&ProgressSink>,
) -> ExhaustiveOutcome {
    let inputs: Vec<TaskFeatures> = all_features();
    let ceiling = LaneCeiling::default();
    let started = tokio::time::Instant::now();
    let mut tally = Tally::new(inputs.len());
    let mut consecutive_errors = 0u32;
    let total = LANE_RULES_EXHAUSTIVE_CALLS;
    for (n, (pass, index)) in order(inputs.len()).enumerate() {
        let n = n as u64;
        // live を先に通す: in-flight が 0 になるまで呼ばない。
        loop {
            if stopped(stop) {
                return tally.stop(ScriptExhaustiveStopCode::DaemonStopped);
            }
            if started.elapsed() >= tuning.wall_limit {
                return tally.stop(ScriptExhaustiveStopCode::WallLimit);
            }
            match probe() {
                TargetState::Ready => break,
                TargetState::Gone => return tally.stop(ScriptExhaustiveStopCode::GenerationGone),
                TargetState::LiveInFlight => {
                    if !sleep_or_stop(tuning.gap.max(LIVE_WAIT_MIN), stop).await {
                        return tally.stop(ScriptExhaustiveStopCode::DaemonStopped);
                    }
                }
            }
        }
        if let Some(hook) = &tuning.before_call {
            tokio::select! {
                biased;
                _ = stop.changed() => return tally.stop(ScriptExhaustiveStopCode::DaemonStopped),
                () = hook(n) => {}
            }
        }
        let features = &inputs[index];
        let rust = match_rule(features);
        let rust_lane = rust.lane;
        let rust = LaneRulesOutput::from(rust);
        let call = call_lane_rules(policy, LaneRulesInput::redacted(features), tuning.deadline);
        let (result, elapsed) = tokio::select! {
            biased;
            _ = stop.changed() => return tally.stop(ScriptExhaustiveStopCode::DaemonStopped),
            r = call => r,
        };
        let classified = classify_lane_rules(&rust, rust_lane, &ceiling, &result);
        let error_kind = classified.error.as_ref().map(|e| e.kind.as_str());
        // `GenerationGone`: isolate が消えた・drain 中なら generation の終わり。在るなら live の timeout による作り直しに
        // 巻き込まれただけなので失敗に数えて続ける。
        if error_kind == Some("generation_gone") && probe() == TargetState::Gone {
            return tally.stop(ScriptExhaustiveStopCode::GenerationGone);
        }
        tally.counts.calls += 1;
        tally
            .latencies_us
            .push(u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
        let safety = !classified.safety_violations.is_empty();
        if safety {
            tally.counts.safety_violations += 1;
        }
        let call_class = ScriptGuardCall::classify(classified.outcome, error_kind);
        let mut timed_out = false;
        match call_class {
            Some(ScriptGuardCall::Ok) => {
                tally.counts.matches += 1;
                tally.covered[index] = true;
                consecutive_errors = 0;
            }
            // `lane_rules` に承認できる改善仕様は無い: `explained` も不一致に数える（D2）。
            Some(ScriptGuardCall::Mismatch | ScriptGuardCall::Explained) => {
                tally.counts.mismatches += 1;
                consecutive_errors = 0;
            }
            Some(ScriptGuardCall::Timeout) => {
                tally.counts.timeouts += 1;
                consecutive_errors += 1;
                timed_out = true;
            }
            Some(ScriptGuardCall::Failure) | None => {
                tally.counts.failures += 1;
                consecutive_errors += 1;
            }
        }
        if (safety || call_class != Some(ScriptGuardCall::Ok)) && tally.first_problem.is_none() {
            tally.first_problem = Some(problem_json(
                pass,
                index,
                features,
                &rust,
                &result,
                classified.outcome,
                &classified,
            ));
        }
        if safety {
            return tally.stop(ScriptExhaustiveStopCode::SafetyViolation);
        }
        if tally.counts.mismatches > 0 {
            return tally.stop(ScriptExhaustiveStopCode::Mismatch);
        }
        if consecutive_errors >= EXHAUSTIVE_CONSECUTIVE_ERRORS {
            return tally.stop(ScriptExhaustiveStopCode::ConsecutiveErrors);
        }
        if tally.counts.timeouts * 1000 > total || tally.counts.failures * 1000 > total {
            return tally.stop(ScriptExhaustiveStopCode::RateExceeded);
        }
        if let Some(sink) = progress
            && tuning.progress_every > 0
            && tally.counts.calls.is_multiple_of(tuning.progress_every)
        {
            write_progress(sink, &tally).await;
        }
        let wait = if timed_out {
            tuning.timeout_backoff
        } else {
            tuning.gap
        };
        if !sleep_or_stop(wait, stop).await {
            return tally.stop(ScriptExhaustiveStopCode::DaemonStopped);
        }
    }
    let counts = tally.snapshot();
    let state = if counts.verdict() {
        ScriptExhaustiveState::Passed
    } else {
        ScriptExhaustiveState::Failed
    };
    tally.outcome(state, None)
}

fn problem_json(
    pass: &str,
    index: usize,
    features: &TaskFeatures,
    rust: &LaneRulesOutput,
    result: &TsResult,
    outcome: CompareOutcome,
    classified: &task_dispatch::script_policy::Classified,
) -> serde_json::Value {
    let ts = match result {
        TsResult::Reply(_) => classified.ts.clone(),
        _ => None,
    };
    serde_json::json!({
        "pass": pass,
        "input_index": index,
        "input": LaneRulesInput::redacted(features),
        "rust": rust,
        "ts": ts,
        "outcome": outcome,
        "error": classified.error,
        "safety_violations": classified.safety_violations,
    })
}

async fn write_progress(sink: &ProgressSink, tally: &Tally) {
    let db = Arc::clone(&sink.db);
    let id = sink.run_id.clone();
    let counts = tally.snapshot();
    let first = tally.first_problem.as_ref().map(|v| v.to_string());
    let done = tokio::task::spawn_blocking(move || {
        db.script_exhaustive_progress(&id, &counts, first.as_deref())
    })
    .await;
    match done {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            tracing::warn!(error = %e, run = %sink.run_id, "script exhaustive: progress not written")
        }
        Err(e) => {
            tracing::warn!(error = %e, run = %sink.run_id, "script exhaustive: progress task failed")
        }
    }
}

/// 結果を書く（D5）。書き込みに失敗したら [`EXHAUSTIVE_FINISH_RETRY`] ごとに書き直す。`stop` が立った後は
/// もう 1 回だけ試して諦める（行は `running` のまま残り、次の起動で `aborted` になる）。
pub async fn finish(
    db: &Arc<SqliteStore>,
    run_id: &str,
    outcome: &ExhaustiveOutcome,
    stop: &watch::Receiver<bool>,
) {
    let first = outcome.first_problem.as_ref().map(|v| v.to_string());
    loop {
        let db2 = Arc::clone(db);
        let id = run_id.to_owned();
        let (state, code, counts, first2) = (
            outcome.state,
            outcome.stop_code,
            outcome.counts,
            first.clone(),
        );
        let done = tokio::task::spawn_blocking(move || {
            db2.script_exhaustive_finish(
                &id,
                state,
                code,
                &counts,
                first2.as_deref(),
                OffsetDateTime::now_utc(),
            )
            .map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(format!("the blocking task failed: {e}")));
        match done {
            Ok(Some(f)) => {
                tracing::info!(
                    run = run_id,
                    state = outcome.state.as_str(),
                    stop_code = outcome.stop_code.map(ScriptExhaustiveStopCode::as_str),
                    calls = outcome.counts.calls,
                    rolled_back = ?f.rolled_back,
                    invalidated = f.invalidated,
                    "script exhaustive: run finished"
                );
                return;
            }
            Ok(None) => {
                tracing::warn!(
                    run = run_id,
                    "script exhaustive: the run was already closed (aborted at a restart); result dropped"
                );
                return;
            }
            Err(error) => {
                tracing::error!(%error, run = run_id, "script exhaustive: could not write the result; retrying");
                if stopped(stop) {
                    return;
                }
                tokio::time::sleep(EXHAUSTIVE_FINISH_RETRY).await;
            }
        }
    }
}

/// runner の本体: 流して結果を書く。
pub async fn run(
    policy: Arc<dyn PolicyScript>,
    probe: TargetProbe,
    tuning: ExhaustiveTuning,
    mut stop: watch::Receiver<bool>,
    db: Arc<SqliteStore>,
    run_id: String,
) {
    let sink = ProgressSink {
        db: Arc::clone(&db),
        run_id: run_id.clone(),
    };
    let outcome = drive(policy.as_ref(), &probe, &tuning, &mut stop, Some(&sink)).await;
    finish(&db, &run_id, &outcome, &stop).await;
}

#[cfg(test)]
#[path = "script_exhaustive_tests.rs"]
mod tests;
