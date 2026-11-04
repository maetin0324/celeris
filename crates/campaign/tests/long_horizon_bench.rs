//! ADR 2026-11-04-long-horizon-bench（A8 実行）: 採点・集計・gate と、述語ごとの陰性 fixture（接頭辞 `long_horizon_bench_`）。
//!
//! raw は `LONG_HORIZON_BENCH_RAW_DIR`（`scripts/dev/test-long-horizon-bench.sh` が作ったばかりの dir）か、無ければ
//! commit 済みの `agent-docs/progress/2026-11-04-long-horizon/a8b/replay/`。`LONG_HORIZON_BENCH_SUMMARY_OUT` があれば
//! `grades.jsonl`・`summary.json` を書く。LLM を呼ばない。子 process を起こさない。

use celeris_campaign::long_horizon::{aggregate, aggregate_tier, cli, gate, grade};
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

/// 写しの raw（facts・arm-map・trials の repo）を作る。`pick` が真の試行だけ、`edit` で facts を書き換える。
fn copy_raw(
    tag: &str,
    pick: impl Fn(&Value) -> bool,
    mut edit: impl FnMut(&Value, &mut Value),
) -> PathBuf {
    let src = repo_root().join("agent-docs/progress/2026-11-04-long-horizon/a8b/replay");
    let tmp = std::env::temp_dir().join(format!("lhb-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    for sub in ["facts", "arm-map", "trials"] {
        std::fs::create_dir_all(tmp.join(sub)).unwrap();
    }
    let mut maps: Vec<PathBuf> = std::fs::read_dir(src.join("arm-map"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    maps.sort();
    for mp in maps {
        let m = read(&mp);
        if !pick(&m) {
            continue;
        }
        let t = m["trial"].as_str().unwrap();
        let mut f = read(&src.join("facts").join(format!("{t}.json")));
        edit(&m, &mut f);
        std::fs::write(tmp.join("facts").join(format!("{t}.json")), f.to_string()).unwrap();
        std::fs::copy(&mp, tmp.join("arm-map").join(format!("{t}.json"))).unwrap();
        copy_tree(
            &src.join("trials").join(t).join("repo"),
            &tmp.join("trials").join(t).join("repo"),
        );
    }
    tmp
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            copy_tree(&e.path(), &to.join(e.file_name()));
        } else {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

/// D2 の同一初期条件: 1 試行の `init_digest` を変えると、その課題の全試行が `invalid_init` になり gate が落ちる。
#[test]
fn long_horizon_bench_init_digest_mismatch_is_invalid_init() {
    let raw = copy_raw(
        "init",
        |_| true,
        |m, f| {
            if m["case"] == "C03" && m["arm"] == "old.passive" {
                f["init_digest"] = json!("0000");
            }
        },
    );
    let (rows, summary) = aggregate(&raw, &fixtures()).unwrap();
    assert_eq!(
        summary["invalid"]["invalid_init"], 4,
        "{}",
        summary["invalid"]
    );
    assert_eq!(summary["invalid"]["cases"], json!(["C03"]));
    assert!(
        rows.iter()
            .filter(|r| r["case"] == "C03")
            .all(|r| r["invalid"] == "invalid_init")
    );
    assert_eq!(summary["arms"]["new"]["trials"], 9);
    let errs = gate(&rows, &summary, &fixtures());
    assert!(
        errs.iter().any(|e| e.starts_with("invalid_init")),
        "{errs:?}"
    );
    // 欠けた digest も invalid_init。
    let raw2 = copy_raw(
        "init2",
        |m| m["case"] == "C01",
        |m, f| {
            if m["arm"] == "new" {
                f.as_object_mut().unwrap().remove("init_digest");
            }
        },
    );
    let (_, s2) = aggregate(&raw2, &fixtures()).unwrap();
    assert_eq!(s2["invalid"]["invalid_init"], 4);
    let _ = std::fs::remove_dir_all(&raw);
    let _ = std::fs::remove_dir_all(&raw2);
}

/// D5 の対照 `ctl.null`: commit 済みの raw で controller の root 0・指示なしの到達 0。値を変えると gate が落ちる。
#[test]
fn long_horizon_bench_ctl_null_control_reaches_nothing_by_itself() {
    let (rows, summary) = aggregate(
        &repo_root().join("agent-docs/progress/2026-11-04-long-horizon/a8b/replay"),
        &fixtures(),
    )
    .unwrap();
    let c = &summary["arms"]["ctl.null"];
    assert_eq!(c["trials"], 10);
    assert_eq!(c["daemon_roots"], 0);
    assert_eq!(c["reached_without_instruction"], 0);
    assert!(
        summary["arms"]["new"]["reached_without_instruction"]
            .as_i64()
            .unwrap()
            > 0
    );
    let mut bad = summary.clone();
    bad["arms"]["ctl.null"]["reached_without_instruction"] = json!(1);
    let errs = gate(&rows, &bad, &fixtures());
    assert!(
        errs.iter()
            .any(|e| e.contains("ctl.null: reached_without_instruction")),
        "{errs:?}"
    );
}

/// D1・D9: 実 LLM pilot の記録の経路。dataset `long_horizon_llm_pilot` の facts（ここでは stub: replay の facts の
/// dataset と usage を書き換えたもの。LLM 0）を `--tier llm_pilot` で採点・集計でき、usage は和になる。
/// replay と pilot は互いの tier で拒まれる。
#[test]
fn long_horizon_bench_llm_pilot_tier_is_recorded_separately() {
    let pilot_cases = ["C04", "C06", "C08", "C09", "C10"];
    let raw = copy_raw(
        "pilot",
        |m| {
            pilot_cases.contains(&m["case"].as_str().unwrap())
                && (m["arm"] == "new" || m["arm"] == "old.passive")
        },
        |_, f| {
            f["dataset"] = json!("long_horizon_llm_pilot");
            f["usage"] = json!({"tokens": 1000, "quota": 0.5, "wall_secs": 60, "missing": null});
        },
    );
    let (rows, summary) = aggregate_tier(&raw, &fixtures(), "llm_pilot").unwrap();
    assert_eq!(rows.len(), 10);
    assert_eq!(summary["dataset"], "long_horizon_llm_pilot");
    assert_eq!(summary["tier"], "llm_pilot");
    assert_eq!(summary["arms"]["new"]["tokens"], json!(5000.0));
    assert!(summary["arms"]["new"]["missing"].is_null());
    let err = aggregate_tier(&raw, &fixtures(), "replay").unwrap_err();
    assert!(err.contains("is not long_horizon_replay"), "{err}");
    // replay の raw を llm_pilot として集計しない。
    let replay = repo_root().join("agent-docs/progress/2026-11-04-long-horizon/a8b/replay");
    assert!(aggregate_tier(&replay, &fixtures(), "llm_pilot").is_err());
    // usage の欠けた pilot の試行があれば和は null（0 で埋めない）。
    let raw2 = copy_raw(
        "pilot2",
        |m| m["case"] == "C04" && m["arm"] == "new",
        |_, f| {
            f["dataset"] = json!("long_horizon_llm_pilot");
            f["usage"] = json!({"tokens": null, "quota": null, "wall_secs": null, "missing": "collector failed"});
        },
    );
    let (_, s2) = aggregate_tier(&raw2, &fixtures(), "llm_pilot").unwrap();
    assert!(s2["arms"]["new"]["tokens"].is_null());
    assert_eq!(s2["arms"]["new"]["missing"], "1 trials have no usage");
    // CLI の本体（`celeris-campaign long-horizon-grade`）が grades.jsonl・summary.json を書く。
    let out = raw.join("graded");
    let args: Vec<String> = [
        "long-horizon-grade",
        "--tier",
        "llm_pilot",
        "--raw",
        raw.to_str().unwrap(),
        "--fixtures",
        fixtures().to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    cli(&args).unwrap();
    assert_eq!(read(&out.join("summary.json"))["tier"], "llm_pilot");
    assert_eq!(
        std::fs::read_to_string(out.join("grades.jsonl"))
            .unwrap()
            .lines()
            .count(),
        10
    );
    let _ = std::fs::remove_dir_all(&raw);
    let _ = std::fs::remove_dir_all(&raw2);
}
