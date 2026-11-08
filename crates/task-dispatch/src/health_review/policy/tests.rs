//! ADR 2026-11-07-health-review-integration D2.3・I3 の試験。本物の `--print-review-policy` の代わりに、一時 git repo に
//! stub の `repo_health.py`（隣の `profiles/review.json` をそのまま出す）を commit して使う。外部ネットワークに出ない。

use std::path::Path;
use std::time::Duration;

use super::*;

/// 隣の `profiles/review.json` を出す stub。`@BASE@` は渡された `--base`、`@SIDE@` は `../source-size-report.py` が
/// 一緒に出ていたか（`yes`・`no`）に置き換える。引数の形が違えば exit 2。
const STUB: &str = r#"import os, sys
here = os.path.dirname(os.path.abspath(__file__))
argv = sys.argv[1:]
if argv[:1] != ["--print-review-policy"] or "--repo" not in argv or "--base" not in argv:
    sys.stderr.write("usage: %r\n" % argv)
    sys.exit(2)
with open(os.path.join(here, "profiles", "review.json")) as f:
    text = f.read()
side = os.path.isfile(os.path.join(here, "..", "source-size-report.py"))
text = text.replace("@BASE@", argv[argv.index("--base") + 1]).replace("@SIDE@", "yes" if side else "no")
sys.stdout.write(text)
"#;

const PROFILE_PATH: &str = "scripts/dev/repo-health/profiles/review.json";
const SIDE_PATH: &str = "scripts/dev/source-size-report.py";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn sh_git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(dir: &Path, path: &str, body: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, body).unwrap();
}

/// `files` を書いて commit し、HEAD の sha を返す。
fn commit(dir: &Path, files: &[(&str, &str)], message: &str) -> String {
    for (path, body) in files {
        write(dir, path, body);
    }
    sh_git(dir, &["add", "-A"]);
    sh_git(dir, &["commit", "-q", "-m", message]);
    sh_git(dir, &["rev-parse", "HEAD"])
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    sh_git(dir.path(), &["init", "-q", "-b", "main"]);
    sh_git(dir.path(), &["config", "user.email", "t@example.com"]);
    sh_git(dir.path(), &["config", "user.name", "t"]);
    commit(dir.path(), &[("README.md", "hello\n")], "first");
    dir
}

fn present_json(class: &str) -> String {
    policy_json("base", class)
}

fn policy_json(source: &str, class: &str) -> String {
    serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA,
        "source": source,
        "owner_map": "docs/architecture-map.md",
        "high_risk": [{"class": class, "paths": ["^crates/task-store/"]}],
        "invariants": {"base": "@BASE@", "side": "@SIDE@"},
        "definition_sha256": DIGEST,
    })
    .to_string()
}

/// stub と方針の JSON を main に commit した repo（`side` なら `source-size-report.py` も）。
fn repo_with(profile: &str, side: bool) -> (tempfile::TempDir, String) {
    let dir = init_repo();
    let mut files = vec![(CHECKER_SCRIPT, STUB), (PROFILE_PATH, profile)];
    if side {
        files.push((SIDE_PATH, "# side\n"));
    }
    let sha = commit(dir.path(), &files, "checker");
    (dir, sha)
}

fn load(dir: &Path, target: &str) -> ReviewPolicyState {
    load_review_policy(dir, target, REVIEW_POLICY_TIMEOUT)
}

fn unavailable(state: ReviewPolicyState) -> String {
    match state {
        ReviewPolicyState::Unavailable(why) => why,
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

/// D2.3: `source = base` は欄をそのまま `Present` に写す。`--base` は解決した sha、検査器の隣の
/// `source-size-report.py` も base から一緒に出る。
#[test]
fn health_review_policy_present_maps_every_field_from_the_base() {
    let (dir, sha) = repo_with(&present_json("store"), true);
    let ReviewPolicyState::Present(policy) = load(dir.path(), "main") else {
        panic!("expected Present");
    };
    assert_eq!(
        policy,
        ReviewPolicy {
            owner_map: Some("docs/architecture-map.md".into()),
            high_risk: vec![PathClass {
                class: "store".into(),
                paths: vec!["^crates/task-store/".into()],
            }],
            invariants: [("base".to_string(), sha), ("side".into(), "yes".into())]
                .into_iter()
                .collect(),
            definition_sha256: DIGEST.into(),
        }
    );
}

/// `source = builtin`（base に profile が無く検査器自身の profile）も `Present`。side の file が base に無ければ
/// 出さない（検査器は無いことを自分で扱う）。
#[test]
fn health_review_policy_builtin_source_is_present_without_side_file() {
    let (dir, _) = repo_with(&policy_json("builtin", "generic"), false);
    let ReviewPolicyState::Present(policy) = load(dir.path(), "main") else {
        panic!("expected Present");
    };
    assert_eq!(policy.invariants["side"], "no");
}

/// `source = absent`（profile に `[review]` が無い）は `Absent`。
#[test]
fn health_review_policy_absent_source_is_absent() {
    let json = serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA,
        "source": "absent",
        "owner_map": null,
        "high_risk": [],
        "invariants": {},
        "definition_sha256": DIGEST,
    })
    .to_string();
    let (dir, _) = repo_with(&json, true);
    assert_eq!(load(dir.path(), "main"), ReviewPolicyState::Absent);
}

/// 非 0 exit は `Unavailable`（stderr の最初の行と exit code を理由に残す）。
#[test]
fn health_review_policy_nonzero_exit_is_unavailable() {
    let dir = init_repo();
    let failing =
        "import sys\nsys.stderr.write('profile_invalid: review.high_risk\\n')\nsys.exit(2)\n";
    commit(dir.path(), &[(CHECKER_SCRIPT, failing)], "failing checker");
    let why = unavailable(load(dir.path(), "main"));
    assert!(
        why.contains("profile_invalid: review.high_risk") && why.contains("Some(2)"),
        "{why}"
    );
}

/// 壊れた JSON・schema 違い・未知の欄・不正な digest は `Unavailable`。
#[test]
fn health_review_policy_broken_output_is_unavailable() {
    let (dir, _) = repo_with("{not json", true);
    assert!(unavailable(load(dir.path(), "main")).contains("JSON でない"));

    let other_schema =
        present_json("store").replace(REVIEW_POLICY_SCHEMA, "celeris.repo-health.review-policy/2");
    let (dir, _) = repo_with(&other_schema, true);
    assert!(unavailable(load(dir.path(), "main")).contains("review-policy/2"));

    let mut extra: serde_json::Value = serde_json::from_str(&present_json("store")).unwrap();
    extra["surprise"] = serde_json::json!(1);
    assert!(unavailable(parse_review_policy(&extra.to_string())).contains("surprise"));

    let short_digest = present_json("store").replace(DIGEST, "abc");
    assert!(unavailable(parse_review_policy(&short_digest)).contains("definition_sha256"));
}

/// `source = bootstrap`（head の profile）と未知の source は使わない（I3）。
#[test]
fn health_review_policy_bootstrap_and_unknown_sources_are_unavailable() {
    let bootstrap = policy_json("bootstrap", "store");
    assert!(unavailable(parse_review_policy(&bootstrap)).contains("bootstrap"));
    let unknown = policy_json("head", "store");
    assert!(unavailable(parse_review_policy(&unknown)).contains("\"head\""));
}

/// base に script が無ければ、作業ツリーに script があっても `Absent`（その repo は `[review]` 節を持たない。作業ツリーの
/// 検査器を起こさない）。解決できない target は `Unavailable`。
#[test]
fn health_review_policy_script_missing_on_base_is_absent() {
    let dir = init_repo();
    write(dir.path(), CHECKER_SCRIPT, STUB);
    write(dir.path(), PROFILE_PATH, &present_json("worktree"));
    assert_eq!(load(dir.path(), "main"), ReviewPolicyState::Absent);
    let why = unavailable(load(dir.path(), "no-such-branch"));
    assert!(why.contains("no-such-branch"), "{why}");
}

/// I3: 候補の branch（HEAD）と作業ツリーで検査器と profile を変えても、base（target）側の結果が使われる。
#[test]
fn health_review_policy_base_side_wins_over_candidate_and_worktree() {
    let (dir, base_sha) = repo_with(&present_json("base-class"), true);
    sh_git(dir.path(), &["checkout", "-q", "-b", "candidate"]);
    let weakened = STUB.replace(
        "sys.stdout.write(text)",
        "sys.stdout.write(text.replace('base-class', 'x'))",
    );
    commit(
        dir.path(),
        &[
            (CHECKER_SCRIPT, &weakened),
            (PROFILE_PATH, &present_json("candidate-class")),
        ],
        "candidate weakens the policy",
    );
    write(dir.path(), PROFILE_PATH, &present_json("worktree-class"));
    write(dir.path(), CHECKER_SCRIPT, "import sys\nsys.exit(7)\n");

    for target in [base_sha.as_str(), "main"] {
        let ReviewPolicyState::Present(policy) = load(dir.path(), target) else {
            panic!("expected Present for {target}");
        };
        assert_eq!(policy.high_risk[0].class, "base-class", "{target}");
        assert_eq!(policy.invariants["base"], base_sha, "{target}");
    }
    // 同じ repo の候補の branch を target にすれば候補の版になる（target を選ぶのは呼び出し側）。
    let ReviewPolicyState::Present(policy) = load(dir.path(), "candidate") else {
        panic!("expected Present for candidate");
    };
    assert_eq!(policy.high_risk[0].class, "candidate-class");
}

/// 時間の上限を超えた script は殺して `Unavailable`（tick を止め続けない）。
#[test]
fn health_review_policy_timeout_is_unavailable() {
    let dir = init_repo();
    commit(
        dir.path(),
        &[(CHECKER_SCRIPT, "import time\ntime.sleep(120)\n")],
        "slow checker",
    );
    let started = std::time::Instant::now();
    let why = unavailable(load_review_policy(
        dir.path(),
        "main",
        Duration::from_secs(2),
    ));
    assert!(
        why.contains("時間内に終わらない") || why.contains("上限を超えた"),
        "{why}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
}
