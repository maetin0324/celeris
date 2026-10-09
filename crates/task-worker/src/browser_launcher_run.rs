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
use crate::browser_launcher::protocol::{AuthenticateArgs, AuthenticationStatus};
use crate::browser_launcher::{
    ActionArgs, LauncherClient, Observation, Outcome, Receipt, SessionFacts, SessionPolicy,
    SessionState, StartedSession, Verb,
};
use crate::{AdapterError, EventSink, RunLimits, RunOutcome, RunRequest, WorkerAdapter};

pub(crate) const UNAVAILABLE: &str = "isolated_runtime_unavailable";

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
        let runtime = Self {
            client: Mutex::new(client),
            session_id: started.session_id.clone(),
            lease_id,
            proof,
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
        Ok((runtime, attestation))
    }

    pub(crate) fn action(
        &self,
        verb: Verb,
        args: ActionArgs,
    ) -> Result<(Receipt, Observation), &'static str> {
        let mut client = self.client.lock().map_err(|_| UNAVAILABLE)?;
        let (receipt, observation) = client
            .action(&self.session_id, &self.lease_id, verb, args)
            .map_err(|_| UNAVAILABLE)?;
        if !receipt.isolation_ok || receipt.session_id != self.session_id {
            return Err(UNAVAILABLE);
        }
        Ok((receipt, observation))
    }

    pub(crate) fn observe(&self) -> Result<(SessionState, SessionFacts), &'static str> {
        let mut client = self.client.lock().map_err(|_| UNAVAILABLE)?;
        client
            .observe(&self.session_id, &self.lease_id)
            .map_err(|_| UNAVAILABLE)
    }

    fn authenticate(&self, args: AuthenticateArgs) -> Result<AuthenticationStatus, &'static str> {
        self.client
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .authenticate(args)
            .map_err(|_| UNAVAILABLE)
    }

    /// daemon 側で照合できた launcher の session 証明。無ければ機密能力の admission は拒否される。
    pub(crate) fn session_proof(&self) -> Option<&LauncherSessionProof> {
        self.proof.as_ref()
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
/// screenshot / download の artifact は launcher の dir にあり daemon へは渡らないので、この経路
/// では失敗として返す（agent には opaque な失敗）。
pub(crate) struct LauncherExecutor {
    pub(crate) runtime: Arc<LauncherRuntime>,
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

/// shim（`celeris-browser.py`）が読む `policy.json`・`config.json` を書く。形は daemon 経路と同じ
/// （`policy_sha256` は `policy.json` の byte の sha256、`action_socket` は共通の短い path）。
/// shim の `load_policy` はこれが揃わないと全 action を拒否する。返すのは shim と action socket の path。
fn write_shim_files(
    runtime_dir: &Path,
    session: &str,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
    action_policy: &[u8],
    approval_actions: &[String],
    credential_admitted: bool,
) -> Result<(std::path::PathBuf, std::path::PathBuf), AdapterError> {
    use sha2::Digest;
    let output = runtime_dir.join("output");
    std::fs::create_dir_all(&output)?;
    let cli = runtime_dir.join("celeris-browser.py");
    let action_socket = crate::browser_action::action_socket_path(runtime_dir)?;
    write_private(&cli, CLI)?;
    write_private(&runtime_dir.join("policy.json"), action_policy)?;
    write_private(
        &runtime_dir.join("config.json"),
        serde_json::to_vec(&serde_json::json!({
            "session_id": session,
            "allowed_domains": policy.allowed_domains(),
            "output": output,
            "action_socket": action_socket,
            "policy_sha256": format!("{:x}", sha2::Sha256::digest(action_policy)),
            "credential_policy_ids": if credential_admitted { policy.effective.credential_policy_ids.clone() } else { BTreeSet::<String>::new() },
            "credential_use": credential_admitted,
            "approval_actions": approval_actions,
        }))?,
    )?;
    Ok((cli, action_socket))
}

/// launcher 経由の browser run。harness は従来と同じ shim（`celeris-browser.py`）を使い、
/// shim の action は daemon の [`ActionServer`] の検査と gate を通ってから launcher に頼まれる。
#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    adapter: Arc<dyn WorkerAdapter>,
    mut req: RunRequest,
    run_id: &str,
    limits: RunLimits,
    sink: &dyn EventSink,
    target: LauncherTarget<'_>,
    policy: &crate::browser_policy::PreparedBrowserPolicy,
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
    let approved_action = approved.as_ref().map(|(_, a)| *a);
    let session = match &approved {
        Some((wait, _)) => wait.session_id.clone(),
        None => super::session_id(req.task.id, run_id),
    };
    let action_policy = match approved_action {
        Some(action) => super::resumed_policy_bytes(policy, action)?,
        None => policy.action_policy.clone(),
    };
    let approval_actions = super::shim_approval_actions(policy, approved_action);
    let runtime_dir = req.workspace.join("runs").join(run_id).join("browser");
    let output = runtime_dir.join("output");
    let _ = write_shim_files(
        &runtime_dir,
        &session,
        policy,
        &action_policy,
        &approval_actions,
        false,
    )?;
    let allowed: task_core::AgentBrowserActionPolicy = serde_json::from_slice(&action_policy)
        .map_err(|_| AdapterError::Other("browser policy rejected".into()))?;
    let control_gate = sink
        .browser_control_gate(run_id, &session)
        .ok_or_else(|| AdapterError::Other("browser control store unavailable".into()))?;
    let session_policy =
        session_policy(&allowed.allow, policy.allowed_domains(), limits.wall_clock);
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
    // Rewrite the shim config only after both gates and the credentiald binding have passed.
    // An unproven session leaves the pre-created fail-closed config in place.
    let (cli, action_socket) = write_shim_files(
        &runtime_dir,
        &session,
        policy,
        &action_policy,
        &approval_actions,
        credential_admitted,
    )?;
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
    let mut credential_used = false;
    if let Some(wait) = approved_credential.as_ref() {
        if !credential_admitted {
            let stop_runtime = runtime;
            let _ = tokio::task::spawn_blocking(move || stop_runtime.stop()).await;
            return Err(credential_denied());
        }
        let consumed = sink
            .browser_operation_approval_consume(wait)
            .map_err(|_| credential_denied())?;
        let credential = consumed
            .wait
            .credential
            .as_ref()
            .ok_or_else(credential_denied)?;
        let trusted = consumed
            .wait
            .trusted_login
            .as_ref()
            .ok_or_else(credential_denied)?;
        let args = AuthenticateArgs {
            session_id: runtime.session_id.clone(),
            auth_section_id: credential.credential_id.clone(),
            lease_id: runtime.lease_id.clone(),
            origin: consumed.wait.origin.clone(),
            target: trusted.password_selector.clone(),
        };
        let status = runtime
            .authenticate(args)
            .map_err(|_| credential_denied())?;
        if status != AuthenticationStatus::Success {
            let stop_runtime = runtime;
            let _ = tokio::task::spawn_blocking(move || stop_runtime.stop()).await;
            return Err(credential_denied());
        }
        credential_used = true;
    }
    let action_server = ActionServer::start_with(
        &action_socket,
        Arc::new(LauncherExecutor {
            runtime: Arc::clone(&runtime),
        }),
        policy.allowed_domains().to_vec(),
        allowed.allow,
        approved_action
            .map(|a| a.upstream_actions().iter().map(|s| s.to_string()).collect())
            .unwrap_or_default(),
        control_gate,
    )
    .map_err(|_| unavailable())?;
    let mut browser = BrowserRun {
        task_id: req.task.id,
        run_id: run_id.into(),
        session_id: session,
        state: BrowserRunState::Running,
        live_view_url: None,
        policy: Some(policy.binding.clone()),
    };
    sink.browser_updated(&browser);
    let live = crate::browser_live::LiveEmitter::new(EventSinkLive {
        sink,
        run_id: run_id.into(),
        session_id: browser.session_id.clone(),
    });
    let events = runtime_dir.join("events.jsonl");
    let mut offset = 0;
    req.context.browser = Some(BrowserContext {
        run: browser.clone(),
        cli,
        credential_used,
        approval_actions,
        approved_operation,
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
    let stop_runtime = Arc::clone(&runtime);
    let stopped = tokio::task::spawn_blocking(move || stop_runtime.stop())
        .await
        .map_err(|_| unavailable())
        .and_then(|r| r.map_err(|_| unavailable()));
    let outcome = match stopped {
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

#[cfg(test)]
#[path = "browser_launcher_run_tests.rs"]
mod tests;
