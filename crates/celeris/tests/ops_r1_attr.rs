//! ops-r1 B の trial 別消費（`celeris-campaign ops-r1-attribution`）: role ごとの行と合計・試行の外の run を
//! 入れないこと・未帰属の turn の reason・byte 一致・DB の写しを変えないこと（設計
//! docs/ops/ops-r1-trace-attribution.md §3・§4）。LLM は呼ばない。
#[path = "ops_r1/attr_fixture.rs"]
mod attr_fixture;
#[path = "ops_r1/trace_fixture.rs"]
mod trace_fixture;

use attr_fixture::*;
use celeris_campaign::ops_r1_attr;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use trace_fixture::*;

const CUTOFF: &str = "2026-10-29T11:00:00+09:00";

fn sha(path: &std::path::Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

fn attr(f: &AttrFixture, name: &str, cutoff: &str) -> (u8, Value, Vec<u8>) {
    let out = f.base.dir.path().join(name);
    let code = ops_r1_attr::cli(&f.args(&out, cutoff));
    let bytes = std::fs::read(&out).unwrap_or_default();
    let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (code, v, bytes)
}

fn row<'a>(v: &'a Value, run: &str) -> &'a Value {
    v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == run)
        .unwrap_or_else(|| panic!("row {run} missing"))
}

#[test]
fn ops_r1_attr_rows_and_totals_match_fixture() {
    let f = AttrFixture::new(Shape { report: true });
    let (code, v, _) = attr(&f, "attr.json", CUTOFF);
    assert_eq!(code, 0);
    assert_eq!(v["schema"], "ops-r1-attribution/2");
    assert_eq!(v["trial_id"], TRIAL);
    assert_eq!(v["condition_id"], CONDITION);
    assert_eq!(v["thread_id"], f.base.thread);
    assert_eq!(v["project_id"], f.base.project);
    assert_eq!(v["db"]["sha256"], sha(&f.base.copy));
    assert_eq!(
        v["window"],
        json!({"from": out_ts(10), "to": "2026-10-29T02:00:00.000Z"})
    );
    assert_eq!(v["root_task_ids"], json!([f.base.root.to_string()]));

    // 帰属行: planner・worker・reviewer の run、案件 CoS の 2 turn（受付・報告）、参照のある統括 CoS の turn。
    let attributed: Vec<(String, String)> = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["reason"].is_null())
        .map(|r| {
            assert_eq!(r["trial_id"], TRIAL, "{r}");
            assert_eq!(r["condition_id"], CONDITION, "{r}");
            (
                r["run_id"].as_str().unwrap().to_owned(),
                r["role"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let want: Vec<(String, String)> = [
        (COS_RUN, "cos_project"),
        ("run-planner-1", "planner"),
        ("run-worker-1", "worker"),
        (GENERAL_REF_RUN, "cos_general"),
        ("run-reviewer-1", "reviewer"),
        (REPORT_RUN, "cos_project"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(attributed, want);

    let worker = row(&v, "run-worker-1");
    assert_eq!(worker["task_id"], f.base.child.to_string());
    assert_eq!(worker["input_tokens"], 1000);
    assert_eq!(worker["output_tokens"], 200);
    assert_eq!(worker["cache_read_tokens"], 0);
    assert_eq!(worker["cache_write_tokens"], 0);
    assert_eq!(worker["wall_ms"], 50_000);
    assert!(worker["cost_usd"].is_null());
    assert!(worker["cost_basis"].is_null());
    let general = row(&v, GENERAL_REF_RUN);
    assert_eq!(general["thread_id"], f.general_thread);
    assert_eq!(general["cache_read_tokens"], 100);
    assert_eq!(general["cache_write_tokens"], 20);
    assert_eq!(general["cost_usd"], 0.01);
    assert_eq!(general["cost_basis"], "pricing_estimate");

    let t = &v["totals"];
    let keys: Vec<_> = t.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "cos_general",
            "cos_project",
            "planner",
            "reviewer",
            "worker"
        ]
    );
    for (role, wall) in [
        ("planner", 20_000),
        ("worker", 50_000),
        ("reviewer", 20_000),
    ] {
        assert_eq!(t[role]["rows"], 1, "{role}");
        assert_eq!(t[role]["input_tokens"], 1000, "{role}");
        assert_eq!(t[role]["output_tokens"], 200, "{role}");
        assert_eq!(t[role]["wall_ms"], wall, "{role}");
        assert_eq!(t[role]["cost_usd"], 0.0, "{role}");
        assert_eq!(t[role]["null_count"]["cost_usd"], 1, "{role}");
    }
    // 案件 CoS: 受付（11〜30s）と報告（162〜171s）の 2 turn、各 120/30/40/10・0.002。
    let p = &t["cos_project"];
    assert_eq!(p["rows"], 2);
    assert_eq!(
        (
            &p["input_tokens"],
            &p["output_tokens"],
            &p["cache_read_tokens"],
            &p["cache_write_tokens"]
        ),
        (&json!(240), &json!(60), &json!(80), &json!(20))
    );
    assert_eq!(p["wall_ms"], 28_000);
    assert!((p["cost_usd"].as_f64().unwrap() - 0.004).abs() < 1e-12);
    assert_eq!(p["null_count"]["cost_usd"], 0);
    let g = &t["cos_general"];
    assert_eq!(g["rows"], 1);
    assert_eq!(
        (
            &g["input_tokens"],
            &g["output_tokens"],
            &g["cache_read_tokens"],
            &g["cache_write_tokens"]
        ),
        (
            &json!(GENERAL_USAGE.0),
            &json!(GENERAL_USAGE.1),
            &json!(GENERAL_USAGE.2),
            &json!(GENERAL_USAGE.3)
        )
    );
    assert_eq!(g["wall_ms"], 10_000);

    // 帰属行と未帰属行の合計が候補行の全量（runs 4 行 + chat_runs 5 行）。
    let rows = v["rows"].as_array().unwrap().len() as u64;
    assert_eq!(rows, 9);
    let sum = |m: &Value| -> u64 {
        m.as_object()
            .unwrap()
            .values()
            .map(|s| s["rows"].as_u64().unwrap())
            .sum()
    };
    assert_eq!(sum(&v["totals"]) + sum(&v["unattributed"]), rows);
}

#[test]
fn ops_r1_attr_excludes_runs_outside_trial_tree() {
    let f = AttrFixture::new(Shape { report: true });
    let (_, v, _) = attr(&f, "attr.json", CUTOFF);
    let other = row(&v, OTHER_RUN);
    assert_eq!(other["task_id"], f.other_task.to_string());
    assert!(other["trial_id"].is_null());
    assert!(other["condition_id"].is_null());
    assert_eq!(other["reason"], "task_not_in_trial_tree");
    // role は帰属と別に元の値から決まるが、合計には入らない。
    assert_eq!(other["role"], "worker");
    assert_eq!(v["totals"]["worker"]["input_tokens"], 1000);
    assert_eq!(
        v["unattributed"]["task_not_in_trial_tree"]["input_tokens"],
        7777
    );
}

#[test]
fn ops_r1_attr_unattributable_turns_are_null_with_reason() {
    let f = AttrFixture::new(Shape { report: true });
    let (_, v, _) = attr(&f, "attr.json", CUTOFF);
    let noref = row(&v, GENERAL_NOREF_RUN);
    assert!(noref["trial_id"].is_null());
    assert!(noref["condition_id"].is_null());
    assert_eq!(noref["role"], "cos_general");
    assert_eq!(noref["reason"], "no_trial_reference");
    assert_eq!(noref["input_tokens"], 300);
    let early = row(&v, EARLY_RUN);
    assert!(early["trial_id"].is_null());
    assert_eq!(early["reason"], "before_intake");
    let u = &v["unattributed"];
    let keys: Vec<_> = u.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "before_intake",
            "no_trial_reference",
            "task_not_in_trial_tree"
        ]
    );
    assert_eq!(u["no_trial_reference"]["rows"], 1);
    assert_eq!(u["no_trial_reference"]["input_tokens"], 300);
    // 受付より前で打ち切ると試行が決まらず、候補行はすべて trial_roots_unknown になる。
    let (code, v, _) = attr(&f, "early.json", &ts(5));
    assert_eq!(code, 0);
    assert!(v["root_task_ids"].as_array().unwrap().is_empty());
    assert!(v["window"]["from"].is_null());
    assert!(v["totals"].as_object().unwrap().is_empty());
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "only the turn started before cutoff");
    assert_eq!(rows[0]["run_id"], EARLY_RUN);
    assert_eq!(rows[0]["reason"], "trial_roots_unknown");
}

#[test]
fn ops_r1_attr_is_byte_identical_and_leaves_db_copy_unchanged() {
    let f = AttrFixture::new(Shape { report: true });
    let before = sha(&f.base.copy);
    let (a, _, first) = attr(&f, "first.json", CUTOFF);
    let (b, _, second) = attr(&f, "second.json", CUTOFF);
    assert_eq!((a, b), (0, 0));
    assert!(!first.is_empty());
    assert_eq!(first, second);
    assert_eq!(first.last(), Some(&b'\n'));
    assert_eq!(sha(&f.base.copy), before);
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut p = f.base.copy.as_os_str().to_owned();
        p.push(suffix);
        assert!(!std::path::Path::new(&p).exists(), "{suffix} was created");
    }
}

#[test]
fn ops_r1_attr_rejects_bad_input_with_exit_2() {
    let f = AttrFixture::new(Shape { report: true });
    let out = f.base.dir.path().join("bad.json");
    let mut args = f.args(&out, CUTOFF);
    let i = args.iter().position(|a| a == "--condition-id").unwrap();
    args.truncate(i);
    assert_eq!(ops_r1_attr::cli(&args), 2);
    let mut args = f.args(&out, CUTOFF);
    let i = args.iter().position(|a| a == "--thread").unwrap();
    args[i + 1] = f.general_thread.clone();
    assert_eq!(ops_r1_attr::cli(&args), 2);
    assert!(!out.exists());
}

#[test]
fn ops_r1_pipeline_attribution_v2_copies_model_and_harness_read_only() {
    let f = AttrFixture::new(Shape { report: true });
    {
        let conn = rusqlite::Connection::open(&f.base.copy).unwrap();
        for (run, model, harness) in [
            ("run-planner-1", "claude-sonnet-5-5", "claude_code"),
            ("run-worker-1", "stub-model", "stub-harness"),
        ] {
            conn.execute(
                "UPDATE runs SET model=?1, adapter=?2 WHERE run_id=?3",
                rusqlite::params![model, harness, run],
            )
            .unwrap();
        }
        conn.execute(
            "UPDATE runs SET model=NULL, adapter=NULL WHERE run_id='run-reviewer-1'",
            [],
        )
        .unwrap();
    }
    let before = sha(&f.base.copy);
    let (code, v, first) = attr(&f, "v2-first.json", CUTOFF);
    assert_eq!(code, 0);
    assert_eq!(v["schema"], "ops-r1-attribution/2");
    assert_eq!(row(&v, "run-planner-1")["model"], "claude-sonnet-5-5");
    assert_eq!(row(&v, "run-planner-1")["harness"], "claude_code");
    assert_eq!(row(&v, "run-worker-1")["model"], "stub-model");
    assert_eq!(row(&v, "run-worker-1")["harness"], "stub-harness");
    for run in ["run-reviewer-1", COS_RUN] {
        assert!(row(&v, run)["model"].is_null());
        assert!(row(&v, run)["harness"].is_null());
    }
    assert_eq!(attr(&f, "v2-second.json", CUTOFF).2, first);
    assert_eq!(sha(&f.base.copy), before);
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(!std::path::PathBuf::from(format!("{}{suffix}", f.base.copy.display())).exists());
    }
}
