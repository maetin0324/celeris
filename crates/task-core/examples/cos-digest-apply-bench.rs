//! ADR 2026-11-03-cos-digest-persist: the store time of one digest pass of 20 runs on a file DB.
//!
//! `per_command` writes each command in its own transaction (the removed apply path: the message,
//! then the requeue); `one_transaction` is `SqliteStore::cos_digest_apply_pass`. Each rep digests
//! 20 fresh stopped runs; the variants alternate. Prints one JSON line per rep.
//!
//! `cargo run -p task-core --example cos-digest-apply-bench -- <dir> <reps>`

use task_core::SqliteStore;
use task_core::chat::digest::CosDigestCommand;
use task_core::chat::digest_apply::{CosDigestApplyPass, CosDigestApplyRun};
use task_core::chat::triage::{COS_TRIAGE_DIGEST_KEY_PREFIX, CosTriageSource};
use task_core::chat::{ChatOpenThreadRequest, ChatRunState};
use time::OffsetDateTime;

const RUNS: usize = 20;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().ok_or("usage: <dir> <reps>")?);
    let reps: usize = args.next().ok_or("usage: <dir> <reps>")?.parse()?;
    std::fs::create_dir_all(&dir)?;
    let store = SqliteStore::open(&dir.join("bench.db"))?;
    let now = OffsetDateTime::now_utc();
    let thread = store
        .cos_thread_open(&ChatOpenThreadRequest { project_id: None }, now)?
        .thread
        .id;
    for rep in 0..reps {
        for variant in ["per_command", "one_transaction"] {
            let mut runs = Vec::new();
            for i in 0..RUNS {
                let id = format!("{variant}-{rep}-{i}");
                store.cos_triage_ingest_batch(
                    "bench",
                    &id,
                    &[CosTriageSource {
                        source_kind: "notice".into(),
                        source_key: id.clone(),
                        source_revision: "1".into(),
                        source_event_id: None,
                        operation_id: None,
                        summary: format!("notice {id}"),
                        detail: String::new(),
                        policy_version: "1".into(),
                        task_id: None,
                    }],
                    now,
                )?;
                let claim = store
                    .cos_triage_claim(&thread, &id, now)?
                    .ok_or("no claim")?;
                store.chat_run_finish(&claim.run_id, ChatRunState::Stopped, None, None, now)?;
                runs.push(claim.run_id);
            }
            let commands = |run: &str| {
                vec![
                    CosDigestCommand::PostDigest {
                        thread: thread.clone(),
                        idempotency_key: format!("{COS_TRIAGE_DIGEST_KEY_PREFIX}{run}"),
                        text: format!("digest of {run}"),
                        cards: Vec::new(),
                        run_id: run.to_owned(),
                    },
                    CosDigestCommand::RequeueRun {
                        run_id: run.to_owned(),
                    },
                ]
            };
            let started = std::time::Instant::now();
            if variant == "per_command" {
                for run in &runs {
                    for command in commands(run) {
                        match command {
                            CosDigestCommand::PostDigest {
                                thread,
                                idempotency_key,
                                text,
                                cards,
                                run_id,
                            } => {
                                store.chat_assistant_message_add_once(
                                    &thread,
                                    &idempotency_key,
                                    &text,
                                    &cards,
                                    Some(&run_id),
                                    now,
                                )?;
                            }
                            CosDigestCommand::RequeueRun { run_id } => {
                                store.cos_triage_requeue_run(&run_id, now)?;
                            }
                        }
                    }
                }
            } else {
                let pass = CosDigestApplyPass {
                    pass_id: format!("bench-{rep}"),
                    verify: false,
                    expect_mode: None,
                    runs: runs
                        .iter()
                        .map(|run| CosDigestApplyRun {
                            thread_id: thread.clone(),
                            run_id: run.clone(),
                            commands: commands(run),
                            record: None,
                        })
                        .collect(),
                };
                store.cos_digest_apply_pass(&pass, now)?;
            }
            let secs = started.elapsed().as_secs_f64();
            println!(
                "{}",
                serde_json::json!({
                    "schema": "celeris.cos-digest-apply-bench/1",
                    "variant": variant,
                    "rep": rep,
                    "runs": RUNS,
                    "secs": secs,
                })
            );
        }
    }
    Ok(())
}
