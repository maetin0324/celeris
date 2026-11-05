//! ADR 2026-11-05-a9-llm-pilot-findings D4: 決定的な検査が落ちているとき、reviewer が合格を返しても done にしない。
//! 食い違いは event に残り、reviewer の reason も `ReviewVerdict` に残る。検査の無い task は従来どおり。
//! fake の判定を `on_review_finished` に直接渡す（LLM は使わない）。

use super::*;
use crate::review::ReviewOutcome;

fn outcome(verdicts: Vec<Verdict>) -> ReviewOutcome {
    ReviewOutcome {
        verdicts,
        plan: None,
        provider_failure: None,
        reviewer_run: None,
        target_stale: None,
        required_checks: vec![],
    }
}

fn verdict(idx: usize, pass: bool, reason: &str) -> Verdict {
    Verdict {
        criterion_idx: idx,
        pass,
        reason: reason.into(),
        repair_hint: None,
    }
}

fn reviewing_task(dir: &std::path::Path, checks: Vec<Check>) -> Task {
    let mut task = new_task(dir, Check::Reviewer, 2);
    task.acceptance = checks
        .into_iter()
        .map(|check| Criterion {
            text: "c".into(),
            check,
        })
        .collect();
    task.status = Status::Reviewing;
    task
}

fn progress_texts(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<String> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::WorkerProgress { msg, .. } => Some(msg),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn failed_command_check_wins_over_reviewer_pass() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = reviewing_task(
        dir.path(),
        vec![
            Check::Command {
                cmd: "test -f out/x.txt".into(),
                expect_exit: 0,
            },
            Check::Reviewer,
        ],
    );
    store.insert(&task).unwrap();
    let mut d = instant_done_dispatcher(store.clone());
    d.on_review_finished(
        task.id,
        "run1".into(),
        outcome(vec![
            verdict(0, false, "exit 1: out/x.txt missing"),
            verdict(1, true, "reviewer(r1): looks complete"),
        ]),
    )
    .unwrap();
    let after = store.get(task.id).unwrap().unwrap();
    assert_ne!(after.status, Status::Done, "the failed check must win");
    let events = store.events_for(task.id).unwrap();
    let reasons: Vec<String> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ReviewVerdict { reason, pass, .. } if *pass => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(reasons, vec!["reviewer(r1): looks complete".to_string()]);
    let notes = progress_texts(&store, task.id);
    assert!(
        notes.iter().any(|m| m.starts_with("review discrepancy:")
            && m.contains("looks complete")
            && m.contains("out/x.txt missing")),
        "{notes:?}"
    );
}

#[tokio::test]
async fn reviewer_only_task_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = reviewing_task(dir.path(), vec![Check::Reviewer]);
    store.insert(&task).unwrap();
    let mut d = instant_done_dispatcher(store.clone());
    d.on_review_finished(
        task.id,
        "run1".into(),
        outcome(vec![verdict(0, true, "reviewer(r1): ok")]),
    )
    .unwrap();
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    assert!(
        !progress_texts(&store, task.id)
            .iter()
            .any(|m| m.starts_with("review discrepancy:"))
    );
}
