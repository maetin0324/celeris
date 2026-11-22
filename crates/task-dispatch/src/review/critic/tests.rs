//! ADR 2026-11-22-strategy-review-critic D3.2・D3.3・D3.5・D7.2 の試験（prefix `strategy_critic_`）。reviewer は stub の adapter
//! で、LLM は動かさない。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use task_core::strategy_critic::{CriticSubject, critic_subjects};
use task_core::*;
use task_worker::protocol::{RunRequest, Terminal};
use task_worker::{AdapterError, EventSink, LocalWorkspace, RunLimits, RunOutcome, WorkerAdapter};

use crate::review::tests::task_with;
use crate::review::{
    REVIEW_FILE_NAME, ReviewExtras, ReviewOutcome, ReviewerRun, review_task, reviewer_hint,
    reviewer_run_required,
};

const DOC: &str = r#"{"schema":"celeris.strategy/1","candidates":[{"id":"fixed-plan"},{"id":"probe-first"}]}"#;

struct Stub {
    review_json: String,
    seen: Mutex<Vec<RunRequest>>,
}

#[async_trait]
impl WorkerAdapter for Stub {
    fn id(&self) -> &str {
        "stub-critic"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        std::fs::create_dir_all(&req.artifacts_dir).expect("artifacts dir");
        std::fs::write(req.artifacts_dir.join(REVIEW_FILE_NAME), &self.review_json)
            .expect("review.json");
        self.seen.lock().expect("lock").push(req);
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "reviewed".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

struct NullSink;
impl EventSink for NullSink {
    fn progress(&self, _msg: &str) {}
    fn artifact(&self, _artifact: &ArtifactRef) {}
}

fn run_with(adapter: Arc<Stub>) -> ReviewerRun {
    ReviewerRun {
        cargo_target_dir: None,
        node: None,
        profile: None,
        skills: Vec::new(),
        adapter,
        llm_route: task_worker::protocol::SelectedLlmRoute {
            harness_id: "h1".into(),
            source_id: "s1".into(),
            model_id: "m".into(),
            account_id: "a1".into(),
            endpoint: None,
            credential: None,
        },
        run_id: "rev-critic".into(),
        limits: RunLimits {
            wall_clock: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
        },
        sink: Box::new(NullSink),
        hint: reviewer_hint(),
        subject_genre: None,
        session: None,
        session_diff: Vec::new(),
        token_budget: None,
        material: Default::default(),
    }
}

/// workspace に文書を置き、その `ArtifactProduced` から対象を導く（dispatcher と同じ導き方）。
fn subjects_in(dir: &std::path::Path) -> (Vec<CriticSubject>, String) {
    let name = task_core::strategy::artifact_name("arch", 1);
    let path = format!("artifacts/{name}");
    let full = dir.join(&path);
    std::fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
    std::fs::write(&full, DOC).expect("doc");
    let sha = task_worker::artifact::sha256_file(&full).expect("sha");
    let events = vec![(
        1,
        Event::ArtifactProduced {
            run_id: "wu-run".into(),
            artifact: ArtifactRef {
                name,
                path,
                sha256: sha.clone(),
                kind: "file".into(),
                declared: true,
            },
        },
    )];
    (critic_subjects(&events), sha)
}

fn ok(reason: &str) -> serde_json::Value {
    serde_json::json!({"ok": true, "reason": reason})
}

fn critic_json(sha: &str, ids: &[&str], revisit_ok: bool) -> serde_json::Value {
    let candidates: Vec<serde_json::Value> = ids
        .iter()
        .map(|id| {
            serde_json::json!({"id": id, "defects": ["前提が未検証"],
                "falsification": ok("観測で確かめられる"), "experiment": ok("仮説を区別できる")})
        })
        .collect();
    serde_json::json!({"subjects": [{"name": "strategy/arch/v1.json", "sha256": sha,
        "candidates": candidates, "comparison": ok("費用と情報価値で比べた"),
        "revisit": {"ok": revisit_ok, "reason": "却下した候補の出し直しは無い"}}]})
}

async fn review(
    checks: Vec<Check>,
    review_json: serde_json::Value,
    with_subjects: bool,
) -> (ReviewOutcome, usize, Option<RunRequest>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (subjects, _) = subjects_in(dir.path());
    let task = task_with(checks, dir.path());
    let stub = Arc::new(Stub {
        review_json: review_json.to_string(),
        seen: Mutex::new(Vec::new()),
    });
    let ws = LocalWorkspace::new(dir.path());
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            reviewer: Some(run_with(stub.clone())),
            critic_subjects: if with_subjects { subjects } else { Vec::new() },
            ..Default::default()
        },
    )
    .await;
    let seen = stub.seen.lock().expect("lock");
    (out, seen.len(), seen.first().cloned())
}

fn sha_of_doc() -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    subjects_in(dir.path()).1
}

/// 不変条件: 戦略の成果物を作った task は `Check::Reviewer` が無くても reviewer run を 1 回起こし、Critic の節の無い
/// `review.json` は暗黙の条件 `strategy_critic` で不合格になる（fail closed）。base は暗黙の条件を持たず合格する。
#[tokio::test]
async fn strategy_critic_missing_section_fails_review() {
    let (out, runs, req) = review(vec![], serde_json::json!({"verdicts": []}), true).await;
    assert_eq!(runs, 1);
    let critic = req
        .and_then(|r| r.context.review)
        .and_then(|r| r.strategy_critic)
        .expect("the reviewer gets the critic request");
    assert_eq!(critic.criterion_idx, 0);
    assert_eq!(critic.rubric.len(), 5);
    assert!(!out.all_pass(), "{:?}", out.verdicts);
    let v = out.verdicts.iter().find(|v| v.criterion_idx == 0).expect("verdict");
    assert!(v.reason.contains("strategy_critic"), "{}", v.reason);
}

/// 不変条件: 対象・候補が文書と一致し、すべての `ok` が `true` の Critic だけが合格する。受け入れ条件の判定と Critic は
/// 同じ 1 回の reviewer run が行う（reviewer run の数は増えない）。
#[tokio::test]
async fn strategy_critic_all_ok_passes() {
    let sha = sha_of_doc();
    let json = serde_json::json!({
        "verdicts": [{"criterion": 0, "pass": true, "reason": "ok"}],
        "critic": critic_json(&sha, &["fixed-plan", "probe-first"], true),
    });
    let (out, runs, _) = review(vec![Check::Reviewer], json, true).await;
    assert_eq!(runs, 1);
    assert_eq!(
        out.verdicts.iter().map(|v| (v.criterion_idx, v.pass)).collect::<Vec<_>>(),
        vec![(0, true), (1, true)],
        "{:?}",
        out.verdicts
    );
}

/// 不変条件: 1 つでも `ok: false`、対象の sha256 の食い違い、候補の欠け、空の理由は不合格である。
#[tokio::test]
async fn strategy_critic_subject_sha_mismatch_or_candidate_missing_fails() {
    let sha = sha_of_doc();
    let cases = [
        critic_json(&"0".repeat(64), &["fixed-plan", "probe-first"], true),
        critic_json(&sha, &["fixed-plan"], true),
        critic_json(&sha, &["fixed-plan", "probe-first", "probe-first"], true),
        critic_json(&sha, &["fixed-plan", "probe-first"], false),
    ];
    for critic in cases {
        let json = serde_json::json!({"verdicts": [], "critic": critic});
        let (out, runs, _) = review(vec![], json.clone(), true).await;
        assert_eq!(runs, 1);
        assert!(!out.all_pass(), "{json}: {:?}", out.verdicts);
    }
    let mut empty_reason = critic_json(&sha, &["fixed-plan", "probe-first"], true);
    empty_reason["subjects"][0]["comparison"]["reason"] = serde_json::json!("");
    let (out, _, _) = review(vec![], serde_json::json!({"verdicts": [], "critic": empty_reason}), true).await;
    assert!(!out.all_pass(), "{:?}", out.verdicts);
}

/// 不変条件: 決定的な条件が落ちれば reviewer run を起こさず、`strategy_critic` も判定されないまま不合格になる（ADR-0007 D5）。
#[tokio::test]
async fn strategy_critic_not_evaluated_when_deterministic_fails() {
    let checks = vec![Check::Command {
        cmd: "exit 1".into(),
        expect_exit: 0,
    }];
    let (out, runs, _) = review(checks, serde_json::json!({"verdicts": []}), true).await;
    assert_eq!(runs, 0);
    let v = out.verdicts.iter().find(|v| v.criterion_idx == 1).expect("critic verdict");
    assert!(!v.pass);
}

/// 回帰: 戦略の成果物の無い task では reviewer run の数が base と同じ（`Check::Reviewer` の無い task は 0 回、ある task は
/// 1 回）で、`request.json` に `strategy_critic` が出ず、暗黙の条件も足されない。
#[tokio::test]
async fn strategy_critic_absent_reviewer_run_count_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(!reviewer_run_required(&task_with(vec![], dir.path()), false, &[]));
    assert!(reviewer_run_required(&task_with(vec![Check::Reviewer], dir.path()), false, &[]));
    assert!(reviewer_run_required(&task_with(vec![], dir.path()), true, &[]));
    let (subjects, _) = subjects_in(dir.path());
    assert!(reviewer_run_required(&task_with(vec![], dir.path()), false, &subjects));

    let (out, runs, _) = review(vec![], serde_json::json!({"verdicts": []}), false).await;
    assert_eq!(runs, 0);
    assert!(out.verdicts.is_empty() && out.reviewer_run.is_none());
    let json = serde_json::json!({"verdicts": [{"criterion": 0, "pass": true, "reason": "ok"}]});
    let (out, runs, req) = review(vec![Check::Reviewer], json, false).await;
    assert_eq!(runs, 1);
    assert_eq!(out.verdicts.len(), 1);
    assert!(out.all_pass());
    let review = req.and_then(|r| r.context.review).expect("review");
    assert!(review.strategy_critic.is_none());
    assert!(!serde_json::to_string(&review).expect("json").contains("strategy_critic"));
}

/// 不変条件: 戦略の成果物を作った task は reviewer の合格の持ち越し（`reviewer_reuse`）で Critic を飛ばせない。
#[tokio::test]
async fn strategy_critic_reuse_not_applied() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (subjects, _) = subjects_in(dir.path());
    let task = task_with(vec![Check::Reviewer], dir.path());
    let ws = LocalWorkspace::new(dir.path());
    let out = review_task(
        &task,
        &ws,
        dir.path(),
        &dir.path().join("artifacts"),
        &[],
        Duration::from_secs(5),
        ReviewExtras {
            reviewer: None,
            reviewer_reuse: Some("target moved without overlap".into()),
            critic_subjects: subjects,
            ..Default::default()
        },
    )
    .await;
    let critic = out.verdicts.iter().find(|v| v.criterion_idx == 1).expect("critic verdict");
    assert!(!critic.pass, "{:?}", out.verdicts);
}
