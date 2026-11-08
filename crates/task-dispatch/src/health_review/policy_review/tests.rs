//! ADR 2026-11-07-health-review-integration D6.1・D6.3・D6.4・I3・I8 の試験（prefix `health_review_policy_review_`）。
//! 分類はディスパッチャ（`try_review_repair`）と同じく、不合格の verdict を `Check::Reviewer` の `FailedCheck` に
//! して `classify_review_failure`（変えていない）に通す。

use task_core::{FailedCheck, RepairDecision, RepoId, classify_review_failure};
use task_worker::protocol::{
    DiffStat, HealthPolicyState, HealthRepoInput, HealthReviewInput, PathStat,
};

use super::*;
use crate::dispatcher::tests::{dispatcher, done_adapter, new_task};
use crate::review::RequiredRepoResult;

const TAG: &str = "reviewer(r1)";
const POLICY_FILE: &str = "scripts/dev/repo-health/profiles/celeris.toml";

fn repo_input(repo_id: &str, paths: &[&str], policy_files: &[&str]) -> HealthRepoInput {
    HealthRepoInput {
        repo_id: repo_id.to_string(),
        target_sha: "t1".into(),
        reviewed_sha: "c1".into(),
        diff: DiffStat {
            files: paths.len() as u64,
            insertions: 3,
            deletions: 1,
            paths: paths
                .iter()
                .map(|p| PathStat {
                    path: p.to_string(),
                    insertions: 3,
                    deletions: 1,
                })
                .collect(),
        },
        policy: HealthPolicyState {
            source: "base".into(),
            changed: !policy_files.is_empty(),
            files: policy_files.iter().map(|f| f.to_string()).collect(),
        },
        ..Default::default()
    }
}

fn review_json(policy: Option<&str>) -> String {
    let policy = policy
        .map(|p| format!(r#","policy":{p}"#))
        .unwrap_or_default();
    format!(
        r#"{{"verdicts":[{{"criterion":0,"pass":true,"reason":"ok"}}],"health":{{"findings":[]{policy}}}}}"#
    )
}

fn from_review(text: &str, repos: Vec<HealthRepoInput>) -> Result<Vec<Verdict>, String> {
    let input = HealthReviewInput { repos, brief: None };
    Verdict::from_review_json(text, Some(&input), &[0], TAG, "artifacts/review.json")
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

const APPROVED: &str = r#"{"approved":true,"reason":"threshold change is justified"}"#;

/// 受け入れ条件 0（D6.1）: policy の file と他の file を混ぜた candidate は、reviewer が承認しても policy_mixed の
/// substantive の不合格（→ replan）。diff の path が上限で切り詰められていても見えない数から混在を数える。
#[test]
fn health_review_policy_review_mixed_paths_replan() {
    let mixed = repo_input("r", &[POLICY_FILE, "crates/x/src/lib.rs"], &[POLICY_FILE]);
    let verdicts = from_review(&review_json(Some(APPROVED)), vec![mixed]).unwrap();
    let failing: Vec<&Verdict> = verdicts.iter().filter(|v| !v.pass).collect();
    assert_eq!(failing.len(), 1, "{verdicts:?}");
    assert!(
        failing[0]
            .reason
            .starts_with("reviewer(r1): health:policy_mixed: r: policy files [scripts/dev/repo-health/profiles/celeris.toml] changed together with other files [crates/x/src/lib.rs]"),
        "{}",
        failing[0].reason
    );
    assert_eq!(classify(&verdicts), Some(RepairDecision::Substantive));
    // 文面に `format` を含んでも局所修復に推測されない（scope = substantive）。
    assert_eq!(
        failing[0].repair_hint.as_ref().map(|h| h.scope.as_str()),
        Some("substantive")
    );

    // `diff.paths` に policy の file しか見えなくても、総数が多ければ混在。
    let mut truncated = repo_input("r", &[POLICY_FILE], &[POLICY_FILE]);
    truncated.diff.files = 3;
    let mixed = crate::health_review::verdict::policy_mixed(&truncated).unwrap();
    assert!(mixed.ends_with("other files [] (+2 more); move the policy change into its own task"));

    // policy の file だけの candidate は混在ではない（切り詰めで見えない policy の file も数える）。
    let only = repo_input("r", &[POLICY_FILE], &[POLICY_FILE]);
    assert_eq!(crate::health_review::verdict::policy_mixed(&only), None);
    let mut hidden_policy = repo_input(
        "r",
        &[POLICY_FILE],
        &[POLICY_FILE, "scripts/dev/check-health.sh"],
    );
    hidden_policy.diff.files = 2;
    assert_eq!(
        crate::health_review::verdict::policy_mixed(&hidden_policy),
        None
    );
}

/// D6.3・I7: policy の file を変えた review で reviewer が `health.policy` を書かなければ全条件が不合格。
/// `approved = false` は substantive の不合格。`approved = true` は合格の記録だけ（合否は人の承認）。
#[test]
fn health_review_policy_review_missing_policy_verdict_fails() {
    let only = || vec![repo_input("r", &[POLICY_FILE], &[POLICY_FILE])];
    let err = from_review(&review_json(None), only()).unwrap_err();
    assert_eq!(
        err,
        format!(
            "{TAG}: {}: {POLICY_FILE}",
            crate::health_review::verdict::POLICY_VERDICT_MISSING
        )
    );

    let rejected = from_review(
        &review_json(Some(r#"{"approved":false,"reason":"weakens the gate"}"#)),
        only(),
    )
    .unwrap();
    let failing: Vec<&Verdict> = rejected.iter().filter(|v| !v.pass).collect();
    assert_eq!(failing.len(), 1);
    assert_eq!(
        failing[0].reason,
        format!("{TAG}: health:policy: {POLICY_FILE}: weakens the gate")
    );
    assert_eq!(classify(&rejected), Some(RepairDecision::Substantive));

    let approved = from_review(&review_json(Some(APPROVED)), only()).unwrap();
    assert!(approved.iter().all(|v| v.pass), "{approved:?}");
    assert!(
        approved[0]
            .reason
            .contains("health:policy (approved by reviewer")
    );
    // 同じ criterion の最後の verdict は reviewer 自身の判定。
    assert_eq!(approved.last().unwrap().reason, format!("{TAG}: ok"));
}

/// policy の変更が無い review は従来どおり: `health.policy` を求めず、verdict も足さない。
#[test]
fn health_review_policy_review_without_policy_change_keeps_previous_behavior() {
    let plain = || vec![repo_input("r", &["crates/x/src/lib.rs"], &[])];
    let verdicts = from_review(&review_json(None), plain()).unwrap();
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].reason, format!("{TAG}: ok"));
    // reviewer が書いた policy は使わない（policy の変更の無い review では合否に効かない）。
    let ignored = from_review(
        &review_json(Some(r#"{"approved":false,"reason":"n/a"}"#)),
        plain(),
    )
    .unwrap();
    assert!(ignored.iter().all(|v| v.pass));
}

struct Gate {
    store: Arc<dyn TaskStore>,
    d: Dispatcher,
    task: Task,
    repo: RepoId,
    _dir: tempfile::TempDir,
}

/// reviewer の条件（index 0）を持つ reviewing の task と、`changed_policy` に応じた health の材料の event。
fn gate(changed_policy: bool) -> Gate {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(task_core::SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(dir.path(), Check::Reviewer, 2);
    task.status = Status::Reviewing;
    store.insert(&task).unwrap();
    let repo = RepoId::new();
    let policy: &[&str] = if changed_policy { &[POLICY_FILE] } else { &[] };
    let input = repo_input(&repo.to_string(), &[POLICY_FILE], policy);
    store
        .append_event(
            task.id,
            &Event::HealthReviewInputBuilt {
                review_run: "r1".into(),
                repo_id: repo.to_string(),
                target_sha: "t1".into(),
                reviewed_sha: "c1".into(),
                definition_sha256: "def".into(),
                input: serde_json::to_value(&input).unwrap(),
            },
        )
        .unwrap();
    let d = dispatcher(store.clone(), done_adapter(), 1);
    Gate {
        store,
        d,
        task,
        repo,
        _dir: dir,
    }
}

impl Gate {
    /// reviewer の承認まで全部合格の outcome（`reviewed_sha` の candidate）。
    fn outcome(&self, reviewed_sha: &str) -> ReviewOutcome {
        ReviewOutcome {
            verdicts: vec![
                Verdict {
                    criterion_idx: 0,
                    pass: true,
                    reason: format!(
                        "{TAG}: health:policy (approved by reviewer; human approval required): {POLICY_FILE}: ok"
                    ),
                    repair_hint: None,
                },
                Verdict {
                    criterion_idx: 0,
                    pass: true,
                    reason: format!("{TAG}: ok"),
                    repair_hint: None,
                },
            ],
            plan: None,
            provider_failure: None,
            reviewer_run: None,
            target_stale: None,
            required_checks: vec![RequiredRepoResult {
                binding: Some(RequiredCheckBinding {
                    repo_id: self.repo,
                    target_sha: "t1".into(),
                    reviewed_sha: reviewed_sha.into(),
                }),
                definition_sha256: "def".into(),
                results: vec![],
            }],
            command_checks: vec![],
        }
    }

    fn approvals(&self) -> Vec<Task> {
        self.store
            .children(self.task.id)
            .unwrap()
            .into_iter()
            .filter(|c| c.kind == TaskKind::Approval)
            .collect()
    }

    fn decide(&self, child: &Task, approved: bool) {
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
                    note: Some("checked the thresholds".into()),
                }),
            )
            .unwrap();
    }
}

/// 受け入れ条件 1（D6.4・I8）: reviewer の approved = true だけでは合格せず、candidate ごとの `Approval` の子
/// （受信箱）を作って待つ。待っている間は review を始めない。人の承認で合格の verdict が付く。
#[test]
fn health_review_policy_review_approved_waits_for_human() {
    let g = gate(true);
    let mut outcome = g.outcome("c1");
    g.d.policy_review_gate(&g.task, Some("r1"), &mut outcome)
        .unwrap();
    // verdict は変えない（全部合格）が、終端でない子が増えるので `on_review_finished` は子を待つ。
    assert_eq!(outcome.verdicts.len(), 2);
    let approvals = g.approvals();
    assert_eq!(approvals.len(), 1);
    let child = &approvals[0];
    assert_eq!(child.status, Status::Ready);
    assert_eq!(
        child.title,
        format!("{POLICY_APPROVAL_TITLE}{} @ c1", g.repo)
    );
    assert!(
        child
            .objective
            .contains(&format!("policy files: {POLICY_FILE}")),
        "{}",
        child.objective
    );
    assert!(child.objective.contains("diff: 1 files, +3 -1"));
    assert!(
        child
            .objective
            .contains("reviewer: reviewer(r1): health:policy (approved by reviewer")
    );
    assert!(
        g.store
            .events_for(child.id)
            .unwrap()
            .iter()
            .any(|(_, e)| matches!(e, Event::ApprovalRequested))
    );
    assert_eq!(
        task_ops::delegate::pending_children(g.store.as_ref(), g.task.id).unwrap(),
        1
    );
    // 承認の子が未決の間は review を始めない（再起動の後も同じ。`Check::Human` の待ちと同じ）。
    assert_eq!(g.d.resolve_human_approvals(&g.task).unwrap(), None);
    // 同じ candidate の再判定で子を作り直さない。
    let mut again = g.outcome("c1");
    g.d.policy_review_gate(&g.task, Some("r1"), &mut again)
        .unwrap();
    assert_eq!(g.approvals().len(), 1);
    assert!(again.all_pass());

    g.decide(child, true);
    assert!(g.d.resolve_human_approvals(&g.task).unwrap().is_some());
    // 子待ちが終わったら `ReviewPass` にせず review をやり直す。
    assert!(g.d.policy_rereview(&g.task).unwrap());
    // やり直しの review は reviewer の合格を再利用する（reviewer run が無い）。元の candidate の承認を使う。
    g.store
        .append_event(
            g.task.id,
            &Event::ReviewerPassReused {
                review_run: "r1".into(),
                repo_id: g.repo,
                target_sha: "t1".into(),
                reviewed_sha: "c1".into(),
                from_target_sha: "t1".into(),
                from_reviewed_sha: "c1".into(),
                target_files: vec![],
            },
        )
        .unwrap();
    let mut rereview = g.outcome("c1");
    g.d.policy_review_gate(&g.task, None, &mut rereview)
        .unwrap();
    assert!(rereview.all_pass());
    let last = rereview.verdicts.last().unwrap();
    assert_eq!(
        last.reason,
        format!(
            "human(approval task {}): health:policy approved by a human: {POLICY_FILE}",
            child.id
        )
    );
}

/// D6.4: 人の却下は substantive の不合格（→ replan）。別の candidate（SHA が変わった）には新しい承認を求める。
#[test]
fn health_review_policy_review_human_rejection_replans() {
    let g = gate(true);
    let mut outcome = g.outcome("c1");
    g.d.policy_review_gate(&g.task, Some("r1"), &mut outcome)
        .unwrap();
    let child = g.approvals().remove(0);
    g.decide(&child, false);

    let mut rereview = g.outcome("c1");
    g.d.policy_review_gate(&g.task, Some("r1"), &mut rereview)
        .unwrap();
    assert!(!rereview.all_pass());
    let failing: Vec<&Verdict> = rereview.verdicts.iter().filter(|v| !v.pass).collect();
    assert_eq!(failing.len(), 1);
    assert_eq!(
        failing[0].reason,
        format!(
            "human(approval task {}): health:policy: rejected by a human: checked the thresholds: {POLICY_FILE}",
            child.id
        )
    );
    assert_eq!(
        classify(&rereview.verdicts),
        Some(RepairDecision::Substantive)
    );

    // 別の candidate（`c2`）の材料には、却下された `c1` の子を使わず新しい子を作って待つ。
    let mut moved = g.outcome("c2");
    let mut input = repo_input(&g.repo.to_string(), &[POLICY_FILE], &[POLICY_FILE]);
    input.reviewed_sha = "c2".into();
    g.store
        .append_event(
            g.task.id,
            &Event::HealthReviewInputBuilt {
                review_run: "r2".into(),
                repo_id: g.repo.to_string(),
                target_sha: "t1".into(),
                reviewed_sha: "c2".into(),
                definition_sha256: "def".into(),
                input: serde_json::to_value(&input).unwrap(),
            },
        )
        .unwrap();
    g.d.policy_review_gate(&g.task, Some("r2"), &mut moved)
        .unwrap();
    assert!(moved.all_pass());
    assert_eq!(g.approvals().len(), 2);
    assert_eq!(g.d.resolve_human_approvals(&g.task).unwrap(), None);
}

/// policy の変更が無い review・不合格を含む review は従来どおり: 承認の子を作らず、verdict も足さない。
#[test]
fn health_review_policy_review_no_policy_change_is_unchanged() {
    let g = gate(false);
    let mut outcome = g.outcome("c1");
    let before = outcome.verdicts.clone();
    g.d.policy_review_gate(&g.task, Some("r1"), &mut outcome)
        .unwrap();
    assert_eq!(outcome.verdicts, before);
    assert!(g.approvals().is_empty());
    assert!(!g.d.policy_rereview(&g.task).unwrap());
    assert_eq!(
        g.d.resolve_human_approvals(&g.task).unwrap(),
        Some(Default::default())
    );

    // policy を変えた candidate でも、ほかの判定が落ちていれば人に聞かない（従来の repair/replan）。
    let g = gate(true);
    let mut failing = g.outcome("c1");
    failing.verdicts[1].pass = false;
    g.d.policy_review_gate(&g.task, Some("r1"), &mut failing)
        .unwrap();
    assert!(g.approvals().is_empty());
    assert_eq!(failing.verdicts.len(), 2);
}
