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
pub struct RedisplayGuard {
    salt: [u8; 16],
    digest: [u8; 32],
    len: usize,
}

impl RedisplayGuard {
    fn new(value: &str) -> Self {
        let mut salt = [0u8; 16];
        // salt が取れなくても判定は成り立つ（0 salt）。秘密は保持しない。
        let _ = getrandom::getrandom(&mut salt);
        Self {
            digest: Self::hash(&salt, value.as_bytes()),
            salt,
            len: value.len(),
        }
    }

    fn hash(salt: &[u8], bytes: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(salt);
        h.update(bytes);
        h.finalize().into()
    }

    /// 観測に注入した値が含まれるか。短すぎる値（4 byte 未満）は誤検出を避けて常に含むとみなす。
    pub fn exposes(&self, observation: &str) -> bool {
        if self.len < 4 {
            return true;
        }
        let b = observation.as_bytes();
        b.windows(self.len)
            .any(|w| Self::hash(&self.salt, w) == self.digest)
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
}
