//! ADR 2026-11-05-phase3-p0e（K3）: 依頼文の Outcome の `checks` が root の `Check::Command` になり、検査が落ちている
//! 間は reviewer が合格を返しても root は done にならず、本物の TS の controller も達成にしない。検査は本物の shell で
//! `review_task` が実行する。reviewer の判定だけ台本で「合格」に差し替える（LLM は 0）。

use super::*;
use crate::review::{ReviewExtras, review_task};
use task_worker::LocalWorkspace;

const CHECK: &str = "test -s out/o1.txt";

fn request() -> String {
    let rows = json!([{"id": "o1", "title": "o1 を仕上げる", "acceptance": ["o1 の成果を書く"],
                       "checks": [{"cmd": CHECK}]}]);
    format!("pilot の案件。\n```celeris-outcomes\n{rows}\n```\n")
}

impl Pilot {
    /// 本物の検査 + 台本の reviewer の合格で review を 1 回終える。
    async fn review_root(&mut self, root: &Task, dir: &std::path::Path, run: &str) {
        self.set_status(root.id, Status::Reviewing, "worker_done");
        let task = self.store.get(root.id).unwrap().unwrap();
        let ws = LocalWorkspace::new(dir);
        let mut outcome = review_task(
            &task,
            &ws,
            dir,
            &dir.join("artifacts"),
            &[],
            Duration::from_secs(120),
            ReviewExtras::default(),
        )
        .await;
        for v in &mut outcome.verdicts {
            if matches!(task.acceptance[v.criterion_idx].check, Check::Reviewer) {
                v.pass = true;
                v.reason = format!("reviewer({run}): looks complete");
            }
        }
        let d = self.d.as_mut().unwrap();
        d.on_review_finished(root.id, run.into(), outcome).unwrap();
    }

    fn acceptance_checks(&self, root: &Task) -> Vec<Check> {
        root.acceptance.iter().map(|c| c.check.clone()).collect()
    }

    fn assert_not_achieved(&self, when: &str) {
        assert_ne!(self.phase(), ControllerPhase::Achieved, "{when}");
        assert!(!self.rules().iter().any(|r| r == "pc/achieved"), "{when}");
    }
}

#[tokio::test]
async fn pilot_review_check_failed_check_blocks_root_and_achievement() {
    let test = "pilot_review_check_failed_check_blocks_root_and_achievement";
    let Some(mut p) = Pilot::new(test, &request(), |_| {}) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    p.run_until(HOUR);
    let first = p.roots().remove(0);
    assert_eq!(
        p.acceptance_checks(&first),
        vec![
            Check::Reviewer,
            Check::Command {
                cmd: CHECK.into(),
                expect_exit: 0,
            },
        ],
        "Rust adds the check of the outcome; the TS proposal has none"
    );

    // 成果が無い: reviewer は合格を返すが、検査の不合格が勝つ。試行を使い切るまで done にも達成にもならない。
    let mut reviews = 0;
    while p.store.get(first.id).unwrap().unwrap().status != Status::Failed {
        reviews += 1;
        assert!(reviews <= 6, "the root never reached failed");
        p.review_root(&first, dir.path(), &format!("run-{reviews}"))
            .await;
        let now = p.store.get(first.id).unwrap().unwrap();
        assert_ne!(now.status, Status::Done, "the failed check must win");
        p.assert_not_achieved("while the check fails");
    }
    let events = p.store.events_for(first.id).unwrap();
    let checked: Vec<(usize, String, i32, Option<i32>, bool, bool)> = events
        .iter()
        .filter_map(|(_, e)| match e {
            Event::ReviewCommandChecked {
                criterion_idx,
                cmd,
                expect_exit,
                exit,
                timed_out,
                pass,
                ..
            } => Some((
                *criterion_idx,
                cmd.clone(),
                *expect_exit,
                *exit,
                *timed_out,
                *pass,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(checked.len(), reviews);
    assert!(
        checked
            .iter()
            .all(|c| *c == (1, CHECK.to_string(), 0, Some(1), false, false)),
        "{checked:?}"
    );
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::WorkerProgress { msg, .. }
            if msg.starts_with("review discrepancy:") && msg.contains("looks complete")
    )));

    // 次の試行の root にも同じ検査が付く。成果を置くと検査が通り、done になって達成する（対照）。
    p.run_until(p.secs() + HOUR);
    p.assert_not_achieved("after the failed root");
    let roots = p.roots();
    assert_eq!(roots.len(), 2, "the controller retried once");
    let retry = roots[1].clone();
    assert_eq!(p.acceptance_checks(&retry), p.acceptance_checks(&first));
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    std::fs::write(dir.path().join("out/o1.txt"), "made\n").unwrap();
    // 第 3 フェーズ I4: repo の無い root は成果物（`ArtifactProduced`）があって初めて取り込み済み。
    p.store
        .append_event(
            retry.id,
            &Event::ArtifactProduced {
                run_id: "run-ok".into(),
                artifact: task_core::ArtifactRef {
                    name: "o1.txt".into(),
                    path: "out/o1.txt".into(),
                    sha256: "0".repeat(64),
                    kind: "file".into(),
                    declared: true,
                },
            },
        )
        .unwrap();
    p.review_root(&retry, dir.path(), "run-ok").await;
    assert_eq!(p.store.get(retry.id).unwrap().unwrap().status, Status::Done);
    p.run_until(p.secs() + HOUR);
    assert_eq!(p.phase(), ControllerPhase::Achieved);
}

/// `checks` の無い Outcome の root は検査を持たない（reviewer だけの経路。ADR の「保証できないこと」）。
#[tokio::test]
async fn pilot_review_check_outcome_without_checks_stays_reviewer_only() {
    let test = "pilot_review_check_outcome_without_checks_stays_reviewer_only";
    let req = request_of(&[("o1", &[])]);
    let Some(mut p) = Pilot::new(test, &req, |_| {}) else {
        return;
    };
    p.run_until(HOUR);
    let root = p.roots().remove(0);
    assert_eq!(p.acceptance_checks(&root), vec![Check::Reviewer]);
}
