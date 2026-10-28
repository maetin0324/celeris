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
    // ADR 2026-10-15-mlsys-min-decision-assisted D4: 外部 provider の API キーの登録待ち（秘密 id ごとに 1 件）。
    CredentialWait,
    // ADR 2026-10-07-build-tmp-hygiene D4.3: ディスク使用率が `critical` を超えた path。
    DiskFull,
    DeliverySkipped,
    IntegrationRequest,
    Unintegrated,
    // ADR 2026-10-23-stage-order-and-plan-stall D5: 計画を持つ task が理由なく止まっている（生存確認の `StallDetected`）。
    Stalled,
    // ADR 2026-10-27-project-stagnation-inbox D1: 案件単位の停滞（決定的な判定。受信箱では答えない）。
    ProjectStalled,
    HumanWaitAging,
    ReleaseUnpromoted,
    TaskUnscheduled,
    // ADR 2026-10-27-review-resync-halt-visible-and-reviewer-reuse D3: review の再同期が上限で止まっている task。
    ReviewResyncHalted,
    // 人の決定 2026-10-08: KB の取り込み待ち（旧 `knowledge_review`）は受信箱に出さない。知識画面で扱う。
}

impl InboxKind {
    pub const ALL: [InboxKind; 23] = [
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
        InboxKind::CredentialWait,
        InboxKind::DiskFull,
        InboxKind::DeliverySkipped,
        InboxKind::IntegrationRequest,
        InboxKind::Unintegrated,
        InboxKind::Stalled,
        InboxKind::ProjectStalled,
        InboxKind::HumanWaitAging,
        InboxKind::ReleaseUnpromoted,
        InboxKind::TaskUnscheduled,
        InboxKind::ReviewResyncHalted,
    ];

    /// ADR 2026-10-27-project-stagnation-inbox D1: 人の操作を待って止まっている種類（`human_wait_aging` の対象）。
    pub const HUMAN_WAITS: [InboxKind; 12] = [
        InboxKind::Decision,
        InboxKind::PlanGate,
        InboxKind::PhaseGate,
        InboxKind::Authorization,
        InboxKind::BrowserWait,
        InboxKind::Question,
        InboxKind::AcceptanceCheck,
        InboxKind::DraftAccept,
        InboxKind::ProjectPlan,
        InboxKind::CredentialWait,
        InboxKind::ClusterLogin,
        InboxKind::IntegrationRequest,
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
            InboxKind::CredentialWait => "credential_wait",
            InboxKind::DiskFull => "disk_full",
            InboxKind::DeliverySkipped => "delivery_skipped",
            InboxKind::IntegrationRequest => "integration_request",
            InboxKind::Unintegrated => "unintegrated",
            InboxKind::Stalled => "stalled",
            InboxKind::ProjectStalled => "project_stalled",
            InboxKind::HumanWaitAging => "human_wait_aging",
            InboxKind::ReleaseUnpromoted => "release_unpromoted",
            InboxKind::TaskUnscheduled => "task_unscheduled",
            InboxKind::ReviewResyncHalted => "review_resync_halted",
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
    /// 判断材料の本文。長い本文は `DETAIL_CLIP_CHARS` 字で切り、末尾を「…」にする。切ったときは
    /// `links` に全文の在りか（「全文」）を足す（ADR 2026-10-28-cos-inbox-chat-summary D1）。
    pub detail: Option<String>,
    /// 切る前の本文（process 内の利用者だけ。API には出さない）。CoS 不在の退避と一次対応の取り込みが、
    /// 切れた `detail` ではなくこれを使う（ADR 2026-10-28-cos-inbox-chat-summary D1）。
    #[serde(skip)]
    #[schemars(skip)]
    pub detail_full: Option<String>,
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
        AttentionItem::Unintegrated { .. } => Some(InboxKind::Unintegrated),
        AttentionItem::Stalled { .. } => Some(InboxKind::Stalled),
        AttentionItem::ReviewResyncHalted { .. } => Some(InboxKind::ReviewResyncHalted),
        // ADR 2026-10-08-browser-prod-enablement D2: browser の前提で止まった task。人の手は task 詳細の
        // browser 節（policy の編集）か release・台帳の作り直しなので、browser の人手の種類に入れる。
        AttentionItem::BrowserPrerequisite { .. } => Some(InboxKind::BrowserWait),
    }
}

/// `inbox::inbox` を組み立て、共通形へ写す（D5 `GET /inbox/items` の元）。
pub fn human_inbox(
    store: &dyn TaskStore,
    snapshot: Option<&DaemonSnapshot>,
    ctx: &ViewContext,
    now: OffsetDateTime,
    evidence: &dyn Fn(&Task, &str) -> Vec<EvidenceView>,
) -> Result<HumanInbox, OpsError> {
    let inbox = crate::inbox::inbox(store, snapshot, ctx, now, evidence)?;
    let by_id: HashMap<TaskId, Task> = store.list(None)?.into_iter().map(|t| (t.id, t)).collect();
    Ok(from_inbox(&inbox, &by_id, now))
}

/// 既存の `Inbox` を D2 の共通形へ写す（純関数。`by_id` は案件 id と root の参照を引くためだけに使う）。
pub fn from_inbox(inbox: &Inbox, by_id: &HashMap<TaskId, Task>, now: OffsetDateTime) -> HumanInbox {
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
    for d in &inbox.disk_full {
        items.push(b.disk_full(d));
    }
    // 同じ秘密を待つ wait は 1 件に束ねる（集約キー = `credential:<secret_id>`）。
    let mut groups: BTreeMap<&str, Vec<&task_core::CredentialWait>> = BTreeMap::new();
    for w in &inbox.credential_waits {
        groups
            .entry(w.aggregation_key.as_str())
            .or_default()
            .push(w);
    }
    for waits in groups.values() {
        items.push(b.credential_wait(waits));
    }
    // ADR 2026-10-27-project-stagnation-inbox D1・D2: 人の操作待ちの長期化は、ここまでの項目の経過から案件ごとに 1 件。
    if let Some(policy) = inbox.stagnation_policy {
        let waits: Vec<crate::stagnation::HumanWait> = items
            .iter()
            .filter(|it| InboxKind::HUMAN_WAITS.contains(&it.kind))
            .map(|it| crate::stagnation::HumanWait {
                item_id: it.id.clone(),
                title: it.title.clone(),
                project_id: it.project_id.clone(),
                created_at: it.created_at.clone(),
                age_secs: it.age_secs,
            })
            .collect();
        for aging in crate::stagnation::human_wait_aging(&waits, policy.human_wait_secs) {
            items.push(b.human_wait_aging(&aging, policy.human_wait_secs));
        }
    }
    for f in &inbox.stagnation {
        items.push(b.stagnation(f));
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

/// ADR 2026-10-07-build-tmp-hygiene D4.3: `disk_full` 項目の id（`disk_full-<path の slug>`）。
/// slug は前後の `/` を落として残りの `/` 等を `-` にしたもの（`/` は `root`、`/local` は `local`）。
pub fn disk_full_item_id(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    let slug = if trimmed.is_empty() {
        "root".to_string()
    } else {
        id_part(trimmed)
    };
    format!("disk_full-{slug}")
}

/// 時刻の数字だけ（`2026-10-02T13:10:00Z` → `20261002131000`）。同じ task の別の出来事を id で分ける。
fn ts_part(ts: &str) -> String {
    ts.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// D2 `detail` の字数の上限。超えた本文は切り、全文の在りかを `links` に残す。
pub const DETAIL_CLIP_CHARS: usize = 600;

/// `links` に足す全文の在りかの label（ADR 2026-10-28-cos-inbox-chat-summary D1）。
pub const FULL_TEXT_LINK_LABEL: &str = "全文";

/// Draft の本文。`Clipped` は `finish` が `DETAIL_CLIP_CHARS` 字で切る。`Whole` は切らない
/// （統合の依頼の Markdown など、全体が判断材料のもの）。
enum Detail {
    Clipped(String),
    Whole(String),
}

/// D2 `detail`: `finish` で 600 字に切る本文。空は `None`。
fn clip(s: &str) -> Option<Detail> {
    let s = s.trim();
    (!s.is_empty()).then(|| Detail::Clipped(s.to_string()))
}

fn opt(key: &str, label: &str, needs_note: bool, effect: &str) -> InboxOption {
    InboxOption {
        key: key.to_string(),
        label: label.to_string(),
        needs_note,
        effect: effect.to_string(),
    }
}

/// ADR 2026-10-28-cos-integrate-task D1: integrate task を起票する endpoint の path。
pub fn integrate_task_path(task: task_core::TaskId, request_id: &str) -> String {
    format!("/api/v1/tasks/{task}/integration-requests/{request_id}/integrate-task")
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
    detail: Option<Detail>,
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
        let mut links = d.links;
        let (detail, detail_full) = match d.detail {
            None => (None, None),
            Some(Detail::Whole(text)) => (Some(text.clone()), Some(text)),
            Some(Detail::Clipped(text)) if text.chars().count() <= DETAIL_CLIP_CHARS => {
                (Some(text.clone()), Some(text))
            }
            Some(Detail::Clipped(text)) => {
                // ADR 2026-10-28-cos-inbox-chat-summary D1: 切ったら全文の在りかを残す。
                // question・認可の本文は task の最新の問い、それ以外も task 詳細に全文がある。
                if let Some(task) = &d.task
                    && !links.iter().any(|l| l.label == FULL_TEXT_LINK_LABEL)
                {
                    links.push(link(
                        FULL_TEXT_LINK_LABEL,
                        format!("/api/v1/tasks/{}", task.id),
                    ));
                }
                let clipped = format!(
                    "{}…",
                    text.chars().take(DETAIL_CLIP_CHARS).collect::<String>()
                );
                (Some(clipped), Some(text))
            }
        };
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
            detail,
            detail_full,
            options: d.options,
            recommended: d.recommended,
            due_at: d.due_at,
            blocking: d.blocking,
            blocked_by: d.blocked_by,
            task: d.task,
            project_id: d.project_id,
            created_at: d.created_at,
            age_secs,
            links,
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
            options: if q.plan_issue_units.is_empty() {
                vec![opt(
                    "answer",
                    "答える（note に回答を書く）",
                    true,
                    &format!("POST /tasks/{}/answer", q.task.id),
                )]
            } else {
                // ADR 2026-10-22-plan-issue-answer-replans D5: 計画の問題の申告は、回答して同じ計画で再開するか、
                // 回答を planner への指示にして check・範囲を直すかを分けて出す。
                vec![
                    opt(
                        "answer",
                        "回答して再開（計画はそのまま）",
                        true,
                        &format!(
                            "POST /tasks/{}/answer（WorkUnit {} を同じ check のまま再開。申告が繰り返され得る）",
                            q.task.id,
                            q.plan_issue_units.join(", ")
                        ),
                    ),
                    opt(
                        "replan",
                        "計画を直す（回答を planner への指示にする）",
                        true,
                        &format!(
                            "POST /tasks/{}/answer intent=replan（planner が WorkUnit {} の check・範囲を直してから再開。replan の上限に数えない）",
                            q.task.id,
                            q.plan_issue_units.join(", ")
                        ),
                    ),
                ]
            },
            recommended: (!q.plan_issue_units.is_empty()).then(|| "replan".to_string()),
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
                detail: clip(&format!("tier={:?} harness=route-selected", hint.tier)),
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
            // ADR 2026-10-08-browser-prod-enablement D2・D4: 受信箱では答えない（選択肢なし・native なし）。
            // 文が次の手（release・台帳の作り直し、task 詳細で policy を入れる）を言い、前提が揃えば消える。
            AttentionItem::BrowserPrerequisite {
                task,
                code,
                message,
                at,
            } => self.finish(Draft {
                id: format!("browser_prerequisite-{}-{}", task.id, code.as_str()),
                kind: InboxKind::BrowserWait,
                title: format!("browser の前提が足りない: {}", task.title),
                detail: clip(message),
                options: Vec::new(),
                recommended: None,
                due_at: None,
                blocking: self.only_task(task),
                blocked_by: Vec::new(),
                native: None,
                project_id: self.project_of(task.id),
                task: Some(task.clone()),
                created_at: at.clone(),
                links: vec![link("task 詳細", format!("/tasks/{}", task.id))],
            }),
            AttentionItem::IntegrationRequest {
                task,
                request_id,
                request,
                integrate_task_offered,
                integrate_task_escalated,
                at,
            } => {
                let mut detail = String::new();
                if let Some(why) = integrate_task_escalated {
                    detail.push_str(&format!("人の判断が必要: {why}\n\n"));
                }
                detail.push_str(&request.to_markdown());
                if !request.conflict_files.is_empty() {
                    detail.push_str("\n## 衝突 file\n");
                    for path in &request.conflict_files {
                        detail.push_str(&format!("- `{path}`\n"));
                    }
                }
                if let Some(sha) = &request.candidate_sha {
                    detail.push_str(&format!("\n- 候補 SHA: `{sha}`\n"));
                }
                // ADR 2026-10-28-cos-integrate-task D4: 衝突を解く integrate task を起票する選択肢と native。
                let mut options = Vec::new();
                if *integrate_task_offered {
                    options.push(opt(
                        task_core::integration_request::INTEGRATE_TASK_OPTION,
                        "integrate task に解かせる",
                        false,
                        "衝突を解く task を起票し、検査と reviewer の判定を経て取り込む",
                    ));
                }
                options.extend([
                    opt(
                        "integrated",
                        "統合した",
                        false,
                        "統合済みとして回答を記録する",
                    ),
                    opt("declined", "見送る", false, "見送りとして回答を記録する"),
                    opt("retry", "再試行する", false, "再試行として回答を記録する"),
                ]);
                let title = if integrate_task_escalated.is_some() {
                    format!("統合の依頼（integrate task が失敗）: {}", task.title)
                } else {
                    format!("統合の依頼: {}", task.title)
                };
                self.finish(Draft {
                    id: format!("integration_request-{}", id_part(request_id)),
                    kind: InboxKind::IntegrationRequest,
                    title,
                    detail: Some(Detail::Whole(detail)),
                    options,
                    // ADR 2026-10-28-cos-routine-delegated-answers 付記: integrate task は ADR が定めた既定の
                    // 答え方なので推奨にする（CoS は documented_default の根拠で代答する）。
                    recommended: integrate_task_offered
                        .then(|| task_core::integration_request::INTEGRATE_TASK_OPTION.to_string()),
                    due_at: None,
                    blocking: self.only_task(task),
                    blocked_by: Vec::new(),
                    native: integrate_task_offered
                        .then(|| {
                            native(
                                "POST",
                                integrate_task_path(task.id, request_id),
                            )
                        })
                        .flatten(),
                    project_id: self.project_of(task.id),
                    task: Some(task.clone()),
                    created_at: at.clone(),
                    links: Vec::new(),
                })
            }
            // ADR 2026-10-23-stage-order-and-plan-stall D5: 受信箱では答えない（native なし。答えると 409）。選択肢は人の手
            // （task 詳細の replan か取り消し）を示すだけ。何か起きれば次の生存確認で消える。
            AttentionItem::Stalled {
                task,
                reason,
                detail,
                since,
                at,
            } => self.finish(Draft {
                id: format!("stalled-{}-{}", task.id, ts_part(at)),
                kind: InboxKind::Stalled,
                title: format!("理由なく止まっている: {}", task.title),
                detail: clip(&format!(
                    "{detail}（{reason}、{since} から）。task 詳細の計画で replan するか取り消す"
                )),
                options: vec![
                    opt(
                        "replan",
                        "task 詳細の計画から replan する",
                        false,
                        "計画を分解し直す（POST /tasks/{id}/execution/decompose）。動けば項目は消える",
                    ),
                    opt(
                        "cancel",
                        "取り消す",
                        false,
                        &format!("POST /tasks/{}/cancel", task.id),
                    ),
                ],
                recommended: Some("replan".to_string()),
                due_at: None,
                blocking: self.only_task(task),
                blocked_by: Vec::new(),
                native: None,
                project_id: self.project_of(task.id),
                task: Some(task.clone()),
                created_at: at.clone(),
                links: vec![link("task 詳細", format!("/tasks/{}", task.id))],
            }),
            // ADR 2026-10-27-review-resync-halt-visible-and-reviewer-reuse D3: 受信箱の `rereview` が
            // `POST /tasks/{id}/rereview`（遷移せず再開の印を残す）へ委ねる。target が静かになれば自動で再開して消える。
            AttentionItem::ReviewResyncHalted {
                task,
                reviewed_sha,
                target_sha,
                resyncs,
                at,
                ..
            } => self.finish(Draft {
                id: format!("review_resync_halted-{}-{}", task.id, ts_part(at)),
                kind: InboxKind::ReviewResyncHalted,
                title: format!("レビューの再同期が上限で止まった: {}", task.title),
                detail: clip(&format!(
                    "取り込み先が {resyncs} 回動いたので review を止めている（検査済み {reviewed_sha}、取り込み先 {target_sha}）。再開すると最新の取り込み先へ同期して検査をやり直す。取り込み先が静かになれば自動で再開する"
                )),
                options: vec![opt(
                    "rereview",
                    "いま再開する",
                    false,
                    &format!("POST /tasks/{}/rereview", task.id),
                )],
                recommended: Some("rereview".to_string()),
                due_at: None,
                blocking: self.only_task(task),
                blocked_by: Vec::new(),
                native: native("POST", format!("/api/v1/tasks/{}/rereview", task.id)),
                project_id: self.project_of(task.id),
                task: Some(task.clone()),
                created_at: at.clone(),
                links: vec![link("task 詳細", format!("/tasks/{}", task.id))],
            }),
            AttentionItem::Unintegrated {
                task,
                repo_id,
                repo,
                state,
                operation_path,
                branch,
                head_sha,
                reason,
                at,
            } => self.finish(Draft {
                // 1 task に未取り込み repo が複数あるため、item は task×repo で一意にする。
                id: format!("unintegrated-{}-{repo_id}", task.id),
                kind: InboxKind::Unintegrated,
                title: format!("未取り込み: {}", task.title),
                detail: Some(Detail::Whole(format!(
                    "repository `{repo}` / state `{state:?}` / branch `{branch}` / head `{head_sha}`\n{}",
                    reason.as_deref().unwrap_or("default branch に未到達")
                ))),
                options: vec![
                    opt(
                        "merge",
                        "merge を依頼",
                        false,
                        "default branch に取り込む",
                    ),
                    opt(
                        "discard",
                        "破棄",
                        true,
                        "理由を付けて破棄する",
                    ),
                    opt(
                        "hold",
                        "保留",
                        true,
                        "理由を付けて保留する",
                    ),
                ],
                recommended: None,
                due_at: None,
                blocking: self.only_task(task),
                blocked_by: Vec::new(),
                native: native("POST", format!("{operation_path}merge")),
                project_id: self.project_of(task.id),
                task: Some(task.clone()),
                created_at: at.clone(),
                links: vec![link("task 詳細", format!("/tasks/{}", task.id))],
            }),
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

    /// 1 つの秘密（集約キー）を待つ wait 全部の要対応 item。`created_at` は最初の wait の時刻で固定し、
    /// 待つ task が増減しても・誤キーで更新されても同じ item（通知の revision が変わらない）。値は載せない。
    fn credential_wait(&self, waits: &[&task_core::CredentialWait]) -> InboxItem {
        let first = waits[0];
        let created_at = waits
            .iter()
            .map(|w| w.created_at)
            .min()
            .unwrap_or(first.created_at);
        let rejected = waits.iter().find_map(|w| {
            w.last_verify_kind
                .filter(|_| w.state == task_core::CredentialWaitState::Rejected)
        });
        let mut tasks: Vec<TaskRef> = Vec::new();
        let mut units: Vec<String> = Vec::new();
        for w in waits {
            if let Some(t) = self.ref_of(w.task_id)
                && !tasks.iter().any(|x| x.id == t.id)
            {
                tasks.push(t);
            }
            if !units.contains(&w.work_unit_id) {
                units.push(w.work_unit_id.clone());
            }
        }
        let names: Vec<String> = tasks
            .iter()
            .map(|t| format!("{}（{}）", t.title, t.id))
            .collect();
        let problem = match rejected {
            Some(task_core::CredentialVerifyKind::InsufficientPermission) => {
                "登録されたキーは権限が足りません。権限のあるキーで更新してください。\n"
            }
            Some(_) => "登録されたキーは無効でした。正しいキーで更新してください。\n",
            None => "",
        };
        let detail = format!(
            "{problem}用途: {}\n待っている task: {}\nアカウント画面（/accounts）の「秘密」で {} を登録すると、\
             検証のあと待っている work unit が自動で続行されます。登録は連携の有効化だけで、\
             測定の予算の承認ではありません。",
            first.purpose,
            names.join("、"),
            first.secret_id
        );
        let title = if rejected.is_some() {
            format!("API キーを更新してください: {}", first.provider)
        } else {
            format!("API キーの登録が必要: {}", first.provider)
        };
        self.finish(Draft {
            id: format!("credential_wait-{}", id_part(&first.secret_id)),
            kind: InboxKind::CredentialWait,
            title,
            detail: clip(&detail),
            options: vec![opt(
                "register",
                "アカウント画面でキーを登録する",
                false,
                "登録すると検証のあと、待っている work unit が自動で続行される",
            )],
            recommended: Some("register".to_string()),
            due_at: None,
            blocking: InboxBlocking {
                root: tasks.first().and_then(|t| self.root_of(t.id)),
                summary: format!(
                    "{} 件の work unit だけ（他の unit・兄弟は止めない）",
                    units.len()
                ),
                tasks,
                units,
            },
            blocked_by: Vec::new(),
            native: native("PUT", format!("/api/v1/secrets/{}", first.secret_id)),
            project_id: self.project_of(first.task_id),
            task: self.ref_of(first.task_id),
            created_at: view::to_rfc3339(created_at),
            links: vec![link(
                "アカウント画面（秘密の登録）",
                "/accounts".to_string(),
            )],
        })
    }

    fn disk_full(&self, d: &task_core::DiskWatchState) -> InboxItem {
        let pct = d
            .last_pct
            .map(|p| format!("{p:.1}%"))
            .unwrap_or_else(|| "?".to_string());
        self.finish(Draft {
            id: disk_full_item_id(&d.path),
            kind: InboxKind::DiskFull,
            title: format!("ディスクがほぼ満杯: {}（使用率 {pct}）", d.path),
            detail: clip(&format!(
                "{} の使用率が {pct} で critical のしきい値を超えている。空きを作る（`celerisctl target sweep --apply`、\
                 不要な file の削除）。使用率がしきい値より 5 ポイント下がると自動で消える。run は止めない\
                 （空きの下限による停止は ADR-0074 の min_free_disk_mb）。",
                d.path
            )),
            options: vec![opt(
                "free_space",
                "空きを作る（host の操作）",
                false,
                "使用率が下がると項目は自動で消える",
            )],
            recommended: Some("free_space".to_string()),
            due_at: None,
            blocking: InboxBlocking {
                tasks: Vec::new(),
                units: Vec::new(),
                root: None,
                summary: "止めているものは無い（知らせるだけ）".to_string(),
            },
            blocked_by: Vec::new(),
            native: None,
            task: None,
            project_id: None,
            created_at: view::to_rfc3339(d.since),
            links: Vec::new(),
        })
    }

    /// ADR 2026-10-27-project-stagnation-inbox D2: id は論理キー、`created_at` は最も古い待ち（revision）。
    fn human_wait_aging(
        &self,
        a: &crate::stagnation::HumanWaitAging,
        threshold_secs: u64,
    ) -> InboxItem {
        let project = a
            .project_id
            .as_deref()
            .map(|p| format!("案件 {p}"))
            .unwrap_or_else(|| "案件なし".to_string());
        let lines: Vec<String> = a
            .waits
            .iter()
            .map(|w| format!("- {}（{} 分）", w.title, w.age_secs / 60))
            .collect();
        self.finish(Draft {
            id: id_part(&a.key),
            kind: InboxKind::HumanWaitAging,
            title: format!("人の操作待ちが長い: {project}（{} 件）", a.waits.len()),
            detail: clip(&format!(
                "次の待ちが {} 秒以上答えられていない。\n{}",
                threshold_secs,
                lines.join("\n")
            )),
            options: vec![opt(
                "answer_items",
                "各項目に答える",
                false,
                "待ちの項目に答えると、閾値を過ぎた待ちが無くなった時点で消える",
            )],
            recommended: Some("answer_items".to_string()),
            due_at: None,
            blocking: InboxBlocking {
                tasks: Vec::new(),
                units: Vec::new(),
                root: None,
                summary: format!("{project} の人の操作待ち {} 件", a.waits.len()),
            },
            blocked_by: a.waits.iter().map(|w| w.item_id.clone()).collect(),
            native: None,
            task: None,
            project_id: a.project_id.clone(),
            created_at: a.since.clone(),
            links: a
                .waits
                .iter()
                .map(|w| link(&w.title, format!("/api/v1/inbox/items/{}", w.item_id)))
                .collect(),
        })
    }

    /// ADR 2026-10-27-project-stagnation-inbox D1〜D3・D5: snapshot の停滞の判定 1 件。受信箱では答えない。
    fn stagnation(&self, f: &crate::stagnation::StagnationFinding) -> InboxItem {
        use crate::stagnation::StagnationKind;
        let task = f.task_id.and_then(|id| self.ref_of(id));
        let project_id = f
            .project_id
            .map(|p| p.to_string())
            .or_else(|| f.task_id.and_then(|id| self.project_of(id)));
        let (kind, title, options, recommended, blocking, links) = match f.kind {
            StagnationKind::ProjectStalled => {
                let pid = project_id.clone().unwrap_or_default();
                (
                    InboxKind::ProjectStalled,
                    format!("案件が進んでいない: {}", f.subject),
                    vec![
                        opt(
                            "inspect",
                            "案件を開いて次の手を決める",
                            false,
                            "task が動くか出来事が起きれば消える",
                        ),
                        opt(
                            "pause",
                            "案件を一時停止する",
                            false,
                            &format!("POST /projects/{pid}/pause（active でなくなれば消える）"),
                        ),
                    ],
                    Some("inspect".to_string()),
                    InboxBlocking {
                        tasks: Vec::new(),
                        units: Vec::new(),
                        root: None,
                        summary: format!("案件 {} 全体", f.subject),
                    },
                    vec![link("案件", format!("/projects/{pid}"))],
                )
            }
            StagnationKind::ReleaseUnpromoted => (
                InboxKind::ReleaseUnpromoted,
                format!("verify 済みの release が昇格されていない: {}", f.subject),
                vec![opt(
                    "promote",
                    "release 画面で昇格する",
                    false,
                    &format!("POST /releases/{}/promote（昇格すれば消える）", f.subject),
                )],
                Some("promote".to_string()),
                InboxBlocking {
                    tasks: Vec::new(),
                    units: Vec::new(),
                    root: None,
                    summary: "本番に入っていない変更".to_string(),
                },
                vec![link("release", "/releases".to_string())],
            ),
            StagnationKind::TaskUnscheduled => (
                InboxKind::TaskUnscheduled,
                format!("ready のまま起きない: {}", f.subject),
                vec![
                    opt(
                        "inspect",
                        "task 詳細で原因を見る",
                        false,
                        "run が起きるか待ちの理由が付けば消える",
                    ),
                    opt(
                        "cancel",
                        "取り消す",
                        false,
                        &format!(
                            "POST /tasks/{}/cancel",
                            f.task_id.map(|t| t.to_string()).unwrap_or_default()
                        ),
                    ),
                ],
                Some("inspect".to_string()),
                match &task {
                    Some(t) => self.only_task(t),
                    None => InboxBlocking {
                        tasks: Vec::new(),
                        units: Vec::new(),
                        root: None,
                        summary: "この task だけ".to_string(),
                    },
                },
                f.task_id
                    .map(|t| vec![link("task 詳細", format!("/tasks/{t}"))])
                    .unwrap_or_default(),
            ),
        };
        self.finish(Draft {
            id: id_part(&f.key),
            kind,
            title,
            detail: clip(&f.detail),
            options,
            recommended,
            due_at: None,
            blocking,
            blocked_by: Vec::new(),
            native: None,
            task,
            project_id,
            created_at: f.since.clone(),
            links,
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

mod item_task;
pub use item_task::item_task_id;

#[cfg(test)]
mod tests;
