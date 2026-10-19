//! 独立観測 runner と所有 process の管理: role 再起動した実 process（External / 停止無応答の child）を相手に、
//! timeout・切断・応答しない child でも有界に停止・回収でき、所有外の sentinel process に触らないこと。
//! 順序は child の ready 行・IPC の応答・exit という出来事で決め、sleep では作らない（長い保険 deadline のみ）。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::{
    collections::BTreeSet,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    dagbench::{
        baseline::{Baseline, BaselineConfig, Outcome, load_scenario},
        celeris_side::{CelerisSide, Paths, RunSpec, load_scenarios, read_worker_ledger},
        ext::{
            ExtBackend, ExtConfig, FaultKind, FaultSpec, Lookup, Notice, NoticeLedger, SubmitError,
            SubmitReply, count_notices, read_ledger, read_notice_ledger,
        },
        ipc::{IpcExt, NoticeChannel},
        metrics::{PROCESS_RAW_SCHEMA, RAW_SCHEMA, RunHeader, compute},
        process::*,
        run::BASELINE_SCOPE,
    },
};
use serde_json::{Value, json};
use task_core::TaskStore;

const TEST_ROLE_ENV: &str = "DAGBENCH_TEST_ROLE";

fn cfg() -> ExtConfig {
    ExtConfig {
        poll_secs: 30,
        job_slots: 2,
        faults: vec![],
    }
}

/// role 再起動の入口。通常の test run では何もしない（環境変数が無い）。
#[test]
fn role_entry() {
    let Ok(role) = std::env::var(TEST_ROLE_ENV) else {
        return;
    };
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let ready = || {
        use std::io::Write;
        let mut o = std::io::stdout();
        let _ = writeln!(o, "{READY_LINE}");
        let _ = o.flush();
    };
    let park = || loop {
        std::thread::park();
    };
    match role.as_str() {
        "external" => run_external_role_from_env().expect("external role"),
        // 受け付けるが決して応答しない。
        "mute" => {
            let l = UnixListener::bind(var(SOCKET_ENV)).expect("bind");
            ready();
            let mut held = Vec::new();
            for c in l.incoming().flatten() {
                held.push(c);
            }
        }
        // 受け付けてすぐ切る（切断）。
        "drop" => {
            let l = UnixListener::bind(var(SOCKET_ENV)).expect("bind");
            ready();
            for c in l.incoming().flatten() {
                drop(c);
            }
        }
        // graceful 要求（stdin close）を無視して止まらない。
        "stubborn" => {
            ready();
            park();
        }
        "control" => control_role(),
        other => panic!("unknown role {other}"),
    }
}

fn role_cmd(role: &str, sock: &Path, ledger: &Path) -> Command {
    role_cmd_cfg(role, sock, ledger, &cfg())
}

fn role_cmd_cfg(role: &str, sock: &Path, ledger: &Path, cfg: &ExtConfig) -> Command {
    let mut c = Command::new(std::env::current_exe().expect("exe"));
    c.args(["--exact", "role_entry", "--nocapture", "--test-threads=1"])
        .env(TEST_ROLE_ENV, role)
        .env(SOCKET_ENV, sock)
        .env(LEDGER_ENV, ledger)
        .env(EXT_CFG_ENV, serde_json::to_string(cfg).expect("cfg"));
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

fn events_of(log: &EventLog, kind: &str) -> Vec<Value> {
    read_events(log.path())
        .expect("events")
        .into_iter()
        .filter(|e| e["kind"] == kind)
        .collect()
}

#[test]
fn ipc_bounded_shutdown() {
    let logd = tempfile::tempdir().expect("log dir");
    // unix socket の path は 107 byte までなので、socket だけ短い dir に置く。
    let sd = tempfile::Builder::new()
        .prefix("pr")
        .tempdir_in("/tmp")
        .expect("sock dir");
    let work = tempfile::tempdir().expect("work");
    let limits = Limits {
        wall: Duration::from_secs(120),
        logical_secs: 10_000,
        max_restarts: 2,
        ipc_io_timeout: Duration::from_millis(300),
        ipc_retries: 1,
        stop_grace: Duration::from_millis(300),
        kill_wait: Duration::from_secs(10),
    };
    let bound = limits.stop_grace + limits.kill_wait + 2 * limits.ipc_io_timeout * 2;
    let log = EventLog::create(logd.path()).expect("log");
    assert!(EventLog::create(Path::new("relative/dir")).is_err());
    let mut sup = Supervisor::new(log.clone(), limits.clone()).expect("sup");

    // 所有外の sentinel（test が直接起こす。supervisor は存在を知らない）。
    let mut sentinel = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("sentinel");
    let sentinel_pid = sentinel.id();

    let mut stop_ipc = |role: &str, name: &str| {
        let sock = sd.path().join(format!("{name}.sock"));
        let mut p = sup
            .spawn(
                Role::External,
                role_cmd(role, &sock, &work.path().join(format!("{name}.jsonl"))),
            )
            .expect("spawn");
        wait_ready(&p);
        let mut ipc = IpcExt::connect_unix(&sock, "control", limits.ipc_options());
        let t = Instant::now();
        let r = p
            .stop(Some(&mut || ipc.shutdown_server()), name)
            .expect("stop");
        assert!(t.elapsed() < bound, "{name}: {:?}", t.elapsed());
        assert!(p.try_exit().expect("wait").is_some(), "{name} reaped");
        r
    };

    // 応答しない server（timeout）→ kill。
    let r = stop_ipc("mute", "mute");
    assert_eq!((r.reason.as_str(), r.killed), ("killed", true), "{r:?}");
    assert!(r.graceful_error.is_some(), "{r:?}");
    // 切断される → kill。
    let r = stop_ipc("drop", "drop");
    assert_eq!((r.reason.as_str(), r.killed), ("killed", true), "{r:?}");
    assert!(r.graceful_error.is_some(), "{r:?}");

    // graceful 要求を無視する child（stdin close を無視）→ 期限で kill。
    let mut p = sup
        .spawn(
            Role::Control,
            role_cmd("stubborn", &sd.path().join("x"), &work.path().join("x")),
        )
        .expect("spawn");
    wait_ready(&p);
    let t = Instant::now();
    let r = p.stop(None, "unresponsive").expect("stop");
    assert_eq!((r.reason.as_str(), r.killed), ("killed", true), "{r:?}");
    assert!(
        r.graceful_error.is_none() && r.exit_signal == Some(9),
        "{r:?}"
    );
    assert!(t.elapsed() < bound);

    // 正常な External は Shutdown で graceful に終わる。観測 runner が control 不在で時計を進める。
    let sock = sd.path().join("ext.sock");
    let ledger = work.path().join("ext.jsonl");
    let mut ext = sup
        .spawn(Role::External, role_cmd("external", &sock, &ledger))
        .expect("spawn");
    wait_ready(&ext);
    // 健全な server への client は長い保険の timeout（短い timeout は上の異常系だけ）。
    let healthy = Limits {
        ipc_io_timeout: Duration::from_secs(60),
        ..limits.clone()
    };
    let mut control = IpcExt::connect_unix(&sock, "control", healthy.ipc_options());
    control.submit("j1", 100).expect("io").expect("accepted");
    let mut observer = IpcExt::connect_unix(&sock, "observer", healthy.ipc_options());
    let out = observe_until_complete(
        &mut observer,
        &["j1".to_string()],
        &limits,
        sup.wall_deadline(),
        &log,
    )
    .expect("observe");
    // observer は通知を消費・ack していない: outbox に 1 件残り、control が後から受け取る。
    assert_eq!((out.final_now, out.high_seq, out.unacked), (100, 1, 1));
    let got = control.advance(0).expect("advance");
    assert_eq!(got.len(), 1, "{got:?}");
    let r = ext
        .stop(Some(&mut || control.shutdown_server()), "done")
        .expect("stop");
    assert_eq!((r.reason.as_str(), r.killed), ("graceful", false), "{r:?}");
    assert_eq!(r.exit_code, Some(0));
    // ledger は sync 済みで外部完了が残る。
    let l = std::fs::read_to_string(&ledger).expect("ledger");
    assert!(l.contains("external_job_completed"), "{l}");

    // 所有外 sentinel は無事で、証跡のどこにも現れない。
    assert!(sentinel.try_wait().expect("try_wait").is_none());
    let events = read_events(log.path()).expect("events");
    assert!(!events.iter().any(|e| e["pid"] == sentinel_pid));
    assert_eq!(events_of(&log, "process_stopped").len(), 4);
    let started = events_of(&log, "process_started");
    assert_eq!(started[0]["role"], "observer");
    assert!(
        started[1..]
            .iter()
            .all(|e| e["owner_pid"] == std::process::id())
    );
    assert_eq!(events_of(&log, "limits").len(), 1);
    let b = events_of(&log, "barrier");
    assert_eq!(b[0]["barrier_seq"], 1);
    let seqs: Vec<u64> = events.iter().filter_map(|e| e["seq"].as_u64()).collect();
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1));
    sentinel.kill().expect("kill sentinel");
    sentinel.wait().expect("reap sentinel");
}

// ================= ipc_external_survives_control_exit =================
//
// 順序は child の stdout 行・IPC 応答・kill/wait の結果という出来事だけで決める（sleep・短い確認ループは無い。
// あるのは長い保険の deadline のみ）。control は fixed DAG（k2-mini / camp-1）を baseline か CelerisSide で走らせ、
// ExtSim は別 process の External が所有する。control の `wait_started` ack が nth 件目に達した所で control が止まり、
// observer がそれを SIGKILL・回収し、External の外部時計を進めて完了を ledger に sync させる。その後に control を再開する。

const SCENARIO: &str = "k2-mini";
const POINT: &str = "camp-1";
const CTL_ENV: &str = "DAGBENCH_TEST_CTL";
/// raw / log の保存先（絶対 path の run 成果物 dir）。
const RAW_DIR_ENV: &str = "DAGBENCH_PROCESS_RAW_DIR";
/// 出来事待ちの長い保険（これを超えたら試験を失敗させる）。
const INSURANCE: Duration = Duration::from_secs(600);
/// 通知が来ない再開 control を待つ上限（凍結した時計で失敗するはずなので短い）。超えたら kill して回収する。
const PROBE_BOUND: Duration = Duration::from_secs(60);

fn scenarios() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/scenarios.json")
}

fn spec(seed: u64) -> RunSpec {
    let file = load_scenarios(&scenarios()).expect("scenarios");
    RunSpec::resolve(&file, SCENARIO, POINT, seed).expect("spec")
}

fn ext_cfg(s: &RunSpec) -> ExtConfig {
    ExtConfig {
        poll_secs: s.poll_secs,
        job_slots: s.scenario.resources.external_job_slots,
        faults: vec![],
    }
}

/// `nth` 件目（異なる論理 job）の `wait_started` が ack された直後に出来事を知らせて止まる backend 包み。
/// `park_on_poll` なら代わりに最初の `lookup`（Celeris 側では poller の照会 = dispatcher が wait の
/// `last_polled_at` を書いた後）で止まる。
struct Tripwire<B> {
    inner: B,
    park_at: Option<usize>,
    park_on_poll: bool,
    /// `nth` 件目（異なる論理 job）の投入が受理された直後（`wait_started` の前）で止まる。
    park_accept: Option<usize>,
    /// 応答喪失（`ResponseLost`）を受け取った直後で止まる（外部は受理・物理起動を ledger に確定済み）。
    park_lost: bool,
    /// 外部時計を進める要求を拒否する（通知が無ければ完了も再開も起きないことの確認用）。
    frozen: bool,
    accepted: BTreeSet<String>,
    seen: BTreeSet<String>,
}

/// 出来事を stdout に知らせ、observer が SIGKILL するまで進まない。
fn park(line: &str) -> ! {
    use std::io::Write;
    let mut o = std::io::stdout();
    let _ = writeln!(o, "{line}");
    let _ = o.flush();
    loop {
        std::thread::park();
    }
}

impl<B: ExtBackend> ExtBackend for Tripwire<B> {
    fn submit(&mut self, l: &str, s: u64) -> std::io::Result<Result<SubmitReply, SubmitError>> {
        let r = self.inner.submit(l, s)?;
        match &r {
            Ok(_) => {
                self.accepted.insert(l.to_string());
                if self.park_accept == Some(self.accepted.len()) {
                    park(&format!("CONTROL_ACKED accepted {:?}", self.accepted));
                }
            }
            Err(SubmitError::ResponseLost) if self.park_lost => {
                park(&format!("CONTROL_ACKED lost {l}"));
            }
            Err(_) => {}
        }
        Ok(r)
    }
    fn wait_started(&mut self, l: &str) -> std::io::Result<()> {
        self.inner.wait_started(l)?;
        self.seen.insert(l.to_string());
        if self.park_at == Some(self.seen.len()) {
            park(&format!("CONTROL_ACKED {:?}", self.seen));
        }
        Ok(())
    }
    fn lookup(&mut self, l: &str) -> std::io::Result<Lookup> {
        if self.park_on_poll {
            park(&format!("CONTROL_ACKED poll {l} after {:?}", self.seen));
        }
        self.inner.lookup(l)
    }
    fn advance(&mut self, n: u64) -> std::io::Result<Vec<Notice>> {
        if self.frozen {
            use std::io::Write;
            let mut o = std::io::stdout();
            let _ = writeln!(o, "CONTROL_FROZEN advance({n}) refused");
            let _ = o.flush();
            return Err(std::io::Error::other("external clock is frozen"));
        }
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

/// task-api の実 router への同期の Transport（`tests/dagbench_accept.rs` と同じ）。
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

/// 制御 child の本体。incarnation 1 は nth 件目の待機 ack で止まる。再開（incarnation > 1）は最初に
/// 通知の再送を受けて ack し（observer が確認するまで stdin で待つ）、ledger / 永続 state から復旧して走り切る。
fn control_role() {
    let cfg: Value = serde_json::from_str(&std::env::var(CTL_ENV).expect("ctl env")).expect("json");
    let dir = PathBuf::from(cfg["dir"].as_str().expect("dir"));
    let sock = PathBuf::from(cfg["sock"].as_str().expect("sock"));
    let inc = cfg["incarnation"].as_u64().expect("inc");
    let s = spec(cfg["seed"].as_u64().expect("seed"));
    // 再起動した client の id は 1 から始まるので、incarnation ごとに client 名を変える。
    let mut ipc =
        IpcExt::connect_unix(&sock, &format!("control-{inc}"), cfg_limits().ipc_options());
    let notice = cfg["notice"] == true;
    let arm = cfg["arm"].as_str().expect("arm").to_string();
    // 通知主導: 完了は外部 push の有界 blocking receive だけで知る（poll・lookup では探さない）。
    let channel = notice.then(|| {
        NoticeChannel::new(
            IpcExt::connect_unix(
                &sock,
                &format!("notice-rx-{inc}"),
                cfg_limits().ipc_options(),
            ),
            NoticeLedger::open(&dir.join("notice.jsonl"), &arm).expect("notice ledger"),
            50,
        )
    });
    if notice && inc > 1 {
        use std::io::Write;
        let mut o = std::io::stdout();
        let _ = writeln!(o, "CONTROL_RESUMED incarnation={inc}");
        let _ = o.flush();
    } else if inc > 1 {
        use std::io::{BufRead, Write};
        let got = ipc.advance(0).expect("resend");
        ipc.flush_ack().expect("ack");
        let mut o = std::io::stdout();
        let _ = writeln!(
            o,
            "CONTROL_RESEND n={} acked_seq={}",
            got.len(),
            ipc.acked_seq()
        );
        let _ = o.flush();
        let mut go = String::new();
        std::io::stdin().lock().read_line(&mut go).expect("go");
    }
    let tripwire = Tripwire {
        inner: ipc,
        park_at: cfg["nth"].as_u64().map(|n| n as usize),
        park_on_poll: cfg["park"] == "poll",
        park_accept: cfg["accept"].as_u64().map(|n| n as usize),
        park_lost: cfg["lost"] == true,
        frozen: cfg["frozen"] == true,
        accepted: BTreeSet::new(),
        seen: BTreeSet::new(),
    };
    match cfg["arm"].as_str().expect("arm") {
        "baseline" => {
            let mut tripwire = tripwire;
            let mut b = Baseline::open(
                load_scenario(&scenarios(), SCENARIO).expect("scenario"),
                BaselineConfig {
                    poll_secs: s.poll_secs,
                    ext_job_secs: s.ext_job_secs,
                },
                &dir.join("state.json"),
            )
            .expect("open");
            b.set_progress_log(&dir.join("worker.jsonl")).expect("log");
            if let Some(ch) = channel {
                b.set_notices(ch);
            }
            assert_eq!(b.run(&mut tripwire).expect("run"), Outcome::Complete);
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
            let report = rt.block_on(async {
                let mut side =
                    CelerisSide::with_backend(s.clone(), paths, store, Box::new(tripwire))
                        .expect("side");
                if let Some(ch) = channel {
                    side.enable_notices(ch, true);
                }
                side.drive(&transport).await
            });
            let report = report.unwrap_or_else(|e| {
                // runtime の後始末を待たずに失敗で終わる（凍結した時計の probe など）。
                eprintln!("drive failed: {e}");
                std::process::exit(3)
            });
            for c in &report.campaigns {
                assert_eq!(c.status, "done", "{:?}", report.campaigns);
            }
        }
    }
}

/// control child と observer が共有する IPC の時間（健全な server への client なので長い保険）。
fn cfg_limits() -> Limits {
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

fn wait_line(p: &OwnedProc, marker: &str) -> String {
    loop {
        let l = p
            .next_line(INSURANCE)
            .unwrap_or_else(|e| panic!("waiting for {marker} from {}: {e}", p.role().as_str()));
        if l.contains(marker) {
            return l[l.find(marker).unwrap_or(0)..].to_string();
        }
    }
}

/// raw 保存先: `DAGBENCH_PROCESS_RAW_DIR`（絶対）の下の run 用 dir。未指定のときは何も残さない作業用 dir
/// （試験後に消え、raw としては記録しない。相対 path は拒否する）。
fn run_dir(arm: String) -> (PathBuf, Option<tempfile::TempDir>) {
    match std::env::var_os(RAW_DIR_ENV) {
        Some(base) => {
            let base = PathBuf::from(base);
            assert!(
                base.is_absolute(),
                "{RAW_DIR_ENV} must be absolute: {base:?}"
            );
            let d = base.join(format!("process-raw-{arm}-{}", std::process::id()));
            std::fs::create_dir_all(&d).expect("run dir");
            (d, None)
        }
        None => {
            let t = tempfile::tempdir().expect("scratch");
            (t.path().to_path_buf(), Some(t))
        }
    }
}

/// incarnation 1 の control を止める場所。
#[derive(Clone, Copy, PartialEq)]
enum Park {
    /// `nth` 件目の `wait_started` の ack 直後。
    WaitAck,
    /// 最初の poll の照会（Celeris 側のみ。dispatcher が wait の `last_polled_at` を書き、poll が走っている最中）。
    Poll,
}

fn survive(arm: &'static str, at: Park) {
    let file = load_scenarios(&scenarios()).expect("scenarios");
    let seed = file.seed;
    let s = spec(seed);
    let externals: usize = s
        .scenario
        .nodes
        .iter()
        .filter(|n| n.external.is_some())
        .count()
        * s.point.concurrent_campaigns;
    let occurrences = externals as u64;
    let nth = 1 + seed % occurrences;
    let limits = cfg_limits();
    let (d, _scratch) = run_dir(match at {
        Park::WaitAck => arm.to_string(),
        Park::Poll => format!("{arm}-poll"),
    });
    // unix socket の path は 107 byte までなので、socket だけ短い dir に置く（記録ではない短命の endpoint）。
    let sd = tempfile::Builder::new()
        .prefix("pr")
        .tempdir_in("/tmp")
        .expect("sock dir");
    let sock = sd.path().join("e.sock");
    let ledger = d.join("ext.jsonl");

    let header = ProcessRunHeader {
        schema: PROCESS_RAW_SCHEMA.into(),
        dataset: celeris_campaign::dagbench::ext::DATASET.into(),
        arm: arm.into(),
        scenario: SCENARIO.into(),
        point: POINT.into(),
        seed,
        event: "external_wait_started".into(),
        nth,
        occurrences,
        poll_secs: s.poll_secs,
        ext_job_secs: s.ext_job_secs,
        worker_slots: s.scenario.resources.worker_slots,
        external_job_slots: s.scenario.resources.external_job_slots,
        limits: limits.clone(),
    };
    header.write(&d).expect("header");
    let log = EventLog::create(&d).expect("log");
    let mut sup = Supervisor::new(log.clone(), limits.clone()).expect("sup");

    let ctl = |inc: u64, nth: Option<u64>| {
        let mut c = Command::new(std::env::current_exe().expect("exe"));
        c.args(["--exact", "role_entry", "--nocapture", "--test-threads=1"])
            .env(TEST_ROLE_ENV, "control")
            .env(
                CTL_ENV,
                json!({"arm": arm, "dir": d, "sock": sock, "seed": seed,
                       "nth": nth, "incarnation": inc,
                       "park": if inc == 1 && at == Park::Poll { "poll" } else { "ack" }})
                .to_string(),
            );
        c
    };

    // External: ExtSim と ledger の唯一の持ち主。
    let mut ext = sup
        .spawn(
            Role::External,
            role_cmd_cfg("external", &sock, &ledger, &ext_cfg(&s)),
        )
        .expect("spawn external");
    wait_ready(&ext);

    // 1. control: submit / 待機 ack が nth 件目に達するまで走る。
    let mut c1 = sup
        .spawn(Role::Control, ctl(1, (at == Park::WaitAck).then_some(nth)))
        .expect("spawn control");
    let acked_line = wait_line(&c1, "CONTROL_ACKED");
    log.barrier(
        B_CONTROL_ACKED,
        json!({"control_pid": c1.pid(), "external_pid": ext.pid(),
               "external_incarnation": ext.incarnation(), "line": acked_line}),
    )
    .expect("barrier");

    // 2. control を SIGKILL し、Child の wait（回収）で終了を確定する（= 終了 ack）。
    let rep = c1.kill("fault_after_wait_ack").expect("kill control");
    assert_eq!(
        (rep.reason.as_str(), rep.exit_signal),
        ("killed", Some(9)),
        "{rep:?}"
    );
    assert!(c1.try_exit().expect("wait").is_some(), "control reaped");
    let proc_gone = !Path::new(&format!("/proc/{}", c1.pid())).exists();
    assert!(proc_gone, "control pid {} still exists", c1.pid());
    log.barrier(
        B_CONTROL_EXIT_ACK,
        json!({"pid": c1.pid(), "incarnation": 1, "exit_signal": rep.exit_signal,
               "proc_gone": proc_gone}),
    )
    .expect("barrier");

    // 3. control 不在のまま、observer（別 client 名・別接続）が同じ External の外部時計を進める。
    let launched: Vec<String> = read_ledger(&ledger)
        .expect("ledger")
        .into_iter()
        .filter(|e| e.event == "physical_launch")
        .filter_map(|e| e.logical_job_id)
        .collect();
    assert!(launched.len() as u64 >= nth, "{launched:?}");
    let mut observer = IpcExt::connect_unix(&sock, "observer", limits.ipc_options());
    let out = observe_until_complete(&mut observer, &launched, &limits, sup.wall_deadline(), &log)
        .expect("observe");
    assert!(
        ext.try_exit().expect("wait").is_none(),
        "external must outlive the control exit"
    );
    assert_eq!(
        out.unacked,
        launched.len(),
        "notices wait for control: {out:?}"
    );

    // 4. External が完了を ledger に sync した ack（observe 応答の ledger_seq）と、ledger の実体で確かめる。
    let entries = read_ledger(&ledger).expect("ledger");
    let done: Vec<&_> = entries
        .iter()
        .filter(|e| e.event == "external_job_completed")
        .collect();
    let completed: Vec<String> = done
        .iter()
        .filter_map(|e| e.logical_job_id.clone())
        .collect();
    for l in &launched {
        assert!(completed.contains(l), "{l} not completed: {completed:?}");
    }
    let max_completed_seq = done.iter().map(|e| e.seq).max().expect("completed");
    log.barrier(
        B_LEDGER_SYNC_ACK,
        json!({"external_pid": ext.pid(), "external_incarnation": ext.incarnation(),
               "ledger": ledger, "ledger_seq": out.ledger_seq,
               "ledger_file_seq": entries.last().map_or(0, |e| e.seq),
               "max_completed_seq": max_completed_seq, "completed": completed,
               "final_now": out.final_now, "high_seq": out.high_seq, "unacked": out.unacked}),
    )
    .expect("barrier");

    // 5. control 再開。通知は再送され（at-least-once）、ack すると outbox が空になり、残りは ledger / 永続 state で復旧する。
    log.barrier(B_CONTROL_RESUME, json!({"incarnation": 2}))
        .expect("barrier");
    let mut c2 = sup
        .spawn(Role::Control, ctl(2, None))
        .expect("respawn control");
    let resend = wait_line(&c2, "CONTROL_RESEND");
    assert_eq!(
        resend,
        format!(
            "CONTROL_RESEND n={} acked_seq={}",
            out.unacked, out.high_seq
        ),
        "all unacked notices are redelivered to the restarted control"
    );
    let (_, high, unacked, _) = observer.observe(out.final_now).expect("observe");
    assert_eq!((high, unacked), (out.high_seq, 0), "ack emptied the outbox");
    log.barrier(
        "notice_resend_ack",
        json!({"resend": resend, "high_seq": high, "unacked_after_ack": unacked}),
    )
    .expect("barrier");
    c2.send_line("go").expect("go");
    let st = c2
        .wait_exit(INSURANCE)
        .expect("wait")
        .unwrap_or_else(|| panic!("control did not finish within the insurance deadline"));
    assert!(st.success(), "resumed control failed: {st:?}");
    log.barrier(B_CONTROL_DONE, json!({"exit_code": st.code()}))
        .expect("barrier");

    // 6. External を graceful に止め、raw を集計する。
    let r = ext
        .stop(Some(&mut || observer.shutdown_server()), "run_done")
        .expect("stop external");
    assert_eq!((r.reason.as_str(), r.killed), ("graceful", false), "{r:?}");
    c2.stop(None, "run_done").expect("reap control");
    let summary = compute_process(&d).expect("compute_process");
    std::fs::write(
        d.join("summary.json"),
        serde_json::to_string_pretty(&summary).expect("json"),
    )
    .expect("summary");
    assert_eq!(summary.proof.control_pid, c1.pid());
    assert_eq!(summary.proof.external_pid, ext.pid());
    assert_eq!(summary.proof.resumed_incarnation, 2);
    assert_eq!((summary.seed, summary.nth), (seed, nth));
    // 各外部 job は 1 回だけ起動し、結果は 1 回だけ消費された。
    assert_eq!(summary.physical_launches, externals, "{summary:?}");
    assert_eq!(summary.logical_jobs, externals);
    assert_eq!(summary.consumed, externals);
    // 完了済みの作業をやり直さない: 各 node の完了はちょうど 1 回。
    let worker = read_worker_ledger(&d.join("worker.jsonl")).expect("worker");
    for n in &s.scenario.nodes {
        let c = worker
            .iter()
            .filter(|e| e.event == "node_finished" && e.node == n.id)
            .count();
        assert_eq!(c, 1, "{arm}: {} finished {c} times", n.id);
    }
}

#[test]
fn ipc_external_survives_control_exit() {
    survive("baseline", Park::WaitAck);
    survive("celeris", Park::WaitAck);
}

/// 前の control が起こした poll が結果を返さないまま SIGKILL されると、wait は `last_polled_at` を残した
/// `waiting` のまま残り、再起動した dispatcher は壁時計で `poll_secs` 経つまで poll し直さない。その間 sleeper も
/// in-flight も無い状態を「stuck」と誤判定せず、次の poll を待って走り切る（負荷下で `Park::WaitAck` の
/// control 1 が待機 ack より先に 1 件目の wait の poll まで進んだときに起きた競合を、止める場所で決定的に作る）。
#[test]
fn ipc_external_survives_control_exit_mid_poll() {
    survive("celeris", Park::Poll);
}

// ================= 通知主導の再開と応答喪失 =================
//
// 同じ fixed DAG / 資源枠 / seed / event / nth を baseline と Celeris の両方式に与える。完了は外部 push の実受信
// （有界 blocking receive）だけで知り、poll・lookup では探さない（Celeris 側は poller を失敗させる）。
// 順序は child の stdout 行・IPC 応答・SIGKILL/wait という出来事だけで決める。

const NOTICE_RAW_SCHEMA: &str = "celeris.dagbench-notice-raw/1";
const NOTICE_MARKER: &str = "notice-run.json";

/// incarnation 1 の control を止める場所。
#[derive(Clone, Copy, PartialEq, Debug)]
enum Stop {
    /// `nth` 件目の投入が受理された直後（待機に入る前）。
    AfterAccept,
    /// `nth` 件目の待機登録（`wait_started`）の ack 直後。
    AfterWait,
    /// 受理・物理起動を永続確定した後、応答だけ喪失した直後。
    LostResponse,
}

impl Stop {
    fn name(self) -> &'static str {
        match self {
            Stop::AfterAccept => "accept",
            Stop::AfterWait => "wait",
            Stop::LostResponse => "lost",
        }
    }
}

/// raw の種別: process/2 の障害時継続（旧版: 通知処理証跡なし）か、通知主導（新版）か。
#[derive(Debug, PartialEq)]
enum RawKind {
    PollEraProcess,
    NoticeDriven,
}

fn raw_kind(raw: &Path) -> Result<RawKind, String> {
    let run =
        std::fs::read_to_string(raw.join("run.json")).map_err(|e| format!("run.json: {e}"))?;
    if !run.contains(PROCESS_RAW_SCHEMA) {
        return Err(format!("not a {PROCESS_RAW_SCHEMA} raw"));
    }
    match std::fs::read_to_string(raw.join(NOTICE_MARKER)) {
        Err(_) => Ok(RawKind::PollEraProcess),
        Ok(m) => {
            let m: Value = serde_json::from_str(&m).map_err(|e| e.to_string())?;
            if m["schema"] != NOTICE_RAW_SCHEMA {
                return Err(format!("unknown notice raw schema {}", m["schema"]));
            }
            Ok(RawKind::NoticeDriven)
        }
    }
}

/// 通知主導 raw を独立 ledger（外部側 ext.jsonl・制御側 notice.jsonl）から数え直す。旧版の raw は拒否する。
#[derive(Debug)]
struct NoticeSummary {
    logical_jobs: usize,
    physical_launches: usize,
    consumed: usize,
    notices_sent: usize,
    received_distinct: usize,
    processed: usize,
    acked: usize,
    continuations: usize,
    suppressed: usize,
    lookups: usize,
}

fn compute_notice(raw: &Path, arm: &str) -> Result<NoticeSummary, String> {
    if raw_kind(raw)? != RawKind::NoticeDriven {
        return Err("poll-era process raw has no notice processing evidence".into());
    }
    let ext = read_ledger(&raw.join("ext.jsonl")).map_err(|e| e.to_string())?;
    let c = celeris_campaign::dagbench::ext::count(&ext);
    let n = read_notice_ledger(&raw.join("notice.jsonl")).map_err(|e| e.to_string())?;
    if n.iter()
        .any(|e| e.role != "control_consume" || e.side != arm)
    {
        return Err("notice ledger row is not the control-side consumer of this arm".into());
    }
    let nc = count_notices(&n);
    let received_distinct = n
        .iter()
        .filter(|e| e.event == "notice_received")
        .filter_map(|e| e.notice_seq)
        .collect::<BTreeSet<_>>()
        .len();
    Ok(NoticeSummary {
        logical_jobs: c.logical_jobs,
        physical_launches: c.physical_launches,
        consumed: c.consumed,
        notices_sent: c.notices_sent,
        received_distinct,
        processed: nc.processed,
        acked: nc.acked,
        continuations: nc.continuations,
        suppressed: nc.suppressed,
        lookups: c.lookups,
    })
}

fn externals_of(s: &RunSpec) -> u64 {
    (s.scenario
        .nodes
        .iter()
        .filter(|n| n.external.is_some())
        .count()
        * s.point.concurrent_campaigns) as u64
}

fn notice_ctl(
    arm: &str,
    d: &Path,
    sock: &Path,
    seed: u64,
    inc: u64,
    stop: Option<(Stop, u64)>,
    frozen: bool,
) -> Command {
    let mut c = Command::new(std::env::current_exe().expect("exe"));
    c.args(["--exact", "role_entry", "--nocapture", "--test-threads=1"])
        .env(TEST_ROLE_ENV, "control")
        .env(
            CTL_ENV,
            json!({"arm": arm, "dir": d, "sock": sock, "seed": seed, "incarnation": inc,
                   "notice": true, "frozen": frozen,
                   "nth": stop.filter(|(s, _)| *s == Stop::AfterWait).map(|(_, n)| n),
                   "accept": stop.filter(|(s, _)| *s == Stop::AfterAccept).map(|(_, n)| n),
                   "lost": matches!(stop, Some((Stop::LostResponse, _)))})
            .to_string(),
        );
    c
}

fn header_for(
    arm: &str,
    s: &RunSpec,
    event: &str,
    nth: u64,
    occurrences: u64,
    limits: &Limits,
) -> ProcessRunHeader {
    ProcessRunHeader {
        schema: PROCESS_RAW_SCHEMA.into(),
        dataset: celeris_campaign::dagbench::ext::DATASET.into(),
        arm: arm.into(),
        scenario: SCENARIO.into(),
        point: POINT.into(),
        seed: s.seed,
        event: event.into(),
        nth,
        occurrences,
        poll_secs: s.poll_secs,
        ext_job_secs: s.ext_job_secs,
        worker_slots: s.scenario.resources.worker_slots,
        external_job_slots: s.scenario.resources.external_job_slots,
        limits: limits.clone(),
    }
}

fn short_sock_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("pr")
        .tempdir_in("/tmp")
        .expect("sock dir")
}

/// 停止 → 外部完了 → 再開の 1 run。通知主導（poll なし）。
fn notice_run(arm: &'static str, stop: Stop) {
    let file = load_scenarios(&scenarios()).expect("scenarios");
    let seed = file.seed;
    let s = spec(seed);
    let occurrences = externals_of(&s);
    let externals = occurrences as usize;
    let nth = 1 + seed % occurrences;
    let limits = cfg_limits();
    let (d, _scratch) = run_dir(format!("notice-{arm}-{}", stop.name()));
    let sd = short_sock_dir();
    let sock = sd.path().join("e.sock");
    let ledger = d.join("ext.jsonl");
    let (fault_event, kind) = match stop {
        Stop::AfterAccept => (
            "external_job_completed",
            FaultKind::DuplicateNotice { extra: 1 },
        ),
        Stop::AfterWait => (
            "external_job_completed",
            FaultKind::DuplicateNotice { extra: 1 },
        ),
        Stop::LostResponse => ("external_submit_accepted", FaultKind::DropResponse),
    };
    let fault = FaultSpec {
        fault_id: format!("{arm}-{}", stop.name()),
        kind,
        event: fault_event.into(),
        seed,
        occurrences,
    };
    let mut cfg = ext_cfg(&s);
    cfg.faults = vec![fault.clone()];
    let stop_event = match stop {
        Stop::AfterAccept => "external_submit_accepted",
        Stop::AfterWait => "external_wait_started",
        Stop::LostResponse => "external_submit_accepted",
    };
    header_for(arm, &s, stop_event, nth, occurrences, &limits)
        .write(&d)
        .expect("header");
    let log = EventLog::create(&d).expect("log");
    let mut sup = Supervisor::new(log.clone(), limits.clone()).expect("sup");

    let mut ext = sup
        .spawn(
            Role::External,
            role_cmd_cfg("external", &sock, &ledger, &cfg),
        )
        .expect("spawn external");
    wait_ready(&ext);

    // 1. control: nth 件目（受理後 / 待機後 / 応答喪失後）まで走って止まる。
    let mut c1 = sup
        .spawn(
            Role::Control,
            notice_ctl(arm, &d, &sock, seed, 1, Some((stop, nth)), false),
        )
        .expect("spawn control");
    let acked_line = wait_line(&c1, "CONTROL_ACKED");
    log.barrier(
        B_CONTROL_ACKED,
        json!({"control_pid": c1.pid(), "external_pid": ext.pid(),
               "external_incarnation": ext.incarnation(), "line": acked_line,
               "stop": stop.name()}),
    )
    .expect("barrier");
    let rep = c1.kill("fault_after_stop_point").expect("kill control");
    assert_eq!(
        (rep.reason.as_str(), rep.exit_signal),
        ("killed", Some(9)),
        "{rep:?}"
    );
    assert!(c1.try_exit().expect("wait").is_some(), "control reaped");
    let gone = !Path::new(&format!("/proc/{}", c1.pid())).exists();
    assert!(gone, "control pid {} still exists", c1.pid());
    log.barrier(
        B_CONTROL_EXIT_ACK,
        json!({"pid": c1.pid(), "incarnation": 1, "exit_signal": rep.exit_signal,
               "proc_gone": gone}),
    )
    .expect("barrier");

    // 外部は受理・物理起動を永続確定済み。応答喪失の run では、その論理 job の応答だけが失われている。
    let before = read_ledger(&ledger).expect("ledger");
    let launched: Vec<String> = before
        .iter()
        .filter(|e| e.event == "physical_launch")
        .filter_map(|e| e.logical_job_id.clone())
        .collect();
    assert!(launched.len() as u64 >= nth, "{launched:?}");
    let lost = before
        .iter()
        .find(|e| e.event == "fault_fired" && stop == Stop::LostResponse);
    let lost_ids = lost.map(|e| {
        let l = e.logical_job_id.clone().expect("lost logical");
        let p = before
            .iter()
            .find(|x| x.event == "physical_launch" && x.logical_job_id.as_deref() == Some(&l))
            .and_then(|x| x.physical_id.clone())
            .expect("physical launch is durable before the response is lost");
        (l, p, e.seq)
    });
    if stop == Stop::LostResponse {
        assert!(lost_ids.is_some(), "no fault_fired: {before:?}");
    }
    let launches_before = launched.len();

    // 2. control 不在のまま、observer（別 client）が同じ External の外部時計を進める。通知は push 待ちに残る。
    let mut observer = IpcExt::connect_unix(&sock, "observer", limits.ipc_options());
    let out = observe_until_complete(&mut observer, &launched, &limits, sup.wall_deadline(), &log)
        .expect("observe");
    assert!(
        ext.try_exit().expect("wait").is_none(),
        "external must outlive the control exit"
    );
    assert!(out.unacked >= 1, "notices wait for control: {out:?}");
    let entries = read_ledger(&ledger).expect("ledger");
    let completed: Vec<String> = entries
        .iter()
        .filter(|e| e.event == "external_job_completed")
        .filter_map(|e| e.logical_job_id.clone())
        .collect();
    for l in &launched {
        assert!(completed.contains(l), "{l} not completed: {completed:?}");
    }
    let max_completed_seq = entries
        .iter()
        .filter(|e| e.event == "external_job_completed")
        .map(|e| e.seq)
        .max()
        .expect("completed");
    log.barrier(
        B_LEDGER_SYNC_ACK,
        json!({"external_pid": ext.pid(), "external_incarnation": ext.incarnation(),
               "ledger": ledger, "ledger_seq": out.ledger_seq,
               "ledger_file_seq": entries.last().map_or(0, |e| e.seq),
               "max_completed_seq": max_completed_seq, "completed": completed,
               "final_now": out.final_now, "high_seq": out.high_seq, "unacked": out.unacked}),
    )
    .expect("barrier");

    // 3. 再開。通知が実際に受信・適用・ack されて継続する（完了の poll / lookup 探索はしない）。
    log.barrier(B_CONTROL_RESUME, json!({"incarnation": 2}))
        .expect("barrier");
    let mut c2 = sup
        .spawn(
            Role::Control,
            notice_ctl(arm, &d, &sock, seed, 2, None, false),
        )
        .expect("respawn control");
    wait_line(&c2, "CONTROL_RESUMED");
    let st = c2
        .wait_exit(INSURANCE)
        .expect("wait")
        .unwrap_or_else(|| panic!("control did not finish within the insurance deadline"));
    assert!(st.success(), "resumed control failed: {st:?}");
    log.barrier(B_CONTROL_DONE, json!({"exit_code": st.code()}))
        .expect("barrier");
    assert!(
        ext.try_exit().expect("wait").is_none(),
        "the same external process stayed alive through the resume"
    );
    let (_, _, unacked, _) = observer.observe(out.final_now).expect("observe");
    assert_eq!(
        unacked, 0,
        "every notice was acked after durable application"
    );

    let r = ext
        .stop(Some(&mut || observer.shutdown_server()), "run_done")
        .expect("stop external");
    assert_eq!((r.reason.as_str(), r.killed), ("graceful", false), "{r:?}");
    c2.stop(None, "run_done").expect("reap control");

    // 4. 新版 raw: 通知処理証跡の marker を置き、独立 ledger（外部 / 制御側）から数え直す。
    assert_eq!(
        raw_kind(&d),
        Ok(RawKind::PollEraProcess),
        "before the marker the raw is the old poll-era kind"
    );
    std::fs::write(
        d.join(NOTICE_MARKER),
        json!({"schema": NOTICE_RAW_SCHEMA, "process_raw_schema": PROCESS_RAW_SCHEMA,
               "arm": arm, "stop": stop.name(), "seed": seed, "nth": nth,
               "ext_ledger": "ext.jsonl", "notice_ledger": "notice.jsonl",
               "polling_for_completion": false})
        .to_string(),
    )
    .expect("marker");
    assert_eq!(raw_kind(&d), Ok(RawKind::NoticeDriven));
    let summary = compute_process(&d).expect("compute_process");
    assert_eq!(summary.proof.control_pid, c1.pid());
    assert_eq!(summary.proof.external_pid, ext.pid());
    assert_eq!(summary.proof.resumed_incarnation, 2);
    assert_eq!((summary.seed, summary.nth), (seed, nth));
    let ns = compute_notice(&d, arm).expect("compute_notice");
    std::fs::write(d.join("notice-summary.json"), format!("{ns:#?}")).expect("summary");
    // 論理 job ごとに物理起動 1 回・消費 1 回。再開後に増えない。
    assert_eq!(ns.physical_launches, externals, "{ns:?}");
    assert_eq!(ns.logical_jobs, externals, "{ns:?}");
    assert_eq!(ns.consumed, externals, "{ns:?}");
    assert!(ns.physical_launches >= launches_before);
    // 再開時の起動照会は物理 ID の確認だけ（追跡中の job ごとに 1 回まで）。完了は通知で知る。
    assert!(
        ns.lookups <= launches_before,
        "more lookups than tracked jobs: {ns:?}"
    );
    // 受信した通知は全て永続適用後に ack され、継続は論理 job ごとに 1 回。
    assert_eq!(ns.received_distinct, ns.notices_sent, "{ns:?}");
    assert_eq!(ns.processed, ns.acked, "{ns:?}");
    assert_eq!(
        ns.continuations, externals,
        "one continuation per job: {ns:?}"
    );
    if matches!(fault.kind, FaultKind::DuplicateNotice { .. }) {
        assert!(ns.suppressed >= 1, "duplicate suppressed: {ns:?}");
        assert!(
            ns.notices_sent > externals,
            "duplicate was really sent: {ns:?}"
        );
    }
    // 完了済みの作業をやり直さない。
    let worker = read_worker_ledger(&d.join("worker.jsonl")).expect("worker");
    for n in &s.scenario.nodes {
        let c = worker
            .iter()
            .filter(|e| e.event == "node_finished" && e.node == n.id)
            .count();
        assert_eq!(c, 1, "{arm}/{}: {} finished {c} times", stop.name(), n.id);
    }
    // 応答喪失: 再開後の照会（lookup / 冪等キーの再投入）が同じ論理・物理 ID に一致し、起動は増えていない。
    if let Some((l, p, fired_seq)) = lost_ids {
        let all = read_ledger(&ledger).expect("ledger");
        let launches: Vec<_> = all
            .iter()
            .filter(|e| e.event == "physical_launch" && e.logical_job_id.as_deref() == Some(&l))
            .collect();
        assert_eq!(launches.len(), 1, "lost job launched once: {all:?}");
        assert_eq!(launches[0].physical_id.as_deref(), Some(p.as_str()));
        let accepted = all
            .iter()
            .filter(|e| {
                e.event == "external_submit_accepted" && e.logical_job_id.as_deref() == Some(&l)
            })
            .count();
        assert_eq!(accepted, 1);
        let recon: Vec<_> = all
            .iter()
            .filter(|e| {
                e.seq > fired_seq
                    && matches!(e.event.as_str(), "lookup" | "submit_deduped")
                    && e.logical_job_id.as_deref() == Some(&l)
            })
            .collect();
        assert!(
            !recon.is_empty(),
            "no reconciliation after the lost response"
        );
        assert!(
            recon
                .iter()
                .filter_map(|e| e.physical_id.as_deref())
                .all(|x| x == p),
            "{recon:?}"
        );
        let notices = read_notice_ledger(&d.join("notice.jsonl")).expect("notice ledger");
        let cont: Vec<_> = notices
            .iter()
            .filter(|e| e.event == "notice_continuation" && e.logical_job_id.as_deref() == Some(&l))
            .collect();
        assert_eq!(cont.len(), 1, "{notices:?}");
        assert_eq!(cont[0].physical_id.as_deref(), Some(p.as_str()));
        let consumed = all
            .iter()
            .filter(|e| e.event == "consumed" && e.logical_job_id.as_deref() == Some(&l))
            .count();
        assert_eq!(consumed, 1);
    }
}

/// 通知が無ければ再開しない: 外部時計を進める要求を拒否した（= 完了も通知も無い）再開 control は、
/// poll・lookup で完了を探さないので走り切れない。消費・継続は 1 件も起きない。
fn no_notice_probe(arm: &'static str) {
    let file = load_scenarios(&scenarios()).expect("scenarios");
    let seed = file.seed;
    let s = spec(seed);
    let occurrences = externals_of(&s);
    let nth = 1 + seed % occurrences;
    let limits = cfg_limits();
    let (d, _scratch) = run_dir(format!("notice-{arm}-probe"));
    let sd = short_sock_dir();
    let sock = sd.path().join("e.sock");
    let ledger = d.join("ext.jsonl");
    header_for(arm, &s, "external_wait_started", nth, occurrences, &limits)
        .write(&d)
        .expect("header");
    let log = EventLog::create(&d).expect("log");
    let mut sup = Supervisor::new(log.clone(), limits.clone()).expect("sup");
    let mut ext = sup
        .spawn(
            Role::External,
            role_cmd_cfg("external", &sock, &ledger, &ext_cfg(&s)),
        )
        .expect("spawn external");
    wait_ready(&ext);
    let mut c1 = sup
        .spawn(
            Role::Control,
            notice_ctl(arm, &d, &sock, seed, 1, Some((Stop::AfterWait, nth)), false),
        )
        .expect("spawn control");
    wait_line(&c1, "CONTROL_ACKED");
    c1.kill("fault_after_wait_ack").expect("kill");
    assert!(c1.try_exit().expect("wait").is_some());

    let mut c2 = sup
        .spawn(
            Role::Control,
            notice_ctl(arm, &d, &sock, seed, 2, None, true),
        )
        .expect("respawn control");
    wait_line(&c2, "CONTROL_RESUMED");
    // 通知が来ないので、時計を進める要求の拒否で止まる（来ない完了を待ち続けない）。
    let st = c2.wait_exit(PROBE_BOUND).expect("wait");
    match st {
        Some(st) => assert!(!st.success(), "resumed without any notice: {st:?}"),
        None => {
            c2.kill("no_notice_bound").expect("bounded kill");
        }
    }
    let entries = read_ledger(&ledger).expect("ledger");
    assert!(
        !entries.iter().any(|e| e.event == "external_job_completed"),
        "nothing completed"
    );
    assert!(
        !entries.iter().any(|e| e.event == "consumed"),
        "nothing consumed"
    );
    // 起動照会は物理 ID の確認だけ: 完了済みとは返らない（完了していない）。
    assert!(
        entries
            .iter()
            .filter(|e| e.event == "lookup")
            .all(|e| e.detail.as_ref().is_some_and(|d| d["result"] == "running")),
        "no lookup saw a completion"
    );
    let n = read_notice_ledger(&d.join("notice.jsonl")).expect("notice ledger");
    assert_eq!(count_notices(&n).continuations, 0, "{n:?}");
    let mut obs = IpcExt::connect_unix(&sock, "observer", limits.ipc_options());
    ext.stop(Some(&mut || obs.shutdown_server()), "run_done")
        .expect("stop external");
}

#[test]
fn recovery_notice_continuation() {
    for arm in ["baseline", "celeris"] {
        no_notice_probe(arm);
        notice_run(arm, Stop::AfterAccept);
        notice_run(arm, Stop::AfterWait);
    }
}

#[test]
fn recovery_lost_response() {
    for arm in ["baseline", "celeris"] {
        notice_run(arm, Stop::LostResponse);
    }
}

#[test]
fn notice_raw_is_distinguished_from_poll_era_process_raw() {
    let d = tempfile::tempdir().expect("dir");
    let s = spec(1);
    header_for("baseline", &s, "external_wait_started", 1, 1, &cfg_limits())
        .write(d.path())
        .expect("header");
    // 旧版（通知処理証跡なし）として扱われ、新版の再集計は明示的に拒否する。
    assert_eq!(raw_kind(d.path()), Ok(RawKind::PollEraProcess));
    let e = compute_notice(d.path(), "baseline").expect_err("old raw");
    assert!(e.contains("no notice processing evidence"), "{e}");
    // in-process の /1 raw は process raw ですらない。
    let old = tempfile::tempdir().expect("old");
    RunHeader::from_spec(&spec(1), "baseline", BASELINE_SCOPE)
        .write(old.path())
        .expect("old header");
    assert!(raw_kind(old.path()).is_err());
}

// ---- 版付き raw と barrier 順序の回帰 ----

#[test]
fn process_raw_is_versioned_and_old_raw_is_rejected() {
    assert_ne!(PROCESS_RAW_SCHEMA, RAW_SCHEMA);
    assert_eq!(RAW_SCHEMA, "celeris.dagbench-raw/1");
    assert!(PROCESS_RAW_SCHEMA.ends_with("/2"));

    // 旧 in-process raw（/1）は process 集計入口が明示的に拒否する（新方式として読み替えない）。
    let old = tempfile::tempdir().expect("old");
    RunHeader::from_spec(&spec(1), "baseline", BASELINE_SCOPE)
        .write(old.path())
        .expect("old header");
    let e = compute_process(old.path()).expect_err("old raw must be refused");
    assert!(
        e.contains(RAW_SCHEMA) && e.contains("in-process raw"),
        "{e}"
    );
    // /1 の既存の意味は変わらない: /1 は /1 の集計入口で版 check を通る（生ログが無いので別の理由で Err）。
    let e = compute(old.path()).expect_err("no worker log");
    assert!(!e.contains("process raw"), "{e}");

    // /2 は /1 の入口（compute）が拒否し、未知の版は process 入口が拒否する。
    let new = tempfile::tempdir().expect("new");
    std::fs::write(
        new.path().join("run.json"),
        json!({"schema": PROCESS_RAW_SCHEMA}).to_string(),
    )
    .expect("run.json");
    let e = compute(new.path()).expect_err("compute refuses /2");
    assert!(e.contains("process raw"), "{e}");
    std::fs::write(
        new.path().join("run.json"),
        json!({"schema": "celeris.dagbench-raw/3"}).to_string(),
    )
    .expect("run.json");
    let e = compute_process(new.path()).expect_err("unknown version");
    assert!(e.contains("unknown raw schema"), "{e}");
}

fn ev(seq: u64, kind: &str, rest: Value) -> Value {
    let mut v = json!({"seq": seq, "kind": kind});
    v.as_object_mut()
        .expect("obj")
        .extend(rest.as_object().cloned().expect("rest"));
    v
}

fn good_events() -> Vec<Value> {
    let b = |seq: u64, name: &str, rest: Value| {
        let mut r = json!({"name": name});
        r.as_object_mut()
            .expect("obj")
            .extend(rest.as_object().cloned().expect("rest"));
        ev(seq, "barrier", r)
    };
    vec![
        ev(
            1,
            "process_started",
            json!({"role": "external", "pid": 50, "incarnation": 1}),
        ),
        ev(
            2,
            "process_started",
            json!({"role": "control", "pid": 60, "incarnation": 1}),
        ),
        b(
            3,
            B_CONTROL_ACKED,
            json!({"external_pid": 50, "external_incarnation": 1}),
        ),
        ev(
            4,
            "process_stopped",
            json!({"role": "control", "pid": 60, "incarnation": 1, "exit_signal": 9}),
        ),
        b(5, B_CONTROL_EXIT_ACK, json!({"pid": 60})),
        b(6, B_OBSERVER_ADVANCE, json!({})),
        b(
            7,
            B_LEDGER_SYNC_ACK,
            json!({
            "external_pid": 50, "external_incarnation": 1, "ledger_seq": 9,
            "ledger_file_seq": 9, "max_completed_seq": 8, "completed": ["j1"]}),
        ),
        b(8, B_CONTROL_RESUME, json!({})),
        ev(
            9,
            "process_started",
            json!({"role": "control", "pid": 61, "incarnation": 2}),
        ),
        b(10, B_CONTROL_DONE, json!({})),
    ]
}

#[test]
fn survival_barrier_order_is_enforced() {
    let proof = verify_survival_barriers(&good_events()).expect("good");
    assert_eq!(
        (
            proof.control_pid,
            proof.external_pid,
            proof.resumed_incarnation
        ),
        (60, 50, 2)
    );

    let bad = |f: &dyn Fn(&mut Vec<Value>), why: &str| {
        let mut e = good_events();
        f(&mut e);
        let err = verify_survival_barriers(&e).expect_err(why);
        eprintln!("{why}: {err}");
    };
    // 完了 ack が終了 ack より前。
    bad(
        &|e| {
            e[4]["seq"] = json!(7);
            e[6]["seq"] = json!(5);
        },
        "sync before exit",
    );
    // control が SIGKILL されていない。
    bad(&|e| e[3]["exit_signal"] = json!(15), "not killed");
    // 終了 ack と再開の間に control が居る。
    bad(
        &|e| {
            e.insert(
                5,
                ev(
                    6,
                    "process_started",
                    json!({"role": "control", "pid": 62, "incarnation": 3}),
                ),
            )
        },
        "control during absence",
    );
    // 別の External。
    bad(&|e| e[6]["external_pid"] = json!(51), "external replaced");
    // External が完了 ack の前に止まった。
    bad(
        &|e| {
            e.insert(
                5,
                ev(
                    6,
                    "process_stopped",
                    json!({"role": "external", "pid": 50, "incarnation": 1}),
                ),
            )
        },
        "external stopped",
    );
    // 完了が sync 済み seq に覆われていない。
    bad(
        &|e| e[6]["max_completed_seq"] = json!(10),
        "completion not covered",
    );
    // 再開が無い。
    bad(
        &|e| {
            e.remove(8);
        },
        "no restart",
    );
}
