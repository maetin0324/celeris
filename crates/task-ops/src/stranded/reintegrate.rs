//! ADR 2026-10-27-done-task-auto-reintegrate: done の後に main が進んで unintegrated になった task を、衝突しなければ
//! 既存の再 review 経路（`Trigger::Rereview` → review 前同期 → 必須 check → reviewer 再利用 → 配送の ff）に乗せる。
//! 判定は git だけで行い、LLM を呼ばない。store のロックを持ったまま git を呼ばない（store の呼び出しは 1 回ずつ）。
use std::{collections::BTreeSet, path::Path};

use task_core::{
    DeliveryState, Event, RepoId, Status, StoreError, TaskId, TaskRepoIntegrationStatus, TaskStore,
    Trigger,
};
use time::OffsetDateTime;

use super::{BranchRecord, git_output, git_status};

/// sweep が「main が進んだだけ」の行に書く理由（`super::sweep` と同じ文字列）。
pub const NOT_ANCESTOR: &str = "recorded head is not an ancestor of the repository default branch";
/// D4・D5: 自動の再取り込みが残す理由の接頭辞。sweep はこの理由を上書きしない。
pub const REASON_PREFIX: &str = "auto reintegrate:";
const CONFLICT_PREFIX: &str = "auto reintegrate: merge conflict";
/// D5: 1 つの task の自動の再取り込みの上限（人の reopen / rereview で数え直す）。
pub const MAX_AUTO_REINTEGRATIONS: u32 = 2;
pub const ACTOR: &str = "auto-reintegrate";

/// D2 の判定結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    /// merge-tree が clean で、main 側の変更 file が task の差分と重ならない。
    Disjoint { target_sha: String },
    /// merge-tree は clean だが、同じ file を両側が変えている（reviewer もやり直す）。
    Overlap {
        target_sha: String,
        files: Vec<String>,
    },
    /// merge-tree が衝突する。
    Conflict {
        target_sha: String,
        files: Vec<String>,
    },
    /// git が失敗した。何もせず次の周期で再判定する。
    Unknown { reason: String },
}

/// D2: `head` を `default_branch` へ取り込めるかを git だけで判定する。
pub fn classify(repo: &Path, head: &str, default_branch: &str) -> Class {
    let target_ref = format!("refs/heads/{default_branch}");
    let Some(target_sha) = git_output(repo, &["rev-parse", "--verify", &target_ref]) else {
        return Class::Unknown {
            reason: format!("cannot resolve {default_branch}"),
        };
    };
    let Some(base) = git_output(repo, &["merge-base", &target_sha, head]) else {
        return Class::Unknown {
            reason: "no merge base".into(),
        };
    };
    let changed = |from: &str, to: &str| -> Option<BTreeSet<String>> {
        git_output(repo, &["diff", "--name-only", from, to])
            .map(|out| out.lines().map(str::to_owned).collect())
    };
    let (Some(target_files), Some(task_files)) =
        (changed(&base, &target_sha), changed(&base, head))
    else {
        return Class::Unknown {
            reason: "git diff failed".into(),
        };
    };
    let overlap: Vec<String> = target_files.intersection(&task_files).cloned().collect();
    let merge = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            &target_sha,
            head,
        ])
        .output();
    match merge.map(|o| (o.status.code(), o.stdout)) {
        Ok((Some(0), _)) if overlap.is_empty() => Class::Disjoint { target_sha },
        Ok((Some(0), _)) => Class::Overlap {
            target_sha,
            files: overlap,
        },
        Ok((Some(1), stdout)) => {
            // 1 行目は tree の id、続く行が衝突した file。
            let files: BTreeSet<String> = String::from_utf8_lossy(&stdout)
                .lines()
                .skip(1)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
            Class::Conflict {
                target_sha,
                files: files.into_iter().collect(),
            }
        }
        Ok((code, _)) => Class::Unknown {
            reason: format!("git merge-tree exited with {code:?}"),
        },
        Err(e) => Class::Unknown {
            reason: format!("git merge-tree failed: {e}"),
        },
    }
}

/// D5: 直近の人の再開（`DoneReintegrationStarted` を伴わない `rereview`/`reopen`）以後の自動の再取り込みの数。
pub fn attempts(events: &[Event], repo: RepoId) -> u32 {
    let mut count = 0;
    for (i, event) in events.iter().enumerate() {
        match event {
            Event::DoneReintegrationStarted { repo_id, .. } if *repo_id == repo => count += 1,
            Event::Transitioned { reason, .. }
                if (reason == "rereview" || reason == "reopen")
                    && !matches!(
                        events.get(i + 1),
                        Some(Event::DoneReintegrationStarted { .. })
                    ) =>
            {
                count = 0
            }
            _ => {}
        }
    }
    count
}

/// 1 回の [`run`] で起きたこと（試験と log 用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Started {
        task_id: TaskId,
        repo_id: RepoId,
        class: &'static str,
        attempt: u32,
    },
    Deferred {
        task_id: TaskId,
        repo_id: RepoId,
        reason: String,
    },
    /// git の失敗・遷移の失敗。何も記録せず次の周期でやり直す（呼び出し側が log に出す）。
    Retry { task_id: TaskId, reason: String },
}

struct Candidate<'a> {
    record: &'a BranchRecord,
    repo: &'a Path,
    default_branch: &'a str,
    head: &'a str,
    done_at: OffsetDateTime,
    attempt: u32,
}

/// D1〜D6: sweep の後に呼ぶ。条件を満たす `unintegrated` 行を判定し、clean なら再 review へ戻し、衝突・上限は
/// 受信箱へ理由を残す。1 回の呼び出しで再 review を始めるのは repo ごとに高々 1 件（`done_at` の古い順）。
pub fn run(
    store: &dyn TaskStore,
    records: &[BranchRecord],
    now: OffsetDateTime,
) -> Result<Vec<Outcome>, StoreError> {
    let mut candidates = Vec::new();
    for record in records
        .iter()
        .filter(|r| r.done && r.root && r.changed_repo && !r.not_applicable)
    {
        let (Some(repo), Ok(default_branch), Some(head)) = (
            record.repo_path.as_deref(),
            record.default_branch.as_deref(),
            record.head.as_deref().filter(|h| !h.is_empty()),
        ) else {
            continue;
        };
        // D1.3・D1.4: main が進んだだけの行（衝突で見送った行は再判定する）。held / discarded・上限到達は触らない。
        let Some(row) = store.task_integration_get(record.task_id, record.repo_id)? else {
            continue;
        };
        let reason = row.reason.as_deref().unwrap_or("");
        if row.state != TaskRepoIntegrationStatus::Unintegrated
            || row.head_sha != head
            || !(reason == NOT_ANCESTOR || reason.starts_with(CONFLICT_PREFIX))
        {
            continue;
        }
        let Some(task) = store.get(record.task_id)? else {
            continue;
        };
        if task.status != Status::Done || task.parent_id.is_some() {
            continue;
        }
        // D1.2: 案件の自動取り込みが有効なものだけ。
        let enabled = match task.project_id {
            Some(project) => store
                .project_get(project)?
                .is_some_and(|p| p.auto_integrate),
            None => false,
        };
        if !enabled {
            continue;
        }
        // D1.5: 走行中の配送が自分で扱う。
        if store.delivery_get(task.id)?.is_some_and(|d| {
            matches!(
                d.state,
                DeliveryState::Reviewing
                    | DeliveryState::MergeQueued
                    | DeliveryState::Merging
                    | DeliveryState::Preparing
            )
        }) {
            continue;
        }
        let events: Vec<Event> = store
            .events_for(task.id)?
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        let done = attempts(&events, record.repo_id);
        if done >= MAX_AUTO_REINTEGRATIONS {
            // D5: 上限。行の理由を変えて受信箱に残し、人の操作を待つ。
            let reason = format!("{REASON_PREFIX} limit reached after {done} attempts");
            let target = git_output(
                repo,
                &["rev-parse", &format!("refs/heads/{default_branch}")],
            )
            .unwrap_or_default();
            defer(store, record, head, &target, &reason, &events, now)?;
            continue;
        }
        candidates.push(Candidate {
            record,
            repo,
            default_branch,
            head,
            done_at: task.updated_at,
            attempt: done + 1,
        });
    }
    candidates.sort_by_key(|c| (c.done_at, c.record.task_id.to_string()));
    let mut started_repos = BTreeSet::new();
    let mut outcomes = Vec::new();
    for c in candidates {
        let (task_id, repo_id) = (c.record.task_id, c.record.repo_id);
        if started_repos.contains(&repo_id) {
            continue;
        }
        let (class, target_sha) = match classify(c.repo, c.head, c.default_branch) {
            Class::Disjoint { target_sha } => ("disjoint", target_sha),
            Class::Overlap { target_sha, .. } => ("overlap", target_sha),
            Class::Conflict { target_sha, files } => {
                let reason = format!("{CONFLICT_PREFIX} in {}", files.join(", "));
                let events: Vec<Event> = store
                    .events_for(task_id)?
                    .into_iter()
                    .map(|(_, e)| e)
                    .collect();
                defer(store, c.record, c.head, &target_sha, &reason, &events, now)?;
                record_conflict_request(store, &c, &target_sha, files)?;
                outcomes.push(Outcome::Deferred {
                    task_id,
                    repo_id,
                    reason,
                });
                continue;
            }
            Class::Unknown { reason } => {
                // D4: git の失敗は理由を変えず、次の周期で再判定する。
                outcomes.push(Outcome::Retry { task_id, reason });
                continue;
            }
        };
        let started = Event::DoneReintegrationStarted {
            repo_id,
            head_sha: c.head.to_owned(),
            target_sha,
            class: class.into(),
            attempt: c.attempt,
        };
        // D5: 遷移と event は同じ transaction。遷移できなければ（並走で状態が変わった）event も残らない。
        match store.apply_transition_with_events(task_id, Trigger::Rereview, vec![started]) {
            Ok(_) => {
                started_repos.insert(repo_id);
                outcomes.push(Outcome::Started {
                    task_id,
                    repo_id,
                    class,
                    attempt: c.attempt,
                });
            }
            Err(e) => outcomes.push(Outcome::Retry {
                task_id,
                reason: format!("rereview transition failed: {e}"),
            }),
        }
    }
    Ok(outcomes)
}

/// D4・D5: 行の理由を `reason` にし、同じ理由の `DoneReintegrationDeferred` が直近に無ければ追記する。
fn defer(
    store: &dyn TaskStore,
    record: &BranchRecord,
    head: &str,
    target: &str,
    reason: &str,
    events: &[Event],
    now: OffsetDateTime,
) -> Result<bool, StoreError> {
    if let Some(mut row) = store.task_integration_get(record.task_id, record.repo_id)?
        && row.reason.as_deref() != Some(reason)
    {
        row.reason = Some(reason.to_owned());
        row.actor = ACTOR.into();
        row.checked_at = now;
        row.updated_at = now;
        store.task_integration_upsert(&row)?;
    }
    let last = events.iter().rev().find_map(|e| match e {
        Event::DoneReintegrationDeferred {
            repo_id,
            head_sha,
            reason,
            ..
        } if *repo_id == record.repo_id => Some((head_sha.as_str(), reason.as_str())),
        Event::DoneReintegrationStarted { .. } | Event::Transitioned { .. } => Some(("", "")),
        _ => None,
    });
    if last == Some((head, reason)) {
        return Ok(false);
    }
    store.append_event(
        record.task_id,
        &Event::DoneReintegrationDeferred {
            repo_id: record.repo_id,
            head_sha: head.to_owned(),
            target_sha: target.to_owned(),
            reason: reason.to_owned(),
        },
    )?;
    Ok(true)
}

/// D4: 衝突は人の統合依頼（origin = delivery）にも出す。同じ両側の head では 1 件（`IntegrationRequest::id_for`）。
fn record_conflict_request(
    store: &dyn TaskStore,
    c: &Candidate<'_>,
    target_sha: &str,
    files: Vec<String>,
) -> Result<(), StoreError> {
    use task_core::integration_request::{FileIntent, IntegrationRequest, SideIntent};
    let branch = c.record.branch.clone().unwrap_or_default();
    let side = |branch: &str, path: &str| SideIntent {
        branch: branch.into(),
        path: path.into(),
        commits: vec![],
        diffstat: None,
        unavailable: Some("auto reintegrate stopped at a merge conflict".into()),
    };
    let request = IntegrationRequest {
        target_branch: c.default_branch.into(),
        target_sha: target_sha.into(),
        source_branch: branch.clone(),
        source_sha: c.head.into(),
        merge_base: git_output(c.repo, &["merge-base", target_sha, c.head]),
        conflict_files: files.clone(),
        intent: files
            .iter()
            .map(|path| FileIntent {
                path: path.clone(),
                target: side(c.default_branch, path),
                source: side(&branch, path),
            })
            .collect(),
        reason: format!(
            "done の後に {} が進み、取り込み直しが衝突しました",
            c.default_branch
        ),
        recommendation: "衝突を解いてから取り込むか、保留・破棄を選んでください".into(),
        actions: vec![],
        candidate_sha: None,
    };
    // git の確認だけ（作業ツリーは触らない）。branch が消えていれば依頼は作らない。
    if git_status(c.repo, &["rev-parse", "--verify", c.head]).is_none() {
        return Ok(());
    }
    store.integration_request_record(c.record.task_id, &request, "delivery")?;
    Ok(())
}

#[cfg(test)]
mod tests;
