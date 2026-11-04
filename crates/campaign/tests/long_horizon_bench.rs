//! ADR 2026-11-04-long-horizon-bench（A8 実行）: 採点・集計・gate と、述語ごとの陰性 fixture（接頭辞 `long_horizon_bench_`）。
//!
//! raw は `LONG_HORIZON_BENCH_RAW_DIR`（`scripts/dev/test-long-horizon-bench.sh` が作ったばかりの dir）か、無ければ
//! commit 済みの `agent-docs/progress/2026-11-04-long-horizon/a8b/replay/`。`LONG_HORIZON_BENCH_SUMMARY_OUT` があれば
//! `grades.jsonl`・`summary.json` を書く。LLM を呼ばない。子 process を起こさない。

use celeris_campaign::long_horizon::{aggregate, gate, grade};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> PathBuf {
    repo_root().join("tests/fixtures/long-horizon")
}

fn raw_dir() -> PathBuf {
    std::env::var_os("LONG_HORIZON_BENCH_RAW_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            repo_root().join("agent-docs/progress/2026-11-04-long-horizon/a8b/replay")
        })
}

fn read(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// commit 済みの raw から、ある課題の `new` の試行（facts・repo）を引く。
fn new_trial(case: &str) -> (Value, PathBuf) {
    let raw = repo_root().join("agent-docs/progress/2026-11-04-long-horizon/a8b/replay");
    for e in std::fs::read_dir(raw.join("arm-map")).unwrap() {
        let m = read(&e.unwrap().path());
        if m["case"] == case && m["arm"] == "new" {
            let t = m["trial"].as_str().unwrap();
            return (
                read(&raw.join("facts").join(format!("{t}.json"))),
                raw.join("trials").join(t).join("repo"),
            );
        }
    }
    panic!("no new trial for {case}");
}

fn grade_of(case: &str, facts: &Value, repo: &Path) -> Value {
    let o = read(&fixtures().join("oracle").join(case).join("oracle.json"));
    let h: BTreeMap<String, String> = serde_json::from_value(read(
        &fixtures()
            .join("oracle")
            .join(case)
            .join("hidden/expected.json"),
    ))
    .unwrap();
    grade(facts, &o, &h, repo).unwrap()
}

/// 陰性 fixture: 事実の export を 1 か所変えると採点が落ち、名指しの述語が fail になる。
fn negative(case: &str, pred: &str, mutate: impl FnOnce(&mut Value)) {
    let (mut facts, repo) = new_trial(case);
    assert_eq!(
        grade_of(case, &facts, &repo)["verdict"],
        "pass",
        "{case}: the untouched trial must pass"
    );
    mutate(&mut facts);
    let g = grade_of(case, &facts, &repo);
    assert_eq!(g["verdict"], "fail", "{case}: {g}");
    let failed: Vec<&str> = g["predicates"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["status"] == "fail")
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert!(
        failed.iter().any(|n| n.starts_with(pred)),
        "{case}: expected {pred} to fail, got {failed:?}"
    );
}

#[test]
fn long_horizon_bench_grade_replay() {
    let raw = raw_dir();
    let (rows, summary) = aggregate(&raw, &fixtures()).unwrap();
    if let Some(out) = std::env::var_os("LONG_HORIZON_BENCH_SUMMARY_OUT") {
        let out = PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let lines: Vec<String> = rows.iter().map(|r| r.to_string()).collect();
        std::fs::write(out.join("grades.jsonl"), lines.join("\n") + "\n").unwrap();
        std::fs::write(
            out.join("summary.json"),
            serde_json::to_string_pretty(&summary).unwrap() + "\n",
        )
        .unwrap();
    }
    let errs = gate(&rows, &summary, &fixtures());
    assert!(errs.is_empty(), "D8 gate: {errs:#?}");
}

#[test]
fn long_horizon_bench_oracle_negative_extra_root() {
    negative("C08", "roots_for_outcome(o2)", |f| {
        let mut r = f["roots"][1].clone();
        r["logical_id"] = json!("o2:2");
        f["roots"].as_array_mut().unwrap().push(r);
    });
}

#[test]
fn long_horizon_bench_oracle_negative_double_effect() {
    negative("C07", "effect_count(C07:o1:external_job)", |f| {
        let e = f["effects"][0].clone();
        f["effects"].as_array_mut().unwrap().push(e);
    });
}

#[test]
fn long_horizon_bench_oracle_negative_effect_before_approval() {
    negative("C06", "effect_before", |f| {
        f["effects"][0]["at"] = json!(0);
    });
}

#[test]
fn long_horizon_bench_oracle_negative_avoidable_human_op() {
    negative("C02", "human_ops(avoidable)", |f| {
        f["human_ops"]
            .as_array_mut()
            .unwrap()
            .push(json!({"at": 100, "class": "avoidable", "kind": "nudge"}));
    });
}

#[test]
fn long_horizon_bench_oracle_negative_unreached_outcome() {
    let (facts, repo) = new_trial("C03");
    let tmp = std::env::temp_dir().join(format!("lhb-neg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("out")).unwrap();
    std::fs::copy(repo.join("out/o1.txt"), tmp.join("out/o1.txt")).unwrap();
    // o2 の成果は「全部できた」と書いてあるが、隠し受入の期待と違う。
    std::fs::write(tmp.join("out/o2.txt"), "all done\n").unwrap();
    let g = grade_of("C03", &facts, &tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    assert_eq!(g["verdict"], "fail");
    assert!(g.to_string().contains("outcome_reached(o2)"));
}

#[test]
fn long_horizon_bench_oracle_negative_repeated_question() {
    negative("C05", "question_key_opened", |f| {
        let q = f["questions"][0].clone();
        f["questions"].as_array_mut().unwrap().push(q);
    });
}

#[test]
fn long_horizon_bench_oracle_negative_cos_while_waiting() {
    negative("C03", "cos_invocations", |f| {
        for at in [7200, 10800] {
            f["invocations"]
                .as_array_mut()
                .unwrap()
                .push(json!({"role": "cos", "at": at, "usage": null}));
        }
    });
}

#[test]
fn long_horizon_bench_oracle_negative_consult_creates_task() {
    negative("C09", "tasks_created(statement 1)", |f| {
        f["statements"][0]["tasks_created_by_cos"] = json!(1);
    });
}

#[test]
fn long_horizon_bench_oracle_negative_root_after_achieved() {
    negative("C08", "roots_after_achieved", |f| {
        let a = f["achieved_at"].as_i64().unwrap();
        f["roots"][3]["created_at"] = json!(a + 60);
    });
}

#[test]
fn long_horizon_bench_oracle_negative_terminal_root_changes() {
    negative("C04", "root_status_stays", |f| {
        f["roots"][0]["statuses"]
            .as_array_mut()
            .unwrap()
            .push(json!("running"));
    });
}

#[test]
fn long_horizon_bench_oracle_negative_ownerless_running() {
    negative("C01", "no_ownerless_running", |f| {
        f["running_untracked"] = json!(1)
    });
}

#[test]
fn long_horizon_bench_oracle_negative_events_rewritten() {
    negative("C01", "events_append_only", |f| {
        f["events_append_only"] = json!(false)
    });
}

#[test]
fn long_horizon_bench_rejects_arm_field_and_mixed_dataset() {
    let (mut facts, repo) = new_trial("C01");
    facts["arm"] = json!("new");
    let o = read(&fixtures().join("oracle/C01/oracle.json"));
    assert!(grade(&facts, &o, &BTreeMap::new(), &repo).is_err());
    // dataset の混在は集計が Err にする。
    let tmp = std::env::temp_dir().join(format!("lhb-mixed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let src = repo_root().join("agent-docs/progress/2026-11-04-long-horizon/a8b/replay");
    std::fs::create_dir_all(tmp.join("facts")).unwrap();
    let mut n = 0;
    for e in std::fs::read_dir(src.join("facts")).unwrap().take(2) {
        let p = e.unwrap().path();
        let mut v = read(&p);
        if n == 1 {
            v["dataset"] = json!("long_horizon_llm_pilot");
        }
        std::fs::write(
            tmp.join("facts").join(p.file_name().unwrap()),
            v.to_string(),
        )
        .unwrap();
        n += 1;
    }
    let err = aggregate(&tmp, &fixtures()).unwrap_err();
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(err.contains("mixed datasets"), "{err}");
}
