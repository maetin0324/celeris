use super::*;

fn config(tmp: &Path) -> PathBuf {
    let path = tmp.join("config.toml");
    std::fs::write(
            &path,
            format!(
                "db = \"{}\"\n[workspace]\nbuild_cache_dir = \"{}\"\n[scratch]\ndir = \"{}\"\n[scratch.l2]\ndir = \"{}\"\n[scratch.cache_server]\nport = 1\n[selfdeploy]\nreleases_dir = \"{}\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
                tmp.join("celeris.sqlite3").display(),
                tmp.join("build-cache").display(),
                tmp.join("scratch").display(),
                tmp.join("l2").display(),
                tmp.join("releases").display(),
            ),
        )
        .unwrap();
    path
}

fn owner_args(config: &Path, owner: &str) -> OwnerArgs {
    OwnerArgs {
        config: ConfigArg {
            config: config.to_path_buf(),
        },
        owner_pos: Some(owner.to_string()),
        owner: None,
    }
}

/// ADR-0129 (1): `celerisctl scratch env` は `CARGO_TARGET_DIR` と `[scratch.cargo]` だけを出す。継いだ
/// `RUSTC_WRAPPER` / `SCCACHE_*` には触らない（export もしないし unset もしない。host の cargo 設定を通す）。
#[test]
fn env_has_only_target_and_cargo_tuning() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = config(tmp.path());
    let cfg = load(&cfg_path).unwrap();
    let settings = active_settings(&cfg).unwrap();
    // dispatcher の設定（`Config::dispatch_config().scratch`）と CLI の設定は同じ値。
    assert_eq!(cfg.dispatch_config().scratch, settings);
    let owner = Owner::parse("agent-a5caa712b0867e383").unwrap();
    let text = render_env(&settings, &owner);
    assert!(!text.starts_with("unset"), "{text}");
    for key in ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "SCCACHE"] {
        assert!(!text.contains(key), "{text}");
    }
    let parsed: Vec<(String, String)> = text
        .lines()
        .map(|l| {
            let (k, v) = l.strip_prefix("export ").unwrap().split_once('=').unwrap();
            (k.to_string(), v.to_string())
        })
        .collect();
    let mut expected = task_worker::scratch::target_env(&settings.pool(), &owner);
    expected.extend(task_worker::scratch::cargo_tuning_env(&settings.cargo));
    assert_eq!(parsed, expected);
    assert_eq!(
        parsed[0].1,
        tmp.path()
            .join("scratch/targets/agent-a5caa712b0867e383/target")
            .display()
            .to_string()
    );
    // `--repo` を渡せば lease も作る。
    run(
        None,
        ScratchCommand::Env(EnvArgs {
            owner: owner_args(&cfg_path, "agent-a5caa712b0867e383"),
            repo: Some(tmp.path().to_path_buf()),
            worktree: None,
            base: None,
            ttl: None,
        }),
    )
    .unwrap();
    assert!(settings.pool().lease_path(&owner).exists());
}

/// ADR-0129 (1): sccache と cache server は Celeris の外（host の cargo 設定）。`scratch status` の `sccache` /
/// `cache` は常に空（cache server の `/stats` も sccache の `--show-stats` も問い合わせない）。
#[test]
fn status_has_no_sccache_or_cache_server() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = load(&config(tmp.path())).unwrap();
    let status = status_of(&cfg, None).unwrap();
    assert!(status.sccache.is_none());
    assert!(status.cache.is_none());
    print_status(&status);
}

/// 外部 lease の作成 → touch → release（P3）→ 再 lease の往復。daemon の owner には lease を取らせない。
#[test]
fn lease_touch_release_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = config(tmp.path());
    let cfg = load(&cfg_path).unwrap();
    let settings = active_settings(&cfg).unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    // release.sh の `.build/<sha12>`（`.git` がファイルの worktree）。checkout は今。
    let tree = tmp.path().join("build");
    std::fs::create_dir_all(&tree).unwrap();
    std::fs::write(tree.join(".git"), "gitdir: /x\n").unwrap();
    let lease_args = |owner: &str, ttl: Option<u64>| LeaseArgs {
        owner: owner_args(&cfg_path, owner),
        repo: repo.clone(),
        worktree: Some(tree.clone()),
        base: Some("abc".into()),
        ttl,
    };
    run(
        None,
        ScratchCommand::Lease(lease_args("release-0123456789ab", Some(60))),
    )
    .unwrap();
    let owner = Owner::parse("release-0123456789ab").unwrap();
    let pool = settings.pool();
    let lease = scratch::read_lease(&pool.lease_path(&owner))
        .unwrap()
        .unwrap();
    assert_eq!(lease.ttl_secs, Some(60));
    assert_eq!(lease.base_commit.as_deref(), Some("abc"));
    let status = status_of(&cfg, None).unwrap();
    let row = |s: &task_ops::daemon::ScratchStatus| {
        s.owners
            .iter()
            .find(|o| o.owner == "release-0123456789ab")
            .unwrap()
            .class
            .clone()
    };
    assert_eq!(row(&status), "p0");
    let old = SystemTime::now() - std::time::Duration::from_secs(120);
    scratch::set_mtime(&pool.lease_path(&owner), old).unwrap();
    assert_eq!(row(&status_of(&cfg, None).unwrap()), "p3");
    run(
        None,
        ScratchCommand::Touch(owner_args(&cfg_path, "release-0123456789ab")),
    )
    .unwrap();
    assert_eq!(row(&status_of(&cfg, None).unwrap()), "p0");
    run(
        None,
        ScratchCommand::Release(owner_args(&cfg_path, "release-0123456789ab")),
    )
    .unwrap();
    assert_eq!(row(&status_of(&cfg, None).unwrap()), "p3");
    assert!(
        scratch::read_lease(&pool.lease_path(&owner))
            .unwrap()
            .unwrap()
            .released_at
            .is_some()
    );
    // 次の release は前の release の target を引き継ぐ（前の lease の最終書き込みが今より前）。
    std::fs::write(pool.target_dir(&owner).join("warm"), "x").unwrap();
    for p in [
        pool.target_dir(&owner).join("warm"),
        pool.target_dir(&owner),
        pool.lease_path(&owner),
    ] {
        std::fs::File::open(&p)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
    }
    run(
        None,
        ScratchCommand::Lease(lease_args("release-ba9876543210", None)),
    )
    .unwrap();
    let next = Owner::parse("release-ba9876543210").unwrap();
    assert!(pool.target_dir(&next).join("warm").exists());
    assert_eq!(
        scratch::read_lease(&pool.lease_path(&next))
            .unwrap()
            .unwrap()
            .adopted_from
            .as_deref(),
        Some("release-0123456789ab")
    );
    // daemon の owner・touch の対象が無いときは失敗。
    assert!(run(None, ScratchCommand::Lease(lease_args("task-01ABC", None))).is_err());
    assert!(
        run(
            None,
            ScratchCommand::Touch(owner_args(&cfg_path, "agent-none"))
        )
        .is_err()
    );
}

/// `gc --dry-run` は一覧だけで何も消さない。`gc` は消す（daemon と同じ `plan_gc`）。legacy は 1h 以上 idle のものだけ。
#[test]
fn gc_dry_run_lists_without_removing() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = config(tmp.path());
    let cfg = load(&cfg_path).unwrap();
    let settings = active_settings(&cfg).unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run(
        None,
        ScratchCommand::Lease(LeaseArgs {
            owner: owner_args(&cfg_path, "release-0123456789ab"),
            repo: repo.clone(),
            worktree: None,
            base: None,
            ttl: None,
        }),
    )
    .unwrap();
    run(
        None,
        ScratchCommand::Release(owner_args(&cfg_path, "release-0123456789ab")),
    )
    .unwrap();
    // legacy: 古いもの（消える）と新しいもの（残る）。
    let old_legacy = tmp.path().join("build-cache/cargo/old-abc");
    let new_legacy = tmp.path().join("build-cache/cargo/agent-platform-g1");
    std::fs::create_dir_all(&old_legacy).unwrap();
    std::fs::create_dir_all(&new_legacy).unwrap();
    std::fs::write(old_legacy.join("f"), "x").unwrap();
    let old = SystemTime::now() - std::time::Duration::from_secs(7200);
    for p in [old_legacy.join("f"), old_legacy.clone()] {
        std::fs::File::open(&p)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
    }
    let owner = Owner::parse("release-0123456789ab").unwrap();
    let text = gc(&cfg, &settings, None, true).unwrap();
    assert!(text.contains("would remove release-0123456789ab"), "{text}");
    assert!(
        text.contains(&format!("would remove {}", old_legacy.display())),
        "{text}"
    );
    assert!(!text.contains("agent-platform-g1"), "{text}");
    assert!(settings.pool().target_dir(&owner).exists());
    assert!(old_legacy.exists());
    let text = gc(&cfg, &settings, None, false).unwrap();
    assert!(text.contains("removed release-0123456789ab"), "{text}");
    assert!(!settings.pool().target_dir(&owner).exists());
    assert!(!old_legacy.exists());
    assert!(new_legacy.exists());
    assert!(!scratch_gc::has_pending_deletes(
        &scratch_gc::deleting_roots(&settings.pool(), &[old_legacy])
    ));
}

#[test]
fn disabled_scratch_makes_lease_fail_so_release_sh_falls_back() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    std::fs::write(
        &path,
        "[scratch]\nenabled = false\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    let err = run(
        None,
        ScratchCommand::Lease(LeaseArgs {
            owner: owner_args(&path, "release-0123456789ab"),
            repo: tmp.path().to_path_buf(),
            worktree: None,
            base: None,
            ttl: None,
        }),
    )
    .unwrap_err();
    assert!(err.to_string().contains("scratch is disabled"), "{err}");
    let status = status_of(&load(&path).unwrap(), None).unwrap();
    assert!(!status.enabled);
}
