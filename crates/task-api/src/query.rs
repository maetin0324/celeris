//! クエリ文字列と path の値の解析。未知のキー・重複・型誤り・ULID でない id は 400 `bad_request`（api.md §1.5）。

use std::collections::HashSet;

use serde::de::DeserializeOwned;
use task_core::{AccountAdapter, Event, TaskId};

use crate::problem::ApiProblem;

pub(crate) struct QueryParams {
    pairs: Vec<(String, String)>,
}

impl QueryParams {
    /// `allowed` 以外のキーがあれば 400。
    pub(crate) fn parse(raw: Option<&str>, allowed: &[&str]) -> Result<Self, ApiProblem> {
        let pairs: Vec<(String, String)> =
            form_urlencoded::parse(raw.unwrap_or_default().as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
        if let Some((key, _)) = pairs.iter().find(|(k, _)| !allowed.contains(&k.as_str())) {
            return Err(ApiProblem::bad_request(format!(
                "unknown query parameter `{key}`"
            )));
        }
        Ok(Self { pairs })
    }

    /// 1 回だけ現れるべきキーの値（重複は 400）。
    pub(crate) fn single(&self, key: &str) -> Result<Option<&str>, ApiProblem> {
        let mut values = self
            .pairs
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str());
        let first = values.next();
        if values.next().is_some() {
            return Err(ApiProblem::bad_request(format!(
                "query parameter `{key}` must not be repeated"
            )));
        }
        Ok(first)
    }

    /// 複数可のキー（`?k=a&k=b` と `?k=a,b` の両方）。空要素は捨てる。
    pub(crate) fn list(&self, key: &str) -> Vec<&str> {
        self.pairs
            .iter()
            .filter(|(k, _)| k == key)
            .flat_map(|(_, v)| v.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub(crate) fn u64(&self, key: &str) -> Result<Option<u64>, ApiProblem> {
        self.single(key)?
            .map(|v| {
                v.trim().parse::<u64>().map_err(|_| {
                    ApiProblem::bad_request(format!(
                        "query parameter `{key}` must be a non-negative integer"
                    ))
                })
            })
            .transpose()
    }

    pub(crate) fn i64(&self, key: &str) -> Result<Option<i64>, ApiProblem> {
        self.single(key)?
            .map(|v| {
                v.trim().parse::<i64>().map_err(|_| {
                    ApiProblem::bad_request(format!("query parameter `{key}` must be an integer"))
                })
            })
            .transpose()
    }

    pub(crate) fn bool(&self, key: &str) -> Result<Option<bool>, ApiProblem> {
        match self.single(key)? {
            None => Ok(None),
            Some("true" | "1") => Ok(Some(true)),
            Some("false" | "0") => Ok(Some(false)),
            Some(_) => Err(ApiProblem::bad_request(format!(
                "query parameter `{key}` must be true, false, 1 or 0"
            ))),
        }
    }

    pub(crate) fn task_id(&self, key: &str) -> Result<Option<TaskId>, ApiProblem> {
        self.single(key)?.map(parse_task_id).transpose()
    }

    /// `limit`: 省略時 `default`、0 は 400、`max` 超は `max` に丸める。
    pub(crate) fn limit(&self, key: &str, default: usize, max: usize) -> Result<usize, ApiProblem> {
        match self.u64(key)? {
            None => Ok(default),
            Some(0) => Err(ApiProblem::bad_request(format!(
                "query parameter `{key}` must be at least 1"
            ))),
            Some(n) => Ok(usize::try_from(n).unwrap_or(usize::MAX).min(max)),
        }
    }

    /// ADR-0025 D6: `?adapter=` クエリ（省略時 `claude-code`）。`"claude-code"` / `"codex"` 以外は 400。
    pub(crate) fn account_adapter(&self) -> Result<AccountAdapter, ApiProblem> {
        match self.single("adapter")? {
            None => Ok(AccountAdapter::ClaudeCode),
            Some(s) => AccountAdapter::parse(s).ok_or_else(|| {
                ApiProblem::bad_request(format!(
                    "unknown adapter `{s}` (must be claude-code or codex)"
                ))
            }),
        }
    }

    /// `types`: `Event` の `type` 名のカンマ区切り。未知の名前は 400。省略（または空）は `None` = 全て。
    pub(crate) fn event_types(&self) -> Result<Option<HashSet<&'static str>>, ApiProblem> {
        let names = self.list("types");
        if names.is_empty() {
            return Ok(None);
        }
        let mut set = HashSet::new();
        for name in names {
            match EVENT_TYPES.iter().find(|known| **known == name) {
                Some(known) => {
                    set.insert(*known);
                }
                None => {
                    return Err(ApiProblem::bad_request(format!(
                        "unknown event type `{name}`"
                    )));
                }
            }
        }
        Ok(Some(set))
    }
}

pub(crate) fn parse_task_id(value: &str) -> Result<TaskId, ApiProblem> {
    value
        .parse::<TaskId>()
        .map_err(|_| ApiProblem::bad_request(format!("`{value}` is not a task id (ULID)")))
}

/// `snake_case` の列挙（`Status` / `TaskKind`）を serde 表現のまま解析する。
pub(crate) fn parse_snake<T: DeserializeOwned>(what: &str, value: &str) -> Result<T, ApiProblem> {
    serde_json::from_value(serde_json::Value::String(value.to_string()))
        .map_err(|_| ApiProblem::bad_request(format!("unknown {what} `{value}`")))
}

/// `Event` の serde の `type` 名（`types` クエリの語彙）。
pub(crate) const EVENT_TYPES: [&str; 50] = [
    "browser_updated",
    // ADR-0080 D4/D5: browser の人待ち（登録依頼・承認）を開いた・解決した（秘密なし）。
    "browser_wait_opened",
    "browser_wait_resolved",
    // ADR-0090 D3: クラスタ job の durable wait（開いた・状態が変わった・終わった）。
    "cluster_job_wait_started",
    "cluster_job_wait_polled",
    "cluster_job_wait_finished",
    "created",
    "transitioned",
    "worker_started",
    "worker_progress",
    "artifact_produced",
    "worker_finished",
    "review_verdict",
    "approval_requested",
    "approval_decided",
    // Phase F7: 認可元のタスクが終端になり、未決の認可の要求を celeris が取り下げた。
    "approvals_withdrawn",
    "answered",
    "provider_throttled",
    "cluster_unavailable",
    "delegated",
    "question_raised",
    "retried",
    "edited",
    // ADR-0046 D5（Phase 59）: matching が担当を決めた。
    "assigned",
    // ADR-0059 D3（Phase 99）: worktree が切れず `shared` に格下げした。
    "workspace_mode_downgraded",
    // ADR-0062 A（Phase 107）: celeris が保持していた ssh master が明示的な切断を経ずに終了した。
    "cluster_master_exited",
    // ADR-0066 D2（Phase 110b）: 終端タスクの作業場所からビルド生成物を刈った。
    "workspace_pruned",
    // ADR-0069 D5（Phase 114）: run ごとの routing の監査記録。
    "routing_decided",
    // ADR-0072 D5（Phase E1）: run 終了時に確定させた checkpoint。
    "checkpoint_saved",
    // ADR-0072 D5（Phase E2）: 計画の採用（新規または replan）。
    "execution_planned",
    // ADR-0072 D5（Phase E2）: WorkUnit の状態遷移。
    "work_unit_transitioned",
    // ADR-0079 R5b-fix1: 人の replan が done の WorkUnit の spec を上書きした。
    "work_unit_spec_overridden",
    "work_unit_checks_failed",
    // ADR-0072 D5/D13（Phase E3）: Complexity Gate の判定。
    "execution_gated",
    // ADR-0072「Phase F6 実装時の決定」: 起票済みの Task の実行の形を人が後から決めた。
    "execution_hint_set",
    // ADR-0074 D6.2（Phase F1）: repair WU を起こしたこと（class・起こした場所）。
    "repair_scheduled",
    // ADR-0074 D4.3（Phase F3）: run 1 件の quota 消費の推定。
    "quota_estimated",
    // ADR-0074 D2.1（Phase F3 途中確認）: `pause_after` を工程の key の集合へ解決した結果。
    "pause_points_resolved",
    // ADR-0074 D2.3（Phase F3 途中確認）: 停止点の工程の統合の後の決定的な途中報告。
    "phase_reported",
    // ADR-0074 D8.1（Phase F4a）: CoS が案件計画を提案した。
    "project_plan_proposed",
    // ADR-0074 D8.2（Phase F4a）: 人が案件計画の提案を採否決定した。
    "project_plan_decided",
    // ADR-0079（Phase R1a）: 再帰的な task の木（型と replay の読みだけ。発行は R1b 以降）。
    "child_task_created",
    "child_adopted",
    "unit_gate_overridden",
    "decision_requested",
    "decision_answered",
    "decision_withdrawn",
    "plan_approval_requested",
    "stall_detected",
    // ADR-0119 D3: root の取り込みを開始できなかった理由。
    "delivery_skipped",
];

pub(crate) fn event_type_name(event: &Event) -> &'static str {
    match event {
        Event::Created { .. } => "created",
        Event::BrowserUpdated { .. } => "browser_updated",
        Event::BrowserWaitOpened { .. } => "browser_wait_opened",
        Event::BrowserWaitResolved { .. } => "browser_wait_resolved",
        Event::ClusterJobWaitStarted { .. } => "cluster_job_wait_started",
        Event::ClusterJobWaitPolled { .. } => "cluster_job_wait_polled",
        Event::ClusterJobWaitFinished { .. } => "cluster_job_wait_finished",
        Event::Transitioned { .. } => "transitioned",
        Event::WorkerStarted { .. } => "worker_started",
        Event::WorkerProgress { .. } => "worker_progress",
        Event::ArtifactProduced { .. } => "artifact_produced",
        Event::WorkerFinished { .. } => "worker_finished",
        Event::ReviewVerdict { .. } => "review_verdict",
        Event::ApprovalRequested => "approval_requested",
        Event::ApprovalDecided { .. } => "approval_decided",
        Event::ApprovalsWithdrawn { .. } => "approvals_withdrawn",
        Event::Answered { .. } => "answered",
        Event::ProviderThrottled { .. } => "provider_throttled",
        Event::ClusterUnavailable { .. } => "cluster_unavailable",
        Event::Delegated { .. } => "delegated",
        Event::QuestionRaised { .. } => "question_raised",
        Event::Retried { .. } => "retried",
        Event::Edited { .. } => "edited",
        Event::Assigned { .. } => "assigned",
        Event::WorkspaceModeDowngraded { .. } => "workspace_mode_downgraded",
        Event::ClusterMasterExited { .. } => "cluster_master_exited",
        Event::WorkspacePruned { .. } => "workspace_pruned",
        Event::RoutingDecided { .. } => "routing_decided",
        Event::CheckpointSaved { .. } => "checkpoint_saved",
        Event::ExecutionPlanned { .. } => "execution_planned",
        Event::WorkUnitTransitioned { .. } => "work_unit_transitioned",
        Event::WorkUnitSpecOverridden { .. } => "work_unit_spec_overridden",
        Event::WorkUnitChecksFailed { .. } => "work_unit_checks_failed",
        Event::ExecutionGated { .. } => "execution_gated",
        Event::ExecutionHintSet { .. } => "execution_hint_set",
        Event::RepairScheduled { .. } => "repair_scheduled",
        Event::QuotaEstimated { .. } => "quota_estimated",
        Event::WorkUnitCommitted { .. } => "work_unit_committed",
        Event::PhaseIntegrated { .. } => "phase_integrated",
        Event::WorkUnitsSerialized { .. } => "work_units_serialized",
        Event::ProjectPlanProposed { .. } => "project_plan_proposed",
        Event::ProjectPlanDecided { .. } => "project_plan_decided",
        Event::PausePointsResolved { .. } => "pause_points_resolved",
        Event::PhaseReported { .. } => "phase_reported",
        Event::ChildTaskCreated { .. } => "child_task_created",
        Event::ChildAdopted { .. } => "child_adopted",
        Event::UnitGateOverridden { .. } => "unit_gate_overridden",
        Event::DecisionRequested { .. } => "decision_requested",
        Event::DecisionAnswered { .. } => "decision_answered",
        Event::DecisionWithdrawn { .. } => "decision_withdrawn",
        Event::PlanApprovalRequested { .. } => "plan_approval_requested",
        Event::StallDetected { .. } => "stall_detected",
        Event::DeliverySkipped { .. } => "delivery_skipped",
    }
}

#[cfg(test)]
#[path = "query/tests.rs"]
mod tests;
