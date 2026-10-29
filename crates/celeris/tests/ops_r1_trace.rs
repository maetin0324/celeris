//! ops-r1 B の工程 trace（`celeris-campaign ops-r1-trace`）: 6 段・欠けた段の reason・byte 一致・DB の写しを
//! 変えないこと（設計 docs/ops/ops-r1-trace-attribution.md §2・§4）。LLM は呼ばない。
#[path = "ops_r1/trace_fixture.rs"]
mod trace_fixture;

use celeris_campaign::ops_r1_trace;
use serde_json::Value;
use sha2::{Digest, Sha256};
use trace_fixture::*;

const CUTOFF: &str = "2026-10-29T11:00:00+09:00";

fn sha(path: &std::path::Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

fn trace(f: &Fixture, name: &str, cutoff: &str) -> (u8, Value, Vec<u8>) {
    let out = f.dir.path().join(name);
    let code = ops_r1_trace::cli(&f.args(&out, cutoff));
    let bytes = std::fs::read(&out).unwrap_or_default();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (code, v, bytes)
}

fn stage<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["stage"] == name)
        .unwrap()
}

#[test]
fn ops_r1_trace_emits_all_six_stages() {
    let f = Fixture::new(Shape { report: true });
    let (code, v, _) = trace(&f, "trace.json", CUTOFF);
    assert_eq!(code, 0);
    assert_eq!(v["schema"], "ops-r1-trace/1");
    assert_eq!(v["trial_id"], TRIAL);
    assert_eq!(v["thread_id"], f.thread);
    assert_eq!(v["project_id"], f.project);
    assert_eq!(v["db"]["schema_version"], 102);
    assert_eq!(v["db"]["sha256"], sha(&f.copy));
    assert_eq!(v["cutoff"], "2026-10-29T02:00:00.000Z");
    let names: Vec<_> = v["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["stage"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "intake", "created", "executed", "outcome", "report", "cutoff"
        ]
    );
    for s in v["stages"].as_array().unwrap() {
        assert!(s["reason"].is_null(), "{s}");
        assert!(s["at"].is_string(), "{s}");
    }
    let intake = stage(&v, "intake");
    assert_eq!(intake["at"], out_ts(10));
    assert_eq!(intake["ref"]["message_id"], f.intake_message);
    assert_eq!(intake["ref"]["author"], "human:tester");
    assert_eq!(intake["ref"]["cos_run_id"], COS_RUN);
    let created = stage(&v, "created");
    assert_eq!(created["at"], out_ts(20));
    assert_eq!(created["ref"]["operation_id"], f.operation_id);
    assert_eq!(created["ref"]["cos_run_id"], COS_RUN);
    assert_eq!(created["ref"]["root_task_id"], f.root.to_string());
    assert_eq!(created["ref"]["created_by"], format!("cos:{}", f.thread));
    assert!(created["ref"]["audit_event_id"].is_i64());
    assert_eq!(created["ref"]["extra_root_task_ids"], serde_json::json!([]));
    let executed = stage(&v, "executed");
    assert_eq!(executed["at"], out_ts(40));
    assert_eq!(executed["ref"]["until"], out_ts(150));
    let mut tasks = vec![f.root.to_string(), f.child.to_string()];
    tasks.sort();
    assert_eq!(executed["ref"]["task_ids"], serde_json::json!(tasks));
    let roles: Vec<_> = executed["ref"]["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["planner", "worker", "reviewer"]);
    let outcome = stage(&v, "outcome");
    assert_eq!(outcome["at"], out_ts(160));
    assert_eq!(outcome["ref"]["to"], "done");
    assert_eq!(
        outcome["ref"]["reviewer_passed"].as_array().unwrap().len(),
        1
    );
    assert_eq!(outcome["ref"]["review_verdicts"], 1);
    let report = stage(&v, "report");
    assert_eq!(report["at"], out_ts(170));
    assert_eq!(report["ref"]["chat_run_id"], REPORT_RUN);
    assert_eq!(report["ref"]["input_message_id"], f.report_input);
    assert_eq!(
        report["ref"]["message_id"],
        *f.report_message.as_ref().unwrap()
    );
    assert_eq!(report["ref"]["root_done_state"], "notified");
    let cutoff = stage(&v, "cutoff");
    assert_eq!(cutoff["ref"]["source"], "argument");
    assert_eq!(cutoff["ref"]["report_before_cutoff"], true);
}

#[test]
fn ops_r1_trace_missing_report_is_null_with_reason() {
    let f = Fixture::new(Shape { report: false });
    let (code, v, _) = trace(&f, "trace.json", CUTOFF);
    assert_eq!(code, 0);
    let report = stage(&v, "report");
    assert!(report["at"].is_null());
    assert!(report["ref"].is_null());
    assert_eq!(report["reason"], "report_run_not_completed");
    assert!(stage(&v, "cutoff")["ref"]["report_before_cutoff"].is_null());
    // 他の段は取れたまま。
    assert!(stage(&v, "outcome")["reason"].is_null());
}

#[test]
fn ops_r1_trace_cutoff_hides_later_rows() {
    let f = Fixture::new(Shape { report: true });
    // 報告（+170s）と done（+160s）より前で打ち切る。
    let (code, v, _) = trace(&f, "trace.json", &ts(155));
    assert_eq!(code, 0);
    assert_eq!(stage(&v, "outcome")["reason"], "not_terminal_by_cutoff");
    assert_eq!(stage(&v, "report")["reason"], "upstream_missing:outcome");
    // 受付より前で打ち切ると、後の段は上流の欠けになる。
    let (_, v, _) = trace(&f, "early.json", &ts(5));
    assert_eq!(stage(&v, "intake")["reason"], "intake_after_cutoff");
    assert_eq!(stage(&v, "created")["reason"], "upstream_missing:intake");
    assert_eq!(stage(&v, "executed")["reason"], "upstream_missing:created");
}

#[test]
fn ops_r1_trace_is_byte_identical_and_leaves_db_copy_unchanged() {
    let f = Fixture::new(Shape { report: true });
    let before = sha(&f.copy);
    let (a, _, first) = trace(&f, "first.json", CUTOFF);
    let (b, _, second) = trace(&f, "second.json", CUTOFF);
    assert_eq!((a, b), (0, 0));
    assert!(!first.is_empty());
    assert_eq!(first, second);
    assert_eq!(first.last(), Some(&b'\n'));
    assert_eq!(sha(&f.copy), before);
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut p = f.copy.as_os_str().to_owned();
        p.push(suffix);
        assert!(!std::path::Path::new(&p).exists(), "{suffix} was created");
    }
}

#[test]
fn ops_r1_trace_rejects_bad_input_with_exit_2() {
    let f = Fixture::new(Shape { report: true });
    let out = f.dir.path().join("bad.json");
    let mut args = f.args(&out, CUTOFF);
    let i = args.iter().position(|a| a == "--thread").unwrap();
    args[i + 1] = "no-such-thread".into();
    assert_eq!(ops_r1_trace::cli(&args), 2);
    assert_eq!(ops_r1_trace::cli(&f.args(&out, "not-a-time")), 2);
    // -wal のある DB は写しの bytes が決まらないので拒否する。
    let mut wal = f.copy.as_os_str().to_owned();
    wal.push("-wal");
    std::fs::write(&wal, b"").unwrap();
    assert_eq!(ops_r1_trace::cli(&f.args(&out, CUTOFF)), 2);
    assert!(!out.exists());
}
