//! Verification of formal A2 raw; legacy pre-save faults cannot supply this proof.
use super::{
    ext::read_ledger,
    process::{B_CONTROL_ACKED, B_CONTROL_RESUME, read_events, verify_survival_barriers},
};
use serde_json::Value;
use std::path::Path;

pub fn verify(raw: &Path) -> Result<(), String> {
    // 実 scheduler raw（壁時計）を論理時計の durable raw として読まない。
    if let Ok(run) = std::fs::read_to_string(raw.join("run.json"))
        && super::metrics::raw_schema_of(&run).as_deref()
            == Ok(super::pbs::raw::SCHEDULER_RAW_SCHEMA)
    {
        return Err(format!(
            "{} is not a durable raw; durable::verify does not read it",
            super::pbs::raw::SCHEDULER_RAW_SCHEMA
        ));
    }
    let events = read_events(&raw.join("process-events.jsonl")).map_err(|e| e.to_string())?;
    let proof = verify_survival_barriers(&events)?;
    if proof.control_pid == proof.external_pid {
        return Err("external and controller must be separate processes".into());
    }
    let ack = events
        .iter()
        .find(|e| e["name"] == B_CONTROL_ACKED)
        .ok_or("no durable ack")?;
    let resume = events
        .iter()
        .find(|e| e["name"] == B_CONTROL_RESUME)
        .ok_or("no resume")?;
    let snapshot: Value = serde_json::from_slice(
        &std::fs::read(raw.join("durable-wait.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let physical = ack["physical_id"].as_str().ok_or("no physical ID")?;
    let logical = ack["logical_job_id"].as_str().ok_or("no logical ID")?;
    if ack["trigger"] != "durable_wait_saved"
        || snapshot != ack["saved"]
        || snapshot != resume["saved"]
        || snapshot["job_id"] != physical
        || snapshot["wait_id"].as_str().is_none_or(str::is_empty)
        || resume["physical_id"] != physical
        || resume["logical_job_id"] != logical
    {
        return Err("durable wait / resume identity mismatch".into());
    }
    let state = &snapshot["state"];
    let valid = match snapshot["source"].as_str() {
        Some("state.json") => state["status"] == "waiting" && state["physical_id"] == physical,
        Some("TaskStore") => {
            state["state"] == "waiting"
                && state["wait_id"] == snapshot["wait_id"]
                && state["jobs"].as_array().is_some_and(|jobs| {
                    jobs.iter()
                        .any(|j| j == physical.trim_start_matches("phys-"))
                })
        }
        _ => false,
    };
    if !valid {
        return Err("snapshot does not contain a saved Waiting with job ID".into());
    }
    let ledger = read_ledger(&raw.join("ext.jsonl")).map_err(|e| e.to_string())?;
    let matching: Vec<_> = ledger
        .iter()
        .filter(|e| e.logical_job_id.as_deref() == Some(logical))
        .collect();
    let before = ack["ledger_seq_before"]
        .as_u64()
        .ok_or("missing pre-stop ledger sequence")?;
    let sync = events
        .iter()
        .find(|e| e["name"] == super::process::B_LEDGER_SYNC_ACK)
        .ok_or("missing ledger sync")?;
    let synced = sync["ledger_seq"]
        .as_u64()
        .ok_or("missing synced sequence")?;
    let mut previous = 0;
    for kind in ["physical_launch", "external_job_completed", "consumed"] {
        let rows: Vec<_> = matching.iter().filter(|e| e.event == kind).collect();
        if rows.len() != 1 || rows[0].physical_id.as_deref() != Some(physical) {
            return Err(format!(
                "{kind}: same job must be launched, completed and consumed once"
            ));
        }
        if rows[0].seq <= previous {
            return Err("launch/completion/consumption order violated".into());
        }
        previous = rows[0].seq;
        if kind == "physical_launch" && rows[0].seq > before {
            return Err("job was not launched before stop".into());
        }
        if kind == "consumed" && rows[0].seq <= synced {
            return Err("job consumed before controller resumed".into());
        }
        if kind == "external_job_completed" && rows[0].seq > synced {
            return Err("job completion not covered by outage sync ack".into());
        }
        if kind == "external_job_completed" && rows[0].seq <= before {
            return Err("job completed before the controller outage".into());
        }
    }
    Ok(())
}
