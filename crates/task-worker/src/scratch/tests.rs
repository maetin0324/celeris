use super::*;
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::MetadataExt;

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

#[test]
fn scratch_shared_extent_is_counted_once_and_fiemap_failure_falls_back() {
    let mut seen = BTreeMap::new();
    let first = super::gc::account_shared_extents(Ok(vec![(4096, 8192)]), 8192, &mut seen);
    let second = super::gc::account_shared_extents(Ok(vec![(4096, 8192)]), 8192, &mut seen);
    assert_eq!(first, (8192, true));
    assert_eq!(second, (0, true));

    let fallback = super::gc::account_shared_extents(
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported)),
        4096,
        &mut seen,
    );
    assert_eq!(fallback, (4096, false));

    let mut index = super::gc::SharedExtentIndex::default();
    index.replace_measurement("owner-a".into(), vec![(100, 40)], 0);
    index.replace_measurement("owner-b".into(), vec![(100, 40)], 0);
    assert_eq!(
        index.owner_bytes("owner-a") + index.owner_bytes("owner-b"),
        40
    );
    index.replace_measurement("owner-b".into(), vec![], 24);
    assert_eq!(
        index.owner_bytes("owner-a") + index.owner_bytes("owner-b"),
        64
    );
}

#[test]
fn scratch_shared_settings_use_adr_watermark_capacity_defaults() {
    let settings = ScratchSettings::with_dir("/tmp/scratch");
    assert_eq!(settings.targets_max_bytes, 160 * GIB);
    assert_eq!(settings.total_max_bytes, 200 * GIB);
}

#[test]
fn scratch_shared_measurement_falls_back_when_fiemap_is_unsupported() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("extent-data");
    std::fs::write(&file, vec![7u8; 128 * 1024]).unwrap();
    let allocated = std::fs::metadata(&file).unwrap().blocks() * 512;
    let mut index = super::gc::SharedExtentIndex::default();
    index.replace_measurement("owner".into(), vec![(10, 20)], 0);
    let result = super::gc::measure_tree_shared_with(tmp.path(), "owner", &mut index, &|_| {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    })
    .unwrap();
    assert_eq!(result.0, allocated);
    assert!(!result.2);
    assert_eq!(index.owner_bytes("owner"), allocated);
}

/// ADR-0129 (1): env は `CARGO_TARGET_DIR` と `[scratch.cargo]` だけを固定の順で持ち、`RUSTC_WRAPPER` / `SCCACHE_*`
/// を足さない（compiler wrapper は host の cargo 設定に任せる）。
#[test]
fn cargo_env_is_target_and_tuning_only() {
    let root = PathBuf::from("/scratch");
    let settings = ScratchSettings::with_dir(&root);
    let owner = Owner::work_unit("01TASK", "01WU");
    let env = cargo_env(&settings, &owner);
    assert_eq!(
        env,
        vec![
            (
                "CARGO_TARGET_DIR".to_string(),
                root.join("targets/task-01TASK/wu-01WU/target")
                    .display()
                    .to_string()
            ),
            ("CARGO_INCREMENTAL".to_string(), "0".to_string()),
            (
                "CARGO_PROFILE_DEV_DEBUG".to_string(),
                "line-tables-only".to_string()
            ),
        ]
    );
    assert!(
        !env.iter()
            .any(|(k, _)| k.starts_with("SCCACHE_") || k.contains("RUSTC_WRAPPER")),
        "{env:?}"
    );
    assert_eq!(cargo_child_env(&settings, &owner), CargoEnv::set_only(env));
    // [scratch.cargo] を変えれば与えない。
    let tuned = ScratchSettings {
        cargo: CargoTuning {
            incremental: true,
            dev_debug: None,
        },
        ..settings
    };
    let keys: Vec<String> = cargo_env(&tuned, &owner)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(keys, ["CARGO_TARGET_DIR"]);
}

// ---------------------------------------------------------------------------
// ADR-0129 (4)(6): seed からの reflink
// ---------------------------------------------------------------------------

const SEED_REPO: &str = "/repo/agent-platform";

fn seed_req<'a>(
    owner: &'a Owner,
    none: &'a dyn Fn(&AdoptCandidate) -> Option<u64>,
) -> AllocateRequest<'a> {
    AllocateRequest {
        owner,
        repo_path: Path::new(SEED_REPO),
        base_commit: None,
        work_unit_key: None,
        checkout: None,
        candidates: &[],
        distance: none,
        adopt: false,
        max_distance: 200,
    }
}

/// `seeds/<key>/current/{target,manifest.json}` を作る（`current` は世代ディレクトリへの symlink）。
fn write_seed(pool: &Pool, manifest: &str) -> PathBuf {
    let key = crate::build_cache::repo_cache_key(Path::new(SEED_REPO));
    let gen_dir = pool.root().join(SEEDS_DIR).join(&key).join("gen-1");
    let target = gen_dir.join(TARGET_SUBDIR);
    std::fs::create_dir_all(target.join("debug/deps")).unwrap();
    std::fs::write(
        target.join("debug/deps/libbig.rlib"),
        vec![7u8; 1024 * 1024],
    )
    .unwrap();
    std::fs::write(target.join("debug/.fingerprint"), b"fp").unwrap();
    std::fs::write(target.join("CACHEDIR.TAG"), b"tag").unwrap();
    std::fs::write(gen_dir.join(SEED_MANIFEST), manifest).unwrap();
    std::os::unix::fs::symlink("gen-1", seed_current_dir(pool, &key)).unwrap();
    target
}

const MANIFEST: &str = r#"{"commit":"abc123","rustc":"rustc 1.98.1","incremental":false,"dev_debug":"line-tables-only"}"#;

fn partials(owner_dir: &Path) -> Vec<String> {
    std::fs::read_dir(owner_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(SEED_COPY_PREFIX))
        .collect()
}

fn is_empty_dir(p: &Path) -> bool {
    p.is_dir() && std::fs::read_dir(p).unwrap().next().is_none()
}

/// reflink できない tmp（tmpfs / ext 系）では、seed があっても共有を確かめられず空の target に戻る。残骸は無い。
#[test]
fn reflink_unavailable_falls_back_to_empty_target() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    write_seed(&pool, MANIFEST);
    let owner = Owner::work_unit("T1", "W1");
    let none = |_: &AdoptCandidate| None;
    // TMPDIR may be on btrfs (including a run's scratch directory). Inject the
    // unsupported result instead of assuming that temporary files cannot share extents.
    let unavailable = |_: &Path| Ok(false);
    let ops = SeedCopyOps {
        copy: &cp_reflink_auto,
        is_shared: &unavailable,
    };
    let a = allocate_with_seed(
        &pool,
        &seed_req(&owner, &none),
        &SeedPolicy::unchecked(),
        &ops,
    )
    .unwrap();
    let TargetOrigin::Empty {
        reason: Some(reason),
    } = &a.origin
    else {
        panic!("expected an empty fallback, got {:?}", a.origin);
    };
    assert!(reason.contains("reflink"), "{reason}");
    assert!(is_empty_dir(&a.target_dir));
    let owner_dir = pool.owner_dir(&owner);
    assert!(partials(&owner_dir).is_empty());
    let rec = read_target_origin(&owner_dir).unwrap().unwrap();
    assert_eq!(rec.schema, TARGET_ORIGIN_SCHEMA);
    assert_eq!(rec.origin, "empty");
    assert!(rec.reason.is_some());
}

/// 共有できる（差し替えた判定）なら seed を写し、`target-origin.json` に seed と commit を残す。
#[test]
fn reflink_shared_copy_creates_target_from_seed() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    write_seed(&pool, MANIFEST);
    let owner = Owner::task("T1");
    let none = |_: &AdoptCandidate| None;
    let shared = |_: &Path| Ok(true);
    let ops = SeedCopyOps {
        copy: &cp_reflink_auto,
        is_shared: &shared,
    };
    let rustc = || Some("rustc 1.98.1\n".to_string());
    let policy = SeedPolicy {
        enabled: true,
        cargo: Some(CargoTuning::default()),
        rustc: Some(&rustc),
    };
    let a = allocate_with_seed(&pool, &seed_req(&owner, &none), &policy, &ops).unwrap();
    assert!(
        matches!(&a.origin, TargetOrigin::Seed { commit: Some(c), .. } if c == "abc123"),
        "{:?}",
        a.origin
    );
    assert_eq!(
        std::fs::read(a.target_dir.join("debug/deps/libbig.rlib"))
            .unwrap()
            .len(),
        1024 * 1024
    );
    assert!(a.target_dir.join("debug/.fingerprint").is_file());
    let owner_dir = pool.owner_dir(&owner);
    assert!(partials(&owner_dir).is_empty());
    let rec = read_target_origin(&owner_dir).unwrap().unwrap();
    assert_eq!(rec.origin, "seed");
    assert_eq!(rec.seed_commit.as_deref(), Some("abc123"));
    // 2 度目の割り当ては既存の target をそのまま使う（seed で上書きしない）。
    std::fs::write(a.target_dir.join("mine"), b"x").unwrap();
    let again = allocate_with_seed(&pool, &seed_req(&owner, &none), &policy, &ops).unwrap();
    assert_eq!(again.origin, TargetOrigin::Existing);
    assert!(again.target_dir.join("mine").is_file());
    assert_eq!(
        read_target_origin(&owner_dir).unwrap().unwrap().origin,
        "seed"
    );
}

/// 全体の写しが途中で失敗したら、部分的に写した一時ディレクトリを消してから空の target に戻る。
#[test]
fn reflink_partial_copy_failure_removes_residue() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    write_seed(&pool, MANIFEST);
    let owner = Owner::task("T1");
    let none = |_: &AdoptCandidate| None;
    let copy = |from: &Path, to: &Path| -> io::Result<()> {
        if from.is_file() {
            return std::fs::copy(from, to).map(|_| ());
        }
        std::fs::create_dir_all(to.join("debug/deps"))?;
        std::fs::write(to.join("debug/deps/half"), b"partial")?;
        Err(io::Error::from_raw_os_error(nix::libc::EXDEV))
    };
    let shared = |_: &Path| Ok(true);
    let ops = SeedCopyOps {
        copy: &copy,
        is_shared: &shared,
    };
    let a = allocate_with_seed(
        &pool,
        &seed_req(&owner, &none),
        &SeedPolicy::unchecked(),
        &ops,
    )
    .unwrap();
    assert!(
        matches!(&a.origin, TargetOrigin::Empty { reason: Some(r) } if r.contains("seed copy failed")),
        "{:?}",
        a.origin
    );
    assert!(is_empty_dir(&a.target_dir));
    assert!(partials(&pool.owner_dir(&owner)).is_empty());
}

/// 写した後に共有されていなければ（通常コピーへ落ちた）消して空に戻る。
#[test]
fn reflink_copy_not_shared_after_full_copy_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    write_seed(&pool, MANIFEST);
    let owner = Owner::task("T1");
    let none = |_: &AdoptCandidate| None;
    // probe（.probe の拡張子）だけ共有を報告する。
    let shared = |p: &Path| Ok(p.extension().is_some_and(|e| e == "probe"));
    let ops = SeedCopyOps {
        copy: &cp_reflink_auto,
        is_shared: &shared,
    };
    let a = allocate_with_seed(
        &pool,
        &seed_req(&owner, &none),
        &SeedPolicy::unchecked(),
        &ops,
    )
    .unwrap();
    assert!(
        matches!(&a.origin, TargetOrigin::Empty { reason: Some(r) } if r.contains("did not share")),
        "{:?}",
        a.origin
    );
    assert!(is_empty_dir(&a.target_dir));
    assert!(partials(&pool.owner_dir(&owner)).is_empty());
}

/// seed が無い・無効・manifest が無い・互換でない（rustc / [scratch.cargo]）なら写さずに空から作る。
#[test]
fn reflink_seed_absent_or_incompatible_starts_empty_without_copy() {
    let none = |_: &AdoptCandidate| None;
    let copied = std::cell::Cell::new(0);
    let copy = |_: &Path, _: &Path| -> io::Result<()> {
        copied.set(copied.get() + 1);
        Err(io::Error::other("must not copy"))
    };
    let shared = |_: &Path| Ok(true);
    let ops = SeedCopyOps {
        copy: &copy,
        is_shared: &shared,
    };
    let other_rustc = || Some("rustc 1.99.0".to_string());
    let cases: Vec<(Option<&str>, SeedPolicy<'_>, Option<&str>)> = vec![
        (None, SeedPolicy::unchecked(), None),
        (Some(MANIFEST), SeedPolicy::disabled(), None),
        (
            Some(""),
            SeedPolicy::unchecked(),
            Some("manifest is unreadable"),
        ),
        (
            Some(MANIFEST),
            SeedPolicy {
                enabled: true,
                cargo: None,
                rustc: Some(&other_rustc),
            },
            Some("rustc"),
        ),
        (
            Some(MANIFEST),
            SeedPolicy {
                enabled: true,
                cargo: Some(CargoTuning {
                    incremental: true,
                    dev_debug: None,
                }),
                rustc: None,
            },
            Some("cargo settings"),
        ),
        (
            Some(r#"{"repo_key":"other"}"#),
            SeedPolicy::unchecked(),
            Some("repo"),
        ),
    ];
    for (manifest, policy, want) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let pool = Pool::new(tmp.path());
        if let Some(m) = manifest {
            write_seed(&pool, m);
        }
        let owner = Owner::task("T1");
        let a = allocate_with_seed(&pool, &seed_req(&owner, &none), &policy, &ops).unwrap();
        match (&a.origin, want) {
            (TargetOrigin::Empty { reason: None }, None) => {}
            (TargetOrigin::Empty { reason: Some(r) }, Some(w)) => assert!(r.contains(w), "{r}"),
            (o, w) => panic!("{manifest:?}: got {o:?}, want {w:?}"),
        }
        assert!(is_empty_dir(&a.target_dir));
    }
    assert_eq!(copied.get(), 0);
}

/// 前回の割り当てで落ちた作りかけ（`.seed-copy-*`）は次の割り当てで消す。
#[test]
fn reflink_stale_partial_copy_is_cleaned_up() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let owner = Owner::task("T1");
    let owner_dir = pool.owner_dir(&owner);
    std::fs::create_dir_all(owner_dir.join(format!("{SEED_COPY_PREFIX}1/debug"))).unwrap();
    std::fs::write(owner_dir.join(format!("{SEED_COPY_PREFIX}2.probe")), b"x").unwrap();
    let none = |_: &AdoptCandidate| None;
    let a = allocate(&pool, &seed_req(&owner, &none)).unwrap();
    assert_eq!(a.origin, TargetOrigin::Empty { reason: None });
    assert!(partials(&owner_dir).is_empty());
    assert!(is_empty_dir(&a.target_dir));
}

/// btrfs の実試験（opt-in）: `CELERIS_REFLINK_TEST_DIR` が btrfs のディレクトリを指すときだけ、実際の
/// `cp -a --reflink=auto` と FIEMAP で seed から写し、extent の共有と `df` の増分を確かめる。未設定なら skip。
#[test]
fn reflink_btrfs_seed_copy_shares_extents() {
    let Some(dir) = std::env::var_os("CELERIS_REFLINK_TEST_DIR") else {
        eprintln!("skip: CELERIS_REFLINK_TEST_DIR is not set");
        return;
    };
    let dir = PathBuf::from(dir);
    let st = nix::sys::statfs::statfs(&dir).unwrap();
    assert_eq!(
        st.filesystem_type(),
        nix::sys::statfs::BTRFS_SUPER_MAGIC,
        "{} is not btrfs",
        dir.display()
    );
    let tmp = tempfile::tempdir_in(&dir).unwrap();
    let pool = Pool::new(tmp.path());
    let seed = write_seed(&pool, MANIFEST);
    // df の増分を見るため 64 MiB の成果物を足す（中身は 0 でない値。sync して extent を確定させる）。
    let big = seed.join("debug/deps/libhuge.rlib");
    std::fs::write(&big, vec![0x5au8; 64 * 1024 * 1024]).unwrap();
    std::fs::File::open(&big).unwrap().sync_all().unwrap();
    nix::unistd::sync();
    let free = || {
        let v = nix::sys::statvfs::statvfs(&dir).unwrap();
        v.blocks_available() as u64 * v.fragment_size() as u64
    };
    let before = free();
    let owner = Owner::task("T1");
    let none = |_: &AdoptCandidate| None;
    let a = allocate(&pool, &seed_req(&owner, &none)).unwrap();
    assert!(
        matches!(a.origin, TargetOrigin::Seed { .. }),
        "{:?}",
        a.origin
    );
    let copied = a.target_dir.join("debug/deps/libhuge.rlib");
    assert!(fiemap_all_shared(&copied).unwrap());
    nix::unistd::sync();
    let after = free();
    let grew = before.saturating_sub(after);
    eprintln!(
        "reflink btrfs: copied 64 MiB seed file, df free {before} -> {after} (grew {} KiB), shared=true",
        grew / 1024
    );
    assert!(
        grew < 16 * 1024 * 1024,
        "df grew {grew} bytes; extents were not shared"
    );
    assert!(partials(&pool.owner_dir(&owner)).is_empty());
}

/// ADR-0129 (3): `mount` が mount されていない・`dir` がその下に無いなら従来の場所へ戻す。
#[test]
fn scratch_mount_check_falls_back_to_previous_dir() {
    let fallback = Path::new("/var/lib/celeris/scratch");
    let mut s = ScratchSettings::with_dir("/local/celeris/data/scratch");
    s.mount = Some(PathBuf::from("/local"));
    let ok = apply_mount_check(s.clone(), fallback, |_| Ok(true));
    assert_eq!(ok.dir, PathBuf::from("/local/celeris/data/scratch"));
    assert!(ok.dir_fallback_reason.is_none());
    let gone = apply_mount_check(s.clone(), fallback, |_| Ok(false));
    assert_eq!(gone.dir, fallback);
    assert!(gone.dir_fallback_reason.unwrap().contains("not mounted"));
    let err = apply_mount_check(s.clone(), fallback, |_| Err(io::Error::other("boom")));
    assert_eq!(err.dir, fallback);
    let mut outside = s.clone();
    outside.dir = PathBuf::from("/srv/scratch");
    let o = apply_mount_check(outside, fallback, |_| Ok(true));
    assert_eq!(o.dir, fallback);
    // mount を書かなければ何もしない。
    let plain = ScratchSettings::with_dir("/srv/scratch");
    assert_eq!(
        apply_mount_check(plain.clone(), fallback, |_| Ok(false)),
        plain
    );
    // 実物: `/` は mount point、無い path は false。
    assert!(is_mount_point(Path::new("/")).unwrap());
    assert!(!is_mount_point(Path::new("/nonexistent-celeris-mount")).unwrap());
}

// ---------------------------------------------------------------------------
// ADR-0129 (4)(5): seed の更新と GC
// ---------------------------------------------------------------------------

/// 偽の build: `CARGO_TARGET_DIR` に 1 MiB の rlib を書く（cargo は呼ばない）。`fail` なら失敗する。
fn fake_build(fail: bool) -> impl Fn(&Path, &[(String, String)]) -> io::Result<()> {
    move |_checkout: &Path, env: &[(String, String)]| {
        let target = env
            .iter()
            .find(|(k, _)| k == CARGO_TARGET_DIR_VAR)
            .map(|(_, v)| PathBuf::from(v))
            .ok_or_else(|| io::Error::other("no CARGO_TARGET_DIR"))?;
        assert!(
            env.iter()
                .any(|(k, v)| k == "CARGO_INCREMENTAL" && v == "0")
        );
        std::fs::create_dir_all(target.join("debug/deps"))?;
        std::fs::write(
            target.join("debug/deps/libbig.rlib"),
            vec![1u8; 1024 * 1024],
        )?;
        if fail {
            return Err(io::Error::other("boom"));
        }
        Ok(())
    }
}

fn no_checkout(_: &Path, checkout: &Path, _: &str) -> io::Result<()> {
    std::fs::create_dir_all(checkout)
}

fn fixed_rustc(_: &Path) -> Option<String> {
    Some("rustc 1.98.1".to_string())
}

fn seed_key() -> String {
    crate::build_cache::repo_cache_key(Path::new(SEED_REPO))
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn refresh(pool: &Pool, commit: &str, secs: u64, fail: bool) -> SeedRefreshOutcome {
    let build = fake_build(fail);
    let ops = SeedBuildOps {
        prepare_checkout: &no_checkout,
        build: &build,
        rustc: &fixed_rustc,
        share_probe: &|_| Ok(()),
        probe_results: std::sync::Mutex::new(std::collections::HashMap::new()),
    };
    refresh_seed(
        pool,
        Path::new(SEED_REPO),
        commit,
        &CargoTuning::default(),
        &ops,
        at(secs),
    )
}

#[test]
fn seed_skipped_when_pool_cannot_share() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path().join("scratch"));
    let build = |_: &Path, _: &[(String, String)]| panic!("cargo build must not run");
    let ops = SeedBuildOps {
        prepare_checkout: &no_checkout,
        build: &build,
        rustc: &fixed_rustc,
        share_probe: &|_| Err("test filesystem has no shared extents".to_string()),
        probe_results: std::sync::Mutex::new(std::collections::HashMap::new()),
    };
    let out = refresh_seed(
        &pool,
        Path::new(SEED_REPO),
        "1111111111111111",
        &CargoTuning::default(),
        &ops,
        at(T0),
    );
    assert!(
        matches!(out, SeedRefreshOutcome::Failed { reason } if reason.contains("cannot share extents"))
    );
    assert!(!seeds_dir(&pool).exists());
}

/// main が進むと新しい世代を別名で build してから `current` を rename で差し替え、旧世代を退避して消す。
/// 既に seed から作った owner の target は影響を受けない。
#[test]
fn seed_refresh_switches_current_atomically_and_retires_the_old_generation() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let key = seed_key();
    let SeedRefreshOutcome::Refreshed {
        generation: g1,
        replaced: None,
        ..
    } = refresh(&pool, "1111111111111111", T0, false)
    else {
        panic!("first refresh should create a seed");
    };
    assert_eq!(
        current_generation(&pool, &key).as_deref(),
        Some(g1.as_str())
    );
    let m = read_current_manifest(&pool, &key).unwrap();
    assert_eq!(m.commit.as_deref(), Some("1111111111111111"));
    assert_eq!(m.repo_key.as_deref(), Some(key.as_str()));
    assert_eq!(m.rustc.as_deref(), Some("rustc 1.98.1"));
    assert_eq!(m.incremental, Some(false));
    assert!(m.size_bytes.unwrap_or(0) >= 1024 * 1024);
    // 使用中の task の target（seed から写したもの）。
    let owner = Owner::task("T1");
    let none = |_: &AdoptCandidate| None;
    let shared = |_: &Path| Ok(true);
    let ops = SeedCopyOps {
        copy: &cp_reflink_auto,
        is_shared: &shared,
    };
    let a = allocate_with_seed(
        &pool,
        &seed_req(&owner, &none),
        &SeedPolicy::unchecked(),
        &ops,
    )
    .unwrap();
    assert!(
        matches!(&a.origin, TargetOrigin::Seed { commit: Some(c), .. } if c == "1111111111111111")
    );
    std::fs::write(a.target_dir.join("mine"), b"x").unwrap();

    let SeedRefreshOutcome::Refreshed {
        generation: g2,
        replaced: Some(old),
        ..
    } = refresh(&pool, "2222222222222222", T0 + 10, false)
    else {
        panic!("second refresh should replace the seed");
    };
    assert_eq!(old, g1);
    assert_ne!(g1, g2);
    let repo_dir = seed_repo_dir(&pool, &key);
    assert!(
        std::fs::symlink_metadata(repo_dir.join(SEED_CURRENT))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        current_generation(&pool, &key).as_deref(),
        Some(g2.as_str())
    );
    // 旧世代・作りかけ・一時 symlink は残らない。
    let left = names(&repo_dir);
    assert!(!left.contains(&g1), "{left:?}");
    assert!(
        !left
            .iter()
            .any(|n| n.starts_with(SEED_BUILDING_PREFIX) || n.starts_with(".current.tmp")),
        "{left:?}"
    );
    assert!(
        !names(&seeds_dir(&pool))
            .iter()
            .any(|n| n.starts_with(DELETING_PREFIX))
    );
    // 使用中の owner の target はそのまま。
    assert!(a.target_dir.join("mine").is_file());
    assert!(a.target_dir.join("debug/deps/libbig.rlib").is_file());
    let again = allocate_with_seed(
        &pool,
        &seed_req(&owner, &none),
        &SeedPolicy::unchecked(),
        &ops,
    )
    .unwrap();
    assert_eq!(again.origin, TargetOrigin::Existing);
    // 新しい owner は新しい世代から写す。
    let other = Owner::task("T2");
    let b = allocate_with_seed(
        &pool,
        &seed_req(&other, &none),
        &SeedPolicy::unchecked(),
        &ops,
    )
    .unwrap();
    assert!(
        matches!(&b.origin, TargetOrigin::Seed { commit: Some(c), .. } if c == "2222222222222222")
    );
}

/// build が失敗したら作りかけを消し、旧 seed をそのまま残す。
#[test]
fn seed_refresh_failure_keeps_the_old_seed() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let key = seed_key();
    let SeedRefreshOutcome::Refreshed { generation: g1, .. } =
        refresh(&pool, "1111111111111111", T0, false)
    else {
        panic!("first refresh should create a seed");
    };
    let out = refresh(&pool, "2222222222222222", T0 + 10, true);
    assert!(
        matches!(&out, SeedRefreshOutcome::Failed { reason } if reason.contains("boom")),
        "{out:?}"
    );
    assert_eq!(
        current_generation(&pool, &key).as_deref(),
        Some(g1.as_str())
    );
    assert_eq!(
        read_current_manifest(&pool, &key)
            .unwrap()
            .commit
            .as_deref(),
        Some("1111111111111111")
    );
    let left = names(&seed_repo_dir(&pool, &key));
    assert!(
        !left.iter().any(|n| n.starts_with(SEED_BUILDING_PREFIX)),
        "{left:?}"
    );
    // 失敗した checkout も旧 seed を残す。
    let bad_checkout =
        |_: &Path, _: &Path, _: &str| -> io::Result<()> { Err(io::Error::other("no commit")) };
    let build = fake_build(false);
    let ops = SeedBuildOps {
        prepare_checkout: &bad_checkout,
        build: &build,
        rustc: &fixed_rustc,
        share_probe: &|_| Ok(()),
        probe_results: std::sync::Mutex::new(std::collections::HashMap::new()),
    };
    let out = refresh_seed(
        &pool,
        Path::new(SEED_REPO),
        "3333333333333333",
        &CargoTuning::default(),
        &ops,
        at(T0 + 20),
    );
    assert!(matches!(out, SeedRefreshOutcome::Failed { .. }));
    assert_eq!(
        current_generation(&pool, &key).as_deref(),
        Some(g1.as_str())
    );
}

/// 同じ repo の更新は `claim_seed` で排他（取れなければ `Busy`。build しない）。手放せば次は取れる。
#[test]
fn seed_refresh_is_busy_while_another_refresh_holds_the_claim() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let repo_dir = seed_repo_dir(&pool, &seed_key());
    std::fs::create_dir_all(&repo_dir).unwrap();
    let held = claim_seed(&repo_dir).unwrap().unwrap();
    assert!(claim_seed(&repo_dir).unwrap().is_none());
    assert_eq!(
        refresh(&pool, "1111111111111111", T0, false),
        SeedRefreshOutcome::Busy
    );
    assert!(current_generation(&pool, &seed_key()).is_none());
    drop(held);
    assert!(matches!(
        refresh(&pool, "1111111111111111", T0, false),
        SeedRefreshOutcome::Refreshed { .. }
    ));
}

/// 更新の時期: seed が無い・main が進んだ・`[scratch.cargo]`・rustc が変わったとき。一致すれば要らない。
#[test]
fn seed_refresh_reason_follows_main_and_toolchain() {
    let cargo = CargoTuning::default();
    let m = SeedManifest {
        commit: Some("aaa".into()),
        rustc: Some("rustc 1.98.1".into()),
        incremental: Some(false),
        dev_debug: Some("line-tables-only".into()),
        ..Default::default()
    };
    assert!(seed_refresh_reason(None, "aaa", &cargo, None).is_some());
    assert_eq!(
        seed_refresh_reason(Some(&m), "aaa", &cargo, Some("rustc 1.98.1\n")),
        None
    );
    assert_eq!(seed_refresh_reason(Some(&m), "aaa", &cargo, None), None);
    assert!(
        seed_refresh_reason(Some(&m), "bbb", &cargo, None)
            .unwrap()
            .contains("main moved")
    );
    assert!(seed_refresh_reason(Some(&m), "aaa", &cargo, Some("rustc 1.99.0")).is_some());
    let inc = CargoTuning {
        incremental: true,
        ..CargoTuning::default()
    };
    assert!(seed_refresh_reason(Some(&m), "aaa", &inc, None).is_some());
}

/// 確かめる間隔（起動直後は直ちに、以後 `SEED_CHECK_INTERVAL_SECS` ごと）と容量による保留。
#[test]
fn seed_check_due_and_hold_are_deterministic() {
    let t0 = std::time::Instant::now();
    let iv = Duration::from_secs(SEED_CHECK_INTERVAL_SECS);
    assert!(seed_check_due(None, t0, iv));
    assert!(!seed_check_due(
        Some(t0),
        t0 + iv - Duration::from_secs(1),
        iv
    ));
    assert!(seed_check_due(Some(t0), t0 + iv, iv));
    assert!(seed_refresh_hold(true, Some(u64::MAX), 0, None).is_some());
    assert_eq!(
        seed_refresh_hold(false, Some(10 * GIB), 5 * GIB, Some(4 * GIB)),
        None
    );
    assert!(seed_refresh_hold(false, Some(8 * GIB), 5 * GIB, Some(4 * GIB)).is_some());
    assert_eq!(seed_refresh_hold(false, None, 5 * GIB, None), None);
}

/// GC は current の世代を消さず、current 以外の世代と放棄された build を消す（build 中の repo には触らない）。
#[test]
fn seed_gc_keeps_current_and_removes_stale_generations_and_abandoned_builds() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let key = seed_key();
    let SeedRefreshOutcome::Refreshed { generation: g1, .. } =
        refresh(&pool, "1111111111111111", T0, false)
    else {
        panic!("refresh should create a seed");
    };
    let repo_dir = seed_repo_dir(&pool, &key);
    // 切り替えの直後に落ちた残骸（current でない世代）と、中断した build。
    std::fs::create_dir_all(repo_dir.join("gen-000000000000-1/target")).unwrap();
    std::fs::create_dir_all(repo_dir.join(".building-gen-x/target")).unwrap();
    let registered: BTreeSet<String> = [key.clone()].into();
    // build 中（`claim_seed` を他が持つ）なら build も旧世代も触らない。
    let held = claim_seed(&repo_dir).unwrap().unwrap();
    let r = seed_gc(&pool, &registered, at(T0 + 60));
    assert!(
        r.retired.is_empty() && r.abandoned_builds.is_empty(),
        "{r:?}"
    );
    assert!(repo_dir.join(".building-gen-x").is_dir());
    drop(held);
    let r = seed_gc(&pool, &registered, at(T0 + 120));
    assert_eq!(r.retired.len(), 1, "{r:?}");
    assert_eq!(r.abandoned_builds.len(), 1);
    assert!(!repo_dir.join("gen-000000000000-1").exists());
    assert!(!repo_dir.join(".building-gen-x").exists());
    assert_eq!(
        current_generation(&pool, &key).as_deref(),
        Some(g1.as_str())
    );
    assert!(
        repo_dir
            .join(&g1)
            .join(TARGET_SUBDIR)
            .join("debug/deps/libbig.rlib")
            .is_file()
    );
    // 退避先は `seeds/.deleting-*`（削除スレッドの根）。
    assert!(r.retired[0].starts_with(seed_deleting_root(&pool)));
    // 2 回目は何もしない。
    let r = seed_gc(&pool, &registered, at(T0 + 180));
    assert_eq!(r, SeedGcResult::default());
}

/// 登録から外れた repo の seed は印を書いて猶予を置き、猶予を過ぎたら全体を消す。登録に戻れば印を消す。
#[test]
fn seed_gc_removes_an_unregistered_repo_seed_after_the_grace_period() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = Pool::new(tmp.path());
    let key = seed_key();
    assert!(matches!(
        refresh(&pool, "1111111111111111", T0, false),
        SeedRefreshOutcome::Refreshed { .. }
    ));
    let repo_dir = seed_repo_dir(&pool, &key);
    let none = BTreeSet::new();
    let r = seed_gc(&pool, &none, at(T0));
    assert_eq!(r.marked_unregistered, vec![key.clone()]);
    assert!(repo_dir.join(SEED_UNREGISTERED_MARK).is_file());
    // 登録に戻れば印を消す。
    let registered: BTreeSet<String> = [key.clone()].into();
    seed_gc(&pool, &registered, at(T0 + 10));
    assert!(!repo_dir.join(SEED_UNREGISTERED_MARK).exists());
    // もう一度外れる → 猶予の直前までは残り、過ぎたら seed 全体を消す。
    seed_gc(&pool, &none, at(T0 + 100));
    let r = seed_gc(
        &pool,
        &none,
        at(T0 + 100 + SEED_UNREGISTERED_GRACE_SECS - 1),
    );
    assert!(r.removed_repos.is_empty());
    assert!(current_generation(&pool, &key).is_some());
    let r = seed_gc(&pool, &none, at(T0 + 100 + SEED_UNREGISTERED_GRACE_SECS));
    assert_eq!(r.removed_repos, vec![key.clone()]);
    assert!(!repo_dir.exists());
    assert_eq!(r.retired.len(), 1);
    assert!(r.retired[0].is_dir());
}

/// 実の git: 専用の checkout を固定 commit に detach し、main が進めば同じ checkout を進める（外部ネットワーク無し）。
#[test]
fn seed_git_checkout_follows_the_main_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README"), "1").unwrap();
    git(&["add", "README"]);
    git(&["commit", "-q", "-m", "one"]);
    let c1 = resolve_commit(&repo, "main").unwrap();
    assert!(!commit_has_cargo_manifest(&repo, &c1));
    std::fs::write(repo.join("Cargo.toml"), "[workspace]\n").unwrap();
    git(&["add", "Cargo.toml"]);
    git(&["commit", "-q", "-m", "two"]);
    let c2 = resolve_commit(&repo, "main").unwrap();
    assert_ne!(c1, c2);
    assert!(commit_has_cargo_manifest(&repo, &c2));
    let checkout = tmp.path().join("scratch/seeds/k/checkout");
    git_detached_checkout(&repo, &checkout, &c1).unwrap();
    assert!(!checkout.join("Cargo.toml").exists());
    git_detached_checkout(&repo, &checkout, &c2).unwrap();
    assert!(checkout.join("Cargo.toml").is_file());
    // 壊れた checkout は作り直す。
    std::fs::remove_file(checkout.join(".git")).unwrap();
    git_detached_checkout(&repo, &checkout, &c2).unwrap();
    assert!(checkout.join("Cargo.toml").is_file());
}
