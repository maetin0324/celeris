//! ADR-0108 D5: identity の復元を、supervisor（prod-wire の D2）が起動して registry に登録した
//! **実**隔離 session（bwrap + chrome-headless-shell）に結合する。HTTP は daemon と同じ
//! `task_api::router` と、supervisor と共有する 1 つの registry を使う。拒否経路では封緘を開かない。
//!
//! 実 runtime（bwrap・playwright の chrome-headless-shell）が無い環境では失敗する（成功扱いにしない）。
//! `CELERIS_ISOLATION_TESTS=skip` のときだけ「SKIPPED (not passed)」を出して抜ける。

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::*;
use serde_json::json;
use task_api::browser_identity::IdentityService;
use task_core::browser_isolation::{
    IsolationAttestation, IsolationViolation, LiveIsolation, LiveSessionEntry, LiveSessionRegistry,
    LiveSessions, RuntimeKind,
};
use task_worker::browser_runtime::{RuntimeSpec, UsernsMode};
use task_worker::browser_supervisor::{Supervisor, SupervisorOptions};

const ORIGIN: &str = "https://app.example";

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

/// 実 browser を隔離 runtime で起動し、supervisor に registry へ登録させる。
fn launch(session: &Path, id: &str, registry: &Arc<LiveSessions>) -> Supervisor {
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
    let mut opts = SupervisorOptions::new(session.join("records"));
    opts.registry = Some(Arc::clone(registry));
    Supervisor::start(spec, opts).expect("isolated runtime starts")
}

/// 種別が `NotIsolated` の session（trusted local 相当）。attestation は出さない。
struct NotIsolated;
impl LiveIsolation for NotIsolated {
    fn current_attestation(&self) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
        Err(vec![IsolationViolation::NoProcessGroup])
    }
}
impl LiveSessionEntry for NotIsolated {
    fn kind(&self) -> RuntimeKind {
        RuntimeKind::NotIsolated
    }
    fn accepts_state(&self) -> bool {
        true
    }
    fn deliver_state(
        &self,
        _state: &[u8],
    ) -> Result<(), task_core::browser_isolation::StateRejected> {
        panic!("state must never reach a non-isolated session");
    }
}

fn register(id: &str, project: &str, ttl: Option<u64>) -> serde_json::Value {
    json!({
        "identity_id": id,
        "project_id": project,
        "origin": ORIGIN,
        "demand_confirmed_by": "human",
        "ttl_secs": ttl,
        "state": {"entries": [{"origin": ORIGIN, "kind": "cookie", "name": "sid", "value": "secret"}]},
    })
}

fn restore(project: &str, session: Option<&str>) -> serde_json::Value {
    match session {
        Some(s) => json!({"project_id": project, "origin": ORIGIN, "session_id": s}),
        None => json!({"project_id": project, "origin": ORIGIN}),
    }
}

async fn expect_denied(
    app: &axum::Router,
    id: &str,
    body: serde_json::Value,
    status: u16,
    code: &str,
) {
    let r = send(
        app,
        post_admin(&format!("/api/v1/browser/identities/{id}/restore"), &body),
    )
    .await;
    assert_eq!(r.status.as_u16(), status, "{id} {body}: {}", r.text());
    assert_eq!(r.json()["code"], code, "{id} {body}: {}", r.text());
    assert!(!r.text().contains("secret"));
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_http_binds_to_real_isolated_session_and_never_opens_on_refusal() {
    if skip() || !userns_available() {
        return;
    }
    let env = admin_env();
    let keys = tempfile::tempdir().expect("keys");
    let sealer = Arc::new(
        celeris_credentiald::identity_seal::IdentitySealer::open(keys.path().join("keys"))
            .expect("sealer"),
    );
    let registry = Arc::new(LiveSessions::default());
    let app = task_api::router(
        env.state
            .clone()
            .with_identity_sealer(Arc::clone(&sealer))
            .with_live_sessions(registry.clone() as Arc<dyn LiveSessionRegistry>),
    );

    for (id, project, ttl) in [
        ("a", "proj", None),
        ("b", "other", None),
        ("short", "proj", Some(1)),
    ] {
        let r = send(
            &app,
            post_admin("/api/v1/browser/identities", &register(id, project, ttl)),
        )
        .await;
        assert_eq!(r.status.as_u16(), 201, "{}", r.text());
    }

    let session = tempfile::tempdir().expect("session");
    let mut sup = launch(session.path(), "live-1", &registry);
    // browser が上がるまで待つ（CDP pipe の往復。bwrap の setup 途中の事実で判定しない）。
    {
        use std::io::{Read, Write};
        let mut w = sup.cdp_write.take().expect("cdp write");
        let mut r = sup.cdp_read.take().expect("cdp read");
        w.write_all(b"{\"id\":1,\"method\":\"Browser.getVersion\"}\0")
            .expect("cdp write");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let n = r.read(&mut buf).unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            drop(w);
        });
        let reply = rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("browser answered over CDP pipe");
        assert!(reply.contains("\"id\":1"), "{reply}");
    }
    // 登録は supervisor が行う（D2）。
    let entry = registry
        .get("live-1")
        .expect("supervisor registered the live session");
    assert_eq!(entry.kind(), RuntimeKind::Isolated);
    // この host は決定 p4a-uid により同一 UID で、daemon が userns を所有する。
    assert_eq!(
        entry.current_attestation().unwrap_err(),
        vec![
            IsolationViolation::SameUid,
            IsolationViolation::UsernsOwnedByDaemon,
            IsolationViolation::LauncherProofMissing,
        ]
    );
    registry.insert("plain-1", Arc::new(NotIsolated));

    // restore_for_session を実隔離 session に対して呼ぶ（FakeLive ではない）。
    {
        let store = task_core::SqliteStore::open(&env.db_path).expect("store");
        let svc = IdentityService {
            store: &store,
            sealer: &sealer,
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        let live: &dyn LiveIsolation = &*entry;
        let err = svc
            .restore_for_session("a", "proj", ORIGIN, "live-1", live, now)
            .unwrap_err();
        assert_eq!(err.code(), "isolation_required");
    }

    // (a) 別 identity / 別 project
    expect_denied(
        &app,
        "zz",
        restore("proj", Some("live-1")),
        404,
        "identity_not_found",
    )
    .await;
    expect_denied(
        &app,
        "b",
        restore("proj", Some("live-1")),
        422,
        "other_project",
    )
    .await;
    // (c) 同一 UID の実隔離 session
    expect_denied(
        &app,
        "a",
        restore("proj", Some("live-1")),
        403,
        "isolation_required",
    )
    .await;
    // session_id 無し（従来の経路）
    expect_denied(&app, "a", restore("proj", None), 403, "isolation_required").await;
    // (d) 未登録
    expect_denied(
        &app,
        "a",
        restore("proj", Some("nope")),
        403,
        "isolation_required",
    )
    .await;
    // (e) NotIsolated
    expect_denied(
        &app,
        "a",
        restore("proj", Some("plain-1")),
        403,
        "isolation_required",
    )
    .await;
    // (b) 期限切れ（実 session を指していても期限が先に効く）
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    let r = send(
        &app,
        post_admin(
            "/api/v1/browser/identities/short/restore",
            &restore("proj", Some("live-1")),
        ),
    )
    .await;
    // sweep_expired が期限切れを失効させるので 410（expired / identity_revoked）。isolation より先に効く。
    assert_eq!(
        r.status.as_u16(),
        410,
        "expired must be refused before isolation: {}",
        r.text()
    );
    assert!(
        matches!(
            r.json()["code"].as_str(),
            Some("expired" | "identity_revoked")
        ),
        "{}",
        r.text()
    );
    assert!(!r.text().contains("secret"));

    // (d') 停止後: supervisor が registry から外し、entry を握っていても attestation は出ない。
    sup.stop();
    assert!(registry.get("live-1").is_none());
    assert_eq!(
        entry.current_attestation().unwrap_err(),
        vec![IsolationViolation::NoProcessGroup]
    );
    expect_denied(
        &app,
        "a",
        restore("proj", Some("live-1")),
        403,
        "isolation_required",
    )
    .await;

    // いずれの拒否でも封緘は開いていない。
    assert_eq!(sealer.open_attempts(), 0);
}
