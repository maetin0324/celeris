//! ADR 2026-11-04-project-controller（A2）: Project Controller の入出力 DTO と Rust の検証。
//!
//! - [`ProjectSnapshot`]（Rust → TS。`celeris.project-snapshot/1`）は案件の task ledger（`Project.request` の
//!   Outcome block・root の acceptance）と progress ledger（root の状態・合格・取り込み・待ち・過去の判断）を
//!   毎回作り直した値。独立の Goal は持たない（D1）。組み立ては `task_ops::project_snapshot`。
//! - [`ProjectProposal`]（TS → Rust。`celeris.project-proposal/1`）は次の行動 1 つ（continue・replan・launch_root・
//!   wait・escalate・mark_milestone）と gap・説明・前提の版。TS は DB も system も書かない。
//! - [`validate`] は proposal を snapshot に当てる Rust の検証（D7・D9）。落ちたら起票しない（I9 fail closed）。
//!   `safety` の印が付いた違反は安全違反で、guard が領域を `rust` へ戻す材料になる。
//! - [`parse_outcomes`] は `Project.request` の ```` ```celeris-outcomes ```` block（人が書く途中目標）を読む。
//!
//! この module は I/O を持たない純粋な関数だけ。

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::project_brief::WaitingOn;

/// snapshot の schema 名。
pub const PROJECT_SNAPSHOT_SCHEMA: &str = "celeris.project-snapshot/1";
/// proposal の schema 名。
pub const PROJECT_PROPOSAL_SCHEMA: &str = "celeris.project-proposal/1";
/// snapshot の直列化の上限（byte）。超える分は古い done の root から落とし `truncated` を真にする。
pub const SNAPSHOT_MAX_BYTES: usize = 1024 * 1024;
/// `RootSpec.title` の上限（文字）。
pub const ROOT_TITLE_MAX: usize = 200;
/// `RootSpec.objective` の上限（文字）。
pub const ROOT_OBJECTIVE_MAX: usize = 8_000;
/// `explanation.signals` の上限（件）。
pub const SIGNALS_MAX: usize = 16;
/// 1 行の欄（rule_id・reason_code・question_key・signal の name/detail 等）の上限（文字）。
pub const SHORT_TEXT_MAX: usize = 256;
/// 文の欄（question・summary）の上限（文字）。
pub const LONG_TEXT_MAX: usize = 2_000;
/// 次の wake までの秒の上限（7 日）。
pub const NEXT_WAKE_MAX_SECS: u64 = 7 * 24 * 3600;
/// root の冪等キーの接頭辞（I5。`pc:<project id>:<root.key>`）。
pub const ROOT_KEY_PREFIX: &str = "pc:";
/// controller が起票した root に付ける label。
pub const CONTROLLER_LABEL: &str = "project-controller";
/// 冪等キーの行の `operation`。
pub const LAUNCH_ROOT_OPERATION: &str = "project_controller.launch_root";
/// escalate の `category`。CoS triage の `human_required` の既定の名（`celeris::config::cos::HUMAN_REQUIRED_REASONS`）と
/// 同じ。食い違いは `crates/celeris/tests/project_controller_e2e.rs` が確かめる。
pub const ESCALATION_CATEGORIES: [&str; 7] = [
    "objective_change",
    "beyond_frame",
    "external_publish",
    "destructive",
    "secret_or_security",
    "standing_permission",
    "explicit_human",
];
/// Outcome block の fence の info string。
pub const OUTCOMES_FENCE: &str = "```celeris-outcomes";

/// root の冪等キー（I5）。key に判断 id を入れない。
pub fn root_idempotency_key(project_id: &str, root_key: &str) -> String {
    format!("{ROOT_KEY_PREFIX}{project_id}:{root_key}")
}

/// 文字列の SHA-256（小文字 hex）。`request_sha256` に使う。
pub fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Outcome（途中目標）
// ---------------------------------------------------------------------------

/// 途中目標 1 つ（A5 までは request の block、以後 Roadmap）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub id: String,
    pub title: String,
    /// 達成の条件。controller の root はこの文を文字どおり含む（I8）。
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub priority: i32,
    /// `request_block`（A5 以後は `roadmap`）。
    #[serde(default = "default_outcome_source")]
    pub source: String,
    /// 依頼文の block が書く決定的な検査（ADR 2026-11-05-phase3-p0e）。Rust が root の acceptance に足す。
    /// TS は command を作らず見もしないので、snapshot の JSON と schema には出さない。
    #[serde(skip)]
    #[schemars(skip)]
    pub checks: Vec<OutcomeCheck>,
}

/// Outcome を決定的に確かめる command（作業 tree の直下で実行する）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeCheck {
    pub cmd: String,
    #[serde(default)]
    pub expect_exit: i32,
}

/// 1 つの Outcome に書ける検査の数と、command の長さの上限。
pub const OUTCOME_CHECKS_MAX: usize = 8;
pub const OUTCOME_CHECK_CMD_MAX_BYTES: usize = 4096;

fn default_outcome_source() -> String {
    "request_block".to_owned()
}

/// request の block の 1 行の形（`source` は書かせない）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutcomeBlockItem {
    id: String,
    title: String,
    acceptance: Vec<String>,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    checks: Vec<OutcomeCheck>,
}

fn valid_outcome_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// `request` の ```` ```celeris-outcomes ```` block を読む。block が無ければ `Ok(vec![])`。
/// 形の誤り・id の重複・未知の依存・循環・空の acceptance は `Err`（snapshot は `outcomes: []`・`outcomes_invalid`）。
pub fn parse_outcomes(request: &str) -> Result<Vec<Outcome>, String> {
    let Some(start) = request.find(OUTCOMES_FENCE) else {
        return Ok(Vec::new());
    };
    let body_start = start + OUTCOMES_FENCE.len();
    let rest = &request[body_start..];
    let Some(end) = rest.find("```") else {
        return Err("the celeris-outcomes block is not closed".to_owned());
    };
    if request[body_start + end + 3..].contains(OUTCOMES_FENCE) {
        return Err("more than one celeris-outcomes block".to_owned());
    }
    let items: Vec<OutcomeBlockItem> =
        serde_json::from_str(rest[..end].trim()).map_err(|e| format!("outcomes: {e}"))?;
    if items.is_empty() {
        return Err("the celeris-outcomes block is empty".to_owned());
    }
    let mut ids = BTreeSet::new();
    for item in &items {
        if !valid_outcome_id(&item.id) {
            return Err(format!(
                "outcome id {:?} must match [a-z0-9_-]{{1,32}}",
                item.id
            ));
        }
        if !ids.insert(item.id.clone()) {
            return Err(format!("outcome id {:?} appears twice", item.id));
        }
        if item.acceptance.is_empty() || item.acceptance.iter().any(|a| a.trim().is_empty()) {
            return Err(format!("outcome {:?} needs non-empty acceptance", item.id));
        }
        if item.title.trim().is_empty() || item.title.chars().count() > ROOT_TITLE_MAX {
            return Err(format!("outcome {:?} has a bad title", item.id));
        }
        if item.checks.len() > OUTCOME_CHECKS_MAX {
            return Err(format!(
                "outcome {:?} has more than {OUTCOME_CHECKS_MAX} checks",
                item.id
            ));
        }
        if item
            .checks
            .iter()
            .any(|c| c.cmd.trim().is_empty() || c.cmd.len() > OUTCOME_CHECK_CMD_MAX_BYTES)
        {
            return Err(format!("outcome {:?} has a bad check command", item.id));
        }
    }
    let deps: BTreeMap<&str, &[String]> = items
        .iter()
        .map(|i| (i.id.as_str(), i.depends_on.as_slice()))
        .collect();
    for item in &items {
        if let Some(d) = item.depends_on.iter().find(|d| !ids.contains(*d)) {
            return Err(format!("outcome {:?} depends on unknown {d:?}", item.id));
        }
    }
    // 循環: 深さ優先で同じ id に戻るか。
    fn visits(
        id: &str,
        deps: &BTreeMap<&str, &[String]>,
        path: &mut Vec<String>,
        done: &mut BTreeSet<String>,
    ) -> bool {
        if done.contains(id) {
            return false;
        }
        if path.iter().any(|p| p == id) {
            return true;
        }
        path.push(id.to_owned());
        let cyc = deps
            .get(id)
            .is_some_and(|ds| ds.iter().any(|d| visits(d, deps, path, done)));
        path.pop();
        done.insert(id.to_owned());
        cyc
    }
    let mut done = BTreeSet::new();
    for item in &items {
        if visits(&item.id, &deps, &mut Vec::new(), &mut done) {
            return Err(format!(
                "outcome dependencies form a cycle at {:?}",
                item.id
            ));
        }
    }
    Ok(items
        .into_iter()
        .map(|i| Outcome {
            id: i.id,
            title: i.title,
            acceptance: i.acceptance,
            depends_on: i.depends_on,
            priority: i.priority,
            source: default_outcome_source(),
            checks: i.checks,
        })
        .collect())
}

// ---------------------------------------------------------------------------
// ProjectSnapshot（Rust → TS）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSnapshot {
    /// 評価の時刻（RFC3339）。TS は時計を読まずにこれを使う。
    pub now: String,
    pub truncated: bool,
    pub project: SnapshotProject,
    /// `ProjectBrief` そのまま。判定の根拠にしない（D4）。
    pub brief: Option<serde_json::Value>,
    pub outcomes: Vec<Outcome>,
    /// request の block の形が誤っている（付記 A2-1）。`outcomes` は空。
    pub outcomes_invalid: bool,
    pub roots: Vec<SnapshotRoot>,
    pub waits: Vec<SnapshotWait>,
    pub stagnation: Vec<SnapshotStagnation>,
    pub wakes: Vec<SnapshotWake>,
    pub decisions: Vec<SnapshotDecision>,
    pub controller: SnapshotController,
    pub limits: SnapshotLimits,
    /// A5 が埋める。
    pub roadmap: Option<serde_json::Value>,
    /// A7 が埋める。
    pub authority: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotProject {
    pub id: String,
    pub title: String,
    /// `active`・`paused`・…（`ProjectStatus` の snake_case）。
    pub status: String,
    pub request: String,
    pub request_sha256: String,
    pub auto_integrate: bool,
}

/// root の受け入れ条件 1 件と結果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotCriterion {
    pub text: String,
    /// `command`・`reviewer`・`human`・`artifact_exists`。
    pub kind: String,
    /// `pass`・`fail`・`unknown`（reviewer の判定・決定的 check の結果）。
    pub result: String,
}

/// 案件の root 1 件（progress ledger の行）。worker の報告の文は持たない（I11）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRoot {
    pub id: String,
    /// task の状態（`Status` の snake_case）。
    pub status: String,
    pub title: String,
    /// controller が起票した root の `RootSpec.key`（`outcome:<id>:<n>`）。人や CoS の root は `None`。
    pub controller_key: Option<String>,
    pub outcome_ids: Vec<String>,
    pub acceptance: Vec<SnapshotCriterion>,
    pub reviewer_passed: bool,
    pub integrated: bool,
    /// `review_failed`・`infra`・`cancelled`・`other`。終端の失敗でなければ `None`。
    pub failure_class: Option<String>,
    pub attempts: u32,
    pub open_waits: Vec<String>,
    pub leaf_done: u32,
    pub leaf_total: u32,
    pub continues_root_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotWait {
    /// `decision`・`question`・`approval`・`plan_gate`。
    pub kind: String,
    pub id: String,
    pub task_id: String,
    pub age_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotStagnation {
    pub kind: String,
    pub key: String,
    pub since: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotWake {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub event_id: u64,
}

/// 過去の判断 1 件（A1 の `project_controller_decisions` から。A1 の前は呼び出し側が渡す）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotDecision {
    pub id: String,
    /// `ProjectAction` の `kind`。
    pub kind: String,
    pub rule_id: String,
    /// `none`・`pending`・`applied`・`suppressed`・`failed`。
    pub effect_state: String,
    pub root_key: Option<String>,
    /// replan の `reason_code`（同じ版での繰り返しを避けるのに使う）。
    pub reason_code: Option<String>,
    /// mark_milestone の `outcome_id`。
    pub outcome_id: Option<String>,
    pub request_sha256: String,
    pub progress_epoch: u64,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotController {
    /// `off`・`shadow`・`enabled`。
    pub mode: String,
    /// `active`・`waiting`・`blocked`・`paused`・`achieved`・`cancelled`。
    pub phase: String,
    pub revision: u64,
    pub human_epoch: u64,
    pub retry_count: u32,
    pub progress_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotLimits {
    pub max_open_roots: u32,
    pub max_root_attempts_per_outcome: u32,
    pub max_no_progress_decisions: u32,
    pub cos_inputs_remaining_today: u32,
    pub roots_remaining_today: u32,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            max_open_roots: 2,
            max_root_attempts_per_outcome: 2,
            max_no_progress_decisions: 5,
            cos_inputs_remaining_today: 10,
            roots_remaining_today: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// ProjectProposal（TS → Rust）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectProposal {
    pub action: ProjectAction,
    pub gap: ProposalGap,
    pub explanation: ProposalExplanation,
    pub based_on: ProposalBasedOn,
}

/// 次の行動（依頼の continue / replan / launch-root / wait / escalate / mark-milestone）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectAction {
    /// 効果なし。`phase = active`。
    Continue {
        watch_root_ids: Vec<String>,
        next_wake_after_secs: u64,
    },
    /// 案件 thread に `role=system` の入力 1 件（既存の案件 CoS run が計画する）。
    Replan {
        reason_code: String,
        question: String,
        context_refs: Vec<String>,
    },
    /// root を 1 件起票する（`pc:<project>:<root.key>` で冪等）。
    LaunchRoot { root: RootSpec },
    /// 効果なし。
    Wait {
        waiting_on: Vec<WaitingOn>,
        next_wake_after_secs: u64,
        phase: WaitPhase,
    },
    /// 受信箱に 1 件。
    Escalate {
        category: String,
        question_key: String,
        summary: String,
        options: Vec<String>,
        evidence_refs: Vec<String>,
    },
    /// Outcome の到達を記録する（A5 までは判断の記録だけ）。
    MarkMilestone {
        outcome_id: String,
        evidence_refs: Vec<String>,
    },
}

impl ProjectAction {
    pub fn kind(&self) -> &'static str {
        match self {
            ProjectAction::Continue { .. } => "continue",
            ProjectAction::Replan { .. } => "replan",
            ProjectAction::LaunchRoot { .. } => "launch_root",
            ProjectAction::Wait { .. } => "wait",
            ProjectAction::Escalate { .. } => "escalate",
            ProjectAction::MarkMilestone { .. } => "mark_milestone",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitPhase {
    Waiting,
    Achieved,
}

/// controller が起票する root の中身。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RootSpec {
    /// `outcome:<id>:<n>`（`n` は 1 から数える試行番号）。
    pub key: String,
    pub title: String,
    pub objective: String,
    pub acceptance: Vec<RootCriterion>,
    pub stages_hint: Vec<String>,
    pub continues_root_ids: Vec<String>,
    pub outcome_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RootCriterion {
    pub text: String,
    pub check: RootCheck,
}

/// root の受け入れ条件の検査。TS は決定的 check の command を作らない（reviewer か human）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RootCheck {
    Reviewer,
    Human,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalGap {
    pub reached: Vec<String>,
    pub open: Vec<String>,
    pub unknown: Vec<String>,
    pub progress_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalExplanation {
    pub rule_id: String,
    pub signals: Vec<ProposalSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalSignal {
    pub name: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposalBasedOn {
    pub revision: u64,
    pub human_epoch: u64,
    pub request_sha256: String,
}

// ---------------------------------------------------------------------------
// 検証
// ---------------------------------------------------------------------------

/// 検証の違反 1 件。`safety` が真なら安全違反（guard が領域を `rust` へ戻す材料）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProposalViolation {
    pub code: String,
    pub safety: bool,
    pub detail: String,
}

impl ProposalViolation {
    fn new(code: &str, safety: bool, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            safety,
            detail: detail.into(),
        }
    }
}

/// 安全違反の code（ADR D13 の `*`）。
pub const SAFETY_VIOLATIONS: [&str; 4] = [
    "root_acceptance_narrowed",
    "root_key_not_owned",
    "escalation_category_unknown",
    "launch_on_paused",
];

/// TS の生の出力（JSON）を DTO に読み、[`validate`] を当てる。形の誤りは `invalid_output` 1 件。
pub fn parse_and_validate(
    snapshot: &ProjectSnapshot,
    raw: &serde_json::Value,
) -> Result<ProjectProposal, Vec<ProposalViolation>> {
    let proposal: ProjectProposal = serde_json::from_value(raw.clone()).map_err(|e| {
        vec![ProposalViolation::new(
            "invalid_output",
            false,
            format!("does not match the proposal DTO: {e}"),
        )]
    })?;
    validate(snapshot, &proposal)?;
    Ok(proposal)
}

fn too_long(text: &str, max: usize) -> bool {
    text.chars().count() > max
}

/// proposal を snapshot に当てる（ADR D13）。違反が 1 件でもあれば `Err` で、呼び出し側は起票しない。
pub fn validate(
    snapshot: &ProjectSnapshot,
    proposal: &ProjectProposal,
) -> Result<(), Vec<ProposalViolation>> {
    let mut v = Vec::new();
    let based = &proposal.based_on;
    if based.revision != snapshot.controller.revision
        || based.human_epoch != snapshot.controller.human_epoch
        || based.request_sha256 != snapshot.project.request_sha256
    {
        v.push(ProposalViolation::new(
            "based_on_mismatch",
            false,
            "based_on does not match the snapshot",
        ));
    }
    let ex = &proposal.explanation;
    if ex.rule_id.is_empty()
        || too_long(&ex.rule_id, SHORT_TEXT_MAX)
        || ex.signals.len() > SIGNALS_MAX
        || ex
            .signals
            .iter()
            .any(|s| too_long(&s.name, SHORT_TEXT_MAX) || too_long(&s.detail, SHORT_TEXT_MAX))
    {
        v.push(ProposalViolation::new(
            "field_too_long",
            false,
            "explanation is empty or too long",
        ));
    }
    let outcome_ids: BTreeSet<&str> = snapshot.outcomes.iter().map(|o| o.id.as_str()).collect();
    let gap = &proposal.gap;
    if gap
        .reached
        .iter()
        .chain(&gap.open)
        .chain(&gap.unknown)
        .any(|id| !outcome_ids.contains(id.as_str()))
    {
        v.push(ProposalViolation::new(
            "gap_unknown_outcome",
            false,
            "gap names an outcome that is not in the snapshot",
        ));
    }
    let root_ids: BTreeSet<&str> = snapshot.roots.iter().map(|r| r.id.as_str()).collect();
    match &proposal.action {
        ProjectAction::Continue {
            watch_root_ids,
            next_wake_after_secs,
        } => {
            if watch_root_ids
                .iter()
                .any(|id| !root_ids.contains(id.as_str()))
            {
                v.push(ProposalViolation::new(
                    "unknown_root",
                    false,
                    "continue watches a root that is not in the snapshot",
                ));
            }
            if *next_wake_after_secs > NEXT_WAKE_MAX_SECS {
                v.push(ProposalViolation::new(
                    "field_too_long",
                    false,
                    "next wake too late",
                ));
            }
        }
        ProjectAction::Wait {
            waiting_on,
            next_wake_after_secs,
            phase,
        } => {
            if *next_wake_after_secs > NEXT_WAKE_MAX_SECS || waiting_on.len() > SIGNALS_MAX {
                v.push(ProposalViolation::new(
                    "field_too_long",
                    false,
                    "wait is too long",
                ));
            }
            if *phase == WaitPhase::Achieved
                && (!gap.open.is_empty() || !gap.unknown.is_empty() || snapshot.outcomes.is_empty())
            {
                v.push(ProposalViolation::new(
                    "achieved_with_open_outcomes",
                    false,
                    "wait(achieved) while outcomes are open",
                ));
            }
        }
        ProjectAction::Replan {
            reason_code,
            question,
            context_refs,
        } => {
            if reason_code.is_empty()
                || too_long(reason_code, SHORT_TEXT_MAX)
                || too_long(question, LONG_TEXT_MAX)
                || context_refs.len() > SIGNALS_MAX
            {
                v.push(ProposalViolation::new(
                    "field_too_long",
                    false,
                    "replan fields",
                ));
            }
            if snapshot.limits.cos_inputs_remaining_today == 0 {
                v.push(ProposalViolation::new(
                    "cos_inputs_exceeded",
                    false,
                    "no CoS input left today",
                ));
            }
        }
        ProjectAction::Escalate {
            category,
            question_key,
            summary,
            options,
            evidence_refs,
        } => {
            if !ESCALATION_CATEGORIES.contains(&category.as_str()) {
                v.push(ProposalViolation::new(
                    "escalation_category_unknown",
                    true,
                    format!("category {category:?}"),
                ));
            }
            if question_key.is_empty()
                || too_long(question_key, SHORT_TEXT_MAX)
                || too_long(summary, LONG_TEXT_MAX)
                || options.len() > SIGNALS_MAX
                || evidence_refs.len() > SIGNALS_MAX
            {
                v.push(ProposalViolation::new(
                    "field_too_long",
                    false,
                    "escalate fields",
                ));
            }
        }
        ProjectAction::MarkMilestone {
            outcome_id,
            evidence_refs,
        } => {
            if !outcome_ids.contains(outcome_id.as_str()) {
                v.push(ProposalViolation::new(
                    "unknown_outcome",
                    false,
                    format!("outcome {outcome_id:?}"),
                ));
            }
            let outcome = snapshot.outcomes.iter().find(|o| &o.id == outcome_id);
            let ok_ref = |r: &String| {
                r.strip_prefix("task:").is_some_and(|id| {
                    snapshot.roots.iter().any(|root| {
                        root.id == id && outcome.is_some_and(|o| root_satisfies(root, o))
                    })
                })
            };
            if evidence_refs.is_empty() || !evidence_refs.iter().all(ok_ref) {
                v.push(ProposalViolation::new(
                    "milestone_evidence_unknown",
                    false,
                    "evidence_refs must name done, passed roots of the outcome",
                ));
            }
        }
        ProjectAction::LaunchRoot { root } => validate_launch(snapshot, root, &mut v),
    }
    if v.is_empty() { Ok(()) } else { Err(v) }
}

/// `outcome:<id>:<n>` を `(id, n)` に分ける。
pub fn parse_root_key(key: &str) -> Option<(&str, u32)> {
    let rest = key.strip_prefix("outcome:")?;
    let (id, n) = rest.rsplit_once(':')?;
    let n: u32 = n.parse().ok()?;
    (valid_outcome_id(id) && n >= 1 && n.to_string() == key.rsplit(':').next()?).then_some((id, n))
}

fn validate_launch(snapshot: &ProjectSnapshot, root: &RootSpec, v: &mut Vec<ProposalViolation>) {
    if snapshot.project.status != "active" {
        v.push(ProposalViolation::new(
            "launch_on_paused",
            true,
            format!("project status is {}", snapshot.project.status),
        ));
    }
    if root.title.trim().is_empty()
        || too_long(&root.title, ROOT_TITLE_MAX)
        || too_long(&root.objective, ROOT_OBJECTIVE_MAX)
        || root.stages_hint.len() > SIGNALS_MAX
    {
        v.push(ProposalViolation::new(
            "field_too_long",
            false,
            "root title/objective",
        ));
    }
    let Some((key_outcome, attempt)) = parse_root_key(&root.key) else {
        v.push(ProposalViolation::new(
            "root_key_not_owned",
            true,
            format!("root key {:?} is not outcome:<id>:<n>", root.key),
        ));
        return;
    };
    let outcome = snapshot.outcomes.iter().find(|o| o.id == key_outcome);
    let owned = outcome.is_some()
        && !root.outcome_ids.is_empty()
        && root
            .outcome_ids
            .iter()
            .all(|id| snapshot.outcomes.iter().any(|o| &o.id == id))
        && root.outcome_ids.iter().any(|id| id == key_outcome);
    if !owned {
        v.push(ProposalViolation::new(
            "root_key_not_owned",
            true,
            "outcome_ids are not the snapshot's outcomes",
        ));
        return;
    }
    // I8: root の acceptance は対象の Outcome の acceptance を文字どおり含む。
    let texts: BTreeSet<&str> = root.acceptance.iter().map(|c| c.text.as_str()).collect();
    for id in &root.outcome_ids {
        if let Some(o) = snapshot.outcomes.iter().find(|o| &o.id == id)
            && o.acceptance.iter().any(|a| !texts.contains(a.as_str()))
        {
            v.push(ProposalViolation::new(
                "root_acceptance_narrowed",
                true,
                format!("root acceptance drops a criterion of outcome {id:?}"),
            ));
        }
    }
    if root
        .continues_root_ids
        .iter()
        .any(|id| !snapshot.roots.iter().any(|r| &r.id == id))
    {
        v.push(ProposalViolation::new(
            "unknown_root",
            false,
            "continues_root_ids names a root that is not in the snapshot",
        ));
    }
    // 同じ key の root が既にあれば、適用は冪等に既存を返す（I5）。数の上限は新しい key にだけ当てる。
    let exists = snapshot
        .roots
        .iter()
        .any(|r| r.controller_key.as_deref() == Some(root.key.as_str()));
    if exists {
        return;
    }
    let tried = snapshot
        .roots
        .iter()
        .filter(|r| {
            r.controller_key
                .as_deref()
                .and_then(parse_root_key)
                .is_some_and(|(o, _)| o == key_outcome)
        })
        .count();
    let limit = snapshot.limits.max_root_attempts_per_outcome;
    if attempt > limit || u32::try_from(tried).unwrap_or(u32::MAX) >= limit {
        v.push(ProposalViolation::new(
            "root_attempts_exceeded",
            false,
            format!("outcome {key_outcome:?} already has {tried} roots (limit {limit})"),
        ));
    }
    let open = snapshot
        .roots
        .iter()
        .filter(|r| r.controller_key.is_some() && !is_terminal_status(&r.status))
        .count();
    if u32::try_from(open).unwrap_or(u32::MAX) >= snapshot.limits.max_open_roots {
        v.push(ProposalViolation::new(
            "open_roots_exceeded",
            false,
            format!("{open} controller roots are open"),
        ));
    }
    if snapshot.limits.roots_remaining_today == 0 {
        v.push(ProposalViolation::new(
            "roots_exceeded_today",
            false,
            "no root left today",
        ));
    }
}

/// root が Outcome の成果を持つか（D6）: done・合格・取り込み済みで、Outcome を名指すか、その acceptance を
/// 文字どおり全部含む。worker・CoS の自己申告は見ない。TS の `satisfies` と同じ規則。
pub fn root_satisfies(root: &SnapshotRoot, outcome: &Outcome) -> bool {
    if root.status != "done" || !root.reviewer_passed || !root.integrated {
        return false;
    }
    if root.outcome_ids.iter().any(|o| o == &outcome.id) {
        return true;
    }
    let texts: BTreeSet<&str> = root.acceptance.iter().map(|c| c.text.as_str()).collect();
    outcome
        .acceptance
        .iter()
        .all(|a| texts.contains(a.as_str()))
}

/// task の状態が終端か（`done`・`failed`・`cancelled`）。
pub fn is_terminal_status(status: &str) -> bool {
    matches!(status, "done" | "failed" | "cancelled")
}

#[path = "project_proposal_fixture.rs"]
mod fixture;
pub use fixture::invariants_fixture_jsonl;

#[cfg(test)]
#[path = "project_proposal_tests.rs"]
mod tests;
