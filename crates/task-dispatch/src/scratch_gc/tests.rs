use super::*;
use task_worker::scratch::{AllocateRequest, NoDb};

#[test]
fn effective_max_shrinks_with_usage_outside_the_pool() {
    let g = scratch::GIB;
    // 252G の fs、空き 91G、pool 20G → 外 141G → 252-141-5 = 106G。
    assert_eq!(
        effective_max(150 * g, Some((252 * g, 91 * g)), 20 * g, 5 * g),
        106 * g
    );
    assert_eq!(effective_max(150 * g, None, 0, 0), 150 * g);
    assert_eq!(
        effective_max(150 * g, Some((1000 * g, 900 * g)), 0, 0),
        150 * g
    );
}

#[test]
fn gc_dry_run_lists_without_removing_and_execute_renames() {
    let tmp = tempfile::tempdir().unwrap();
    let s = ScratchSettings::with_dir(tmp.path().join("scratch"));
    let pool = s.pool();
    let owner = Owner::parse("release-0123456789ab").unwrap();
    let none = |_: &AdoptCandidate| None;
    scratch::allocate(
        &pool,
        &AllocateRequest {
            owner: &owner,
            repo_path: Path::new("/repo/x"),
            base_commit: None,
            work_unit_key: None,
            checkout: None,
            candidates: &[],
            distance: &none,
            adopt: false,
            max_distance: 0,
        },
    )
    .unwrap();
    scratch::release(&pool, &owner).unwrap();
    let profile = pool.target_dir(&owner).join("debug");
    std::fs::create_dir_all(&profile).unwrap();
    let lock_path = profile.join(".cargo-lock");
    std::fs::write(&lock_path, "").unwrap();
    let held = nix::fcntl::Flock::lock(
        std::fs::File::open(&lock_path).unwrap(),
        nix::fcntl::FlockArg::LockExclusiveNonblock,
    )
    .unwrap();
    let locked = run_gc(
        &s,
        &[],
        &NoDb,
        false,
        &HashMap::new(),
        0.0,
        false,
        &BTreeSet::new(),
        false,
    );
    assert!(locked.executed.unwrap().removed.is_empty());
    assert!(pool.target_dir(&owner).exists());
    drop(held);
    let run = run_gc(
        &s,
        &[],
        &NoDb,
        false,
        &HashMap::new(),
        0.0,
        false,
        &BTreeSet::new(),
        true,
    );
    // 不変条件: pool の競合中は退避せず即座に戻り、解放後に同じ plan を実行できる。
    // cargo lock だけの既存試験は pool の待機を検出できない。
    let held = pool.lock().unwrap();
    let (finished, receive) = std::sync::mpsc::channel();
    let guard = std::thread::spawn(move || {
        let progressed = receive.recv_timeout(Duration::from_secs(30)).is_ok();
        drop(held);
        progressed
    });
    let deferred = execute(&pool, &run.plan, &run.scan, false);
    let _ = finished.send(());
    assert!(guard.join().unwrap(), "GC waited for the pool lock");
    assert!(deferred.removed.is_empty());
    assert!(pool.target_dir(&owner).exists());
    assert_eq!(run.plan.selected.len(), 1);
    assert!(run.executed.is_none());
    assert!(pool.target_dir(&owner).exists());
    let run = run_gc(
        &s,
        &[],
        &NoDb,
        false,
        &HashMap::new(),
        0.0,
        false,
        &BTreeSet::new(),
        false,
    );
    assert_eq!(run.executed.unwrap().removed.len(), 1);
    assert!(!pool.target_dir(&owner).exists());
    // lease は「刈った」記録として残る。
    assert!(pool.lease_path(&owner).exists());
    let roots = deleting_roots(&pool, &[]);
    assert!(has_pending_deletes(&roots));
    let busy = Arc::new(AtomicBool::new(false));
    spawn_removal(roots.clone(), busy.clone());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while (busy.load(Ordering::SeqCst) || has_pending_deletes(&roots))
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!has_pending_deletes(&roots));
}

#[test]
fn legacy_paths_list_build_cache_entries_and_the_release_target() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("build-cache");
    std::fs::create_dir_all(cache.join("cargo/agent-platform-abc")).unwrap();
    std::fs::create_dir_all(cache.join("cargo/.deleting-x")).unwrap();
    std::fs::write(cache.join("cargo/file"), "x").unwrap();
    let releases = tmp.path().join("releases");
    let real = tmp.path().join("release-build/.cargo-target");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::create_dir_all(&releases).unwrap();
    std::os::unix::fs::symlink(&real, releases.join(".cargo-target")).unwrap();
    let got = legacy_paths(&cache, Some(&releases));
    assert_eq!(got, vec![cache.join("cargo/agent-platform-abc"), real]);
}

#[test]
fn measurement_is_cached_and_written_to_the_lease_without_touching_it() {
    let tmp = tempfile::tempdir().unwrap();
    let s = ScratchSettings::with_dir(tmp.path());
    let pool = s.pool();
    let owner = Owner::parse("agent-x").unwrap();
    let none = |_: &AdoptCandidate| None;
    let a = scratch::allocate(
        &pool,
        &AllocateRequest {
            owner: &owner,
            repo_path: Path::new("/repo/x"),
            base_commit: None,
            work_unit_key: None,
            checkout: None,
            candidates: &[],
            distance: &none,
            adopt: false,
            max_distance: 0,
        },
    )
    .unwrap();
    std::fs::write(a.target_dir.join("blob"), vec![1u8; 8192]).unwrap();
    let sizes: SizeCache = Arc::new(Mutex::new(HashMap::new()));
    let scan1 = scan(&s, &[], &NoDb, false, &HashMap::new(), SystemTime::now());
    let (path, lease) = next_to_measure(&scan1, &HashMap::new()).unwrap();
    assert_eq!(path, a.target_dir);
    let before = scratch::mtime(&pool.lease_path(&owner));
    measure_one(
        &path,
        lease.as_deref(),
        &sizes,
        &Arc::new(Mutex::new(scratch::SharedExtentIndex::default())),
    );
    assert!(sizes.lock().unwrap()[&path].bytes >= 8192);
    let l = scratch::read_lease(&pool.lease_path(&owner))
        .unwrap()
        .unwrap();
    assert!(l.size_bytes.unwrap() >= 8192);
    assert_eq!(scratch::mtime(&pool.lease_path(&owner)), before);
}

/// ADR-0129 (1): sccache / cache server は Celeris の外。status の両欄は常に `None` で、旧 `sccache-l1/`・
/// `cache-l1/` は GC（execute）でも自動では消さない。
#[test]
fn status_has_no_sccache_or_cache_server_and_gc_keeps_old_l1_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let s = ScratchSettings::with_dir(tmp.path().join("scratch"));
    let pool = s.pool();
    let old_dirs = [pool.root().join("sccache-l1"), pool.root().join("cache-l1")];
    for dir in &old_dirs {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("blob"), b"x").unwrap();
    }
    let run = run_gc(
        &s,
        &[],
        &NoDb,
        false,
        &HashMap::new(),
        0.0,
        true,
        &BTreeSet::new(),
        false,
    );
    let now = SystemTime::now();
    let status = build_status(
        &s,
        &run.scan,
        &run.plan,
        run.fs,
        0,
        &HashMap::new(),
        None,
        now,
    );
    assert!(status.sccache.is_none());
    assert!(status.cache.is_none());
    let disabled = disabled_status(&s, now);
    assert!(disabled.sccache.is_none());
    assert!(disabled.cache.is_none());
    for dir in &old_dirs {
        assert!(dir.join("blob").exists());
    }
}

// ---------------------------------------------------------------------------
// ADR-0129 (4)(5): seed の更新と GC
// ---------------------------------------------------------------------------

fn seed_fake_build(_: &Path, env: &[(String, String)]) -> std::io::Result<()> {
    let target = env
        .iter()
        .find(|(k, _)| k == "CARGO_TARGET_DIR")
        .map(|(_, v)| PathBuf::from(v))
        .ok_or_else(|| std::io::Error::other("no CARGO_TARGET_DIR"))?;
    std::fs::create_dir_all(target.join("debug"))?;
    std::fs::write(target.join("debug/libx.rlib"), vec![3u8; 256 * 1024])
}

fn seed_no_checkout(_: &Path, checkout: &Path, _: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(checkout)
}

fn seed_rustc(_: &Path) -> Option<String> {
    Some("rustc 1.98.1".to_string())
}

fn seed_ops() -> scratch::SeedBuildOps<'static> {
    scratch::SeedBuildOps {
        prepare_checkout: &seed_no_checkout,
        build: &seed_fake_build,
        rustc: &seed_rustc,
        share_probe: &|_| Ok(()),
        probe_results: std::sync::Mutex::new(std::collections::HashMap::new()),
    }
}

#[test]
fn seed_skipped_when_pool_cannot_share_in_refresh_batch() {
    let tmp = tempfile::tempdir().unwrap();
    let settings = ScratchSettings::with_dir(tmp.path().join("scratch"));
    let probes = std::cell::Cell::new(0);
    let unavailable = |_: &scratch::Pool| {
        probes.set(probes.get() + 1);
        Err("test filesystem cannot share extents".to_string())
    };
    let no_build = |_: &Path, _: &[(String, String)]| panic!("cargo build must not run");
    let ops = scratch::SeedBuildOps {
        prepare_checkout: &seed_no_checkout,
        build: &no_build,
        rustc: &seed_rustc,
        share_probe: &unavailable,
        probe_results: std::sync::Mutex::new(std::collections::HashMap::new()),
    };
    let repos = ["/repo/a", "/repo/b"].map(|path| SeedRepo {
        path: PathBuf::from(path),
        commit: Some("c1".to_string()),
        cargo: true,
    });
    let steps = refresh_seeds(
        &settings,
        &repos,
        &seed_rustc,
        &|_| None,
        &ops,
        SystemTime::UNIX_EPOCH,
    );
    assert_eq!(probes.get(), 1);
    assert!(steps.iter().all(|(_, step)| matches!(
        step,
        SeedStep::Held { reason } if reason.contains("cannot share extents")
    )));
    assert!(!scratch::seeds_dir(&settings.pool()).exists());
}

/// owner の semantic GC（緊急でも）は seed を消さず、`targets_bytes` にも数えない。退避した seed は削除スレッドの根にある。
#[test]
fn seed_is_not_reclaimed_by_owner_gc_even_in_emergency() {
    let tmp = tempfile::tempdir().unwrap();
    let s = ScratchSettings::with_dir(tmp.path().join("scratch"));
    let pool = s.pool();
    let repo = Path::new("/repo/x");
    let out = scratch::refresh_seed(
        &pool,
        repo,
        "1111111111111111",
        &s.cargo,
        &seed_ops(),
        SystemTime::now(),
    );
    assert!(matches!(out, scratch::SeedRefreshOutcome::Refreshed { .. }));
    let key = task_worker::build_cache::repo_cache_key(repo);
    let owner = Owner::parse("release-0123456789ab").unwrap();
    let none = |_: &AdoptCandidate| None;
    scratch::allocate(
        &pool,
        &AllocateRequest {
            owner: &owner,
            repo_path: repo,
            base_commit: None,
            work_unit_key: None,
            checkout: None,
            candidates: &[],
            distance: &none,
            adopt: false,
            max_distance: 0,
        },
    )
    .unwrap();
    scratch::release(&pool, &owner).unwrap();
    let run = run_gc(
        &s,
        &[],
        &NoDb,
        false,
        &HashMap::new(),
        99.9,
        true,
        &BTreeSet::new(),
        false,
    );
    assert!(
        run.plan
            .selected
            .iter()
            .all(|p| !p.path.starts_with(scratch::seeds_dir(&pool)))
    );
    assert!(
        run.scan
            .entries
            .iter()
            .all(|e| e.in_pool && e.owner.is_some())
    );
    assert!(!pool.target_dir(&owner).exists());
    assert!(scratch::current_generation(&pool, &key).is_some());
    assert!(scratch::read_current_manifest(&pool, &key).is_some());
    assert!(deleting_roots(&pool, &[]).contains(&scratch::seed_deleting_root(&pool)));
}

/// 更新の 1 回: commit 不明・cargo でない repo は飛ばし、main が進めば作り直し、一致すれば何もしない。容量が
/// 足りなければ保留して旧 seed を残す。
#[test]
fn seed_refresh_seeds_follows_main_and_holds_on_low_space() {
    let tmp = tempfile::tempdir().unwrap();
    let s = ScratchSettings::with_dir(tmp.path().join("scratch"));
    let pool = s.pool();
    let a = PathBuf::from("/repo/a");
    let repos = |commit: &str| {
        vec![
            SeedRepo {
                path: PathBuf::from("/repo/none"),
                commit: None,
                cargo: false,
            },
            SeedRepo {
                path: PathBuf::from("/repo/docs"),
                commit: Some("d".into()),
                cargo: false,
            },
            SeedRepo {
                path: a.clone(),
                commit: Some(commit.to_string()),
                cargo: true,
            },
        ]
    };
    let no_hold = |_: Option<u64>| None;
    let ops = seed_ops();
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let steps = refresh_seeds(&s, &repos("c1"), &seed_rustc, &no_hold, &ops, t);
    assert_eq!(steps[0].1, SeedStep::NoCommit);
    assert_eq!(steps[1].1, SeedStep::NotCargo);
    assert!(matches!(
        &steps[2].1,
        SeedStep::Done(scratch::SeedRefreshOutcome::Refreshed { replaced: None, .. })
    ));
    let key = task_worker::build_cache::repo_cache_key(&a);
    let g1 = scratch::current_generation(&pool, &key).unwrap();
    // main が同じなら作り直さない。
    let steps = refresh_seeds(&s, &repos("c1"), &seed_rustc, &no_hold, &ops, t);
    assert_eq!(steps[2].1, SeedStep::UpToDate);
    // main が進んでも容量が足りなければ保留（hold には今の seed の大きさが渡る）。
    let seen = Mutex::new(None);
    let hold = |b: Option<u64>| {
        *seen.lock().unwrap() = b;
        Some("low".to_string())
    };
    let steps = refresh_seeds(&s, &repos("c2"), &seed_rustc, &hold, &ops, t);
    assert_eq!(
        steps[2].1,
        SeedStep::Held {
            reason: "low".into()
        }
    );
    assert!(seen.lock().unwrap().unwrap_or(0) >= 256 * 1024);
    assert_eq!(scratch::current_generation(&pool, &key), Some(g1.clone()));
    // 空きが戻れば進んだ main で作り直す。
    let steps = refresh_seeds(
        &s,
        &repos("c2"),
        &seed_rustc,
        &no_hold,
        &ops,
        t + Duration::from_secs(1),
    );
    assert!(
        matches!(
            &steps[2].1,
            SeedStep::Done(scratch::SeedRefreshOutcome::Refreshed { replaced: Some(r), .. }) if *r == g1
        ),
        "{:?}",
        steps[2].1
    );
    assert_eq!(
        scratch::read_current_manifest(&pool, &key)
            .unwrap()
            .commit
            .as_deref(),
        Some("c2")
    );
}

// ---------------------------------------------------------------------------
// ADR 2026-11-11-disk-aware-dispatch 付記（2026-11-12、親の数え方）
// ---------------------------------------------------------------------------

/// 親 task とその WU の状態を与える lookup。
struct ParentLookup {
    parent: task_core::Status,
    units: Vec<scratch::UnitTargetUse>,
}
impl StatusLookup for ParentLookup {
    fn task_status(&self, id: &str) -> Option<task_core::Status> {
        Some(match id {
            "P" => self.parent,
            _ => task_core::Status::Done,
        })
    }
    /// `wu-W` は自分の worktree の WU（`shares_task_worktree = false` の材料と同じ状態）。
    fn work_unit_status(&self, _: &str) -> Option<task_core::WorkUnitStatus> {
        self.units
            .iter()
            .find(|u| !u.shares_task_worktree)
            .map(|u| u.status)
    }
    fn task_work_units(&self, id: &str) -> Vec<scratch::UnitTargetUse> {
        if id == "P" {
            self.units.clone()
        } else {
            Vec::new()
        }
    }
}

/// 不変条件（受け入れ条件 3）: 親 task が `running` で自分の worktree の WU の run が 1 本走っているとき、走っている
/// target の増分（`admission.running_growth_bytes`）はその WU の owner の分だけで、親の owner（lease だけで target の
/// 無い `task-<id>`）は数えない。Task の worktree を共有する WU が走る親と、`reviewing` の親（review の検査）は数える。
/// `disk_admission` の試験は `Sample` の `grows` を直接与え、`task_worker::scratch` の試験は分類の理由までしか見ない
/// ので、pool の走査から status の増分までの経路で本番の形（WU 1 本で repo の見込みの 2 倍）を確かめるのはここだけ。
#[test]
fn a_parent_waiting_for_its_work_unit_reserves_only_the_work_unit_growth() {
    const GB: u64 = 1_000_000_000;
    let tmp = tempfile::tempdir().unwrap();
    let s = ScratchSettings::with_dir(tmp.path().join("scratch"));
    let pool = s.pool();
    let none = |_: &AdoptCandidate| None;
    let mut sizes = HashMap::new();
    let now = SystemTime::now();
    for (owner, size) in [
        ("task-old", Some(70 * GB)),
        ("task-P", None),
        ("task-P/wu-W", Some(4 * GB)),
    ] {
        let owner = Owner::parse(owner).unwrap();
        scratch::allocate(
            &pool,
            &AllocateRequest {
                owner: &owner,
                repo_path: Path::new("/repo/ap"),
                base_commit: None,
                work_unit_key: None,
                checkout: None,
                candidates: &[],
                distance: &none,
                adopt: false,
                max_distance: 0,
            },
        )
        .unwrap();
        let target = pool.target_dir(&owner);
        match size {
            Some(bytes) => {
                std::fs::create_dir_all(&target).unwrap();
                sizes.insert(
                    target,
                    Measured {
                        bytes,
                        last_write: now,
                        measured_at: now,
                    },
                );
            }
            // 親の dir は lease と `wu-*` だけ（本番の 2026-11-12 の形）。
            None => {
                let _ = std::fs::remove_dir_all(&target);
            }
        }
    }
    let admission = |lookup: &ParentLookup| {
        let run = run_gc(
            &s,
            &[],
            lookup,
            true,
            &sizes,
            0.0,
            false,
            &BTreeSet::new(),
            true,
        );
        build_status(&s, &run.scan, &run.plan, run.fs, 0, &sizes, None, now)
    };
    let reason = |st: &ScratchStatus, owner: &str| {
        st.owners
            .iter()
            .find(|o| o.owner == owner)
            .map(|o| o.reason.clone())
    };
    let growth = |a: &task_ops::daemon::ScratchAdmissionView, owner: &str| {
        a.owners
            .iter()
            .find(|o| o.owner == owner)
            .map(|o| (o.grows, o.growth_bytes))
    };
    use task_core::Status::{Reviewing, Running};
    use task_core::WorkUnitStatus as W;
    let own_tree = |status| scratch::UnitTargetUse {
        status,
        shares_task_worktree: false,
    };
    let shared = |status| scratch::UnitTargetUse {
        status,
        shares_task_worktree: true,
    };
    // 自分の worktree の WU が 1 本走っているだけ: 増分は WU の 66 GB だけ（旧い数え方は親の 70 GB を足して 136 GB）。
    // 親は P0 のまま（消さない）で、target が無いので見込みの材料にも出ない。
    let st = admission(&ParentLookup {
        parent: Running,
        units: vec![own_tree(W::Running), shared(W::Pending)],
    });
    assert_eq!(
        reason(&st, "task-P").as_deref(),
        Some(scratch::TASK_AWAITING_WORK_UNITS)
    );
    let a = st.admission;
    assert_eq!(a.expected_by_repo.values().copied().max(), Some(70 * GB));
    assert_eq!(growth(&a, "task-P/wu-W"), Some((true, 66 * GB)));
    assert_eq!(growth(&a, "task-P"), None);
    assert_eq!(a.running_growth_bytes, 66 * GB);
    // Task の worktree を共有する WU（統合 WU・repair WU）が走る間は、親の owner の増分を数える。
    let a = admission(&ParentLookup {
        parent: Running,
        units: vec![own_tree(W::Done), shared(W::Running)],
    })
    .admission;
    assert_eq!(growth(&a, "task-P"), Some((true, 70 * GB)));
    assert_eq!(a.running_growth_bytes, 70 * GB);
    // review の検査は親の owner で build する。
    let a = admission(&ParentLookup {
        parent: Reviewing,
        units: vec![own_tree(W::Done)],
    })
    .admission;
    assert_eq!(growth(&a, "task-P"), Some((true, 70 * GB)));
}
