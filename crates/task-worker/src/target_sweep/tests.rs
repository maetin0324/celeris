use super::*;

const NOW_SECS: u64 = 1_800_000_000;

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(NOW_SECS)
}

fn days_ago(days: u64) -> SystemTime {
    now() - Duration::from_secs(days * DAY_SECS)
}

fn params() -> SweepParams {
    SweepParams {
        roots: vec![PathBuf::from("/r")],
        ..SweepParams::default()
    }
}

/// `/r/<target>/debug/<sub>/<name>` の項目。
fn item(
    target: &str,
    sub: &str,
    name: &str,
    kind: ItemKind,
    bytes: u64,
    used: SystemTime,
) -> TargetSnapshot {
    let target_dir = PathBuf::from("/r").join(target);
    let profile_dir = target_dir.join("debug");
    let key = match kind {
        ItemKind::Deps => deps_key(name),
        _ => dir_key(name),
    }
    .unwrap_or_else(|| name.to_string());
    TargetSnapshot {
        root: PathBuf::from("/r"),
        path: profile_dir.join(sub).join(name),
        target_dir,
        profile_dir,
        kind,
        key,
        bytes,
        used_at: used,
        symlink: false,
    }
}

fn deleted_paths(plan: &SweepPlan) -> Vec<String> {
    plan.delete
        .iter()
        .map(|d| d.path.display().to_string())
        .collect()
}

#[test]
fn target_sweep_params_default_values() {
    let p = SweepParams::default();
    assert_eq!(p.max_age_days, 7);
    assert_eq!(p.max_bytes_per_root, 120 * GIB);
    assert_eq!(p.target_ratio, 0.8);
    assert_eq!(p.stale_target_days, 14);
    assert!(p.roots.is_empty());
    assert_eq!(p.cap_target_bytes(), 96 * GIB);
}

#[test]
fn target_sweep_deletes_items_older_than_max_age() {
    let snap = vec![
        item(
            "t",
            "incremental",
            "old-0a0a",
            ItemKind::Incremental,
            10,
            days_ago(8),
        ),
        item(
            "t",
            "incremental",
            "new-0b0b",
            ItemKind::Incremental,
            20,
            days_ago(1),
        ),
        item(
            "t",
            "incremental",
            "edge-0c0c",
            ItemKind::Incremental,
            30,
            days_ago(7),
        ),
    ];
    let plan = plan(&snap, &BTreeSet::new(), now(), &params());
    assert_eq!(
        deleted_paths(&plan),
        vec!["/r/t/debug/incremental/old-0a0a"]
    );
    assert_eq!(plan.delete[0].reason, DeleteReason::Age);
    assert_eq!(plan.delete[0].bytes, 10);
    assert_eq!(plan.roots.len(), 1);
    assert_eq!(plan.roots[0].before_bytes, 60);
    assert_eq!(plan.roots[0].after_bytes, 50);
    assert!(!plan.over_cap_unresolved);
    assert!(plan.skip.is_empty());
}

#[test]
fn target_sweep_trims_oldest_until_80pct_of_cap() {
    let mut p = params();
    p.max_bytes_per_root = 100;
    // 合計 130 > 100。80 以下まで古い順に消す。同時刻（3 日前）は path の辞書順。
    let snap = vec![
        item(
            "t",
            "incremental",
            "a-01",
            ItemKind::Incremental,
            30,
            days_ago(3),
        ),
        item(
            "t",
            "incremental",
            "b-02",
            ItemKind::Incremental,
            30,
            days_ago(3),
        ),
        item(
            "t",
            "incremental",
            "c-03",
            ItemKind::Incremental,
            30,
            days_ago(5),
        ),
        item(
            "t",
            "incremental",
            "d-04",
            ItemKind::Incremental,
            40,
            days_ago(1),
        ),
    ];
    let plan = plan(&snap, &BTreeSet::new(), now(), &p);
    assert_eq!(
        deleted_paths(&plan),
        vec!["/r/t/debug/incremental/c-03", "/r/t/debug/incremental/a-01"]
    );
    assert!(plan.delete.iter().all(|d| d.reason == DeleteReason::Cap));
    assert_eq!(plan.roots[0].before_bytes, 130);
    assert_eq!(plan.roots[0].after_bytes, 70);
    assert!(!plan.over_cap_unresolved);
}

#[test]
fn target_sweep_cap_is_not_applied_under_cap() {
    let mut p = params();
    p.max_bytes_per_root = 100;
    // 合計 90 は上限以下（80% より上でも上限を超えていなければ消さない）。
    let snap = vec![
        item(
            "t",
            "incremental",
            "a-01",
            ItemKind::Incremental,
            45,
            days_ago(3),
        ),
        item(
            "t",
            "incremental",
            "b-02",
            ItemKind::Incremental,
            45,
            days_ago(2),
        ),
    ];
    let plan = plan(&snap, &BTreeSet::new(), now(), &p);
    assert!(plan.delete.is_empty());
}

#[test]
fn target_sweep_skips_locked_profile_in_plan() {
    let mut p = params();
    p.max_bytes_per_root = 100;
    let mut snap = vec![
        item(
            "busy",
            "incremental",
            "old-01",
            ItemKind::Incremental,
            100,
            days_ago(30),
        ),
        item(
            "busy",
            "deps",
            "libold-02.rlib",
            ItemKind::Deps,
            50,
            days_ago(30),
        ),
        item(
            "idle",
            "incremental",
            "old-03",
            ItemKind::Incremental,
            5,
            days_ago(8),
        ),
    ];
    // 別の profile（release）の項目は lock と無関係に消える。
    let mut rel = item(
        "busy",
        "incremental",
        "rel-04",
        ItemKind::Incremental,
        7,
        days_ago(9),
    );
    rel.profile_dir = PathBuf::from("/r/busy/release");
    rel.path = PathBuf::from("/r/busy/release/incremental/rel-04");
    snap.push(rel);
    let locked: BTreeSet<PathBuf> = [PathBuf::from("/r/busy/debug")].into_iter().collect();
    let plan = plan(&snap, &locked, now(), &p);
    let deleted = deleted_paths(&plan);
    assert!(
        deleted.iter().all(|d| !d.starts_with("/r/busy/debug")),
        "{deleted:?}"
    );
    // lock の取れない profile を持つ target は丸ごと消さない。
    assert!(!deleted.contains(&"/r/busy".to_string()));
    assert!(deleted.contains(&"/r/busy/release/incremental/rel-04".to_string()));
    assert!(deleted.contains(&"/r/idle/debug/incremental/old-03".to_string()));
    assert_eq!(
        plan.skip,
        vec![Skip {
            path: PathBuf::from("/r/busy/debug"),
            reason: SkipReason::BuildInProgress,
        }]
    );
    // 消さないことを優先し、上限 80% を割れなければ記録する。
    assert_eq!(plan.roots[0].before_bytes, 162);
    assert_eq!(plan.roots[0].after_bytes, 150);
    assert!(plan.roots[0].over_cap_unresolved);
    assert!(plan.over_cap_unresolved);
}

#[test]
fn target_sweep_groups_deps_by_key() {
    assert_eq!(
        deps_key("libserde-0123abcd.rlib").as_deref(),
        Some("serde-0123abcd")
    );
    assert_eq!(
        deps_key("libserde-0123abcd.rmeta").as_deref(),
        Some("serde-0123abcd")
    );
    assert_eq!(
        deps_key("serde-0123abcd.d").as_deref(),
        Some("serde-0123abcd")
    );
    assert_eq!(deps_key("liblibc-ff00.rlib").as_deref(), Some("libc-ff00"));
    assert_eq!(deps_key("libc-ff00.d").as_deref(), Some("libc-ff00"));
    assert_eq!(
        deps_key("task_worker-9f9f").as_deref(),
        Some("task_worker-9f9f")
    );
    assert_eq!(
        deps_key("foo-1a2b.foo.3c4d-cgu.0.rcgu.o").as_deref(),
        Some("foo-1a2b")
    );
    assert_eq!(deps_key("README"), None);
    assert_eq!(deps_key("foo-nothex.d"), None);
    assert_eq!(dir_key("serde-0123abcd").as_deref(), Some("serde-0123abcd"));

    let groups = group_deps_by_key([
        "libserde-0123abcd.rlib",
        "libserde-0123abcd.rmeta",
        "serde-0123abcd.d",
        "foo-1a2b",
        "stray",
    ]);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups["serde-0123abcd"].len(), 3);
    assert_eq!(groups["foo-1a2b"], vec!["foo-1a2b"]);

    // deps・.fingerprint・build は使用時刻の最大で 1 単位として一緒に消える（片方が新しければ全部残る）。
    let snap = vec![
        item(
            "t",
            "deps",
            "libserde-0a.rlib",
            ItemKind::Deps,
            10,
            days_ago(9),
        ),
        item("t", "deps", "serde-0a.d", ItemKind::Deps, 1, days_ago(9)),
        item(
            "t",
            ".fingerprint",
            "serde-0a",
            ItemKind::Fingerprint,
            1,
            days_ago(10),
        ),
        item("t", "build", "serde-0a", ItemKind::Build, 2, days_ago(10)),
        item(
            "t",
            "deps",
            "libfoo-0b.rlib",
            ItemKind::Deps,
            10,
            days_ago(9),
        ),
        item(
            "t",
            ".fingerprint",
            "foo-0b",
            ItemKind::Fingerprint,
            1,
            days_ago(1),
        ),
        // incremental は同じ key でも別の単位。
        item(
            "t",
            "incremental",
            "foo-0b",
            ItemKind::Incremental,
            5,
            days_ago(9),
        ),
    ];
    let plan = plan(&snap, &BTreeSet::new(), now(), &params());
    let mut deleted = deleted_paths(&plan);
    deleted.sort();
    assert_eq!(
        deleted,
        vec![
            "/r/t/debug/.fingerprint/serde-0a",
            "/r/t/debug/build/serde-0a",
            "/r/t/debug/deps/libserde-0a.rlib",
            "/r/t/debug/deps/serde-0a.d",
            "/r/t/debug/incremental/foo-0b",
        ]
    );
    assert_eq!(plan.deleted_bytes(), 19);
}

#[test]
fn target_sweep_stale_target_dir_whole() {
    let mut p = params();
    p.max_age_days = 30; // 古さの段では消えない設定で、放置 target の段だけを見る。
    let snap = vec![
        item(
            "review-x",
            "incremental",
            "a-01",
            ItemKind::Incremental,
            10,
            days_ago(20),
        ),
        item(
            "review-x",
            "deps",
            "liba-02.rlib",
            ItemKind::Deps,
            5,
            days_ago(15),
        ),
        item(
            "live",
            "incremental",
            "b-03",
            ItemKind::Incremental,
            7,
            days_ago(20),
        ),
        item(
            "live",
            "incremental",
            "c-04",
            ItemKind::Incremental,
            7,
            days_ago(3),
        ),
    ];
    let plan = plan(&snap, &BTreeSet::new(), now(), &p);
    assert_eq!(
        plan.delete,
        vec![PlannedDelete {
            root: PathBuf::from("/r"),
            path: PathBuf::from("/r/review-x"),
            bytes: 15,
            reason: DeleteReason::StaleTarget,
        }]
    );
    assert_eq!(plan.roots[0].after_bytes, 14);

    // 既定値では古さの段が先に項目を消し、target 全体の削除は残りの分だけを数える（二重に数えない）。
    let plan = super::plan(&snap, &BTreeSet::new(), now(), &params());
    let stale: Vec<_> = plan
        .delete
        .iter()
        .filter(|d| d.reason == DeleteReason::StaleTarget)
        .collect();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].path, PathBuf::from("/r/review-x"));
    assert_eq!(stale[0].bytes, 0);
    assert_eq!(plan.deleted_bytes(), 22);
    assert_eq!(plan.roots[0].after_bytes, 7);

    // lock が取れない profile を持つ target は丸ごと消さない。
    let locked: BTreeSet<PathBuf> = [PathBuf::from("/r/review-x/debug")].into_iter().collect();
    let plan = super::plan(&snap, &locked, now(), &p);
    assert!(plan.delete.is_empty());
}

#[test]
fn target_sweep_rejects_outside_root_and_symlink() {
    let mut outside = item(
        "t",
        "incremental",
        "x-01",
        ItemKind::Incremental,
        10,
        days_ago(30),
    );
    outside.root = PathBuf::from("/other");
    outside.target_dir = PathBuf::from("/other/t");
    outside.profile_dir = PathBuf::from("/other/t/debug");
    outside.path = PathBuf::from("/other/t/debug/incremental/x-01");
    let mut escape = item(
        "t",
        "incremental",
        "y-02",
        ItemKind::Incremental,
        10,
        days_ago(30),
    );
    escape.path = PathBuf::from("/r/t/debug/../../../etc/y-02");
    let mut wrong_profile = item(
        "t",
        "incremental",
        "z-03",
        ItemKind::Incremental,
        10,
        days_ago(30),
    );
    wrong_profile.path = PathBuf::from("/r/u/debug/incremental/z-03");
    let mut link = item(
        "t",
        "incremental",
        "l-04",
        ItemKind::Incremental,
        10,
        days_ago(30),
    );
    link.symlink = true;
    let ok = item(
        "t",
        "incremental",
        "ok-05",
        ItemKind::Incremental,
        10,
        days_ago(30),
    );
    let snap = vec![outside, escape, wrong_profile, link, ok];
    let plan = plan(&snap, &BTreeSet::new(), now(), &params());
    assert_eq!(deleted_paths(&plan), vec!["/r/t/debug/incremental/ok-05"]);
    let skips: Vec<(String, SkipReason)> = plan
        .skip
        .iter()
        .map(|s| (s.path.display().to_string(), s.reason))
        .collect();
    assert_eq!(
        skips,
        vec![
            (
                "/other/t/debug/incremental/x-01".to_string(),
                SkipReason::OutsideRoot
            ),
            (
                "/r/t/debug/../../../etc/y-02".to_string(),
                SkipReason::OutsideRoot
            ),
            (
                "/r/u/debug/incremental/z-03".to_string(),
                SkipReason::OutsideRoot
            ),
            (
                "/r/t/debug/incremental/l-04".to_string(),
                SkipReason::Symlink
            ),
        ]
    );
    // skip のある target は丸ごと消さない。
    assert!(
        plan.delete
            .iter()
            .all(|d| d.reason != DeleteReason::StaleTarget)
    );
}

#[test]
fn target_sweep_plan_serializes_to_json() {
    let snap = vec![item(
        "t",
        "incremental",
        "a-01",
        ItemKind::Incremental,
        10,
        days_ago(8),
    )];
    let locked: BTreeSet<PathBuf> = BTreeSet::new();
    let plan = plan(&snap, &locked, now(), &params());
    let json = serde_json::to_value(&plan).expect("serialize");
    assert_eq!(json["delete"][0]["reason"], "age");
    assert_eq!(json["delete"][0]["bytes"], 10);
    assert_eq!(json["roots"][0]["before_bytes"], 10);
    assert_eq!(json["over_cap_unresolved"], false);
    let snap_json = serde_json::to_value(&snap[0]).expect("serialize");
    assert_eq!(snap_json["kind"], "incremental");
    let skip = serde_json::to_value(Skip {
        path: PathBuf::from("/r/t/debug"),
        reason: SkipReason::BuildInProgress,
    })
    .expect("serialize");
    assert_eq!(skip["reason"], "build_in_progress");
    assert_eq!(
        serde_json::to_value(DeleteReason::StaleTarget).expect("serialize"),
        "stale_target"
    );
}
