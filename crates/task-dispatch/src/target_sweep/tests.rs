use super::*;
use std::collections::BTreeMap;
use std::fs::FileTimes;
use std::time::Duration;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// 時計を固定し、大きさは file の長さ（dir は 0）にする（量を厳密に比べられる）。
struct FixedEnv {
    now: SystemTime,
}

impl SweepEnv for FixedEnv {
    fn now(&self) -> SystemTime {
        self.now
    }
    fn bytes(&self, _path: &Path, meta: &Metadata) -> u64 {
        if meta.file_type().is_file() {
            meta.len()
        } else {
            0
        }
    }
}

/// 時計だけ固定し、大きさは既定（`st_blocks × 512`）。
struct FixedClock {
    now: SystemTime,
}

impl SweepEnv for FixedClock {
    fn now(&self) -> SystemTime {
        self.now
    }
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_000)
}

fn set_time(path: &Path, t: SystemTime) {
    let f = File::open(path).unwrap();
    f.set_times(FileTimes::new().set_accessed(t).set_modified(t))
        .unwrap();
}

/// `<target>/<profile>`（`.cargo-lock` つき）を作る。
fn profile(target: &Path, rel: &str) -> PathBuf {
    let p = target.join(rel);
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join(CARGO_LOCK), b"").unwrap();
    p
}

/// 1 crate 分（deps の 2 file・.fingerprint・build）を書き、全部の時刻を `t` にする。返り値は項目の path と大きさの合計。
fn add_crate(profile: &Path, key: &str, t: SystemTime) -> (Vec<PathBuf>, u64) {
    let deps = profile.join("deps");
    let fp = profile.join(".fingerprint").join(key);
    let build = profile.join("build").join(key);
    for d in [&deps, &fp, &build] {
        std::fs::create_dir_all(d).unwrap();
    }
    let rlib = deps.join(format!("lib{key}.rlib"));
    let dfile = deps.join(format!("{key}.d"));
    let fp_file = fp.join("lib-x");
    let build_file = build.join("output");
    std::fs::write(&rlib, vec![0u8; 3000]).unwrap();
    std::fs::write(&dfile, vec![0u8; 100]).unwrap();
    std::fs::write(&fp_file, vec![0u8; 20]).unwrap();
    std::fs::write(&build_file, vec![0u8; 7]).unwrap();
    for p in [&rlib, &dfile, &fp_file, &build_file, &fp, &build] {
        set_time(p, t);
    }
    (vec![rlib, dfile, fp, build], 3000 + 100 + 20 + 7)
}

/// 木の写し（path → 種類・長さ・mtime）。symlink は辿らない。
fn tree(root: &Path) -> BTreeMap<PathBuf, (bool, u64, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        let m = std::fs::symlink_metadata(&p).unwrap();
        out.insert(
            p.clone(),
            (m.file_type().is_dir(), m.len(), m.modified().unwrap()),
        );
        if m.file_type().is_dir() {
            for e in std::fs::read_dir(&p).unwrap() {
                stack.push(e.unwrap().path());
            }
        }
    }
    out
}

fn has_deleting(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with(DELETING_PREFIX))
}

#[test]
fn target_sweep_skips_profile_with_held_cargo_lock() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("cargo");
    let target = root.join("review-x");
    let debug = profile(&target, "debug");
    let release = profile(&target, "release");
    let old = now() - 30 * DAY;
    let (debug_items, _) = add_crate(&debug, "foo-0123abcd", old);
    let (release_items, _) = add_crate(&release, "bar-4567ef01", old);

    // cargo の代わりに試験が debug の `.cargo-lock` を握る（別の open file description なので LOCK_NB が失敗する）。
    let held = Flock::lock(
        File::open(debug.join(CARGO_LOCK)).unwrap(),
        FlockArg::LockExclusive,
    )
    .unwrap();
    let report = run_sweep(
        std::slice::from_ref(&root),
        &SweepParams::default(),
        TargetSweepMode::Apply,
        &FixedEnv { now: now() },
    );

    for p in &debug_items {
        assert!(p.exists(), "locked profile item removed: {}", p.display());
    }
    assert!(!has_deleting(&debug));
    assert!(target.exists(), "target dir with a held lock must survive");
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.path == debug && s.reason == "build_in_progress"),
        "{report:?}"
    );
    assert!(
        report.deleted.iter().all(|d| !d.path.starts_with(&debug)),
        "{report:?}"
    );
    // lock の取れた release の古い項目は消える（放置 target としてではなく古さで）。
    for p in &release_items {
        assert!(!p.exists(), "unlocked old item kept: {}", p.display());
    }
    assert_eq!(report.roots[0].by_reason.age, 4);
    assert_eq!(report.roots[0].by_reason.stale_target, 0);

    // lock を放せば次の回で debug も消える。
    drop(held);
    let report = run_sweep(
        std::slice::from_ref(&root),
        &SweepParams::default(),
        TargetSweepMode::Apply,
        &FixedEnv { now: now() },
    );
    assert!(report.skipped.is_empty(), "{report:?}");
    assert!(!target.exists());
}

#[test]
fn target_sweep_dry_run_deletes_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("cargo");
    let target = root.join("t");
    let debug = profile(&target, "debug");
    add_crate(&debug, "old-00000001", now() - 30 * DAY);
    add_crate(&debug, "new-00000002", now() - DAY);
    let leftover = debug.join(".deleting-01OLD");
    std::fs::create_dir_all(leftover.join("deps")).unwrap();
    std::fs::write(leftover.join("deps/libx-01.rlib"), b"x").unwrap();
    let stale = profile(&root.join("stale"), "debug");
    add_crate(&stale, "s-0000000a", now() - 30 * DAY);

    let before = tree(tmp.path());
    let report = run_sweep(
        std::slice::from_ref(&root),
        &SweepParams::default(),
        TargetSweepMode::DryRun,
        &FixedEnv { now: now() },
    );
    assert_eq!(tree(tmp.path()), before, "dry_run changed the tree");

    // 計画（消す予定）と前回の残りは report に出る。
    assert_eq!(report.mode, TargetSweepMode::DryRun);
    assert!(report.deleted.iter().any(|d| d.reason == "age"));
    assert!(
        report
            .deleted
            .iter()
            .any(|d| d.reason == "stale_target" && d.path == root.join("stale"))
    );
    assert_eq!(report.leftovers, vec![leftover]);
    assert!(report.errors.is_empty(), "{report:?}");
}

#[test]
fn target_sweep_apply_removes_old_items_and_reports_bytes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("cargo");
    // 深さ 2 の `<repo-key>/wu-*` と `<triple>/<profile>` も走査する。
    let target = root.join("agent-platform").join("wu-01ABC");
    let prof = profile(&target, "x86_64-unknown-linux-gnu/debug");
    let (old_items, old_bytes) = add_crate(&prof, "old-00000001", now() - 30 * DAY);
    let (new_items, new_bytes) = add_crate(&prof, "new-00000002", now() - DAY);
    let inc = prof.join("incremental/old-0000000f");
    std::fs::create_dir_all(&inc).unwrap();
    std::fs::write(inc.join("s"), vec![0u8; 500]).unwrap();
    set_time(&inc.join("s"), now() - 10 * DAY);
    set_time(&inc, now() - 10 * DAY);

    let report = run_sweep(
        std::slice::from_ref(&root),
        &SweepParams::default(),
        TargetSweepMode::Apply,
        &FixedEnv { now: now() },
    );

    for p in old_items.iter().chain([&inc]) {
        assert!(!p.exists(), "old item kept: {}", p.display());
    }
    for p in &new_items {
        assert!(p.exists(), "fresh item removed: {}", p.display());
    }
    assert!(!has_deleting(&prof), "rename target left behind");
    assert!(report.errors.is_empty(), "{report:?}");

    let r = &report.roots[0];
    assert_eq!(r.root, root);
    assert_eq!(r.before_bytes, old_bytes + new_bytes + 500);
    assert_eq!(r.deleted_bytes, old_bytes + 500);
    assert_eq!(r.after_bytes, new_bytes);
    assert_eq!(r.deleted_items, 5);
    assert_eq!(r.by_reason.age, 5);
    assert_eq!(report.deleted_bytes(), old_bytes + 500);

    // JSON 化でき、TargetSweepRan に写せる。
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["roots"][0]["deleted_bytes"], old_bytes + 500);
    let Event::TargetSweepRan {
        mode,
        roots,
        skipped_total,
        over_cap_unresolved,
        ..
    } = report.to_event()
    else {
        panic!("not TargetSweepRan");
    };
    assert_eq!(mode, TargetSweepMode::Apply);
    assert_eq!(roots[0].deleted_bytes, old_bytes + 500);
    assert_eq!(roots[0].by_reason.age, 5);
    assert_eq!(skipped_total, 0);
    assert!(!over_cap_unresolved);
}

#[test]
fn target_sweep_apply_reports_real_block_bytes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("cargo");
    let prof = profile(&root.join("t"), "debug");
    let (old_items, _) = add_crate(&prof, "old-00000001", now() - 30 * DAY);
    add_crate(&prof, "new-00000002", now() - DAY);
    let expected: u64 = old_items
        .iter()
        .map(|p| {
            let mut stack = vec![p.clone()];
            let mut sum = 0;
            while let Some(q) = stack.pop() {
                let m = std::fs::symlink_metadata(&q).unwrap();
                sum += m.blocks() * 512;
                if m.is_dir() {
                    stack.extend(std::fs::read_dir(&q).unwrap().map(|e| e.unwrap().path()));
                }
            }
            sum
        })
        .sum();

    let report = run_sweep(
        std::slice::from_ref(&root),
        &SweepParams::default(),
        TargetSweepMode::Apply,
        &FixedClock { now: now() },
    );
    assert!(expected > 0);
    assert_eq!(report.roots[0].deleted_bytes, expected);
    assert!(old_items.iter().all(|p| !p.exists()));
}

#[test]
fn target_sweep_never_touches_live_release_lease() {
    let tmp = tempfile::TempDir::new().unwrap();
    // scratch pool の release-build lease（root の外）。中身は古くても消してはいけない。
    let lease = tmp.path().join("scratch/targets/release-build");
    std::fs::create_dir_all(&lease).unwrap();
    std::fs::write(lease.join("lease.json"), br#"{"owner":"release-build"}"#).unwrap();
    let lease_prof = profile(&lease.join("target"), "release");
    add_crate(&lease_prof, "celeris-00000001", now() - 60 * DAY);

    let root = tmp.path().join("cache/cargo");
    let prof = profile(&root.join("t"), "debug");
    let (old_items, _) = add_crate(&prof, "old-00000001", now() - 30 * DAY);
    add_crate(&prof, "new-00000002", now() - DAY);
    // root の中から lease へ向く symlink（辿らない）。
    std::os::unix::fs::symlink(lease.join("target"), root.join("lease-link")).unwrap();
    std::os::unix::fs::symlink(
        lease_prof.join("deps/libceleris-00000001.rlib"),
        prof.join("deps/liblinked-0000000b.rlib"),
    )
    .unwrap();

    let before = tree(&lease);
    let report = run_sweep(
        std::slice::from_ref(&root),
        &SweepParams::default(),
        TargetSweepMode::Apply,
        &FixedEnv { now: now() },
    );
    assert_eq!(tree(&lease), before, "release-build lease was touched");
    assert!(old_items.iter().all(|p| !p.exists()));
    assert!(
        report.deleted.iter().all(|d| !d.path.starts_with(&lease)),
        "{report:?}"
    );
    assert!(
        report
            .skipped
            .iter()
            .any(|s| s.reason == "symlink" && s.path == prof.join("deps/liblinked-0000000b.rlib")),
        "{report:?}"
    );
    assert!(prof.join("deps/liblinked-0000000b.rlib").is_symlink());
}

#[test]
fn target_sweep_cleans_leftover_deleting_dirs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("cargo");
    let prof = profile(&root.join("t"), "debug");
    let (new_items, _) = add_crate(&prof, "new-00000002", now() - DAY);
    let in_profile = prof.join(".deleting-01PREV");
    std::fs::create_dir_all(in_profile.join("deps")).unwrap();
    std::fs::write(in_profile.join("deps/libx-01.rlib"), b"x").unwrap();
    let in_root = root.join(".deleting-01PREVTARGET");
    profile(&in_root, "debug");
    let in_repo = root.join("repo").join(".deleting-01PREVWU");
    std::fs::create_dir_all(&in_repo).unwrap();

    let report = run_sweep(
        std::slice::from_ref(&root),
        &SweepParams::default(),
        TargetSweepMode::Apply,
        &FixedEnv { now: now() },
    );
    for p in [&in_profile, &in_root, &in_repo] {
        assert!(!p.exists(), "leftover kept: {}", p.display());
    }
    assert!(new_items.iter().all(|p| p.exists()));
    let mut leftovers = report.leftovers.clone();
    leftovers.sort();
    let mut expected = vec![in_profile, in_root, in_repo];
    expected.sort();
    assert_eq!(leftovers, expected);
    assert!(report.deleted.is_empty(), "{report:?}");
    assert!(report.errors.is_empty(), "{report:?}");
}

#[test]
fn target_sweep_scratch_cap_preserves_live_owners_and_cargo_locks() {
    struct Lookup;
    impl task_worker::scratch::StatusLookup for Lookup {
        fn task_status(&self, id: &str) -> Option<task_core::Status> {
            Some(if id == "running" {
                task_core::Status::Running
            } else {
                task_core::Status::Done
            })
        }
        fn work_unit_status(&self, _: &str) -> Option<task_core::WorkUnitStatus> {
            None
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("targets");
    let old = now() - DAY * 2;
    let mut items = Vec::new();
    for owner in [
        "task-done",
        "task-done/wu-one",
        "task-locked",
        "task-running",
        "release-build",
        "agent-external",
    ] {
        let dir = root.join(owner);
        let p = profile(&dir.join("target"), "debug");
        let (paths, _) = add_crate(&p, "foo-1234567890abcdef", old);
        std::fs::write(dir.join("lease.json"), "preserved").unwrap();
        items.push((dir, p, paths));
    }
    let held = Flock::lock(
        File::open(items[2].1.join(CARGO_LOCK)).unwrap(),
        FlockArg::LockExclusiveNonblock,
    )
    .unwrap();
    let params = SweepParams {
        max_bytes_per_root: 4000,
        target_ratio: 0.5,
        ..Default::default()
    };
    let before = tree(&root);
    let dry = run_sweep_with_scratch(
        std::slice::from_ref(&root),
        std::slice::from_ref(&root),
        &params,
        TargetSweepMode::DryRun,
        &FixedEnv { now: now() },
        &Lookup,
    );
    assert!(dry.errors.is_empty(), "{:?}", dry.errors);
    assert_eq!(tree(&root), before);
    let applied = run_sweep_with_scratch(
        std::slice::from_ref(&root),
        std::slice::from_ref(&root),
        &params,
        TargetSweepMode::Apply,
        &FixedEnv { now: now() },
        &Lookup,
    );
    assert!(applied.errors.is_empty(), "{:?}", applied.errors);
    assert!(applied.roots[0].by_reason.cap > 0);
    for (i, (dir, _, paths)) in items.iter().enumerate() {
        assert!(dir.join("lease.json").exists());
        for p in paths {
            assert_eq!(p.exists(), i >= 2, "{}", p.display());
        }
    }
    assert!(
        applied
            .skipped
            .iter()
            .any(|s| s.reason == "build_in_progress")
    );
    assert!(
        applied
            .skipped
            .iter()
            .any(|s| s.reason == "active_or_unknown_owner")
    );
    drop(held);
}
