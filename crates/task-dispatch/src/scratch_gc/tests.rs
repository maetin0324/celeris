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
    let run = run_gc(
        &s,
        &[],
        &NoDb,
        false,
        &HashMap::new(),
        0,
        false,
        &BTreeSet::new(),
        true,
    );
    assert_eq!(run.plan.selected.len(), 1);
    assert!(run.executed.is_none());
    assert!(pool.target_dir(&owner).exists());
    let run = run_gc(
        &s,
        &[],
        &NoDb,
        false,
        &HashMap::new(),
        0,
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
    measure_one(&path, lease.as_deref(), &sizes);
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
        0,
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
    }
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
        u64::MAX,
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
