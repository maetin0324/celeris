//! ADR-0079 D7（Phase R3a）: 人への決定の要求の一覧・回答・取り下げ・revise（`POST /decisions/{id}/answer` など、
//! MCP `decision_answer` / `decision_list`、受信箱の「決定」の節）。
//!
//! - 回答は 1 トランザクション（`TaskStore::decision_resolve_apply`）: 決定が今も `open` であることを確かめ、
//!   `DecisionAnswered`（表の行は store が同じトランザクションで書く）と、待っていた unit の再評価
//!   （`blocked(decision)` → `pending` / `ready`、取り下げなら `cancelled`）と、効き目の event（replan の依頼 =
//!   `ExecutionHintSet{replan: true}`、atomic の run への注入 = `Answered`）を積む。
//! - 効き目は決定の種類と選択肢だけから決まる（`task_core::decision::answer_effect`。表は ADR-0079 付記 R3a）。
//!   run 時の木の上限の余裕（`raise-once`）・`plan_invalid` の atomic / replan は daemon が回答済みの行を読んで
//!   決定的に当てる（`task_dispatch`）。ここは store の読み書きだけで、LLM は呼ばない（DESIGN 原則 1）。
//! - 節点の中止（`needed_before: [self]` の取り下げ）だけは回答の後の別の書き込み（既存の `gate::cancel`）。
//!   木の子なら先に親の unit を `cancelled` にする（子の中止を「失敗」として親の replan に写さない）。

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::decision::{
    self, DecisionEffect, DecisionKind, DecisionRequest, DecisionRow, DecisionStatus,
    NEEDED_BEFORE_SELF, NEEDED_BEFORE_STAGE_PREFIX,
};
use task_core::{
    Event, ExecutionMode, Task, TaskId, TaskStore, WorkUnitBlockedReason, WorkUnitRow,
    WorkUnitStatus,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::OpsError;

/// 回答で `blocked(decision)` から戻した unit の遷移の reason。
pub const RESUME_REASON: &str = "decision_answered";
/// 取り下げで `cancelled` にした unit の遷移の reason。
pub const WITHDRAW_REASON: &str = "decision_withdrawn";
/// atomic の run（`needed_before: [self]` の worker の決定）への回答を `Event::Answered` で渡すときの問いの接頭辞。
pub const ANSWERED_QUESTION_PREFIX: &str = "人の決定（ADR-0079 D7）: ";

/// `GET /decisions` / `GET /tasks/{id}/decisions` の 1 件。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DecisionView {
    pub decision: DecisionRequest,
    /// 決定を出した節点（`Event::DecisionRequested` を積んだ task）。
    pub task_id: TaskId,
    pub root_id: TaskId,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
    /// 回答済みなら、その回答の効き目（`answer_effect`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<DecisionEffect>,
}

impl DecisionView {
    pub fn from_row(row: &DecisionRow) -> Self {
        DecisionView {
            decision: row.request.clone(),
            task_id: row.task_id,
            root_id: row.root_id,
            created_at: row.created_at.clone(),
            answered_at: row.answered_at.clone(),
            effect: row
                .request
                .answer
                .as_ref()
                .map(|a| decision::answer_effect(row.kind, &a.option)),
        }
    }
}

/// `GET /decisions` の応答。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DecisionList {
    pub items: Vec<DecisionView>,
}

/// `POST /decisions/{id}/answer`・`revise` の本文（`option` は決定の選択肢の key。`choice` の決定だけ、
/// `option` を省いて `note` に自由記述で答えられる）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionAnswerBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `POST /decisions/{id}/withdraw` の本文（省略可）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionWithdrawBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 回答・取り下げ・revise の結果。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DecisionOutcome {
    pub decision: DecisionView,
    pub effect: DecisionEffect,
    /// `blocked(decision)` から戻した unit（`pending` / `ready`）の key。
    pub resumed: Vec<String>,
    /// 取り下げた（`cancelled` にした）unit の key。
    pub cancelled: Vec<String>,
    /// 決定を出した節点の replan を依頼した（`ExecutionHintSet{replan: true}`）。
    pub replan_requested: bool,
    /// 中止した節点（`needed_before: [self]` の取り下げ）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled_task: Option<TaskId>,
    /// revise で、既に作られた子 task にコメントとして届けた先。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notified_children: Vec<TaskId>,
}

/// 一覧の絞り込み（`GET /decisions?open=&root_id=`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecisionFilter {
    /// `Some(true)` = 未回答だけ、`Some(false)` = 回答済み・取り下げ済みだけ、`None` = すべて。
    pub open: Option<bool>,
    pub root_id: Option<TaskId>,
}

/// `GET /decisions`（`created_at` 昇順）。
pub fn list(store: &dyn TaskStore, filter: &DecisionFilter) -> Result<DecisionList, OpsError> {
    let rows = store.decisions_list(filter.root_id)?;
    let items = rows
        .iter()
        .filter(|r| match filter.open {
            Some(true) => r.status == DecisionStatus::Open,
            Some(false) => r.status != DecisionStatus::Open,
            None => true,
        })
        .map(DecisionView::from_row)
        .collect();
    Ok(DecisionList { items })
}

/// `GET /tasks/{id}/decisions`: その task の subtree（その task か子孫が出した決定。`path` にその task を含む）。
pub fn for_subtree(
    store: &dyn TaskStore,
    task_id: TaskId,
    open: Option<bool>,
) -> Result<DecisionList, OpsError> {
    let task = store.get(task_id)?.ok_or(OpsError::NotFound(task_id))?;
    let root = task_core::tree::root_id_of(&task);
    let mut out = list(
        store,
        &DecisionFilter {
            open,
            root_id: Some(root),
        },
    )?;
    out.items
        .retain(|v| v.task_id == task_id || v.decision.path.iter().any(|p| p.task_id == task_id));
    Ok(out)
}

/// 未回答の決定の数（`DaemonSnapshot.decisions_open`・受信箱の件数）。決定を出した節点が終端のものは数えない。
pub fn open_count(store: &dyn TaskStore) -> Result<u32, OpsError> {
    Ok(u32::try_from(open_rows(store)?.len()).unwrap_or(u32::MAX))
}

/// 未回答で、決定を出した節点が終端でない行（受信箱・通知の対象）。
pub fn open_rows(store: &dyn TaskStore) -> Result<Vec<DecisionRow>, OpsError> {
    let mut out = Vec::new();
    let mut terminal: BTreeMap<TaskId, bool> = BTreeMap::new();
    for row in store.decisions_list(None)? {
        if row.status != DecisionStatus::Open {
            continue;
        }
        let is_terminal = match terminal.get(&row.task_id) {
            Some(t) => *t,
            None => {
                let t = store
                    .get(row.task_id)?
                    .is_none_or(|t| t.status.is_terminal());
                terminal.insert(row.task_id, t);
                t
            }
        };
        if !is_terminal {
            out.push(row);
        }
    }
    Ok(out)
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

fn not_open(row: &DecisionRow, action: &str) -> OpsError {
    OpsError::DecisionNotOpen {
        id: row.id.clone(),
        status: row.status.as_str().to_string(),
        action: action.to_string(),
    }
}

fn get_row(store: &dyn TaskStore, id: &str) -> Result<DecisionRow, OpsError> {
    store
        .decision_get(id)?
        .ok_or_else(|| OpsError::DecisionNotFound(id.to_string()))
}

/// 決定を出した節点（終端なら 409。終端の節点の決定に答えても何も起きない）。
fn live_node(store: &dyn TaskStore, row: &DecisionRow, action: &str) -> Result<Task, OpsError> {
    let node = store
        .get(row.task_id)?
        .ok_or(OpsError::NotFound(row.task_id))?;
    if node.status.is_terminal() {
        return Err(OpsError::DecisionNotOpen {
            id: row.id.clone(),
            status: format!(
                "{}; the node {} is {:?}",
                row.status.as_str(),
                node.id,
                node.status
            ),
            action: action.to_string(),
        });
    }
    Ok(node)
}

/// この決定が unit を名指しで止めているか（`needed_before` にその unit か `stage:<その段階>`）。
fn names_unit(d: &DecisionRequest, u: &WorkUnitRow) -> bool {
    d.needed_before.iter().any(|n| {
        n == &u.key
            || u.phase.as_deref().is_some_and(|p| {
                n.strip_prefix(NEEDED_BEFORE_STAGE_PREFIX)
                    .is_some_and(|s| s == p)
            })
    })
}

/// 決定がまだ unit を止めているか: 未回答、または `replan` で答えてまだ replan が済んでいない（回答の後に
/// 計画が採用されていない）。
fn still_holds(row: &DecisionRow, answered_after_last_plan: &dyn Fn(&str) -> bool) -> bool {
    match row.status {
        DecisionStatus::Open => true,
        DecisionStatus::Answered => {
            row.request.answer.as_ref().is_some_and(|a| {
                decision::answer_effect(row.kind, &a.option) == DecisionEffect::Replan
            }) && answered_after_last_plan(&row.id)
        }
        DecisionStatus::Withdrawn => false,
    }
}

/// 節点の events で、`id` の最後の `DecisionAnswered` が最後の `ExecutionPlanned` より後か（`pending` の回答
/// = これから積むものは常に後）。
fn answered_after_last_plan_fn(
    events: &[(u64, Event)],
    pending: Option<String>,
) -> impl Fn(&str) -> bool + '_ {
    let last_plan = events
        .iter()
        .rposition(|(_, e)| matches!(e, Event::ExecutionPlanned { .. }));
    move |id: &str| {
        if pending.as_deref() == Some(id) {
            return true;
        }
        let answered = events
            .iter()
            .rposition(|(_, e)| matches!(e, Event::DecisionAnswered { id: a, .. } if a == id));
        match (answered, last_plan) {
            (Some(a), Some(p)) => a > p,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }
}

/// 回答の後の状態（`after`: 節点の決定の行。今回の回答・取り下げを当てたもの）で、`blocked(decision)` の
/// unit のうち、止めている決定が無く `needs_decisions` がすべて回答済みのものを戻す（依存が満たされて段階が
/// 今の段階なら `ready`、でなければ `pending`）。戻す行と `WorkUnitTransitioned` を返す。
fn release_rows(
    units: &[WorkUnitRow],
    after: &[DecisionRow],
    answered_after_last_plan: &dyn Fn(&str) -> bool,
    now: &str,
) -> Vec<(WorkUnitRow, Event)> {
    let answered: BTreeSet<&str> = after
        .iter()
        .filter(|r| r.status == DecisionStatus::Answered)
        .map(|r| r.key.as_str())
        .collect();
    let mut released: Vec<String> = Vec::new();
    let mut sim: Vec<WorkUnitRow> = units.to_vec();
    for u in sim.iter_mut() {
        if u.status != WorkUnitStatus::Blocked
            || u.blocked_reason != Some(WorkUnitBlockedReason::Decision)
        {
            continue;
        }
        let held = after
            .iter()
            .any(|d| still_holds(d, answered_after_last_plan) && names_unit(&d.request, u));
        let needs_ok = u
            .needs_decisions
            .iter()
            .all(|k| answered.contains(k.as_str()));
        if held || !needs_ok {
            continue;
        }
        u.status = WorkUnitStatus::Pending;
        u.blocked_reason = None;
        released.push(u.id.clone());
    }
    let ready: BTreeSet<String> = task_core::newly_ready(&sim).into_iter().collect();
    let mut out = Vec::new();
    for u in sim.iter_mut().filter(|u| released.contains(&u.id)) {
        if ready.contains(&u.id) {
            u.status = WorkUnitStatus::Ready;
        }
        u.updated_at = now.to_string();
        let event = Event::WorkUnitTransitioned {
            work_unit_id: u.id.clone(),
            key: u.key.clone(),
            from: WorkUnitStatus::Blocked,
            to: u.status,
            reason: RESUME_REASON.to_string(),
            run_id: None,
        };
        out.push((u.clone(), event));
    }
    out
}

/// 取り下げ: 決定が名指しした unit（と `needs_decisions` にその決定を持つ unit）で、まだ走っていないもの
/// （`pending` / `ready` / `blocked`）を `cancelled` にする。それに（推移的に）依存する未着手の unit も
/// 取り下げる（依存先の無い unit を残すと二度と ready にならない）。
fn withdraw_rows(units: &[WorkUnitRow], d: &DecisionRow, now: &str) -> Vec<(WorkUnitRow, Event)> {
    let cancellable = |u: &WorkUnitRow| {
        matches!(
            u.status,
            WorkUnitStatus::Pending | WorkUnitStatus::Ready | WorkUnitStatus::Blocked
        ) && u.kind != task_core::WorkUnitKind::Integrate
    };
    let mut keys: BTreeSet<String> = units
        .iter()
        .filter(|u| cancellable(u))
        .filter(|u| names_unit(&d.request, u) || u.needs_decisions.contains(&d.key))
        .map(|u| u.key.clone())
        .collect();
    loop {
        let more: Vec<String> = units
            .iter()
            .filter(|u| cancellable(u) && !keys.contains(&u.key))
            .filter(|u| u.depends_on.iter().any(|dep| keys.contains(dep)))
            .map(|u| u.key.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        keys.extend(more);
    }
    units
        .iter()
        .filter(|u| keys.contains(&u.key) && cancellable(u))
        .map(|u| {
            let mut row = u.clone();
            row.status = WorkUnitStatus::Cancelled;
            row.blocked_reason = None;
            row.updated_at = now.to_string();
            let event = Event::WorkUnitTransitioned {
                work_unit_id: u.id.clone(),
                key: u.key.clone(),
                from: u.status,
                to: WorkUnitStatus::Cancelled,
                reason: WITHDRAW_REASON.to_string(),
                run_id: None,
            };
            (row, event)
        })
        .collect()
}

fn is_node_scoped(d: &DecisionRequest) -> bool {
    d.needed_before.iter().any(|n| n == NEEDED_BEFORE_SELF)
}

/// 決定の効き目を unit と events に写す（回答・取り下げで共有）。`after` は今回の回答・取り下げを当てた
/// 節点の決定の行。
#[derive(Debug, Clone)]
struct Plan {
    rows: Vec<WorkUnitRow>,
    events: Vec<Event>,
    resumed: Vec<String>,
    cancelled: Vec<String>,
    replan_requested: bool,
    cancel_node: bool,
}

#[allow(clippy::too_many_arguments)]
fn plan_effect(
    store: &dyn TaskStore,
    node: &Task,
    row: &DecisionRow,
    after_row: &DecisionRow,
    effect: DecisionEffect,
    source: &str,
    note: Option<&str>,
    now: OffsetDateTime,
) -> Result<Plan, OpsError> {
    let now_s = rfc3339(now);
    let units = store.work_units_for(node.id)?;
    let events = store.events_for(node.id)?;
    let after: Vec<DecisionRow> = store
        .decisions_list(Some(row.root_id))?
        .into_iter()
        .filter(|r| r.task_id == node.id)
        .map(|r| if r.id == row.id { after_row.clone() } else { r })
        .collect();
    let answered_after = answered_after_last_plan_fn(&events, Some(row.id.clone()));
    let mut plan = Plan {
        rows: Vec::new(),
        events: Vec::new(),
        resumed: Vec::new(),
        cancelled: Vec::new(),
        replan_requested: false,
        cancel_node: false,
    };
    match effect {
        DecisionEffect::Withdraw => {
            for (r, e) in withdraw_rows(&units, row, &now_s) {
                plan.cancelled.push(r.key.clone());
                plan.rows.push(r);
                plan.events.push(e);
            }
            plan.cancel_node = is_node_scoped(&row.request);
        }
        DecisionEffect::Resume
        | DecisionEffect::RaiseOnce
        | DecisionEffect::Replan
        | DecisionEffect::Atomic => {
            for (r, e) in release_rows(&units, &after, &answered_after, &now_s) {
                plan.resumed.push(r.key.clone());
                plan.rows.push(r);
                plan.events.push(e);
            }
        }
    }
    // replan の依頼（計画を持つ節点だけ。`plan_invalid` はさらに daemon が planner の試行の窓を開け直し、note を
    // planner に渡す。初回の計画〈計画が無い〉の `plan_invalid` は止めが外れるだけで planner がもう一度走る）。
    if effect == DecisionEffect::Replan && store.execution_plan_active(node.id)?.is_some() {
        let previous = node.routing.as_ref().and_then(|r| r.execution_hint);
        let mut text = format!(
            "決定 {}「{}」への回答で replan",
            row.key, row.request.question
        );
        if let Some(n) = note.filter(|n| !n.trim().is_empty()) {
            text.push_str(&format!(": {}", n.trim()));
        }
        plan.events.push(Event::ExecutionHintSet {
            mode: ExecutionMode::Compound,
            previous,
            previous_decision: None,
            source: format!("{source} (decision {})", row.id),
            note: Some(text),
            replan: true,
        });
        plan.replan_requested = true;
    }
    // atomic の run が出した `self` の決定（worker）: 次の run の前置きの `answers` に答えを渡す。
    if matches!(effect, DecisionEffect::Resume)
        && row.kind == DecisionKind::Choice
        && is_node_scoped(&row.request)
        && let Some(line) = decision::answer_line(&after_row.request)
    {
        plan.events.push(Event::Answered {
            question: format!(
                "{ANSWERED_QUESTION_PREFIX}{} {}",
                row.key, row.request.question
            ),
            answer: line,
        });
    }
    Ok(plan)
}

/// 節点の中止（`needed_before: [self]` の取り下げ）。木の子なら先に親の unit を `cancelled` にする
/// （子の中止を失敗として親の replan に写さない）。既に終端なら何もしない。
fn cancel_node(
    store: &dyn TaskStore,
    node_id: TaskId,
    now: OffsetDateTime,
) -> Result<(), OpsError> {
    let Some(node) = store.get(node_id)? else {
        return Ok(());
    };
    if node.status.is_terminal() {
        return Ok(());
    }
    if let Some(pu) = node.tree.as_ref().and_then(|t| t.parent_unit.as_ref()) {
        let units = store.work_units_for(pu.task_id)?;
        if let Some(u) = units.iter().find(|u| {
            u.child_task_id.as_deref() == Some(node_id.to_string().as_str())
                && !u.status.is_terminal()
        }) {
            let mut row = u.clone();
            row.status = WorkUnitStatus::Cancelled;
            row.blocked_reason = None;
            row.updated_at = rfc3339(now);
            store.work_units_apply(
                pu.task_id,
                Vec::new(),
                vec![row],
                vec![Event::WorkUnitTransitioned {
                    work_unit_id: u.id.clone(),
                    key: u.key.clone(),
                    from: u.status,
                    to: WorkUnitStatus::Cancelled,
                    reason: WITHDRAW_REASON.to_string(),
                    run_id: None,
                }],
            )?;
        }
    }
    crate::gate::cancel(store, node_id, None)?;
    Ok(())
}

/// `POST /decisions/{id}/answer`（管理系）/ MCP `decision_answer`（`by` = `"human"` / `"mcp:<client>"`）。
///
/// - 無い id は `DecisionNotFound`（404）、`open` でない・節点が終端なら `DecisionNotOpen`（409）、選択肢の外・
///   daemon の決定で `option` 無しは `Validation`（422）。
/// - 1 トランザクションで `DecisionAnswered` と、待っていた unit の再評価（効き目の表どおり）と効き目の event。
pub fn answer(
    store: &dyn TaskStore,
    id: &str,
    option: Option<&str>,
    note: Option<&str>,
    by: &str,
    now: OffsetDateTime,
) -> Result<DecisionOutcome, OpsError> {
    let plan = plan_answer(store, id, option, note, by, now)?;
    apply_plan(store, &plan)?;
    finish_answer(store, plan, now)
}

/// 決定への書き込み計画（読むだけ）。回答・取り下げ・訂正が同じ形を使う。
/// `decision_resolve_apply(node_id, decision_id, expect, rows, events)` で書き、結果を [`finish_answer`] に渡す。
/// CoS の操作（ADR 2026-10-05 D3・ADR 2026-10-09 D5）は同じ計画を監査と同じトランザクションで
/// `SqliteStore::decision_resolve_apply_tx` に渡す。
#[derive(Debug, Clone)]
pub struct AnswerPlan {
    pub node_id: TaskId,
    pub decision_id: String,
    /// 書き込み時に期待する決定の状態（回答・取り下げは `Open`、訂正は `Answered`）。
    pub expect: DecisionStatus,
    /// 競合（期待した状態でない）のときの 409 に載せる動詞。
    pub verb: &'static str,
    effect: DecisionEffect,
    plan: Plan,
    /// 訂正だけ: 既に作られた子へ新しい答えをコメントで届ける（commit 後の後始末）。
    revise: Option<RevisePlan>,
}

#[derive(Debug, Clone)]
struct RevisePlan {
    row: DecisionRow,
    line: String,
    by: String,
}

impl AnswerPlan {
    pub fn rows(&self) -> Vec<WorkUnitRow> {
        self.plan.rows.clone()
    }

    pub fn events(&self) -> Vec<Event> {
        self.plan.events.clone()
    }
}

/// 計画を書く（監査なしの経路）。期待した状態でなくなっていれば 409。
pub fn apply_plan(store: &dyn TaskStore, plan: &AnswerPlan) -> Result<(), OpsError> {
    if !store.decision_resolve_apply(
        plan.node_id,
        &plan.decision_id,
        plan.expect,
        plan.rows(),
        plan.events(),
    )? {
        let current = get_row(store, &plan.decision_id)?;
        return Err(not_open(&current, plan.verb));
    }
    Ok(())
}

pub fn plan_answer(
    store: &dyn TaskStore,
    id: &str,
    option: Option<&str>,
    note: Option<&str>,
    by: &str,
    now: OffsetDateTime,
) -> Result<AnswerPlan, OpsError> {
    let row = get_row(store, id)?;
    if row.status != DecisionStatus::Open {
        return Err(not_open(
            &row,
            "answered (only open decisions accept an answer; use revise for an answered choice)",
        ));
    }
    let node = live_node(store, &row, "answered")?;
    let option = decision::validate_answer(&row.request, option, note)
        .map_err(|e| OpsError::Validation(e.to_string()))?;
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    let effect = decision::answer_effect(row.kind, &option);
    let mut after_row = row.clone();
    after_row.apply_answer(&option, note, by, &rfc3339(now));
    let mut plan = plan_effect(store, &node, &row, &after_row, effect, by, note, now)?;
    let mut events = vec![Event::DecisionAnswered {
        id: row.id.clone(),
        option: option.clone(),
        note: note.map(str::to_string),
        by: by.to_string(),
    }];
    events.append(&mut plan.events);
    plan.events = events;
    Ok(AnswerPlan {
        node_id: node.id,
        decision_id: row.id,
        expect: DecisionStatus::Open,
        verb: "answered",
        effect,
        plan,
        revise: None,
    })
}

/// 書いた後の後始末（`self` の取り下げなら節点を中止、訂正なら既にある子へのコメント）と結果。
pub fn finish_answer(
    store: &dyn TaskStore,
    plan: AnswerPlan,
    now: OffsetDateTime,
) -> Result<DecisionOutcome, OpsError> {
    let mut cancelled_task = None;
    if plan.plan.cancel_node {
        cancel_node(store, plan.node_id, now)?;
        cancelled_task = Some(plan.node_id);
    }
    let mut notified = Vec::new();
    if let Some(revise) = &plan.revise {
        // 既に作られた子（この決定を待っていた kind task の unit の子）にコメントで届ける。
        let row = &revise.row;
        for u in store.work_units_for(plan.node_id)? {
            let waits = u.needs_decisions.contains(&row.key) || names_unit(&row.request, &u);
            let Some(child) = u
                .child_task_id
                .as_deref()
                .and_then(|s| s.parse::<TaskId>().ok())
            else {
                continue;
            };
            if !waits {
                continue;
            }
            if store.get(child)?.is_none_or(|t| t.status.is_terminal()) {
                continue;
            }
            crate::comment::post_node_comment(
                store,
                child,
                Some(revise.by.clone()),
                None,
                format!(
                    "{}（回答を変更しました。作り直しはしません）\n{}",
                    task_ops_decisions_heading(),
                    revise.line
                ),
                now,
            )?;
            notified.push(child);
        }
    }
    Ok(DecisionOutcome {
        decision: DecisionView::from_row(&get_row(store, &plan.decision_id)?),
        effect: plan.effect,
        resumed: plan.plan.resumed,
        cancelled: plan.plan.cancelled,
        replan_requested: plan.plan.replan_requested,
        cancelled_task,
        notified_children: notified,
    })
}

/// `POST /decisions/{id}/withdraw`（管理系）: 人が決定を取り下げる（`DecisionWithdrawn`）。効き目は選択肢の
/// `withdraw` と同じ（止めていた unit を取り下げ、`self` なら節点を中止）。`open` でなければ 409。
pub fn withdraw(
    store: &dyn TaskStore,
    id: &str,
    reason: Option<&str>,
    by: &str,
    now: OffsetDateTime,
) -> Result<DecisionOutcome, OpsError> {
    let plan = plan_withdraw(store, id, reason, by, now)?;
    apply_plan(store, &plan)?;
    finish_answer(store, plan, now)
}

/// [`withdraw`] の書き込み計画（読むだけ）。
pub fn plan_withdraw(
    store: &dyn TaskStore,
    id: &str,
    reason: Option<&str>,
    by: &str,
    now: OffsetDateTime,
) -> Result<AnswerPlan, OpsError> {
    let row = get_row(store, id)?;
    if row.status != DecisionStatus::Open {
        return Err(not_open(
            &row,
            "withdrawn (only open decisions can be withdrawn)",
        ));
    }
    let node = live_node(store, &row, "withdrawn")?;
    let reason = match reason.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => format!("{by}: {r}"),
        None => format!("withdrawn by {by}"),
    };
    let mut after_row = row.clone();
    after_row.apply_withdrawal(&reason);
    let mut plan = plan_effect(
        store,
        &node,
        &row,
        &after_row,
        DecisionEffect::Withdraw,
        by,
        None,
        now,
    )?;
    let mut events = vec![Event::DecisionWithdrawn {
        id: row.id.clone(),
        reason,
    }];
    events.append(&mut plan.events);
    plan.events = events;
    plan.resumed = Vec::new();
    plan.replan_requested = false;
    Ok(AnswerPlan {
        node_id: node.id,
        decision_id: row.id,
        expect: DecisionStatus::Open,
        verb: "withdrawn",
        effect: DecisionEffect::Withdraw,
        plan,
        revise: None,
    })
}

/// `POST /decisions/{id}/revise`（管理系）: 回答済みの `choice` の決定の答えを変える（新しい `DecisionAnswered`。
/// 最後の回答が有効）。まだ作られていない子・これから走る leaf は新しい答えを読む。**既に作られた子は作り直さず**、
/// 人のコメント（ADR-0044 D2、人を起こさない node のコメント）として新しい答えを届ける。daemon の決定
/// （limit / leaf_too_large / plan_invalid）は回答の時点で効き目を当てているので revise できない（409）。
pub fn revise(
    store: &dyn TaskStore,
    id: &str,
    option: Option<&str>,
    note: Option<&str>,
    by: &str,
    now: OffsetDateTime,
) -> Result<DecisionOutcome, OpsError> {
    let plan = plan_revise(store, id, option, note, by, now)?;
    apply_plan(store, &plan)?;
    finish_answer(store, plan, now)
}

/// [`revise`] の書き込み計画（読むだけ）。
pub fn plan_revise(
    store: &dyn TaskStore,
    id: &str,
    option: Option<&str>,
    note: Option<&str>,
    by: &str,
    now: OffsetDateTime,
) -> Result<AnswerPlan, OpsError> {
    let row = get_row(store, id)?;
    if row.status != DecisionStatus::Answered {
        return Err(not_open(
            &row,
            "revised (only answered decisions can be revised)",
        ));
    }
    if row.kind != DecisionKind::Choice {
        return Err(not_open(
            &row,
            "revised (a daemon decision took effect when it was answered; only kind=choice can be revised)",
        ));
    }
    let node = live_node(store, &row, "revised")?;
    let option = decision::validate_answer(&row.request, option, note)
        .map_err(|e| OpsError::Validation(e.to_string()))?;
    let note = note.map(str::trim).filter(|n| !n.is_empty());
    let mut after_row = row.clone();
    after_row.apply_answer(&option, note, by, &rfc3339(now));
    let events = vec![Event::DecisionAnswered {
        id: row.id.clone(),
        option: option.clone(),
        note: note.map(str::to_string),
        by: by.to_string(),
    }];
    let revise = decision::answer_line(&after_row.request).map(|line| RevisePlan {
        row: row.clone(),
        line,
        by: by.to_string(),
    });
    Ok(AnswerPlan {
        node_id: node.id,
        decision_id: row.id,
        expect: DecisionStatus::Answered,
        verb: "revised",
        effect: DecisionEffect::Resume,
        plan: Plan {
            rows: Vec::new(),
            events,
            resumed: Vec::new(),
            cancelled: Vec::new(),
            replan_requested: false,
            cancel_node: false,
        },
        revise,
    })
}

fn task_ops_decisions_heading() -> &'static str {
    crate::tree::DECISIONS_HEADING
}

/// 受信箱の「決定」の 1 件（ADR-0079 D7 の `AttentionItem::Decision` の形。受信箱では独立の節 `decisions`）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DecisionInboxItem {
    pub id: String,
    pub key: String,
    pub kind: DecisionKind,
    /// 決定を出した節点。
    pub task_id: TaskId,
    pub root_id: TaskId,
    /// root から出した節点まで（パンくず）。
    pub path: Vec<task_core::DecisionPathEntry>,
    pub question: String,
    pub options: Vec<task_core::DecisionOption>,
    pub recommended: String,
    pub cost_of_reversal: task_core::CostOfReversal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_note: Option<String>,
    pub needed_before: Vec<String>,
    pub origin: task_core::DecisionOrigin,
    pub created_at: String,
    /// 経過秒（`now − created_at`。読めなければ 0）。
    pub age_secs: u64,
}

/// 受信箱の「決定」の節（未回答・節点が終端でないもの。古い順）。
pub fn inbox_items(
    store: &dyn TaskStore,
    now: OffsetDateTime,
) -> Result<Vec<DecisionInboxItem>, OpsError> {
    Ok(open_rows(store)?
        .into_iter()
        .map(|r| {
            let age_secs = OffsetDateTime::parse(&r.created_at, &Rfc3339)
                .map(|t| u64::try_from((now - t).whole_seconds()).unwrap_or(0))
                .unwrap_or(0);
            DecisionInboxItem {
                id: r.id.clone(),
                key: r.key.clone(),
                kind: r.kind,
                task_id: r.task_id,
                root_id: r.root_id,
                path: r.request.path.clone(),
                question: r.request.question.clone(),
                options: r.request.options.clone(),
                recommended: r.request.recommended.clone(),
                cost_of_reversal: r.request.cost_of_reversal,
                cost_note: r.request.cost_note.clone(),
                needed_before: r.request.needed_before.clone(),
                origin: r.request.raised_by.origin,
                created_at: r.created_at.clone(),
                age_secs,
            }
        })
        .collect())
}

/// ADR-0079 D7（Phase R3a）: `leaf`（WU）の前置きの「人の決定」節に入れる行（その unit が待っていた決定
/// 〈`needs_decisions`〉と、その unit を名指しした決定〈worker の `self` を含む〉の回答。固定の書式）。
pub fn leaf_decision_lines(
    store: &dyn TaskStore,
    task_id: TaskId,
    root_id: TaskId,
    unit: &WorkUnitRow,
) -> Result<Vec<String>, OpsError> {
    let mut out = Vec::new();
    for r in store.decisions_list(Some(root_id))? {
        if r.task_id != task_id || r.status != DecisionStatus::Answered {
            continue;
        }
        if !(unit.needs_decisions.contains(&r.key) || names_unit(&r.request, unit)) {
            continue;
        }
        if let Some(line) = decision::answer_line(&r.request) {
            out.push(line);
        }
    }
    Ok(out)
}

/// ADR-0079 D7（Phase R3a）: 回答済みの run 時の上限（`limit:*` の `raise-once` / `replan`）の件数を種類ごとに
/// （木全体の上限は木で、節点の replan〈`max_replans`〉は `node` の決定だけ）。daemon が上限に余裕を足すのに使う。
pub fn limit_allowances(
    store: &dyn TaskStore,
    root_id: TaskId,
    node: Option<TaskId>,
) -> Result<BTreeMap<task_core::TreeLimitKind, u32>, OpsError> {
    let mut out: BTreeMap<task_core::TreeLimitKind, u32> = BTreeMap::new();
    for r in store.decisions_list(Some(root_id))? {
        if r.kind != DecisionKind::Limit || r.status != DecisionStatus::Answered {
            continue;
        }
        let Some(kind) = task_core::TreeLimitKind::from_decision_key(&r.key) else {
            continue;
        };
        if !kind.is_run_time() {
            continue;
        }
        if kind == task_core::TreeLimitKind::NodeReplans && Some(r.task_id) != node {
            continue;
        }
        let effect = r
            .request
            .answer
            .as_ref()
            .map(|a| decision::answer_effect(r.kind, &a.option));
        if matches!(
            effect,
            Some(DecisionEffect::RaiseOnce) | Some(DecisionEffect::Replan)
        ) {
            *out.entry(kind).or_insert(0) += 1;
        }
    }
    Ok(out)
}
