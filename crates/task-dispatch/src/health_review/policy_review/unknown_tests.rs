//! ADR 2026-11-07-health-review-integration I8・D6.4 の fail closed の試験（prefix `health_review_policy_unknown_`）。
//! base に検査器がある candidate で policy の状態を決められない（report を書かない・壊れた report・方針を読めない）
//! ときは不明として candidate の SHA に結び付けて記録し、関門は人の承認なしに合格させない。
//!
//! 一時 git repo の main に stub の `repo_health.py` を commit し、branch `cand` に 1 commit を足す。材料作り
//! （[`build_health_input`]）は stub を本当に起こし、残った event を [`Dispatcher::policy_review_gate`] が読む。
//! 外部ネットワークに出ず、LLM を呼ばない。

use std::path::Path;

use task_core::{FailedCheck, RepairDecision, RepoId, classify_review_failure};

use super::*;
use crate::dispatcher::tests::{dispatcher_without_bundled_checker, done_adapter, new_task};
use crate::health_review::input::{
    CandidateRepo, HEALTH_INPUT_TIMEOUT, HealthInputWork, build_health_input,
};
use crate::health_review::policy::REVIEW_POLICY_SCHEMA;
use crate::review::RequiredRepoResult;
use task_core::required_checks::health::CHECKER_SCRIPT;

pub(super) const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
pub(super) const POLICY_FILE: &str = "scripts/dev/repo-health/profiles/celeris.toml";

/// stub の検査器の振る舞い。
#[derive(Clone, Copy)]
pub(super) enum Policy {
    /// `[review]` あり（`source = base`）。
    Present,
    /// `[review]` なし（`source = absent`）。
    Absent,
    /// `--print-review-policy` が exit 2。
    Fails,
}

/// stub の report の書き方。
#[derive(Clone, Copy)]
pub(super) enum Report {
    /// 正しい report（`policy.changed` は引数）。
    Valid(bool),
    /// report を書かずに exit 2。
    Missing,
    /// JSON でない report。
    Broken,
}

fn report_text(report: Report) -> String {
    let changed = match report {
        Report::Valid(changed) => changed,
        Report::Missing => return String::new(),
        Report::Broken => return "{not json".into(),
    };
    let head = if changed { "b" } else { "a" };
    serde_json::json!({
        "schema": "celeris.repo-health.report/1",
        "summary": {"verdict": if changed { "fail" } else { "pass" }, "exit_code": 0, "reviewable": true,
            "counts": {"findings": 0, "fail_grade": 0, "new": 0, "worsened": 0, "existing": 0}},
        "findings": [],
        "policy": {"version": "1", "source": "base", "changed": changed, "files": [
            {"path": POLICY_FILE, "base_blob": "a".repeat(40), "head_blob": head.repeat(40)},
        ]},
    })
    .to_string()
}

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

/// main に stub の検査器（`checker = None` なら検査器なし）、branch `cand` に `changed` の 1 commit。
fn stub_repo(dir: &Path, checker: Option<(Policy, Report)>, changed: &str) -> (String, String) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("README.md"), "base\n").unwrap();
    if let Some((policy, report)) = checker {
        let printed = serde_json::json!({
            "schema": REVIEW_POLICY_SCHEMA,
            "source": if matches!(policy, Policy::Absent) { "absent" } else { "base" },
            "owner_map": null, "high_risk": [], "invariants": {}, "definition_sha256": DIGEST,
        });
        let print_exit = if matches!(policy, Policy::Fails) {
            2
        } else {
            0
        };
        let script = format!(
            "import sys\nargs = sys.argv[1:]\nif '--print-review-policy' in args:\n    sys.stdout.write({printed:?})\n    sys.exit({print_exit})\nreport = {report:?}\nif not report:\n    sys.exit(2)\nopen(args[args.index('--out') + 1], 'w').write(report)\nsys.exit(1)\n",
            printed = printed.to_string(),
            report = report_text(report),
        );
        let path = dir.join(CHECKER_SCRIPT);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, script).unwrap();
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "base"]);
    let target = git(dir, &["rev-parse", "HEAD"]);
    git(dir, &["checkout", "-q", "-b", "cand"]);
    let path = dir.join(changed);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "changed = true\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "cand"]);
    (target, git(dir, &["rev-parse", "HEAD"]))
}

pub(super) struct Case {
    pub store: Arc<dyn TaskStore>,
    pub d: Dispatcher,
    pub task: Task,
    pub repo: RepoId,
    pub target: String,
    pub reviewed: String,
    /// task が `Check::Reviewer` を持ち reviewer run（`r1`）が判定した review か（false は command の条件だけの
    /// task で reviewer run の無い review。材料作りは `policy_only`）。
    pub reviewer: bool,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

/// reviewer の条件（index 0）を持つ reviewing の task で、stub の検査器を本当に起こして final review の材料を作る。
fn case(checker: Option<(Policy, Report)>, changed: &str) -> Case {
    case_for(checker, changed, true)
}

/// [`case`] の reviewer の有無を選べる形。`reviewer = false` は command の条件（index 0）だけの task で、材料作りは
/// reviewer run の無い review（`policy_only`、`review_run` は worker run の `worker`）。
pub(super) fn case_for(checker: Option<(Policy, Report)>, changed: &str, reviewer: bool) -> Case {
    let (repo_dir, scratch) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (target, reviewed) = stub_repo(repo_dir.path(), checker, changed);
    let store: Arc<dyn TaskStore> = Arc::new(task_core::SqliteStore::open_in_memory().unwrap());
    let check = if reviewer {
        Check::Reviewer
    } else {
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        }
    };
    let mut task = new_task(scratch.path(), check, 2);
    task.status = Status::Reviewing;
    store.insert(&task).unwrap();
    let repo = RepoId::new();
    let work = HealthInputWork {
        store: store.clone(),
        task_id: task.id,
        review_run: if reviewer { "r1" } else { "worker" }.into(),
        policy_only: !reviewer,
        candidates: vec![CandidateRepo {
            repo_id: repo.to_string(),
            dir: repo_dir.path().to_path_buf(),
            target_sha: target.clone(),
            reviewed_sha: reviewed.clone(),
        }],
        skipped: vec![],
        out_dir: scratch.path().join("health"),
        timeout: HEALTH_INPUT_TIMEOUT,
        evaluated: Default::default(),
    };
    build_health_input(&work);
    // 同梱の検査器は設定しない（`checker = None` の repo は base にも同梱にも検査器が無い場合）。
    let d = dispatcher_without_bundled_checker(store.clone(), done_adapter(), 1);
    Case {
        store,
        d,
        task,
        repo,
        target,
        reviewed,
        reviewer,
        _dirs: (repo_dir, scratch),
    }
}

impl Case {
    /// reviewer が合格を書いた review（reviewer の policy verdict は無い）、または command の条件が合格した review の
    /// outcome。
    pub(super) fn outcome(&self) -> ReviewOutcome {
        ReviewOutcome {
            verdicts: vec![Verdict {
                criterion_idx: 0,
                pass: true,
                reason: if self.reviewer {
                    "reviewer(r1): ok"
                } else {
                    "exit 0"
                }
                .into(),
                repair_hint: None,
            }],
            plan: None,
            provider_failure: None,
            reviewer_run: None,
            target_stale: None,
            required_checks: vec![RequiredRepoResult {
                binding: Some(RequiredCheckBinding {
                    repo_id: self.repo,
                    target_sha: self.target.clone(),
                    reviewed_sha: self.reviewed.clone(),
                }),
                definition_sha256: DIGEST.into(),
                results: vec![],
                health_repair_hint: None,
                health_note: None,
            }],
            command_checks: vec![],
        }
    }

    /// 関門に通した outcome。
    pub(super) fn gate(&self) -> ReviewOutcome {
        let mut outcome = self.outcome();
        let review_run = self.reviewer.then_some("r1");
        self.d
            .policy_review_gate(&self.task, review_run, &mut outcome)
            .unwrap();
        outcome
    }

    pub(super) fn approvals(&self) -> Vec<Task> {
        self.store
            .children(self.task.id)
            .unwrap()
            .into_iter()
            .filter(|c| c.kind == TaskKind::Approval)
            .collect()
    }

    /// 承認の子を作って待つ（合格にならない）ことを確かめ、その子を返す。
    pub(super) fn assert_waits(&self) -> Task {
        let outcome = self.gate();
        assert_eq!(outcome.verdicts.len(), 1, "{:?}", outcome.verdicts);
        let approvals = self.approvals();
        assert_eq!(approvals.len(), 1);
        let child = approvals[0].clone();
        assert_eq!(child.status, Status::Ready);
        assert_eq!(
            child.title,
            format!(
                "{POLICY_APPROVAL_TITLE}{} @ {}",
                self.repo,
                &self.reviewed[..12]
            )
        );
        // 終端でない子がある間は `on_review_finished` が待ち、review も始めない（合格しない）。
        assert_eq!(
            task_ops::delegate::pending_children(self.store.as_ref(), self.task.id).unwrap(),
            1
        );
        assert_eq!(self.d.resolve_human_approvals(&self.task).unwrap(), None);
        child
    }

    pub(super) fn decide(&self, child: &Task, approved: bool) {
        let trigger = if approved {
            Trigger::Approve
        } else {
            Trigger::Reject
        };
        self.store
            .apply_transition(
                child.id,
                trigger,
                Some(Event::ApprovalDecided {
                    by: "human".into(),
                    approved,
                    note: None,
                }),
            )
            .unwrap();
    }
}

fn classify(verdicts: &[Verdict]) -> Option<RepairDecision> {
    let failing: Vec<FailedCheck> = verdicts
        .iter()
        .filter(|v| !v.pass)
        .map(|v| FailedCheck {
            check: Check::Reviewer,
            reason: v.reason.clone(),
            repair_hint: v.repair_hint.clone(),
        })
        .collect();
    (!failing.is_empty()).then(|| classify_review_failure(&failing))
}

/// 受け入れ条件 0: report を書かない・壊れた report・方針を読めない candidate は、reviewer が合格を書いても
/// 承認の子を作って待つ（題は candidate の SHA の鍵、本文に不明の理由）。
///
/// `[review]` の無い（absent）repo の report の故障は、`[review]` のある repo と同じ理由の文になる。ADR
/// 2026-11-11-health-generic-review-input の R1・R4 で absent の repo も同じ材料の構築を通り、absent 専用の
/// 省略の理由（旧い「policy の状態を決められない」）は撤去したので、期待する文を report の故障の文に替えた。
/// 守る不変条件（I8・D6.4 の fail closed）は変えていない。
#[test]
fn health_review_policy_unknown_candidates_wait_for_a_human() {
    let cases = [
        (
            Some((Policy::Present, Report::Missing)),
            "repo_health.py が report を書かなかった",
        ),
        (Some((Policy::Present, Report::Broken)), "report を読めない"),
        (
            Some((Policy::Fails, Report::Valid(false))),
            "--print-review-policy が失敗",
        ),
        (
            Some((Policy::Absent, Report::Missing)),
            "repo_health.py が report を書かなかった",
        ),
    ];
    for (checker, why) in cases {
        let c = case(checker, "crates/x/src/lib.rs");
        let child = c.assert_waits();
        assert!(
            child.objective.contains("could not be determined")
                && child.objective.contains("policy state unknown: ")
                && child.objective.contains(why)
                && child.objective.contains(&c.reviewed),
            "{why}: {}",
            child.objective
        );
        // 同じ candidate の再判定で子を作り直さない。
        c.gate();
        assert_eq!(c.approvals().len(), 1);
    }
}

/// 受け入れ条件 0: 人の承認で合格し、却下は substantive の不合格（→ replan）。
#[test]
fn health_review_policy_unknown_human_approval_passes_and_rejection_replans() {
    let approved = case(
        Some((Policy::Present, Report::Missing)),
        "crates/x/src/lib.rs",
    );
    let child = approved.assert_waits();
    approved.decide(&child, true);
    assert!(approved.d.policy_rereview(&approved.task).unwrap());
    let outcome = approved.gate();
    assert!(outcome.all_pass(), "{:?}", outcome.verdicts);
    let last = outcome.verdicts.last().unwrap();
    assert!(
        last.reason.starts_with(&format!(
            "human(approval task {}): health:policy approved by a human: policy state unknown (",
            child.id
        )),
        "{}",
        last.reason
    );

    let rejected = case(
        Some((Policy::Present, Report::Broken)),
        "crates/x/src/lib.rs",
    );
    let child = rejected.assert_waits();
    rejected.decide(&child, false);
    let outcome = rejected.gate();
    assert!(!outcome.all_pass());
    assert_eq!(
        classify(&outcome.verdicts),
        Some(RepairDecision::Substantive)
    );
    assert!(
        outcome.verdicts.iter().any(|v| !v.pass
            && v.reason.contains("health:policy: rejected by a human")
            && v.reason.contains("policy state unknown")),
        "{:?}",
        outcome.verdicts
    );
}

/// 不明の candidate でも、後の review で report を作り直して変更なしと決まれば承認を求めない（既知の状態が優先）。
#[test]
fn health_review_policy_unknown_rebuilt_report_decides() {
    let c = case(
        Some((Policy::Present, Report::Missing)),
        "crates/x/src/lib.rs",
    );
    let mut input = task_worker::protocol::HealthRepoInput {
        repo_id: c.repo.to_string(),
        target_sha: c.target.clone(),
        reviewed_sha: c.reviewed.clone(),
        ..Default::default()
    };
    input.policy.source = "base".into();
    c.store
        .append_event(
            c.task.id,
            &Event::HealthReviewInputBuilt {
                review_run: "r2".into(),
                repo_id: c.repo.to_string(),
                target_sha: c.target.clone(),
                reviewed_sha: c.reviewed.clone(),
                definition_sha256: DIGEST.into(),
                input: serde_json::to_value(&input).unwrap(),
            },
        )
        .unwrap();
    c.gate();
    assert!(c.approvals().is_empty());
}

/// 受け入れ条件 1: `[review]` の無い（absent）が検査器のある repo で policy の file を変えた candidate は、
/// reviewer に health の材料を渡し、report の `policy.changed` で承認を求める。
///
/// 守る不変条件: absent の repo でも、candidate が policy を変えたことは材料（`HealthReviewInputBuilt` の
/// `input.policy.changed = true`）に残り、関門は人の承認なしに合格させない（I8）。旧い試験は「absent では
/// `HealthReviewInputBuilt` を作らない」を固定していたが、ADR 2026-11-11-health-generic-review-input の R1・R4 で
/// その省略を撤去したので、材料が 1 件あり `policy.changed` を運ぶことを確かめる形に替えた。承認の子の assert は残す。
#[test]
fn health_review_policy_unknown_absent_policy_change_needs_approval() {
    let c = case(Some((Policy::Absent, Report::Valid(true))), POLICY_FILE);
    let child = c.assert_waits();
    assert!(
        child
            .objective
            .contains(&format!("policy files: {POLICY_FILE}")),
        "{}",
        child.objective
    );
    let built: Vec<task_worker::protocol::HealthRepoInput> = c
        .store
        .events_for(c.task.id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::HealthReviewInputBuilt { input, .. } => {
                Some(serde_json::from_value(input).unwrap())
            }
            _ => None,
        })
        .collect();
    assert_eq!(built.len(), 1, "an absent policy still builds the material");
    assert!(built[0].policy.changed, "{:?}", built[0].policy);
    assert!(
        built[0].owner_rows.is_empty() && built[0].invariants.is_empty(),
        "an absent policy has no owner map nor invariants: {:?}",
        built[0]
    );
}

/// 受け入れ条件 2: policy を変えない通常変更は承認の子を作らない（verdict も変えない）。検査器の無い repo は
/// 合格にしない（`health_enforce_g2_unavailable_is_not_pass`）。
#[test]
fn health_review_policy_unknown_normal_change_passes_without_approval() {
    for checker in [
        Some((Policy::Present, Report::Valid(false))),
        Some((Policy::Absent, Report::Valid(false))),
    ] {
        let c = case(checker, "crates/x/src/lib.rs");
        let outcome = c.gate();
        assert_eq!(outcome.verdicts, c.outcome().verdicts);
        assert!(outcome.all_pass());
        assert!(c.approvals().is_empty());
        assert!(!c.d.policy_rereview(&c.task).unwrap());
    }
}

/// health_enforce_g2_（ADR 2026-11-08-health-enforcement-loop D1.1・D1.2・J4）: base にも同梱にも検査器が無い repo は、
/// 当てないのではなく合格にしない。G2 の材料（reviewer あり）と policy の状態（reviewer なし）のどちらも、policy の
/// 状態を不明（理由 `checker_missing`）として candidate に結び付けて記録し、ほかが全部合格でも承認の子を作って待つ
/// （`assert_waits`: 終端でない子がある間は合格しない）。
/// 以前は「検査器の無い repo は何も記録せず合格」で、同じ入力で承認を求めない（この試験が検出する退行）。
#[test]
fn health_enforce_g2_unavailable_is_not_pass() {
    for reviewer in [true, false] {
        let c = case_for(None, "crates/x/src/lib.rs", reviewer);
        let child = c.assert_waits();
        assert!(
            child.objective.contains("policy state unknown")
                && child.objective.contains("checker_missing"),
            "{}",
            child.objective
        );
    }
}
