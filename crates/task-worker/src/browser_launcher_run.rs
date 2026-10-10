//! ADR-0116 D5: launcher 経由の isolated browser runtime（daemon 側）。
//!
//! `[browser] runtime = "launcher"` のとき、daemon は bwrap / sandboxd / Chrome も CDP pipe も
//! 持たない。[`LauncherRuntime`] が [`crate::browser_launcher::LauncherClient`] で session の
//! start / action / observe / stop を頼み、受けるのは receipt と非機密の観測だけ。観測から
//! [`RuntimeFacts`] を組んで `verify_isolation` に通し、通らなければ session を止めて
//! `isolated_runtime_unavailable` にする。どの失敗も daemon 所有経路へ fallback しない。
//!
//! CredentialUse は launcher session proof と isolation attestation が成立した場合だけ許可する。
//! IdentityRestore は引き続き拒否し、秘密は harness に渡さない。

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use task_core::browser_isolation::{
    CdpEndpoint, IsolationAttestation, LauncherSessionProof, Namespace, REQUIRED_NAMESPACES,
    RuntimeFacts, verify_isolation,
};
use task_core::browser_wait::{BrowserWait, BrowserWaitState};
use task_core::{BrowserRun, BrowserRunState};

use super::{BrowserContext, BrowserSink, CLI, EventSinkLive, forward_events, write_private};
use crate::browser_action::{ActionExecutor, ActionRequest, ActionServer};
use crate::browser_launcher::backend::ARTIFACT_FAILURE_PREFIX;
use crate::browser_launcher::protocol::{
    ARTIFACT_PROTOCOL, ArtifactKind, AuthenticateArgs, AuthenticationStatus,
    CREDENTIAL_LOGIN_PROTOCOL, LoginObservation, LoginResult, MAX_ARTIFACT_BYTES, MAX_ARTIFACTS,
    POST_LOGIN_PROTOCOL, artifact_name_verb,
};
use crate::browser_launcher::{
    ActionArgs, ClientError, ErrorCode, LauncherClient, Observation, Outcome, Receipt,
    SessionFacts, SessionPolicy, SessionState, StartedSession, Verb,
};
use crate::{AdapterError, EventSink, RunLimits, RunOutcome, RunRequest, WorkerAdapter};

pub(crate) const UNAVAILABLE: &str = "isolated_runtime_unavailable";
/// v8 artifact transfer の固定理由（付記 2026-10-10e）。file の中身・path は入れない。
pub(crate) const ARTIFACTS_REQUIRE_V8: &str = "browser_launcher_protocol_artifacts_required";
pub(crate) const ARTIFACT_REJECTED: &str = "browser_artifact_type_rejected";
pub(crate) const ARTIFACT_TOO_LARGE: &str = "browser_artifact_too_large";
pub(crate) const ARTIFACT_LIMIT: &str = "browser_artifact_count_limit";
pub(crate) const ARTIFACT_FAILED: &str = "browser_artifact_transfer_failed";
/// screenshot / download の操作そのものが失敗した（他 origin の download の取消を含む）。
pub(crate) const ARTIFACT_ACTION_FAILED: &str = "browser_artifact_action_failed";
/// 付記 2026-10-10f: the launcher refused the verb (session policy, or the post-login read does
/// not include it / observation is held after the login).
pub(crate) const ARTIFACT_ACTION_REFUSED: &str = "browser_artifact_action_refused";
/// The launcher's runner or action deadline expired.
pub(crate) const ARTIFACT_ACTION_TIMEOUT: &str = "browser_artifact_action_timeout";
/// The launcher's isolation check failed before the action ran.
pub(crate) const ARTIFACT_ISOLATION_FAILED: &str = "browser_artifact_isolation_failed";
/// The launcher could not be reached or answered outside the protocol.
pub(crate) const ARTIFACT_LAUNCHER_UNAVAILABLE: &str = "browser_artifact_launcher_unavailable";

/// The fixed reason for a failed launcher `action` of a screenshot / download (付記 2026-10-10f).
/// `BadRequest` is the runner's failure (agent-browser or the relay's gate); the launcher journal
/// has its fixed `runner_reason` / `error_class` / `gate` tokens.
pub(crate) fn artifact_action_reason(code: Option<ErrorCode>) -> &'static str {
    match code {
        Some(ErrorCode::Unauthorized) => ARTIFACT_ACTION_REFUSED,
        Some(ErrorCode::Timeout) => ARTIFACT_ACTION_TIMEOUT,
        Some(ErrorCode::Limit) => ARTIFACT_LIMIT,
        Some(ErrorCode::IsolationFailed) => ARTIFACT_ISOLATION_FAILED,
        Some(
            ErrorCode::BadRequest
            | ErrorCode::LaunchFailed
            | ErrorCode::LeaseMismatch
            | ErrorCode::ArtifactRejected,
        ) => ARTIFACT_ACTION_FAILED,
        None => ARTIFACT_LAUNCHER_UNAVAILABLE,
    }
}

/// 1 要求の読み書きの期限。launcher 側の最長（`action` 120 秒）より長く取る。
const CLIENT_TIMEOUT: Duration = Duration::from_secs(150);

/// launcher の中で Chrome が動く userns 内の UID / GID（ADR-0115 の `1000 → S`）。
const INNER_ID: u32 = 1000;

/// daemon は launcher の process group を観測できない（別 UID の process）。launcher の
/// `isolation_ok`（launcher 自身が `collect_facts` → `verify_isolation` を通した結果）が真のとき
/// だけ、group が launcher に保持されていることの印としてこの値を入れる。daemon はこの値で
/// signal を送らない（回収は launcher の stop / 切断で行う）。
const LAUNCHER_HELD_PGID: i32 = i32::MAX;

/// launcher の 1 session。drop で stop を頼み、接続も閉じる（launcher は切断でも回収する）。
pub(crate) struct LauncherRuntime {
    client: Mutex<LauncherClient>,
    session_id: String,
    lease_id: String,
    /// daemon 側で照合できた launcher の session 証明（ADR-0138 D-L）。照合に失敗したら `None`。
    proof: Option<LauncherSessionProof>,
    /// 起動時に `verify_isolation` を通した attestation（Live View の registry entry に載せる）。
    attestation: Option<IsolationAttestation>,
    stopped: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for LauncherRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LauncherRuntime")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl LauncherRuntime {
    /// 試験専用 loopback 許可を拒否しない（本番でない daemon の）起動。
    #[cfg(test)]
    pub(crate) fn start(
        socket: &Path,
        task_id: &str,
        run_id: &str,
        policy: SessionPolicy,
    ) -> Result<(Self, IsolationAttestation), &'static str> {
        Self::start_guarded(socket, false, task_id, run_id, policy)
    }

    /// 接続・起動・観測・隔離検査。どこで失敗しても session を残さず `UNAVAILABLE`。
    /// `refuse_test_loopback` が真なら、`start_session` の前に `hello` で launcher の申告を問い、
    /// 試験専用 loopback 許可が有効な launcher・申告を返さない launcher とは session を作らない
    /// （付記 E2 の daemon 側。fail closed）。
    pub(crate) fn start_guarded(
        socket: &Path,
        refuse_test_loopback: bool,
        task_id: &str,
        run_id: &str,
        policy: SessionPolicy,
    ) -> Result<(Self, IsolationAttestation), &'static str> {
        let mut client =
            LauncherClient::connect(socket, CLIENT_TIMEOUT).map_err(|_| UNAVAILABLE)?;
        if refuse_test_loopback {
            refuse_test_loopback_launcher(&mut client)?;
        }
        let lease_id = crate::browser_launcher::random_id().map_err(|_| UNAVAILABLE)?;
        let started = client
            .start_session(task_id, run_id, &lease_id, policy)
            .map_err(|_| UNAVAILABLE)?;
        let ids = DaemonIds::current();
        let proof = launcher_session_proof(&started, client.responder_uid(), &ids);
        let mut runtime = Self {
            client: Mutex::new(client),
            session_id: started.session_id.clone(),
            lease_id,
            proof,
            attestation: None,
            stopped: std::sync::atomic::AtomicBool::new(false),
        };
        // ここから先の失敗は drop が stop を頼む。
        if started.receipt.outcome != Outcome::Started
            || started.receipt.session_id != started.session_id
        {
            return Err(UNAVAILABLE);
        }
        let (state, facts) = runtime.observe()?;
        if state != SessionState::Running {
            return Err(UNAVAILABLE);
        }
        let facts = runtime_facts(
            &runtime.session_id,
            &ids,
            &facts,
            started.receipt.isolation_ok,
        )
        .ok_or(UNAVAILABLE)?;
        let attestation = verify_isolation(&facts).map_err(|_| UNAVAILABLE)?;
        runtime.attestation = Some(attestation.clone());
        Ok((runtime, attestation))
    }

    pub(crate) fn action(
        &self,
        verb: Verb,
        args: ActionArgs,
    ) -> Result<(Receipt, Observation), &'static str> {
        self.action_coded(verb, args).map_err(|_| UNAVAILABLE)
    }

    /// Like [`Self::action`], but a refusal keeps the launcher's fixed error code (`None`: no
    /// protocol answer, or a receipt that does not hold).
    pub(crate) fn action_coded(
        &self,
        verb: Verb,
        args: ActionArgs,
    ) -> Result<(Receipt, Observation), Option<ErrorCode>> {
        let mut client = self.client.lock().map_err(|_| None)?;
        let (receipt, observation) = client
            .action(&self.session_id, &self.lease_id, verb, args)
            .map_err(|e| match e {
                ClientError::Remote(code) => Some(code),
                _ => None,
            })?;
        if !receipt.isolation_ok || receipt.session_id != self.session_id {
            return Err(None);
        }
        Ok((receipt, observation))
    }

    /// v8: the whole artifact `name` (launcher-generated) in bounded chunks over the session's own
    /// connection. The launcher's type, the total size and each chunk must stay consistent, the
    /// total must not exceed [`MAX_ARTIFACT_BYTES`], and the bytes are sniffed again here. Errors are
    /// fixed reasons; no byte of the file is put into them.
    pub(crate) fn fetch_artifact(
        &self,
        name: &str,
        verb: Verb,
    ) -> Result<(ArtifactKind, Vec<u8>), &'static str> {
        if artifact_name_verb(name) != Some(verb) {
            return Err(ARTIFACT_FAILED);
        }
        let mut client = self.client.lock().map_err(|_| UNAVAILABLE)?;
        let mut data: Vec<u8> = Vec::new();
        let mut expect: Option<(ArtifactKind, u64)> = None;
        loop {
            let chunk = client
                .fetch_artifact(&self.session_id, &self.lease_id, name, data.len() as u64)
                .map_err(|e| match e {
                    crate::browser_launcher::ClientError::Remote(
                        crate::browser_launcher::ErrorCode::ArtifactRejected,
                    ) => ARTIFACT_REJECTED,
                    crate::browser_launcher::ClientError::Remote(
                        crate::browser_launcher::ErrorCode::Limit,
                    ) => ARTIFACT_TOO_LARGE,
                    _ => ARTIFACT_FAILED,
                })?;
            if chunk.size > MAX_ARTIFACT_BYTES {
                return Err(ARTIFACT_TOO_LARGE);
            }
            match expect {
                None => expect = Some((chunk.kind, chunk.size)),
                Some(first) if first == (chunk.kind, chunk.size) => {}
                Some(_) => return Err(ARTIFACT_FAILED),
            }
            if chunk.data.len() > crate::browser_launcher::protocol::ARTIFACT_CHUNK
                || data.len() as u64 + chunk.data.len() as u64 > chunk.size
            {
                return Err(ARTIFACT_FAILED);
            }
            if chunk.data.is_empty() && (data.len() as u64) < chunk.size {
                return Err(ARTIFACT_FAILED);
            }
            data.extend_from_slice(&chunk.data);
            if data.len() as u64 == chunk.size {
                break;
            }
        }
        let (kind, _) = expect.ok_or(ARTIFACT_FAILED)?;
        match ArtifactKind::sniff(&data) {
            Some(sniffed) if sniffed == kind && kind.allowed_for(verb) => Ok((kind, data)),
            _ => Err(ARTIFACT_REJECTED),
        }
    }

    pub(crate) fn observe(&self) -> Result<(SessionState, SessionFacts), &'static str> {
        let mut client = self.client.lock().map_err(|_| UNAVAILABLE)?;
        client
            .observe(&self.session_id, &self.lease_id)
            .map_err(|_| UNAVAILABLE)
    }

    /// launcher の protocol 版（同じ接続の `hello`）。session には触らない。
    pub(crate) fn protocol_version(&self) -> Result<u32, &'static str> {
        self.client
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .hello()
            .map(|(version, _)| version)
            .map_err(|_| UNAVAILABLE)
    }

    /// v4 `auth_begin`: the launcher stops agent observation and creates the login target.
    pub(crate) fn auth_begin(&self, auth_section_id: &str) -> Result<String, &'static str> {
        self.client
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .auth_begin(&self.session_id, &self.lease_id, auth_section_id)
            .map_err(|_| UNAVAILABLE)
    }

    /// v4 `authenticate` over `broker` (an `injection.sock` connection this daemon opened). The
    /// FD is passed with `SCM_RIGHTS`; this process's copy is closed as soon as it has been sent.
    pub(crate) fn authenticate(
        &self,
        args: AuthenticateArgs,
        broker: std::os::fd::OwnedFd,
    ) -> Result<(AuthenticationStatus, LoginResult), &'static str> {
        let mut client = self.client.lock().map_err(|_| UNAVAILABLE)?;
        client.authenticate(args, broker).map_err(|_| UNAVAILABLE)
    }

    /// launcher が採番した session id（credentiald に登録した稼働 session）。
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    /// daemon 側で照合できた launcher の session 証明。無ければ機密能力の admission は拒否される。
    pub(crate) fn session_proof(&self) -> Option<&LauncherSessionProof> {
        self.proof.as_ref()
    }

    /// 付記 2026-10-10b: session の Live View frame 接続を開く。版は同じ control 接続の `hello` で
    /// 確かめ、8 未満なら `live_start` を送らない（[`open_frame_relay`]）。
    pub(crate) fn open_live_frames(
        &self,
        socket: &Path,
    ) -> Result<crate::browser_live::FrameRelay, &'static str> {
        let version = self
            .protocol_version()
            .map_err(|_| crate::browser_live::LIVE_STREAM_UNAVAILABLE_REASON)?;
        open_frame_relay(socket, version, &self.session_id, &self.lease_id)
    }

    /// 一度だけ stop を頼む。2 回目以降は何もしない。
    pub(crate) fn stop(&self) -> Result<Option<Receipt>, &'static str> {
        if self.stopped.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Ok(None);
        }
        let mut client = self.client.lock().map_err(|_| UNAVAILABLE)?;
        client
            .stop(&self.session_id, &self.lease_id)
            .map(Some)
            .map_err(|_| UNAVAILABLE)
    }
}

/// frame 接続の読み書きの期限（`live_start` から `live_started` までを含む）。frame が来なくても
/// （静止した頁）この間隔で停止の旗を見直す。期限切れは応答の先頭の `MSG_PEEK` で起きるので、
/// stream の区切りは崩れない。
const LIVE_READ_TICK: Duration = Duration::from_secs(10);

/// 付記 2026-10-10b daemon: launcher の `hello` の版（`launcher_protocol`）が 8 以上のときだけ
/// session の Live View frame 接続を開き、[`crate::browser_live::FrameRelay`] で容量 1 の slot に
/// 中継する。8 未満なら接続せず（`live_start` を送らず）
/// [`crate::browser_live::LIVE_NO_FRAMES_REASON`] を返す。frame は opaque に slot へ入れるだけで、
/// 中身を log・event に出さない。別 session の frame・seq の巻き戻り・上限超過は stream の終わり
/// （slot を閉じる）として扱う。
pub(crate) fn open_frame_relay(
    socket: &Path,
    launcher_protocol: u32,
    session_id: &str,
    lease_id: &str,
) -> Result<crate::browser_live::FrameRelay, &'static str> {
    use crate::browser_launcher::ClientError;
    use crate::browser_launcher::client::LiveRead;
    use crate::browser_live::RelayRead;
    let mut stream = match LauncherClient::open_live(
        socket,
        LIVE_READ_TICK,
        launcher_protocol,
        session_id,
        lease_id,
    ) {
        Ok(stream) => stream,
        Err(ClientError::NoLiveFrames(_)) => {
            return Err(crate::browser_live::LIVE_NO_FRAMES_REASON);
        }
        Err(_) => return Err(crate::browser_live::LIVE_STREAM_UNAVAILABLE_REASON),
    };
    let read = move || {
        match stream.next_frame() {
            Ok(LiveRead::Frame(seq, image)) => {
                let (width, height) = (image.width(), image.height());
                let encoding = match image.encoding() {
                    crate::browser_launcher::protocol::LiveEncoding::Jpeg => {
                        task_core::browser_live_frame::LiveFrameEncoding::Jpeg
                    }
                    crate::browser_launcher::protocol::LiveEncoding::Png => {
                        task_core::browser_live_frame::LiveFrameEncoding::Png
                    }
                };
                match task_core::browser_live_frame::LiveFrame::new(
                    seq,
                    width,
                    height,
                    encoding,
                    image.into_body(),
                ) {
                    Ok(frame) => RelayRead::Frame(frame),
                    // 空・寸法 0 の frame は捨てて続ける（上限超過は client が protocol error にする）。
                    Err(_) => RelayRead::Idle,
                }
            }
            Ok(LiveRead::Stopped) => RelayRead::End,
            Err(ClientError::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                RelayRead::Idle
            }
            Err(_) => RelayRead::End,
        }
    };
    crate::browser_live::FrameRelay::spawn("celeris-live", read)
        .map_err(|_| crate::browser_live::LIVE_STREAM_UNAVAILABLE_REASON)
}

/// 本番の daemon が試験許可の launcher を使わない検査（付記 E2）。`hello` に答えない launcher
/// （旧版・error）も申告を確かめられないので拒否する。
pub(crate) fn refuse_test_loopback_launcher(
    client: &mut LauncherClient,
) -> Result<(), &'static str> {
    match client.hello() {
        Ok((_, allow)) if allow.is_empty() => Ok(()),
        Ok((_, allow)) => {
            tracing::error!(
                test_loopback_allow = ?allow,
                "browser launcher has test-only loopback egress enabled; a production daemon refuses it"
            );
            Err(UNAVAILABLE)
        }
        Err(e) => {
            tracing::error!(error = %e, "browser launcher did not answer hello; a production daemon refuses it");
            Err(UNAVAILABLE)
        }
    }
}

impl Drop for LauncherRuntime {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// daemon 自身の UID / GID（`RuntimeFacts.host_uid` と、map に現れてはならない値）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct DaemonIds {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

impl DaemonIds {
    pub(crate) fn current() -> Self {
        Self {
            uid: nix::unistd::getuid().as_raw(),
            gid: nix::unistd::getgid().as_raw(),
        }
    }
}

/// `pid` が生きている（zombie でない）ときだけ `/proc/<pid>/stat` の starttime を返す。
fn live_starttime(pid: i32) -> Option<u64> {
    if pid <= 1 {
        return None;
    }
    let t = crate::browser_runtime::process_starttime(pid)?;
    crate::browser_runtime::same_process_alive(pid, t).then_some(t)
}

/// launcher の `Started` 応答を daemon 自身の観測と照合し、通ったときだけ
/// [`LauncherSessionProof`] を組む（ADR-0138 D-L、fail closed）。次のどれかなら `None`:
/// receipt に束縛が無い（v1 の launcher）・束縛に 6 つの namespace の inode が揃っていない
/// （v2 の launcher）・receipt と応答の session / instance が食い違う・
/// launcher の `isolation_ok` が偽・応答の送り手（`SCM_CREDENTIALS`、ADR-0116 付記 D-P）を採れない・pid の process が無い（zombie を含む）・
/// `/proc/<pid>/stat` の starttime が束縛と違う・owner UID が不明か daemon の UID。
/// 欠けた値を安全そうな値で埋めることはしない。launcher UID の設定値との照合は admission 側
/// （`verify_launcher_session`）が行う。
pub(crate) fn launcher_session_proof(
    started: &StartedSession,
    peer_uid: Option<u32>,
    daemon: &DaemonIds,
) -> Option<LauncherSessionProof> {
    let r = &started.receipt;
    let binding = r.binding.as_ref()?;
    // v2 以前の束縛（namespace の inode が無い・欠けている）は証明なし。
    if !REQUIRED_NAMESPACES
        .iter()
        .all(|ns| binding.ns_inodes.contains_key(ns))
    {
        return None;
    }
    if r.outcome != Outcome::Started
        || !r.isolation_ok
        || r.session_id != started.session_id
        || r.instance_id != started.instance_id
        || started.instance_id.is_empty()
    {
        return None;
    }
    let launcher_uid = peer_uid?;
    let owner = binding.ns_owner_uid?;
    if owner == daemon.uid {
        return None;
    }
    if live_starttime(binding.pid)? != binding.starttime {
        return None;
    }
    Some(LauncherSessionProof {
        session_id: started.session_id.clone(),
        instance_id: started.instance_id.clone(),
        pid: binding.pid,
        starttime: binding.starttime,
        ns_owner_uid: Some(owner),
        launcher_uid,
        isolation_ok: true,
        ns_inodes: binding.ns_inodes.clone(),
    })
}

/// `/proc/<pid>/uid_map` 形式の 1 行 = `inside outside count`。読めない行があれば `None`。
fn parse_map(map: &str) -> Option<Vec<(u32, u32, u32)>> {
    let mut out = Vec::new();
    for line in map.lines().filter(|l| !l.trim().is_empty()) {
        let mut it = line.split_whitespace().map(str::parse::<u32>);
        let (Some(Ok(inside)), Some(Ok(outside)), Some(Ok(count)), None) =
            (it.next(), it.next(), it.next(), it.next())
        else {
            return None;
        };
        out.push((inside, outside, count));
    }
    (!out.is_empty()).then_some(out)
}

fn covers_outside(map: &[(u32, u32, u32)], id: u32) -> bool {
    map.iter().any(|&(_, outside, count)| {
        id >= outside && u64::from(id) < u64::from(outside) + u64::from(count)
    })
}

fn outside_of(map: &[(u32, u32, u32)], inside_id: u32) -> Option<u32> {
    map.iter().find_map(|&(inside, outside, count)| {
        (inside_id >= inside && u64::from(inside_id) < u64::from(inside) + u64::from(count))
            .then(|| outside + (inside_id - inside))
    })
}

/// launcher の非機密の観測から `RuntimeFacts` を組む。daemon が観測で確かめられる項目
/// （map・namespace owner・CapEff・NoNewPrivs・listen 数）は観測値をそのまま使い、daemon UID / GID
/// が map に現れる・owner が daemon UID・owner が不明のいずれかなら `None`（fail closed）。
/// daemon から見えない項目（mount・namespace の別・process group）は launcher の
/// `isolation_ok` が真のときだけ満たした値にし、偽なら違反になる値にする。
pub(crate) fn runtime_facts(
    session_id: &str,
    daemon: &DaemonIds,
    facts: &SessionFacts,
    launcher_attested: bool,
) -> Option<RuntimeFacts> {
    let uid_map = parse_map(&facts.uid_map)?;
    let gid_map = parse_map(&facts.gid_map)?;
    if covers_outside(&uid_map, daemon.uid) || covers_outside(&gid_map, daemon.gid) {
        return None;
    }
    let owner = facts.ns_owner_uid?;
    if owner == daemon.uid {
        return None;
    }
    let runtime_uid = outside_of(&uid_map, INNER_ID)?;
    let capabilities_dropped = u64::from_str_radix(facts.cap_eff.trim(), 16) == Ok(0);
    let namespaces: BTreeSet<Namespace> = if launcher_attested {
        REQUIRED_NAMESPACES.into_iter().collect()
    } else {
        // map が host と異なることだけは観測から言える。
        [Namespace::User].into_iter().collect()
    };
    Some(RuntimeFacts {
        session_id: session_id.to_owned(),
        host_uid: daemon.uid,
        runtime_uid,
        namespaces,
        root_readonly: launcher_attested,
        writable_mounts: Vec::new(),
        visible_paths: Vec::new(),
        // The launcher owns Chrome's CDP pipe (fds 3/4). The observed TCP
        // listeners belong to sandboxd's proxy and shared-CDP relay inside
        // the private netns; their count does not describe Chrome's CDP
        // endpoint. The launcher's isolation_ok attests the real netns and
        // the fixed pipe-based runtime before this observation is accepted.
        cdp: CdpEndpoint::Pipe,
        no_new_privs: facts.no_new_privs,
        capabilities_dropped,
        userns_owner_uid: Some(owner),
        pgid: if launcher_attested {
            LAUNCHER_HELD_PGID
        } else {
            0
        },
    })
}

/// agent-browser の action 名（harness policy の `allow`）を launcher の固定 verb に写す。
pub(crate) fn session_policy(
    allow: &[String],
    domains: &[String],
    lease: Duration,
) -> SessionPolicy {
    let mut actions = Vec::new();
    for a in allow {
        let verb = match a.as_str() {
            "navigate" => Verb::Open,
            "click" => Verb::Click,
            "snapshot" => Verb::Snapshot,
            "gettext" => Verb::Extract,
            "screenshot" => Verb::Screenshot,
            "download" => Verb::Download,
            "scroll" => Verb::Scroll,
            "close" => Verb::Close,
            _ => continue,
        };
        if !actions.contains(&verb) {
            actions.push(verb);
        }
    }
    SessionPolicy {
        allowed_domains: domains.to_vec(),
        allowed_actions: actions,
        lease_seconds: lease
            .as_secs()
            .clamp(1, crate::browser_launcher::protocol::MAX_LEASE_SECONDS),
    }
}

/// shim の検査済み action を launcher に頼む（[`ActionServer`] の検査と control gate の後）。
/// screenshot / download は v8 launcher から file を chunk で受け取り、shim が名付けた
/// `output/<name>` に新規に書く（付記 2026-10-10e）。v8 未満の launcher・型/長さ/件数の超過・転送の
/// 失敗は固定理由で失敗にし、file を残さない（agent には opaque な失敗）。
pub(crate) struct LauncherExecutor {
    pub(crate) runtime: Arc<LauncherRuntime>,
    /// run の `browser/output`（shim の `config.output`）。
    pub(crate) output: std::path::PathBuf,
    /// session 開始時に `hello` で得た launcher の protocol 版。
    pub(crate) protocol: u32,
    /// この run で渡した file の数。
    pub(crate) delivered: std::sync::atomic::AtomicUsize,
    /// 失敗させた screenshot / download の固定理由（run が progress に記録する）。
    pub(crate) refusals: Mutex<Vec<&'static str>>,
}

impl LauncherExecutor {
    pub(crate) fn new(
        runtime: Arc<LauncherRuntime>,
        output: std::path::PathBuf,
        protocol: u32,
    ) -> Self {
        Self {
            runtime,
            output,
            protocol,
            delivered: std::sync::atomic::AtomicUsize::new(0),
            refusals: Mutex::new(Vec::new()),
        }
    }

    /// 記録された固定理由を取り出す。
    pub(crate) fn take_refusals(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.refusals.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// 失敗を shim への固定理由付きの応答にする（file・path・page の data は入れない）。
    fn refused(&self, refusal: ArtifactRefusal) -> serde_json::Value {
        let ArtifactRefusal { reason, detail } = refusal;
        match &detail {
            Some(detail) => {
                tracing::warn!(reason, detail = %detail, "browser launcher artifact refused")
            }
            None => tracing::warn!(reason, "browser launcher artifact refused"),
        }
        self.refusals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(reason);
        let mut out = serde_json::json!({"status": 1, "stdout": "", "reason": reason});
        if let Some(detail) = detail {
            out["detail"] = detail.into();
        }
        out
    }

    /// screenshot / download: the launcher runs the verb, then the daemon pulls the produced file
    /// and writes it under the shim's name. Returns the fixed reason on failure.
    fn artifact_action(
        &self,
        verb: Verb,
        args: ActionArgs,
        shim_name: Option<&str>,
    ) -> Result<serde_json::Value, ArtifactRefusal> {
        if self.protocol < ARTIFACT_PROTOCOL {
            return Err(ARTIFACTS_REQUIRE_V8.into());
        }
        let shim_name = shim_name
            .filter(|n| artifact_name_verb(n) == Some(verb))
            .ok_or(ARTIFACT_FAILED)?;
        if self.delivered.load(std::sync::atomic::Ordering::SeqCst) >= MAX_ARTIFACTS {
            return Err(ARTIFACT_LIMIT.into());
        }
        let (_receipt, observation) = self
            .runtime
            .action_coded(verb, args)
            .map_err(artifact_action_reason)?;
        // 付記 2026-10-10j: the launcher's runner could not produce the file; its fixed tokens go
        // to the agent with the reason.
        if observation.artifact.is_none()
            && let Some(text) = observation
                .text
                .as_deref()
                .and_then(|t| t.strip_prefix(ARTIFACT_FAILURE_PREFIX))
        {
            return Err(ArtifactRefusal {
                reason: ARTIFACT_ACTION_FAILED,
                detail: artifact_failure_detail(text),
            });
        }
        let name = observation.artifact.as_deref().ok_or(ARTIFACT_FAILED)?;
        let (kind, bytes) = self.runtime.fetch_artifact(name, verb)?;
        write_artifact(&self.output.join(shim_name), &bytes).map_err(|_| ARTIFACT_FAILED)?;
        self.delivered
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(serde_json::json!({
            "status": 0,
            "stdout": serde_json::json!({
                "success": true,
                "data": {"media_type": kind.media_type(), "bytes": bytes.len()},
            })
            .to_string(),
        }))
    }
}

/// A refused screenshot / download: the fixed reason and, when the launcher gave them, its fixed
/// diagnostic tokens (付記 2026-10-10j).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArtifactRefusal {
    pub(crate) reason: &'static str,
    pub(crate) detail: Option<String>,
}

impl From<&'static str> for ArtifactRefusal {
    fn from(reason: &'static str) -> Self {
        Self {
            reason,
            detail: None,
        }
    }
}

/// The launcher's failure tokens, if they keep to the fixed shape: `key=value` words of ASCII
/// letters, digits, `_ , . ! -`, at most 600 bytes. Anything else is dropped (no free text).
pub(crate) fn artifact_failure_detail(text: &str) -> Option<String> {
    let ok = (1..=600).contains(&text.len())
        && text.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'=' | b',' | b'.' | b'!' | b' ' | b'-')
        });
    ok.then(|| text.to_owned())
}

/// `path` を新規に（symlink を辿らず、既存を上書きせず）0600 で書く。途中で失敗したら消す。
fn write_artifact(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(path)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    if written.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    written
}

impl ActionExecutor for LauncherExecutor {
    fn run(&self, _sequence: u64, req: &ActionRequest) -> std::io::Result<serde_json::Value> {
        let unsupported = || std::io::Error::other(UNAVAILABLE);
        let first = || req.args.first().cloned();
        let (verb, args) = match req.verb.as_str() {
            "open" => (
                Verb::Open,
                ActionArgs {
                    url: first(),
                    ..ActionArgs::default()
                },
            ),
            "click" | "extract" => (
                if req.verb == "click" {
                    Verb::Click
                } else {
                    Verb::Extract
                },
                ActionArgs {
                    selector: first(),
                    ..ActionArgs::default()
                },
            ),
            "screenshot" if req.args.is_empty() => {
                return Ok(self
                    .artifact_action(
                        Verb::Screenshot,
                        ActionArgs::default(),
                        req.artifact.as_deref(),
                    )
                    .unwrap_or_else(|reason| self.refused(reason)));
            }
            "download" if req.args.len() == 1 => {
                let args = ActionArgs {
                    selector: first(),
                    ..ActionArgs::default()
                };
                return Ok(self
                    .artifact_action(Verb::Download, args, req.artifact.as_deref())
                    .unwrap_or_else(|reason| self.refused(reason)));
            }
            "snapshot" => (Verb::Snapshot, ActionArgs::default()),
            "scroll" => {
                let amount: i32 = req
                    .args
                    .get(1)
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(unsupported)?;
                let y = if req.args.first().map(String::as_str) == Some("up") {
                    -amount
                } else {
                    amount
                };
                (
                    Verb::Scroll,
                    ActionArgs {
                        y: Some(y),
                        ..ActionArgs::default()
                    },
                )
            }
            "close" => (Verb::Close, ActionArgs::default()),
            _ => return Err(unsupported()),
        };
        let (_receipt, observation) = self.runtime.action(verb, args).map_err(|_| unsupported())?;
        Ok(serde_json::json!({
            "status": 0,
            "stdout": observation.text.unwrap_or_default(),
        }))
    }
}

const CREDENTIAL_DENIED: &str =
    "browser credential use is not available through the launcher runtime";

fn credential_denied() -> AdapterError {
    AdapterError::Other(CREDENTIAL_DENIED.into())
}

/// launcher 経路の接続先（`[browser] runtime = "launcher"` の設定）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct LauncherTarget<'a> {
    pub(crate) socket: &'a Path,
    pub(crate) refuse_test_loopback: bool,
    /// 設定上の launcher の host UID。CredentialUse の接続前 gate と証明の照合に使う。
    pub(crate) launcher_uid: Option<u32>,
}

/// run が CredentialUse を求めるか。`requested` は admission の対象（policy の CredentialUse、
/// または最後の wait が credential_use の Registered / Approved）、`refused_without_admission` は
/// admission が無ければ拒否される run（ADR 2026-10-08 D2: 登録済み・承認済みの credential 待ち、
/// 操作の無い承認も含む）。`requested` なら必ず `refused_without_admission`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct CredentialDemand {
    pub(crate) requested: bool,
    pub(crate) refused_without_admission: bool,
}

pub(crate) fn credential_demand(
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    waits: &[BrowserWait],
) -> CredentialDemand {
    let by_policy = policy
        .effective
        .actions
        .contains(&task_core::BrowserAction::CredentialUse);
    let last = waits.last();
    let credential_wait = last.is_some_and(|w| {
        matches!(
            w.state,
            BrowserWaitState::Registered | BrowserWaitState::Approved
        ) && w
            .operation
            .as_ref()
            .is_some_and(|o| o.action == "credential_use")
    });
    let refused_wait = last.is_some_and(|w| {
        w.state == BrowserWaitState::Registered
            || (w.state == BrowserWaitState::Approved
                && w.operation
                    .as_ref()
                    .is_none_or(|o| o.action == "credential_use"))
    });
    CredentialDemand {
        requested: by_policy || credential_wait,
        refused_without_admission: by_policy || refused_wait,
    }
}

/// launcher に接続する前に決まる条件（ADR 2026-10-09 付記「接続前 gate」）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PreconnectFacts<'a> {
    pub(crate) launcher_socket: &'a Path,
    pub(crate) launcher_uid: Option<u32>,
    pub(crate) daemon_uid: u32,
    /// credentiald の制御経路（injection / control socket の置き場所）。
    pub(crate) credentiald_runtime: Option<&'a Path>,
}

/// 接続前 gate。CredentialUse を求めない run は素通し（従来どおり）。求める run は、launcher runtime
/// の設定・launcher UID の設定（0 でも daemon UID でもない）・credentiald の制御経路が揃わなければ
/// launcher に接続せず従来の文言で拒否する。admission の対象でない拒否対象の run（credential 以外の
/// 登録済み wait 等）は接続後に必ず拒否されるので、ここで拒否する。
pub(crate) fn preconnect_credential_gate(
    demand: CredentialDemand,
    facts: &PreconnectFacts<'_>,
) -> Result<(), AdapterError> {
    if !demand.refused_without_admission {
        return Ok(());
    }
    let ready = demand.requested
        && !facts.launcher_socket.as_os_str().is_empty()
        && facts
            .launcher_uid
            .is_some_and(|uid| uid != 0 && uid != facts.daemon_uid)
        && facts.credentiald_runtime.is_some();
    if ready {
        Ok(())
    } else {
        Err(credential_denied())
    }
}

/// credentiald の制御経路の置き場所（daemon の credentiald 設定、無ければ `XDG_RUNTIME_DIR`）。
fn credentiald_runtime_dir() -> Option<std::path::PathBuf> {
    crate::browser_credential::configured()
        .and_then(|sup| sup.runtime_dir.clone())
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from))
}

/// 証明依存の gate（session 起動後・credentiald 登録より前）。証明の検証・owner が daemon でない・
/// `isolation_ok` を満たすときだけ真。
fn credential_admitted(
    requested: bool,
    proof: Option<&LauncherSessionProof>,
    isolation_ok: bool,
    daemon_uid: u32,
) -> bool {
    requested
        && isolation_ok
        && proof.is_some_and(|proof| {
            proof.isolation_ok
                && proof.ns_owner_uid.is_some_and(|owner| owner != daemon_uid)
                && REQUIRED_NAMESPACES
                    .iter()
                    .all(|ns| proof.ns_inodes.contains_key(ns))
        })
}

async fn stop_in_background(runtime: LauncherRuntime) {
    let _ = tokio::task::spawn_blocking(move || runtime.stop()).await;
}

/// 二段 gate で launcher session を開く。
/// 1. 接続前 gate（[`preconnect_credential_gate`]）。不成立なら launcher socket に接続しない。
/// 2. launcher に session を作らせ、隔離検査を通す（失敗は `UNAVAILABLE`）。
/// 3. 証明依存の gate（証明・owner≠daemon・`isolation_ok`・証明の launcher UID が設定値と一致）。
///    不成立で拒否対象の run なら session を stop してから従来の文言で拒否する。
/// 4. 通った run だけ credentiald に session と証明を登録する。失敗なら stop して拒否。
///
/// どの拒否も shim の credential_use 有効化・Authenticate・harness 起動より前に返る。
/// 戻り値の bool は credential admission の成否。
pub(crate) async fn open_launcher_session(
    target: LauncherTarget<'_>,
    task_id: &str,
    run_id: &str,
    session_policy: SessionPolicy,
    demand: CredentialDemand,
    credentiald_runtime: Option<&Path>,
) -> Result<(LauncherRuntime, bool), AdapterError> {
    let daemon_uid = DaemonIds::current().uid;
    preconnect_credential_gate(
        demand,
        &PreconnectFacts {
            launcher_socket: target.socket,
            launcher_uid: target.launcher_uid,
            daemon_uid,
            credentiald_runtime,
        },
    )?;
    let (socket, refuse, task_id, run_id) = (
        target.socket.to_path_buf(),
        target.refuse_test_loopback,
        task_id.to_owned(),
        run_id.to_owned(),
    );
    let (runtime, attestation) = tokio::task::spawn_blocking(move || {
        LauncherRuntime::start_guarded(&socket, refuse, &task_id, &run_id, session_policy)
    })
    .await
    .map_err(|_| AdapterError::Other(UNAVAILABLE.into()))?
    .map_err(|_| AdapterError::Other(UNAVAILABLE.into()))?;
    let admitted = credential_admitted(
        demand.requested,
        runtime.session_proof(),
        attestation.session_id() == runtime.session_id,
        daemon_uid,
    ) && runtime
        .session_proof()
        .is_some_and(|p| Some(p.launcher_uid) == target.launcher_uid);
    if demand.refused_without_admission && !admitted {
        stop_in_background(runtime).await;
        return Err(credential_denied());
    }
    if admitted {
        // Bind the proof to credentiald before the shim's credential capability is enabled.
        let registered = match (runtime.session_proof(), credentiald_runtime) {
            (Some(proof), Some(dir)) => register_launcher_proof(&runtime.session_id, proof, dir),
            _ => Err(()),
        };
        if registered.is_err() {
            stop_in_background(runtime).await;
            return Err(credential_denied());
        }
    }
    Ok((runtime, admitted))
}

/// credentiald に稼働 session（controller = この daemon worker、runtime = launcher の browser）と
/// launcher 証明を登録する。秘密は含めない。
fn register_launcher_proof(
    session_id: &str,
    proof: &LauncherSessionProof,
    credentiald_runtime: &Path,
) -> Result<(), ()> {
    let client = crate::browser_cdp_sink::UnixInjectionClient::new(
        celeris_credentiald::injection_ipc::injection_socket(credentiald_runtime),
    );
    let controller_start =
        crate::browser_runtime::process_starttime(std::process::id() as i32).ok_or(())?;
    client
        .register_live_session(
            celeris_credentiald::injection_ipc::LiveSessionRegistration {
                session_id: session_id.to_owned(),
                controller_pid: std::process::id(),
                controller_start,
                runtime_pid: u32::try_from(proof.pid).map_err(|_| ())?,
                runtime_start: proof.starttime,
            },
        )
        .map_err(|_| ())?;
    client
        .attach_launcher_proof(
            celeris_credentiald::injection_ipc::LauncherProofRegistration {
                session_id: session_id.to_owned(),
                instance_id: proof.instance_id.clone(),
                peer_uid: Some(proof.launcher_uid),
                proof: proof.clone(),
            },
        )
        .map_err(|_| ())
}

/// The approved click/download this run resumes (ADR 2026-10-08 D2), if the last wait is one.
fn approved_operation_wait(
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    waits: &[BrowserWait],
) -> Result<Option<(BrowserWait, task_core::BrowserAction)>, AdapterError> {
    let Some(wait) = waits
        .last()
        .filter(|w| w.state == BrowserWaitState::Approved)
        .cloned()
    else {
        return Ok(None);
    };
    Ok(super::approved_operation(&wait, policy)?.map(|action| (wait, action)))
}

/// shim（`celeris-browser.py`）が読む `config.json` の中身。形は daemon 経路と同じ
/// （`policy_sha256` は `policy.json` の byte の sha256、`action_socket` は共通の短い path）。
fn shim_config(
    runtime_dir: &Path,
    session: &str,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    action_policy: &[u8],
    approval_actions: &[String],
    credential_admitted: bool,
) -> Result<Vec<u8>, AdapterError> {
    use sha2::Digest;
    let action_socket = crate::browser_action::action_socket_path(runtime_dir)?;
    Ok(serde_json::to_vec(&serde_json::json!({
        "session_id": session,
        "allowed_domains": policy.allowed_domains(),
        "output": runtime_dir.join("output"),
        "action_socket": action_socket,
        "policy_sha256": format!("{:x}", sha2::Sha256::digest(action_policy)),
        "credential_policy_ids": if credential_admitted { policy.effective.credential_policy_ids.clone() } else { BTreeSet::<String>::new() },
        "credential_use": credential_admitted,
        "approval_actions": approval_actions,
    }))?)
}

/// shim（`celeris-browser.py`）が読む `policy.json`・`config.json` を新規に書く（run ごとに一度だけ。
/// 既にあれば `create_new` で失敗する）。shim の `load_policy` はこれが揃わないと全 action を拒否する。
/// 返すのは shim と action socket の path。
fn write_shim_files(
    runtime_dir: &Path,
    session: &str,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    action_policy: &[u8],
    approval_actions: &[String],
    credential_admitted: bool,
) -> Result<(std::path::PathBuf, std::path::PathBuf), AdapterError> {
    std::fs::create_dir_all(runtime_dir.join("output"))?;
    let cli = runtime_dir.join("celeris-browser.py");
    let action_socket = crate::browser_action::action_socket_path(runtime_dir)?;
    write_private(&cli, CLI)?;
    write_private(&runtime_dir.join("policy.json"), action_policy)?;
    write_private(
        &runtime_dir.join("config.json"),
        shim_config(
            runtime_dir,
            session,
            policy,
            action_policy,
            approval_actions,
            credential_admitted,
        )?,
    )?;
    Ok((cli, action_socket))
}

/// [`write_shim_files`] が先に書いた fail-closed の `config.json` を、credential admission の結果で
/// 置き換える（`config.next` に書いて rename）。shim と `policy.json` は書き直さない。
fn admit_shim_config(
    runtime_dir: &Path,
    session: &str,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    action_policy: &[u8],
    approval_actions: &[String],
    credential_admitted: bool,
) -> Result<(), AdapterError> {
    let config = shim_config(
        runtime_dir,
        session,
        policy,
        action_policy,
        approval_actions,
        credential_admitted,
    )?;
    super::replace_private(&runtime_dir.join("config.json"), &config)?;
    Ok(())
}

/// launcher 経由の browser run。harness は従来と同じ shim（`celeris-browser.py`）を使い、
/// shim の action は daemon の [`ActionServer`] の検査と gate を通ってから launcher に頼まれる。
/// 試験用の入口（registry なし）。本番は [`run_registered`]。
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    adapter: Arc<dyn WorkerAdapter>,
    req: RunRequest,
    run_id: &str,
    limits: RunLimits,
    sink: &dyn EventSink,
    target: LauncherTarget<'_>,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    credentials: Option<&crate::browser_credential::CredentialSupervisor>,
) -> Result<RunOutcome, AdapterError> {
    run_registered(
        adapter,
        req,
        run_id,
        limits,
        sink,
        target,
        policy,
        credentials,
        None,
    )
    .await
}

/// launcher 経由の browser run（[`run`] と同じ）。`live_sessions` は daemon の稼働 session registry（ADR-0108 D5、task-api と共有）。session が
/// 動いている間、`browser.session_id` で [`crate::browser_live::LauncherLiveEntry`] を載せる
/// （付記 2026-10-10b: v8 の launcher なら frame の購読口付き）。
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_registered(
    adapter: Arc<dyn WorkerAdapter>,
    mut req: RunRequest,
    run_id: &str,
    limits: RunLimits,
    sink: &dyn EventSink,
    target: LauncherTarget<'_>,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    credentials: Option<&crate::browser_credential::CredentialSupervisor>,
    live_sessions: Option<Arc<task_core::browser_isolation::LiveSessions>>,
) -> Result<RunOutcome, AdapterError> {
    let unavailable = || AdapterError::Other(UNAVAILABLE.into());
    // The wait store must be readable before anything else; a CredentialUse run that cannot
    // read it is refused with the credential wording (fail closed before connecting).
    let waits = match sink.browser_waits() {
        Ok(waits) => waits,
        Err(_)
            if policy
                .effective
                .actions
                .contains(&task_core::BrowserAction::CredentialUse) =>
        {
            return Err(credential_denied());
        }
        Err(_) => {
            return Err(AdapterError::Other("browser wait store unavailable".into()));
        }
    };
    // 登録済み credential（`Registered`）は daemon 経路と同じ共有段で credential 使用の承認 wait
    // （`WaitingForApproval`、operation `credential_use`）にし、launcher に接続せず `Terminal::Question`
    // で止まる。承認後の run が下の `approved_credential` → Authenticate に進む（ADR-0116 / ADR-0138、
    // ADR 2026-10-09 付記）。これが無いと harness が再びログイン画面で credential を要求し続けた。
    if let Some(outcome) = super::registered_credential_approval(
        req.task.id,
        run_id,
        &waits,
        policy,
        credentials,
        sink,
    )? {
        return Ok(outcome);
    }
    let demand = credential_demand(policy, &waits);
    // ADR 2026-10-08 D2: an approved operation resumes the logical session that asked for it,
    // with that one action allowed once.
    let approved = approved_operation_wait(policy, &waits)?;
    let approved_credential = waits
        .last()
        .filter(|w| {
            w.state == BrowserWaitState::Approved
                && w.operation
                    .as_ref()
                    .is_some_and(|o| o.action == "credential_use")
        })
        .cloned();
    // The approval must still match the task policy (same check as the daemon path); a stale
    // approval is refused before connecting and is not consumed.
    if let Some(wait) = approved_credential.as_ref() {
        super::check_approved_credential(wait, policy, credentials)?;
    }
    let approved_action = approved.as_ref().map(|(_, a)| *a);
    // A resumed run continues the logical session that asked for the approval (operation or
    // credential use), as on the daemon path.
    let session = match (&approved, &approved_credential) {
        (Some((wait, _)), _) | (None, Some(wait)) => wait.session_id.clone(),
        (None, None) => super::session_id(req.task.id, run_id),
    };
    // After credential use the harness gets the same reduced policy as on the daemon path
    // (launch / close / navigate; ADR-0080 H3 keeps observation stopped for the session).
    let action_policy = match approved_action {
        Some(action) => super::resumed_policy_bytes(policy, action)?,
        None if approved_credential.is_some() => {
            super::credential_harness_policy(&policy.action_policy)?
        }
        None => policy.action_policy.clone(),
    };
    // ADR 2026-10-09 credential username / post-login D2: the read the pinned site policy opts into.
    // The launcher session is started with the post-login verbs so they can run once the auth
    // section closes on the post-login conditions; until then (or if it never does) the launcher's
    // controller refuses every agent command and the daemon's action server does not allow them.
    let post_login_plan = match approved_credential
        .as_ref()
        .and_then(|w| w.trusted_login.as_ref())
    {
        Some(pinned) => super::post_login_read(policy, pinned)?,
        None => None,
    };
    let approval_actions = super::shim_approval_actions(policy, approved_action);
    let runtime_dir = req.workspace.join("runs").join(run_id).join("browser");
    let output = runtime_dir.join("output");
    // The shim files are created once per run, fail closed (no credential capability).
    let (cli, action_socket) = write_shim_files(
        &runtime_dir,
        &session,
        policy,
        &action_policy,
        &approval_actions,
        false,
    )?;
    let mut allowed: task_core::AgentBrowserActionPolicy =
        serde_json::from_slice(&action_policy)
            .map_err(|_| AdapterError::Other("browser policy rejected".into()))?;
    let control_gate = sink
        .browser_control_gate(run_id, &session)
        .ok_or_else(|| AdapterError::Other("browser control store unavailable".into()))?;
    let launcher_allow = match &post_login_plan {
        Some((_, bytes)) => {
            serde_json::from_slice::<task_core::AgentBrowserActionPolicy>(bytes)
                .map_err(|_| AdapterError::Other("browser policy rejected".into()))?
                .allow
        }
        None => allowed.allow.clone(),
    };
    let session_policy =
        session_policy(&launcher_allow, policy.allowed_domains(), limits.wall_clock);
    let credentiald_runtime = credentiald_runtime_dir();
    let (runtime, credential_admitted) = open_launcher_session(
        target,
        &req.task.id.to_string(),
        run_id,
        session_policy,
        demand,
        credentiald_runtime.as_deref(),
    )
    .await?;
    // Replace the shim config only after both gates and the credentiald binding have passed.
    // An unproven session leaves the pre-created fail-closed config in place. The files written
    // above already exist (`create_new`), so only `config.json` is replaced, atomically.
    if credential_admitted
        && let Err(error) = admit_shim_config(
            &runtime_dir,
            &session,
            policy,
            &action_policy,
            &approval_actions,
            true,
        )
    {
        stop_in_background(runtime).await;
        return Err(error);
    }
    tracing::info!(
        session = %runtime.session_id,
        launcher_proof = runtime.session_proof().is_some(),
        "browser launcher session started"
    );
    let runtime = Arc::new(runtime);
    // The launcher session is up; only now is the one-time approval spent.
    let approved_operation = match &approved {
        Some((wait, action)) => {
            let consumed = match sink.browser_operation_approval_consume(wait) {
                Ok(consumed) => consumed,
                Err(_) => {
                    let stop_runtime = Arc::clone(&runtime);
                    let _ = tokio::task::spawn_blocking(move || stop_runtime.stop()).await;
                    return Err(AdapterError::Other(
                        "browser approval could not be consumed".into(),
                    ));
                }
            };
            Some(super::ApprovedOperation {
                action: action.as_str().into(),
                origin: consumed.wait.origin,
                purpose: consumed.wait.purpose,
            })
        }
        None => None,
    };
    let mut browser = BrowserRun {
        task_id: req.task.id,
        run_id: run_id.into(),
        session_id: session,
        state: BrowserRunState::Running,
        live_view_url: None,
        policy: Some(policy.binding.clone()),
    };
    sink.browser_updated(&browser);
    // 付記 2026-10-10b: credential login の前に frame 接続を開く（auth section 中も本人向けの slot
    // は流れる。H3 の観測停止は下の `LiveEmitter` の auth guard が持ち、frame はそこを通らない）。
    // 版が 8 未満・開けない場合は理由だけを持ち、session と他の機能は続ける。
    let live_frames = live_registration(
        &runtime,
        target.socket,
        live_sessions,
        &browser.session_id,
        (req.task.id.to_string(), run_id.to_owned()),
    )
    .await;
    let live = crate::browser_live::LiveEmitter::new(EventSinkLive {
        sink,
        run_id: run_id.into(),
        session_id: browser.session_id.clone(),
    });
    let mut credential_used = false;
    let mut post_login_context = None;
    // ADR 2026-10-09 付記「launcher の Authenticate 経路」5: once the store records the auth
    // section it stays recorded (and worker events stay dropped) until the session has stopped.
    let mut auth_recorded = false;
    let mut auth_guard = None;
    if let Some(wait) = approved_credential.as_ref() {
        let refuse = |runtime: Arc<LauncherRuntime>, error: AdapterError| async move {
            let _ = tokio::task::spawn_blocking(move || runtime.stop()).await;
            Err(error)
        };
        if !credential_admitted {
            return refuse(runtime, credential_denied()).await;
        }
        // Version check before the approval is spent: an old launcher fails closed with a clear
        // message and the approval stays usable once the launcher is replaced. A pinned login with
        // a username field or a post-login read needs protocol 5 (ADR 2026-10-09 credential
        // username / post-login D1-5).
        let required = required_launcher_protocol(wait);
        let probe = Arc::clone(&runtime);
        let version = tokio::task::spawn_blocking(move || probe.protocol_version())
            .await
            .map_err(|_| unavailable())?;
        match version {
            Ok(v) if v >= required => {}
            Ok(v) => {
                tracing::error!(
                    launcher_protocol = v,
                    required,
                    "browser launcher is too old for credential login"
                );
                return refuse(runtime, launcher_too_old(v, required)).await;
            }
            Err(_) => return refuse(runtime, credential_denied()).await,
        }
        let (Some(sup), Some(credentiald_dir)) = (credentials, credentiald_runtime.as_deref())
        else {
            return refuse(runtime, credential_denied()).await;
        };
        // A credential-use approval is spent through the credential consume (the operation
        // consume refuses `credential_use` by design).
        let consumed = match sink.browser_approval_consume(wait) {
            Ok(consumed) => consumed,
            Err(_) => return refuse(runtime, credential_denied()).await,
        };
        let login = launcher_credential_login(
            &runtime,
            &consumed,
            post_login_plan.as_ref().map(|(read, _)| read),
            sup,
            credentiald_dir,
            &req.task.id.to_string(),
            run_id,
            &browser.session_id,
            sink,
        )
        .await;
        let live_session = runtime.session_id().to_owned();
        let mut login_observation: Result<LoginResult, &'static str> = Err("not_logged_in");
        let result = match login {
            CredentialLogin::NotRecorded => {
                let stop_runtime = runtime;
                let stopped = tokio::task::spawn_blocking(move || stop_runtime.stop())
                    .await
                    .is_ok_and(|r| r.is_ok());
                unregister_live_session(credentiald_dir, &live_session, stopped);
                browser.state = BrowserRunState::Failed;
                sink.browser_updated(&browser);
                return Ok(RunOutcome {
                    terminal: crate::Terminal::Error {
                        message: "browser auth section could not be recorded".into(),
                        retryable: true,
                    },
                    exit_code: None,
                });
            }
            CredentialLogin::Failed(code) => Err(code),
            CredentialLogin::Recorded(result) => {
                login_observation = result.clone();
                auth_recorded = true;
                // Nothing has been forwarded since the launcher stopped observation; from here
                // on every worker event of this session is dropped (ADR-0080 H3).
                auth_guard = Some(live.auth_section());
                result.map(|_| ())
            }
        };
        let status = if result.is_ok() { "success" } else { "failure" };
        sink.progress_with(
            &format!("browser.credential_use: {status}"),
            &task_core::ProgressFields {
                kind: Some(task_core::ProgressKind::ToolResult),
                tool: Some("browser.credential_use".into()),
                summary: Some(result.err().unwrap_or(status).into()),
                error: result.is_err(),
                ..Default::default()
            },
        );
        if let Err(code) = result {
            let stop_runtime = runtime;
            let stopped = tokio::task::spawn_blocking(move || stop_runtime.stop())
                .await
                .is_ok_and(|r| r.is_ok());
            unregister_live_session(credentiald_dir, &live_session, stopped);
            if stopped && auth_recorded {
                // The session has ended; only now may the auth interval be released.
                let _ = sink.browser_auth_section(run_id, &browser.session_id, false);
            }
            drop(auth_guard);
            browser.state = BrowserRunState::Failed;
            sink.browser_updated(&browser);
            return Ok(RunOutcome {
                terminal: crate::Terminal::Error {
                    message: format!(
                        "browser credential use failed ({code}){}",
                        if stopped {
                            ""
                        } else {
                            "; session cleanup failed"
                        }
                    ),
                    retryable: false,
                },
                exit_code: None,
            });
        }
        credential_used = true;
        // ADR 2026-10-09 credential username / post-login D2-2: the launcher closed the auth section
        // on the post-login conditions. Store (auth section closed; the credential-session mark keeps
        // takeover refused), worker events (Live View stays off: no live view URL) and the shim
        // policy follow, before the harness starts.
        if let (Ok(login), Some((read, bytes))) = (&login_observation, &post_login_plan) {
            let outcome = match (login.observation, login.held_reason) {
                (LoginObservation::Resumed, _) => {
                    let reopened = sink
                        .browser_auth_section(run_id, &browser.session_id, false)
                        .is_ok()
                        && super::replace_private(&runtime_dir.join("policy.json"), bytes).is_ok()
                        && admit_shim_config(
                            &runtime_dir,
                            &browser.session_id,
                            policy,
                            bytes,
                            &approval_actions,
                            true,
                        )
                        .is_ok();
                    if reopened {
                        Ok(login.consent_pressed)
                    } else {
                        Err(crate::browser_cdp_sink::PostLoginHeld::ResumeFailed.into())
                    }
                }
                // A launcher that held without a reason (or before v6) gives no detail.
                (LoginObservation::Held, held) => Err(crate::browser_cdp_sink::PostLoginHeldInfo {
                    reason: held.unwrap_or(crate::browser_cdp_sink::PostLoginHeld::CheckFailed),
                    consent_pressed: login.consent_pressed,
                    consent_controls: login.consent_controls.clone(),
                }),
            };
            if outcome.is_ok() {
                allowed = serde_json::from_slice(bytes)
                    .map_err(|_| AdapterError::Other("browser policy rejected".into()))?;
                post_login_context = Some(read.clone());
                drop(auth_guard.take());
            }
            super::post_login_progress(sink, &outcome);
        }
    }
    // 付記 2026-10-10e: screenshot / download need protocol v8 (artifact transfer). An older
    // launcher keeps the session but those verbs fail closed with a fixed reason.
    let probe = Arc::clone(&runtime);
    let launcher_protocol = tokio::task::spawn_blocking(move || probe.protocol_version())
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(0);
    if launcher_protocol < ARTIFACT_PROTOCOL
        && allowed
            .allow
            .iter()
            .any(|a| a == "screenshot" || a == "download")
    {
        tracing::warn!(
            launcher_protocol,
            required = ARTIFACT_PROTOCOL,
            "browser launcher cannot hand over screenshot/download files"
        );
        artifact_refusal_progress(sink, ARTIFACTS_REQUIRE_V8);
    }
    let executor = Arc::new(LauncherExecutor::new(
        Arc::clone(&runtime),
        output.clone(),
        launcher_protocol,
    ));
    let action_server = ActionServer::start_with(
        &action_socket,
        Arc::clone(&executor) as Arc<dyn crate::browser_action::ActionExecutor>,
        policy.allowed_domains().to_vec(),
        allowed.allow,
        approved_action
            .map(|a| a.upstream_actions().iter().map(|s| s.to_string()).collect())
            .unwrap_or_default(),
        control_gate,
    )
    .map_err(|_| unavailable())?;
    let events = runtime_dir.join("events.jsonl");
    let mut offset = 0;
    req.context.browser = Some(BrowserContext {
        run: browser.clone(),
        cli,
        credential_used,
        approval_actions,
        approved_operation,
        post_login: post_login_context,
    });
    let monitor_req = req.clone();
    let outcome = {
        let browser_sink = BrowserSink(sink);
        let future = adapter.run(req, run_id, limits, &browser_sink);
        tokio::pin!(future);
        let mut interval = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                result = &mut future => break result,
                _ = interval.tick() => forward_events(&events, &mut offset, &monitor_req, &output, sink, &live),
            }
        }
    };
    forward_events(&events, &mut offset, &monitor_req, &output, sink, &live);
    drop(action_server);
    let mut refusals = executor.take_refusals();
    refusals.dedup();
    for reason in refusals {
        artifact_refusal_progress(sink, reason);
    }
    // ADR 2026-10-09 付記: shim が run 後に残した request は daemon 経路と同じ共有段で durable wait
    // になる。順も daemon 経路と同じ — `credential-request.json` があれば**それを先に**処理して
    // `WaitingForAuth` wait（resume key `auth:<task>:<run>`、outcome は `Terminal::Question`）を開き、
    // `credential-request.json` が無いときだけ `approval-request.json` を見て `WaitingForApproval`
    // wait を開く（ADR 2026-10-08 D2）。policy に合わない request と wait を開けなかった場合は
    // wait を開かず `Err`（fail closed）。wait・event・log には origin / purpose / policy_id だけが
    // 入り、credential 値は入らない。
    let (outcome, wait_state) = super::shim_request_wait(
        &runtime_dir,
        monitor_req.task.id,
        run_id,
        &browser.session_id,
        policy,
        sink,
        outcome,
    );
    // registry から外し slot を閉じてから session を止める（停止の最初に外す。ADR-0108 D5）。
    drop(live_frames);
    let stop_runtime = Arc::clone(&runtime);
    let stopped = tokio::task::spawn_blocking(move || stop_runtime.stop())
        .await
        .map_err(|_| unavailable())
        .and_then(|r| r.map_err(|_| unavailable()));
    if credential_admitted && let Some(dir) = credentiald_runtime.as_deref() {
        unregister_live_session(dir, runtime.session_id(), stopped.is_ok());
    }
    let mut outcome = match stopped {
        Ok(_) => outcome,
        Err(_) => Ok(RunOutcome {
            terminal: crate::Terminal::Error {
                message:
                    "browser session cleanup failed; inspect the launcher session before retrying"
                        .into(),
                retryable: false,
            },
            exit_code: None,
        }),
    };
    if auth_recorded && stopped.is_ok() {
        // The session is gone; no observation can resume. A failed stop leaves the store's
        // auth section recorded (fail closed), as on the daemon path.
        if sink
            .browser_auth_section(run_id, &browser.session_id, false)
            .is_err()
        {
            outcome = Ok(RunOutcome {
                terminal: crate::Terminal::Error {
                    message: "browser auth section could not be closed after session end".into(),
                    retryable: true,
                },
                exit_code: None,
            });
        }
    }
    drop(auth_guard);
    browser.state = match wait_state {
        // A wait is open: the state follows the wait, whatever the harness reported.
        Some(state) => state,
        None => match &outcome {
            Ok(RunOutcome {
                terminal: crate::Terminal::Done { .. },
                ..
            }) => BrowserRunState::Completed,
            Ok(RunOutcome {
                terminal: crate::Terminal::Question { .. },
                ..
            }) => BrowserRunState::WaitingForHuman,
            _ => BrowserRunState::Failed,
        },
    };
    sink.browser_updated(&browser);
    outcome
}

/// screenshot / download を固定理由で失敗させたことの progress（理由だけ。file・URL は入れない）。
fn artifact_refusal_progress(sink: &dyn EventSink, reason: &'static str) {
    sink.progress_with(
        &format!("browser.artifact: {reason}"),
        &task_core::ProgressFields {
            kind: Some(task_core::ProgressKind::ToolResult),
            tool: Some("browser.artifact".into()),
            summary: Some(reason.into()),
            error: true,
            ..Default::default()
        },
    );
}

/// 付記 2026-10-10b: session の registry 登録と frame 中継を始める。attestation が無い（起動時の
/// 隔離検査を経ていない）session は登録しない。
async fn live_registration(
    runtime: &Arc<LauncherRuntime>,
    socket: &Path,
    live_sessions: Option<Arc<task_core::browser_isolation::LiveSessions>>,
    session_id: &str,
    live_key: (String, String),
) -> Option<crate::browser_live::LiveFrameRegistration> {
    let attestation = runtime.attestation.clone()?;
    let (probe, socket) = (Arc::clone(runtime), socket.to_path_buf());
    let relay = tokio::task::spawn_blocking(move || probe.open_live_frames(&socket))
        .await
        .unwrap_or(Err(crate::browser_live::LIVE_STREAM_UNAVAILABLE_REASON));
    if let Err(reason) = &relay {
        tracing::info!(
            session = %runtime.session_id,
            reason,
            "browser launcher Live View frames unavailable; the session continues without them"
        );
    }
    Some(crate::browser_live::LiveFrameRegistration::register(
        live_sessions,
        session_id,
        live_key,
        attestation,
        relay,
    ))
}

/// launcher の protocol が credential login（v4、username 欄・ログイン後の読み取りは v5）に足りない。
/// 承認は消費していない。
fn launcher_too_old(version: u32, required: u32) -> AdapterError {
    AdapterError::Other(format!(
        "browser launcher protocol {version} lacks the credential login verbs; rebuild and replace \
         celeris-browser-launcher (protocol {required} required)"
    ))
}

/// The launcher protocol the pinned login needs (ADR 2026-10-09 credential username / post-login
/// D1-5, 付記 2026-10-10): 6 with a post-login read, 5 with a username field only, else 4.
pub(crate) fn required_launcher_protocol(wait: &BrowserWait) -> u32 {
    match &wait.trusted_login {
        Some(t) if t.consent.is_some() => crate::browser_launcher::protocol::CONSENT_PROTOCOL,
        Some(t) if t.post_login.is_some() => POST_LOGIN_PROTOCOL,
        Some(t) if t.username_selector.is_some() => {
            crate::browser_launcher::protocol::USERNAME_PROTOCOL
        }
        _ => CREDENTIAL_LOGIN_PROTOCOL,
    }
}

/// credentiald から launcher session の登録を外す（session を止められたときだけ）。失敗は無視
/// （登録は credentiald の memory だけにあり、runtime の pid が消えれば注入は通らない）。
fn unregister_live_session(credentiald_runtime: &Path, session_id: &str, stopped: bool) {
    if stopped {
        let _ = crate::browser_cdp_sink::UnixInjectionClient::new(
            celeris_credentiald::injection_ipc::injection_socket(credentiald_runtime),
        )
        .unregister_live_session(session_id);
    }
}

/// [`launcher_credential_login`] の結果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CredentialLogin {
    /// store に auth section を記録できなかった（lease は revoke 済み、注入なし）。
    NotRecorded,
    /// auth section を記録する前に失敗した（lease を発行していれば revoke 済み、注入なし）。
    Failed(&'static str),
    /// store に auth section を記録した（解除は session の stop 後、または v5 で launcher が観測を
    /// 再開した後）。中身は login の成否と観測の再開の有無で、失敗なら lease は revoke 済み。
    Recorded(Result<LoginResult, &'static str>),
}

/// ADR 2026-10-09 付記「launcher の Authenticate 経路」: 承認を消費した後の launcher の login。
/// daemon 経路（`browser.rs` の credential segment）と同じ順で、
/// 1. credentiald の lease を launcher の稼働 session（`runtime.session_id`）に束縛して発行し、
/// 2. launcher に `auth_begin` させ（launcher の controller が agent の観測を止め、target を作る）、
/// 3. store に auth section を記録し、
/// 4. credentiald に auth section を開き（control は daemon だけが呼べる）、
/// 5. daemon が `injection.sock` に接続し、その接続を `authenticate` に付けて launcher に渡し、
/// 6. credentiald の auth section を閉じる。失敗なら lease を revoke する。
///
/// 秘密は credentiald → launcher の sink だけを通る。この関数・daemon・store・event に入るのは
/// id・origin・固定 code だけ。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn launcher_credential_login(
    runtime: &Arc<LauncherRuntime>,
    consumed: &task_core::browser_wait::ConsumedBrowserApproval,
    post_login: Option<&super::PostLoginRead>,
    sup: &crate::browser_credential::CredentialSupervisor,
    credentiald_runtime: &Path,
    task_id: &str,
    run_id: &str,
    session: &str,
    sink: &dyn EventSink,
) -> CredentialLogin {
    let Some(trusted) = consumed.trusted_login.as_ref() else {
        return CredentialLogin::Failed("trusted_selector_missing");
    };
    let lease = match crate::browser_credential::grant_h3_lease(
        sup,
        consumed,
        task_id,
        &runtime.session_id,
    ) {
        Ok(lease) => lease,
        Err(code) => return CredentialLogin::Failed(code),
    };
    let auth_id = format!("auth-{}", consumed.wait.wait_id);
    let begin = {
        let runtime = Arc::clone(runtime);
        let auth_id = auth_id.clone();
        tokio::task::spawn_blocking(move || runtime.auth_begin(&auth_id)).await
    };
    let target = match begin {
        Ok(Ok(target)) => target,
        _ => {
            sup.broker.revoke(&lease, "supervisor");
            return CredentialLogin::Failed("auth_begin_failed");
        }
    };
    if sink.browser_auth_section(run_id, session, true).is_err() {
        sup.broker.revoke(&lease, "supervisor");
        return CredentialLogin::NotRecorded;
    }
    let injection = celeris_credentiald::injection_ipc::injection_socket(credentiald_runtime);
    let broker = crate::browser_cdp_sink::UnixInjectionClient::new(injection.clone());
    let opened = broker
        .open_auth_section(
            celeris_credentiald::injection_ipc::AuthSectionRegistration {
                session_id: runtime.session_id.clone(),
                auth_section_id: auth_id.clone(),
                lease_id: lease.clone(),
                exact_origin: consumed.wait.origin.clone(),
                cdp_target_id: target,
            },
        )
        .is_ok();
    let mut result = if opened {
        let args = AuthenticateArgs {
            session_id: runtime.session_id.clone(),
            lease_id: runtime.lease_id.clone(),
            auth_section_id: auth_id.clone(),
            credential_lease_id: lease.clone(),
            origin: consumed.wait.origin.clone(),
            login_url: trusted.login_url.clone(),
            password_selector: trusted.password_selector.clone(),
            submit_selector: trusted.submit_selector.clone(),
            username_selector: trusted.username_selector.clone(),
            report_held_reason: post_login.is_some(),
            consent: post_login.and(trusted.consent.clone()),
            report_consent_controls: post_login.is_some(),
            // Only the effective read (site policy ∩ task policy) crosses to the launcher.
            post_login: post_login.map(|read| task_core::browser_wait::PostLogin {
                read_origins: read.read_origins.clone(),
                actions: read
                    .actions
                    .iter()
                    .filter_map(|a| {
                        task_core::browser_wait::PostLoginAction::ALL
                            .into_iter()
                            .find(|p| p.as_str() == a)
                    })
                    .collect(),
            }),
        };
        // This process connects, so credentiald admits the registered controller as the peer.
        match std::os::unix::net::UnixStream::connect(&injection) {
            Ok(stream) => {
                let runtime = Arc::clone(runtime);
                match tokio::task::spawn_blocking(move || {
                    runtime.authenticate(args, std::os::fd::OwnedFd::from(stream))
                })
                .await
                {
                    Ok(Ok((AuthenticationStatus::Success, observation))) => Ok(observation),
                    Ok(Ok((AuthenticationStatus::Rejected, _))) => {
                        Err("launcher_authenticate_rejected")
                    }
                    _ => Err("launcher_authenticate_failed"),
                }
            }
            Err(_) => Err("broker_unavailable"),
        }
    } else {
        Err("auth_section_open_failed")
    };
    if opened
        && broker
            .close_auth_section(&runtime.session_id, &auth_id)
            .is_err()
    {
        result = Err("auth_section_close_failed");
    }
    if result.is_err() {
        sup.broker.revoke(&lease, "supervisor");
    }
    CredentialLogin::Recorded(result)
}

#[cfg(test)]
#[path = "browser_launcher_run_tests.rs"]
mod tests;

/// 層横断の Live View 試験（`tests/browser_live_cross.rs`）専用: daemon が launcher session を持つ
/// 入口（[`LauncherRuntime`]）と frame 中継（[`open_frame_relay`]）を integration test から呼べる
/// ようにする薄い包み。dev-dependency の feature `live-cross-test-support` でだけ build される。
#[cfg(feature = "live-cross-test-support")]
pub mod cross_test_support {
    use std::path::Path;

    use task_core::browser_isolation::IsolationAttestation;

    use crate::browser_launcher::protocol::{AuthenticateArgs, AuthenticationStatus, LoginResult};
    use crate::browser_launcher::{
        ActionArgs, Observation, Receipt, SessionFacts, SessionPolicy, SessionState, Verb,
    };
    use crate::browser_live::FrameRelay;

    /// daemon 側の launcher session（[`super::LauncherRuntime`]）。試験専用 loopback の拒否はしない。
    pub struct DaemonLauncherSession(super::LauncherRuntime);

    impl DaemonLauncherSession {
        pub fn start(
            socket: &Path,
            task_id: &str,
            run_id: &str,
            policy: SessionPolicy,
        ) -> Result<(Self, IsolationAttestation), &'static str> {
            super::LauncherRuntime::start_guarded(socket, false, task_id, run_id, policy)
                .map(|(runtime, attestation)| (Self(runtime), attestation))
        }
        pub fn session_id(&self) -> &str {
            self.0.session_id()
        }
        /// daemon が採番した lease（frame 接続を生で開く試験が使う）。
        pub fn lease_id(&self) -> &str {
            &self.0.lease_id
        }
        pub fn protocol_version(&self) -> Result<u32, &'static str> {
            self.0.protocol_version()
        }
        /// daemon の Live View 開始（版確認 → v9 なら frame 接続）。
        pub fn open_live_frames(&self, socket: &Path) -> Result<FrameRelay, &'static str> {
            self.0.open_live_frames(socket)
        }
        pub fn action(
            &self,
            verb: Verb,
            args: ActionArgs,
        ) -> Result<(Receipt, Observation), &'static str> {
            self.0.action(verb, args)
        }
        pub fn observe(&self) -> Result<(SessionState, SessionFacts), &'static str> {
            self.0.observe()
        }
        pub fn auth_begin(&self, auth_section_id: &str) -> Result<String, &'static str> {
            self.0.auth_begin(auth_section_id)
        }
        /// credential login。`args` の session・lease は本 session のものに置き換える（本番の
        /// `credential_login` と同じ組み方）。
        pub fn authenticate(
            &self,
            mut args: AuthenticateArgs,
            broker: std::os::fd::OwnedFd,
        ) -> Result<(AuthenticationStatus, LoginResult), &'static str> {
            args.session_id = self.0.session_id.clone();
            args.lease_id = self.0.lease_id.clone();
            self.0.authenticate(args, broker)
        }
        pub fn stop(&self) -> Result<Option<Receipt>, &'static str> {
            self.0.stop()
        }
    }

    /// [`super::open_frame_relay`]（hello の版を引数で受ける frame 中継）。
    pub fn open_frame_relay(
        socket: &Path,
        launcher_protocol: u32,
        session_id: &str,
        lease_id: &str,
    ) -> Result<FrameRelay, &'static str> {
        super::open_frame_relay(socket, launcher_protocol, session_id, lease_id)
    }
}

/// 付記 2026-10-10b daemon 側の試験。偽 launcher は実の `LauncherServer` に frame を channel で
/// 渡す偽 backend を差したもの、v7 の launcher はその前に置いた `hello` の版を書き換える proxy。
/// userns・実 browser は使わない。待ちは出来事待ち（slot の `next`）と長い保険の期限だけ。
#[cfg(test)]
mod live_frame_tests {
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    use super::*;
    use crate::browser_launcher::protocol::{
        LiveEncoding, MAX_LIVE_BODY, Request, Response, read_frame, write_frame, write_message,
    };
    use crate::browser_launcher::{
        BackendSession, ErrorCode, Launched, LauncherLimits, LauncherServer, LiveFeed, LiveImage,
        LiveNext, Registry, ServerConfig, ServerHandle, SessionBackend, StartRequest,
    };
    use crate::browser_live::{
        CollectingSink, InMemoryGate, LIVE_NO_FRAMES_REASON, LiveEmitter, LiveFrameRegistration,
    };
    use task_core::browser_isolation::{LiveSessionRegistry, LiveSessions};
    use task_core::browser_live::LiveEvent;

    /// 長い保険（出来事待ちが来なかったときだけ効く）。
    const INSURANCE: Duration = Duration::from_secs(30);
    const LAUNCHER_UID: u32 = 4_000_001;
    const SUBUID: u32 = 5_000_000;
    const MARKER: &[u8] = b"CREDENTIAL-INPUT-hunter2-PIXELS";

    type Feeds = Arc<Mutex<Vec<Option<mpsc::Sender<LiveNext>>>>>;
    /// harness が run 中に見たもの: frame 購読口の有無・live key・slot で受け取った frame。
    type Seen = (bool, Option<(String, String)>, Option<Vec<u8>>);

    #[derive(Default)]
    struct LiveBackend {
        feeds: Feeds,
        live_opened: Arc<AtomicUsize>,
        actions: Arc<AtomicUsize>,
        stopped: Arc<AtomicUsize>,
    }

    struct ChannelFeed(mpsc::Receiver<LiveNext>);

    impl LiveFeed for ChannelFeed {
        fn next_frame(&mut self, wait: Duration) -> LiveNext {
            match self.0.recv_timeout(wait) {
                Ok(n) => n,
                Err(mpsc::RecvTimeoutError::Timeout) => LiveNext::Idle,
                Err(mpsc::RecvTimeoutError::Disconnected) => LiveNext::Ended,
            }
        }
    }

    struct LiveSession {
        child: Option<std::process::Child>,
        index: usize,
        feeds: Feeds,
        live_opened: Arc<AtomicUsize>,
        actions: Arc<AtomicUsize>,
        stopped: Arc<AtomicUsize>,
    }

    fn good_facts() -> SessionFacts {
        SessionFacts {
            host_uid: LAUNCHER_UID,
            host_gid: LAUNCHER_UID,
            uid_map: format!("0 {LAUNCHER_UID} 1\n1000 {SUBUID} 1\n"),
            gid_map: format!("0 {LAUNCHER_UID} 1\n1000 {SUBUID} 1\n"),
            ns_owner_uid: Some(LAUNCHER_UID),
            cap_eff: "0000000000000000".into(),
            no_new_privs: true,
            listen_count: 0,
        }
    }

    impl SessionBackend for LiveBackend {
        fn start(&self, _req: &StartRequest) -> Result<Launched, ErrorCode> {
            use std::os::unix::process::CommandExt;
            let child = std::process::Command::new("sleep")
                .arg("600")
                .process_group(0)
                .stdin(std::process::Stdio::null())
                .spawn()
                .map_err(|_| ErrorCode::LaunchFailed)?;
            let pid = child.id() as i32;
            let starttime =
                crate::browser_runtime::process_starttime(pid).ok_or(ErrorCode::LaunchFailed)?;
            let index = {
                let mut feeds = self.feeds.lock().expect("lock");
                feeds.push(None);
                feeds.len() - 1
            };
            Ok(Launched {
                session: Box::new(LiveSession {
                    child: Some(child),
                    index,
                    feeds: self.feeds.clone(),
                    live_opened: self.live_opened.clone(),
                    actions: self.actions.clone(),
                    stopped: self.stopped.clone(),
                }),
                pid,
                pgid: pid,
                starttime,
                runtime_pid: pid,
                runtime_starttime: starttime,
                ns_inodes: task_core::browser_isolation::collect_ns_inodes("self")
                    .map_err(|_| ErrorCode::LaunchFailed)?,
            })
        }
    }

    impl BackendSession for LiveSession {
        fn action(&mut self, verb: Verb, _args: &ActionArgs) -> Result<Observation, ErrorCode> {
            self.actions.fetch_add(1, Ordering::SeqCst);
            Ok(Observation {
                text: Some(format!(
                    r#"{{"success":true,"data":{{"verb":"{verb:?}"}}}}"#
                )),
                artifact: None,
            })
        }
        fn live(&mut self) -> Result<Box<dyn LiveFeed>, ErrorCode> {
            let (tx, rx) = mpsc::channel();
            self.feeds.lock().expect("lock")[self.index] = Some(tx);
            self.live_opened.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(ChannelFeed(rx)))
        }
        fn observe(&mut self) -> (SessionState, SessionFacts) {
            (SessionState::Running, good_facts())
        }
        fn isolation_ok(&mut self) -> bool {
            true
        }
        fn auth_begin(&mut self, _auth_section_id: &str) -> Result<String, ErrorCode> {
            Ok("TARGET-LOGIN".into())
        }
        fn stop(mut self: Box<Self>) {
            if let Some(mut c) = self.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            self.stopped.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Fixture {
        dir: tempfile::TempDir,
        sock: PathBuf,
        backend: Arc<LiveBackend>,
        _handle: ServerHandle,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("l.sock");
        let registry = Registry::open(dir.path().join("state"), "inst-live").expect("registry");
        let backend = Arc::new(LiveBackend::default());
        let server = LauncherServer::bind(
            &sock,
            ServerConfig {
                allowed_uids: vec![DaemonIds::current().uid],
                limits: LauncherLimits::default(),
            },
            backend.clone(),
            registry,
        )
        .expect("bind");
        let handle = server.spawn().expect("spawn");
        Fixture {
            dir,
            sock,
            backend,
            _handle: handle,
        }
    }

    fn policy() -> SessionPolicy {
        session_policy(
            &["navigate".into(), "snapshot".into(), "close".into()],
            &["example.com".into()],
            Duration::from_secs(600),
        )
    }

    /// 出来事待ち: 偽 backend の frame 接続（`live()`）が開くまで。
    fn feed(f: &Fixture, session: usize) -> mpsc::Sender<LiveNext> {
        let deadline = std::time::Instant::now() + INSURANCE;
        loop {
            if let Some(tx) = f.backend.feeds.lock().expect("lock")[session].clone() {
                return tx;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "live stream never opened"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn push(f: &Fixture, session: usize, body: &[u8]) {
        feed(f, session)
            .send(LiveNext::Frame(LiveImage::new(
                640,
                480,
                LiveEncoding::Jpeg,
                body.to_vec(),
            )))
            .expect("send frame");
    }

    async fn next_body(slot: &task_core::browser_live_frame::LatestFrameSlot) -> Option<Vec<u8>> {
        tokio::time::timeout(INSURANCE, slot.next())
            .await
            .expect("slot event")
            .map(|frame| frame.body().to_vec())
    }

    fn registry_entry(
        reg: &LiveSessions,
        session: &str,
    ) -> Arc<dyn task_core::browser_isolation::LiveSessionEntry> {
        reg.get(session).expect("registered")
    }

    // ---- v7 launcher: `hello` の版だけを書き換える proxy ----

    /// 偽 launcher の前に置く proxy。全接続の要求の種類を順に記録し、`rewrite` があれば `hello` の
    /// `protocol_version` をその値にする（v7 launcher の再現）。
    struct Proxy {
        sock: PathBuf,
        seen: Arc<Mutex<Vec<String>>>,
    }

    fn proxy(upstream: &Path, dir: &Path, rewrite: Option<u32>) -> Proxy {
        let sock = dir.join("p.sock");
        let listener = UnixListener::bind(&sock).expect("bind proxy");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (upstream, seen_t) = (upstream.to_path_buf(), Arc::clone(&seen));
        std::thread::spawn(move || {
            for client in listener.incoming() {
                let Ok(client) = client else { return };
                let Ok(server) = UnixStream::connect(&upstream) else {
                    return;
                };
                let (mut c_in, mut s_out) = (
                    client.try_clone().expect("clone"),
                    server.try_clone().expect("clone"),
                );
                let seen = Arc::clone(&seen_t);
                std::thread::spawn(move || {
                    while let Ok(body) = read_frame(&mut c_in, 4 * MAX_LIVE_BODY) {
                        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
                            let kind = v["type"].as_str().unwrap_or("?").to_owned();
                            seen.lock().expect("lock").push(kind);
                        }
                        if write_frame(&mut s_out, &body, 4 * MAX_LIVE_BODY).is_err() {
                            break;
                        }
                    }
                    let _ = s_out.shutdown(std::net::Shutdown::Both);
                });
                let (mut s_in, mut c_out) = (server, client);
                std::thread::spawn(move || {
                    while let Ok(mut body) = read_frame(&mut s_in, 4 * MAX_LIVE_BODY) {
                        if let (Some(v), Ok(mut json)) =
                            (rewrite, serde_json::from_slice::<serde_json::Value>(&body))
                            && json["type"] == "hello"
                        {
                            json["protocol_version"] = v.into();
                            body = serde_json::to_vec(&json).expect("json");
                        }
                        if write_frame(&mut c_out, &body, 4 * MAX_LIVE_BODY).is_err() {
                            break;
                        }
                    }
                    let _ = c_out.shutdown(std::net::Shutdown::Both);
                });
            }
        });
        Proxy { sock, seen }
    }

    // ---- 1. frame 接続の束縛と後始末 ----

    /// 偽の frame 接続: `live_started` は正しい session に答え、1 枚目は正しい session、2 枚目は
    /// 別 session の frame を流す。daemon は 1 枚目だけを slot に入れ、2 枚目で stream を終えて
    /// slot を閉じる（別 session の frame は誰にも届かない）。
    #[tokio::test]
    async fn browser_live_frame_session_mismatch_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("raw.sock");
        let listener = UnixListener::bind(&sock).expect("bind");
        // 2 枚目は 1 枚目を受け取った後に送る（close は保持中の frame を捨てるので順を固定する）。
        let (taken_tx, taken_rx) = mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("accept");
            let req = read_frame(&mut s, 1 << 20).expect("live_start");
            let req: Request = serde_json::from_slice(&req).expect("request");
            assert!(
                matches!(&req, Request::LiveStart { session_id, lease_id }
                    if session_id == "sess-A" && lease_id == "lease-A"),
                "{req:?}"
            );
            let send = |s: &mut UnixStream, session: &str, seq: u64, body: &[u8]| {
                write_message(
                    s,
                    &Response::LiveFrame {
                        session_id: session.into(),
                        seq,
                        width: 2,
                        height: 2,
                        encoding: LiveEncoding::Png,
                        body_len: body.len() as u64,
                    },
                    1 << 20,
                )
                .expect("meta");
                // The daemon closes the relay as soon as it rejects the foreign-session
                // metadata. That can race this body write, so BrokenPipe is the expected
                // outcome for the deliberately invalid second frame.
                let _ = write_frame(s, body, MAX_LIVE_BODY);
            };
            write_message(
                &mut s,
                &Response::LiveStarted {
                    session_id: "sess-A".into(),
                    max_body: MAX_LIVE_BODY as u64,
                },
                1 << 20,
            )
            .expect("started");
            send(&mut s, "sess-A", 1, b"OWN-FRAME");
            taken_rx.recv_timeout(INSURANCE).expect("first frame taken");
            send(&mut s, "sess-B", 2, b"OTHER-SESSION-FRAME");
            // 接続は開いたまま: 終わりは daemon 側の判定による。
            let mut rest = Vec::new();
            let _ = std::io::Read::read_to_end(&mut s, &mut rest);
        });
        let relay = tokio::task::spawn_blocking({
            let sock = sock.clone();
            move || open_frame_relay(&sock, 9, "sess-A", "lease-A")
        })
        .await
        .expect("join")
        .unwrap_or_else(|reason| panic!("relay: {reason}"));
        let slot = relay.slot();
        assert_eq!(next_body(&slot).await.as_deref(), Some(&b"OWN-FRAME"[..]));
        taken_tx.send(()).expect("signal");
        assert_eq!(
            next_body(&slot).await,
            None,
            "other session's frame closes the slot"
        );
        assert!(slot.is_closed());
        drop(relay);
        server.join().expect("server");
    }

    /// session の後始末: registry 登録の drop（run の終わり・早期 return）は entry を外して slot を
    /// 閉じ、session の stop は launcher 側の stream 終了で slot を閉じる。閉じた slot に後から来た
    /// frame は捨てる。
    #[tokio::test]
    async fn browser_live_frame_cleanup_closes_slot_on_drop_and_stop() {
        let f = fixture();
        let registry = Arc::new(LiveSessions::default());
        // (a) registration の drop。
        let (runtime, _) = LauncherRuntime::start(&f.sock, "t1", "r1", policy()).expect("start");
        let runtime = Arc::new(runtime);
        let reg = live_registration(
            &runtime,
            &f.sock,
            Some(Arc::clone(&registry)),
            "logical-1",
            ("t1".into(), "r1".into()),
        )
        .await
        .expect("registered");
        let entry = registry_entry(&registry, "logical-1");
        assert_eq!(entry.live_key(), Some(("t1".into(), "r1".into())));
        assert!(
            !entry.accepts_state(),
            "launcher path takes no identity state"
        );
        let slot = entry.live_frames().expect("v9 frames");
        push(&f, 0, b"FRAME-1");
        assert_eq!(next_body(&slot).await.as_deref(), Some(&b"FRAME-1"[..]));
        drop(reg);
        assert!(registry.get("logical-1").is_none(), "entry removed");
        assert!(slot.is_closed());
        assert!(entry.live_frames().is_none());
        assert!(
            entry.current_attestation().is_err(),
            "ended session has no attestation"
        );
        let _ = feed(&f, 0).send(LiveNext::Frame(LiveImage::new(
            1,
            1,
            LiveEncoding::Jpeg,
            b"LATE".to_vec(),
        )));
        assert!(slot.try_take().is_none(), "no frame after close");
        let stop = Arc::clone(&runtime);
        tokio::task::spawn_blocking(move || stop.stop())
            .await
            .expect("join")
            .expect("stop");

        // (b) session の stop（registration はまだ持っている）。
        let (runtime, _) = LauncherRuntime::start(&f.sock, "t2", "r2", policy()).expect("start");
        let runtime = Arc::new(runtime);
        let reg = live_registration(
            &runtime,
            &f.sock,
            Some(Arc::clone(&registry)),
            "logical-2",
            ("t2".into(), "r2".into()),
        )
        .await
        .expect("registered");
        let slot = registry_entry(&registry, "logical-2")
            .live_frames()
            .expect("v9 frames");
        push(&f, 1, b"FRAME-2");
        assert_eq!(next_body(&slot).await.as_deref(), Some(&b"FRAME-2"[..]));
        let stop = Arc::clone(&runtime);
        tokio::task::spawn_blocking(move || stop.stop())
            .await
            .expect("join")
            .expect("stop");
        assert_eq!(next_body(&slot).await, None, "session stop closes the slot");
        drop(reg);
        assert!(registry.is_empty());
    }

    /// auth section（credential 注入中）の daemon 側: `LiveEmitter` は auth guard の間 event を捨て、
    /// H3 の観測停止（restored の旗）は frame が流れても解けない。一方、本人向けの slot には auth
    /// section の login 画面の frame が届く（D3）。frame は emitter の sink に一切入らない。
    #[tokio::test]
    async fn browser_live_frame_auth_section_owner_only() {
        let f = fixture();
        let registry = Arc::new(LiveSessions::default());
        let (runtime, _) = LauncherRuntime::start(&f.sock, "t1", "r1", policy()).expect("start");
        let runtime = Arc::new(runtime);
        let reg = live_registration(
            &runtime,
            &f.sock,
            Some(Arc::clone(&registry)),
            "logical-auth",
            ("t1".into(), "r1".into()),
        )
        .await
        .expect("registered");
        let observation_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let emitter = LiveEmitter::with_observation_stop(
            CollectingSink::default(),
            Arc::clone(&observation_stop),
        );
        let guard = emitter.auth_section();
        let auth = Arc::clone(&runtime);
        let target = tokio::task::spawn_blocking(move || auth.auth_begin("auth-1"))
            .await
            .expect("join")
            .expect("auth_begin");
        assert_eq!(target, "TARGET-LOGIN");
        observation_stop.store(true, Ordering::SeqCst);
        let slot = registry_entry(&registry, "logical-auth")
            .live_frames()
            .expect("owner slot stays open in the auth section");
        push(&f, 0, MARKER);
        assert_eq!(next_body(&slot).await.as_deref(), Some(MARKER));
        assert!(!emitter.emit(&LiveEvent::Status {
            state: "typing".into()
        }));
        drop(guard);
        assert!(
            emitter.in_auth_section(),
            "H3 observation stop is not lifted by frames"
        );
        assert!(!emitter.emit(&LiveEvent::Status {
            state: "after".into()
        }));
        assert!(
            emitter.sink().events().is_empty(),
            "no event reached the persisted sink"
        );
        drop(reg);
        let stop = Arc::clone(&runtime);
        let _ = tokio::task::spawn_blocking(move || stop.stop()).await;
    }

    // ---- 2. 版確認と v7 互換 ----

    #[derive(Default)]
    struct RecordingSink {
        browsers: Mutex<Vec<BrowserRun>>,
        text: Mutex<String>,
    }

    impl RecordingSink {
        fn note(&self, s: &str) {
            let mut t = self.text.lock().expect("lock");
            t.push_str(s);
            t.push('\n');
        }
    }

    impl EventSink for RecordingSink {
        fn browser_control_gate(
            &self,
            _run_id: &str,
            _session_id: &str,
        ) -> Option<Arc<dyn crate::browser_live::ControlGate>> {
            Some(Arc::new(InMemoryGate::new()))
        }
        fn browser_updated(&self, browser: &BrowserRun) {
            self.note(&format!("{browser:?}"));
            self.browsers.lock().expect("lock").push(browser.clone());
        }
        fn browser_live(
            &self,
            _run_id: &str,
            _session_id: &str,
            event: &task_core::browser_live::ScrubbedLiveEvent,
        ) {
            self.note(&format!("{:?}", event.as_persisted()));
        }
        fn progress(&self, msg: &str) {
            self.note(msg);
        }
        fn progress_with(&self, msg: &str, fields: &task_core::ProgressFields) {
            self.note(&format!("{msg} {fields:?}"));
        }
        fn artifact(&self, artifact: &task_core::ArtifactRef) {
            self.note(&format!("{artifact:?}"));
        }
        fn comment(&self, body: &str) {
            self.note(body);
        }
    }

    /// harness の代わり: run 中に registry の entry を見て、frame 購読口があれば偽 backend に
    /// MARKER の frame を流し、本人向けの slot で受け取れたかを記録する。agent への応答（outcome）
    /// には frame を入れない。
    struct ProbeAdapter {
        registry: Arc<LiveSessions>,
        feeds: Feeds,
        seen: Arc<Mutex<Option<Seen>>>,
    }

    #[async_trait::async_trait]
    impl WorkerAdapter for ProbeAdapter {
        fn id(&self) -> &str {
            "fake"
        }
        async fn run(
            &self,
            req: RunRequest,
            _run_id: &str,
            _limits: RunLimits,
            _sink: &dyn EventSink,
        ) -> Result<RunOutcome, AdapterError> {
            let session = req
                .context
                .browser
                .as_ref()
                .map(|b| b.run.session_id.clone())
                .expect("browser context");
            let entry = self
                .registry
                .get(&session)
                .expect("registered during the run");
            let frames = entry.live_frames();
            let mut got = None;
            if let Some(slot) = &frames {
                let deadline = std::time::Instant::now() + INSURANCE;
                let tx = loop {
                    if let Some(tx) = self.feeds.lock().expect("lock")[0].clone() {
                        break tx;
                    }
                    assert!(std::time::Instant::now() < deadline, "live never opened");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                };
                tx.send(LiveNext::Frame(LiveImage::new(
                    8,
                    8,
                    LiveEncoding::Jpeg,
                    MARKER.to_vec(),
                )))
                .expect("send");
                got = next_body(slot).await;
            }
            *self.seen.lock().expect("lock") = Some((frames.is_some(), entry.live_key(), got));
            Ok(RunOutcome {
                terminal: crate::Terminal::Done {
                    summary: "harness finished".into(),
                    evidence: vec![],
                    usage: None,
                },
                exit_code: Some(0),
            })
        }
    }

    fn prepared() -> crate::browser_policy::PreparedBrowserPolicy {
        let grant = task_core::BrowserCapability {
            approval_actions: vec![],
            allowed_domains: vec!["example.com".into()],
            ..Default::default()
        };
        let task = task_core::BrowserTaskPolicy {
            policy_id: "launcher-live".into(),
            revision: 1,
            domain_mode: task_core::BrowserDomainMode::CommonHosts,
            navigation_origins: vec![],
            network_domains: vec!["example.com".into()],
            allowed_actions: vec![
                task_core::BrowserAction::Navigate,
                task_core::BrowserAction::Snapshot,
            ],
            approval_actions: vec![],
            credential_policy_ids: vec![],
            artifact_policy_id: None,
        };
        crate::browser_policy::prepare(&grant, Some(&task), super::super::SUPPORTED_VERSION)
            .expect("policy")
    }

    fn request(workspace: &Path) -> RunRequest {
        let mut task = crate::protocol::tests::sample_task();
        task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
        RunRequest {
            protocol: crate::protocol::PROTOCOL_VERSION,
            task,
            workspace: workspace.into(),
            work_dir: None,
            artifacts_dir: workspace.join("artifacts"),
            context: Default::default(),
            cargo_target_dir: None,
        }
    }

    struct RunResult {
        outcome: RunOutcome,
        seen: Seen,
        requests: Vec<String>,
        sink: RecordingSink,
        registry: Arc<LiveSessions>,
        workspace: tempfile::TempDir,
    }

    /// 偽 launcher（`rewrite` があれば v7 に見せる proxy 越し）で launcher 経路の run を最後まで通す。
    async fn run_through(f: &Fixture, rewrite: Option<u32>) -> RunResult {
        let p = proxy(&f.sock, f.dir.path(), rewrite);
        let registry = Arc::new(LiveSessions::default());
        let seen = Arc::new(Mutex::new(None));
        let workspace = tempfile::tempdir().expect("workspace");
        let sink = RecordingSink::default();
        let policy = prepared();
        let outcome = run_registered(
            Arc::new(ProbeAdapter {
                registry: Arc::clone(&registry),
                feeds: f.backend.feeds.clone(),
                seen: Arc::clone(&seen),
            }),
            request(workspace.path()),
            "run-live",
            RunLimits {
                wall_clock: Duration::from_secs(60),
                idle_timeout: Duration::from_secs(60),
                kill_grace: Duration::from_secs(1),
            },
            &sink,
            LauncherTarget {
                socket: &p.sock,
                refuse_test_loopback: false,
                launcher_uid: None,
            },
            &policy,
            None,
            Some(Arc::clone(&registry)),
        )
        .await
        .expect("launcher run");
        let seen = seen.lock().expect("lock").take().expect("harness ran");
        let requests = p.seen.lock().expect("lock").clone();
        RunResult {
            outcome,
            seen,
            requests,
            sink,
            registry,
            workspace,
        }
    }

    fn states(sink: &RecordingSink) -> Vec<BrowserRunState> {
        sink.browsers
            .lock()
            .expect("lock")
            .iter()
            .map(|b| b.state)
            .collect()
    }

    /// 互換表 v9 daemon × v8 launcher: `hello` が 8 なら `live_start` を送らず（frame 接続を開かず）、
    /// entry は frame 購読口なしで理由 `launcher_protocol_no_live_frames` を持つ。session・action・
    /// harness・stop は従来どおり。
    #[tokio::test]
    async fn browser_launcher_v8_continues_without_live_view() {
        let f = fixture();
        let r = run_through(&f, Some(8)).await;
        assert!(
            matches!(r.outcome.terminal, crate::Terminal::Done { .. }),
            "{:?}",
            r.outcome.terminal
        );
        let (has_frames, key, got) = r.seen;
        assert!(!has_frames, "a v8 launcher gives no frame slot");
        assert!(key.is_some(), "the session is still registered for the run");
        assert_eq!(got, None);
        assert!(r.requests.iter().any(|t| t == "hello"));
        assert!(
            !r.requests.iter().any(|t| t == "live_start"),
            "no live_start reaches a v8 launcher: {:?}",
            r.requests
        );
        assert_eq!(f.backend.live_opened.load(Ordering::SeqCst), 0);
        assert_eq!(
            f.backend.stopped.load(Ordering::SeqCst),
            1,
            "session stopped"
        );
        assert_eq!(
            states(&r.sink),
            vec![BrowserRunState::Running, BrowserRunState::Completed]
        );
        assert!(r.registry.is_empty(), "entry removed at the end of the run");

        // 理由と他の機能（同じ v8 launcher の session で action が通る）。
        let again = f.dir.path().join("again");
        std::fs::create_dir(&again).expect("dir");
        let p = proxy(&f.sock, &again, Some(8));
        let (runtime, attestation) =
            LauncherRuntime::start(&p.sock, "t9", "r9", policy()).expect("start");
        let relay = runtime.open_live_frames(&p.sock);
        assert_eq!(relay.as_ref().err(), Some(&LIVE_NO_FRAMES_REASON));
        let reg = LiveFrameRegistration::register(
            None,
            "logical-9",
            ("t9".into(), "r9".into()),
            attestation,
            relay,
        );
        assert_eq!(
            reg.entry().unavailable_reason(),
            Some(LIVE_NO_FRAMES_REASON)
        );
        let (_, obs) = runtime
            .action(Verb::Snapshot, ActionArgs::default())
            .expect("action on a v8 launcher");
        assert!(obs.text.is_some());
        assert!(
            !p.seen
                .lock()
                .expect("lock")
                .iter()
                .any(|t| t == "live_start")
        );
    }

    /// daemon は Live View を有効にする前に同じ session の control 接続で `hello` の版を確かめる:
    /// v9 では `hello` の後に `live_start` が 1 回だけ行き、v8 では `hello` だけで `live_start` は無い。
    #[tokio::test]
    async fn browser_launcher_daemon_checks_live_protocol_before_enable() {
        for (rewrite, expect_live) in [(None, true), (Some(7), false), (Some(8), false)] {
            let f = fixture();
            let r = run_through(&f, rewrite).await;
            let hello = r.requests.iter().position(|t| t == "hello");
            let starts: Vec<_> = r
                .requests
                .iter()
                .enumerate()
                .filter(|(_, t)| *t == "live_start")
                .map(|(i, _)| i)
                .collect();
            assert!(hello.is_some(), "{:?}", r.requests);
            if expect_live {
                assert_eq!(starts.len(), 1, "{:?}", r.requests);
                assert!(hello < starts.first().copied(), "{:?}", r.requests);
                assert!(r.seen.0, "v8 gives the owner a frame slot");
            } else {
                assert!(starts.is_empty(), "{:?}", r.requests);
                assert!(!r.seen.0);
            }
            assert!(matches!(r.outcome.terminal, crate::Terminal::Done { .. }));
        }
    }

    fn files_containing(root: &Path, needle: &[u8]) -> Vec<PathBuf> {
        let mut hits = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let path = e.path();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    stack.push(path);
                } else if ft.is_file()
                    && std::fs::read(&path)
                        .is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle))
                {
                    hits.push(path);
                }
            }
        }
        hits
    }

    /// login 画面の入力が写った frame（MARKER）は本人向けの slot にだけ届き、run の events・
    /// progress・browser 更新・live event・comment・outcome（agent への tool result）・run の作業
    /// 場所の file（shim の log・events.jsonl・artifacts）・launcher の CDP 応答（action の観測）の
    /// どこにも現れない。
    #[tokio::test]
    async fn launcher_credential_input_not_in_events_cdp_response_or_logs() {
        let f = fixture();
        let r = run_through(&f, None).await;
        let (has_frames, _, got) = &r.seen;
        assert!(has_frames);
        assert_eq!(got.as_deref(), Some(MARKER), "the owner slot got the frame");
        let marker = std::str::from_utf8(MARKER).expect("utf8");
        let recorded = r.sink.text.lock().expect("lock").clone();
        assert!(!recorded.is_empty());
        assert!(!recorded.contains(marker), "{recorded}");
        assert!(!format!("{:?}", r.outcome).contains(marker));
        assert_eq!(
            files_containing(r.workspace.path(), MARKER),
            Vec::<PathBuf>::new()
        );
        assert_eq!(
            files_containing(f.dir.path(), MARKER),
            Vec::<PathBuf>::new()
        );

        // CDP 応答（launcher の action の観測）にも frame は混ざらない。
        let (runtime, _) = LauncherRuntime::start(&f.sock, "t3", "r3", policy()).expect("start");
        let runtime = Arc::new(runtime);
        let registry = Arc::new(LiveSessions::default());
        let _reg = live_registration(
            &runtime,
            &f.sock,
            Some(Arc::clone(&registry)),
            "logical-3",
            ("t3".into(), "r3".into()),
        )
        .await
        .expect("registered");
        let session = f.backend.feeds.lock().expect("lock").len() - 1;
        push(&f, session, MARKER);
        let slot = registry_entry(&registry, "logical-3")
            .live_frames()
            .expect("frames");
        assert_eq!(next_body(&slot).await.as_deref(), Some(MARKER));
        let act = Arc::clone(&runtime);
        let (receipt, obs) =
            tokio::task::spawn_blocking(move || act.action(Verb::Snapshot, ActionArgs::default()))
                .await
                .expect("join")
                .expect("action");
        let shown = format!("{receipt:?} {obs:?}");
        assert!(!shown.contains(marker), "{shown}");
        assert!(!format!("{:?}", slot).contains(marker));
        let stop = Arc::clone(&runtime);
        let _ = tokio::task::spawn_blocking(move || stop.stop()).await;
    }
}
