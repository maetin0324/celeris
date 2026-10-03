//! ADR-0133 D1 / D2: 受信箱（人の判断が要るものだけ）の共通形。
//!
//! 既存の受信箱の組み立て（`inbox::inbox`）と KB の取り込み待ちの件数を読み、D1 の対応表で受信箱に割り当てた
//! 種類だけを D2 の `InboxItem`（何を決めるか・選択肢・推奨・期限・止めている範囲・答え方）へ写す。通知に
//! 割り当てた種類（`attention[type=requeue_limit_near]` 等）は出さない。
//!
//! 意味を失った項目を自動で閉じる規則（置き換え済み failed 子・終端 task の attention 等）は ADR-0131 D7 の
//! inbox-rules（`inbox::attention_suppression`）が `inbox::inbox` で適用する（ADR-0133 D4）。ここはその出力と
//! 規則別の除外件数（`Inbox::suppressed`）をそのまま使い、規則を持たない。
//! 受信箱は DB に行を持たない派生の一覧なので、答える操作が状態を書き換えた後の一覧には項目が構造的に現れない。

use std::collections::{BTreeMap, HashMap};

use schemars::JsonSchema;
use serde::Serialize;
use task_core::{Task, TaskId, TaskStore};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::daemon::DaemonSnapshot;
use crate::derive::FailureClass;
use crate::error::OpsError;
use crate::inbox::{AttentionItem, DraftGroup, EvidenceView, Inbox};
use crate::view::{self, TaskRef, ViewContext};

/// D2: 受信箱の種類。宣言順が D2 の並びの固定順。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    Decision,
    PlanGate,
    PhaseGate,
    Authorization,
    BrowserWait,
    Question,
    AcceptanceCheck,
    DraftAccept,
    ProjectPlan,
    Failed,
    Unroutable,
    ClusterLogin,
    DeliverySkipped,
    IntegrationRequest,
    KnowledgeReview,
}

impl InboxKind {
    pub const ALL: [InboxKind; 15] = [
        InboxKind::Decision,
        InboxKind::PlanGate,
        InboxKind::PhaseGate,
        InboxKind::Authorization,
        InboxKind::BrowserWait,
        InboxKind::Question,
        InboxKind::AcceptanceCheck,
        InboxKind::DraftAccept,
        InboxKind::ProjectPlan,
        InboxKind::Failed,
        InboxKind::Unroutable,
        InboxKind::ClusterLogin,
        InboxKind::DeliverySkipped,
        InboxKind::IntegrationRequest,
        InboxKind::KnowledgeReview,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            InboxKind::Decision => "decision",
            InboxKind::PlanGate => "plan_gate",
            InboxKind::PhaseGate => "phase_gate",
            InboxKind::Authorization => "authorization",
            InboxKind::BrowserWait => "browser_wait",
            InboxKind::Question => "question",
            InboxKind::AcceptanceCheck => "acceptance_check",
            InboxKind::DraftAccept => "draft_accept",
            InboxKind::ProjectPlan => "project_plan",
            InboxKind::Failed => "failed",
            InboxKind::Unroutable => "unroutable",
            InboxKind::ClusterLogin => "cluster_login",
            InboxKind::DeliverySkipped => "delivery_skipped",
            InboxKind::IntegrationRequest => "integration_request",
            InboxKind::KnowledgeReview => "knowledge_review",
        }
    }
}

/// D2 `options[]`: 選択肢 1 件。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct InboxOption {
    pub key: String,
    pub label: String,
    /// `true` なら note 必須（replan・質問への回答など）。
    pub needs_note: bool,
    /// 選んだら何が起きるかの 1 行。
    pub effect: String,
}

/// D2 `blocking`: 止めている範囲。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct InboxBlocking {
    pub tasks: Vec<TaskRef>,
    pub units: Vec<String>,
    pub root: Option<TaskRef>,
    pub summary: String,
}

/// D2 `answer.native`: 委ね先の既存 endpoint。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct InboxNativeOp {
    pub method: String,
    pub path: String,
}

/// D2 `answer`: 答え方の操作（新 API の answer と委ね先）。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct InboxAnswer {
    pub method: String,
    pub path: String,
    pub body_schema: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<InboxNativeOp>,
}

/// D2 `links[]`: 判断材料への API path。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct InboxLink {
    pub label: String,
    pub href: String,
}

/// ADR-0133 D2: 受信箱項目の共通形。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct InboxItem {
    /// 決定的な id `<kind>-<元の id>`（URL にそのまま使える文字だけ）。
    pub id: String,
    pub kind: InboxKind,
    /// 何を決めるか（1 行）。
    pub title: String,
    pub detail: Option<String>,
    pub options: Vec<InboxOption>,
    pub recommended: Option<String>,
    pub due_at: Option<String>,
    pub blocking: InboxBlocking,
    pub blocked_by: Vec<String>,
    pub answer: InboxAnswer,
    pub task: Option<TaskRef>,
    pub project_id: Option<String>,
    pub created_at: String,
    pub age_secs: u64,
    pub links: Vec<InboxLink>,
}

/// D5 `GET /inbox/items` の `counts`。
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct HumanInboxCounts {
    pub total: u32,
    pub by_kind: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct HumanInbox {
    pub items: Vec<InboxItem>,
    pub counts: HumanInboxCounts,
    /// ADR-0133 D4: inbox-rules が自動で閉じた attention の件数（規則名 → 件数）。`Inbox::suppressed` の写し。
    /// API では `HumanInboxView.suppressed` として出す。
    #[serde(skip)]
    #[schemars(skip)]
    pub suppressed: BTreeMap<String, u32>,
}

/// D1.2 `knowledge_review`: KB の取り込み待ち（`_inbox/` の候補）の件数。KB は task-api が読むので呼び出し側が渡す。
/// ADR-0131 D6 の日次整理 job が `enabled ∧ mode=apply` の間は呼び出し側が `None` を渡す（判断は `decision` で来る）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgePending {
    pub count: u32,
    /// 最も古い候補の作成時刻（RFC 3339）。無ければ `now`。
    pub oldest_created: Option<String>,
}

/// D1.1: attention の種類の割り当て。`None` は通知側（受信箱には出さない）。
pub fn route_attention(item: &AttentionItem) -> Option<InboxKind> {
    match item {
        AttentionItem::Failed { .. } => Some(InboxKind::Failed),
        // まだ止まっていない警告。通知の `requeue_limit_near`。
        AttentionItem::RequeueLimitNear { .. } => None,
        AttentionItem::Unroutable { .. } => Some(InboxKind::Unroutable),
        AttentionItem::ClusterUnavailable { .. } => Some(InboxKind::ClusterLogin),
        AttentionItem::PhaseCheckpoint { .. } => Some(InboxKind::PhaseGate),
        AttentionItem::PlanApproval { .. } => Some(InboxKind::PlanGate),
        AttentionItem::DeliverySkipped { .. } => Some(InboxKind::DeliverySkipped),
        AttentionItem::IntegrationRequest { .. } => Some(InboxKind::IntegrationRequest),
    }
}

/// `inbox::inbox` を組み立て、共通形へ写す（D5 `GET /inbox/items` の元）。
pub fn human_inbox(
    store: &dyn TaskStore,
    snapshot: Option<&DaemonSnapshot>,
    ctx: &ViewContext,
    now: OffsetDateTime,
    evidence: &dyn Fn(&Task, &str) -> Vec<EvidenceView>,
    knowledge: Option<&KnowledgePending>,
) -> Result<HumanInbox, OpsError> {
    let inbox = crate::inbox::inbox(store, snapshot, ctx, now, evidence)?;
    let by_id: HashMap<TaskId, Task> = store.list(None)?.into_iter().map(|t| (t.id, t)).collect();
    Ok(from_inbox(&inbox, knowledge, &by_id, now))
}

/// 既存の `Inbox` を D2 の共通形へ写す（純関数。`by_id` は案件 id と root の参照を引くためだけに使う）。
pub fn from_inbox(
    inbox: &Inbox,
    knowledge: Option<&KnowledgePending>,
    by_id: &HashMap<TaskId, Task>,
    now: OffsetDateTime,
) -> HumanInbox {
    let b = Builder { by_id, now };
    let mut items = Vec::new();

    for d in &inbox.decisions {
        items.push(b.decision(d));
    }
    for a in &inbox.approvals {
        items.push(b.acceptance_check(a));
    }
    for q in &inbox.questions {
        // D1.1: 認可で止まっている質問は `authorization` に吸収し、`question` として二重に出さない。
        items.push(match q.approval_id {
            Some(approval_id) => b.authorization(q, approval_id),
            None => b.question(q),
        });
    }
    for g in &inbox.drafts {
        if let Some(item) = b.draft_group(g) {
            items.push(item);
        }
    }
    for a in &inbox.attention {
        if route_attention(a).is_some()
            && let Some(item) = b.attention(a, &inbox.decisions)
        {
            items.push(item);
        }
    }
    for w in &inbox.browser_waits {
        items.push(b.browser_wait(w));
    }
    if let Some(k) = knowledge.filter(|k| k.count > 0) {
        items.push(b.knowledge_review(k));
    }

    sort_items(&mut items);
    let mut by_kind: BTreeMap<String, u32> = BTreeMap::new();
    for it in &items {
        *by_kind.entry(it.kind.as_str().to_string()).or_default() += 1;
    }
    HumanInbox {
        counts: HumanInboxCounts {
            total: items.len() as u32,
            by_kind,
        },
        items,
        suppressed: inbox.suppressed.clone(),
    }
}

/// D2 の並び: `due_at` の近い順（無いものは後）→ kind の固定順 → `created_at` の古い順 → id。
fn sort_items(items: &mut [InboxItem]) {
    items.sort_by(|a, b| {
        let due = match (&a.due_at, &b.due_at) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        due.then(a.kind.cmp(&b.kind))
            .then(a.created_at.cmp(&b.created_at))
            .then(a.id.cmp(&b.id))
    });
}

/// id に使えない文字を `-` にする（`[A-Za-z0-9_.-]` だけを残す）。
fn id_part(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// 時刻の数字だけ（`2026-10-02T13:10:00Z` → `20261002131000`）。同じ task の別の出来事を id で分ける。
fn ts_part(ts: &str) -> String {
    ts.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// D2 `detail`: 600 字で切る。空は `None`。
fn clip(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    const MAX: usize = 600;
    if s.chars().count() <= MAX {
        Some(s.to_string())
    } else {
        Some(format!("{}…", s.chars().take(MAX).collect::<String>()))
    }
}

fn opt(key: &str, label: &str, needs_note: bool, effect: &str) -> InboxOption {
    InboxOption {
        key: key.to_string(),
        label: label.to_string(),
        needs_note,
        effect: effect.to_string(),
    }
}

fn native(method: &str, path: String) -> Option<InboxNativeOp> {
    Some(InboxNativeOp {
        method: method.to_string(),
        path,
    })
}

fn link(label: &str, href: String) -> InboxLink {
    InboxLink {
        label: label.to_string(),
        href,
    }
}

struct Builder<'a> {
    by_id: &'a HashMap<TaskId, Task>,
    now: OffsetDateTime,
}

/// 共通の欄を埋める前の、kind ごとの中身。
struct Draft {
    id: String,
    kind: InboxKind,
    title: String,
    detail: Option<String>,
    options: Vec<InboxOption>,
    recommended: Option<String>,
    due_at: Option<String>,
    blocking: InboxBlocking,
    blocked_by: Vec<String>,
    native: Option<InboxNativeOp>,
    task: Option<TaskRef>,
    project_id: Option<String>,
    created_at: String,
    links: Vec<InboxLink>,
}

impl Builder<'_> {
    fn finish(&self, d: Draft) -> InboxItem {
        let age_secs = OffsetDateTime::parse(&d.created_at, &Rfc3339)
            .map(|t| u64::try_from((self.now - t).whole_seconds()).unwrap_or(0))
            .unwrap_or(0);
        let mut body_schema = BTreeMap::new();
        body_schema.insert("option".to_string(), "string (options[].key)".to_string());
        body_schema.insert(
            "note".to_string(),
            "string? (needs_note なら必須)".to_string(),
        );
        InboxItem {
            answer: InboxAnswer {
                method: "POST".to_string(),
                path: format!("/api/v1/inbox/items/{}/answer", d.id),
                body_schema,
                native: d.native,
            },
            id: d.id,
            kind: d.kind,
            title: d.title,
            detail: d.detail,
            options: d.options,
            recommended: d.recommended,
            due_at: d.due_at,
            blocking: d.blocking,
            blocked_by: d.blocked_by,
            task: d.task,
            project_id: d.project_id,
            created_at: d.created_at,
            age_secs,
            links: d.links,
        }
    }

    fn project_of(&self, id: TaskId) -> Option<String> {
        self.by_id
            .get(&id)
            .and_then(|t| t.project_id)
            .map(|p| p.to_string())
    }

    fn root_of(&self, id: TaskId) -> Option<TaskRef> {
        let t = self.by_id.get(&id)?;
        let root_id = task_core::tree::root_id_of(t);
        self.by_id.get(&root_id).map(view::task_ref)
    }

    fn ref_of(&self, id: TaskId) -> Option<TaskRef> {
        self.by_id.get(&id).map(view::task_ref)
    }

    /// その task だけを止めている。
    fn only_task(&self, task: &TaskRef) -> InboxBlocking {
        InboxBlocking {
            tasks: vec![task.clone()],
            units: Vec::new(),
            root: self.root_of(task.id).filter(|r| r.id != task.id),
            summary: "この task だけ".to_string(),
        }
    }

    fn decision(&self, d: &crate::decision::DecisionInboxItem) -> InboxItem {
        let task = self.ref_of(d.task_id);
        let units: Vec<String> = d
            .needed_before
            .iter()
            .filter(|u| u.as_str() != "self")
            .cloned()
            .collect();
        let summary = if units.is_empty() {
            "決定を出した節点が待っている".to_string()
        } else {
            format!("unit {} が待っている", units.join(", "))
        };
        // D1.3: 未回答の決定の再通知は 24 時間後（D6 の受信箱経路が引き継ぐ）。
        let due_at = OffsetDateTime::parse(&d.created_at, &Rfc3339)
            .ok()
            .and_then(|t| (t + time::Duration::hours(24)).format(&Rfc3339).ok());
        self.finish(Draft {
            id: format!("decision-{}", id_part(&d.id)),
            kind: InboxKind::Decision,
            title: format!("決定: {}", d.question.trim()),
            detail: clip(
                &d.path
                    .iter()
                    .map(|p| p.title.as_str())
                    .collect::<Vec<_>>()
                    .join(" > "),
            ),
            options: d
                .options
                .iter()
                .map(|o| InboxOption {
                    key: o.key.clone(),
                    label: o.label.clone(),
                    needs_note: false,
                    effect: o.consequence.clone().unwrap_or_default(),
                })
                .collect(),
            recommended: Some(d.recommended.clone()).filter(|r| !r.is_empty()),
            due_at,
            blocking: InboxBlocking {
                tasks: task.iter().cloned().collect(),
                units,
                root: self.ref_of(d.root_id),
                summary,
            },
            blocked_by: Vec::new(),
            native: native("POST", format!("/api/v1/decisions/{}/answer", d.id)),
            project_id: self.project_of(d.task_id),
            task,
            created_at: d.created_at.clone(),
            links: vec![link("計画", format!("/api/v1/tasks/{}/tree", d.root_id))],
        })
    }

    fn acceptance_check(&self, a: &crate::inbox::ApprovalItem) -> InboxItem {
        let subject = a.parent.clone().unwrap_or_else(|| a.approval.clone());
        let mut links = Vec::new();
        if let Some(p) = &a.parent {
            for art in &a.artifacts {
                links.push(link(
                    "成果物",
                    format!("/api/v1/tasks/{}/artifacts/{}", p.id, art.idx),
                ));
            }
        }
        self.finish(Draft {
            id: format!("acceptance_check-{}", a.approval.id),
            kind: InboxKind::AcceptanceCheck,
            title: format!("受け入れ確認: {}", subject.title),
            detail: clip(&a.criterion_text),
            options: vec![
                opt(
                    "approve",
                    "合格にする",
                    false,
                    &format!("POST /tasks/{}/approve", a.approval.id),
                ),
                opt(
                    "reject",
                    "不合格にする",
                    true,
                    &format!("POST /tasks/{}/reject", a.approval.id),
                ),
            ],
            recommended: None,
            due_at: None,
            blocking: self.only_task(&subject),
            blocked_by: Vec::new(),
            native: native("POST", format!("/api/v1/tasks/{}/approve", a.approval.id)),
            project_id: self.project_of(subject.id),
            task: Some(subject),
            created_at: a.requested_at.clone(),
            links,
        })
    }

    fn question(&self, q: &crate::inbox::QuestionItem) -> InboxItem {
        let created_at = q
            .asked_at
            .clone()
            .unwrap_or_else(|| self.updated_at(q.task.id));
        self.finish(Draft {
            id: format!("question-{}", q.task.id),
            kind: InboxKind::Question,
            title: format!("質問: {}", q.task.title),
            detail: clip(&q.question),
            options: vec![opt(
                "answer",
                "答える（note に回答を書く）",
                true,
                &format!("POST /tasks/{}/answer", q.task.id),
            )],
            recommended: None,
            due_at: None,
            blocking: self.only_task(&q.task),
            blocked_by: Vec::new(),
            native: native("POST", format!("/api/v1/tasks/{}/answer", q.task.id)),
            project_id: self.project_of(q.task.id),
            task: Some(q.task.clone()),
            created_at,
            links: Vec::new(),
        })
    }

    fn authorization(
        &self,
        q: &crate::inbox::QuestionItem,
        approval_id: task_core::approval::ApprovalId,
    ) -> InboxItem {
        let created_at = q
            .asked_at
            .clone()
            .unwrap_or_else(|| self.updated_at(q.task.id));
        let path = format!("/api/v1/approvals/{approval_id}/decide");
        self.finish(Draft {
            id: format!("authorization-{approval_id}"),
            kind: InboxKind::Authorization,
            title: format!("認可: {}", q.task.title),
            detail: clip(&q.question),
            options: vec![
                opt(
                    "once",
                    "今回だけ認める",
                    false,
                    &format!("POST {path} once"),
                ),
                opt(
                    "standing",
                    "今後ずっと認める",
                    false,
                    &format!("POST {path} standing（standing_rules に 1 行増える）"),
                ),
                opt("denied", "認めない", false, &format!("POST {path} denied")),
            ],
            recommended: None,
            due_at: None,
            blocking: self.only_task(&q.task),
            blocked_by: Vec::new(),
            native: native("POST", path),
            project_id: self.project_of(q.task.id),
            task: Some(q.task.clone()),
            created_at,
            links: Vec::new(),
        })
    }

    fn draft_group(&self, g: &DraftGroup) -> Option<InboxItem> {
        if let Some(pp) = &g.project_plan {
            let path = format!(
                "/api/v1/projects/{}/project-plan/{}/decide",
                pp.project_id, pp.version
            );
            let created_at = g
                .drafts
                .iter()
                .map(|d| d.created_at.clone())
                .min()
                .unwrap_or_else(|| self.now_str());
            return Some(self.finish(Draft {
                id: format!("project_plan-{}-v{}", pp.project_id, pp.version),
                kind: InboxKind::ProjectPlan,
                title: format!("案件計画の承認: version {}", pp.version),
                detail: g.plan_summary.as_deref().and_then(clip),
                options: vec![
                    opt(
                        "approve",
                        "承認する",
                        false,
                        &format!("POST {path} approve"),
                    ),
                    opt("reject", "却下する", true, &format!("POST {path} reject")),
                ],
                recommended: None,
                due_at: None,
                blocking: InboxBlocking {
                    tasks: Vec::new(),
                    units: Vec::new(),
                    root: None,
                    summary: format!("案件 {} の途中目標", pp.project_id),
                },
                blocked_by: Vec::new(),
                native: native("POST", path),
                task: None,
                project_id: Some(pp.project_id.to_string()),
                created_at,
                links: Vec::new(),
            }));
        }
        let first = g.drafts.first()?;
        let (id_key, subject, title) = match &g.parent {
            Some(p) => (
                p.id.to_string(),
                p.clone(),
                format!("下書きの受け入れ: {}（{} 件）", p.title, g.drafts.len()),
            ),
            None => (
                "root".to_string(),
                self.ref_of(first.id)?,
                format!("下書きの受け入れ: 案件直下（{} 件）", g.drafts.len()),
            ),
        };
        let tasks: Vec<TaskRef> = g.drafts.iter().filter_map(|d| self.ref_of(d.id)).collect();
        let created_at = g
            .drafts
            .iter()
            .map(|d| d.created_at.clone())
            .min()
            .unwrap_or_else(|| self.now_str());
        let titles: Vec<&str> = g.drafts.iter().map(|d| d.title.as_str()).collect();
        Some(
            self.finish(Draft {
                id: format!("draft_accept-{id_key}"),
                kind: InboxKind::DraftAccept,
                title,
                detail: clip(&match &g.plan_summary {
                    Some(s) => format!("{s}\n\n- {}", titles.join("\n- ")),
                    None => format!("- {}", titles.join("\n- ")),
                }),
                options: vec![
                    opt(
                        "accept",
                        "受け入れて始める",
                        false,
                        "POST /tasks/{id}/accept（下書きを ready にする）",
                    ),
                    opt("cancel", "取り消す", false, "POST /tasks/{id}/cancel"),
                ],
                recommended: None,
                due_at: None,
                blocking: InboxBlocking {
                    summary: format!("下書き {} 件", tasks.len()),
                    tasks,
                    units: Vec::new(),
                    root: g.parent.clone(),
                },
                blocked_by: Vec::new(),
                native: (g.drafts.len() == 1)
                    .then(|| native("POST", format!("/api/v1/tasks/{}/accept", first.id)))
                    .flatten(),
                project_id: self.project_of(subject.id),
                task: Some(subject),
                created_at,
                links: Vec::new(),
            }),
        )
    }

    fn attention(
        &self,
        a: &AttentionItem,
        decisions: &[crate::decision::DecisionInboxItem],
    ) -> Option<InboxItem> {
        Some(match a {
            AttentionItem::Failed {
                task,
                reason,
                at,
                class,
                ..
            } => self.finish(Draft {
                id: format!("failed-{}-{}", task.id, ts_part(at)),
                kind: InboxKind::Failed,
                title: format!("失敗: {}（{}）", task.title, class.as_str()),
                detail: clip(reason),
                options: vec![
                    opt(
                        "retry",
                        "やり直す（複製）",
                        false,
                        &format!("POST /tasks/{}/retry", task.id),
                    ),
                    opt(
                        "reopen",
                        "同じ task を開き直す",
                        false,
                        &format!("POST /tasks/{}/reopen", task.id),
                    ),
                    opt(
                        "cancel",
                        "諦める",
                        false,
                        "failed → cancelled（ADR-0131 D7）",
                    ),
                ],
                recommended: (*class == FailureClass::Infra).then(|| "retry".to_string()),
                due_at: None,
                blocking: self.only_task(task),
                blocked_by: Vec::new(),
                native: native("POST", format!("/api/v1/tasks/{}/retry", task.id)),
                project_id: self.project_of(task.id),
                task: Some(task.clone()),
                created_at: at.clone(),
                links: Vec::new(),
            }),
            AttentionItem::RequeueLimitNear { .. } => return None,
            AttentionItem::Unroutable { task, hint, at } => self.finish(Draft {
                id: format!("unroutable-{}", task.id),
                kind: InboxKind::Unroutable,
                title: format!("担当が決まらない: {}", task.title),
                detail: clip(&format!(
                    "tier={:?} adapter={}",
                    hint.tier,
                    hint.adapter.as_deref().unwrap_or("-")
                )),
                options: vec![
                    opt(
                        "reassign",
                        "担当の指定を変える（note に hint を書く）",
                        true,
                        &format!("PATCH /tasks/{}", task.id),
                    ),
                    opt(
                        "cancel",
                        "取り消す",
                        false,
                        &format!("POST /tasks/{}/cancel", task.id),
                    ),
                ],
                recommended: None,
                due_at: None,
                blocking: self.only_task(task),
                blocked_by: Vec::new(),
                native: native("PATCH", format!("/api/v1/tasks/{}", task.id)),
                project_id: self.project_of(task.id),
                task: Some(task.clone()),
                created_at: at.clone(),
                links: Vec::new(),
            }),
            AttentionItem::ClusterUnavailable {
                cluster,
                host,
                at,
                tasks,
            } => self.finish(Draft {
                id: format!("cluster_login-{}", id_part(cluster)),
                kind: InboxKind::ClusterLogin,
                title: format!("クラスタ {cluster} に再ログインする"),
                detail: clip(&format!(
                    "{host} への接続が切れている。クラスタ接続画面で TOTP を入力する"
                )),
                options: vec![opt(
                    "logged_in",
                    "ログインした",
                    false,
                    "接続の回復で消える",
                )],
                recommended: Some("logged_in".to_string()),
                due_at: None,
                blocking: InboxBlocking {
                    tasks: Vec::new(),
                    units: Vec::new(),
                    root: None,
                    summary: format!("クラスタ {cluster} を使う task {tasks} 件"),
                },
                blocked_by: Vec::new(),
                native: None,
                task: None,
                project_id: None,
                created_at: at.clone(),
                links: Vec::new(),
            }),
            AttentionItem::PhaseCheckpoint {
                task,
                phase,
                phase_title,
                phases_done,
                phases_total,
                report_idx,
                next_phase,
                at,
            } => {
                let path = format!("/api/v1/tasks/{}/execution/phase-gate", task.id);
                let next = next_phase
                    .as_deref()
                    .map(|n| format!("。次は {n}"))
                    .unwrap_or_default();
                self.finish(Draft {
                    id: format!("phase_gate-{}-{}", task.id, id_part(phase)),
                    kind: InboxKind::PhaseGate,
                    title: format!("途中確認: {}（{phase_title}）", task.title),
                    detail: clip(&format!("工程 {phases_done}/{phases_total} が済んだ{next}")),
                    options: vec![
                        opt(
                            "continue",
                            "次の工程へ進める",
                            false,
                            &format!("POST {path} continue"),
                        ),
                        opt(
                            "replan",
                            "計画を直す（note に指示）",
                            true,
                            &format!("POST {path} replan"),
                        ),
                        opt(
                            "withdraw",
                            "取り下げる",
                            false,
                            &format!("POST {path} withdraw"),
                        ),
                    ],
                    recommended: None,
                    due_at: None,
                    blocking: self.only_task(task),
                    blocked_by: Vec::new(),
                    native: native("POST", path),
                    project_id: self.project_of(task.id),
                    task: Some(task.clone()),
                    created_at: at.clone(),
                    links: report_idx
                        .map(|i| {
                            vec![link(
                                "途中報告",
                                format!("/api/v1/tasks/{}/artifacts/{i}", task.id),
                            )]
                        })
                        .unwrap_or_default(),
                })
            }
            AttentionItem::PlanApproval {
                task,
                plan_version,
                summary,
                stages,
                decision_ids,
                at,
                ..
            } => {
                let path = format!("/api/v1/tasks/{}/execution/plan-gate", task.id);
                let detail = std::iter::once(summary.clone())
                    .chain(
                        stages
                            .iter()
                            .map(|s| format!("{}: {}（{} 件）", s.key, s.title, s.units.len())),
                    )
                    .collect::<Vec<_>>()
                    .join("\n");
                // D1.1: 計画の決定の要求は別の `decision` 項目として並び、この項目の `blocked_by` に載る。
                let blocked_by = decision_ids
                    .iter()
                    .filter(|id| decisions.iter().any(|d| &d.id == *id))
                    .map(|id| format!("decision-{}", id_part(id)))
                    .collect();
                self.finish(Draft {
                    id: format!("plan_gate-{}-v{plan_version}", task.id),
                    kind: InboxKind::PlanGate,
                    title: format!("計画の承認: {}（version {plan_version}）", task.title),
                    detail: clip(&detail),
                    options: vec![
                        opt(
                            "approve",
                            "計画のとおり進める",
                            false,
                            &format!("POST {path} approve"),
                        ),
                        opt(
                            "replan",
                            "計画を書き直させる（note に指示）",
                            true,
                            &format!("POST {path} replan"),
                        ),
                        opt(
                            "withdraw",
                            "取り下げる",
                            false,
                            &format!("POST {path} withdraw"),
                        ),
                    ],
                    recommended: Some("approve".to_string()),
                    due_at: None,
                    blocking: InboxBlocking {
                        tasks: vec![task.clone()],
                        units: Vec::new(),
                        root: Some(task.clone()),
                        summary: "root の木全体".to_string(),
                    },
                    blocked_by,
                    native: native("POST", path),
                    project_id: self.project_of(task.id),
                    task: Some(task.clone()),
                    created_at: at.clone(),
                    links: vec![link("計画", format!("/api/v1/tasks/{}/tree", task.id))],
                })
            }
            AttentionItem::DeliverySkipped {
                task,
                summary,
                detail,
                at,
                ..
            } => self.finish(Draft {
                id: format!("delivery_skipped-{}-{}", task.id, ts_part(at)),
                kind: InboxKind::DeliverySkipped,
                title: format!("配送の見送り: {}", task.title),
                detail: clip(&format!("{summary}\n{detail}")),
                options: vec![
                    opt(
                        "assign",
                        "担当を付けて配送し直す（note に担当）",
                        true,
                        &format!("PATCH /tasks/{}（assignee）", task.id),
                    ),
                    opt("skip", "配送しない", false, "main へ取り込まない"),
                ],
                recommended: None,
                due_at: None,
                blocking: self.only_task(task),
                blocked_by: Vec::new(),
                native: native("PATCH", format!("/api/v1/tasks/{}", task.id)),
                project_id: self.project_of(task.id),
                task: Some(task.clone()),
                created_at: at.clone(),
                links: Vec::new(),
            }),
            AttentionItem::IntegrationRequest {
                task,
                request_id,
                request,
                at,
            } => {
                let mut detail = request.to_markdown();
                if !request.conflict_files.is_empty() {
                    detail.push_str("\n## 衝突 file\n");
                    for path in &request.conflict_files {
                        detail.push_str(&format!("- `{path}`\n"));
                    }
                }
                if let Some(sha) = &request.candidate_sha {
                    detail.push_str(&format!("\n- 候補 SHA: `{sha}`\n"));
                }
                self.finish(Draft {
                    id: format!("integration_request-{}", id_part(request_id)),
                    kind: InboxKind::IntegrationRequest,
                    title: format!("統合の依頼: {}", task.title),
                    detail: Some(detail),
                    options: vec![
                        opt(
                            "integrated",
                            "統合した",
                            false,
                            "統合済みとして回答を記録する",
                        ),
                        opt("declined", "見送る", false, "見送りとして回答を記録する"),
                        opt("retry", "再試行する", false, "再試行として回答を記録する"),
                    ],
                    recommended: None,
                    due_at: None,
                    blocking: self.only_task(task),
                    blocked_by: Vec::new(),
                    native: None,
                    project_id: self.project_of(task.id),
                    task: Some(task.clone()),
                    created_at: at.clone(),
                    links: Vec::new(),
                })
            }
        })
    }

    fn browser_wait(&self, w: &crate::browser::BrowserWaitItem) -> InboxItem {
        let base = format!(
            "/api/v1/tasks/{}/browser/waits/{}",
            w.task.id, w.wait.wait_id
        );
        let (title, options, native_path) = match w.wait.reason {
            task_core::browser_wait::BrowserWaitReason::WaitingForAuth => (
                format!("browser の資格情報: {}", w.wait.origin),
                vec![
                    opt(
                        "registered",
                        "登録した",
                        false,
                        &format!(
                            "POST {base}/registered（資格情報は {base}/credential で登録する）"
                        ),
                    ),
                    opt(
                        "deny",
                        "認めない",
                        false,
                        &format!("POST {base}/decision deny"),
                    ),
                ],
                format!("{base}/credential"),
            ),
            task_core::browser_wait::BrowserWaitReason::WaitingForApproval => (
                format!("browser の操作の承認: {}", w.wait.origin),
                vec![
                    opt(
                        "approve",
                        "一回だけ認める",
                        false,
                        &format!("POST {base}/decision approve"),
                    ),
                    opt(
                        "deny",
                        "認めない",
                        false,
                        &format!("POST {base}/decision deny"),
                    ),
                ],
                format!("{base}/decision"),
            ),
        };
        self.finish(Draft {
            id: format!("browser_wait-{}", id_part(&w.wait.wait_id)),
            kind: InboxKind::BrowserWait,
            title,
            detail: clip(&w.wait.purpose),
            options,
            recommended: None,
            due_at: w.wait.deadline.format(&Rfc3339).ok(),
            blocking: InboxBlocking {
                units: w.wait.work_unit_id.iter().cloned().collect(),
                ..self.only_task(&w.task)
            },
            blocked_by: Vec::new(),
            native: native("POST", native_path),
            project_id: self.project_of(w.task.id),
            task: Some(w.task.clone()),
            created_at: w
                .wait
                .created_at
                .format(&Rfc3339)
                .unwrap_or_else(|_| self.now_str()),
            links: Vec::new(),
        })
    }

    fn knowledge_review(&self, k: &KnowledgePending) -> InboxItem {
        self.finish(Draft {
            id: "knowledge_review".to_string(),
            kind: InboxKind::KnowledgeReview,
            title: format!("KB の取り込み待ち {} 件を確かめる", k.count),
            detail: None,
            options: vec![
                opt(
                    "accept",
                    "取り込む（KB 画面で 1 件ずつ）",
                    false,
                    "POST /knowledge/inbox/{id}/accept",
                ),
                opt(
                    "reject",
                    "捨てる（KB 画面で 1 件ずつ）",
                    false,
                    "POST /knowledge/inbox/{id}/reject",
                ),
            ],
            recommended: None,
            due_at: None,
            blocking: InboxBlocking {
                tasks: Vec::new(),
                units: Vec::new(),
                root: None,
                summary: "KB の候補（0 件で消える）".to_string(),
            },
            blocked_by: Vec::new(),
            native: None,
            task: None,
            project_id: None,
            created_at: k.oldest_created.clone().unwrap_or_else(|| self.now_str()),
            links: vec![link("候補", "/api/v1/knowledge/inbox".to_string())],
        })
    }

    fn updated_at(&self, id: TaskId) -> String {
        self.by_id
            .get(&id)
            .map(|t| view::to_rfc3339(t.updated_at))
            .unwrap_or_else(|| self.now_str())
    }

    fn now_str(&self) -> String {
        view::to_rfc3339(self.now)
    }
}

#[cfg(test)]
mod tests;
