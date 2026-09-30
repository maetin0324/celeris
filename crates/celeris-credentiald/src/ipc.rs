use crate::{
    Binding, Broker, CredentialPolicy, CredentialRef, Error, LeaseRequest, SecretEnvelope,
    injection_ipc::{
        Admission, AuthSectionRegistration, InjectCode, InjectionReply, InjectionService,
        LiveRegistry, LiveSessionRegistration, PeerCred, SeqpacketSink,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
        io::AsRawFd,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};
use zeroize::Zeroize;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlRequest {
    InitializeKey,
    Register {
        reference: CredentialRef,
        policy: CredentialPolicy,
        revision: u64,
        secret: SecretEnvelope,
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
    // ADR-0089 D2: live isolated sessions and H3 auth sections (memory only).
    RegisterLiveSession(LiveSessionRegistration),
    UnregisterLiveSession {
        session_id: String,
    },
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
        // ADR-0085: even an admitted control PID with a valid binding/lease
        // cannot retrieve secrets over this retired endpoint. Read only the
        // bounded frame for orderly shutdown; never parse or consume it.
        if !control {
            bytes.zeroize();
            return Err(Error::TrustedInjectionRequired);
        }
        let response = (|| -> Result<IpcReply, Error> {
            match serde_json::from_slice::<ControlRequest>(&bytes).map_err(|_| Error::Invalid)? {
                ControlRequest::InitializeKey => {
                    broker.provider().initialize_key()?;
                    Ok(IpcReply::ok())
                }
                ControlRequest::Register {
                    reference,
                    policy,
                    revision,
                    secret,
                } => {
                    broker
                        .provider()
                        .register(&reference, &policy, revision, &secret)?;
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
/// One `injection.sock` connection (ADR-0089 D1): one request, one reply.
fn serve_injection(mut stream: UnixStream, service: &InjectionService) {
    let denied = |c: InjectCode| InjectionReply {
        v: 1,
        request_id: String::new(),
        ok: false,
        receipt: None,
        code: Some(c.code().into()),
        redisplay_guard: None,
    };
    let reply = (|| -> InjectionReply {
        if stream
            .set_read_timeout(Some(crate::injection_ipc::IO_TIMEOUT))
            .is_err()
        {
            return denied(InjectCode::InvalidRequest);
        }
        let Ok((uid, pid)) = peer(&stream) else {
            return denied(InjectCode::PeerUidMismatch);
        };
        let (mut body, mut fds) = match crate::injection_ipc::read_request(stream.as_raw_fd()) {
            Ok(v) => v,
            Err(c) => return denied(c),
        };
        let sink = match (fds.pop(), fds.is_empty()) {
            (Some(fd), true) => SeqpacketSink::new(fd),
            _ => Err(InjectCode::InvalidRequest),
        };
        let reply = match sink {
            Ok(mut sink) => service.handle(PeerCred { uid, pid }, &body, &mut sink),
            Err(c) => denied(c),
        };
        body.zeroize();
        reply
    })();
    crate::injection_ipc::write_reply(&mut stream, &reply);
}
pub fn serve(broker: Arc<Broker>, runtime: &Path, control_pids: Vec<u32>) -> Result<(), Error> {
    serve_with(broker, runtime, control_pids, Admission::Attested)
}
/// [`serve`] with an explicit admission. Production (`main`) always uses `Attested`;
/// other admissions exist only under the `same-uid-harness` test feature.
pub fn serve_with(
    broker: Arc<Broker>,
    runtime: &Path,
    control_pids: Vec<u32>,
    admission: Admission,
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
    let service = InjectionService::new(Arc::clone(&broker), Arc::clone(&registry), admission);
    thread::spawn(move || {
        for s in injection.incoming().flatten() {
            serve_injection(s, &service)
        }
    });
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
/// Retired agent-browser credential-read protocol (ADR-0085).
/// Keep the fixed protocol response for old callers, but never contact a socket
/// or serialize a secret. Trusted injection uses a separate controller endpoint.
pub fn bridge_request(_input: &[u8], _binding_token: &str, _resolve_socket: &Path) -> Vec<u8> {
    br#"{"protocol":"agent-browser.plugin.v1","success":false}"#.to_vec()
}
pub fn resolve_socket(runtime: &Path) -> PathBuf {
    runtime.join("celeris-credentiald/resolve.sock")
}
