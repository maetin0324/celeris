//! 対話（ADR-0033 D4 / Phase 24）。人が組織のノードに話しかけ、そのノードが返事をする経路の
//! 「判断と検証」を 1 か所にまとめる（`task-api` と `task-dispatch` の両方から呼ぶ）。
//!
//! **新しいプロトコルは作らない**: 話しかけると `messages` に `role = user` の行が 1 つ増え、
//! 既存の `tasks` に `kind = execute` の対話用タスクが 1 件できるだけ。返事はその run の
//! `artifacts/result.json` の `summary`（ADR-0006 D3）で、ディスパッチャが `role = node` の行にする。
//!
//! I/O は `TaskStore` の読み書きだけ。LLM 呼び出しは無い（DESIGN 原則 1）。

use std::path::PathBuf;

use task_core::approval::{Approval, Decision, StandingRule};
use task_core::{
    Budget, DelegateTask, GenreSpec, ListFilter, ListOrder, Message, MessageId, MessageRole,
    OrgNode, ProjectId, RoleSpec, Status, Task, TaskId, TaskKind, TaskStore, Tier, Trigger,
    WorkerHint, WorkspaceMode, WorkspaceSpec, conversation_title, department_of, failure_reply,
};
use time::OffsetDateTime;

use crate::error::OpsError;

/// 対話用 run の予算（ADR-0033 D4「budget は小さめ」）。返事 1 回ぶんなので短く切る。
/// 役割の既定（`[[roles]]`）より**こちらが勝つ**: 対話は「ひと言返す」仕事で、実装 run の予算とは別物。
/// Phase 28: 実機で `error_max_turns`（6 ターンで打ち切り）が起きたため 10 に上げた（読むだけでも数ターン
/// 使う。ADR-0033 D4 追記 / 実機の一本目、2026-09-17）。`max_wall_secs` は変えない。
pub const CONVERSATION_MAX_TURNS: u32 = 10;
pub const CONVERSATION_MAX_WALL_SECS: u64 = 300;
pub const CONVERSATION_MAX_RETRIES: u32 = 1;
/// 対話用タスクの優先度（人が画面の前で待っているので、通常のタスクより少しだけ前に出す）。
pub const CONVERSATION_PRIORITY: i32 = 1;
/// run の前置きに載せる直近のやり取りの既定件数（ADR-0033 D4）。
pub const CONVERSATION_HISTORY: usize = 20;
/// 直列化（Phase 27 の監査 M-3）のために見る「終端でないタスク」の件数の上限。
const OPEN_TASK_SCAN: usize = 500;

/// `start` の結果。API は 202 で `{message_id, task_id}` を返す。
#[derive(Debug, Clone, PartialEq)]
pub struct StartedConversation {
    pub message: Message,
    pub task: Task,
}

/// 人がノードに話しかける（ADR-0033 D4。Phase 30 で対話用分野の解決を変更）。`messages` に
/// `role = user` の行を入れ、そのノードの run を 1 回起こすための対話用タスク（`kind = execute`、
/// 受け入れ条件なし）を作って `ready` にする。
///
/// 道具立て（分野・役割・tier・アダプタ）は決定的に引く: **ノードの `genre` は使わない**（実機の事故:
/// 検索ハーネス genre のノードに話しかけると検索ハーネスが会話しようとして証拠ゲートで落ちた）。
/// 常に `conversation_genre`（設定 `[conversation] genre`、既定 `CONVERSATION_GENRE = "secretary"`）
/// → その分野の `default_role` → 役割の `tier` / `adapter`。予算は上の定数。
#[allow(clippy::too_many_arguments)]
pub fn start(
    store: &dyn TaskStore,
    node_id: &str,
    project_id: Option<ProjectId>,
    text: &str,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    conversation_genre: &str,
    now: OffsetDateTime,
) -> Result<StartedConversation, OpsError> {
    start_full(
        store,
        node_id,
        project_id,
        None,
        None,
        text,
        roles,
        genres,
        conversation_genre,
        now,
    )
}

/// ADR-0056 D2（Phase 78）: `start` と同じ対話を、**外部の発言者**（`mcp:<client_id>`）を印として
/// 添えて起こす。人が Console から話しかける経路（`start`）は常に `author = None`。
#[allow(clippy::too_many_arguments)]
pub fn start_as(
    store: &dyn TaskStore,
    node_id: &str,
    project_id: Option<ProjectId>,
    author: &str,
    text: &str,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    conversation_genre: &str,
    now: OffsetDateTime,
) -> Result<StartedConversation, OpsError> {
    start_full(
        store,
        node_id,
        project_id,
        None,
        Some(author),
        text,
        roles,
        genres,
        conversation_genre,
        now,
    )
}

/// Phase 41（ADR-0038 D1）: `start` と同じ対話を、**途中目標に紐づけて**起こす（レビューの対話）。
/// `milestone_id` が入った対話用タスクは裏方の `support = "milestone_review"`（`task_core::support_kind`）
/// になり、仕事の木には出ない。人が話しかける経路（`start`）は常に `None` を渡す。
#[allow(clippy::too_many_arguments)]
pub fn start_with_milestone(
    store: &dyn TaskStore,
    node_id: &str,
    project_id: Option<ProjectId>,
    milestone_id: Option<task_core::MilestoneId>,
    text: &str,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    conversation_genre: &str,
    now: OffsetDateTime,
) -> Result<StartedConversation, OpsError> {
    start_full(
        store,
        node_id,
        project_id,
        milestone_id,
        None,
        text,
        roles,
        genres,
        conversation_genre,
        now,
    )
}

/// `start` / `start_with_milestone` / `start_as` の共通の芯（ADR-0056 D2 で `author` を足した）。
#[allow(clippy::too_many_arguments)]
fn start_full(
    store: &dyn TaskStore,
    node_id: &str,
    project_id: Option<ProjectId>,
    milestone_id: Option<task_core::MilestoneId>,
    author: Option<&str>,
    text: &str,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    conversation_genre: &str,
    now: OffsetDateTime,
) -> Result<StartedConversation, OpsError> {
    if text.trim().is_empty() {
        return Err(OpsError::Validation("text must not be blank".to_string()));
    }
    let Some(node) = store.org_get(node_id)? else {
        return Err(OpsError::Validation(format!(
            "{node_id:?} is not an org node"
        )));
    };
    if let Some(project_id) = project_id
        && store.project_get(project_id)?.is_none()
    {
        return Err(OpsError::Validation(format!(
            "project {project_id} does not exist"
        )));
    }

    // 監査 M-3: 同じノード・同じ案件に未終了の対話タスクがあれば、その後ろに並べる（返事は送った順に返る）。
    let depends_on = open_conversation_tasks(store, &node.id, project_id)?;

    let mut task = conversation_task(
        &node,
        project_id,
        text,
        roles,
        genres,
        conversation_genre,
        now,
    );
    task.depends_on = depends_on;
    task.milestone_id = milestone_id;
    let message = Message {
        id: MessageId::new(),
        node_id: node.id.clone(),
        project_id,
        role: MessageRole::User,
        text: text.to_string(),
        run_id: None,
        // R4（migration 0007）: 人の発言も返事も、同じ対話用タスクの id を持つ。
        task_id: Some(task.id),
        // ADR-0056 D2（Phase 78）: `author` があれば（MCP クライアント）その印を残す。
        metadata: author.map(|a| task_core::MessageMetadata {
            author: Some(a.to_string()),
            ..Default::default()
        }),
        created_at: now,
    };
    store.message_append(&message)?;
    let task = Task {
        conversation: Some(message.id),
        ..task
    };
    store.create_task(&task, vec![])?;
    // 対話用タスクは人が承認するものではない（話しかけた時点が承認）。draft のままだと run が起きない。
    store.apply_transition(task.id, Trigger::Accept, None)?;
    let task = store.get(task.id)?.unwrap_or(task);
    Ok(StartedConversation { message, task })
}

/// 監査 M-3: そのノード・その案件の、まだ終端に達していない対話用タスク（古い順）。
/// 新しい対話タスクの `depends_on` に入れて、返事が送った順に返るようにする。
///
/// ADR-0054 D1/D2（Phase 68）: **CoS**（`task_core::COS_ID`）は案件をまたいでも継続セッションが
/// 全体で 1 本（`node_sessions` は `project_id = None` 固定）なので、待ち行列も**案件をまたいで**
/// 直列化する（そうしないと、`scope=project:<id>` の一言と `scope=all`（無案件）の一言が同じ CoS の
/// run として同時に走り、同じ継続セッションに 2 つの run がぶつかる）。それ以外のノードは従来どおり
/// `project_id` が一致するものだけ（ノードの継続セッションは案件ごとには分かれていないが、対話の
/// 直列化そのものは Phase 27 の監査 M-3 のまま案件単位でよい。人が違う案件で同時に別々の対話をしても
/// 待たされない）。
fn open_conversation_tasks(
    store: &dyn TaskStore,
    node_id: &str,
    project_id: Option<ProjectId>,
) -> Result<Vec<TaskId>, OpsError> {
    let is_cos = node_id == task_core::COS_ID;
    let filter = ListFilter {
        statuses: vec![
            Status::Draft,
            Status::Ready,
            Status::Running,
            Status::Blocked,
            Status::Reviewing,
        ],
        // CoS は案件で絞らずストアから全部引き、下のフィルタで案件をまたいで直列化する。
        project_id: if is_cos { None } else { project_id },
        ..ListFilter::default()
    };
    let page = store.list_page(&filter, ListOrder::CreatedDesc, None, OPEN_TASK_SCAN)?;
    let mut open: Vec<&Task> = page
        .items
        .iter()
        .filter(|t| {
            task_core::is_conversation(t)
                && t.assignee.as_deref() == Some(node_id)
                && (is_cos || t.project_id == project_id)
        })
        .collect();
    open.sort_by_key(|t| t.created_at);
    Ok(open.iter().map(|t| t.id).collect())
}

/// 対話用タスクを組み立てる（純粋。ストアは見ない）。`conversation`（人の発言 id）は呼び出し側が入れる。
fn conversation_task(
    node: &OrgNode,
    project_id: Option<ProjectId>,
    text: &str,
    roles: &[RoleSpec],
    genres: &[GenreSpec],
    conversation_genre: &str,
    now: OffsetDateTime,
) -> Task {
    // Phase 30: 対話はノードの `genre`（仕事のハーネス）に関係なく、常に対話用分野で走る
    // （実機の事故: 関連研究調査課＝検索ハーネスに話しかけたら検索ハーネスが会話しようとして落ちた）。
    let genre = GenreSpec::find(genres, conversation_genre);
    let role = genre
        .and_then(|g| g.default_role.as_deref())
        .and_then(|r| RoleSpec::find(roles, r));
    let id = TaskId::new();
    Task {
        tree: None,
        paused_at: None,
        routing: None,
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: TaskKind::Execute,
        title: conversation_title(text),
        objective: text.to_string(),
        // 受け入れ条件は無い（返事に合否は無い。レビューは素通りする）。
        acceptance: vec![],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Draft,
        priority: CONVERSATION_PRIORITY,
        worker_hint: WorkerHint {
            tier: role.and_then(|r| r.tier).unwrap_or(Tier::Standard),
            adapter: role.and_then(|r| r.adapter.clone()),
        },
        // ADR-0006 Phase 115 D3: 対話は diff を作らない（repos も常に空。上の `repos: Vec::new()`）ので
        // worktree を作らない。念のため `mode: Shared` を明示する（`resolve_repos` は経由しないが、
        // 将来リポジトリ付きの案件に対話タスクを置く経路ができても worktree を切らせない）。
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from(id.to_string()),
            mode: Some(WorkspaceMode::Shared),
        },
        budget: Budget {
            max_turns: CONVERSATION_MAX_TURNS,
            max_wall_secs: CONVERSATION_MAX_WALL_SECS,
            max_retries: CONVERSATION_MAX_RETRIES,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: role.map(|r| r.id.clone()),
        genre: genre.map(|g| g.id.clone()),
        aggregate: false,
        // ADR-0046 D2 / D4（Phase 59）: 対話には必要な能力タグも進め方も無い（返事に合否は無い）。
        skills: Vec::new(),
        mode: task_core::TaskMode::default(),
        project_id,
        milestone_id: None,
        assignee: Some(node.id.clone()),
        // 対話由来の印（`tasks` の列は増やさない。`task_core::message` 参照）。呼び出し側が入れる。
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

/// 対話用タスクの run が終わったときの返事（`role = node` の行）を追記する（ADR-0033 D4）。
/// `task` が対話用でなければ何もしない（`Ok(None)`）。`summary` が空なら run が何も言わなかったという
/// ことなので、その旨を残す（空行は入れない）。
pub fn record_reply(
    store: &dyn TaskStore,
    task: &Task,
    run_id: &str,
    text: &str,
    now: OffsetDateTime,
) -> Result<Option<Message>, OpsError> {
    record_reply_with_metadata(store, task, run_id, text, None, now)
}

/// ADR-0048 D3（Phase 60b）: `record_reply` と同じだが、CoS の `actions` の実行結果
/// （`Message.metadata`）を一緒に残す。
pub fn record_reply_with_metadata(
    store: &dyn TaskStore,
    task: &Task,
    run_id: &str,
    text: &str,
    metadata: Option<task_core::MessageMetadata>,
    now: OffsetDateTime,
) -> Result<Option<Message>, OpsError> {
    if !task_core::is_conversation(task) {
        return Ok(None);
    }
    let Some(node_id) = task.assignee.clone() else {
        return Ok(None);
    };
    let text = if text.trim().is_empty() {
        failure_reply("返事の本文が空でした")
    } else {
        text.to_string()
    };
    let message = Message {
        id: MessageId::new(),
        node_id,
        project_id: task.project_id,
        role: MessageRole::Node,
        text,
        run_id: Some(run_id.to_string()),
        // R4（migration 0007）: 人の発言の行と同じ対話用タスクの id。
        task_id: Some(task.id),
        metadata,
        created_at: now,
    };
    store.message_append(&message)?;
    Ok(Some(message))
}

// ---- SPEC §3.1「部をまたぐ連携は秘書が認める」（ADR-0033 D4 最終項 + D5。Phase 27 で認可に接続）----

/// 部をまたぐ委譲の質問の先頭（`approvals.question` と `standing_rules.rule` の照合の鍵）。
/// Phase 27: 質問を人が読む自由文から**構造化した固定の形**に変えた。これが無いと、人が答えても
/// 次の run で同じ質問に戻る（Phase 24 の監査 H-1: 永久ループ）。
pub const CROSS_DEPARTMENT_PREFIX: &str = "cross-department: ";

/// 部をまたぐ委譲 1 件。`question` は `"cross-department: <from> -> <to>: <理由>"`、
/// 照合の鍵は `"cross-department: <from> -> <to>"`（前方一致で決定的に照合する）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossDepartment {
    /// 委譲元のノード（`task.assignee`）。
    pub from: String,
    /// 委譲先のノード（提案の `assignee`）。
    pub to: String,
    /// 理由（提案の `title`）。
    pub title: String,
}

impl CrossDepartment {
    /// 認可を照合する鍵（`approvals.question` の先頭 / `standing_rules.rule` の先頭）。
    pub fn key(&self) -> String {
        format!("{CROSS_DEPARTMENT_PREFIX}{} -> {}", self.from, self.to)
    }

    /// `approvals.question` に入れる文面（鍵 + 理由）。
    pub fn question(&self) -> String {
        format!("{}: {}", self.key(), self.title)
    }
}

/// 質問文（`approvals.question`）が部をまたぐ委譲のものなら、その照合の鍵を返す。
/// `"cross-department: <from> -> <to>: <理由>"` の 3 つ目の `:` より前まで（ノードの id に `:` は使わない）。
pub fn cross_department_key(question: &str) -> Option<String> {
    let rest = question.strip_prefix(CROSS_DEPARTMENT_PREFIX)?;
    let pair = rest.split(':').next().unwrap_or(rest).trim_end();
    if pair.is_empty() {
        return None;
    }
    Some(format!("{CROSS_DEPARTMENT_PREFIX}{pair}"))
}

/// 1 件の部をまたぐ委譲について、人がもう答えているか（決定的。文字列の前方一致だけで決める）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossAuthorization {
    /// 通してよい（`once` / `standing` の決定、または `standing_rules` の規則がある）。
    Allowed,
    /// まだ聞いていない（秘書の認可待ち）。
    Pending,
    /// 人が認めなかった（`denied`）。
    Denied,
}

/// 認可の照合（純粋関数）。`rules` は委譲元宛て + 全員向け、`approvals` は委譲元のノード宛ての全件。
pub fn cross_authorization(
    key: &str,
    task_id: TaskId,
    approvals: &[Approval],
    rules: &[StandingRule],
) -> CrossAuthorization {
    // 「今後ずっと」は案件・タスクに依らず効く（SPEC §3.6）。
    if rules.iter().any(|r| r.rule.starts_with(key)) {
        return CrossAuthorization::Allowed;
    }
    // 同じタスクの、同じ鍵の決定のうち最も新しいもの（人が答え直せるので後勝ち）。
    let decided = approvals
        .iter()
        .filter(|a| a.task_id == Some(task_id) && a.question.starts_with(key))
        // Phase F7: `withdrawn`（認可元のタスクが終わって celeris が取り下げた）は人の決定ではない。
        // 「まだ決まっていない」と読む（やり直した run がもう一度聞けるように）。
        .filter(|a| a.decision != Some(Decision::Withdrawn))
        .filter_map(|a| a.decision.map(|d| (a.decided_at, a.created_at, d)))
        .max_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)))
        .map(|(_, _, d)| d);
    match decided {
        Some(Decision::Once) | Some(Decision::Standing) => CrossAuthorization::Allowed,
        Some(Decision::Denied) => CrossAuthorization::Denied,
        Some(Decision::Withdrawn) | None => CrossAuthorization::Pending,
    }
}

/// 委譲の提案を「そのまま子にしてよいもの」と「秘書の認可待ち」「人が認めなかったもの」に分ける。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DelegationSplit {
    /// 子を作ってよい提案（同じ部宛て、または認可済みの部またぎ）。
    pub allowed: Vec<DelegateTask>,
    /// 秘書に聞く部またぎ（子は作らない）。
    pub pending: Vec<CrossDepartment>,
    /// 人が認めなかった部またぎ（子は作らないが、もう聞かない）。
    pub denied: Vec<CrossDepartment>,
}

/// SPEC §3.1 / ADR-0033 D4・D5: 委譲の提案を認可の状態で振り分ける（Phase 27 の監査 H-1 / H-2 対応）。
///
/// - **バッチは分ける**: 同じ部宛ての提案はその場で子にし、部またぎの提案だけを質問にする
///   （Phase 24 は 1 件でも部またぎがあると全件を止めていた）。
/// - 委譲元に担当が無い / 担当が秘書（部に属さない）なら、この規則は効かない（誰にでも振れる）。
/// - 提案に `assignee` が無い、または組織に無い id なら、この規則は効かない（従来の委譲のまま）。
///
/// 読むのは `org_nodes` / `approvals` / `standing_rules` だけで、LLM は呼ばない（DESIGN 原則 1）。
pub fn split_delegation(
    store: &dyn TaskStore,
    org: &[OrgNode],
    parent: &Task,
    proposals: &[DelegateTask],
) -> Result<DelegationSplit, OpsError> {
    let mut split = DelegationSplit::default();
    let (Some(from), Some(from_dept)) = (
        parent.assignee.as_deref(),
        parent
            .assignee
            .as_deref()
            .and_then(|from| department_of(org, from)),
    ) else {
        split.allowed = proposals.to_vec();
        return Ok(split);
    };
    // 「全員向け + この委譲元向け」の規則と、この委譲元が聞いた認可の全件（決定を含む）。
    let rules = store.standing_rule_list(Some(from))?;
    let approvals = store.approval_list(None, None, Some(from))?;
    for proposal in proposals {
        let crossing = match proposal.assignee.as_deref() {
            Some(to) => match department_of(org, to) {
                Some(to_dept) if to_dept != from_dept => CrossDepartment {
                    from: from.to_string(),
                    to: to.to_string(),
                    title: proposal.title.clone(),
                },
                _ => {
                    split.allowed.push(proposal.clone());
                    continue;
                }
            },
            None => {
                split.allowed.push(proposal.clone());
                continue;
            }
        };
        match cross_authorization(&crossing.key(), parent.id, &approvals, &rules) {
            CrossAuthorization::Allowed => split.allowed.push(proposal.clone()),
            CrossAuthorization::Pending => {
                if !split.pending.contains(&crossing) {
                    split.pending.push(crossing);
                }
            }
            CrossAuthorization::Denied => {
                if !split.denied.contains(&crossing) {
                    split.denied.push(crossing);
                }
            }
        }
    }
    Ok(split)
}

#[cfg(test)]
mod tests;
