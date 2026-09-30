use super::*;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).expect("ts")
}

fn row(id: &str, role: InstanceRole, heartbeat: i64, pid: u32) -> DaemonInstance {
    DaemonInstance {
        instance_id: id.into(),
        release: "r".into(),
        pid,
        role,
        started_at: at(0),
        heartbeat_at: at(heartbeat),
        handoff_requested_at: None,
        drained_at: None,
    }
}

const W: Duration = Duration::from_secs(30);

#[test]
fn the_holder_is_gone_only_when_no_other_live_active_or_draining_instance_exists() {
    let alive = |_: u32| true;
    let dead = |_: u32| false;
    // 自分だけ（旧は SIGTERM で自分の行を消して終わった: 本番 2026-09-28 17:05:55Z）。
    let me = row("me", InstanceRole::Active, 100, 1);
    assert!(holder_gone(
        std::slice::from_ref(&me),
        "me",
        at(100),
        W,
        &alive
    ));
    assert!(holder_gone(&[], "me", at(100), W, &alive));
    // draining の旧が heartbeat を打っている（ライブ切替）→ 横取りしない。
    let old = row("old", InstanceRole::Draining, 95, 2);
    assert!(!holder_gone(
        &[me.clone(), old.clone()],
        "me",
        at(100),
        W,
        &alive
    ));
    // 旧の heartbeat が古い → 居ない。
    assert!(holder_gone(
        &[me.clone(), old.clone()],
        "me",
        at(200),
        W,
        &alive
    ));
    // 旧の pid が消えている（SIGKILL 直後で heartbeat はまだ新しい）→ 居ない。
    assert!(holder_gone(
        &[me.clone(), old.clone()],
        "me",
        at(100),
        W,
        &dead
    ));
    // drained（手元が 0 になって exit する旧）→ 居ない。
    let mut drained = old.clone();
    drained.drained_at = Some(at(99));
    assert!(holder_gone(
        &[me.clone(), drained],
        "me",
        at(100),
        W,
        &alive
    ));
    // standby は run を持たない。
    let standby = row("sb", InstanceRole::Standby, 100, 3);
    assert!(holder_gone(
        &[me.clone(), standby],
        "me",
        at(100),
        W,
        &alive
    ));
    // 生きている別の active（異常だが保守的に）→ 横取りしない。
    let other = row("other", InstanceRole::Active, 100, 4);
    assert!(!holder_gone(&[me, other], "me", at(100), W, &alive));
}

#[test]
fn proc_pid_alive_sees_this_process_and_not_an_unused_pid() {
    assert!(proc_pid_alive(std::process::id()));
    assert!(proc_pid_alive(0));
    // pid_max（既定 4194304）を超える pid は存在しない。
    assert!(!proc_pid_alive(u32::MAX - 1));
}
