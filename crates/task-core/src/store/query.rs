use rusqlite::types::Value as SqlValue;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{Status, Task, TaskId, TaskKind};
use crate::org::{MilestoneId, ProjectId};

use super::{StoreError, format_rfc3339, kind_str, status_str, tier_str};

/// `TaskStore::list_page` のフィルタ（ADR-0013 D10）。既定は絞り込み無し。
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    /// 空なら status で絞らない。
    pub statuses: Vec<Status>,
    /// 空なら kind で絞らない。
    pub kinds: Vec<TaskKind>,
    /// ADR-0027 D1: 空なら genre で絞らない。完全一致（`kinds` と同じ形）。
    pub genres: Vec<String>,
    pub parent_id: Option<TaskId>,
    /// ADR-0033 D2: 案件で絞る（案件の仕事の木 = `tasks WHERE project_id = ?`）。
    pub project_id: Option<ProjectId>,
    /// true なら `parent_id IS NULL` のタスクのみ（`parent_id` フィルタとは独立に AND で効く）。
    pub root_only: bool,
    /// `title` または `objective` に対する部分一致（SQLite の LIKE なので ASCII の大文字小文字は区別しない。ADR-0014 D2）。
    /// `%` / `_` はリテラルとして扱う。
    pub text_contains: Option<String>,
    /// ADR-0033 D4（Phase 33）: 担当（`org_nodes.id`）で絞る。完全一致。`None` なら絞らない。
    pub assignee: Option<String>,
    // ---- ADR-0044 D4（Phase 53）: ボードと検索のフィルタ。ここから ----
    /// ADR-0044 D4: ラベル。複数指定は **AND**（全部持つタスクだけ）。
    pub labels: Vec<String>,
    /// ADR-0044 D4: 種類。複数指定は IN（どれか）。
    pub categories: Vec<crate::model::TaskCategory>,
    /// ADR-0044 D4: 途中目標（`milestones.id`）。完全一致。
    pub milestone_id: Option<MilestoneId>,
    /// ADR-0044 D4: tier（`worker_hint.tier`）。複数指定は IN。
    pub tiers: Vec<crate::model::Tier>,
    /// ADR-0044 D4: 優先度（P0〜P3 を `i32` に写したもの）。複数指定は IN。
    pub priorities: Vec<i32>,
    /// ADR-0044 D4: `text_contains` を**コメント本文にも**広げる（`GET /tasks?q=`）。
    /// `false` なら従来どおり title / objective だけ（`celerisctl` の既存の挙動）。
    pub text_includes_comments: bool,
    // ---- ADR-0044 D4（Phase 53）: ここまで ----
    /// ADR-0044 D6（Phase 55）: **アーカイブされた案件のタスクを隠す**（`GET /tasks` の既定。
    /// `?archived=1` で `false`）。既定は `false`（＝隠さない）なので、ディスパッチャ・報告・
    /// 途中目標のレビューなど既存の呼び出し側の挙動は変わらない。
    pub hide_archived: bool,
}

/// `TaskStore::list_page` の並び順（ADR-0013 D10）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListOrder {
    /// `priority DESC, created_at ASC, id ASC`（ディスパッチ順）。
    Dispatch,
    /// `updated_at DESC, id DESC`。
    UpdatedDesc,
    /// `created_at DESC, id DESC`。
    CreatedDesc,
}

/// keyset ページングの 1 ページ。
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// 次ページがあれば `Some`。`list_page` にそのまま渡せる不透明な文字列。
    pub next_cursor: Option<String>,
    /// 同じフィルタでの総件数（cursor に依らない）。
    pub total: u64,
}

/// `list_page` の cursor の内側の表現。`{priority, created_at, updated_at, id}` を JSON にして
/// バイト列を 16 進エンコードしたものが `cursor` 文字列（不透明・実装依存。`ListOrder` ごとに
/// 必要な列だけを使って keyset 述語を組み立てる）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CursorPayload {
    priority: i64,
    created_at: String,
    updated_at: String,
    id: String,
}

impl CursorPayload {
    pub(crate) fn from_task(task: &Task) -> Result<Self, StoreError> {
        Ok(Self {
            priority: task.priority as i64,
            created_at: format_rfc3339(task.created_at)?,
            updated_at: format_rfc3339(task.updated_at)?,
            id: task.id.to_string(),
        })
    }
}

pub(crate) fn encode_cursor(payload: &CursorPayload) -> Result<String, StoreError> {
    let json = serde_json::to_vec(payload)?;
    let mut out = String::with_capacity(json.len() * 2);
    for byte in json {
        out.push_str(&format!("{byte:02x}"));
    }
    Ok(out)
}

pub(crate) fn decode_cursor(cursor: &str) -> Result<CursorPayload, StoreError> {
    let invalid = || StoreError::Invalid(format!("invalid cursor: {cursor}"));
    if cursor.is_empty()
        || !cursor.len().is_multiple_of(2)
        || !cursor.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(invalid());
    }
    let bytes_chars: Vec<char> = cursor.chars().collect();
    let mut bytes = Vec::with_capacity(bytes_chars.len() / 2);
    for pair in bytes_chars.chunks(2) {
        let s: String = pair.iter().collect();
        let byte = u8::from_str_radix(&s, 16).map_err(|_| invalid())?;
        bytes.push(byte);
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

pub(crate) fn usize_to_i64(v: usize) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

pub(crate) fn u64_to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            other => out.push(other),
        }
    }
    out
}

/// `ListFilter` を `WHERE` 述語と束縛パラメータに変換する（`cursor` の keyset 述語は含まない）。
pub(crate) fn filter_predicate(filter: &ListFilter) -> (String, Vec<SqlValue>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<SqlValue> = Vec::new();

    if !filter.statuses.is_empty() {
        let placeholders = vec!["?"; filter.statuses.len()].join(", ");
        clauses.push(format!("status IN ({placeholders})"));
        for s in &filter.statuses {
            params.push(SqlValue::Text(status_str(*s).to_string()));
        }
    }
    if !filter.kinds.is_empty() {
        let placeholders = vec!["?"; filter.kinds.len()].join(", ");
        clauses.push(format!("kind IN ({placeholders})"));
        for k in &filter.kinds {
            params.push(SqlValue::Text(kind_str(*k).to_string()));
        }
    }
    if !filter.genres.is_empty() {
        let placeholders = vec!["?"; filter.genres.len()].join(", ");
        clauses.push(format!("genre IN ({placeholders})"));
        for g in &filter.genres {
            params.push(SqlValue::Text(g.clone()));
        }
    }
    if filter.root_only {
        clauses.push("parent_id IS NULL".to_string());
    }
    if let Some(parent_id) = filter.parent_id {
        clauses.push("parent_id = ?".to_string());
        params.push(SqlValue::Text(parent_id.to_string()));
    }
    if let Some(project_id) = filter.project_id {
        clauses.push("project_id = ?".to_string());
        params.push(SqlValue::Text(project_id.to_string()));
    }
    if let Some(assignee) = &filter.assignee {
        clauses.push("assignee = ?".to_string());
        params.push(SqlValue::Text(assignee.clone()));
    }
    // ---- ADR-0044 D4（Phase 53）----
    if let Some(milestone_id) = filter.milestone_id {
        clauses.push("milestone_id = ?".to_string());
        params.push(SqlValue::Text(milestone_id.to_string()));
    }
    if !filter.categories.is_empty() {
        let placeholders = vec!["?"; filter.categories.len()].join(", ");
        // 導入前の行（`category` が NULL）は `other` として扱う。
        clauses.push(format!("COALESCE(category, 'other') IN ({placeholders})"));
        for c in &filter.categories {
            params.push(SqlValue::Text(c.as_str().to_string()));
        }
    }
    for label in &filter.labels {
        // AND: 指定したラベルを**全部**持つタスクだけ。`labels_json` は `PATCH` でも書き直す写し。
        clauses.push(
            "EXISTS (SELECT 1 FROM json_each(COALESCE(tasks.labels_json, '[]')) WHERE json_each.value = ?)"
                .to_string(),
        );
        params.push(SqlValue::Text(label.clone()));
    }
    if !filter.tiers.is_empty() {
        // tier は `json` の中にしか無い（列を増やさない）。JSON1 の `json_extract` で決定的に引く。
        let placeholders = vec!["?"; filter.tiers.len()].join(", ");
        clauses.push(format!(
            "json_extract(json, '$.worker_hint.tier') IN ({placeholders})"
        ));
        for t in &filter.tiers {
            params.push(SqlValue::Text(tier_str(*t).to_string()));
        }
    }
    if !filter.priorities.is_empty() {
        let placeholders = vec!["?"; filter.priorities.len()].join(", ");
        clauses.push(format!("priority IN ({placeholders})"));
        for p in &filter.priorities {
            params.push(SqlValue::Integer(*p as i64));
        }
    }
    // ADR-0044 D6（Phase 55）: アーカイブされた案件のタスクを隠す（案件に属さないタスクは常に見える）。
    if filter.hide_archived {
        clauses.push(
            "NOT EXISTS (SELECT 1 FROM projects p WHERE p.id = tasks.project_id AND p.archived_at IS NOT NULL)"
                .to_string(),
        );
    }
    if let Some(needle) = &filter.text_contains {
        let pattern = format!("%{}%", escape_like(needle));
        if filter.text_includes_comments {
            // ADR-0044 D4: `q=` は title / objective / コメント本文の 3 つ（OR）。
            clauses.push(
                "(title LIKE ? ESCAPE '\\' OR objective LIKE ? ESCAPE '\\' OR EXISTS \
                 (SELECT 1 FROM task_comments c WHERE c.task_id = tasks.id AND c.body LIKE ? ESCAPE '\\'))"
                    .to_string(),
            );
            params.push(SqlValue::Text(pattern.clone()));
            params.push(SqlValue::Text(pattern.clone()));
            params.push(SqlValue::Text(pattern));
        } else {
            clauses.push("(title LIKE ? ESCAPE '\\' OR objective LIKE ? ESCAPE '\\')".to_string());
            params.push(SqlValue::Text(pattern.clone()));
            params.push(SqlValue::Text(pattern));
        }
    }

    if clauses.is_empty() {
        ("1=1".to_string(), params)
    } else {
        (clauses.join(" AND "), params)
    }
}

pub(crate) fn order_by_sql(order: ListOrder) -> &'static str {
    match order {
        ListOrder::Dispatch => "priority DESC, created_at ASC, id ASC",
        ListOrder::UpdatedDesc => "updated_at DESC, id DESC",
        ListOrder::CreatedDesc => "created_at DESC, id DESC",
    }
}

/// `cursor` より後（= 次ページ側）の行だけを選ぶ keyset 述語。`order_by_sql` と対にして使う。
pub(crate) fn keyset_predicate(
    order: ListOrder,
    cursor: &CursorPayload,
) -> (String, Vec<SqlValue>) {
    match order {
        ListOrder::Dispatch => (
            "(priority < ?) OR (priority = ? AND created_at > ?) OR \
             (priority = ? AND created_at = ? AND id > ?)"
                .to_string(),
            vec![
                SqlValue::Integer(cursor.priority),
                SqlValue::Integer(cursor.priority),
                SqlValue::Text(cursor.created_at.clone()),
                SqlValue::Integer(cursor.priority),
                SqlValue::Text(cursor.created_at.clone()),
                SqlValue::Text(cursor.id.clone()),
            ],
        ),
        ListOrder::UpdatedDesc => (
            "(updated_at < ?) OR (updated_at = ? AND id < ?)".to_string(),
            vec![
                SqlValue::Text(cursor.updated_at.clone()),
                SqlValue::Text(cursor.updated_at.clone()),
                SqlValue::Text(cursor.id.clone()),
            ],
        ),
        ListOrder::CreatedDesc => (
            "(created_at < ?) OR (created_at = ? AND id < ?)".to_string(),
            vec![
                SqlValue::Text(cursor.created_at.clone()),
                SqlValue::Text(cursor.created_at.clone()),
                SqlValue::Text(cursor.id.clone()),
            ],
        ),
    }
}

pub(crate) fn parse_status(s: &str) -> Result<Status, StoreError> {
    match s {
        "draft" => Ok(Status::Draft),
        "ready" => Ok(Status::Ready),
        "running" => Ok(Status::Running),
        "blocked" => Ok(Status::Blocked),
        "reviewing" => Ok(Status::Reviewing),
        "done" => Ok(Status::Done),
        "failed" => Ok(Status::Failed),
        "cancelled" => Ok(Status::Cancelled),
        other => Err(StoreError::Invalid(format!(
            "invalid status in tasks table: {other}"
        ))),
    }
}
