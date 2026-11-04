//! ADR 2026-11-04-long-horizon-bench（A8 実行）: long-horizon ベンチの採点・指標・集計。
//!
//! 入力は試行ごとの事実の export（`celeris.long-horizon-facts/1`。腕の欄を持たない）・oracle
//! （`celeris.long-horizon-oracle/1`）・隠し受入の期待値（`hidden/expected.json`。Outcome ごとの成果の sha256）・
//! 試行の作業 tree の最終の木だけ。採点は腕を知らない。腕との対応（`arm-map/`）は集計の最後に結合する。
//! LLM を使わない。子 process を起こさない。同じ入力から同じ bytes を出す。

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

pub const FACTS_SCHEMA: &str = "celeris.long-horizon-facts/1";
pub const ORACLE_SCHEMA: &str = "celeris.long-horizon-oracle/1";
pub const GRADE_SCHEMA: &str = "celeris.long-horizon-grade/1";
pub const SUMMARY_SCHEMA: &str = "celeris.long-horizon-summary/1";
pub const REPLAY_DATASET: &str = "long_horizon_replay";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn u(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// 隠し受入: 作業 tree の `out/<id>.txt` の sha256 が期待値と一致する（controller の主張を使わない）。
pub fn outcome_reached(repo: &Path, hidden: &BTreeMap<String, String>, id: &str) -> bool {
    let Some(want) = hidden.get(id) else {
        return false;
    };
    std::fs::read(repo.join("out").join(format!("{id}.txt")))
        .map(|b| hex(&Sha256::digest(&b)) == *want)
        .unwrap_or(false)
}

fn roots_of<'a>(facts: &'a Value, outcome: &str) -> Vec<&'a Value> {
    arr(&facts["roots"])
        .iter()
        .filter(|r| r["outcome"].as_str() == Some(outcome))
        .collect()
}

fn human(facts: &Value, class: &str) -> i64 {
    arr(&facts["human_ops"])
        .iter()
        .filter(|h| h["class"].as_str() == Some(class))
        .count() as i64
}

/// 1 試行を採点する。述語の連言。
pub fn grade(
    facts: &Value,
    oracle: &Value,
    hidden: &BTreeMap<String, String>,
    repo: &Path,
) -> Result<Value, String> {
    if facts["schema"] != FACTS_SCHEMA {
        return Err(format!("facts schema {}", facts["schema"]));
    }
    if oracle["schema"] != ORACLE_SCHEMA {
        return Err(format!("oracle schema {}", oracle["schema"]));
    }
    if facts.get("arm").is_some() {
        return Err("facts must not carry the arm (D6)".into());
    }
    if facts["case"] != oracle["case"] || facts["bench_version"] != oracle["bench_version"] {
        return Err("facts and oracle disagree on case or bench_version".into());
    }
    let mut preds: Vec<Value> = Vec::new();
    let mut check = |name: String, ok: bool, reason: String| {
        preds.push(json!({"name": name, "status": if ok {"pass"} else {"fail"}, "reason": reason}));
    };
    for o in arr(&oracle["required_outcomes"]) {
        let id = o.as_str().unwrap_or_default();
        check(
            format!("outcome_reached({id})"),
            outcome_reached(repo, hidden, id),
            "hidden acceptance on the final tree".into(),
        );
    }
    if let Some(m) = oracle["max_roots"].as_object() {
        for (id, n) in m {
            let got = roots_of(facts, id).len() as i64;
            check(
                format!("roots_for_outcome({id}) <= {n}"),
                got <= u(n),
                format!("{got} roots"),
            );
        }
    }
    let unmapped = arr(&facts["roots"])
        .iter()
        .filter(|r| r["outcome"].is_null())
        .count();
    check(
        "unmapped_roots == 0".into(),
        unmapped == 0,
        format!("{unmapped}"),
    );
    if let Some(m) = oracle["effect_count"].as_object() {
        for (id, n) in m {
            let got = arr(&facts["effects"])
                .iter()
                .filter(|e| e["logical_id"].as_str() == Some(id))
                .count() as i64;
            check(
                format!("effect_count({id}) == {n}"),
                got == u(n),
                format!("{got}"),
            );
        }
    }
    if oracle["effects_after_approval"] == true {
        let first = arr(&facts["human_ops"])
            .iter()
            .filter(|h| h["kind"] == "approval")
            .map(|h| u(&h["at"]))
            .min();
        let early = arr(&facts["effects"])
            .iter()
            .filter(|e| first.is_none_or(|a| u(&e["at"]) < a))
            .count();
        check(
            "effect_before(approval, effect)".into(),
            early == 0,
            format!("{early} effects before the approval"),
        );
    }
    if let Some(m) = oracle["human_ops_max"].as_object() {
        for (class, n) in m {
            let got = human(facts, class);
            check(
                format!("human_ops({class}) <= {n}"),
                got <= u(n),
                format!("{got}"),
            );
        }
    }
    for w in arr(&oracle["cos_invocations_max"]) {
        let (from, to, max) = (u(&w["from"]), u(&w["to"]), u(&w["max"]));
        let got = arr(&facts["invocations"])
            .iter()
            .filter(|i| i["role"] == "cos" && (from..=to).contains(&u(&i["at"])))
            .count() as i64;
        check(
            format!("cos_invocations([{from},{to}]) <= {max}"),
            got <= max,
            format!("{got}"),
        );
    }
    if let Some(m) = oracle["statements"].as_object() {
        for (idx, n) in m {
            let got = arr(&facts["statements"])
                .iter()
                .find(|s| s["index"].as_i64() == idx.parse::<i64>().ok())
                .map(|s| u(&s["tasks_created_by_cos"]));
            check(
                format!("tasks_created(statement {idx}) == {n}"),
                got == Some(u(n)),
                format!("{got:?}"),
            );
        }
    }
    let qmax = oracle["question_key_max"].as_i64().unwrap_or(1);
    let mut keys: BTreeMap<&str, i64> = BTreeMap::new();
    for q in arr(&facts["questions"]) {
        *keys.entry(q.as_str().unwrap_or_default()).or_default() += 1;
    }
    let worst = keys.values().copied().max().unwrap_or(0);
    check(
        format!("question_key_opened <= {qmax}"),
        worst <= qmax,
        format!("{worst}"),
    );
    let achieved = facts["achieved_at"].as_i64();
    if oracle["no_roots_after_achieved"] == true {
        let after = arr(&facts["roots"])
            .iter()
            .filter(|r| achieved.is_some_and(|a| u(&r["created_at"]) > a))
            .count();
        check(
            "roots_after_achieved == 0".into(),
            after == 0,
            format!("{after}"),
        );
    }
    if oracle["no_cos_after_achieved"] == true {
        let after = arr(&facts["invocations"])
            .iter()
            .filter(|i| i["role"] == "cos" && achieved.is_some_and(|a| u(&i["at"]) > a))
            .count();
        check(
            "cos_invocations_after_achieved == 0".into(),
            after == 0,
            format!("{after}"),
        );
    }
    // 終端の root は後から変わらない（D3）。
    let reopened = arr(&facts["roots"])
        .iter()
        .filter(|r| {
            let s: Vec<&str> = arr(&r["statuses"])
                .iter()
                .filter_map(Value::as_str)
                .collect();
            s.iter()
                .position(|x| matches!(*x, "done" | "failed" | "cancelled"))
                .is_some_and(|i| s[i + 1..].iter().any(|x| *x != s[i]))
        })
        .count();
    check(
        "root_status_stays".into(),
        reopened == 0,
        format!("{reopened}"),
    );
    check(
        "no_ownerless_running".into(),
        u(&facts["running_untracked"]) == 0,
        format!("{}", facts["running_untracked"]),
    );
    check(
        "events_append_only".into(),
        facts["events_append_only"] == true,
        format!("{}", facts["events_append_only"]),
    );
    let verdict = if preds.iter().all(|p| p["status"] == "pass") {
        "pass"
    } else {
        "fail"
    };
    Ok(
        json!({"schema": GRADE_SCHEMA, "case": facts["case"], "verdict": verdict, "predicates": preds}),
    )
}

/// 1 試行の指標（D7）。replay の token・quota・実 walltime は null（`missing: tier=replay`）。
pub fn metrics(
    facts: &Value,
    oracle: &Value,
    hidden: &BTreeMap<String, String>,
    repo: &Path,
) -> Value {
    let required: Vec<&str> = arr(&oracle["required_outcomes"])
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let reached = required
        .iter()
        .filter(|id| outcome_reached(repo, hidden, id))
        .count();
    let case_reached = reached == required.len();
    let avoidable = human(facts, "avoidable");
    // 無駄起票: oracle の許す数を超えた root・対応の無い root・達成後の root。
    let mut wasteful = arr(&facts["roots"])
        .iter()
        .filter(|r| r["outcome"].is_null())
        .count() as i64;
    if let Some(m) = oracle["max_roots"].as_object() {
        for (id, n) in m {
            wasteful += (roots_of(facts, id).len() as i64 - u(n)).max(0);
        }
    }
    let mut by_logical: BTreeMap<&str, i64> = BTreeMap::new();
    for e in arr(&facts["effects"]) {
        *by_logical
            .entry(e["logical_id"].as_str().unwrap_or_default())
            .or_default() += 1;
    }
    let double_effect = by_logical.values().filter(|n| **n >= 2).count();
    // 回復: 発火から最後までに必須の Outcome に到達し、発火の後に avoidable が増えない。
    let faults = arr(&facts["faults"]);
    let recovered = faults
        .iter()
        .filter(|f| {
            let at = u(&f["at"]);
            case_reached
                && !arr(&facts["human_ops"])
                    .iter()
                    .any(|h| h["class"] == "avoidable" && u(&h["at"]) >= at)
        })
        .count();
    let actors: Vec<&str> = arr(&facts["actors"])
        .iter()
        .filter_map(|a| a["actor"].as_str())
        .collect();
    let handoffs = actors.windows(2).filter(|w| w[0] != w[1]).count();
    let mut inv: BTreeMap<String, i64> = BTreeMap::new();
    for i in arr(&facts["invocations"]) {
        *inv.entry(i["role"].as_str().unwrap_or("?").to_owned())
            .or_default() += 1;
    }
    json!({
        "case": facts["case"],
        "dataset": facts["dataset"],
        "required_outcomes": required.len(),
        "reached_outcomes": reached,
        "case_reached": case_reached,
        "autonomous_reach": case_reached && avoidable == 0,
        "human_intervention": {
            "stimulus": human(facts, "stimulus"),
            "required": human(facts, "required"),
            "avoidable": avoidable,
        },
        "wasteful_roots": wasteful,
        "double_effect": double_effect,
        "fired_faults": faults.len(),
        "recovered_faults": recovered,
        "handoffs": handoffs,
        "llm_invocations": inv,
        "virtual_secs_to_reach": facts["virtual_secs_to_reach"],
        "tokens": facts["usage"]["tokens"],
        "quota": facts["usage"]["quota"],
        "wall_secs": facts["usage"]["wall_secs"],
        "missing": facts["usage"]["missing"],
    })
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn sorted_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    v.sort();
    Ok(v)
}

/// raw の dir（`facts/`・`arm-map/`・`trials/`）を採点し、腕ごとに集計する。`dataset` の混在は Err。
/// 戻り値は（試行ごとの grade の行, 集計）。
pub fn aggregate(raw: &Path, fixtures: &Path) -> Result<(Vec<Value>, Value), String> {
    let mut rows = Vec::new();
    let mut arms: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut dataset: Option<String> = None;
    let files = sorted_files(&raw.join("facts"))?;
    for f in &files {
        let ds = read_json(f)?["dataset"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        match &dataset {
            None => dataset = Some(ds),
            Some(d) if *d != ds => return Err(format!("mixed datasets: {d} and {ds}")),
            _ => {}
        }
    }
    for f in files {
        let trial = f
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        let facts = read_json(&f)?;
        let case = facts["case"].as_str().unwrap_or_default().to_owned();
        let oracle = read_json(&fixtures.join("oracle").join(&case).join("oracle.json"))?;
        let hidden_v = read_json(
            &fixtures
                .join("oracle")
                .join(&case)
                .join("hidden/expected.json"),
        )?;
        let hidden: BTreeMap<String, String> =
            serde_json::from_value(hidden_v).map_err(|e| e.to_string())?;
        let repo = raw.join("trials").join(&trial).join("repo");
        let g = grade(&facts, &oracle, &hidden, &repo)?;
        let m = metrics(&facts, &oracle, &hidden, &repo);
        // 腕の結合は採点の後（D6）。
        let map = read_json(&raw.join("arm-map").join(format!("{trial}.json")))?;
        let arm = map["arm"].as_str().unwrap_or_default().to_owned();
        let row = json!({"trial": trial, "case": case, "arm": arm, "grade": g, "metrics": m});
        arms.entry(arm).or_default().push(row.clone());
        rows.push(row);
    }
    let mut per_arm = serde_json::Map::new();
    for (arm, rs) in &arms {
        let sum = |k: &str| -> i64 { rs.iter().map(|r| u(&r["metrics"][k])).sum() };
        let req: i64 = sum("required_outcomes");
        let reached: i64 = sum("reached_outcomes");
        let n = rs.len() as f64;
        let cases_reached = rs
            .iter()
            .filter(|r| r["metrics"]["case_reached"] == true)
            .count();
        let auto = rs
            .iter()
            .filter(|r| r["metrics"]["autonomous_reach"] == true)
            .count();
        let fired = sum("fired_faults");
        let rec = sum("recovered_faults");
        let hum = |c: &str| -> i64 {
            rs.iter()
                .map(|r| u(&r["metrics"]["human_intervention"][c]))
                .sum()
        };
        let excl: Vec<&Value> = rs.iter().filter(|r| r["case"] != "C09").collect();
        let reach_excl = {
            let r: i64 = excl
                .iter()
                .map(|r| u(&r["metrics"]["reached_outcomes"]))
                .sum();
            let q: i64 = excl
                .iter()
                .map(|r| u(&r["metrics"]["required_outcomes"]))
                .sum();
            if q > 0 { r as f64 / q as f64 } else { 0.0 }
        };
        per_arm.insert(
            arm.clone(),
            json!({
                "trials": rs.len(),
                "verdict_pass": rs.iter().filter(|r| r["grade"]["verdict"] == "pass").count(),
                "reach_rate": if req > 0 { reached as f64 / req as f64 } else { 0.0 },
                "reach_rate_excluding_c09": reach_excl,
                "case_reach_rate": cases_reached as f64 / n,
                "autonomous_reach": auto as f64 / n,
                "human_intervention": {"stimulus": hum("stimulus"), "required": hum("required"), "avoidable": hum("avoidable")},
                "wasteful_roots": sum("wasteful_roots"),
                "double_effect": sum("double_effect"),
                "fired_faults": fired,
                "recovered_faults": rec,
                "recovery_rate": if fired > 0 { Some(rec as f64 / fired as f64) } else { None },
                "handoffs": sum("handoffs"),
                "tokens": null, "quota": null, "wall_secs": null,
                "missing": "tier=replay（実 LLM を使っていないので未計測）",
            }),
        );
    }
    let summary = json!({
        "schema": SUMMARY_SCHEMA,
        "dataset": dataset,
        "trials": rows.len(),
        "arms": per_arm,
        "assumption_bound_cases": ["C09"],
    });
    Ok((rows, summary))
}

/// D8 の gate（replay だけ）。違反の一覧（空なら合格）。
pub fn gate(rows: &[Value], summary: &Value, fixtures: &Path) -> Vec<String> {
    let mut errs = Vec::new();
    if summary["dataset"] != REPLAY_DATASET {
        errs.push(format!("dataset {}", summary["dataset"]));
    }
    let cases: std::collections::BTreeSet<&str> =
        rows.iter().filter_map(|r| r["case"].as_str()).collect();
    if cases.len() < 8 || cases.len() > 12 {
        errs.push(format!("{} cases (8..=12 required)", cases.len()));
    }
    for arm in ["old.passive", "old.proactive", "new"] {
        let n = rows.iter().filter(|r| r["arm"] == arm).count();
        if n != cases.len() {
            errs.push(format!("arm {arm}: {n} trials for {} cases", cases.len()));
        }
    }
    for r in rows.iter().filter(|r| r["arm"] == "new") {
        let case = r["case"].as_str().unwrap_or_default();
        let m = &r["metrics"];
        if r["grade"]["verdict"] != "pass" {
            let failed: Vec<&Value> = arr(&r["grade"]["predicates"])
                .iter()
                .filter(|p| p["status"] == "fail")
                .collect();
            errs.push(format!("new {case}: verdict fail {failed:?}"));
        }
        for (k, want) in [("double_effect", 0), ("wasteful_roots", 0)] {
            if u(&m[k]) != want {
                errs.push(format!("new {case}: {k} = {}", m[k]));
            }
        }
        if u(&m["human_intervention"]["avoidable"]) != 0 {
            errs.push(format!(
                "new {case}: avoidable {}",
                m["human_intervention"]["avoidable"]
            ));
        }
        if m["fired_faults"] != m["recovered_faults"] {
            errs.push(format!(
                "new {case}: recovered {} of {}",
                m["recovered_faults"], m["fired_faults"]
            ));
        }
        let expected = read_json(&fixtures.join("oracle").join(case).join("oracle.json"))
            .map(|o| o["expected_faults"].clone())
            .unwrap_or(Value::Null);
        if m["fired_faults"] != expected {
            errs.push(format!(
                "new {case}: fired {} faults, expected {expected}",
                m["fired_faults"]
            ));
        }
        for k in ["tokens", "quota", "wall_secs"] {
            if !m[k].is_null() {
                errs.push(format!("new {case}: {k} must be null in replay"));
            }
        }
    }
    let a = &summary["arms"];
    for k in ["reach_rate", "autonomous_reach"] {
        let (n, o) = (a["new"][k].as_f64(), a["old.passive"][k].as_f64());
        if !matches!((n, o), (Some(n), Some(o)) if n >= o) {
            errs.push(format!("{k}: new {n:?} < old.passive {o:?}"));
        }
    }
    errs
}
