//! ADR 2026-11-04-project-controller A1: controller の判定の口と設定（公開の型）。tick の段は
//! `dispatcher/project_wake.rs`。

use serde::{Deserialize, Serialize};
use task_core::project_wake::{
    ControllerPhase, NewProjectWake, ProjectDecideInput, ProjectWakeKind,
};
use task_core::store::SqliteStore;
use task_core::{Event, Status, TaskId};
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
    /// P4-D: `[project_controller.retention]`（wake・判断・compare の記録の GC）。`None` なら GC しない。
    pub retention: Option<ProjectRetentionSettings>,
    /// `[project_controller.checkpoint]`（ADR 2026-11-22-parent-checkpoint-evaluator D1.2）。snapshot の due と
    /// 評価の run の試行の上限の正本。
    pub checkpoint: task_core::parent_checkpoint::CheckpointSettings,
}

/// P4-D（ADR 2026-11-05-phase4-p4d-retention）: GC の規則・archive の置き場・間隔。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRetentionSettings {
    pub policy: task_core::project_retention::RetentionPolicy,
    /// 消す行を書く JSONL の dir（必須。監査のため archive の無い GC はしない）。
    pub archive_dir: std::path::PathBuf,
    /// GC の間隔（既定 3600 秒。process の中で数え、再起動の後は最初の tick で 1 回走る）。
    pub interval_secs: u64,
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
            retention: None,
            checkpoint: task_core::parent_checkpoint::CheckpointSettings::default(),
        }
    }
}

/// root の task 木の全 run の使った token の読み口（dispatcher の `token_budget::tree_tokens_used` を渡す。
/// ADR 2026-11-22-parent-checkpoint-evaluator D1.2 の予算の割合）。
pub type TreeTokens =
    fn(&SqliteStore, TaskId, Option<&str>) -> Result<u64, task_core::store::StoreError>;

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
    /// 第 3 フェーズ I2（付記 P0-B）: `mark_milestone` の到達の記録。効果の時に Rust が根拠を確かめ直し
    /// （`project_decider_ts::record_outcome`）、approved の Roadmap があれば同じ tx で門を読み `reached` を書く。
    /// Roadmap の無い案件は、適用済みの判断の行（`effect_ref` = 根拠の root）が到達の正本。
    RecordOutcome {
        outcome_id: String,
        evidence_refs: Vec<String>,
    },
    /// ADR 2026-11-22-parent-checkpoint-evaluator D2.3: strategy root の親の途中評価の要求。効果は何も書かず
    /// `applied`（`effect_ref` = root）にする。評価の run は dispatcher の段が実行中の判断から起こす。
    RequestEvaluation {
        root_id: String,
        triggers: Vec<String>,
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

    /// controller を入れる時（`set_project_controller`）に 1 回、実効の設定と木の使った token の読み口を渡す。既定は何も
    /// しない（TS の判定は `checkpoint` の設定と読み口で snapshot の checkpoint を導く）。
    fn configure(&mut self, _settings: &ProjectControllerSettings, _tree_tokens: TreeTokens) {}
    /// 直前の `decide` の判断の記録（`proposal_json` に残す JSON）。既定は無し（入力をそのまま残す）。
    fn take_record(&mut self) -> Option<String> {
        None
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

/// ADR 2026-11-04-project-controller A2: task の event 1 件を案件の wake に写す（tick の段
/// `dispatcher/project_wake.rs` の cursor が読んだ event ごとに呼ぶ）。どの event が controller を起こすかは controller の
/// 契約なので、dispatcher ではなくこの module に置く。
pub(crate) fn wakes_for_event(
    store: &SqliteStore,
    event_id: u64,
    task_id: TaskId,
    event: &Event,
) -> Result<Vec<NewProjectWake>, String> {
    let kind = match event {
        // 第 4 フェーズ P4-B（G1）: 根拠の失効。done の root が非終端へ戻った（reopen・retry）か、取り込みが
        // reopen された。achieved の案件でも Rust が達成を数え直す（`project_guard` の 1 段目）。
        Event::Transitioned {
            from: Status::Done,
            to,
            ..
        } if !to.is_terminal() => Some(ProjectWakeKind::EvidenceChanged),
        Event::IntegrationReopened { .. } => Some(ProjectWakeKind::EvidenceChanged),
        Event::Transitioned { to, reason, .. } => match to {
            Status::Done => Some(ProjectWakeKind::RootDone),
            Status::Failed if reason == "review_fail" => Some(ProjectWakeKind::ReviewFailed),
            Status::Failed => Some(ProjectWakeKind::RootFailed),
            Status::Cancelled => None,
            _ => return Ok(Vec::new()),
        },
        Event::DecisionAnswered { .. }
        | Event::Answered { .. }
        | Event::ApprovalDecided { .. }
        | Event::PlanGateDecided { .. } => Some(ProjectWakeKind::DecisionAnswered),
        // A9: root の取り込みが閉じた（merge）。done の wake の時点では
        // 取り込み前なので、閉じたときにもう一度起こす（次の root を起票できる）。
        Event::IntegrationMerged { .. } => Some(ProjectWakeKind::RootDone),
        Event::ClusterJobWaitFinished { .. }
        | Event::BrowserWaitResolved { .. }
        | Event::BrowserPrerequisiteResumed { .. } => Some(ProjectWakeKind::ExternalReleased),
        _ => return Ok(Vec::new()),
    };
    let Some(task) = store.get(task_id).map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    let Some(project) = task.project_id else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut push = |kind: ProjectWakeKind, subject: TaskId| {
        let prefix = match kind {
            ProjectWakeKind::ExternalReleased => "external",
            other => other.as_str(),
        };
        out.push(NewProjectWake {
            project_id: project.to_string(),
            wake_key: format!("{prefix}:{subject}:{event_id}"),
            kind,
            source_event_id: Some(event_id as i64),
            source_ref: Some(subject.to_string()),
        });
    };
    match kind {
        Some(ProjectWakeKind::RootDone) => {
            if task_ops::root_done::is_root_done(&task) {
                push(ProjectWakeKind::RootDone, task.id);
            }
        }
        Some(
            k @ (ProjectWakeKind::RootFailed
            | ProjectWakeKind::ReviewFailed
            | ProjectWakeKind::EvidenceChanged),
        ) => {
            if task_core::is_root_task(&task) {
                push(k, task.id);
            }
        }
        Some(k) => push(k, task.id),
        None => {}
    }
    // 子の終端: 親の待ち（`awaited_children`）が空になったら親の案件に `children_settled`（I15: 親を起こすのは
    // 既存の tree。controller は観測するだけ）。
    if let (Event::Transitioned { to, .. }, Some(parent)) = (event, task.parent_id)
        && to.is_terminal()
    {
        let awaited =
            task_ops::delegate::child_wait::awaited_children(store, parent, true, &mut |_| {
                Ok(false)
            })
            .map_err(|e| e.to_string())?;
        if awaited.is_empty() {
            push(ProjectWakeKind::ChildrenSettled, parent);
        }
    }
    Ok(out)
}

/// decide の失敗が `max_retries` を超えて phase `blocked` にするときの notice（付記 A1 の 12）。
/// source_key は案件と `revision`。同じ止まり方の再試行（crash の後など）では 1 件のまま、人が mode を
/// 置き直す（`revision + 1`）と次に止まったときに新しい notice になる。
pub(crate) fn decide_exhausted_notice(
    state: &task_core::project_wake::ProjectControllerState,
    failures: u32,
    error: &str,
    now: time::OffsetDateTime,
) -> task_core::feed::NoticeEvent {
    let project = &state.project_id;
    let title = format!(
        "案件 {project} の controller の判断が {failures} 回失敗し、controller を止めました"
    );
    let summary = format!(
        "{title}。controller の phase は blocked で、以後この案件の判断をしません。最後の失敗: {error}。原因を直してから controller の mode を置き直すと再開します。"
    );
    task_core::feed::NoticeEvent {
        source_key: format!(
            "project_controller:decide_failed:{project}:{}",
            state.revision
        ),
        kind: task_core::feed::NoticeKind::BadNews,
        group_key: format!("bad_news:project_controller:{project}"),
        title,
        summary,
        project_id: Some(project.clone()),
        task_id: None,
        target: None,
        links: Vec::new(),
        at: now,
    }
}
