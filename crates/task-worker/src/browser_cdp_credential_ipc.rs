//! Credential broker control and injection IPC; replies contain receipts only.
use super::{InjectionError, TIMEOUT, broker_denial};
use celeris_credentiald::{
    injection_ipc::{AuthSectionRegistration, LiveSessionRegistration},
    ipc,
};
use serde_json::{Value, json};
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use zeroize::Zeroize;

/// A completed broker call returns only a receipt. The FD carries the CDP
/// command in the other direction and is never embedded in this response.
pub trait PendingInjection: Send {
    fn finish(self: Box<Self>) -> Result<Value, InjectionError>;
}

/// IPC transport is supplied by credentiald integration. A fake can use the
/// same sink protocol without opening a production broker socket.
pub trait BrokerClient {
    fn start(
        &mut self,
        request: Value,
        sink: OwnedFd,
    ) -> Result<Box<dyn PendingInjection>, InjectionError>;
}

/// The production injection-only IPC client. It never opens the retired
/// resolve endpoint; the path must be the broker's injection.sock.
pub struct UnixInjectionClient {
    socket: PathBuf,
}

impl UnixInjectionClient {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    fn control(&self, request: Value) -> Result<(), InjectionError> {
        self.check_socket()?;
        let path = self.socket.with_file_name("control.sock");
        let bytes = serde_json::to_vec(&request).map_err(|_| InjectionError::SinkFailed)?;
        let reply = ipc::call(&path, &bytes).map_err(|_| InjectionError::SinkFailed)?;
        if reply.success {
            Ok(())
        } else {
            let code = json!({"code":reply.code});
            Err(broker_denial(&code))
        }
    }

    fn check_socket(&self) -> Result<(), InjectionError> {
        if self
            .socket
            .file_name()
            .is_some_and(|name| name == "injection.sock")
        {
            Ok(())
        } else {
            Err(InjectionError::SinkFailed)
        }
    }

    pub fn register_live_session(
        &self,
        registration: LiveSessionRegistration,
    ) -> Result<(), InjectionError> {
        self.control(json!({"op":"register_live_session","session_id":registration.session_id,
            "controller_pid":registration.controller_pid,"controller_start":registration.controller_start,
            "runtime_pid":registration.runtime_pid,"runtime_start":registration.runtime_start}))
    }

    /// Bind the launcher proof to the live broker session using the credentiald control wire shape.
    /// The proof contains process identity only; no credential material crosses this call.
    pub fn attach_launcher_proof(
        &self,
        registration: celeris_credentiald::injection_ipc::LauncherProofRegistration,
    ) -> Result<(), InjectionError> {
        self.control(
            json!({"op":"attach_launcher_proof","session_id":registration.session_id,
            "instance_id":registration.instance_id,"peer_uid":registration.peer_uid,
            "proof":registration.proof}),
        )
    }

    pub fn open_auth_section(
        &self,
        section: AuthSectionRegistration,
    ) -> Result<(), InjectionError> {
        self.control(
            json!({"op":"open_auth_section","session_id":section.session_id,
            "auth_section_id":section.auth_section_id,"lease_id":section.lease_id,
            "exact_origin":section.exact_origin,"cdp_target_id":section.cdp_target_id}),
        )
    }

    pub fn close_auth_section(
        &self,
        session_id: &str,
        auth_section_id: &str,
    ) -> Result<(), InjectionError> {
        self.control(json!({"op":"close_auth_section","session_id":session_id,"auth_section_id":auth_section_id}))
    }

    pub fn unregister_live_session(&self, session_id: &str) -> Result<(), InjectionError> {
        self.control(json!({"op":"unregister_live_session","session_id":session_id}))
    }
}

struct UnixPending(UnixStream);

impl PendingInjection for UnixPending {
    fn finish(mut self: Box<Self>) -> Result<Value, InjectionError> {
        let mut size = [0u8; 4];
        self.0
            .read_exact(&mut size)
            .map_err(|_| InjectionError::SinkFailed)?;
        let len = u32::from_be_bytes(size) as usize;
        if len == 0 || len > 16 * 1024 {
            return Err(InjectionError::SinkFailed);
        }
        let mut body = vec![0u8; len];
        self.0
            .read_exact(&mut body)
            .map_err(|_| InjectionError::SinkFailed)?;
        let response = serde_json::from_slice(&body).map_err(|_| InjectionError::SinkFailed);
        body.zeroize();
        response
    }
}

impl BrokerClient for UnixInjectionClient {
    fn start(
        &mut self,
        request: Value,
        sink: OwnedFd,
    ) -> Result<Box<dyn PendingInjection>, InjectionError> {
        self.check_socket()?;
        let stream = UnixStream::connect(&self.socket).map_err(|_| InjectionError::SinkFailed)?;
        start_on_stream(stream, request, sink)
    }
}

/// ADR 2026-10-09 付記（launcher の Authenticate 経路）2: an `injection.sock` connection the
/// daemon opened (so credentiald sees the registered controller as the peer) and handed to the
/// launcher with `SCM_RIGHTS`. It carries exactly one injection; the launcher never learns the
/// broker's path and never talks to its control socket.
pub struct PassedInjectionStream(Option<UnixStream>);

impl PassedInjectionStream {
    pub fn new(stream: UnixStream) -> Self {
        Self(Some(stream))
    }
}

impl BrokerClient for PassedInjectionStream {
    fn start(
        &mut self,
        request: Value,
        sink: OwnedFd,
    ) -> Result<Box<dyn PendingInjection>, InjectionError> {
        let stream = self.0.take().ok_or(InjectionError::SinkFailed)?;
        start_on_stream(stream, request, sink)
    }
}

/// Send one injection request with its sink FD on a connected `injection.sock` stream.
fn start_on_stream(
    stream: UnixStream,
    request: Value,
    sink: OwnedFd,
) -> Result<Box<dyn PendingInjection>, InjectionError> {
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|_| InjectionError::SinkFailed)?;
    stream
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|_| InjectionError::SinkFailed)?;
    let mut body = serde_json::to_vec(&request).map_err(|_| InjectionError::SinkFailed)?;
    if body.len() > 16 * 1024 {
        body.zeroize();
        return Err(InjectionError::SinkFailed);
    }
    let mut header = (body.len() as u32).to_be_bytes();
    let mut iov = [
        nix::libc::iovec {
            iov_base: header.as_mut_ptr().cast(),
            iov_len: header.len(),
        },
        nix::libc::iovec {
            iov_base: body.as_mut_ptr().cast(),
            iov_len: body.len(),
        },
    ];
    let mut control = [0u64; 4];
    // SAFETY: msghdr and aligned CMSG buffer are valid for one SCM_RIGHTS fd.
    let sent = unsafe {
        let mut msg: nix::libc::msghdr = std::mem::zeroed();
        msg.msg_iov = iov.as_mut_ptr();
        msg.msg_iovlen = iov.len();
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = nix::libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as _;
        let cmsg = nix::libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = nix::libc::SOL_SOCKET;
        (*cmsg).cmsg_type = nix::libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = nix::libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as _;
        std::ptr::write_unaligned(nix::libc::CMSG_DATA(cmsg).cast::<i32>(), sink.as_raw_fd());
        nix::libc::sendmsg(stream.as_raw_fd(), &msg, nix::libc::MSG_NOSIGNAL)
    };
    body.zeroize();
    if sent != (header.len() + iov[1].iov_len) as isize {
        return Err(InjectionError::SinkFailed);
    }
    Ok(Box::new(UnixPending(stream)))
}
