//! 実行 shadow の queue の試験（ADR 2026-10-04 §10 Phase 4）。時間は `tokio::time::pause`
//! （`start_paused`）で、実行の進みは gate（Notify）で決定的に制御する。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use task_core::model_router::shadow::{SHADOW_ALLOW_ANY, ShadowAllowlist};
use time::macros::datetime;
use tokio::sync::Notify;

use super::*;
use crate::openai::{ChatMessage, MessageContent};
use crate::reservation::FixedClock;

#[derive(Default)]
struct RecordingSink {
    events: StdMutex<Vec<ShadowEvent>>,
    changed: Notify,
}

impl RecordingSink {
    fn records(&self) -> Vec<ShadowRecord> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|e| e.record.clone())
            .collect()
    }
    fn by_id(&self, id: &str) -> ShadowRecord {
        self.records()
            .into_iter()
            .find(|r| r.shadow_id == id)
            .unwrap_or_else(|| panic!("no record for {id}"))
    }
    async fn wait_len(&self, n: usize) {
        loop {
            let changed = self.changed.notified();
            if self.records().len() >= n {
                return;
            }
            changed.await;
        }
    }
}

impl ShadowSink for RecordingSink {
    fn record(&self, event: ShadowEvent) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event);
        self.changed.notify_one();
    }
}

/// tool call を含む応答（実行してはいけない出力）。
const TOOL_CALL_BODY: &str = r#"{"choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"c1","type":"function","function":{"name":"rm_rf","arguments":"{\"path\":\"/\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":11,"completion_tokens":7}}"#;

enum Behavior {
    /// gate が開くまで止まり、tool call を含む出力を返す。
    Gate(Arc<Notify>),
    /// 返らない（timeout させる）。
    Hang,
}

#[derive(Default)]
struct GateExecutor {
    behaviors: StdMutex<HashMap<String, Behavior>>,
    calls: StdMutex<HashMap<String, u32>>,
    total: AtomicU32,
    arrived: Notify,
}

impl GateExecutor {
    fn set(&self, id: &str, b: Behavior) {
        self.behaviors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), b);
    }
    fn calls(&self, id: &str) -> u32 {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .copied()
            .unwrap_or(0)
    }
}

impl ShadowExecutor for GateExecutor {
    fn execute(&self, job: &ShadowJob) -> ShadowFuture {
        *self
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(job.shadow_id.clone())
            .or_insert(0) += 1;
        self.total.fetch_add(1, Ordering::SeqCst);
        self.arrived.notify_one();
        let gate = match self
            .behaviors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&job.shadow_id)
        {
            Some(Behavior::Gate(n)) => Some(n.clone()),
            Some(Behavior::Hang) | None => None,
        };
        Box::pin(async move {
            match gate {
                Some(n) => {
                    n.notified().await;
                    Ok(ShadowOutput {
                        body: TOOL_CALL_BODY.as_bytes().to_vec(),
                        input_tokens: Some(11),
                        output_tokens: Some(7),
                        effective_usd: Some(0.002),
                    })
                }
                None => std::future::pending().await,
            }
        })
    }
}

fn policy() -> ShadowPolicy {
    ShadowPolicy {
        execute: true,
        allowlist: ShadowAllowlist {
            task_kinds: vec![SHADOW_ALLOW_ANY.to_string()],
            roles: vec!["coder".to_string()],
            lanes: vec![SHADOW_ALLOW_ANY.to_string()],
            sources: vec![SHADOW_ALLOW_ANY.to_string()],
        },
        sample_rate: 1.0,
        daily_max_requests: Some(100),
        daily_max_tokens: Some(1_000_000),
        daily_max_effective_usd: Some(10.0),
        max_concurrency: Some(1),
        max_queue_depth: Some(1),
        timeout_ms: Some(1000),
    }
}

fn job(id: &str) -> ShadowJob {
    ShadowJob {
        shadow_id: id.to_string(),
        primary_decision_id: format!("pdec_{id}"),
        task_id: Some("t1".to_string()),
        run_id: Some("r1".to_string()),
        request_id: Some(format!("req_{id}")),
        target: ShadowTarget {
            task_kind: "coding".to_string(),
            role: "coder".to_string(),
            lane: "cheap".to_string(),
            source: "openai-compatible:qwen".to_string(),
        },
        candidate_model: "qwen3.8-27b".to_string(),
        primary_source: "claude-oauth".to_string(),
        primary_resource_group: None,
        candidate_resource_group: None,
        request: ChatCompletionRequest {
            model: "celeris/cheap".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: Some(MessageContent::Text("hi".to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            temperature: None,
            top_p: None,
            max_tokens: Some(64),
            stop: None,
            stream: true,
            tools: None,
            tool_choice: None,
        },
        worst_tokens: 80,
        worst_effective_usd: Some(0.01),
    }
}

#[tokio::test(start_paused = true)]
async fn routing_shadow_backpressure_timeout_and_no_tool_execution() {
    let sink = Arc::new(RecordingSink::default());
    let exec = Arc::new(GateExecutor::default());
    let budget = Arc::new(AllowAllBudget::default());
    let queue = ShadowQueue::new(
        policy(),
        "instance-a",
        exec.clone(),
        budget.clone(),
        sink.clone(),
        Arc::new(FixedClock(datetime!(2026-10-05 12:00:00 UTC))),
    )
    .expect("execute policy is valid");

    // off の policy は queue を作らない（送信 0）。
    assert!(
        ShadowQueue::new(
            ShadowPolicy::default(),
            "instance-a",
            exec.clone(),
            budget.clone(),
            sink.clone(),
            Arc::new(FixedClock(datetime!(2026-10-05 12:00:00 UTC))),
        )
        .is_none()
    );

    // A. a は gate で止まる。submit は同期で返り（primary は待たない）、b は queue、c は満杯で dropped。
    let gate_a = Arc::new(Notify::new());
    exec.set("a", Behavior::Gate(gate_a.clone()));
    exec.set("b", Behavior::Hang);
    let arrived = exec.arrived.notified();
    assert_eq!(queue.submit(job("a")), SubmitOutcome::Started);
    arrived.await;
    assert_eq!(queue.submit(job("b")), SubmitOutcome::Queued);
    assert_eq!(
        queue.submit(job("c")),
        SubmitOutcome::Dropped(ShadowReason::QueueFull)
    );
    assert_eq!(queue.depth(), (1, 1));
    sink.wait_len(1).await;
    let c = sink.by_id("c");
    assert_eq!(
        (c.status, c.reason),
        (ShadowStatus::Dropped, Some(ShadowReason::QueueFull))
    );
    assert_eq!(exec.calls("c"), 0);

    // a を完了させる。tool call を含む出力は hash と tokens だけが残り、実行も続きの送信もしない。
    gate_a.notify_one();
    sink.wait_len(2).await;
    let a = sink.by_id("a");
    assert_eq!((a.status, a.reason), (ShadowStatus::Completed, None));
    assert_eq!(a.kind, ShadowKind::Execution);
    assert_eq!(
        a.output_sha256.as_deref(),
        Some(output_sha256(TOOL_CALL_BODY.as_bytes()).as_str())
    );
    assert_eq!((a.input_tokens, a.output_tokens), (Some(11), Some(7)));
    assert_eq!(a.reservation_id.as_deref(), Some("fake-a"));
    assert_eq!(exec.calls("a"), 1, "no follow-up turn after a tool call");
    let a_json = serde_json::to_string(&a).expect("json");
    assert!(!a_json.contains("rm_rf") && !a_json.contains("tool_calls"));
    a.validate().expect("valid record");

    // B. a の後に b が始まり、実行中に timeout する（時計は pause、gate は開かない）→ failed/timeout。
    sink.wait_len(3).await;
    let b = sink.by_id("b");
    assert_eq!(
        (b.status, b.reason),
        (ShadowStatus::Failed, Some(ShadowReason::Timeout))
    );
    assert_eq!(b.latency_ms, Some(1000));
    assert!(
        budget
            .settled()
            .contains(&("fake-b".to_string(), ShadowSettlement::TimedOut))
    );

    // C. d が実行中に e が queue で待ち、d の timeout と同時に e は期限切れで dropped（未送信）。
    exec.set("d", Behavior::Hang);
    exec.set("e", Behavior::Hang);
    assert_eq!(queue.submit(job("d")), SubmitOutcome::Started);
    assert_eq!(queue.submit(job("e")), SubmitOutcome::Queued);
    sink.wait_len(5).await;
    let d = sink.by_id("d");
    assert_eq!(
        (d.status, d.reason),
        (ShadowStatus::Failed, Some(ShadowReason::Timeout))
    );
    let e = sink.by_id("e");
    assert_eq!(
        (e.status, e.reason, e.detail.as_deref()),
        (
            ShadowStatus::Dropped,
            Some(ShadowReason::Timeout),
            Some("expired_in_queue")
        )
    );
    assert_eq!(exec.calls("e"), 0);
    assert_eq!(queue.depth(), (0, 0));

    // D. primary の予約待ちが起きたら未開始の g を落とす（実行中の f は止めない）。
    let gate_f = Arc::new(Notify::new());
    exec.set("f", Behavior::Gate(gate_f.clone()));
    let arrived = exec.arrived.notified();
    assert_eq!(queue.submit(job("f")), SubmitOutcome::Started);
    arrived.await;
    assert_eq!(queue.submit(job("g")), SubmitOutcome::Queued);
    assert_eq!(queue.primary_pressure(), 1);
    sink.wait_len(6).await;
    let g = sink.by_id("g");
    assert_eq!(
        (g.status, g.reason),
        (ShadowStatus::Dropped, Some(ShadowReason::PrimaryPressure))
    );
    assert_eq!(exec.calls("g"), 0);
    gate_f.notify_one();
    sink.wait_len(7).await;
    assert_eq!(sink.by_id("f").status, ShadowStatus::Completed);

    // E. primary と同じ非分離 resource group（明示の group 一致・同じ source）は送らずに dropped。
    let mut h = job("h");
    h.primary_resource_group = Some("gpu0".to_string());
    h.candidate_resource_group = Some("gpu0".to_string());
    assert_eq!(
        queue.submit(h),
        SubmitOutcome::Dropped(ShadowReason::ResourceGroupShared)
    );
    let mut i = job("i");
    i.primary_source = i.target.source.clone();
    assert_eq!(
        queue.submit(i),
        SubmitOutcome::Dropped(ShadowReason::ResourceGroupShared)
    );
    let mut j = job("j");
    j.primary_resource_group = Some("gpu0".to_string());
    j.candidate_resource_group = Some("gpu1".to_string());
    j.worst_effective_usd = None;
    // F. 分離 group でも未知の費用は予約されず dropped（送信 0）。
    assert_eq!(queue.submit(j), SubmitOutcome::Started);
    sink.wait_len(10).await;
    assert_eq!(
        sink.by_id("h").reason,
        Some(ShadowReason::ResourceGroupShared)
    );
    assert_eq!(
        sink.by_id("i").reason,
        Some(ShadowReason::ResourceGroupShared)
    );
    let j = sink.by_id("j");
    assert_eq!(
        (j.status, j.reason, j.reservation_id.as_deref()),
        (ShadowStatus::Dropped, Some(ShadowReason::UnknownCost), None)
    );

    // G. allowlist 外は入口で落ち、記録も送信もしない。
    let mut k = job("k");
    k.target.role = "reviewer".to_string();
    assert_eq!(
        queue.submit(k),
        SubmitOutcome::NotAdmitted(ShadowReason::NotAllowlisted)
    );

    for id in ["c", "e", "g", "h", "i", "j", "k"] {
        assert_eq!(exec.calls(id), 0, "{id} must not reach the executor");
    }
    assert_eq!(exec.total.load(Ordering::SeqCst), 4, "a, b, d, f only");
    assert_eq!(sink.records().len(), 10);
    for r in sink.records() {
        r.validate().expect("every record is valid");
    }
}

#[test]
fn shadow_job_upstream_body_is_a_non_stream_copy_for_the_candidate() {
    let j = job("x");
    let body = j.upstream_body();
    assert_eq!(body["model"], "qwen3.8-27b");
    assert_eq!(body["stream"], false);
    assert_eq!(body["messages"][0]["content"], "hi");
    assert_eq!(body["max_tokens"], 64);
}

#[test]
fn decision_record_compares_candidates_without_usage() {
    let candidates = vec![
        ShadowCandidate {
            source: "claude-oauth".to_string(),
            model: "claude-haiku".to_string(),
        },
        ShadowCandidate {
            source: "openai-compatible:qwen".to_string(),
            model: "qwen3.8-27b".to_string(),
        },
    ];
    let r = decision_record(
        &PreferSelfHosted,
        "s1".to_string(),
        "pdec_1".to_string(),
        None,
        None,
        &candidates,
        &candidates[0],
    );
    assert_eq!(r.kind, ShadowKind::Decision);
    assert_eq!(r.detail.as_deref(), Some("differs_from_primary"));
    assert_eq!(
        r.candidate_source.as_deref(),
        Some("openai-compatible:qwen")
    );
    r.validate().expect("decision record carries no usage");
    let same = decision_record(
        &PreferSelfHosted,
        "s2".to_string(),
        "pdec_2".to_string(),
        None,
        None,
        &candidates,
        &candidates[1],
    );
    assert_eq!(same.detail.as_deref(), Some("same_as_primary"));
}

#[tokio::test(start_paused = true)]
async fn routing_shadow_uses_only_capacity_left_by_primary() {
    use crate::reservation::{CapacityLimits, ReservationTable, SlotKey};

    let sink = Arc::new(RecordingSink::default());
    let exec = Arc::new(GateExecutor::default());
    let budget = Arc::new(AllowAllBudget::default());
    let table = ReservationTable::new(CapacityLimits {
        per_account: None,
        resource_groups: HashMap::from([("gpu-b".to_string(), 1)]),
    });
    let queue = ShadowQueue::new(
        policy(),
        "instance-a",
        exec.clone(),
        budget.clone(),
        sink.clone(),
        Arc::new(FixedClock(datetime!(2026-10-05 12:00:00 UTC))),
    )
    .expect("execute policy is valid")
    .with_capacity(table.clone());
    let on_gpu_b = |id: &str| {
        let mut j = job(id);
        j.candidate_resource_group = Some("gpu-b".to_string());
        j
    };

    // primary が先に gpu-b の枠を取っている: shadow は待たずに dropped、予約・送信 0。
    let primary = table
        .try_reserve(&[SlotKey::group("gpu-b")])
        .expect("primary takes the slot first");
    assert_eq!(queue.submit(on_gpu_b("p")), SubmitOutcome::Started);
    sink.wait_len(1).await;
    let p = sink.by_id("p");
    assert_eq!(
        (p.status, p.reason),
        (ShadowStatus::Dropped, Some(ShadowReason::ConcurrencyLimit))
    );
    assert_eq!(exec.calls("p"), 0);
    assert!(budget.settled().is_empty());
    assert_eq!(queue.depth(), (0, 0));

    // primary が返した残り枠は使う。shadow が持つ間は primary 1 + shadow 1 にならない（上限 1）。
    drop(primary);
    let gate = Arc::new(Notify::new());
    exec.set("q", Behavior::Gate(gate.clone()));
    let arrived = exec.arrived.notified();
    assert_eq!(queue.submit(on_gpu_b("q")), SubmitOutcome::Started);
    arrived.await;
    assert_eq!(table.held(&SlotKey::group("gpu-b")), 1);
    gate.notify_one();
    sink.wait_len(2).await;
    assert_eq!(sink.by_id("q").status, ShadowStatus::Completed);
    assert_eq!(table.held(&SlotKey::group("gpu-b")), 0);
    assert_eq!(table.peak(&SlotKey::group("gpu-b")), 1);
}
