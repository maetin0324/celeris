//! 受信箱（`docs/gui/api.md` §3.2 / §5.1 / §6.2）。原則 5「人間は承認待ちキューだけを見ればよい」の画面の元データ。

use std::collections::{BTreeMap, HashMap, HashSet};

use schemars::JsonSchema;
use serde::Serialize;
use task_core::{
    ArtifactRef, Check, Event, Status, Task, TaskId, TaskKind, TaskStore, WorkerHint, WorkspaceSpec,
};
use time::OffsetDateTime;

use crate::daemon::DaemonSnapshot;
use crate::derive::{self, AnswerNote};
use crate::error::OpsError;
use crate::view::{
    self, ApprovalDecisionView, RunOutcomeKind, RunSummary, TaskRef, TaskSummary, VerdictView,
    ViewContext,
};

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Inbox {
    pub approvals: Vec<ApprovalItem>,
    pub questions: Vec<QuestionItem>,
    pub drafts: Vec<DraftGroup>,
    pub attention: Vec<AttentionItem>,
    /// ADR-0131 D7: 表示から外した attention の件数（規則別）。events は保持する。
    pub suppressed: BTreeMap<String, u32>,
    /// ADR-0080 D5: 人の対応（credential の登録・一回だけの承認・拒否）を待っている browser の wait。
    pub browser_waits: Vec<crate::browser::BrowserWaitItem>,
    /// ADR-0079 D7（Phase R3a）: 未回答の決定の要求（path・問い・推奨・止めている unit・経過時間）。
    /// 回答は `POST /decisions/{id}/answer`。
    pub decisions: Vec<crate::decision::DecisionInboxItem>,
    pub counts: InboxCounts,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct InboxCounts {
    pub approvals: u32,
    pub questions: u32,
    pub drafts: u32,
    pub attention: u32,
    /// ADR-0080 D5: `browser_waits` の件数。
    pub browser_waits: u32,
    /// ADR-0079 D7（Phase R3a）: 未回答の決定の要求の件数。
    pub decisions: u32,
    /// status 名 → 件数（DB 全体）。
    pub by_status: std::collections::BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ApprovalItem {
    pub approval: TaskRef,
    pub parent: Option<TaskRef>,
    pub criterion_text: String,
    pub criterion_idx: Option<usize>,
    pub attempt: Option<u32>,
    pub requested_at: String,
    pub last_run: Option<RunSummary>,
    pub evidence: Vec<EvidenceView>,
    pub other_verdicts: Vec<VerdictView>,
    pub artifacts: Vec<ApprovalArtifact>,
    /// ADR-0067 D4: 親タスクの `acceptance` にある `knowledge_page` の参照（この承認の判断材料の一部かも
    /// しれない、知識ベースのページへのリンク。GUI が「知識ベースを見る」ボタンを出すのに使う）。
    pub knowledge_pages: Vec<KnowledgePageRef>,
    pub previous_decisions: Vec<ApprovalDecisionView>,
}

/// ADR-0067 D4: 成果物 1 件と、`GET /tasks/{parent_id}/artifacts/{idx}` の添字（GUI がその場で本文を
/// 取りに行くのに使う。`derive::artifacts_for_run_with_idx` と同じ番号づけ）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ApprovalArtifact {
    pub idx: usize,
    #[serde(flatten)]
    pub artifact: ArtifactRef,
}

/// ADR-0067 D4: `Check::KnowledgePage` 1 件（どの受け入れ条件の話かと、そのページの KB 相対パス）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct KnowledgePageRef {
    pub criterion_idx: usize,
    pub path: String,
}

/// `task_worker::Evidence` と同じ形。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct EvidenceView {
    pub criterion: usize,
    pub command: Option<String>,
    pub exit: Option<i32>,
    pub stdout_tail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct QuestionItem {
    pub task: TaskRef,
    pub question: String,
    pub asked_at: Option<String>,
    pub run_id: Option<String>,
    pub previous: Vec<AnswerNote>,
    /// GUI 監査対応 Phase 29: 対応する未決の `approvals` の id（`POST /approvals/{id}/decide` へ
    /// GUI が直接リンクできるように）。無ければ `null`（`approvals` の行がまだ無い、または既に決定済み）。
    pub approval_id: Option<task_core::approval::ApprovalId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct DraftGroup {
    pub parent: Option<TaskRef>,
    pub plan_summary: Option<String>,
    pub drafts: Vec<TaskSummary>,
    /// ADR-0074 D3.3 / D3.4（Phase F4b (h)）: 案件計画の未決の提案なら、その案件と版（GUI は
    /// `POST /projects/{project_id}/project-plan/{version}/decide` をそのまま呼べる）。replan の差分で
    /// `add` が無い（modify / remove / cancel だけの）提案は `drafts` が空でもこの 1 まとまりで出る。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_plan: Option<ProjectPlanRef>,
}

/// `DraftGroup.project_plan`。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ProjectPlanRef {
    pub project_id: task_core::ProjectId,
    pub version: u32,
    /// replan の差分なら元の版。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttentionItem {
    Failed {
        task: TaskRef,
        reason: String,
        at: String,
        /// ADR-0070 D1（Phase 116）: `infra`（lease失効・切替中断・result.json不在・セッション再開
        /// 拒否・レート制限・DB busy）か `work`（レビュー不合格・max_turns・ワーカーの明示的な
        /// error）かの分類（`task_ops::derive::classify_task_failure`）。
        class: derive::FailureClass,
        /// 配送済み（`deliveries` に `release` が付いた記録がある）なら sha12。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivered_release: Option<String>,
        /// ADR-0120 D5: review 前同期の衝突解消（IntegrationRepair）の現在の状況（履歴が無ければ省略）。
        /// `reason`/`class` は実装失敗（レビュー不合格・ワーカーの明示的な error）の分類であり、これは
        /// 別物（`exhausted` でも task を直接 `failed` にはしない）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        integration_repair: Option<view::IntegrationRepairView>,
    },
    RequeueLimitNear {
        task: TaskRef,
        count: u32,
        max: u32,
        at: String,
    },
    Unroutable {
        task: TaskRef,
        hint: WorkerHint,
        at: String,
    },
    /// ADR-0018 D2: 直近 24 時間に `ClusterUnavailable` があったクラスタ（人がログインし直すまで用件が続く）。
    ClusterUnavailable {
        cluster: String,
        host: String,
        at: String,
        tasks: u32,
    },
    /// ADR-0074 D2.4（Phase F3 途中確認）: 工程の後で止まった Task（`blocked(awaiting_human)`）。
    /// 質問ではない（`questions` には出さない）。`report_idx` は途中報告の Markdown
    /// （`GET /tasks/{id}/artifacts/{idx}`）。
    PhaseCheckpoint {
        task: TaskRef,
        phase: String,
        phase_title: String,
        /// 済んだ工程の数（止まった工程を含む）。
        phases_done: u32,
        phases_total: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        report_idx: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_phase: Option<String>,
        at: String,
    },
    /// ADR-0079 D8（Phase R3b）: root の計画が人の承認を待っている（`blocked(awaiting_plan_approval)`）。
    /// 質問ではない（`questions` には出さない）。操作は `POST /tasks/{id}/execution/plan-gate`
    /// （approve / replan / withdraw）。計画の決定の要求は受信箱の `decisions` に同じく出る（`decision_ids`）。
    PlanApproval {
        task: TaskRef,
        plan_id: String,
        plan_version: u32,
        /// `PlanApprovalRequested.reasons`（`decisions:…` / `review_human:…` / `near_limit:<設定名>:<値>/<上限>`）。
        reasons: Vec<String>,
        /// 理由の人が読む 1 行。
        summary: String,
        /// 計画の見取り図（段階ごとの unit）。
        stages: Vec<PlanApprovalStage>,
        /// この節点の未回答の決定の id（`decisions` の節の同じ id）。
        decision_ids: Vec<String>,
        at: String,
    },
    /// ADR-0121 D3: 完了した root の成果を main へ取り込み始められなかった（`Event::DeliverySkipped` の写し。
    /// 正本は event）。同じ head の delivery が後で作られたら出さない。
    DeliverySkipped {
        task: TaskRef,
        reason: task_core::DeliverySkipReason,
        /// 「完了したが main への取り込みを開始できなかった」と理由の人が読む 1 行。
        summary: String,
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        head: Option<String>,
        at: String,
    },
}

/// `AttentionItem::PlanApproval.stages[]`（計画の見取り図の 1 段階）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct PlanApprovalStage {
    pub key: String,
    pub title: String,
    /// `review: human`（段階の後で人の確認）。
    pub review_human: bool,
    /// `<key>: <title>（leaf | 子 task）` の 1 行ずつ。
    pub units: Vec<String>,
}

/// ADR-0131 D7 の表示抑制規則。順序は、同じ task が複数に当たるときの集計の優先順位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressRule {
    TerminalTask,
    ParentDone,
    AncestorCancelled,
    UnitRetriedDone,
}

impl SuppressRule {
    fn key(self) -> &'static str {
        match self {
            Self::TerminalTask => "r3_terminal_task",
            Self::ParentDone => "r1_parent_done",
            Self::AncestorCancelled => "r2_ancestor_cancelled",
            Self::UnitRetriedDone => "r4_unit_retried_done",
        }
    }
}

/// Task 木の現在状態だけから表示可否を決める。欠けた親や巡回は判定不能として残す。
pub fn attention_suppression(task: &Task, by_id: &HashMap<TaskId, Task>) -> Option<SuppressRule> {
    if matches!(task.status, Status::Done | Status::Cancelled) {
        return Some(SuppressRule::TerminalTask);
    }
    if task.status != Status::Failed {
        return None;
    }
    let parent_id = task_core::tree::tree_parent(task).or(task.parent_id);
    if parent_id
        .and_then(|id| by_id.get(&id))
        .is_some_and(|parent| parent.status == Status::Done)
    {
        return Some(SuppressRule::ParentDone);
    }
    let mut seen = HashSet::new();
    let mut ancestor = parent_id;
    while let Some(id) = ancestor {
        if !seen.insert(id) {
            break;
        }
        let Some(parent) = by_id.get(&id) else { break };
        if parent.status == Status::Cancelled {
            return Some(SuppressRule::AncestorCancelled);
        }
        ancestor = task_core::tree::tree_parent(parent).or(parent.parent_id);
    }
    let unit = task.tree.as_ref()?.parent_unit.as_ref()?;
    by_id
        .values()
        .find(|other| {
            other.id != task.id
                && other.status == Status::Done
                && other.created_at > task.created_at
                && other
                    .tree
                    .as_ref()
                    .and_then(|tree| tree.parent_unit.as_ref())
                    .is_some_and(|candidate| {
                        candidate.task_id == unit.task_id && candidate.unit_key == unit.unit_key
                    })
        })
        .map(|_| SuppressRule::UnitRetriedDone)
}

fn attention_task(item: &AttentionItem) -> Option<TaskId> {
    match item {
        AttentionItem::Failed { task, .. }
        | AttentionItem::RequeueLimitNear { task, .. }
        | AttentionItem::Unroutable { task, .. }
        | AttentionItem::PhaseCheckpoint { task, .. }
        | AttentionItem::PlanApproval { task, .. }
        | AttentionItem::DeliverySkipped { task, .. } => Some(task.id),
        AttentionItem::ClusterUnavailable { .. } => None,
    }
}

fn attention_at(item: &AttentionItem) -> &str {
    match item {
        AttentionItem::Failed { at, .. } => at,
        AttentionItem::RequeueLimitNear { at, .. } => at,
        AttentionItem::Unroutable { at, .. } => at,
        AttentionItem::ClusterUnavailable { at, .. } => at,
        AttentionItem::PhaseCheckpoint { at, .. } => at,
        AttentionItem::PlanApproval { at, .. } => at,
        AttentionItem::DeliverySkipped { at, .. } => at,
    }
}

/// `kind == Approval && status == Ready` のタスク（Human check の Approval 子）を集める。
fn build_approvals(
    store: &dyn TaskStore,
    all_tasks: &[Task],
    by_id: &HashMap<TaskId, Task>,
    evidence: &dyn Fn(&Task, &str) -> Vec<EvidenceView>,
) -> Result<Vec<ApprovalItem>, OpsError> {
    let mut items = Vec::new();

    for t in all_tasks
        .iter()
        .filter(|t| t.kind == TaskKind::Approval && t.status == Status::Ready)
    {
        let parent = t.parent_id.and_then(|pid| by_id.get(&pid));
        let parsed = view::parse_human_approval_title(&t.title);
        let criterion_idx = parsed.map(|(i, _)| i);
        let attempt = parsed.map(|(_, a)| a);

        let criterion_text = match (parent, criterion_idx) {
            (Some(p), Some(idx)) => p
                .acceptance
                .get(idx)
                .map(|c| c.text.clone())
                .unwrap_or_else(|| t.objective.clone()),
            _ => t.objective.clone(),
        };

        let own_rows = store.event_rows_for(t.id, None, view::ALL_EVENTS)?;
        let requested_at = own_rows
            .iter()
            .find_map(|r| match &r.event {
                Event::ApprovalRequested => Some(r.ts.clone()),
                _ => None,
            })
            .unwrap_or_else(|| view::to_rfc3339(t.created_at));

        let mut last_run: Option<RunSummary> = None;
        let mut artifacts: Vec<ApprovalArtifact> = Vec::new();
        let mut other_verdicts: Vec<VerdictView> = Vec::new();
        let mut evidence_items: Vec<EvidenceView> = Vec::new();

        if let Some(parent_task) = parent {
            let parent_rows = store.event_rows_for(parent_task.id, None, view::ALL_EVENTS)?;
            let parent_events = view::seq_pairs(&parent_rows);
            if let Some(run_id) = derive::last_run_id(&parent_events) {
                let run_summaries = view::runs(&parent_rows);
                last_run = run_summaries.into_iter().find(|r| r.run_id == run_id);
                // ADR-0067 D4: `idx` は `GET /tasks/{parent_id}/artifacts/{idx}` と同じ添字。
                artifacts = derive::artifacts_for_run_with_idx(&parent_events, &run_id)
                    .into_iter()
                    .map(|(idx, artifact)| ApprovalArtifact { idx, artifact })
                    .collect();
                for r in &parent_rows {
                    if let Event::ReviewVerdict {
                        run_id: rid,
                        criterion_idx: c_idx,
                        pass,
                        reason,
                    } = &r.event
                        && rid == &run_id
                    {
                        other_verdicts.push(VerdictView {
                            run_id: rid.clone(),
                            criterion_idx: *c_idx,
                            pass: *pass,
                            reason: reason.clone(),
                            ts: r.ts.clone(),
                        });
                    }
                }
                let is_done = matches!(
                    last_run.as_ref().and_then(|r| r.outcome),
                    Some(RunOutcomeKind::Done)
                );
                if is_done {
                    evidence_items = evidence(parent_task, &run_id);
                }
            }
        }

        let mut previous_decisions: Vec<ApprovalDecisionView> = Vec::new();
        if let (Some(parent_task), Some(idx)) = (parent, criterion_idx) {
            for sibling in all_tasks.iter().filter(|s| {
                s.parent_id == Some(parent_task.id) && s.kind == TaskKind::Approval && s.id != t.id
            }) {
                if view::parse_human_approval_title(&sibling.title).map(|(i, _)| i) != Some(idx) {
                    continue;
                }
                let sib_rows = store.event_rows_for(sibling.id, None, view::ALL_EVENTS)?;
                if let Some(decided) = sib_rows.iter().rev().find_map(|r| match &r.event {
                    Event::ApprovalDecided { by, approved, note } => Some(ApprovalDecisionView {
                        by: by.clone(),
                        approved: *approved,
                        note: note.clone(),
                        ts: r.ts.clone(),
                    }),
                    _ => None,
                }) {
                    previous_decisions.push(decided);
                }
            }
            previous_decisions.sort_by(|a, b| a.ts.cmp(&b.ts));
        }

        // ADR-0067 D4: 親タスクの `knowledge_page` 参照（人が判断材料としてリンクを開けるように）。
        let knowledge_pages: Vec<KnowledgePageRef> = parent
            .map(|p| {
                p.acceptance
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, c)| match &c.check {
                        Check::KnowledgePage { path } => Some(KnowledgePageRef {
                            criterion_idx: idx,
                            path: path.clone(),
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        items.push(ApprovalItem {
            approval: view::task_ref(t),
            parent: parent.map(view::task_ref),
            criterion_text,
            criterion_idx,
            attempt,
            requested_at,
            last_run,
            evidence: evidence_items,
            other_verdicts,
            artifacts,
            knowledge_pages,
            previous_decisions,
        });
    }

    items.sort_by(|a, b| a.requested_at.cmp(&b.requested_at));
    Ok(items)
}

/// `status == Blocked` のタスク。
fn build_questions(
    store: &dyn TaskStore,
    all_tasks: &[Task],
) -> Result<Vec<QuestionItem>, OpsError> {
    // GUI 監査対応 Phase 29: 未決の approvals を task_id で引けるように 1 回だけ読む。
    let pending_approval_by_task: HashMap<TaskId, task_core::approval::ApprovalId> = store
        .approval_list(Some(true), None, None)?
        .into_iter()
        .filter_map(|a| a.task_id.map(|task_id| (task_id, a.id)))
        .collect();

    // ADR-0080 D5: browser の wait で止まっている task は質問ではない（`browser_waits` に出し、
    // 一般の回答では再開できない）。
    let browser_waiting: std::collections::HashSet<TaskId> = store
        .browser_waits_pending()?
        .into_iter()
        .map(|w| w.task_id)
        .collect();
    // ADR-0090 D4: クラスタ job を待っている（`waiting` の wait）task も質問ではない（daemon が poll して再開する。
    // 上限を過ぎた wait は `timed_out` になり、その質問〈`QuestionRaised`〉はここに出る）。
    let cluster_waiting: std::collections::HashSet<TaskId> = store
        .cluster_job_waits_waiting()?
        .into_iter()
        .map(|w| w.task_id)
        .collect();
    let mut items = Vec::new();
    for t in all_tasks.iter().filter(|t| {
        t.status == Status::Blocked
            && !browser_waiting.contains(&t.id)
            && !cluster_waiting.contains(&t.id)
    }) {
        let rows = store.event_rows_for(t.id, None, view::ALL_EVENTS)?;
        let events = view::seq_pairs(&rows);
        // ADR-0074 D2.4（Phase F3 途中確認）: 工程の後の途中確認は質問ではない（attention に出す）。
        // ADR-0079 D8（Phase R3b）: root の計画の承認待ちも同じ（attention の `plan_approval`）。
        if crate::plan_gate::is_human_gate(t, &events) {
            continue;
        }
        let question = derive::latest_question(&events);

        let mut asked_at: Option<String> = None;
        let mut run_id: Option<String> = None;
        for r in rows.iter().rev() {
            match &r.event {
                Event::WorkerFinished {
                    run_id: rid,
                    outcome,
                    role,
                    ..
                } if !derive::is_reviewer(*role) && outcome.starts_with("question: ") => {
                    asked_at = Some(r.ts.clone());
                    run_id = Some(rid.clone());
                    break;
                }
                // ADR-0021 D2: ディスパッチャが出した質問（委譲した子が失敗し、やり直せなかった）。
                Event::QuestionRaised { run_id: rid, .. } => {
                    asked_at = Some(r.ts.clone());
                    run_id = Some(rid.clone());
                    break;
                }
                _ => {}
            }
        }

        let previous = derive::answers_from_events(&events);
        items.push(QuestionItem {
            task: view::task_ref(t),
            question,
            asked_at,
            run_id,
            previous,
            approval_id: pending_approval_by_task.get(&t.id).copied(),
        });
    }
    items.sort_by(|a, b| a.asked_at.cmp(&b.asked_at));
    Ok(items)
}

/// `status == Draft` のタスクを `parent_id` でまとめる。根（`parent_id == None`）は最後。
///
/// ADR-0074 D3.3（Phase F4a (b)）: 案件計画（マイルストーン DAG）の提案で作られた draft Task は
/// **案件直下**（`parent_id == None`）なので、通常の「親でまとめる」規則には乗らない。それらは
/// `is_root_task`（旧 `is_milestone_task`）かつ紐づく途中目標が `proposed`（=このタスク自身が手で作った・承認済みの
/// 途中目標ではなく、まだ人が決めていない提案の一部）で見分け、案件ごとに 1 つの `DraftGroup` に
/// まとめる（1 まとまり）。`plan_summary` は `Event::ProjectPlanProposed` の rationale と、
/// マイルストーンの DAG の 1 行ずつ。
fn build_drafts(
    store: &dyn TaskStore,
    all_tasks: &[Task],
    by_id: &HashMap<TaskId, Task>,
    ctx: &ViewContext,
    now: OffsetDateTime,
) -> Result<Vec<DraftGroup>, OpsError> {
    let all_drafts: Vec<&Task> = all_tasks
        .iter()
        .filter(|t| t.status == Status::Draft)
        .collect();

    // ADR-0074 D3.4（Phase F4b）: 案件ごとの未決の案件計画の提案（plan タスクの events が正本。
    // `project_plan::plan_state`）。案件計画 run を持つ案件だけを見る。
    let plan_projects: HashSet<task_core::ProjectId> = all_tasks
        .iter()
        .filter(|t| task_core::is_milestones_plan_task(t))
        .filter_map(|t| t.project_id)
        .collect();
    let mut pending_by_project: HashMap<task_core::ProjectId, crate::project_plan::PlanVersion> =
        HashMap::new();
    for project_id in plan_projects {
        if let Some(pending) = crate::project_plan::plan_state(store, project_id)?.pending() {
            pending_by_project.insert(project_id, pending.clone());
        }
    }
    let proposal_task_ids: HashSet<TaskId> = pending_by_project
        .values()
        .flat_map(|v| v.created().into_iter().map(|m| m.task_id))
        .collect();

    let mut project_plan_drafts: HashMap<task_core::ProjectId, Vec<Task>> = pending_by_project
        .keys()
        .map(|pid| (*pid, Vec::new()))
        .collect();
    let mut groups: HashMap<TaskId, Vec<Task>> = HashMap::new();
    let mut root_drafts: Vec<Task> = Vec::new();
    for t in all_drafts {
        let in_proposal = proposal_task_ids.contains(&t.id) && task_core::is_root_task(t);
        if let (true, Some(project_id)) = (in_proposal, t.project_id) {
            project_plan_drafts
                .entry(project_id)
                .or_default()
                .push(t.clone());
        } else if let Some(parent_id) = t.parent_id {
            groups.entry(parent_id).or_default().push(t.clone());
        } else {
            root_drafts.push(t.clone());
        }
    }

    let mut keys: Vec<TaskId> = groups.keys().copied().collect();
    keys.sort_by_key(|k| sort_key_for_draft_group(&Some(*k), by_id));
    // 子の件数は全件から 1 回だけ集計する（draft ごとに全件を読むと件数の二乗になる。Phase 9 監査）。
    let counts = view::child_counts(all_tasks);

    let mut out = Vec::with_capacity(keys.len() + project_plan_drafts.len() + 1);
    for key in keys {
        let mut members = groups.remove(&key).unwrap_or_default();
        members.sort_by_key(|t| t.id);

        let parent_task = by_id.get(&key);
        let parent = parent_task.map(view::task_ref);
        let plan_summary = match parent_task {
            Some(p) => {
                let rows = store.event_rows_for(p.id, None, view::ALL_EVENTS)?;
                rows.iter().rev().find_map(|r| match &r.event {
                    Event::WorkerFinished { outcome, role, .. } if !derive::is_reviewer(*role) => {
                        outcome.strip_prefix("done: ").map(str::to_string)
                    }
                    _ => None,
                })
            }
            None => None,
        };

        let drafts: Vec<TaskSummary> = members
            .iter()
            .map(|t| {
                let (children, pending) = counts.get(&t.id).copied().unwrap_or((0, 0));
                view::build_task_summary(t, children, pending, ctx, now)
            })
            .collect();

        out.push(DraftGroup {
            parent,
            plan_summary,
            drafts,
            project_plan: None,
        });
    }

    // ADR-0074 D3.3: 案件計画の提案。`parent` は無い（構造上どの Task の子でもない）が、根
    // （後述の `root_drafts`）よりは先に出す（案件の created_at 昇順で決定的に並べる）。
    let mut project_ids: Vec<task_core::ProjectId> = project_plan_drafts.keys().copied().collect();
    project_ids.sort_by_key(|pid| {
        by_id
            .values()
            .find(|t| t.project_id == Some(*pid))
            .map(|t| view::to_rfc3339(t.created_at))
            .unwrap_or_default()
    });
    for project_id in project_ids {
        let mut members = project_plan_drafts.remove(&project_id).unwrap_or_default();
        members.sort_by_key(|t| t.id);
        let Some(pending) = pending_by_project.get(&project_id) else {
            continue;
        };
        let plan_summary = Some(project_plan_summary(pending));
        let drafts: Vec<TaskSummary> = members
            .iter()
            .map(|t| {
                let (children, pending) = counts.get(&t.id).copied().unwrap_or((0, 0));
                view::build_task_summary(t, children, pending, ctx, now)
            })
            .collect();
        out.push(DraftGroup {
            parent: None,
            plan_summary,
            drafts,
            project_plan: Some(ProjectPlanRef {
                project_id,
                version: pending.version,
                supersedes: pending.supersedes,
            }),
        });
    }

    // 根（誰の子でもなく、案件計画の提案でもない draft）は最後。
    if !root_drafts.is_empty() {
        root_drafts.sort_by_key(|t| t.id);
        let drafts: Vec<TaskSummary> = root_drafts
            .iter()
            .map(|t| {
                let (children, pending) = counts.get(&t.id).copied().unwrap_or((0, 0));
                view::build_task_summary(t, children, pending, ctx, now)
            })
            .collect();
        out.push(DraftGroup {
            parent: None,
            plan_summary: None,
            drafts,
            project_plan: None,
        });
    }
    Ok(out)
}

/// ADR-0074 D3.3（Phase F4a (b)）/ D3.4（Phase F4b）: 未決の提案の rationale と、承認後の DAG の 1 行ずつ。
/// replan の差分なら、変える / 外す / 取り下げる key を添える（決定的。plan タスクの events が正本）。
fn project_plan_summary(pending: &crate::project_plan::PlanVersion) -> String {
    let plan = &pending.plan;
    let mut out = plan.rationale.clone();
    if let Some(base) = pending.supersedes {
        out.push_str(&format!("\n\n（version {base} の見直し）"));
    }
    out.push_str("\n\n");
    for m in &plan.milestones {
        let added = pending
            .delta
            .as_ref()
            .is_some_and(|d| d.add.iter().any(|a| a.key == m.key));
        let marker = if added { " [add]" } else { "" };
        if m.depends_on.is_empty() {
            out.push_str(&format!("- {}: {}{marker}\n", m.key, m.title));
        } else {
            out.push_str(&format!(
                "- {}: {}{marker} (depends on: {})\n",
                m.key,
                m.title,
                m.depends_on.join(", ")
            ));
        }
    }
    if let Some(delta) = &pending.delta {
        if !delta.modify.is_empty() {
            let keys: Vec<&str> = delta.modify.iter().map(|m| m.key.as_str()).collect();
            out.push_str(&format!("modify: {}\n", keys.join(", ")));
        }
        if !delta.remove.is_empty() {
            out.push_str(&format!("remove: {}\n", delta.remove.join(", ")));
        }
        if !delta.cancel.is_empty() {
            out.push_str(&format!("cancel: {}\n", delta.cancel.join(", ")));
        }
    }
    out.trim_end().to_string()
}

/// 親の `created_at` 昇順、根（`None`）は最後になるようなソートキー。
fn sort_key_for_draft_group(
    key: &Option<TaskId>,
    by_id: &HashMap<TaskId, Task>,
) -> (u8, String, String) {
    match key {
        Some(pid) => (
            0,
            by_id
                .get(pid)
                .map(|p| view::to_rfc3339(p.created_at))
                .unwrap_or_default(),
            pid.to_string(),
        ),
        None => (1, String::new(), String::new()),
    }
}

/// (a) 24h 以内に `failed`、(b) `ready` で連続 requeue が上限近く、(c) スナップショットの `unroutable`。
fn build_attention(
    store: &dyn TaskStore,
    all_tasks: &[Task],
    by_id: &HashMap<TaskId, Task>,
    snapshot: Option<&DaemonSnapshot>,
    ctx: &ViewContext,
    now: OffsetDateTime,
) -> Result<Vec<AttentionItem>, OpsError> {
    let mut items = Vec::new();
    let cutoff = now - time::Duration::hours(24);

    for t in all_tasks.iter().filter(|t| t.status == Status::Failed) {
        if t.updated_at < cutoff {
            continue;
        }
        let rows = store.event_rows_for(t.id, None, view::ALL_EVENTS)?;
        let events = view::seq_pairs(&rows);

        let mut reasons: Vec<String> = Vec::new();
        if let Some(outcome) = rows.iter().rev().find_map(|r| match &r.event {
            Event::WorkerFinished { outcome, role, .. } if !derive::is_reviewer(*role) => {
                Some(outcome.clone())
            }
            _ => None,
        }) {
            reasons.push(outcome);
        }
        if let Some(run_id) = derive::last_run_id(&events) {
            for r in &rows {
                if let Event::ReviewVerdict {
                    run_id: rid,
                    pass,
                    reason,
                    ..
                } = &r.event
                    && rid == &run_id
                    && !*pass
                {
                    reasons.push(reason.clone());
                }
            }
        }

        // ADR-0070 D1（Phase 116）: 分類・配送済みの release・「再レビュー」操作を足す。
        let (class, _) = derive::classify_task_failure(&events);
        let delivered_release = store.delivery_get(t.id)?.and_then(|d| d.release.clone());
        let integration_repair = view::integration_repair_view(&events);
        let mut task_ref = view::task_ref(t);
        task_ref.actions = view::actions_with_events(t, &events);

        items.push(AttentionItem::Failed {
            task: task_ref,
            reason: reasons.join("; "),
            at: view::to_rfc3339(t.updated_at),
            class,
            delivered_release,
            integration_repair,
        });
    }

    // ADR-0074 D2.4（Phase F3 途中確認）: 工程の後で止まった Task。期限は設けない（人の判断を待つ）。
    for t in all_tasks.iter().filter(|t| t.status == Status::Blocked) {
        let rows = store.event_rows_for(t.id, None, view::ALL_EVENTS)?;
        let events = view::seq_pairs(&rows);
        let Some(info) = crate::phase_gate::latest_phase_checkpoint(t, &events) else {
            continue;
        };
        let at = rows
            .iter()
            .find(|r| r.seq == info.transition_seq)
            .map(|r| r.ts.clone())
            .unwrap_or_else(|| view::to_rfc3339(t.updated_at));
        let phases_total = store
            .execution_plan_active(t.id)?
            .map(|p| p.spec.phases.len() as u32)
            .unwrap_or(0);
        let mut task_ref = view::task_ref(t);
        task_ref.actions = view::actions_with_events(t, &events);
        items.push(AttentionItem::PhaseCheckpoint {
            task: task_ref,
            phase: info.report.phase.clone(),
            phase_title: info.report.phase_title.clone(),
            phases_done: info.report.phases_done.len() as u32 + 1,
            phases_total,
            report_idx: info.report_idx,
            next_phase: info.report.next_phase.clone(),
            at,
        });
    }

    // ADR-0079 D8（Phase R3b）: root の計画の承認待ち。期限は設けない（人の判断を待つ）。
    for t in all_tasks.iter().filter(|t| t.status == Status::Blocked) {
        let rows = store.event_rows_for(t.id, None, view::ALL_EVENTS)?;
        let events = view::seq_pairs(&rows);
        let Some(info) = crate::plan_gate::latest_plan_approval(t, &events) else {
            continue;
        };
        let at = rows
            .iter()
            .find(|r| r.seq == info.transition_seq)
            .map(|r| r.ts.clone())
            .unwrap_or_else(|| view::to_rfc3339(t.updated_at));
        let plan = store
            .execution_plan_list(t.id)?
            .into_iter()
            .find(|p| p.id == info.plan_id);
        let stages = plan
            .as_ref()
            .map(|p| {
                p.spec
                    .stages
                    .iter()
                    .map(|s| PlanApprovalStage {
                        key: s.key.clone(),
                        title: s.title.clone(),
                        review_human: s.review == task_core::StageReview::Human,
                        units: p
                            .spec
                            .units
                            .iter()
                            .filter(|u| u.stage == s.key)
                            .map(|u| {
                                let kind = if u.kind == task_core::WorkUnitKind::Task {
                                    "子 task"
                                } else {
                                    "leaf"
                                };
                                format!("{}: {}（{kind}）", u.key, u.title)
                            })
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let root_id = task_core::tree::root_id_of(t);
        let decision_ids = store
            .decisions_list(Some(root_id))?
            .into_iter()
            .filter(|d| d.task_id == t.id && d.status == task_core::DecisionStatus::Open)
            .map(|d| d.id)
            .collect();
        let mut task_ref = view::task_ref(t);
        task_ref.actions = view::actions_with_events(t, &events);
        items.push(AttentionItem::PlanApproval {
            task: task_ref,
            plan_id: info.plan_id.clone(),
            plan_version: plan.as_ref().map(|p| p.version).unwrap_or(0),
            summary: crate::plan_gate::describe_reasons(&info.reasons),
            reasons: info.reasons,
            stages,
            decision_ids,
            at,
        });
    }

    // ADR-0121 D3: 完了した root で取り込みを見送ったもの。期限は設けない（成果が main に入っていない）。
    let latest_skipped = store.latest_delivery_skipped_rows()?;
    for row in latest_skipped {
        let Some(t) = all_tasks.iter().find(|t| {
            t.id == row.task_id
                && t.status == Status::Done
                && t.project_id.is_some()
                && !task_core::tree::is_tree_child(t)
                && task_core::support_kind(t).is_none()
        }) else {
            continue;
        };
        let Event::DeliverySkipped {
            reason,
            detail,
            head,
        } = row.event
        else {
            continue;
        };
        let delivered = store
            .delivery_get(t.id)?
            .is_some_and(|d| head.as_ref().is_none_or(|h| *h == d.head));
        if delivered {
            continue;
        }
        let rows = store.event_rows_for(t.id, None, view::ALL_EVENTS)?;
        let events = view::seq_pairs(&rows);
        let mut task_ref = view::task_ref(t);
        task_ref.actions = view::actions_with_events(t, &events);
        items.push(AttentionItem::DeliverySkipped {
            task: task_ref,
            reason,
            summary: format!(
                "完了しましたが main への取り込みを開始できませんでした: {}",
                reason.label()
            ),
            detail,
            head,
            at: row.ts,
        });
    }

    if ctx.max_requeues > 0 {
        for t in all_tasks.iter().filter(|t| t.status == Status::Ready) {
            let rows = store.event_rows_for(t.id, None, view::ALL_EVENTS)?;
            let events = view::seq_pairs(&rows);
            let count = derive::consecutive_requeues(&events);
            // 一度も requeue していないタスクは対象外（`max_requeues = 1` で全 ready が並ぶのを防ぐ。Phase 9 監査）。
            if count > 0 && count >= ctx.max_requeues - 1 {
                items.push(AttentionItem::RequeueLimitNear {
                    task: view::task_ref(t),
                    count,
                    max: ctx.max_requeues,
                    at: view::to_rfc3339(t.updated_at),
                });
            }
        }
    }

    if let Some(snap) = snapshot {
        for tid in &snap.unroutable {
            if let Some(t) = by_id.get(tid) {
                items.push(AttentionItem::Unroutable {
                    task: view::task_ref(t),
                    hint: t.worker_hint.clone(),
                    at: snap.last_tick_at.clone(),
                });
            }
        }
    }

    // (d) 直近 24 時間に `ClusterUnavailable` があったクラスタを 1 件ずつ出す（ADR-0018 D2）。
    // ワークスペースが `Remote` で**終端でない**タスクだけを対象にする（`Local` はクラスタと無関係。done / failed / cancelled の
    // タスクはもうクラスタを待っていないので、イベント列を読まない。監査 4-2: 走査を待っているタスクの数に抑える）。
    let mut cluster_agg: HashMap<String, (OffsetDateTime, String, String, HashSet<TaskId>)> =
        HashMap::new();
    for t in all_tasks.iter() {
        if t.status.is_terminal() || !matches!(t.workspace, WorkspaceSpec::Remote { .. }) {
            continue;
        }
        let rows = store.event_rows_for(t.id, None, view::ALL_EVENTS)?;
        for r in &rows {
            let Event::ClusterUnavailable {
                cluster: ev_cluster,
                host,
                ..
            } = &r.event
            else {
                continue;
            };
            let Ok(ts) =
                OffsetDateTime::parse(&r.ts, &time::format_description::well_known::Rfc3339)
            else {
                continue;
            };
            if ts < cutoff {
                continue;
            }
            let entry = cluster_agg
                .entry(ev_cluster.clone())
                .or_insert_with(|| (ts, r.ts.clone(), host.clone(), HashSet::new()));
            entry.3.insert(t.id);
            if ts > entry.0 {
                entry.0 = ts;
                entry.1 = r.ts.clone();
                entry.2 = host.clone();
            }
        }
    }

    let mut cluster_ids: Vec<String> = cluster_agg.keys().cloned().collect();
    cluster_ids.sort();
    for cluster_id in cluster_ids {
        // 接続が戻っている（人が再度ログインした）なら、この呼びかけはもう不要。
        if let Some(snap) = snapshot
            && snap
                .clusters
                .iter()
                .any(|c| c.id == cluster_id && c.connected)
        {
            continue;
        }
        let (_, latest_ts, latest_host, task_ids) = &cluster_agg[&cluster_id];
        let host = if !latest_host.is_empty() {
            latest_host.clone()
        } else {
            snapshot
                .and_then(|snap| snap.clusters.iter().find(|c| c.id == cluster_id))
                .map(|c| c.host.clone())
                .unwrap_or_default()
        };
        items.push(AttentionItem::ClusterUnavailable {
            cluster: cluster_id,
            host,
            at: latest_ts.clone(),
            tasks: task_ids.len() as u32,
        });
    }

    items.sort_by(|a, b| attention_at(b).cmp(attention_at(a)));
    Ok(items)
}

/// `docs/gui/api.md` §5.1。`evidence` は `(親タスク, run_id)` から `runs/<run_id>/result.json` の `evidence[]` を読む
/// 呼び出し側の関数（ファイル I/O は task-api が行う）。
pub fn inbox(
    store: &dyn TaskStore,
    snapshot: Option<&DaemonSnapshot>,
    ctx: &ViewContext,
    now: OffsetDateTime,
    evidence: &dyn Fn(&Task, &str) -> Vec<EvidenceView>,
) -> Result<Inbox, OpsError> {
    let all_tasks = store.list(None)?;
    let by_id: HashMap<TaskId, Task> = all_tasks.iter().map(|t| (t.id, t.clone())).collect();

    let approvals = build_approvals(store, &all_tasks, &by_id, evidence)?;
    let questions = build_questions(store, &all_tasks)?;
    let drafts = build_drafts(store, &all_tasks, &by_id, ctx, now)?;
    let mut suppressed = BTreeMap::new();
    let attention = build_attention(store, &all_tasks, &by_id, snapshot, ctx, now)?
        .into_iter()
        .filter(|item| {
            let rule = attention_task(item)
                .and_then(|id| by_id.get(&id))
                .and_then(|task| attention_suppression(task, &by_id));
            if let Some(rule) = rule {
                *suppressed.entry(rule.key().to_string()).or_insert(0) += 1;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>();
    let browser_waits = crate::browser::pending_items(store, &by_id)?;

    let by_status = store
        .count_by_status()?
        .into_iter()
        .map(|(s, n)| (view::status_key(s).to_string(), n))
        .collect();

    // ADR-0079 D7（Phase R3a）: 未回答の決定の要求（決定を出した節点が終端でないもの。古い順）。
    let decisions = crate::decision::inbox_items(store, now)?;

    let counts = InboxCounts {
        approvals: approvals.len() as u32,
        questions: questions.len() as u32,
        // グループ数ではなく draft タスクの件数（バッジ表示用。Phase 9 監査）。
        drafts: drafts.iter().map(|g| g.drafts.len() as u32).sum(),
        attention: attention.len() as u32,
        browser_waits: browser_waits.len() as u32,
        decisions: decisions.len() as u32,
        by_status,
    };

    Ok(Inbox {
        approvals,
        questions,
        drafts,
        attention,
        suppressed,
        browser_waits,
        decisions,
        counts,
    })
}

#[cfg(test)]
mod tests;
