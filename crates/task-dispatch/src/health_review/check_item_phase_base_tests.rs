//! ADR 2026-11-08-health-enforcement-loop D1.2 の付記（2026-11-10）: 段の統合の health の base の試験。
//!
//! 守る不変条件: 段の途中で Task の branch に取り込んだ review target の commit（`P..M`）は段の変更として数えず、段の
//! worker の変更（unit の branch の commit）は今どおり `base..head` に残る。target を解決できなければ合格にしない。
//! 既存の流れの試験（`dispatcher/tests/health_enforce.rs`）は Task の branch が段の途中で進まない作業場所（案件の repo の
//! 参照が無い）だけを通り、base = 段の基点しか見ないので、取り込みのある branch の base と、検査器が受け取る base・head は
//! ここで確かめる。検査器は試験の repo の base に commit した stub で、受け取った `--base`・`--head` を repo の隣の
//! log に書き、`git diff --name-only base head` から policy の file（`scripts/dev/repo-health/`）の変更を
//! `policy.changed`、`*.big` の file を `size.file` の fail 級の finding にする。外部ネットワークに出ず、LLM を呼ばない。

use super::*;
use task_core::required_checks::health::{CHECKER_SCRIPT, HealthVerdict, SOURCE_SIZE_REPORT};

const STUB: &str = r#"import json, os, subprocess, sys
argv = sys.argv[1:]
if "--print-review-policy" in argv:
    sys.stdout.write(json.dumps({"schema": "celeris.repo-health.review-policy/1", "source": "absent",
        "owner_map": None, "high_risk": [], "invariants": {}, "definition_sha256": "0" * 64}))
    sys.exit(0)
arg = lambda name: argv[argv.index(name) + 1]
repo, base, head = arg("--repo"), arg("--base"), arg("--head")
with open(os.path.join(os.path.dirname(repo), "checker-args.log"), "a") as f:
    f.write("%s %s\n" % (base, head))
names = subprocess.run(["git", "-C", repo, "diff", "--no-renames", "--name-only", base, head],
    capture_output=True, text=True, check=True).stdout.split()
def finding(rule, number, paths):
    return {"rule_id": rule, "fingerprint": "%s:%016d" % (rule, number), "severity": "error", "delta": "new",
        "exception": None, "evidence": [{"path": p} for p in paths]}
findings = []
policy = [n for n in names if n.startswith("scripts/dev/repo-health/")]
if policy:
    findings.append(finding("policy.changed", 1, policy))
big = [n for n in names if n.endswith(".big")]
if big:
    findings.append(finding("size.file", 2, big))
code = 1 if findings else 0
rules = [{"id": r, "required": True, "status": "fail" if any(f["rule_id"] == r for f in findings) else "pass",
    "reason": ""} for r in ("policy.changed", "size.file")]
report = {"schema": "celeris.repo-health.report/1", "tool": {"name": "repo_health.py", "version": "stub"},
    "inputs": {"profile": {"resolved": "builtin", "source": "builtin"}},
    "policy": {"version": "1", "source": "builtin", "changed": bool(policy), "files": []},
    "rules": rules, "findings": findings,
    "summary": {"verdict": "fail" if code else "pass", "exit_code": code, "reviewable": True,
        "counts": {"findings": len(findings), "fail_grade": len(findings), "new": len(findings),
        "worsened": 0, "existing": 0}}}
with open(arg("--out"), "w") as f:
    json.dump(report, f)
sys.exit(code)
"#;

/// 本物の `health/engine.py` と同じ fail 級の判定（PolicyOnly にはしない）。
const ENGINE: &str = r#"def is_fail_grade(f):
    return f["severity"] == "error" and f["delta"] in ("new", "worsened") and f["exception"] is None


def policy_review_files(report, changed):
    return None
"#;

/// `dir` で git を実行し、成功を確かめて stdout（trim）を返す。
fn git(dir: &Path, args: &[&str]) -> String {
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

/// `path`（repo 相対）に `body` を書いて commit し、HEAD の sha を返す。
fn commit_file(repo: &Path, path: &str, body: &str) -> String {
    let file = repo.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, body).unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", path]);
    git(repo, &["rev-parse", "HEAD"])
}

/// 試験の repo（`root/repo`）。`main`（review target）に stub の検査器を commit し、段の基点 P として Task の branch
/// `task` を切って checkout する。返すのは `(repo, P)`。
fn repo_with_checker(root: &Path) -> (PathBuf, String) {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    let dir = repo.join("scripts/dev/repo-health");
    std::fs::create_dir_all(dir.join("health")).unwrap();
    std::fs::write(repo.join(CHECKER_SCRIPT), STUB).unwrap();
    std::fs::write(dir.join("health/engine.py"), ENGINE).unwrap();
    std::fs::write(dir.join("health/__init__.py"), "").unwrap();
    let side = repo.join(SOURCE_SIZE_REPORT);
    std::fs::create_dir_all(side.parent().unwrap()).unwrap();
    std::fs::write(side, "# side\n").unwrap();
    let start = commit_file(&repo, "README.md", "hello\n");
    git(&repo, &["checkout", "-q", "-b", "task"]);
    (repo, start)
}

/// 段の unit の branch `wu-<key>` を `from` から切り、`files`（`(path, body)`）を commit する（worker の変更）。checkout は
/// `task` に戻す。
fn unit_branch(repo: &Path, key: &str, from: &str, files: &[(&str, &str)]) {
    git(repo, &["checkout", "-q", "-b", &format!("wu-{key}"), from]);
    for (path, body) in files {
        commit_file(repo, path, body);
    }
    git(repo, &["checkout", "-q", "task"]);
}

/// 承認済みの policy の変更を `main` に入れ、`task` を main の HEAD まで ff-only で進める（policy だけの子 task の上陸の
/// 後に親の branch を進める手順）。返すのは main の HEAD（= M）。
fn land_policy_on_main_and_advance_task(repo: &Path) -> String {
    git(repo, &["checkout", "-q", "main"]);
    let landed = commit_file(
        repo,
        "scripts/dev/repo-health/rules/landed.txt",
        "approved\n",
    );
    git(repo, &["checkout", "-q", "task"]);
    git(repo, &["merge", "-q", "--ff-only", "main"]);
    landed
}

/// 段の統合の代わり: `wu-<key>` を `task` に no-ff で merge し、統合の HEAD を返す。
fn integrate(repo: &Path, key: &str) -> String {
    git(
        repo,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "integrate",
            &format!("wu-{key}"),
        ],
    );
    git(repo, &["rev-parse", "HEAD"])
}

/// 統合の前に段の health の対象を決め（`target` は review target の解決の結果）、`key` の unit を統合して評価する。
fn evaluate_phase(
    repo: &Path,
    start: &str,
    key: &str,
    target: Result<Option<String>, String>,
) -> (HealthItem, String) {
    let items = [(repo.to_path_buf(), None::<RepoId>, ())];
    let health = phase_target(Some(start.to_string()), &items, target).expect("git repo");
    let head = integrate(repo, key);
    (evaluate(&health.at_head(&head)), head)
}

/// 検査器が受け取った `(--base, --head)` の並び。
fn checker_args(repo: &Path) -> Vec<(String, String)> {
    let log = repo.parent().unwrap().join("checker-args.log");
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(b, h)| (b.to_string(), h.to_string()))
        .collect()
}

fn main_target() -> Result<Option<String>, String> {
    Ok(Some("refs/heads/main".to_string()))
}

/// 不変条件: 段の途中で Task の branch を review target の承認済みの commit（policy の file を変えた commit）まで進めても、
/// 段の統合の health は base を target との merge-base M にし、`policy.changed` で落ちない。検査器は base = M、head = 統合の
/// HEAD を受け取る。target の無い作業場所（base = 段の基点 P）で同じ HEAD を評価すると `policy.changed` で落ちるので、
/// 合格は base の決め方による。
#[test]
fn health_enforce_phase_base_target_commits_taken_mid_phase_do_not_count() {
    let root = tempfile::tempdir().unwrap();
    let (repo, start) = repo_with_checker(root.path());
    unit_branch(&repo, "a", &start, &[("a.txt", "a\n")]);
    let landed = land_policy_on_main_and_advance_task(&repo);

    let (item, head) = evaluate_phase(&repo, &start, "a", main_target());
    assert!(item.pass, "{item:?}");
    assert_eq!(item.evidence.verdict, HealthVerdict::Pass);
    assert_eq!(item.evidence.base_sha, landed);
    assert_eq!(item.evidence.head_sha, head);
    assert_eq!(checker_args(&repo), vec![(landed.clone(), head.clone())]);

    let items = [(repo.clone(), None::<RepoId>, ())];
    let at_start = phase_target(Some(start.clone()), &items, Ok(None)).unwrap();
    let old = evaluate(&at_start.at_head(&head));
    assert!(!old.pass, "{old:?}");
    assert!(old.reason.contains("health:policy.changed:"), "{old:?}");
    assert_eq!(old.evidence.base_sha, start);
}

/// 不変条件: base を M に進めても、段の中で worker が policy の file を変えた変更は `M..head` に残り、今どおり
/// `policy.changed` の fail 級（Repair）で落ちる（I3: 候補が自分の policy を弱めて合格にできない）。
#[test]
fn health_enforce_phase_base_worker_policy_change_still_fails() {
    let root = tempfile::tempdir().unwrap();
    let (repo, start) = repo_with_checker(root.path());
    unit_branch(
        &repo,
        "a",
        &start,
        &[("scripts/dev/repo-health/rules/worker.txt", "weaker\n")],
    );
    let landed = land_policy_on_main_and_advance_task(&repo);

    let (item, head) = evaluate_phase(&repo, &start, "a", main_target());
    assert!(!item.pass, "{item:?}");
    assert_eq!(item.exit, Some(1));
    assert_eq!(item.evidence.verdict, HealthVerdict::Fail);
    assert!(item.reason.contains("health:policy.changed:"), "{item:?}");
    assert!(item.reason.contains("rules/worker.txt"), "{item:?}");
    assert!(!item.reason.contains("rules/landed.txt"), "{item:?}");
    assert_eq!(checker_args(&repo), vec![(landed, head)]);
}

/// 不変条件: base を M に進めても、段の中の worker の fail 級の変更（size）は今どおり検出される。target 側の policy の
/// 変更は数えないので、不合格は size の rule だけになる。
#[test]
fn health_enforce_phase_base_worker_fail_grade_change_still_detected() {
    let root = tempfile::tempdir().unwrap();
    let (repo, start) = repo_with_checker(root.path());
    unit_branch(&repo, "a", &start, &[("data/huge.big", "x\n")]);
    let landed = land_policy_on_main_and_advance_task(&repo);

    let (item, head) = evaluate_phase(&repo, &start, "a", main_target());
    assert!(!item.pass, "{item:?}");
    assert_eq!(item.evidence.verdict, HealthVerdict::Fail);
    assert!(item.reason.contains("health:size.file:"), "{item:?}");
    assert!(!item.reason.contains("policy.changed"), "{item:?}");
    assert_eq!(repair_buckets(&item.reason), vec!["health:size.file"]);
    assert_eq!(checker_args(&repo), vec![(landed, head)]);
}

/// 不変条件: 段の途中で target を取り込んでいない branch（段の基点 P が前の段の commit を持ち、target との merge-base M が
/// P より前）は base = P のまま。前の段で統合済みの policy の変更を段の変更として数えない（常に merge-base を base に
/// する案を退けた理由）。
#[test]
fn health_enforce_phase_base_without_target_intake_keeps_phase_start() {
    let root = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_checker(root.path());
    let start = commit_file(&repo, "scripts/dev/repo-health/rules/earlier.txt", "x\n");
    unit_branch(&repo, "a", &start, &[("a.txt", "a\n")]);
    git(&repo, &["checkout", "-q", "main"]);
    commit_file(&repo, "main.txt", "moved\n");
    git(&repo, &["checkout", "-q", "task"]);

    let (item, head) = evaluate_phase(&repo, &start, "a", main_target());
    assert!(item.pass, "{item:?}");
    assert_eq!(item.evidence.base_sha, start);
    assert_eq!(checker_args(&repo), vec![(start, head)]);
}

/// 不変条件: review target を解決できない（`review_target_ref` が返さない）・target の ref で git が失敗したときは、検査器を
/// 起こさず health の項目を Error（`health_check_error: facts_unreadable:phase_base: …`）にして合格にしない（fail closed）。
#[test]
fn health_enforce_phase_base_unresolved_target_does_not_pass() {
    let root = tempfile::tempdir().unwrap();
    let (repo, start) = repo_with_checker(root.path());
    unit_branch(&repo, "a", &start, &[("a.txt", "a\n")]);
    unit_branch(&repo, "b", &start, &[("b.txt", "b\n")]);
    land_policy_on_main_and_advance_task(&repo);

    let unresolved = Err("repo r の review target を解決できない".to_string());
    let missing = Ok(Some("refs/heads/no-such-branch".to_string()));
    for (key, target) in [("a", unresolved), ("b", missing)] {
        let (item, _) = evaluate_phase(&repo, &start, key, target);
        assert!(!item.pass, "{item:?}");
        assert_eq!(item.exit, None);
        assert_eq!(item.evidence.verdict, HealthVerdict::Error);
        assert!(
            item.reason
                .starts_with("health_check_error: facts_unreadable:phase_base: "),
            "{item:?}"
        );
        assert_eq!(
            check_error(HEALTH_CHECK_ID, item.pass, &item.reason),
            Some(item.reason.as_str())
        );
        assert!(stops_without_replan(&item.reason));
    }
    assert!(checker_args(&repo).is_empty(), "the checker must not run");
}
