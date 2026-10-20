//! 方式ごとに隔離した制御 child の CPU/メモリ生標本（有料 LLM・実 job 0、性能優位の閾値なし）。
//! 各 run は runner が方式ごと・再起動ごとに新しい制御 child を起こし、External は別 process に置く。
//! 時間は論理時計と出来事待ち（child の barrier 行・stdin の ack）だけ。負荷は掛けない（ballast は一度確保して触るだけ）。
//! 実行: `cargo test -p celeris-campaign --test dagbench_overhead`。`DAGBENCH_OVERHEAD_RAW_DIR`（絶対）を渡すと
//! 固定点 sweep の raw・表・manifest をそこへ残す（未指定は一時 dir）。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    dagbench::{
        baseline::{Baseline, BaselineConfig, Outcome},
        celeris_side::{CelerisSide, Paths, RunSpec, load_scenarios},
        ext::{ExtBackend, ExtConfig, Lookup, Notice, SubmitError, SubmitReply},
        ipc::IpcExt,
        metrics::{RunHeader, append_campaign_submitted, sweep_items},
        overhead::*,
        process::*,
        run::merged_scenario,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::TaskStore;

const ROLE_ENV: &str = "DAGBENCH_OV_ROLE";
const CTL_ENV: &str = "DAGBENCH_OV_CTL";
const RAW_DIR_ENV: &str = "DAGBENCH_OVERHEAD_RAW_DIR";
const SCENARIO: &str = "k2-mini";
const INSURANCE: Duration = Duration::from_secs(600);

fn scenarios() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/scenarios.json")
}

fn spec(point: &str, seed: u64) -> RunSpec {
    let file = load_scenarios(&scenarios()).expect("scenarios");
    RunSpec::resolve(&file, SCENARIO, point, seed).expect("spec")
}

fn limits() -> Limits {
    Limits {
        wall: Duration::from_secs(1800),
        logical_secs: 100_000,
        max_restarts: 2,
        ipc_io_timeout: Duration::from_secs(120),
        ipc_retries: 1,
        stop_grace: Duration::from_secs(60),
        kill_wait: Duration::from_secs(20),
    }
}

// ---- 子 process の側 ----

struct RouterTransport(axum::Router);

impl Transport for RouterTransport {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        let app = self.0.clone();
        let req = match (method, body) {
            ("POST", Some(b)) => api_common::post_admin(path, b),
            ("GET", None) => api_common::get_admin(path),
            _ => return Err(format!("unsupported {method} {path}")),
        };
        std::thread::scope(|s| {
            s.spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                let r = rt.block_on(api_common::send(&app, req));
                Ok(HttpResponse {
                    status: r.status.as_u16(),
                    body: serde_json::from_slice(&r.body).unwrap_or(Value::Null),
                })
            })
            .join()
            .map_err(|_| "transport thread panicked".to_string())?
        })
    }
}

fn router_at(dir: &Path) -> axum::Router {
    let opts = api_common::EnvOptions {
        token: Some(api_common::TOKEN.to_string()),
        ..Default::default()
    };
    let settings = api_common::settings(
        &dir.join("celeris.db"),
        &dir.join("api-workspaces"),
        &dir.join("workspace"),
        &dir.join("knowledge"),
        opts,
    );
    let (_tx, rx) = tokio::sync::watch::channel(None);
    task_api::router(task_api::ApiState::new(settings, rx).expect("api state"))
}

/// `nth` 件目（異なる論理 job）の待機開始の ack 直後に「制御 process の停止」を模す backend 包み:
/// 終了 barrier で標本を ack させてから exit する（kill ではない。永続 state は保存済み）。
struct StopAt<B> {
    inner: B,
    nth: Option<usize>,
    seen: BTreeSet<String>,
}

impl<B: ExtBackend> ExtBackend for StopAt<B> {
    fn submit(&mut self, l: &str, s: u64) -> std::io::Result<Result<SubmitReply, SubmitError>> {
        self.inner.submit(l, s)
    }
    fn wait_started(&mut self, l: &str) -> std::io::Result<()> {
        self.inner.wait_started(l)?;
        self.seen.insert(l.to_string());
        if self.nth == Some(self.seen.len()) {
            child_barrier(Kind::End)?;
            std::process::exit(0);
        }
        Ok(())
    }
    fn lookup(&mut self, l: &str) -> std::io::Result<Lookup> {
        self.inner.lookup(l)
    }
    fn advance(&mut self, n: u64) -> std::io::Result<Vec<Notice>> {
        self.inner.advance(n)
    }
    fn record_consumed(&mut self, l: &str) -> std::io::Result<bool> {
        self.inner.record_consumed(l)
    }
    fn now(&mut self) -> std::io::Result<u64> {
        self.inner.now()
    }
    fn due_secs(&mut self, l: &str) -> std::io::Result<Option<u64>> {
        self.inner.due_secs(l)
    }
}

/// role 再起動の入口（通常の test run では環境変数が無いので何もしない）。
#[test]
fn ov_role_entry() {
    let Ok(role) = std::env::var(ROLE_ENV) else {
        return;
    };
    match role.as_str() {
        "external" => run_external_role_from_env().expect("external role"),
        "control" => control_role(),
        other => panic!("unknown role {other}"),
    }
}

fn control_role() {
    let cfg: Value = serde_json::from_str(&std::env::var(CTL_ENV).expect("ctl env")).expect("json");
    let dir = PathBuf::from(cfg["dir"].as_str().expect("dir"));
    let sock = PathBuf::from(cfg["sock"].as_str().expect("sock"));
    let inc = cfg["incarnation"].as_u64().expect("inc");
    let s = spec(
        cfg["point"].as_str().expect("point"),
        cfg["seed"].as_u64().expect("seed"),
    );
    let mut ipc = IpcExt::connect_unix(&sock, &format!("control-{inc}"), limits().ipc_options());
    if inc > 1 {
        // 再起動した制御は未 ack の通知の再送を受けて ack する（測定開始前の復旧処理）。
        ipc.advance(0).expect("resend");
        ipc.flush_ack().expect("ack");
    }
    let mut backend = StopAt {
        inner: ipc,
        nth: cfg["stop_nth"].as_u64().map(|n| n as usize),
        seen: BTreeSet::new(),
    };
    match cfg["arm"].as_str().expect("arm") {
        "baseline" => {
            let mut b = Baseline::open(
                merged_scenario(&s),
                BaselineConfig {
                    poll_secs: s.poll_secs,
                    ext_job_secs: s.ext_job_secs,
                },
                &dir.join("state.json"),
            )
            .expect("open");
            b.set_worker_log(&dir.join("worker.jsonl")).expect("log");
            child_barrier(Kind::Start).expect("start barrier");
            assert_eq!(b.run(&mut backend).expect("run"), Outcome::Complete);
        }
        _ => {
            let paths = Paths::under(&dir.join("celeris.db"), &dir);
            let store: Arc<dyn TaskStore> =
                Arc::new(task_core::SqliteStore::open(&paths.db).expect("store"));
            let transport = RouterTransport(router_at(&dir));
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .expect("rt");
            let mut side = CelerisSide::with_backend(s.clone(), paths, store, Box::new(backend))
                .expect("side");
            child_barrier(Kind::Start).expect("start barrier");
            let report = rt.block_on(side.drive(&transport)).expect("drive");
            for c in &report.campaigns {
                assert_eq!(c.status, "done", "{:?}", report.campaigns);
            }
        }
    }
    child_barrier(Kind::End).expect("end barrier");
}

// ---- runner の側 ----

fn role_cmd(role: &str) -> Command {
    let mut c = Command::new(std::env::current_exe().expect("exe"));
    c.args([
        "--exact",
        "ov_role_entry",
        "--nocapture",
        "--test-threads=1",
    ])
    .env(ROLE_ENV, role);
    c
}

fn wait_ready(p: &OwnedProc) {
    loop {
        let l = p.next_line(Duration::from_secs(60)).expect("ready line");
        if l.ends_with(READY_LINE) {
            return;
        }
    }
}

/// 方式 1 つ・点 1 つを、制御 child（epoch 1 → 停止 → epoch 2 で再開）で流して raw dir を作る。
fn run_arm(base: &Path, arm: &str, point: &str, seed: u64) -> PathBuf {
    let s = spec(point, seed);
    let d = base.join(format!("{SCENARIO}-{point}-{seed}-{arm}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).expect("raw dir");
    let occurrences = (s.point.concurrent_campaigns
        * s.scenario
            .nodes
            .iter()
            .filter(|n| n.external.is_some())
            .count()) as u64;
    let nth = 1 + seed % occurrences;
    let lim = limits();

    RunHeader::from_spec(&s, arm, SCOPE_LABEL)
        .write(&d)
        .expect("run.json");
    OvHeader::new(arm, SCENARIO, point, seed)
        .expect("units")
        .write(&d)
        .expect("overhead-run.json");
    if arm == "baseline" {
        for (i, c) in s.campaign_ids().iter().enumerate() {
            append_campaign_submitted(&d, c, s.arrival(i)).expect("submitted");
        }
    }
    let log = EventLog::create(&d).expect("log");
    let mut sup = Supervisor::new(log.clone(), lim.clone()).expect("sup");
    let units = Units::host().expect("units");
    let mut rec = EpochRecorder::new(&d, &log, arm, units);

    // socket の path だけ短い dir に置く（記録ではない短命の endpoint）。
    let sd = tempfile::Builder::new()
        .prefix("ov")
        .tempdir_in("/tmp")
        .expect("sock dir");
    let sock = sd.path().join("e.sock");
    let ext_cfg = ExtConfig {
        poll_secs: s.poll_secs,
        job_slots: s.scenario.resources.external_job_slots,
        faults: vec![],
    };
    let mut ext_cmd = role_cmd("external");
    ext_cmd
        .env(SOCKET_ENV, &sock)
        .env(LEDGER_ENV, d.join("ext.jsonl"))
        .env(EXT_CFG_ENV, serde_json::to_string(&ext_cfg).expect("cfg"));
    let mut ext = sup.spawn(Role::External, ext_cmd).expect("external");
    wait_ready(&ext);

    for inc in 1..=2u64 {
        let mut c = role_cmd("control");
        c.env(
            CTL_ENV,
            json!({"arm": arm, "dir": d, "sock": sock, "point": point, "seed": seed,
                   "incarnation": inc, "stop_nth": (inc == 1).then_some(nth)})
            .to_string(),
        );
        let mut p = sup.spawn(Role::Control, c).expect("control");
        rec.begin(&p).expect("epoch begin");
        rec.sample_at_barrier(&mut p, Kind::Start, INSURANCE)
            .expect("start sample");
        rec.sample_at_barrier(&mut p, Kind::End, INSURANCE)
            .unwrap_or_else(|e| panic!("{arm} epoch {inc}: {e}"));
        let st = p
            .wait_exit(INSURANCE)
            .expect("wait")
            .unwrap_or_else(|| panic!("control did not exit after the ack"));
        assert!(st.success(), "{arm} epoch {inc}: {st:?}");
        p.stop(None, "epoch_done").expect("reap");
    }
    let mut shut = IpcExt::connect_unix(&sock, "runner-shutdown", lim.ipc_options());
    let r = ext
        .stop(Some(&mut || shut.shutdown_server()), "run_done")
        .expect("stop external");
    assert!(!r.killed, "{r:?}");
    d
}

/// 親 process の生涯 HWM を押し上げる（確保して触るだけ。以後の子の値に混ざらないことの確認用）。
fn ballast(mb: usize) -> Vec<u8> {
    let mut v = vec![0u8; mb << 20];
    for i in (0..v.len()).step_by(4096) {
        v[i] = 1;
    }
    v
}

fn self_hwm_kb() -> u64 {
    fs::read_to_string("/proc/self/status")
        .expect("status")
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .expect("VmHWM")
}

fn events(d: &Path) -> Vec<Value> {
    read_events(&d.join("process-events.jsonl")).expect("events")
}

fn pid_of(d: &Path, role: &str) -> Vec<u64> {
    events(d)
        .iter()
        .filter(|e| e["kind"] == "process_started" && e["role"] == role)
        .filter_map(|e| e["pid"].as_u64())
        .collect()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("mkdir");
    for e in fs::read_dir(from).expect("ls").flatten() {
        let p = e.path();
        if p.is_file() {
            fs::copy(&p, to.join(e.file_name())).expect("copy");
        }
    }
}

fn assert_complete_two_epochs(r: &Row) {
    let o = &r.overhead;
    assert_eq!(o.epochs.len(), 2, "{o:?}");
    assert!(o.complete, "{o:?}");
    assert!(
        o.cpu_ms.is_some() && o.peak_rss_kb.is_some_and(|k| k > 0),
        "{o:?}"
    );
    assert_eq!(r.completed_nodes, r.completed_nodes_expected, "{r:?}");
}

#[test]
fn isolated_epochs_in_both_orders_do_not_inherit_the_parent_hwm() {
    let file = load_scenarios(&scenarios()).expect("scenarios");
    let seed = file.seed;
    let tmp = tempfile::tempdir().expect("tmp");
    let parent = u64::from(std::process::id());
    // 順序 A: baseline → celeris。2 本目の前に親の HWM を押し上げる。
    let a_base = run_arm(&tmp.path().join("a"), "baseline", "camp-1", seed);
    let _big = ballast(300);
    let a_cel = run_arm(&tmp.path().join("a"), "celeris", "camp-1", seed);
    // 順序 B: celeris → baseline（親の HWM は既に高い）。
    let b_cel = run_arm(&tmp.path().join("b"), "celeris", "camp-1", seed);
    let b_base = run_arm(&tmp.path().join("b"), "baseline", "camp-1", seed);
    let parent_hwm = self_hwm_kb();
    assert!(
        parent_hwm >= 300 * 1024,
        "ballast not reflected: {parent_hwm}"
    );
    for d in [&a_base, &a_cel, &b_cel, &b_base] {
        let row = regenerate(d).expect("regenerate");
        assert_complete_two_epochs(&row);
        // 親・External の pid は標本に無く、制御 epoch の pid だけがある。
        let samples = fs::read_to_string(d.join(OVERHEAD_SAMPLES)).expect("samples");
        let ctl = pid_of(d, "control");
        assert_eq!(ctl.len(), 2);
        for l in samples.lines() {
            let s: OvSample = serde_json::from_str(l).expect("sample");
            assert!(ctl.contains(&u64::from(s.pid)), "{s:?}");
            assert_ne!(u64::from(s.pid), parent);
            assert!(!pid_of(d, "external").contains(&u64::from(s.pid)));
            assert_eq!(
                (s.role.as_str(), s.dataset.as_str()),
                ("control", "control_measured")
            );
        }
        // 親の ballast（300 MiB）は子の最大に現れない。
        let peak = row.overhead.peak_rss_kb.expect("peak");
        assert!(
            peak < 250 * 1024,
            "{} peak {peak} kB looks inherited",
            row.arm
        );
        assert!(
            peak < parent_hwm,
            "{} peak {peak} >= parent HWM {parent_hwm}",
            row.arm
        );
        // 隔離しているので、各 epoch は別の pid・starttime。
        let pids: BTreeSet<u32> = row.overhead.epochs.iter().map(|e| e.pid).collect();
        assert_eq!(pids.len(), 2);
        // window_after_reset なら開始前の初期化分は peak に入らない（pre_start_hwm_kb と別に残る）。
        for e in &row.overhead.epochs {
            assert!(e.pre_start_hwm_kb.is_some_and(|k| k > 0), "{e:?}");
            if e.hwm_scope == "window_after_reset" {
                assert!(e.peak_kb.is_some());
            }
        }
        // 実環境値が単位として残っている。
        assert!(row.overhead.units.clk_tck > 0 && row.overhead.units.page_size >= 4096);
    }
    // 順序を入れ替えても同じ完了量（makespan は論理時計の値で、再起動点の競合で poll 1〜2 回分ずれうるため、
    // ここでは断言しない。ずれの観測は進捗に残す）。
    let done = |d: &Path| regenerate(d).expect("row").completed_nodes;
    assert_eq!(done(&a_base), done(&b_base));
    assert_eq!(done(&a_cel), done(&b_cel));
}

fn good_run() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tmp");
    let d = run_arm(tmp.path(), "baseline", "camp-1", spec("camp-1", 0).seed);
    (tmp, d)
}

fn rewrite_samples(d: &Path, f: impl Fn(Vec<Value>) -> Vec<Value>) {
    let p = d.join(OVERHEAD_SAMPLES);
    let v: Vec<Value> = fs::read_to_string(&p)
        .expect("samples")
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    let out: String = f(v).iter().map(|x| format!("{x}\n")).collect();
    fs::write(p, out).expect("write");
}

fn rewrite_events(d: &Path, f: impl Fn(&Value) -> bool) {
    let p = d.join("process-events.jsonl");
    let keep: String = fs::read_to_string(&p)
        .expect("events")
        .lines()
        .filter(|l| f(&serde_json::from_str::<Value>(l).expect("json")))
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(p, keep).expect("write");
}

fn err_of(d: &Path) -> String {
    compute_overhead(d).expect_err("must be rejected")
}

#[test]
fn foreign_missing_and_unit_broken_samples_are_detected() {
    let (_t, good) = good_run();
    let row = regenerate(&good).expect("good");
    assert_complete_two_epochs(&row);
    let ext_pid = pid_of(&good, "external")[0];
    let first = |d: &Path| -> Value {
        serde_json::from_str(
            fs::read_to_string(d.join(OVERHEAD_SAMPLES))
                .expect("s")
                .lines()
                .next()
                .expect("l"),
        )
        .expect("json")
    };
    let neg = |name: &str| {
        let d = good.parent().expect("p").join(name);
        copy_dir(&good, &d);
        d
    };

    // 外部 process / 観測側 / 試験親 / 別 arm の標本を制御として混ぜる。
    for (name, pid) in [("ext", ext_pid), ("parent", u64::from(std::process::id()))] {
        let d = neg(name);
        rewrite_samples(&d, |mut v| {
            let mut x = first(&good);
            x["pid"] = json!(pid);
            x["seq"] = json!(99);
            v.push(x);
            v
        });
        assert!(
            err_of(&d).contains("never started as a control epoch"),
            "{name}"
        );
    }
    let d = neg("role");
    rewrite_samples(&d, |mut v| {
        v[0]["role"] = json!("external");
        v
    });
    assert!(err_of(&d).contains("only control"));
    let d = neg("observer");
    rewrite_samples(&d, |mut v| {
        v[1]["role"] = json!("observer");
        v
    });
    assert!(err_of(&d).contains("only control"));
    let d = neg("otherarm");
    rewrite_samples(&d, |mut v| {
        v[0]["arm"] = json!("celeris");
        v
    });
    assert!(err_of(&d).contains("another dataset/arm"));

    // 単位が header と違う / RSS の page 換算が合わない。
    let d = neg("hz");
    rewrite_samples(&d, |mut v| {
        v[0]["clk_tck"] = json!(1000);
        v
    });
    assert!(err_of(&d).contains("units"));
    let d = neg("page");
    rewrite_samples(&d, |mut v| {
        v[0]["rss_pages"] = json!(v[0]["rss_pages"].as_u64().expect("n") * 16);
        v
    });
    assert!(err_of(&d).contains("unit mismatch"));

    // 欠損は 0 ではなく不完全（CPU/メモリは None、表は NA）。
    let d = neg("noend");
    rewrite_samples(&d, |v| {
        let mut dropped = false;
        v.into_iter()
            .filter(|x| {
                let hit = !dropped && x["kind"] == "end";
                dropped |= hit;
                !hit
            })
            .collect()
    });
    let r = regenerate(&d).expect("incomplete is not an error");
    assert!(!r.overhead.complete);
    assert_eq!((r.overhead.cpu_ms, r.overhead.peak_rss_kb), (None, None));
    assert!(
        r.overhead
            .epochs
            .iter()
            .any(|e| e.reason.as_deref() == Some("missing end sample"))
    );
    assert!(render_tsv(&[r]).contains("\tNA\tNA\t"));

    // ack の無い終了標本は不完全。
    let d = neg("noack");
    let end_seq = |d: &Path| -> u64 {
        let s: Vec<Value> = fs::read_to_string(d.join(OVERHEAD_SAMPLES))
            .expect("s")
            .lines()
            .map(|l| serde_json::from_str(l).expect("j"))
            .collect();
        s.iter().find(|x| x["kind"] == "end").expect("end")["seq"]
            .as_u64()
            .expect("seq")
    };
    let victim = end_seq(&d);
    rewrite_events(&d, |e| {
        !(e["name"] == B_SAMPLE_ACKED && e["sample_seq"] == victim)
    });
    let r = regenerate(&d).expect("incomplete");
    assert!(!r.overhead.complete && r.overhead.cpu_ms.is_none());
    assert!(
        r.overhead
            .epochs
            .iter()
            .any(|e| e.reason.as_deref() == Some("end sample not acked"))
    );

    // starttime が start と end で違う（pid 再利用・別 process の混入）。
    let d = neg("starttime");
    rewrite_samples(&d, |mut v| {
        let i = v.iter().position(|x| x["kind"] == "end").expect("end");
        v[i]["starttime_ticks"] = json!(1);
        v
    });
    assert!(!regenerate(&d).expect("incomplete").overhead.complete);

    // 旧 inclusive 標本は制御単体と混ぜず拒否する。
    let d = neg("legacy");
    fs::write(d.join("samples.jsonl"), "{}\n").expect("legacy");
    assert!(err_of(&d).contains("driver_process_inclusive"));
    let d = neg("legacy-run");
    let rj = fs::read_to_string(d.join("run.json"))
        .expect("run.json")
        .replace(SCOPE_LABEL, "driver_process_inclusive(x)");
    fs::write(d.join("run.json"), rj).expect("write");
    assert!(err_of(&d).contains("not control-only"));
    // 逆に旧 in-process raw に overhead を再生成する入口は無い（overhead-run.json が無い）。
    let d = neg("noheader");
    fs::remove_file(d.join(OVERHEAD_RUN)).expect("rm");
    assert!(err_of(&d).contains(OVERHEAD_RUN));
}

#[test]
fn tables_are_byte_identical_from_the_same_raw_and_follow_the_raw() {
    let (_t, d) = good_run();
    let render = || {
        let r = regenerate(&d).expect("row");
        (
            render_tsv(std::slice::from_ref(&r)),
            render_json(&[r]).expect("json"),
        )
    };
    let (a, b) = (render(), render());
    assert_eq!(a, b);
    assert!(a.0.starts_with("dataset\tmeasurement_id\t"));
    // raw の 1 値を変えると表が変わる（集計値を別に持っていない）。
    let victim = d.parent().expect("p").join("edited");
    copy_dir(&d, &victim);
    rewrite_samples(&victim, |mut v| {
        let i = v.iter().position(|x| x["kind"] == "end").expect("end");
        v[i]["utime_ticks"] = json!(v[i]["utime_ticks"].as_u64().expect("u") + 7);
        v
    });
    let r = regenerate(&victim).expect("row");
    let orig = regenerate(&d).expect("row");
    assert_eq!(
        r.overhead.cpu_ticks.expect("c"),
        orig.overhead.cpu_ticks.expect("c") + 7
    );
    assert_ne!(render_tsv(&[r]), a.0);
}

fn sha256(p: &Path) -> String {
    Sha256::digest(fs::read(p).expect("read"))
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn sweep_base() -> (PathBuf, Option<tempfile::TempDir>) {
    match std::env::var_os(RAW_DIR_ENV) {
        Some(b) => {
            let b = PathBuf::from(b);
            assert!(b.is_absolute(), "{RAW_DIR_ENV} must be absolute");
            fs::create_dir_all(&b).expect("base");
            (b, None)
        }
        None => {
            let t = tempfile::tempdir().expect("tmp");
            (t.path().to_path_buf(), Some(t))
        }
    }
}

/// 既存の事前固定点（OFAT、k2-mini の 7 点）の 1 点を固定 seed で両方式とも実際に流し、raw から行を再生成する。
/// 点ごとの試験に分けてあるのは、1 本の試験時間を nextest の上限に近づけないため。総当たりはしない。
fn run_point(point: &str, baseline_first: bool) {
    let file = load_scenarios(&scenarios()).expect("scenarios");
    assert!(
        file.fixed_points.points.iter().any(|p| p.id == point),
        "{point} is not a fixed point"
    );
    let (base, _keep) = sweep_base();
    let order = if baseline_first {
        ["baseline", "celeris"]
    } else {
        ["celeris", "baseline"]
    };
    let mut rows = Vec::new();
    for arm in order {
        let d = run_arm(&base.join("raw"), arm, point, file.seed);
        let row = regenerate(&d).expect("row");
        assert_complete_two_epochs(&row);
        assert_eq!((row.point.as_str(), row.seed), (point, file.seed));
        rows.push(row);
    }
    // 両方式が同じ点・seed の同じ DAG を最後まで処理した。
    assert_eq!(rows[0].completed_nodes, rows[1].completed_nodes);
    assert_eq!(
        render_tsv(&rows),
        render_tsv(&rows.iter().rev().cloned().collect::<Vec<_>>())
    );
}

macro_rules! point_test {
    ($name:ident, $point:expr, $first:expr) => {
        #[test]
        fn $name() {
            run_point($point, $first);
        }
    };
}
point_test!(ofat_center, "center", true);
point_test!(ofat_camp_1, "camp-1", false);
point_test!(ofat_camp_4, "camp-4", true);
point_test!(ofat_load_lo, "load-lo", false);
point_test!(ofat_load_hi, "load-hi", true);
point_test!(ofat_ext_lo, "ext-lo", false);
point_test!(ofat_ext_hi, "ext-hi", true);

/// `DAGBENCH_OVERHEAD_RAW_DIR` の `raw/` に実行済みの run だけから表と manifest を作る（未指定なら何もしない）。
/// 一覧に載るのは raw dir が実在する run だけで、無い点は `not_executed` に出る（未測定を結果として扱わない）。
#[test]
fn ofat_tables_and_manifest_from_saved_raw() {
    let Some(base) = std::env::var_os(RAW_DIR_ENV).map(PathBuf::from) else {
        return;
    };
    assert!(base.is_absolute(), "{RAW_DIR_ENV} must be absolute");
    let file = load_scenarios(&scenarios()).expect("scenarios");
    let mut executed = Vec::new();
    let mut not_executed = Vec::new();
    for it in sweep_items(&file, &[file.seed])
        .into_iter()
        .filter(|i| i.scenario == SCENARIO)
    {
        let rel = format!("raw/{SCENARIO}-{}-{}-{}", it.point, it.seed, it.arm);
        let d = base.join(&rel);
        if !d.is_dir() {
            not_executed.push(json!({"point": it.point, "arm": it.arm, "reason": "no raw dir"}));
            continue;
        }
        let files: std::collections::BTreeMap<String, String> = fs::read_dir(&d)
            .expect("ls")
            .flatten()
            .filter(|e| e.path().is_file() && !e.file_name().to_string_lossy().ends_with(".stderr"))
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    sha256(&e.path()),
                )
            })
            .collect();
        executed.push(json!({"point": it.point, "arm": it.arm, "seed": it.seed, "raw_dir": rel, "sha256": files}));
    }
    let regen = || -> Vec<Row> {
        executed
            .iter()
            .map(|e| regenerate(&base.join(e["raw_dir"].as_str().expect("dir"))).expect("row"))
            .collect()
    };
    let rows = regen();
    for r in &rows {
        assert_complete_two_epochs(r);
    }
    let (tsv, json_t) = (render_tsv(&rows), render_json(&rows).expect("json"));
    let again = regen();
    assert_eq!(tsv, render_tsv(&again));
    assert_eq!(json_t, render_json(&again).expect("json"));
    let out = base.join("tables");
    fs::create_dir_all(&out).expect("tables");
    fs::write(out.join(OVERHEAD_TSV), &tsv).expect("tsv");
    fs::write(out.join(OVERHEAD_JSON), &json_t).expect("json");
    not_executed.push(json!({"scenario": "k2-wide", "reason": "all points not measured; absence is not a result"}));
    not_executed.push(json!({"seeds": "other than the base seed", "reason": "not measured"}));
    let manifest = json!({
        "dataset": "control_measured", "scenario": SCENARIO, "seed": file.seed,
        "design": "fixed one-factor-at-a-time points from fixtures/dagbench/scenarios.json; no full grid",
        "paid": {"llm_calls": 0, "cluster_jobs": 0},
        "executed": executed,
        "tables": {OVERHEAD_TSV: sha256(&out.join(OVERHEAD_TSV)), OVERHEAD_JSON: sha256(&out.join(OVERHEAD_JSON))},
        "not_executed": not_executed,
        "claims": "no performance-superiority threshold; rows are raw measurements",
    });
    fs::write(
        base.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).expect("m") + "\n",
    )
    .expect("manifest");
}
