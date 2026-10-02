//! ADR-0051: 既存レビュアーの対象SHAを固定し、同じrunに取り込み条件を追加する。
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use task_core::{
    Check, Criterion, Delivery, DeliveryState, Event, RepoId, StoreError, Task, TaskStore,
};

/// ADR-0118 D4: target 再進行による自動の再 sync→再 check→再 review は初回に加え最大 2 回。
pub const MAX_TARGET_RESYNCS: u32 = 2;

/// ADR-0118 D4: 直近の「人の再開」以後に記録された、`repo` の target 再進行（`ReviewTargetAdvanced`）の数。
/// 自動の再レビューは `Transitioned{reason:"rereview"}` の直後に同じ transaction で
/// `ReviewTargetAdvanced` を追記するので、その直後に再進行が続かない `rereview`/`reopen` だけを
/// 人の明示的な再開として数え直す。
pub fn target_restale_count(events: &[Event], repo: RepoId) -> u32 {
    let mut count = 0;
    for (i, event) in events.iter().enumerate() {
        match event {
            Event::ReviewTargetAdvanced { repo_id, .. } if *repo_id == repo => count += 1,
            Event::Transitioned { reason, .. }
                if (reason == "rereview" || reason == "reopen")
                    && !matches!(events.get(i + 1), Some(Event::ReviewTargetAdvanced { .. })) =>
            {
                count = 0
            }
            _ => {}
        }
    }
    count
}

#[derive(Debug, Clone, Default)]
pub struct DeliveryPolicy {
    pub projects: Vec<String>,
    pub repo: PathBuf,
}

pub fn begin(
    store: &dyn TaskStore,
    task: &mut Task,
    workspace_root: &Path,
    policy: &DeliveryPolicy,
    worker_run: &str,
    review_run: &str,
) -> Result<Option<Delivery>, StoreError> {
    let Some(project_id) = task.project_id else {
        return Ok(None);
    };
    // ADR-0079 D6（Phase R1c）: 木の子 task は main に取り込まない（成果は親の段階末尾の統合で親の
    // ブランチに入る）。部署の取り込み判定・`deliveries` の行・release は root だけ。
    if !policy.projects.contains(&project_id.to_string())
        || task_core::support_kind(task).is_some()
        || task_core::tree::is_tree_child(task)
        || task.repos.len() != 1
    {
        return Ok(None);
    }
    let org = store.org_list()?;
    let Some(department) = task
        .assignee
        .as_deref()
        .and_then(|id| task_core::department_of(&org, id))
    else {
        return Ok(None);
    };
    let Some(marker) = crate::workspace::read_marker(&workspace_root.join(task.id.to_string()))
    else {
        return Ok(None);
    };
    if marker.repos.len() != 1 {
        return Ok(None);
    }
    let mr = &marker.repos[0];
    let Some(row) = store
        .repo_list(project_id)?
        .into_iter()
        .find(|r| r.id == task.repos[0].repo_id)
    else {
        return Ok(None);
    };
    let task_core::WorkspaceSpec::Local { path, .. } = &row.location else {
        return Ok(None);
    };
    if !path.is_dir()
        || path.canonicalize().ok() != policy.repo.canonicalize().ok()
        || Path::new(&mr.source).canonicalize().ok() != path.canonicalize().ok()
        || mr.kind != "git"
    {
        return Ok(None);
    }
    let Some(branch) = &mr.branch else {
        return Ok(None);
    };
    if !branch.ends_with(&task.id.to_string()) {
        return Ok(None);
    }
    let default_branch = crate::changes::default_branch(path, row.default_branch.as_deref());
    let resolve = |r: &str| {
        crate::changes::git(path, &["rev-parse", "--verify", r], Duration::from_secs(10))
            .filter(|o| o.ok)
            .map(|o| o.stdout.trim().to_string())
    };
    let (Some(base), Some(head)) = (
        resolve(&format!("refs/heads/{default_branch}")),
        resolve(&format!("refs/heads/{branch}")),
    ) else {
        return Ok(None);
    };
    let old = store.delivery_get(task.id)?;
    if old.as_ref().is_some_and(|d| {
        matches!(d.state, DeliveryState::Merging | DeliveryState::Preparing)
            || (d.state == DeliveryState::Ready && d.head == head)
    }) {
        return Ok(None);
    }
    let criterion_idx = task.acceptance.len();
    task.acceptance.push(Criterion {check:Check::Reviewer,text:format!("【部署内のマージ可否判定】リポジトリ {} の {} ({}) に、コミット {} (ブランチ {}) を取り込めること。指定SHAの実際の差分、依頼範囲、テスト、移行・設定の互換性、秘密の混入を確認し、対象の実装worktreeに未コミット変更や未解決の欠陥があれば不合格にする。登録元リポジトリにはユーザーの未コミット変更があり得る。対象差分と重ならずfast-forwardで保持できる既存変更は拒否理由にせず、復元・コミット・stashしない。対象差分と衝突する変更は停止理由にする。レビューは部署をまとめる担当 {} がこの独立runで行い、上司やCoSで重複レビューしない。合格すると制御プレーンがマージ・release・verifyまで実行し、最終デプロイだけ人が押す。要件や権限などユーザーに確認しないと判断できない場合は、理由の先頭に [needs-human] と具体的な質問を含める。技術的な不備だけなら部署内で差し戻す。自分でmergeやdeploy、実装変更はしない。",path.display(),default_branch,base,head,branch,department) });
    let d = Delivery {
        task_id: task.id,
        project_id,
        repo_id: row.id,
        repo: row.name,
        branch: branch.clone(),
        base,
        head,
        target_sha: None,
        reviewed_sha: None,
        merge_candidate_sha: None,
        default_branch,
        department,
        review_run: review_run.into(),
        worker_run: worker_run.into(),
        criterion_idx,
        decision: None,
        state: DeliveryState::Reviewing,
        detail: "部署のレビュアーが実装とマージ可否を確認中です".into(),
        release: None,
        prepare_pid: None,
        notification: None,
        pushed_at: None,
        push_error: None,
    };
    if !store.delivery_save(old.as_ref(), &d)? {
        return Err(StoreError::Invalid(
            "delivery review changed concurrently".into(),
        ));
    }
    Ok(Some(d))
}

#[cfg(test)]
mod tests;
