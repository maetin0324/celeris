use super::*;
use task_core::workspace_config::ContainerSection;

fn plan(runtime: Runtime) -> ContainerPlan {
    ContainerPlan {
        runtime,
        program: runtime.as_str().to_string(),
        image: "celeris-worker:latest".to_string(),
        task_dir: PathBuf::from("/home/u/.local/celeris/workspaces/01TASK"),
        dir_repos: vec![PathBuf::from("/data/benchfs-runs")],
        creds: vec![],
        extra_mounts: vec![],
        knowledge_root: None,
        env: vec![],
        task_id: "01TASK".to_string(),
        uid: 1001,
        gid: 1001,
    }
}

fn joined(args: &[String]) -> String {
    args.join(" ")
}

/// ADR-0043 D3: podman は `--userns=keep-id`、docker は `--user <uid>:<gid>`。
/// タスクのディレクトリと `dir` リポジトリの実体は**同じパス**、cwd は `-w`。
#[test]
fn podman_and_docker_differ_only_in_how_they_keep_the_uid() {
    let cwd = PathBuf::from("/home/u/.local/celeris/workspaces/01TASK/repos/benchfs");
    let args = vec!["-c".to_string(), "echo hi".to_string()];
    let podman = argv(&plan(Runtime::Podman), "sh", &args, &[], &cwd);
    let docker = argv(&plan(Runtime::Docker), "sh", &args, &[], &cwd);

    for got in [&podman, &docker] {
        let line = joined(got);
        assert!(line.starts_with("run --rm -i --network host"), "{line}");
        assert!(
            line.contains("-w /home/u/.local/celeris/workspaces/01TASK/repos/benchfs"),
            "{line}"
        );
        assert!(
            line.contains(
                "-v /home/u/.local/celeris/workspaces/01TASK:/home/u/.local/celeris/workspaces/01TASK"
            ),
            "{line}"
        );
        assert!(
            line.contains("-v /data/benchfs-runs:/data/benchfs-runs"),
            "{line}"
        );
        assert!(line.contains("--label celeris.task=01TASK"), "{line}");
        assert!(
            line.ends_with("celeris-worker:latest sh -c echo hi"),
            "{line}"
        );
        // cwd は task_dir の下なので、重ねてマウントしない。
        assert_eq!(got.iter().filter(|a| *a == "-v").count(), 2, "{line}");
    }
    assert!(
        joined(&podman).contains("--userns=keep-id"),
        "{}",
        joined(&podman)
    );
    assert!(!joined(&podman).contains("--user 1001:1001"));
    assert!(
        joined(&docker).contains("--user 1001:1001"),
        "{}",
        joined(&docker)
    );
    assert!(!joined(&docker).contains("keep-id"));
}

/// ADR-0043 D3: 認証情報は**アダプタが env に書いた場所**だけ、読み取り専用で。
/// `OPENCODE_CONFIG` はファイルなので親ディレクトリ。`~/.local/celeris` を丸ごと見せない。
#[test]
fn credentials_from_the_adapter_env_are_mounted_read_only() {
    let cwd = PathBuf::from("/home/u/.local/celeris/workspaces/01TASK/repos/benchfs");
    let env = vec![
        (
            "CLAUDE_CONFIG_DIR".to_string(),
            "/home/u/.local/celeris/accounts/claude/a1".to_string(),
        ),
        (
            "CODEX_HOME".to_string(),
            "/home/u/.local/celeris/accounts/codex/c1".to_string(),
        ),
        (
            "OPENCODE_CONFIG".to_string(),
            "/home/u/qwen/opencode.json".to_string(),
        ),
        ("ANTHROPIC_MODEL".to_string(), "sonnet".to_string()),
    ];
    let got = argv(&plan(Runtime::Podman), "claude", &[], &env, &cwd);
    let line = joined(&got);
    assert!(line.contains("-v /home/u/.local/celeris/accounts/claude/a1:/home/u/.local/celeris/accounts/claude/a1:ro"), "{line}");
    assert!(line.contains("-v /home/u/.local/celeris/accounts/codex/c1:/home/u/.local/celeris/accounts/codex/c1:ro"), "{line}");
    assert!(line.contains("-v /home/u/qwen:/home/u/qwen:ro"), "{line}");
    assert!(
        !line.contains("-v /home/u/.local/celeris:/"),
        "celeris の根を丸ごと見せない: {line}"
    );
    assert!(
        !line.contains("-v /home/u:/home/u"),
        "ホームを丸ごと見せない: {line}"
    );
    // env はそのまま渡る（値も含めて）。
    assert!(line.contains("--env ANTHROPIC_MODEL=sonnet"), "{line}");
    assert!(
        line.contains("--env CLAUDE_CONFIG_DIR=/home/u/.local/celeris/accounts/claude/a1"),
        "{line}"
    );
    // HOME はタスクのディレクトリ（ホストのホームは見せない）。
    assert!(
        line.contains("--env HOME=/home/u/.local/celeris/workspaces/01TASK"),
        "{line}"
    );
}

/// ADR-0047 D3（Phase 61）: 知識ベースは**同じパスに読み取り専用**、`_inbox` だけ書き込み可。
/// 設定していなければ 1 バイトも変わらない。
#[test]
fn the_knowledge_base_is_mounted_read_only_with_a_writable_inbox() {
    let cwd = PathBuf::from("/home/u/.local/celeris/workspaces/01TASK/repos/benchfs");
    let without = joined(&argv(&plan(Runtime::Podman), "sh", &[], &[], &cwd));
    assert!(!without.contains("knowledge"), "{without}");

    let mut p = plan(Runtime::Podman);
    p.knowledge_root = Some(PathBuf::from("/home/u/knowledge"));
    let line = joined(&argv(&p, "sh", &[], &[], &cwd));
    assert!(
        line.contains("-v /home/u/knowledge:/home/u/knowledge:ro"),
        "{line}"
    );
    assert!(
        line.contains("-v /home/u/knowledge/_inbox:/home/u/knowledge/_inbox"),
        "{line}"
    );
    // `_inbox` の方は `:ro` が付かない（候補を書けないと `record` が使えない）。
    assert!(!line.contains("/home/u/knowledge/_inbox:ro"), "{line}");
    // 相対パスは無視する（ホストのどこを指すか分からないものはマウントしない）。
    let mut relative = plan(Runtime::Podman);
    relative.knowledge_root = Some(PathBuf::from("knowledge"));
    assert!(!joined(&argv(&relative, "sh", &[], &[], &cwd)).contains("knowledge"));
}

/// `[container] env` はアダプタの環境より**後**（同じキーなら勝つ）。`mounts` はそのまま渡る。
#[test]
fn workspace_toml_env_comes_last_and_extra_mounts_are_passed_through() {
    let cwd = PathBuf::from("/home/u/.local/celeris/workspaces/01TASK/repos/benchfs");
    let mut p = plan(Runtime::Podman);
    p.extra_mounts = vec!["/dev/infiniband:/dev/infiniband".to_string()];
    p.env = vec![(
        "CARGO_TARGET_DIR".to_string(),
        "/w/.cargo-target".to_string(),
    )];
    let env = vec![("CARGO_TARGET_DIR".to_string(), "/host".to_string())];
    let got = argv(&p, "sh", &[], &env, &cwd);
    let line = joined(&got);
    assert!(
        line.contains("-v /dev/infiniband:/dev/infiniband"),
        "{line}"
    );
    let host_at = line.find("--env CARGO_TARGET_DIR=/host").expect(&line);
    let toml_at = line
        .find("--env CARGO_TARGET_DIR=/w/.cargo-target")
        .expect(&line);
    assert!(host_at < toml_at, "workspace.toml が後（後勝ち）: {line}");
}

/// `mode = shared` のタスク（cwd が task_dir の外）では cwd も同じパスでマウントする。
#[test]
fn a_cwd_outside_the_task_directory_is_mounted_too() {
    let cwd = PathBuf::from("/home/u/workspace/agent-platform");
    let got = argv(&plan(Runtime::Docker), "sh", &[], &[], &cwd);
    let line = joined(&got);
    assert!(
        line.contains("-v /home/u/workspace/agent-platform:/home/u/workspace/agent-platform"),
        "{line}"
    );
}

/// ADR-0043 D3: 1 つでも `container` を要求したら、そのタスクはコンテナ。
/// イメージは **primary → repos[0] の順で最初に要求したリポジトリ**のもの。
#[test]
fn the_decision_follows_the_repos_and_the_workspace_toml() {
    let host = RepoRunInput {
        name: "data".into(),
        run: RepoRun::Host,
        is_primary: false,
        config: WorkspaceConfig::default(),
        config_dir: PathBuf::from("/data"),
    };
    // `auto` + `workspace.toml` の `[run] mode = "container"`
    let mut auto_cfg = WorkspaceConfig::default();
    auto_cfg.run.mode = RunMode::Container;
    auto_cfg.container = ContainerSection {
        image: Some("ghcr.io/x/rust-dev:1.90".into()),
        ..ContainerSection::default()
    };
    let auto = RepoRunInput {
        name: "benchfs".into(),
        run: RepoRun::Auto,
        is_primary: false,
        config: auto_cfg,
        config_dir: PathBuf::from("/src/benchfs"),
    };
    // 明示の `run = container`（`workspace.toml` は何も言っていない）→ 既定のイメージ
    let explicit = RepoRunInput {
        name: "paper".into(),
        run: RepoRun::Container,
        is_primary: true,
        config: WorkspaceConfig::default(),
        config_dir: PathBuf::from("/src/paper"),
    };

    // ホストだけ → None
    assert_eq!(decide(std::slice::from_ref(&host), "claude-code"), None);
    // auto + toml → そのイメージ
    let got = decide(&[host.clone(), auto.clone()], "claude-code").expect("container");
    assert_eq!(got.repo, "benchfs");
    assert_eq!(
        got.image,
        ImageSource::Named("ghcr.io/x/rust-dev:1.90".into())
    );
    // 混在: primary が先（`repos[0]` の順より primary が勝つ）
    let got = decide(&[auto.clone(), explicit.clone()], "claude-code").expect("container");
    assert_eq!(got.repo, "paper");
    assert_eq!(got.image, ImageSource::Default);
    // `auto` で `mode = host`（既定）のリポジトリはコンテナを要求しない
    let plain_auto = RepoRunInput {
        run: RepoRun::Auto,
        ..host.clone()
    };
    assert_eq!(decide(&[plain_auto], "claude-code"), None);
    // paperqa / local-deep-research は常にホスト（道具立てがホストの venv にある）
    for adapter in HOST_ONLY_ADAPTERS {
        assert_eq!(
            decide(&[auto.clone(), explicit.clone()], adapter),
            None,
            "{adapter}"
        );
    }
}

/// `dockerfile` を書いたリポジトリは `ImageSource::Dockerfile`。
#[test]
fn a_dockerfile_in_the_workspace_toml_is_built() {
    let mut cfg = WorkspaceConfig::default();
    cfg.run.mode = RunMode::Container;
    cfg.container.dockerfile = Some(DEFAULT_DOCKERFILE.to_string());
    let repo = RepoRunInput {
        name: "benchfs".into(),
        run: RepoRun::Auto,
        is_primary: true,
        config: cfg,
        config_dir: PathBuf::from("/src/benchfs"),
    };
    let got = decide(&[repo], "claude-code").expect("container");
    assert_eq!(
        got.image,
        ImageSource::Dockerfile {
            repo_dir: PathBuf::from("/src/benchfs"),
            dockerfile: DEFAULT_DOCKERFILE.to_string(),
        }
    );
}

/// タグは Dockerfile の内容と `.config/celeris/` の中身で決まる（どちらが変わっても変わる）。
#[test]
fn the_tag_changes_when_the_dockerfile_or_the_context_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join(".config/celeris");
    std::fs::create_dir_all(&config).expect("mkdir");
    std::fs::write(config.join("Dockerfile"), b"FROM debian:stable-slim\n").expect("write");

    let source = ImageSource::Dockerfile {
        repo_dir: dir.path().to_path_buf(),
        dockerfile: DEFAULT_DOCKERFILE.to_string(),
    };
    let build_root = dir.path().join("containers");
    let first = resolve_image(&source, DEFAULT_IMAGE, &build_root).expect("resolve");
    let tag_a = first.tag().to_string();
    assert!(tag_a.starts_with(BUILT_IMAGE_PREFIX), "{tag_a}");
    assert_eq!(tag_a.len(), BUILT_IMAGE_PREFIX.len() + 12, "{tag_a}");
    // 同じ内容なら同じタグ。
    assert_eq!(
        resolve_image(&source, DEFAULT_IMAGE, &build_root)
            .expect("resolve")
            .tag(),
        tag_a
    );
    // 文脈（`.config/celeris/` の別のファイル）が変わればタグも変わる。
    std::fs::write(
        config.join("workspace.toml"),
        b"[run]\nmode = \"container\"\n",
    )
    .expect("write");
    let tag_b = resolve_image(&source, DEFAULT_IMAGE, &build_root)
        .expect("resolve")
        .tag()
        .to_string();
    assert_ne!(tag_a, tag_b);
    // Dockerfile が変わってもタグは変わる。
    std::fs::write(
        config.join("Dockerfile"),
        b"FROM debian:stable-slim\nRUN true\n",
    )
    .expect("write");
    let tag_c = resolve_image(&source, DEFAULT_IMAGE, &build_root)
        .expect("resolve")
        .tag()
        .to_string();
    assert_ne!(tag_b, tag_c);

    // `image` / 既定はビルドしない。
    assert_eq!(
        resolve_image(
            &ImageSource::Named("x:1".into()),
            DEFAULT_IMAGE,
            &build_root
        )
        .expect("resolve"),
        ResolvedImage::Ready("x:1".into())
    );
    assert_eq!(
        resolve_image(&ImageSource::Default, DEFAULT_IMAGE, &build_root).expect("resolve"),
        ResolvedImage::Ready(DEFAULT_IMAGE.into())
    );
}

/// キャッシュに当たったらビルドを起こさない（`image_exists` を差し替えて見る。ネットワークに出ない）。
#[tokio::test]
async fn a_cache_hit_skips_the_build() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log = dir.path().join("runs/container-build.log");
    let request = BuildRequest {
        tag: "celeris-ws-0123456789ab".into(),
        dockerfile: dir.path().join(".config/celeris/Dockerfile"),
        context_src: dir.path().join(".config/celeris"),
        build_dir: dir.path().join("containers/celeris-ws-0123456789ab"),
    };
    // 在る → 何もしない（存在しない runtime の名前を渡しても落ちない = 起こしていない証拠）。
    ensure_image(
        "no-such-runtime",
        &request,
        Duration::from_secs(5),
        &log,
        &|_| true,
    )
    .await
    .expect("cache hit");
    assert!(!log.exists(), "ビルドしていないので記録も無い");
    assert!(!request.build_dir.exists());
}

/// 無ければビルドする。偽の runtime（argv を記録して失敗する）で、引数と記録を見る。
#[tokio::test]
async fn a_cache_miss_builds_and_logs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join(".config/celeris");
    std::fs::create_dir_all(&config).expect("mkdir");
    std::fs::write(config.join("Dockerfile"), b"FROM scratch\n").expect("write");
    let fake = dir.path().join("fake-runtime");
    std::fs::write(
        &fake,
        "#!/bin/sh\necho \"argv: $*\"\necho 'boom' 1>&2\nexit 9\n",
    )
    .expect("write");
    set_executable(&fake);

    let log = dir.path().join("runs/container-build.log");
    let request = BuildRequest {
        tag: "celeris-ws-aaaaaaaaaaaa".into(),
        dockerfile: config.join("Dockerfile"),
        context_src: config.clone(),
        build_dir: dir.path().join("containers/celeris-ws-aaaaaaaaaaaa"),
    };
    // 並列のテストが fork している最中に書いたばかりのスクリプトを exec すると ETXTBSY（Text file busy）に
    // なることがある（他スレッドの子が書き込み fd を一瞬継いでいる）。負荷が高いときだけ出るので、その場合だけやり直す。
    let mut err = String::new();
    for _ in 0..20 {
        err = ensure_image(
            &fake.display().to_string(),
            &request,
            Duration::from_secs(30),
            &log,
            &|_| false,
        )
        .await
        .expect_err("build fails");
        if !err.contains("Text file busy") && !err.contains("os error 26") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(err.contains("celeris-ws-aaaaaaaaaaaa"), "{err}");
    assert!(err.contains("boom"), "{err}");
    let text = std::fs::read_to_string(&log).expect("log");
    assert!(
        text.contains("argv: build --network host -t celeris-ws-aaaaaaaaaaaa -f"),
        "{text}"
    );
    assert!(text.contains("[stderr] boom"), "{text}");
    // 文脈は `.config/celeris/` の写しだけ（リポジトリ全体は送らない）。
    assert!(request.build_dir.join("context/Dockerfile").is_file());
}

/// 検出は podman → docker の順。両方駄目なら `None` と理由が残る（ADR-0043 D3）。
#[test]
fn detection_prefers_podman_and_records_why_each_one_failed() {
    let ok = detect_with(RuntimePreference::Auto, |_| Ok(()));
    assert_eq!(ok.runtime, Some(Runtime::Podman));
    assert_eq!(ok.tried, vec![("podman".to_string(), "ok".to_string())]);

    let fallback = detect_with(RuntimePreference::Auto, |rt| match rt {
        Runtime::Podman => Err("newuidmap: Operation not permitted".into()),
        Runtime::Docker => Ok(()),
    });
    assert_eq!(fallback.runtime, Some(Runtime::Docker));
    assert_eq!(fallback.tried.len(), 2);
    assert!(fallback.tried[0].1.contains("newuidmap"));

    let none = detect_with(RuntimePreference::Auto, |rt| {
        Err(format!("{} が見つからない", rt.as_str()))
    });
    assert!(!none.is_available());
    assert!(none.summary().contains("podman"), "{}", none.summary());
    assert!(none.summary().contains("docker"), "{}", none.summary());

    // 明示した runtime は 1 つだけ試す。
    let only = detect_with(RuntimePreference::Docker, |rt| {
        assert_eq!(rt, Runtime::Docker);
        Ok(())
    });
    assert_eq!(only.runtime, Some(Runtime::Docker));
}

/// 実物の `probe_program`（`<program> info`）: 成功・`info` が失敗・実行ファイルが無い。
/// PATH は触らず、偽物を絶対パスで指す（他の試験と競合しない）。
#[test]
fn probing_a_runtime_sees_success_failure_and_absence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let good = dir.path().join("podman");
    crate::test_support::write_executable(
        &good,
        "#!/bin/sh\n[ \"$1\" = info ] || exit 2\necho host: ok\n",
    );
    assert_eq!(
        probe_program(&good.display().to_string(), Duration::from_secs(10)),
        Ok(())
    );

    let bad = dir.path().join("docker");
    crate::test_support::write_executable(
        &bad,
        "#!/bin/sh\necho 'Cannot connect to the Docker daemon' 1>&2\nexit 1\n",
    );
    let err =
        probe_program(&bad.display().to_string(), Duration::from_secs(10)).expect_err("info fails");
    assert!(err.contains("Cannot connect"), "{err}");

    let missing = dir.path().join("nope");
    let err = probe_program(&missing.display().to_string(), Duration::from_secs(10))
        .expect_err("missing");
    assert!(err.contains("見つからない"), "{err}");
}

/// `wrap(cmd, None)` はホスト実行のまま（1 バイトも変えない）。`Some` なら runtime に置き換わる。
#[tokio::test]
async fn wrap_is_a_no_op_without_a_plan_and_rewrites_the_command_with_one() {
    let mut host = tokio::process::Command::new("claude");
    host.arg("-p")
        .arg("hi")
        .env("CODEX_HOME", "/creds/c1")
        .current_dir("/w/01TASK/repos/x");
    let kept = wrap(host, None);
    assert_eq!(kept.as_std().get_program().to_string_lossy(), "claude");

    let mut inner = tokio::process::Command::new("claude");
    inner
        .arg("-p")
        .arg("hi")
        .env("CODEX_HOME", "/creds/c1")
        .current_dir("/w/01TASK/repos/x");
    let mut p = plan(Runtime::Podman);
    p.task_dir = PathBuf::from("/w/01TASK");
    p.dir_repos = vec![];
    let wrapped = wrap(inner, Some(&p));
    let std_cmd = wrapped.as_std();
    assert_eq!(std_cmd.get_program().to_string_lossy(), "podman");
    let args: Vec<String> = std_cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let line = joined(&args);
    assert!(
        line.starts_with("run --rm -i --network host --userns=keep-id"),
        "{line}"
    );
    assert!(line.contains("-w /w/01TASK/repos/x"), "{line}");
    assert!(line.contains("-v /w/01TASK:/w/01TASK"), "{line}");
    assert!(line.contains("-v /creds/c1:/creds/c1:ro"), "{line}");
    assert!(line.contains("--env CODEX_HOME=/creds/c1"), "{line}");
    assert!(
        line.ends_with("celeris-worker:latest claude -p hi"),
        "{line}"
    );
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod");
}
