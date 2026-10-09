//! ADR 2026-10-09-cos-operations-all-mutations D3 / 2026-10-09-cos-operations-external-effects
//! (WU ops-projects-cron): `POST /tasks/{id}/changes/{repo}/integrate` and `…/pr/merge` change git
//! and GitHub, so through `/cos/operations` they are C-class: pending before the effect, applied
//! after it, and a resent request never runs git or `gh` again. GitHub is a fake `gh` script that
//! logs its calls; `origin` is a local bare repository (no network).
mod common;

use common::cos_ops::{OPS, audit_events, cos_bearer, op_body};
use common::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use task_core::{Status, TaskId, TaskKind};

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A fake `gh`: `auth status` succeeds, `pr create` prints a fixed URL, `pr view` reports the state
/// in `<dir>/state`, and `pr merge` appends to `<dir>/merged.log` and marks the PR merged.
fn fake_gh(dir: &Path) -> String {
    let path = dir.join("gh");
    let script = r#"#!/bin/sh
here="$(cd "$(dirname "$0")" && pwd)"
echo "$@" >> "$here/calls.log"
case "$1 $2" in
  "auth status") exit 0 ;;
  "pr create") echo "https://github.com/o/r/pull/42"; exit 0 ;;
  "pr view")
    state="$(cat "$here/state" 2>/dev/null || echo OPEN)"
    if [ "$state" = "MERGED" ]; then
      echo '{"state":"MERGED","mergedAt":"2026-10-09T10:00:00Z","mergeable":"MERGEABLE","reviewDecision":null,"url":"https://github.com/o/r/pull/42"}'
    else
      echo '{"state":"OPEN","mergedAt":null,"mergeable":"MERGEABLE","reviewDecision":null,"url":"https://github.com/o/r/pull/42"}'
    fi
    exit 0 ;;
  "pr merge") echo "$@" >> "$here/merged.log"; echo MERGED > "$here/state"; exit 0 ;;
esac
exit 1
"#;
    std::fs::write(&path, script).expect("write gh");
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path.to_string_lossy().into_owned()
}

fn env_with_gh(bin: &Path) -> TestEnv {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.into()),
        github: task_api::GithubSettings {
            gh: fake_gh(bin),
            merge_method: "merge".into(),
        },
        ..EnvOptions::default()
    });
    task_ops::changes::forget_gh_auth();
    env
}

/// A source repository (`main`, 1 commit) and the task's worktree (branch, 1 commit) with its marker.
fn seed_git_task(env: &TestEnv) -> (TaskId, PathBuf) {
    let task = new_task(TaskKind::Execute, Status::Done);
    env.seed(&task);
    let repo = env.dir.path().join(format!("src-{}", task.id));
    std::fs::create_dir_all(&repo).expect("mkdir");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("README.md"), "hello\n").expect("write");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    let task_dir = env.workspace(&task);
    let tree = task_dir.join("repos").join("code");
    let branch = format!("celeris/{}", task.id);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &tree.to_string_lossy(),
            "main",
        ],
    );
    git(&tree, &["config", "user.email", "t@example.com"]);
    git(&tree, &["config", "user.name", "t"]);
    std::fs::write(tree.join("src.txt"), "a\n").expect("write");
    git(&tree, &["add", "-A"]);
    git(&tree, &["commit", "-q", "-m", "work"]);
    let base = git(&repo, &["rev-parse", "refs/heads/main"]);
    let marker = task_ops::workspace::WorktreeMarker {
        repo: repo.to_string_lossy().into_owned(),
        dir: tree.to_string_lossy().into_owned(),
        branch: branch.clone(),
        base: base.clone(),
        base_kind: "main".into(),
        repos: vec![task_ops::workspace::WorktreeMarkerRepo {
            name: "code".into(),
            kind: "git".into(),
            source: repo.to_string_lossy().into_owned(),
            dir: tree.to_string_lossy().into_owned(),
            branch: Some(branch),
            base: Some(base),
            base_kind: Some("main".into()),
        }],
    };
    task_ops::workspace::write_marker(&task_dir, &marker).expect("marker");
    (task.id, repo)
}

async fn send_op(env: &TestEnv, bearer: &str, key: &str, path: &str, body: Value) -> Value {
    let resp = send(
        &env.router(),
        post_json_with(
            OPS,
            &op_body(key, "POST", path, body),
            &[("authorization", bearer)],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 200, "{key}: {}", resp.text());
    resp.json()["operation"].clone()
}

fn states(env: &TestEnv, op: &Value) -> Vec<Value> {
    audit_events(env, op["id"].as_str().expect("id"))
        .into_iter()
        .map(|e| e["state"].clone())
        .collect()
}

#[tokio::test]
async fn cos_ops_projects_cron_changes_merge_is_external_once() {
    let bin = tempfile::tempdir().expect("bin");
    let env = env_with_gh(bin.path());
    let (task_id, repo) = seed_git_task(&env);
    let path = format!("/api/v1/tasks/{task_id}/changes/code/integrate");
    let (_, _, bearer) = cos_bearer(&env, "integrate");

    let direct = send(
        &env.router(),
        post_json_with(
            &path,
            &json!({"method": "merge"}),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&direct, 422, "cos_audit_context_required");
    let before = git(&repo, &["rev-parse", "main"]);

    let op = send_op(&env, &bearer, "merge", &path, json!({"method": "merge"})).await;
    assert_eq!(op["state"], "applied", "{op}");
    assert_eq!(op["action"], "task.integrate");
    assert_eq!(op["result"]["integration"]["state"], "done", "{op}");
    assert_eq!(states(&env, &op), vec![json!("pending"), json!("applied")]);
    let merged = git(&repo, &["rev-parse", "main"]);
    assert_ne!(merged, before, "main advanced");

    // The resent request returns the record; git is not run again.
    let again = send_op(&env, &bearer, "merge", &path, json!({"method": "merge"})).await;
    assert_eq!(again["id"], op["id"]);
    assert_eq!(git(&repo, &["rev-parse", "main"]), merged);
    assert_eq!(states(&env, &op).len(), 2);

    // A discard without confirmation is refused before git: a rejected record.
    let resp = send(
        &env.router(),
        post_json_with(
            OPS,
            &op_body("discard", "POST", &path, json!({"method": "discard"})),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 422, "{}", resp.text());
}

#[tokio::test]
async fn cos_ops_projects_cron_changes_pr_create_and_merge_call_gh_once() {
    let bin = tempfile::tempdir().expect("bin");
    let env = env_with_gh(bin.path());
    let (task_id, repo) = seed_git_task(&env);
    let origin = env.dir.path().join("origin.git");
    let out = std::process::Command::new("git")
        .args(["init", "-q", "--bare", "-b", "main"])
        .arg(&origin)
        .output()
        .expect("git init --bare");
    assert!(out.status.success());
    git(
        &repo,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    );
    let (_, _, bearer) = cos_bearer(&env, "pr");

    let op = send_op(
        &env,
        &bearer,
        "pr",
        &format!("/api/v1/tasks/{task_id}/changes/code/integrate"),
        json!({"method": "pr"}),
    )
    .await;
    assert_eq!(op["result"]["integration"]["state"], "open", "{op}");
    assert_eq!(op["result"]["integration"]["pr_number"], 42);

    let merge = format!("/api/v1/tasks/{task_id}/changes/code/pr/merge");
    let op = send_op(&env, &bearer, "pr-merge", &merge, json!({})).await;
    assert_eq!(op["action"], "task.pr_merge");
    assert_eq!(op["result"]["integration"]["state"], "merged", "{op}");
    assert_eq!(states(&env, &op), vec![json!("pending"), json!("applied")]);
    let merges = || {
        std::fs::read_to_string(bin.path().join("merged.log"))
            .unwrap_or_default()
            .lines()
            .count()
    };
    assert_eq!(merges(), 1);

    // Resent: the same record, no second `gh pr merge`.
    let again = send_op(&env, &bearer, "pr-merge", &merge, json!({})).await;
    assert_eq!(again["id"], op["id"]);
    assert_eq!(merges(), 1);

    // A new request after the merge is refused before `gh` (the PR is no longer open).
    let resp = send(
        &env.router(),
        post_json_with(
            OPS,
            &op_body("pr-merge-2", "POST", &merge, json!({})),
            &[("authorization", bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&resp, 409, "pr_unavailable");
    assert_eq!(merges(), 1);
}
