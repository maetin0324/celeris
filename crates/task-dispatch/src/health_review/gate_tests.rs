//! ADR 2026-11-08-health-enforcement-loop D1.3・D2.3・D3.1 の試験。本物の検査器の代わりに、一時 git repo の base に
//! stub の `repo_health.py`（隣の `stub.json` の report の文字列をそのまま `--out` に書き、`exit` で終わる）と
//! `health/engine.py`（本物と同じ `is_fail_grade`・`policy_review_files`）を commit して使う。検査器は適用判定
//! （`repo_facts` → `applicability`）が返したものを渡す。外部ネットワークに出ず、LLM を呼ばない。

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use task_core::RepoId;
use task_core::required_checks::health::{
    CHECKER_DIR, CHECKER_SCRIPT, CheckerId, CheckerSource, HealthApplicability, HealthVerdict,
    PROFILES_DIR, PolicyIdentity, SOURCE_SIZE_REPORT, applicability, base_checker, policy_sha256,
    repo_facts,
};

use super::*;

const STUB: &str = r#"import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
argv = sys.argv[1:]
for name in ("--repo", "--base", "--head", "--policy-version", "--out"):
    if name not in argv:
        sys.stderr.write("usage: %r\n" % argv)
        sys.exit(2)
with open(os.path.join(here, "stub.json")) as f:
    stub = json.load(f)
if stub.get("sleep"):
    import time
    time.sleep(stub["sleep"])
if stub.get("report") is not None:
    with open(argv[argv.index("--out") + 1], "w") as f:
        f.write(stub["report"])
sys.exit(stub["exit"])
"#;

/// 本物の `health/engine.py` と同じ判定（`scripts/dev/repo-health/health/engine.py`）。
const ENGINE: &str = r#"def is_fail_grade(f):
    return f["severity"] == "error" and f["delta"] in ("new", "worsened") and f["exception"] is None


def policy_review_files(report, changed):
    if report["summary"]["verdict"] != "fail" or not changed:
        return None
    if any(r["required"] and r["status"] == "fail" and r["id"] != "policy.changed" for r in report["rules"]):
        return None
    if any(is_fail_grade(f) and f["rule_id"] != "policy.changed" for f in report["findings"]):
        return None
    policy_paths = {f["path"] for f in report["policy"]["files"]}
    if not report["policy"]["changed"] or not set(changed) <= policy_paths:
        return None
    return sorted(changed)
"#;

/// stub の検査器の repo に `health/engine.py`（[`ENGINE`]）を置く。report の exit 1 を分類する入口（`policy_review`）が
/// 同じ検査器の `policy_review_files` を呼ぶので、exit 1 の report を書く stub の検査器はこれが要る（dispatcher の試験の
/// 土台も使う）。
pub(crate) fn write_engine(repo: &Path) {
    for (path, text) in [(ENGINE_PATH, ENGINE), (INIT_PATH, "")] {
        let path = repo.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

const STUB_JSON: &str = "scripts/dev/repo-health/stub.json";
const ENGINE_PATH: &str = "scripts/dev/repo-health/health/engine.py";
const INIT_PATH: &str = "scripts/dev/repo-health/health/__init__.py";
const PROFILE: &str = "scripts/dev/repo-health/profiles/celeris.toml";
const SRC: &str = "src/lib.rs";
const TIMEOUT: Duration = Duration::from_secs(60);
/// 同梱の検査器を渡さない（base の検査器の試験）。
const BUNDLED: Option<&BundledChecker> = None;

fn git_in(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

fn sh_git(dir: &Path, args: &[&str]) -> String {
    git_in(dir, args)
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"))
        .trim()
        .to_string()
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
    sh_git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
    sh_git(dir, &["rev-parse", "HEAD"])
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    sh_git(dir.path(), &["init", "-q", "-b", "main"]);
    sh_git(dir.path(), &["config", "user.email", "t@example.com"]);
    sh_git(dir.path(), &["config", "user.name", "t"]);
    commit(
        dir.path(),
        &[("README.md", "hello\n"), (SRC, "fn a() {}\n")],
        "first",
    );
    dir
}

/// stub の設定（`report` は `--out` に書く文字列。`None` なら書かない）。
fn stub_json(exit: i32, report: Option<&str>) -> String {
    json!({"exit": exit, "report": report}).to_string()
}

/// 検査器の file（stub・engine・side）と `stub`。`profile` なら profile の file も。
fn checker_files(stub: &str, profile: bool) -> Vec<(&'static str, String)> {
    let mut files = vec![
        (CHECKER_SCRIPT, STUB.to_string()),
        (STUB_JSON, stub.to_string()),
        (ENGINE_PATH, ENGINE.to_string()),
        (INIT_PATH, String::new()),
        (SOURCE_SIZE_REPORT, "# side\n".to_string()),
    ];
    if profile {
        files.push((PROFILE, "[review]\n".to_string()));
    }
    files
}

/// base に検査器を commit した repo と base の sha。
fn repo_with_checker(stub: &str, profile: bool) -> (tempfile::TempDir, String) {
    let dir = init_repo();
    let files = checker_files(stub, profile);
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, b)| (*p, b.as_str())).collect();
    let base = commit(dir.path(), &refs, "checker");
    (dir, base)
}

/// 適用判定（`repo_facts` → `applicability`）が返す検査器。
fn applied(dir: &Path, base: &str, bundled: Option<&CheckerId>) -> CheckerId {
    match applicability(&repo_facts(true, base, bundled, |args| git_in(dir, args))) {
        HealthApplicability::Applies { checker } => checker,
        other => panic!("not applicable: {other:?}"),
    }
}

fn finding(rule: &str, n: usize, severity: &str, delta: &str, exception: bool) -> Value {
    json!({
        "rule_id": rule,
        "fingerprint": format!("{rule}:{n:016x}"),
        "severity": severity,
        "delta": delta,
        "exception": if exception { json!({"reason": "r", "source": "profile"}) } else { Value::Null },
        "evidence": [{"path": SRC}],
    })
}

/// report/1 の文字列。`policy_files` は `policy.files`、`rules` は `rules`。
fn report(
    verdict: &str,
    exit: i32,
    findings: Vec<Value>,
    policy: Value,
    rules: Vec<Value>,
) -> String {
    json!({
        "schema": "celeris.repo-health.report/1",
        "tool": {"name": "repo_health.py", "version": "0.9.1"},
        "inputs": {"base": "main", "head": "x", "policy_version": "1",
            "profile": {"requested": null, "resolved": "celeris", "source": "repo"}, "mode": "full"},
        "policy": policy,
        "rules": rules,
        "findings": findings,
        "summary": {"verdict": verdict, "exit_code": exit, "reviewable": true,
            "counts": {"findings": 0, "fail_grade": 0, "new": 0, "worsened": 0, "existing": 0}},
    })
    .to_string()
}

fn base_policy(changed: bool) -> Value {
    json!({"version": "1", "source": "base", "changed": changed, "files": [
        {"path": PROFILE, "base_blob": "a".repeat(40), "head_blob": if changed { "b".repeat(40) } else { "a".repeat(40) }},
    ]})
}

fn rule(id: &str, status: &str, reason: &str) -> Value {
    json!({"id": id, "required": true, "status": status, "reason": reason})
}

fn sha256_of(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

/// 不変条件: exit 0 の証跡は、解決した base・head の SHA、head の tree、D2.3 の policy の指紋（base の profile の
/// dir の tree id と report の欄）、適用判定の検査器、report の byte 列の sha256、`tool.version`、`summary.reviewable`
/// に結び付く。head は ref で渡しても commit の SHA で残す。
#[test]
fn health_evidence_gate_pass_binds_each_field() {
    let text = report(
        "pass",
        0,
        vec![],
        base_policy(false),
        vec![rule("size.x", "pass", "")],
    );
    let (dir, base) = repo_with_checker(&stub_json(0, Some(&text)), true);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let checker = applied(dir.path(), &base, None);
    assert_eq!(checker.source, CheckerSource::Base);
    let repo_id = RepoId::new();

    let ev = run_health_check(
        repo_id,
        dir.path(),
        &base,
        "main",
        &checker,
        BUNDLED,
        TIMEOUT,
    )
    .evidence;

    assert_eq!(ev.verdict, HealthVerdict::Pass, "{ev:?}");
    assert_eq!(ev.repo_id, repo_id);
    assert_eq!(ev.base_sha, base);
    assert_eq!(ev.head_sha, head, "the ref is bound as its commit sha");
    let tree_rev = format!("{head}^{{tree}}");
    assert_eq!(ev.head_tree, sh_git(dir.path(), &["rev-parse", &tree_rev]));
    assert_eq!(ev.checker, Some(checker));
    assert_eq!(ev.report_sha256, sha256_of(&text));
    assert_eq!(ev.tool_version, "0.9.1");
    assert!(ev.reviewable);
    assert!(ev.blocking.is_empty());
    assert_eq!(ev.reason, None);
    assert_eq!(ev.reused_from, None);
    let profiles = sh_git(
        dir.path(),
        &["rev-parse", &format!("{base}:{PROFILES_DIR}")],
    );
    let expected = policy_sha256(&PolicyIdentity {
        policy_version: "1",
        policy_source: "base",
        profile_resolved: "celeris",
        profile_source: "repo",
        profiles_tree: Some(&profiles),
        base_blobs: &[Some("a".repeat(40))],
    });
    assert_eq!(ev.policy_sha256, expected);
}

/// 不変条件: exit 1 で policy だけの変更でなければ Fail。blocking は fail 級（error・new か worsened・例外なし）の
/// fingerprint を sort して 50 件まで、超えた数は reason に書く。
#[test]
fn health_evidence_gate_exit1_is_fail_with_sorted_blocking() {
    let mut findings: Vec<Value> = (0..53)
        .rev()
        .map(|n| finding("size.x", n, "error", "new", false))
        .collect();
    findings.push(finding("size.x", 90, "warning", "new", false));
    findings.push(finding("size.x", 91, "error", "existing", false));
    findings.push(finding("size.x", 92, "error", "worsened", true));
    let text = report(
        "fail",
        1,
        findings,
        base_policy(false),
        vec![rule("size.x", "fail", "")],
    );
    let (dir, base) = repo_with_checker(&stub_json(1, Some(&text)), true);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let checker = applied(dir.path(), &base, None);

    let ev = run_health_check(
        RepoId::new(),
        dir.path(),
        &base,
        &head,
        &checker,
        BUNDLED,
        TIMEOUT,
    )
    .evidence;

    assert_eq!(ev.verdict, HealthVerdict::Fail, "{ev:?}");
    assert_eq!(ev.blocking.len(), MAX_BLOCKING);
    let mut sorted = ev.blocking.clone();
    sorted.sort();
    assert_eq!(ev.blocking, sorted);
    assert_eq!(ev.blocking[0], format!("size.x:{:016x}", 0));
    assert!(
        ev.reason.as_deref().unwrap_or("").contains("3 件"),
        "{ev:?}"
    );
    assert_eq!(ev.report_sha256, sha256_of(&text));
}

/// policy.changed だけが fail 級の report（`policy.files` に profile）。
fn policy_report() -> String {
    report(
        "fail",
        1,
        vec![finding("policy.changed", 1, "error", "new", false)],
        base_policy(true),
        vec![rule("policy.changed", "fail", "")],
    )
}

/// 不変条件: base..head の変更 path が全部 policy の file なら、同じ検査器の `policy_review_files` が file を返し、
/// verdict は PolicyOnly（G2 D6.2）。
#[test]
fn health_evidence_gate_policy_only() {
    let text = policy_report();
    let (dir, base) = repo_with_checker(&stub_json(1, Some(&text)), true);
    let head = commit(dir.path(), &[(PROFILE, "[review]\nx = 1\n")], "policy");
    let checker = applied(dir.path(), &base, None);

    let ev = run_health_check(
        RepoId::new(),
        dir.path(),
        &base,
        &head,
        &checker,
        BUNDLED,
        TIMEOUT,
    )
    .evidence;

    assert_eq!(ev.verdict, HealthVerdict::PolicyOnly, "{ev:?}");
    assert_eq!(
        ev.reason.as_deref(),
        Some(&*format!("policy_only: {PROFILE}"))
    );
    assert_eq!(ev.blocking, vec![format!("policy.changed:{:016x}", 1)]);
}

/// 不変条件: policy の file と他の file を一緒に変えた候補は、report が同じでも Fail（混在を policy だけと見ない）。
#[test]
fn health_evidence_gate_policy_mixed_is_fail() {
    let text = policy_report();
    let (dir, base) = repo_with_checker(&stub_json(1, Some(&text)), true);
    let head = commit(
        dir.path(),
        &[(PROFILE, "[review]\nx = 1\n"), (SRC, "fn b() {}\n")],
        "policy and code",
    );
    let checker = applied(dir.path(), &base, None);

    let ev = run_health_check(
        RepoId::new(),
        dir.path(),
        &base,
        &head,
        &checker,
        BUNDLED,
        TIMEOUT,
    )
    .evidence;

    assert_eq!(ev.verdict, HealthVerdict::Fail, "{ev:?}");
    assert_eq!(ev.blocking, vec![format!("policy.changed:{:016x}", 1)]);
}

/// 不変条件: exit 2 は report があっても Error（合格にしない。J4）で、reason に必須 rule の unsupported・unknown を書く。
#[test]
fn health_evidence_gate_exit2_is_error() {
    let rules = vec![
        rule("size.x", "unsupported", "tool_missing:x"),
        rule("deps.cycle", "unknown", ""),
        rule("docs.y", "pass", ""),
    ];
    let text = report("error", 2, vec![], base_policy(false), rules);
    let (dir, base) = repo_with_checker(&stub_json(2, Some(&text)), true);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let checker = applied(dir.path(), &base, None);

    let ev = run_health_check(
        RepoId::new(),
        dir.path(),
        &base,
        &head,
        &checker,
        BUNDLED,
        TIMEOUT,
    )
    .evidence;

    assert_eq!(ev.verdict, HealthVerdict::Error, "{ev:?}");
    let reason = ev.reason.clone().unwrap_or_default();
    assert!(
        reason.contains("unsupported:size.x(tool_missing:x)"),
        "{reason}"
    );
    assert!(reason.contains("unknown:deps.cycle"), "{reason}");
    assert!(!reason.contains("docs.y"), "{reason}");
    assert_eq!(ev.report_sha256, sha256_of(&text));
}

/// 不変条件: report が無い・壊れた・schema が違う・時間切れは、exit 0 でも Error（J4）。reason の先頭の語で区別する。
#[test]
fn health_evidence_gate_missing_or_broken_report_is_error() {
    let wrong_schema = json!({"schema": "other/1", "summary": {}, "policy": {}}).to_string();
    let cases = [
        (stub_json(0, None), "report_missing:"),
        (stub_json(0, Some("not json")), "report_invalid:"),
        (stub_json(0, Some(&wrong_schema)), "report_invalid:"),
        (
            json!({"exit": 0, "sleep": 30, "report": null}).to_string(),
            "checker_not_run:",
        ),
    ];
    for (stub, token) in cases {
        let (dir, base) = repo_with_checker(&stub, true);
        let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
        let checker = applied(dir.path(), &base, None);
        let timeout = if token == "checker_not_run:" {
            Duration::from_secs(3)
        } else {
            TIMEOUT
        };

        let ev = run_health_check(
            RepoId::new(),
            dir.path(),
            &base,
            &head,
            &checker,
            BUNDLED,
            timeout,
        )
        .evidence;

        assert_eq!(ev.verdict, HealthVerdict::Error, "{token}: {ev:?}");
        let reason = ev.reason.clone().unwrap_or_default();
        assert!(reason.starts_with(token), "{token}: {reason}");
        assert!(!ev.reviewable);
    }
}

/// 不変条件: base に検査器の無い repo は、適用判定が同梱の検査器を返し、gate はそれ（release の `repo-health/`。base から
/// 出したときと同じ配置）で評価する。同梱の版は CHECKER_TREE から計算し、同じ commit の検査器の base の版と同じ値。
/// 渡した同梱の検査器と違う版の CheckerId は Error。
#[test]
fn health_evidence_gate_bundled_checker() {
    let text = report(
        "pass",
        0,
        vec![],
        json!({"version": "1", "source": "builtin", "changed": false, "files": []}),
        vec![],
    );
    let (source, source_base) = repo_with_checker(&stub_json(0, Some(&text)), true);
    let bundle_root = tempfile::tempdir().unwrap();
    let bundle = bundle_root.path().join("repo-health");
    for (path, body) in checker_files(&stub_json(0, Some(&text)), false) {
        write(&bundle, path, &body);
    }
    let tree = sh_git(
        source.path(),
        &["rev-parse", &format!("{source_base}:{CHECKER_DIR}")],
    );
    let blob = sh_git(
        source.path(),
        &["rev-parse", &format!("{source_base}:{SOURCE_SIZE_REPORT}")],
    );
    write(
        &bundle,
        CHECKER_TREE_FILE,
        &format!("repo-health {tree}\nsource-size-report {blob}\n"),
    );
    let configured = configure_bundled_checker(&bundle).expect("configure");
    assert_eq!(bundled_checker(), Some(configured));
    assert_eq!(configured.id.source, CheckerSource::Bundled);
    let same_commit = base_checker(&source_base, |args| git_in(source.path(), args))
        .expect("base checker")
        .expect("present");
    assert_eq!(configured.id.sha256, same_commit.sha256);

    let dir = init_repo();
    let base = sh_git(dir.path(), &["rev-parse", "HEAD"]);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let checker = applied(dir.path(), &base, Some(&configured.id));
    assert_eq!(checker, configured.id);

    let ev = run_health_check(
        RepoId::new(),
        dir.path(),
        &base,
        &head,
        &checker,
        Some(configured),
        TIMEOUT,
    )
    .evidence;
    assert_eq!(ev.verdict, HealthVerdict::Pass, "{ev:?}");
    assert_eq!(ev.checker, Some(configured.id.clone()));
    assert_eq!(ev.report_sha256, sha256_of(&text));

    let other = CheckerId {
        source: CheckerSource::Bundled,
        sha256: "0".repeat(64),
    };
    let ev = run_health_check(
        RepoId::new(),
        dir.path(),
        &base,
        &head,
        &other,
        Some(configured),
        TIMEOUT,
    )
    .evidence;
    assert_eq!(ev.verdict, HealthVerdict::Error);
    assert!(
        ev.reason
            .as_deref()
            .unwrap_or("")
            .starts_with("checker_mismatch:"),
        "{ev:?}"
    );
}

/// 不変条件: base に profile の dir が無い repo の policy の指紋は、profile の tree の代わりに builtin を入れた値。
#[test]
fn health_evidence_gate_no_profile_policy_is_builtin() {
    let policy = json!({"version": "1", "source": "builtin", "changed": false, "files": []});
    let text = report("pass", 0, vec![], policy, vec![]);
    let (dir, base) = repo_with_checker(&stub_json(0, Some(&text)), false);
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let checker = applied(dir.path(), &base, None);

    let ev = run_health_check(
        RepoId::new(),
        dir.path(),
        &base,
        &head,
        &checker,
        BUNDLED,
        TIMEOUT,
    )
    .evidence;

    assert_eq!(ev.verdict, HealthVerdict::Pass, "{ev:?}");
    let identity = |profiles_tree| PolicyIdentity {
        policy_version: "1",
        policy_source: "builtin",
        profile_resolved: "celeris",
        profile_source: "repo",
        profiles_tree,
        base_blobs: &[],
    };
    assert_eq!(ev.policy_sha256, policy_sha256(&identity(None)));
    assert_ne!(
        ev.policy_sha256,
        policy_sha256(&identity(Some("builtin-tree")))
    );
}

/// 不変条件: 候補（head）が検査器・stub を書き換えて合格を出すようにしても、gate は base の検査器で評価する（J3）。
#[test]
fn health_evidence_gate_ignores_candidate_checker() {
    let failing = report(
        "fail",
        1,
        vec![finding("size.x", 1, "error", "new", false)],
        base_policy(false),
        vec![rule("size.x", "fail", "")],
    );
    let passing = report("pass", 0, vec![], base_policy(false), vec![]);
    let (dir, base) = repo_with_checker(&stub_json(1, Some(&failing)), true);
    let head = commit(
        dir.path(),
        &[
            (STUB_JSON, &stub_json(0, Some(&passing))),
            (CHECKER_SCRIPT, "import sys\nsys.exit(0)\n"),
            (ENGINE_PATH, "def is_fail_grade(f):\n    return False\n"),
            (SRC, "fn b() {}\n"),
        ],
        "weaken the checker",
    );
    let checker = applied(dir.path(), &base, None);

    let ev = run_health_check(
        RepoId::new(),
        dir.path(),
        &base,
        &head,
        &checker,
        BUNDLED,
        TIMEOUT,
    )
    .evidence;

    assert_eq!(ev.verdict, HealthVerdict::Fail, "{ev:?}");
    assert_eq!(ev.report_sha256, sha256_of(&failing));
    assert_eq!(ev.blocking, vec![format!("size.x:{:016x}", 1)]);
}

#[path = "entry_tests.rs"]
mod entry;

#[path = "review_item_tests.rs"]
mod review_item;

#[path = "reuse_tests.rs"]
mod reuse;
