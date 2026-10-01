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

/// ADR-0075 D6（Phase G2）: `sccache --show-stats --stats-format=json`（0.18 の形）の要約。
#[test]
fn sccache_stats_summary_reads_the_json_shape_of_0_18() {
    let json = r#"{"stats":{"compile_requests":624,"requests_executed":576,
        "cache_hits":{"counts":{"Rust":163,"C/C++":261,"Assembler":121},"adv_counts":{}},
        "cache_misses":{"counts":{"Rust":29},"adv_counts":{}},"multi_level":null},
        "cache_location":"Local disk: \"/x\"","cache_size":1073741824,"max_cache_size":21474836480,"version":"0.18.0"}"#;
    let s = parse_sccache_stats(json).unwrap();
    assert_eq!(
        s,
        ScratchSccacheStats {
            compile_requests: 624,
            hits: 545,
            misses: 29,
            rust_hits: 163,
            rust_misses: 29,
            cache_size_bytes: Some(1_073_741_824),
        }
    );
    // 起動直後（counts が空、cache_size が null）。
    let s = parse_sccache_stats(
        r#"{"stats":{"compile_requests":0,"cache_hits":{"counts":{}},"cache_misses":{"counts":{}}},"cache_size":null}"#,
    )
    .unwrap();
    assert_eq!((s.hits, s.misses, s.cache_size_bytes), (0, 0, None));
    assert!(parse_sccache_stats("not json").is_none());
}
