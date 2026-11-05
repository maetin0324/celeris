//! ADR 2026-11-04-project-controller D12・A1: 案件ごとの durable wake（再評価のきっかけ）の型。
//!
//! wake は `events`・停滞の検出・cron・人の操作から作る**派生**で、別のイベントストアではない（I2）。
//! task の状態は持たない（I1）。表は `project_controller_state`・`project_wakes`・
//! `project_controller_decisions`（migration 0112）。読み書きは `store/project_wake.rs`。

use serde::{Deserialize, Serialize};

/// 再評価のきっかけの種類（ADR D12 の表。依頼の 8 種に `human_changed` を足した 9 種）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectWakeKind {
    RootDone,
    RootFailed,
    ReviewFailed,
    ChildrenSettled,
    DecisionAnswered,
    ExternalReleased,
    Stagnation,
    Cron,
    HumanChanged,
}

impl ProjectWakeKind {
    pub const ALL: [ProjectWakeKind; 9] = [
        ProjectWakeKind::RootDone,
        ProjectWakeKind::RootFailed,
        ProjectWakeKind::ReviewFailed,
        ProjectWakeKind::ChildrenSettled,
        ProjectWakeKind::DecisionAnswered,
        ProjectWakeKind::ExternalReleased,
        ProjectWakeKind::Stagnation,
        ProjectWakeKind::Cron,
        ProjectWakeKind::HumanChanged,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ProjectWakeKind::RootDone => "root_done",
            ProjectWakeKind::RootFailed => "root_failed",
            ProjectWakeKind::ReviewFailed => "review_failed",
            ProjectWakeKind::ChildrenSettled => "children_settled",
            ProjectWakeKind::DecisionAnswered => "decision_answered",
            ProjectWakeKind::ExternalReleased => "external_released",
            ProjectWakeKind::Stagnation => "stagnation",
            ProjectWakeKind::Cron => "cron",
            ProjectWakeKind::HumanChanged => "human_changed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// mode を `dry_run` に置いたときに積む wake の key の接頭辞（`dry_run:<revision>`）。
pub const DRY_RUN_WAKE_PREFIX: &str = "dry_run:";

/// 案件 mode（D11）。上げるのは人だけ、`off` へ戻すのは誰でも。
/// brief の内容変更の wake の接頭辞（`brief:<epoch>`）。
pub const BRIEF_WAKE_PREFIX: &str = "brief:";
/// 判断の後に許可が切れたときの再評価の wake の接頭辞。
pub const AUTHORITY_DENIED_WAKE_PREFIX: &str = "authority-denied:";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ControllerMode {
    #[default]
    Off,
    DryRun,
    Shadow,
    Enabled,
}

impl ControllerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ControllerMode::Off => "off",
            ControllerMode::DryRun => "dry_run",
            ControllerMode::Shadow => "shadow",
            ControllerMode::Enabled => "enabled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "off" => ControllerMode::Off,
            "dry_run" => ControllerMode::DryRun,
            "shadow" => ControllerMode::Shadow,
            "enabled" => ControllerMode::Enabled,
            _ => return None,
        })
    }

    /// wake を積み、tick で評価する mode（D11 の表: `shadow`・`enabled`）。
    pub fn wakes(self) -> bool {
        matches!(self, ControllerMode::Shadow | ControllerMode::Enabled)
    }

    /// tick で評価する mode（`dry_run`・`shadow`・`enabled`）。`dry_run` は頼まれた wake
    /// （[`ControllerMode::accepts_wake`]）だけで起き、判断を記録して効果は `suppressed`（付記 A3 の 7）。
    pub fn evaluates(self) -> bool {
        self != ControllerMode::Off
    }

    /// この mode の案件に wake を積むか。`shadow`・`enabled` は全部、`dry_run` は人・CoS の頼み
    /// （mode を `dry_run` に置いたときの `dry_run:<revision>` と `POST …/controller/wake` の外部待ちの解除）だけ。
    /// event から写した `external_released`（cluster job・browser の待ちの解除。`from_event`）は `dry_run` では積まない。
    pub fn accepts_wake(self, kind: ProjectWakeKind, wake_key: &str, from_event: bool) -> bool {
        self.wakes()
            || (self == ControllerMode::DryRun
                && !from_event
                && (kind == ProjectWakeKind::ExternalReleased
                    || wake_key.starts_with(DRY_RUN_WAKE_PREFIX)))
    }

    /// `off` より上か（上げる操作は人だけ）。
    pub fn rank(self) -> u8 {
        match self {
            ControllerMode::Off => 0,
            ControllerMode::DryRun => 1,
            ControllerMode::Shadow => 2,
            ControllerMode::Enabled => 3,
        }
    }
}

/// 外からの入力として数える wake か（TTL と `ttl` の止まりの解除）。cron・停滞の検出は daemon の照合で、
/// `brief:` は CoS 自身の要約の更新、`authority-denied:` は daemon が自分に積む再評価なので数えない。
pub fn is_external_input(kind: ProjectWakeKind, wake_key: &str) -> bool {
    !matches!(kind, ProjectWakeKind::Cron | ProjectWakeKind::Stagnation)
        && !wake_key.starts_with(BRIEF_WAKE_PREFIX)
        && !wake_key.starts_with(AUTHORITY_DENIED_WAKE_PREFIX)
}

/// controller の phase（D12。Task の running とは独立）。A1 は列と `active`・`waiting`・`blocked` の
/// 遷移を持ち、残りの遷移は A6 が完成させる。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPhase {
    #[default]
    Active,
    Waiting,
    Blocked,
    Paused,
    Achieved,
    Cancelled,
}

impl ControllerPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            ControllerPhase::Active => "active",
            ControllerPhase::Waiting => "waiting",
            ControllerPhase::Blocked => "blocked",
            ControllerPhase::Paused => "paused",
            ControllerPhase::Achieved => "achieved",
            ControllerPhase::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "active" => ControllerPhase::Active,
            "waiting" => ControllerPhase::Waiting,
            "blocked" => ControllerPhase::Blocked,
            "paused" => ControllerPhase::Paused,
            "achieved" => ControllerPhase::Achieved,
            "cancelled" => ControllerPhase::Cancelled,
            _ => return None,
        })
    }

    /// tick が評価しない phase（`blocked` は人が mode を置き直すまで止まる。I12）。
    pub fn halted(self) -> bool {
        matches!(
            self,
            ControllerPhase::Blocked | ControllerPhase::Paused | ControllerPhase::Cancelled
        )
    }
}

/// 判断の効果の進み具合（`project_controller_decisions.effect_state`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectState {
    None,
    Pending,
    Applied,
    Suppressed,
    Failed,
}

impl EffectState {
    pub fn as_str(self) -> &'static str {
        match self {
            EffectState::None => "none",
            EffectState::Pending => "pending",
            EffectState::Applied => "applied",
            EffectState::Suppressed => "suppressed",
            EffectState::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "none" => EffectState::None,
            "pending" => EffectState::Pending,
            "applied" => EffectState::Applied,
            "suppressed" => EffectState::Suppressed,
            "failed" => EffectState::Failed,
            _ => return None,
        })
    }
}

/// `project_controller_state` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectControllerState {
    pub project_id: String,
    pub mode: ControllerMode,
    pub phase: ControllerPhase,
    pub revision: i64,
    /// 判断に畳んだ wake の元の `events.id` の最大（案件別の cursor）。
    pub event_cursor: i64,
    pub next_wake_at: Option<String>,
    pub coalesce_until: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<String>,
    pub retry_count: u32,
    pub backoff_until: Option<String>,
    pub last_decision_id: Option<String>,
    pub human_epoch: i64,
    pub progress_epoch: i64,
    pub updated_at: String,
    pub updated_by: String,
}

/// `project_wakes` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectWake {
    pub id: i64,
    pub project_id: String,
    pub wake_key: String,
    pub kind: ProjectWakeKind,
    pub source_event_id: Option<i64>,
    pub source_ref: Option<String>,
    pub created_at: String,
    pub consumed_by: Option<String>,
}

/// 取り込みが作る wake（`UNIQUE(project_id, wake_key)` で再配送を 1 行に束ねる）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProjectWake {
    pub project_id: String,
    pub wake_key: String,
    pub kind: ProjectWakeKind,
    pub source_event_id: Option<i64>,
    pub source_ref: Option<String>,
}

/// `project_controller_decisions` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectControllerDecision {
    pub id: String,
    pub project_id: String,
    pub based_on_revision: i64,
    pub human_epoch: i64,
    pub snapshot_sha256: String,
    pub request_sha256: String,
    pub generation_id: Option<String>,
    pub mode: ControllerMode,
    pub proposal_json: String,
    pub validation: String,
    pub effect_json: String,
    pub effect_state: EffectState,
    pub effect_ref: Option<String>,
    pub progress_epoch: i64,
    pub created_at: String,
    pub applied_at: Option<String>,
}

/// `GET /projects/{id}/controller` の応答（A1 の DTO）。行が無い案件は mode `off` の既定値。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectControllerView {
    pub state: ProjectControllerState,
    pub pending_wakes: Vec<ProjectWake>,
    pub last_decision: Option<ProjectControllerDecision>,
    /// 今日（UTC）の日次予算の使用量。案件 CoS の chat run の token と起動数を含む（P1-G）。
    pub budget_today: ProjectBudgetToday,
}

/// 今日（UTC）の日次予算の使用量（数え方は `task_ops::project_budget`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectBudgetToday {
    /// `YYYY-MM-DD`（UTC）。
    pub day: String,
    pub cos_inputs: u32,
    pub roots: u32,
    /// task run と案件 CoS の chat run の input + output（cache を含めない）。
    pub tokens: u64,
    /// 起動した案件 CoS の chat run の数。
    pub chat_starts: u32,
}

/// 判定に渡す入力（A1 の DTO。Proposal の型には依存しない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectDecideInput {
    pub project_id: String,
    pub revision: i64,
    pub human_epoch: i64,
    pub mode: ControllerMode,
    pub wakes: Vec<ProjectWake>,
    /// RFC 3339。
    pub now: String,
}

/// CAS（I6）で適用しなかった理由。判断の行の `validation` に `rejected:<code>` で残る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleReason {
    Revision,
    HumanEpoch,
    Lease,
    Mode,
}

impl StaleReason {
    pub fn code(self) -> &'static str {
        match self {
            StaleReason::Revision => "rejected:stale_revision",
            StaleReason::HumanEpoch => "rejected:stale_human_epoch",
            StaleReason::Lease => "rejected:stale_lease",
            StaleReason::Mode => "rejected:stale_mode",
        }
    }
}

/// root の冪等キー（I5）。判断 id を入れない。
pub fn root_idempotency_key(project_id: &str, root_key: &str) -> String {
    format!("pc:{project_id}:{root_key}")
}

/// controller が積む CoS 入力の key（`chat_messages.client_message_id`）の接頭辞。
pub const COS_INPUT_KEY_PREFIX: &str = "pc-input:";

/// 人の操作の wake の key（`human_epoch` の新しい値ごとに 1 件）。
pub fn human_wake_key(epoch: i64) -> String {
    format!("human:{epoch}")
}

/// 再試行の待ち（D12: `retry_base_secs × 2^(n-1)`、上限 `retry_max_secs`）。`failures` は 1 始まり。
pub fn backoff_secs(failures: u32, base: u64, max: u64) -> u64 {
    let shift = failures.saturating_sub(1).min(32);
    base.saturating_mul(1u64 << shift).min(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_and_are_nine() {
        for k in ProjectWakeKind::ALL {
            assert_eq!(ProjectWakeKind::parse(k.as_str()), Some(k));
            let json = serde_json::to_string(&k).unwrap();
            assert_eq!(json, format!("\"{}\"", k.as_str()));
        }
        assert_eq!(ProjectWakeKind::ALL.len(), 9);
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let got: Vec<u64> = (1..=7).map(|n| backoff_secs(n, 60, 3600)).collect();
        assert_eq!(got, vec![60, 120, 240, 480, 960, 1920, 3600]);
    }
}
