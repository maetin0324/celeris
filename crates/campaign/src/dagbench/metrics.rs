//! 生ログからの指標再生成（`docs/ops/dag-fault-comparison.md`）。
//!
//! 入力は raw dir の JSONL だけ。系の自己申告の集計値は読まない（R10 の原則）。
//! - `run.json`: 測定の入力の写し（scenario の DAG・点・seed・枠。結果ではない）。
//! - `ext.jsonl`: 外部 ledger（ext-sim が書く。論理/物理 ID・受理・起動・完了・消費・照会・fault 発火）。
//! - `worker.jsonl`: 台本 worker / 実行器の開始・終了の記録（制御側から独立に追記）。
//! - `control.jsonl`: 制御 process の再起動・再開・手動介入（driver が書く）。無ければ空。
//! - `samples.jsonl`: `/proc/<pid>` の CPU tick と RSS の標本（読むだけ。負荷は掛けない）。無ければ空。
//!
//! 出力表は `dataset: control_measured` を持ち、`TrialRecord` / `final_accounting`（`llm_campaign`）とは
//! 別 file・別 schema。成功率の欄は持たない（stub の成功を能力として混ぜない）。正常/障害は同じ研究問題の条件違いで、
//! 問題数としては数えない（`condition` の文字列にだけ現れる）。出力に時刻・乱数は入れず、同じ raw からは同じ bytes になる。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    celeris_side::{RunSpec, ScenarioFile, WorkerEntry, read_worker_ledger},
    ext::{DATASET, Entry, read_ledger},
};

/// Formal A2: RunHeader plus mandatory persisted-wait and process survival proof.
pub const DURABLE_RAW_SCHEMA: &str = "celeris.dagbench-durable-raw/1";

pub const RAW_SCHEMA: &str = "celeris.dagbench-raw/1";
/// 独立 process 版（制御 child を kill しても外部 process が完了する run）の raw 版。旧 in-process の
/// [`RAW_SCHEMA`]（`/1`）とは別物で、`compute`（/1 専用）は /2 を、`dagbench::process::compute_process`（/2 専用）は /1 を
/// 明示的に拒否する。
pub const PROCESS_RAW_SCHEMA: &str = "celeris.dagbench-raw/2";
pub const METRICS_SCHEMA: &str = "celeris.dagbench-metrics/1";
pub const METRICS_FILE: &str = "control_measured_metrics.json";
pub const METRICS_TSV: &str = "control_measured_metrics.tsv";
/// Linux の USER_HZ（`/proc/<pid>/stat` の tick 単位）。標本にも書く。
const CLK_TCK: u64 = 100;

// ---- raw 形式 ----

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHdr {
    pub id: String,
    pub deps: Vec<String>,
    pub work_secs: u64,
    /// 外部 node の (論理 job ID, 後処理の作業秒)。
    pub external: Option<(String, u64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunHeader {
    pub schema: String,
    pub dataset: String,
    pub measurement_id: String,
    pub arm: String,
    pub scenario: String,
    pub point: String,
    pub seed: u64,
    pub poll_secs: u64,
    pub ext_job_secs: u64,
    pub worker_slots: usize,
    pub external_job_slots: usize,
    pub max_logical_secs: u64,
    pub llm_calls: u64,
    pub paid_jobs: u64,
    /// 制御 CPU/RSS の標本が何の process か（driver を in-process で動かす系では driver process 全体）。
    pub control_scope: String,
    pub campaigns: Vec<(String, u64)>,
    pub nodes: Vec<NodeHdr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grading: Option<Value>,
}

impl RunHeader {
    pub fn from_spec(spec: &RunSpec, arm: &str, control_scope: &str) -> Self {
        RunHeader {
            grading: spec
                .grading
                .as_ref()
                .map(|g| g.pin().expect("read grading pins")),
            schema: RAW_SCHEMA.into(),
            dataset: DATASET.into(),
            measurement_id: spec
                .measurement_id()
                .replace("-celeris", &format!("-{arm}")),
            arm: arm.into(),
            scenario: spec.scenario.id.clone(),
            point: spec.point.id.clone(),
            seed: spec.seed,
            poll_secs: spec.poll_secs,
            ext_job_secs: spec.ext_job_secs,
            worker_slots: spec.scenario.resources.worker_slots,
            external_job_slots: spec.scenario.resources.external_job_slots,
            max_logical_secs: spec.caps.max_logical_secs,
            llm_calls: 0,
            paid_jobs: 0,
            control_scope: control_scope.into(),
            campaigns: spec
                .campaign_ids()
                .into_iter()
                .enumerate()
                .map(|(i, c)| (c, spec.arrival(i)))
                .collect(),
            nodes: spec
                .scenario
                .nodes
                .iter()
                .map(|n| NodeHdr {
                    id: n.id.clone(),
                    deps: n.deps.clone(),
                    work_secs: n.work_secs,
                    external: n
                        .external
                        .as_ref()
                        .map(|e| (e.logical_job_id.clone(), e.post_wait_work_secs)),
                })
                .collect(),
        }
    }

    pub fn write(&self, raw: &Path) -> std::io::Result<()> {
        fs::create_dir_all(raw)?;
        if self.grading.is_some() && raw.join("run.json").exists() {
            let old: Value = serde_json::from_slice(&fs::read(raw.join("run.json"))?)
                .map_err(std::io::Error::other)?;
            if old != serde_json::to_value(self).map_err(std::io::Error::other)? {
                return Err(std::io::Error::other(
                    "graded run pins changed; use a fresh output directory",
                ));
            }
            return Ok(());
        }
        fs::write(
            raw.join("run.json"),
            serde_json::to_string_pretty(self).map_err(std::io::Error::other)?,
        )
    }
}

/// 制御 process の出来事（再起動・再開・手動介入）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlEntry {
    pub seq: u64,
    pub logical_secs: u64,
    #[serde(default)]
    pub wall_ms: Option<u64>,
    /// `control_restart` / `control_resumed` / `intervention`。
    pub event: String,
    pub dataset: String,
    #[serde(default)]
    pub intervention_id: Option<String>,
}

/// `/proc/<pid>` の標本 1 件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub seq: u64,
    pub wall_ms: u64,
    pub dataset: String,
    pub role: String,
    pub pid: u32,
    pub clk_tck: u64,
    pub utime_ticks: u64,
    pub stime_ticks: u64,
    pub vm_rss_kb: u64,
    pub vm_hwm_kb: u64,
}

/// `/proc/<pid>/stat` と `status` を読んで標本にする（読むだけ。sleep も負荷も無い）。
pub fn read_proc_sample(pid: u32, role: &str, wall_ms: u64, seq: u64) -> Result<Sample, String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
    // comm は括弧の中に空白を含み得る。最後の ')' の後ろが state 以降の欄。
    let rest = stat
        .rsplit_once(')')
        .map(|(_, r)| r)
        .ok_or("malformed /proc stat")?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    // rest[0] = state(3 番目) なので utime(14) は index 11、stime(15) は 12。
    let num = |i: usize| -> Result<u64, String> {
        f.get(i)
            .ok_or("short /proc stat")?
            .parse::<u64>()
            .map_err(|e| e.to_string())
    };
    let status = fs::read_to_string(format!("/proc/{pid}/status")).map_err(|e| e.to_string())?;
    let kb = |key: &str| -> u64 {
        status
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    Ok(Sample {
        seq,
        wall_ms,
        dataset: DATASET.into(),
        role: role.into(),
        pid,
        clk_tck: CLK_TCK,
        utime_ticks: num(11)?,
        stime_ticks: num(12)?,
        vm_rss_kb: kb("VmRSS:"),
        vm_hwm_kb: kb("VmHWM:"),
    })
}

/// JSONL に 1 行追記する（fsync 付き）。
pub fn append_jsonl<T: Serialize>(path: &Path, v: &T) -> std::io::Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let mut line = serde_json::to_string(v).map_err(std::io::Error::other)?;
    line.push('\n');
    f.write_all(line.as_bytes())?;
    f.sync_data()
}

fn line_count(path: &Path) -> u64 {
    fs::read_to_string(path)
        .map(|t| t.lines().count() as u64)
        .unwrap_or(0)
}

/// 制御 process の標本を `samples.jsonl` に足す。
pub fn append_sample(raw: &Path, pid: u32, wall_ms: u64) -> Result<(), String> {
    let p = raw.join("samples.jsonl");
    let s = read_proc_sample(pid, "control", wall_ms, line_count(&p) + 1)?;
    append_jsonl(&p, &s).map_err(|e| e.to_string())
}

/// `control.jsonl` に出来事を足す。
pub fn append_control(
    raw: &Path,
    event: &str,
    logical_secs: u64,
    wall_ms: Option<u64>,
    intervention_id: Option<&str>,
) -> std::io::Result<()> {
    let p = raw.join("control.jsonl");
    append_jsonl(
        &p,
        &ControlEntry {
            seq: line_count(&p) + 1,
            logical_secs,
            wall_ms,
            event: event.into(),
            dataset: DATASET.into(),
            intervention_id: intervention_id.map(str::to_string),
        },
    )
}

/// campaign の投入を worker ledger に足す（baseline の投入。Celeris 側は `CelerisSide` が書く）。
pub fn append_campaign_submitted(raw: &Path, campaign: &str, at: u64) -> std::io::Result<()> {
    let p = raw.join("worker.jsonl");
    append_jsonl(
        &p,
        &WorkerEntry {
            seq: line_count(&p) + 1,
            logical_secs: at,
            event: "campaign_submitted".into(),
            dataset: DATASET.into(),
            campaign: campaign.into(),
            node: String::new(),
            detail: None,
        },
    )
}

fn read_jsonl<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<Vec<T>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    fs::read_to_string(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("{}: {e}", path.display())))
        .collect()
}

fn sha256_file(path: &Path) -> String {
    match fs::read(path) {
        Ok(b) => Sha256::digest(&b)
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect(),
        Err(_) => "absent".into(),
    }
}

// ---- 指標 ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Stat {
    pub n: usize,
    pub sum: u64,
    pub p50: u64,
    pub p95: u64,
    pub max: u64,
}

/// 最近順位法（整数のまま。浮動小数を使わない）。
fn stat(mut v: Vec<u64>) -> Stat {
    v.sort_unstable();
    let rank = |p: usize| -> u64 {
        if v.is_empty() {
            return 0;
        }
        v[(p * v.len()).div_ceil(100).max(1) - 1]
    };
    Stat {
        n: v.len(),
        sum: v.iter().sum(),
        p50: rank(50),
        p95: rank(95),
        max: v.last().copied().unwrap_or(0),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Metrics {
    pub schema: String,
    /// 実 LLM キャンペーンの記録（`llm_campaign`）とは別の dataset。
    pub dataset: String,
    pub measurement_id: String,
    pub arm: String,
    pub scenario: String,
    pub point: String,
    pub seed: u64,
    /// `normal` または `fault:<fault_id>[,…]`。研究問題の数ではなく条件のラベル。
    pub condition: String,
    pub llm_calls: u64,
    pub paid_jobs: u64,
    pub raw_sha256: BTreeMap<String, String>,
    pub makespan: Makespan,
    pub completed: Completed,
    pub ready_wait: ReadyWait,
    pub control_overhead: Overhead,
    pub duplicates: Duplicates,
    pub reconciliation: Reconciliation,
    pub recovery: Recovery,
    pub lost_progress: LostProgress,
    pub interventions: u64,
    /// Sum of execution intervals across worker slots, including failed attempts.
    #[serde(default)]
    pub execution_busy_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Makespan {
    /// campaign ごと（投入受理 → 最後の node 完了、論理秒）。完了しなかった campaign は含めない。
    pub per_campaign: BTreeMap<String, u64>,
    pub max: u64,
    pub median: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Completed {
    pub nodes: usize,
    pub nodes_expected: usize,
    pub campaigns: usize,
    pub campaigns_expected: usize,
    /// 完了 node の work_secs（外部は後処理を足す）と外部 job の job_secs の和。
    pub work_secs: u64,
    pub job_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadyWait {
    pub all: Stat,
    /// 待ちが正の node のうち、ready_at に worker 枠が埋まっていた（枠待ち）。
    pub slot_wait_nodes: usize,
    pub slot_wait_secs: u64,
    /// ready_at に枠が空いていたのに始まらなかった（制御遅れ）。
    pub control_delay_nodes: usize,
    pub control_delay_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Overhead {
    pub scope: String,
    pub sample_count: usize,
    pub cpu_ms: u64,
    pub peak_rss_kb: u64,
    /// 完了 node あたり（切り捨て）。標本が無い・完了 0 なら null。
    pub cpu_ms_per_completed_node: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Duplicates {
    /// Σ_論理 job (物理起動 − 1)。
    pub extra_physical_launches: usize,
    /// ledger の消費記録が論理 job あたり 2 以上の分と、制御側が数えた初回消費が node あたり 2 以上の分。
    pub double_counted: usize,
    /// 参考: ext-sim が送った重複通知（copy ≥ 1）の数。制御側が無視できたかは double_counted が示す。
    pub duplicate_notices_sent: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reconciliation {
    pub disturbed: bool,
    /// 障害後の ext への照会（論理 ID lookup）回数と結果別の内訳。
    pub lookups_after_disturbance: usize,
    pub by_result: BTreeMap<String, usize>,
    pub deduped_submits: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Recovery {
    pub disturbance_logical_secs: Option<u64>,
    pub in_flight_at_disturbance: usize,
    /// 障害前の in-flight が全部再確定するまでの論理秒。未再確定があれば null。
    pub logical_secs: Option<u64>,
    pub wall_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LostProgress {
    pub rerun_node_starts: usize,
    pub rerun_work_secs: u64,
    pub extra_physical_launches: usize,
    pub restarted_job_secs: u64,
    pub total_secs: u64,
}

/// `run.json` の本文から `schema` 欄だけを取り出す（無い・文字列でないは Err）。
pub fn raw_schema_of(run_json: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(run_json).map_err(|e| format!("run.json: {e}"))?;
    v["schema"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "run.json: no schema".to_string())
}

/// raw dir の生ログから共通指標を計算する。A2 は保存・独立processの証明を必須検証する。
pub fn compute(raw: &Path) -> Result<Metrics, String> {
    let run_json =
        fs::read_to_string(raw.join("run.json")).map_err(|e| format!("run.json: {e}"))?;
    // header の形を読む前に版を見る（別の版の欄で壊れた parse error に埋もれさせない）。
    let schema = raw_schema_of(&run_json)?;
    if schema == super::pbs::raw::SCHEDULER_RAW_SCHEMA {
        return Err(format!(
            "{schema} is a real-scheduler raw (wall clock); `compute` does not read it — \
             use dagbench::pbs::raw::validate"
        ));
    }
    if schema == PROCESS_RAW_SCHEMA {
        return Err(format!(
            "{PROCESS_RAW_SCHEMA} is a process raw (separate external process); \
             `compute` reads only {RAW_SCHEMA} (in-process) — use dagbench::process::compute_process"
        ));
    }
    let h: RunHeader = serde_json::from_str(&run_json).map_err(|e| format!("run.json: {e}"))?;
    if (h.schema != RAW_SCHEMA && h.schema != DURABLE_RAW_SCHEMA) || h.dataset != DATASET {
        return Err(format!(
            "raw dir is not a {DATASET} dagbench record ({} / {})",
            h.schema, h.dataset
        ));
    }
    if h.schema == DURABLE_RAW_SCHEMA {
        super::durable::verify(raw)?;
    }
    let workers = read_worker_ledger(&raw.join("worker.jsonl")).map_err(|e| e.to_string())?;
    let ext: Vec<Entry> = read_ledger(&raw.join("ext.jsonl")).map_err(|e| e.to_string())?;
    let control: Vec<ControlEntry> = read_jsonl(&raw.join("control.jsonl"))?;
    let samples: Vec<Sample> = read_jsonl(&raw.join("samples.jsonl"))?;
    let bad = workers.iter().any(|e| e.dataset != DATASET)
        || ext.iter().any(|e| e.dataset != DATASET)
        || control.iter().any(|e| e.dataset != DATASET)
        || samples.iter().any(|e| e.dataset != DATASET);
    if bad {
        return Err(format!("raw log contains a record that is not {DATASET}"));
    }
    let mut workers = workers;
    workers.sort_by_key(|e| e.seq);

    // --- worker 側: 投入・開始・終了 ---
    let key = |e: &WorkerEntry| (e.campaign.clone(), e.node.clone());
    let mut submit_at: BTreeMap<String, u64> = h.campaigns.iter().cloned().collect();
    let mut first_start: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut first_finish: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut last_finish: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut starts: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut post_starts: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut consumes: BTreeMap<(String, String), usize> = BTreeMap::new();
    // 枠を占有した区間。
    let mut first_start_seq: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut open: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut intervals: Vec<((String, String), u64, u64)> = Vec::new();
    for e in &workers {
        let k = key(e);
        match e.event.as_str() {
            "campaign_submitted" => {
                submit_at.insert(e.campaign.clone(), e.logical_secs);
            }
            "node_started" | "post_wait_started" => {
                if let Some(s) = open.remove(&k) {
                    intervals.push((k.clone(), s, e.logical_secs));
                }
                open.insert(k.clone(), e.logical_secs);
                if e.event == "node_started" {
                    first_start.entry(k.clone()).or_insert(e.logical_secs);
                    first_start_seq.entry(k.clone()).or_insert(e.seq);
                    *starts.entry(k).or_default() += 1;
                } else {
                    *post_starts.entry(k).or_default() += 1;
                }
            }
            "external_wait_started" => {
                if let Some(s) = open.remove(&k) {
                    intervals.push((k, s, e.logical_secs));
                }
            }
            "execution_finished" => {
                if let Some(s) = open.remove(&k) {
                    intervals.push((k, s, e.logical_secs));
                }
            }
            event
                if (h.grading.is_some() && event == "validated_completed")
                    || (h.grading.is_none() && event == "node_finished") =>
            {
                if let Some(s) = open.remove(&k) {
                    intervals.push((k.clone(), s, e.logical_secs));
                }
                first_finish.entry(k.clone()).or_insert(e.logical_secs);
                last_finish.insert(k, e.logical_secs);
            }
            "consumed" => {
                let first = e
                    .detail
                    .as_ref()
                    .and_then(|d| d["first"].as_bool())
                    .unwrap_or(true);
                if first {
                    *consumes.entry(k).or_default() += 1;
                }
            }
            _ => {}
        }
    }
    for (k, s) in open {
        intervals.push((k, s, u64::MAX));
    }

    // --- makespan・完了量 ---
    let cap = h.max_logical_secs;
    let done = |c: &str, n: &str| {
        last_finish
            .get(&(c.to_string(), n.to_string()))
            .is_some_and(|t| *t <= cap)
    };
    let mut per_campaign = BTreeMap::new();
    let (mut nodes_done, mut work_secs, mut job_secs, mut camps_done) =
        (0usize, 0u64, 0u64, 0usize);
    for (c, _) in &h.campaigns {
        let mut all = true;
        let mut end = 0;
        for n in &h.nodes {
            if done(c, &n.id) {
                nodes_done += 1;
                work_secs += n.work_secs + n.external.as_ref().map_or(0, |x| x.1);
                if n.external.is_some() {
                    job_secs += h.ext_job_secs;
                }
                end = end.max(last_finish[&(c.clone(), n.id.clone())]);
            } else {
                all = false;
            }
        }
        if all {
            camps_done += 1;
            per_campaign.insert(
                c.clone(),
                end.saturating_sub(submit_at.get(c).copied().unwrap_or(0)),
            );
        }
    }
    let mut ms: Vec<u64> = per_campaign.values().copied().collect();
    ms.sort_unstable();
    let makespan = Makespan {
        max: ms.last().copied().unwrap_or(0),
        median: if ms.is_empty() {
            0
        } else {
            ms[(ms.len() - 1) / 2]
        },
        per_campaign,
    };

    // --- ready 待ち（依存辺と完了時刻から独立に ready_at を出す） ---
    let mut waits = Vec::new();
    let (mut sw_n, mut sw_s, mut cd_n, mut cd_s) = (0usize, 0u64, 0usize, 0u64);
    for (c, _) in &h.campaigns {
        for n in &h.nodes {
            let k = (c.clone(), n.id.clone());
            let Some(&start) = first_start.get(&k) else {
                continue;
            };
            let dep_ready: Option<u64> = n
                .deps
                .iter()
                .map(|d| first_finish.get(&(c.clone(), d.clone())).copied())
                .collect::<Option<Vec<_>>>()
                .map(|v| v.into_iter().max().unwrap_or(0));
            let Some(dep_ready) = dep_ready else {
                continue;
            };
            let ready = dep_ready.max(submit_at.get(c).copied().unwrap_or(0));
            let wait = start.saturating_sub(ready);
            waits.push(wait);
            if wait > 0 {
                let busy = intervals
                    .iter()
                    .filter(|(ik, s, e)| *ik != k && *s <= ready && ready < *e)
                    .count();
                if busy >= h.worker_slots {
                    sw_n += 1;
                    sw_s += wait;
                } else {
                    cd_n += 1;
                    cd_s += wait;
                }
            }
        }
    }
    let ready_wait = ReadyWait {
        all: stat(waits),
        slot_wait_nodes: sw_n,
        slot_wait_secs: sw_s,
        control_delay_nodes: cd_n,
        control_delay_secs: cd_s,
    };

    // --- 制御 CPU・RSS ---
    let mut by_pid: BTreeMap<u32, Vec<&Sample>> = BTreeMap::new();
    for s in samples.iter().filter(|s| s.role == "control") {
        by_pid.entry(s.pid).or_default().push(s);
    }
    let mut cpu_ms = 0u64;
    for v in by_pid.values() {
        let tick = |s: &&Sample| s.utime_ticks + s.stime_ticks;
        let (lo, hi) = (
            v.iter().map(tick).min().unwrap_or(0),
            v.iter().map(tick).max().unwrap_or(0),
        );
        let hz = v.first().map_or(CLK_TCK, |s| s.clk_tck.max(1));
        cpu_ms += (hi - lo) * 1000 / hz;
    }
    let control_overhead = Overhead {
        scope: h.control_scope.clone(),
        sample_count: samples.len(),
        cpu_ms,
        peak_rss_kb: samples.iter().map(|s| s.vm_hwm_kb).max().unwrap_or(0),
        cpu_ms_per_completed_node: (!samples.is_empty() && nodes_done > 0)
            .then(|| cpu_ms / nodes_done as u64),
    };

    // --- 重複・二重計上 ---
    let mut launches: BTreeMap<String, usize> = BTreeMap::new();
    let mut launch_secs: BTreeMap<String, u64> = BTreeMap::new();
    let mut ledger_consumed: BTreeMap<String, usize> = BTreeMap::new();
    for e in &ext {
        let Some(l) = &e.logical_job_id else { continue };
        match e.event.as_str() {
            "physical_launch" => {
                *launches.entry(l.clone()).or_default() += 1;
                *launch_secs.entry(l.clone()).or_default() += e
                    .detail
                    .as_ref()
                    .and_then(|d| d["job_secs"].as_u64())
                    .unwrap_or(h.ext_job_secs);
            }
            "consumed" => *ledger_consumed.entry(l.clone()).or_default() += 1,
            _ => {}
        }
    }
    let extra_launches: usize = launches.values().map(|n| n - 1).sum();
    let restarted_job_secs: u64 = launches
        .iter()
        .map(|(l, n)| (*n as u64 - 1) * launch_secs[l] / *n as u64)
        .sum();
    let double_counted = ledger_consumed
        .values()
        .map(|n| n.saturating_sub(1))
        .sum::<usize>()
        + consumes
            .values()
            .map(|n| n.saturating_sub(1))
            .sum::<usize>();
    let duplicates = Duplicates {
        extra_physical_launches: extra_launches,
        double_counted,
        duplicate_notices_sent: ext
            .iter()
            .filter(|e| {
                e.event == "notice_sent"
                    && e.detail
                        .as_ref()
                        .and_then(|d| d["copy"].as_u64())
                        .unwrap_or(0)
                        >= 1
            })
            .count(),
    };

    // --- 障害・再照合・復旧 ---
    let fired: Vec<&Entry> = ext.iter().filter(|e| e.event == "fault_fired").collect();
    let restarts: Vec<&ControlEntry> = control
        .iter()
        .filter(|e| e.event == "control_restart")
        .collect();
    let t0 = fired
        .iter()
        .map(|e| e.logical_secs)
        .chain(restarts.iter().map(|e| e.logical_secs))
        .min();
    let mut faults: BTreeSet<String> = fired
        .iter()
        .filter_map(|e| {
            e.detail
                .as_ref()
                .and_then(|d| d["fault_id"].as_str())
                .map(str::to_string)
        })
        .collect();
    if h.schema == DURABLE_RAW_SCHEMA {
        faults.insert("a2-restart-after-wait".into());
    }
    if !restarts.is_empty() {
        faults.insert("control_restart".into());
    }
    let condition = if faults.is_empty() {
        "normal".to_string()
    } else {
        format!("fault:{}", faults.into_iter().collect::<Vec<_>>().join(","))
    };
    let mut by_result: BTreeMap<String, usize> = BTreeMap::new();
    let mut lookups = 0;
    if let Some(t0) = t0 {
        for e in ext
            .iter()
            .filter(|e| e.event == "lookup" && e.logical_secs >= t0)
        {
            lookups += 1;
            let r = e
                .detail
                .as_ref()
                .and_then(|d| d["result"].as_str())
                .unwrap_or("unknown");
            *by_result.entry(r.to_string()).or_default() += 1;
        }
    }
    let reconciliation = Reconciliation {
        disturbed: t0.is_some(),
        lookups_after_disturbance: lookups,
        by_result,
        deduped_submits: ext.iter().filter(|e| e.event == "submit_deduped").count(),
    };

    let recovery = match t0 {
        None => Recovery {
            disturbance_logical_secs: None,
            in_flight_at_disturbance: 0,
            logical_secs: Some(0),
            wall_ms: None,
        },
        Some(t0) => {
            // 障害時点で in-flight: 開始済みで終了していない worker 作業と、投入済みで消費前の外部 job。
            let mut inflight_nodes: BTreeSet<(String, String)> = BTreeSet::new();
            let mut inflight_jobs: BTreeSet<String> = BTreeSet::new();
            for e in workers.iter().filter(|e| e.logical_secs <= t0) {
                let k = key(e);
                match e.event.as_str() {
                    "node_started" | "post_wait_started" => {
                        inflight_nodes.insert(k);
                    }
                    event
                        if (h.grading.is_some() && event == "validated_completed")
                            || (h.grading.is_none() && event == "node_finished") =>
                    {
                        inflight_nodes.remove(&k);
                    }
                    _ => {}
                }
            }
            for e in ext.iter().filter(|e| e.logical_secs <= t0) {
                let Some(l) = &e.logical_job_id else { continue };
                match e.event.as_str() {
                    "physical_launch" => {
                        inflight_jobs.insert(l.clone());
                    }
                    "consumed" => {
                        inflight_jobs.remove(l);
                    }
                    _ => {}
                }
            }
            let mut latest = t0;
            let mut ok = true;
            for (c, n) in &inflight_nodes {
                let t = workers
                    .iter()
                    .filter(|e| e.campaign == *c && e.node == *n && e.logical_secs >= t0)
                    .filter(|e| {
                        matches!(
                            e.event.as_str(),
                            "node_started"
                                | "node_finished"
                                | "external_wait_started"
                                | "post_wait_started"
                        )
                    })
                    // 最初の開始そのものは再確定ではない（やり直しの開始だけ数える）。
                    .filter(|e| {
                        e.event != "node_started" || first_start_seq.get(&key(e)) != Some(&e.seq)
                    })
                    .map(|e| e.logical_secs)
                    .min();
                match t {
                    Some(t) => latest = latest.max(t),
                    None => ok = false,
                }
            }
            for l in &inflight_jobs {
                let t = ext
                    .iter()
                    .filter(|e| e.logical_job_id.as_deref() == Some(l) && e.logical_secs >= t0)
                    .filter(|e| {
                        matches!(
                            e.event.as_str(),
                            "lookup" | "submit_deduped" | "external_job_completed" | "consumed"
                        )
                    })
                    .map(|e| e.logical_secs)
                    .min();
                match t {
                    Some(t) => latest = latest.max(t),
                    None => ok = false,
                }
            }
            let wall = restarts.iter().find_map(|r| r.wall_ms).and_then(|a| {
                control
                    .iter()
                    .filter(|e| e.event == "control_resumed")
                    .find_map(|e| e.wall_ms)
                    .map(|b| b.saturating_sub(a))
            });
            Recovery {
                disturbance_logical_secs: Some(t0),
                in_flight_at_disturbance: inflight_nodes.len() + inflight_jobs.len(),
                logical_secs: ok.then(|| latest - t0),
                wall_ms: wall,
            }
        }
    };

    // --- 失われた進捗 ---
    let mut rerun_starts = 0usize;
    let mut rerun_secs = 0u64;
    for (c, _) in &h.campaigns {
        for n in &h.nodes {
            let k = (c.clone(), n.id.clone());
            let extra = starts.get(&k).copied().unwrap_or(0).saturating_sub(1);
            rerun_starts += extra;
            rerun_secs += extra as u64 * n.work_secs;
            let extra_post = post_starts.get(&k).copied().unwrap_or(0).saturating_sub(1);
            rerun_starts += extra_post;
            rerun_secs += extra_post as u64 * n.external.as_ref().map_or(0, |x| x.1);
        }
    }
    let lost_progress = LostProgress {
        rerun_node_starts: rerun_starts,
        rerun_work_secs: rerun_secs,
        extra_physical_launches: extra_launches,
        restarted_job_secs,
        total_secs: rerun_secs + restarted_job_secs,
    };

    // --- 介入（intervention_id で 1 件） ---
    let interventions = control
        .iter()
        .filter(|e| e.event == "intervention")
        .map(|e| {
            e.intervention_id
                .clone()
                .unwrap_or_else(|| format!("seq-{}", e.seq))
        })
        .collect::<BTreeSet<_>>()
        .len() as u64;

    let mut raw_files = vec![
        "run.json",
        "ext.jsonl",
        "worker.jsonl",
        "control.jsonl",
        "samples.jsonl",
    ];
    if h.schema == DURABLE_RAW_SCHEMA {
        raw_files.extend([
            "durable-wait.json",
            "process-events.jsonl",
            "controller-outcome.json",
        ]);
    }
    let raw_sha256 = raw_files
        .iter()
        .map(|f| (f.to_string(), sha256_file(&raw.join(f))))
        .collect();
    let observed_end = workers
        .iter()
        .map(|e| e.logical_secs)
        .chain(ext.iter().map(|e| e.logical_secs))
        .max()
        .unwrap_or(0)
        .min(cap);
    let execution_busy_secs = intervals
        .iter()
        .map(|(_, start, end)| end.min(&observed_end).saturating_sub(*start))
        .sum();
    Ok(Metrics {
        execution_busy_secs,
        schema: if h.grading.is_some() {
            "celeris.dagbench-metrics/2"
        } else {
            METRICS_SCHEMA
        }
        .into(),
        dataset: DATASET.into(),
        measurement_id: h.measurement_id.clone(),
        arm: h.arm.clone(),
        scenario: h.scenario.clone(),
        point: h.point.clone(),
        seed: h.seed,
        condition,
        llm_calls: h.llm_calls,
        paid_jobs: h.paid_jobs,
        raw_sha256,
        makespan,
        completed: Completed {
            nodes: nodes_done,
            nodes_expected: h.nodes.len() * h.campaigns.len(),
            campaigns: camps_done,
            campaigns_expected: h.campaigns.len(),
            work_secs,
            job_secs,
        },
        ready_wait,
        control_overhead,
        duplicates,
        reconciliation,
        recovery,
        lost_progress,
        interventions,
    })
}

/// 指標表を `out` に書く（JSON と 1 行 TSV。LLM 成功率の表とは別 file・別 schema）。
pub fn write_tables(out: &Path, m: &Metrics) -> std::io::Result<()> {
    fs::create_dir_all(out)?;
    let mut json = serde_json::to_string_pretty(m).map_err(std::io::Error::other)?;
    json.push('\n');
    fs::write(out.join(METRICS_FILE), json)?;
    let cols: Vec<(&str, Value)> = vec![
        ("dataset", json!(m.dataset)),
        ("measurement_id", json!(m.measurement_id)),
        ("condition", json!(m.condition)),
        ("makespan_max_secs", json!(m.makespan.max)),
        ("makespan_median_secs", json!(m.makespan.median)),
        ("execution_busy_secs", json!(m.execution_busy_secs)),
        ("completed_nodes", json!(m.completed.nodes)),
        ("completed_campaigns", json!(m.completed.campaigns)),
        ("ready_wait_sum_secs", json!(m.ready_wait.all.sum)),
        ("ready_wait_p50_secs", json!(m.ready_wait.all.p50)),
        ("ready_wait_p95_secs", json!(m.ready_wait.all.p95)),
        ("control_delay_secs", json!(m.ready_wait.control_delay_secs)),
        ("control_cpu_ms", json!(m.control_overhead.cpu_ms)),
        ("control_peak_rss_kb", json!(m.control_overhead.peak_rss_kb)),
        (
            "extra_physical_launches",
            json!(m.duplicates.extra_physical_launches),
        ),
        ("double_counted", json!(m.duplicates.double_counted)),
        (
            "lookups_after_disturbance",
            json!(m.reconciliation.lookups_after_disturbance),
        ),
        ("recovery_logical_secs", json!(m.recovery.logical_secs)),
        ("lost_progress_secs", json!(m.lost_progress.total_secs)),
        ("interventions", json!(m.interventions)),
    ];
    let cell = |v: &Value| match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        o => o.to_string(),
    };
    let mut tsv = cols.iter().map(|c| c.0).collect::<Vec<_>>().join("\t");
    tsv.push('\n');
    tsv.push_str(
        &cols
            .iter()
            .map(|c| cell(&c.1))
            .collect::<Vec<_>>()
            .join("\t"),
    );
    tsv.push('\n');
    File::create(out.join(METRICS_TSV))?.write_all(tsv.as_bytes())
}

// ---- 固定点の掃引（提供のみ。本 unit では実行しない） ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepItem {
    pub scenario: String,
    pub point: String,
    pub seed: u64,
    pub arm: String,
    pub measurement_id: String,
}

/// scenarios.json の事前固定点（one-factor-at-a-time、scenario ごとに 7 点）× seed × 系の一覧を返す。
/// 全軸の総当たりはしない。実行はしない（呼び出し側が各項を `dagbench run` する）。
pub fn sweep_items(file: &ScenarioFile, seeds: &[u64]) -> Vec<SweepItem> {
    let mut out = Vec::new();
    for sc in &file.scenarios {
        for p in &file.fixed_points.points {
            for &seed in seeds {
                for arm in ["baseline", "celeris"] {
                    out.push(SweepItem {
                        scenario: sc.id.clone(),
                        point: p.id.clone(),
                        seed,
                        arm: arm.into(),
                        measurement_id: format!(
                            "dagbench-{}-{}-{}-{seed}-{arm}",
                            file.schema_version, sc.id, p.id
                        ),
                    });
                }
            }
        }
    }
    out
}
