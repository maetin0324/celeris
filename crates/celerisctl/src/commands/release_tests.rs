use std::path::Path;
use std::process::Command;

use clap::Parser;

use super::*;

#[derive(Parser)]
struct Wrap {
    #[command(subcommand)]
    command: ReleaseCommand,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(dir: &Path, rel: &str, msg: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(&p, msg).expect("write");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

const TASK: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

/// base, task branch (2 commits, merged --no-ff), direct commit, migration.
fn make_repo(dir: &Path) -> (String, String) {
    git(dir, &["init", "-q", "-b", "main"]);
    commit_file(dir, "a.txt", "base");
    let base = git(dir, &["rev-parse", "HEAD"]);
    git(dir, &["checkout", "-q", "-b", &format!("celeris/{TASK}")]);
    commit_file(dir, "t1.txt", "task one");
    commit_file(dir, "t2.txt", "task two");
    git(dir, &["checkout", "-q", "main"]);
    git(
        dir,
        &[
            "merge",
            "--no-ff",
            "-m",
            &format!("Merge branch 'celeris/{TASK}'"),
            &format!("celeris/{TASK}"),
        ],
    );
    commit_file(dir, "direct.txt", "direct change");
    commit_file(dir, "crates/task-core/migrations/0099_x.sql", "-- m");
    (base, git(dir, &["rev-parse", "HEAD"]))
}

fn parse_notes(args: &[&str]) -> NotesArgs {
    let mut v = vec!["x", "notes"];
    v.extend_from_slice(args);
    match Wrap::try_parse_from(v).expect("parse").command {
        ReleaseCommand::Notes(a) => a,
        ReleaseCommand::Preview(_) => panic!("expected notes"),
    }
}

#[test]
fn parses_notes_args() {
    let a = parse_notes(&[
        "--repo",
        "/r",
        "--sha",
        "abc",
        "--base",
        "def",
        "--schema-from",
        "3",
        "--schema-to",
        "4",
        "--gate-json",
        "/g.json",
        "--api",
        "http://127.0.0.1:1",
        "--token-file",
        "/t",
        "--out-dir",
        "/o",
    ]);
    assert_eq!(a.sha, "abc");
    assert_eq!(a.base.as_deref(), Some("def"));
    assert_eq!((a.schema_from, a.schema_to), (Some(3), Some(4)));
    assert_eq!(a.api.as_deref(), Some("http://127.0.0.1:1"));
    assert!(Wrap::try_parse_from(["x", "notes", "--repo", "/r"]).is_err());
}

#[test]
fn parses_preview_args() {
    let w = Wrap::try_parse_from([
        "x",
        "preview",
        "abcdef123456",
        "--releases-dir",
        "/d",
        "--json",
    ])
    .expect("parse");
    match w.command {
        ReleaseCommand::Preview(p) => {
            assert_eq!(p.sha12, "abcdef123456");
            assert!(p.json);
        }
        ReleaseCommand::Notes(_) => panic!("expected preview"),
    }
}

#[test]
fn notes_writes_json_and_markdown_without_api() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).expect("mkdir");
    let (base, head) = make_repo(&repo);
    let out = tmp.path().join("out");
    let args = parse_notes(&[
        "--repo",
        repo.to_str().expect("utf8"),
        "--sha",
        &head,
        "--base",
        &base,
        "--schema-from",
        "5",
        "--schema-to",
        "6",
        "--out-dir",
        out.to_str().expect("utf8"),
    ]);
    let line = run_notes(&args).expect("notes");
    assert!(line.contains("tasks=1"), "{line}");
    assert!(
        line.contains("direct=2") || line.contains("direct=1"),
        "{line}"
    );
    assert!(line.contains("migrations=1"), "{line}");

    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("notes.json")).expect("json"))
            .expect("valid json");
    assert_eq!(v["sha"], head.as_str());
    assert_eq!(v["tasks"][0]["task_id"], TASK);
    assert_eq!(v["schema"]["changed"], true);
    assert!(
        std::fs::read_to_string(out.join("notes.md"))
            .expect("md")
            .contains(TASK)
    );
    // tmp が残らない。
    let names: Vec<_> = std::fs::read_dir(&out)
        .expect("ls")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[test]
fn notes_fails_on_unknown_sha() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).expect("mkdir");
    make_repo(&repo);
    let args = parse_notes(&[
        "--repo",
        repo.to_str().expect("utf8"),
        "--sha",
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        "--out-dir",
        tmp.path().join("o").to_str().expect("utf8"),
    ]);
    assert!(run_notes(&args).is_err());
}

#[test]
fn preview_missing_release_is_none() {
    let tmp = tempfile::tempdir().expect("tmp");
    let args = PreviewArgs {
        sha12: "abcdef123456".into(),
        releases_dir: tmp.path().to_path_buf(),
        json: false,
    };
    assert!(render_preview(&args).expect("ok").is_none());
}
