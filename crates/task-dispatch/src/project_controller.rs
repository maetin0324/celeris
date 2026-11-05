//! ADR 2026-11-04-project-controller A1: controller の判定の口と設定（公開の型）。tick の段は
//! `dispatcher/project_wake.rs`。

use serde::{Deserialize, Serialize};
use task_core::project_wake::{ControllerPhase, ProjectDecideInput};
use task_ops::add::NewTaskSpec;

/// controller の root に付ける label。
pub const CONTROLLER_LABEL: &str = "project-controller";

/// `[project_controller]` の実効値（D12 の既定）。`enabled = false`（既定）なら tick の段は何もしない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectControllerSettings {
    pub enabled: bool,
    pub coalesce_secs: u64,
    pub lease_secs: u64,
    pub max_evaluations_per_tick: usize,
    pub retry_base_secs: u64,
    pub retry_max_secs: u64,
    pub max_retries: u32,
    pub max_cos_inputs_open_per_project: u32,
    /// A6: `[project_controller.budget]`（予算・停滞・同じ失敗・TTL の guard）。
    pub budget: task_ops::project_budget::BudgetLimits,
}

impl Default for ProjectControllerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            coalesce_secs: 30,
            lease_secs: 60,
            max_evaluations_per_tick: 2,
            retry_base_secs: 60,
            retry_max_secs: 3600,
            max_retries: 5,
            max_cos_inputs_open_per_project: 1,
            budget: task_ops::project_budget::BudgetLimits::default(),
        }
    }
}

/// 判断の効果（A1 の DTO）。tx1 で JSON にして判断の行に残し、tx2 はそれを読み直して実行する。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectEffect {
    None,
    /// 案件 thread に `role=system` の入力を 1 件（I10）。`key` は `pc-input:` で始める。
    QueueCosInput {
        key: String,
        text: String,
    },
    /// root を 1 件。`idempotency_key` は `pc:<project>:<root.key>`（I5）。
    CreateRoot {
        idempotency_key: String,
        spec: Box<NewTaskSpec>,
        /// 冪等キーの行の `result_json`（A2 の `LaunchRootMeta`。snapshot が controller の root を引く）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<String>,
        /// A7: root を作る tx で使う `launch_root` の許可の範囲。[`crate::project_decider_ts`] が必ず入れる。
        /// 既定の無い A1 の判定（試験の fake）だけが `None`。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authority: Option<task_core::authority::GrantScope>,
    },
    /// 人への判断の要求（A3）。notice（`bad_news`、source key
    /// `project_controller:escalate:<project>:<question_key>`）を 1 件。同じ key は 2 回目を作らない。
    OpenEscalation {
        question_key: String,
        category: String,
        summary: String,
    },
}

impl ProjectEffect {
    pub(crate) fn has_effect(&self) -> bool {
        !matches!(self, ProjectEffect::None)
    }
}

/// 判定の結果。
#[derive(Debug, Clone)]
pub struct ProjectDecision {
    pub effect: ProjectEffect,
    /// 次に timer で起きるまでの秒（`None` なら wake が来るまで寝る）。
    pub next_wake_after_secs: Option<u64>,
    pub phase: ControllerPhase,
    /// 判断の記録に残す規則の名（`validation` の欄）。
    pub rule_id: String,
}

impl ProjectDecision {
    pub fn none(rule_id: &str) -> Self {
        Self {
            effect: ProjectEffect::None,
            next_wake_after_secs: None,
            phase: ControllerPhase::Active,
            rule_id: rule_id.into(),
        }
    }
}

/// 判定の口（A1）。A3 が TS の `PolicyScript::project_controller` と接続する。`Err` は再試行（backoff）。
pub trait ProjectDecider: Send {
    fn decide(&mut self, input: &ProjectDecideInput) -> Result<ProjectDecision, String>;

    /// tick の始めに store とその tick の領域 `project_controller` の宛先を渡す（A3）。既定は何もしない。
    fn attach(
        &mut self,
        _store: &std::sync::Arc<task_core::store::SqliteStore>,
        _access: Option<crate::project_decider_ts::ScriptAccess>,
    ) {
    }

    /// 直前の `decide` の判断の記録（`proposal_json` に残す JSON）。既定は無し（入力をそのまま残す）。
    fn take_record(&mut self) -> Option<String> {
        None
    }
}

/// A1 の既定: 常に効果なし（起票も CoS 入力もしない）。
#[derive(Debug, Default)]
pub struct NoopProjectDecider;

impl ProjectDecider for NoopProjectDecider {
    fn decide(&mut self, _input: &ProjectDecideInput) -> Result<ProjectDecision, String> {
        Ok(ProjectDecision::none("pc/noop"))
    }
}

/// crash の境界（ADR D12 の B1〜B6 と、判断の後・tx1 の前）。試験の hook が panic する場所。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    /// B1 ingest 前。
    BeforeIngest,
    /// `blocked` の再開（P1-G）の tx の後・評価の前。
    AfterResume,
    /// B2 ingest 後・評価前。
    AfterIngest,
    /// B3 lease 後・判断前。
    AfterLease,
    /// 判断の後・tx1 の前（人の割り込みの試験）。
    BeforeCommit,
    /// B4 tx1 の後・効果の前。
    AfterCommit,
    /// B5 効果の後・`applied` の前。
    AfterEffect,
    /// B6 `applied` の後。
    AfterApplied,
}

/// 1 tick の段の集計（試験・log）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControllerTickStats {
    pub ingested: usize,
    pub decided: usize,
    pub decide_failed: usize,
    pub stale: Vec<String>,
    pub applied: usize,
    pub busy: usize,
    /// 門で止めた効果（`suppressed`）。
    pub halted: usize,
    /// 失敗して延期した効果。
    pub effect_failed: usize,
    /// 上限を超えて `failed` にした効果。
    pub exhausted: usize,
    /// A6: guard が decider を呼ばずに書いた判断（`pc/guard/*`）。
    pub guarded: usize,
    /// A6: `Project.status` に合わせて phase を変えた案件。
    pub phase_synced: usize,
    /// P1-G: 根拠つきで `blocked` から再開した案件。
    pub resumed: usize,
    /// A6: decider の 1 回の呼び出しの時間（マイクロ秒。TS の isolate の op を含む）。
    pub decide_micros: Vec<u64>,
    /// A6: この tick の段の全体の時間（マイクロ秒。loop cost）。
    pub pass_micros: u64,
}
