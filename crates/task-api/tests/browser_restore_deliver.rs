//! ADR-0080 H3 / ADR-0101 D4: identity を復元した session は、credential を注入した session と
//! 同じく session の終わりまで LLM の観測と Live View を止める。HTTP の復元
//! （`POST /api/v1/browser/identities/{id}/restore`）が、実隔離 session（bwrap +
//! chrome-headless-shell、supervisor が registry に登録）への投入の前に observation_stopped を
//! 記録し、Live View の既存・新規接続と worker の live event が `observation_stopped` で拒否される。
//!
//! この host には別 UID が無いので、成功経路は `same-uid-harness` の試験 admission で実証する
//! （本番の `Attested` は SameUid を拒否する。task-worker の `identity_restore_sameuid_rejected_in_production`）。
//! 実 runtime が無い環境では失敗する。`CELERIS_ISOLATION_TESTS=skip` のときだけ
//! 「SKIPPED (not passed)」を出して抜ける。

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use celeris_credentiald::identity_seal::{IdentitySealer, IdentityStatePlain, StateEntry};
use common::*;
use ring::signature::KeyPair;
use serde_json::{Value, json};
use task_api::browser::BrowserApiConfig;
use task_api::browser_identity::{IdentityRegisterInput, IdentityService};
use task_core::browser_isolation::LiveSessions;
use task_core::{RunIndexRole, RunIndexStatus, RunRow, Status, TaskKind, TaskStore};
use task_worker::browser_cdp_sink::CdpController;
use task_worker::browser_runtime::{RestoreAdmission, RuntimeSpec, UsernsMode};
use task_worker::browser_supervisor::{Supervisor, SupervisorOptions};
use time::OffsetDateTime;

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

fn launch(session: &Path, id: &str, opts: SupervisorOptions) -> Supervisor {
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
    let spec = RuntimeSpec {
        bwrap,
        session_id: id.into(),
        session_dir: session.join("runtime"),
        ro_dirs: vec![install],
        argv: argv.into_iter().collect::<Vec<OsString>>(),
        cdp_pipe: true,
        egress: None,
        userns: UsernsMode::Unshare,
    };
    Supervisor::start(spec, opts).expect("isolated runtime starts")
}

fn sign(key: &ring::signature::Ed25519KeyPair, task: &str, run: &str, browser: &str) -> Value {
    let payload = json!({"task_id":task,"run_id":run,"browser_session_id":browser,
        "owner_session_id":"owner","owner_session":true,"origin_ok":true,
        "expires_at":OffsetDateTime::now_utc().unix_timestamp()+30})
    .to_string();
    let signature: String = key
        .sign(payload.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    json!({"payload":payload,"signature":signature})
}

fn live(task: &str, run: &str, session: &str, method: &str) -> String {
    format!("/api/v1/tasks/{task}/browser/live/{run}/{session}/{method}")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_enters_observation_stop_until_session_end() {
    if skip() {
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback");
    let origin = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().expect("addr").port()
    );
    let env = admin_env();
    let key = {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("pkcs8");
        ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("key")
    };
    let keys = tempfile::tempdir().expect("keys");
    let sealer = Arc::new(IdentitySealer::open(keys.path().join("keys")).expect("sealer"));
    let registry = Arc::new(LiveSessions::default());
    let app = task_api::router(
        env.state
            .clone()
            .with_browser(BrowserApiConfig {
                attestation_public_key: Some(key.public_key().as_ref().to_vec()),
                broker: None,
            })
            .with_identity_sealer(Arc::clone(&sealer))
            .with_live_sessions(registry.clone()),
    );

    // 稼働中の task・run と、その (task, run) を鍵に持つ実隔離 session。
    let task = new_task(TaskKind::Execute, Status::Ready);
    env.seed(&task);
    env.store
        .acquire_lease(task.id, "r1", std::time::Duration::from_secs(60))
        .expect("lease");
    let id = task.id.to_string();
    env.store
        .run_index_start(RunRow {
            run_id: "r1".into(),
            task_id: id.clone(),
            work_unit_id: None,
            role: RunIndexRole::Worker,
            seq: 1,
            status: RunIndexStatus::Running,
            adapter: None,
            model: None,
            account: None,
            session_id: None,
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: "2026-09-30T00:00:00Z".into(),
            finished_at: None,
        })
        .expect("run");
    let session = tempfile::tempdir().expect("session");
    let mut opts = SupervisorOptions::new(session.path().join("records"));
    opts.registry = Some(Arc::clone(&registry));
    opts.admission = RestoreAdmission::SameUidHarness;
    opts.live_key = Some((id.clone(), "r1".into()));
    let stop = Arc::clone(&opts.observation_stop);
    let mut sup = launch(session.path(), "live-s", opts);
    let controller = Arc::new(std::sync::Mutex::new(CdpController::new(
        sup.cdp_write.take().expect("cdp write"),
        sup.cdp_read.take().expect("cdp read"),
    )));
    controller
        .lock()
        .expect("controller")
        .controller_command("Browser.getVersion", json!({}), None)
        .expect("browser answered over CDP pipe");
    sup.attach_controller(Arc::clone(&controller));

    let svc = IdentityService {
        store: &env.store,
        sealer: &sealer,
    };
    let t = now();
    svc.register(
        IdentityRegisterInput {
            identity_id: "a".into(),
            project_id: "proj".into(),
            origin: origin.clone(),
            demand_confirmed_by: Some("rmaeda".into()),
            ttl_secs: None,
            state: IdentityStatePlain {
                entries: vec![StateEntry {
                    origin: origin.clone(),
                    kind: "cookie".into(),
                    name: "sid".into(),
                    value: "restore-obs-stop-secret".into(),
                }],
            },
        },
        t,
    )
    .expect("register");

    // 復元前: Live View の接続（grant → check）が張れている。
    let proof = sign(&key, &id, "r1", "live-s");
    let grant = send(
        &app,
        post_admin(
            &live(&id, "r1", "live-s", "grant"),
            &json!({"assertion":proof}),
        ),
    )
    .await;
    assert_eq!(grant.status, 200, "{}", grant.text());
    let grant_id = grant.json()["grant_id"].as_str().expect("grant").to_owned();
    let relay = json!({"assertion":proof,"grant_id":grant_id});
    let check = send(
        &app,
        post_admin(&live(&id, "r1", "live-s", "check"), &relay),
    )
    .await;
    assert_eq!(check.status, 200, "{}", check.text());

    // 開封前に落ちる復元（他 project）は停止を記録しない。
    let restore = |project: &str| {
        post_admin(
            "/api/v1/browser/identities/a/restore",
            &json!({"project_id":project,"origin":origin,"session_id":"live-s"}),
        )
    };
    assert_problem(&send(&app, restore("other")).await, 422, "other_project");
    let check = send(
        &app,
        post_admin(&live(&id, "r1", "live-s", "check"), &relay),
    )
    .await;
    assert_eq!(
        check.status, 200,
        "refused restore does not stop observation"
    );
    assert!(!stop.load(Ordering::SeqCst));

    // 成功した復元: 204、投入の前に observation_stopped が記録され、worker 側の旗も立つ。
    let ok = send(&app, restore("proj")).await;
    assert_eq!(ok.status, 204, "{}", ok.text());
    assert!(!ok.text().contains("restore-obs-stop-secret"));
    assert!(
        stop.load(Ordering::SeqCst),
        "worker observation stop raised"
    );
    assert!(controller.lock().expect("controller").observation_stopped());

    // 既存の接続も新規の grant も ObservationStopped で拒否される。
    assert_problem(
        &send(
            &app,
            post_admin(&live(&id, "r1", "live-s", "check"), &relay),
        )
        .await,
        403,
        "observation_stopped",
    );
    let proof2 = sign(&key, &id, "r1", "live-s");
    assert_problem(
        &send(
            &app,
            post_admin(
                &live(&id, "r1", "live-s", "grant"),
                &json!({"assertion":proof2}),
            ),
        )
        .await,
        403,
        "observation_stopped",
    );
    // worker が送る live event（console 等）も書き込まれない（停止は上書きされない）。
    assert_problem(
        &send(
            &app,
            post_admin(
                &live(&id, "r1", "live-s", "events"),
                &json!({"kind":"console","level":"log","text":"after restore"}),
            ),
        )
        .await,
        403,
        "observation_stopped",
    );
    assert_problem(
        &send(
            &app,
            post_admin(&live(&id, "r1", "live-s", "check"), &relay),
        )
        .await,
        403,
        "observation_stopped",
    );

    // 解除は session の終わりだけ: session を止めても Live View は再開しない。
    sup.stop();
    assert!(stop.load(Ordering::SeqCst));
    assert_problem(
        &send(
            &app,
            post_admin(&live(&id, "r1", "live-s", "check"), &relay),
        )
        .await,
        403,
        "observation_stopped",
    );
    // 止まった session への復元は開封前に拒否される。
    assert_problem(
        &send(&app, restore("proj")).await,
        403,
        "isolation_required",
    );
    eprintln!("restored session live-s: Live View refused (observation_stopped) until session end");
}
