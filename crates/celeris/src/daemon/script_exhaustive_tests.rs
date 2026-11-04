//! runner の単体試験（ADR 2026-11-04-lane-rules-exhaustive-evidence D3）。偽の isolate（[`PolicyScript`] の実装）と
//! `tokio::time::pause` で、間隔・timeout の待ち・打ち切りを決定的に確かめる。CPU 負荷はかけない。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use task_core::model_policy::{TaskFeatures, all_features, match_rule};
use task_core::script_compare::{LaneRulesInput, RoutePriorityInput};
use task_core::script_exhaustive::{
    LANE_RULES_EXHAUSTIVE_CALLS, LANE_RULES_EXHAUSTIVE_INPUTS, ScriptExhaustiveState,
    ScriptExhaustiveStopCode,
};
use task_dispatch::script_policy::{
    CosDigestPassCall, LaneRulesOutput, PolicyFuture, PolicyReply, PolicyScript,
    PolicyScriptBundle, PolicyScriptError,
};
use tokio::sync::watch;
use tokio::time::Instant;

use super::*;

/// 偽の isolate の答え方。
#[derive(Clone, Copy)]
enum Mode {
    /// Rust と同じ答え。
    Reference,
    /// 常に違う答え。
    Different,
    /// 答えない（deadline で timeout）。
    Hang,
    /// capability の拒否（安全違反）。
    Denied,
    /// 奇数番目だけ例外（連続 3 件にならない失敗）。
    FailOdd,
    /// 10 ms かかる参照の答え。
    Slow,
}

struct Fake {
    mode: Mode,
    calls: AtomicU64,
    /// 呼ばれた入力と時刻。
    seen: Mutex<Vec<(TaskFeatures, Instant)>>,
}

impl Fake {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            calls: AtomicU64::new(0),
            seen: Mutex::new(Vec::new()),
        })
    }
}

impl PolicyScript for Fake {
    fn bundle(&self) -> PolicyScriptBundle {
        PolicyScriptBundle {
            bundle_sha256: "f".repeat(64),
            host_api: "1".into(),
        }
    }

    fn lane_rules(&self, input: LaneRulesInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async move {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen
                .lock()
                .expect("seen")
                .push((input.features, Instant::now()));
            let reference = || {
                serde_json::to_value(LaneRulesOutput::from(match_rule(&input.features)))
                    .expect("json")
            };
            let output = match self.mode {
                Mode::Reference => reference(),
                Mode::Slow => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    reference()
                }
                Mode::Different => serde_json::json!({
                    "rule_id": "always/other", "proposed": "frontier", "condition": "different"
                }),
                Mode::Hang => {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    reference()
                }
                Mode::Denied => {
                    return Err(PolicyScriptError {
                        kind: "denied".into(),
                        message: "capability".into(),
                        safety_violation: Some("denied_capability".into()),
                        isolate_epoch: Some(1),
                    });
                }
                Mode::FailOdd if n % 2 == 1 => {
                    return Err(PolicyScriptError::new("exception", "boom"));
                }
                Mode::FailOdd => reference(),
            };
            Ok(PolicyReply {
                output,
                isolate_epoch: 1,
            })
        })
    }

    fn route_priority(&self, _input: RoutePriorityInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "unused")) })
    }

    fn cos_digest_pass(&self, _call: CosDigestPassCall, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "unused")) })
    }
}

fn ready() -> TargetProbe {
    Arc::new(|| TargetState::Ready)
}

async fn go(fake: &Arc<Fake>, probe: &TargetProbe, tuning: &ExhaustiveTuning) -> ExhaustiveOutcome {
    let (_tx, mut rx) = watch::channel(false);
    drive(fake.as_ref(), probe, tuning, &mut rx, None).await
}

/// 全入力を正順・逆順で 1 回ずつ流し、全件 `match` なら合格。呼び出しの間は 5 ms ずつ空く。
#[tokio::test(start_paused = true)]
async fn exhaustive_drive_all_inputs_forward_then_reverse() {
    let fake = Fake::new(Mode::Reference);
    let out = go(&fake, &ready(), &ExhaustiveTuning::default()).await;
    assert_eq!(out.state, ScriptExhaustiveState::Passed, "{out:?}");
    assert_eq!(out.stop_code, None);
    assert_eq!(out.counts.calls, LANE_RULES_EXHAUSTIVE_CALLS);
    assert_eq!(out.counts.matches, LANE_RULES_EXHAUSTIVE_CALLS);
    assert_eq!(out.counts.uncovered_inputs, 0);
    assert!(out.first_problem.is_none());
    let seen = fake.seen.lock().expect("seen");
    let all = all_features();
    let n = LANE_RULES_EXHAUSTIVE_INPUTS as usize;
    assert_eq!(seen.len(), 2 * n);
    for (i, f) in all.iter().enumerate() {
        assert_eq!(&seen[i].0, f, "forward {i}");
        assert_eq!(&seen[2 * n - 1 - i].0, f, "reverse {i}");
    }
    // 間隔: 次の呼び出しは前の呼び出しから EXHAUSTIVE_CALL_GAP 以上後。
    for w in seen.windows(2) {
        assert!(w[1].1 - w[0].1 >= EXHAUSTIVE_CALL_GAP);
    }
}

/// live の in-flight がある間は呼ばない。0 になったら続ける。
#[tokio::test(start_paused = true)]
async fn exhaustive_waits_for_live_in_flight() {
    let fake = Fake::new(Mode::Reference);
    let live = Arc::new(AtomicBool::new(true));
    let probe: TargetProbe = {
        let live = Arc::clone(&live);
        Arc::new(move || {
            if live.load(Ordering::SeqCst) {
                TargetState::LiveInFlight
            } else {
                TargetState::Ready
            }
        })
    };
    let task = {
        let fake = Arc::clone(&fake);
        let probe = Arc::clone(&probe);
        tokio::spawn(async move { go(&fake, &probe, &ExhaustiveTuning::default()).await })
    };
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(
        fake.calls.load(Ordering::SeqCst),
        0,
        "no call while live runs"
    );
    live.store(false, Ordering::SeqCst);
    let out = task.await.expect("drive");
    assert_eq!(out.state, ScriptExhaustiveState::Passed);
}

/// timeout の後は 1 秒待ち、3 件続いたら打ち切る（`consecutive_errors`）。deadline は live と同じ 50 ms。
#[tokio::test(start_paused = true)]
async fn exhaustive_timeout_backs_off_and_stops_at_three() {
    let fake = Fake::new(Mode::Hang);
    let tuning = ExhaustiveTuning::default();
    assert_eq!(tuning.deadline, Duration::from_millis(50));
    let out = go(&fake, &ready(), &tuning).await;
    assert_eq!(out.state, ScriptExhaustiveState::Failed);
    assert_eq!(
        out.stop_code,
        Some(ScriptExhaustiveStopCode::ConsecutiveErrors)
    );
    assert_eq!(out.counts.calls, 3);
    assert_eq!(out.counts.timeouts, 3);
    assert_eq!(out.counts.latency_max_us, 50_000);
    let seen = fake.seen.lock().expect("seen");
    for w in seen.windows(2) {
        assert!(
            w[1].1 - w[0].1 >= tuning.deadline + EXHAUSTIVE_TIMEOUT_BACKOFF,
            "backoff after a timeout"
        );
    }
    let problem = out.first_problem.expect("first problem");
    assert_eq!(problem["error"]["kind"], "timeout");
}

/// 失敗の率が 0.1% を超えると決まった時点で打ち切る（`rate_exceeded`）。
#[tokio::test(start_paused = true)]
async fn exhaustive_failure_rate_stops() {
    let fake = Fake::new(Mode::FailOdd);
    let out = go(&fake, &ready(), &ExhaustiveTuning::default()).await;
    assert_eq!(out.state, ScriptExhaustiveState::Failed);
    assert_eq!(out.stop_code, Some(ScriptExhaustiveStopCode::RateExceeded));
    assert_eq!(out.counts.failures, 40);
    assert_eq!(out.counts.calls, 80);
}

/// 不一致は 1 件目で止めて不合格。
#[tokio::test(start_paused = true)]
async fn exhaustive_mismatch_stops_at_first() {
    let fake = Fake::new(Mode::Different);
    let out = go(&fake, &ready(), &ExhaustiveTuning::default()).await;
    assert_eq!(out.state, ScriptExhaustiveState::Failed);
    assert_eq!(out.stop_code, Some(ScriptExhaustiveStopCode::Mismatch));
    assert_eq!((out.counts.calls, out.counts.mismatches), (1, 1));
    assert!(!out.counts.verdict());
    let problem = out.first_problem.expect("first problem");
    assert_eq!(problem["pass"], "forward");
    assert_eq!(problem["input_index"], 0);
    assert_eq!(problem["ts"]["rule_id"], "always/other");
}

/// 安全違反は 1 件目で止めて不合格。
#[tokio::test(start_paused = true)]
async fn exhaustive_safety_violation_stops() {
    let fake = Fake::new(Mode::Denied);
    let out = go(&fake, &ready(), &ExhaustiveTuning::default()).await;
    assert_eq!(
        out.stop_code,
        Some(ScriptExhaustiveStopCode::SafetyViolation)
    );
    assert_eq!(out.state, ScriptExhaustiveState::Failed);
    assert_eq!(out.counts.safety_violations, 1);
}

/// generation が消えたら `aborted`（`generation_gone`）。
#[tokio::test(start_paused = true)]
async fn exhaustive_generation_gone_aborts() {
    let fake = Fake::new(Mode::Reference);
    let probe: TargetProbe = Arc::new(|| TargetState::Gone);
    let out = go(&fake, &probe, &ExhaustiveTuning::default()).await;
    assert_eq!(out.state, ScriptExhaustiveState::Aborted);
    assert_eq!(
        out.stop_code,
        Some(ScriptExhaustiveStopCode::GenerationGone)
    );
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
}

/// 30 分を超えたら `aborted`（`wall_limit`）。
#[tokio::test(start_paused = true)]
async fn exhaustive_wall_limit_aborts() {
    let fake = Fake::new(Mode::Slow);
    let tuning = ExhaustiveTuning {
        wall_limit: Duration::from_secs(1),
        ..ExhaustiveTuning::default()
    };
    let out = go(&fake, &ready(), &tuning).await;
    assert_eq!(out.stop_code, Some(ScriptExhaustiveStopCode::WallLimit));
    assert_eq!(out.state, ScriptExhaustiveState::Aborted);
    // 1 呼び出し 10 ms + 間隔 5 ms で 1 秒: 67 件。
    assert_eq!(out.counts.calls, 67);
    assert_eq!(out.counts.latency_p50_us, 10_000);
}

/// 停止の合図で `aborted`（`daemon_stopped`）。
#[tokio::test(start_paused = true)]
async fn exhaustive_stop_signal_aborts() {
    let fake = Fake::new(Mode::Reference);
    let (tx, mut rx) = watch::channel(false);
    let task = {
        let fake = Arc::clone(&fake);
        tokio::spawn(async move {
            drive(
                fake.as_ref(),
                &ready(),
                &ExhaustiveTuning::default(),
                &mut rx,
                None,
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(52)).await;
    tx.send(true).expect("stop");
    let out = task.await.expect("drive");
    assert_eq!(out.stop_code, Some(ScriptExhaustiveStopCode::DaemonStopped));
    assert_eq!(out.state, ScriptExhaustiveState::Aborted);
    assert!(out.counts.calls > 0 && out.counts.calls < LANE_RULES_EXHAUSTIVE_CALLS);
}

#[test]
fn nearest_rank_percentiles() {
    assert_eq!(nearest_rank(&[], 95), 0);
    let v: Vec<u64> = (1..=100).collect();
    assert_eq!(nearest_rank(&v, 50), 50);
    assert_eq!(nearest_rank(&v, 95), 95);
    assert_eq!(nearest_rank(&[7], 95), 7);
}
