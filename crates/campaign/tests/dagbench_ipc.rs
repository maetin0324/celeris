//! ExtSim の IPC adapter: baseline / CelerisSide が in-process と IPC の両 backend で同じ結果になること、
//! 通知の seq + ack + 再送が冪等なこと、切断・期限超過が有界に Err になること。
//! 順序は request/response と accept（出来事）だけで決まり、sleep で作らない。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::{
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    dagbench::{
        baseline::{Baseline, BaselineConfig, Outcome, load_scenario},
        celeris_side::{CelerisSide, Paths, RunSpec, load_scenarios},
        ext::*,
        ipc::*,
    },
};
use serde_json::Value;
use task_core::TaskStore;

fn cfg(faults: Vec<FaultSpec>) -> ExtConfig {
    ExtConfig {
        poll_secs: 30,
        job_slots: 2,
        faults,
    }
}

/// unix socket の path は 107 byte までなので、socket だけ短い dir に置く（中身は空の socket file のみ）。
fn sock_dir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("ipc")
        .tempdir_in("/tmp")
        .unwrap()
}

fn opts() -> IpcOptions {
    IpcOptions {
        io_timeout: Duration::from_secs(5),
        retries: 1,
    }
}

fn scenarios() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/dagbench/scenarios.json")
}

fn run_baseline(ext: &mut dyn ExtBackend, dir: &Path) -> Option<u64> {
    let sc = load_scenario(&scenarios(), "k2-mini").unwrap();
    let mut b = Baseline::open(
        sc,
        BaselineConfig {
            poll_secs: 30,
            ext_job_secs: 330,
        },
        &dir.join("state.json"),
    )
    .unwrap();
    assert_eq!(b.run(ext).unwrap(), Outcome::Complete);
    b.makespan()
}

fn dup_fault() -> FaultSpec {
    FaultSpec {
        fault_id: "c1".into(),
        kind: FaultKind::DuplicateNotice { extra: 1 },
        event: "external_job_completed".into(),
        seed: 0,
        occurrences: 1,
    }
}

#[test]
fn baseline_runs_over_ipc_with_the_same_result_as_in_process() {
    let a = tempfile::tempdir().unwrap();
    let mut sim = ExtSim::open(&a.path().join("ext.jsonl"), cfg(vec![dup_fault()])).unwrap();
    let local = run_baseline(&mut sim, a.path());

    let b = tempfile::tempdir().unwrap();
    let sd = sock_dir();
    let sock = sd.path().join("e.sock");
    let ledger = b.path().join("ext.jsonl");
    let server = spawn_server(&sock, &ledger, cfg(vec![dup_fault()])).unwrap();
    let mut ipc = IpcExt::connect_unix(&sock, "control", opts());
    let remote = run_baseline(&mut ipc, b.path());
    ipc.shutdown_server().unwrap();
    server.join().unwrap();

    assert_eq!(local, remote);
    assert_eq!(
        count(&read_ledger(&a.path().join("ext.jsonl")).unwrap()),
        count(&read_ledger(&ledger).unwrap())
    );
}

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

#[test]
fn celeris_side_runs_over_ipc() {
    let file = load_scenarios(&scenarios()).unwrap();
    let spec = RunSpec::resolve(&file, "k2-mini", "camp-1", file.seed).unwrap();
    let env = api_common::admin_env();
    let out = env.dir.path().join("dagbench");
    std::fs::create_dir_all(&out).unwrap();
    let paths = Paths::under(&env.db_path, &out);
    let store: Arc<dyn TaskStore> = Arc::new(task_core::SqliteStore::open(&env.db_path).unwrap());
    let sd = sock_dir();
    let sock = sd.path().join("e.sock");
    let slots = spec.scenario.resources.external_job_slots;
    let server = spawn_server(
        &sock,
        &paths.ext_ledger,
        ExtConfig {
            poll_secs: spec.poll_secs,
            job_slots: slots,
            faults: vec![],
        },
    )
    .unwrap();
    let ipc = IpcExt::connect_unix(&sock, "control", opts());
    let transport = RouterTransport(env.router());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let ledger = paths.ext_ledger.clone();
    let report = rt.block_on(async {
        let mut side = CelerisSide::with_backend(spec, paths, store, Box::new(ipc)).unwrap();
        side.drive(&transport).await
    });
    let report = report.unwrap_or_else(|e| panic!("drive failed: {e}"));
    assert!(report.campaigns.iter().all(|c| c.status == "done"));
    IpcExt::connect_unix(&sock, "observer", opts())
        .shutdown_server()
        .unwrap();
    server.join().unwrap();
    let c = count(&read_ledger(&ledger).unwrap());
    assert!(c.logical_jobs > 0);
    assert_eq!(c.physical_launches, c.logical_jobs);
    assert_eq!(c.consumed, c.logical_jobs);
}

#[test]
fn notices_carry_seq_and_are_resent_until_acked_idempotently() {
    let d = tempfile::tempdir().unwrap();
    let sd = sock_dir();
    let sock = sd.path().join("e.sock");
    let server = spawn_server(&sock, &d.path().join("ext.jsonl"), cfg(vec![dup_fault()])).unwrap();
    let mut c = IpcExt::connect_unix(&sock, "control", opts());
    c.submit("j1", 60).unwrap().unwrap();
    assert!(c.advance(59).unwrap().is_empty());
    let first = c.advance(60).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(c.acked_seq(), 1);
    // 新しい client（制御の作り直し）は ack 前の通知を再送で受け取る = at-least-once。
    let mut c2 = IpcExt::connect_unix(&sock, "control2", opts());
    let again = c2.advance(60).unwrap();
    assert_eq!(again, first);
    // 元の client の次の要求が ack=1 を運び、同じ時刻の再 advance は何も渡さない（seq で冪等）。
    assert!(c.advance(60).unwrap().is_empty());
    // 重複通知（copy 1）は +30 秒後に新しい seq で来る。
    let dup = c2.advance(90).unwrap();
    assert_eq!(dup.len(), 1);
    assert_eq!(dup[0].copy, 1);
    assert_eq!(c2.acked_seq(), 2);
    // ack で outbox が空になった後は、新しい client にも何も再送されない。
    c2.flush_ack().unwrap();
    let mut c3 = IpcExt::connect_unix(&sock, "control3", opts());
    assert!(c3.advance(90).unwrap().is_empty());
    c3.shutdown_server().unwrap();
    server.join().unwrap();
}

#[test]
fn retried_request_id_is_not_executed_twice() {
    let d = tempfile::tempdir().unwrap();
    let sd = sock_dir();
    let sock = sd.path().join("e.sock");
    let ledger = d.path().join("ext.jsonl");
    let server = spawn_server(&sock, &ledger, cfg(vec![])).unwrap();
    let mut c = IpcExt::connect_unix(&sock, "control", opts());
    c.submit("j1", 60).unwrap().unwrap();
    // 同じ (client, id) の生の再送: 応答は同じで ledger は増えない。
    let mut raw = std::os::unix::net::UnixStream::connect(&sock).unwrap();
    let req = Request {
        v: PROTOCOL_VERSION,
        client: "raw".into(),
        id: 1,
        ack: 0,
        op: Op::Submit {
            logical: "j2".into(),
            job_secs: 60,
        },
    };
    let line = format!("{}\n", serde_json::to_string(&req).unwrap());
    let mut r = std::io::BufReader::new(raw.try_clone().unwrap());
    let mut read = || {
        let mut s = String::new();
        std::io::BufRead::read_line(&mut r, &mut s).unwrap();
        s
    };
    std::io::Write::write_all(&mut raw, line.as_bytes()).unwrap();
    let a = read();
    std::io::Write::write_all(&mut raw, line.as_bytes()).unwrap();
    assert_eq!(a, read());
    // 版違いは黙って解釈せず拒否される。
    let bad = Request {
        v: 99,
        id: 2,
        ..req
    };
    let l = format!("{}\n", serde_json::to_string(&bad).unwrap());
    std::io::Write::write_all(&mut raw, l.as_bytes()).unwrap();
    assert!(read().contains("not supported"));
    c.shutdown_server().unwrap();
    server.join().unwrap();
    assert_eq!(count(&read_ledger(&ledger).unwrap()).physical_launches, 2);
}

#[test]
fn disconnect_and_timeout_return_err_within_bounds() {
    let d = tempfile::tempdir().unwrap();
    // 1. server なし: 接続拒否で Err。
    let mut gone = IpcExt::connect_unix(&sock_dir().path().join("n.sock"), "c", opts());
    assert!(gone.now().is_err());

    // 2. 受け付けるが応答しない server: io_timeout × (retries+1) で Err。accept は出来事として数える。
    let sd = sock_dir();
    let sock = sd.path().join("m.sock");
    let l = UnixListener::bind(&sock).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || {
        let mut held = Vec::new();
        for s in l.incoming().take(2) {
            held.push(s.unwrap());
            tx.send(()).unwrap();
        }
        held
    });
    let o = IpcOptions {
        io_timeout: Duration::from_millis(200),
        retries: 1,
    };
    let mut c = IpcExt::connect_unix(&sock, "c", o);
    let t0 = Instant::now();
    let e = c.now().unwrap_err();
    assert!(matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ));
    assert!(t0.elapsed() < Duration::from_secs(5));
    assert_eq!(rx.try_iter().count(), 2, "one reconnect per attempt");
    drop(t.join().unwrap());

    // 3. 稼働中に server が切断: Err（BrokenPipe 系 / 接続拒否）。
    let sd = sock_dir();
    let sock = sd.path().join("e.sock");
    let server = spawn_server(&sock, &d.path().join("ext.jsonl"), cfg(vec![])).unwrap();
    let mut c = IpcExt::connect_unix(&sock, "c", opts());
    assert_eq!(c.now().unwrap(), 0);
    c.shutdown_server().unwrap();
    server.join().unwrap();
    assert!(c.now().is_err());
}

// ---- 通知主導（push の有界 blocking receive + 永続適用後 ack + 処理証跡 ledger）----

fn channel(sock: &Path, ledger: &Path, side: &str, client: &str) -> NoticeChannel {
    NoticeChannel::new(
        IpcExt::connect_unix(sock, client, opts()),
        NoticeLedger::open(ledger, side).unwrap(),
        20,
    )
}

#[test]
fn dagbench_notice_receive_is_bounded_and_ack_is_selective() {
    let d = tempfile::tempdir().unwrap();
    let sd = sock_dir();
    let sock = sd.path().join("e.sock");
    let server = spawn_server(&sock, &d.path().join("ext.jsonl"), cfg(vec![])).unwrap();
    let mut ctl = IpcExt::connect_unix(&sock, "control", opts());
    let mut rx = IpcExt::connect_unix(&sock, "notice-rx", opts());
    // 通知が無ければ上限で空を返す（無限に待たない・時計は進めない）。
    let t = Instant::now();
    assert!(rx.receive(60, 0).unwrap().is_empty());
    assert!(t.elapsed() < Duration::from_secs(3));
    ctl.submit("j1", 60).unwrap().unwrap();
    ctl.submit("j2", 60).unwrap().unwrap();
    assert!(rx.receive(0, 0).unwrap().is_empty());
    // 外部時計を進める側（advance）とは別に、push で届く。
    ctl.advance(60).unwrap();
    let got = rx.receive(1000, 0).unwrap();
    assert_eq!(got.len(), 2);
    // advance 側の ack（piggyback）は push の outbox を減らさない。未 ack は再配送される。
    ctl.flush_ack().unwrap();
    assert_eq!(rx.receive(0, 0).unwrap(), got);
    assert!(rx.receive(0, got[1].seq).unwrap().is_empty());
    // 選択的 ack: 1 件だけ捨てる。
    rx.ack_notice(got[0].seq).unwrap();
    assert_eq!(rx.receive(0, 0).unwrap(), vec![got[1].clone()]);
    rx.ack_notice(got[1].seq).unwrap();
    assert!(rx.receive(0, 0).unwrap().is_empty());
    rx.shutdown_server().unwrap();
    server.join().unwrap();
}

#[test]
fn dagbench_notice_restart_between_apply_and_ack_does_not_apply_twice() {
    let d = tempfile::tempdir().unwrap();
    let sd = sock_dir();
    let sock = sd.path().join("e.sock");
    let nl = d.path().join("notice.jsonl");
    let server = spawn_server(&sock, &d.path().join("ext.jsonl"), cfg(vec![])).unwrap();
    let mut ctl = IpcExt::connect_unix(&sock, "control", opts());
    ctl.submit("j1", 10).unwrap().unwrap();
    ctl.advance(10).unwrap();

    // 1 回目の incarnation: 受信して永続適用（processed）まで記録したが、ack の前に止まる。
    let mut a = channel(&sock, &nl, "celeris", "notice-1");
    let got = a.receive().unwrap();
    assert_eq!(got.len(), 1);
    a.ledger_mut()
        .append(
            "notice_processed",
            Some(got[0].seq),
            Some(0),
            Some("j1"),
            Some("phys-1"),
            &NoticeIds::default(),
            Some("finished"),
        )
        .unwrap();
    drop(a);

    // 再起動: server は未 ack を再配送する。processed 済みの seq は適用せず ack だけ返す。
    let mut b = channel(&sock, &nl, "celeris", "notice-2");
    assert!(b.ledger().processed(got[0].seq));
    let again = b.receive().unwrap();
    assert_eq!(again, got);
    b.settle(
        &again[0],
        &Settled {
            outcome: "redelivered".into(),
            suppressed: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(b.receive().unwrap().is_empty());
    let entries = read_notice_ledger(&nl).unwrap();
    let c = count_notices(&entries);
    assert_eq!(c.received, 2, "{entries:?}");
    assert_eq!(c.suppressed, 1);
    assert_eq!(c.acked, 1);
    assert!(entries.iter().all(|e| e.role == "control_consume"));
    ctl.shutdown_server().unwrap();
    server.join().unwrap();
}

#[test]
fn dagbench_notice_baseline_resumes_by_push_and_suppresses_duplicates() {
    let d = tempfile::tempdir().unwrap();
    let sd = sock_dir();
    let sock = sd.path().join("e.sock");
    let ext_ledger = d.path().join("ext.jsonl");
    let nl = d.path().join("notice.jsonl");
    let server = spawn_server(&sock, &ext_ledger, cfg(vec![dup_fault()])).unwrap();
    let mut ipc = IpcExt::connect_unix(&sock, "control", opts());
    let sc = load_scenario(&scenarios(), "k2-mini").unwrap();
    let mut b = Baseline::open(
        sc,
        BaselineConfig {
            poll_secs: 30,
            ext_job_secs: 330,
        },
        &d.path().join("state.json"),
    )
    .unwrap();
    b.set_notices(channel(&sock, &nl, "baseline", "notice-rx"));
    assert_eq!(b.run(&mut ipc).unwrap(), Outcome::Complete);
    ipc.shutdown_server().unwrap();
    server.join().unwrap();

    let ext = count(&read_ledger(&ext_ledger).unwrap());
    let n = read_notice_ledger(&nl).unwrap();
    let c = count_notices(&n);
    assert!(ext.logical_jobs > 0);
    assert_eq!(ext.consumed, ext.logical_jobs);
    assert_eq!(ext.physical_launches, ext.logical_jobs);
    assert_eq!(ext.lookups, 0, "completion is not searched by lookup");
    // 独立観測の notice_sent（外部が送った）と、制御側が消費した記録（control_consume）は別 file。
    assert_eq!(c.received, ext.notices_sent);
    assert_eq!(c.processed, c.received);
    assert_eq!(c.acked, c.received);
    assert_eq!(c.continuations, ext.logical_jobs);
    assert!(c.suppressed >= 1, "the duplicate copy is suppressed: {n:?}");
    assert!(
        n.iter()
            .all(|e| e.side == "baseline" && e.role == "control_consume")
    );
}

#[test]
fn dagbench_notice_celeris_side_resumes_by_push_with_polling_disabled() {
    let file = load_scenarios(&scenarios()).unwrap();
    let spec = RunSpec::resolve(&file, "k2-mini", "camp-1", file.seed).unwrap();
    let env = api_common::admin_env();
    let out = env.dir.path().join("dagbench");
    std::fs::create_dir_all(&out).unwrap();
    let paths = Paths::under(&env.db_path, &out);
    let store: Arc<dyn TaskStore> = Arc::new(task_core::SqliteStore::open(&env.db_path).unwrap());
    let sd = sock_dir();
    let sock = sd.path().join("e.sock");
    let nl = out.join("notice.jsonl");
    let server = spawn_server(
        &sock,
        &paths.ext_ledger,
        ExtConfig {
            poll_secs: spec.poll_secs,
            job_slots: spec.scenario.resources.external_job_slots,
            faults: vec![dup_fault()],
        },
    )
    .unwrap();
    let ipc = IpcExt::connect_unix(&sock, "control", opts());
    let transport = RouterTransport(env.router());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let ledger = paths.ext_ledger.clone();
    let (report, attempts) = rt.block_on(async {
        let mut side = CelerisSide::with_backend(spec, paths, store, Box::new(ipc)).unwrap();
        side.enable_notices(channel(&sock, &nl, "celeris", "notice-rx"), true);
        let r = side.drive(&transport).await;
        (r, side.poll_attempts())
    });
    let report = report.unwrap_or_else(|e| panic!("drive failed: {e}"));
    assert!(report.campaigns.iter().all(|c| c.status == "done"));
    IpcExt::connect_unix(&sock, "observer", opts())
        .shutdown_server()
        .unwrap();
    server.join().unwrap();

    let ext = count(&read_ledger(&ledger).unwrap());
    let n = read_notice_ledger(&nl).unwrap();
    let c = count_notices(&n);
    assert!(ext.logical_jobs > 0);
    assert_eq!(ext.consumed, ext.logical_jobs);
    assert_eq!(ext.physical_launches, ext.logical_jobs);
    assert_eq!(
        ext.lookups, 0,
        "poll was disabled; nothing looked completion up"
    );
    assert_eq!(
        c.continuations, ext.logical_jobs,
        "one continuation per job: {n:?}"
    );
    assert_eq!(c.acked, c.processed);
    assert!(c.suppressed >= 1, "{n:?}");
    // 継続の記録は wait / task / run の ID を持つ。
    assert!(
        n.iter()
            .filter(|e| e.event == "notice_continuation")
            .all(|e| {
                e.wait_id.is_some()
                    && e.task_id.is_some()
                    && e.run_id.is_some()
                    && e.logical_job_id.is_some()
            })
    );
    let _ = attempts; // poll は試みられても失敗するだけで、再開には使われない。
}
