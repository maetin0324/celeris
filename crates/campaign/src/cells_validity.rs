//! cmp4 系（K4）のセル有効性を固定 raw から決定的に再計算し、成果物の表（`results/cells.csv`）と報告の件数 metric に
//! 照合する（ADR 2026-10-23-k4-cells-validity-grading D1/D2）。LLM は使わない。報告の自由記述は読まない。
//!
//! 期待セル集合は `planned-cells.json`（欠測も分母に残す）。セルは「自身の完了記録（exit 0）・実効設定（fsync/checkRead/api、
//! fsync fallback 警告なし）・完全な出力・job の終了状態」が揃ったときだけ有効。job は `qstat -xf` 記録で
//! `F` かつ `Exit_status=0` が正常。`F` 以外・記録なしは無効、`F` かつ `Exit_status≠0` は完了記録の無いセルの理由になる。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::grader::{read_json, safe_join};

/// 表の数値列（有効セルについて許容差内で照合する）。
pub const VALUE_COLUMNS: [&str; 7] = [
    "write_MiB_s",
    "read_MiB_s",
    "write_ops",
    "read_ops",
    "md_create",
    "md_stat",
    "md_remove",
];
/// 表に必須の列。
pub const KEY_COLUMNS: [&str; 5] = ["fs", "rep", "workload", "valid", "reasons"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellsSpec {
    /// raw root（固定入力の展開先）からの run dir の相対 path（例 `inputs/runs/43706-20261008T045834Z`）。
    pub run: String,
    /// run dir からの planned-cells.json の相対 path。
    #[serde(default = "default_planned")]
    pub planned: String,
    /// job id（`.pbs` 等の suffix 無し）→ `qstat -xf` 記録（raw root 相対）。
    #[serde(default)]
    pub jobs: BTreeMap<String, String>,
    /// 成果物内の表。
    #[serde(default = "default_table")]
    pub table: String,
    /// fs → IOR の api 名（無い fs は api を見ない）。
    #[serde(default)]
    pub api: BTreeMap<String, String>,
    /// 有効セルの数値の相対許容差。
    #[serde(default = "default_tol")]
    pub tolerance_rel: f64,
}

fn default_planned() -> String {
    "planned-cells.json".into()
}
fn default_table() -> String {
    "results/cells.csv".into()
}
fn default_tol() -> f64 {
    0.001
}

/// `qstat -xf` 記録から読んだ job の終了状態。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobState {
    /// 記録なし・`Unknown Job Id`。
    Unknown,
    /// `job_state` が F 以外。
    NotFinished { state: String },
    /// F かつ Exit_status != 0。
    Failed { exit_status: i64 },
    /// F かつ Exit_status == 0。
    Finished,
}

/// `qstat -xf` の text を読む。`job_state` と `Exit_status` の行だけ見る。
pub fn parse_qstat(text: &str) -> JobState {
    if text.contains("Unknown Job Id") {
        return JobState::Unknown;
    }
    let field = |name: &str| -> Option<String> {
        text.lines().find_map(|l| {
            let (k, v) = l.trim().split_once('=')?;
            (k.trim() == name).then(|| v.trim().to_string())
        })
    };
    match field("job_state").as_deref() {
        None => JobState::Unknown,
        Some("F") => match field("Exit_status").and_then(|v| v.parse::<i64>().ok()) {
            Some(0) => JobState::Finished,
            Some(n) => JobState::Failed { exit_status: n },
            None => JobState::NotFinished {
                state: "F (no Exit_status)".into(),
            },
        },
        Some(s) => JobState::NotFinished { state: s.into() },
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CellRow {
    pub fs: String,
    pub rep: u64,
    pub workload: String,
    pub job: Option<String>,
    pub valid: bool,
    /// 無効の理由 code（先頭が主理由）。`missing` / `exit=<n>` / `job_unknown` / `job_not_finished(<state>)` /
    /// `job_failed(exit=<n>)` / `fsync=<v>` / `checkRead=<v>` / `api=<v>` / `ior_incomplete` / `mdtest_incomplete`。
    pub reasons: Vec<String>,
    pub values: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Expected {
    pub rows: Vec<CellRow>,
}

impl Expected {
    pub fn fs_list(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|r| r.fs.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// 報告に要る件数 metric: `cells_total` `cells_valid` `cells_total_<fs>` `cells_valid_<fs>`。
    pub fn counts(&self) -> BTreeMap<String, f64> {
        let mut m = BTreeMap::new();
        m.insert("cells_total".into(), self.rows.len() as f64);
        m.insert(
            "cells_valid".into(),
            self.rows.iter().filter(|r| r.valid).count() as f64,
        );
        for fs in self.fs_list() {
            let rows: Vec<_> = self.rows.iter().filter(|r| r.fs == fs).collect();
            m.insert(format!("cells_total_{fs}"), rows.len() as f64);
            m.insert(
                format!("cells_valid_{fs}"),
                rows.iter().filter(|r| r.valid).count() as f64,
            );
        }
        m
    }

    /// 期待値どおりの `results/cells.csv`（試験・oracle 用）。
    pub fn csv(&self) -> String {
        let mut s = String::new();
        s.push_str(&KEY_COLUMNS.join(","));
        for c in VALUE_COLUMNS {
            s.push(',');
            s.push_str(c);
        }
        s.push('\n');
        for r in &self.rows {
            let mut cols = vec![
                r.fs.clone(),
                r.rep.to_string(),
                r.workload.clone(),
                if r.valid { "valid" } else { "invalid" }.to_string(),
                format!("\"{}\"", r.reasons.join(";")),
            ];
            for c in VALUE_COLUMNS {
                cols.push(r.values.get(c).map(|v| v.to_string()).unwrap_or_default());
            }
            s.push_str(&cols.join(","));
            s.push('\n');
        }
        s
    }

    /// 期待値どおりの `metric <name> = <value>` 行。
    pub fn metric_lines(&self) -> String {
        self.counts()
            .iter()
            .map(|(k, v)| format!("metric {k} = {v}\n"))
            .collect()
    }
}

fn job_key(id: &str) -> String {
    id.trim().split('.').next().unwrap_or("").to_string()
}

/// 固定 raw から期待セル集合と各セルの有効性・理由・値を計算する。Err は raw 側（grader 資産）の不整合。
pub fn compute(raw: &Path, spec: &CellsSpec) -> Result<Expected, String> {
    let run = safe_join(raw, &spec.run)?;
    let planned_path = safe_join(&run, &spec.planned)?;
    let planned =
        read_json(&planned_path).map_err(|e| format!("planned cells {}: {e}", spec.planned))?;
    let arr = planned
        .as_array()
        .ok_or(format!("planned cells {} is not an array", spec.planned))?;
    let mut jobs: BTreeMap<String, JobState> = BTreeMap::new();
    for (id, rel) in &spec.jobs {
        let p = safe_join(raw, rel)?;
        let text = fs::read_to_string(&p)
            .map_err(|_| format!("job record {rel} for job {id} is missing from the raw inputs"))?;
        jobs.insert(job_key(id), parse_qstat(&text));
    }
    let sole_job = (jobs.len() == 1)
        .then(|| jobs.keys().next().cloned())
        .flatten();
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for p in arr {
        let fs_name = p["fs"].as_str().ok_or("planned cell without fs")?;
        let rep = p["rep"].as_u64().ok_or("planned cell without rep")?;
        let wl = p["workload"]
            .as_str()
            .ok_or("planned cell without workload")?;
        if !seen.insert((fs_name.to_string(), rep, wl.to_string())) {
            return Err(format!(
                "planned cell {fs_name}/rep{rep}/{wl} is listed twice"
            ));
        }
        let dir = run
            .join("cells")
            .join(fs_name)
            .join(format!("rep{rep}"))
            .join(wl);
        rows.push(audit_cell(
            &dir,
            fs_name,
            rep,
            wl,
            spec,
            &jobs,
            sole_job.as_deref(),
        ));
    }
    if rows.is_empty() {
        return Err(format!("planned cells {} is empty", spec.planned));
    }
    Ok(Expected { rows })
}

fn text_of(p: &Path) -> String {
    fs::read(p)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

fn positive(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite() && *x > 0.0)
}

fn audit_cell(
    dir: &Path,
    fs_name: &str,
    rep: u64,
    wl: &str,
    spec: &CellsSpec,
    jobs: &BTreeMap<String, JobState>,
    sole_job: Option<&str>,
) -> CellRow {
    let mut reasons: Vec<String> = Vec::new();
    let mut values = BTreeMap::new();
    let cell = read_json(&dir.join("cell.json")).ok();
    let exit = read_json(&dir.join("exit.json")).ok();
    match (&cell, &exit) {
        (Some(c), Some(e)) => {
            let (ce, ee) = (c["exit"].as_i64(), e["exit"].as_i64());
            if ce != Some(0) || ee != Some(0) {
                reasons.push(format!(
                    "exit={}",
                    ee.or(ce).map(|x| x.to_string()).unwrap_or("?".into())
                ));
            }
        }
        _ => reasons.push("missing".into()),
    }
    if cell.is_some() {
        if wl == "w3" {
            audit_mdtest(dir, &mut reasons, &mut values);
        } else {
            audit_ior(dir, fs_name, spec, &mut reasons, &mut values);
        }
    }
    let complete = reasons.is_empty();
    let job = cell
        .as_ref()
        .and_then(|c| c["job"].as_str().map(job_key))
        .or_else(|| sole_job.map(String::from));
    match job.as_deref().and_then(|j| jobs.get(j)) {
        None => reasons.push("job_unknown".into()),
        Some(JobState::Unknown) => reasons.push("job_unknown".into()),
        Some(JobState::NotFinished { state }) => {
            reasons.push(format!("job_not_finished({state})"));
        }
        Some(JobState::Failed { exit_status }) if !complete => {
            reasons.push(format!("job_failed(exit={exit_status})"));
        }
        Some(JobState::Failed { .. }) | Some(JobState::Finished) => {}
    }
    CellRow {
        fs: fs_name.into(),
        rep,
        workload: wl.into(),
        job,
        valid: reasons.is_empty(),
        reasons,
        values,
    }
}

fn audit_ior(
    dir: &Path,
    fs_name: &str,
    spec: &CellsSpec,
    reasons: &mut Vec<String>,
    values: &mut BTreeMap<String, f64>,
) {
    let mut incomplete = false;
    match read_json(&dir.join("ior.json")) {
        Err(_) => incomplete = true,
        Ok(j) => {
            if j.get("Finished").is_none() {
                incomplete = true;
            }
            let empty = Vec::new();
            let summ = j["summary"].as_array().unwrap_or(&empty);
            for op in ["write", "read"] {
                match summ.iter().find(|s| s["operation"] == op) {
                    Some(s) => match (positive(&s["bwMeanMIB"]), positive(&s["OPsMean"])) {
                        (Some(bw), Some(ops)) => {
                            values.insert(format!("{op}_MiB_s"), bw);
                            values.insert(format!("{op}_ops"), ops);
                        }
                        _ => incomplete = true,
                    },
                    None => incomplete = true,
                }
            }
            let prm = &j["tests"][0]["Parameters"];
            if prm["fsync"].as_i64() != Some(1) {
                reasons.push(format!("fsync={}", prm["fsync"]));
            }
            if prm["checkRead"].as_i64() != Some(1) {
                reasons.push(format!("checkRead={}", prm["checkRead"]));
            }
            if let Some(want) = spec.api.get(fs_name)
                && prm["api"].as_str() != Some(want)
            {
                reasons.push(format!("api={}", prm["api"]));
            }
        }
    }
    if incomplete {
        reasons.push("ior_incomplete".into());
    }
    let log = text_of(&dir.join("stdout.log")) + &text_of(&dir.join("stderr.log"));
    let fallback = log.contains("fsync() not supported")
        || log
            .lines()
            .any(|l| l.contains("WARNING: fsync(") && l.contains(") failed"));
    if fallback && !reasons.iter().any(|r| r.starts_with("fsync")) {
        reasons.push("fsync=0(fallback)".into());
    }
}

fn audit_mdtest(dir: &Path, reasons: &mut Vec<String>, values: &mut BTreeMap<String, f64>) {
    let mut incomplete = false;
    match parse_csv(&text_of(&dir.join("mdtest-ranks.csv"))) {
        Ok((header, rows)) => {
            let col = |n: &str| header.iter().position(|h| h == n);
            let all = rows.iter().find(|r| {
                col("rank")
                    .and_then(|i| r.get(i))
                    .is_some_and(|v| v == "all")
            });
            for (label, key) in [
                ("rate-File creation", "md_create"),
                ("rate-File stat", "md_stat"),
                ("rate-File removal", "md_remove"),
            ] {
                let v = all
                    .and_then(|r| col(label).and_then(|i| r.get(i)))
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|x| x.is_finite() && *x > 0.0);
                match v {
                    Some(x) => {
                        values.insert(key.into(), x);
                    }
                    None => incomplete = true,
                }
            }
        }
        Err(_) => incomplete = true,
    }
    if !text_of(&dir.join("stdout.log")).contains("-- finished at") {
        incomplete = true;
    }
    if incomplete {
        reasons.push("mdtest_incomplete".into());
    }
}

/// 1 行の CSV record（RFC 4180 の引用符つき欄に対応。欄内の改行は扱わない）。
fn split_record(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            '"' if cur.is_empty() => quoted = true,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).collect()
}

/// header 行と data 行。空行は飛ばす。
pub fn parse_csv(text: &str) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut lines = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty());
    let header = split_record(lines.next().ok_or("empty csv")?);
    Ok((header, lines.map(split_record).collect()))
}

/// 無効理由 code の主理由を、作業者の reasons 欄が含むべき語（小文字で比較）。
pub fn keywords(code: &str) -> &'static [&'static str] {
    let head = code.split(['=', '(']).next().unwrap_or(code);
    match head {
        "missing" => &["missing", "欠測", "not run", "未実行", "no cell", "absent"],
        "exit" => &["exit"],
        "job_unknown" | "job_not_finished" | "job_failed" => &["job"],
        "fsync" => &["fsync"],
        "checkRead" => &["checkread"],
        "api" => &["api"],
        "ior_incomplete" => &["ior"],
        "mdtest_incomplete" => &["mdtest"],
        _ => &[],
    }
}

pub struct Outcome {
    /// 成果物側の不備（model failure）。空なら合格。
    pub reasons: Vec<String>,
    pub details: Value,
}

/// 表と報告の件数 metric を期待値に照合する。Err は raw（grader 資産）側の不整合。
pub fn check(
    spec: &CellsSpec,
    artifact: &Path,
    raw: &Path,
    reported: &BTreeMap<String, String>,
) -> Result<Outcome, String> {
    let exp = compute(raw, spec)?;
    let mut reasons = Vec::new();
    let table_text = safe_join(artifact, &spec.table).and_then(|p| {
        fs::read_to_string(&p).map_err(|_| format!("cells table {} is missing", spec.table))
    });
    let mut table_counts: Option<BTreeMap<String, f64>> = None;
    match table_text.and_then(|t| parse_csv(&t)) {
        Err(e) => reasons.push(e),
        Ok((header, rows)) => {
            let missing_cols: Vec<&str> = KEY_COLUMNS
                .iter()
                .chain(VALUE_COLUMNS.iter())
                .filter(|c| !header.iter().any(|h| h == *c))
                .copied()
                .collect();
            if !missing_cols.is_empty() {
                reasons.push(format!(
                    "cells table lacks column(s) {}",
                    missing_cols.join(", ")
                ));
            } else {
                let col = |n: &str| header.iter().position(|h| h == n).unwrap_or(usize::MAX);
                let get = |r: &Vec<String>, n: &str| r.get(col(n)).cloned().unwrap_or_default();
                let mut by_key: BTreeMap<(String, u64, String), Vec<String>> = BTreeMap::new();
                for (i, r) in rows.iter().enumerate() {
                    let key = (
                        get(r, "fs"),
                        get(r, "rep").parse::<u64>().unwrap_or(0),
                        get(r, "workload"),
                    );
                    if by_key.insert(key.clone(), r.clone()).is_some() {
                        reasons.push(format!(
                            "cells table row {}: cell {}/rep{}/{} appears twice",
                            i + 2,
                            key.0,
                            key.1,
                            key.2
                        ));
                    }
                }
                let mut tc: BTreeMap<String, f64> = BTreeMap::new();
                for e in &exp.rows {
                    let tag = format!("cell {}/rep{}/{}", e.fs, e.rep, e.workload);
                    let Some(r) = by_key.remove(&(e.fs.clone(), e.rep, e.workload.clone())) else {
                        reasons.push(format!(
                            "{tag}: not in the table (missing cells must stay in the denominator)"
                        ));
                        continue;
                    };
                    *tc.entry("cells_total".into()).or_default() += 1.0;
                    *tc.entry(format!("cells_total_{}", e.fs)).or_default() += 1.0;
                    let claimed = match get(&r, "valid").to_lowercase().as_str() {
                        "valid" => Some(true),
                        "invalid" => Some(false),
                        _ => None,
                    };
                    match claimed {
                        None => reasons.push(format!("{tag}: valid must be `valid` or `invalid`")),
                        Some(true) => {
                            *tc.entry("cells_valid".into()).or_default() += 1.0;
                            *tc.entry(format!("cells_valid_{}", e.fs)).or_default() += 1.0;
                        }
                        Some(false) => {}
                    }
                    match (claimed, e.valid) {
                        (Some(true), false) => reasons.push(format!(
                            "{tag}: marked valid but the raw shows an invalid cell ({})",
                            e.reasons.join(";")
                        )),
                        (Some(false), true) => reasons.push(format!(
                            "{tag}: marked invalid but the raw shows a valid cell"
                        )),
                        (Some(false), false) => {
                            let primary = e.reasons.first().cloned().unwrap_or_default();
                            let have = get(&r, "reasons").to_lowercase();
                            let want = keywords(&primary);
                            if !want.iter().any(|k| have.contains(k)) {
                                reasons.push(format!(
                                    "{tag}: reasons `{}` do not mention the primary cause {primary} (expected one of {})",
                                    get(&r, "reasons"),
                                    want.join("/")
                                ));
                            }
                        }
                        (Some(true), true) => {
                            for c in VALUE_COLUMNS {
                                let Some(x) = e.values.get(c) else { continue };
                                match get(&r, c).parse::<f64>() {
                                    Ok(v)
                                        if (v - x).abs()
                                            <= spec.tolerance_rel * x.abs().max(1e-12) => {}
                                    Ok(v) => reasons.push(format!(
                                        "{tag}: {c} = {v} but the raw gives {x} (tolerance {})",
                                        spec.tolerance_rel
                                    )),
                                    Err(_) => reasons.push(format!(
                                        "{tag}: {c} is missing or not a number for a valid cell"
                                    )),
                                }
                            }
                        }
                        (None, _) => {}
                    }
                }
                for (k, _) in by_key {
                    reasons.push(format!(
                        "cells table has {}/rep{}/{} which is not a planned cell",
                        k.0, k.1, k.2
                    ));
                }
                table_counts = Some(tc);
            }
        }
    }
    let counts = exp.counts();
    for (name, x) in &counts {
        let rep = reported.get(name).map(|s| s.parse::<f64>());
        match rep {
            None => reasons.push(format!("metric {name}: not reported")),
            Some(Err(_)) => reasons.push(format!("metric {name}: reported value is not a number")),
            Some(Ok(r)) => {
                if (r - x).abs() > 0.5 {
                    reasons.push(format!(
                        "metric {name}: reported {r} but recomputed {x} from raw data"
                    ));
                }
                if let Some(tc) = &table_counts {
                    let t = tc.get(name).copied().unwrap_or(0.0);
                    if (t - r).abs() > 0.5 {
                        reasons.push(format!(
                            "metric {name}: the table counts {t} but the report says {r} (table and report disagree)"
                        ));
                    }
                }
            }
        }
    }
    Ok(Outcome {
        reasons,
        details: json!({"expected": exp.rows, "counts": counts, "table_counts": table_counts}),
    })
}

/// 試験・無 LLM 検証用の合成 raw（本物の tgz と同じ配置: `runs/<run>/planned-cells.json`・`cells/<fs>/rep<n>/<wl>/…`・
/// `review/qstat-<job>.txt`）。
pub mod synth {
    use std::{collections::BTreeMap, fs, path::Path};

    use serde_json::json;

    use super::CellsSpec;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Defect {
        None,
        /// セルの dir ごと無い（未実行）。
        Absent,
        /// command.txt と途中で切れた ior.json だけ（cell.json・exit.json なし）。
        Truncated,
        ExitNonZero,
        /// Parameters.fsync = 0。
        FsyncZero,
        /// Parameters は fsync=1 だが stdout に fallback 警告。
        FsyncFallback,
        CheckReadZero,
        MdtestIncomplete,
    }

    #[derive(Clone, Debug)]
    pub struct Cell {
        pub fs: &'static str,
        pub rep: u64,
        pub workload: &'static str,
        pub job: &'static str,
        pub defect: Defect,
    }

    #[derive(Clone, Debug)]
    pub struct Plan {
        pub run: String,
        pub cells: Vec<Cell>,
        /// job id → qstat -xf text（`None` なら記録 file を置かない）。
        pub jobs: BTreeMap<&'static str, Option<String>>,
    }

    pub fn qstat(job: &str, state: &str, exit: Option<i64>) -> String {
        let mut s = format!("Job Id: {job}.pbs\n    job_state = {state}\n");
        if let Some(e) = exit {
            s.push_str(&format!("    Exit_status = {e}\n"));
        }
        s
    }

    pub fn api_of(fs: &str) -> &'static str {
        match fs {
            "benchfs" => "BENCHFS",
            "chfs" => "CHFS",
            _ => "POSIX",
        }
    }

    /// 4 FS × (w1a, w2, w3) × 1 rep、job 1001 が F/0 の健全な計画。
    pub fn healthy() -> Plan {
        let mut cells = Vec::new();
        for fs in ["benchfs", "chfs", "gekkofs", "unifyfs"] {
            for wl in ["w1a", "w2", "w3"] {
                cells.push(Cell {
                    fs,
                    rep: 1,
                    workload: wl,
                    job: "1001",
                    defect: Defect::None,
                });
            }
        }
        let mut jobs = BTreeMap::new();
        jobs.insert("1001", Some(qstat("1001", "F", Some(0))));
        Plan {
            run: "9001-20260101T000000Z".into(),
            cells,
            jobs,
        }
    }

    fn w(p: &Path, s: &str) -> Result<(), String> {
        if let Some(d) = p.parent() {
            fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        fs::write(p, s).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// `root/runs/<run>/…` と `root/review/qstat-<job>.txt` を書く。`root` は tgz の中身に相当。
    pub fn write(root: &Path, plan: &Plan) -> Result<(), String> {
        let run = root.join("runs").join(&plan.run);
        let planned: Vec<_> = plan
            .cells
            .iter()
            .map(|c| json!({"fs": c.fs, "rep": c.rep, "workload": c.workload, "ppn": 4, "nhosts": 2}))
            .collect();
        w(
            &run.join("planned-cells.json"),
            &serde_json::to_string_pretty(&planned).map_err(|e| e.to_string())?,
        )?;
        for (job, text) in &plan.jobs {
            if let Some(t) = text {
                w(&root.join("review").join(format!("qstat-{job}.txt")), t)?;
            }
        }
        for (i, c) in plan.cells.iter().enumerate() {
            let d = run
                .join("cells")
                .join(c.fs)
                .join(format!("rep{}", c.rep))
                .join(c.workload);
            if c.defect == Defect::Absent {
                continue;
            }
            w(&d.join("command.txt"), "ior\n")?;
            let base = 100.0 + i as f64 * 10.0;
            if c.defect == Defect::Truncated {
                w(&d.join("ior.json"), "{\n  \"Version\": \"4.1.0+dev\",\n")?;
                w(&d.join("stdout.log"), "started\n")?;
                continue;
            }
            let exit = if c.defect == Defect::ExitNonZero {
                255
            } else {
                0
            };
            w(
                &d.join("cell.json"),
                &json!({"fs": c.fs, "rep": c.rep, "workload": c.workload, "ppn": 4, "np": 8, "nhosts": 2, "exit": exit, "job": c.job}).to_string(),
            )?;
            w(
                &d.join("exit.json"),
                &json!({"exit": exit, "seconds": 10.0, "timeout": 600}).to_string(),
            )?;
            if c.workload == "w3" {
                let mut csv = String::from(
                    "rank,items,rate-File creation,time-File creation,rate-File stat,time-File stat,rate-File removal,time-File removal\n",
                );
                if c.defect == Defect::MdtestIncomplete {
                    csv.push_str(&format!("all,4000,{},1.0,,0.4,{},2.0\n", base, base + 2.0));
                    w(&d.join("stdout.log"), "V-0: Rank 0 Shifting ranks by 4\n")?;
                } else {
                    csv.push_str(&format!(
                        "all,4000,{},1.0,{},0.4,{},2.0\n",
                        base,
                        base + 1.0,
                        base + 2.0
                    ));
                    w(
                        &d.join("stdout.log"),
                        "V-0: Rank 0 Shifting ranks by 4\n-- finished at 01/01/2026 00:00:10 --\n",
                    )?;
                }
                w(&d.join("mdtest-ranks.csv"), &csv)?;
                w(&d.join("stderr.log"), "")?;
            } else {
                let fsync = if c.defect == Defect::FsyncZero { 0 } else { 1 };
                let check_read = if c.defect == Defect::CheckReadZero {
                    0
                } else {
                    1
                };
                let ior = json!({
                    "Version": "4.1.0+dev",
                    "summary": [
                        {"operation": "write", "bwMeanMIB": base, "OPsMean": base / 4.0},
                        {"operation": "read", "bwMeanMIB": base * 2.0, "OPsMean": base / 2.0}
                    ],
                    "tests": [{"Parameters": {"fsync": fsync, "fsyncperwrite": 0, "checkRead": check_read, "api": api_of(c.fs), "numTasks": 8}}],
                    "Finished": "Thu Jan  1 00:00:10 2026"
                });
                w(&d.join("ior.json"), &ior.to_string())?;
                let out = if c.defect == Defect::FsyncFallback {
                    "WARNING: fsync() not supported in selected backend\nMax Write: 1 MiB/sec\n"
                } else {
                    "Max Write: 1 MiB/sec\n"
                };
                w(&d.join("stdout.log"), out)?;
                w(&d.join("stderr.log"), "")?;
            }
        }
        Ok(())
    }

    /// `write` した raw が `extract_to`（既定 `inputs`）の下に展開される前提の spec。
    pub fn spec(plan: &Plan, extract_to: &str) -> CellsSpec {
        let mut api = BTreeMap::new();
        for fs in ["benchfs", "chfs", "gekkofs", "unifyfs"] {
            api.insert(fs.to_string(), api_of(fs).to_string());
        }
        CellsSpec {
            run: format!("{extract_to}/runs/{}", plan.run),
            planned: "planned-cells.json".into(),
            // 記録 file を置かない job（Unknown Job Id 相当）は map に入れない。
            jobs: plan
                .jobs
                .iter()
                .filter(|(_, t)| t.is_some())
                .map(|(j, _)| (j.to_string(), format!("{extract_to}/review/qstat-{j}.txt")))
                .collect(),
            table: "results/cells.csv".into(),
            api,
            tolerance_rel: 0.001,
        }
    }
}

impl synth::Plan {
    /// 試験用: 実 tgz の無い環境で K4 の case を preflight・lock・grade に通せるよう、合成 raw の tgz を
    /// `inputs_src/<name>`（公開入力）と grade-case の隣（評価側の写し）に置き、grade-case を合成 raw に合わせて書き直し、
    /// `cases_root` の case.json の `inputs[0]`（name・sha256）と `grade_case_ref.sha256` を再固定する。
    /// 合成 raw は実 archive（公開入力 v2 `main-v3-n2-43706-public2.tgz`）とは別物なので、name も合成用の名前に替える。
    pub fn install_k4_fixture(
        &self,
        grade_cases_root: &Path,
        cases_root: &Path,
        inputs_src: &Path,
    ) -> Result<(), String> {
        use crate::grader::sha256_file;
        const ID: &str = "k4-cmp4-validity-analysis";
        const NAME: &str = "main-v3-n2-43706.tgz";
        let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
        let raw = tmp.path().join("inputs");
        synth::write(&raw, self)?;
        let tgz = tmp.path().join(NAME);
        crate::asset_assembly::create_tgz(&raw, &tgz)?;
        let sha = sha256_file(&tgz)?;
        let case_dir = grade_cases_root.join("final").join(ID);
        for d in [&case_dir, &inputs_src.to_path_buf()] {
            fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            fs::copy(&tgz, d.join(NAME)).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let spec = synth::spec(self, "inputs");
        let planned_rel = format!("{}/planned-cells.json", spec.run);
        let planned_sha = sha256_file(&tmp.path().join(&planned_rel))?;
        let gc = json!({
            "schema": crate::grader::case::GRADE_CASE_SCHEMA, "id": ID, "split": "final",
            "recompute": {
                "report": "report.md",
                "input_archive": {"path": NAME, "sha256": sha, "extract_to": "inputs"},
                "pinned_raw": {planned_rel: planned_sha},
                "metrics": [],
                "cells": spec,
            }
        });
        let gp = case_dir.join(format!("{ID}.json"));
        fs::write(
            &gp,
            serde_json::to_string_pretty(&gc).map_err(|e| e.to_string())? + "\n",
        )
        .map_err(|e| format!("{}: {e}", gp.display()))?;
        let cp = cases_root.join("final").join(ID).join("case.json");
        let mut c = read_json(&cp)?;
        c["inputs"][0]["name"] = json!(NAME);
        c["inputs"][0]["sha256"] = json!(sha);
        c["grade_case_ref"]["sha256"] = json!(sha256_file(&gp)?);
        fs::write(
            &cp,
            serde_json::to_string_pretty(&c).map_err(|e| e.to_string())? + "\n",
        )
        .map_err(|e| format!("{}: {e}", cp.display()))
    }
}
