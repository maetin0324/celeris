//! 本番 identity 復元 admission（`RestoreAdmission::Attested`）の判定（ADR-0116）。
//!
//! 別 UID・userns owner が daemon でない（ptrace 拒否、ADR-0115 launcher 経由）検証済み隔離
//! session で、かつ launcher の session 証明（ADR-0116 D-L）を検証できたときだけ通す。
//! 証明なし・証明の検証失敗・SameUid・非隔離・owner=daemon・owner 不明は拒否する。

use task_core::browser_isolation::{
    CdpEndpoint, IsolationViolation, LauncherObservation, LauncherProofDefect,
    LauncherSessionProof, Namespace, REQUIRED_NAMESPACES, RuntimeFacts,
};
use task_worker::browser_runtime::RestoreAdmission;
use task_worker::browser_supervisor::SupervisorOptions;

const DAEMON_UID: u32 = 1001;
const RUNTIME_UID: u32 = 165_536;
const LAUNCHER_UID: u32 = 165_000;
const RUNTIME_PID: i32 = 4243;
const STARTTIME: u64 = 987_654;

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

/// launcher が `Started` で返し daemon が照合した session 証明。
fn proof() -> LauncherSessionProof {
    LauncherSessionProof {
        session_id: "prod-s1".into(),
        instance_id: "inst-1".into(),
        pid: RUNTIME_PID,
        starttime: STARTTIME,
        ns_owner_uid: Some(LAUNCHER_UID),
        launcher_uid: LAUNCHER_UID,
        isolation_ok: true,
        ns_inodes: Default::default(),
    }
}

/// daemon 自身が採った照合値（応答の `SCM_CREDENTIALS`・設定・`/proc/<pid>/stat`）。
fn seen() -> LauncherObservation {
    LauncherObservation {
        session_id: "prod-s1".into(),
        instance_id: "inst-1".into(),
        peer_uid: Some(LAUNCHER_UID),
        configured_launcher_uid: LAUNCHER_UID,
        runtime_pid: RUNTIME_PID,
        runtime_starttime: Some(STARTTIME),
    }
}

/// 証明付きで判定し、拒否を期待する。
fn rejected_with(
    f: &RuntimeFacts,
    p: &LauncherSessionProof,
    o: &LauncherObservation,
) -> Vec<IsolationViolation> {
    RestoreAdmission::Attested
        .admit_launched(f, Some((p, o)))
        .expect_err("production admission must reject")
}

/// 有効な証明を添えて判定する（隔離の検査が弱まっていないことを確かめる）。
fn rejected(f: &RuntimeFacts) -> Vec<IsolationViolation> {
    rejected_with(f, &proof(), &seen())
}

fn invalid(defect: LauncherProofDefect) -> IsolationViolation {
    IsolationViolation::LauncherProofInvalid { defect }
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
fn launcher_proven_separate_uid_session_is_admitted() {
    let att = RestoreAdmission::Attested
        .admit_launched(&separated(), Some((&proof(), &seen())))
        .expect("launcher-proven separate-uid session");
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
            IsolationViolation::UsernsOwnedByDaemon,
            invalid(LauncherProofDefect::OwnerMismatch),
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
    // launcher の証明と daemon の観測した owner が食い違う。
    assert_eq!(
        rejected(&f),
        vec![
            IsolationViolation::UsernsOwnedByDaemon,
            invalid(LauncherProofDefect::OwnerMismatch),
        ]
    );
}

#[test]
fn unknown_userns_owner_is_rejected() {
    let mut f = separated();
    f.userns_owner_uid = None;
    assert_eq!(
        rejected(&f),
        vec![
            IsolationViolation::OwnerUnknown,
            invalid(LauncherProofDefect::OwnerMismatch),
        ]
    );
}

#[test]
fn attested_without_launcher_proof_is_rejected_even_if_isolated() {
    // owner 検査を含む隔離の全条件に通っても、launcher の証明が無ければ拒否（fail closed）。
    assert_eq!(
        RestoreAdmission::Attested.admit(&separated()).unwrap_err(),
        vec![IsolationViolation::LauncherProofMissing]
    );
    assert_eq!(
        RestoreAdmission::Attested
            .admit_launched(&separated(), None)
            .unwrap_err(),
        vec![IsolationViolation::LauncherProofMissing]
    );
}

#[test]
fn same_uid_without_proof_reports_all_violations() {
    let mut f = separated();
    f.runtime_uid = DAEMON_UID;
    f.userns_owner_uid = Some(DAEMON_UID);
    assert_eq!(
        RestoreAdmission::Attested.admit(&f).unwrap_err(),
        vec![
            IsolationViolation::SameUid,
            IsolationViolation::UsernsOwnedByDaemon,
            IsolationViolation::LauncherProofMissing,
        ]
    );
}

#[test]
fn non_isolated_with_valid_proof_is_rejected() {
    // 証明があっても隔離の検査は弱めない。
    let mut f = separated();
    f.namespaces.clear();
    f.no_new_privs = false;
    let v = rejected(&f);
    assert!(v.contains(&IsolationViolation::PrivilegesKept));
    assert!(
        v.iter()
            .any(|x| matches!(x, IsolationViolation::MissingNamespace { .. }))
    );
}

#[test]
fn starttime_mismatch_is_rejected() {
    // PID 再利用・process 入替え。
    let mut o = seen();
    o.runtime_starttime = Some(STARTTIME + 1);
    assert_eq!(
        rejected_with(&separated(), &proof(), &o),
        vec![invalid(LauncherProofDefect::StarttimeMismatch)]
    );
    let mut o = seen();
    o.runtime_starttime = None;
    assert_eq!(
        rejected_with(&separated(), &proof(), &o),
        vec![invalid(LauncherProofDefect::StarttimeUnavailable)]
    );
}

#[test]
fn pid_mismatch_is_rejected() {
    let mut o = seen();
    o.runtime_pid = RUNTIME_PID + 1;
    assert_eq!(
        rejected_with(&separated(), &proof(), &o),
        vec![invalid(LauncherProofDefect::PidMismatch)]
    );
}

#[test]
fn proof_owned_by_daemon_is_rejected() {
    let mut p = proof();
    p.ns_owner_uid = Some(DAEMON_UID);
    assert_eq!(
        rejected_with(&separated(), &p, &seen()),
        vec![invalid(LauncherProofDefect::OwnerIsDaemon)]
    );
    let mut p = proof();
    p.ns_owner_uid = Some(LAUNCHER_UID + 1);
    assert_eq!(
        rejected_with(&separated(), &p, &seen()),
        vec![invalid(LauncherProofDefect::OwnerMismatch)]
    );
}

#[test]
fn launcher_identity_mismatch_is_rejected() {
    let mut o = seen();
    o.peer_uid = None;
    assert_eq!(
        rejected_with(&separated(), &proof(), &o),
        vec![invalid(LauncherProofDefect::PeerUidUnavailable)]
    );
    let mut o = seen();
    o.peer_uid = Some(LAUNCHER_UID + 7);
    assert_eq!(
        rejected_with(&separated(), &proof(), &o),
        vec![invalid(LauncherProofDefect::LauncherUidMismatch)]
    );
    // launcher を daemon 自身の UID で名乗っても通らない。
    let mut p = proof();
    p.launcher_uid = DAEMON_UID;
    let mut o = seen();
    o.peer_uid = Some(DAEMON_UID);
    o.configured_launcher_uid = DAEMON_UID;
    assert_eq!(
        rejected_with(&separated(), &p, &o),
        vec![invalid(LauncherProofDefect::LauncherUidPrivileged)]
    );
}

#[test]
fn launcher_isolation_not_ok_is_rejected() {
    let mut p = proof();
    p.isolation_ok = false;
    assert_eq!(
        rejected_with(&separated(), &p, &seen()),
        vec![invalid(LauncherProofDefect::IsolationNotOk)]
    );
}

#[test]
fn proof_for_another_session_is_rejected() {
    let mut p = proof();
    p.session_id = "prod-s2".into();
    assert_eq!(
        rejected_with(&separated(), &p, &seen()),
        vec![invalid(LauncherProofDefect::SessionMismatch)]
    );
    let mut o = seen();
    o.instance_id = "inst-other".into();
    assert_eq!(
        rejected_with(&separated(), &proof(), &o),
        vec![invalid(LauncherProofDefect::SessionMismatch)]
    );
}
