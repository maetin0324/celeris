//! 本番 Attested 判定の境界（ADR-0138 条件 1〜5・D-L）。試験用 SameUidHarness は使わない。
//! 隔離条件と owner 検査に通っても、launcher の session 証明が無い・検証に失敗した runtime は拒否する。

use celeris_credentiald::injection_ipc::{
    Admission, InjectCode, LauncherProofRegistration, admit_attested, process_start,
};
use std::collections::BTreeSet;
use task_core::browser_isolation::{
    CdpEndpoint, LauncherObservation, LauncherSessionProof, Namespace, REQUIRED_NAMESPACES,
    RuntimeFacts,
};

const LAUNCHER_UID: u32 = 1500;

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

fn proof() -> LauncherSessionProof {
    LauncherSessionProof {
        session_id: "session-1".into(),
        instance_id: "launcher-1".into(),
        pid: 4242,
        starttime: 777,
        ns_owner_uid: Some(165536),
        launcher_uid: LAUNCHER_UID,
        isolation_ok: true,
        ns_inodes: Default::default(),
    }
}

fn seen() -> LauncherObservation {
    LauncherObservation {
        session_id: "session-1".into(),
        instance_id: "launcher-1".into(),
        peer_uid: Some(LAUNCHER_UID),
        configured_launcher_uid: LAUNCHER_UID,
        runtime_pid: 4242,
        runtime_starttime: Some(777),
    }
}

#[test]
fn launcher_credential_verified_proof_is_admitted() {
    let facts = isolated_facts();
    assert!(admit_attested(&facts, Some(&proof()), &seen()).is_ok());
}

#[test]
fn launcher_credential_missing_proof_is_rejected() {
    assert_eq!(
        admit_attested(&isolated_facts(), None, &seen()),
        Err(InjectCode::IsolationRequired)
    );
}

#[test]
fn launcher_credential_forged_proof_is_rejected() {
    let mut forged = proof();
    forged.instance_id = "forged-launcher".into();
    assert_eq!(
        admit_attested(&isolated_facts(), Some(&forged), &seen()),
        Err(InjectCode::IsolationRequired)
    );
}

#[test]
fn launcher_credential_expired_process_proof_is_rejected() {
    let mut observation = seen();
    // PID は再利用され得るため、現在の process starttime が証明と違えば古い証明として拒否する。
    observation.runtime_starttime = Some(778);
    assert_eq!(
        admit_attested(&isolated_facts(), Some(&proof()), &observation),
        Err(InjectCode::IsolationRequired)
    );
}

#[test]
fn launcher_credential_uid_mismatch_is_rejected() {
    let mut observation = seen();
    observation.peer_uid = Some(LAUNCHER_UID + 1);
    assert_eq!(
        admit_attested(&isolated_facts(), Some(&proof()), &observation),
        Err(InjectCode::IsolationRequired)
    );
}

#[test]
fn separate_uid_with_verified_launcher_proof_is_admitted() {
    let facts = isolated_facts();
    let attestation = admit_attested(&facts, Some(&proof()), &seen()).expect("launcher session");
    assert_eq!(attestation.session_id(), facts.session_id);
    assert_eq!(attestation.pid(), 4242);
    assert_eq!(attestation.starttime(), 777);
    assert_eq!(attestation.launcher_uid(), LAUNCHER_UID);
    let iso = attestation.isolation_attestation();
    assert_eq!(iso.runtime_uid(), facts.runtime_uid);
    assert_eq!(iso.userns_owner_uid(), facts.userns_owner_uid.unwrap());
}

#[test]
fn credential_injection_without_launcher_proof_is_rejected_even_if_owner_check_passes() {
    let facts = isolated_facts();
    // owner 検査を含む隔離条件だけなら通る事実でも、証明が無ければ拒否する（fail closed）。
    assert!(task_core::browser_isolation::verify_isolation(&facts).is_ok());
    assert_eq!(
        admit_attested(&facts, None, &seen()),
        Err(InjectCode::IsolationRequired)
    );
}

#[test]
fn credential_injection_with_failed_launcher_proof_is_rejected() {
    let facts = isolated_facts();
    type Case = (
        &'static str,
        fn(&mut LauncherSessionProof, &mut LauncherObservation),
    );
    let cases: [Case; 9] = [
        ("starttime mismatch (pid reuse)", |_, s| {
            s.runtime_starttime = Some(778)
        }),
        ("runtime gone", |_, s| s.runtime_starttime = None),
        ("pid of another process", |p, _| p.pid = 4243),
        ("peer uid unavailable", |_, s| s.peer_uid = None),
        ("launcher uid mismatch", |p, _| p.launcher_uid = 1501),
        ("launcher is the daemon", |p, s| {
            p.launcher_uid = 1000;
            s.peer_uid = Some(1000);
            s.configured_launcher_uid = 1000;
        }),
        ("launcher isolation not ok", |p, _| p.isolation_ok = false),
        ("owner is the daemon", |p, _| p.ns_owner_uid = Some(1000)),
        ("proof of another session", |p, _| {
            p.session_id = "session-2".into()
        }),
    ];
    for (name, mutate) in cases {
        let (mut p, mut s) = (proof(), seen());
        mutate(&mut p, &mut s);
        assert_eq!(
            admit_attested(&facts, Some(&p), &s),
            Err(InjectCode::IsolationRequired),
            "{name}"
        );
    }
}

#[test]
fn same_uid_is_rejected_even_with_launcher_proof() {
    let mut facts = isolated_facts();
    facts.runtime_uid = facts.host_uid;
    assert_eq!(
        admit_attested(&facts, Some(&proof()), &seen()),
        Err(InjectCode::IsolationRequired)
    );
}

#[test]
fn missing_namespace_is_rejected_even_with_launcher_proof() {
    let mut facts = isolated_facts();
    facts.namespaces.remove(&Namespace::User);
    assert_eq!(
        admit_attested(&facts, Some(&proof()), &seen()),
        Err(InjectCode::IsolationRequired)
    );
}

#[test]
fn daemon_owned_userns_is_rejected() {
    let mut facts = isolated_facts();
    facts.userns_owner_uid = Some(facts.host_uid);
    assert_eq!(
        admit_attested(&facts, Some(&proof()), &seen()),
        Err(InjectCode::IsolationRequired)
    );
}

#[test]
fn unknown_userns_owner_is_rejected() {
    let mut facts = isolated_facts();
    facts.userns_owner_uid = None;
    assert_eq!(
        admit_attested(&facts, Some(&proof()), &seen()),
        Err(InjectCode::IsolationRequired)
    );
}

#[test]
fn dead_runtime_is_session_not_live() {
    assert_eq!(
        Admission::Attested.admit("session-1", i32::MAX, None, Some(LAUNCHER_UID)),
        Err(InjectCode::SessionNotLive)
    );
    assert_eq!(
        Admission::Attested.admit("session-1", 0, None, Some(LAUNCHER_UID)),
        Err(InjectCode::SessionNotLive)
    );
}

#[test]
fn live_runtime_without_configured_launcher_uid_or_proof_is_rejected() {
    // 実 process（この試験 process 自身）に結び付いた証明でも、設定上の launcher UID が無ければ、
    // また証明が無ければ拒否する。SameUid でもあるので、どちらでも許可されない。
    let pid = std::process::id();
    let reg = LauncherProofRegistration {
        session_id: "session-1".into(),
        instance_id: "launcher-1".into(),
        peer_uid: Some(LAUNCHER_UID),
        proof: LauncherSessionProof {
            pid: pid as i32,
            starttime: process_start(pid).expect("self start"),
            ..proof()
        },
    };
    assert_eq!(
        Admission::Attested.admit("session-1", pid as i32, Some(&reg), None),
        Err(InjectCode::IsolationRequired)
    );
    assert_eq!(
        Admission::Attested.admit("session-1", pid as i32, None, Some(LAUNCHER_UID)),
        Err(InjectCode::IsolationRequired)
    );
    assert_eq!(
        Admission::Attested.admit("session-1", pid as i32, Some(&reg), Some(LAUNCHER_UID)),
        Err(InjectCode::IsolationRequired)
    );
}
