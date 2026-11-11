//! D4・I5 の材料作りの単体試験。repo health は一時 git repo に commit した stub の `repo_health.py` で代える。

use super::*;

use crate::health_review::gate::REPORT_SCHEMA;
use crate::health_review::policy::REVIEW_POLICY_SCHEMA;
use task_core::SqliteStore;
use task_core::required_checks::definition_digest;
use task_core::required_checks::health::CHECKER_SCRIPT;

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

fn git_result(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    git_result(dir, args).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
}

/// stub の検査器（`--print-review-policy` は方針の `source`、report の mode は `log` に 1 行足して `report` を書く。
/// `report` が無ければ exit 2）を `dir` に書く。
fn write_stub(dir: &Path, source: &str, report: Option<&serde_json::Value>, log: &Path) {
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
    let git = |args: &[&str]| git_in(dir, args);
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    write_stub(dir, source, report.as_ref(), log);
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
        repo_id: task_core::RepoId::new().to_string(),
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
        policy_only: false,
        candidates: vec![candidate.clone()],
        skipped: vec![("repo-remote".into(), "remote workspace".into())],
        out_dir: out.to_path_buf(),
        timeout: HEALTH_INPUT_TIMEOUT,
        evaluated: Default::default(),
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

/// candidate の event（`HealthReviewInputBuilt` の `definition_sha256` と `input`、candidate の
/// `HealthReviewInputSkipped` の数）。
fn built_and_skipped(
    store: &Arc<dyn TaskStore>,
    task: TaskId,
    c: &CandidateRepo,
) -> (Vec<(String, HealthRepoInput)>, usize) {
    let mut built = Vec::new();
    let mut skipped = 0;
    for (_, e) in store.events_for(task).unwrap() {
        match e {
            Event::HealthReviewInputBuilt {
                repo_id,
                definition_sha256,
                input,
                ..
            } if repo_id == c.repo_id => {
                built.push((definition_sha256, serde_json::from_value(input).unwrap()));
            }
            Event::HealthReviewInputSkipped { repo_id, .. } if repo_id == c.repo_id => skipped += 1,
            _ => {}
        }
    }
    (built, skipped)
}

/// base の commit の検査器から作る再利用の鍵（R2: `definition_digest(&[], Some(&checker))`）。
fn base_definition(c: &CandidateRepo) -> String {
    let id = task_core::required_checks::health::base_checker(&c.target_sha, |args| {
        git_result(&c.dir, args)
    })
    .unwrap()
    .expect("the base has a checker");
    definition_digest(&[], Some(&id))
}

/// 不変条件（ADR 2026-11-11-health-generic-review-input R1・R2）: review 方針が `Absent` の repo の材料は `Present` と
/// 同じ構築で作り、`diff`・`findings`・`policy` を持ち、`owner_rows`・`invariants` だけが空。`HealthReviewInputBuilt`
/// は candidate ごとに 1 回で、鍵の 4 つ目は選んだ検査器の `definition_digest`（方針の `definition_sha256` ではない）。
/// report は経路共通の評価（`evaluated`）と共有し、同じ candidate の 2 回目は検査器を起こさない。
/// 既存の試験で足りない理由: 旧い `health_review_input_policy_absent_is_skipped` は `Absent` を
/// `HealthReviewInputSkipped` に固定していた。材料の試験（`built_once_per_candidate`）は方針が `Present` だけを見る。
#[test]
fn health_review_input_policy_absent_builds_material_without_policy_values() {
    let scratch = tempfile::tempdir().unwrap();
    let log = scratch.path().join("runs.log");
    let (_repo, candidate) = stub_repo(
        "absent",
        Some(report(true, vec![finding("new", false, 1)])),
        &log,
    );
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut w = work(&store, "review-1", &candidate, scratch.path());
    let first = build_health_input(&w).expect("absent repos get material");
    assert_eq!(log_lines(&log), 1);
    assert_eq!(
        w.evaluated.lock().unwrap().len(),
        1,
        "the report is shared with the health item"
    );
    w.review_run = "review-2".into();
    assert_eq!(build_health_input(&w).as_ref(), Some(&first));
    assert_eq!(log_lines(&log), 1, "the recorded material is reused");
    let repo = &first.repos[0];
    assert_eq!(
        (repo.target_sha.as_str(), repo.reviewed_sha.as_str()),
        (
            candidate.target_sha.as_str(),
            candidate.reviewed_sha.as_str()
        )
    );
    assert_eq!(
        (repo.diff.files, repo.diff.paths[0].path.as_str()),
        (1, "src/a.rs")
    );
    assert_eq!(repo.findings.new.len(), 1);
    assert_eq!(
        repo.findings.new[0].path.as_deref(),
        Some("crates/x/src/f1.rs")
    );
    assert!(repo.policy.changed);
    assert_eq!(
        repo.policy.files,
        vec!["scripts/dev/repo-health/profiles/celeris.toml".to_string()]
    );
    assert!(
        repo.owner_rows.is_empty(),
        "the base map is not used without [review]"
    );
    assert!(repo.invariants.is_empty());
    let (built, skipped) = built_and_skipped(&store, w.task_id, &candidate);
    assert_eq!(skipped, 0);
    assert_eq!(built.len(), 1);
    assert_eq!(built[0].0, base_definition(&candidate));
    assert_ne!(built[0].0, DIGEST);
    assert_eq!(&built[0].1, repo);
}

/// 不変条件（R2.6）: 材料の再利用は `(repo, target, reviewed, 検査器の definition_digest)` に束縛する。candidate・
/// base・base の方針（`Absent` → `Present` で `owner_rows`・`invariants` が入る）のどれかが変われば作り直し、同じ
/// candidate のやり直しでは検査器を起こさない。既存の試験で足りない理由: `built_once_per_candidate` は同じ candidate の
/// 再利用だけを見て、candidate・base・方針が変わったときの失効を見ない。
#[test]
fn health_review_input_reuse_is_bound_to_candidate_base_and_policy() {
    let scratch = tempfile::tempdir().unwrap();
    let log = scratch.path().join("runs.log");
    let mut rep = report(true, vec![finding("new", false, 1)]);
    rep["policy"]["changed"] = false.into();
    let (repo, first) = stub_repo("absent", Some(rep.clone()), &log);
    let dir = repo.path();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut w = work(&store, "review-1", &first, scratch.path());
    let build = |w: &HealthInputWork| build_health_input(w).expect("material").repos.remove(0);
    let a = build(&w);
    assert_eq!(log_lines(&log), 1);

    // candidate が変わる（rework の commit）→ 作り直す。
    std::fs::write(dir.join("src/b.rs"), "fn b() {}\n").unwrap();
    git_in(dir, &["add", "-A"]);
    git_in(dir, &["commit", "-q", "-m", "rework"]);
    let reworked = CandidateRepo {
        reviewed_sha: git_in(dir, &["rev-parse", "HEAD"]),
        ..first.clone()
    };
    w.candidates = vec![reworked.clone()];
    let b = build(&w);
    assert_eq!(log_lines(&log), 2, "a new candidate is evaluated again");
    assert_eq!(
        (b.reviewed_sha.as_str(), b.diff.files),
        (reworked.reviewed_sha.as_str(), 2)
    );
    assert_eq!(build(&w), b);
    assert_eq!(
        log_lines(&log),
        2,
        "a retry of the same candidate reuses the material"
    );

    // base の方針が変わる（main の profile に [review] が入る）→ target が変わり作り直す。
    git_in(dir, &["checkout", "-q", "main"]);
    write_stub(dir, "base", Some(&rep), &log);
    git_in(dir, &["add", "-A"]);
    git_in(dir, &["commit", "-q", "-m", "review policy"]);
    let moved = CandidateRepo {
        target_sha: git_in(dir, &["rev-parse", "HEAD"]),
        ..reworked.clone()
    };
    git_in(dir, &["checkout", "-q", "cand"]);
    w.candidates = vec![moved.clone()];
    let c = build(&w);
    assert_eq!(log_lines(&log), 3, "a new base is evaluated again");
    assert_eq!(c.target_sha, moved.target_sha);
    assert_eq!(c.owner_rows.len(), 1, "{c:?}");
    assert_eq!(
        c.invariants.get("events-append-only").map(String::as_str),
        Some("CLAUDE.md")
    );
    assert!(a.owner_rows.is_empty() && b.invariants.is_empty());

    let (built, _) = built_and_skipped(&store, w.task_id, &first);
    let keys: Vec<_> = built
        .iter()
        .map(|(d, i)| (i.target_sha.clone(), i.reviewed_sha.clone(), d.clone()))
        .collect();
    assert_eq!(
        keys,
        vec![
            (
                first.target_sha.clone(),
                first.reviewed_sha.clone(),
                base_definition(&first)
            ),
            (
                reworked.target_sha.clone(),
                reworked.reviewed_sha.clone(),
                base_definition(&reworked)
            ),
            (
                moved.target_sha.clone(),
                moved.reviewed_sha.clone(),
                base_definition(&moved)
            ),
        ]
    );
    assert_ne!(keys[1].2, keys[2].2, "the checker of the new base differs");
}

/// 不変条件（R2.5・R3.3）: 方針が `Absent` の repo でも report を作れなければ、`findings.verdict = "error"`・
/// policy の状態が不明の材料を `HealthReviewInputBuilt` に記録して reviewer に渡し、再利用しない（次の review で
/// 検査器を起こし直す）。既存の試験で足りない理由: `failed_report_is_recorded_as_unknown` は方針が `Present` だけで、
/// `Absent` の故障は旧い経路（`HealthReviewInputSkipped`）を通っていた。
#[test]
fn health_review_input_policy_absent_failed_report_is_not_reused() {
    let scratch = tempfile::tempdir().unwrap();
    let log = scratch.path().join("runs.log");
    let (_repo, candidate) = stub_repo("absent", None, &log);
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let w = work(&store, "review-1", &candidate, scratch.path());
    for n in 1..=2 {
        let input = build_health_input(&w).expect("material with an unknown policy");
        let repo = &input.repos[0];
        assert_eq!(
            (repo.findings.verdict.as_str(), repo.findings.reviewable),
            ("error", false)
        );
        assert_eq!(repo.policy.source, POLICY_UNKNOWN);
        assert_eq!(repo.diff.files, 1);
        assert_eq!(log_lines(&log), n, "an unknown report is not reused");
    }
    let (built, skipped) = built_and_skipped(&store, w.task_id, &candidate);
    assert_eq!((built.len(), skipped), (2, 0));
    assert!(built.iter().all(|(_, i)| i.policy.source == POLICY_UNKNOWN));
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

/// base に検査器の無い一時 git repo（Celeris でない repo）: main に README、branch `cand` に 1 commit。
fn plain_repo() -> (tempfile::TempDir, CandidateRepo) {
    let repo = tempfile::tempdir().unwrap();
    let dir = repo.path();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    std::fs::write(dir.join("README.md"), "base\n").unwrap();
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
        repo_id: task_core::RepoId::new().to_string(),
        dir: dir.to_path_buf(),
        target_sha,
        reviewed_sha,
    };
    (repo, candidate)
}

/// `HealthReviewInputSkipped` の理由と policy の状態。
fn skips(store: &Arc<dyn TaskStore>, task: TaskId) -> Vec<(String, Option<HealthPolicyState>)> {
    store
        .events_for(task)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::HealthReviewInputSkipped { reason, policy, .. }
                if reason != "remote workspace" =>
            {
                Some((reason, policy.map(|p| serde_json::from_value(p).unwrap())))
            }
            _ => None,
        })
        .collect()
}

/// health_enforce_g2_（ADR 2026-11-08-health-enforcement-loop D1.1・D1.3・J3）: base に検査器の無い repo では、G2 の
/// 材料（reviewer あり）と policy の状態（reviewer なし）を、呼び出し側が渡した同梱の検査器で作る。材料の report は
/// 経路共通の評価（`gate::check_with`）が読んだもので、同じ candidate で検査器（report）は 1 回だけ起きる。同梱も
/// 無ければ材料を作らず、policy の状態を不明（`checker_missing`）で記録する（合格にしない）。以前は base に検査器が
/// 無いと何も作らず何も記録しなかった（`NO_CHECKER`）ので、既存の試験はこの経路を通らない。
#[test]
fn health_enforce_g2_bundled_checker_builds_material_and_policy_state() {
    let scratch = tempfile::tempdir().unwrap();
    let log = scratch.path().join("runs.log");
    let mut rep = report(true, vec![finding("new", false, 1)]);
    rep["policy"]["changed"] = false.into();
    let (stub, _) = stub_repo("base", Some(rep), &log);
    let bundle = tempfile::tempdir().unwrap();
    let script = bundle.path().join(CHECKER_SCRIPT);
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::copy(stub.path().join(CHECKER_SCRIPT), &script).unwrap();
    let bundled = BundledChecker {
        dir: bundle.path().to_path_buf(),
        id: task_core::required_checks::health::CheckerId {
            source: task_core::required_checks::health::CheckerSource::Bundled,
            sha256: "b".repeat(64),
        },
    };
    let (_repo, candidate) = plain_repo();

    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let w = work(
        &store,
        "review-1",
        &candidate,
        &scratch.path().join("material"),
    );
    let built =
        build_health_input_with(&w, Some(&bundled)).expect("material from the bundled checker");
    assert_eq!(log_lines(&log), 1, "the checker ran once for the candidate");
    let repo = &built.repos[0];
    assert_eq!(
        (repo.diff.files, repo.findings.new.len()),
        (1, 1),
        "{repo:?}"
    );
    assert_eq!(
        (repo.policy.source.as_str(), repo.policy.changed),
        ("base", false)
    );
    assert_eq!(build_health_input_with(&w, Some(&bundled)), Some(built));
    assert_eq!(log_lines(&log), 1, "the recorded material is reused");

    let only: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut w = work(
        &only,
        "worker",
        &candidate,
        &scratch.path().join("policy-only"),
    );
    w.policy_only = true;
    assert_eq!(build_health_input_with(&w, Some(&bundled)), None);
    assert_eq!(
        log_lines(&log),
        2,
        "one report for the policy-only candidate"
    );
    let recorded = skips(&only, w.task_id);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].0, NO_REVIEWER);
    assert_eq!(
        recorded[0]
            .1
            .as_ref()
            .map(|p| (p.source.as_str(), p.changed)),
        Some(("base", false))
    );

    for policy_only in [false, true] {
        let none: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let mut w = work(&none, "review-1", &candidate, &scratch.path().join("none"));
        w.policy_only = policy_only;
        assert_eq!(build_health_input_with(&w, None), None);
        let recorded = skips(&none, w.task_id);
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert!(
            recorded[0]
                .0
                .contains(&format!("{CHECKER_UNAVAILABLE}: checker_missing")),
            "{recorded:?}"
        );
        assert_eq!(
            recorded[0].1.as_ref().map(|p| p.source.as_str()),
            Some(POLICY_UNKNOWN)
        );
    }
    assert_eq!(log_lines(&log), 2, "no checker runs without a checker");
}

/// 不変条件（R2.2・R2.6・R3.2）: base に検査器の無い repo（同梱の検査器、方針は `Absent`）の材料の鍵は同梱の検査器の
/// `definition_digest` で、同じ版なら再利用し、同梱の検査器の版が変われば同じ candidate でも作り直す。適用判定が
/// `Unavailable`（同梱も無い）・方針を読めない repo は材料を作らず、不明の policy を `HealthReviewInputSkipped` に残す。
/// 既存の試験で足りない理由: `health_enforce_g2_bundled_checker_builds_material_and_policy_state` は方針が `Present`
/// で同じ版の再利用だけを見て、検査器の版が変わったときの失効と `Absent` の材料を見ない。
#[test]
fn health_review_input_reuse_is_bound_to_the_checker_version() {
    let scratch = tempfile::tempdir().unwrap();
    let log = scratch.path().join("runs.log");
    let bundle = tempfile::tempdir().unwrap();
    write_stub(
        bundle.path(),
        "absent",
        Some(&report(true, vec![finding("new", false, 1)])),
        &log,
    );
    let bundled = |sha: char| BundledChecker {
        dir: bundle.path().to_path_buf(),
        id: task_core::required_checks::health::CheckerId {
            source: task_core::required_checks::health::CheckerSource::Bundled,
            sha256: sha.to_string().repeat(64),
        },
    };
    let (_repo, candidate) = plain_repo();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let w = work(&store, "review-1", &candidate, scratch.path());
    let (old, new) = (bundled('b'), bundled('c'));
    let first = build_health_input_with(&w, Some(&old)).expect("material");
    assert_eq!(
        build_health_input_with(&w, Some(&old)).as_ref(),
        Some(&first)
    );
    assert_eq!(log_lines(&log), 1, "the same checker reuses the material");
    let second = build_health_input_with(&w, Some(&new)).expect("material");
    assert_eq!(log_lines(&log), 2, "a new checker version builds again");
    assert_eq!(first, second);
    let repo = &second.repos[0];
    assert_eq!((repo.diff.files, repo.findings.new.len()), (1, 1));
    assert!(repo.owner_rows.is_empty() && repo.invariants.is_empty());
    let (built, skipped) = built_and_skipped(&store, w.task_id, &candidate);
    let keys: Vec<_> = built.iter().map(|(d, _)| d.clone()).collect();
    assert_eq!(
        keys,
        vec![
            definition_digest(&[], Some(&old.id)),
            definition_digest(&[], Some(&new.id)),
        ]
    );
    assert_eq!(skipped, 0);

    // 同梱も無い（Unavailable）と、方針を読めない検査器。
    let garbled = tempfile::tempdir().unwrap();
    write_stub(garbled.path(), "garbled", Some(&report(true, vec![])), &log);
    let unreadable = BundledChecker {
        dir: garbled.path().to_path_buf(),
        ..bundled('d')
    };
    for (checker, want) in [
        (None, CHECKER_UNAVAILABLE),
        (Some(&unreadable), "review 方針を読めない"),
    ] {
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let w = work(&store, "review-1", &candidate, scratch.path());
        assert_eq!(build_health_input_with(&w, checker), None);
        let recorded = skips(&store, w.task_id);
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert!(recorded[0].0.contains(want), "{recorded:?}");
        assert_eq!(
            recorded[0].1.as_ref().map(|p| p.source.as_str()),
            Some(POLICY_UNKNOWN)
        );
        assert!(
            built_and_skipped(&store, w.task_id, &candidate)
                .0
                .is_empty()
        );
    }
    assert_eq!(log_lines(&log), 2, "no report without a readable policy");
}
