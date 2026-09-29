//! ADR-0084 P4-A: isolated browser runtime の契約・検査・egress 判定・orphan 回収。
//!
//! I/O も時計も持たない。runtime の起動側（worker）が観測した事実（[`RuntimeFacts`]）を渡し、
//! ここが「隔離が証明されたか」を決める。証明の結果 [`IsolationAttestation`] は
//! [`verify_isolation`] からしか作れず、これだけが `Isolation::Isolated` を名乗れる
//! （ADR-0083 D3: identity の復元は隔離下でのみ）。

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

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
    RootUid,
    MissingNamespace { ns: Namespace },
    RootWritable,
    WritableOutsideSession { path: String },
    BrokerVisible { path: String },
    HostIpcVisible { path: String },
    CdpOnTcp,
    CdpOutsideControllerDir,
    PrivilegesKept,
    InvalidSession,
    NoProcessGroup,
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
    pgid: i32,
}

impl IsolationAttestation {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn runtime_uid(&self) -> u32 {
        self.runtime_uid
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
    if v.is_empty() {
        Ok(IsolationAttestation {
            session_id: facts.session_id.clone(),
            runtime_uid: facts.runtime_uid,
            pgid: facts.pgid,
        })
    } else {
        Err(v)
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
    // ADR-0086: only ordinary global unicast can be enabled; reject IETF
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
            if looks_like_ip_literal(host) {
                return Err(EgressDenied::IpLiteral);
            }
            if !valid_host(host) {
                return Err(EgressDenied::InvalidHost);
            }
            if !policy.allow.contains(&format!("{host}:{port}")) {
                return Err(EgressDenied::NotAllowed);
            }
            if resolved.is_empty() {
                return Err(EgressDenied::Unresolved);
            }
            for ip in resolved {
                if is_non_public(*ip) {
                    return Err(EgressDenied::PrivateAddress);
                }
                if ip.is_ipv6() && !policy.allow_ipv6 && ip.to_canonical().is_ipv6() {
                    return Err(EgressDenied::Ipv6Disabled);
                }
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

/// 稼働中の隔離 session（ADR-0087 D5）。呼ぶたびに事実を採り直して検査する。
/// session が止まっていれば、または隔離に違反していれば attestation を返さない。
pub trait LiveIsolation {
    fn current_attestation(&self) -> Result<IsolationAttestation, Vec<IsolationViolation>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> RuntimeFacts {
        RuntimeFacts {
            session_id: "s1".into(),
            host_uid: 1000,
            runtime_uid: 200_001,
            namespaces: REQUIRED_NAMESPACES.into_iter().collect(),
            root_readonly: true,
            writable_mounts: vec!["/session/profile".into(), "/session/downloads".into()],
            visible_paths: vec!["/usr".into(), "/etc/ssl".into(), "/session".into()],
            cdp: CdpEndpoint::Pipe,
            no_new_privs: true,
            capabilities_dropped: true,
            pgid: 4242,
        }
    }

    fn violations(f: &RuntimeFacts) -> Vec<IsolationViolation> {
        verify_isolation(f).unwrap_err()
    }

    #[test]
    fn verified_runtime_is_isolated() {
        let a = verify_isolation(&good()).unwrap();
        assert_eq!(a.isolation(), Isolation::Isolated);
        assert_eq!(a.session_id(), "s1");
        assert_eq!(a.pgid(), 4242);
    }

    #[test]
    fn same_uid_and_root_are_rejected() {
        let mut f = good();
        f.runtime_uid = 1000;
        assert!(violations(&f).contains(&IsolationViolation::SameUid));
        f.runtime_uid = 0;
        assert!(violations(&f).contains(&IsolationViolation::RootUid));
    }

    #[test]
    fn every_namespace_is_required() {
        for ns in REQUIRED_NAMESPACES {
            let mut f = good();
            f.namespaces.remove(&ns);
            assert_eq!(
                violations(&f),
                vec![IsolationViolation::MissingNamespace { ns }]
            );
        }
    }

    #[test]
    fn root_must_be_readonly_and_writes_stay_in_session() {
        let mut f = good();
        f.root_readonly = false;
        assert_eq!(violations(&f), vec![IsolationViolation::RootWritable]);
        for bad in ["/home/u", "/sessionx", "/session/../etc", "session", "/tmp"] {
            let mut f = good();
            f.writable_mounts.push(bad.into());
            assert_eq!(
                violations(&f),
                vec![IsolationViolation::WritableOutsideSession { path: bad.into() }],
                "{bad}"
            );
        }
    }

    #[test]
    fn broker_and_host_ipc_are_not_visible() {
        for p in [
            "/run/celeris/credentiald",
            "/run/celeris/credentiald/sock",
            "/etc/celeris",
            "/run/celeris",
            "/var/lib/celeris",
        ] {
            let mut f = good();
            f.visible_paths.push(p.into());
            assert!(
                matches!(
                    violations(&f)[..],
                    [IsolationViolation::BrokerVisible { .. }]
                ),
                "{p}"
            );
        }
        for p in [
            "/run/user/1000",
            "/dev/shm",
            "/tmp/.X11-unix/X0",
            "/run/dbus",
            "/var/run/docker.sock",
        ] {
            let mut f = good();
            f.visible_paths.push(p.into());
            assert!(
                matches!(
                    violations(&f)[..],
                    [IsolationViolation::HostIpcVisible { .. }]
                ),
                "{p}"
            );
        }
    }

    #[test]
    fn cdp_must_not_be_on_tcp_or_outside_controller_dir() {
        let mut f = good();
        f.cdp = CdpEndpoint::Tcp {
            addr: "127.0.0.1:9222".into(),
        };
        assert_eq!(violations(&f), vec![IsolationViolation::CdpOnTcp]);
        f.cdp = CdpEndpoint::UnixSocket {
            host_path: "/tmp/cdp.sock".into(),
        };
        assert_eq!(
            violations(&f),
            vec![IsolationViolation::CdpOutsideControllerDir]
        );
        f.cdp = CdpEndpoint::UnixSocket {
            host_path: format!("{CONTROLLER_DIR_PREFIX}../x"),
        };
        assert_eq!(
            violations(&f),
            vec![IsolationViolation::CdpOutsideControllerDir]
        );
        f.cdp = CdpEndpoint::UnixSocket {
            host_path: format!("{CONTROLLER_DIR_PREFIX}s1/cdp.sock"),
        };
        assert!(verify_isolation(&f).is_ok());
    }

    #[test]
    fn privileges_and_pgid_are_checked() {
        let mut f = good();
        f.no_new_privs = false;
        f.pgid = 1;
        f.session_id = "../x".into();
        let v = violations(&f);
        assert!(v.contains(&IsolationViolation::PrivilegesKept));
        assert!(v.contains(&IsolationViolation::NoProcessGroup));
        assert!(v.contains(&IsolationViolation::InvalidSession));
    }

    #[test]
    fn bwrap_argv_unshares_everything_and_remounts_ro() {
        let a = bwrap_argv(
            200_001,
            "/var/lib/x/s1",
            "/run/celeris/browser-controller/s1",
        );
        for flag in [
            "--unshare-user",
            "--unshare-pid",
            "--unshare-net",
            "--unshare-ipc",
            "--unshare-uts",
            "--die-with-parent",
            "--new-session",
        ] {
            assert!(a.iter().any(|x| x == flag), "{flag}");
        }
        assert!(!a.iter().any(|x| x.contains("credentiald")));
        assert_eq!(a.last().map(String::as_str), Some("/"));
        assert!(a.windows(2).any(|w| w[0] == "--uid" && w[1] == "200001"));
    }

    fn policy() -> EgressPolicy {
        EgressPolicy {
            allow: ["example.com:443".to_string()].into_iter().collect(),
            resolver: "10.200.0.1".parse().unwrap(),
            allow_ipv6: false,
        }
    }

    fn connect(host: &str, ips: &[&str]) -> EgressRequest {
        EgressRequest::Connect {
            host: host.into(),
            port: 443,
            resolved: ips.iter().map(|s| s.parse().unwrap()).collect(),
        }
    }

    #[test]
    fn egress_allows_public_allowed_host() {
        assert_eq!(
            check_egress(&policy(), &connect("example.com", &["93.184.216.34"])),
            Ok(())
        );
    }

    #[test]
    fn egress_rejects_private_ranges_and_rebinding() {
        for ip in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "127.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "198.18.0.1",
            "240.0.0.1",
            "192.88.99.1",
        ] {
            assert_eq!(
                check_egress(&policy(), &connect("example.com", &["93.184.216.34", ip])),
                Err(EgressDenied::PrivateAddress),
                "{ip}"
            );
        }
    }

    #[test]
    fn egress_rejects_ipv6_private_and_disabled() {
        for ip in [
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "2002:a00:1::",
            "::a00:1",
            "2001:db8::1",
            "100::1",
            "100:0:0:1::1",
            "2001:2::1",
            "2001:20::1",
            "3fff:fff::1",
            "5f00::1",
            "4000::1",
        ] {
            assert_eq!(
                check_egress(&policy(), &connect("example.com", &[ip])),
                Err(EgressDenied::PrivateAddress),
                "{ip}"
            );
        }
        assert_eq!(
            check_egress(&policy(), &connect("example.com", &["2606:2800:220:1::1"])),
            Err(EgressDenied::Ipv6Disabled)
        );
        let mut p = policy();
        p.allow_ipv6 = true;
        for ip in ["100::1", "2001:2::1", "3fff:fff::1", "5f00::1", "4000::1"] {
            assert_eq!(
                check_egress(&p, &connect("example.com", &[ip])),
                Err(EgressDenied::PrivateAddress)
            );
        }
        assert_eq!(
            check_egress(&p, &connect("example.com", &["2606:2800:220:1::1"])),
            Ok(())
        );
        // mapped の公開 v4 は v4 として扱う
        assert_eq!(
            check_egress(
                &policy(),
                &connect("example.com", &["::ffff:93.184.216.34"])
            ),
            Ok(())
        );
    }

    #[test]
    fn egress_rejects_ip_literals_and_unlisted_hosts() {
        for h in [
            "127.0.0.1",
            "2130706433",
            "0x7f.1",
            "0177.0.0.1",
            "[::1]",
            "93.184.216.34",
        ] {
            assert_eq!(
                check_egress(&policy(), &connect(h, &["93.184.216.34"])),
                Err(EgressDenied::IpLiteral),
                "{h}"
            );
        }
        assert_eq!(
            check_egress(&policy(), &connect("evil.com", &["93.184.216.34"])),
            Err(EgressDenied::NotAllowed)
        );
        assert_eq!(
            check_egress(&policy(), &connect("EXAMPLE.com", &["93.184.216.34"])),
            Err(EgressDenied::InvalidHost)
        );
        assert_eq!(
            check_egress(&policy(), &connect("example.com.", &["93.184.216.34"])),
            Err(EgressDenied::InvalidHost)
        );
        assert_eq!(
            check_egress(&policy(), &connect("example.com", &[])),
            Err(EgressDenied::Unresolved)
        );
        let other_port = EgressRequest::Connect {
            host: "example.com".into(),
            port: 8443,
            resolved: vec!["93.184.216.34".parse().unwrap()],
        };
        assert_eq!(
            check_egress(&policy(), &other_port),
            Err(EgressDenied::NotAllowed)
        );
    }

    #[test]
    fn egress_rejects_dns_bypass_and_proxy_chain() {
        let p = policy();
        assert_eq!(
            check_egress(
                &p,
                &EgressRequest::Dns {
                    server: p.resolver,
                    port: 53
                }
            ),
            Ok(())
        );
        assert_eq!(
            check_egress(
                &p,
                &EgressRequest::Dns {
                    server: "8.8.8.8".parse().unwrap(),
                    port: 53
                }
            ),
            Err(EgressDenied::DnsBypass)
        );
        assert_eq!(
            check_egress(
                &p,
                &EgressRequest::Dns {
                    server: p.resolver,
                    port: 853
                }
            ),
            Err(EgressDenied::DnsBypass)
        );
        assert_eq!(
            check_egress(
                &p,
                &EgressRequest::UpstreamProxy {
                    host: "example.com".into(),
                    port: 443
                }
            ),
            Err(EgressDenied::ProxyChain)
        );
    }

    #[test]
    fn orphans_are_labelled_runtime_groups_without_live_session() {
        let procs = vec![
            ProcessRecord {
                pid: 10,
                pgid: 10,
                uid: 200_001,
                session_label: Some("dead".into()),
            },
            ProcessRecord {
                pid: 11,
                pgid: 10,
                uid: 200_001,
                session_label: Some("dead".into()),
            },
            ProcessRecord {
                pid: 20,
                pgid: 20,
                uid: 200_002,
                session_label: Some("live".into()),
            },
            ProcessRecord {
                pid: 30,
                pgid: 30,
                uid: 1000,
                session_label: Some("dead".into()),
            },
            ProcessRecord {
                pid: 40,
                pgid: 40,
                uid: 200_001,
                session_label: None,
            },
            ProcessRecord {
                pid: 50,
                pgid: 50,
                uid: 0,
                session_label: Some("dead".into()),
            },
            ProcessRecord {
                pid: 60,
                pgid: 60,
                uid: 300_000,
                session_label: Some("dead".into()),
            },
            ProcessRecord {
                pid: 70,
                pgid: 1,
                uid: 200_001,
                session_label: Some("dead".into()),
            },
        ];
        let live = ["live".to_string()].into_iter().collect();
        let uids = [200_001, 200_002].into_iter().collect();
        assert_eq!(orphan_groups(&procs, &live, &uids, 1000), vec![10]);
    }
}
