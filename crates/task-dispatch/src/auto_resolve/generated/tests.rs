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
        generated: rules(cmd),
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
    let mut rule = schema_rule(generator(bin.path(), REGEN));
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
    let mut rule = schema_rule(generator(bin.path(), REGEN));
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
    let rule = schema_rule(failing);
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
    let rule = schema_rule(generator(bin.path(), "echo x >> src/lib.rs\n"));
    let Resolution::NeedsHuman { request } = regenerate_if_drifted(repo, &ctx, &rule).unwrap()
    else {
        panic!("out-of-scope write must need human");
    };
    assert!(request.reason.contains("src/lib.rs"));
    assert_eq!(command(repo, &["rev-parse", "HEAD"]), head);

    // timeout は sleep の長さに依らず、上限 0 秒で決定的に起こす。
    command(repo, &["checkout", "-q", "--", "src/lib.rs"]);
    let mut rule = schema_rule(vec!["sleep".into(), "30".into()]);
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
    assert!(request.reason.contains("規則が無い"));

    let rule = GeneratedRule {
        globs: vec!["docs/api/**/*.json".into()],
        cmd: vec!["true".into()],
        env: Default::default(),
        timeout_secs: None,
        cargo_workspace: false,
    };
    assert!(rule.matches("docs/api/v1/task.schema.json"));
    assert!(!rule.matches("docs/protocol/task.schema.json"));
    let default = schema_rule(vec!["true".into()]);
    assert!(default.matches("docs/protocol/task.schema.json"));
    assert!(!default.matches("docs/protocol/nested/task.schema.json"));
    assert!(!default.matches("docs/protocol/task.json"));
}

/// delivery の設定から組まれる schema の規則。
fn schema_rule(cmd: Vec<String>) -> GeneratedRule {
    rules(Some(cmd)).remove(1)
}

fn cargo(dir: &Path, args: &[&str]) {
    let out = Command::new("cargo")
        .args(args)
        .current_dir(dir)
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn member(repo: &Path, dir: &str, deps: &str) {
    write(
        repo,
        &format!("{dir}/Cargo.toml"),
        &format!(
            "[package]\nname = \"{dir}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{deps}"
        ),
    );
    write(repo, &format!("{dir}/src/lib.rs"), "");
}

/// path 依存だけの Cargo workspace を `sub`（"" なら repo 直下）に置き、target は app1 に dep_a、
/// source は app2（`toml_conflict` なら app1）に dep_b を足して Cargo.lock を衝突させる。
fn cargo_conflict(repo: &Path, sub: &str, toml_conflict: bool) -> (String, String) {
    let at = |p: &str| {
        if sub.is_empty() {
            p.to_string()
        } else {
            format!("{sub}/{p}")
        }
    };
    write(
        repo,
        &at("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"app1\", \"app2\", \"dep_a\", \"dep_b\"]\n",
    );
    for (dir, deps) in [("app1", ""), ("app2", ""), ("dep_a", ""), ("dep_b", "")] {
        member(&repo.join(sub), dir, deps);
    }
    cargo(&repo.join(sub), &["metadata", "--format-version", "1"]);
    commit(repo, "base");
    command(repo, &["branch", "source"]);
    member(&repo.join(sub), "app1", "dep_a = { path = \"../dep_a\" }\n");
    cargo(&repo.join(sub), &["metadata", "--format-version", "1"]);
    let target_sha = commit(repo, "target adds dep_a");
    command(repo, &["checkout", "-q", "source"]);
    let app = if toml_conflict { "app1" } else { "app2" };
    member(&repo.join(sub), app, "dep_b = { path = \"../dep_b\" }\n");
    cargo(&repo.join(sub), &["metadata", "--format-version", "1"]);
    let source_sha = commit(repo, "source adds dep_b");
    command(repo, &["checkout", "-q", "target"]);
    assert!(git(repo, &["merge", "--no-ff", "--no-edit", "source"]).is_err());
    (target_sha, source_sha)
}

fn lock_ctx(target_sha: String, source_sha: String) -> ResolveContext {
    ResolveContext {
        generated: rules(None),
        ..ctx(target_sha, source_sha, None)
    }
}

#[test]
fn cargo_lock_only_conflict_regenerates_with_both_deps() {
    for sub in ["", "nested/ws"] {
        let tmp = new_repo();
        let repo = tmp.path();
        let (t, s) = cargo_conflict(repo, sub, false);
        let lock = if sub.is_empty() {
            "Cargo.lock".to_string()
        } else {
            format!("{sub}/Cargo.lock")
        };
        assert_eq!(
            command(repo, &["diff", "--name-only", "--diff-filter=U"]),
            lock
        );
        let Resolution::Resolved { actions } = resolve(repo, &lock_ctx(t, s)).unwrap() else {
            panic!("Cargo.lock conflict must resolve");
        };
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].path, lock);
        assert_eq!(actions[0].kind, ConflictKind::Generated);
        command(repo, &["commit", "-q", "--no-edit"]);
        let body = command(repo, &["show", &format!("HEAD:{lock}")]);
        assert!(body.contains("\"dep_a\""), "{body}");
        assert!(body.contains("\"dep_b\""), "{body}");
        assert!(
            body.contains("name = \"app1\"\nversion = \"0.1.0\"\ndependencies = [\n \"dep_a\",")
        );
        assert!(
            body.contains("name = \"app2\"\nversion = \"0.1.0\"\ndependencies = [\n \"dep_b\",")
        );
        cargo(
            &repo.join(sub),
            &["metadata", "--format-version", "1", "--locked"],
        );
    }
}

#[test]
fn cargo_lock_with_cargo_toml_conflict_needs_human() {
    let tmp = new_repo();
    let repo = tmp.path();
    let (t, s) = cargo_conflict(repo, "", true);
    let Resolution::NeedsHuman { request } = resolve(repo, &lock_ctx(t, s)).unwrap() else {
        panic!("Cargo.toml conflict must go to a human");
    };
    assert!(
        request
            .conflict_files
            .iter()
            .any(|f| f == "app1/Cargo.toml")
    );
}

#[test]
fn cargo_lock_rule_needs_cargo_toml_beside_lock() {
    let tmp = new_repo();
    let repo = tmp.path();
    write(repo, "vendor/Cargo.lock", "base\n");
    commit(repo, "base");
    command(repo, &["branch", "source"]);
    write(repo, "vendor/Cargo.lock", "target\n");
    let t = commit(repo, "t");
    command(repo, &["checkout", "-q", "source"]);
    write(repo, "vendor/Cargo.lock", "source\n");
    let s = commit(repo, "s");
    command(repo, &["checkout", "-q", "target"]);
    assert!(git(repo, &["merge", "--no-ff", "--no-edit", "source"]).is_err());
    let Resolution::NeedsHuman { request } = resolve(repo, &lock_ctx(t, s)).unwrap() else {
        panic!("lock without Cargo.toml must go to a human");
    };
    assert!(request.reason.contains("Cargo.toml"), "{}", request.reason);
}

#[test]
fn cargo_lock_regeneration_touching_other_files_needs_human() {
    let tmp = new_repo();
    let repo = tmp.path();
    let bin = tempfile::tempdir().unwrap();
    let (t, s) = cargo_conflict(repo, "", false);
    let mut rule = cargo_lock_rule();
    rule.cmd = generator(bin.path(), "echo '// x' >> app1/src/lib.rs\n");
    let ctx = ResolveContext {
        generated: vec![rule],
        ..ctx(t, s, None)
    };
    let Resolution::NeedsHuman { request } = resolve(repo, &ctx).unwrap() else {
        panic!("regeneration outside Cargo.lock must go to a human");
    };
    assert!(
        request.reason.contains("app1/src/lib.rs"),
        "{}",
        request.reason
    );
}

#[test]
fn cargo_lock_regeneration_failure_and_timeout_need_human() {
    for (cmd, timeout, want) in [
        (vec!["false".to_string()], None, "失敗"),
        (
            vec!["sleep".to_string(), "30".to_string()],
            Some(1),
            "終わらない",
        ),
    ] {
        let tmp = new_repo();
        let repo = tmp.path();
        let (t, s) = cargo_conflict(repo, "", false);
        let mut rule = cargo_lock_rule();
        rule.cmd = cmd;
        rule.timeout_secs = timeout;
        let ctx = ResolveContext {
            generated: vec![rule],
            ..ctx(t, s, None)
        };
        let Resolution::NeedsHuman { request } = resolve(repo, &ctx).unwrap() else {
            panic!("failed regeneration must go to a human");
        };
        assert!(request.reason.contains(want), "{}", request.reason);
    }
}
