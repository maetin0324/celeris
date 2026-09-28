use crate::{
    Binding, Broker, CredentialPolicy, CredentialRef, Error, LeaseRequest, SecretEnvelope,
    canonical_origin,
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
}
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResolveRequest {
    Resolve {
        binding_token: String,
        lease_id: String,
        observed_origin: String,
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
fn serve_one(mut stream: UnixStream, broker: &Broker, control: bool, control_pids: &[(u32, u64)]) {
    let reply = match (|| -> Result<IpcReply, Error> {
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .map_err(|_| Error::Io)?;
        let mut bytes = read_limited(&mut stream)?;
        let (uid, pid) = peer(&stream)?;
        if uid != local_uid()
            || (control
                && !control_pids
                    .iter()
                    .any(|(allowed, start)| *allowed == pid && process_start(pid) == Some(*start)))
        {
            bytes.zeroize();
            return Err(Error::Permission);
        }
        let response = (|| -> Result<IpcReply, Error> {
            if control {
                match serde_json::from_slice::<ControlRequest>(&bytes)
                    .map_err(|_| Error::Invalid)?
                {
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
                }
            } else {
                match serde_json::from_slice::<ResolveRequest>(&bytes)
                    .map_err(|_| Error::Invalid)?
                {
                    ResolveRequest::Resolve {
                        binding_token,
                        lease_id,
                        observed_origin,
                    } => {
                        let secret = broker.resolve(&binding_token, &lease_id, &observed_origin)?;
                        let mut out = IpcReply::ok();
                        out.credential = Some(IpcCredential {
                            username: secret.username.clone(),
                            password: secret.password.clone(),
                        });
                        Ok(out)
                    }
                }
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
pub fn serve(broker: Arc<Broker>, runtime: &Path, control_pids: Vec<u32>) -> Result<(), Error> {
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
    let b = Arc::clone(&broker);
    thread::spawn(move || {
        for s in control.incoming().flatten() {
            serve_one(s, &b, true, &control_pids)
        }
    });
    for s in resolve.incoming().flatten() {
        serve_one(s, &broker, false, &[])
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginRequest {
    protocol: String,
    #[serde(rename = "type")]
    kind: String,
    capability: String,
    request: PluginArgs,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PluginArgs {
    profile_name: Option<String>,
    item_ref: Option<String>,
    url: Option<String>,
}
#[derive(Serialize)]
struct PluginResponse {
    protocol: &'static str,
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential: Option<IpcCredential>,
}
/// The private plugin pipe is the only public function that serializes a secret.
/// The binding token is supplied out of band by the supervisor, never by plugin JSON.
pub fn bridge_request(input: &[u8], binding_token: &str, resolve_socket: &Path) -> Vec<u8> {
    let result = (|| -> Result<IpcCredential, Error> {
        let req: PluginRequest = serde_json::from_slice(input).map_err(|_| Error::Invalid)?;
        if req.protocol != "agent-browser.plugin.v1"
            || req.kind != "credential.resolve"
            || req.capability != "credential.read"
        {
            return Err(Error::Denied);
        }
        // profileName identifies the browser profile; it is not broker authority.
        let _ = req.request.profile_name;
        let lease_id = req.request.item_ref.ok_or(Error::Invalid)?;
        let url = req.request.url.ok_or(Error::Invalid)?;
        let origin = url_origin(&url)?;
        let ipc = serde_json::json!({"op":"resolve","binding_token":binding_token,"lease_id":lease_id,"observed_origin":origin});
        let bytes = serde_json::to_vec(&ipc).map_err(|_| Error::Invalid)?;
        let reply = call(resolve_socket, &bytes)?;
        if !reply.success {
            return Err(Error::Denied);
        }
        reply.credential.ok_or(Error::Denied)
    })();
    match serde_json::to_vec(&PluginResponse {
        protocol: "agent-browser.plugin.v1",
        success: result.is_ok(),
        credential: result.ok(),
    }) {
        Ok(bytes) => bytes,
        Err(_) => b"{\"protocol\":\"agent-browser.plugin.v1\",\"success\":false}".to_vec(),
    }
}
fn url_origin(url: &str) -> Result<String, Error> {
    let rest = url.strip_prefix("https://").ok_or(Error::Invalid)?;
    let authority = rest.split(['/', '?', '#']).next().ok_or(Error::Invalid)?;
    canonical_origin(&format!("https://{authority}"))
}
pub fn resolve_socket(runtime: &Path) -> PathBuf {
    runtime.join("celeris-credentiald/resolve.sock")
}
