//! D4・I5 の材料作りの単体試験。repo health は一時 git repo に commit した stub の `repo_health.py` で代える。

use super::*;

use crate::health_review::policy::{CHECKER_SCRIPT, REVIEW_POLICY_SCHEMA};
use task_core::SqliteStore;

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn finding(delta: &str, exception: bool, n: usize) -> serde_json::Value {
    serde_json::json!({
        "rule_id": "size.responsibility-growth",
        "fingerprint": format!("size.responsibility-growth:{n:016x}"),
        "severity": "error",
        "delta": delta,
        "exception": if exception { serde_json::json!({"reason": "r", "source": "profile"}) } else { serde_json::Value::Null },
        "evidence": [{"ref": "head"}, {"path": format!("crates/x/src/f{n}.rs")}],
    })
}

fn report(reviewable: bool, findings: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({
        "schema": REPORT_SCHEMA,
        "summary": {"verdict": "fail", "exit_code": 1, "reviewable": reviewable,
            "counts": {"findings": findings.len(), "fail_grade": 1, "new": 1, "worsened": 0, "existing": 0}},
        "findings": findings,
        "policy": {"version": "1", "source": "base", "changed": true, "files": [
            {"path": "scripts/dev/repo-health/profiles/celeris.toml", "base_blob": "a".repeat(40), "head_blob": "b".repeat(40)},
            {"path": "scripts/dev/check-health.sh", "base_blob": "c".repeat(40), "head_blob": "c".repeat(40)},
        ]},
    })
}

fn parse(value: serde_json::Value) -> RepoHealthReport {
    serde_json::from_value(value).unwrap()
}

/// `findings.new` は new・worsened で exception の無いものだけを 50 件まで。超えた数が `truncated`。
#[test]
fn health_review_input_findings_are_capped_and_truncated() {
    let mut findings: Vec<_> = (0..53).map(|n| finding("new", false, n)).collect();
    findings.push(finding("worsened", false, 60));
    findings.push(finding("existing", false, 61));
    findings.push(finding("new", true, 62));
    let (summary, policy) = summarize_report(parse(report(true, findings)));
    assert_eq!(summary.new.len(), MAX_FINDINGS);
    assert_eq!(summary.truncated, 4, "53 new + 1 worsened - 50");
    assert_eq!(summary.reason, None);
    assert_eq!(summary.new[0].path.as_deref(), Some("crates/x/src/f0.rs"));
    assert!(summary.new.iter().all(|f| f.delta == "new"));
    assert_eq!(policy.source, "base");
    assert!(policy.changed);
    assert_eq!(
        policy.files,
        vec!["scripts/dev/repo-health/profiles/celeris.toml".to_string()]
    );
}

/// `reviewable = false` の report は finding を材料にせず、理由を残す。
#[test]
fn health_review_input_unreviewable_report_keeps_no_findings() {
    let (summary, _) = summarize_report(parse(report(false, vec![finding("new", false, 1)])));
    assert!(!summary.reviewable);
    assert!(summary.new.is_empty());
    assert_eq!(summary.truncated, 0);
    assert!(summary.reason.unwrap().contains("reviewable"));
}

/// `paths` は 40 件まで、`files`・行数は全件。binary（`-`）は 0 行。
#[test]
fn health_review_input_diff_paths_are_capped() {
    let mut out: String = (0..45).map(|n| format!("2\t1\tsrc/f{n}.rs\0")).collect();
    out.push_str("-\t-\tlogo.png\0");
    let (stat, all) = read_numstat(&out);
    assert_eq!(stat.files, 46);
    assert_eq!((stat.insertions, stat.deletions), (90, 45));
    assert_eq!(stat.paths.len(), MAX_PATHS);
    assert_eq!(all.len(), 46);
    assert_eq!(all[45], "logo.png");
}

/// owner 地図の行は `…` の中の path（`{a,b}` の展開・親 dir・`foo.rs` の module dir）で変更 path に当てる。
#[test]
fn health_review_input_owner_rows_match_changed_paths() {
    let map = "\
| subsystem | crate | entry |
|---|---|---|
| foo | foo | `crates/foo/src/{lib.rs,bar.rs}` |
| dispatch | d | `crates/d/src/dispatcher.rs`（`Dispatcher::tick`） |
| docs | - | `notes/` |
| other | o | `crates/other/` |
not a row `crates/foo/src/lib.rs`
";
    let changed = [
        "crates/foo/src/lib.rs",
        "crates/d/src/dispatcher/review_spawn.rs",
        "notes/a.md",
    ]
    .map(String::from);
    let rows = owner_rows(map, &changed);
    let got: Vec<_> = rows.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(got, changed.iter().map(String::as_str).collect::<Vec<_>>());
    assert!(rows[0].row.contains("{lib.rs,bar.rs}"));
    let many: String = (0..50)
        .map(|n| format!("| r{n} | x | `notes/` |\n"))
        .collect();
    assert_eq!(owner_rows(&many, &changed).len(), MAX_OWNER_ROWS);
}

/// 一時 git repo: main に stub の検査器（`--print-review-policy` は方針、report の mode は `log` に 1 行足して
/// `report` を書く）と、branch `cand` に 1 commit。
fn stub_repo(
    source: &str,
    report: Option<serde_json::Value>,
    log: &Path,
) -> (tempfile::TempDir, CandidateRepo) {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    let git = |args: &[&str]| {
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
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    let policy = serde_json::json!({
        "schema": REVIEW_POLICY_SCHEMA, "source": source, "owner_map": "owners/map.md",
        "high_risk": [], "invariants": {"events-append-only": "CLAUDE.md"}, "definition_sha256": DIGEST,
    });
    let report = report.map(|r| r.to_string()).unwrap_or_default();
    let script = format!(
        "import json, sys\nargs = sys.argv[1:]\nif '--print-review-policy' in args:\n    sys.stdout.write({policy:?})\n    sys.exit(0)\nopen({log:?}, 'a').write(' '.join(args) + '\\n')\nreport = {report:?}\nif not report:\n    sys.exit(2)\nopen(args[args.index('--out') + 1], 'w').write(report)\n",
        policy = policy.to_string(),
        log = log.display().to_string(),
    );
    let path = dir.join(CHECKER_SCRIPT);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, script).unwrap();
    std::fs::create_dir_all(dir.join("owners")).unwrap();
    std::fs::write(dir.join("owners/map.md"), "| a | `src/` |\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "base"]);
    let target_sha = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-q", "-b", "cand"]);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "fn a() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "cand"]);
    let reviewed_sha = git(&["rev-parse", "HEAD"]);
    let candidate = CandidateRepo {
        repo_id: "repo-1".into(),
        dir: dir.to_path_buf(),
        target_sha,
        reviewed_sha,
    };
    (repo, candidate)
}

fn work(
    store: &Arc<dyn TaskStore>,
    review_run: &str,
    candidate: &CandidateRepo,
    out: &Path,
) -> HealthInputWork {
    HealthInputWork {
        store: store.clone(),
        task_id: TaskId::new(),
        review_run: review_run.into(),
        candidates: vec![candidate.clone()],
        skipped: vec![("repo-remote".into(), "remote workspace".into())],
        out_dir: out.to_path_buf(),
        timeout: HEALTH_INPUT_TIMEOUT,
    }
}

fn log_lines(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

/// 同じ candidate で 2 回作っても repo health は 1 回だけ走り、2 回目は event の `input` を使う。skip の理由も残る。
#[test]
fn health_review_input_built_once_per_candidate() {
    let scratch = tempfile::tempdir().unwrap();
    let log = scratch.path().join("runs.log");
    let mut rep = report(true, vec![finding("new", false, 1)]);
    rep["policy"]["changed"] = false.into();
    let (_repo, candidate) = stub_repo("base", Some(rep), &log);
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut first_work = work(&store, "review-1", &candidate, scratch.path());
    let task_id = first_work.task_id;
    let first = build_health_input(&first_work).unwrap();
    first_work.review_run = "review-2".into();
    let second = build_health_input(&first_work).unwrap();
    assert_eq!(first, second);
    assert_eq!(log_lines(&log), 1, "repo health ran once");
    let repo = &first.repos[0];
    assert_eq!(repo.diff.files, 1);
    assert_eq!(repo.owner_rows.len(), 1);
    assert_eq!(repo.findings.new.len(), 1);
    assert_eq!(
        repo.invariants
            .get("events-append-only")
            .map(String::as_str),
        Some("CLAUDE.md")
    );
    let events: Vec<Event> = store
        .events_for(task_id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    let built = events.iter().filter(|e| matches!(e, Event::HealthReviewInputBuilt { review_run, .. } if review_run == "review-1")).count();
    let skipped = events.iter().filter(|e| matches!(e, Event::HealthReviewInputSkipped { reason, .. } if reason == "remote workspace")).count();
    assert_eq!((built, skipped), (1, 2), "{events:?}");
}

/// 方針が absent の repo は作らず `HealthReviewInputSkipped` に理由を残す。policy の関門のため、repo health は
/// 1 回走り、candidate の policy の状態を同じ event に残す（同じ candidate の 2 回目は記録を使う）。
#[test]
fn health_review_input_policy_absent_is_skipped() {
    let scratch = tempfile::tempdir().unwrap();
    let log = scratch.path().join("runs.log");
    let (_repo, candidate) = stub_repo("absent", Some(report(true, vec![])), &log);
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let w = work(&store, "review-1", &candidate, scratch.path());
    assert_eq!(build_health_input(&w), None);
    assert_eq!(build_health_input(&w), None);
    assert_eq!(
        log_lines(&log),
        1,
        "the policy state is decided once per candidate"
    );
    let skipped: Vec<(String, String, Option<HealthPolicyState>)> = store
        .events_for(w.task_id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::HealthReviewInputSkipped {
                repo_id,
                reason,
                reviewed_sha,
                policy,
                ..
            } if repo_id == "repo-1" => Some((
                reason,
                reviewed_sha,
                policy.map(|p| serde_json::from_value(p).unwrap()),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(skipped.len(), 2);
    let (reason, reviewed, policy) = &skipped[0];
    assert_eq!(reason, POLICY_ABSENT);
    assert_eq!(reviewed, &candidate.reviewed_sha);
    let policy = policy.as_ref().unwrap();
    assert!(policy.changed);
    assert_eq!(
        policy.files,
        vec!["scripts/dev/repo-health/profiles/celeris.toml".to_string()]
    );
    assert_eq!(skipped[0], skipped[1]);
}

/// report を書かない repo health（exit 2）は理由付きの空の要約を渡し、policy の状態を不明（`unavailable`）として
/// 記録する（policy の関門のため）。不明の記録は再利用せず、次の review で走らせ直す（D4.3）。
#[test]
fn health_review_input_failed_report_is_recorded_as_unknown() {
    let scratch = tempfile::tempdir().unwrap();
    let log = scratch.path().join("runs.log");
    let (_repo, candidate) = stub_repo("base", None, &log);
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let w = work(&store, "review-1", &candidate, scratch.path());
    let input = build_health_input(&w).unwrap();
    let findings = &input.repos[0].findings;
    assert_eq!(
        (findings.verdict.as_str(), findings.reviewable),
        ("error", false)
    );
    assert!(findings.reason.as_deref().unwrap().contains("report"));
    assert_eq!(input.repos[0].policy.source, POLICY_UNKNOWN);
    build_health_input(&w).unwrap();
    assert_eq!(log_lines(&log), 2, "not reused");
    let built: Vec<HealthRepoInput> = store
        .events_for(w.task_id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::HealthReviewInputBuilt { input, .. } => serde_json::from_value(input).ok(),
            _ => None,
        })
        .collect();
    assert_eq!(built.len(), 2);
    assert!(built.iter().all(|i| i.policy.source == POLICY_UNKNOWN));
    assert_eq!(built[0].reviewed_sha, candidate.reviewed_sha);
}
