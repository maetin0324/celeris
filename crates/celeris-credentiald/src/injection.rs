//! ADR-0084 P4-B: trusted injection の接点。
//!
//! [`CredentialProvider`] の契約（P2-B）は変えない。provider を呼べるのは
//! [`PeerRole::Injector`]（isolated runtime の controller 側）だけで、worker・agent は拒否する。
//! 注入は 2 段: [`TrustedInjector::prepare`] で対象（frame 鎖・document・要素）を固定し、
//! [`TrustedInjector::commit`] で直前に同じ検査をやり直してから（TOCTOU）秘密を sink に渡す。
//! 秘密は戻り値に出ない。注入後の観測は [`RedisplayGuard`] で秘密の再表示を検出して捨てる。
//! 認証区間（ADR-0080 H3）の外では注入しない。

use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::{AuthorizedLeaseContext, CredentialProvider, CredentialRef};

/// IPC の相手の役割。peer の UID から broker が決める（自己申告ではない）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerRole {
    /// isolated runtime の controller（CDP の pipe を持つ側）。
    Injector,
    /// task-worker（LLM の harness を回す側）。
    Worker,
    /// agent / harness の子 process。
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Username,
    Password,
}

/// 注入先の要素。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldInfo {
    pub element_id: String,
    pub kind: FieldKind,
    /// DOM 上の `type`（小文字）。
    pub input_type: String,
}

/// 注入先の frame の観測（controller が CDP で読む）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InjectionTarget {
    pub session_id: String,
    pub navigation_id: u64,
    pub document_id: String,
    /// top から注入先 frame までの origin（全部 exact_origin であること）。
    pub frame_chain: Vec<String>,
    /// この document に至るまでの redirect の origin（最後が現在の origin）。
    pub redirect_chain: Vec<String>,
    pub field: FieldInfo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InjectionDenied {
    WorkerNotAllowed,
    AuthSectionRequired,
    LeaseExpired,
    OtherSession,
    CrossOriginFrame,
    Redirected,
    EmptyFrameChain,
    RedisplayField,
    TargetChanged,
    Provider,
    Sink,
}

impl InjectionDenied {
    pub fn code(self) -> &'static str {
        match self {
            Self::WorkerNotAllowed => "injection_worker_not_allowed",
            Self::AuthSectionRequired => "auth_section_required",
            Self::LeaseExpired => "lease_expired",
            Self::OtherSession => "other_session",
            Self::CrossOriginFrame => "cross_origin_frame",
            Self::Redirected => "redirected",
            Self::EmptyFrameChain => "empty_frame_chain",
            Self::RedisplayField => "redisplay_field",
            Self::TargetChanged => "target_changed",
            Self::Provider => "provider_failed",
            Self::Sink => "sink_failed",
        }
    }
}

/// sink の失敗（理由は秘密を含み得るので持たない）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinkError;

/// 秘密を browser に入れる経路（CDP `Input.insertText` など）。controller だけが実装する。
pub trait InjectionSink {
    fn insert(&mut self, target: &InjectionTarget, value: &str) -> Result<(), SinkError>;
}

/// prepare の結果。1 回だけ commit できる（move で消費）。
#[derive(Debug)]
pub struct InjectionTicket {
    context: AuthorizedLeaseContext,
    reference: CredentialRef,
    target: InjectionTarget,
}

/// commit の結果。秘密は含まない。
#[derive(Debug, PartialEq, Eq)]
pub struct InjectionReceipt {
    pub lease_id: String,
    pub element_id: String,
    pub navigation_id: u64,
}

pub struct TrustedInjector<'p> {
    provider: &'p dyn CredentialProvider,
}

fn check_target(ctx: &AuthorizedLeaseContext, t: &InjectionTarget) -> Result<(), InjectionDenied> {
    if t.session_id != ctx.session_id {
        return Err(InjectionDenied::OtherSession);
    }
    if t.frame_chain.is_empty() {
        return Err(InjectionDenied::EmptyFrameChain);
    }
    if t.frame_chain.iter().any(|o| *o != ctx.exact_origin) {
        return Err(InjectionDenied::CrossOriginFrame);
    }
    if t.redirect_chain.iter().any(|o| *o != ctx.exact_origin) {
        return Err(InjectionDenied::Redirected);
    }
    // password を password 以外の input に入れると画面に再表示される。
    let ok_type = match t.field.kind {
        FieldKind::Password => t.field.input_type == "password",
        FieldKind::Username => matches!(t.field.input_type.as_str(), "text" | "email"),
    };
    if !ok_type {
        return Err(InjectionDenied::RedisplayField);
    }
    Ok(())
}

impl<'p> TrustedInjector<'p> {
    pub fn new(provider: &'p dyn CredentialProvider) -> Self {
        Self { provider }
    }

    pub fn prepare(
        &self,
        role: PeerRole,
        auth_section_active: bool,
        context: AuthorizedLeaseContext,
        reference: CredentialRef,
        target: InjectionTarget,
        now: u64,
    ) -> Result<InjectionTicket, InjectionDenied> {
        if role != PeerRole::Injector {
            return Err(InjectionDenied::WorkerNotAllowed);
        }
        if !auth_section_active {
            return Err(InjectionDenied::AuthSectionRequired);
        }
        if context.expires_at <= now {
            return Err(InjectionDenied::LeaseExpired);
        }
        check_target(&context, &target)?;
        Ok(InjectionTicket {
            context,
            reference,
            target,
        })
    }

    /// `current` は commit の直前に controller が読み直した対象。prepare の時と 1 つでも
    /// 違えば（navigation・document・frame・要素・redirect）秘密を取り出さずに拒否する。
    pub fn commit(
        &self,
        ticket: InjectionTicket,
        auth_section_active: bool,
        current: &InjectionTarget,
        sink: &mut dyn InjectionSink,
        now: u64,
    ) -> Result<(InjectionReceipt, RedisplayGuard), InjectionDenied> {
        if !auth_section_active {
            return Err(InjectionDenied::AuthSectionRequired);
        }
        if ticket.context.expires_at <= now {
            return Err(InjectionDenied::LeaseExpired);
        }
        if *current != ticket.target {
            return Err(InjectionDenied::TargetChanged);
        }
        check_target(&ticket.context, current)?;
        let secret = self
            .provider
            .resolve(&ticket.reference, &ticket.context)
            .map_err(|_| InjectionDenied::Provider)?;
        let value = match current.field.kind {
            FieldKind::Username => &secret.username,
            FieldKind::Password => &secret.password,
        };
        let guard = RedisplayGuard::new(value);
        sink.insert(current, value)
            .map_err(|_| InjectionDenied::Sink)?;
        Ok((
            InjectionReceipt {
                lease_id: ticket.context.lease_id.clone(),
                element_id: current.field.element_id.clone(),
                navigation_id: current.navigation_id,
            },
            guard,
        ))
    }
}

impl std::fmt::Debug for RedisplayGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RedisplayGuard(<redacted>)")
    }
}

/// 注入した値の salt 付き hash だけを持ち、観測（snapshot・text）に値が再表示されたかを調べる。
///
/// ADR-0092: broker が注入ごとに作り、[`RedisplayGuardWire`]（salt・digest・長さ）だけを
/// controller に渡す。検査は raw に加え percent・UTF-16LE・base64（標準/URL-safe）・JSON escape の
/// 表現を復号してから行う（入れ子は 2 段まで）。
pub struct RedisplayGuard {
    salt: [u8; 16],
    digest: [u8; 32],
    len: usize,
}

/// controller へ渡す guard の形。秘密そのものは入らない（salt と salt 付き SHA-256 と byte 長）。
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedisplayGuardWire {
    pub salt: String,
    pub digest: String,
    pub len: usize,
}

impl std::fmt::Debug for RedisplayGuardWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RedisplayGuardWire(<redacted>)")
    }
}

/// 復号の入れ子の深さ。base64 の中の UTF-16LE・percent の中の base64 などまで見る。
const DECODE_DEPTH: u8 = 2;
/// これより長い観測の復号結果は作らない（raw の検査はする）。
const MAX_DECODE: usize = 16 << 20;

impl RedisplayGuard {
    /// 値から guard を作る。broker（と試験の fake broker）だけが使う。
    pub fn new(value: &str) -> Self {
        let mut salt = [0u8; 16];
        // salt が取れなくても判定は成り立つ（0 salt）。秘密は保持しない。
        let _ = getrandom::getrandom(&mut salt);
        Self {
            digest: Self::hash(&salt, value.as_bytes()),
            salt,
            len: value.len(),
        }
    }

    pub fn to_wire(&self) -> RedisplayGuardWire {
        RedisplayGuardWire {
            salt: hex::encode(self.salt),
            digest: hex::encode(self.digest),
            len: self.len,
        }
    }

    /// 形が壊れた wire は `None`（呼び出し側は注入を失敗扱いにする）。
    pub fn from_wire(wire: &RedisplayGuardWire) -> Option<Self> {
        let salt: [u8; 16] = hex::decode(&wire.salt).ok()?.try_into().ok()?;
        let digest: [u8; 32] = hex::decode(&wire.digest).ok()?.try_into().ok()?;
        if wire.len == 0 || wire.len > 4096 {
            return None;
        }
        Some(Self {
            salt,
            digest,
            len: wire.len,
        })
    }

    fn hash(salt: &[u8], bytes: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(salt);
        h.update(bytes);
        h.finalize().into()
    }

    fn raw(&self, b: &[u8]) -> bool {
        b.windows(self.len)
            .any(|w| Self::hash(&self.salt, w) == self.digest)
    }

    fn bytes(&self, b: &[u8], depth: u8) -> bool {
        if self.raw(b) {
            return true;
        }
        if depth == 0 || b.len() > MAX_DECODE {
            return false;
        }
        decodings(b).into_iter().any(|mut d| {
            let hit = self.bytes(&d, depth - 1);
            d.zeroize();
            hit
        })
    }

    /// 観測に注入した値が含まれるか。短すぎる値（4 byte 未満）は誤検出を避けて常に含むとみなす。
    pub fn exposes(&self, observation: &str) -> bool {
        self.exposes_bytes(observation.as_bytes())
    }

    pub fn exposes_bytes(&self, observation: &[u8]) -> bool {
        if self.len < 4 {
            return true;
        }
        self.bytes(observation, DECODE_DEPTH)
    }

    /// CDP の応答・event。直列化した text（escape 付き）と全ての key・文字列葉（escape 解除済み）を見る。
    pub fn exposes_json(&self, value: &serde_json::Value) -> bool {
        if self.len < 4 {
            return true;
        }
        let text = serde_json::to_vec(value).unwrap_or_default();
        self.exposes_bytes(&text) || self.leaves(value)
    }

    fn leaves(&self, value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::String(s) => self.exposes(s),
            serde_json::Value::Array(a) => a.iter().any(|v| self.leaves(v)),
            serde_json::Value::Object(o) => {
                o.iter().any(|(k, v)| self.exposes(k) || self.leaves(v))
            }
            _ => false,
        }
    }

    /// 観測を LLM に渡してよい形にする。再表示があれば全体を捨てる。
    pub fn filter(&self, observation: String) -> Option<String> {
        if self.exposes(&observation) {
            let mut o = observation;
            o.zeroize();
            None
        } else {
            Some(observation)
        }
    }
}

/// 観測の別表現を復号する。該当する表現が無ければ空。
fn decodings(b: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    if b.contains(&b'%') {
        out.push(percent_decode(b));
    }
    if b.contains(&b'\\') {
        out.push(json_unescape(b));
    }
    if b.contains(&0) {
        for offset in 0..2 {
            let units: Vec<u16> = b[offset.min(b.len())..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            out.push(String::from_utf16_lossy(&units).into_bytes());
        }
    }
    out.extend(base64_runs(b));
    out
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex_val(b[i + 1]), hex_val(b[i + 2]))
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn json_unescape(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut units: Vec<u16> = Vec::new();
    let mut i = 0;
    let flush = |units: &mut Vec<u16>, out: &mut Vec<u8>| {
        if !units.is_empty() {
            out.extend_from_slice(String::from_utf16_lossy(units).as_bytes());
            units.clear();
        }
    };
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            let c = b[i + 1];
            if c == b'u' && i + 5 < b.len() {
                let quad = b[i + 2..i + 6]
                    .iter()
                    .try_fold(0u16, |acc, &d| hex_val(d).map(|v| acc << 4 | u16::from(v)));
                if let Some(u) = quad {
                    units.push(u);
                    i += 6;
                    continue;
                }
            }
            flush(&mut units, &mut out);
            let mapped = match c {
                b'n' => Some(b'\n'),
                b't' => Some(b'\t'),
                b'r' => Some(b'\r'),
                b'b' => Some(8),
                b'f' => Some(12),
                b'"' | b'\\' | b'/' | b'\'' => Some(c),
                _ => None,
            };
            if let Some(m) = mapped {
                out.push(m);
                i += 2;
                continue;
            }
        }
        flush(&mut units, &mut out);
        out.push(b[i]);
        i += 1;
    }
    flush(&mut units, &mut out);
    out
}

fn b64_val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

/// 8 文字以上の base64 文字の連なりを、4 通りの開始位置で復号する（前置きの語が付いた token を拾う）。
fn base64_runs(b: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for run in b.split(|c| b64_val(*c).is_none()) {
        if run.len() < 8 {
            continue;
        }
        for start in 0..4.min(run.len()) {
            let mut bytes = Vec::with_capacity(run.len() * 3 / 4 + 3);
            let mut acc: u32 = 0;
            let mut bits = 0u32;
            for &c in &run[start..] {
                let Some(v) = b64_val(c) else { break };
                acc = (acc << 6 | u32::from(v)) & 0xff_ffff;
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    bytes.push((acc >> bits) as u8);
                }
            }
            out.push(bytes);
        }
    }
    out
}

/// broker の IPC 入口で使う。provider の resolve は Injector だけ。
pub fn resolve_for_peer(
    provider: &dyn CredentialProvider,
    role: PeerRole,
    reference: &CredentialRef,
    context: &AuthorizedLeaseContext,
) -> Result<crate::SecretEnvelope, InjectionDenied> {
    if role != PeerRole::Injector {
        return Err(InjectionDenied::WorkerNotAllowed);
    }
    provider
        .resolve(reference, context)
        .map_err(|_| InjectionDenied::Provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, ProviderCapabilities, SecretEnvelope};
    use std::sync::Mutex;

    const ORIGIN: &str = "https://login.example.com";
    const PASSWORD: &str = "hunter2-very-secret";

    struct Fake {
        calls: Mutex<u32>,
    }
    impl CredentialProvider for Fake {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                manual_registration: true,
                interactive_unlock: false,
                totp: false,
                revoke: true,
            }
        }
        fn resolve(
            &self,
            _: &CredentialRef,
            _: &AuthorizedLeaseContext,
        ) -> Result<SecretEnvelope, Error> {
            *self.calls.lock().expect("lock") += 1;
            Ok(SecretEnvelope {
                username: "alice@example.com".into(),
                password: PASSWORD.into(),
            })
        }
        fn revoke(&self, _: &str) -> Result<(), Error> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Sink {
        got: Vec<(String, String)>,
    }
    impl InjectionSink for Sink {
        fn insert(&mut self, t: &InjectionTarget, v: &str) -> Result<(), SinkError> {
            self.got.push((t.field.element_id.clone(), v.to_owned()));
            Ok(())
        }
    }

    fn fake() -> Fake {
        Fake {
            calls: Mutex::new(0),
        }
    }
    fn ctx() -> AuthorizedLeaseContext {
        AuthorizedLeaseContext {
            lease_id: "l1".into(),
            task_id: "t1".into(),
            run_id: "r1".into(),
            session_id: "s1".into(),
            exact_origin: ORIGIN.into(),
            expires_at: 2_000,
            credential_revision: 1,
        }
    }
    fn reference() -> CredentialRef {
        CredentialRef {
            credential_id: "c1".into(),
            provider: "manual".into(),
            policy_id: "p1".into(),
        }
    }
    fn target() -> InjectionTarget {
        InjectionTarget {
            session_id: "s1".into(),
            navigation_id: 7,
            document_id: "doc-a".into(),
            frame_chain: vec![ORIGIN.into()],
            redirect_chain: vec![ORIGIN.into()],
            field: FieldInfo {
                element_id: "pw".into(),
                kind: FieldKind::Password,
                input_type: "password".into(),
            },
        }
    }

    fn prepare(
        inj: &TrustedInjector<'_>,
        t: InjectionTarget,
    ) -> Result<InjectionTicket, InjectionDenied> {
        inj.prepare(PeerRole::Injector, true, ctx(), reference(), t, 1_000)
    }

    #[test]
    fn injects_into_verified_target_without_returning_secret() {
        let p = fake();
        let inj = TrustedInjector::new(&p);
        let ticket = prepare(&inj, target()).unwrap();
        let mut sink = Sink::default();
        let (receipt, guard) = inj
            .commit(ticket, true, &target(), &mut sink, 1_001)
            .unwrap();
        assert_eq!(sink.got, vec![("pw".to_string(), PASSWORD.to_string())]);
        assert!(!format!("{receipt:?}").contains(PASSWORD));
        assert!(guard.filter("<form>ok</form>".into()).is_some());
    }

    #[test]
    fn worker_and_agent_cannot_obtain_secret() {
        let p = fake();
        let inj = TrustedInjector::new(&p);
        for role in [PeerRole::Worker, PeerRole::Agent] {
            assert_eq!(
                inj.prepare(role, true, ctx(), reference(), target(), 1_000)
                    .unwrap_err(),
                InjectionDenied::WorkerNotAllowed
            );
            assert_eq!(
                resolve_for_peer(&p, role, &reference(), &ctx()).unwrap_err(),
                InjectionDenied::WorkerNotAllowed
            );
        }
        assert_eq!(*p.calls.lock().unwrap(), 0);
        assert!(resolve_for_peer(&p, PeerRole::Injector, &reference(), &ctx()).is_ok());
    }

    #[test]
    fn auth_section_is_required_for_both_steps() {
        let p = fake();
        let inj = TrustedInjector::new(&p);
        assert_eq!(
            inj.prepare(
                PeerRole::Injector,
                false,
                ctx(),
                reference(),
                target(),
                1_000
            )
            .unwrap_err(),
            InjectionDenied::AuthSectionRequired
        );
        let ticket = prepare(&inj, target()).unwrap();
        let err = inj
            .commit(ticket, false, &target(), &mut Sink::default(), 1_001)
            .unwrap_err();
        assert_eq!(err, InjectionDenied::AuthSectionRequired);
        assert_eq!(*p.calls.lock().unwrap(), 0);
    }

    #[test]
    fn toctou_changes_between_prepare_and_commit_are_rejected() {
        let p = fake();
        let inj = TrustedInjector::new(&p);
        let mutations: Vec<fn(&mut InjectionTarget)> = vec![
            |t| t.navigation_id += 1,
            |t| t.document_id = "doc-b".into(),
            |t| t.field.element_id = "pw2".into(),
            |t| t.field.input_type = "text".into(),
            |t| t.frame_chain.push("https://evil.example".into()),
            |t| t.redirect_chain.push("https://evil.example".into()),
        ];
        for m in mutations {
            let ticket = prepare(&inj, target()).unwrap();
            let mut now = target();
            m(&mut now);
            let mut sink = Sink::default();
            assert_eq!(
                inj.commit(ticket, true, &now, &mut sink, 1_001)
                    .unwrap_err(),
                InjectionDenied::TargetChanged
            );
            assert!(sink.got.is_empty());
        }
        assert_eq!(*p.calls.lock().unwrap(), 0);
        let ticket = prepare(&inj, target()).unwrap();
        assert_eq!(
            inj.commit(ticket, true, &target(), &mut Sink::default(), 2_000)
                .unwrap_err(),
            InjectionDenied::LeaseExpired
        );
    }

    #[test]
    fn redirect_and_cross_origin_iframe_are_rejected() {
        let p = fake();
        let inj = TrustedInjector::new(&p);
        let mut t = target();
        t.redirect_chain = vec!["https://evil.example".into(), ORIGIN.into()];
        assert_eq!(prepare(&inj, t).unwrap_err(), InjectionDenied::Redirected);
        let mut t = target();
        t.redirect_chain = vec![ORIGIN.into(), "https://login.example.com:8443".into()];
        assert_eq!(prepare(&inj, t).unwrap_err(), InjectionDenied::Redirected);
        // top は正しいが、注入先の iframe が他 origin
        let mut t = target();
        t.frame_chain = vec![ORIGIN.into(), "https://evil.example".into()];
        assert_eq!(
            prepare(&inj, t).unwrap_err(),
            InjectionDenied::CrossOriginFrame
        );
        // 他 origin の top に正しい iframe が埋め込まれている
        let mut t = target();
        t.frame_chain = vec!["https://evil.example".into(), ORIGIN.into()];
        assert_eq!(
            prepare(&inj, t).unwrap_err(),
            InjectionDenied::CrossOriginFrame
        );
        let mut t = target();
        t.frame_chain.clear();
        assert_eq!(
            prepare(&inj, t).unwrap_err(),
            InjectionDenied::EmptyFrameChain
        );
        let mut t = target();
        t.session_id = "s2".into();
        assert_eq!(prepare(&inj, t).unwrap_err(), InjectionDenied::OtherSession);
        assert_eq!(*p.calls.lock().unwrap(), 0);
    }

    #[test]
    fn dom_redisplay_is_rejected_and_detected() {
        let p = fake();
        let inj = TrustedInjector::new(&p);
        // password を text input に入れさせる罠
        let mut t = target();
        t.field.input_type = "text".into();
        assert_eq!(
            prepare(&inj, t).unwrap_err(),
            InjectionDenied::RedisplayField
        );
        let mut t = target();
        t.field.kind = FieldKind::Username;
        t.field.input_type = "hidden".into();
        assert_eq!(
            prepare(&inj, t).unwrap_err(),
            InjectionDenied::RedisplayField
        );
        // 注入後に page script が値を DOM に書き戻した観測は捨てる
        let ticket = prepare(&inj, target()).unwrap();
        let (_, guard) = inj
            .commit(ticket, true, &target(), &mut Sink::default(), 1_001)
            .unwrap();
        let leaked = format!("<div id=echo>your password is {PASSWORD}!</div>");
        assert!(guard.exposes(&leaked));
        assert!(guard.filter(leaked).is_none());
        assert!(
            guard
                .filter("<div>hunter2-very-secreT</div>".into())
                .is_some()
        );
    }

    #[test]
    fn guard_detects_encoded_redisplay_and_round_trips_over_the_wire() {
        let g = RedisplayGuard::from_wire(&RedisplayGuard::new(PASSWORD).to_wire()).expect("wire");
        let utf16: Vec<u8> = PASSWORD.bytes().flat_map(|b| [b, 0]).collect();
        let b64 = |b: &[u8]| {
            const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            b.chunks(3)
                .flat_map(|c| {
                    let n =
                        c.iter().fold(0u32, |a, &x| a << 8 | u32::from(x)) << (8 * (3 - c.len()));
                    (0..=c.len()).map(move |i| A[(n >> (18 - 6 * i) & 63) as usize] as char)
                })
                .collect::<String>()
        };
        let percent: String = PASSWORD.bytes().map(|b| format!("%{b:02X}")).collect();
        let escaped: String = PASSWORD
            .chars()
            .map(|c| format!("\\u{:04x}", c as u32))
            .collect();
        for obs in [
            format!("x{PASSWORD}y"),
            format!("q={percent}"),
            format!("data:{}", b64(PASSWORD.as_bytes())),
            format!("k=zz{}", b64(format!("ab{PASSWORD}").as_bytes())),
            b64(&utf16),
            format!("{{\"v\":\"{escaped}\"}}"),
        ] {
            assert!(g.exposes(&obs), "{obs}");
        }
        assert!(g.exposes_bytes(&utf16));
        let v = serde_json::json!({"r": String::from_utf8_lossy(&utf16)});
        assert!(g.exposes_json(&v));
        assert!(!g.exposes("hunter2-very-secreT %41 aGVsbG8gd29ybGQ="));
        assert!(!g.exposes_json(&serde_json::json!({"ok": [1, "clean"]})));
        let bad = RedisplayGuardWire {
            salt: "00".into(),
            digest: "11".into(),
            len: 5,
        };
        assert!(RedisplayGuard::from_wire(&bad).is_none());
        assert_eq!(
            format!("{:?}", g.to_wire()),
            "RedisplayGuardWire(<redacted>)"
        );
    }
}
