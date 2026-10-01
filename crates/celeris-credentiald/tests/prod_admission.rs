//! 本番 Attested 判定の境界。試験用 SameUidHarness は使わない。

use celeris_credentiald::injection_ipc::{Admission, InjectCode, admit_attested};
use std::collections::BTreeSet;
use task_core::browser_isolation::{CdpEndpoint, Namespace, REQUIRED_NAMESPACES, RuntimeFacts};

fn isolated_facts() -> RuntimeFacts {
    RuntimeFacts {
        session_id: "session-1".into(),
        host_uid: 1000,
        runtime_uid: 165536,
        userns_owner_uid: Some(165536),
        namespaces: BTreeSet::from(REQUIRED_NAMESPACES),
        root_readonly: true,
        writable_mounts: vec!["/session/profile".into()],
        visible_paths: vec!["/session/profile".into()],
        cdp: CdpEndpoint::Pipe,
        no_new_privs: true,
        capabilities_dropped: true,
        pgid: 4242,
    }
}

#[test]
fn separate_uid_with_non_daemon_userns_owner_is_admitted() {
    let facts = isolated_facts();
    let attestation = admit_attested(&facts).expect("isolated runtime");
    assert_eq!(attestation.session_id(), facts.session_id);
    assert_eq!(attestation.runtime_uid(), facts.runtime_uid);
    assert_eq!(
        attestation.userns_owner_uid(),
        facts.userns_owner_uid.unwrap()
    );
}

#[test]
fn same_uid_is_rejected() {
    let mut facts = isolated_facts();
    facts.runtime_uid = facts.host_uid;
    assert_eq!(admit_attested(&facts), Err(InjectCode::IsolationRequired));
}

#[test]
fn missing_namespace_is_rejected() {
    let mut facts = isolated_facts();
    facts.namespaces.remove(&Namespace::User);
    assert_eq!(admit_attested(&facts), Err(InjectCode::IsolationRequired));
}

#[test]
fn daemon_owned_userns_is_rejected() {
    let mut facts = isolated_facts();
    facts.userns_owner_uid = Some(facts.host_uid);
    assert_eq!(admit_attested(&facts), Err(InjectCode::IsolationRequired));
}

#[test]
fn unknown_userns_owner_is_rejected() {
    let mut facts = isolated_facts();
    facts.userns_owner_uid = None;
    assert_eq!(admit_attested(&facts), Err(InjectCode::IsolationRequired));
}

#[test]
fn dead_runtime_is_session_not_live() {
    assert_eq!(
        Admission::Attested.admit("session-1", i32::MAX),
        Err(InjectCode::SessionNotLive)
    );
    assert_eq!(
        Admission::Attested.admit("session-1", 0),
        Err(InjectCode::SessionNotLive)
    );
}
