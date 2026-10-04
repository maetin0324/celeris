use super::super::{Resolution, ResolveContext, git, resolve};
use super::*;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn command(repo: &Path, args: &[&str]) -> String {
    git(repo, args).unwrap().trim().to_string()
}

fn write(repo: &Path, path: &str, body: &str) {
    let path = repo.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn commit(repo: &Path, subject: &str) -> String {
    command(repo, &["add", "."]);
    command(repo, &["commit", "-q", "-m", subject]);
    command(repo, &["rev-parse", "HEAD"])
}

fn new_repo() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    command(repo, &["init", "-q", "-b", "target"]);
    command(repo, &["config", "user.email", "test@example.invalid"]);
    command(repo, &["config", "user.name", "Test"]);
    tmp
}

/// task-worker の `test_support::write_executable` と同じく、書き込みを別 process に任せて
/// この試験 process が書き込み fd を持たないようにする（ETXTBSY 回避、ADR-0010 D10）。
fn write_executable(path: &Path, contents: &str) {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(contents.as_bytes()).unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

/// repo の外に置いた偽の生成台本。repo の中に untracked file を作らない。
fn generator(dir: &Path, body: &str) -> Vec<String> {
    let path = dir.join("gen.sh");
    write_executable(&path, &format!("#!/bin/sh\nset -e\n{body}"));
    vec![path.to_string_lossy().into_owned()]
}

const REGEN: &str = "mkdir -p docs/protocol docs/api/v1\n\
printf '{\"v\":\"%s\"}\\n' \"$GEN_VALUE\" > docs/protocol/task.schema.json\n\
printf '{\"api\":\"%s\"}\\n' \"$GEN_VALUE\" > docs/api/v1/task.schema.json\n";

fn ctx(target_sha: String, source_sha: String, cmd: Option<Vec<String>>) -> ResolveContext {
    ResolveContext {
        target_branch: "target".into(),
        target_sha,
        source_branch: "source".into(),
        source_sha,
        merge_base: None,
        generated_command: cmd,
    }
}

/// target と source が同じ生成物を別々に変えた merge 中の repo を作る。
fn conflicted_merge(repo: &Path) -> (String, String) {
    write(repo, "docs/protocol/task.schema.json", "{\"v\":\"base\"}\n");
    write(repo, "docs/api/v1/task.schema.json", "{\"api\":\"base\"}\n");
    commit(repo, "base");
    command(repo, &["branch", "source"]);
    write(
        repo,
        "docs/protocol/task.schema.json",
        "{\"v\":\"target\"}\n",
    );
    let target_sha = commit(repo, "target regenerates");
    command(repo, &["checkout", "-q", "source"]);
    write(
        repo,
        "docs/protocol/task.schema.json",
        "{\"v\":\"source\"}\n",
    );
    let source_sha = commit(repo, "source regenerates");
    command(repo, &["checkout", "-q", "target"]);
    let merged = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["merge", "--no-commit", "-q", "source"])
        .output()
        .unwrap();
    assert!(!merged.status.success());
    (target_sha, source_sha)
}

#[test]
fn conflict_takes_target_then_regenerates_and_stages_globs() {
    let tmp = new_repo();
    let repo = tmp.path();
    let bin = tempfile::tempdir().unwrap();
    let (target_sha, source_sha) = conflicted_merge(repo);
    let ctx = ctx(target_sha, source_sha, Some(generator(bin.path(), REGEN)));

    // 既定の規則では env が空なので値は空文字列で再生成される。
    let Resolution::Resolved { actions } = resolve(repo, &ctx).unwrap() else {
        panic!("generated conflict must resolve");
    };
    let paths: Vec<&str> = actions.iter().map(|a| a.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "docs/api/v1/task.schema.json",
            "docs/protocol/task.schema.json"
        ]
    );
    assert!(actions.iter().all(|a| a.kind == ConflictKind::Generated));
    assert!(command(repo, &["diff", "--name-only", "--diff-filter=U"]).is_empty());
    command(repo, &["commit", "-q", "--no-edit"]);
    assert_eq!(
        command(repo, &["show", "HEAD:docs/protocol/task.schema.json"]),
        "{\"v\":\"\"}"
    );
    assert_eq!(
        command(repo, &["show", "HEAD:docs/api/v1/task.schema.json"]),
        "{\"api\":\"\"}"
    );
}

#[test]
fn conflict_uses_rule_env_and_records_action() {
    let tmp = new_repo();
    let repo = tmp.path();
    let bin = tempfile::tempdir().unwrap();
    conflicted_merge(repo);
    let mut rule = GeneratedRule::with_default_globs(generator(bin.path(), REGEN));
    rule.env.insert("GEN_VALUE".into(), "regen".into());
    let item = ClassifiedPath {
        path: "docs/protocol/task.schema.json".into(),
        kind: ConflictKind::Generated,
        duplicate_with: Vec::new(),
    };
    let ResolveAttempt::Handled { actions } = resolve_with_rule(repo, &rule, &item).unwrap() else {
        panic!("must handle");
    };
    assert!(
        actions
            .iter()
            .any(|a| a.path == item.path && a.detail.contains("target 側を採用"))
    );
    assert_eq!(
        fs::read_to_string(repo.join("docs/protocol/task.schema.json")).unwrap(),
        "{\"v\":\"regen\"}\n"
    );
}

#[test]
fn drift_is_committed_once_and_rerun_is_noop() {
    let tmp = new_repo();
    let repo = tmp.path();
    let bin = tempfile::tempdir().unwrap();
    write(
        repo,
        "docs/protocol/task.schema.json",
        "{\"v\":\"stale\"}\n",
    );
    write(repo, "src/lib.rs", "// code\n");
    let head = commit(repo, "base");
    let mut rule = GeneratedRule::with_default_globs(generator(bin.path(), REGEN));
    rule.env.insert("GEN_VALUE".into(), "fresh".into());
    let ctx = ctx(head.clone(), head.clone(), None);

    let Resolution::Resolved { actions } = regenerate_if_drifted(repo, &ctx, &rule).unwrap() else {
        panic!("drift must be committed");
    };
    assert_eq!(actions.len(), 2);
    assert_ne!(command(repo, &["rev-parse", "HEAD"]), head);
    assert!(command(repo, &["log", "-1", "--format=%s"]).starts_with("生成物を再生成"));
    assert_eq!(
        command(repo, &["show", "HEAD:docs/protocol/task.schema.json"]),
        "{\"v\":\"fresh\"}"
    );
    assert!(command(repo, &["status", "--porcelain"]).is_empty());

    let after = command(repo, &["rev-parse", "HEAD"]);
    let Resolution::Resolved { actions } = regenerate_if_drifted(repo, &ctx, &rule).unwrap() else {
        panic!("no drift must resolve");
    };
    assert!(actions.is_empty());
    assert_eq!(command(repo, &["rev-parse", "HEAD"]), after);
}

#[test]
fn command_failure_needs_human_in_merge_and_drift() {
    let tmp = new_repo();
    let repo = tmp.path();
    let bin = tempfile::tempdir().unwrap();
    let failing = generator(bin.path(), "echo 'schema drift detected' >&2\nexit 3\n");
    let (target_sha, source_sha) = conflicted_merge(repo);
    let merge_ctx = ctx(target_sha, source_sha, Some(failing.clone()));
    let Resolution::NeedsHuman { request } = resolve(repo, &merge_ctx).unwrap() else {
        panic!("command failure must need human");
    };
    assert!(
        request.reason.contains("schema drift detected"),
        "{}",
        request.reason
    );
    assert!(repo.join(".git/MERGE_HEAD").exists());

    let clean = new_repo();
    let clean_repo = clean.path();
    write(clean_repo, "docs/protocol/task.schema.json", "{}\n");
    let head = commit(clean_repo, "base");
    let rule = GeneratedRule::with_default_globs(failing);
    let Resolution::NeedsHuman { request } =
        regenerate_if_drifted(clean_repo, &ctx(head.clone(), head.clone(), None), &rule).unwrap()
    else {
        panic!("command failure must need human");
    };
    assert!(request.reason.contains("失敗"));
    assert_eq!(command(clean_repo, &["rev-parse", "HEAD"]), head);
}

#[test]
fn out_of_scope_write_and_timeout_need_human() {
    let tmp = new_repo();
    let repo = tmp.path();
    let bin = tempfile::tempdir().unwrap();
    write(repo, "docs/protocol/task.schema.json", "{}\n");
    write(repo, "src/lib.rs", "// code\n");
    let head = commit(repo, "base");
    let ctx = ctx(head.clone(), head.clone(), None);
    let rule = GeneratedRule::with_default_globs(generator(bin.path(), "echo x >> src/lib.rs\n"));
    let Resolution::NeedsHuman { request } = regenerate_if_drifted(repo, &ctx, &rule).unwrap()
    else {
        panic!("out-of-scope write must need human");
    };
    assert!(request.reason.contains("src/lib.rs"));
    assert_eq!(command(repo, &["rev-parse", "HEAD"]), head);

    // timeout は sleep の長さに依らず、上限 0 秒で決定的に起こす。
    command(repo, &["checkout", "-q", "--", "src/lib.rs"]);
    let mut rule = GeneratedRule::with_default_globs(vec!["sleep".into(), "30".into()]);
    rule.timeout_secs = Some(0);
    let Resolution::NeedsHuman { request } = regenerate_if_drifted(repo, &ctx, &rule).unwrap()
    else {
        panic!("timeout must need human");
    };
    assert!(request.reason.contains("終わらない"), "{}", request.reason);
}

#[test]
fn missing_command_or_out_of_glob_path_is_not_handled() {
    let tmp = new_repo();
    let repo = tmp.path();
    let (target_sha, source_sha) = conflicted_merge(repo);
    let Resolution::NeedsHuman { request } =
        resolve(repo, &ctx(target_sha, source_sha, None)).unwrap()
    else {
        panic!("missing command must need human");
    };
    assert!(request.reason.contains("設定されていない"));

    let rule = GeneratedRule {
        globs: vec!["docs/api/**/*.json".into()],
        cmd: vec!["true".into()],
        env: Default::default(),
        timeout_secs: None,
    };
    assert!(rule.matches("docs/api/v1/task.schema.json"));
    assert!(!rule.matches("docs/protocol/task.schema.json"));
    let default = GeneratedRule::with_default_globs(vec!["true".into()]);
    assert!(default.matches("docs/protocol/task.schema.json"));
    assert!(!default.matches("docs/protocol/nested/task.schema.json"));
    assert!(!default.matches("docs/protocol/task.json"));
}
