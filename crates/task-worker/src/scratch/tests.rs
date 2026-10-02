use super::*;
use std::collections::HashMap;

const T0: u64 = 1_700_000_000;

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn entry(id: &str, class: Class, last: u64, size: Option<u64>, repo: &str) -> GcEntry {
    let owner = Owner::parse(id).ok();
    GcEntry {
        id: id.to_string(),
        path: PathBuf::from(format!("/scratch/targets/{id}/target")),
        owner,
        repo_key: Some(repo.to_string()),
        class,
        reason: String::new(),
        last_write: at(last),
        size_bytes: size,
        in_pool: true,
    }
}

fn params(max: u64) -> GcParams {
    GcParams {
        now: at(T0 + 100_000),
        targets_max_bytes: max,
        high_watermark: 0.9,
        low_watermark: 0.7,
        min_free_bytes: 0,
        fs_free_bytes: None,
        emergency: false,
        completed_grace_secs: 600,
        warm_seeds_per_repo: 1,
        max_per_tick: 100,
        active_repo_keys: BTreeSet::new(),
    }
}

fn ids(plan: &GcPlan) -> Vec<&str> {
    plan.selected.iter().map(|p| p.id.as_str()).collect()
}

#[test]
fn owner_paths_nest_work_units_under_their_task() {
    let pool = Pool::new("/var/lib/celeris/scratch");
    let task = Owner::task("01TASK");
    let wu = Owner::work_unit("01TASK", "01WU");
    assert_eq!(
        pool.target_dir(&task),
        PathBuf::from("/var/lib/celeris/scratch/targets/task-01TASK/target")
    );
    assert_eq!(
        pool.target_dir(&wu),
        PathBuf::from("/var/lib/celeris/scratch/targets/task-01TASK/wu-01WU/target")
    );
    assert!(pool.owner_dir(&wu).starts_with(pool.owner_dir(&task)));
    assert_eq!(wu.to_string(), "task-01TASK/wu-01WU");
    assert_eq!(Owner::parse("task-01TASK/wu-01WU").unwrap(), wu);
    assert_eq!(
        Owner::parse("release-0123456789ab").unwrap().kind(),
        OwnerKind::Release
    );
    assert_eq!(
        Owner::parse("agent-agent-a5caa712b0867e383").unwrap(),
        Owner::Agent {
            name: "agent-a5caa712b0867e383".into()
        }
    );
    assert_eq!(wu.flat(), "task-01TASK__wu-01WU");
    for bad in [
        "",
        "x-1",
        "task-",
        "task-a/b",
        "release-../x",
        "agent-.hidden",
        "agent-a/b",
    ] {
        assert!(Owner::parse(bad).is_err(), "{bad}");
    }
    assert_eq!(
        target_env(&pool, &wu),
        vec![(
            "CARGO_TARGET_DIR".to_string(),
            "/var/lib/celeris/scratch/targets/task-01TASK/wu-01WU/target".to_string()
        )]
    );
}

#[test]
fn plan_gc_never_selects_pinned_owners() {
    let entries = vec![
        entry("task-A", Class::Pinned, T0, Some(90), "r"),
        entry("task-B", Class::Pinned, T0, Some(90), "r"),
        entry("task-C", Class::Waiting, T0, Some(1), "r"),
    ];
    // watermark も空きも目標に届かない・緊急でも P0 は選ばない。
    let mut p = params(100);
    p.emergency = true;
    p.fs_free_bytes = Some(0);
    p.min_free_bytes = 1_000_000;
    let plan = plan_gc(&entries, &p);
    assert_eq!(plan.pressure, Pressure::Emergency);
    assert_eq!(ids(&plan), vec!["task-C"]);
    assert_eq!(plan.pinned_bytes, 180);
}

#[test]
fn plan_gc_follows_the_semantic_order_and_stops_at_low_watermark() {
    // max 100、used 100 > 90 → low 70 まで 30 減らす。各 10。
    let mut legacy = entry("/cache/cargo/old", Class::Legacy, T0 + 50, Some(10), "r");
    legacy.owner = None;
    legacy.in_pool = false;
    let entries = vec![
        entry("task-P1", Class::Waiting, T0, Some(10), "r"),
        entry("task-P2", Class::Retry, T0, Some(10), "r"),
        entry("agent-old", Class::Completed, T0 + 10, Some(10), "r"),
        entry("agent-new", Class::Completed, T0 + 20, Some(10), "r"),
        entry("task-Pin", Class::Pinned, T0, Some(40), "r"),
        entry("stray-x", Class::Stray, T0 + 40, Some(10), "r"),
        entry("agent-seed", Class::Completed, T0 + 30, Some(10), "r"),
        legacy,
    ];
    let mut p = params(100);
    p.active_repo_keys.insert("r".into());
    let plan = plan_gc(&entries, &p);
    assert_eq!(plan.pressure, Pressure::HighWatermark);
    assert_eq!(plan.used_bytes, 100);
    assert_eq!(plan.pool_need_bytes, 30);
    // ① stray / legacy（即回収、legacy は pool の外なので pool の目標には数えない）→ ② P3 LRU → 止まる。
    assert_eq!(
        ids(&plan),
        vec!["stray-x", "/cache/cargo/old", "agent-old", "agent-new"]
    );
    // 目標を大きくすると seed → P2 → P1 の順に続く（P0 は選ばない）。
    let mut p = params(10);
    p.active_repo_keys.insert("r".into());
    let plan = plan_gc(&entries, &p);
    assert_eq!(
        ids(&plan),
        vec![
            "stray-x",
            "/cache/cargo/old",
            "agent-old",
            "agent-new",
            "agent-seed",
            "task-P2",
            "task-P1"
        ]
    );
    assert!(
        plan.selected
            .iter()
            .find(|s| s.id == "agent-seed")
            .unwrap()
            .seed
    );
    // 目標が無ければ即回収の分だけ（agent の P3 と seed・P2・P1 は残る）。
    let mut p = params(1_000_000);
    p.active_repo_keys.insert("r".into());
    let plan = plan_gc(&entries, &p);
    assert_eq!(plan.pressure, Pressure::None);
    assert_eq!(ids(&plan), vec!["stray-x", "/cache/cargo/old"]);
    // 1 回の件数の上限。
    let mut p = params(10);
    p.max_per_tick = 2;
    assert_eq!(ids(&plan_gc(&entries, &p)).len(), 2);
}

#[test]
fn plan_gc_reclaims_finished_work_units_and_releases_immediately_but_task_after_grace() {
    let now = T0 + 100_000;
    let entries = vec![
        entry("task-T/wu-W", Class::Completed, now - 5, Some(10), "r"),
        entry("release-abc", Class::Completed, now - 5, Some(10), "q"),
        entry("task-Fresh", Class::Completed, now - 5, Some(10), "q"),
        entry("task-Old", Class::Completed, now - 601, Some(10), "q"),
        entry("task-W/wu-X", Class::Waiting, now - 5, Some(10), "r"),
    ];
    let plan = plan_gc(&entries, &params(1_000_000));
    assert_eq!(ids(&plan), vec!["task-Old", "release-abc", "task-T/wu-W"]);
}

#[test]
fn plan_gc_keeps_one_warm_seed_per_repo() {
    let now = T0 + 100_000;
    let entries = vec![
        entry("task-T/wu-a", Class::Completed, now - 30, Some(10), "r"),
        entry("task-T/wu-b", Class::Completed, now - 10, Some(10), "r"),
        entry("task-T/wu-c", Class::Completed, now - 20, Some(10), "r"),
        entry(
            "task-U/wu-d",
            Class::Completed,
            now - 10,
            Some(10),
            "unused",
        ),
        entry("task-Live", Class::Pinned, now, Some(10), "r"),
    ];
    let mut p = params(1_000_000);
    p.active_repo_keys.insert("r".into());
    let plan = plan_gc(&entries, &p);
    assert_eq!(plan.seeds, BTreeSet::from(["task-T/wu-b".to_string()]));
    // seed（最新の wu-b）は即回収しない。使われていない repo の seed は作らない。
    assert_eq!(
        ids(&plan),
        vec!["task-T/wu-a", "task-T/wu-c", "task-U/wu-d"]
    );
}

#[test]
fn plan_gc_treats_unmeasured_owners_as_the_repo_maximum() {
    let entries = vec![
        entry("task-A", Class::Pinned, T0, Some(50), "r"),
        entry("task-B", Class::Completed, T0, None, "r"),
        entry("agent-c", Class::Completed, T0, None, "other"),
        entry("agent-d", Class::Completed, T0, Some(5), "other"),
    ];
    let plan = plan_gc(&entries, &params(1000));
    assert_eq!(plan.estimated["task-B"], 50);
    assert_eq!(plan.estimated["agent-c"], 5);
    assert_eq!(plan.used_bytes, 110);
    // 未測定を 0 とみなすと超えない上限でも、repo の最大値で数えれば high watermark を超える。
    let plan = plan_gc(&entries, &params(120));
    assert_eq!(plan.pressure, Pressure::HighWatermark);
}

struct Db {
    tasks: HashMap<String, Status>,
    wus: HashMap<String, WorkUnitStatus>,
}
impl StatusLookup for Db {
    fn task_status(&self, id: &str) -> Option<Status> {
        self.tasks.get(id).copied()
    }
    fn work_unit_status(&self, id: &str) -> Option<WorkUnitStatus> {
        self.wus.get(id).copied()
    }
}

fn lease_for(owner: &Owner) -> Lease {
    Lease {
        schema: LEASE_SCHEMA.into(),
        owner: owner.to_string(),
        kind: owner.kind(),
        repo_key: "r".into(),
        repo_path: "/repo".into(),
        base_commit: None,
        work_unit_key: None,
        created_at: rfc3339(at(T0)),
        released_at: None,
        size_bytes: None,
        measured_at: None,
        adopted_from: None,
        ttl_secs: None,
    }
}

#[test]
fn classify_follows_the_adr_table() {
    let s = ScratchSettings::with_dir("/s");
    let db = Db {
        tasks: HashMap::from([
            ("R".to_string(), Status::Running),
            ("V".to_string(), Status::Reviewing),
            ("B".to_string(), Status::Blocked),
            ("F".to_string(), Status::Failed),
            ("D".to_string(), Status::Done),
        ]),
        wus: HashMap::from([
            ("w-run".to_string(), WorkUnitStatus::Running),
            ("w-pend".to_string(), WorkUnitStatus::Pending),
            ("w-fail".to_string(), WorkUnitStatus::Failed),
            ("w-block".to_string(), WorkUnitStatus::Blocked),
            ("w-done".to_string(), WorkUnitStatus::Done),
            ("w-sup".to_string(), WorkUnitStatus::Superseded),
        ]),
    };
    let now = at(T0 + 10);
    let c = |o: &str| {
        let owner = Owner::parse(o).unwrap();
        classify(&owner, Some(&lease_for(&owner)), at(T0), now, &s, &db, true).0
    };
    assert_eq!(c("task-R"), Class::Pinned);
    assert_eq!(c("task-V"), Class::Pinned);
    assert_eq!(c("task-B"), Class::Waiting);
    assert_eq!(c("task-F"), Class::Retry);
    assert_eq!(c("task-D"), Class::Completed);
    assert_eq!(c("task-Gone"), Class::Completed);
    assert_eq!(c("task-R/wu-w-run"), Class::Pinned);
    assert_eq!(c("task-R/wu-w-pend"), Class::Pinned);
    assert_eq!(c("task-R/wu-w-fail"), Class::Waiting);
    assert_eq!(c("task-R/wu-w-block"), Class::Waiting);
    assert_eq!(c("task-R/wu-w-done"), Class::Completed);
    assert_eq!(c("task-R/wu-w-sup"), Class::Completed);
    assert_eq!(c("task-D/wu-w-run"), Class::Completed);
    // 保持期限を過ぎた P1 / P2 は P3。
    let late = at(T0 + 172_800 + 1);
    let owner = Owner::task("B");
    assert_eq!(
        classify(
            &owner,
            Some(&lease_for(&owner)),
            at(T0),
            late,
            &s,
            &db,
            true
        )
        .0,
        Class::Completed
    );
    let owner = Owner::task("F");
    assert_eq!(
        classify(
            &owner,
            Some(&lease_for(&owner)),
            at(T0),
            at(T0 + 86_401),
            &s,
            &db,
            true
        )
        .0,
        Class::Completed
    );
    // DB が無い（celerisctl で DB を開けない）ときは daemon 由来の owner を消さない。
    let owner = Owner::task("Gone");
    assert_eq!(
        classify(
            &owner,
            Some(&lease_for(&owner)),
            at(T0),
            now,
            &s,
            &NoDb,
            false
        )
        .0,
        Class::Pinned
    );
    // lease の無いディレクトリ: 1h 未満は作りかけ（P0）、以後は野良。
    assert_eq!(
        classify(&owner, None, at(T0), now, &s, &db, true).0,
        Class::Pinned
    );
    assert_eq!(
        classify(&owner, None, at(T0), at(T0 + 3600), &s, &db, true).0,
        Class::Stray
    );
}

#[test]
fn external_lease_expires_after_ttl() {
    let s = ScratchSettings::with_dir("/s");
    let owner = Owner::parse("release-0123456789ab").unwrap();
    let mut lease = lease_for(&owner);
    let c = |lease: &Lease, now: u64| {
        classify(&owner, Some(lease), at(T0), at(now), &s, &NoDb, false).0
    };
    assert_eq!(c(&lease, T0 + 21_599), Class::Pinned);
    assert_eq!(c(&lease, T0 + 21_600), Class::Completed);
    // lease ごとの TTL（`--ttl`）が既定より優先。
    lease.ttl_secs = Some(60);
    assert_eq!(c(&lease, T0 + 59), Class::Pinned);
    assert_eq!(c(&lease, T0 + 60), Class::Completed);
    lease.ttl_secs = None;
    lease.released_at = Some(rfc3339(at(T0 + 1)));
    assert_eq!(c(&lease, T0 + 2), Class::Completed);
}

#[test]
fn lease_touch_release_round_trip_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let owner = Owner::parse("agent-x").unwrap();
    let none = |_: &AdoptCandidate| None;
    let req = AllocateRequest {
        owner: &owner,
        repo_path: Path::new("/repo/agent-platform"),
        base_commit: Some("abc".into()),
        work_unit_key: None,
        checkout: None,
        candidates: &[],
        distance: &none,
        adopt: true,
        max_distance: 200,
    };
    let a = allocate(&pool, &req).unwrap();
    assert!(a.created);
    assert_eq!(a.target_dir, pool.target_dir(&owner));
    assert!(a.target_dir.is_dir());
    let lease_path = pool.lease_path(&owner);
    set_mtime(&lease_path, at(T0)).unwrap();
    assert!(touch(&pool, &owner).unwrap());
    assert!(mtime(&lease_path).unwrap() > at(T0));
    assert!(release(&pool, &owner).unwrap());
    assert!(
        read_lease(&lease_path)
            .unwrap()
            .unwrap()
            .released_at
            .is_some()
    );
    // 再び lease を取ると released_at が消える。
    let a = allocate(&pool, &req).unwrap();
    assert!(!a.created);
    assert!(
        read_lease(&lease_path)
            .unwrap()
            .unwrap()
            .released_at
            .is_none()
    );
    // 測定は mtime を変えない。
    set_mtime(&lease_path, at(T0)).unwrap();
    write_lease_measurement(&lease_path, 42, at(T0 + 5)).unwrap();
    assert_eq!(mtime(&lease_path).unwrap(), at(T0));
    assert_eq!(
        read_lease(&lease_path).unwrap().unwrap().size_bytes,
        Some(42)
    );
    assert!(!touch(&pool, &Owner::parse("agent-none").unwrap()).unwrap());
}

fn cand(owner: &str, last: u64, base: Option<&str>) -> AdoptCandidate {
    AdoptCandidate {
        owner: Owner::parse(owner).unwrap(),
        repo_key: "r".into(),
        base_commit: base.map(str::to_string),
        last_write: at(last),
        lease_mtime: None,
    }
}

#[test]
fn adopt_requires_the_target_to_predate_the_checkout() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let repo = Path::new("/repo/agent-platform");
    let key = crate::build_cache::repo_cache_key(repo);
    let none = |_: &AdoptCandidate| None;
    // 候補（P3 の task-Old）: target の中に目印のファイル、最終書き込みを T0 に固定。
    let old = Owner::task("Old");
    let alloc = |owner: &Owner, checkout: Option<SystemTime>, cands: &[AdoptCandidate]| {
        allocate(
            &pool,
            &AllocateRequest {
                owner,
                repo_path: repo,
                base_commit: None,
                work_unit_key: None,
                checkout,
                candidates: cands,
                distance: &none,
                adopt: true,
                max_distance: 200,
            },
        )
        .unwrap()
    };
    alloc(&old, None, &[]);
    std::fs::write(pool.target_dir(&old).join("marker"), "x").unwrap();
    for p in [
        pool.target_dir(&old).join("marker"),
        pool.target_dir(&old),
        pool.lease_path(&old),
    ] {
        set_mtime_any(&p, at(T0));
    }
    let candidate = AdoptCandidate {
        owner: old.clone(),
        repo_key: key.clone(),
        base_commit: None,
        last_write: at(T0),
        lease_mtime: mtime(&pool.lease_path(&old)),
    };
    // checkout が候補の最終書き込みより前 → adopt しない（空から）。
    let a = alloc(
        &Owner::task("Early"),
        Some(at(T0 - 1)),
        std::slice::from_ref(&candidate),
    );
    assert_eq!(a.adopted_from, None);
    assert!(!a.target_dir.join("marker").exists());
    assert!(pool.target_dir(&old).join("marker").exists());
    // 同時刻も不可（「前」であること）。
    let a = alloc(
        &Owner::task("Same"),
        Some(at(T0)),
        std::slice::from_ref(&candidate),
    );
    assert_eq!(a.adopted_from, None);
    // checkout が後 → rename で引き継ぐ。
    let a = alloc(
        &Owner::task("Late"),
        Some(at(T0 + 1)),
        std::slice::from_ref(&candidate),
    );
    assert_eq!(a.adopted_from.as_deref(), Some("task-Old"));
    assert!(a.target_dir.join("marker").exists());
    assert!(!pool.target_dir(&old).exists());
    let lease = read_lease(&pool.lease_path(&Owner::task("Late")))
        .unwrap()
        .unwrap();
    assert_eq!(lease.adopted_from.as_deref(), Some("task-Old"));
}

fn set_mtime_any(path: &Path, t: SystemTime) {
    let f = std::fs::File::open(path).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(t))
        .unwrap();
}

#[test]
fn adopt_skips_a_candidate_that_was_leased_again_after_the_scan() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let repo = Path::new("/repo/x");
    let key = crate::build_cache::repo_cache_key(repo);
    let none = |_: &AdoptCandidate| None;
    let old = Owner::task("Old");
    fn mk<'a>(
        owner: &'a Owner,
        repo: &'a Path,
        none: &'a dyn Fn(&AdoptCandidate) -> Option<u64>,
    ) -> AllocateRequest<'a> {
        AllocateRequest {
            owner,
            repo_path: repo,
            base_commit: None,
            work_unit_key: None,
            checkout: None,
            candidates: &[],
            distance: none,
            adopt: true,
            max_distance: 200,
        }
    }
    allocate(&pool, &mk(&old, repo, &none)).unwrap();
    for p in [pool.target_dir(&old), pool.lease_path(&old)] {
        set_mtime_any(&p, at(T0));
    }
    let candidate = AdoptCandidate {
        owner: old.clone(),
        repo_key: key,
        base_commit: None,
        last_write: at(T0),
        // 走査のときの mtime と違う → 再び使われ始めた。
        lease_mtime: Some(at(T0 - 100)),
    };
    let new = Owner::task("New");
    let cands = [candidate];
    let mut r = mk(&new, repo, &none);
    r.checkout = Some(at(T0 + 10));
    r.candidates = &cands;
    assert_eq!(allocate(&pool, &r).unwrap().adopted_from, None);
    assert!(pool.target_dir(&old).exists());
}

#[test]
fn adopt_prefers_the_nearest_commit_within_the_limit() {
    let cands = vec![
        cand("task-far", T0 + 30, Some("far")),
        cand("task-near", T0 + 10, Some("near")),
        cand("task-mid", T0 + 20, Some("mid")),
        cand("task-late", T0 + 1000, Some("near")),
    ];
    let dist = |c: &AdoptCandidate| match c.base_commit.as_deref() {
        Some("near") => Some(3),
        Some("mid") => Some(50),
        Some("far") => Some(500),
        _ => None,
    };
    let checkout = at(T0 + 100);
    // task-late は checkout より後なので安全条件で外れる。
    let got = choose_adopt(&cands, "r", checkout, &dist, 200).unwrap();
    assert_eq!(got.owner.to_string(), "task-near");
    // 上限を 2 にすると範囲内が無い → 最も新しい安全な候補。
    let got = choose_adopt(&cands, "r", checkout, &dist, 2).unwrap();
    assert_eq!(got.owner.to_string(), "task-far");
    // 別の repo は選ばない。
    assert!(choose_adopt(&cands, "other", checkout, &dist, 200).is_none());
}

#[test]
fn nfs_check_disables_scratch_with_a_reason() {
    let s = ScratchSettings::with_dir("/mnt/nfs/scratch");
    let got = apply_nfs_check(s.clone(), |_| Ok(true));
    assert!(!got.enabled);
    assert!(got.disabled_reason.unwrap().contains("NFS"));
    let got = apply_nfs_check(s.clone(), |p| Ok(p.ends_with(L1_DIR)));
    assert!(!got.enabled);
    let got = apply_nfs_check(s.clone(), |_| Ok(false));
    assert!(got.enabled);
    // 本物の検査は一時ディレクトリ（ローカル）では NFS ではない。
    let tmp = tempfile::tempdir().unwrap();
    assert!(!is_on_nfs(&tmp.path().join("missing/child")).unwrap());
}

#[test]
fn list_owner_dirs_finds_tasks_work_units_and_strays() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let t = pool.targets_dir();
    for d in [
        "task-A/wu-W1",
        "task-A/target",
        "release-abc",
        "agent-x",
        "junk",
        ".deleting-task-B-1",
    ] {
        std::fs::create_dir_all(t.join(d)).unwrap();
    }
    let got = pool.list_owner_dirs();
    let owners: Vec<String> = got
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .map(|o| o.to_string())
        .collect();
    assert_eq!(
        owners,
        vec!["agent-x", "release-abc", "task-A", "task-A/wu-W1"]
    );
    let strays: Vec<&PathBuf> = got.iter().filter_map(|r| r.as_ref().err()).collect();
    assert_eq!(strays, vec![&t.join("junk")]);
}

#[test]
fn legacy_build_cache_entries_are_reclaimed_only_when_idle() {
    let now = at(T0 + 10_000);
    assert_eq!(legacy_class(None, now).0, Class::Pinned);
    assert_eq!(
        legacy_class(Some(at(T0 + 10_000 - 3599)), now).0,
        Class::Pinned
    );
    assert_eq!(
        legacy_class(Some(at(T0 + 10_000 - 3600)), now).0,
        Class::Legacy
    );
    // legacy は pool の外なので pinned の量にも pool の使用量にも数えない。
    let mut busy = entry(
        "/cache/cargo/agent-platform-g1",
        Class::Pinned,
        T0,
        Some(99),
        "r",
    );
    busy.owner = None;
    busy.in_pool = false;
    let mut idle = entry("/cache/cargo/old", Class::Legacy, T0, Some(5), "r");
    idle.owner = None;
    idle.in_pool = false;
    let plan = plan_gc(&[busy, idle], &params(1_000_000));
    assert_eq!(ids(&plan), vec!["/cache/cargo/old"]);
    assert_eq!(plan.used_bytes, 0);
    assert_eq!(plan.pinned_bytes, 0);
}

#[test]
fn measure_tree_counts_blocks_and_latest_mtime() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
    std::fs::write(tmp.path().join("a/b/f"), vec![0u8; 10_000]).unwrap();
    let (size, latest) = measure_tree(tmp.path()).unwrap();
    assert!(size >= 10_000, "{size}");
    assert!(latest > at(T0 - 1_000_000_000));
    assert_eq!(measure_tree(&tmp.path().join("missing")).unwrap().0, 0);
}

/// テスト用の sccache のバイナリ（実行ビットつきの空の script。呼ばれない）。
fn fake_sccache(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("tools/sccache/bin/sccache");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn sccache_settings(dir: &Path, binary: PathBuf) -> ScratchSettings {
    ScratchSettings {
        l1_max_bytes: 40 * GIB,
        sccache: SccacheSettings {
            enabled: true,
            binary,
            server_port: 4236,
        },
        ..ScratchSettings::with_dir(dir.join("scratch"))
    }
}

/// ADR-0075 D4 / G2 受け入れ条件 2: server が応答するとき、env は `CARGO_TARGET_DIR`・`[scratch.cargo]`・sccache 系を
/// 固定の順で全部持ち、何度組んでも同じ。wrapper は `<scratch>/bin/sccache`（cc-rs が認める名前）で、
/// `CARGO_TARGET_DIR` を外して本物の sccache を exec する。
#[test]
fn sccache_env_is_complete_and_stable() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = fake_sccache(tmp.path());
    let settings = sccache_settings(tmp.path(), bin.clone());
    let owner = Owner::work_unit("01TASK", "01WU");
    let up = |p: u16| p == 4236;
    let state = resolve_sccache(&settings, up);
    let root = tmp.path().join("scratch");
    let wrapper = root.join("bin/sccache");
    assert_eq!(
        state,
        SccacheState::Ready {
            wrapper: wrapper.clone()
        }
    );
    let env = cargo_env_with(&settings, &owner, &state);
    let s = |p: PathBuf| p.display().to_string();
    assert_eq!(
        env,
        vec![
            (
                "CARGO_TARGET_DIR".to_string(),
                s(root.join("targets/task-01TASK/wu-01WU/target"))
            ),
            ("CARGO_INCREMENTAL".to_string(), "0".to_string()),
            (
                "CARGO_PROFILE_DEV_DEBUG".to_string(),
                "line-tables-only".to_string()
            ),
            ("RUSTC_WRAPPER".to_string(), s(wrapper.clone())),
            ("SCCACHE_DIR".to_string(), s(root.join("sccache-l1"))),
            ("SCCACHE_CACHE_SIZE".to_string(), "40G".to_string()),
            ("SCCACHE_SERVER_PORT".to_string(), "4236".to_string()),
            ("SCCACHE_IDLE_TIMEOUT".to_string(), "0".to_string()),
        ]
    );
    // 何度組んでも同じ（wrapper は書き直さない）。
    let before = mtime(&wrapper);
    let again = cargo_env_with(&settings, &owner, &resolve_sccache(&settings, up));
    assert_eq!(env, again);
    assert_eq!(mtime(&wrapper), before);
    let script = std::fs::read_to_string(&wrapper).unwrap();
    assert!(script.starts_with("#!/bin/bash\n"), "{script}");
    assert!(
        script.contains("unset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR"),
        "{script}"
    );
    assert!(
        script.contains(&format!("exec '{}' \"$@\"", bin.display())),
        "{script}"
    );
    // server の env は client の sccache 系と同じ値（RUSTC_WRAPPER を除く）。
    assert_eq!(sccache_server_env(&settings), env[4..].to_vec());
    // [scratch.cargo] を変えれば与えない。
    let tuned = ScratchSettings {
        cargo: CargoTuning {
            incremental: true,
            dev_debug: None,
        },
        ..settings.clone()
    };
    let env = cargo_env_with(&tuned, &owner, &state);
    assert!(
        !env.iter()
            .any(|(k, _)| k.starts_with("CARGO_INCREMENTAL") || k == "CARGO_PROFILE_DEV_DEBUG")
    );
    // 実際に wrapper を通すと CARGO_TARGET_DIR が消えている（本物の sccache の代わりに env を出す script）。
    let echo = tmp.path().join("echo-env");
    std::fs::write(
        &echo,
        "#!/bin/sh\necho \"T=${CARGO_TARGET_DIR-unset} A=$1\"\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&echo, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let w = ensure_wrapper(&settings.pool(), &echo).unwrap();
    // R7-7: wrapper は server に届くときだけ sccache を通すので、server 役の listener を立てる。
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let out = std::process::Command::new(&w)
        .arg("rustc")
        .env("CARGO_TARGET_DIR", "/somewhere")
        .env(
            "SCCACHE_SERVER_PORT",
            server.local_addr().unwrap().port().to_string(),
        )
        .env_remove("SCCACHE_SERVER_UDS")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "T=unset A=rustc\n");
}

/// 実行できる sh の script を置く（試験用）。
fn write_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// 使われていない port（bind して手放す）。
fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// ADR-0075 R7-7（本番の事故の再現）: エージェントの sandbox（codex の `workspace-write`、ネットワーク無し）の中では
/// sccache の client が server に繋げず `sccache: error: Operation not permitted (os error 1)` で build を落としていた。
/// wrapper は自分の居る場所から server に届かなければ compiler を直接 exec する（引数・stdout・exit code はそのまま、
/// `CARGO_TARGET_DIR` は外したまま）。届けば sccache を通す。sccache 自身の操作と `SCCACHE_SERVER_UDS` は常に sccache。
#[test]
fn wrapper_runs_the_compiler_directly_when_the_server_is_unreachable() {
    let tmp = tempfile::tempdir().unwrap();
    let sccache = tmp.path().join("fake-sccache");
    // 本物の sccache の代わり: 呼ばれたことと引数を出す（sccache は exit 2 で落ちる役もできるが、ここでは呼ばれたかだけを見る）。
    write_script(
        &sccache,
        "#!/bin/sh\necho \"SCCACHE T=${CARGO_TARGET_DIR-unset} A=$*\"\n",
    );
    let compiler = tmp.path().join("fake-rustc");
    write_script(
        &compiler,
        "#!/bin/sh\necho \"COMPILER T=${CARGO_TARGET_DIR-unset} A=$*\"\nexit 3\n",
    );
    let settings = sccache_settings(tmp.path(), sccache.clone());
    let wrapper = ensure_wrapper(&settings.pool(), &sccache).unwrap();
    let run = |port: u16, uds: Option<&str>, args: &[&str]| {
        let mut c = std::process::Command::new(&wrapper);
        c.args(args)
            .env("CARGO_TARGET_DIR", "/somewhere")
            .env("SCCACHE_SERVER_PORT", port.to_string());
        match uds {
            Some(v) => c.env("SCCACHE_SERVER_UDS", v),
            None => c.env_remove("SCCACHE_SERVER_UDS"),
        };
        let out = c.output().unwrap();
        (
            String::from_utf8_lossy(&out.stdout).into_owned(),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let compiler_s = compiler.display().to_string();

    // server に届く → sccache を通す。
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let up = server.local_addr().unwrap().port();
    let (out, code, _) = run(up, None, &[&compiler_s, "-vV"]);
    assert_eq!(out, format!("SCCACHE T=unset A={compiler_s} -vV\n"));
    assert_eq!(code, Some(0));

    // server に届かない → compiler を直接（exit code もそのまま、probe の失敗は stderr に出さない）。
    let down = closed_port();
    let (out, code, err) = run(down, None, &[&compiler_s, "-vV"]);
    assert_eq!(out, "COMPILER T=unset A=-vV\n");
    assert_eq!(code, Some(3));
    assert!(err.is_empty(), "stderr: {err}");

    // sccache 自身の操作（compiler ではない）は届かなくても sccache へ。引数なしも同じ。
    let (out, _, _) = run(down, None, &["--show-stats"]);
    assert_eq!(out, "SCCACHE T=unset A=--show-stats\n");
    let (out, _, _) = run(down, None, &[]);
    assert_eq!(out, "SCCACHE T=unset A=\n");

    // SCCACHE_SERVER_UDS（Celeris は与えない）があれば TCP は見ない。
    let (out, _, _) = run(down, Some("/nonexistent.sock"), &[&compiler_s, "-vV"]);
    assert_eq!(out, format!("SCCACHE T=unset A={compiler_s} -vV\n"));

    // ネットワークの無い sandbox の代わり: user + network namespace（`unshare -rn`）の中からは、外の listener に届かない
    // （codex の seccomp では socket が EPERM、ここでは lo が無く connect が失敗。wrapper から見て同じ「届かない」）。
    // ADR-0126 付記: userns 前提の部分だけを既定 skip にする（残りの assert は上で既に済んでいる）。
    if !crate::test_support::skip_unless_userns_tests() {
        let out = std::process::Command::new("unshare")
            .arg("-rn")
            .arg(&wrapper)
            .arg(&compiler_s)
            .arg("-vV")
            .env("SCCACHE_SERVER_PORT", up.to_string())
            .env_remove("SCCACHE_SERVER_UDS")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "COMPILER T=unset A=-vV\n"
        );
        assert_eq!(out.status.code(), Some(3));
    }
    drop(server);
}

/// ADR-0075 D4 / G2 受け入れ条件 2: バイナリが無い・server が応答しない・`enabled = false`・scratch が無効のときは
/// sccache 系を与えない（`CARGO_TARGET_DIR` と `[scratch.cargo]` は残る）。
/// Phase G3: `/healthz` と `/stats` を読む最小の HTTP クライアント（`Content-Length` で切る。閉じた port・
/// 200 以外は unhealthy）。
#[test]
fn cache_server_health_reads_a_minimal_http_response() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut c) = conn else { continue };
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            // 要求の終わり（空行）まで読み切ってから答える（高負荷で要求が分かれて届いても RST にしない）。
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match c.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            let req = String::from_utf8_lossy(&req).to_string();
            let resp: &[u8] = if req.starts_with("GET /healthz ") {
                b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\n\r\nok\ntrailing"
            } else {
                b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n"
            };
            let _ = c.write_all(resp);
        }
    });
    assert_eq!(
        http_get_local(port, "/healthz", Duration::from_secs(2)),
        Some((200, "ok\n".to_string()))
    );
    assert!(cache_server_healthy(port));
    assert_eq!(
        http_get_local(port, "/stats", Duration::from_secs(2)).map(|r| r.0),
        Some(404)
    );
    assert!(!cache_server_healthy(1));
    // token は 0600 で作り、2 回目は同じ値を返す。
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sub/cache-server.token");
    let t = ensure_token(&path).unwrap();
    assert_eq!(t.len(), 64);
    assert_eq!(ensure_token(&path).unwrap(), t);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

/// G3-fix1: 配線しないときは sccache の族（`RUSTC_WRAPPER` / `RUSTC_WORKSPACE_WRAPPER` / 既知と継いだ `SCCACHE_*`）
/// を全部外し、配線するときは与えない key だけを外す。結果は親の env に依らない（既知の key は常に入る）。
#[test]
fn sccache_family_is_removed_unless_set() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = fake_sccache(tmp.path());
    let owner = Owner::task("01TASK");
    let settings = sccache_settings(tmp.path(), bin);
    let inherited = || {
        [
            "RUSTC_WRAPPER",
            "SCCACHE_REDIS_ENDPOINT",
            "SCCACHE_WEIRD-NAME",
            "PATH",
        ]
        .map(String::from)
    };
    let down = resolve_sccache(&settings, |_| false);
    let set = cargo_env_with(&settings, &owner, &down);
    let removed = sccache_env_removals_with(&set, inherited());
    let mut want: Vec<String> = RUSTC_WRAPPER_VARS
        .iter()
        .chain(KNOWN_SCCACHE_VARS.iter())
        .map(|k| k.to_string())
        .chain(["SCCACHE_REDIS_ENDPOINT".to_string()])
        .collect();
    want.sort();
    assert_eq!(removed, want);
    // 親に何も無くても既知の key は外す（判定が親の env に依らない）。
    let bare = sccache_env_removals_with(&set, Vec::new());
    assert!(bare.iter().any(|k| k == "RUSTC_WRAPPER"), "{bare:?}");
    assert!(bare.iter().any(|k| k == "SCCACHE_DIR"), "{bare:?}");
    // 空の値の代替は wrapper の 2 つだけ（SCCACHE_* は空にしない）。
    let env = CargoEnv {
        set: set.clone(),
        remove: removed,
    };
    let fallback = env.set_with_empty_wrappers();
    assert_eq!(&fallback[..set.len()], &set[..]);
    assert_eq!(
        fallback[set.len()..].to_vec(),
        vec![
            ("RUSTC_WRAPPER".to_string(), String::new()),
            ("RUSTC_WORKSPACE_WRAPPER".to_string(), String::new()),
        ]
    );
    // 配線するとき: 与える key は外さない。与えない族（workspace wrapper・webdav 系・継いだ他の SCCACHE_*）は外す。
    let up = resolve_sccache(&settings, |_| true);
    let set = cargo_env_with(&settings, &owner, &up);
    let removed = sccache_env_removals_with(&set, inherited());
    assert!(
        removed.iter().all(|k| !set.iter().any(|(s, _)| s == k)),
        "{removed:?}"
    );
    assert_eq!(
        removed,
        vec![
            "RUSTC_WORKSPACE_WRAPPER",
            "SCCACHE_REDIS_ENDPOINT",
            "SCCACHE_WEBDAV_ENDPOINT",
            "SCCACHE_WEBDAV_KEY_PREFIX",
            "SCCACHE_WEBDAV_TOKEN",
        ]
    );
    assert!(is_sccache_family("SCCACHE_DIR") && is_sccache_family("RUSTC_WRAPPER"));
    assert!(!is_sccache_family("CARGO_TARGET_DIR"));
}

#[test]
fn sccache_env_is_omitted_without_binary_or_server() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = fake_sccache(tmp.path());
    let owner = Owner::task("01TASK");
    let keys = |env: &[(String, String)]| env.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
    let plain = vec![
        "CARGO_TARGET_DIR".to_string(),
        "CARGO_INCREMENTAL".to_string(),
        "CARGO_PROFILE_DEV_DEBUG".to_string(),
    ];
    // バイナリが無い。
    let settings = sccache_settings(tmp.path(), tmp.path().join("missing/sccache"));
    let state = resolve_sccache(&settings, |_| true);
    assert_eq!(state.label(), "unavailable");
    assert!(state.reason().unwrap().contains("not found"), "{state:?}");
    assert_eq!(keys(&cargo_env_with(&settings, &owner, &state)), plain);
    // server が応答しない。
    let settings = sccache_settings(tmp.path(), bin.clone());
    let state = resolve_sccache(&settings, |_| false);
    assert_eq!(state.label(), "unavailable");
    assert!(
        state.reason().unwrap().contains("127.0.0.1:4236"),
        "{state:?}"
    );
    assert_eq!(keys(&cargo_env_with(&settings, &owner, &state)), plain);
    // `[scratch.sccache] enabled = false`。
    let mut off = sccache_settings(tmp.path(), bin.clone());
    off.sccache.enabled = false;
    let state = resolve_sccache(&off, |_| true);
    assert_eq!(state.label(), "disabled");
    assert_eq!(keys(&cargo_env_with(&off, &owner, &state)), plain);
    // scratch が無効。
    let mut off = sccache_settings(tmp.path(), bin);
    off.enabled = false;
    assert_eq!(resolve_sccache(&off, |_| true).label(), "disabled");
    // wrapper は Ready のときだけ書く。
    assert!(!tmp.path().join("scratch/bin/sccache").exists());
    // 本物の probe（loopback だけ）。閉じた port には port 1（特権 port。テストが bind できないので他のテストと
    // 競合しない）を使う。
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    assert!(server_listening(l.local_addr().unwrap().port()));
    assert!(!server_listening(1));
}
