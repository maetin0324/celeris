//! ADR 2026-11-08-health-enforcement-loop D2.5・D2.6: review の health の証跡の再利用（reviewer の合格の再利用・review 前の
//! main 同期の後）の試験（試験名は `health_enforce_reviewer_reuse_` と `health_enforce_pre_review_sync_`）。
//! `review_item::evaluate_all` に同じ task の event を渡し、写した証跡が今の `(base, head)` に束縛し直され `reused_from` を
//! 持つこと、束縛（head の tree・base・検査器・policy）が違えば検査器を起こして評価し直すことを見る。検査器を起こしたかは
//! 証跡の `tool_version`（評価し直せば stub の `0.9.1`、写せば記録のときの印）で見分ける。外部ネットワークに出ず、LLM を呼ばない。

use std::path::Path;

use task_core::required_checks::definition_digest;
use task_core::required_checks::health::{HEALTH_CHECK_ID, HealthEvidence, HealthVerdict};
use task_core::{Event, PhaseCheckResult, RepoId};

use super::*;
use crate::health_review::review_item;
use crate::review::{RepoHealth, RequiredCheckBinding, RequiredRepoChecks};

const MARK: &str = "recorded-earlier";

fn binding(repo_id: RepoId, base: &str, head: &str) -> RequiredCheckBinding {
    RequiredCheckBinding {
        repo_id,
        target_sha: base.to_string(),
        reviewed_sha: head.to_string(),
    }
}

/// `(base, head)` を入口で 1 回評価した証跡（印つき）を、review の証跡の event にしたもの。
fn recorded(dir: &Path, repo_id: RepoId, base: &str, head: &str) -> (HealthEvidence, Event) {
    let outcome = health_gate(&HealthGateRequest {
        repo_id,
        worktree: dir,
        base,
        head,
        integrated_by_celeris: true,
        bundled: BUNDLED,
        timeout: TIMEOUT,
    });
    let HealthGateOutcome::Evaluated(mut run) = outcome else {
        panic!("not evaluated: {outcome:?}");
    };
    run.evidence.tool_version = MARK.into();
    assert_eq!(run.evidence.verdict, HealthVerdict::Pass);
    assert!(run.evidence.reviewable, "{:?}", run.evidence);
    let evidence = run.evidence;
    let event = Event::RequiredChecksEvaluated {
        review_run: "r".into(),
        repo_id,
        target_sha: base.into(),
        reviewed_sha: head.into(),
        definition_sha256: definition_digest(&[], evidence.checker.as_ref()),
        results: vec![PhaseCheckResult {
            cmd: HEALTH_CHECK_ID.into(),
            pass: true,
            summary: String::new(),
            health: Some(Box::new(evidence.clone())),
        }],
    };
    (evidence, event)
}

/// `spawn_review` と同じ未評価の health を `events` つきで評価し、health の証跡を返す。
async fn evaluated(
    dir: &Path,
    repo_id: RepoId,
    base: &str,
    head: &str,
    events: Vec<Event>,
) -> HealthEvidence {
    let mut repo = vec![RequiredRepoChecks {
        binding: Some(binding(repo_id, base, head)),
        commands: Ok(Vec::new()),
        reusable: Default::default(),
        workspace: None,
        health: Some(RepoHealth::At(dir.to_path_buf())),
    }];
    review_item::evaluate_all(&mut repo, Vec::new(), false, None, events).await;
    let Some(RepoHealth::Evaluated(item)) = repo.remove(0).health else {
        panic!("not evaluated");
    };
    item.result
        .and_then(|r| r.health)
        .map(|h| *h)
        .expect("health evidence")
}

fn pass_repo() -> (tempfile::TempDir, String) {
    let text = report("pass", 0, vec![], base_policy(false), vec![]);
    repo_with_checker(&stub_json(0, Some(&text)), true)
}

/// 不変条件: 同じ tree（別の commit。reviewer の合格の再利用・同じ内容の再 commit）の Pass は検査器を起こさず写し、今の
/// `(base, head)` に束縛し直して `reused_from` に元の head を書く（reviewer を省いても health の項目は付く）。
/// 既存の `health_enforce_direct_review_*` は event を渡さないので、写す経路を通らない。
#[tokio::test]
async fn health_enforce_reviewer_reuse_same_tree_rebinds_and_writes_reused_from() {
    let (dir, base) = pass_repo();
    let first = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let repo_id = RepoId::new();
    let (original, event) = recorded(dir.path(), repo_id, &base, &first);
    let again = commit(dir.path(), &[], "same tree, another commit");
    assert_ne!(first, again);

    let got = evaluated(dir.path(), repo_id, &base, &again, vec![event]).await;

    assert_eq!(got.tool_version, MARK, "the checker was not run again");
    assert_eq!(got.head_sha, again);
    assert_eq!(got.base_sha, base);
    assert_eq!(got.reused_from.as_deref(), Some(first.as_str()));
    assert_eq!(got.head_tree, original.head_tree);
    assert_eq!(got.verdict, HealthVerdict::Pass);
}

/// 不変条件: head の tree・base・検査器・policy のどれかが違う、または Pass でない証跡は写さず、入口で評価し直す
/// （reused_from は付かない）。
#[tokio::test]
async fn health_enforce_reviewer_reuse_reevaluates_when_the_binding_differs() {
    let (dir, base) = pass_repo();
    let first = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let repo_id = RepoId::new();
    let (original, event) = recorded(dir.path(), repo_id, &base, &first);
    let fresh = |got: &HealthEvidence, why: &str| {
        assert_eq!(got.tool_version, "0.9.1", "{why}: {got:?}");
        assert_eq!(got.reused_from, None, "{why}");
    };

    // head の tree が違う。
    let changed = commit(dir.path(), &[(SRC, "fn c() {}\n")], "other tree");
    let got = evaluated(dir.path(), repo_id, &base, &changed, vec![event.clone()]).await;
    fresh(&got, "head tree");
    assert_eq!(got.head_sha, changed);

    // base が違う（同じ tree でも base の sha が違えば写さない）。
    let new_base = commit(dir.path(), &[], "base moved");
    let got = evaluated(
        dir.path(),
        repo_id,
        &new_base,
        &new_base,
        vec![event.clone()],
    )
    .await;
    fresh(&got, "base");

    // 検査器が違う（証跡の検査器の版が今の適用判定と違う）。
    let mut other_checker = original.clone();
    other_checker.checker.as_mut().unwrap().sha256 = "other-checker".into();
    let events = vec![recorded_event(&other_checker)];
    let again = commit(dir.path(), &[], "same tree again");
    let got = evaluated(dir.path(), repo_id, &base, &again, events).await;
    fresh(&got, "checker");

    // policy が違う（同じ base・検査器の後の証跡の policy が違えば、前の policy の証跡は写さない）。
    let mut newer_policy = original.clone();
    newer_policy.head_tree = "some-other-tree".into();
    newer_policy.policy_sha256 = "other-policy".into();
    let events = vec![event.clone(), recorded_event(&newer_policy)];
    let got = evaluated(dir.path(), repo_id, &base, &again, events).await;
    fresh(&got, "policy");

    // 合格でない（Fail・dirty な作業ツリーの証跡）は写さない。
    for bad in [
        HealthEvidence {
            verdict: HealthVerdict::Fail,
            ..original.clone()
        },
        HealthEvidence {
            reviewable: false,
            ..original.clone()
        },
    ] {
        let got = evaluated(
            dir.path(),
            repo_id,
            &base,
            &again,
            vec![recorded_event(&bad)],
        )
        .await;
        fresh(&got, "not a reusable pass");
    }

    // 別の repo の証跡は写さない。
    let got = evaluated(dir.path(), RepoId::new(), &base, &again, vec![event]).await;
    fresh(&got, "repo");
}

fn recorded_event(evidence: &HealthEvidence) -> Event {
    Event::RequiredChecksEvaluated {
        review_run: "r".into(),
        repo_id: evidence.repo_id,
        target_sha: evidence.base_sha.clone(),
        reviewed_sha: evidence.head_sha.clone(),
        definition_sha256: definition_digest(&[], evidence.checker.as_ref()),
        results: vec![PhaseCheckResult {
            cmd: HEALTH_CHECK_ID.into(),
            pass: evidence.verdict == HealthVerdict::Pass,
            summary: String::new(),
            health: Some(Box::new(evidence.clone())),
        }],
    }
}

/// 不変条件: 必須検査の cmd の再利用（`reusable_checks`）は今の適用判定の検査器を定義指紋に入れる。同じ検査器で合格済みの
/// cmd だけが返り、検査器の無い指紋（旧い証跡）・違う検査器の指紋の cmd は返らない。
#[test]
fn health_enforce_reviewer_reuse_required_commands_follow_the_applied_checker() {
    let (dir, base) = pass_repo();
    let head = commit(dir.path(), &[(SRC, "fn b() {}\n")], "change");
    let repo_id = RepoId::new();
    let checker = applied(dir.path(), &base, None);
    let cmds = vec!["cargo test".to_string()];
    let event = |digest: String| Event::RequiredChecksEvaluated {
        review_run: "r".into(),
        repo_id,
        target_sha: base.clone(),
        reviewed_sha: head.clone(),
        definition_sha256: digest,
        results: vec![PhaseCheckResult::new(
            "cargo test".into(),
            true,
            String::new(),
        )],
    };
    let reuse = |events: &[Event]| {
        review_item::reusable_checks(
            events,
            &binding(repo_id, &base, &head),
            dir.path(),
            false,
            &cmds,
        )
    };

    let same = event(definition_digest(&cmds, Some(&checker)));
    assert_eq!(reuse(&[same]), ["cargo test".to_string()].into());
    assert!(reuse(&[event(definition_digest(&cmds, None))]).is_empty());
    let mut other = checker.clone();
    other.sha256 = "other-checker".into();
    assert!(reuse(&[event(definition_digest(&cmds, Some(&other)))]).is_empty());
}

/// 不変条件: review 前の同期（rebase）で head の tree が変われば、同期の前の証跡を写さず、同期の後の
/// `(target_sha, reviewed_sha)` で評価し直す。同期が target を進めただけで tree が同じでも base の sha が変わるので
/// 評価し直す。既存の試験は rebase の前後の束縛を見ない。
#[tokio::test]
async fn health_enforce_pre_review_sync_rebase_changes_the_tree_and_reevaluates() {
    let (dir, base) = pass_repo();
    sh_git(dir.path(), &["checkout", "-q", "-b", "task"]);
    let before = commit(dir.path(), &[(SRC, "fn b() {}\n")], "task change");
    let repo_id = RepoId::new();
    let (_, event) = recorded(dir.path(), repo_id, &base, &before);

    // target が進む（別の file）。task を rebase すると head の tree が変わる。
    sh_git(dir.path(), &["checkout", "-q", "-b", "advanced", &base]);
    let target = commit(dir.path(), &[("other.txt", "other\n")], "target advances");
    sh_git(dir.path(), &["checkout", "-q", "task"]);
    sh_git(dir.path(), &["rebase", "-q", "advanced"]);
    let after = sh_git(dir.path(), &["rev-parse", "HEAD"]);
    assert_ne!(before, after);

    let got = evaluated(dir.path(), repo_id, &target, &after, vec![event.clone()]).await;
    assert_eq!(got.tool_version, "0.9.1", "{got:?}");
    assert_eq!(got.reused_from, None);
    assert_eq!(
        (got.base_sha.as_str(), got.head_sha.as_str()),
        (target.as_str(), after.as_str())
    );

    // 同期が target を進めただけ（empty commit）で head の tree が同じでも、base の sha が違えば写さない。
    sh_git(dir.path(), &["checkout", "-q", "advanced"]);
    let empty_target = commit(dir.path(), &[], "target advances without changes");
    sh_git(dir.path(), &["checkout", "-q", "-B", "task2", &before]);
    sh_git(dir.path(), &["rebase", "-q", &empty_target]);
    let rebased = sh_git(dir.path(), &["rev-parse", "HEAD"]);
    let got = evaluated(dir.path(), repo_id, &empty_target, &rebased, vec![event]).await;
    assert_eq!(got.tool_version, "0.9.1", "{got:?}");
    assert_eq!(got.reused_from, None);
}
