//! ADR-0102 P4-A: isolated browser runtime の契約・検査・egress 判定・orphan 回収。
//!
//! I/O も時計も持たない。runtime の起動側（worker）が観測した事実（[`RuntimeFacts`]）を渡し、
//! ここが「隔離が証明されたか」を決める。証明の結果 [`IsolationAttestation`] は
//! [`verify_isolation`] からしか作れず、これだけが `Isolation::Isolated` を名乗れる
//! （ADR-0101 D3: identity の復元は隔離下でのみ）。

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use serde::{Deserialize, Serialize};

use crate::browser_identity::Isolation;

/// 隔離に要る namespace。全部が host と別であること。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Namespace {
    User,
    Pid,
    Net,
    Mount,
    Ipc,
    Uts,
}

pub const REQUIRED_NAMESPACES: [Namespace; 6] = [
    Namespace::User,
    Namespace::Pid,
    Namespace::Net,
    Namespace::Mount,
    Namespace::Ipc,
    Namespace::Uts,
];

/// CDP の出口。host の TCP port に出すことは許さない（別 UID の他 process から届く）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CdpEndpoint {
    /// `--remote-debugging-pipe`。controller だけが fd を持つ。
    Pipe,
    /// sandbox 内の unix socket。host 側の path は controller 専用 dir（0700）。
    UnixSocket { host_path: String },
    /// host の TCP（loopback を含む）。常に拒否。
    Tcp { addr: String },
}

/// runtime の起動側が観測した事実。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeFacts {
    pub session_id: String,
    /// daemon（broker / worker）の UID。
    pub host_uid: u32,
    /// sandbox の中の browser process の実 UID（host から見た値）。
    pub runtime_uid: u32,
    /// runtime user namespace の owner UID（daemon と同じ親 namespace で解釈）。
    #[serde(default)]
    pub userns_owner_uid: Option<u32>,
    /// host と別であると確認できた namespace。
    pub namespaces: BTreeSet<Namespace>,
    pub root_readonly: bool,
    /// 書ける mount（sandbox 内の path）。session の profile / download dir だけ。
    pub writable_mounts: Vec<String>,
    /// sandbox の中から見える path（bind mount の宛先）。
    pub visible_paths: Vec<String>,
    pub cdp: CdpEndpoint,
    /// no_new_privs と capability の全落とし。
    pub no_new_privs: bool,
    pub capabilities_dropped: bool,
    /// runtime の process group（orphan 回収の単位）。
    pub pgid: i32,
}

/// 隔離の違反。1 つでもあれば attestation を出さない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IsolationViolation {
    SameUid,
    OwnerUnknown,
    UsernsOwnedByDaemon,
    RootUid,
    MissingNamespace {
        ns: Namespace,
    },
    RootWritable,
    WritableOutsideSession {
        path: String,
    },
    BrokerVisible {
        path: String,
    },
    HostIpcVisible {
        path: String,
    },
    CdpOnTcp,
    CdpOutsideControllerDir,
    PrivilegesKept,
    InvalidSession,
    NoProcessGroup,
    /// ADR-0138 D-L: launcher の session 証明が無い（daemon 起動の runtime を含む）。
    LauncherProofMissing,
    /// ADR-0138 D-L: 証明はあるが検証に失敗した（不一致・採取不能）。
    LauncherProofInvalid {
        defect: LauncherProofDefect,
    },
}

/// launcher 証明の検証に失敗した理由（ADR-0138 D-L）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LauncherProofDefect {
    /// launcher の応答の送り手（`SCM_CREDENTIALS`）を採れなかった。
    PeerUidUnavailable,
    /// 証明の launcher UID が応答の送り手の UID・設定上の launcher UID と一致しない。
    LauncherUidMismatch,
    /// launcher UID が daemon の UID か root。
    LauncherUidPrivileged,
    /// `Receipt.isolation_ok` が偽。
    IsolationNotOk,
    /// 証明の owner UID が無い。
    OwnerUnknown,
    /// 証明の owner UID が daemon の UID。
    OwnerIsDaemon,
    /// 証明の owner UID が daemon 自身の採った owner UID と一致しない。
    OwnerMismatch,
    /// 証明の PID が `RuntimeFacts` を採った process ではない。
    PidMismatch,
    /// daemon が runtime の starttime を採れなかった（session 終了を含む）。
    StarttimeUnavailable,
    /// starttime が証明と一致しない（PID 再利用・process 入替え）。
    StarttimeMismatch,
    /// 証明の session・instance が admission の対象と一致しない。
    SessionMismatch,
}

/// sandbox の中の固定の path。起動側はここにだけ session の書き込みを置く。
pub const SESSION_ROOT: &str = "/session";
/// host 側で controller だけが読む dir の接頭辞（CDP socket の置き場）。
pub const CONTROLLER_DIR_PREFIX: &str = "/run/celeris/browser-controller/";

/// 見えてはいけない path（broker の socket・鍵・vault・host の IPC）。
const BROKER_PATHS: [&str; 3] = [
    "/run/celeris/credentiald",
    "/var/lib/celeris/credentiald",
    "/etc/celeris",
];
const HOST_IPC_PATHS: [&str; 5] = [
    "/run/user",
    "/tmp/.X11-unix",
    "/dev/shm",
    "/run/dbus",
    "/var/run/docker.sock",
];

fn under(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    path == root || path.starts_with(&format!("{root}/"))
}

fn normal(path: &str) -> Option<&str> {
    if !path.starts_with('/') || path.split('/').any(|c| c == ".." || c == ".") {
        return None;
    }
    Some(path)
}

/// 隔離が証明された session。[`verify_isolation`] 以外では作れない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolationAttestation {
    session_id: String,
    runtime_uid: u32,
    userns_owner_uid: u32,
    pgid: i32,
}

impl IsolationAttestation {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn runtime_uid(&self) -> u32 {
        self.runtime_uid
    }
    pub fn userns_owner_uid(&self) -> u32 {
        self.userns_owner_uid
    }
    pub fn pgid(&self) -> i32 {
        self.pgid
    }
    /// identity の利用に渡す隔離。attestation を持つ時だけ `Isolated`。
    pub fn isolation(&self) -> Isolation {
        Isolation::Isolated
    }
}

/// 観測した事実を検査する。違反は全部返す（最初の 1 つで止めない）。
pub fn verify_isolation(
    facts: &RuntimeFacts,
) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
    let mut v = Vec::new();
    if facts.session_id.is_empty()
        || !facts
            .session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        v.push(IsolationViolation::InvalidSession);
    }
    if facts.runtime_uid == 0 {
        v.push(IsolationViolation::RootUid);
    }
    if facts.runtime_uid == facts.host_uid {
        v.push(IsolationViolation::SameUid);
    }
    match facts.userns_owner_uid {
        None => v.push(IsolationViolation::OwnerUnknown),
        Some(owner) if owner == facts.host_uid => v.push(IsolationViolation::UsernsOwnedByDaemon),
        Some(_) => {}
    }
    for ns in REQUIRED_NAMESPACES {
        if !facts.namespaces.contains(&ns) {
            v.push(IsolationViolation::MissingNamespace { ns });
        }
    }
    if !facts.root_readonly {
        v.push(IsolationViolation::RootWritable);
    }
    for p in &facts.writable_mounts {
        if normal(p).is_none_or(|p| !under(p, SESSION_ROOT)) {
            v.push(IsolationViolation::WritableOutsideSession { path: p.clone() });
        }
    }
    for p in &facts.visible_paths {
        let Some(n) = normal(p) else {
            v.push(IsolationViolation::HostIpcVisible { path: p.clone() });
            continue;
        };
        if BROKER_PATHS
            .iter()
            .any(|b| under(n, b) || under(b, n) && n != "/")
        {
            v.push(IsolationViolation::BrokerVisible { path: p.clone() });
        } else if HOST_IPC_PATHS.iter().any(|b| under(n, b)) {
            v.push(IsolationViolation::HostIpcVisible { path: p.clone() });
        }
    }
    match &facts.cdp {
        CdpEndpoint::Pipe => {}
        CdpEndpoint::UnixSocket { host_path } => {
            if normal(host_path).is_none_or(|p| !p.starts_with(CONTROLLER_DIR_PREFIX)) {
                v.push(IsolationViolation::CdpOutsideControllerDir);
            }
        }
        CdpEndpoint::Tcp { .. } => v.push(IsolationViolation::CdpOnTcp),
    }
    if !facts.no_new_privs || !facts.capabilities_dropped {
        v.push(IsolationViolation::PrivilegesKept);
    }
    if facts.pgid <= 1 {
        v.push(IsolationViolation::NoProcessGroup);
    }
    if let (true, Some(userns_owner_uid)) = (v.is_empty(), facts.userns_owner_uid) {
        Ok(IsolationAttestation {
            session_id: facts.session_id.clone(),
            runtime_uid: facts.runtime_uid,
            userns_owner_uid,
            pgid: facts.pgid,
        })
    } else {
        Err(v)
    }
}

/// ADR-0115 の launcher が `Response::Started` / `observe` で返す session 証明（ADR-0138 D-L）。
/// daemon 側が `Receipt`・`SessionFacts`・`SessionRecord` の値から組み立てる。
/// 持っているだけでは何も許さず、[`verify_launcher_session`] を通ったときだけ本番の
/// [`LauncherAttestation`] になる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LauncherSessionProof {
    pub session_id: String,
    pub instance_id: String,
    /// launcher 内の `SessionRecord.pid`（runtime の leader）。
    pub pid: i32,
    /// launcher 内の `SessionRecord.starttime`。
    pub starttime: u64,
    /// launcher が採った `SessionFacts.ns_owner_uid`。
    pub ns_owner_uid: Option<u32>,
    /// 証明を発行した launcher の UID。
    pub launcher_uid: u32,
    /// `Receipt.isolation_ok`（launcher 自身の `verify_isolation` の結果）。
    pub isolation_ok: bool,
    /// launcher が runtime（`pid`）で採った namespace の inode（protocol v3 の束縛）。daemon UID は
    /// 別 UID の runtime の `/proc/<pid>/ns/*` を開けない（EACCES）ので、namespace の別は
    /// これと daemon 自身の inode を比べて決める。欠けた namespace は「別」と数えない。
    #[serde(default)]
    pub ns_inodes: BTreeMap<Namespace, u64>,
}

/// admission 側（daemon）が自分で観測・設定から得た照合値。採取できない値は `None` のまま渡す
/// （安全な値に置き換えない。`None` は検証失敗になる）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherObservation {
    /// admission の対象 session。
    pub session_id: String,
    /// 対象 session を起動した launcher の instance id。
    pub instance_id: String,
    /// launcher の応答を書いた process の UID（kernel が付けた `SCM_CREDENTIALS`。ADR-0116 付記 D-P）。
    /// socket 起動では `SO_PEERCRED` が listen socket を作った systemd（uid 0）を指すので使わない。
    pub peer_uid: Option<u32>,
    /// 設定上の launcher UID。
    pub configured_launcher_uid: u32,
    /// `RuntimeFacts` を採った process の PID。
    pub runtime_pid: i32,
    /// daemon が `/proc/<runtime_pid>/stat` から採った starttime。
    pub runtime_starttime: Option<u64>,
}

/// 本番 admission の attestation（ADR-0138 条件 1・2・5）。隔離の検査と launcher 証明の検証の
/// 両方を通ったときだけ [`verify_launcher_session`] が作る。試験 harness からは作れない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherAttestation {
    isolation: IsolationAttestation,
    instance_id: String,
    pid: i32,
    starttime: u64,
    launcher_uid: u32,
}

impl LauncherAttestation {
    pub fn isolation_attestation(&self) -> &IsolationAttestation {
        &self.isolation
    }
    pub fn session_id(&self) -> &str {
        self.isolation.session_id()
    }
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
    pub fn pid(&self) -> i32 {
        self.pid
    }
    pub fn starttime(&self) -> u64 {
        self.starttime
    }
    pub fn launcher_uid(&self) -> u32 {
        self.launcher_uid
    }
    pub fn isolation(&self) -> Isolation {
        self.isolation.isolation()
    }
}

/// 証明の各条件を検査する。不一致は全部返す。
fn launcher_proof_defects(
    facts: &RuntimeFacts,
    proof: &LauncherSessionProof,
    seen: &LauncherObservation,
) -> Vec<LauncherProofDefect> {
    use LauncherProofDefect as D;
    let mut d = Vec::new();
    match seen.peer_uid {
        None => d.push(D::PeerUidUnavailable),
        Some(peer) if peer != proof.launcher_uid || peer != seen.configured_launcher_uid => {
            d.push(D::LauncherUidMismatch)
        }
        Some(_) => {}
    }
    if proof.launcher_uid == 0
        || proof.launcher_uid == facts.host_uid
        || seen.configured_launcher_uid == 0
        || seen.configured_launcher_uid == facts.host_uid
    {
        d.push(D::LauncherUidPrivileged);
    }
    if !proof.isolation_ok {
        d.push(D::IsolationNotOk);
    }
    match proof.ns_owner_uid {
        None => d.push(D::OwnerUnknown),
        Some(owner) if owner == facts.host_uid => d.push(D::OwnerIsDaemon),
        Some(owner) if facts.userns_owner_uid != Some(owner) => d.push(D::OwnerMismatch),
        Some(_) => {}
    }
    if proof.pid <= 1 || proof.pid != seen.runtime_pid {
        d.push(D::PidMismatch);
    }
    match seen.runtime_starttime {
        None => d.push(D::StarttimeUnavailable),
        Some(t) if t != proof.starttime => d.push(D::StarttimeMismatch),
        Some(_) => {}
    }
    if proof.session_id != seen.session_id
        || proof.session_id != facts.session_id
        || proof.instance_id != seen.instance_id
        || proof.instance_id.is_empty()
    {
        d.push(D::SessionMismatch);
    }
    d
}

/// 本番 admission の共通条件（ADR-0138 D-L）。[`verify_isolation`] の全条件に加えて launcher の
/// session 証明を要求する。証明が無ければ `LauncherProofMissing`、検証に失敗すれば
/// `LauncherProofInvalid` を返し、owner 検査が通っていても attestation を作らない。
pub fn verify_launcher_session(
    facts: &RuntimeFacts,
    proof: Option<&LauncherSessionProof>,
    seen: &LauncherObservation,
) -> Result<LauncherAttestation, Vec<IsolationViolation>> {
    let isolation = verify_isolation(facts);
    let mut v = isolation.as_ref().err().cloned().unwrap_or_default();
    let Some(proof) = proof else {
        v.push(IsolationViolation::LauncherProofMissing);
        return Err(v);
    };
    v.extend(
        launcher_proof_defects(facts, proof, seen)
            .into_iter()
            .map(|defect| IsolationViolation::LauncherProofInvalid { defect }),
    );
    match isolation {
        Ok(isolation) if v.is_empty() => Ok(LauncherAttestation {
            isolation,
            instance_id: proof.instance_id.clone(),
            pid: proof.pid,
            starttime: proof.starttime,
            launcher_uid: proof.launcher_uid,
        }),
        _ => Err(v),
    }
}

/// bubblewrap の argv（決定的）。起動方式の既定案。実際の起動と観測は worker が行い、
/// その結果の [`RuntimeFacts`] を [`verify_isolation`] で検査する。
pub fn bwrap_argv(
    runtime_uid: u32,
    session_dir_host: &str,
    controller_dir_host: &str,
) -> Vec<String> {
    let mut a: Vec<String> = [
        "bwrap",
        "--unshare-user",
        "--unshare-pid",
        "--unshare-net",
        "--unshare-ipc",
        "--unshare-uts",
        "--die-with-parent",
        "--new-session",
        "--cap-drop",
        "ALL",
        "--ro-bind",
        "/usr",
        "/usr",
        "--ro-bind",
        "/etc/ssl",
        "/etc/ssl",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    a.extend([
        "--uid".into(),
        runtime_uid.to_string(),
        "--bind".into(),
        session_dir_host.into(),
        SESSION_ROOT.into(),
        "--bind".into(),
        controller_dir_host.into(),
        "/run/cdp".into(),
        "--remount-ro".into(),
        "/".into(),
    ]);
    a
}

// ---- egress ----

/// egress の許可。browser の network namespace の唯一の出口（filtering proxy）が使う。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPolicy {
    /// 許す `host:port`（host は小文字の DNS 名）。IP literal は入れられない。
    pub allow: BTreeSet<String>,
    /// 解決に使ってよい resolver（proxy 自身）。これ以外の DNS は拒否。
    pub resolver: IpAddr,
    /// IPv6 の宛先を許すか。既定 false（v6 の private 判定漏れを避ける）。
    pub allow_ipv6: bool,
    /// 試験専用の loopback 許可（`127.0.0.1:<port>` の完全一致のみ）。既定は空で、空なら何も変わらない。
    /// 入れられるのは launcher の root 所有 config だけ（ADR 2026-10-05-browser-department-web-live-view 付記 E1）。
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub test_loopback_allow: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EgressRequest {
    /// proxy に来た CONNECT / 直接の接続。`resolved` は proxy が自分で解決した結果。
    Connect {
        host: String,
        port: u16,
        resolved: Vec<IpAddr>,
    },
    /// sandbox から直接の DNS 問い合わせ。
    Dns { server: IpAddr, port: u16 },
    /// 上位 proxy への連鎖（環境変数 / PAC / 設定で別の proxy を指す）。
    UpstreamProxy { host: String, port: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressDenied {
    NotAllowed,
    IpLiteral,
    PrivateAddress,
    Ipv6Disabled,
    Unresolved,
    DnsBypass,
    ProxyChain,
    InvalidHost,
}

/// private / loopback / link-local / metadata / CGNAT / 予約。
pub fn is_non_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => non_public_v4(v4),
        IpAddr::V6(v6) => non_public_v6(v6),
    }
}

fn non_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 192 && o[1] == 88 && o[2] == 99)
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
        || o[0] >= 240
}

fn non_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return non_public_v4(v4);
    }
    let s = ip.segments();
    // ADR-0104: only ordinary global unicast can be enabled; reject IETF
    // special assignments conservatively, including their anycast exceptions.
    (s[0] & 0xe000) != 0x2000
        || (s[0] == 0x2001 && s[1] < 0x0200)
        || (s[0] == 0x3fff && s[1] < 0x1000)
        || ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] & 0xffc0) == 0xfec0
        || s[0] == 0x2001 && s[1] == 0x0db8
        // NAT64 / 6to4 / Teredo は中の v4 が読めないので拒否。
        || (s[0] == 0x0064 && s[1] == 0xff9b)
        || s[0] == 0x2002
        || (s[0] == 0x2001 && s[1] == 0)
        // v4-compatible（::a.b.c.d）
        || (s[..6].iter().all(|x| *x == 0))
}

/// host が IP literal か（10 進・8 進・16 進の v4 の変種も含む）。
fn looks_like_ip_literal(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    if h.parse::<IpAddr>().is_ok() {
        return true;
    }
    // 2130706433 / 0x7f.1 / 0177.0.0.1 のような変種
    !h.is_empty()
        && h.split('.').all(|p| {
            !p.is_empty()
                && (p.chars().all(|c| c.is_ascii_digit())
                    || (p.starts_with("0x") && p[2..].chars().all(|c| c.is_ascii_hexdigit())))
        })
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host == host.to_ascii_lowercase()
        && !host.ends_with('.')
        && host.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// 試験専用 loopback 許可に当たる宛先。host が文字列として `127.0.0.1` そのもので、port が 0・53・853
/// 以外、かつ `127.0.0.1:<port>` が `test_loopback_allow` に完全一致するときだけ `Some`。
/// 当たれば DNS を引かずにこの宛先へつなぐ。
pub fn test_loopback_target(policy: &EgressPolicy, host: &str, port: u16) -> Option<SocketAddr> {
    if policy.test_loopback_allow.is_empty()
        || host != "127.0.0.1"
        || matches!(port, 0 | 53 | 853)
        || !policy
            .test_loopback_allow
            .contains(&format!("127.0.0.1:{port}"))
    {
        return None;
    }
    Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
}

/// 検査済みの解決結果から接続先を 1 つ選ぶ。IPv6 無効なら v4（v4-mapped を含む正規形）だけから選ぶ。
pub fn egress_destination(policy: &EgressPolicy, resolved: &[IpAddr]) -> Option<IpAddr> {
    resolved
        .iter()
        .map(|ip| ip.to_canonical())
        .find(|ip| policy.allow_ipv6 || ip.is_ipv4())
}

/// `host:port` が許可に入るか。完全一致、または `*.<base>:<port>` の下位 host（apex は含まない）。
/// wildcard は origin 許可と同じ包含規則（ADR 2026-10-05-browser-allowed-origins、
/// ADR 2026-10-09-egress-wildcard-origin）。public suffix への wildcard は origin の解析で拒否済みだが、
/// ここでも同じ判定（`*.com`・`*.ac.jp` など）で認めない。port は完全一致だけ。
fn allow_covers(policy: &EgressPolicy, host: &str, port: u16) -> bool {
    if policy.allow.contains(&format!("{host}:{port}")) {
        return true;
    }
    let port_suffix = format!(":{port}");
    policy.allow.iter().any(|entry| {
        entry
            .strip_suffix(&port_suffix)
            .and_then(|pattern| pattern.strip_prefix("*."))
            .filter(|base| valid_host(base) && !crate::browser::public_suffix(base))
            .and_then(|base| host.strip_suffix(base))
            .and_then(|sub| sub.strip_suffix('.'))
            .is_some_and(|sub| !sub.is_empty())
    })
}

/// egress の判定。解決先は全部を検査する（DNS rebinding で 1 つだけ private も拒否）。
pub fn check_egress(policy: &EgressPolicy, req: &EgressRequest) -> Result<(), EgressDenied> {
    match req {
        EgressRequest::Dns { server, port } => {
            if *server == policy.resolver && *port == 53 {
                Ok(())
            } else {
                Err(EgressDenied::DnsBypass)
            }
        }
        EgressRequest::UpstreamProxy { .. } => Err(EgressDenied::ProxyChain),
        EgressRequest::Connect {
            host,
            port,
            resolved,
        } => {
            if test_loopback_target(policy, host, *port).is_some() {
                return Ok(());
            }
            if looks_like_ip_literal(host) {
                return Err(EgressDenied::IpLiteral);
            }
            if !valid_host(host) {
                return Err(EgressDenied::InvalidHost);
            }
            if !allow_covers(policy, host, *port) {
                return Err(EgressDenied::NotAllowed);
            }
            if resolved.is_empty() {
                return Err(EgressDenied::Unresolved);
            }
            // 非公開の判定は v4・v6 の全部に掛ける（rebinding で 1 つだけ private も拒否）。
            if resolved.iter().any(|ip| is_non_public(*ip)) {
                return Err(EgressDenied::PrivateAddress);
            }
            // IPv6 無効なら v6 の宛先へは決してつながない。公開 v6 が混ざる dual-stack の名前は
            // v4 だけで足りるので拒否しない（接続先は [`egress_destination`] が v4 から選ぶ）。
            // v4 が 1 つも無ければ拒否（ADR 2026-10-09-egress-wildcard-origin）。
            if egress_destination(policy, resolved).is_none() {
                return Err(EgressDenied::Ipv6Disabled);
            }
            Ok(())
        }
    }
}

// ---- orphan ----

/// host 上の process の観測。`session_label` は runtime 起動時に付けた印（cgroup 名など）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub pid: i32,
    pub pgid: i32,
    pub uid: u32,
    pub session_label: Option<String>,
}

/// 回収する process group。印が付き、runtime の UID で、生きている session に属さないもの。
/// daemon 自身の UID の process と印の無い process には触らない。
pub fn orphan_groups(
    procs: &[ProcessRecord],
    live_sessions: &BTreeSet<String>,
    runtime_uids: &BTreeSet<u32>,
    host_uid: u32,
) -> Vec<i32> {
    let mut out = BTreeSet::new();
    for p in procs {
        let Some(label) = &p.session_label else {
            continue;
        };
        if p.uid == host_uid || p.uid == 0 || !runtime_uids.contains(&p.uid) || p.pgid <= 1 {
            continue;
        }
        if !live_sessions.contains(label) {
            out.insert(p.pgid);
        }
    }
    out.into_iter().collect()
}

/// 稼働中の隔離 session（ADR-0105 D5）。呼ぶたびに事実を採り直して検査する。
/// session が止まっていれば、または隔離に違反していれば attestation を返さない。
pub trait LiveIsolation {
    fn current_attestation(&self) -> Result<IsolationAttestation, Vec<IsolationViolation>>;
}

/// 稼働中 session の runtime 種別（ADR-0108 D5）。`NotIsolated` の session には復元しない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Isolated,
    NotIsolated,
}

/// controller が開封済み state を受け取らなかった（ADR-0108 D5）。理由は持たない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateRejected;

/// registry に載る稼働中 session（ADR-0108 D5）。attestation は呼ぶたびに採り直す。
pub trait LiveSessionEntry: LiveIsolation + Send + Sync {
    fn kind(&self) -> RuntimeKind;
    /// controller（CDP 側）に開封済み state の投入口があるか。無ければ開封しない。
    fn accepts_state(&self) -> bool;
    /// 開封済み state を controller にだけ渡す。agent・HTTP 応答には出さない。
    fn deliver_state(&self, state: &[u8]) -> Result<(), StateRejected>;
    /// Live View の鍵（task_id, run_id）。復元は投入の前にこの鍵で session を観測停止として
    /// 記録する（ADR-0080 H3 / ADR-0101 D4）。無ければ記録できないので開封しない。
    fn live_key(&self) -> Option<(String, String)> {
        None
    }
}

/// 稼働中 session の索引（ADR-0108 D5）。登録・削除は runtime の supervisor だけが行う。
pub trait LiveSessionRegistry: Send + Sync {
    fn get(&self, session_id: &str) -> Option<std::sync::Arc<dyn LiveSessionEntry>>;
}

/// daemon が 1 つ作り、supervisor と API に配る registry。
#[derive(Default)]
pub struct LiveSessions {
    map: std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<dyn LiveSessionEntry>>>,
}

impl std::fmt::Debug for LiveSessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSessions")
            .field("len", &self.len())
            .finish()
    }
}

impl LiveSessions {
    pub fn insert(&self, session_id: &str, entry: std::sync::Arc<dyn LiveSessionEntry>) {
        if let Ok(mut m) = self.map.lock() {
            m.insert(session_id.to_owned(), entry);
        }
    }

    pub fn remove(&self, session_id: &str) {
        if let Ok(mut m) = self.map.lock() {
            m.remove(session_id);
        }
    }

    pub fn len(&self) -> usize {
        self.map.lock().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl LiveSessionRegistry for LiveSessions {
    fn get(&self, session_id: &str) -> Option<std::sync::Arc<dyn LiveSessionEntry>> {
        self.map.lock().ok()?.get(session_id).cloned()
    }
}

// ---- 事実採取（ADR-0109 D2: broker と worker で共有）----

const FACT_NAMESPACES: [(Namespace, &str); 6] = [
    (Namespace::User, "user"),
    (Namespace::Pid, "pid"),
    (Namespace::Net, "net"),
    (Namespace::Mount, "mnt"),
    (Namespace::Ipc, "ipc"),
    (Namespace::Uts, "uts"),
];

fn proc_status_field(status: &str, key: &str) -> Option<String> {
    status.lines().find_map(|l| {
        l.strip_prefix(key)?
            .strip_prefix(':')
            .map(|v| v.trim().to_owned())
    })
}

fn proc_real_uid(status: &str) -> Option<u32> {
    proc_status_field(status, "Uid")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// runtime の user namespace FD から owner UID を採る。取得不能なら不明として拒否させる。
pub fn collect_userns_owner_uid(pid: i32) -> Option<u32> {
    use std::os::fd::AsRawFd;

    // linux/nsfs.h: NS_GET_OWNER_UID = _IO(NSIO, 0x4), NSIO = 0xb7.
    const NS_GET_OWNER_UID: std::ffi::c_ulong = 0xb704;
    unsafe extern "C" {
        fn ioctl(fd: std::ffi::c_int, request: std::ffi::c_ulong, ...) -> std::ffi::c_int;
    }
    let ns = std::fs::File::open(format!("/proc/{pid}/ns/user")).ok()?;
    let mut owner: u32 = 0;
    // SAFETY: ns is an open namespace FD, and owner points to a writable uid_t.
    let result = unsafe { ioctl(ns.as_raw_fd(), NS_GET_OWNER_UID, &mut owner) };
    (result == 0).then_some(owner)
}

/// `/proc/<pid>/ns/<name>` の link（`user:[4026531837]`）から inode を採る。
fn ns_link_inode(path: &str) -> std::io::Result<u64> {
    let link = std::fs::read_link(path)?;
    link.to_str()
        .and_then(|l| l.rsplit_once(":["))
        .and_then(|(_, rest)| rest.strip_suffix(']'))
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| std::io::Error::other(format!("unexpected ns link: {path}")))
}

/// `pid` の 6 つの namespace の inode。1 つでも読めなければ Err（欠けを埋めない）。
/// launcher が自分の runtime に対して呼び、protocol v3 の束縛に載せる（ADR-0138 D-L）。
pub fn collect_ns_inodes(pid: &str) -> std::io::Result<BTreeMap<Namespace, u64>> {
    FACT_NAMESPACES
        .into_iter()
        .map(|(ns, name)| Ok((ns, ns_link_inode(&format!("/proc/{pid}/ns/{name}"))?)))
        .collect()
}

/// daemon UID でも読める `/proc/<pid>` の項目（`status` と `mountinfo`）から採った事実。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcView {
    pub runtime_uid: u32,
    pub no_new_privs: bool,
    pub capabilities_dropped: bool,
    pub root_readonly: bool,
    pub writable_mounts: Vec<String>,
    pub visible_paths: Vec<String>,
}

impl ProcView {
    /// `status` と `mountinfo` の本文から組む。読み取りは呼び出し側（Err はそこで返す）。
    /// 解釈できない値は隔離を否定する側（uid 0・書込み可）に倒す。
    pub fn parse(status: &str, mountinfo: &str) -> Self {
        let runtime_uid = proc_real_uid(status).unwrap_or(0);
        let no_new_privs = proc_status_field(status, "NoNewPrivs").as_deref() == Some("1");
        let zero =
            |k: &str| proc_status_field(status, k).is_some_and(|v| v.chars().all(|c| c == '0'));
        let capabilities_dropped = zero("CapEff") && zero("CapPrm");
        let mut root_readonly = false;
        let mut writable_mounts = Vec::new();
        let mut visible_paths = Vec::new();
        for line in mountinfo.lines() {
            let Some((pre, post)) = line.split_once(" - ") else {
                continue;
            };
            let f: Vec<&str> = pre.split_whitespace().collect();
            let fstype = post.split_whitespace().next().unwrap_or("");
            let (Some(mp), Some(opts)) = (f.get(4), f.get(5)) else {
                continue;
            };
            let ro = opts.split(',').any(|o| o == "ro");
            visible_paths.push((*mp).to_owned());
            if *mp == "/" {
                root_readonly = ro;
            }
            let pseudo = fstype == "proc" || *mp == "/dev" || mp.starts_with("/dev/");
            if !ro && !pseudo {
                writable_mounts.push((*mp).to_owned());
            }
        }
        Self {
            runtime_uid,
            no_new_privs,
            capabilities_dropped,
            root_readonly,
            writable_mounts,
            visible_paths,
        }
    }

    fn read(pid: i32) -> std::io::Result<Self> {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
        let mountinfo = std::fs::read_to_string(format!("/proc/{pid}/mountinfo"))?;
        Ok(Self::parse(&status, &mountinfo))
    }

    fn into_facts(
        self,
        session_id: &str,
        host_uid: u32,
        userns_owner_uid: Option<u32>,
        namespaces: BTreeSet<Namespace>,
        pgid: i32,
    ) -> RuntimeFacts {
        RuntimeFacts {
            session_id: session_id.to_owned(),
            host_uid,
            runtime_uid: self.runtime_uid,
            userns_owner_uid,
            namespaces,
            root_readonly: self.root_readonly,
            writable_mounts: self.writable_mounts,
            visible_paths: self.visible_paths,
            cdp: CdpEndpoint::Pipe,
            no_new_privs: self.no_new_privs,
            capabilities_dropped: self.capabilities_dropped,
            pgid,
        }
    }
}

fn host_uid() -> std::io::Result<u32> {
    let host_status = std::fs::read_to_string("/proc/self/status")?;
    proc_real_uid(&host_status).ok_or_else(|| std::io::Error::other("host uid unavailable"))
}

/// `/proc/<pid>` から runtime の事実を採る（呼び出し側 process を host とみなす）。
/// 読めない値は隔離を否定する側（uid 0・namespace 無し・書込み可）に倒す。
/// `/proc/<pid>/ns/*` を開くので、別 UID の runtime（launcher 起動）には daemon UID から使えない
/// （EACCES）。本番の broker は [`collect_launched_runtime_facts`] を使う。
pub fn collect_runtime_facts(
    session_id: &str,
    pid: i32,
    pgid: i32,
) -> std::io::Result<RuntimeFacts> {
    let mut namespaces = BTreeSet::new();
    for (ns, name) in FACT_NAMESPACES {
        let mine = std::fs::read_link(format!("/proc/self/ns/{name}"))?;
        let theirs = std::fs::read_link(format!("/proc/{pid}/ns/{name}"))?;
        if mine != theirs {
            namespaces.insert(ns);
        }
    }
    let host_uid = host_uid()?;
    Ok(ProcView::read(pid)?.into_facts(
        session_id,
        host_uid,
        collect_userns_owner_uid(pid),
        namespaces,
        pgid,
    ))
}

/// launcher 起動の runtime の事実を組む純関数（ADR-0138 D-L）。`status`・`mountinfo` は daemon が
/// 自分で読んだ値、namespace の別は launcher の束縛の inode と daemon 自身の inode の比較、
/// userns owner は launcher の束縛の値。束縛に inode が無い・daemon と同じ inode の namespace は
/// 「別」に数えないので `MissingNamespace` で拒否される（安全値で埋めない）。
pub fn launched_runtime_facts(
    session_id: &str,
    pgid: i32,
    host_uid: u32,
    own_ns: &BTreeMap<Namespace, u64>,
    view: ProcView,
    proof: &LauncherSessionProof,
) -> RuntimeFacts {
    let namespaces = REQUIRED_NAMESPACES
        .into_iter()
        .filter(|ns| match (own_ns.get(ns), proof.ns_inodes.get(ns)) {
            (Some(mine), Some(theirs)) => mine != theirs,
            _ => false,
        })
        .collect();
    view.into_facts(session_id, host_uid, proof.ns_owner_uid, namespaces, pgid)
}

/// 本番 broker の事実採取（ADR-0138 D-L）。daemon UID で読める `/proc/<pid>/{status,mountinfo}` と
/// 自分の `/proc/self/ns/*` を読み、読めない namespace・owner は launcher の束縛から採る。
/// どれかが読めなければ Err（呼び出し側は拒否する）。
pub fn collect_launched_runtime_facts(
    session_id: &str,
    pid: i32,
    pgid: i32,
    proof: &LauncherSessionProof,
) -> std::io::Result<RuntimeFacts> {
    let own_ns = collect_ns_inodes("self")?;
    let host_uid = host_uid()?;
    let view = ProcView::read(pid)?;
    Ok(launched_runtime_facts(
        session_id, pgid, host_uid, &own_ns, view, proof,
    ))
}

#[cfg(test)]
#[path = "browser_isolation/tests.rs"]
mod tests;
