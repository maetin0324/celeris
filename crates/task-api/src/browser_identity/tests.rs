use super::*;
use celeris_credentiald::identity_seal::StateEntry;
use task_core::browser_identity::IDENTITY_TTL_MAX_SECS;

const NOW: u64 = 1_800_000_000;
const ORIGIN: &str = "https://example.com";
const SECRET: &str = "cookie-secret-value-zz9";

fn state(origin: &str) -> IdentityStatePlain {
    IdentityStatePlain {
        entries: vec![StateEntry {
            origin: origin.into(),
            kind: "cookie".into(),
            name: "sid".into(),
            value: SECRET.into(),
        }],
    }
}

fn input(id: &str) -> IdentityRegisterInput {
    IdentityRegisterInput {
        identity_id: id.into(),
        project_id: "proj".into(),
        origin: ORIGIN.into(),
        demand_confirmed_by: Some("rmaeda".into()),
        ttl_secs: None,
        state: state(ORIGIN),
    }
}

fn fixture() -> (SqliteStore, IdentitySealer, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let sealer = IdentitySealer::open(dir.path().join("keys")).unwrap();
    (SqliteStore::open_in_memory().unwrap(), sealer, dir)
}

fn code(r: Result<IdentityView, IdentityApiError>) -> &'static str {
    r.unwrap_err().code()
}

#[test]
fn register_defaults_to_seven_days_and_lists_metadata_only() {
    let (store, sealer, _d) = fixture();
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    let v = svc.register(input("a"), NOW).unwrap();
    assert_eq!(v.expires_at, NOW + IDENTITY_TTL_DEFAULT_SECS);
    assert_eq!(v.state, IdentityState::Active);
    let list = svc.list("proj", NOW).unwrap();
    assert_eq!(list, vec![v]);
    let json = serde_json::to_string(&list).unwrap();
    assert!(!json.contains(SECRET));
    assert!(!json.contains("ciphertext"));
    assert!(!json.contains("key_label"));
    assert!(!json.contains("sealed"));
    // 保存されたのは封緘だけ（平文の cookie は store に無い）。
    let stored = store.browser_identity_get("a").unwrap().unwrap();
    let blob = stored.sealed_blob.clone().unwrap();
    assert!(!String::from_utf8_lossy(&blob).contains(SECRET));
    assert!(!format!("{stored:?}").contains(SECRET));
}

#[test]
fn register_rejects_with_fixed_codes() {
    let (store, sealer, _d) = fixture();
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    let mut i = input("a");
    i.demand_confirmed_by = None;
    assert_eq!(code(svc.register(i, NOW)), "demand_not_confirmed");
    let mut i = input("a");
    i.ttl_secs = Some(IDENTITY_TTL_MAX_SECS + 1);
    assert_eq!(code(svc.register(i, NOW)), "ttl_too_long");
    let mut i = input("a");
    i.origin = "http://example.com".into();
    assert_eq!(code(svc.register(i, NOW)), "invalid_origin");
    let mut i = input("a");
    i.state = state("https://other.example");
    assert_eq!(code(svc.register(i, NOW)), "foreign_origin");
    let mut i = input("a");
    i.state
        .entries
        .extend(state("https://evil.example").entries.clone());
    assert_eq!(code(svc.register(i, NOW)), "foreign_origin");
    assert!(svc.list("proj", NOW).unwrap().is_empty());
    // 上限ちょうどは通る（切り詰めではない）。
    let mut i = input("b");
    i.ttl_secs = Some(IDENTITY_TTL_MAX_SECS);
    assert_eq!(
        svc.register(i, NOW).unwrap().expires_at,
        NOW + IDENTITY_TTL_MAX_SECS
    );
    assert_eq!(code(svc.register(input("b"), NOW)), "identity_conflict");
}

#[test]
fn revoked_old_generation_cannot_be_opened() {
    let (store, sealer, _d) = fixture();
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    svc.register(input("a"), NOW).unwrap();
    let before = store.browser_identity_get("a").unwrap().unwrap();
    let sealed = sealed_of(&before).unwrap();
    assert!(sealer.open_state(&before.identity, &sealed, NOW).is_ok());
    let v = svc.revoke("a").unwrap();
    assert_eq!(v.state, IdentityState::Revoked);
    assert_eq!(v.generation, before.identity.generation + 1);
    let after = store.browser_identity_get("a").unwrap().unwrap();
    assert!(after.sealed_blob.is_none());
    let err = sealer
        .open_state(&after.identity, &sealed, NOW)
        .unwrap_err();
    assert!(matches!(err, SealError::Denied(IdentityDenied::Revoked)));
    let mut reactivated = after.identity.clone();
    reactivated.state = IdentityState::Active;
    let err = sealer.open_state(&reactivated, &sealed, NOW).unwrap_err();
    assert!(matches!(
        err,
        SealError::Denied(IdentityDenied::StaleGeneration)
    ));
    assert_eq!(svc.revoke("nope").unwrap_err().code(), "identity_not_found");
}

#[test]
fn deleted_leaves_tombstone_and_seal_cannot_be_opened() {
    let (store, sealer, _d) = fixture();
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    svc.register(input("a"), NOW).unwrap();
    let before = store.browser_identity_get("a").unwrap().unwrap();
    let sealed = sealed_of(&before).unwrap();
    let v = svc.delete("a").unwrap();
    assert_eq!(v.state, IdentityState::Deleted);
    let tomb = store.browser_identity_get("a").unwrap().unwrap();
    assert!(tomb.sealed_blob.is_none());
    assert_eq!(svc.list("proj", NOW).unwrap().len(), 1);
    // 鍵が消えているので、世代が合う元の metadata でも開けない。
    let err = sealer
        .open_state(&before.identity, &sealed, NOW)
        .unwrap_err();
    assert!(matches!(err, SealError::KeyErased | SealError::Tampered));
    let err = sealer.open_state(&tomb.identity, &sealed, NOW).unwrap_err();
    assert!(matches!(err, SealError::Denied(IdentityDenied::Deleted)));
}

fn attestation() -> task_core::browser_isolation::IsolationAttestation {
    use task_core::browser_isolation::{
        CdpEndpoint, REQUIRED_NAMESPACES, RuntimeFacts, verify_isolation,
    };
    verify_isolation(&RuntimeFacts {
        session_id: "s1".into(),
        host_uid: 1000,
        runtime_uid: 200_001,
        userns_owner_uid: Some(1001),
        namespaces: REQUIRED_NAMESPACES.into_iter().collect(),
        root_readonly: true,
        writable_mounts: vec!["/session/profile".into()],
        visible_paths: vec!["/usr".into()],
        cdp: CdpEndpoint::Pipe,
        no_new_privs: true,
        capabilities_dropped: true,
        pgid: 4242,
    })
    .unwrap()
}

#[test]
fn restore_is_allowed_only_under_verified_isolation() {
    let (store, sealer, _d) = fixture();
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    svc.register(input("a"), NOW).unwrap();
    // trusted local は拒否のまま
    assert_eq!(
        svc.restore("a", "proj", ORIGIN, NOW).unwrap_err().code(),
        "isolation_required"
    );
    let att = attestation();
    let plain = svc
        .restore_isolated("a", "proj", ORIGIN, &att, NOW)
        .unwrap();
    assert_eq!(plain.entries[0].value, SECRET);
    // 隔離下でも他 project / 他 origin / 期限切れは拒否
    assert_eq!(
        svc.restore_isolated("a", "other", ORIGIN, &att, NOW)
            .unwrap_err()
            .code(),
        "other_project"
    );
    assert_eq!(
        svc.restore_isolated("a", "proj", "https://other.example", &att, NOW)
            .unwrap_err()
            .code(),
        "other_origin"
    );
    assert!(
        svc.restore_isolated(
            "a",
            "proj",
            ORIGIN,
            &att,
            NOW + IDENTITY_TTL_DEFAULT_SECS + 1
        )
        .is_err()
    );
}

struct FakeLive(Option<task_core::browser_isolation::IsolationAttestation>);
impl task_core::browser_isolation::LiveIsolation for FakeLive {
    fn current_attestation(
        &self,
    ) -> Result<
        task_core::browser_isolation::IsolationAttestation,
        Vec<task_core::browser_isolation::IsolationViolation>,
    > {
        self.0
            .clone()
            .ok_or_else(|| vec![task_core::browser_isolation::IsolationViolation::SameUid])
    }
}

#[test]
fn restore_for_session_binds_to_live_isolated_session() {
    let (store, sealer, _d) = fixture();
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    svc.register(input("a"), NOW).unwrap();
    svc.register(input("b"), NOW).unwrap();
    let live = FakeLive(Some(attestation()));
    let plain = svc
        .restore_for_session("a", "proj", ORIGIN, "s1", &live, NOW)
        .unwrap();
    assert_eq!(plain.entries[0].value, SECRET);
    // 隔離でない（停止・同一 UID など attestation が出ない）session
    let dead = FakeLive(None);
    assert_eq!(
        svc.restore_for_session("a", "proj", ORIGIN, "s1", &dead, NOW)
            .unwrap_err()
            .code(),
        "isolation_required"
    );
    // 別 session の attestation
    assert_eq!(
        svc.restore_for_session("a", "proj", ORIGIN, "s2", &live, NOW)
            .unwrap_err()
            .code(),
        "isolation_required"
    );
    // 別 identity（存在しない / 他 project の identity）
    assert_eq!(
        svc.restore_for_session("zz", "proj", ORIGIN, "s1", &live, NOW)
            .unwrap_err()
            .code(),
        "identity_not_found"
    );
    assert_eq!(
        svc.restore_for_session("b", "other", ORIGIN, "s1", &live, NOW)
            .unwrap_err()
            .code(),
        "other_project"
    );
    // 期限切れ
    assert!(
        svc.restore_for_session(
            "a",
            "proj",
            ORIGIN,
            "s1",
            &live,
            NOW + IDENTITY_TTL_DEFAULT_SECS + 1
        )
        .is_err()
    );
}

/// registry の entry（controller の投入口つき）。渡された state を記録する。
struct FakeEntry {
    live: FakeLive,
    kind: task_core::browser_isolation::RuntimeKind,
    accepts: bool,
    delivered: std::sync::Mutex<Vec<Vec<u8>>>,
}
impl task_core::browser_isolation::LiveIsolation for FakeEntry {
    fn current_attestation(
        &self,
    ) -> Result<
        task_core::browser_isolation::IsolationAttestation,
        Vec<task_core::browser_isolation::IsolationViolation>,
    > {
        self.live.current_attestation()
    }
}
impl task_core::browser_isolation::LiveSessionEntry for FakeEntry {
    fn kind(&self) -> task_core::browser_isolation::RuntimeKind {
        self.kind
    }
    fn accepts_state(&self) -> bool {
        self.accepts
    }
    fn deliver_state(
        &self,
        state: &[u8],
    ) -> Result<(), task_core::browser_isolation::StateRejected> {
        self.delivered.lock().unwrap().push(state.to_vec());
        Ok(())
    }
    fn live_key(&self) -> Option<(String, String)> {
        Some(("task-1".into(), "run-1".into()))
    }
}

#[test]
fn restore_in_session_delivers_only_to_controller_after_all_checks() {
    use task_core::browser_isolation::{LiveSessions, RuntimeKind};
    let (store, sealer, _d) = fixture();
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    svc.register(input("a"), NOW).unwrap();
    let reg = LiveSessions::default();
    let entry = |kind, accepts, att: bool| {
        std::sync::Arc::new(FakeEntry {
            live: FakeLive(att.then(attestation)),
            kind,
            accepts,
            delivered: std::sync::Mutex::new(Vec::new()),
        })
    };
    let ok = entry(RuntimeKind::Isolated, true, true);
    let plain = entry(RuntimeKind::NotIsolated, true, true);
    let no_inlet = entry(RuntimeKind::Isolated, false, true);
    let same_uid = entry(RuntimeKind::Isolated, true, false);
    reg.insert("s1", ok.clone());
    reg.insert("plain", plain.clone());
    reg.insert("same-uid", same_uid.clone());
    let denied =
        |session: &str, r: Option<&dyn task_core::browser_isolation::LiveSessionRegistry>| {
            svc.restore_in_session("a", "proj", ORIGIN, session, r, NOW)
                .unwrap_err()
                .code()
        };
    assert_eq!(denied("s1", None), "isolation_required");
    assert_eq!(denied("nope", Some(&reg)), "isolation_required");
    assert_eq!(denied("plain", Some(&reg)), "isolation_required");
    assert_eq!(denied("same-uid", Some(&reg)), "isolation_required");
    // attestation の session id（s1）と要求の session が違う
    reg.insert("s2", ok.clone());
    assert_eq!(denied("s2", Some(&reg)), "isolation_required");
    reg.remove("s2");
    // controller に投入口が無い
    let reg2 = LiveSessions::default();
    reg2.insert("s1", no_inlet.clone());
    assert_eq!(denied("s1", Some(&reg2)), "isolation_required");
    assert_eq!(sealer.open_attempts(), 0, "no refusal may open the seal");
    assert!(plain.delivered.lock().unwrap().is_empty());
    assert!(same_uid.delivered.lock().unwrap().is_empty());
    // 全部通ったときだけ開封し、controller にだけ渡す（戻り値に state は無い）。
    let () = svc
        .restore_in_session("a", "proj", ORIGIN, "s1", Some(&reg), NOW)
        .unwrap();
    assert_eq!(sealer.open_attempts(), 1);
    let got = ok.delivered.lock().unwrap();
    assert_eq!(got.len(), 1);
    assert!(String::from_utf8_lossy(&got[0]).contains(SECRET));
}

#[test]
fn restore_is_isolation_required_on_trusted_local() {
    let (store, sealer, _d) = fixture();
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    svc.register(input("a"), NOW).unwrap();
    let err = svc.restore("a", "proj", ORIGIN, NOW).unwrap_err();
    assert_eq!(err.code(), "isolation_required");
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
    // 他 project / 他 origin は先に拒否する。
    assert_eq!(
        svc.restore("a", "other", ORIGIN, NOW).unwrap_err().code(),
        "other_project"
    );
    assert_eq!(
        svc.restore("a", "proj", "https://other.example", NOW)
            .unwrap_err()
            .code(),
        "other_origin"
    );
}

#[test]
fn sealed_state_cannot_cross_project_or_identity() {
    let (store, sealer, _d) = fixture();
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    svc.register(input("a"), NOW).unwrap();
    let source = store.browser_identity_get("a").unwrap().unwrap();
    let sealed = sealed_of(&source).unwrap();

    let mut other_project = input("b");
    other_project.project_id = "other".into();
    svc.register(other_project, NOW).unwrap();
    let target = store.browser_identity_get("b").unwrap().unwrap();
    assert_eq!(
        sealer.open_state(&target.identity, &sealed, NOW),
        Err(SealError::Denied(IdentityDenied::OtherIdentity))
    );
    // Even if the outer envelope is forged, AEAD binds the original project and identity.
    let mut forged = sealed.clone();
    forged.envelope = bi::envelope_for(&target.identity);
    assert_eq!(
        sealer.open_state(&target.identity, &forged, NOW),
        Err(SealError::Tampered)
    );
    let mut same_id_other_project = target.identity.clone();
    same_id_other_project.identity_id = "a".into();
    assert_eq!(
        sealer.open_state(&same_id_other_project, &sealed, NOW),
        Err(SealError::Denied(IdentityDenied::OtherProject))
    );
}

#[test]
fn public_output_and_debug_never_include_sealed_state_or_cookie() {
    let (store, sealer, _d) = fixture();
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    let view = svc.register(input("a"), NOW).unwrap();
    let stored = store.browser_identity_get("a").unwrap().unwrap();
    let sealed = sealed_of(&stored).unwrap();
    let response = serde_json::to_string(&serde_json::json!({"identity": view})).unwrap();
    let log = format!(
        "{:?} {:?} {:?}",
        input("a").state,
        view,
        IdentityApiError::SealFailed.problem()
    );
    for output in [&response, &log] {
        assert!(!output.contains(SECRET));
        assert!(!output.contains(&sealed.ciphertext));
        assert!(!output.contains(&sealed.envelope.key_label));
    }
    assert!(!response.contains("sealed_blob"));
    assert!(!response.contains("key_label"));
}

#[test]
fn expired_identities_are_revoked_on_read() {
    let (store, sealer, _d) = fixture();
    let svc = IdentityService {
        store: &store,
        sealer: &sealer,
    };
    let v = svc.register(input("a"), NOW).unwrap();
    let later = v.expires_at;
    let list = svc.list("proj", later).unwrap();
    assert_eq!(list[0].state, IdentityState::Revoked);
    assert!(
        store
            .browser_identity_get("a")
            .unwrap()
            .unwrap()
            .sealed_blob
            .is_none()
    );
    assert_eq!(
        svc.restore("a", "proj", ORIGIN, later).unwrap_err().code(),
        "identity_revoked"
    );
}

#[test]
fn problem_body_is_fixed_code_only() {
    let p = IdentityApiError::Denied(IdentityDenied::IsolationRequired).problem();
    let s = format!("{p:?}");
    assert!(s.contains("isolation_required"));
    assert!(!s.contains(SECRET));
}
