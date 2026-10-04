use super::*;
use task_core::SqliteStore;

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_800_000_000 + secs).expect("ts")
}

const WINDOW: Duration = Duration::from_secs(30);

fn row(id: &str, release: &str, role: InstanceRole, heartbeat: i64) -> DaemonInstance {
    DaemonInstance {
        instance_id: id.into(),
        release: release.into(),
        pid: 1234,
        role,
        started_at: at(0),
        heartbeat_at: at(heartbeat),
        handoff_requested_at: None,
        drained_at: None,
    }
}

/// `release` は `--release` > `CELERIS_RELEASE` > `"dev"`。
#[test]
fn the_release_string_comes_from_the_flag_then_the_env_then_dev() {
    assert_eq!(resolve_release(Some("abc123def456")), "abc123def456");
    assert_eq!(resolve_release(Some("  spaced  ")), "spaced");
    // 環境変数はプロセス全体なので、この 1 つのテストの中だけで触る。
    assert_eq!(resolve_release(None), DEV_RELEASE);
    unsafe { std::env::set_var(RELEASE_ENV, "fromenv12345") };
    assert_eq!(resolve_release(None), "fromenv12345");
    assert_eq!(
        resolve_release(Some("cli12345")),
        "cli12345",
        "flag wins over the env"
    );
    unsafe { std::env::set_var(RELEASE_ENV, "  ") };
    assert_eq!(
        resolve_release(None),
        DEV_RELEASE,
        "blank env is not a release"
    );
    unsafe { std::env::remove_var(RELEASE_ENV) };

    let identity = InstanceIdentity::new(Some("sha12sha12ab"));
    assert_eq!(identity.release, "sha12sha12ab");
    assert_eq!(identity.pid, std::process::id());
    assert!(!identity.instance_id.is_empty());
}

/// ADR-0040 D4: `3 × tick + lease_grace`。
#[test]
fn the_freshness_window_is_three_ticks_plus_the_lease_grace() {
    assert_eq!(
        freshness_window(Duration::from_millis(500), 30),
        Duration::from_millis(31_500)
    );
    assert_eq!(
        freshness_window(Duration::from_secs(1), 0),
        Duration::from_secs(3)
    );
}

/// (d) 同じ版の `active` が生きていれば二重起動なので exit 3 の判断になる。
#[test]
fn the_same_release_running_as_active_is_a_duplicate() {
    let live = |_: u32| true;
    let rows = vec![row("old", "sha-aaa", InstanceRole::Active, 0)];
    assert_eq!(
        decide_startup(&rows, "sha-aaa", at(1), WINDOW, &live),
        StartupDecision::DuplicateRelease {
            instance_id: "old".into(),
            pid: 1234
        }
    );
    // heartbeat が古ければ二重起動ではない（前のプロセスは死んでいる）。
    assert_eq!(
        decide_startup(&rows, "sha-aaa", at(31), WINDOW, &live),
        StartupDecision::Active
    );
    // `drained_at` が付いた行も同様に数えない。
    let mut drained = rows.clone();
    drained[0].drained_at = Some(at(0));
    assert_eq!(
        decide_startup(&drained, "sha-aaa", at(1), WINDOW, &live),
        StartupDecision::Active
    );
    // heartbeat が新しくてもプロセスが消えていれば二重起動ではない（SIGKILL 直後の起こし直し）。
    assert_eq!(
        decide_startup(&rows, "sha-aaa", at(1), WINDOW, &|_| false),
        StartupDecision::Active
    );
    // 自分自身の pid は必ず生きている（`pid_alive` の素振り）。
    assert!(pid_alive(std::process::id()));
    assert!(pid_alive(0), "pid が分からない行は生きている扱い");
}

/// 別の版の `active` がいれば standby になり、その行に引き継ぎを要求する。
#[test]
fn a_different_release_becomes_standby_and_an_empty_table_becomes_active() {
    let live = |_: u32| true;
    let rows = vec![row("old", "sha-aaa", InstanceRole::Active, 0)];
    assert_eq!(
        decide_startup(&rows, "sha-bbb", at(1), WINDOW, &live),
        StartupDecision::Standby {
            active_instance_id: "old".into()
        }
    );
    assert_eq!(
        decide_startup(&[], "sha-bbb", at(1), WINDOW, &live),
        StartupDecision::Active
    );
    // `standby` / `draining` / `verify` の行は「active がいる」ことにならない。
    let others = vec![
        row("s", "sha-ccc", InstanceRole::Standby, 0),
        row("d", "sha-ddd", InstanceRole::Draining, 0),
        row("v", "sha-eee", InstanceRole::Verify, 0),
    ];
    assert_eq!(
        decide_startup(&others, "sha-bbb", at(1), WINDOW, &live),
        StartupDecision::Active
    );
}

/// 毎 tick の規則: active は要求を見たら drain、standby は active が消えたら promote。
#[test]
fn the_tick_rules_are_symmetric() {
    let mut me = row("me", "new", InstanceRole::Active, 0);
    assert_eq!(
        decide_tick(InstanceRole::Active, "me", &[me.clone()], at(1), WINDOW),
        TickDecision::Stay
    );
    me.handoff_requested_at = Some(at(1));
    assert_eq!(
        decide_tick(InstanceRole::Active, "me", &[me.clone()], at(1), WINDOW),
        TickDecision::Drain
    );

    let standby = row("me", "new", InstanceRole::Standby, 0);
    let old_active = row("old", "prev", InstanceRole::Active, 0);
    let rows = vec![standby.clone(), old_active.clone()];
    assert_eq!(
        decide_tick(InstanceRole::Standby, "me", &rows, at(1), WINDOW),
        TickDecision::Stay
    );
    // (a) 旧が draining になったら昇格する。
    let mut draining = rows.clone();
    draining[1].role = InstanceRole::Draining;
    assert_eq!(
        decide_tick(InstanceRole::Standby, "me", &draining, at(1), WINDOW),
        TickDecision::Promote
    );
    // (e) heartbeat が止まっても昇格する。
    assert_eq!(
        decide_tick(InstanceRole::Standby, "me", &rows, at(31), WINDOW),
        TickDecision::Promote
    );
    // draining と verify は役割を変えない。
    assert_eq!(
        decide_tick(InstanceRole::Draining, "me", &rows, at(31), WINDOW),
        TickDecision::Stay
    );
    assert_eq!(
        decide_tick(InstanceRole::Verify, "me", &[], at(31), WINDOW),
        TickDecision::Stay
    );
}

/// Phase 119 D4: `drained_at` が付いた行のうち、pid がまだ生きている（＝「drain 後にプロセスが
/// 終了しない」障害）ものだけを拾う。自分自身の行・`drained_at` 無し・pid が死んでいる行は拾わない。
#[test]
fn stale_but_alive_rows_finds_only_drained_rows_whose_pid_is_still_running() {
    let mut still_stuck = row("stuck", "old-rel", InstanceRole::Draining, 0);
    still_stuck.drained_at = Some(at(5));
    still_stuck.pid = 9001;
    let mut cleanly_exited = row("exited", "older-rel", InstanceRole::Draining, 0);
    cleanly_exited.drained_at = Some(at(3));
    cleanly_exited.pid = 9002;
    let still_draining = row("draining", "mid-rel", InstanceRole::Draining, 10);
    // まだ drain していない（`drained_at` 無し）行は、pid が生きていても対象外。
    let mut myself = row("me", "new-rel", InstanceRole::Active, 10);
    myself.drained_at = Some(at(5)); // 自分自身は（理屈上あり得なくても）除外される。
    myself.pid = 9001;

    let rows = vec![
        still_stuck.clone(),
        cleanly_exited.clone(),
        still_draining,
        myself,
    ];
    let alive = |pid: u32| pid == 9001; // 9001 だけ生きている扱い。9002 は死んでいる。

    let found = stale_but_alive_rows(&rows, "me", &alive);
    assert_eq!(
        found
            .iter()
            .map(|r| r.instance_id.as_str())
            .collect::<Vec<_>>(),
        vec!["stuck"],
        "only the still-running, drained, non-self row should be flagged"
    );
}

fn supervisor(
    store: &Arc<dyn TaskStore>,
    release: &str,
    now: OffsetDateTime,
    drain: Duration,
) -> Started {
    supervisor_with(store, release, now, drain, false)
}

/// ADR-0070 D4（Phase 116）: `drain_force_abort` を選べる版。既定の `supervisor()` は常に `false`
/// （drain timeout で abort しない、が既定の挙動）。
fn supervisor_with(
    store: &Arc<dyn TaskStore>,
    release: &str,
    now: OffsetDateTime,
    drain: Duration,
    drain_force_abort: bool,
) -> Started {
    Supervisor::start(
        Arc::clone(store),
        InstanceIdentity {
            instance_id: format!("inst-{release}"),
            release: release.into(),
            pid: 1,
        },
        SharedRole::new(InstanceRole::Standby),
        WINDOW,
        drain,
        drain_force_abort,
        now,
    )
    .expect("start")
}

/// (a) 新 standby → 旧 draining → 新 active、を `Supervisor` の一連の呼び出しで確かめる（DB 付き）。
#[test]
fn the_handoff_runs_through_the_store() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let Started::Running(mut old) = supervisor(&store, "old", at(0), Duration::from_secs(60))
    else {
        panic!("the first instance must become active");
    };
    assert_eq!(old.role(), InstanceRole::Active);

    let Started::Running(mut new) = supervisor(&store, "new", at(1), Duration::from_secs(60))
    else {
        panic!("a different release must become standby");
    };
    assert_eq!(new.role(), InstanceRole::Standby);
    let rows = store.instance_list().expect("list");
    let old_row = rows
        .iter()
        .find(|r| r.instance_id == "inst-old")
        .expect("old row");
    assert_eq!(
        old_row.handoff_requested_at,
        Some(at(1)),
        "standby は active に引き継ぎを要求する"
    );

    // 旧は次の tick で draining になる（手元に run が 1 つあるのでまだ終わらない）。
    assert_eq!(old.step(at(2), 1).expect("step"), Step::Draining);
    assert_eq!(old.role(), InstanceRole::Draining);
    // 新はそれを見て active になる。
    assert_eq!(new.step(at(3), 0).expect("step"), Step::Promoted);
    assert_eq!(new.role(), InstanceRole::Active);
    assert_eq!(new.step(at(4), 0).expect("step"), Step::Stay);

    // 旧は run が終わったら drained になり、その行は新 active が掃除する。
    assert_eq!(old.step(at(5), 1).expect("step"), Step::Stay);
    assert_eq!(old.step(at(6), 0).expect("step"), Step::Drained);
    assert!(
        store
            .instance_list()
            .expect("list")
            .iter()
            .any(|r| r.drained_at == Some(at(6)))
    );
    assert_eq!(new.step(at(7), 0).expect("step"), Step::Stay);
    let left: Vec<String> = store
        .instance_list()
        .expect("list")
        .into_iter()
        .map(|r| r.instance_id)
        .collect();
    assert_eq!(left, ["inst-new"], "drained の行は消える");
}

/// (d) 同じ版をもう一度起こしたら行を書かずに `Duplicate`。
#[test]
fn starting_the_same_release_twice_is_refused_without_touching_the_table() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let Started::Running(_first) = supervisor(&store, "same", at(0), Duration::from_secs(60))
    else {
        panic!("the first instance must become active");
    };
    match supervisor(&store, "same", at(1), Duration::from_secs(60)) {
        Started::Duplicate { instance_id, pid } => {
            assert_eq!((instance_id.as_str(), pid), ("inst-same", 1));
        }
        Started::Running(_) => panic!("the same release must not start twice"),
    }
    assert_eq!(
        store.instance_list().expect("list").len(),
        1,
        "行は増えない"
    );
}

/// ADR-0070 D4（Phase 116）: `drain_force_abort = true` のときだけ、drain timeout を過ぎたら
/// run が残っていても `DrainTimedOut` になる（従来の挙動、人が明示的に強い昇格を選んだとき）。
#[test]
fn the_drain_timeout_ends_the_drain_with_runs_left_when_force_abort_is_set() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let Started::Running(mut old) =
        supervisor_with(&store, "old", at(0), Duration::from_secs(10), true)
    else {
        panic!("active");
    };
    let Started::Running(_new) =
        supervisor_with(&store, "new", at(1), Duration::from_secs(10), true)
    else {
        panic!("standby");
    };
    assert_eq!(old.step(at(2), 3).expect("step"), Step::Draining);
    assert_eq!(old.step(at(5), 3).expect("step"), Step::Stay);
    assert_eq!(old.step(at(12), 3).expect("step"), Step::DrainTimedOut);
    let row = store
        .instance_list()
        .expect("list")
        .into_iter()
        .find(|r| r.instance_id == "inst-old")
        .expect("row");
    assert_eq!(row.drained_at, Some(at(12)));
}

/// ADR-0070 D4（Phase 116。D6(d)）: 既定（`drain_force_abort = false`）では、drain timeout を
/// 過ぎても run が残っている間は `DrainTimedOut` にならない（`Step::Stay` のまま待ち続け、
/// `drained_at` も書かない）。手元の run が本当に 0 になったときだけ `Drained` になる。
#[test]
fn the_drain_timeout_does_not_abort_alive_runs_by_default() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let Started::Running(mut old) = supervisor(&store, "old", at(0), Duration::from_secs(10))
    else {
        panic!("active");
    };
    let Started::Running(_new) = supervisor(&store, "new", at(1), Duration::from_secs(10)) else {
        panic!("standby");
    };
    assert_eq!(old.step(at(2), 3).expect("step"), Step::Draining);
    assert_eq!(old.step(at(5), 3).expect("step"), Step::Stay);
    // drain_timeout_secs (10) を過ぎても、in_flight が 0 でない限り待ち続ける。
    assert_eq!(old.step(at(12), 3).expect("step"), Step::Stay);
    assert_eq!(old.step(at(3600), 1).expect("step"), Step::Stay);
    let row = store
        .instance_list()
        .expect("list")
        .into_iter()
        .find(|r| r.instance_id == "inst-old")
        .expect("row");
    assert!(
        row.drained_at.is_none(),
        "abort していないので drained_at は書かれない"
    );
    // 手元の run が 0 になれば、通常どおり Drained で終わる。
    assert_eq!(old.step(at(3601), 0).expect("step"), Step::Drained);
}

/// (e) 旧の heartbeat が止まったら standby は昇格し、旧の行を消す。
#[test]
fn a_stale_heartbeat_promotes_the_standby_and_removes_the_dead_row() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let Started::Running(_old) = supervisor(&store, "old", at(0), Duration::from_secs(60)) else {
        panic!("active");
    };
    let Started::Running(mut new) = supervisor(&store, "new", at(1), Duration::from_secs(60))
    else {
        panic!("standby");
    };
    assert_eq!(new.step(at(2), 0).expect("step"), Step::Stay);
    // 旧が heartbeat を打たないまま窓を過ぎる。
    assert_eq!(new.step(at(100), 0).expect("step"), Step::Promoted);
    let left: Vec<String> = store
        .instance_list()
        .expect("list")
        .into_iter()
        .map(|r| r.instance_id)
        .collect();
    assert_eq!(left, ["inst-new"]);
}

/// 行が消えても heartbeat で気づいて登録し直す（`deregister` は自分の行だけ消す）。
#[test]
fn a_missing_row_is_registered_again_and_deregister_removes_it() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().expect("open"));
    let Started::Running(mut only) = supervisor(&store, "solo", at(0), Duration::from_secs(60))
    else {
        panic!("active");
    };
    assert!(store.instance_delete("inst-solo").expect("delete"));
    assert_eq!(only.step(at(1), 0).expect("step"), Step::Stay);
    let rows = store.instance_list().expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].role, InstanceRole::Active);
    only.deregister();
    assert!(store.instance_list().expect("list").is_empty());
}

// ADR-0040 付記（2026-10-02）: 昇格の認可の判定。

fn evidence(current: Option<&str>, promoting: Option<(&str, i64)>) -> PromotionEvidence {
    PromotionEvidence {
        release_managed: true,
        current_target: current.map(std::path::PathBuf::from),
        promoting: promoting.map(|(sha12, started)| {
            Ok(PromotingMarker {
                sha12: sha12.into(),
                started_at: at(started)
                    .format(&time::format_description::well_known::Rfc3339)
                    .expect("fmt"),
            })
        }),
        promote_lock: None,
    }
}

#[test]
fn dev_and_unmanaged_releases_skip_the_promotion_gate() {
    let ev = evidence(Some("releases/other"), None);
    assert!(matches!(
        decide_promotion(DEV_RELEASE, &ev, at(0)),
        PromotionGate::Skipped(_)
    ));
    let unmanaged = PromotionEvidence {
        release_managed: false,
        ..ev
    };
    assert!(matches!(
        decide_promotion("abc", &unmanaged, at(0)),
        PromotionGate::Skipped(_)
    ));
}

#[test]
fn current_pointing_at_the_release_authorizes_by_name() {
    let ev = evidence(Some("releases/abc"), None);
    assert!(matches!(
        decide_promotion("abc", &ev, at(0)),
        PromotionGate::Authorized(_)
    ));
    // 名前の比較は最後の要素だけ（`abcd` は `abc` ではない）。
    let ev = evidence(Some("releases/abcd"), None);
    assert!(matches!(
        decide_promotion("abc", &ev, at(0)),
        PromotionGate::Rejected(_)
    ));
}

#[test]
fn a_fresh_matching_marker_authorizes_and_stale_or_foreign_ones_do_not() {
    let fresh = evidence(Some("releases/old"), Some(("abc", -10)));
    assert!(matches!(
        decide_promotion("abc", &fresh, at(0)),
        PromotionGate::Authorized(_)
    ));
    let edge = evidence(None, Some(("abc", -PROMOTING_MAX_AGE_SECS)));
    assert!(matches!(
        decide_promotion("abc", &edge, at(0)),
        PromotionGate::Authorized(_)
    ));
    let stale = evidence(
        Some("releases/old"),
        Some(("abc", -PROMOTING_MAX_AGE_SECS - 1)),
    );
    assert!(matches!(
        decide_promotion("abc", &stale, at(0)),
        PromotionGate::Rejected(_)
    ));
    let future = evidence(None, Some(("abc", PROMOTING_MAX_AGE_SECS + 1)));
    assert!(matches!(
        decide_promotion("abc", &future, at(0)),
        PromotionGate::Rejected(_)
    ));
    let foreign = evidence(Some("releases/old"), Some(("xyz", -10)));
    assert!(matches!(
        decide_promotion("abc", &foreign, at(0)),
        PromotionGate::Rejected(_)
    ));
    let broken = PromotionEvidence {
        promoting: Some(Err("bad json".into())),
        ..evidence(None, None)
    };
    match decide_promotion("abc", &broken, at(0)) {
        PromotionGate::Rejected(reason) => assert!(reason.contains("bad json"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn read_promotion_evidence_reads_current_and_the_marker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let releases = dir.path().join("releases");
    std::fs::create_dir_all(releases.join("abc")).expect("mkdir");
    std::os::unix::fs::symlink("releases/abc", dir.path().join("current")).expect("symlink");
    std::fs::write(
        releases.join("abc").join(PROMOTING_FILE),
        r#"{"sha12":"abc","script":"promote.sh","mode":"live","pid":1,"started_at":"2026-10-02T00:00:00Z"}"#,
    )
    .expect("write");
    let ev = read_promotion_evidence(&releases, "abc");
    assert!(ev.release_managed);
    assert_eq!(
        ev.current_target.as_deref(),
        Some(std::path::Path::new("releases/abc"))
    );
    assert_eq!(
        ev.promoting,
        Some(Ok(PromotingMarker {
            sha12: "abc".into(),
            started_at: "2026-10-02T00:00:00Z".into()
        }))
    );
    assert_eq!(ev.promote_lock, None);
    // 管理外の名前は印を読まない。
    let other = read_promotion_evidence(&releases, "zzz");
    assert!(!other.release_managed);
    assert_eq!(other.promoting, None);
    assert_eq!(other.promote_lock, None);
}

// ADR-0040 付記の規則 3: 旧版の GUI/API が書いた `promote.lock`。

fn lock_evidence(pid: u32, alive: bool, modified: i64) -> PromotionEvidence {
    PromotionEvidence {
        promote_lock: Some(Ok(PromoteLockFact {
            pid,
            alive,
            modified: at(modified),
        })),
        ..evidence(Some("releases/old"), None)
    }
}

fn rejected_reason(gate: PromotionGate) -> String {
    match gate {
        PromotionGate::Rejected(reason) => reason,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_live_pid_and_a_fresh_promote_lock_authorize_the_old_promote_script() {
    let fresh = lock_evidence(4242, true, -10);
    assert!(matches!(
        decide_promotion("abc", &fresh, at(0)),
        PromotionGate::Authorized(_)
    ));
    let edge = lock_evidence(4242, true, -PROMOTING_MAX_AGE_SECS);
    assert!(matches!(
        decide_promotion("abc", &edge, at(0)),
        PromotionGate::Authorized(_)
    ));
    // dev・管理外は lock があっても素通しのまま。
    assert!(matches!(
        decide_promotion(DEV_RELEASE, &fresh, at(0)),
        PromotionGate::Skipped(_)
    ));
    let unmanaged = PromotionEvidence {
        release_managed: false,
        ..fresh
    };
    assert!(matches!(
        decide_promotion("abc", &unmanaged, at(0)),
        PromotionGate::Skipped(_)
    ));
}

#[test]
fn a_dead_pid_or_pid_zero_in_promote_lock_is_rejected() {
    let dead = rejected_reason(decide_promotion(
        "abc",
        &lock_evidence(4242, false, -10),
        at(0),
    ));
    assert!(dead.contains("pid 4242 is not running"), "{dead}");
    // pid 0 は `alive` が true でも（`pid_alive(0)` は true を返す）認可しない。
    let zero = rejected_reason(decide_promotion("abc", &lock_evidence(0, true, -10), at(0)));
    assert!(zero.contains("invalid pid 0"), "{zero}");
}

#[test]
fn a_stale_or_future_promote_lock_is_rejected_even_with_a_live_pid() {
    let stale = rejected_reason(decide_promotion(
        "abc",
        &lock_evidence(4242, true, -PROMOTING_MAX_AGE_SECS - 1),
        at(0),
    ));
    assert!(stale.contains(PROMOTE_LOCK_FILE), "{stale}");
    let future = rejected_reason(decide_promotion(
        "abc",
        &lock_evidence(4242, true, PROMOTING_MAX_AGE_SECS + 1),
        at(0),
    ));
    assert!(future.contains("limit 900s"), "{future}");
}

#[test]
fn an_unreadable_promote_lock_is_rejected_with_its_reason() {
    let broken = PromotionEvidence {
        promote_lock: Some(Err("not a pid `x`".into())),
        ..evidence(Some("releases/old"), None)
    };
    let reason = rejected_reason(decide_promotion("abc", &broken, at(0)));
    assert!(reason.contains("not a pid `x`"), "{reason}");
    // lock が無ければ理由に「無い」と出る（規則 1・2 の理由も残る）。
    let none = rejected_reason(decide_promotion("abc", &evidence(None, None), at(0)));
    assert!(none.contains("no promote.lock"), "{none}");
    assert!(none.contains("no promoting.json"), "{none}");
}

#[test]
fn rules_one_and_two_still_win_over_a_bad_promote_lock() {
    let current = PromotionEvidence {
        promote_lock: Some(Ok(PromoteLockFact {
            pid: 4242,
            alive: false,
            modified: at(-10_000),
        })),
        ..evidence(Some("releases/abc"), None)
    };
    assert!(matches!(
        decide_promotion("abc", &current, at(0)),
        PromotionGate::Authorized(_)
    ));
    let marker = PromotionEvidence {
        promote_lock: Some(Err("bad".into())),
        ..evidence(None, Some(("abc", -10)))
    };
    assert!(matches!(
        decide_promotion("abc", &marker, at(0)),
        PromotionGate::Authorized(_)
    ));
}

#[test]
fn read_promotion_evidence_reads_promote_lock_pid_liveness_and_mtime() {
    let dir = tempfile::tempdir().expect("tempdir");
    let releases = dir.path().join("releases");
    let rel = releases.join("abc");
    std::fs::create_dir_all(&rel).expect("mkdir");
    let lock = rel.join(PROMOTE_LOCK_FILE);
    // 自分自身の pid は生きている。前後の空白は許す。
    let me = std::process::id();
    std::fs::write(&lock, format!(" {me}\n")).expect("write");
    let before = OffsetDateTime::now_utc() - time::Duration::seconds(5);
    let ev = read_promotion_evidence(&releases, "abc");
    let fact = ev.promote_lock.clone().expect("some").expect("ok");
    assert_eq!(fact.pid, me);
    assert!(fact.alive);
    assert!(fact.modified >= before, "{:?}", fact.modified);
    assert!(matches!(
        decide_promotion("abc", &ev, OffsetDateTime::now_utc()),
        PromotionGate::Authorized(_)
    ));
    // 古い mtime は拒否。
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&lock)
        .expect("open")
        .set_modified(old)
        .expect("set mtime");
    let ev = read_promotion_evidence(&releases, "abc");
    assert!(matches!(
        decide_promotion("abc", &ev, OffsetDateTime::now_utc()),
        PromotionGate::Rejected(_)
    ));
    // pid でない中身は読めない lock。
    std::fs::write(&lock, "not-a-pid").expect("write");
    let ev = read_promotion_evidence(&releases, "abc");
    assert!(
        matches!(ev.promote_lock, Some(Err(_))),
        "{:?}",
        ev.promote_lock
    );
    // pid 0 は生死を判定せず false。
    std::fs::write(&lock, "0").expect("write");
    let ev = read_promotion_evidence(&releases, "abc");
    let fact = ev.promote_lock.clone().expect("some").expect("ok");
    assert_eq!((fact.pid, fact.alive), (0, false));
}
