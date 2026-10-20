//! 方式ごとに隔離した制御 child の CPU/メモリ生標本と、そこからの再生成（無 LLM・有料 0）。
//!
//! # 測定範囲（両方式で同じ規則）
//! - 対象は runner が方式ごと・再起動ごとに **新規に起動した制御 child（role=control）だけ**。
//!   External（ExtSim と ledger の持ち主）・Observer・試験 harness（runner 自身）・他方式の child は含めない。
//! - 制御 child が子孫 process を持つ場合は両方式共通の規則で含める: CPU は回収済み子孫の `cutime+cstime` を
//!   child の `utime+stime` に足し、メモリは標本時点で生きている子孫の `VmHWM` を child の値に足す。
//!   子孫の pid は標本に残す（本 harness の 2 方式は子孫を作らない。空であることも生データで分かる）。
//! - epoch = 制御 child 1 起動分（再起動ごとに `process_started.incarnation`）。epoch ごとに
//!   `pid / starttime / role / epoch` を `ov_epoch_begin` barrier に残し、測定開始・終了の barrier
//!   （child が stdin の `ack` を待って止まっている間に runner が `/proc/<pid>` を読む）で `start` / `end` 標本を取る。
//!
//! # 集計
//! - CPU は epoch 内の差分（end − start、tick 単位）を epoch 間で合算。
//! - メモリは `VmHWM`（kB）。child は start barrier で `/proc/self/clear_refs` に `5` を書き HWM を現在値へ戻す。
//!   戻せた epoch の end の `VmHWM` は「start barrier 以降の最大」（`window_after_reset`）。戻せなかった epoch は
//!   生涯 HWM（開始前の初期化分を含む。`lifetime_includes_pre_start`）。開始前の生涯 HWM は
//!   `pre_start_hwm_kb` に必ず残す。方式のメモリは epoch の最大。
//! - 標本欠損・end 標本の ack 欠落・starttime 不一致は 0 にせず epoch を不完全とし、方式の CPU/メモリは `None`（表では `NA`）。
//! - `role` が control でない標本、control として起動された記録の無い pid（試験親・External・Observer・他 arm）の標本、
//!   単位（USER_HZ・page size）が header と違う標本は集計せず Err にする。
//! - 旧 in-process raw（`driver_process_inclusive` / `samples.jsonl`）は制御単体と混ぜず Err で拒否する。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, Write},
    path::Path,
    process::Command,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    ext::DATASET,
    metrics::{self, Metrics, append_jsonl},
    process::{EventLog, OwnedProc, read_events},
};

pub const OVERHEAD_SCHEMA: &str = "celeris.dagbench-overhead/1";
pub const OVERHEAD_RUN: &str = "overhead-run.json";
pub const OVERHEAD_SAMPLES: &str = "overhead-samples.jsonl";
pub const OVERHEAD_TSV: &str = "control_isolated_overhead.tsv";
pub const OVERHEAD_JSON: &str = "control_isolated_overhead.json";
pub const SCOPE_LABEL: &str = "control_child_isolated";
pub const SCOPE_RULE: &str = "fresh control child per arm per epoch; excludes external/observer/test harness/other arms; \
descendants: cutime+cstime in CPU, live descendants' VmHWM added to memory (same rule for both arms)";
pub const HWM_RULE: &str = "VmHWM kB; child resets via /proc/self/clear_refs=5 at the start barrier. \
window_after_reset = max since start barrier; lifetime_includes_pre_start = whole life incl. init (pre_start_hwm_kb)";
pub const B_EPOCH_BEGIN: &str = "ov_epoch_begin";
pub const B_SAMPLE_ACKED: &str = "ov_sample_acked";
pub const START_MARK: &str = "OV_START";
pub const END_MARK: &str = "OV_END";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Start,
    End,
}

impl Kind {
    fn mark(self) -> &'static str {
        match self {
            Kind::Start => START_MARK,
            Kind::End => END_MARK,
        }
    }
}

/// 実環境の単位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Units {
    /// USER_HZ（`getconf CLK_TCK`）。
    pub clk_tck: u64,
    /// `getconf PAGESIZE`（byte）。
    pub page_size: u64,
}

fn getconf(name: &str) -> Result<u64, String> {
    let out = Command::new("getconf")
        .arg(name)
        .output()
        .map_err(|e| format!("getconf {name}: {e}"))?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|e| format!("getconf {name}: {e}"))
}

impl Units {
    pub fn host() -> Result<Self, String> {
        Ok(Units {
            clk_tck: getconf("CLK_TCK")?,
            page_size: getconf("PAGESIZE")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvHeader {
    pub schema: String,
    pub dataset: String,
    pub arm: String,
    pub scenario: String,
    pub point: String,
    pub seed: u64,
    pub units: Units,
    pub scope_rule: String,
    pub hwm_rule: String,
}

impl OvHeader {
    pub fn new(arm: &str, scenario: &str, point: &str, seed: u64) -> Result<Self, String> {
        Ok(OvHeader {
            schema: OVERHEAD_SCHEMA.into(),
            dataset: DATASET.into(),
            arm: arm.into(),
            scenario: scenario.into(),
            point: point.into(),
            seed,
            units: Units::host()?,
            scope_rule: SCOPE_RULE.into(),
            hwm_rule: HWM_RULE.into(),
        })
    }

    pub fn write(&self, raw: &Path) -> io::Result<()> {
        fs::write(
            raw.join(OVERHEAD_RUN),
            serde_json::to_string_pretty(self).map_err(io::Error::other)?,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Desc {
    pub pid: u32,
    pub vm_hwm_kb: u64,
}

/// `/proc/<pid>` の生標本 1 件（runner が child の停止中に外側から読む）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvSample {
    pub seq: u64,
    pub dataset: String,
    pub arm: String,
    pub role: String,
    pub epoch: u64,
    pub kind: Kind,
    pub pid: u32,
    pub starttime_ticks: u64,
    pub clk_tck: u64,
    pub page_size: u64,
    pub utime_ticks: u64,
    pub stime_ticks: u64,
    pub cutime_ticks: u64,
    pub cstime_ticks: u64,
    pub rss_pages: u64,
    pub vm_rss_kb: u64,
    pub vm_hwm_kb: u64,
    pub descendants: Vec<Desc>,
    /// start のみ: child が申告した clear_refs の成否と、reset 前の生涯 HWM。
    pub hwm_reset: Option<bool>,
    pub pre_start_hwm_kb: Option<u64>,
}

struct Proc {
    ppid: u32,
    starttime: u64,
    utime: u64,
    stime: u64,
    cutime: u64,
    cstime: u64,
    rss_pages: u64,
    vm_rss_kb: u64,
    vm_hwm_kb: u64,
}

fn read_proc(pid: u32) -> Result<Proc, String> {
    let stat =
        fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| format!("{pid}: {e}"))?;
    let rest = stat
        .rsplit_once(')')
        .map(|(_, r)| r)
        .ok_or("malformed /proc stat")?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    // field n は index n-3（index 0 = state）。
    let num = |i: usize| -> Result<u64, String> {
        f.get(i)
            .ok_or("short /proc stat")?
            .parse::<u64>()
            .map_err(|e| e.to_string())
    };
    let status =
        fs::read_to_string(format!("/proc/{pid}/status")).map_err(|e| format!("{pid}: {e}"))?;
    Ok(Proc {
        ppid: num(1)? as u32,
        starttime: num(19)?,
        utime: num(11)?,
        stime: num(12)?,
        cutime: num(13)?,
        cstime: num(14)?,
        rss_pages: num(21)?,
        vm_rss_kb: status_kb(&status, "VmRSS:"),
        vm_hwm_kb: status_kb(&status, "VmHWM:"),
    })
}

fn status_kb(status: &str, key: &str) -> u64 {
    status
        .lines()
        .find_map(|l| l.strip_prefix(key))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// 生きている子孫（pid と VmHWM）。
fn live_descendants(root: u32) -> Vec<Desc> {
    let mut parent_of: BTreeMap<u32, u32> = BTreeMap::new();
    if let Ok(rd) = fs::read_dir("/proc") {
        for e in rd.flatten() {
            if let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok())
                && let Ok(p) = read_proc(pid)
            {
                parent_of.insert(pid, p.ppid);
            }
        }
    }
    let mut found: BTreeSet<u32> = BTreeSet::new();
    let mut frontier = vec![root];
    while let Some(p) = frontier.pop() {
        for (&c, &pp) in &parent_of {
            if pp == p && found.insert(c) {
                frontier.push(c);
            }
        }
    }
    found
        .into_iter()
        .filter_map(|pid| {
            read_proc(pid).ok().map(|p| Desc {
                pid,
                vm_hwm_kb: p.vm_hwm_kb,
            })
        })
        .collect()
}

// ---- 制御 child の側 ----

/// 制御 child が測定の開始/終了 barrier に着いたことを知らせ、runner の `ack` が来るまで止まる。
/// start では先に HWM を現在値へ戻す（開始前の生涯 HWM は runner へ申告する）。
/// 終了は ack の後にだけ起きるので、終了直前の標本は必ず runner が取って ack 済みになる。
pub fn child_barrier(kind: Kind) -> io::Result<()> {
    let mut line = kind.mark().to_string();
    if kind == Kind::Start {
        let status = fs::read_to_string("/proc/self/status")?;
        let pre = status_kb(&status, "VmHWM:");
        let reset = fs::write("/proc/self/clear_refs", "5").is_ok();
        line.push_str(&format!(" pre={pre} reset={}", u8::from(reset)));
    }
    let mut out = io::stdout();
    writeln!(out, "{line}")?;
    out.flush()?;
    let mut got = String::new();
    io::stdin().lock().read_line(&mut got)?;
    if got.trim() == "ack" {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "barrier {line}: no ack ({got:?})"
        )))
    }
}

// ---- runner の側 ----

/// runner が epoch ごとの barrier・標本・ack を raw dir に残す。
pub struct EpochRecorder<'a> {
    raw: &'a Path,
    log: &'a EventLog,
    arm: String,
    units: Units,
    seq: u64,
}

impl<'a> EpochRecorder<'a> {
    pub fn new(raw: &'a Path, log: &'a EventLog, arm: &str, units: Units) -> Self {
        Self {
            raw,
            log,
            arm: arm.into(),
            units,
            seq: 0,
        }
    }

    /// 起動直後の制御 child の `pid / starttime / role / epoch` を barrier に残す。
    pub fn begin(&self, p: &OwnedProc) -> Result<(), String> {
        let st = read_proc(p.pid())?.starttime;
        self.log
            .barrier(
                B_EPOCH_BEGIN,
                json!({"arm": self.arm, "role": p.role().as_str(), "pid": p.pid(),
                       "starttime_ticks": st, "epoch": p.incarnation()}),
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// child の barrier 行を待ち、child の停止中に標本を取って fsync し、その後 `ack` を送る。
    /// 標本が取れなかった・書けなかった場合は ack せず Err（epoch は不完全のまま残る）。
    pub fn sample_at_barrier(
        &mut self,
        p: &mut OwnedProc,
        kind: Kind,
        timeout: Duration,
    ) -> Result<OvSample, String> {
        let line = loop {
            let l = p
                .next_line(timeout)
                .map_err(|e| format!("waiting for {}: {e}", kind.mark()))?;
            if let Some(i) = l.find(kind.mark()) {
                break l[i..].to_string();
            }
        };
        let field = |k: &str| {
            line.split_whitespace()
                .find_map(|w| w.strip_prefix(k))
                .and_then(|v| v.parse::<u64>().ok())
        };
        let pr = read_proc(p.pid())?;
        self.seq += 1;
        let s = OvSample {
            seq: self.seq,
            dataset: DATASET.into(),
            arm: self.arm.clone(),
            role: p.role().as_str().into(),
            epoch: p.incarnation(),
            kind,
            pid: p.pid(),
            starttime_ticks: pr.starttime,
            clk_tck: self.units.clk_tck,
            page_size: self.units.page_size,
            utime_ticks: pr.utime,
            stime_ticks: pr.stime,
            cutime_ticks: pr.cutime,
            cstime_ticks: pr.cstime,
            rss_pages: pr.rss_pages,
            vm_rss_kb: pr.vm_rss_kb,
            vm_hwm_kb: pr.vm_hwm_kb,
            descendants: live_descendants(p.pid()),
            hwm_reset: (kind == Kind::Start).then(|| field("reset=") == Some(1)),
            pre_start_hwm_kb: (kind == Kind::Start).then(|| field("pre=").unwrap_or(0)),
        };
        append_jsonl(&self.raw.join(OVERHEAD_SAMPLES), &s).map_err(|e| e.to_string())?;
        p.send_line("ack").map_err(|e| e.to_string())?;
        self.log
            .barrier(
                B_SAMPLE_ACKED,
                json!({"arm": self.arm, "pid": s.pid, "epoch": s.epoch, "sample_kind": kind,
                       "sample_seq": s.seq}),
            )
            .map_err(|e| e.to_string())?;
        Ok(s)
    }
}

// ---- 生標本からの再生成 ----

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EpochOverhead {
    pub epoch: u64,
    pub pid: u32,
    pub starttime_ticks: u64,
    pub complete: bool,
    pub reason: Option<String>,
    pub cpu_ticks: Option<u64>,
    pub peak_kb: Option<u64>,
    pub hwm_scope: String,
    pub pre_start_hwm_kb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArmOverhead {
    pub schema: String,
    pub dataset: String,
    pub arm: String,
    pub scenario: String,
    pub point: String,
    pub seed: u64,
    pub units: Units,
    pub epochs: Vec<EpochOverhead>,
    pub complete: bool,
    pub cpu_ticks: Option<u64>,
    pub cpu_ms: Option<u64>,
    pub peak_rss_kb: Option<u64>,
    pub hwm_scope: String,
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

/// 旧 in-process 標本（driver process 全体の CPU/HWM）を制御単体として読まない。
fn reject_legacy(raw: &Path) -> Result<(), String> {
    if raw.join("samples.jsonl").exists() {
        return Err(
            "samples.jsonl is a legacy driver_process_inclusive sample (driver + ext-sim + harness in one process); \
             it is not a control-only sample and is rejected, not mixed in"
                .into(),
        );
    }
    if let Ok(t) = fs::read_to_string(raw.join("run.json"))
        && t.contains("driver_process_inclusive")
    {
        return Err("run.json declares a driver_process_inclusive control scope; rejected as not control-only".into());
    }
    Ok(())
}

/// raw dir の生標本と process-events だけから方式の CPU/メモリを再生成する。
pub fn compute_overhead(raw: &Path) -> Result<ArmOverhead, String> {
    reject_legacy(raw)?;
    let hdr_text =
        fs::read_to_string(raw.join(OVERHEAD_RUN)).map_err(|e| format!("{OVERHEAD_RUN}: {e}"))?;
    let h: OvHeader =
        serde_json::from_str(&hdr_text).map_err(|e| format!("{OVERHEAD_RUN}: {e}"))?;
    if h.schema != OVERHEAD_SCHEMA || h.dataset != DATASET {
        return Err(format!(
            "{OVERHEAD_RUN}: not a {OVERHEAD_SCHEMA}/{DATASET} record"
        ));
    }
    let events = read_events(&raw.join("process-events.jsonl")).map_err(|e| e.to_string())?;
    let samples: Vec<OvSample> = read_jsonl(&raw.join(OVERHEAD_SAMPLES))?;
    let seqof = |e: &Value| e["seq"].as_u64().unwrap_or(0);

    // 制御 child として起動された記録（epoch の登録簿）。
    let mut started: BTreeMap<u32, u64> = BTreeMap::new();
    for e in events
        .iter()
        .filter(|e| e["kind"] == "process_started" && e["role"] == "control")
    {
        started.insert(
            e["pid"].as_u64().unwrap_or(0) as u32,
            e["incarnation"].as_u64().unwrap_or(0),
        );
    }
    let mut begun: BTreeMap<u32, (u64, u64)> = BTreeMap::new();
    for e in events
        .iter()
        .filter(|e| e["kind"] == "barrier" && e["name"] == B_EPOCH_BEGIN)
    {
        if e["arm"] != h.arm.as_str() || e["role"] != "control" {
            return Err(format!(
                "epoch registry entry is not this arm's control: {e}"
            ));
        }
        let pid = e["pid"].as_u64().unwrap_or(0) as u32;
        if started.get(&pid).copied() != e["epoch"].as_u64() {
            return Err(format!(
                "epoch registry pid {pid} was not started as control epoch {}",
                e["epoch"]
            ));
        }
        begun.insert(
            pid,
            (
                e["epoch"].as_u64().unwrap_or(0),
                e["starttime_ticks"].as_u64().unwrap_or(0),
            ),
        );
    }

    for s in &samples {
        if s.dataset != DATASET || s.arm != h.arm {
            return Err(format!(
                "sample {} belongs to another dataset/arm ({}/{})",
                s.seq, s.dataset, s.arm
            ));
        }
        if s.role != "control" {
            return Err(format!(
                "sample {} has role {} (only control is measured)",
                s.seq, s.role
            ));
        }
        if !begun.contains_key(&s.pid) {
            return Err(format!(
                "sample {} is for pid {} which was never started as a control epoch of this arm \
                 (test parent / external / observer / another arm)",
                s.seq, s.pid
            ));
        }
        if (s.clk_tck, s.page_size) != (h.units.clk_tck, h.units.page_size) {
            return Err(format!(
                "sample {} units ({},{}) differ from the header",
                s.seq, s.clk_tck, s.page_size
            ));
        }
        let from_pages = s.rss_pages * s.page_size / 1024;
        if from_pages.abs_diff(s.vm_rss_kb) > 512 {
            return Err(format!(
                "sample {}: rss {} pages x {} B = {} kB disagrees with VmRSS {} kB (unit mismatch)",
                s.seq, s.rss_pages, s.page_size, from_pages, s.vm_rss_kb
            ));
        }
    }

    let mut epochs = Vec::new();
    for (&pid, &(epoch, starttime)) in &begun {
        let pick = |k: Kind| samples.iter().find(|s| s.pid == pid && s.kind == k);
        let (start, end) = (pick(Kind::Start), pick(Kind::End));
        let acked = |s: &OvSample| {
            events
                .iter()
                .find(|e| {
                    e["kind"] == "barrier"
                        && e["name"] == B_SAMPLE_ACKED
                        && e["pid"].as_u64() == Some(u64::from(s.pid))
                        && e["sample_seq"].as_u64() == Some(s.seq)
                })
                .map(seqof)
        };
        let stopped = events
            .iter()
            .find(|e| e["kind"] == "process_stopped" && e["pid"].as_u64() == Some(u64::from(pid)))
            .map(seqof);
        let mut bad: Option<String> = None;
        let (Some(st), Some(en)) = (start, end) else {
            let missing = if start.is_none() { "start" } else { "end" };
            epochs.push(EpochOverhead {
                epoch,
                pid,
                starttime_ticks: starttime,
                complete: false,
                reason: Some(format!("missing {missing} sample")),
                cpu_ticks: None,
                peak_kb: None,
                hwm_scope: "unknown".into(),
                pre_start_hwm_kb: start.and_then(|s| s.pre_start_hwm_kb),
            });
            continue;
        };
        if st.starttime_ticks != starttime
            || en.starttime_ticks != starttime
            || st.epoch != epoch
            || en.epoch != epoch
        {
            bad = Some(
                "pid/starttime/epoch differ between samples (pid reuse or mixed processes)".into(),
            );
        } else if acked(st).is_none() {
            bad = Some("start sample not acked".into());
        } else if let Some(a) = acked(en) {
            if stopped.is_some_and(|s| s < a) {
                bad = Some("end sample acked after the child had already exited".into());
            }
        } else {
            bad = Some("end sample not acked".into());
        }
        let total = |s: &OvSample| s.utime_ticks + s.stime_ticks + s.cutime_ticks + s.cstime_ticks;
        if bad.is_none() && total(en) < total(st) {
            bad = Some("cpu ticks decreased between start and end".into());
        }
        // 戻せたと申告し、かつ start 時点の HWM が現在の RSS に追従している（戻っていない HWM は RSS より大きく残る）。
        let reset = st.hwm_reset == Some(true) && st.vm_hwm_kb <= st.vm_rss_kb + 512;
        let desc = |s: &OvSample| s.descendants.iter().map(|d| d.vm_hwm_kb).sum::<u64>();
        let hwm_scope = if reset {
            "window_after_reset"
        } else {
            "lifetime_includes_pre_start"
        };
        epochs.push(EpochOverhead {
            epoch,
            pid,
            starttime_ticks: starttime,
            complete: bad.is_none(),
            cpu_ticks: bad.is_none().then(|| total(en) - total(st)),
            peak_kb: bad.is_none().then(|| en.vm_hwm_kb + desc(en)),
            reason: bad,
            hwm_scope: hwm_scope.into(),
            pre_start_hwm_kb: st.pre_start_hwm_kb,
        });
    }
    epochs.sort_by_key(|e| e.epoch);
    let complete = !epochs.is_empty() && epochs.iter().all(|e| e.complete);
    let cpu_ticks = complete.then(|| epochs.iter().filter_map(|e| e.cpu_ticks).sum::<u64>());
    let peak = complete.then(|| epochs.iter().filter_map(|e| e.peak_kb).max().unwrap_or(0));
    let scope = if epochs.iter().all(|e| e.hwm_scope == "window_after_reset") {
        "window_after_reset"
    } else {
        "lifetime_includes_pre_start"
    };
    Ok(ArmOverhead {
        schema: OVERHEAD_SCHEMA.into(),
        dataset: DATASET.into(),
        arm: h.arm,
        scenario: h.scenario,
        point: h.point,
        seed: h.seed,
        units: h.units,
        complete,
        cpu_ticks,
        cpu_ms: cpu_ticks.map(|t| t * 1000 / h.units.clk_tck.max(1)),
        peak_rss_kb: peak,
        hwm_scope: scope.into(),
        epochs,
    })
}

// ---- 表 ----

/// 1 run（scenario × point × seed × 方式）の行: makespan・完了量・ready 待ち（`metrics::compute`）と制御 overhead。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub measurement_id: String,
    pub arm: String,
    pub scenario: String,
    pub point: String,
    pub seed: u64,
    pub makespan_max_secs: u64,
    pub completed_nodes: usize,
    pub completed_nodes_expected: usize,
    pub ready_wait_sum_secs: u64,
    pub ready_wait_p95_secs: u64,
    pub overhead: ArmOverhead,
}

/// raw dir 1 つから行を再生成する（`run.json` は makespan 等用の /1 header、overhead は本 module の raw）。
pub fn regenerate(raw: &Path) -> Result<Row, String> {
    let overhead = compute_overhead(raw)?;
    let m: Metrics = metrics::compute(raw)?;
    Ok(Row {
        measurement_id: m.measurement_id,
        arm: m.arm,
        scenario: m.scenario,
        point: m.point,
        seed: m.seed,
        makespan_max_secs: m.makespan.max,
        completed_nodes: m.completed.nodes,
        completed_nodes_expected: m.completed.nodes_expected,
        ready_wait_sum_secs: m.ready_wait.all.sum,
        ready_wait_p95_secs: m.ready_wait.all.p95,
        overhead,
    })
}

fn na(v: Option<u64>) -> String {
    v.map_or("NA".into(), |x| x.to_string())
}

/// 決定的な TSV（行は `measurement_id` 順。欠損は `NA`、0 にしない）。
pub fn render_tsv(rows: &[Row]) -> String {
    let mut rows: Vec<&Row> = rows.iter().collect();
    rows.sort_by(|a, b| a.measurement_id.cmp(&b.measurement_id));
    let mut out = String::from(
        "dataset\tmeasurement_id\tarm\tscenario\tpoint\tseed\tmakespan_max_secs\tcompleted_nodes\t\
completed_nodes_expected\tready_wait_sum_secs\tready_wait_p95_secs\tepochs\toverhead_complete\t\
control_cpu_ticks\tcontrol_cpu_ms\tcontrol_peak_hwm_kb\thwm_scope\tclk_tck\tpage_size\n",
    );
    for r in rows {
        let o = &r.overhead;
        out.push_str(&format!(
            "{DATASET}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            r.measurement_id,
            r.arm,
            r.scenario,
            r.point,
            r.seed,
            r.makespan_max_secs,
            r.completed_nodes,
            r.completed_nodes_expected,
            r.ready_wait_sum_secs,
            r.ready_wait_p95_secs,
            o.epochs.len(),
            o.complete,
            na(o.cpu_ticks),
            na(o.cpu_ms),
            na(o.peak_rss_kb),
            o.hwm_scope,
            o.units.clk_tck,
            o.units.page_size,
        ));
    }
    out
}

pub fn render_json(rows: &[Row]) -> Result<String, String> {
    let mut rows: Vec<&Row> = rows.iter().collect();
    rows.sort_by(|a, b| a.measurement_id.cmp(&b.measurement_id));
    let mut s = serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?;
    s.push('\n');
    Ok(s)
}
