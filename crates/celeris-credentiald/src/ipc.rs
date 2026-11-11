use crate::{
    Binding, Broker, CredentialPolicy, CredentialRef, Error, LeaseRequest, SecretEnvelope,
    injection_ipc::{
        Admission, AuthSectionRegistration, InjectCode, InjectionReply, InjectionService,
        LauncherProofRegistration, LiveRegistry, LiveSessionRegistration, PeerCred, SeqpacketSink,
        write_reply,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::{
        fd::OwnedFd,
        unix::{
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
            io::AsRawFd,
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlRequest {
    /// Read-only health check, after the same UID/PID admission as every control request.
    Ping,
    InitializeKey,
    Register {
        reference: CredentialRef,
        policy: CredentialPolicy,
        revision: u64,
        secret: SecretEnvelope,
        #[serde(default)]
        owner_id: Option<String>,
    },
    Inspect {
        reference: CredentialRef,
        revision: u64,
        origin: String,
    },
    DescribePolicy {
        reference: CredentialRef,
        origin: String,
    },
    FindReusable {
        owner_id: String,
        policy_id: String,
        origin: String,
        trusted_login: task_core::browser_wait::TrustedLogin,
    },
    CheckReusable {
        owner_id: String,
        reference: CredentialRef,
        origin: String,
        trusted_login: task_core::browser_wait::TrustedLogin,
    },
    ListOwned {
        owner_id: String,
    },
    RemoveOwned {
        owner_id: String,
        reference: CredentialRef,
    },
    Invalidate {
        reference: CredentialRef,
    },
    Revoke {
        lease_id: String,
        actor_id: String,
    },
    Bind {
        binding: Binding,
    },
    Grant {
        request: LeaseRequest,
    },
    // ADR-0109 D2: live isolated sessions and H3 auth sections (memory only).
    RegisterLiveSession(LiveSessionRegistration),
    UnregisterLiveSession {
        session_id: String,
    },
    // ADR-0138 D-L: the launcher session proof the daemon received for a live session.
    AttachLauncherProof(LauncherProofRegistration),
    OpenAuthSection(AuthSectionRegistration),
    CloseAuthSection {
        session_id: String,
        auth_section_id: String,
    },
}
#[derive(Serialize, Deserialize)]
pub struct IpcReply {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<IpcCredential>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted_login: Option<task_core::browser_wait::TrustedLogin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<CredentialRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_credentials: Option<Vec<(CredentialRef, u64, u64)>>,
}
#[derive(Serialize, Deserialize)]
pub struct IpcCredential {
    pub username: String,
    pub password: String,
}
impl Drop for IpcCredential {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}
impl IpcReply {
    fn ok() -> Self {
        Self {
            success: true,
            code: None,
            binding_token: None,
            lease_id: None,
            expires_at: None,
            credential: None,
            trusted_login: None,
            credential_ref: None,
            saved_credentials: None,
        }
    }
    fn code(code: InjectCode) -> Self {
        Self {
            success: false,
            code: Some(code.code().into()),
            binding_token: None,
            lease_id: None,
            expires_at: None,
            credential: None,
            trusted_login: None,
            credential_ref: None,
            saved_credentials: None,
        }
    }
    fn err(e: Error) -> Self {
        Self {
            success: false,
            code: Some(e.code().into()),
            binding_token: None,
            lease_id: None,
            expires_at: None,
            credential: None,
            trusted_login: None,
            credential_ref: None,
            saved_credentials: None,
        }
    }
}
fn peer(stream: &UnixStream) -> Result<(u32, u32), Error> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut len,
        )
    };
    if rc != 0 || len as usize != std::mem::size_of::<libc::ucred>() {
        return Err(Error::Permission);
    }
    Ok((cred.uid, cred.pid as u32))
}
fn local_uid() -> u32 {
    unsafe { libc::geteuid() }
}
fn process_start(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let mut fields = stat.rsplit_once(") ")?.1.split_whitespace();
    fields.nth(19)?.parse().ok()
}
fn read_limited<R: Read>(r: R) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    r.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Io)?;
    if bytes.len() > 65536 {
        bytes.zeroize();
        return Err(Error::Invalid);
    }
    Ok(bytes)
}
fn serve_one(
    mut stream: UnixStream,
    broker: &Broker,
    registry: &LiveRegistry,
    control: bool,
    control_pids: &[(u32, u64)],
) {
    let reply = match (|| -> Result<IpcReply, Error> {
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .map_err(|_| Error::Io)?;
        let (uid, pid) = peer(&stream)?;
        let mut bytes = read_limited(&mut stream)?;
        if uid != local_uid()
            || (control
                && !control_pids
                    .iter()
                    .any(|(allowed, start)| *allowed == pid && process_start(pid) == Some(*start)))
        {
            bytes.zeroize();
            return Err(Error::Permission);
        }
        // ADR-0103: even an admitted control PID with a valid binding/lease
        // cannot retrieve secrets over this retired endpoint. Read only the
        // bounded frame for orderly shutdown; never parse or consume it.
        if !control {
            bytes.zeroize();
            return Err(Error::TrustedInjectionRequired);
        }
        let response = (|| -> Result<IpcReply, Error> {
            match serde_json::from_slice::<ControlRequest>(&bytes).map_err(|_| Error::Invalid)? {
                ControlRequest::Ping => Ok(IpcReply::ok()),
                ControlRequest::InitializeKey => {
                    broker.provider().initialize_key()?;
                    Ok(IpcReply::ok())
                }
                ControlRequest::Register {
                    reference,
                    policy,
                    revision,
                    secret,
                    owner_id,
                } => {
                    broker.provider().register_owned(
                        &reference,
                        &policy,
                        revision,
                        &secret,
                        owner_id.as_deref(),
                    )?;
                    Ok(IpcReply::ok())
                }
                ControlRequest::Inspect {
                    reference,
                    revision,
                    origin,
                } => {
                    // Verification performs authenticated decryption, but never returns the secret.
                    let _secret = broker
                        .provider()
                        .resolve_registered(&reference, revision, &origin)?;
                    Ok(IpcReply::ok())
                }
                ControlRequest::DescribePolicy { reference, origin } => {
                    let trusted = broker
                        .provider()
                        .describe_registered(&reference, &origin, None)?;
                    let mut out = IpcReply::ok();
                    out.trusted_login = Some(trusted);
                    Ok(out)
                }
                ControlRequest::FindReusable {
                    owner_id,
                    policy_id,
                    origin,
                    trusted_login,
                } => {
                    let mut out = IpcReply::ok();
                    out.credential_ref = broker.provider().find_reusable(
                        &owner_id,
                        &policy_id,
                        &origin,
                        &trusted_login,
                    )?;
                    Ok(out)
                }
                ControlRequest::CheckReusable {
                    owner_id,
                    reference,
                    origin,
                    trusted_login,
                } => {
                    let mut out = IpcReply::ok();
                    if broker.provider().reusable_reference(
                        &owner_id,
                        &reference,
                        &origin,
                        &trusted_login,
                    )? {
                        out.credential_ref = Some(reference);
                    }
                    Ok(out)
                }
                ControlRequest::ListOwned { owner_id } => {
                    let mut out = IpcReply::ok();
                    out.saved_credentials = Some(broker.provider().list_owned(&owner_id)?);
                    Ok(out)
                }
                ControlRequest::RemoveOwned {
                    owner_id,
                    reference,
                } => {
                    broker.provider().remove_owned(&reference, &owner_id)?;
                    Ok(IpcReply::ok())
                }
                ControlRequest::Invalidate { reference } => {
                    broker.provider().remove(&reference)?;
                    Ok(IpcReply::ok())
                }
                ControlRequest::Revoke { lease_id, actor_id } => {
                    broker.revoke(&lease_id, &actor_id)?;
                    Ok(IpcReply::ok())
                }
                ControlRequest::Bind { binding } => {
                    let token = broker.register_binding(binding)?;
                    let mut out = IpcReply::ok();
                    out.binding_token = Some(token);
                    Ok(out)
                }
                ControlRequest::Grant { request } => {
                    let grant = broker.grant(request)?;
                    let mut out = IpcReply::ok();
                    out.lease_id = Some(grant.lease_id);
                    out.expires_at = Some(grant.expires_at);
                    Ok(out)
                }
                ControlRequest::RegisterLiveSession(r) => Ok(registry
                    .register(r)
                    .map_or_else(IpcReply::code, |_| IpcReply::ok())),
                ControlRequest::UnregisterLiveSession { session_id } => Ok(registry
                    .unregister(&session_id)
                    .map_or_else(IpcReply::code, |_| IpcReply::ok())),
                ControlRequest::AttachLauncherProof(l) => Ok(registry
                    .attach_launcher_proof(l)
                    .map_or_else(IpcReply::code, |_| IpcReply::ok())),
                ControlRequest::OpenAuthSection(a) => Ok(registry
                    .open_section(a)
                    .map_or_else(IpcReply::code, |_| IpcReply::ok())),
                ControlRequest::CloseAuthSection {
                    session_id,
                    auth_section_id,
                } => Ok(registry
                    .close_section(&session_id, &auth_section_id)
                    .map_or_else(IpcReply::code, |_| IpcReply::ok())),
            }
        })();
        bytes.zeroize();
        response
    })() {
        Ok(reply) => reply,
        Err(error) => IpcReply::err(error),
    };
    if let Ok(mut bytes) = serde_json::to_vec(&reply) {
        let _ = stream.write_all(&bytes);
        bytes.zeroize();
    }
}
fn socket(path: &Path) -> Result<UnixListener, Error> {
    if let Ok(m) = std::fs::symlink_metadata(path) {
        if !m.file_type().is_socket() || m.uid() != local_uid() || m.mode() & 0o777 != 0o600 {
            return Err(Error::Permission);
        }
        if UnixStream::connect(path).is_ok() {
            return Err(Error::Permission);
        }
        std::fs::remove_file(path).map_err(|_| Error::Io)?;
    }
    let listener = UnixListener::bind(path).map_err(|_| Error::Io)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| Error::Io)?;
    let m = std::fs::symlink_metadata(path).map_err(|_| Error::Permission)?;
    if !m.file_type().is_socket() || m.uid() != local_uid() || m.mode() & 0o777 != 0o600 {
        return Err(Error::Permission);
    }
    Ok(listener)
}
/// 同時に受け付けておける `injection.sock` の接続の数（ADR 2026-11-11-credentiald-passed-stream-wait D5）。
/// accept から応答の書き込みまでを数えるので、待ちの thread と照合 thread の順番待ちもこの数を超えない。
pub const MAX_WAITING_STREAMS: usize = 8;

/// 受け付けている接続の枠（D5）。
#[derive(Default)]
struct WaitSlots(Arc<AtomicUsize>);

/// 1 接続の枠。drop で戻るので、期限切れ・拒否・thread の起動の失敗・panic のどの抜け道でも数が戻る。
struct WaitSlot(Arc<AtomicUsize>);

impl WaitSlots {
    fn try_take(&self) -> Option<WaitSlot> {
        self.0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < MAX_WAITING_STREAMS).then_some(n + 1)
            })
            .ok()?;
        Some(WaitSlot(Arc::clone(&self.0)))
    }
}

impl Drop for WaitSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 待ちの thread が読めた 1 要求。peer は受け付けた直後に取った値。body は drop で zeroize する。
struct ReadInjection {
    stream: Arc<UnixStream>,
    peer: PeerCred,
    body: Zeroizing<Vec<u8>>,
    fds: Vec<OwnedFd>,
    _slot: WaitSlot,
}

fn denied(code: InjectCode) -> InjectionReply {
    InjectionReply {
        v: 1,
        request_id: String::new(),
        ok: false,
        receipt: None,
        code: Some(code.code().into()),
        redisplay_guard: None,
    }
}

/// `injection.sock` の受付（ADR-0109 D1、ADR 2026-11-11-credentiald-passed-stream-wait D4・D5）。1 接続 = 1 要求 =
/// 1 応答。最初の要求の待ちだけを接続ごとの thread で行い（[`InjectionService::await_first_request`]）、
/// 読めた要求の照合（[`InjectionService::handle`]、D3 の照合順・lease の消費・注入）と応答の書き込みは
/// 1 本の照合 thread が受け取った順に行う。`on_wait` は待ちの thread が待ちに入る直前に呼ばれる
/// （本番は何もしない）。`streams` が尽きると、すべての thread が終わってから戻る。
fn accept_injection(
    streams: impl Iterator<Item = UnixStream>,
    service: &InjectionService,
    on_wait: &(dyn Fn() + Sync),
) {
    let (to_matcher, requests) = mpsc::channel::<ReadInjection>();
    let slots = WaitSlots::default();
    thread::scope(|scope| {
        scope.spawn(move || {
            for request in requests {
                answer_request(request, service);
            }
        });
        for stream in streams {
            admit_stream(stream, &slots, |stream, slot| {
                let to_matcher = to_matcher.clone();
                thread::Builder::new()
                    .name("injection-wait".into())
                    .spawn_scoped(scope, move || {
                        wait_and_forward(&stream, slot, service, &to_matcher, on_wait)
                    })
                    .map(drop)
            });
        }
        drop(to_matcher);
    });
}

/// 1 接続に枠を取り、`spawn` で待ちの thread を起こす。枠が無い・thread を起こせないときは、要求を読まずに
/// `invalid_request` を書いて閉じる（lease にも provider にも触れない）。
fn admit_stream(
    stream: UnixStream,
    slots: &WaitSlots,
    spawn: impl FnOnce(Arc<UnixStream>, WaitSlot) -> std::io::Result<()>,
) {
    let stream = Arc::new(stream);
    let started = match slots.try_take() {
        Some(slot) => spawn(Arc::clone(&stream), slot).is_ok(),
        None => false,
    };
    if !started {
        write_reply(&stream, &denied(InjectCode::InvalidRequest));
    }
}

/// 待ちの thread: peer を取り、最初の要求を待って読み、照合 thread へ渡す。読めなければ固定の拒否を書く。
fn wait_and_forward(
    stream: &Arc<UnixStream>,
    slot: WaitSlot,
    service: &InjectionService,
    to_matcher: &mpsc::Sender<ReadInjection>,
    on_wait: &(dyn Fn() + Sync),
) {
    let read = (|| {
        stream
            .set_read_timeout(Some(crate::injection_ipc::IO_TIMEOUT))
            .map_err(|_| InjectCode::InvalidRequest)?;
        let (uid, pid) = peer(stream).map_err(|_| InjectCode::PeerUidMismatch)?;
        let peer = PeerCred { uid, pid };
        on_wait();
        service.await_first_request(stream.as_raw_fd(), peer)?;
        let (body, fds) = crate::injection_ipc::read_request(stream.as_raw_fd())?;
        Ok((peer, Zeroizing::new(body), fds))
    })();
    match read {
        Ok((peer, body, fds)) => {
            let request = ReadInjection {
                stream: Arc::clone(stream),
                peer,
                body,
                fds,
                _slot: slot,
            };
            // 照合 thread が無ければ拒否する。渡せなかった要求の body は drop で zeroize される。
            if let Err(mpsc::SendError(request)) = to_matcher.send(request) {
                write_reply(&request.stream, &denied(InjectCode::InvalidRequest));
            }
        }
        Err(code) => write_reply(stream, &denied(code)),
    }
}

/// 照合 thread の 1 要求: sink FD を確かめ、D3 の照合をして応答を書く。
fn answer_request(request: ReadInjection, service: &InjectionService) {
    let ReadInjection {
        stream,
        peer,
        body,
        mut fds,
        _slot,
    } = request;
    let sink = match (fds.pop(), fds.is_empty()) {
        (Some(fd), true) => SeqpacketSink::new(fd),
        _ => Err(InjectCode::InvalidRequest),
    };
    let reply = match sink {
        Ok(mut sink) => service.handle(peer, &body, &mut sink),
        Err(c) => denied(c),
    };
    drop(body);
    write_reply(&stream, &reply);
}

/// 試験専用（ADR-0109 D6 と同じ feature）: 本番と同じ受付を `streams` について回す。`on_wait` は待ちの
/// thread が最初の要求の待ちに入る直前に呼ばれる。`streams` が尽きると、すべての thread が終わってから戻る。
#[cfg(feature = "same-uid-harness")]
pub fn serve_injection_streams(
    streams: impl Iterator<Item = UnixStream>,
    service: &InjectionService,
    on_wait: &(dyn Fn() + Sync),
) {
    accept_injection(streams, service, on_wait)
}
pub fn serve(broker: Arc<Broker>, runtime: &Path, control_pids: Vec<u32>) -> Result<(), Error> {
    serve_attested(broker, runtime, control_pids, None)
}
/// Production serve: `Attested` checks launcher session proofs against `launcher_uid`
/// (ADR-0138 D-L). Without a configured launcher UID no session is admitted.
pub fn serve_attested(
    broker: Arc<Broker>,
    runtime: &Path,
    control_pids: Vec<u32>,
    launcher_uid: Option<u32>,
) -> Result<(), Error> {
    serve_inner(
        broker,
        runtime,
        control_pids,
        Admission::Attested,
        launcher_uid,
    )
}
/// [`serve`] with an explicit admission. Production (`main`) always uses `Attested`;
/// other admissions exist only under the `same-uid-harness` test feature.
pub fn serve_with(
    broker: Arc<Broker>,
    runtime: &Path,
    control_pids: Vec<u32>,
    admission: Admission,
) -> Result<(), Error> {
    serve_inner(broker, runtime, control_pids, admission, None)
}
fn serve_inner(
    broker: Arc<Broker>,
    runtime: &Path,
    control_pids: Vec<u32>,
    admission: Admission,
    launcher_uid: Option<u32>,
) -> Result<(), Error> {
    let control_pids: Vec<(u32, u64)> = control_pids
        .into_iter()
        .map(|pid| {
            process_start(pid)
                .map(|start| (pid, start))
                .ok_or(Error::Permission)
        })
        .collect::<Result<_, _>>()?;
    let metadata = std::fs::symlink_metadata(runtime).map_err(|_| Error::Permission)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != local_uid()
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(Error::Permission);
    }
    let dir = runtime.join("celeris-credentiald");
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|_| Error::Io)?
    }
    let m = std::fs::symlink_metadata(&dir).map_err(|_| Error::Permission)?;
    if !m.is_dir()
        || m.file_type().is_symlink()
        || m.uid() != local_uid()
        || m.mode() & 0o777 != 0o700
    {
        return Err(Error::Permission);
    }
    let control = socket(&dir.join("control.sock"))?;
    let resolve = socket(&dir.join("resolve.sock"))?;
    let injection = socket(&dir.join("injection.sock"))?;
    let registry = Arc::new(LiveRegistry::default());
    let b = Arc::clone(&broker);
    let r = Arc::clone(&registry);
    thread::spawn(move || {
        for s in control.incoming().flatten() {
            serve_one(s, &b, &r, true, &control_pids)
        }
    });
    let service = InjectionService::new(Arc::clone(&broker), Arc::clone(&registry), admission)
        .with_launcher_uid(launcher_uid);
    thread::spawn(move || accept_injection(injection.incoming().flatten(), &service, &|| {}));
    for s in resolve.incoming().flatten() {
        serve_one(s, &broker, &registry, false, &[])
    }
    Ok(())
}
pub fn call(socket: &Path, request: &[u8]) -> Result<IpcReply, Error> {
    let mut stream = UnixStream::connect(socket).map_err(|_| Error::Io)?;
    stream.write_all(request).map_err(|_| Error::Io)?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|_| Error::Io)?;
    let mut b = read_limited(&mut stream)?;
    let result = serde_json::from_slice(&b).map_err(|_| Error::Invalid);
    b.zeroize();
    result
}
/// Retired agent-browser credential-read protocol (ADR-0103).
/// Keep the fixed protocol response for old callers, but never contact a socket
/// or serialize a secret. Trusted injection uses a separate controller endpoint.
pub fn bridge_request(_input: &[u8], _binding_token: &str, _resolve_socket: &Path) -> Vec<u8> {
    br#"{"protocol":"agent-browser.plugin.v1","success":false}"#.to_vec()
}
pub fn resolve_socket(runtime: &Path) -> PathBuf {
    runtime.join("celeris-credentiald/resolve.sock")
}

#[cfg(test)]
mod accept_tests {
    //! ADR 2026-11-11-credentiald-passed-stream-wait D5: 受付の枠は、どの抜け道でも戻る。
    use super::*;

    fn taken(slots: &WaitSlots) -> usize {
        slots.0.load(Ordering::SeqCst)
    }

    fn reply_code(theirs: &mut UnixStream) -> Option<String> {
        let mut len = [0u8; 4];
        theirs.read_exact(&mut len).expect("reply length");
        let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
        theirs.read_exact(&mut body).expect("reply body");
        serde_json::from_slice::<InjectionReply>(&body)
            .expect("reply")
            .code
    }

    /// 待ちの thread が panic しても枠は戻り、次の接続が枠を取れる。
    #[test]
    fn concurrent_first_request_slot_returns_when_the_waiting_thread_panics() {
        let slots = WaitSlots::default();
        let held: Vec<WaitSlot> = (0..MAX_WAITING_STREAMS - 1)
            .map(|_| slots.try_take().expect("slot"))
            .collect();
        let last = slots.try_take().expect("last slot");
        assert!(
            slots.try_take().is_none(),
            "the limit is {MAX_WAITING_STREAMS}"
        );
        let panicked = thread::spawn(move || {
            let _slot = last;
            panic!("waiting thread panics");
        })
        .join();
        assert!(panicked.is_err());
        assert_eq!(taken(&slots), MAX_WAITING_STREAMS - 1);
        assert!(slots.try_take().is_some());
        drop(held);
        assert_eq!(taken(&slots), 0);
    }

    /// 枠が無い接続と thread を起こせなかった接続は、`spawn` に渡さず（または渡しても失敗のまま）
    /// `invalid_request` を返して閉じる。どちらも枠は残らない。
    #[test]
    fn concurrent_first_request_over_the_limit_or_without_a_thread_is_denied() {
        let slots = WaitSlots::default();
        let held: Vec<WaitSlot> = (0..MAX_WAITING_STREAMS)
            .map(|_| slots.try_take().expect("slot"))
            .collect();
        let (ours, mut theirs) = UnixStream::pair().expect("pair");
        let mut spawned = false;
        admit_stream(ours, &slots, |_, _| {
            spawned = true;
            Ok(())
        });
        assert!(!spawned, "a stream over the limit must not get a thread");
        assert_eq!(reply_code(&mut theirs).as_deref(), Some("invalid_request"));
        assert_eq!(taken(&slots), MAX_WAITING_STREAMS);
        drop(held);

        let (ours, mut theirs) = UnixStream::pair().expect("pair");
        admit_stream(ours, &slots, |_, _| {
            Err(std::io::Error::other("thread could not be started"))
        });
        assert_eq!(reply_code(&mut theirs).as_deref(), Some("invalid_request"));
        assert_eq!(taken(&slots), 0);
    }
}
