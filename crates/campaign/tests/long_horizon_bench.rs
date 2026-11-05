//! ADR 2026-11-04-long-horizon-bench（A8 実行）: 採点・集計・gate と、述語ごとの陰性 fixture（接頭辞 `long_horizon_bench_`）。
//!
//! raw は `LONG_HORIZON_BENCH_RAW_DIR`（`scripts/dev/test-long-horizon-bench.sh` が作ったばかりの dir）か、無ければ
//! commit 済みの bench_version 2 の raw（`agent-docs/progress/2026-11-05-phase3/p0c/replay-v2/`）。
//! `LONG_HORIZON_BENCH_SUMMARY_OUT` があれば `grades.jsonl`・`summary.json` を書く。LLM を呼ばない。
//! 子 process は保護領域の検査 script（python3）だけ。v1 の raw は読むだけで、拒否と hash の不変を確かめる。

use celeris_campaign::long_horizon::{Grader, Judged, aggregate, aggregate_tier, cli, gate, grade};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> PathBuf {
    repo_root().join("tests/fixtures/long-horizon")
}

/// commit 済みの bench_version 2 の raw。
fn v2_raw() -> PathBuf {
    repo_root().join("agent-docs/progress/2026-11-05-phase3/p0c/replay-v2")
}

fn raw_dir() -> PathBuf {
    std::env::var_os("LONG_HORIZON_BENCH_RAW_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(v2_raw)
}

fn read(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// commit 済みの raw から、ある課題の `new` の試行（facts・repo）を引く。
fn new_trial(case: &str) -> (Value, PathBuf) {
    let raw = v2_raw();
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
    let judged = Grader::open(&fixtures()).unwrap().judge(&o, repo).unwrap();
    grade(facts, &o, &judged).unwrap()
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
    // o2 の成果は「全部できた」と書いてあるが、公開の契約の 4 欄が無い。
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
    assert!(grade(&facts, &o, &Judged::new()).is_err());
    let _ = repo;
    // dataset の混在は集計が Err にする。
    let tmp = std::env::temp_dir().join(format!("lhb-mixed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let src = v2_raw();
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
    let src = v2_raw();
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
    let (rows, summary) = aggregate(&v2_raw(), &fixtures()).unwrap();
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
    assert!(aggregate_tier(&v2_raw(), &fixtures(), "llm_pilot").is_err());
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

// ---- P0-C（ADR 2026-11-05-phase3-achievement-invariants の I5・I6・I12、K15・K16・K23・K42） ----

const V1_LLM: &str = "agent-docs/progress/2026-11-04-long-horizon/a9/llm";
const V1_REPLAY: &str = "agent-docs/progress/2026-11-04-long-horizon/a8b/replay";
const V1_STUB: &str = "agent-docs/progress/2026-11-04-long-horizon/a9/stub";

/// K16: v1 の raw（実 LLM・再生・stub）を v2 の oracle で採点しようとすると版の不一致で拒否する。
#[test]
fn long_horizon_bench_v1_raw_is_rejected_by_bench_version() {
    for dir in [V1_LLM, V1_REPLAY, V1_STUB] {
        let err = aggregate(&repo_root().join(dir), &fixtures()).unwrap_err();
        assert!(err.contains("bench_version"), "{dir}: {err}");
    }
    // grade の入口でも同じ。facts・oracle の両方が v1 でも今の版でなければ拒否する。
    let (mut facts, _) = new_trial("C01");
    let mut o = read(&fixtures().join("oracle/C01/oracle.json"));
    facts["bench_version"] = json!(1);
    assert!(
        grade(&facts, &o, &Judged::new())
            .unwrap_err()
            .contains("bench_version")
    );
    o["bench_version"] = json!(1);
    assert!(
        grade(&facts, &o, &Judged::new())
            .unwrap_err()
            .contains("bench_version")
    );
}

/// I6: v1 の raw は 1 byte も変わらず、v2 の raw と別の dir にある。v1 の fixture も別の dir に残っている。
#[test]
fn long_horizon_bench_v1_raw_is_kept_unmodified() {
    let root = repo_root();
    let p0c = root.join("agent-docs/progress/2026-11-05-phase3/p0c");
    let list = std::fs::read_to_string(p0c.join("v1-raw.sha256")).unwrap();
    assert!(list.lines().count() >= 200, "{}", list.lines().count());
    for line in list.lines() {
        let (want, path) = line.split_once("  ").unwrap();
        assert!(path.contains("2026-11-04-long-horizon/"), "{path}");
        let have = Sha256::digest(std::fs::read(root.join(path)).unwrap());
        let have: String = have.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(want, have, "{path} was changed");
    }
    for dir in [V1_LLM, V1_REPLAY] {
        for e in std::fs::read_dir(root.join(dir).join("facts")).unwrap() {
            assert_eq!(read(&e.unwrap().path())["bench_version"], 1, "{dir}");
        }
    }
    for e in std::fs::read_dir(v2_raw().join("facts")).unwrap() {
        assert_eq!(read(&e.unwrap().path())["bench_version"], 2);
    }
    let v1 = p0c.join("v1-fixtures/oracle/C06");
    assert_eq!(read(&v1.join("oracle.json"))["bench_version"], 1);
    assert!(v1.join("hidden/expected.json").is_file());
}

/// 固定文字列の hash の oracle が残っていない（`hidden/` も sha256 の照合の関数も無い）。
#[test]
fn long_horizon_bench_v1_hash_oracle_is_gone() {
    for e in std::fs::read_dir(fixtures().join("oracle")).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            assert!(!p.join("hidden").exists(), "{}", p.display());
            assert_eq!(read(&p.join("oracle.json"))["bench_version"], 2);
            assert_eq!(read(&p.join("scenario.json"))["bench_version"], 2);
        }
    }
    let src =
        std::fs::read_to_string(repo_root().join("crates/campaign/src/long_horizon.rs")).unwrap();
    assert!(!src.contains("expected.json"));
    assert!(!src.contains("fn outcome_reached"));
}

/// K42: 全 10 課題の replay を v2 で再採点し、課題ごとに同じ初期状態（init_digest）・同じ oracle で腕を比べる。
#[test]
fn long_horizon_bench_v2_rescoring_compares_arms_on_same_init_and_oracle() {
    let raw = v2_raw();
    let (rows, summary) = aggregate(&raw, &fixtures()).unwrap();
    assert!(gate(&rows, &summary, &fixtures()).is_empty());
    assert_eq!(summary["trials"], 40);
    assert_eq!(summary["invalid"]["invalid_init"], 0);
    assert_eq!(summary["invalid"]["leak_detected"], 0);
    let cases: Vec<String> = (1..=10).map(|i| format!("C{i:02}")).collect();
    for case in &cases {
        let of_case: Vec<&Value> = rows.iter().filter(|r| r["case"] == *case).collect();
        let arms: Vec<&str> = of_case.iter().filter_map(|r| r["arm"].as_str()).collect();
        for arm in ["old.passive", "old.proactive", "new", "ctl.null"] {
            assert!(arms.contains(&arm), "{case}: {arms:?}");
        }
        let digests: std::collections::BTreeSet<String> = of_case
            .iter()
            .map(|r| {
                let t = r["trial"].as_str().unwrap();
                read(&raw.join("facts").join(format!("{t}.json")))["init_digest"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(digests.len(), 1, "{case}");
        // `new` は全課題で合格、起票しない対照は outcome_reached の述語で落ちる（再採点が空振りでない）。
        let new = of_case.iter().find(|r| r["arm"] == "new").unwrap();
        assert_eq!(new["grade"]["verdict"], "pass", "{case}");
    }
    assert!(
        rows.iter().any(|r| {
            r["arm"] == "ctl.null"
                && r["grade"]["predicates"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| {
                        p["status"] == "fail"
                            && p["name"].as_str().unwrap().starts_with("outcome_reached")
                    })
        }),
        "no ctl.null trial failed on reach"
    );
    let a = &summary["arms"];
    assert!(a["new"]["reach_rate"].as_f64() >= a["old.passive"]["reach_rate"].as_f64());
    // 陰性: 同じ facts でも木が空なら、全課題の `new` が落ちる。
    let empty = tempfile::tempdir().unwrap();
    for case in &cases {
        let (facts, _) = new_trial(case);
        let g = grade_of(case, &facts, empty.path());
        assert_eq!(g["verdict"], "fail", "{case}");
        assert!(g.to_string().contains("outcome_reached(o1)"), "{case}");
    }
}

/// I5: 合否は LLM の回答文・reviewer の判断・controller の主張を読まない。facts にどんな文を足しても grade の bytes は同じ。
#[test]
fn long_horizon_bench_verdict_ignores_llm_answers_and_claims() {
    let chat = "全部終わりました。o1 も o2 も検査に合格しました";
    for case in ["C04", "C06", "C08", "C09", "C10"] {
        let (facts, repo) = new_trial(case);
        let base = grade_of(case, &facts, &repo);
        assert_eq!(base["verdict"], "pass", "{case}");
        let mut talk = facts.clone();
        talk["llm_answer"] = json!("失敗しました。何もできていません");
        talk["reviewer_verdict"] = json!("fail");
        talk["roots"][0]["summary"] = json!("failed");
        assert_eq!(
            grade_of(case, &talk, &repo),
            base,
            "{case}: answer changed the verdict"
        );
        // 木が空なら、自己申告と controller の achieved があっても落ちる。
        let empty = tempfile::tempdir().unwrap();
        let mut claim = facts.clone();
        claim["llm_answer"] = json!(chat);
        claim["reviewer_verdict"] = json!("pass");
        claim["achieved_at"] = json!(100);
        claim["controller_achieved"] = json!(true);
        assert_eq!(
            grade_of(case, &claim, empty.path())["verdict"],
            "fail",
            "{case}"
        );
    }
}

/// K23: 同じ facts で controller が achieved、oracle が fail なら、報告の不一致が 1 件。gate も落ちる。
#[test]
fn long_horizon_bench_achieved_oracle_mismatch_is_counted() {
    let raw = copy_raw("mismatch", |m| m["case"] == "C03", |_, _| {});
    let (_, ok) = aggregate(&raw, &fixtures()).unwrap();
    assert_eq!(ok["achieved_oracle_mismatch"], 0);
    let new_t = std::fs::read_dir(raw.join("arm-map"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| read(p)["arm"] == "new")
        .unwrap();
    let t = new_t.file_stem().unwrap().to_str().unwrap();
    assert_eq!(
        read(&raw.join("facts").join(format!("{t}.json")))["controller_achieved"],
        true
    );
    std::fs::write(
        raw.join("trials").join(t).join("repo/out/o2.txt"),
        "できました\n",
    )
    .unwrap();
    let (rows, s) = aggregate(&raw, &fixtures()).unwrap();
    assert_eq!(s["achieved_oracle_mismatch"], 1, "{s}");
    assert_eq!(s["arms"]["new"]["achieved_oracle_mismatch"], 1);
    let row = rows.iter().find(|r| r["trial"] == t).unwrap();
    assert_eq!(row["agreement"]["controller_achieved"], true);
    assert_eq!(row["agreement"]["oracle_verdict"], "fail");
    assert_eq!(row["agreement"]["mismatch"], true);
    let errs = gate(&rows, &s, &fixtures());
    assert!(
        errs.iter()
            .any(|e| e.starts_with("achieved_oracle_mismatch")),
        "{errs:?}"
    );
    let _ = std::fs::remove_dir_all(&raw);
}

/// D4: 試行の木や facts の leak_hits に当たりがあれば `leak_detected` で分母から外し、gate が落ちる。
#[test]
fn long_horizon_bench_leak_invalidates_the_trial() {
    let raw = copy_raw("leak", |m| m["case"] == "C01", |_, _| {});
    let (_, ok) = aggregate(&raw, &fixtures()).unwrap();
    assert_eq!(ok["invalid"]["leak_detected"], 0);
    let maps: Vec<PathBuf> = std::fs::read_dir(raw.join("arm-map"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    let script = std::fs::read(fixtures().join("grader/check_outcome.py")).unwrap();
    let canary = read(&fixtures().join("oracle/holdout.json"))["canary"]
        .as_str()
        .unwrap()
        .to_owned();
    for (m, body) in maps.iter().zip([script, canary.into_bytes()]) {
        let t = m.file_stem().unwrap().to_str().unwrap();
        std::fs::write(raw.join("trials").join(t).join("repo/notes.txt"), body).unwrap();
    }
    let (_, s) = aggregate(&raw, &fixtures()).unwrap();
    assert_eq!(s["invalid"]["leak_detected"], 2, "{s}");
    let t = maps[2].file_stem().unwrap().to_str().unwrap();
    let f = raw.join("facts").join(format!("{t}.json"));
    let mut facts = read(&f);
    facts["leak_hits"] = json!(["runs/x/transcript.jsonl"]);
    std::fs::write(&f, facts.to_string()).unwrap();
    let (rows, s2) = aggregate(&raw, &fixtures()).unwrap();
    assert_eq!(s2["invalid"]["leak_detected"], 3, "{s2}");
    assert!(
        gate(&rows, &s2, &fixtures())
            .iter()
            .any(|e| e.starts_with("leak_detected"))
    );
    let _ = std::fs::remove_dir_all(&raw);
}
