//! ADR-0109 D1〜D3: injection-only IPC の照合順・拒否コード・receipt。
//!
//! 実 socket（`injection.sock`、実 SO_PEERCRED、実 SCM_RIGHTS、実 SOCK_SEQPACKET の sink FD）で試す。
//! 隔離の admission だけは試験 feature の `SameUidHarnessFacts`（与えた事実を verify_isolation に通す）で、
//! production の `Attested` がこの host で `isolation_required` になることも別に確かめる。
use celeris_credentiald::{
    AuthorizedLeaseContext, Broker, CredentialPolicy, CredentialProvider, CredentialRef, Error,
    LeaseRequest, ManualProvider, ProviderCapabilities, SecretEnvelope,
    injection_ipc::{
        self, Admission, CdpSink, Field, InjectionReply, InjectionRequest, InjectionService,
        LiveRegistry, PeerCred, SinkFailed, process_start,
    },
    ipc,
};
use std::{
    collections::BTreeSet,
    fs,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::PermissionsExt,
    },
    path::PathBuf,
    process::{Child, Command},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use task_core::browser_isolation::{
    CdpEndpoint, IsolationViolation, Namespace, RuntimeFacts, collect_runtime_facts,
    verify_isolation,
};
use tempfile::TempDir;

const SENTINEL: &str = "SENTINEL-INJECT-3f9a1c7e5b2d4a60";
const ORIGIN: &str = "https://login.example.test";
const TARGET: &str = "TARGET-A";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

struct Counting {
    calls: AtomicUsize,
}
impl CredentialProvider for Counting {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            manual_registration: false,
            interactive_unlock: false,
            totp: false,
            revoke: true,
        }
    }
    fn resolve(
        &self,
        _: &CredentialRef,
        _: &AuthorizedLeaseContext,
    ) -> Result<SecretEnvelope, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(SecretEnvelope {
            username: "user@example.test".into(),
            password: SENTINEL.into(),
        })
    }
    fn revoke(&self, _: &str) -> Result<(), Error> {
        Ok(())
    }
}

/// 試験の事実: 違反が SameUid だけ（`unisolated-` の session は root が書込み可）。
fn facts(session_id: &str, _pid: i32) -> RuntimeFacts {
    RuntimeFacts {
        session_id: session_id.into(),
        host_uid: 1000,
        runtime_uid: 1000,
        namespaces: BTreeSet::from([
            Namespace::User,
            Namespace::Pid,
            Namespace::Net,
            Namespace::Mount,
            Namespace::Ipc,
            Namespace::Uts,
        ]),
        root_readonly: !session_id.starts_with("unisolated-"),
        writable_mounts: vec!["/session".into()],
        visible_paths: vec![],
        cdp: CdpEndpoint::Pipe,
        no_new_privs: true,
        capabilities_dropped: true,
        pgid: 4242,
    }
}

struct Fx {
    root: TempDir,
    broker: Arc<Broker>,
    provider: Arc<Counting>,
    runtime: Vec<Child>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        for c in &mut self.runtime {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Fx {
    fn new(admission: Admission) -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        for d in ["config", "data", "run"] {
            let p = root.path().join(d);
            fs::create_dir(&p).expect("mkdir");
            fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).expect("chmod");
        }
        let manual = ManualProvider::open(
            root.path().join("config/keys"),
            root.path().join("data/vault"),
        )
        .expect("open");
        manual.initialize_key().expect("init");
        let mut broker = Broker::new(manual, root.path().join("data/audit")).expect("broker");
        let provider = Arc::new(Counting {
            calls: AtomicUsize::new(0),
        });
        broker
            .add_provider("counting".into(), provider.clone())
            .expect("provider");
        let broker = Arc::new(broker);
        let b = Arc::clone(&broker);
        let run = root.path().join("run");
        thread::spawn(move || ipc::serve_with(b, &run, vec![std::process::id()], admission));
        let fx = Self {
            root,
            broker,
            provider,
            runtime: Vec::new(),
        };
        for _ in 0..200 {
            if fx.sock("injection.sock").exists() && fx.sock("control.sock").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        fx
    }
    fn sock(&self, name: &str) -> PathBuf {
        self.root.path().join("run/celeris-credentiald").join(name)
    }
    fn calls(&self) -> usize {
        self.provider.calls.load(Ordering::SeqCst)
    }
    fn grant(&self, session_id: &str, key: &str, ttl: u64) -> String {
        self.grant_with(session_id, key, ttl, Some("input[name=password]"))
    }
    /// `selector` が `None` なら管理者 selector の無い（旧い）policy で grant する。
    fn grant_with(&self, session_id: &str, key: &str, ttl: u64, selector: Option<&str>) -> String {
        let policy = CredentialPolicy {
            policy_id: "site-1".into(),
            revision: 1,
            exact_origin: ORIGIN.into(),
            task_id: "task-1".into(),
            max_ttl_seconds: 60,
            require_approval: true,
            allow_persistence: false,
            login_url: selector.map(|_| format!("{ORIGIN}/login")),
            password_selector: selector.map(str::to_owned),
            submit_selector: None,
        };
        self.broker
            .grant(LeaseRequest {
                reference: CredentialRef {
                    credential_id: "login-1".into(),
                    provider: "counting".into(),
                    policy_id: "site-1".into(),
                },
                policy,
                credential_revision: 1,
                task_id: "task-1".into(),
                run_id: "run-1".into(),
                session_id: session_id.into(),
                approval_id: "approval-1".into(),
                approved_by: "human-1".into(),
                policy_hash: "hash-1".into(),
                idempotency_key: key.into(),
                ttl_seconds: ttl,
                approval_expires_at: now() + 100,
                session_expires_at: now() + 100,
            })
            .expect("grant")
            .lease_id
    }
    fn child(&mut self) -> (u32, u64) {
        let c = Command::new("sleep").arg("30").spawn().expect("sleep");
        let pid = c.id();
        self.runtime.push(c);
        let mut start = None;
        for _ in 0..100 {
            start = process_start(pid);
            if start.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        (pid, start.expect("start"))
    }
    fn control(&self, v: serde_json::Value) -> Option<String> {
        let reply = ipc::call(&self.sock("control.sock"), v.to_string().as_bytes()).expect("ctl");
        if reply.success { None } else { reply.code }
    }
    /// controller = この試験 process、runtime = 生きている子 process。
    fn live(&mut self, session_id: &str) -> u32 {
        let me = std::process::id();
        let (rt, rt_start) = self.child();
        let r = self.control(serde_json::json!({
            "op": "register_live_session",
            "session_id": session_id,
            "controller_pid": me,
            "controller_start": process_start(me).expect("self start"),
            "runtime_pid": rt,
            "runtime_start": rt_start,
        }));
        assert_eq!(r, None);
        rt
    }
    fn open(&self, session_id: &str, section: &str, lease: &str) {
        let r = self.control(serde_json::json!({
            "op": "open_auth_section",
            "session_id": session_id,
            "auth_section_id": section,
            "lease_id": lease,
            "exact_origin": ORIGIN,
            "cdp_target_id": TARGET,
        }));
        assert_eq!(r, None);
    }
    fn journal(&self) -> String {
        fs::read_to_string(self.root.path().join("data/audit/journal.jsonl")).unwrap_or_default()
    }
}

fn request(session_id: &str, section: &str, lease: &str) -> InjectionRequest {
    InjectionRequest {
        v: 1,
        request_id: "01J0000000000000000000REQ1".into(),
        session_id: session_id.into(),
        cdp_target_id: TARGET.into(),
        cdp_session_id: Some("CDPSESSION1".into()),
        frame_id: "FRAME-1".into(),
        loader_id: "LOADER-1".into(),
        frame_chain: vec![ORIGIN.into()],
        redirect_chain: vec![ORIGIN.into()],
        selector: "input[name=password]".into(),
        object_id: "-4611686018427387903.1.7".into(),
        field: Field::Password,
        input_type: "password".into(),
        auth_section_id: section.into(),
        lease_id: lease.into(),
        cdp_command_id: 900_001,
    }
}

type Mutate = Box<dyn Fn(&mut InjectionRequest)>;

fn seqpacket() -> (OwnedFd, OwnedFd) {
    let mut fds = [0; 2];
    let rc = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    assert_eq!(rc, 0);
    unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
}

/// 試験用の CDP mux: broker の frame を 1 本受け、`verdict` を返す。frame が来なければ None。
fn mux(end: OwnedFd, verdict: &'static str) -> thread::JoinHandle<Option<Vec<u8>>> {
    thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        let n = unsafe { libc::recv(end.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        let frame: serde_json::Value =
            serde_json::from_slice(buf.strip_suffix(&[0]).unwrap_or(&buf)).expect("frame json");
        let id = frame["id"].as_u64().expect("id");
        let reply = serde_json::json!({"id": id, "result": {"result": {"type": "string", "value": verdict}}})
            .to_string();
        unsafe { libc::send(end.as_raw_fd(), reply.as_ptr().cast(), reply.len(), 0) };
        Some(buf)
    })
}

/// 要求を送る。応答の生 byte 列と、sink に届いた frame を返す。
fn inject_raw(
    fx: &Fx,
    body: &[u8],
    verdict: &'static str,
    fds: usize,
) -> (Vec<u8>, Option<Vec<u8>>) {
    let (broker_end, mux_end) = seqpacket();
    let handle = mux(mux_end, verdict);
    let raw_fds: Vec<i32> = (0..fds).map(|_| broker_end.as_raw_fd()).collect();
    let raw = injection_ipc::call_raw(&fx.sock("injection.sock"), body, &raw_fds).expect("call");
    drop(broker_end);
    (raw, handle.join().expect("mux"))
}

fn inject(fx: &Fx, req: &InjectionRequest) -> (InjectionReply, Vec<u8>, Option<Vec<u8>>) {
    let body = serde_json::to_vec(req).expect("json");
    let (raw, frame) = inject_raw(fx, &body, "ok", 1);
    let reply: InjectionReply = serde_json::from_slice(&raw).expect("reply");
    (reply, raw, frame)
}

fn code(fx: &Fx, req: &InjectionRequest) -> String {
    let (reply, raw, frame) = inject(fx, req);
    assert!(!reply.ok, "unexpected success");
    assert!(reply.receipt.is_none());
    assert!(frame.is_none(), "denied request must not reach the sink");
    assert!(!String::from_utf8_lossy(&raw).contains(SENTINEL));
    reply.code.expect("code")
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[test]
fn success_reply_is_receipt_only_and_secret_reaches_only_the_sink() {
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let lease = fx.grant("sess-1", "k1", 60);
    fx.live("sess-1");
    fx.open("sess-1", "auth-1", &lease);
    let before = fx.calls();
    let (reply, raw, frame) = inject(&fx, &request("sess-1", "auth-1", &lease));
    assert!(reply.ok, "{:?}", reply.code);
    assert_eq!(fx.calls(), before + 1);
    let receipt = reply.receipt.expect("receipt");
    assert_eq!(receipt.lease_id, lease);
    assert_eq!(receipt.auth_section_id, "auth-1");
    assert_eq!(receipt.session_id, "sess-1");
    assert_eq!(receipt.cdp_target_id, TARGET);
    assert_eq!(receipt.frame_id, "FRAME-1");
    assert_eq!(receipt.loader_id, "LOADER-1");
    assert_eq!(receipt.field, Field::Password);
    // 応答の byte 列に秘密・object id・selector が無い。
    assert!(!contains(&raw, SENTINEL));
    assert!(!contains(&raw, "-4611686018427387903.1.7"));
    assert!(!contains(&raw, "input[name=password]"));
    // 秘密は sink の 1 frame にだけ乗る（Runtime.callFunctionOn、予約 id、flatten session）。
    let frame = frame.expect("frame");
    assert!(contains(&frame, SENTINEL));
    let f: serde_json::Value =
        serde_json::from_slice(frame.strip_suffix(&[0]).unwrap_or(&frame)).expect("frame");
    assert_eq!(f["id"], 900_001);
    assert_eq!(f["method"], "Runtime.callFunctionOn");
    assert_eq!(f["sessionId"], "CDPSESSION1");
    assert_eq!(f["params"]["objectId"], "-4611686018427387903.1.7");
    assert_eq!(f["params"]["arguments"][0]["value"], ORIGIN);
    assert_eq!(f["params"]["arguments"][1]["value"], 0);
    assert_eq!(f["params"]["returnByValue"], true);
    // audit には非秘密の記録だけ。
    let journal = fx.journal();
    assert!(journal.contains("\"decision_code\":\"injected\""));
    assert!(journal.contains("\"admission\":\"same_uid_harness\""));
    assert!(!journal.contains(SENTINEL));
    // lease は単回: 同じ lease の再使用は lease_used、provider は呼ばれず sink に 2 本目は無い。
    let before = fx.calls();
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "lease_used"
    );
    assert_eq!(fx.calls(), before);
}

#[test]
fn username_goes_only_into_text_or_email_inputs() {
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let lease = fx.grant("sess-1", "k1", 60);
    fx.live("sess-1");
    fx.open("sess-1", "auth-1", &lease);
    let mut req = request("sess-1", "auth-1", &lease);
    req.field = Field::Username;
    req.input_type = "password".into();
    assert_eq!(code(&fx, &req), "redisplay_field");
    req.input_type = "email".into();
    let (reply, raw, frame) = inject(&fx, &req);
    assert!(reply.ok, "{:?}", reply.code);
    assert!(!contains(&raw, "user@example.test"));
    assert!(contains(&frame.expect("frame"), "user@example.test"));
}

#[test]
fn non_injector_peers_are_rejected_without_provider_or_lease_use() {
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let lease = fx.grant("sess-1", "k1", 60);
    // controller は別 process（この試験 process は worker として接続する）。
    let (other, other_start) = fx.child();
    let (rt, rt_start) = fx.child();
    let r = fx.control(serde_json::json!({
        "op": "register_live_session", "session_id": "sess-1",
        "controller_pid": other, "controller_start": other_start,
        "runtime_pid": rt, "runtime_start": rt_start,
    }));
    assert_eq!(r, None);
    fx.open("sess-1", "auth-1", &lease);
    let before = fx.calls();
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "injection_worker_not_allowed"
    );
    // 別 session の controller（自分は sess-2 の controller）も sess-1 には注入できない。
    fx.live("sess-2");
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "injection_worker_not_allowed"
    );
    assert_eq!(fx.calls(), before);
    // lease は消費されていない: 正しい controller に登録し直すと成功する。
    assert_eq!(
        fx.control(serde_json::json!({"op": "unregister_live_session", "session_id": "sess-1"})),
        None
    );
    fx.live("sess-1");
    fx.open("sess-1", "auth-1", &lease);
    let (reply, _, _) = inject(&fx, &request("sess-1", "auth-1", &lease));
    assert!(reply.ok, "{:?}", reply.code);
}

#[test]
fn peer_with_another_uid_is_rejected() {
    let fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let service = InjectionService::new(
        Arc::clone(&fx.broker),
        Arc::new(LiveRegistry::default()),
        Admission::SameUidHarnessFacts(facts),
    );
    struct NoSink;
    impl CdpSink for NoSink {
        fn exchange(&mut self, _: &[u8]) -> Result<Vec<u8>, SinkFailed> {
            panic!("sink must not be reached")
        }
    }
    let body = serde_json::to_vec(&request("sess-1", "auth-1", "lease-1")).expect("json");
    let peer = PeerCred {
        uid: unsafe { libc::geteuid() } + 1,
        pid: std::process::id(),
    };
    let reply = service.handle(peer, &body, &mut NoSink);
    assert_eq!(reply.code.as_deref(), Some("peer_uid_mismatch"));
    assert_eq!(fx.calls(), 0);
}

#[test]
fn unregistered_and_ended_sessions_are_rejected() {
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let lease = fx.grant("sess-1", "k1", 60);
    let before = fx.calls();
    // どの session の controller でもない peer。
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "injection_worker_not_allowed"
    );
    // 別 session の controller が未登録の session を指す。
    fx.live("sess-other");
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "session_not_live"
    );
    // 登録はあるが runtime が終了済み。
    let rt = fx.live("sess-1");
    fx.open("sess-1", "auth-1", &lease);
    for c in &mut fx.runtime {
        if c.id() == rt {
            c.kill().expect("kill");
            c.wait().expect("wait");
        }
    }
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "session_not_live"
    );
    // 登録解除後。
    assert_eq!(
        fx.control(serde_json::json!({"op": "unregister_live_session", "session_id": "sess-1"})),
        None
    );
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "session_not_live"
    );
    assert_eq!(fx.calls(), before);
}

#[test]
fn isolation_is_verified_by_the_broker() {
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let lease = fx.grant("unisolated-1", "k1", 60);
    fx.live("unisolated-1");
    fx.open("unisolated-1", "auth-1", &lease);
    let before = fx.calls();
    assert_eq!(
        code(&fx, &request("unisolated-1", "auth-1", &lease)),
        "isolation_required"
    );
    assert_eq!(fx.calls(), before);
}

#[test]
fn credential_injection_sameuid_rejected_in_production() {
    // Attested は /proc から事実を採り直す。子 process は host と同じ namespace・UID なので必ず拒否。
    let mut fx = Fx::new(Admission::Attested);
    let lease = fx.grant("sess-1", "k1", 60);
    let runtime_pid = fx.live("sess-1") as i32;
    let pgid = unsafe { libc::getpgid(runtime_pid) };
    assert!(pgid > 0);
    let runtime_facts = collect_runtime_facts("sess-1", runtime_pid, pgid).expect("runtime facts");
    assert_eq!(runtime_facts.host_uid, runtime_facts.runtime_uid);
    assert!(
        verify_isolation(&runtime_facts)
            .unwrap_err()
            .contains(&IsolationViolation::SameUid)
    );
    fx.open("sess-1", "auth-1", &lease);
    let before = fx.calls();
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "isolation_required"
    );
    assert_eq!(fx.calls(), before);
    assert!(fx.journal().contains("\"admission\":\"attested\""));
}

#[test]
fn target_origin_and_field_mismatches_are_rejected() {
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let lease = fx.grant("sess-1", "k1", 60);
    fx.live("sess-1");
    fx.open("sess-1", "auth-1", &lease);
    let before = fx.calls();
    let base = request("sess-1", "auth-1", &lease);
    let cases: Vec<(&str, Mutate)> = vec![
        (
            "target_mismatch",
            Box::new(|r| r.cdp_target_id = "TARGET-OOPIF".into()),
        ),
        ("empty_frame_chain", Box::new(|r| r.frame_chain.clear())),
        (
            "cross_origin_frame",
            Box::new(|r| r.frame_chain = vec![ORIGIN.into(), "https://evil.example.test".into()]),
        ),
        (
            "cross_origin_frame",
            Box::new(|r| r.frame_chain = vec!["https://evil.example.test".into(), ORIGIN.into()]),
        ),
        (
            "redirected",
            Box::new(|r| {
                r.redirect_chain = vec!["https://evil.example.test".into(), ORIGIN.into()]
            }),
        ),
        (
            "redisplay_field",
            Box::new(|r| r.input_type = "text".into()),
        ),
        (
            "auth_section_mismatch",
            Box::new(|r| r.auth_section_id = "auth-2".into()),
        ),
        ("unsupported_version", Box::new(|r| r.v = 2)),
    ];
    for (want, mutate) in cases {
        let mut req = base.clone();
        mutate(&mut req);
        assert_eq!(code(&fx, &req), want);
    }
    assert_eq!(fx.calls(), before);
    // どれも lease を消費していない。
    let (reply, _, _) = inject(&fx, &base);
    assert!(reply.ok, "{:?}", reply.code);
}

#[test]
fn auth_section_is_required_and_bound_to_its_lease() {
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let lease = fx.grant("sess-1", "k1", 60);
    let other = fx.grant("sess-1", "k2", 60);
    fx.live("sess-1");
    let before = fx.calls();
    // 区間を開く前。
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "auth_section_required"
    );
    // 区間の lease と違う lease。
    fx.open("sess-1", "auth-1", &lease);
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &other)),
        "auth_section_mismatch"
    );
    // 区間を閉じた後。
    assert_eq!(
        fx.control(serde_json::json!({
            "op": "close_auth_section", "session_id": "sess-1", "auth_section_id": "auth-1"
        })),
        None
    );
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "auth_section_required"
    );
    assert_eq!(fx.calls(), before);
}

#[test]
fn expired_revoked_and_foreign_leases_are_rejected() {
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let short = fx.grant("sess-1", "k1", 1);
    let revoked = fx.grant("sess-1", "k2", 60);
    let foreign = fx.grant("sess-9", "k3", 60);
    fx.broker.revoke(&revoked, "human-1").expect("revoke");
    fx.live("sess-1");
    let before = fx.calls();
    for (lease, want) in [
        (revoked.as_str(), "lease_invalid"),
        (foreign.as_str(), "other_session"),
        ("0000000000000000", "lease_invalid"),
    ] {
        fx.open("sess-1", "auth-1", lease);
        assert_eq!(code(&fx, &request("sess-1", "auth-1", lease)), want);
        fx.control(serde_json::json!({
            "op": "close_auth_section", "session_id": "sess-1", "auth_section_id": "auth-1"
        }));
    }
    thread::sleep(Duration::from_millis(2100));
    fx.open("sess-1", "auth-1", &short);
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &short)),
        "lease_expired"
    );
    assert_eq!(fx.calls(), before);
}

#[test]
fn malformed_requests_and_missing_sink_are_rejected() {
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let lease = fx.grant("sess-1", "k1", 60);
    fx.live("sess-1");
    fx.open("sess-1", "auth-1", &lease);
    let before = fx.calls();
    let good = serde_json::to_value(request("sess-1", "auth-1", &lease)).expect("json");
    // 秘密・長さを混ぜた要求（deny_unknown_fields）。
    for extra in ["value", "length"] {
        let mut v = good.clone();
        v[extra] = serde_json::json!(SENTINEL);
        let (raw, frame) = inject_raw(&fx, v.to_string().as_bytes(), "ok", 1);
        let reply: InjectionReply = serde_json::from_slice(&raw).expect("reply");
        assert_eq!(reply.code.as_deref(), Some("invalid_request"));
        assert!(frame.is_none());
        assert!(!contains(&raw, SENTINEL));
    }
    // sink FD が無い・2 本。
    for fds in [0, 2] {
        let (raw, frame) = inject_raw(&fx, good.to_string().as_bytes(), "ok", fds);
        let reply: InjectionReply = serde_json::from_slice(&raw).expect("reply");
        assert_eq!(reply.code.as_deref(), Some("invalid_request"));
        assert!(frame.is_none());
    }
    assert_eq!(fx.calls(), before);
    assert!(!fx.journal().contains(SENTINEL));
    let (reply, _, _) = inject(&fx, &request("sess-1", "auth-1", &lease));
    assert!(reply.ok, "{:?}", reply.code);
}

#[test]
fn in_page_recheck_failure_is_target_changed_and_lease_stays_consumed() {
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let lease = fx.grant("sess-1", "k1", 60);
    fx.live("sess-1");
    fx.open("sess-1", "auth-1", &lease);
    let body = serde_json::to_vec(&request("sess-1", "auth-1", &lease)).expect("json");
    let (raw, frame) = inject_raw(&fx, &body, "target_changed", 1);
    let reply: InjectionReply = serde_json::from_slice(&raw).expect("reply");
    assert_eq!(reply.code.as_deref(), Some("target_changed"));
    assert!(frame.is_some());
    assert!(!contains(&raw, SENTINEL));
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "lease_used"
    );
}

#[test]
fn request_selector_must_equal_the_admin_policy_selector_before_lease_use() {
    // ADR-0110 D2 照合 3: 要求の selector は lease の policy 断面の管理者 selector と byte 一致でなければ
    // `selector_mismatch`。順 4（auth section）の後・順 5（lease 消費）の前なので lease も provider も使わない。
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let lease = fx.grant("sess-1", "k1", 60);
    fx.live("sess-1");
    let before = fx.calls();
    // 区間を開く前は順 4 が先に落ちる（照合順の確認）。
    let mut swapped = request("sess-1", "auth-1", &lease);
    swapped.selector = "#attacker".into();
    assert_eq!(code(&fx, &swapped), "auth_section_required");
    fx.open("sess-1", "auth-1", &lease);
    for other in [
        "#attacker",
        "input[name=password] ",
        "input[name=\"password\"]",
        "input",
    ] {
        let mut req = request("sess-1", "auth-1", &lease);
        req.selector = other.into();
        assert_eq!(code(&fx, &req), "selector_mismatch", "{other:?}");
    }
    // 空の selector は形式の段（順 0）で落ちる。
    let mut empty = request("sess-1", "auth-1", &lease);
    empty.selector = String::new();
    assert_eq!(code(&fx, &empty), "invalid_request");
    assert_eq!(fx.calls(), before);
    // 拒否の後も lease は未消費: 管理者 selector の要求は通る。
    let (reply, raw, _) = inject(&fx, &request("sess-1", "auth-1", &lease));
    assert!(reply.ok, "{:?}", reply.code);
    assert_eq!(fx.calls(), before + 1);
    assert!(!contains(&raw, "#attacker"));
    let journal = fx.journal();
    assert!(journal.contains("\"decision_code\":\"selector_mismatch\""));
}

#[test]
fn policy_without_admin_selector_cannot_inject() {
    // 旧い policy（login_url・password_selector 無し）でも grant はできるが、注入は固定拒否で lease は残る。
    let mut fx = Fx::new(Admission::SameUidHarnessFacts(facts));
    let lease = fx.grant_with("sess-1", "k1", 60, None);
    fx.live("sess-1");
    fx.open("sess-1", "auth-1", &lease);
    let before = fx.calls();
    assert_eq!(
        code(&fx, &request("sess-1", "auth-1", &lease)),
        "trusted_selector_missing"
    );
    assert_eq!(fx.calls(), before);
}
