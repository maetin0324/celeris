//! 報告の生成（ADR-0033 D3。Phase 25。監査による修正は ADR-0034 D2/D7）。
//!
//! 報告は「run の終端」ではなく「**タスクの終端状態**」に合わせて 1 件作る。**LLM は呼ばない**
//! （DESIGN 原則 1）: 文面は結果ファイルの `summary` / `evidence` と、その run が出した成果物の一覧から
//! 決定的に組む（文面の組み立て自体は `task_core::report` の純粋関数）。
//!
//! - `assignee` が無いタスクでは何も作らない（従来どおりのタスクは報告の対象外）。`assignee` があっても
//!   組織に存在しないノードなら、報告は作らず `warn!` する（監査 M-5。level 0 の秘書の報告に化けるのを防ぐ）。
//! - `result`（`kind = result`）は、レビューを通って `Status::Done` になったときに作る。ワーカーの
//!   「できました」がレビューで差し戻された場合は作らない（DESIGN 原則 4）。
//! - `bad_news` は、`Status::Failed` になったときに原因を問わず 1 件作る（ワーカーの `error`、供給側失敗の
//!   requeue 上限到達、レビュー不合格のどれでも）。生成と同時に各祖先へ複製する（圧縮を待たない。SPEC §2.4）。
//!   `retryable` でまだ `ready` に戻るだけの途中の失敗は作らない。
//! - `question` は run の終端（人の返事を待つ状態は即知らせる）。
//! - まとめの run（`role = COMPACTION_ROLE`）の `done` は、親ノードの報告になり `sources` に子の報告が入る。
//!   `sources` は同じ案件の子報告に限る（監査 H-1。案件をまたいで混ざらない）。
//! - 途中経過（`progress`）は作らない（通知は数時間単位なので run の途中は要らない）。

use task_core::report::{self, Report};
use task_core::{Event, StoreError, Task, TaskId, TaskStore};
use task_worker::{AdapterError, Evidence, RunOutcome, Terminal};
use time::OffsetDateTime;

/// run の終端のうち、報告に必要な部分だけを取り出したもの。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TerminalReport {
    Done {
        summary: String,
        evidence: Vec<String>,
    },
    Error {
        message: String,
        retryable: bool,
    },
    Question {
        text: String,
    },
}

/// `evidence` を報告の本文用に整形する（ワーカー run の直接の `Done` からでも、レビュー後の `entry.subject` からでも使う）。
pub(crate) fn format_evidence(evidence: &[Evidence]) -> Vec<String> {
    evidence
        .iter()
        .map(|e| {
            let mut line = format!("条件 {}", e.criterion);
            if let Some(cmd) = &e.command {
                line.push_str(&format!(": {cmd}"));
            }
            if let Some(exit) = e.exit {
                line.push_str(&format!(" → exit {exit}"));
            }
            line
        })
        .collect()
}

/// アダプタの結果から報告の材料を取り出す。`Err`（供給側の失敗）は `None`
/// （呼び出し側が `outcome.next == Status::Failed` のときだけ別途 bad_news を組む。ADR-0034 D2）。
pub(crate) fn terminal_report(result: &Result<RunOutcome, AdapterError>) -> Option<TerminalReport> {
    match result {
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary, evidence, ..
            },
            ..
        }) => Some(TerminalReport::Done {
            summary: summary.clone(),
            evidence: format_evidence(evidence),
        }),
        Ok(RunOutcome {
            terminal: Terminal::Error { message, retryable },
            ..
        }) => Some(TerminalReport::Error {
            message: message.clone(),
            retryable: *retryable,
        }),
        Ok(RunOutcome {
            terminal: Terminal::Question { text },
            ..
        }) => Some(TerminalReport::Question { text: text.clone() }),
        // ADR-0072 D9/D11（Phase E1）: yield / 予算切れは `Continue` で続くので、この時点では
        // 「タスクの終端状態」に至っていない（bad_news/result の材料にしない）。
        Ok(RunOutcome {
            terminal:
                Terminal::Yielded { .. } | Terminal::BudgetExhausted { .. } | Terminal::Waiting { .. },
            ..
        }) => None,
        Err(_) => None,
    }
}

/// ADR-0034 D7: 結果ファイルの `report.kind`（ワーカーの自己申告）を `ReportKind` に写す**固定表**。
/// `"proposal"` だけが `Proposal` になり、それ以外・未知の値・欠落は `Result`（従来の規則）。
/// 判断（この結果が提案に値するか）はしない（DESIGN 原則 1）。
pub(crate) fn declared_kind(declared: Option<&str>) -> report::ReportKind {
    match declared {
        Some("proposal") => report::ReportKind::Proposal,
        _ => report::ReportKind::Result,
    }
}

/// その run が出した成果物の一覧（`ArtifactProduced` イベントから。決定的）。
fn artifacts_of_run(
    store: &dyn TaskStore,
    task_id: TaskId,
    run_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut out = Vec::new();
    for (_, event) in store.events_for(task_id)? {
        if let Event::ArtifactProduced {
            run_id: rid,
            artifact,
        } = event
            && rid == run_id
        {
            out.push(format!("{} ({})", artifact.name, artifact.path));
        }
    }
    Ok(out)
}

/// 終端に達した run から、担当ノードの報告を作って追記する（`assignee` が無ければ何もしない）。
/// 追記は `reports` 表への INSERT だけで、タスクの状態遷移とは別（ADR-0033 D3）。
pub(crate) fn record_run_report(
    store: &dyn TaskStore,
    task: &Task,
    run_id: &str,
    terminal: &TerminalReport,
    declared: Option<&str>,
    now: OffsetDateTime,
) -> Result<Option<Report>, StoreError> {
    // 監査 M-6: 対話用タスク（人への返事）は報告にしない。返事は `messages` に残り、GUI の「対話」で読む。
    if task_core::is_conversation(task) {
        return Ok(None);
    }
    let Some(assignee) = task.assignee.as_deref() else {
        return Ok(None);
    };
    let org = store.org_list()?;
    if !org.iter().any(|n| n.id == assignee) {
        // 監査 M-5: 未知の assignee は `level_of` が 0 を返すので「秘書の報告」に化ける。作らず警告する。
        tracing::warn!(task_id = %task.id, assignee, "reports: assignee is not a known org node; skipping report");
        return Ok(None);
    }
    let level = report::level_of(&org, assignee);
    let report = match terminal {
        TerminalReport::Done { summary, evidence } => {
            let artifacts = artifacts_of_run(store, task.id, run_id)?;
            // まとめの run なら、objective に載せた子の報告（= このタスクを作った時点で
            // レビュー待ちだったもの）を `sources` にする。1 件も無ければ報告を作らない。
            // 監査 H-1: 案件をまたいで混ざらないよう、まとめタスク自身の案件と同じ子報告に限る。
            let sources = if task.role.as_deref() == Some(report::COMPACTION_ROLE) {
                let ids: Vec<_> = store
                    .report_unreviewed_children(assignee)?
                    .into_iter()
                    .filter(|r| r.created_at <= task.created_at && r.project_id == task.project_id)
                    .map(|r| r.id)
                    .collect();
                if ids.is_empty() {
                    return Ok(None);
                }
                ids
            } else {
                Vec::new()
            };
            let mut done = report::report_for_done(
                assignee,
                level,
                task.project_id,
                task.id,
                &task.title,
                summary,
                &evidence.clone(),
                &artifacts,
                sources,
                now,
            );
            // ADR-0034 D7: ワーカーが宣言した `report.kind` を**固定表で写すだけ**（判断はしない。
            // 未知・欠落は従来どおり `result`）。
            done.kind = declared_kind(declared);
            done
        }
        TerminalReport::Error { message, retryable } => report::report_for_error(
            assignee,
            level,
            task.project_id,
            task.id,
            &task.title,
            message,
            *retryable,
            now,
        ),
        TerminalReport::Question { text } => report::report_for_question(
            assignee,
            level,
            task.project_id,
            task.id,
            &task.title,
            text,
            now,
        ),
    };
    append_with_escalation(store, &org, report).map(Some)
}

/// 報告を追記する。`bad_news` なら同じ内容を秘書まで複製する（SPEC §2.4「目立つ形で届く」）。
fn append_with_escalation(
    store: &dyn TaskStore,
    org: &[task_core::OrgNode],
    report: Report,
) -> Result<Report, StoreError> {
    let mut batch = vec![report.clone()];
    if report.kind == report::ReportKind::BadNews {
        let technical_delivery = match report.task_id {
            Some(id) => store
                .delivery_get(id)?
                .is_some_and(|d| !d.detail.trim_start().starts_with("[needs-human]")),
            None => false,
        };
        batch.extend(
            report::bad_news_chain(&report, org, report.created_at)
                .into_iter()
                .filter(|r| {
                    !technical_delivery
                        || !org
                            .iter()
                            .any(|n| n.id == r.node_id && n.kind == task_core::OrgKind::Secretary)
                }),
        );
    }
    store.report_append_all(&batch)?;
    Ok(report)
}

/// ADR-0033 D3: クラスタに接続できないこと（`Event::ClusterUnavailable`）を `infra` 相当のノードの
/// `bad_news` として 1 件記録する。案件に紐づかないので `project_id` は「案件なし」。
pub(crate) fn record_cluster_unavailable_report(
    store: &dyn TaskStore,
    cluster: &str,
    host: &str,
    reason: &str,
    task_id: Option<TaskId>,
    now: OffsetDateTime,
) -> Result<Option<Report>, StoreError> {
    let org = store.org_list()?;
    let Some(node) = report::infra_node(&org) else {
        // 組織がまだ無い（種を蒔いていない）DB では報告を作らない。
        return Ok(None);
    };
    let node_id = node.id.clone();
    let level = report::level_of(&org, &node_id);
    let report = report::report_for_cluster_unavailable(
        &node_id, level, cluster, host, reason, task_id, now,
    );
    append_with_escalation(store, &org, report).map(Some)
}

/// ADR-0053 D3（Phase 66）: クラスタの ssh master が落ち、鍵認証も失敗したこと（人の TOTP が要る）を
/// `infra` 相当のノードの `bad_news` として 1 件記録する。案件に紐づかないので `project_id` は「案件なし」。
/// 呼び出し側（`Dispatcher::mark_login_needed`）が同じ outage の間の重複を防ぐので、ここでは間引きしない。
pub(crate) fn record_cluster_login_needed_report(
    store: &dyn TaskStore,
    cluster: &str,
    host: &str,
    detail: Option<&str>,
    now: OffsetDateTime,
) -> Result<Option<Report>, StoreError> {
    let org = store.org_list()?;
    let Some(node) = report::infra_node(&org) else {
        return Ok(None);
    };
    let node_id = node.id.clone();
    let level = report::level_of(&org, &node_id);
    let report =
        report::report_for_cluster_login_needed(&node_id, level, cluster, host, detail, now);
    append_with_escalation(store, &org, report).map(Some)
}

/// 同じクラスタの障害を毎 tick 報告しないための間引き（直近 `within_secs` 以内に同じ見出しの
/// 未読の報告があれば作らない）。決定的な判定。
pub(crate) fn cluster_report_recently_recorded(
    store: &dyn TaskStore,
    host: &str,
    now: OffsetDateTime,
    within_secs: i64,
) -> Result<bool, StoreError> {
    let headline = report::truncate_chars(
        &format!("{host} に接続できない"),
        report::HEADLINE_MAX_CHARS,
    );
    // 監査 L-1: docstring どおり「未読」だけを見る（既読の古い障害報告に引きずられない）。
    let recent = store.report_list(&task_core::ReportFilter {
        level: Some(0),
        unread_only: true,
        limit: 50,
        ..Default::default()
    })?;
    Ok(recent
        .iter()
        .any(|r| r.headline == headline && (now - r.created_at).whole_seconds() < within_secs))
}

#[cfg(test)]
mod tests;
