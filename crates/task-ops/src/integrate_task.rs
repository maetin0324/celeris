//! ADR 2026-10-28-cos-integrate-task: 機械的に解けない統合の依頼（origin `delivery`）を、衝突を解く通常の root task
//! （integrate task）に渡す。起票の中身（[`spec`]）、結びの状態（[`link_for`]）、決着（[`settle`]）、人に回す理由
//! （[`escalated_reason`]）を持つ。判定は store と git だけで、LLM は呼ばない。git は store のロックの外で呼ぶ
//! （store の呼び出しは 1 回ずつ）。
use std::path::Path;
use std::process::Command;

use task_core::integration_request::{
    DELIVERY_ORIGIN, INTEGRATE_TASK_ESCALATED, INTEGRATE_TASK_INTEGRATED, INTEGRATED_ANSWER,
    IntegrateTaskLink, IntegrationRequest, integrate_task_link,
};
use task_core::{Event, Status, StoreError, Task, TaskId, TaskStore, WorkspaceSpec};

use crate::add::NewTaskSpec;

/// integrate task に付ける label（一覧で探せるように）。
pub const LABEL: &str = "integrate-task";

/// 未回答の依頼 1 件（元の task・依頼の id・origin・中身）。
#[derive(Debug, Clone)]
pub struct OpenRequest {
    pub task_id: TaskId,
    pub request_id: String,
    pub origin: String,
    pub request: IntegrationRequest,
}

/// store の未回答の依頼を [`OpenRequest`] にする。
pub fn open_requests(store: &dyn TaskStore) -> Result<Vec<OpenRequest>, StoreError> {
    Ok(store
        .open_integration_requests()?
        .into_iter()
        .filter_map(|row| match row.event {
            Event::IntegrationRequested { request, origin } => Some(OpenRequest {
                task_id: row.task_id,
                request_id: request.id_for(row.task_id),
                origin,
                request: *request,
            }),
            _ => None,
        })
        .collect())
}

/// D2: 元の task の依頼 `request_id` の最新の結び。
pub fn link_for(
    store: &dyn TaskStore,
    task_id: TaskId,
    request_id: &str,
) -> Result<Option<IntegrateTaskLink>, StoreError> {
    let events = store.events_for(task_id)?;
    Ok(integrate_task_link(
        events.iter().map(|(_, e)| e),
        request_id,
    ))
}

/// D4: 受信箱で integrate task を選べる依頼か（origin `delivery` で、まだ結んでいない）。
pub fn offerable(origin: &str, link: Option<&IntegrateTaskLink>) -> bool {
    origin == DELIVERY_ORIGIN && link.is_none()
}

/// D4: 元の task に、integrate task が失敗して人に回した未回答の依頼があれば、その理由。
pub fn escalated_reason(
    store: &dyn TaskStore,
    task_id: TaskId,
) -> Result<Option<String>, StoreError> {
    for open in open_requests(store)?
        .into_iter()
        .filter(|o| o.task_id == task_id)
    {
        if let Some(IntegrateTaskLink::Escalated { task_id, detail }) =
            link_for(store, open.task_id, &open.request_id)?
        {
            return Ok(Some(format!(
                "integrate task {task_id} failed; a person decides this integration ({detail})"
            )));
        }
    }
    Ok(None)
}

/// D1: 先頭のリポジトリの `[commands] check`（ローカルの repo だけ。読めなければ空）。
pub fn repo_checks(store: &dyn TaskStore, task: &Task) -> Result<Vec<String>, StoreError> {
    let Some(first) = task.repos.first() else {
        return Ok(Vec::new());
    };
    let Some(repo) = store.repo_get(first.repo_id)? else {
        return Ok(Vec::new());
    };
    let WorkspaceSpec::Local { path, .. } = repo.location else {
        return Ok(Vec::new());
    };
    let (config, _) = task_core::workspace_config::load_or_default(&path);
    Ok(config.commands.check)
}

/// D1: integrate task の起票内容。受け入れ条件は両側の取り込み（git の祖先判定）・リポジトリの検査・reviewer。
pub fn spec(
    original: &Task,
    request: &IntegrationRequest,
    request_id: &str,
    checks: &[String],
    note: Option<&str>,
) -> Result<NewTaskSpec, serde_json::Error> {
    let mut acceptance = vec![serde_json::json!({
        "type": "command",
        "cmd": format!(
            "git merge-base --is-ancestor {} HEAD && git merge-base --is-ancestor {} HEAD",
            request.source_sha, request.target_sha
        ),
    })];
    for cmd in checks {
        acceptance.push(serde_json::json!({"type": "command", "cmd": cmd}));
    }
    acceptance.push(serde_json::json!({
        "type": "reviewer",
        "text": format!(
            "{} の変更と {} の変更の両方の意図が残り、衝突の解き方に誤りがない",
            request.source_branch, request.target_branch
        ),
    }));
    let mut objective = format!(
        "統合の依頼 {request_id} の衝突を解く（ADR 2026-10-28-cos-integrate-task）。\n\n\
         1. この作業ツリー（{target} の先端から切った branch）で `git merge --no-ff {source}` を実行し、\
         元の task {task}「{title}」の変更（{source_branch}）を取り込む。\n\
         2. 衝突は両側の意図を保って解く。片側を捨てて解かない。\n\
         3. リポジトリの検査コマンドを通してから commit する。\n\
         4. {target} への取り込みは celeris の配送が行う。自分で {target} を動かさない。\n\n",
        target = request.target_branch,
        source = request.source_sha,
        task = original.id,
        title = original.title,
        source_branch = request.source_branch,
    );
    if let Some(note) = note.filter(|n| !n.trim().is_empty()) {
        objective.push_str(&format!("依頼者のメモ: {note}\n\n"));
    }
    objective.push_str(&request.to_markdown());
    let value = serde_json::json!({
        "title": format!("統合の衝突を解く: {}", original.title),
        "objective": objective,
        "acceptance": acceptance,
        "project_id": original.project_id,
        "repos": original.repos.iter().map(|r| r.name.clone()).collect::<Vec<_>>(),
        "genre": original.genre,
        "role": original.role,
        "skills": original.skills,
        "labels": [LABEL],
    });
    serde_json::from_value(value)
}

/// [`settle`] の 1 件の結果（試験と log 用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// integrate task の取り込みで元の依頼を閉じた。
    Integrated {
        task_id: TaskId,
        request_id: String,
        integrate_task_id: TaskId,
    },
    /// integrate task が失敗し、依頼を人に回した。
    Escalated {
        task_id: TaskId,
        request_id: String,
        integrate_task_id: TaskId,
    },
}

/// D3: 結んだ integrate task の決着を記録する。done で `source_sha` が既定ブランチの祖先なら依頼を閉じ、
/// failed / cancelled なら人に回す。それ以外（走行中・done だが未取り込み）は何もしない。
pub fn settle(store: &dyn TaskStore) -> Result<Vec<Outcome>, StoreError> {
    let mut outcomes = Vec::new();
    for open in open_requests(store)? {
        let Some(IntegrateTaskLink::Active { task_id: worker }) =
            link_for(store, open.task_id, &open.request_id)?
        else {
            continue;
        };
        let Some(integrate) = store.get(worker)? else {
            continue;
        };
        match integrate.status {
            Status::Done => {
                let Some(original) = store.get(open.task_id)? else {
                    continue;
                };
                let mut paths = Vec::new();
                for reference in &original.repos {
                    if let Some(repo) = store.repo_get(reference.repo_id)?
                        && let WorkspaceSpec::Local { path, .. } = repo.location
                    {
                        paths.push(path);
                    }
                }
                // store の呼び出しの外で git を見る。
                if !paths.iter().any(|path| {
                    is_ancestor(path, &open.request.source_sha, &open.request.target_branch)
                }) {
                    continue;
                }
                let note = format!(
                    "integrate task {worker} の取り込みで {} が {} を含んだ",
                    open.request.target_branch, open.request.source_sha
                );
                if store.integration_request_answer(
                    open.task_id,
                    &open.request_id,
                    INTEGRATED_ANSWER,
                    Some(&note),
                    task_core::DAEMON_ACTOR,
                )? {
                    store.append_event(
                        open.task_id,
                        &Event::IntegrateTaskSettled {
                            request_id: open.request_id.clone(),
                            integrate_task_id: worker,
                            outcome: INTEGRATE_TASK_INTEGRATED.to_owned(),
                            detail: note,
                        },
                    )?;
                    outcomes.push(Outcome::Integrated {
                        task_id: open.task_id,
                        request_id: open.request_id,
                        integrate_task_id: worker,
                    });
                }
            }
            Status::Failed | Status::Cancelled => {
                store.append_event(
                    open.task_id,
                    &Event::IntegrateTaskSettled {
                        request_id: open.request_id.clone(),
                        integrate_task_id: worker,
                        outcome: INTEGRATE_TASK_ESCALATED.to_owned(),
                        detail: format!(
                            "integrate task {worker} は {}",
                            if integrate.status == Status::Failed {
                                "failed"
                            } else {
                                "cancelled"
                            }
                        ),
                    },
                )?;
                outcomes.push(Outcome::Escalated {
                    task_id: open.task_id,
                    request_id: open.request_id,
                    integrate_task_id: worker,
                });
            }
            _ => {}
        }
    }
    Ok(outcomes)
}

fn is_ancestor(repo: &Path, sha: &str, branch: &str) -> bool {
    git_ok(
        repo,
        &[
            "merge-base",
            "--is-ancestor",
            sha,
            &format!("refs/heads/{branch}"),
        ],
    )
}

fn git_ok(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(test)]
mod tests;
