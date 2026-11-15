//! campaign の結合試験（`campaign_acceptance_e2e.rs`・`campaign_lock_e2e.rs`・`campaign_wiring_e2e.rs`）が
//! 共有する部品: chunked の応答本文を解く `dechunk` と、dispatcher が積む run の event の雛形。
//!
//! `#[path]` で読む。campaign の crate（`crates/campaign`）の試験の部品ではない。

use task_core::{Event, RunEnd, RunMetrics, RunRole, Usage as RunUsage};

/// `Transfer-Encoding: chunked` の本文を連結して返す（長さ 0 の chunk か壊れた chunk で止める）。
pub fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = b.windows(2).position(|w| w == b"\r\n") {
        let n = usize::from_str_radix(String::from_utf8_lossy(&b[..i]).trim(), 16).unwrap_or(0);
        if n == 0 || b.len() < i + 2 + n {
            break;
        }
        out.extend_from_slice(&b[i + 2..i + 2 + n]);
        b = &b[(i + 2 + n + 2).min(b.len())..];
    }
    out
}

/// dispatcher が積む `WorkerStarted`（adapter `claude-code`・model `m-exec`・source `src`）。
pub fn ev_started(run: &str, role: Option<RunRole>) -> Event {
    Event::WorkerStarted {
        run_id: run.into(),
        adapter: "claude-code".into(),
        model: "m-exec".into(),
        provider: Some("src".into()),
        account: None,
        role,
        task_role: None,
    }
}

/// dispatcher が積む `WorkerFinished`（usage と metrics 付き、`Completed` で終わる）。
pub fn ev_finished(run: &str, role: Option<RunRole>, tokens: u64) -> Event {
    Event::WorkerFinished {
        run_id: run.into(),
        outcome: "x".into(),
        usage: Some(RunUsage {
            input_tokens: Some(tokens),
            output_tokens: Some(1),
            cache_read_tokens: Some(0),
            cache_creation_tokens: Some(0),
            cost_usd: Some(0.5),
            duplicate_reads: None,
            session_resumed: None,
            context_tokens: None,
        }),
        role,
        metrics: Some(RunMetrics {
            wall_ms: 100,
            retries: 0,
            peak_context_tokens: None,
            turns: None,
            knowledge: None,
        }),
        end: Some(RunEnd::Completed),
    }
}
