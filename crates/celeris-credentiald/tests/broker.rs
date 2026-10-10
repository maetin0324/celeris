use celeris_credentiald::{
    Binding, Broker, CredentialPolicy, CredentialRef, Error, LeaseRequest, ManualProvider,
    SecretEnvelope, canonical_origin, ipc,
};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tempfile::TempDir;
const SENTINEL: &str = "SENTINEL-PASSWORD-6c4fe259";
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}
struct Fixture {
    root: TempDir,
    manual: ManualProvider,
    broker: Broker,
    reference: CredentialRef,
    policy: CredentialPolicy,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let config = root.path().join("config");
        let data = root.path().join("data");
        fs::create_dir(&config).expect("config");
        fs::create_dir(&data).expect("data");
        fs::set_permissions(&config, fs::Permissions::from_mode(0o700)).expect("chmod");
        fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).expect("chmod");
        let manual = ManualProvider::open(config.join("keys"), data.join("vault")).expect("open");
        manual.initialize_key().expect("init");
        let broker = Broker::new(manual.clone(), data.join("audit")).expect("broker");
        let reference = CredentialRef {
            credential_id: "login-1".into(),
            provider: "manual".into(),
            policy_id: "site-1".into(),
        };
        let policy = CredentialPolicy {
            policy_id: "site-1".into(),
            revision: 1,
            exact_origin: "https://example.test".into(),
            task_id: "task-1".into(),
            max_ttl_seconds: 60,
            require_approval: true,
            allow_persistence: false,
            login_url: None,
            password_selector: None,
            submit_selector: None,
            username_selector: None,
            post_login: None,
            consent: None,
        };
        Self {
            root,
            manual,
            broker,
            reference,
            policy,
        }
    }
    fn register(&self) {
        self.manual
            .register(
                &self.reference,
                &self.policy,
                1,
                &SecretEnvelope {
                    username: "test-user".into(),
                    password: SENTINEL.into(),
                },
            )
            .expect("register")
    }
    fn request(&self, key: &str, ttl: u64) -> LeaseRequest {
        LeaseRequest {
            reference: self.reference.clone(),
            policy: self.policy.clone(),
            credential_revision: 1,
            task_id: "task-1".into(),
            run_id: "run-1".into(),
            session_id: "session-1".into(),
            approval_id: "approval-1".into(),
            approved_by: "human-1".into(),
            policy_hash: "hash-1".into(),
            idempotency_key: key.into(),
            ttl_seconds: ttl,
            approval_expires_at: now() + 100,
            session_expires_at: now() + 100,
        }
    }
    fn bind(&self) -> String {
        self.broker
            .register_binding(Binding {
                token: "caller-must-not-choose".into(),
                task_id: "task-1".into(),
                run_id: "run-1".into(),
                session_id: "session-1".into(),
                exact_origin: self.policy.exact_origin.clone(),
                policy_hash: "hash-1".into(),
                expires_at: now() + 100,
            })
            .expect("binding")
    }
    fn audit(&self) -> String {
        fs::read_to_string(self.root.path().join("data/audit/journal.jsonl")).expect("audit")
    }
}
#[test]
fn manual_roundtrip_permissions_and_revoke() {
    let f = Fixture::new();
    f.register();
    let key = f.root.path().join("config/keys/master-v1.key");
    let vault = f.root.path().join("data/vault/login-1.json");
    for p in [&key, &vault] {
        assert_eq!(
            fs::metadata(p).expect("file").permissions().mode() & 0o777,
            0o600
        )
    }
    for p in ["config/keys", "data/vault", "data/audit"] {
        assert_eq!(
            fs::metadata(f.root.path().join(p))
                .expect("dir")
                .permissions()
                .mode()
                & 0o777,
            0o700
        )
    }
    let raw = fs::read_to_string(&vault).expect("ciphertext");
    assert!(!raw.contains(SENTINEL));
    assert!(!raw.contains("test-user"));
    let s = f
        .manual
        .resolve_registered(&f.reference, 1, &f.policy.exact_origin)
        .expect("resolve");
    assert_eq!(s.password, SENTINEL);
    drop(s);
    f.manual.remove(&f.reference).expect("remove");
    assert!(matches!(
        f.manual
            .resolve_registered(&f.reference, 1, &f.policy.exact_origin),
        Err(Error::Permission)
    ));
}
#[test]
fn tamper_wrong_origin_missing_key_and_bad_modes() {
    let f = Fixture::new();
    f.register();
    assert!(matches!(
        f.manual
            .resolve_registered(&f.reference, 1, "https://other.test"),
        Err(Error::Denied)
    ));
    let path = f.root.path().join("data/vault/login-1.json");
    let mut doc: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    doc["ciphertext"] = "0000".into();
    fs::write(&path, serde_json::to_vec(&doc).expect("json")).expect("write");
    assert!(
        f.manual
            .resolve_registered(&f.reference, 1, &f.policy.exact_origin)
            .is_err()
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");
    assert!(matches!(
        f.manual
            .resolve_registered(&f.reference, 1, &f.policy.exact_origin),
        Err(Error::Permission)
    ));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");
    fs::remove_file(f.root.path().join("config/keys/master-v1.key")).expect("remove");
    assert!(matches!(f.manual.initialize_key(), Err(Error::VaultLocked)));
}
#[test]
fn origin_parser_is_exact_https() {
    assert_eq!(
        canonical_origin("https://EXAMPLE.test:443"),
        Ok("https://example.test".into())
    );
    for s in [
        "http://example.test",
        "https://example.test/login",
        "https://*.example.test",
        "https://user@example.test",
        "https://example.test:0",
        "https://example.test?q=1",
        "https://example.test#x",
    ] {
        assert!(canonical_origin(s).is_err(), "{s}")
    }
}
#[test]
fn lease_binding_origin_one_use_revoke_and_sentinel() {
    let f = Fixture::new();
    f.register();
    let token = f.bind();
    let lease = f.broker.grant(f.request("key-a", 60)).expect("grant");
    assert_eq!(lease.max_uses, 1);
    assert!(matches!(
        f.broker
            .resolve(&token, &lease.lease_id, "https://other.test"),
        Err(Error::Denied)
    ));
    assert!(matches!(
        f.broker
            .resolve("wrong", &lease.lease_id, &f.policy.exact_origin),
        Err(Error::Denied)
    ));
    let secret = f
        .broker
        .resolve(&token, &lease.lease_id, &f.policy.exact_origin)
        .expect("resolve");
    assert_eq!(secret.password, SENTINEL);
    assert!(!format!("{secret:?}").contains(SENTINEL));
    drop(secret);
    assert!(matches!(
        f.broker
            .resolve(&token, &lease.lease_id, &f.policy.exact_origin),
        Err(Error::Used)
    ));
    let revoked = f.broker.grant(f.request("key-b", 60)).expect("grant");
    f.broker
        .revoke(&revoked.lease_id, "human-1")
        .expect("revoke");
    assert!(matches!(
        f.broker
            .resolve(&token, &revoked.lease_id, &f.policy.exact_origin),
        Err(Error::Denied)
    ));
    let audit = f.audit();
    assert!(audit.contains("\"action\":\"request\""));
    assert!(audit.contains("\"action\":\"grant\""));
    assert!(audit.contains("\"action\":\"use\""));
    assert!(audit.contains("\"action\":\"deny\""));
    assert!(audit.contains("\"action\":\"revoke\""));
    assert!(!audit.contains(SENTINEL));
    assert!(!audit.contains("test-user"));
    let err = Error::Denied;
    assert!(!format!("{err:?} {err}").contains(SENTINEL));
}
#[test]
fn expiry_audit_failure_and_restart_reject() {
    let f = Fixture::new();
    f.register();
    let token = f.bind();
    let lease = f.broker.grant(f.request("expiry", 1)).expect("grant");
    thread::sleep(Duration::from_secs(2));
    assert!(matches!(
        f.broker
            .resolve(&token, &lease.lease_id, &f.policy.exact_origin),
        Err(Error::Expired)
    ));
    assert!(f.audit().contains("\"action\":\"expire\""));
    let unexpired = f.broker.grant(f.request("restart", 60)).expect("grant");
    let restarted =
        Broker::new(f.manual.clone(), f.root.path().join("data/audit")).expect("restart");
    assert!(matches!(
        restarted.resolve(&token, &unexpired.lease_id, &f.policy.exact_origin),
        Err(Error::Denied) | Err(Error::NotFound)
    ));
    let audit = f.root.path().join("data/audit/journal.jsonl");
    fs::set_permissions(&audit, fs::Permissions::from_mode(0o644)).expect("chmod");
    assert!(matches!(
        f.broker.grant(f.request("audit-broken", 60)),
        Err(Error::AuditUnavailable)
    ));
}
#[test]
fn concurrent_resolve_only_one_wins() {
    let f = Fixture::new();
    f.register();
    let token = f.bind();
    let lease = f.broker.grant(f.request("race", 60)).expect("grant");
    let b = Arc::new(f.broker);
    let mut threads = Vec::new();
    for _ in 0..2 {
        let b = Arc::clone(&b);
        let t = token.clone();
        let id = lease.lease_id.clone();
        threads.push(thread::spawn(move || {
            b.resolve(&t, &id, "https://example.test").is_ok()
        }));
    }
    let wins: usize = threads
        .into_iter()
        .map(|h| usize::from(h.join().expect("join")))
        .sum();
    assert_eq!(wins, 1);
}
#[test]
fn retired_plugin_bridge_never_connects_or_echoes_input() {
    use std::os::unix::net::UnixListener;
    let dir = tempfile::tempdir().expect("tempdir");
    let path: PathBuf = dir.path().join("resolve.sock");
    let listener = UnixListener::bind(&path).expect("bind");
    listener.set_nonblocking(true).unwrap();
    for input in [
        br#"{"protocol":"agent-browser.plugin.v1","type":"credential.resolve","capability":"credential.read","request":{"profileName":"default","itemRef":"lease-1","url":"https://example.test/login"}}"#.as_slice(),
        SENTINEL.as_bytes(),
    ] {
        let out = ipc::bridge_request(input, SENTINEL, &path);
        assert_eq!(out, br#"{"protocol":"agent-browser.plugin.v1","success":false}"#);
        assert!(!String::from_utf8_lossy(&out).contains(SENTINEL));
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }
}
#[test]
fn daemon_rejects_worker_secret_retrieval_even_with_valid_lease() {
    use std::process::{Child, Command, Stdio};
    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().expect("tempdir");
    let home = root.path().join("home");
    let runtime = root.path().join("runtime");
    fs::create_dir(&home).expect("home");
    fs::create_dir(&runtime).expect("runtime");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).expect("chmod");
    for p in [
        home.join(".config"),
        home.join(".config/celeris"),
        home.join(".local"),
        home.join(".local/celeris"),
    ] {
        fs::create_dir(&p).expect("parent")
    }
    let binary = std::env::var("CARGO_BIN_EXE_celeris-credentiald").expect("binary path");
    // The child must only see this test's temporary HOME/XDG paths. A worker
    // sandbox exports CELERIS_CREDENTIALD_DATA_DIR (the production data dir) and
    // other CELERIS_*/XDG_* values; inheriting them sends vault/audit elsewhere.
    let child = Command::new(binary)
        .arg("serve")
        .arg(std::process::id().to_string())
        .env_clear()
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env_remove("CELERIS_CREDENTIALD_DATA_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let _child = ChildGuard(child);
    let control = runtime.join("celeris-credentiald/control.sock");
    let resolve = runtime.join("celeris-credentiald/resolve.sock");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if control.exists() && resolve.exists() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for fixture readiness"
        );
        thread::sleep(Duration::from_millis(10))
    }
    assert!(control.exists());
    assert_eq!(
        fs::metadata(&control).expect("socket").permissions().mode() & 0o777,
        0o600
    );
    let init = ipc::call(&control, br#"{"op":"initialize_key"}"#).expect("init");
    assert!(init.success, "initialize_key failed: code={:?}", init.code);
    let f = Fixture::new();
    let reference = f.reference.clone();
    let policy = f.policy.clone();
    let reg = serde_json::json!({"op":"register","reference":reference,"policy":policy,"revision":1,"secret":{"username":"u","password":SENTINEL}});
    assert!(
        ipc::call(&control, &serde_json::to_vec(&reg).expect("json"))
            .expect("register")
            .success
    );
    let bind = serde_json::json!({"op":"bind","binding":{"token":"ignored","task_id":"task-1","run_id":"run-1","session_id":"session-1","exact_origin":"https://example.test","policy_hash":"hash-1","expires_at":now()+100}});
    let token = ipc::call(&control, &serde_json::to_vec(&bind).expect("json"))
        .expect("bind")
        .binding_token
        .expect("token");
    let grant = serde_json::json!({"op":"grant","request":f.request("ipc",60)});
    let lease_id = ipc::call(&control, &serde_json::to_vec(&grant).expect("json"))
        .expect("grant")
        .lease_id
        .expect("lease");
    let plugin = serde_json::json!({"protocol":"agent-browser.plugin.v1","type":"credential.resolve","capability":"credential.read","request":{"profileName":"default","itemRef":lease_id,"url":"https://example.test/login"}});
    let out = ipc::bridge_request(
        &serde_json::to_vec(&plugin).expect("json"),
        &token,
        &resolve,
    );
    let response: serde_json::Value = serde_json::from_slice(&out).expect("response");
    assert_eq!(response["success"], false);
    assert!(response.get("credential").is_none());
    // Bypass the retired bridge and call the real broker socket directly, with
    // credentials that used to authorize it. Even the admitted control PID fails.
    for request in [
        serde_json::json!({"op":"resolve","binding_token":token,"lease_id":lease_id,"observed_origin":"https://example.test"}),
        serde_json::json!({"op":"resolve","role":"injector","binding_token":token,"lease_id":lease_id,"observed_origin":"https://example.test"}),
    ] {
        let reply = ipc::call(&resolve, &serde_json::to_vec(&request).unwrap()).unwrap();
        assert!(!reply.success);
        assert_eq!(reply.code.as_deref(), Some("trusted_injection_required"));
        assert!(reply.credential.is_none());
    }
    // A separate worker process shares the UID but is not an admitted control
    // PID. It can connect to the socket and knows a valid lease: still no secret.
    let mut worker = Command::new("python3")
        .arg("-c")
        .arg("import socket,sys; s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); s.sendall(sys.stdin.buffer.read()); s.shutdown(socket.SHUT_WR); sys.stdout.buffer.write(s.makefile('rb').read())")
        .arg(&resolve)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("worker process");
    let direct = serde_json::json!({"op":"resolve","binding_token":token,"lease_id":lease_id,"observed_origin":"https://example.test"});
    worker
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&direct).unwrap())
        .unwrap();
    let output = worker.wait_with_output().unwrap();
    assert!(output.status.success());
    let denied: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(denied["code"], "trusted_injection_required");
    assert_eq!(denied["success"], false);
    assert!(denied.get("credential").is_none());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SENTINEL));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SENTINEL));
    // Exercise the actual stdio executable with a private inherited FD 3 binding.
    use std::os::fd::AsRawFd;
    use std::os::unix::{net::UnixStream, process::CommandExt};
    let next = serde_json::json!({"op":"grant","request":f.request("ipc-bridge",60)});
    let next_id = ipc::call(&control, &serde_json::to_vec(&next).expect("json"))
        .expect("grant")
        .lease_id
        .expect("lease");
    let (reader, mut writer) = UnixStream::pair().expect("pair");
    writer.write_all(token.as_bytes()).expect("binding write");
    writer
        .shutdown(std::net::Shutdown::Write)
        .expect("binding close");
    let read_fd = reader.as_raw_fd();
    let binary = std::env::var("CARGO_BIN_EXE_celeris-credentiald").expect("binary");
    let mut command = Command::new(binary);
    command
        .arg("bridge")
        .env_clear()
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &runtime)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(read_fd, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut bridge = command.spawn().expect("bridge spawn");
    drop(reader);
    let request = serde_json::json!({"protocol":"agent-browser.plugin.v1","type":"credential.resolve","capability":"credential.read","request":{"profileName":"default","itemRef":next_id,"url":"https://example.test/login"}});
    if let Some(mut stdin) = bridge.stdin.take() {
        stdin
            .write_all(&serde_json::to_vec(&request).expect("json"))
            .expect("plugin stdin")
    }
    let output = bridge.wait_with_output().expect("bridge output");
    assert!(output.status.success());
    let actual: serde_json::Value = serde_json::from_slice(&output.stdout).expect("bridge json");
    assert_eq!(actual["success"], false);
    assert!(actual.get("credential").is_none());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SENTINEL));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SENTINEL));
    let cross = ipc::call(&resolve, br#"{"op":"initialize_key"}"#).expect("resolve response");
    assert!(!cross.success);
    assert_eq!(cross.code.as_deref(), Some("trusted_injection_required"));
    let journal = fs::read_to_string(home.join(".local/celeris/credentiald/audit/journal.jsonl"))
        .expect("journal");
    assert!(!journal.contains(SENTINEL));
    let events: Vec<serde_json::Value> = journal
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(events.iter().filter(|e| e["action"] == "grant").count(), 2);
    assert!(!events.iter().any(|e| e["action"] == "use"));
}
#[test]
fn missing_key_symlink_hardlink_and_directory_modes_fail_closed() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.register();
    let key = f.root.path().join("config/keys/master-v1.key");
    let backup = f.root.path().join("key-backup");
    fs::rename(&key, &backup).expect("move");
    assert!(matches!(
        f.manual
            .resolve_registered(&f.reference, 1, &f.policy.exact_origin),
        Err(Error::VaultLocked)
    ));
    symlink(&backup, &key).expect("symlink");
    assert!(matches!(
        f.manual
            .resolve_registered(&f.reference, 1, &f.policy.exact_origin),
        Err(Error::VaultLocked)
    ));
    fs::remove_file(&key).expect("unlink");
    fs::hard_link(&backup, &key).expect("hardlink");
    assert!(matches!(
        f.manual
            .resolve_registered(&f.reference, 1, &f.policy.exact_origin),
        Err(Error::VaultLocked)
    ));
    fs::remove_file(&backup).expect("unlink backup");
    let data = f.root.path().join("data");
    fs::set_permissions(&data, fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(matches!(
        ManualProvider::open(f.root.path().join("config/keys"), data.join("vault")),
        Err(Error::Permission)
    ));
}
#[test]
fn different_run_session_or_revision_cannot_reuse_lease() {
    let f = Fixture::new();
    f.register();
    let good = f.bind();
    let lease = f.broker.grant(f.request("scope-a", 60)).expect("grant");
    let other = f
        .broker
        .register_binding(Binding {
            token: String::new(),
            task_id: "task-1".into(),
            run_id: "run-2".into(),
            session_id: "session-2".into(),
            exact_origin: f.policy.exact_origin.clone(),
            policy_hash: "hash-1".into(),
            expires_at: now() + 100,
        })
        .expect("binding");
    assert!(matches!(
        f.broker
            .resolve(&other, &lease.lease_id, &f.policy.exact_origin),
        Err(Error::Denied)
    ));
    f.manual
        .register(
            &f.reference,
            &f.policy,
            2,
            &SecretEnvelope {
                username: "new-user".into(),
                password: SENTINEL.into(),
            },
        )
        .expect("update");
    assert!(matches!(
        f.broker
            .resolve(&good, &lease.lease_id, &f.policy.exact_origin),
        Err(Error::Denied)
    ));
    assert!(matches!(
        f.broker.grant(f.request("old-revision", 60)),
        Err(Error::Denied)
    ));
    assert!(!f.audit().contains(SENTINEL));
}
#[test]
fn invalid_control_process_id_is_rejected() {
    use std::process::{Command, Stdio};
    let root = tempfile::tempdir().expect("tempdir");
    let home = root.path().join("home");
    let runtime = root.path().join("runtime");
    fs::create_dir(&home).expect("home");
    fs::create_dir(&runtime).expect("runtime");
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).expect("chmod");
    for p in [
        home.join(".config"),
        home.join(".config/celeris"),
        home.join(".local"),
        home.join(".local/celeris"),
    ] {
        fs::create_dir(&p).expect("parent")
    }
    let binary = std::env::var("CARGO_BIN_EXE_celeris-credentiald").expect("binary");
    let mut child = Command::new(binary)
        .arg("serve")
        .arg("1")
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env_remove("CELERIS_CREDENTIALD_DATA_DIR")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let control = runtime.join("celeris-credentiald/control.sock");
    let resolve = runtime.join("celeris-credentiald/resolve.sock");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if control.exists() && resolve.exists() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for fixture readiness"
        );
        thread::sleep(Duration::from_millis(10))
    }
    assert!(control.exists() && resolve.exists());
    let reply = ipc::call(&control, br#"{"op":"initialize_key"}"#).expect("response");
    assert!(!reply.success);
    assert_eq!(reply.code.as_deref(), Some("permission_denied"));
    child.kill().expect("kill");
    child.wait().expect("wait");
}
#[test]
fn failed_consume_audit_never_returns_secret_or_consumes_lease() {
    let f = Fixture::new();
    f.register();
    let token = f.bind();
    let grant = f.broker.grant(f.request("use-audit", 60)).expect("grant");
    let path = f.root.path().join("data/audit/journal.jsonl");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");
    assert!(matches!(
        f.broker
            .resolve(&token, &grant.lease_id, &f.policy.exact_origin),
        Err(Error::AuditUnavailable)
    ));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");
    let secret = f
        .broker
        .resolve(&token, &grant.lease_id, &f.policy.exact_origin)
        .expect("later resolve");
    assert_eq!(secret.password, SENTINEL);
    drop(secret);
    assert!(!f.audit().contains(SENTINEL));
}

/// ADR 2026-10-09 credential username / post-login D1-1・D2-1: 登録時の site policy の username selector と
/// post_login は vault に入り（秘密ではない）、承認画面用の describe に出て、grant は登録時の値と一致する
/// policy だけを受ける。
#[test]
fn registered_username_selector_and_post_login_pin_the_grant() {
    use task_core::browser_wait::{PostLogin, PostLoginAction};
    let mut f = Fixture::new();
    f.policy.login_url = Some("https://example.test/idp/login".into());
    f.policy.password_selector = Some("input[name=j_password]".into());
    f.policy.username_selector = Some("input[name=j_username]".into());
    f.policy.post_login = Some(PostLogin {
        read_origins: vec!["https://lms.example.test".into()],
        actions: vec![PostLoginAction::Snapshot, PostLoginAction::Click],
    });
    f.policy.consent = Some(task_core::browser_wait::ConsentPolicy {
        selector: "input[name=_eventId_proceed]".into(),
        choice_selector: None,
    });
    f.register();
    let described = f
        .manual
        .describe_registered(&f.reference, "https://example.test", Some(1))
        .expect("describe");
    assert_eq!(
        described.username_selector.as_deref(),
        Some("input[name=j_username]")
    );
    assert_eq!(described.post_login, f.policy.post_login);
    let vault = fs::read_to_string(f.root.path().join("data/vault/login-1.json")).expect("vault");
    assert!(!vault.contains(SENTINEL) && !vault.contains("test-user"));
    let mut swapped = f.request("k-user", 60);
    swapped.policy.username_selector = Some("#attacker".into());
    assert!(f.broker.grant(swapped).is_err());
    let mut dropped = f.request("k-none", 60);
    dropped.policy.username_selector = None;
    assert!(f.broker.grant(dropped).is_err());
    let mut widened = f.request("k-post", 60);
    widened.policy.post_login = Some(PostLogin {
        read_origins: vec!["https://other.example.test".into()],
        actions: vec![PostLoginAction::Snapshot],
    });
    assert!(f.broker.grant(widened).is_err());
    let mut other_button = f.request("k-consent", 60);
    other_button.policy.consent = Some(task_core::browser_wait::ConsentPolicy {
        selector: "input[name=_eventId_AttributeReleaseRejected]".into(),
        choice_selector: None,
    });
    assert!(f.broker.grant(other_button).is_err());
    assert_eq!(described.consent, f.policy.consent);
    assert!(f.broker.grant(f.request("k-ok", 60)).is_ok());
}

#[test]
fn owner_bound_saved_credential_is_reusable_only_for_matching_owner_and_trusted_login() {
    let mut f = Fixture::new();
    f.policy.login_url = Some("https://example.test/login".into());
    f.policy.password_selector = Some("input[name=password]".into());
    f.manual
        .register_owned(
            &f.reference,
            &f.policy,
            1,
            &SecretEnvelope {
                username: "user".into(),
                password: SENTINEL.into(),
            },
            Some("owner"),
        )
        .expect("register owner-bound credential");
    let trusted = f
        .manual
        .describe_registered(&f.reference, "https://example.test", Some(1))
        .unwrap();
    assert_eq!(
        f.manual
            .find_reusable("owner", "site-1", "https://example.test", &trusted)
            .unwrap(),
        Some(f.reference.clone())
    );
    assert_eq!(
        f.manual
            .find_reusable("another-owner", "site-1", "https://example.test", &trusted)
            .unwrap(),
        None
    );
    assert_eq!(
        f.manual
            .find_reusable("owner", "other-policy", "https://example.test", &trusted)
            .unwrap(),
        None
    );
    let listed = f.manual.list_owned("owner").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, f.reference);
    assert!(!format!("{listed:?}").contains(SENTINEL));
}

#[test]
fn saved_credentials_policy_change_expiry_legacy_and_owner_scoped_deletion() {
    let mut f = Fixture::new();
    f.policy.login_url = Some("https://example.test/login".into());
    f.policy.password_selector = Some("#password".into());
    f.register();
    let login = f
        .manual
        .describe_registered(&f.reference, "https://example.test", None)
        .unwrap();
    assert!(
        f.manual
            .find_reusable("owner", "site-1", "https://example.test", &login)
            .unwrap()
            .is_none(),
        "legacy has no owner"
    );
    f.manual
        .register_owned(
            &f.reference,
            &f.policy,
            1,
            &SecretEnvelope {
                username: "user".into(),
                password: SENTINEL.into(),
            },
            Some("owner"),
        )
        .unwrap();
    let mut changed = login.clone();
    changed.username_selector = Some("#username".into());
    assert!(
        f.manual
            .find_reusable("owner", "site-1", "https://example.test", &changed)
            .unwrap()
            .is_none()
    );
    assert!(f.manual.list_owned("other").unwrap().is_empty());
    assert!(f.manual.remove_owned(&f.reference, "other").is_err());
    assert!(
        f.manual
            .find_reusable("owner", "site-1", "https://example.test", &login)
            .unwrap()
            .is_some()
    );
    let path = f.root.path().join("data/vault/login-1.json");
    let bytes = fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(SENTINEL));
    let mut entry: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let created = entry["created_at"].as_u64().unwrap();
    assert!((1..=90 * 86400).contains(&(entry["expires_at"].as_u64().unwrap() - created)));
    entry["expires_at"] = serde_json::json!(1);
    fs::write(&path, serde_json::to_vec(&entry).unwrap()).unwrap();
    assert!(
        f.manual
            .find_reusable("owner", "site-1", "https://example.test", &login)
            .unwrap()
            .is_none()
    );
    assert!(
        f.manual
            .describe_registered(&f.reference, "https://example.test", None)
            .is_err()
    );
    assert_eq!(
        f.manual.list_owned("owner").unwrap().len(),
        1,
        "expired entries remain deletable"
    );
    f.manual.remove_owned(&f.reference, "owner").unwrap();
    assert!(!path.exists());
    assert!(
        f.manual
            .find_reusable("owner", "site-1", "https://example.test", &login)
            .unwrap()
            .is_none()
    );
    assert!(
        f.manual
            .resolve_registered(&f.reference, 1, "https://example.test")
            .is_err()
    );
}

#[test]
fn owned_credential_cannot_be_leased_by_another_approver() {
    let f = Fixture::new();
    f.manual
        .register_owned(
            &f.reference,
            &f.policy,
            1,
            &SecretEnvelope {
                username: "owner-user".into(),
                password: SENTINEL.into(),
            },
            Some("owner"),
        )
        .unwrap();
    let mut request = f.request("wrong-owner", 60);
    request.approved_by = "another-owner".into();
    assert!(f.broker.grant(request).is_err());
    let mut request = f.request("right-owner", 60);
    request.approved_by = "owner".into();
    assert!(f.broker.grant(request).is_ok());
}

#[test]
fn multiple_saved_credentials_revalidate_the_selected_reference() {
    let mut f = Fixture::new();
    f.policy.login_url = Some("https://example.test/login".into());
    f.policy.password_selector = Some("#password".into());
    let second = CredentialRef {
        credential_id: "login-2".into(),
        ..f.reference.clone()
    };
    for reference in [&f.reference, &second] {
        f.manual
            .register_owned(
                reference,
                &f.policy,
                1,
                &SecretEnvelope {
                    username: "user".into(),
                    password: SENTINEL.into(),
                },
                Some("owner"),
            )
            .unwrap();
    }
    let login = f
        .manual
        .describe_registered(&f.reference, "https://example.test", None)
        .unwrap();
    for reference in [&f.reference, &second] {
        assert!(
            f.manual
                .reusable_reference("owner", reference, "https://example.test", &login)
                .unwrap()
        );
        assert!(
            !f.manual
                .reusable_reference("other", reference, "https://example.test", &login)
                .unwrap()
        );
    }
    f.manual.remove(&f.reference).unwrap();
    assert!(
        f.manual
            .reusable_reference("owner", &f.reference, "https://example.test", &login)
            .is_err()
    );
    assert!(
        f.manual
            .reusable_reference("owner", &second, "https://example.test", &login)
            .unwrap()
    );
}
