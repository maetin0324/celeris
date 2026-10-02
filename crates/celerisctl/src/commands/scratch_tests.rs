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

/// ADR-0075 §5 G1 受け入れ条件 6: `celerisctl scratch env` の出力と dispatcher が組む env が同じ。
#[test]
fn env_matches_the_dispatcher_env() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = config(tmp.path());
    let cfg = load(&cfg_path).unwrap();
    let settings = active_settings(&cfg).unwrap();
    // dispatcher の設定（`Config::dispatch_config().scratch`）と CLI の設定は同じ値。
    assert_eq!(cfg.dispatch_config().scratch, settings);
    let owner = Owner::parse("agent-a5caa712b0867e383").unwrap();
    let text = render_env(&settings, &owner);
    // G3-fix1: 先頭の 1 行は dispatcher が外すのと同じ key の `unset`（sccache の族のうち与えないもの。この
    // host の既定の sccache の server が動いているかで中身は変わる）。
    let mut lines = text.lines();
    let unset: Vec<String> = lines
        .next()
        .unwrap()
        .strip_prefix("unset ")
        .unwrap()
        .split(' ')
        .map(String::from)
        .collect();
    let child = task_worker::scratch::cargo_child_env(&settings, &owner);
    assert_eq!(unset, child.remove);
    for key in ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "SCCACHE_DIR"] {
        let set = child.set.iter().any(|(k, _)| k == key);
        assert!(
            set != unset.iter().any(|k| k == key),
            "{key} must be either exported or unset: {text}"
        );
    }
    let parsed: Vec<(String, String)> = lines
        .map(|l| {
            let (k, v) = l.strip_prefix("export ").unwrap().split_once('=').unwrap();
            (k.to_string(), v.to_string())
        })
        .collect();
    assert_eq!(parsed, task_worker::scratch::cargo_env(&settings, &owner));
    assert_eq!(parsed, child.set);
    // shell で評価すると、継いだ RUSTC_WRAPPER / SCCACHE_* は消え、Celeris の値だけが残る。
    let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "{text}echo \"${{RUSTC_WRAPPER-unset}}|${{SCCACHE_DIR-unset}}|${{RUSTC_WORKSPACE_WRAPPER-unset}}\""
            ))
            .env("RUSTC_WRAPPER", "/inherited/sccache")
            .env("RUSTC_WORKSPACE_WRAPPER", "/inherited/ws")
            .env("SCCACHE_DIR", "/inherited/dir")
            .output()
            .unwrap();
    assert!(out.status.success(), "{out:?}");
    let value = |key: &str| {
        child
            .set
            .iter()
            .find(|(k, _)| k == key)
            .map_or("unset".to_string(), |(_, v)| v.clone())
    };
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("{}|{}|unset", value("RUSTC_WRAPPER"), value("SCCACHE_DIR"))
    );
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
            server: false,
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

/// ADR-0075 §5 G3: `env --server` は cache server が応答すれば webdav（`SCCACHE_WEBDAV_*`、token、`SCCACHE_DIR` なし）、
/// 応答しなければ G2 の local disk を選び、選んだ方を `<scratch>/bin/sccache-server.mode` に書く。dispatcher と
/// `scratch env` は webdav のときだけ cache server の応答も確かめ、無ければ sccache 系を与えない。
#[test]
fn env_server_switches_to_webdav_when_the_cache_server_is_up() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("sccache-real");
    std::fs::write(&bin, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cfg_path = config(tmp.path());
    let mut text = std::fs::read_to_string(&cfg_path).unwrap();
    text.push_str(&format!(
        "[scratch.sccache]\nbinary = \"{}\"\nport = 1\n",
        bin.display()
    ));
    std::fs::write(&cfg_path, text).unwrap();
    let settings = active_settings(&load(&cfg_path).unwrap()).unwrap();
    task_worker::scratch::ensure_token(&settings.cache_server.token_file).unwrap();
    let token = task_worker::scratch::read_token(&settings.cache_server.token_file).unwrap();
    assert_eq!(token.len(), 64);

    let server = render_server_env_with(&settings, |_| true).unwrap();
    assert!(
        server.starts_with("# celeris: sccache backend = webdav http://127.0.0.1:1 "),
        "{server}"
    );
    assert!(server.contains("export SCCACHE_WEBDAV_ENDPOINT=http://127.0.0.1:1\n"));
    assert!(server.contains("export SCCACHE_WEBDAV_KEY_PREFIX=sccache\n"));
    assert!(server.contains(&format!("export SCCACHE_WEBDAV_TOKEN={token}\n")));
    assert!(server.contains("export SCCACHE_SERVER_PORT=1\n"));
    assert!(!server.contains("SCCACHE_DIR"), "{server}");
    let pool = settings.pool();
    assert_eq!(
        task_worker::scratch::read_sccache_mode(&pool).as_deref(),
        Some("webdav")
    );
    // webdav の sccache に対して cache server が応答しなければ配線しない。
    let down = task_worker::scratch::resolve_sccache_with(&settings, |_| true, |_| false);
    assert!(
        down.reason().unwrap_or_default().contains("cache server"),
        "{down:?}"
    );
    let up = task_worker::scratch::resolve_sccache_with(&settings, |_| true, |_| true);
    assert!(up.wrapper().is_some(), "{up:?}");
    // client（run）の env は G2 のまま（webdav 系も token も入れない）。
    let env =
        task_worker::scratch::cargo_env_with(&settings, &Owner::parse("agent-g3").unwrap(), &up);
    assert!(
        env.iter().all(|(k, _)| !k.starts_with("SCCACHE_WEBDAV")),
        "{env:?}"
    );

    // cache server が居なければ disk に戻り、記録も disk（cache server の有無に関係なく配線する）。
    let server = render_server_env_with(&settings, |_| false).unwrap();
    assert!(server.contains("export SCCACHE_DIR="), "{server}");
    assert!(!server.contains("WEBDAV"), "{server}");
    assert_eq!(
        task_worker::scratch::read_sccache_mode(&pool).as_deref(),
        Some("disk")
    );
    let disk = task_worker::scratch::resolve_sccache_with(&settings, |_| true, |_| false);
    assert!(disk.wrapper().is_some(), "{disk:?}");
}

/// ADR-0075 §5 G2 受け入れ条件 2: server が応答するとき `scratch env` は dispatcher と同じ sccache 系を含み、
/// `env --server` は server の env（client と同じ値）と本物のバイナリを出す。`status` に配線の状態が出る。
#[test]
fn env_includes_sccache_when_the_server_is_up() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("sccache-real");
    std::fs::write(&bin, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            drop(conn);
        }
    });
    let cfg_path = config(tmp.path());
    let mut text = std::fs::read_to_string(&cfg_path).unwrap();
    text.push_str(&format!(
        "[scratch.sccache]\nbinary = \"{}\"\nport = {port}\n",
        bin.display()
    ));
    std::fs::write(&cfg_path, text).unwrap();
    let cfg = load(&cfg_path).unwrap();
    let settings = active_settings(&cfg).unwrap();
    assert_eq!(cfg.dispatch_config().scratch, settings);
    let owner = Owner::parse("agent-g2").unwrap();
    let env = task_worker::scratch::cargo_env(&settings, &owner);
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "CARGO_TARGET_DIR",
            "CARGO_INCREMENTAL",
            "CARGO_PROFILE_DEV_DEBUG",
            "RUSTC_WRAPPER",
            "SCCACHE_DIR",
            "SCCACHE_CACHE_SIZE",
            "SCCACHE_SERVER_PORT",
            "SCCACHE_IDLE_TIMEOUT"
        ]
    );
    let rendered = render_env(&settings, &owner);
    // G3-fix1: 配線するときは与えない族（workspace wrapper・webdav 系）だけを `unset` する。
    let mut lines = rendered.lines();
    let unset = lines.next().unwrap_or_default();
    assert!(
        unset.starts_with("unset ")
            && !unset.contains("RUSTC_WRAPPER ")
            && !unset.contains(" SCCACHE_DIR"),
        "{rendered}"
    );
    assert!(unset.contains("RUSTC_WORKSPACE_WRAPPER"), "{rendered}");
    let parsed: Vec<(String, String)> = lines
        .map(|l| {
            let (k, v) = l.strip_prefix("export ").unwrap().split_once('=').unwrap();
            (k.to_string(), v.to_string())
        })
        .collect();
    assert_eq!(parsed, env);
    let server = render_server_env_with(&settings, |_| false).unwrap();
    assert!(
        server.starts_with("# celeris: sccache backend = disk "),
        "{server}"
    );
    assert!(server.contains(&format!(
        "export SCCACHE_DIR={}\n",
        tmp.path().join("scratch/sccache-l1").display()
    )));
    assert!(server.contains(&format!("export SCCACHE_SERVER_PORT={port}\n")));
    assert!(server.contains("export SCCACHE_CACHE_SIZE=40G\n"));
    assert!(server.contains(&format!("export CELERIS_SCCACHE_BIN={}\n", bin.display())));
    assert!(!server.contains("RUSTC_WRAPPER"));
    // ADR-0129 (1): `status` は sccache の欄を持たない。
    assert!(status_of(&cfg, None).unwrap().sccache.is_none());
    // server が居なければ sccache 系は消え、`status` は理由を出す（閉じた port は特権 port の 1。並行するテストと
    // 競合しない）。
    let text = std::fs::read_to_string(&cfg_path)
        .unwrap()
        .replace(&format!("port = {port}"), "port = 1");
    std::fs::write(&cfg_path, text).unwrap();
    let cfg = load(&cfg_path).unwrap();
    let settings = active_settings(&cfg).unwrap();
    let env = task_worker::scratch::cargo_env(&settings, &owner);
    assert!(!env.iter().any(|(k, _)| k == "RUSTC_WRAPPER"));
    // G3-fix1: shell の snippet は人の shell が継いだ wrapper と SCCACHE_* を外す。
    let text = render_env(&settings, &owner);
    let unset = text.lines().next().unwrap_or_default();
    for key in [
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "SCCACHE_DIR",
        "SCCACHE_SERVER_PORT",
    ] {
        assert!(
            unset.starts_with("unset ") && unset.split(' ').any(|k| k == key),
            "{key}: {text}"
        );
    }
    assert!(status_of(&cfg, None).unwrap().sccache.is_none());
    // バイナリが無ければ `env --server` は失敗する（unit は起動に失敗して気づける）。
    std::fs::remove_file(&bin).unwrap();
    assert!(render_server_env(&settings).is_err());
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
