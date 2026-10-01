//! 本番 identity 復元 admission（`RestoreAdmission::Attested`）の判定（ADR-0116）。
//!
//! 別 UID・userns owner が daemon でない（ptrace 拒否、ADR-0115 launcher 経由）検証済み隔離
//! session だけを通し、SameUid・非隔離・owner=daemon・owner 不明は拒否する。

use task_core::browser_isolation::{
    CdpEndpoint, IsolationViolation, Namespace, RuntimeFacts, REQUIRED_NAMESPACES,
};
use task_worker::browser_runtime::RestoreAdmission;
use task_worker::browser_supervisor::SupervisorOptions;

const DAEMON_UID: u32 = 1001;
const RUNTIME_UID: u32 = 165_536;
const LAUNCHER_UID: u32 = 165_000;

/// launcher が持ち主の userns で別 UID の runtime が動く、正常な事実。
fn separated() -> RuntimeFacts {
    RuntimeFacts {
        session_id: "prod-s1".into(),
        host_uid: DAEMON_UID,
        runtime_uid: RUNTIME_UID,
        userns_owner_uid: Some(LAUNCHER_UID),
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

fn rejected(f: &RuntimeFacts) -> Vec<IsolationViolation> {
    RestoreAdmission::Attested
        .admit(f)
        .expect_err("production admission must reject")
}

#[test]
fn supervisor_default_is_attested() {
    assert_eq!(RestoreAdmission::default(), RestoreAdmission::Attested);
    assert_eq!(
        SupervisorOptions::new("/nonexistent").admission,
        RestoreAdmission::Attested
    );
}

#[test]
fn separated_uid_with_non_daemon_owner_is_admitted() {
    let att = RestoreAdmission::Attested
        .admit(&separated())
        .expect("verified separate-uid session");
    assert_eq!(att.session_id(), "prod-s1");
    assert_eq!(att.runtime_uid(), RUNTIME_UID);
    assert_eq!(att.userns_owner_uid(), LAUNCHER_UID);
    assert_eq!(att.pgid(), 4242);
}

#[test]
fn same_uid_is_rejected() {
    let mut f = separated();
    f.runtime_uid = DAEMON_UID;
    assert_eq!(rejected(&f), vec![IsolationViolation::SameUid]);
}

#[test]
fn same_uid_owned_by_daemon_is_rejected() {
    // 同一 UID の bwrap（daemon が userns の持ち主）は本番では通らない。
    let mut f = separated();
    f.runtime_uid = DAEMON_UID;
    f.userns_owner_uid = Some(DAEMON_UID);
    assert_eq!(
        rejected(&f),
        vec![
            IsolationViolation::SameUid,
            IsolationViolation::UsernsOwnedByDaemon
        ]
    );
}

#[test]
fn missing_namespace_is_rejected() {
    let mut f = separated();
    f.namespaces.remove(&Namespace::Net);
    assert_eq!(
        rejected(&f),
        vec![IsolationViolation::MissingNamespace { ns: Namespace::Net }]
    );
}

#[test]
fn writable_root_is_rejected() {
    let mut f = separated();
    f.root_readonly = false;
    assert_eq!(rejected(&f), vec![IsolationViolation::RootWritable]);
}

#[test]
fn other_isolation_failures_are_rejected() {
    let mut f = separated();
    f.cdp = CdpEndpoint::Tcp {
        addr: "127.0.0.1:9222".into(),
    };
    assert!(rejected(&f).contains(&IsolationViolation::CdpOnTcp));

    let mut f = separated();
    f.capabilities_dropped = false;
    assert!(rejected(&f).contains(&IsolationViolation::PrivilegesKept));

    let mut f = separated();
    f.visible_paths.push("/run/celeris/credentiald".into());
    assert!(matches!(
        rejected(&f).as_slice(),
        [IsolationViolation::BrokerVisible { .. }]
    ));
}

#[test]
fn userns_owned_by_daemon_is_rejected() {
    // 別 UID でも daemon が userns の持ち主なら ptrace を拒めない（ADR-0115）。
    let mut f = separated();
    f.userns_owner_uid = Some(DAEMON_UID);
    assert_eq!(rejected(&f), vec![IsolationViolation::UsernsOwnedByDaemon]);
}

#[test]
fn unknown_userns_owner_is_rejected() {
    let mut f = separated();
    f.userns_owner_uid = None;
    assert_eq!(rejected(&f), vec![IsolationViolation::OwnerUnknown]);
}
