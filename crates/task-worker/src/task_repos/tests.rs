use super::*;
use crate::local_worktree::{DEFAULT_BRANCH_PREFIX, resolve_base};

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("mkdir");
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("README.md"), b"hello\n").expect("write");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "first"]);
}

fn workspaces(task_dir: &Path, gits: &[(&str, &Path)], dirs: &[(&str, &Path)]) -> TaskWorkspaces {
    let id = "01TASK";
    let mut repos = Vec::new();
    for (name, repo) in gits {
        let dir = task_dir.join(REPOS_DIR_NAME).join(name);
        repos.push(TaskRepo::git(
            *name,
            LocalWorktree {
                repo: repo.to_path_buf(),
                task_dir: task_dir.to_path_buf(),
                dir,
                branch: format!("{DEFAULT_BRANCH_PREFIX}{id}"),
                base: resolve_base(repo, None).expect("base"),
            },
        ));
    }
    for (name, target) in dirs {
        repos.push(TaskRepo::link(
            *name,
            *target,
            task_dir.join(REPOS_DIR_NAME).join(name),
        ));
    }
    TaskWorkspaces {
        task_dir: task_dir.to_path_buf(),
        repos,
    }
}

/// ADR-0043 D2: git 2 つ + `dir` 1 つ → worktree 2 つ + シンボリックリンク 1 つ。cwd は先頭。
#[tokio::test]
async fn two_git_repos_and_one_directory_become_two_worktrees_and_a_symlink() {
    let root = tempfile::tempdir().expect("tempdir");
    let code = root.path().join("benchfs");
    let paper = root.path().join("benchfs-paper");
    init_repo(&code);
    init_repo(&paper);
    let data = root.path().join("data");
    std::fs::create_dir_all(data.join("runs")).expect("mkdir");
    std::fs::write(data.join("runs/one.csv"), b"1\n").expect("write");

    let task_dir = root.path().join("ws").join("01TASK");
    let ws = workspaces(
        &task_dir,
        &[("benchfs", &code), ("benchfs-paper", &paper)],
        &[("data", &data)],
    );
    ws.ensure().await.expect("ensure");

    assert_eq!(
        ws.cwd(),
        Some(task_dir.join("repos/benchfs").as_path()),
        "cwd は先頭のリポジトリ"
    );
    assert!(task_dir.join("repos/benchfs/README.md").is_file());
    assert!(task_dir.join("repos/benchfs-paper/README.md").is_file());
    // `dir` はシンボリックリンク（コピーしない）。
    let link = task_dir.join("repos/data");
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("meta")
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read_link(&link).expect("readlink"), data);
    assert!(link.join("runs/one.csv").is_file(), "リンク越しに読める");

    // 冪等: もう一度 ensure しても作業は消えない。
    std::fs::write(task_dir.join("repos/benchfs/wip.txt"), b"x").expect("write");
    ws.ensure().await.expect("again");
    assert!(task_dir.join("repos/benchfs/wip.txt").is_file());
}

/// ADR-0043 D2: 中止したタスクは worktree とブランチを消し、リンクを外す（実体は残る）。
#[tokio::test]
async fn cancel_removes_every_worktree_its_branch_and_the_symlink() {
    let root = tempfile::tempdir().expect("tempdir");
    let code = root.path().join("benchfs");
    init_repo(&code);
    let data = root.path().join("data");
    std::fs::create_dir_all(&data).expect("mkdir");
    std::fs::write(data.join("keep.txt"), b"keep").expect("write");

    let task_dir = root.path().join("ws").join("01TASK");
    let ws = workspaces(&task_dir, &[("benchfs", &code)], &[("data", &data)]);
    ws.ensure().await.expect("ensure");
    // 未コミットの変更があっても cancel は消す（人の指示なので）。
    std::fs::write(task_dir.join("repos/benchfs/dirty.txt"), b"x").expect("write");

    let outcomes = ws.remove_for_cancel();
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes.iter().all(|(_, o)| *o == CleanupOutcome::Removed),
        "{outcomes:?}"
    );
    assert!(!task_dir.join("repos/benchfs").exists());
    assert!(!task_dir.join("repos/data").exists());
    assert!(data.join("keep.txt").is_file(), "リンク先の実体は消さない");
    let branches = std::process::Command::new("git")
        .arg("-C")
        .arg(&code)
        .args(["branch", "--list"])
        .output()
        .expect("git");
    assert!(
        !String::from_utf8_lossy(&branches.stdout).contains(DEFAULT_BRANCH_PREFIX),
        "ブランチも消える: {}",
        String::from_utf8_lossy(&branches.stdout)
    );
    // 2 回目は「もう無い」。
    assert!(
        ws.remove_for_cancel()
            .iter()
            .all(|(_, o)| *o == CleanupOutcome::AlreadyGone)
    );
}

/// ADR-0043 D3 / D4: `[commands] setup` は worktree の中で走り、`runs/setup.log` に残る。
#[tokio::test]
async fn setup_commands_run_in_the_repo_and_are_logged() {
    let root = tempfile::tempdir().expect("tempdir");
    let code = root.path().join("benchfs");
    init_repo(&code);
    std::fs::create_dir_all(code.join(".config/celeris")).expect("mkdir");
    std::fs::write(
        code.join(".config/celeris/workspace.toml"),
        b"[commands]\nsetup = [\"pwd > setup-ran.txt\"]\n",
    )
    .expect("write");
    git(&code, &["add", "-A"]);
    git(&code, &["commit", "-q", "-m", "workspace.toml"]);

    let task_dir = root.path().join("ws").join("01TASK");
    let ws = workspaces(&task_dir, &[("benchfs", &code)], &[]);
    ws.ensure().await.expect("ensure");
    let outcome = run_setup(&ws.repos, &task_dir, std::time::Duration::from_secs(30))
        .await
        .expect("setup");
    assert_eq!(
        outcome,
        SetupOutcome {
            ok: true,
            ran: 1,
            failures: Vec::new()
        }
    );
    let ran = task_dir.join("repos/benchfs/setup-ran.txt");
    assert!(ran.is_file(), "setup は worktree の中で走る");
    let log = std::fs::read_to_string(task_dir.join(SETUP_LOG)).expect("log");
    assert!(log.contains("$ (benchfs) pwd > setup-ran.txt"), "{log}");
    assert!(log.contains("=> exit 0"), "{log}");
}

/// 落ちた `setup` は `ok = false` と理由を返す（呼び出し側が run を始めずに人へ聞く）。
#[tokio::test]
async fn a_failing_setup_reports_the_reason() {
    let root = tempfile::tempdir().expect("tempdir");
    let code = root.path().join("benchfs");
    init_repo(&code);
    std::fs::create_dir_all(code.join(".config/celeris")).expect("mkdir");
    std::fs::write(
        code.join(".config/celeris/workspace.toml"),
        b"[commands]\nsetup = [\"echo boom 1>&2; exit 7\"]\n",
    )
    .expect("write");
    git(&code, &["add", "-A"]);
    git(&code, &["commit", "-q", "-m", "workspace.toml"]);

    let task_dir = root.path().join("ws").join("01TASK");
    let ws = workspaces(&task_dir, &[("benchfs", &code)], &[]);
    ws.ensure().await.expect("ensure");
    let outcome = run_setup(&ws.repos, &task_dir, std::time::Duration::from_secs(30))
        .await
        .expect("setup");
    assert!(!outcome.ok);
    assert_eq!(outcome.ran, 1);
    assert_eq!(outcome.failures.len(), 1);
    assert!(
        outcome.failures[0].contains("exit Some(7)"),
        "{:?}",
        outcome.failures
    );
    let log = std::fs::read_to_string(task_dir.join(SETUP_LOG)).expect("log");
    assert!(log.contains("[stderr] boom"), "{log}");
}

/// ADR-0043 D3（Phase 56）: コンテナのタスクでは `setup` も**コンテナの中で**走る。
///
/// 偽の runtime（argv を記録して、`-w` の場所で本体のコマンドをそのまま実行する sh スクリプト）を
/// 使う。**ネットワークにも本物の podman / docker にも触らない**。
#[tokio::test]
async fn setup_runs_inside_the_container_when_the_task_is_containerized() {
    let root = tempfile::tempdir().expect("tempdir");
    let code = root.path().join("benchfs");
    init_repo(&code);
    std::fs::create_dir_all(code.join(".config/celeris")).expect("mkdir");
    std::fs::write(
        code.join(".config/celeris/workspace.toml"),
        b"[run]\nmode = \"container\"\n\n[commands]\nsetup = [\"pwd > setup-ran.txt\"]\n",
    )
    .expect("write");
    git(&code, &["add", "-A"]);
    git(&code, &["commit", "-q", "-m", "workspace.toml"]);

    // 偽の runtime: argv を NUL 区切りで記録し、`-w` のディレクトリでイメージ以降を実行する。
    let argv_log = root.path().join("argv.log");
    let fake = root.path().join("fake-runtime");
    crate::test_support::write_executable(
        &fake,
        &format!(
            r#"#!/bin/sh
for a in "$@"; do printf '%s\0' "$a" >> "{log}"; done
cwd=""
while [ $# -gt 0 ]; do
  case "$1" in
    -w) cwd="$2"; shift 2 ;;
    -v|--env|--label|--user) shift 2 ;;
    run|--rm|-i|--network|host|--userns=keep-id) shift ;;
    *) break ;;
  esac
done
shift
cd "$cwd" || exit 1
exec "$@"
"#,
            log = argv_log.display()
        ),
    );

    let task_dir = root.path().join("ws").join("01TASK");
    let ws = workspaces(&task_dir, &[("benchfs", &code)], &[]);
    ws.ensure().await.expect("ensure");

    let plan: crate::container::SharedPlan = std::sync::Arc::new(crate::container::ContainerPlan {
        runtime: crate::container::Runtime::Podman,
        program: fake.display().to_string(),
        image: "celeris-worker:latest".to_string(),
        task_dir: task_dir.clone(),
        dir_repos: vec![],
        creds: vec![],
        extra_mounts: vec![],
        knowledge_root: None,
        env: vec![],
        task_id: "01TASK".to_string(),
        uid: 1000,
        gid: 1000,
    });
    let outcome = run_setup_in(
        &ws.repos,
        &task_dir,
        std::time::Duration::from_secs(60),
        Some(&plan),
    )
    .await
    .expect("setup");
    assert_eq!(
        outcome,
        SetupOutcome {
            ok: true,
            ran: 1,
            failures: Vec::new()
        }
    );

    // コマンドは runtime 越しに渡っている（ホストで直接 `sh -c` していない）。
    let argv = std::fs::read(&argv_log).expect("argv.log");
    let argv: Vec<String> = String::from_utf8_lossy(&argv)
        .split('\0')
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect();
    let line = argv.join(" ");
    assert!(
        line.starts_with("run --rm -i --network host --userns=keep-id"),
        "{line}"
    );
    assert!(
        line.contains(&format!("-w {}", task_dir.join("repos/benchfs").display())),
        "{line}"
    );
    assert!(
        line.contains(&format!("-v {0}:{0}", task_dir.display())),
        "{line}"
    );
    assert!(line.contains("--label celeris.task=01TASK"), "{line}");
    assert!(
        line.contains("celeris-worker:latest sh -c pwd > setup-ran.txt"),
        "{line}"
    );

    // 中身も本当に走っている（worktree の中に結果が残る）。
    assert!(task_dir.join("repos/benchfs/setup-ran.txt").is_file());
    let log = std::fs::read_to_string(task_dir.join(SETUP_LOG)).expect("log");
    assert!(
        log.contains("# 実行環境: コンテナ celeris-worker:latest （podman）"),
        "{log}"
    );
    assert!(log.contains("=> exit 0"), "{log}");
}

/// 人が実体のディレクトリを置いていたら、リンクで上書きしない。
#[tokio::test]
async fn an_existing_real_directory_is_left_alone() {
    let root = tempfile::tempdir().expect("tempdir");
    let data = root.path().join("data");
    std::fs::create_dir_all(&data).expect("mkdir");
    let task_dir = root.path().join("ws").join("01TASK");
    let real = task_dir.join(REPOS_DIR_NAME).join("data");
    std::fs::create_dir_all(&real).expect("mkdir");
    std::fs::write(real.join("mine.txt"), b"mine").expect("write");

    let ws = workspaces(&task_dir, &[], &[("data", &data)]);
    ws.ensure().await.expect("ensure");
    assert!(real.join("mine.txt").is_file());
    assert!(
        !std::fs::symlink_metadata(&real)
            .expect("meta")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        ws.remove_for_cancel(),
        vec![("data".to_string(), CleanupOutcome::Dirty)]
    );
    assert!(real.join("mine.txt").is_file());
}
