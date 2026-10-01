//! ADR-0114: identity 復元の成功経路を**実** bwrap + chrome-headless-shell で確かめる。
//! `restore_isolated` で開封した state を `deliver_state` で controller の CDP（`Storage.setCookies`）
//! にだけ投入し、controller から cookie が見えることを loopback の origin で確かめる。
//!
//! この host には別 UID が無いので、成功経路は `same-uid-harness` の試験 admission（違反が
//! `SameUid` だけの実 runtime を通す）で実証する。本番 admission（`Attested`）では同じ runtime が
//! `SameUid` で拒否され、開封も投入も起きないことも確かめる。他 project・別 origin・期限切れ・
//! 削除済み・別 session も開封前に拒否される。state は argv・記録ファイルに出ない。
//!
//! 外部ネットワークには出ない（browser は about:blank のまま、origin は 127.0.0.1 の listener）。
//! 前提（bwrap・playwright の chrome-headless-shell）が無い環境では失敗する。
//! `CELERIS_ISOLATION_TESTS=skip` のときだけ「SKIPPED (not passed)」を出して抜ける。
use std::ffi::OsString;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use celeris_credentiald::identity_seal::{IdentitySealer, IdentityStatePlain, StateEntry};
use task_api::browser_identity::{IdentityRegisterInput, IdentityService};
use task_core::SqliteStore;
use task_core::browser_isolation::{
    IsolationViolation, LiveIsolation, LiveSessionEntry, LiveSessionRegistry, LiveSessions,
};
use task_worker::browser_cdp_sink::{CdpController, InjectionError};
use task_worker::browser_live::{CollectingSink, LiveEmitter};
use task_worker::browser_runtime::{IsolatedRuntime, LiveSession, RestoreAdmission, RuntimeSpec};
use task_worker::browser_supervisor::{Supervisor, SupervisorOptions};

const SECRET: &str = "restore-deliver-secret-5f1a";
const OTHER_SECRET: &str = "restore-deliver-other-9b2c";

fn skip() -> bool {
    if std::env::var("CELERIS_ISOLATION_TESTS").as_deref() == Ok("skip") {
        eprintln!("SKIPPED (not passed): CELERIS_ISOLATION_TESTS=skip");
        return true;
    }
    false
}

fn browser() -> PathBuf {
    if let Ok(p) = std::env::var("CELERIS_TEST_BROWSER") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").expect("HOME");
    let base = Path::new(&home).join(".cache/ms-playwright");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&base)
        .expect("playwright cache (agent-browser's browser) is required")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("chromium_headless_shell-"))
        })
        .map(|p| p.join("chrome-headless-shell-linux64/chrome-headless-shell"))
        .filter(|p| p.exists())
        .collect();
    found.sort();
    found.pop().expect("chrome-headless-shell not installed")
}

fn spec(session: &Path, id: &str) -> RuntimeSpec {
    let bwrap =
        PathBuf::from(std::env::var("CELERIS_TEST_BWRAP").unwrap_or("/usr/bin/bwrap".into()));
    assert!(bwrap.exists(), "bwrap not found at {bwrap:?}");
    let exe = browser();
    let install = exe.parent().expect("install dir").to_path_buf();
    let argv = [
        exe.as_os_str().to_owned(),
        "--headless".into(),
        "--no-sandbox".into(),
        "--no-zygote".into(),
        "--disable-gpu".into(),
        "--disable-dev-shm-usage".into(),
        "--remote-debugging-pipe".into(),
        "--user-data-dir=/session/profile".into(),
        "about:blank".into(),
    ];
    RuntimeSpec {
        bwrap,
        session_id: id.into(),
        session_dir: session.join("runtime"),
        ro_dirs: vec![install],
        argv: argv.into_iter().collect::<Vec<OsString>>(),
        cdp_pipe: true,
        egress: None,
    }
}

fn launch(
    session: &Path,
    id: &str,
    registry: &Arc<LiveSessions>,
    admission: RestoreAdmission,
) -> (Supervisor, Arc<Mutex<CdpController>>, Arc<AtomicBool>) {
    let mut opts = SupervisorOptions::new(session.join("records"));
    opts.registry = Some(Arc::clone(registry));
    opts.admission = admission;
    opts.live_key = Some(("task-r".into(), "run-r".into()));
    let stop = Arc::clone(&opts.observation_stop);
    let mut sup = Supervisor::start(spec(session, id), opts).expect("isolated runtime starts");
    let mut controller = CdpController::new(
        sup.cdp_write.take().expect("cdp write"),
        sup.cdp_read.take().expect("cdp read"),
    );
    // browser が上がるまで待つ（CDP pipe の往復）。
    controller
        .controller_command("Browser.getVersion", serde_json::json!({}), None)
        .expect("browser answered over CDP pipe");
    (sup, Arc::new(Mutex::new(controller)), stop)
}

/// loopback の fixture（listener を持ったまま、その origin を identity の origin にする）。
fn loopback_origin() -> (TcpListener, String) {
    let l = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let port = l.local_addr().expect("addr").port();
    (l, format!("https://127.0.0.1:{port}"))
}

fn state(origin: &str, name: &str, value: &str) -> IdentityStatePlain {
    IdentityStatePlain {
        entries: vec![StateEntry {
            origin: origin.into(),
            kind: "cookie".into(),
            name: name.into(),
            value: value.into(),
        }],
    }
}

fn input(
    id: &str,
    project: &str,
    origin: &str,
    ttl: Option<u64>,
    st: IdentityStatePlain,
) -> IdentityRegisterInput {
    IdentityRegisterInput {
        identity_id: id.into(),
        project_id: project.into(),
        origin: origin.into(),
        demand_confirmed_by: Some("rmaeda".into()),
        ttl_secs: ttl,
        state: st,
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

/// controller の CDP から見える cookie（name, value, domain）。
fn cookies(c: &Arc<Mutex<CdpController>>) -> Vec<(String, String, String)> {
    let reply = c
        .lock()
        .expect("controller")
        .controller_command("Storage.getCookies", serde_json::json!({}), None)
        .expect("Storage.getCookies");
    reply["result"]["cookies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap_or_default().to_owned(),
                c["value"].as_str().unwrap_or_default().to_owned(),
                c["domain"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// runtime の process の argv と記録ファイルに state が出ていない。
fn assert_state_not_in_argv_or_record(sup: &Supervisor) {
    let procs = sup.processes();
    assert!(!procs.is_empty(), "runtime processes recorded");
    for p in &procs {
        let cmdline = std::fs::read(format!("/proc/{}/cmdline", p.pid)).unwrap_or_default();
        let cmdline = String::from_utf8_lossy(&cmdline);
        assert!(
            !cmdline.contains(SECRET),
            "state leaked into argv of {}",
            p.role
        );
        assert!(!cmdline.contains("--restore") && !cmdline.contains("--state="));
    }
    let record = std::fs::read_to_string(sup.record_path()).unwrap_or_default();
    assert!(!record.contains(SECRET), "state leaked into the record");
}

#[test]
fn supervisor_entry_delivers_restored_state_to_controller_cdp_under_harness_admission() {
    if skip() {
        return;
    }
    let (_fixture, origin) = loopback_origin();
    let keys = tempfile::tempdir().expect("keys");
    let sealer = IdentitySealer::open(keys.path().join("keys")).expect("sealer");
    let store = SqliteStore::open_in_memory().expect("store");
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    let t = now();
    for (id, project, ttl, name, value) in [
        ("a", "proj", None, "sid", SECRET),
        ("b", "other", None, "sid_b", OTHER_SECRET),
        ("short", "proj-short", Some(1), "sid_short", OTHER_SECRET),
        ("gone", "proj-gone", None, "sid_gone", OTHER_SECRET),
    ] {
        svc.register(
            input(id, project, &origin, ttl, state(&origin, name, value)),
            t,
        )
        .expect("register");
    }
    svc.delete("gone").expect("delete");

    let registry = Arc::new(LiveSessions::default());
    let session = tempfile::tempdir().expect("session");
    let (sup, controller, _) = launch(
        session.path(),
        "live-h",
        &registry,
        RestoreAdmission::SameUidHarness,
    );
    let entry = registry
        .get("live-h")
        .expect("supervisor registered the session");
    let reg: &dyn LiveSessionRegistry = &*registry;

    // controller を渡す前は投入口が無いので開封しない。
    assert!(!entry.accepts_state());
    let err = svc
        .restore_in_session("a", "proj", &origin, "live-h", Some(reg), t)
        .unwrap_err();
    assert_eq!(err.code(), "isolation_required");
    assert!(cookies(&controller).iter().all(|c| c.1 != SECRET));

    sup.attach_controller(Arc::clone(&controller));
    assert!(entry.accepts_state());
    // 試験 admission は実 runtime の事実から attestation を作る（違反は SameUid だけ）。
    let att = entry.current_attestation().expect("harness admission");
    assert_eq!(att.session_id(), "live-h");

    // 拒否経路: 他 project・別 origin・期限切れ・削除済み・別 session。どれも投入しない。
    // 期限切れは sweep で失効（identity_revoked）になる。
    let denied = [
        ("b", "proj", origin.clone(), "live-h", t, "other_project"),
        ("a", "other", origin.clone(), "live-h", t, "other_project"),
        (
            "a",
            "proj",
            "https://127.0.0.2:1".to_owned(),
            "live-h",
            t,
            "other_origin",
        ),
        (
            "short",
            "proj-short",
            origin.clone(),
            "live-h",
            t + 10,
            "identity_revoked",
        ),
        (
            "gone",
            "proj-gone",
            origin.clone(),
            "live-h",
            t,
            "identity_deleted",
        ),
        (
            "a",
            "proj",
            origin.clone(),
            "live-other",
            t,
            "isolation_required",
        ),
    ];
    for (id, project, o, sess, at, code) in &denied {
        let err = svc
            .restore_in_session(id, project, o, sess, Some(reg), *at)
            .unwrap_err();
        eprintln!(
            "denied: id={id} project={project} origin={o} session={sess} -> {}",
            err.code()
        );
        assert_eq!(err.code(), *code);
    }
    let before = cookies(&controller);
    assert!(
        before.iter().all(|c| c.1 != SECRET && c.1 != OTHER_SECRET),
        "no state was delivered on a refused path"
    );

    // 成功経路: 開封 → controller の CDP（Storage.setCookies）へ投入。応答に state は無い。
    svc.restore_in_session("a", "proj", &origin, "live-h", Some(reg), t)
        .expect("restore delivers into the live session");
    let after = cookies(&controller);
    assert!(
        after
            .iter()
            .any(|(n, v, d)| n == "sid" && v == SECRET && d == "127.0.0.1"),
        "controller sees the restored cookie: {:?}",
        after.iter().map(|c| (&c.0, &c.2)).collect::<Vec<_>>()
    );
    assert!(after.iter().all(|c| c.1 != OTHER_SECRET));
    assert_state_not_in_argv_or_record(&sup);
    eprintln!("delivered: identity a -> session live-h, cookie sid visible to controller CDP");

    // 停止後は registry から外れ、投入口も閉じる。
    let held = Arc::clone(&entry);
    sup.stop();
    assert!(registry.get("live-h").is_none());
    assert!(!held.accepts_state());
}

#[test]
fn identity_restore_sameuid_rejected_in_production() {
    if skip() {
        return;
    }
    let (_fixture, origin) = loopback_origin();
    let keys = tempfile::tempdir().expect("keys");
    let sealer = IdentitySealer::open(keys.path().join("keys")).expect("sealer");
    let store = SqliteStore::open_in_memory().expect("store");
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    let t = now();
    svc.register(
        input("a", "proj", &origin, None, state(&origin, "sid", SECRET)),
        t,
    )
    .expect("register");

    let registry = Arc::new(LiveSessions::default());
    let session = tempfile::tempdir().expect("session");
    let (sup, controller, _) = launch(
        session.path(),
        "live-p",
        &registry,
        RestoreAdmission::Attested,
    );
    sup.attach_controller(Arc::clone(&controller));
    let entry = registry.get("live-p").expect("registered");
    assert!(entry.accepts_state());
    // 本番 admission: この host の同一 UID と daemon 所有 userns は拒否する。
    assert_eq!(
        entry.current_attestation().unwrap_err(),
        vec![
            IsolationViolation::SameUid,
            IsolationViolation::UsernsOwnedByDaemon,
        ]
    );
    let reg: &dyn LiveSessionRegistry = &*registry;
    let err = svc
        .restore_in_session("a", "proj", &origin, "live-p", Some(reg), t)
        .unwrap_err();
    assert_eq!(err.code(), "isolation_required");
    let live: &dyn LiveIsolation = &*entry;
    let err = svc
        .restore_for_session("a", "proj", &origin, "live-p", live, t)
        .unwrap_err();
    assert_eq!(err.code(), "isolation_required");
    assert!(cookies(&controller).iter().all(|c| c.1 != SECRET));
    eprintln!(
        "attested admission refused SameUid session live-p: isolation_required, nothing delivered"
    );
    sup.stop();
}

#[test]
fn live_session_delivers_restored_state_over_its_own_cdp_pipe() {
    if skip() {
        return;
    }
    let (_fixture, origin) = loopback_origin();
    let keys = tempfile::tempdir().expect("keys");
    let sealer = IdentitySealer::open(keys.path().join("keys")).expect("sealer");
    let store = SqliteStore::open_in_memory().expect("store");
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    let t = now();
    svc.register(
        input("a", "proj", &origin, None, state(&origin, "sid", SECRET)),
        t,
    )
    .expect("register");

    let session = tempfile::tempdir().expect("session");
    let rt = IsolatedRuntime::launch(&spec(session.path(), "live-d")).expect("runtime");
    let live = LiveSession(Mutex::new(rt));
    assert!(live.accepts_state());
    // bwrap の setup が終わるまで待つ（setup 途中の事実では判定しない）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let facts = loop {
        let facts = live.0.lock().expect("rt").facts().expect("facts");
        if RestoreAdmission::Attested.admit(&facts).err()
            == Some(vec![
                IsolationViolation::SameUid,
                IsolationViolation::UsernsOwnedByDaemon,
            ])
            || std::time::Instant::now() > deadline
        {
            break facts;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert_eq!(
        RestoreAdmission::Attested.admit(&facts).unwrap_err(),
        vec![
            IsolationViolation::SameUid,
            IsolationViolation::UsernsOwnedByDaemon,
        ]
    );
    let att = RestoreAdmission::SameUidHarness
        .admit(&facts)
        .expect("harness admission");
    // 他 project は開封前に拒否。
    assert_eq!(
        svc.restore_isolated("a", "other", &origin, &att, t)
            .unwrap_err()
            .code(),
        "other_project"
    );
    let plain = svc
        .restore_isolated("a", "proj", &origin, &att, t)
        .expect("open under harness admission");
    let bytes = zeroize::Zeroizing::new(serde_json::to_vec(&plain).expect("json"));
    drop(plain);
    live.deliver_state(&bytes)
        .expect("delivered over the runtime's CDP pipe");
    // 知らない種別は 1 件も投入しない。
    let bad = serde_json::to_vec(&state(&origin, "x", "y")).expect("json");
    let bad = String::from_utf8(bad)
        .expect("utf8")
        .replace("\"cookie\"", "\"file\"");
    assert!(live.deliver_state(bad.as_bytes()).is_err());

    let mut rt = live.0.into_inner().expect("rt");
    let c = Arc::new(Mutex::new(CdpController::new(
        rt.cdp_write.take().expect("w"),
        rt.cdp_read.take().expect("r"),
    )));
    let seen = cookies(&c);
    assert!(
        seen.iter()
            .any(|(n, v, d)| n == "sid" && v == SECRET && d == "127.0.0.1")
    );
    assert!(seen.iter().all(|(n, _, _)| n != "x"));
    eprintln!("delivered: LiveSession live-d, cookie sid visible over CDP pipe");
    rt.kill();
}

/// ADR-0080 H3 / ADR-0101 D4: 復元を受けた session は、session の終わりまで agent 由来の観測
/// （snapshot・console・event）を拒否・破棄する。投入前は観測が通り、拒否された復元は停止に入らない。
#[test]
fn restored_session_refuses_agent_observation() {
    if skip() {
        return;
    }
    let (_fixture, origin) = loopback_origin();
    let keys = tempfile::tempdir().expect("keys");
    let sealer = IdentitySealer::open(keys.path().join("keys")).expect("sealer");
    let store = SqliteStore::open_in_memory().expect("store");
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    let t = now();
    svc.register(
        input("a", "proj", &origin, None, state(&origin, "sid", SECRET)),
        t,
    )
    .expect("register");

    let registry = Arc::new(LiveSessions::default());
    let session = tempfile::tempdir().expect("session");
    let (sup, controller, stop) = launch(
        session.path(),
        "live-o",
        &registry,
        RestoreAdmission::SameUidHarness,
    );
    sup.attach_controller(Arc::clone(&controller));
    let reg: &dyn LiveSessionRegistry = &*registry;
    let emitter = LiveEmitter::with_observation_stop(CollectingSink::default(), Arc::clone(&stop));
    let observe = |c: &Arc<Mutex<CdpController>>| {
        let mut c = c.lock().expect("controller");
        let snapshot = c
            .agent_command("Target.getTargets", serde_json::json!({}), None)
            .map(|_| ());
        (snapshot, c.take_agent_events())
    };

    // 投入前: agent の観測（browser 全体の target 一覧）は通る。
    let (snap, _) = observe(&controller);
    assert_eq!(snap, Ok(()), "observation works before restore");
    assert!(emitter.emit(&task_core::browser_live::LiveEvent::Status {
        state: "browser.navigate: success".into()
    }));
    // 拒否された復元（他 project）は停止に入らない（開封前に落ちる）。
    assert_eq!(
        svc.restore_in_session("a", "other", &origin, "live-o", Some(reg), t)
            .unwrap_err()
            .code(),
        "other_project"
    );
    assert!(!stop.load(Ordering::SeqCst));
    assert!(!controller.lock().expect("controller").observation_stopped());

    // 成功した復元: 投入の前に停止へ入り、session の終わりまで解除されない。
    svc.restore_in_session("a", "proj", &origin, "live-o", Some(reg), t)
        .expect("restore delivers under harness admission");
    assert!(
        stop.load(Ordering::SeqCst),
        "entry raised the observation stop"
    );
    assert!(controller.lock().expect("controller").observation_stopped());
    for _ in 0..3 {
        let (snap, events) = observe(&controller);
        assert_eq!(snap, Err(InjectionError::AuthSectionRequired));
        assert!(events.is_empty(), "console/events are dropped");
    }
    // Page のスナップショット系も同じく拒否（method を問わない）。
    for method in [
        "Page.captureScreenshot",
        "DOM.getDocument",
        "Runtime.evaluate",
        "Storage.getCookies",
    ] {
        assert_eq!(
            controller
                .lock()
                .expect("controller")
                .agent_command(method, serde_json::json!({}), None)
                .map(|_| ()),
            Err(InjectionError::AuthSectionRequired),
            "{method} refused after restore"
        );
    }
    // worker 側の live event・progress・artifact の出口（LiveEmitter）も閉じる。
    assert!(emitter.in_auth_section());
    assert!(!emitter.emit(&task_core::browser_live::LiveEvent::Console {
        level: "log".into(),
        text: "obs".into()
    }));
    assert_eq!(
        emitter.sink().events().len(),
        1,
        "only the pre-restore event"
    );
    // controller（信頼側）は使えるので、復元した cookie は controller からだけ見える。
    assert!(cookies(&controller).iter().any(|c| c.1 == SECRET));
    // 認証区間の開閉でも解除されない（解除の口は session の終わりだけ）。
    {
        let mut c = controller.lock().expect("controller");
        c.open_auth_section("h3".into());
        c.close_auth_section().expect("close");
        assert!(c.observation_stopped());
    }
    assert_eq!(
        observe(&controller).0,
        Err(InjectionError::AuthSectionRequired)
    );
    eprintln!("restored session live-o: agent observation refused, events dropped until stop");
    sup.stop();
    assert!(registry.get("live-o").is_none());
    assert!(stop.load(Ordering::SeqCst), "not cleared by stopping");
}
