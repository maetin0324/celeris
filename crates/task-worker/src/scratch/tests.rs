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
