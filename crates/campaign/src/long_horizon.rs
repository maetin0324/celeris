//! ADR 2026-11-04-long-horizon-bench（A8 実行）: long-horizon ベンチの採点・指標・集計。
//!
//! 入力は試行ごとの事実の export（`celeris.long-horizon-facts/1`。腕の欄を持たない）・oracle
//! （`celeris.long-horizon-oracle/1`）・保護領域の検査 script（`grader/check_outcome.py`）・試行の作業 tree の最終の木だけ。
//! 採点は腕を知らない。腕との対応（`arm-map/`）は集計の最後に結合する。LLM を使わず、LLM の回答文を入力にしない。
//! Outcome の到達は検査 script の exit code だけで決まる（ADR 2026-11-05-phase3-achievement-invariants I5、
//! ADR 2026-11-05-phase3-p0c）。script は `env -i`・`python3 -I -S`・空の一時 dir を cwd にした子 process で走り、
//! 試行の木のコードを実行も import もしない。同じ入力から同じ bytes を出す。

use crate::grade_runner::ProcessRunner;
use crate::grader::CommandRunner;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const FACTS_SCHEMA: &str = "celeris.long-horizon-facts/1";
pub const ORACLE_SCHEMA: &str = "celeris.long-horizon-oracle/1";
pub const GRADE_SCHEMA: &str = "celeris.long-horizon-grade/1";
pub const SUMMARY_SCHEMA: &str = "celeris.long-horizon-summary/1";
pub const REPLAY_DATASET: &str = "long_horizon_replay";
/// 実 LLM pilot の dataset（ADR D1。replay と混ぜない。raw の置き場は A9 の進捗 dir の `a9/llm/`）。
pub const LLM_PILOT_DATASET: &str = "long_horizon_llm_pilot";
/// 対照の腕（ADR D5。常に待つ decider・人の bot は催促しない）。
pub const CONTROL_ARM: &str = "ctl.null";

/// tier の名前（`replay`・`llm_pilot`）から dataset の値。
pub fn dataset_of_tier(tier: &str) -> Result<&'static str, String> {
    match tier {
        "replay" => Ok(REPLAY_DATASET),
        "llm_pilot" => Ok(LLM_PILOT_DATASET),
        other => Err(format!("unknown tier {other} (replay | llm_pilot)")),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn u(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// 今の bench の版（I6。facts・oracle の `bench_version` と一致しなければ採点を拒否する）。
pub const BENCH_VERSION: i64 = 2;
/// fixtures の下の検査 script（保護領域。public にも試行の木にも入れない）。
pub const GRADER_SCRIPT: &str = "grader/check_outcome.py";
const GRADER_TIMEOUT: Duration = Duration::from_secs(30);

/// Outcome 1 件の検査の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeCheck {
    pub reached: bool,
    pub violations: Vec<String>,
}

/// Outcome ごとの検査の結果（1 試行分）。`grade`・`metrics` はこれだけから到達を読む。
pub type Judged = BTreeMap<String, OutcomeCheck>;

/// 保護領域の検査 script の実行器。開くときに script の hash を `oracle/MANIFEST.sha256` と照合する。
#[derive(Debug, Clone)]
pub struct Grader {
    fixtures: PathBuf,
    script: PathBuf,
    python: PathBuf,
    needles: Vec<String>,
}

impl Grader {
    /// script が manifest と違う・python3 が無いなら Err（採点しない）。
    pub fn open(fixtures: &Path) -> Result<Self, String> {
        let fixtures = fixtures
            .canonicalize()
            .map_err(|e| format!("{}: {e}", fixtures.display()))?;
        let script = fixtures.join(GRADER_SCRIPT);
        let bytes = std::fs::read(&script).map_err(|e| format!("{}: {e}", script.display()))?;
        let have = hex(&Sha256::digest(&bytes));
        let manifest_path = fixtures.join("oracle/MANIFEST.sha256");
        let manifest = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        let want = manifest
            .lines()
            .find_map(|l| l.strip_suffix(&format!("  {GRADER_SCRIPT}")))
            .ok_or_else(|| format!("{GRADER_SCRIPT} is not in MANIFEST.sha256"))?;
        if want != have {
            return Err(format!(
                "{GRADER_SCRIPT} does not match MANIFEST.sha256 (tampered or stale)"
            ));
        }
        let path = std::env::var_os("PATH").ok_or("PATH is not set")?;
        let python = std::env::split_paths(&path)
            .map(|d| d.join("python3"))
            .find(|p| p.is_file())
            .ok_or("python3 is missing")?;
        let holdout = read_json(&fixtures.join("oracle/holdout.json"))?;
        let marker = String::from_utf8_lossy(&bytes)
            .split("GRADER_MARKER = \"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .map(str::to_owned)
            .ok_or("the grader script has no GRADER_MARKER")?;
        let mut needles = vec![have, marker];
        for k in ["canary", "trial_salt"] {
            needles.push(
                holdout[k]
                    .as_str()
                    .ok_or_else(|| format!("holdout.json has no {k}"))?
                    .to_owned(),
            );
        }
        Ok(Self {
            fixtures,
            script,
            python,
            needles,
        })
    }

    /// 検査 script を 1 回走らせる。exit 0 = 到達、1 = 未到達、それ以外は Err（観測できなかった。未到達に数えない）。
    pub fn check(&self, case: &str, outcome: &str, tree: &Path) -> Result<OutcomeCheck, String> {
        let tree = tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
        // 木と保護領域が重なるなら採点しない（worker が script を書き換えられる配置を拒む）。
        if tree.starts_with(&self.fixtures) || self.fixtures.starts_with(&tree) {
            return Err(format!(
                "tree {} overlaps the protected fixtures",
                tree.display()
            ));
        }
        let cwd = tempfile::tempdir().map_err(|e| e.to_string())?;
        let work = cwd.path().join("cwd");
        std::fs::create_dir(&work).map_err(|e| e.to_string())?;
        let argv: Vec<String> = vec![
            "/usr/bin/env".into(),
            "-i".into(),
            self.python.display().to_string(),
            "-I".into(),
            "-S".into(),
            self.script.display().to_string(),
            "--case".into(),
            case.into(),
            "--outcome".into(),
            outcome.into(),
            "--tree".into(),
            tree.display().to_string(),
        ];
        let runner = ProcessRunner {
            timeout: GRADER_TIMEOUT,
        };
        let out = runner.run(&argv, &work)?;
        match out.exit {
            Some(code @ (0 | 1)) => {
                let v: Value = serde_json::from_str(out.stdout.trim())
                    .map_err(|e| format!("grader output {:?}: {e}", out.stdout))?;
                if v["reached"].as_bool() != Some(code == 0) {
                    return Err(format!("grader exit {code} with {}", out.stdout));
                }
                Ok(OutcomeCheck {
                    reached: code == 0,
                    violations: arr(&v["violations"])
                        .iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect(),
                })
            }
            other => Err(format!(
                "grader exit {other:?} for {case} {outcome}: {} {}",
                out.stdout, out.stderr
            )),
        }
    }

    /// oracle の必須の Outcome をすべて検査する。
    pub fn judge(&self, oracle: &Value, tree: &Path) -> Result<Judged, String> {
        let case = oracle["case"].as_str().unwrap_or_default();
        let mut judged = Judged::new();
        for o in arr(&oracle["required_outcomes"]) {
            let id = o.as_str().unwrap_or_default();
            judged.insert(id.to_owned(), self.check(case, id, tree)?);
        }
        Ok(judged)
    }

    /// 試行の木に検査 script の印・hash・holdout の値が現れた file（D4 の検出。`leak_detected`）。
    pub fn leaks(&self, tree: &Path) -> Vec<String> {
        let mut hits = Vec::new();
        let mut stack = vec![tree.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let (p, Ok(ft)) = (e.path(), e.file_type()) else {
                    continue;
                };
                if ft.is_dir() {
                    stack.push(p);
                } else if ft.is_file() {
                    let name_hit = matches!(
                        e.file_name().to_str(),
                        Some("check_outcome.py" | "oracle.json" | "scenario.json" | "holdout.json")
                    );
                    let bytes = std::fs::read(&p).unwrap_or_default();
                    let text_hit = self
                        .needles
                        .iter()
                        .any(|n| bytes.windows(n.len()).any(|w| w == n.as_bytes()));
                    if name_hit || text_hit {
                        hits.push(p.strip_prefix(tree).unwrap_or(&p).display().to_string());
                    }
                }
            }
        }
        hits.sort();
        hits
    }
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
pub fn grade(facts: &Value, oracle: &Value, judged: &Judged) -> Result<Value, String> {
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
    if oracle["bench_version"] != BENCH_VERSION {
        return Err(format!(
            "bench_version {} is not {BENCH_VERSION}",
            oracle["bench_version"]
        ));
    }
    let mut preds: Vec<Value> = Vec::new();
    let mut check = |name: String, ok: bool, reason: String| {
        preds.push(json!({"name": name, "status": if ok {"pass"} else {"fail"}, "reason": reason}));
    };
    for o in arr(&oracle["required_outcomes"]) {
        let id = o.as_str().unwrap_or_default();
        let c = judged.get(id);
        check(
            format!("outcome_reached({id})"),
            c.is_some_and(|c| c.reached),
            match c {
                Some(c) if c.reached => "the grader script exited 0 on the final tree".to_owned(),
                Some(c) => format!("grader: {}", c.violations.join(",")),
                None => "the grader script did not run".to_owned(),
            },
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
pub fn metrics(facts: &Value, oracle: &Value, judged: &Judged) -> Value {
    let reached_id = |id: &&str| judged.get(*id).is_some_and(|c| c.reached);
    let required: Vec<&str> = arr(&oracle["required_outcomes"])
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let reached = required.iter().filter(|id| reached_id(id)).count();
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
    // 人の指示を引かない root（controller の起票・CoS の自発の起票）が done になって到達した必須の Outcome の数。
    // 対照 `ctl.null` で 0 になることを gate が確かめる（到達が台本の足場でなく機構によること）。
    let reached_without_instruction = required
        .iter()
        .filter(|id| reached_id(id))
        .filter(|id| {
            roots_of(facts, id).iter().any(|r| {
                r["instructed"] == false
                    && arr(&r["statuses"]).last().and_then(Value::as_str) == Some("done")
            })
        })
        .count();
    let daemon_roots = arr(&facts["roots"])
        .iter()
        .filter(|r| r["created_by"] == "daemon")
        .count();
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
        "reached_without_instruction": reached_without_instruction,
        "daemon_roots": daemon_roots,
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
    // 同一初期条件（D2）: 課題ごとに全試行の `init_digest` が 1 つに揃う。違う・欠けるならその課題の試行を
    // すべて `invalid_init` にして分母から外す。
    let mut digests: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    for f in &files {
        let v = read_json(f)?;
        let ds = v["dataset"].as_str().unwrap_or_default().to_owned();
        match &dataset {
            None => dataset = Some(ds),
            Some(d) if *d != ds => return Err(format!("mixed datasets: {d} and {ds}")),
            _ => {}
        }
        digests
            .entry(v["case"].as_str().unwrap_or_default().to_owned())
            .or_default()
            .insert(v["init_digest"].as_str().unwrap_or("missing").to_owned());
    }
    let invalid_cases: Vec<String> = digests
        .iter()
        .filter(|(_, d)| d.len() != 1 || d.contains("missing"))
        .map(|(c, _)| c.clone())
        .collect();
    let mut invalid_init = 0usize;
    let mut leak_detected = 0usize;
    let grader = Grader::open(fixtures)?;
    for f in files {
        let trial = f
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        let facts = read_json(&f)?;
        let case = facts["case"].as_str().unwrap_or_default().to_owned();
        let oracle = read_json(&fixtures.join("oracle").join(&case).join("oracle.json"))?;
        let repo = raw.join("trials").join(&trial).join("repo");
        // 版が違う raw（v1 を含む）は検査 script を走らせる前に拒む（I6）。
        if facts["bench_version"] != oracle["bench_version"] {
            return Err(format!(
                "{trial}: facts and oracle disagree on bench_version ({} vs {})",
                facts["bench_version"], oracle["bench_version"]
            ));
        }
        let judged = grader.judge(&oracle, &repo)?;
        let g = grade(&facts, &oracle, &judged)?;
        let m = metrics(&facts, &oracle, &judged);
        let mut leaks = grader.leaks(&repo);
        leaks.extend(
            arr(&facts["leak_hits"])
                .iter()
                .filter_map(|h| h.as_str().map(str::to_owned)),
        );
        // 腕の結合は採点の後（D6）。
        let map = read_json(&raw.join("arm-map").join(format!("{trial}.json")))?;
        let arm = map["arm"].as_str().unwrap_or_default().to_owned();
        // I12: controller の achieved と oracle の合否を試行ごとに並べる。
        let achieved = facts["controller_achieved"]
            .as_bool()
            .ok_or_else(|| format!("{trial}: facts have no controller_achieved"))?;
        let verdict = g["verdict"].clone();
        let agreement = json!({
            "controller_achieved": achieved,
            "oracle_verdict": verdict,
            "mismatch": achieved && verdict != "pass",
            "oracle_pass_without_achieved": !achieved && verdict == "pass",
        });
        let mut row = json!({"trial": trial, "case": case, "arm": arm, "grade": g, "metrics": m, "agreement": agreement});
        if !leaks.is_empty() {
            row["invalid"] = json!("leak_detected");
            row["leaks"] = json!(leaks);
            leak_detected += 1;
        } else if invalid_cases.contains(&case) {
            row["invalid"] = json!("invalid_init");
            invalid_init += 1;
        } else {
            arms.entry(arm).or_default().push(row.clone());
        }
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
                "reached_without_instruction": sum("reached_without_instruction"),
                "daemon_roots": sum("daemon_roots"),
                "controller_achieved": rs.iter().filter(|r| r["agreement"]["controller_achieved"] == true).count(),
                "achieved_oracle_mismatch": rs.iter().filter(|r| r["agreement"]["mismatch"] == true).count(),
                "oracle_pass_without_achieved": rs.iter().filter(|r| r["agreement"]["oracle_pass_without_achieved"] == true).count(),
                "tokens": usage_sum(rs, "tokens"),
                "quota": usage_sum(rs, "quota"),
                "wall_secs": usage_sum(rs, "wall_secs"),
                "missing": usage_missing(rs, dataset.as_deref()),
            }),
        );
    }
    let summary = json!({
        "schema": SUMMARY_SCHEMA,
        "dataset": dataset,
        "trials": rows.len(),
        "invalid": {"invalid_init": invalid_init, "cases": invalid_cases, "leak_detected": leak_detected},
        "achieved_oracle_mismatch": rows.iter().filter(|r| r["invalid"].is_null() && r["agreement"]["mismatch"] == true).count(),
        "arms": per_arm,
        "assumption_bound_cases": ["C09"],
    });
    Ok((rows, summary))
}

/// 腕の usage の和。1 試行でも null（未計測）なら null（0 や推定値で埋めない。D7）。
fn usage_sum(rs: &[Value], k: &str) -> Value {
    let mut total = 0f64;
    for r in rs {
        match r["metrics"][k].as_f64() {
            Some(x) => total += x,
            None => return Value::Null,
        }
    }
    json!(total)
}

fn usage_missing(rs: &[Value], dataset: Option<&str>) -> Value {
    if dataset == Some(REPLAY_DATASET) {
        return json!("tier=replay（実 LLM を使っていないので未計測）");
    }
    let n = rs
        .iter()
        .filter(|r| r["metrics"]["tokens"].is_null())
        .count();
    if n == 0 {
        Value::Null
    } else {
        json!(format!("{n} trials have no usage"))
    }
}

/// tier を指定した集計（`celeris-campaign long-horizon-grade --tier …` の本体）。facts の dataset が tier と違えば Err
/// （replay と実 LLM pilot の raw を混ぜない。D1）。`llm_pilot` で usage が `tier=replay` の facts も Err。
pub fn aggregate_tier(
    raw: &Path,
    fixtures: &Path,
    tier: &str,
) -> Result<(Vec<Value>, Value), String> {
    let want = dataset_of_tier(tier)?;
    for f in sorted_files(&raw.join("facts"))? {
        let v = read_json(&f)?;
        if v["dataset"] != want {
            return Err(format!(
                "{}: dataset {} is not {want} (tier {tier})",
                f.display(),
                v["dataset"]
            ));
        }
        if tier == "llm_pilot" && v["usage"]["missing"] == "tier=replay" {
            return Err(format!("{}: replay usage in llm_pilot", f.display()));
        }
    }
    let (rows, mut summary) = aggregate(raw, fixtures)?;
    summary["tier"] = json!(tier);
    Ok((rows, summary))
}

/// `celeris-campaign long-horizon-grade --tier <replay|llm_pilot> --raw <dir> --fixtures <dir> --out <dir>`。
/// `<out>/grades.jsonl` と `<out>/summary.json` を書く。replay なら D8 の gate も確かめる。
pub fn cli(args: &[String]) -> Result<String, String> {
    let get = |k: &str| -> Result<std::path::PathBuf, String> {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .map(std::path::PathBuf::from)
            .ok_or_else(|| format!("{k} is required"))
    };
    let tier = get("--tier")?.display().to_string();
    let (raw, fixtures, out) = (get("--raw")?, get("--fixtures")?, get("--out")?);
    let (rows, summary) = aggregate_tier(&raw, &fixtures, &tier)?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let lines: Vec<String> = rows.iter().map(|r| r.to_string()).collect();
    std::fs::write(out.join("grades.jsonl"), lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?;
    std::fs::write(out.join("summary.json"), text + "\n").map_err(|e| e.to_string())?;
    if tier == "replay" {
        let errs = gate(&rows, &summary, &fixtures);
        if !errs.is_empty() {
            return Err(format!("D8 gate: {errs:?}"));
        }
    }
    Ok(format!(
        "long-horizon-grade: tier {tier}, {} trials, written to {}",
        rows.len(),
        out.display()
    ))
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
    if summary["invalid"]["invalid_init"] != 0 {
        errs.push(format!("invalid_init: {}", summary["invalid"]));
    }
    if summary["invalid"]["leak_detected"] != 0 {
        errs.push(format!("leak_detected: {}", summary["invalid"]));
    }
    if summary["achieved_oracle_mismatch"] != 0 {
        errs.push(format!(
            "achieved_oracle_mismatch: {}",
            summary["achieved_oracle_mismatch"]
        ));
    }
    for arm in ["old.passive", "old.proactive", "new", CONTROL_ARM] {
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
    // 対照（D5）: 常に待つ decider では controller の root が 0、人の指示を引かない到達が 0。
    // `new` には人の指示を引かない到達がある（到達が機構によることの裏づけ）。
    let a = &summary["arms"];
    for k in ["reached_without_instruction", "daemon_roots"] {
        if a[CONTROL_ARM][k] != 0 {
            errs.push(format!("{CONTROL_ARM}: {k} = {}", a[CONTROL_ARM][k]));
        }
    }
    if u(&a["new"]["reached_without_instruction"]) == 0 {
        errs.push("new: no outcome was reached without a human instruction".into());
    }
    for k in ["reach_rate", "autonomous_reach"] {
        let (n, o) = (a["new"][k].as_f64(), a["old.passive"][k].as_f64());
        if !matches!((n, o), (Some(n), Some(o)) if n >= o) {
            errs.push(format!("{k}: new {n:?} < old.passive {o:?}"));
        }
    }
    errs
}
