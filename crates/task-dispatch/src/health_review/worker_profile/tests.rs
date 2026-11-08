//! 付記 B2・B5: run の起動の非同期部分で組む `RunContext.workspace`（dispatcher を通す試験は
//! `src/dispatcher/tests/health_review_policy.rs`）。

use super::*;

/// B5: `Unavailable`（tick が target を決められなかった理由）と `Absent`（読む repo が無い）は方針を入れず、作業場所の
/// 説明だけを残す。説明も方針も無ければ `RunContext.workspace` 自体を省く（`request.json` に欄が出ない）。
#[tokio::test]
async fn health_review_worker_profile_without_a_policy_keeps_only_the_note() {
    let task_id = TaskId::new();
    let unavailable = RunWorkspace {
        note: Some("この案件のコードは手元にある。".into()),
        policy_target: Err("remote workspace".into()),
    };
    assert_eq!(
        unavailable.into_context(task_id).await,
        Some(RunWorkspaceContext {
            note: Some("この案件のコードは手元にある。".into()),
            review_policy: None,
        })
    );
    assert_eq!(RunWorkspace::default().into_context(task_id).await, None);
    let unreadable = RunWorkspace {
        note: None,
        policy_target: Err("repo agent-platform の target branch を解決できない".into()),
    };
    assert_eq!(unreadable.into_context(task_id).await, None);
}
