//! ADR-0109 D4: controller owned CDP pipe and one way broker sink.
//!
//! The controller never receives a credential in an IPC response. The broker
//! writes one CDP command to a private seqpacket socket; its bytes are forwarded
//! to the browser without decoding or logging them. Only the CDP reply goes back
//! to the broker. The caller gets a receipt with a fixed, non-secret shape.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use celeris_credentiald::injection::RedisplayGuard;
use celeris_credentiald::injection_ipc::{
    AuthSectionRegistration, Field, InjectionReply, InjectionRequest as WireRequest,
    LiveSessionRegistration,
};
use celeris_credentiald::ipc;
use serde_json::{Value, json};
use url::Url;
use zeroize::Zeroize;

use crate::browser_relay;

const MAX_FRAME: usize = 65536;
const TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on events no agent connection has taken yet (oldest dropped first).
const MAX_QUEUED_EVENTS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectionError {
    AuthSectionRequired,
    TargetMismatch,
    CrossOriginFrame,
    Redirected,
    RedisplayField,
    TargetChanged,
    SinkFailed,
    /// ADR-0111: an agent observation re-displayed an injected value; it was discarded.
    RedisplayDetected,
    BrokerRejected(&'static str),
}

impl InjectionError {
    pub fn code(self) -> &'static str {
        match self {
            Self::AuthSectionRequired => "auth_section_required",
            Self::TargetMismatch => "target_mismatch",
            Self::CrossOriginFrame => "cross_origin_frame",
            Self::Redirected => "redirected",
            Self::RedisplayField => "redisplay_field",
            Self::TargetChanged => "target_changed",
            Self::SinkFailed => "sink_failed",
            Self::RedisplayDetected => "redisplay_detected",
            Self::BrokerRejected(code) => code,
        }
    }
}

/// Controller-side target intent. This is resolved against live CDP state;
/// the serialized IPC request uses credentiald's shared `WireRequest` type.
#[derive(Debug, Clone)]
pub struct InjectionRequest {
    pub request_id: String,
    pub session_id: String,
    pub cdp_target_id: String,
    pub frame_id: String,
    pub loader_id: String,
    pub exact_origin: String,
    pub redirect_chain: Vec<String>,
    pub selector: String,
    pub field: String,
    pub auth_section_id: String,
    pub lease_id: String,
}

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
}

pub struct CdpController {
    write: File,
    read: File,
    buffered: Vec<u8>,
    next_id: u64,
    auth_section: Option<String>,
    /// ADR-0080 H3 / ADR-0101 D4: identity の復元を受けた session。session（= controller）の
    /// 終わりまで agent の観測を止める。解除する口は無い。
    restored: bool,
    injected: Vec<(String, String, String, String)>,
    events: Vec<Value>,
    login_navigation: Option<LoginNavigation>,
    /// ADR-0111: one guard per injected value, kept for the controller's (= session's) lifetime.
    guards: Vec<RedisplayGuard>,
    /// Test-only (ADR-0109 A1): navigate the page after all checks and before the
    /// broker's `Runtime.callFunctionOn` frame is written. Absent from production builds.
    #[cfg(feature = "attack-test-hooks")]
    retarget_before_sink: Option<String>,
    #[cfg(feature = "attack-test-hooks")]
    response_timeout: Duration,
}

/// Non-secret, bounded history for the trusted login top document only.
struct LoginNavigation {
    session: String,
    frame: Option<String>,
    origins: Vec<String>,
    redirects: usize,
    documents: usize,
    invalid: bool,
    deadline: std::time::Instant,
    waiting: bool,
}

impl CdpController {
    pub fn new(write: File, read: File) -> Self {
        Self {
            write,
            read,
            buffered: Vec::new(),
            next_id: 1,
            auth_section: None,
            restored: false,
            injected: Vec::new(),
            events: Vec::new(),
            login_navigation: None,
            guards: Vec::new(),
            #[cfg(feature = "attack-test-hooks")]
            retarget_before_sink: None,
            #[cfg(feature = "attack-test-hooks")]
            response_timeout: TIMEOUT,
        }
    }

    /// Test-only bound for a CDP response while a real browser is being started.
    #[cfg(feature = "attack-test-hooks")]
    pub fn response_timeout_for_test(&mut self, timeout: Duration) {
        self.response_timeout = timeout;
    }

    /// Test-only (ADR-0109 A1): the next injection navigates its page session to `url`
    /// after the controller's checks and before the broker's command reaches CDP.
    #[cfg(feature = "attack-test-hooks")]
    pub fn retarget_before_sink_for_test(&mut self, url: String) {
        self.retarget_before_sink = Some(url);
    }

    #[cfg(feature = "attack-test-hooks")]
    fn retarget_for_test(&mut self, url: &str, session: &str) -> Result<(), InjectionError> {
        self.call("Page.navigate", json!({"url":url}), Some(session))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            let tree = self.call("Page.getFrameTree", json!({}), Some(session))?;
            let ready = self.call(
                "Runtime.evaluate",
                json!({"expression":"document.readyState","returnByValue":true}),
                Some(session),
            );
            if tree["result"]["frameTree"]["frame"]["url"] == url
                && ready.is_ok_and(|r| r["result"]["result"]["value"] == "complete")
            {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Err(InjectionError::SinkFailed)
    }

    pub fn open_auth_section(&mut self, id: String) {
        self.events.clear();
        self.login_navigation = None;
        self.auth_section = Some(id);
    }

    /// Start before Page.navigate; history survives same-origin JS relay documents.
    pub fn begin_login_navigation(
        &mut self,
        session: &str,
        timeout: Duration,
    ) -> Result<(), InjectionError> {
        let tree = self.call("Page.getFrameTree", json!({}), Some(session))?;
        let frame = tree["result"]["frameTree"]["frame"]["id"]
            .as_str()
            .ok_or(InjectionError::TargetMismatch)?
            .to_owned();
        self.login_navigation = Some(LoginNavigation {
            session: session.into(),
            frame: Some(frame),
            origins: Vec::new(),
            redirects: 0,
            documents: 0,
            invalid: false,
            deadline: std::time::Instant::now() + timeout,
            waiting: true,
        });
        Ok(())
    }

    pub fn login_deadline(&self) -> Option<std::time::Instant> {
        self.login_navigation.as_ref().map(|nav| nav.deadline)
    }

    pub fn login_redirect_chain(&self, exact_origin: &str) -> Result<Vec<String>, InjectionError> {
        let nav = self
            .login_navigation
            .as_ref()
            .ok_or(InjectionError::AuthSectionRequired)?;
        if nav.invalid || nav.origins.iter().any(|o| o != exact_origin) {
            return Err(InjectionError::Redirected);
        }
        Ok(nav.origins.clone())
    }

    /// Only readiness is returned; no input value or page text crosses H3.
    pub fn login_password_document(
        &mut self,
        session: &str,
        exact_origin: &str,
        selector: &str,
    ) -> Result<Option<(String, String)>, InjectionError> {
        let tree = self.call("Page.getFrameTree", json!({}), Some(session))?;
        self.login_redirect_chain(exact_origin)?;
        let frame = &tree["result"]["frameTree"]["frame"];
        let url = frame["url"].as_str();
        if url == Some("about:blank") {
            return Ok(None);
        }
        if origin(url).as_deref() != Some(exact_origin) {
            return Err(InjectionError::Redirected);
        }
        let (Some(id), Some(loader)) = (frame["id"].as_str(), frame["loaderId"].as_str()) else {
            return Ok(None);
        };
        let world = match self.call(
            "Page.createIsolatedWorld",
            json!({"frameId":id,"worldName":"celeris-credential"}),
            Some(session),
        ) {
            Ok(world) => world,
            Err(InjectionError::TargetChanged) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(context) = world["result"]["executionContextId"].as_i64() else {
            return Ok(None);
        };
        let expr = format!(
            "(()=>{{const es=document.querySelectorAll({});return es.length===1 && es[0] instanceof HTMLInputElement && es[0].type==='password';}})()",
            serde_json::to_string(selector).map_err(|_| InjectionError::TargetMismatch)?
        );
        let ready = self.call(
            "Runtime.evaluate",
            json!({"expression":expr,"contextId":context,"returnByValue":true,"silent":true}),
            Some(session),
        );
        // Even a destroyed context can carry navigation events: inspect the history first.
        self.login_redirect_chain(exact_origin)?;
        if !ready.is_ok_and(|r| r["result"]["result"]["value"] == true) {
            return Ok(None);
        }
        let after = self.call("Page.getFrameTree", json!({}), Some(session))?;
        self.login_redirect_chain(exact_origin)?;
        let after = &after["result"]["frameTree"]["frame"];
        if origin(after["url"].as_str()).as_deref() != Some(exact_origin) {
            return Err(InjectionError::Redirected);
        }
        if after["id"] != id || after["loaderId"] != loader {
            return Ok(None);
        }
        if let Some(nav) = self.login_navigation.as_mut() {
            nav.waiting = false;
        }
        Ok(Some((id.into(), loader.into())))
    }

    /// Remove injected values while the agent relay remains blocked.
    pub fn clear_injected_values(&mut self) -> Result<(), InjectionError> {
        // Clear on the same document before observations become available.
        for (object_id, session, frame_id, loader_id) in self.injected.clone() {
            let tree = self.call("Page.getFrameTree", json!({}), Some(&session))?;
            let mut chain = Vec::new();
            if find_frame(&tree["result"]["frameTree"], &frame_id, &mut chain)
                && chain.last().is_some_and(|f| f["loaderId"] == loader_id)
            {
                let cleared = self.call("Runtime.callFunctionOn", json!({"objectId":object_id,"functionDeclaration":"function(){if(this.isConnected){Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(this,'');this.dispatchEvent(new Event('input',{bubbles:true}));}return 'ok';}","returnByValue":true,"silent":true}), Some(&session))?;
                if cleared["result"]["result"]["value"] != "ok" {
                    return Err(InjectionError::TargetChanged);
                }
            }
        }
        self.injected.clear();
        Ok(())
    }

    /// identity 復元の state を投入する前に呼ぶ。以後この controller が生きている間、
    /// agent 由来の観測は拒否され、CDP event（console を含む）は捨てられる。
    pub fn enter_restored_observation_stop(&mut self) {
        self.events.clear();
        self.restored = true;
    }

    /// 認証区間中か、復元を受けた session か（どちらも agent の観測を止める）。
    pub fn observation_stopped(&self) -> bool {
        self.auth_section.is_some() || self.restored
    }

    pub fn close_auth_section(&mut self) -> Result<(), InjectionError> {
        self.clear_injected_values()?;
        // Drain frames produced during the stopped interval while suppression is
        // still active, so the relay cannot forward them after the section closes.
        self.pump_events()?;
        self.auth_section = None;
        self.login_navigation = None;
        Ok(())
    }

    /// Agent-originating observation commands are refused throughout H3. CDP
    /// events (including console output) are consumed internally and discarded.
    pub fn agent_command(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, InjectionError> {
        if self.observation_stopped() {
            return Err(InjectionError::AuthSectionRequired);
        }
        let reply = self.call(method, params, session)?;
        if self.redisplayed(&reply) {
            // The whole observation is dropped; only the fixed reason crosses.
            drop(reply);
            return Err(InjectionError::RedisplayDetected);
        }
        Ok(reply)
    }

    /// ADR-0111: whether an observation carries an injected value in any
    /// representation the guard decodes (raw, percent, UTF-16LE, base64, JSON escape).
    fn redisplayed(&self, observation: &Value) -> bool {
        self.guards.iter().any(|g| g.exposes_json(observation))
    }

    /// Number of redisplay guards received from the broker for this session.
    pub fn redisplay_guards(&self) -> usize {
        self.guards.len()
    }

    /// Trusted controller operations needed to prepare and finish H3. The
    /// caller must hold the controller handle, never an agent connection.
    pub fn controller_command(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, InjectionError> {
        self.call(method, params, session)
    }

    /// Events collected while a command was in flight. An auth section discards
    /// them before the relay can expose them to an agent connection.
    pub fn take_agent_events(&mut self) -> Vec<Value> {
        if self.observation_stopped() {
            self.events.clear();
            Vec::new()
        } else {
            let mut events = std::mem::take(&mut self.events);
            events.retain(|e| !self.redisplayed(e));
            events
        }
    }

    /// Like [`Self::take_agent_events`], but only the events of `sessions`; events of
    /// other connections' sessions stay queued for them (F1: idle pumping per connection).
    pub fn take_agent_events_for(
        &mut self,
        sessions: &std::collections::HashSet<String>,
    ) -> Vec<Value> {
        if self.observation_stopped() {
            self.events.clear();
            return Vec::new();
        }
        let (mine, rest): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.events)
            .into_iter()
            .partition(|e| e["sessionId"].as_str().is_none_or(|s| sessions.contains(s)));
        self.events = rest;
        mine.into_iter().filter(|e| !self.redisplayed(e)).collect()
    }

    pub fn auth_section_active(&self) -> bool {
        self.auth_section.is_some()
    }

    pub fn inject(
        &mut self,
        request: &InjectionRequest,
        cdp_session: &str,
        broker: &mut dyn BrokerClient,
    ) -> Result<Value, InjectionError> {
        if self.auth_section.as_deref() != Some(request.auth_section_id.as_str()) {
            return Err(InjectionError::AuthSectionRequired);
        }
        let target = self.call(
            "Target.getTargetInfo",
            json!({"targetId":request.cdp_target_id}),
            None,
        )?;
        let info = &target["result"]["targetInfo"];
        if info["targetId"] != request.cdp_target_id || info["type"] != "page" {
            return Err(InjectionError::TargetMismatch);
        }
        if origin(info["url"].as_str()) != Some(request.exact_origin.clone()) {
            return Err(InjectionError::TargetMismatch);
        }
        let tree = self.call("Page.getFrameTree", json!({}), Some(cdp_session))?;
        let root = &tree["result"]["frameTree"];
        let mut chain = Vec::new();
        if !find_frame(root, &request.frame_id, &mut chain) {
            return Err(InjectionError::TargetMismatch);
        }
        if chain
            .iter()
            .any(|frame| origin(frame["url"].as_str()) != Some(request.exact_origin.clone()))
        {
            return Err(InjectionError::CrossOriginFrame);
        }
        let frame = chain.last().ok_or(InjectionError::TargetMismatch)?;
        if frame["loaderId"] != request.loader_id {
            return Err(InjectionError::TargetChanged);
        }
        if request
            .redirect_chain
            .iter()
            .any(|o| o != &request.exact_origin)
        {
            return Err(InjectionError::Redirected);
        }
        // The DOM domain addresses the top document for this page session.
        // A child frame needs its own CDP session to resolve nodes safely.
        if chain.len() != 1 {
            return Err(InjectionError::TargetMismatch);
        }
        let world = self.call(
            "Page.createIsolatedWorld",
            json!({"frameId":request.frame_id,"worldName":"celeris-credential"}),
            Some(cdp_session),
        )?;
        let context = world["result"]["executionContextId"]
            .as_i64()
            .ok_or(InjectionError::TargetChanged)?;
        let document = self.call("DOM.getDocument", json!({}), Some(cdp_session))?;
        let root_id = document["result"]["root"]["nodeId"]
            .as_i64()
            .ok_or(InjectionError::TargetChanged)?;
        let queried = self.call(
            "DOM.querySelectorAll",
            json!({"nodeId":root_id,"selector":request.selector}),
            Some(cdp_session),
        )?;
        let nodes = queried["result"]["nodeIds"]
            .as_array()
            .ok_or(InjectionError::TargetMismatch)?;
        if nodes.len() != 1 {
            return Err(InjectionError::TargetMismatch);
        }
        let node_id = nodes[0]
            .as_i64()
            .filter(|id| *id > 0)
            .ok_or(InjectionError::TargetMismatch)?;
        let resolved = self.call(
            "DOM.resolveNode",
            json!({"nodeId":node_id,"executionContextId":context}),
            Some(cdp_session),
        )?;
        let object_id = resolved["result"]["object"]["objectId"]
            .as_str()
            .ok_or(InjectionError::TargetChanged)?;
        let check = self.call("Runtime.callFunctionOn", json!({"objectId":object_id,"functionDeclaration":"function(){return this instanceof HTMLInputElement ? this.type.toLowerCase() : '';}","returnByValue":true,"silent":true}), Some(cdp_session))?;
        let input_type = check["result"]["result"]["value"]
            .as_str()
            .ok_or(InjectionError::TargetChanged)?;
        let valid = match request.field.as_str() {
            "password" => input_type == "password",
            "username" => matches!(input_type, "text" | "email"),
            _ => false,
        };
        if !valid {
            return Err(InjectionError::RedisplayField);
        }
        let command_id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(InjectionError::SinkFailed)?;
        let (controller_fd, broker_fd) =
            browser_relay::seqpacket_pair().map_err(|_| InjectionError::SinkFailed)?;
        let field = match request.field.as_str() {
            "password" => Field::Password,
            "username" => Field::Username,
            _ => return Err(InjectionError::RedisplayField),
        };
        let redirect_chain = if self.login_navigation.is_some() {
            let chain = self.login_redirect_chain(&request.exact_origin)?;
            if chain.is_empty() {
                return Err(InjectionError::Redirected);
            }
            chain
        } else {
            request.redirect_chain.clone()
        };
        let wire = serde_json::to_value(WireRequest {
            v: 1,
            request_id: request.request_id.clone(),
            session_id: request.session_id.clone(),
            cdp_target_id: request.cdp_target_id.clone(),
            cdp_session_id: Some(cdp_session.to_owned()),
            frame_id: request.frame_id.clone(),
            loader_id: request.loader_id.clone(),
            frame_chain: chain
                .iter()
                .filter_map(|f| origin(f["url"].as_str()))
                .collect(),
            redirect_chain,
            selector: request.selector.clone(),
            object_id: object_id.to_owned(),
            field,
            input_type: input_type.to_owned(),
            auth_section_id: request.auth_section_id.clone(),
            lease_id: request.lease_id.clone(),
            cdp_command_id: command_id,
        })
        .map_err(|_| InjectionError::SinkFailed)?;
        let pending = broker.start(wire, broker_fd)?;
        let mut command = match recv_packet(&controller_fd) {
            Ok(command) => command,
            Err(_) => {
                let reply = pending.finish()?;
                return Err(broker_denial(&reply));
            }
        };
        if command.last() != Some(&0) || command.len() > MAX_FRAME {
            command.zeroize();
            return Err(InjectionError::SinkFailed);
        }
        #[cfg(feature = "attack-test-hooks")]
        if let Some(url) = self.retarget_before_sink.take()
            && let Err(e) = self.retarget_for_test(&url, cdp_session)
        {
            command.zeroize();
            return Err(e);
        }
        self.write
            .write_all(&command)
            .map_err(|_| InjectionError::SinkFailed)?;
        command.zeroize();
        let reply = self.read_response(command_id)?;
        let status = if reply["error"].is_null() && reply["result"]["result"]["value"] == "ok" {
            Ok(())
        } else {
            Err(InjectionError::TargetChanged)
        };
        let mut raw = serde_json::to_vec(&reply).map_err(|_| InjectionError::SinkFailed)?;
        send_packet(&controller_fd, &raw)?;
        raw.zeroize();
        if status.is_err() {
            // The broker has already consumed the lease; report its own verdict.
            let receipt = pending.finish()?;
            return Err(if receipt["ok"] == false {
                broker_denial(&receipt)
            } else {
                InjectionError::TargetChanged
            });
        }
        let receipt = pending.finish()?;
        let typed: InjectionReply =
            serde_json::from_value(receipt.clone()).map_err(|_| InjectionError::SinkFailed)?;
        if typed.v != 1 {
            return Err(InjectionError::SinkFailed);
        }
        if !typed.ok {
            return Err(broker_denial(&receipt));
        }
        if typed.code.is_some()
            || typed.receipt.is_none()
            || receipt["ok"] != true
            || receipt["request_id"] != request.request_id
            || receipt["receipt"]["lease_id"] != request.lease_id
            || receipt["receipt"]["auth_section_id"] != request.auth_section_id
            || receipt["receipt"]["session_id"] != request.session_id
            || receipt["receipt"]["cdp_target_id"] != request.cdp_target_id
            || receipt["receipt"]["frame_id"] != request.frame_id
            || receipt["receipt"]["loader_id"] != request.loader_id
            || receipt["receipt"]["field"] != request.field
        {
            return Err(InjectionError::SinkFailed);
        }
        let injected_at = receipt["receipt"]["injected_at"]
            .as_u64()
            .ok_or(InjectionError::SinkFailed)?;
        // A receipt without a well-formed guard is a failed injection (fail closed).
        let guard = typed
            .redisplay_guard
            .as_ref()
            .and_then(RedisplayGuard::from_wire)
            .ok_or(InjectionError::SinkFailed)?;
        self.guards.push(guard);
        self.login_navigation = None;
        self.injected.push((
            object_id.to_owned(),
            cdp_session.to_owned(),
            request.frame_id.clone(),
            request.loader_id.clone(),
        ));
        Ok(
            json!({"v":1,"request_id":request.request_id,"ok":true,"receipt":{
                "lease_id":request.lease_id,"auth_section_id":request.auth_section_id,
                "session_id":request.session_id,"cdp_target_id":request.cdp_target_id,
                "frame_id":request.frame_id,"loader_id":request.loader_id,
                "field":request.field,"injected_at":injected_at
            }}),
        )
    }

    fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, InjectionError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(InjectionError::SinkFailed)?;
        let mut message = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            message["sessionId"] = session.into();
        }
        let mut bytes = serde_json::to_vec(&message).map_err(|_| InjectionError::SinkFailed)?;
        bytes.push(0);
        self.write
            .write_all(&bytes)
            .map_err(|_| InjectionError::SinkFailed)?;
        bytes.zeroize();
        let reply = self.read_response(id)?;
        if !reply["error"].is_null() {
            return Err(InjectionError::TargetChanged);
        }
        Ok(reply)
    }

    fn queue_event(&mut self, value: Value) {
        if value.get("method").is_none() {
            return;
        }
        if self.auth_section.is_some()
            && value["method"] == "Network.requestWillBeSent"
            && let Some(nav) = self.login_navigation.as_mut()
            && value["sessionId"] == nav.session
            && value["params"]["type"] == "Document"
        {
            let params = &value["params"];
            if let Some(frame) = params["frameId"].as_str() {
                if nav.frame.as_deref() == Some(frame) {
                    nav.documents += 1;
                    if params["redirectResponse"].is_object() {
                        nav.redirects += 1;
                    }
                    if nav.redirects > 32 || nav.documents > 32 {
                        nav.invalid = true;
                    } else {
                        if params["redirectResponse"].is_object() {
                            match origin(params["redirectResponse"]["url"].as_str()) {
                                Some(o) => {
                                    if nav.origins.last() != Some(&o) {
                                        nav.origins.push(o);
                                    }
                                }
                                None => nav.invalid = true,
                            }
                        }
                        match origin(params["request"]["url"].as_str()) {
                            Some(o) => nav.origins.push(o),
                            None => nav.invalid = true,
                        }
                        if nav.origins.len() > 32 {
                            nav.origins.truncate(32);
                            nav.invalid = true;
                        }
                    }
                }
            } else {
                nav.invalid = true;
            }
        }
        if !self.observation_stopped() {
            if self.events.len() >= MAX_QUEUED_EVENTS {
                self.events.remove(0);
            }
            self.events.push(value);
        }
    }

    /// F1: queue CDP events the browser emitted while no command was in flight
    /// (e.g. `Page.loadEventFired` after the `Page.navigate` reply). Never waits.
    pub fn pump_events(&mut self) -> Result<(), InjectionError> {
        loop {
            while let Some(end) = self.buffered.iter().position(|b| *b == 0) {
                let mut frame: Vec<u8> = self.buffered.drain(..=end).collect();
                let parsed = serde_json::from_slice::<Value>(&frame[..end]);
                frame.zeroize();
                // A reply nobody waits for (all calls hold the lock until answered) is dropped.
                if let Ok(value) = parsed {
                    self.queue_event(value);
                }
            }
            if self.buffered.len() >= MAX_FRAME {
                return Err(InjectionError::SinkFailed);
            }
            let mut pollfd = nix::libc::pollfd {
                fd: self.read.as_raw_fd(),
                events: nix::libc::POLLIN,
                revents: 0,
            };
            // SAFETY: pollfd points to one valid descriptor and structure.
            if unsafe { nix::libc::poll(&mut pollfd, 1, 0) } <= 0 {
                return Ok(());
            }
            let mut chunk = [0u8; 4096];
            let n = self
                .read
                .read(&mut chunk)
                .map_err(|_| InjectionError::SinkFailed)?;
            if n == 0 {
                return Err(InjectionError::SinkFailed);
            }
            self.buffered.extend_from_slice(&chunk[..n]);
            chunk.zeroize();
        }
    }

    fn read_response(&mut self, id: u64) -> Result<Value, InjectionError> {
        #[cfg(feature = "attack-test-hooks")]
        let timeout = self.response_timeout;
        #[cfg(not(feature = "attack-test-hooks"))]
        let timeout = TIMEOUT;
        let deadline = std::time::Instant::now() + timeout;
        let deadline = self
            .login_navigation
            .as_ref()
            .filter(|nav| nav.waiting)
            .map_or(deadline, |nav| deadline.min(nav.deadline));
        loop {
            if std::time::Instant::now() >= deadline {
                return Err(InjectionError::SinkFailed);
            }
            if let Some(end) = self.buffered.iter().position(|b| *b == 0) {
                let mut frame: Vec<u8> = self.buffered.drain(..=end).collect();
                let parsed = serde_json::from_slice::<Value>(&frame[..end])
                    .map_err(|_| InjectionError::SinkFailed);
                frame.zeroize();
                let value = parsed?;
                if value["id"] == id {
                    return Ok(value);
                }
                self.queue_event(value);
                continue;
            }
            if self.buffered.len() >= MAX_FRAME {
                return Err(InjectionError::SinkFailed);
            }
            let mut pollfd = nix::libc::pollfd {
                fd: self.read.as_raw_fd(),
                events: nix::libc::POLLIN,
                revents: 0,
            };
            // SAFETY: pollfd points to one valid descriptor and structure.
            if unsafe {
                nix::libc::poll(
                    &mut pollfd,
                    1,
                    timeout
                        .min(deadline.saturating_duration_since(std::time::Instant::now()))
                        .as_millis() as i32,
                )
            } <= 0
            {
                return Err(InjectionError::SinkFailed);
            }
            let mut chunk = [0u8; 4096];
            let n = self
                .read
                .read(&mut chunk)
                .map_err(|_| InjectionError::SinkFailed)?;
            if n == 0 {
                return Err(InjectionError::SinkFailed);
            }
            self.buffered.extend_from_slice(&chunk[..n]);
            chunk.zeroize();
        }
    }
}

fn broker_denial(reply: &Value) -> InjectionError {
    // Only the broker's fixed ADR-0109 vocabulary may cross this boundary.
    let code = match reply["code"].as_str() {
        Some("invalid_request") => "invalid_request",
        Some("unsupported_version") => "unsupported_version",
        Some("peer_uid_mismatch") => "peer_uid_mismatch",
        Some("injection_worker_not_allowed") => "injection_worker_not_allowed",
        Some("session_not_live") => "session_not_live",
        Some("isolation_required") => "isolation_required",
        Some("target_mismatch") => "target_mismatch",
        Some("empty_frame_chain") => "empty_frame_chain",
        Some("cross_origin_frame") => "cross_origin_frame",
        Some("redirected") => "redirected",
        Some("redisplay_field") => "redisplay_field",
        Some("auth_section_required") => "auth_section_required",
        Some("auth_section_mismatch") => "auth_section_mismatch",
        Some("selector_mismatch") => "selector_mismatch",
        Some("lease_expired") => "lease_expired",
        Some("lease_used") => "lease_used",
        Some("other_session") => "other_session",
        Some("lease_invalid") => "lease_invalid",
        Some("audit_unavailable") => "audit_unavailable",
        Some("provider_failed") => "provider_failed",
        Some("target_changed") => "target_changed",
        Some("sink_failed") => "sink_failed",
        _ => return InjectionError::SinkFailed,
    };
    InjectionError::BrokerRejected(code)
}

fn origin(url: Option<&str>) -> Option<String> {
    let parsed = Url::parse(url?).ok()?;
    let host = parsed.host_str()?;
    let port = parsed.port();
    let scheme = parsed.scheme();
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    Some(match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

fn find_frame<'a>(node: &'a Value, id: &str, chain: &mut Vec<&'a Value>) -> bool {
    chain.push(&node["frame"]);
    if node["frame"]["id"] == id {
        return true;
    }
    if let Some(children) = node["childFrames"].as_array() {
        for child in children {
            if find_frame(child, id, chain) {
                return true;
            }
        }
    }
    chain.pop();
    false
}

fn recv_packet(fd: &OwnedFd) -> Result<Vec<u8>, InjectionError> {
    let mut pollfd = nix::libc::pollfd {
        fd: fd.as_raw_fd(),
        events: nix::libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pollfd points to one valid descriptor and structure.
    if unsafe { nix::libc::poll(&mut pollfd, 1, TIMEOUT.as_millis() as i32) } <= 0 {
        return Err(InjectionError::SinkFailed);
    }
    let mut buf = vec![0u8; MAX_FRAME];
    // SAFETY: buffer is writable and fd is a connected seqpacket socket.
    let n = unsafe {
        nix::libc::recv(
            fd.as_raw_fd(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            nix::libc::MSG_TRUNC,
        )
    };
    if n <= 0 || n as usize > buf.len() {
        buf.zeroize();
        return Err(InjectionError::SinkFailed);
    }
    buf.truncate(n as usize);
    Ok(buf)
}

fn send_packet(fd: &OwnedFd, bytes: &[u8]) -> Result<(), InjectionError> {
    // SAFETY: bytes is a valid buffer and fd is a connected seqpacket socket.
    let n = unsafe {
        nix::libc::send(
            fd.as_raw_fd(),
            bytes.as_ptr().cast(),
            bytes.len(),
            nix::libc::MSG_NOSIGNAL,
        )
    };
    if n != bytes.len() as isize {
        return Err(InjectionError::SinkFailed);
    }
    Ok(())
}

#[cfg(test)]
mod idle_pump_tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    fn controller() -> (CdpController, UnixStream) {
        let (controller_end, browser) = UnixStream::pair().expect("socket pair");
        let read = File::from(OwnedFd::from(controller_end.try_clone().expect("clone")));
        let write = File::from(OwnedFd::from(controller_end));
        (CdpController::new(write, read), browser)
    }

    fn login_controller() -> (CdpController, UnixStream) {
        let (mut c, stream) = controller();
        c.open_auth_section("auth".into());
        c.login_navigation = Some(LoginNavigation {
            session: "S".into(),
            frame: Some("F".into()),
            origins: Vec::new(),
            redirects: 0,
            documents: 0,
            invalid: false,
            deadline: std::time::Instant::now() + Duration::from_secs(15),
            waiting: true,
        });
        (c, stream)
    }

    fn document(url: &str) -> Value {
        json!({"method":"Network.requestWillBeSent","sessionId":"S",
            "params":{"type":"Document","frameId":"F","request":{"url":url}}})
    }

    #[test]
    fn browser_trusted_login_sso_origin_history_is_session_and_top_document_scoped() {
        let (mut c, _stream) = login_controller();
        let mut other = document("https://other.test/secret?SAMLRequest=opaque");
        other["sessionId"] = "OTHER".into();
        c.queue_event(other);
        let mut subresource = document("https://other.test/script");
        subresource["params"]["type"] = "Script".into();
        c.queue_event(subresource);
        let mut iframe = document("https://other.test/frame");
        iframe["params"]["frameId"] = "CHILD".into();
        c.queue_event(iframe);
        c.queue_event(document("https://idp.test/entry?SAMLRequest=opaque"));
        let mut redirect = document("https://idp.test/entry?execution=e1s1");
        redirect["params"]["redirectResponse"] =
            json!({"url":"https://idp.test/entry?SAMLRequest=opaque"});
        c.queue_event(redirect);
        c.queue_event(document("https://idp.test/form?execution=e1s2"));
        assert_eq!(
            c.login_redirect_chain("https://idp.test").unwrap(),
            vec!["https://idp.test"; 3]
        );
        assert!(c.take_agent_events().is_empty());
    }

    #[test]
    fn browser_trusted_login_sso_cross_origin_source_and_js_history_stay_rejected() {
        let (mut c, _stream) = login_controller();
        let mut redirect = document("https://idp.test/form");
        redirect["params"]["redirectResponse"] = json!({"url":"https://other.test/entry"});
        c.queue_event(redirect);
        assert_eq!(
            c.login_redirect_chain("https://idp.test"),
            Err(InjectionError::Redirected)
        );
        let (mut c, _stream) = login_controller();
        c.queue_event(document("https://idp.test/relay"));
        c.queue_event(document("https://other.test/relay"));
        c.queue_event(document("https://idp.test/form"));
        assert_eq!(
            c.login_redirect_chain("https://idp.test"),
            Err(InjectionError::Redirected)
        );
    }

    #[test]
    fn browser_trusted_login_sso_history_limit_and_invalid_origin_fail_closed() {
        let (mut c, _stream) = login_controller();
        for _ in 0..32 {
            c.queue_event(document("https://idp.test/loop"));
        }
        assert_eq!(
            c.login_redirect_chain("https://idp.test").unwrap().len(),
            32
        );
        for _ in 0..100 {
            c.queue_event(document("https://idp.test/loop"));
        }
        assert_eq!(
            c.login_redirect_chain("https://idp.test"),
            Err(InjectionError::Redirected)
        );
        assert_eq!(c.login_navigation.as_ref().unwrap().origins.len(), 32);
        let (mut c, _stream) = login_controller();
        c.queue_event(document("not-a-url"));
        assert_eq!(
            c.login_redirect_chain("https://idp.test"),
            Err(InjectionError::Redirected)
        );
    }

    #[test]
    fn event_queue_drops_oldest_after_4096_entries() {
        let (mut controller, _browser) = controller();
        for index in 0..=MAX_QUEUED_EVENTS {
            controller.queue_event(
                json!({"method":"Page.loadEventFired", "sessionId":"S", "params":{"index":index}}),
            );
        }
        assert_eq!(controller.events.len(), MAX_QUEUED_EVENTS);
        let sessions = ["S".to_owned()].into_iter().collect();
        let events = controller.take_agent_events_for(&sessions);
        assert_eq!(events.len(), MAX_QUEUED_EVENTS);
        assert_eq!(events[0]["params"]["index"], 1);
        assert_eq!(
            events.last().expect("last event")["params"]["index"],
            MAX_QUEUED_EVENTS
        );
    }

    #[test]
    fn taking_one_session_preserves_another_sessions_events() {
        let (mut controller, _browser) = controller();
        controller.queue_event(json!({"method":"Page.loadEventFired","sessionId":"OTHER"}));
        controller.queue_event(json!({"method":"Page.loadEventFired","sessionId":"S"}));
        let mine = ["S".to_owned()].into_iter().collect();
        let other = ["OTHER".to_owned()].into_iter().collect();
        assert_eq!(controller.take_agent_events_for(&mine).len(), 1);
        assert_eq!(controller.take_agent_events_for(&other).len(), 1);
    }

    #[test]
    fn auth_section_discards_pending_events_before_relay_resumes() {
        let (mut controller, mut browser) = controller();
        let sessions = ["S".to_owned()].into_iter().collect();
        controller.open_auth_section("auth".into());
        browser
            .write_all(b"{\"method\":\"Page.loadEventFired\",\"sessionId\":\"S\"}\0")
            .expect("auth event");
        controller.close_auth_section().expect("close auth");
        assert!(controller.take_agent_events_for(&sessions).is_empty());
        browser
            .write_all(b"{\"method\":\"Page.loadEventFired\",\"sessionId\":\"S\"}\0")
            .expect("normal event");
        controller.pump_events().expect("pump after auth");
        assert_eq!(controller.take_agent_events_for(&sessions).len(), 1);
    }
}
