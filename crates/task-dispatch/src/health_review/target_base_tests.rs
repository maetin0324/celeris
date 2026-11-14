//! ADR 2026-11-13-health-base-merged-target: WU の check と段の統合の health の base（[`target_base::health_base`]）の試験。
//!
//! 全試験は git の実 repo と、base の commit に置いた stub の検査器を使う（時計・負荷・外部ネットワーク・LLM に依らない）。
//! stub は受け取った `--base`・`--head` を repo の隣の log に書き、`git diff --name-only base head` から次を fail 級の
//! finding にする: policy の file（`scripts/dev/repo-health/`）の変更 = `policy.changed`、`*.big` の file = `size.file`、
//! `*.grow` の file で head の行数が base より多い = `size.grow`（衝突の印を含む base では見えない悪化を、base = T の
//! 2 回目が見つけることの試験に使う）。
//!
//! 既存の試験（`dispatcher/tests/health_enforce.rs` の流れ、旧 `health_enforce_phase_base_*`）は ff の取り込みと統合の前の
//! HEAD だけを作り、WU の branch が target を merge する場合・衝突する merge・merge commit の中の変更を作らない。そのため
//! R4 の sync-policy の形の不具合（取り込んだ target の変更が fail 級になる、または解き方が検査から外れる）を検出できない。
//! 各試験の doc comment に、守る不変条件と、既存の試験で検出できない理由を書く。WU の check と段の統合の両方（`phase`
//! 0/1）を同じ形で通す試験は、2 つの経路が同じ規則を使うこと（1 関数）も確かめる。

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
def lines(rev, path):
    r = subprocess.run(["git", "-C", repo, "show", "%s:%s" % (rev, path)], capture_output=True, text=True)
    return len(r.stdout.splitlines()) if r.returncode == 0 else 0
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
grow = [n for n in names if n.endswith(".grow") and lines(head, n) > lines(base, n)]
if grow:
    findings.append(finding("size.grow", 3, grow))
code = 1 if findings else 0
rules = [{"id": r, "required": True, "status": "fail" if any(f["rule_id"] == r for f in findings) else "pass",
    "reason": ""} for r in ("policy.changed", "size.file", "size.grow")]
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

const POLICY_LANDED: &str = "scripts/dev/repo-health/rules/landed.txt";
const POLICY_SHARED: &str = "scripts/dev/repo-health/rules/shared.txt";

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

/// 失敗してよい git（衝突する merge）。
fn git_any(dir: &Path, args: &[&str]) {
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
}

fn write(repo: &Path, path: &str, body: &str) {
    let file = repo.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, body).unwrap();
}

/// `files` を書いて 1 commit にし、HEAD の sha を返す。
fn commit_files(repo: &Path, files: &[(&str, &str)]) -> String {
    for (path, body) in files {
        write(repo, path, body);
    }
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", files[0].0]);
    git(repo, &["rev-parse", "HEAD"])
}

/// 試験の repo。`main`（review target）に stub の検査器を commit し、Task の branch `task` に Task 自身の commit を 1 つ
/// 足した点を start（WU の `base_commit`・段の基点 P）にする。この commit は `shared.*` も変える（main が変えると start と
/// T の merge が衝突する。衝突の 2 回目の検査は start 側の変更も数えるので、policy の file は必要な試験だけが足す）。WU の branch `wu` は start から切って checkout している。
/// start が main の祖先でないので、main が進むと start と T の merge は ff にならない。
struct Fx {
    _root: tempfile::TempDir,
    repo: PathBuf,
    start: String,
}

fn setup() -> Fx {
    setup_with(&[])
}

/// [`setup`] で、start の commit が `task_files` も変える（main が同じ file を変えると衝突する）。
fn setup_with(task_files: &[(&str, &str)]) -> Fx {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    write(&repo, CHECKER_SCRIPT, STUB);
    write(&repo, "scripts/dev/repo-health/health/engine.py", ENGINE);
    write(&repo, "scripts/dev/repo-health/health/__init__.py", "");
    write(&repo, SOURCE_SIZE_REPORT, "# side\n");
    commit_files(
        &repo,
        &[
            ("README.md", "hello\n"),
            ("shared.txt", "base\n"),
            ("shared.grow", "base\n"),
            (POLICY_SHARED, "base\n"),
        ],
    );
    git(&repo, &["checkout", "-q", "-b", "task"]);
    let mut files = vec![
        ("task-only.txt", "task\n"),
        ("shared.txt", "task\n"),
        ("shared.grow", "task\n"),
    ];
    files.extend_from_slice(task_files);
    let start = commit_files(&repo, &files);
    git(&repo, &["checkout", "-q", "-b", "wu"]);
    Fx {
        _root: root,
        repo,
        start,
    }
}

impl Fx {
    /// `main` に commit を足し（承認済みの target の変更）、`wu` に戻って main の HEAD（= T）を返す。
    fn land(&self, files: &[(&str, &str)]) -> String {
        git(&self.repo, &["checkout", "-q", "main"]);
        let t = commit_files(&self.repo, files);
        git(&self.repo, &["checkout", "-q", "wu"]);
        t
    }

    fn wu_commit(&self, files: &[(&str, &str)]) {
        commit_files(&self.repo, files);
    }

    /// worker が `wu` で main を merge する。衝突しても `resolve` が作業ツリーを直し、merge commit にする。
    fn merge_main(&self, resolve: &dyn Fn(&Path)) {
        git_any(&self.repo, &["merge", "--no-commit", "--no-ff", "main"]);
        resolve(&self.repo);
        git(&self.repo, &["add", "-A"]);
        git(&self.repo, &["commit", "-q", "-m", "merge main"]);
    }

    fn args(&self) -> Vec<(String, String)> {
        let log = self.repo.parent().unwrap().join("checker-args.log");
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once(' '))
            .map(|(b, h)| (b.to_string(), h.to_string()))
            .collect()
    }

    fn health(&self, target: Result<Option<String>, String>) -> HealthTarget {
        HealthTarget {
            repo_id: health_repo_id(None),
            worktree: self.repo.clone(),
            start: Ok(self.start.clone()),
            target,
            head: "HEAD".to_string(),
            bundled: None,
        }
    }

    /// `phase = false` は WU の check（WU の作業ツリーの HEAD）、`true` は段の統合（`wu` を `task` に no-ff で統合した
    /// HEAD。start と target は統合の前に読む）。返すのは項目と評価した head。
    fn evaluate(
        &self,
        phase: bool,
        target: Result<Option<String>, String>,
    ) -> (HealthItem, String) {
        if !phase {
            let head = git(&self.repo, &["rev-parse", "HEAD"]);
            return (evaluate(&self.health(target)), head);
        }
        git(&self.repo, &["checkout", "-q", "task"]);
        let items = [(self.repo.clone(), None::<RepoId>, ())];
        let health = phase_target(Some(self.start.clone()), &items, target).expect("git repo");
        git(
            &self.repo,
            &["merge", "-q", "--no-ff", "-m", "integrate", "wu"],
        );
        let head = git(&self.repo, &["rev-parse", "HEAD"]);
        (evaluate(&health.at_head(&head)), head)
    }
}

fn main_target() -> Result<Option<String>, String> {
    Ok(Some("refs/heads/main".to_string()))
}

fn parents(repo: &Path, sha: &str) -> Vec<String> {
    git(repo, &["rev-list", "--parents", "-n", "1", sha])
        .split_whitespace()
        .skip(1)
        .map(str::to_string)
        .collect()
}

fn blob(repo: &Path, sha: &str, path: &str) -> String {
    git(repo, &["show", &format!("{sha}:{path}")])
}

fn assert_error(item: &HealthItem, repo: &Path, args_empty: bool) {
    assert!(!item.pass, "{item:?}");
    assert_eq!(item.exit, None, "{item:?}");
    assert_eq!(item.evidence.verdict, HealthVerdict::Error, "{item:?}");
    assert!(
        item.reason
            .starts_with("health_check_error: facts_unreadable:"),
        "{item:?}"
    );
    assert_eq!(
        check_error(HEALTH_CHECK_ID, item.pass, &item.reason),
        Some(item.reason.as_str())
    );
    assert!(stops_without_replan(&item.reason));
    if args_empty {
        let log = repo.parent().unwrap().join("checker-args.log");
        assert!(!log.exists(), "the checker must not run");
    }
}

/// 形 (1) と K5。不変条件: target から分かれた WU（または段の unit）が target を衝突なく merge しただけなら、取り込んだ
/// target の変更（policy の file を含む）は WU・段の変更として数えず合格になる。base は start と T の merge commit（親は
/// start、T の順）で、検査器は base = その commit・head = 評価した head を受け取る。同じ入力で base の sha は同じ（K5）。
/// 既存の試験が検出できない理由: 旧い規則は ff の取り込みだけを数えず、merge の取り込みは base = start のまま
/// `policy.changed` で落とす。同じ head を base = start で評価すると落ちることを同じ試験で確かめるので、規則を外すと
/// この試験が落ちる。
#[test]
fn health_base_clean_merge_of_target_passes() {
    for phase in [false, true] {
        let fx = setup();
        fx.wu_commit(&[("a.txt", "a\n")]);
        let t = fx.land(&[(POLICY_LANDED, "approved\n"), ("landed.txt", "x\n")]);
        fx.merge_main(&|_| {});

        let (item, head) = fx.evaluate(phase, main_target());
        assert!(item.pass, "phase={phase}: {item:?}");
        assert_eq!(item.evidence.verdict, HealthVerdict::Pass);
        let base = item.evidence.base_sha.clone();
        assert_ne!(base, fx.start);
        assert_ne!(base, t);
        assert_eq!(parents(&fx.repo, &base), vec![fx.start.clone(), t.clone()]);
        assert_eq!(fx.args(), vec![(base.clone(), head.clone())]);

        // 規則を使わず base = start で評価すると、取り込んだ policy の変更で落ちる。
        let old = evaluate_at(&fx.health(main_target()), &fx.start);
        assert!(!old.pass, "{old:?}");
        assert!(old.reason.contains("health:policy.changed:"), "{old:?}");

        // K5: 同じ入力から同じ base。
        let (again, _) = if phase {
            (evaluate(&fx.health(main_target()).at_head(&head)), head)
        } else {
            fx.evaluate(false, main_target())
        };
        assert_eq!(again.evidence.base_sha, base);
    }
}

/// 形 (2)。不変条件: 衝突した merge を worker が解いた場合（衝突した file が policy の file でない）、解き方は
/// `base..head` に残るが fail 級でなければ合格。1 回目の base は merge-tree の衝突の印を含む tree の commit、2 回目の base は
/// T で、検査器は 2 回呼ばれる（D3.1）。既存の試験が検出できない理由: 衝突する merge を作る試験が無く、`merge-tree` の
/// 出力（tree、衝突 path）の読みが実 repo で確かめられていない。
#[test]
fn health_base_resolved_conflict_passes_and_runs_against_target() {
    for phase in [false, true] {
        let fx = setup();
        fx.wu_commit(&[("shared.txt", "wu\n")]);
        let t = fx.land(&[("shared.txt", "main\n")]);
        fx.merge_main(&|repo| write(repo, "shared.txt", "resolved\n"));

        let (item, head) = fx.evaluate(phase, main_target());
        assert!(item.pass, "phase={phase}: {item:?}");
        let args = fx.args();
        assert_eq!(args.len(), 2, "{args:?}");
        let merged = args[0].0.clone();
        assert_eq!(args[1], (t.clone(), head));
        assert_ne!(merged, t);
        assert_eq!(parents(&fx.repo, &merged), vec![fx.start.clone(), t]);
        assert!(
            blob(&fx.repo, &merged, "shared.txt").contains("<<<<<<<"),
            "the first base keeps the conflict markers"
        );
    }
}

/// 形 (3)。不変条件（ADR D3）: 衝突を解かない（印を残したまま commit）、または解き方が target の版より大きい場合、
/// 1 回目（base = 印を含む commit）では差分が見えなくても、2 回目（base = T）で fail 級になり合格しない。target 側の
/// 内容で解けば合格。衝突した file が検査器の dir の file なら、target の版で解いても評価せず Unavailable（D3.5）で、
/// 人の判断なしには合格しない。既存の試験が検出できない理由: 1 回目の指標だけでは
/// 解き方の悪化を見逃すことを作る試験が無い。
#[test]
fn health_base_conflict_resolution_is_judged_against_target() {
    for phase in [false, true] {
        // 解かない: 印のまま commit。1 回目は base と head の file が同じで差分なし、2 回目が T より大きいので落ちる。
        let fx = setup();
        fx.wu_commit(&[("shared.grow", "wu\n")]);
        let t = fx.land(&[("shared.grow", "main\n")]);
        fx.merge_main(&|_| {});
        let (item, _) = fx.evaluate(phase, main_target());
        assert!(!item.pass, "unresolved phase={phase}: {item:?}");
        assert!(item.reason.contains("health:size.grow:"), "{item:?}");
        assert_eq!(item.evidence.base_sha, t, "the worse (2nd) result is kept");
        assert_eq!(fx.args().len(), 2);

        // 解き方が印より小さく T より大きい: 1 回目は通り、2 回目で落ちる。
        let fx = setup();
        fx.wu_commit(&[("shared.grow", "wu\n")]);
        let t = fx.land(&[("shared.grow", "main\n")]);
        fx.merge_main(&|repo| write(repo, "shared.grow", "main\nwu\nboth\nmore\n"));
        let (item, _) = fx.evaluate(phase, main_target());
        assert!(!item.pass, "larger phase={phase}: {item:?}");
        assert_eq!(item.evidence.base_sha, t);
        assert!(item.reason.contains("health:size.grow:"), "{item:?}");

        // target 側の内容で解く（非 policy の file）: 合格。
        let fx = setup();
        fx.wu_commit(&[("shared.grow", "wu\n")]);
        fx.land(&[("shared.grow", "main\n")]);
        fx.merge_main(&|repo| write(repo, "shared.grow", "main\n"));
        let (item, _) = fx.evaluate(phase, main_target());
        assert!(item.pass, "theirs phase={phase}: {item:?}");

        // 検査器の dir の policy の file の衝突は、target の版で解いても評価せず Unavailable（D3.5）。
        let fx = setup_with(&[(POLICY_SHARED, "task\n")]);
        fx.land(&[(POLICY_SHARED, "main\n")]);
        fx.merge_main(&|repo| write(repo, POLICY_SHARED, "main\n"));
        let (item, _) = fx.evaluate(phase, main_target());
        assert_error(&item, &fx.repo, true);
        assert!(item.reason.contains("rules/shared.txt"), "{item:?}");
    }
}

/// 形 (4)・K3。不変条件: target を取り込まない WU・段は、target が進んでいても base が start のまま（証跡の `base_sha` が
/// start）。target 側の policy の変更を WU の変更に数えず、前の段の変更も遡って数えない。案件の repo の参照が無い作業場所
/// （`Ok(None)`）も同じ。既存の試験が検出できない理由: 旧 5 件のうち 1 件だけが段の経路で、WU の経路には「取り込まない」
/// を固定する試験が無く、新しい規則が常に merge-base を使うように変わっても気付けない。
#[test]
fn health_base_without_intake_keeps_start() {
    for phase in [false, true] {
        for target in [main_target(), Ok(None)] {
            let fx = setup();
            fx.wu_commit(&[("a.txt", "a\n")]);
            fx.land(&[(POLICY_LANDED, "approved\n")]);
            let (item, head) = fx.evaluate(phase, target);
            assert!(item.pass, "phase={phase}: {item:?}");
            assert_eq!(item.evidence.base_sha, fx.start);
            assert_eq!(fx.args(), vec![(fx.start.clone(), head)]);
        }
    }
}

/// 形 (5)。不変条件（K2）: merge commit に worker が足した変更（clean な merge に足した fail 級の file、merge の後の
/// commit、衝突の解き方に混ぜた fail 級の file）はすべて `base..head` に残り、fail 級の指摘で落ちる。base の tree に
/// merge commit の tree を使う案（ADR D8 の d）だと、これらが差分から消えて合格してしまう。既存の試験が検出できない理由:
/// merge commit の中身を作る試験が無い。
#[test]
fn health_base_merge_commit_additions_fail() {
    for phase in [false, true] {
        // 衝突の無い merge に足した変更。
        let fx = setup();
        fx.wu_commit(&[("a.txt", "a\n")]);
        fx.land(&[(POLICY_LANDED, "approved\n")]);
        fx.merge_main(&|repo| write(repo, "data/huge.big", "x\n"));
        let (item, _) = fx.evaluate(phase, main_target());
        assert!(!item.pass, "in-merge phase={phase}: {item:?}");
        assert_eq!(item.exit, Some(1));
        assert!(item.reason.contains("health:size.file:"), "{item:?}");
        assert!(!item.reason.contains("policy.changed"), "{item:?}");
        assert_eq!(repair_buckets(&item.reason), vec!["health:size.file"]);

        // merge の後の commit。
        let fx = setup();
        fx.wu_commit(&[("a.txt", "a\n")]);
        fx.land(&[(POLICY_LANDED, "approved\n")]);
        fx.merge_main(&|_| {});
        fx.wu_commit(&[("data/after.big", "x\n")]);
        let (item, _) = fx.evaluate(phase, main_target());
        assert!(!item.pass, "after-merge phase={phase}: {item:?}");
        assert!(item.reason.contains("data/after.big"), "{item:?}");

        // 衝突を解くときに混ぜた変更。
        let fx = setup();
        fx.wu_commit(&[("shared.txt", "wu\n")]);
        fx.land(&[("shared.txt", "main\n")]);
        fx.merge_main(&|repo| {
            write(repo, "shared.txt", "resolved\n");
            write(repo, "data/in-conflict.big", "x\n");
        });
        let (item, _) = fx.evaluate(phase, main_target());
        assert!(!item.pass, "in-conflict phase={phase}: {item:?}");
        assert!(item.reason.contains("data/in-conflict.big"), "{item:?}");
    }
}

/// 形 (6)（段の統合に固有の部分）。不変条件（ADR D2.1・D2.3）: 段の基点 P が target の commit T の祖先なら（Task の branch を
/// target まで ff で進めた）base = T で、merge commit は作らない。base は統合の HEAD が決まった後に決まり、unit の branch が
/// target を merge した場合（R4 の sync-policy）も同じ規則に入る。この ff の場合は旧 `health_enforce_phase_base_*` の
/// 不変条件（ff の取り込みは数えない・worker の policy の変更は残る）を引き継ぐ。既存の試験が検出できない理由: 旧い試験は
/// 統合の前の HEAD から M を求めるので、統合の後に取り込まれた T は見えない。
#[test]
fn health_base_phase_fast_forward_intake_and_worker_policy_change() {
    // P = task（main の祖先）。task を main へ ff で進め、unit は P から切った branch。
    for worker_policy in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.email", "t@example.com"]);
        git(&repo, &["config", "user.name", "t"]);
        write(&repo, CHECKER_SCRIPT, STUB);
        write(&repo, "scripts/dev/repo-health/health/engine.py", ENGINE);
        write(&repo, "scripts/dev/repo-health/health/__init__.py", "");
        write(&repo, SOURCE_SIZE_REPORT, "# side\n");
        let start = commit_files(&repo, &[("README.md", "hello\n")]);
        git(&repo, &["checkout", "-q", "-b", "task"]);
        git(&repo, &["checkout", "-q", "-b", "wu"]);
        let body = if worker_policy {
            (POLICY_SHARED, "weaker\n")
        } else {
            ("a.txt", "a\n")
        };
        commit_files(&repo, &[body]);
        git(&repo, &["checkout", "-q", "main"]);
        let t = commit_files(&repo, &[(POLICY_LANDED, "approved\n")]);
        git(&repo, &["checkout", "-q", "task"]);
        git(&repo, &["merge", "-q", "--ff-only", "main"]);

        let items = [(repo.clone(), None::<RepoId>, ())];
        let health = phase_target(Some(start.clone()), &items, main_target()).unwrap();
        git(&repo, &["merge", "-q", "--no-ff", "-m", "integrate", "wu"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);
        let item = evaluate(&health.at_head(&head));
        assert_eq!(item.evidence.base_sha, t);
        let args = std::fs::read_to_string(root.path().join("checker-args.log")).unwrap();
        assert_eq!(args.trim(), format!("{t} {head}"));
        if worker_policy {
            assert!(!item.pass, "{item:?}");
            assert!(item.reason.contains("health:policy.changed:"), "{item:?}");
            assert!(item.reason.contains("rules/shared.txt"), "{item:?}");
            assert!(!item.reason.contains("rules/landed.txt"), "{item:?}");
        } else {
            assert!(item.pass, "{item:?}");
        }
    }
}

/// 形 (7)・K4。不変条件: target を解決できない（`Err`）・target の ref が無い・start を読めない・head を commit に解決
/// できない・merge-base が複数（criss-cross）・検査器の file の衝突のとき、検査器を起こさず Unavailable（verdict Error）に
/// し、合格にしない。`stops_without_replan` が真になり repair・replan に回らない。既存の試験が検出できない理由: 旧い試験は
/// target の `Err` と存在しない ref だけで、git 以外の失敗（複数の merge-base・検査器の衝突）と WU の経路を確かめない。
#[test]
fn health_base_unavailable_inputs_do_not_pass() {
    for phase in [false, true] {
        // target を解決できない。
        let fx = setup();
        fx.wu_commit(&[("a.txt", "a\n")]);
        let (item, _) = fx.evaluate(phase, Err("repo r の review target を解決できない".into()));
        assert_error(&item, &fx.repo, true);
        assert!(item.reason.contains("health_base:"), "{item:?}");

        // target の ref が無い（git が失敗）。
        let fx = setup();
        fx.wu_commit(&[("a.txt", "a\n")]);
        let missing = Ok(Some("refs/heads/no-such-branch".to_string()));
        let (item, _) = fx.evaluate(phase, missing);
        assert_error(&item, &fx.repo, true);

        // 検査器の file の衝突（両側が変えた）。
        let fx = setup_with(&[(CHECKER_SCRIPT, &format!("{STUB}# task\n"))]);
        fx.land(&[(CHECKER_SCRIPT, &format!("{STUB}# main\n"))]);
        fx.merge_main(&|repo| write(repo, CHECKER_SCRIPT, STUB));
        let (item, _) = fx.evaluate(phase, main_target());
        assert_error(&item, &fx.repo, true);
        assert!(item.reason.contains("検査器の file が衝突した"), "{item:?}");
        assert!(item.reason.contains("repo_health.py"), "{item:?}");
    }

    // start を読めない・head を解決できない（WU の経路の入力を直接作る）。
    let fx = setup();
    fx.wu_commit(&[("a.txt", "a\n")]);
    let mut health = fx.health(main_target());
    health.start = Err("start を読めない".to_string());
    assert_error(&evaluate(&health), &fx.repo, true);
    let mut health = fx.health(main_target());
    health.head = "0123456789012345678901234567890123456789".to_string();
    assert_error(&evaluate(&health), &fx.repo, true);

    // criss-cross: head と target の merge-base が 2 件で、どちらも start の祖先でない。
    let fx = setup();
    git(&fx.repo, &["checkout", "-q", "-b", "side", &fx.start]);
    let b1 = commit_files(&fx.repo, &[("b.txt", "b\n")]);
    git(&fx.repo, &["checkout", "-q", "wu"]);
    let a1 = commit_files(&fx.repo, &[("a.txt", "a\n")]);
    git(&fx.repo, &["branch", "wu-a1"]);
    git(
        &fx.repo,
        &["merge", "-q", "--no-ff", "-m", "wu takes b1", &b1],
    );
    git(&fx.repo, &["checkout", "-q", "side"]);
    git(
        &fx.repo,
        &["merge", "-q", "--no-ff", "-m", "side takes a1", &a1],
    );
    git(&fx.repo, &["checkout", "-q", "wu"]);
    let item = evaluate(&fx.health(Ok(Some("refs/heads/side".to_string()))));
    assert_error(&item, &fx.repo, true);
    assert!(item.reason.contains("merge-base"), "{item:?}");
}

/// 形 (8)・I3・K1。不変条件: 候補（worker の commit・worker の merge commit）が検査器や policy を弱めても、base 側の版が
/// 使われる。base の tree は start・T・git の merge だけで決まり、worker の merge commit の tree は入らない。worker が merge
/// commit で検査器を常に合格するものに差し替えても、実際に動くのは base の版（log が残る）で、差し替えは `policy.changed`
/// の fail 級になる。target が承認した policy の file を merge commit で戻しても同じ。既存の試験が検出できない理由: base に
/// worker の merge commit を使う実装（ADR D8 の d）は、差し替えが base と head の差分から消え、worker の検査器で合格する。
/// 既存の試験は merge commit を作らないのでこの不具合を検出できない。
#[test]
fn health_base_candidate_cannot_weaken_policy_or_checker() {
    for phase in [false, true] {
        // 検査器の差し替え。
        let fx = setup();
        fx.wu_commit(&[("a.txt", "a\n")]);
        fx.land(&[(POLICY_LANDED, "approved\n")]);
        fx.merge_main(&|repo| write(repo, CHECKER_SCRIPT, "import sys\nsys.exit(0)\n"));
        let (item, head) = fx.evaluate(phase, main_target());
        assert!(!item.pass, "checker phase={phase}: {item:?}");
        assert_eq!(item.evidence.verdict, HealthVerdict::Fail);
        assert!(item.reason.contains("health:policy.changed:"), "{item:?}");
        assert!(item.reason.contains("repo_health.py"), "{item:?}");
        let args = fx.args();
        assert_eq!(args.len(), 1, "the base's checker ran: {args:?}");
        assert_eq!(args[0].1, head);
        assert_ne!(args[0].0, head);

        // target が承認した policy の file を merge commit で戻す。
        let fx = setup();
        fx.wu_commit(&[("a.txt", "a\n")]);
        fx.land(&[(POLICY_LANDED, "approved\n")]);
        fx.merge_main(&|repo| write(repo, POLICY_LANDED, "weaker\n"));
        let (item, _) = fx.evaluate(phase, main_target());
        assert!(!item.pass, "policy phase={phase}: {item:?}");
        assert!(item.reason.contains("rules/landed.txt"), "{item:?}");
    }
}
