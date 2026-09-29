//! SQLite ベースの `TaskStore` 実装。DESIGN.md §5.1 / §4.3 準拠。
//!
//! - `events` テーブルは追記専用（append-only）の正典。このモジュールから
//!   `events` に対して UPDATE/DELETE を発行することはない。
//! - `tasks` テーブルは `Task` 全体を `json` 列に保持しつつ、検索・排他制御に
//!   使う列（`status` / `kind` / `parent_id` / `priority` / `created_at` /
//!   `lease_*`）を非正規化して複製する。
//!
//! 状態機械のロジック（`crate::transition`）には依存しない（意図的な疎結合。
//! Phase 3 のディスパッチャが `transition()` の結果をこのストアに書き戻す）。

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration as StdDuration;

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, params_from_iter};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::comment::{CommentAuthorKind, TaskComment};
use crate::execution_plan::{
    ExecutionPlanRow, ExecutionPlanSpec, PlanOrigin, PlanStatus, RunIndexRole, RunIndexStatus,
    RunRow, WorkUnitBlockedReason, WorkUnitKind, WorkUnitRow, WorkUnitSpec, WorkUnitStatus,
};
use crate::instance::{DaemonInstance, InstanceRole, SELECT_INSTANCE, row_to_instance};
use crate::integrations::{IntegrationId, IntegrationMethod, IntegrationState, TaskIntegration};
use crate::message::{Message, MessageId, MessageRole, is_conversation};
use crate::model::{Event, Status, Task, TaskId, TaskKind, WorkspaceSpec};
use crate::org::{
    Milestone, MilestoneId, MilestoneStatus, OrgError, OrgKind, OrgNode, Project, ProjectId,
    ProjectStatus,
};
use crate::repos::{ProjectRepo, RepoError, RepoId, RepoKind, RepoRun, RepoSync};
use crate::transition::{InvalidTransition, Outcome, StateView, Trigger, transition};

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_events_global_id.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_tasks_list_columns.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_tasks_objective_column.sql");
const MIGRATION_0005: &str = include_str!("../migrations/0005_tasks_genre_column.sql");
const MIGRATION_0006: &str = include_str!("../migrations/0006_organization.sql");
const MIGRATION_0007: &str =
    include_str!("../migrations/0007_messages_task_id_and_reports_project.sql");
const MIGRATION_0008: &str = include_str!("../migrations/0008_notifications.sql");
const MIGRATION_0009: &str = include_str!("../migrations/0009_notifications_project_id.sql");
const MIGRATION_0010: &str = include_str!("../migrations/0010_projects_workspace.sql");
const MIGRATION_0011: &str = include_str!("../migrations/0011_daemon_instances.sql");
/// ADR-0043 D1/D2（Phase 52）: `project_repos` と `tasks.repos_json`。
/// この版を当てた直後に、同じトランザクションで `backfill_project_repos` が写しを作る。
const MIGRATION_0012: &str = include_str!("../migrations/0012_project_repos.sql");
/// ADR-0044 D2/D3（Phase 53）: `task_comments` と `tasks.labels_json` / `tasks.category`。
const MIGRATION_0013: &str = include_str!("../migrations/0013_task_comments.sql");
/// ADR-0043 D5（Phase 54）: `task_integrations`（取り込みの記録）。
const MIGRATION_0014: &str = include_str!("../migrations/0014_task_integrations.sql");
/// ADR-0044 D6（Phase 55）: 案件・途中目標の中止・一時停止・アーカイブ（`archived_at` / `paused_from`）。
const MIGRATION_0015: &str = include_str!("../migrations/0015_lifecycle.sql");
/// ADR-0046 D1/D2/D4（Phase 59）: `org_nodes.profile_json` と `tasks.skills_json` / `tasks.mode`。
const MIGRATION_0016: &str = include_str!("../migrations/0016_org_profiles.sql");
const MIGRATION_0017: &str = include_str!("../migrations/0017_console_actions.sql");
/// ADR-0047 D4（Phase 62）: `knowledge_runs`（知識整理 run を高々 1 回だけ起こすための目印）。
const MIGRATION_0020: &str = include_str!("../migrations/0020_deliveries.sql");
const MIGRATION_0019: &str = include_str!("../migrations/0019_notification_scan.sql");
const MIGRATION_0018: &str = include_str!("../migrations/0018_knowledge_runs.sql");
/// ADR-0052 D2 / D3（Phase 64）: `knowledge_runs.retried_at` と `knowledge_runs.via`。
const MIGRATION_0021: &str = include_str!("../migrations/0021_knowledge_run_retry.sql");
/// ADR-0053 D1（Phase 65）: `llm_proxy_requests`（LLM source ローカルプロキシの要求記録）。
const MIGRATION_0022: &str = include_str!("../migrations/0022_llm_proxy_requests.sql");
/// ADR-0054 D1（Phase 67）: `node_sessions`（ノードごとの継続セッションと resume）。
const MIGRATION_0023: &str = include_str!("../migrations/0023_node_sessions.sql");
/// ADR-0056 D1 / D4（Phase 78）: `mcp_clients` / `mcp_calls`（MCP サーバーの認証とログ）。
const MIGRATION_0024: &str = include_str!("../migrations/0024_mcp.sql");
/// ADR-0059 D6（Phase 99）: `cluster_settings`（クラスタの作業ディレクトリの DB 上書き）。
const MIGRATION_0025: &str = include_str!("../migrations/0025_cluster_settings.sql");
/// ADR-0072 D5/D23（Phase E2）: `execution_plans` / `work_units` / `runs`（Task 下の内部実行層の
/// 派生索引。正本は events。`CREATE TABLE IF NOT EXISTS` だけで既存の表には触れない）。
const MIGRATION_0026: &str = include_str!("../migrations/0026_execution.sql");
/// ADR-0074 D1/§5.2（Phase F2）: `work_units` に v2（並列実行）の phase/lease/branch/commit 列を足す。
const MIGRATION_0027: &str = include_str!("../migrations/0027_parallel_work_units.sql");
/// ADR-0074 D3.2 / D3.8（Phase F4b）: `projects.auto_advance` と `milestones.plan_key`。
const MIGRATION_0028: &str = include_str!("../migrations/0028_project_plan_go.sql");
/// ADR-0044 D7 追記 / ADR-0047 追記（Phase K-1）: `projects.slug`（知識ベースの `projects/<slug>/`）。
const MIGRATION_0029: &str = include_str!("../migrations/0029_project_slug.sql");
/// ADR-0078 D5: `cluster_connection_log`（クラスタの ssh master の接続・切断の記録）。
const MIGRATION_0030: &str = include_str!("../migrations/0030_cluster_connection_log.sql");
/// ADR-0079 D4 / D7 / D15（Phase R1a）: `tasks.root_id`・`work_units.child_task_id` /
/// `needs_decisions_json`・`decisions`（再帰的な task の木。既存の行は書き換えない）。
const MIGRATION_0031: &str = include_str!("../migrations/0031_task_tree.sql");
/// ADR-0080 D4/D5: browser の人待ち（登録依頼・承認）と credential 台帳（秘密なし）。
const MIGRATION_0032: &str = include_str!("../migrations/0032_browser_waits.sql");
const MIGRATION_0033: &str = include_str!("../migrations/0033_browser_task_policies.sql");

/// このバイナリが知っている最新のスキーマ版数（ADR-0013 D5）。DB の版数がこれより大きければ
/// `SqliteStore::open`/`open_with` は `StoreError::SchemaTooNew` で失敗する。
pub const SCHEMA_VERSION: u32 = 33;

/// ADR-0074 D3.4（Phase F4b (e)）: `TaskStore::project_plan_apply` の入力。
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectPlanApply {
    /// `decided_event` を積む Plan タスク（提案の正本）。
    pub plan_task_id: TaskId,
    pub milestones: Vec<ProjectPlanMilestoneChange>,
    /// 書き換える Task の新しい値（状態・attempts・lease はトランザクションの中で読んだ値を使う）。
    pub task_updates: Vec<Task>,
    pub transitions: Vec<(TaskId, Trigger)>,
    pub decided_event: Event,
}

/// `ProjectPlanApply.milestones[]`。`title` / `description` は `Some` のときだけ書き換える。
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectPlanMilestoneChange {
    pub id: MilestoneId,
    pub status: MilestoneStatus,
    pub title: Option<String>,
    pub description: Option<String>,
}

/// ADR-0079 D15（Phase R5b-prep）: 既存の task を木の子として採用する書き換え（`TaskStore::execution_plan_adopt_tree`
/// / `TaskStore::tree_adopt_apply` の入力）。`task` は `tree`（と、`parent_id` が無かったなら `parent_id`）を書いた後の
/// 値。状態・attempts・lease はトランザクションの中で読み直した値を使う（`update_task` と同じ規律）。
#[derive(Debug, Clone, PartialEq)]
pub struct TreeAdoption {
    pub task: Task,
    /// 採用を判断したときの対象の状態（トランザクションの中で変わっていれば何も書かない）。
    pub expect_status: Status,
    /// 対象の events に積む 1 件（`Event::Edited{fields: ["tree", ...], by}`）。
    pub event: Event,
}

/// `SqliteStore::open_with` に渡す接続オプション（ADR-0013 D5、ADR-0064 D1/D4/D5）。
#[derive(Debug, Clone, Copy)]
pub struct StoreOptions {
    /// `PRAGMA busy_timeout`。複数接続（ディスパッチャ・API・celerisctl）が同じファイルを
    /// 開くときにロック待ちする時間。既定は 5000 ms（本番では 15000 ms を勧める。ADR-0064 D1）。
    pub busy_timeout: StdDuration,
    /// ADR-0064 D4: ファイル DB のとき、読み取り専用（`query_only=ON`）の小さな接続プールを
    /// この本数だけ作る（既定 2）。`0` ならプールを作らず、読み取りも書き込み接続を使う
    /// （従来どおり）。インメモリ DB では接続間で状態を共有できないため常にプールを作らない。
    pub read_pool_size: usize,
    /// ADR-0064 D5: `true` なら `wal_autocheckpoint=0` にし、`journal_size_limit` を設定する
    /// （チェックポイントは背景の専用接続がタイマーで行う想定。デーモンの接続だけ立てる。
    /// `celerisctl` 等デーモン外の接続は既定の `false` のまま）。
    pub background_checkpoint: bool,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self {
            busy_timeout: StdDuration::from_millis(5000),
            read_pool_size: 2,
            background_checkpoint: false,
        }
    }
}

/// ADR-0064 D5: `background_checkpoint` のときに設定する `PRAGMA journal_size_limit`（64 MB）。
/// WAL がこれを超えたら次のバックグラウンド・チェックポイントが `TRUNCATE` を試みる。
const BACKGROUND_CHECKPOINT_JOURNAL_SIZE_LIMIT: i64 = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("time formatting error: {0}")]
    TimeFormat(#[from] time::error::Format),
    #[error("time parsing error: {0}")]
    TimeParse(#[from] time::error::Parse),
    #[error("mutex poisoned")]
    Poisoned,
    #[error("invalid stored data: {0}")]
    Invalid(String),
    #[error(transparent)]
    InvalidTransition(#[from] InvalidTransition),
    /// ADR-0013 D5: DB の `schema_migrations` の最大版数がこのバイナリの `SCHEMA_VERSION` より
    /// 大きい場合に返す。DB を書き換えずに `open`/`open_with` を失敗させる。
    #[error("db schema version {found} is newer than the {supported} this binary supports")]
    SchemaTooNew { found: u32, supported: u32 },
    /// ADR-0033 D1: 使用中のため消せない（組織のノードが未終了のタスクを抱えている）。API は 409。
    #[error("{kind} {id} is still in use: {detail}")]
    InUse {
        kind: &'static str,
        id: String,
        detail: String,
    },
    /// ADR-0033 D1: 組織の検証に落ちた（API は 422）。
    #[error(transparent)]
    Org(#[from] OrgError),
    /// ADR-0043 D1（Phase 52）: 案件のリポジトリの検証に落ちた（API は 422）。
    #[error(transparent)]
    Repo(#[from] RepoError),
}

/// ADR-0070 D5（Phase 116）: `SQLITE_BUSY` / `SQLITE_LOCKED` かどうか（純粋関数。I/O は無い）。
/// `renew_lease` のような「失敗しても run を止めたくない」呼び出し側が、一時的な DB busy と
/// それ以外のエラーを区別してリトライするために使う。
pub fn is_busy_error(e: &StoreError) -> bool {
    matches!(
        e,
        StoreError::Sqlite(rusqlite::Error::SqliteFailure(inner, _))
            if matches!(
                inner.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    )
}

/// ADR-0046 D1（Phase 59）: `org_nodes.profile_json` に書く値。空の profile は NULL
/// （導入前のノードの行と 1 バイトも変わらない）。
fn profile_json(profile: &crate::profile::Profile) -> Result<Option<String>, StoreError> {
    if profile.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(profile)?))
}

/// ADR-0046 D7: ノード id を持つ表と列（`celerisctl org migrate-v2` が触ってよいものだけ）。
fn is_known_node_ref(table: &str, column: &str) -> bool {
    matches!(
        (table, column),
        ("tasks", "assignee")
            | ("messages", "node_id")
            | ("reports", "node_id")
            | ("approvals", "node_id")
            | ("standing_rules", "node_id")
    )
}

/// ADR-0046 D7: 1 行のノード id を書き換える。`tasks` は `json`（正本）も直す。
fn set_node_ref(
    tx: &Connection,
    table: &str,
    column: &str,
    id: &str,
    value: &str,
) -> Result<(), StoreError> {
    if table == "tasks" {
        let json: Option<String> = tx
            .query_row("SELECT json FROM tasks WHERE id = ?1", params![id], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(json) = json else { return Ok(()) };
        let mut task: Task = serde_json::from_str(&json)?;
        task.assignee = Some(value.to_string());
        let updated = serde_json::to_string(&task)?;
        tx.execute(
            "UPDATE tasks SET assignee = ?1, json = ?2 WHERE id = ?3",
            params![value, updated, id],
        )?;
        return Ok(());
    }
    let sql = format!("UPDATE {table} SET {column} = ?1 WHERE id = ?2");
    tx.execute(&sql, params![value, id])?;
    Ok(())
}

/// `events` テーブルの 1 行（ADR-0013 D6）。`id` はテーブル全体でのグローバル単調増加値。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EventRow {
    pub id: u64,
    pub task_id: TaskId,
    pub seq: u64,
    /// DB に保存された RFC 3339 文字列そのまま。
    pub ts: String,
    pub event: Event,
}

/// 生成した `EventRow` の JSON Schema（`serde_json::Value`）。`docs/api/v1/event.schema.json` と
/// 一致することを `event_row_schema_matches_committed` で検証する。
pub fn event_row_schema_value() -> serde_json::Value {
    let schema = schemars::schema_for!(EventRow);
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

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
struct CursorPayload {
    priority: i64,
    created_at: String,
    updated_at: String,
    id: String,
}

impl CursorPayload {
    fn from_task(task: &Task) -> Result<Self, StoreError> {
        Ok(Self {
            priority: task.priority as i64,
            created_at: format_rfc3339(task.created_at)?,
            updated_at: format_rfc3339(task.updated_at)?,
            id: task.id.to_string(),
        })
    }
}

fn encode_cursor(payload: &CursorPayload) -> Result<String, StoreError> {
    let json = serde_json::to_vec(payload)?;
    let mut out = String::with_capacity(json.len() * 2);
    for byte in json {
        out.push_str(&format!("{byte:02x}"));
    }
    Ok(out)
}

fn decode_cursor(cursor: &str) -> Result<CursorPayload, StoreError> {
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

fn usize_to_i64(v: usize) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn u64_to_i64(v: u64) -> i64 {
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
fn filter_predicate(filter: &ListFilter) -> (String, Vec<SqlValue>) {
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

fn order_by_sql(order: ListOrder) -> &'static str {
    match order {
        ListOrder::Dispatch => "priority DESC, created_at ASC, id ASC",
        ListOrder::UpdatedDesc => "updated_at DESC, id DESC",
        ListOrder::CreatedDesc => "created_at DESC, id DESC",
    }
}

/// `cursor` より後（= 次ページ側）の行だけを選ぶ keyset 述語。`order_by_sql` と対にして使う。
fn keyset_predicate(order: ListOrder, cursor: &CursorPayload) -> (String, Vec<SqlValue>) {
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

fn parse_status(s: &str) -> Result<Status, StoreError> {
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

/// `GET /metrics/execution` 用の、索引から取得した最新 run の情報。
/// `metrics_json` は RunMetrics であり、WorkerFinished の `end`（予算切れの種類）は含まない。
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionMetricsLatestRun {
    pub run_id: String,
    pub role: RunIndexRole,
    pub status: RunIndexStatus,
    pub adapter: Option<String>,
    pub model: Option<String>,
    pub metrics_json: Option<String>,
    pub usage_json: Option<String>,
}

/// `GET /metrics/execution` 用のタスク別の索引集計行。`routing_json` は `tasks.json.routing`。
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionMetricsTaskRow {
    pub task_id: TaskId,
    pub status: Status,
    pub genre: Option<String>,
    pub assignee: Option<String>,
    pub routing_json: Option<String>,
    pub repairs: u32,
    pub replans: u32,
    pub continuations: u32,
    pub retries: u32,
    pub runs_count: u32,
    pub budget_exhausted_runs: u32,
    pub latest_run: Option<ExecutionMetricsLatestRun>,
    /// 索引が無い旧タスクでも実行履歴があるか。Created だけのタスクは false。
    pub has_execution_events: bool,
    /// WorkerFinished.end の BudgetExhausted。runs.status と異なる場合も events が正。
    pub has_budget_events: bool,
    /// atomic の continue/retry は work_units に記録されない。
    pub has_transition_metrics: bool,
    /// ADR-0074 D4.3（Phase F3 quota）: `Event::QuotaEstimated` を 1 件でも持つ
    /// （`ExecutionMetrics.quota`/`cost_usd_complete` を求めるには events を読む必要がある）。
    pub has_quota_events: bool,
}

/// ADR-0033 D3: 報告（`reports`）の読み書きは `crate::report::ReportStore` にあり、`TaskStore` はそれを
/// supertrait として要求する（ディスパッチャの `Arc<dyn TaskStore>` から報告を追記できるようにするため。
/// 実装は `report.rs` にあり、この表の SQL はここには無い）。
/// ADR-0033 D5（Phase 26）: 認可（`approvals` / `standing_rules`）も同じ形で `crate::approval::ApprovalStore`
/// にある。
/// ADR-0037 D1（Phase 39）: 通知の台帳（`notifications`）も同じ形で `crate::notify::NotificationStore` にある。
pub trait TaskStore:
    Send
    + Sync
    + crate::report::ReportStore
    + crate::approval::ApprovalStore
    + crate::notify::NotificationStore
    + crate::knowledge_run::KnowledgeRunStore
    + crate::delivery::DeliveryStore
    + crate::node_session::NodeSessionStore
    + crate::mcp::McpClientStore
    + crate::mcp::McpCallStore
    + crate::browser_wait::BrowserWaitStore
{
    fn insert(&self, task: &Task) -> Result<(), StoreError>;
    fn get(&self, id: TaskId) -> Result<Option<Task>, StoreError>;
    fn list(&self, filter: Option<Status>) -> Result<Vec<Task>, StoreError>;
    /// イベントを追記し、割り当てられた `seq`（0始まり、task_id 内で単調増加）を返す。
    fn append_event(&self, task_id: TaskId, event: &Event) -> Result<u64, StoreError>;
    /// `task_id` に紐づく全イベントを `seq` 昇順で返す。**`seq` はタスクごとに 0 始まりで単調増加する
    /// ローカルな連番**であり、別のタスクの `seq` と大小比較することはできない（Phase 45 実機バグ:
    /// ADR-0021 D3 の「一度扱った失敗は数え直さない」判定で、親の `seq` と子の `seq` を比較していたため、
    /// 子の失敗が毎回「新規」と判定され、同じ質問が繰り返し出た）。タスクをまたいでイベントの前後関係を
    /// 比較したい場合は `events_for_with_global_ids` を使うこと。
    fn events_for(&self, task_id: TaskId) -> Result<Vec<(u64, Event)>, StoreError>;
    /// `task_id` に紐づく全イベントを、`events` テーブルの**グローバルに単調増加する id**（`seq` ではない）
    /// と一緒に id 昇順で返す（Phase 45）。この id は全タスクを横断して単調増加するため、
    /// 異なるタスクのイベント同士の前後関係を比較してよい（`events_for` の `seq` はタスクごとにローカルなので
    /// 比較できない）。
    fn events_for_with_global_ids(&self, task_id: TaskId) -> Result<Vec<(u64, Event)>, StoreError>;
    /// 排他的にリースを取得する。成功したら true を返し、task の status を Running にし、
    /// lease = Some{worker_run_id, expires_at: now + ttl} をDBに書く。
    /// 既にリースされている／status != Ready の場合は false を返す（エラーではない）。
    fn acquire_lease(
        &self,
        task_id: TaskId,
        worker_run_id: &str,
        ttl: StdDuration,
    ) -> Result<bool, StoreError>;
    /// worker_run_id が現在のリースと一致する場合のみ lease を None にする。status は変更しない。
    fn release_lease(&self, task_id: TaskId, worker_run_id: &str) -> Result<(), StoreError>;
    /// status=Ready かつ depends_on が全て Done かつ（親が存在し kind=Approval の場合は親が Done）
    /// を満たすタスクを priority DESC, created_at ASC で最大 limit 件返す。
    fn ready_tasks(&self, limit: usize) -> Result<Vec<Task>, StoreError>;
    /// ADR-0079 D13（Phase R5a）: `task` が subtree の一時停止（自分か祖先の `paused_at`）か、祖先の案件の
    /// 停止（paused / cancelled / archived）で止まっているか。`ready_tasks` と同じ判定を、既に `running` の
    /// task の並列 WU の 2 本目以降（`dispatch_parallel_work_units`）にも当てるために出す。対話は常に `false`。
    fn halted_by_pause(&self, task: &Task) -> Result<bool, StoreError>;
    /// `transition::transition()` で検証した任意のトリガーを適用する汎用の書き込み口
    /// （ADR-0004 D1）。タスクの取得・`transition()` の呼び出し・`tasks` 行の更新・
    /// `Event::Transitioned` の追記（と任意の `extra_event`）を単一トランザクションで行う。
    /// タスクが存在しない場合は `StoreError::Invalid`、遷移が無効な場合は
    /// `StoreError::InvalidTransition` を返し、いずれもタスクの状態は変更しない。
    fn apply_transition(
        &self,
        task_id: TaskId,
        trigger: Trigger,
        extra_event: Option<Event>,
    ) -> Result<Outcome, StoreError> {
        self.apply_transition_with_events(task_id, trigger, extra_event.into_iter().collect())
    }
    /// `apply_transition` の一般形（ADR-0005 D4）。`extra_events` を順に、`Event::Transitioned`
    /// の直後に同一トランザクションで追記する。ディスパッチャが `WorkerFinished` や
    /// 条件ごとの `ReviewVerdict` を遷移と原子的に記録するために使う。
    fn apply_transition_with_events(
        &self,
        task_id: TaskId,
        trigger: Trigger,
        extra_events: Vec<Event>,
    ) -> Result<Outcome, StoreError>;

    /// ADR-0007 D3: Plan の子タスク群を挿入（`Created`、`accept_children` なら続けて `Accept`）し、
    /// 親に `ReviewPass` を適用して `verdict_events` を追記する。全体が 1 トランザクション。
    fn complete_plan(
        &self,
        plan_id: TaskId,
        verdict_events: Vec<Event>,
        children: Vec<Task>,
        accept_children: bool,
    ) -> Result<Outcome, StoreError>;

    /// ADR-0010 D2: `insert` + `Event::Created` + `extra_events` を 1 トランザクションで行う。
    fn create_task(&self, task: &Task, extra_events: Vec<Event>) -> Result<(), StoreError>;

    /// ADR-0074 D3.3（Phase F4a (c)）: 案件計画の提案の決定を **1 トランザクションで**適用する。
    /// `milestones` を全て `milestone_status` にし、`tasks` のうちまだ `draft` のもの（`Trigger::Cancel`
    /// のカスケードで既に終端になったもの等は飛ばす）に `trigger` を適用し、最後に `decided_event`
    /// を `plan_task_id` の events に積む。途中で失敗すれば何も書かない。無い途中目標・Task は
    /// `StoreError::Invalid`。
    fn project_plan_decide_apply(
        &self,
        plan_task_id: TaskId,
        milestones: &[MilestoneId],
        milestone_status: MilestoneStatus,
        tasks: &[TaskId],
        trigger: Trigger,
        decided_event: Event,
    ) -> Result<(), StoreError>;

    /// ADR-0074 D3.4（Phase F4b (e)）: 案件計画の決定（replan の差分を含む）を **1 トランザクションで**
    /// 適用する。順に: 途中目標の状態・題名の更新 → Task の書き換え（`draft` / `ready` で lease の無い
    /// ものだけ。dispatch 済みなら `StoreError::Invalid` で何も書かない）→ 遷移（`Accept` は `draft` の
    /// ときだけ、それ以外は終端でないときだけ適用。`Cancel` のカスケードで先に終端になったものは飛ばす）
    /// → `decided_event` を `plan_task_id` の events に積む。
    fn project_plan_apply(&self, apply: &ProjectPlanApply) -> Result<(), StoreError>;

    /// ADR-0016 D2 / M2: 実行中の委譲。子タスク群を挿入（`Created` → `Accept` で `ready`）し、親に
    /// `Event::Delegated{run_id, task_ids}` を追記する。全体が 1 トランザクション。親の状態は変えない。
    /// 子の `parent_id` が `parent_id` と違えば `StoreError::Invalid`。
    fn delegate_children(
        &self,
        parent_id: TaskId,
        run_id: &str,
        children: Vec<Task>,
    ) -> Result<Vec<TaskId>, StoreError>;

    /// `parent_id` を親に持つタスク（終端を含む）を `created_at` 昇順（同時刻は挿入順）で返す（ADR-0016 M5 / M6）。
    fn children(&self, parent_id: TaskId) -> Result<Vec<Task>, StoreError>;

    /// ADR-0010 D2 / D7（P-7）: `status = running` かつリースの run_id が一致するときだけ `expires_at = now + ttl` に
    /// 延長して true を返す。状態遷移ではないのでイベントは追記しない。
    fn renew_lease(
        &self,
        task_id: TaskId,
        worker_run_id: &str,
        ttl: StdDuration,
    ) -> Result<bool, StoreError>;

    /// ADR-0013 D6: `events` を `id` 昇順で `after_id` より後、最大 `limit` 件返す。
    fn events_since(&self, after_id: u64, limit: usize) -> Result<Vec<EventRow>, StoreError>;
    /// ADR-0013 D6: `events` の現在の最大 `id`。行が無ければ 0。
    fn latest_event_id(&self) -> Result<u64, StoreError>;
    /// ADR-0013（Phase 9b）: 1 タスクのイベントを `seq` 昇順で、`after_seq` より後（`None` なら最初から）最大 `limit` 件、
    /// グローバル `id` と `ts` 付きで返す（API の `GET /tasks/{id}/events` と run の要約が使う）。
    fn event_rows_for(
        &self,
        task_id: TaskId,
        after_seq: Option<u64>,
        limit: usize,
    ) -> Result<Vec<EventRow>, StoreError>;

    /// ADR-0013 D10: `filter` に一致する `tasks` を `order` で keyset ページングして返す。`cursor` は
    /// 前回の `Page::next_cursor`（不透明な文字列）。不正な `cursor` は `StoreError::Invalid`。
    fn list_page(
        &self,
        filter: &ListFilter,
        order: ListOrder,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<Task>, StoreError>;
    /// ADR-0013 D10: `tasks` の件数を `status` ごとに集計する（0 件の status は含まない）。
    fn count_by_status(&self) -> Result<Vec<(Status, u64)>, StoreError>;

    // ---- ADR-0033 D1: 組織（`org_nodes`）。DB が正で、設定は空のときの種蒔きにしか使わない ----

    /// 全ノードを `position`、同値なら `id` の昇順で返す（木は呼び出し側が `parent_id` で組む）。
    fn org_list(&self) -> Result<Vec<OrgNode>, StoreError>;
    /// 1 ノード。無ければ `None`。
    fn org_get(&self, id: &str) -> Result<Option<OrgNode>, StoreError>;
    /// 挿入または更新。`crate::org::validate_upsert` を通してから書く（secretary は 1 つ、親は既存、
    /// 自分を祖先にできない、secretary > department > section）。`created_at` は既存行のものを保つ。
    fn org_upsert(&self, node: &OrgNode) -> Result<OrgNode, StoreError>;
    /// 監査 D-4: 設定からの種蒔き専用。`nodes` を渡された順に検証しながら**1 トランザクション**で書く
    /// （後の要素は前の要素を `existing` に含めて検証できるので、`secretary` → `department` → `section`
    /// の順に並んでいれば通る）。途中の 1 件でも `crate::org::validate_upsert` に落ちたら、それより前の
    /// 分も含めて何も書かない（部分的に蒔かれた組織が残ると、次回起動時は `org_list` が空でなくなり
    /// 補完されないため）。
    fn org_seed(&self, nodes: &[OrgNode]) -> Result<(), StoreError>;
    /// 削除。そのノードを `assignee` に持つ未終了タスクがあれば `StoreError::InUse`、
    /// 子ノードがあっても `StoreError::InUse`（木を宙ぶらりんにしない）。無い id は `Ok(false)`。
    fn org_delete(&self, id: &str) -> Result<bool, StoreError>;

    // ---- ADR-0033 D2: 案件（`projects`）と途中目標（`milestones`）----

    /// 案件を作る（`status` は呼び出し側が決める。API は `proposed`）。
    fn project_create(&self, project: &Project) -> Result<(), StoreError>;
    fn project_get(&self, id: ProjectId) -> Result<Option<Project>, StoreError>;
    /// `created_at` の降順（新しい案件が先）。
    fn project_list(&self) -> Result<Vec<Project>, StoreError>;
    /// 状態だけを変える（`updated_at` も更新）。無い案件は `Ok(false)`。
    fn project_set_status(&self, id: ProjectId, status: ProjectStatus) -> Result<bool, StoreError>;
    /// ADR-0039 D1: 作業場所だけを変える（`None` で消す。`updated_at` も更新）。無い案件は `Ok(false)`。
    fn project_set_workspace(
        &self,
        id: ProjectId,
        workspace: Option<&WorkspaceSpec>,
    ) -> Result<bool, StoreError>;
    /// ADR-0044 D6（Phase 55）: 状態と `paused_from` を**同時に**書く（`pause` / `resume` / `cancel`）。
    /// `paused_from` は `Some(None)` で消し、`None` なら触らない。無い案件は `Ok(false)`。
    fn project_set_lifecycle(
        &self,
        id: ProjectId,
        status: ProjectStatus,
        paused_from: Option<Option<ProjectStatus>>,
    ) -> Result<bool, StoreError>;
    /// ADR-0044 D6（Phase 55）: アーカイブの時刻を書く（`None` で解除）。無い案件は `Ok(false)`。
    fn project_set_archived_at(
        &self,
        id: ProjectId,
        at: Option<OffsetDateTime>,
    ) -> Result<bool, StoreError>;
    /// ADR-0044 D7 追記（Phase K-1）: `projects.slug` を書く。綴りが違えば `StoreError::Invalid`
    /// （API は 422）、他の案件が使っていれば `StoreError::InUse`（409）。無い案件は `Ok(false)`。
    /// **知識ベースのディレクトリは動かさない**（`projects/<旧>/` を `projects/<新>/` へ動かすのは人）。
    fn project_set_slug(&self, id: ProjectId, slug: &str) -> Result<bool, StoreError>;
    /// ADR-0072「Phase F6 実装時の決定」P3: 案件の題名（`title`）と説明（依頼文 `request`）を
    /// 人が書き換える。`None` の欄は変えない。検証（空でない・長さの上限）は呼び出し側（API）が行う。
    /// 無い案件は `Ok(false)`。
    fn project_set_text(
        &self,
        id: ProjectId,
        title: Option<&str>,
        request: Option<&str>,
    ) -> Result<bool, StoreError>;

    // ---- ADR-0043 D1（Phase 52）: 案件のリポジトリ（`project_repos`）----

    /// 案件のリポジトリを 1 件作る。`repos::validate_upsert` に落ちれば `StoreError::Repo`（API は 422）。
    /// `is_primary` を立てた行を作ると、同じ案件の他の行の `is_primary` は落ちる（1 案件に 1 つ）。
    fn repo_create(&self, repo: &ProjectRepo) -> Result<(), StoreError>;
    fn repo_get(&self, id: RepoId) -> Result<Option<ProjectRepo>, StoreError>;
    /// その案件のリポジトリ（primary が先、あとは作った順）。
    fn repo_list(&self, project_id: ProjectId) -> Result<Vec<ProjectRepo>, StoreError>;
    /// 既存の 1 件を差し替える（`id` と `project_id` は変えない）。無い id は `Ok(false)`。
    fn repo_update(&self, repo: &ProjectRepo) -> Result<bool, StoreError>;
    /// 消す。未終端のタスクが参照していれば `StoreError::InUse`（API は 409）。無い id は `Ok(false)`。
    fn repo_delete(&self, id: RepoId) -> Result<bool, StoreError>;
    /// その案件の primary をこの行にする。無い id は `Ok(false)`。
    fn repo_set_primary(&self, id: RepoId) -> Result<bool, StoreError>;
    /// そのリポジトリを参照している未終端のタスク（`DELETE` の 409 の理由に使う）。
    fn repo_active_tasks(&self, id: RepoId) -> Result<Vec<TaskId>, StoreError>;

    // ---- ADR-0043 D5（Phase 54）: 変更の取り込み（`task_integrations`）----

    /// 取り込みの記録を 1 件書く（同じ `id` があれば差し替える。`updated_at` は呼び出し側が入れる）。
    /// **人（管理系 API）だけが呼ぶ**。組織の「人」がここに届く経路は無い（SPEC §3.6）。
    fn integration_put(&self, integration: &TaskIntegration) -> Result<(), StoreError>;
    fn integration_get(&self, id: IntegrationId) -> Result<Option<TaskIntegration>, StoreError>;
    /// そのタスクの記録（新しい順）。ADR-0044 B1 の timeline はこれを読めばよい。
    fn integration_list_for_task(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<TaskIntegration>, StoreError>;
    /// そのタスクのそのリポジトリの**最新の** 1 件（`GET /tasks/{id}/changes` の `integration`）。
    fn integration_latest(
        &self,
        task_id: TaskId,
        repo: &str,
    ) -> Result<Option<TaskIntegration>, StoreError>;
    /// 案件のタスクの取り込み（**タスク × リポジトリごとに最新の 1 件**。新しい順、`limit` 件まで）。
    /// 案件画面の「PR と取り込み」（ADR-0043 D5）。
    fn integration_list_for_project(
        &self,
        project_id: ProjectId,
        limit: usize,
    ) -> Result<Vec<TaskIntegration>, StoreError>;

    /// 途中目標を作る。`seq` はその案件の最大 + 1 をストアが採番し、確定した行を返す。
    /// 案件が無ければ `StoreError::Invalid`。
    fn milestone_create(
        &self,
        project_id: ProjectId,
        title: &str,
        description: &str,
        status: MilestoneStatus,
    ) -> Result<Milestone, StoreError>;
    /// ADR-0074 D3.3（Phase F4b）: 案件計画から作った途中目標に key を結ぶ（`milestones.plan_key`）。
    /// 無い途中目標は `Ok(false)`。
    fn milestone_set_plan_key(&self, id: MilestoneId, plan_key: &str) -> Result<bool, StoreError>;
    /// その案件の途中目標を `seq` 昇順で返す。
    fn milestone_list(&self, project_id: ProjectId) -> Result<Vec<Milestone>, StoreError>;
    /// ADR-0044 D6（Phase 55）: 途中目標を id 1 つで引く（案件を知らなくてよい）。
    fn milestone_get(&self, id: MilestoneId) -> Result<Option<Milestone>, StoreError>;
    /// 状態だけを変える。無い途中目標は `Ok(false)`。
    fn milestone_set_status(
        &self,
        id: MilestoneId,
        status: MilestoneStatus,
    ) -> Result<bool, StoreError>;
    /// 現在の状態が `from` のときだけ更新する。dispatch と自動到達が人の状態変更を上書きしないために使う。
    fn milestone_transition_status(
        &self,
        id: MilestoneId,
        from: MilestoneStatus,
        to: MilestoneStatus,
    ) -> Result<bool, StoreError>;
    /// ADR-0044 D6（Phase 55）: 状態と `paused_from` を**同時に**書く（`pause` / `resume` / `cancel`）。
    /// `paused_from` は `Some(None)` で消し、`None` なら触らない。無い途中目標は `Ok(false)`。
    fn milestone_set_lifecycle(
        &self,
        id: MilestoneId,
        status: MilestoneStatus,
        paused_from: Option<Option<MilestoneStatus>>,
    ) -> Result<bool, StoreError>;

    // ---- ADR-0033 D4: 対話（`messages`）----

    /// 1 行を追記する（対話は追記専用。更新も削除もしない）。
    fn message_append(&self, message: &Message) -> Result<(), StoreError>;
    /// `node_id` とのやり取りを**古い順**（`created_at` 昇順、同時刻は `id` 昇順）で最大 `limit` 件返す。
    /// `project_id` が `Some` ならその案件の行だけ、`None` なら案件に紐づかない行だけ（雑談）。
    /// 件数が `limit` を超えるときは**新しい方**を残す（直近のやり取りを渡すため）。
    fn message_list(
        &self,
        node_id: &str,
        project_id: Option<ProjectId>,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError>;

    /// ADR-0048 D1（Phase 60a）: Console の一本の流れ用。`message_list` と違い **絞り込みは任意**で、
    /// `node_id` / `project_id` が `None` なら「その軸では絞らない」（`message_list` の `project_id: None`
    /// は「案件に紐づかない行だけ」なので意味が違う）。
    ///
    /// `after` は RFC 3339 の `created_at`。`Some` なら **その時刻以降（`>=`、閉区間）を古い順**に最大
    /// `limit` 件（同じ時刻に複数行あっても取りこぼさないため閉区間。呼び出し側がカーソルで重複を落とす）。
    /// `None` なら **いちばん新しい `limit` 件**を古い順に返す（Console の初期表示）。
    fn message_page(
        &self,
        node_id: Option<&str>,
        project_id: Option<ProjectId>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError>;

    // ---- ADR-0048 D3（Phase 60b）: CoS の actions の冪等性 ----

    /// この `run_id` の actions をまだ実行していなければ記録して `true`、既に実行済みなら
    /// 何もせず `false`（`console_action_runs.run_id` は一意。ADR-0048 D3「Idempotent per run」）。
    fn console_action_run_claim(
        &self,
        run_id: &str,
        task_id: TaskId,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError>;

    // ---- Phase 31: 失敗した仕事をやり直す（実機の事故、2026-09-18）----

    /// `original` は `failed` または `cancelled` でなければ `StoreError::InvalidTransition`（`trigger = "retry"`）。
    /// `new_task` を挿入し（`Event::Created` + `Event::Retried{from: original}`)、`original` に依存していた
    /// 「未終端」または「`dependency_failed` で `cancelled` になった」タスクの `depends_on` を `new_task.id` に
    /// 張り替える（後者は `draft` に戻し、`Event::Transitioned{from: cancelled, to: draft, reason: "retried"}`
    /// を追記する）。全体を単一トランザクションで行い、張り替えたタスクの id を返す。
    fn retry_task(&self, original: TaskId, new_task: &Task) -> Result<Vec<TaskId>, StoreError>;

    // ---- ADR-0044 D1/D2（Phase 53）: 人の編集とタスク単位のコメント ----

    /// ADR-0044 D1: 人の編集を書き戻す（`json` と絞り込みの列、`Event::Edited` を単一トランザクションで）。
    /// **状態機械は通らない**: `status` / `attempts` / `lease` は渡された `task` の値を**使わず**、
    /// トランザクションの中で読み直した現在の行の値を書く（編集を組み立てている間にディスパッチャが
    /// リースを取っていても、その run を壊さないため。`acquire_lease` / `release_lease` と同じ規律）。
    /// 書き込んだ後の `Task`（= 読み直した状態を持つもの）を返す。無いタスクは `StoreError::Invalid`。
    fn update_task(&self, task: &Task, event: Event) -> Result<Task, StoreError>;

    /// ADR-0044 D2: コメントを 1 件追記する。`transition` が `Some((trigger, extra_events))` なら
    /// **同じトランザクション**で状態遷移も行い、その結果を返す（人のコメントの割り込みが
    /// 「コメントは残ったが run は止まらなかった」状態にならないようにするため）。
    fn comment_add(
        &self,
        comment: &TaskComment,
        transition: Option<(Trigger, Vec<Event>)>,
    ) -> Result<Option<Outcome>, StoreError>;

    /// ADR-0044 D2: そのタスクのコメントを古い順（`created_at`、同時刻は `id` 昇順）で返す。
    fn comments_for(&self, task_id: TaskId) -> Result<Vec<TaskComment>, StoreError>;

    // ---- ADR-0040 D4（Phase 47）: celeris のインスタンスの役割（`daemon_instances`）----
    //
    // ここにあるのは「行を読み書きする」だけの操作で、役割を決める規則（誰が active になるか、いつ
    // drain するか）は celeris 側（`celeris::instance`）にある。`verify` のインスタンスはこの表に触れない。

    /// 自分の行を作る（既にあれば上書きする＝同じ `instance_id` で起動し直したとき）。
    /// `handoff_requested_at` / `drained_at` は NULL に戻る。
    fn instance_register(&self, instance: &DaemonInstance) -> Result<(), StoreError>;
    /// 自分の行の `heartbeat_at` を更新する。行が無ければ `Ok(false)`（呼び出し側は登録し直す）。
    fn instance_heartbeat(&self, instance_id: &str, at: OffsetDateTime)
    -> Result<bool, StoreError>;
    /// `instance_id` の行に `handoff_requested_at` を書く（既に入っていれば**上書きしない**。
    /// 引き継ぎの要求は 1 回だけ）。書いたら `Ok(true)`。
    fn instance_request_handoff(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError>;
    /// 役割を変える（`heartbeat_at` も同時に更新する）。行が無ければ `Ok(false)`。
    fn instance_set_role(
        &self,
        instance_id: &str,
        role: InstanceRole,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError>;
    /// 手元の run が 0 になったので `drained_at` を書く（役割は `draining` のまま）。行が無ければ `Ok(false)`。
    fn instance_mark_drained(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError>;
    /// 全インスタンスを `started_at` 昇順（同時刻は `instance_id` 昇順）で返す。
    fn instance_list(&self) -> Result<Vec<DaemonInstance>, StoreError>;
    /// 1 行消す。無い id は `Ok(false)`。
    fn instance_delete(&self, instance_id: &str) -> Result<bool, StoreError>;
    /// 終わった・死んだ他のインスタンスの行を消す（`keep` は消さない）。対象は `drained_at` が入っている
    /// 行と、`heartbeat_at` が `heartbeat_before` より古い行。消した `instance_id` を昇順で返す。
    fn instance_delete_stale(
        &self,
        keep: &str,
        heartbeat_before: OffsetDateTime,
    ) -> Result<Vec<String>, StoreError>;

    // ---- ADR-0059 D6（Phase 99）: クラスタの作業ディレクトリの DB 上書き ----

    /// 1 クラスタ分の上書き。無ければ `Ok(None)`（設定ファイルの値を使う）。
    fn cluster_settings_get(&self, cluster_id: &str)
    -> Result<Option<ClusterSettings>, StoreError>;
    /// 全クラスタの上書き一覧（`cluster_id` 昇順。`GET /clusters` が一括で使う）。
    fn cluster_settings_list(&self) -> Result<Vec<ClusterSettings>, StoreError>;
    /// `work_dir = Some(..)` なら upsert、`None` なら行を消す（上書きの解除）。
    fn cluster_settings_set(
        &self,
        cluster_id: &str,
        work_dir: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<(), StoreError>;
    /// ADR-0078 D5: クラスタの接続・切断・鍵認証の試みを 1 行追記する。
    fn cluster_connection_record(&self, record: &ClusterConnectionRecord)
    -> Result<(), StoreError>;
    /// ADR-0078 D5: `since` 以降（含む）の記録（古い順）。`GET /clusters` の `stats` が数える。
    fn cluster_connection_list_since(
        &self,
        since: OffsetDateTime,
    ) -> Result<Vec<ClusterConnectionRecord>, StoreError>;

    // ---- ADR-0072 D5（Phase E2）: 実行層の派生索引（execution_plans / work_units / runs）----
    //
    // 正本は `events`（`Event::ExecutionPlanned` / `WorkUnitTransitioned` / `WorkerStarted` /
    // `WorkerFinished` / `CheckpointSaved`）。この 3 表は対応する Event と同じトランザクションで
    // 書く。`task_ops::execution::rebuild_from_events` で events から作り直して一致を確かめられる。

    /// D14: 新しい計画を採用する（`Event::ExecutionPlanned` と `execution_plans` / `work_units` の
    /// 行を同じトランザクションで書く）。E2 は新規のみ: そのタスクに既に `active` な計画があれば
    /// `StoreError::InUse`（replan は E4。`PlanOrigin::Repair` 等で明示的に旧版を `superseded` にした
    /// 上で採用する経路は別に用意する）。
    /// `extra_events`（ADR-0074 D2.1（Phase F3 途中確認）: `Event::PausePointsResolved` 用）は
    /// `event`（`ExecutionPlanned`）より先に、同じトランザクションで書く（`execution_plan_replan` と
    /// 同じ規律）。
    fn execution_plan_adopt(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
    ) -> Result<(), StoreError>;

    /// ADR-0074 D3.7（Phase F4b (f)）: `execution_plan_adopt` と同じことを行い、**同じトランザクションで**
    /// planner の `children` から作った子 Task（`parent_id = task_id`、draft なら `Accept`）を挿入し、
    /// `Event::Delegated{run_id: <planner run>, task_ids}` を親に残す（`delegate_children` と同じ形）。
    #[allow(clippy::too_many_arguments)]
    fn execution_plan_adopt_delegating(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        run_id: &str,
        children: Vec<Task>,
    ) -> Result<Vec<TaskId>, StoreError>;

    /// そのタスクの `active` な計画（無ければ `None`）。
    fn execution_plan_active(
        &self,
        task_id: TaskId,
    ) -> Result<Option<ExecutionPlanRow>, StoreError>;
    /// そのタスクの計画（`version` 昇順。履歴を含む）。
    fn execution_plan_list(&self, task_id: TaskId) -> Result<Vec<ExecutionPlanRow>, StoreError>;

    /// そのタスクの WorkUnit（`seq` 昇順）。
    fn work_units_for(&self, task_id: TaskId) -> Result<Vec<WorkUnitRow>, StoreError>;
    /// 1 件。無ければ `None`。
    fn work_unit_get(&self, id: &str) -> Result<Option<WorkUnitRow>, StoreError>;

    /// ADR-0079 D7（Phase R1a / migration 0031）: 決定の要求の行（`created_at` 昇順、同順位は id）。
    /// `root_id` を渡せばその木だけ。行は `Event::DecisionRequested` / `DecisionAnswered` /
    /// `DecisionWithdrawn` の追記と同じトランザクションで store が書く（派生）。
    fn decisions_list(
        &self,
        root_id: Option<TaskId>,
    ) -> Result<Vec<crate::decision::DecisionRow>, StoreError>;
    /// ADR-0079 D7: 1 件（無ければ `None`）。
    fn decision_get(&self, id: &str) -> Result<Option<crate::decision::DecisionRow>, StoreError>;
    /// ADR-0079 D15: `decisions` の全行を渡した集合でまるごと置き換える（`events` は変えない）。
    /// replay の再構築（`task_ops::replay::rebuild_decisions`）を書き戻すのに使う。
    fn decisions_replace(&self, rows: Vec<crate::decision::DecisionRow>) -> Result<(), StoreError>;
    /// ADR-0079 D7（Phase R3a）: 決定への回答・取り下げ・revise の 1 トランザクション。`decisions` の行
    /// `decision_id` の状態が今も `expect` であることを同じトランザクションで確かめ（違えば何も書かず
    /// `Ok(false)` = API の 409）、`task_id`（決定を出した節点）の WU の行 `updated` を書き換え、`events`
    /// （`DecisionAnswered` / `DecisionWithdrawn`・`WorkUnitTransitioned` など）を積む。
    fn decision_resolve_apply(
        &self,
        task_id: TaskId,
        decision_id: &str,
        expect: crate::decision::DecisionStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<bool, StoreError>;

    /// D6: WorkUnit の行を書き換え、`Event::WorkUnitTransitioned`（`event`）を同じトランザクションで
    /// 追記する。`updated.id` の行を丸ごと差し替える（呼び出し側が新しい状態・カウンタを計算済み）。
    fn work_unit_transition(
        &self,
        task_id: TaskId,
        updated: WorkUnitRow,
        event: Event,
    ) -> Result<(), StoreError>;

    /// ADR-0074 D1.5（Phase F2）: WU の lease を取る。1 トランザクションで「Task が `Running`」
    /// 「WU が `ready` / `needs_continuation`」を確かめ、WU を `running` にし（`runs + 1`、
    /// `last_run_id = run_id`、lease 列）、`WorkUnitTransitioned{to: running, reason: "dispatch"}` を
    /// 追記し、**Task の lease の期限を WU の lease の期限まで延ばす**（短くはしない）。
    /// `branch` / `base_commit` が `Some` なら WU の行にも書く（WU の worktree を切ったとき）。
    /// 条件に合わなければ `Ok(false)`（何も書かない）。
    #[allow(clippy::too_many_arguments)]
    fn acquire_work_unit_lease(
        &self,
        task_id: TaskId,
        work_unit_id: &str,
        run_id: &str,
        ttl: StdDuration,
        branch: Option<String>,
        base_commit: Option<String>,
    ) -> Result<bool, StoreError>;

    /// ADR-0074 D1.4（Phase F2b）: WU の行の追加（`inserted`）と書き換え（`updated`）と `events` を
    /// 1 トランザクションで書く（統合の repair WU を足し、統合 WU の依存を付け足す、など）。
    /// Task の状態は変えない。
    fn work_units_apply(
        &self,
        task_id: TaskId,
        inserted: Vec<WorkUnitRow>,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<(), StoreError>;

    /// ADR-0079 §7 R1b: 終わっていない kind task の unit（`pending` / `ready` / `running` /
    /// `needs_continuation` / `blocked`）を持つ task の id（重複なし、昇順）。dispatcher の tick が
    /// 子 task の生成と状態の写しの照合に使う（木が無ければ空で、何もしない）。
    fn tasks_with_open_task_units(&self) -> Result<Vec<TaskId>, StoreError>;

    /// ADR-0079 D3（Phase R2a）: 木の節点（`root_id` の task 自身と、`tasks.root_id = root_id` の子孫）。
    /// `created_at` 昇順（同順位は rowid）。木の上限の数え上げ（`task_ops::tree::tree_counters`）に使う。
    /// root 自身は `tree` を持たない（`root_id` 列は NULL。R1b 付記 6.）ので id で拾う。
    fn tree_tasks(&self, root_id: TaskId) -> Result<Vec<Task>, StoreError>;

    /// ADR-0079 D4 (4)（Phase R1b）: kind task の unit から子 task を作る 1 トランザクション。
    /// 親が終端でなく、unit の行（`unit.id`）が今も `ready` で `child_task_id` を持たないことを確かめ、
    /// 子を挿入して `Event::Created{origin: plan_unit}` を積み、unit の行を `unit`（呼び出し側が
    /// `running`・`child_task_id` を書いたもの）に差し替え、親の events に `parent_events`
    /// （`WorkUnitTransitioned` と `ChildTaskCreated`）を積む。条件に合わなければ何も書かず `Ok(false)`。
    ///
    /// ADR-0079 D9（Phase R2b）: `replaces = Some(prev)` は基盤の失敗の作り直し（同じ unit から新しい子）。
    /// unit の行が `running` で `child_task_id = prev` であることを確かめる（`ready`・子なしの代わりに）。
    fn tree_child_create(
        &self,
        parent_id: TaskId,
        child: &Task,
        unit: WorkUnitRow,
        parent_events: Vec<Event>,
        replaces: Option<&str>,
    ) -> Result<bool, StoreError>;

    /// ADR-0079 D15（Phase R5b-prep）: 人の計画（origin human）の採用と、その計画の unit の `adopt` による既存の
    /// task の採用を 1 トランザクションで書く。`execution_plan_adopt` と同じことをした後に `after_events`
    /// （`WorkUnitTransitioned`・`ChildAdopted`・`UnitGateOverridden`・`DecisionRequested` など。`work_units` の行は
    /// 呼び出し側がこれらを当てた後の値で渡す）を積み、`adoptions` の各 task を書き換える。採用する task が今も
    /// `expect_status` で木に属していない（`tree` が無い）ことを確かめ、違えば何も書かずに `Ok(false)`。
    #[allow(clippy::too_many_arguments)]
    fn execution_plan_adopt_tree(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        after_events: Vec<Event>,
        adoptions: Vec<TreeAdoption>,
    ) -> Result<bool, StoreError>;

    /// ADR-0079 D15（Phase R5b-prep）: 採用済みの計画の kind task の unit（`unit_id`）に既存の task を後から採用する
    /// 1 トランザクション（`POST /tasks/{id}/tree/adopt`）。計画を持つ task が終端でなく、unit の行が今も
    /// `expect_unit_status` の kind task で `child_task_id` を持たず、採用する task が `expect_status` で `tree` を
    /// 持たないことを確かめる。`updated` の行を書き換え、計画を持つ task に `events` を積み、採用する task を書き換える。
    /// 条件に合わなければ何も書かず `Ok(false)`。
    #[allow(clippy::too_many_arguments)]
    fn tree_adopt_apply(
        &self,
        owner_id: TaskId,
        unit_id: &str,
        expect_unit_status: WorkUnitStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
        adoption: TreeAdoption,
    ) -> Result<bool, StoreError>;

    /// ADR-0074 D1.4（Phase F2）: Task の lease（工程の保持者）の期限を `ttl` 先まで延ばす
    /// （短くはしない）。`Running` でなければ `Ok(false)`。統合の間の延長に使う。
    fn extend_task_lease(&self, task_id: TaskId, ttl: StdDuration) -> Result<bool, StoreError>;

    /// ADR-0074 D1.3 3.（Phase F2）: `Running` で、`phase` のある（v2 の）WU に `ready` /
    /// `needs_continuation` のものを持つ Task（`created_at` 昇順、最大 `limit` 件）。
    /// 公平性: `dispatch_ready` はこの Task たちの 2 本目以降を、Ready の Task の 1 本目の後に回す。
    fn running_tasks_with_runnable_work_units(&self, limit: usize)
    -> Result<Vec<Task>, StoreError>;

    /// D5: run 開始時に `runs` の行を 1 件作る（`status = running`）。`Event::WorkerStarted` と同じ
    /// トランザクションでは**ない**（既存コードの慣習に合わせ、`append_event` の直後に呼ぶ。監査上の
    /// 実害は無い: 再起動時の照合は `work_units.status` を正とする）。
    fn run_index_start(&self, row: RunRow) -> Result<(), StoreError>;
    /// D5: run 終了時に `runs` の行を更新する。無ければ `Ok(false)`。
    #[allow(clippy::too_many_arguments)]
    fn run_index_finish(
        &self,
        run_id: &str,
        status: RunIndexStatus,
        checkpoint: Option<crate::execution::Checkpoint>,
        usage: Option<crate::model::Usage>,
        metrics: Option<crate::model::RunMetrics>,
        finished_at: OffsetDateTime,
    ) -> Result<bool, StoreError>;
    fn run_index_get(&self, run_id: &str) -> Result<Option<RunRow>, StoreError>;
    /// そのタスクの run（`started_at` 昇順）。
    fn runs_for_task(&self, task_id: TaskId) -> Result<Vec<RunRow>, StoreError>;
    /// その WorkUnit の run（`started_at` 昇順）。
    fn runs_for_work_unit(&self, work_unit_id: &str) -> Result<Vec<RunRow>, StoreError>;

    /// `updated_at >= since` のタスク別実行集計を派生索引から 1 回の SQL で読む。
    ///
    /// | API の欄 | events 版 | 索引版 |
    /// |---|---|---|
    /// | tasks / done / failed / other | Task と最終 status | tasks.status（API が集約） |
    /// | completion_rate | done / (done + failed) | 同じ計算（API） |
    /// | continuations | Transitioned `continue` | work_units.continuations の和 |
    /// | max_turn_failures | WorkerFinished `BudgetExhausted(Turns)` | runs.status には種類が無く、最新 run の events だけでは過去 run 分も復元できない |
    /// | repairs | repair WorkUnit の key 数 | work_units.kind = repair の行数 |
    /// | replans | supersedes のある ExecutionPlanned 数 | execution_plans.version > 1 の行数 |
    /// | gate_mode / genre / assignee | Task.routing / Task の欄 | tasks.json.routing / 列 |
    /// | lane | routing_audit の最後の lane | runs に lane 列が無く、最新 run の events だけでは過去の監査結果を保証できない |
    ///
    /// Atomic task には work_units 行が無く、その `continue` と `worker_error` retry は索引から
    /// 復元できない。古い run（E2 より前）も runs に埋め戻していない。API が補完する場合は
    /// 該当タスクに限って events を読む必要があり、全タスクの events 走査へ戻さないこと。
    /// `budget_exhausted_runs` は種類を問わない数で、turns の数として扱ってはならない。
    fn execution_metrics_task_rows(
        &self,
        since: Option<OffsetDateTime>,
    ) -> Result<Vec<ExecutionMetricsTaskRow>, StoreError>;

    /// ADR-0072 D5/D15（Phase E2b）: `task_id` の `work_units` 行を、渡した集合でまるごと置き換える
    /// （既存行を全て削除してから挿入。1 トランザクション）。`events` は変えない。
    /// `celerisctl replay --apply`（`task_ops::replay::check_and_apply_execution`）が
    /// `rebuild_work_units_and_runs` の再構築結果を書き戻すのに使う（events が正本、索引は派生。D5）。
    fn work_units_replace(&self, task_id: TaskId, rows: Vec<WorkUnitRow>)
    -> Result<(), StoreError>;
    /// 同上。`task_id` の `runs` 行を渡した集合でまるごと置き換える。
    fn runs_replace(&self, task_id: TaskId, rows: Vec<RunRow>) -> Result<(), StoreError>;
    /// ADR-0072 D5/D17（Phase E4b 項目5）: 同上。`task_id` の `execution_plans` 行（版の履歴）を
    /// 渡した集合でまるごと置き換える。`task_ops::replay::check_and_apply_execution` が
    /// `rebuild_execution_plans`（`Event::ExecutionPlanned` からの再構築。replan で複数版になった
    /// 履歴を含む）の結果を書き戻すのに使う。
    fn execution_plans_replace(
        &self,
        task_id: TaskId,
        rows: Vec<ExecutionPlanRow>,
    ) -> Result<(), StoreError>;

    /// ADR-0072 D16（Phase E4）: reviewer repair の採用。`Trigger::ReviewRepair`
    /// （`Reviewing → Ready`、attempts 据え置き）と、`new_plan`（`Some` のときだけ挿入。atomic な
    /// Task で暗黙の WorkUnit を初めて実体化するときの `execution_plans` 行、D5）・`work_units`
    /// （repair WU。atomic なら `main`（done）も含む）・`extra_events`（`WorkerFinished{role:
    /// reviewer}` / `ReviewVerdict` / repair WU の `WorkUnitTransitioned` 等）を同じトランザクションで
    /// 書く。呼び出し側（`dispatcher.rs`）が D16 の上限（`max_repairs`/`max_repairs_per_class`）を
    /// 検査済みであることが前提（ここでは検査しない）。
    fn review_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError>;

    /// 配送の局所修復。Reopen と repair WU を同一トランザクションで保存する。
    fn delivery_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError>;

    /// ADR-0072 D17（Phase E4）: replan の採用。旧 `active` な計画を `superseded` にし、新しい版
    /// （`new_plan.version = old.version + 1`）を挿入する。`updated_work_units` は既存行の書き換え
    /// （spec が変わった未完了 WU、または `superseded` にする削除された WU）、`new_work_units` は
    /// 新しい key の追加。`extra_events`（削除された WU の `WorkUnitTransitioned` 等）→
    /// `plan_event`（`Event::ExecutionPlanned{supersedes: Some(old_plan_id), ..}`）の順で同じ
    /// トランザクションに追記する。旧版が `active` でなければ `StoreError::InUse`
    /// （並行 replan の検出）。
    #[allow(clippy::too_many_arguments)]
    fn execution_plan_replan(
        &self,
        task_id: TaskId,
        old_plan_id: String,
        new_plan: ExecutionPlanRow,
        updated_work_units: Vec<WorkUnitRow>,
        new_work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        plan_event: Event,
    ) -> Result<(), StoreError>;
}

/// ADR-0059 D6（Phase 99）: `cluster_settings` の 1 行。`work_dir` は絶対パスか `~`/`~/…`
/// （検証は書き込み側〈API ハンドラ〉で行う。ここは型だけ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClusterSettings {
    pub cluster_id: String,
    pub work_dir: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub updated_at: OffsetDateTime,
}

/// ADR-0078 D5: `cluster_connection_log` の 1 行。`kind` / `method` / `cause` の値は
/// migration 0030 の注釈を見よ（文字列のまま持つ。書き手は dispatcher だけ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClusterConnectionRecord {
    pub cluster_id: String,
    pub kind: String,
    pub method: Option<String>,
    pub cause: Option<String>,
    pub uptime_secs: Option<u64>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub at: OffsetDateTime,
}

/// ADR-0078 D5: クラスタごとの接続・切断の回数（`GET /clusters` の `stats`）。
/// `ClusterConnectionRecord` を `add` で積んで作る（dispatcher の起動以降の値も、DB から数える直近
/// 24 時間の値も同じ規則で数える）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClusterConnectionStats {
    #[serde(default)]
    pub connects_totp: u64,
    #[serde(default)]
    pub connects_publickey: u64,
    /// 人（や前の daemon）が張った master を `-O check` で見つけた回数。
    #[serde(default)]
    pub connects_borrowed: u64,
    #[serde(default)]
    pub losses: u64,
    /// `cause`（`check_failed` / `probe_failed` / `master_exited` / `explicit`）ごとの切断の回数。
    #[serde(default)]
    pub losses_by_cause: std::collections::BTreeMap<String, u64>,
    #[serde(default)]
    pub key_auth_attempts: u64,
    /// 最後に切れた時刻（RFC 3339）。
    #[serde(default)]
    pub last_lost_at: Option<String>,
    #[serde(default)]
    pub last_lost_cause: Option<String>,
}

impl ClusterConnectionStats {
    /// 1 行を数える（`kind` が未知なら何もしない）。
    pub fn add(&mut self, record: &ClusterConnectionRecord) {
        match record.kind.as_str() {
            "connected" => match record.method.as_deref() {
                Some("totp") => self.connects_totp += 1,
                Some("publickey") => self.connects_publickey += 1,
                _ => self.connects_borrowed += 1,
            },
            "lost" => {
                self.losses += 1;
                let cause = record
                    .cause
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                *self.losses_by_cause.entry(cause.clone()).or_insert(0) += 1;
                self.last_lost_at = record.at.format(&Rfc3339).ok();
                self.last_lost_cause = Some(cause);
            }
            "key_auth_attempt" => self.key_auth_attempts += 1,
            _ => {}
        }
    }

    /// `records` のうち `cluster_id` の行だけを数える。
    pub fn from_records(records: &[ClusterConnectionRecord], cluster_id: &str) -> Self {
        let mut stats = Self::default();
        for record in records.iter().filter(|r| r.cluster_id == cluster_id) {
            stats.add(record);
        }
        stats
    }
}

pub struct SqliteStore {
    conn: Mutex<Connection>,
    /// ADR-0064 D4: ファイル DB のときだけ `Some`（インメモリでは接続間の状態共有ができないため）。
    read_pool: Option<ReadPool>,
}

/// ADR-0064 D4: 読み取り専用（`query_only=ON`）の小さな接続プール。書き込み接続（`conn`）の
/// `Mutex` とは別物で、読み取りは書き込みのトランザクション中でも待たされない（WAL の性質）。
struct ReadPool {
    conns: Mutex<Vec<Connection>>,
    available: std::sync::Condvar,
}

impl ReadPool {
    fn open(path: &Path, size: usize, busy_timeout: StdDuration) -> Result<Self, StoreError> {
        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            let conn = Connection::open(path)?;
            conn.busy_timeout(busy_timeout)?;
            // ADR-0064 D4: `?mode=ro` ではなく通常の接続を `query_only=ON` にする（`ATTACH` や
            // 一時テーブルなど、真の読み取り専用オープンでは使えない機能を素朴に避けるため）。
            conn.pragma_update(None, "query_only", true)?;
            conns.push(conn);
        }
        Ok(Self {
            conns: Mutex::new(conns),
            available: std::sync::Condvar::new(),
        })
    }

    /// 空いている接続を 1 本取り出す（無ければ空くまで待つ）。
    fn checkout(&self) -> Result<Connection, StoreError> {
        let mut guard = self.conns.lock().map_err(|_| StoreError::Poisoned)?;
        loop {
            if let Some(conn) = guard.pop() {
                return Ok(conn);
            }
            guard = self
                .available
                .wait(guard)
                .map_err(|_| StoreError::Poisoned)?;
        }
    }

    /// 使い終わった接続をプールへ返す。プールの `Mutex` が poison していたら、その接続は
    /// 静かに捨てる（プールは縮むだけで、以後の `with_read_conn` はエラーにはならない）。
    fn checkin(&self, conn: Connection) {
        if let Ok(mut guard) = self.conns.lock() {
            guard.push(conn);
            self.available.notify_one();
        }
    }
}

/// ADR-0043 D1: 既存の作業場所を写すとき・API が `kind` を省略したときの `kind` の決め方
/// （決定的。LLM は使わない）。
///
/// - `Local` — `<path>/.git` があれば `git`（ディレクトリでも worktree の gitfile でもよい）、無ければ `dir`
/// - `Remote` — `git`。クラスタ側のファイルシステムは celeris からは見えないが、ADR-0018 / ADR-0019 の
///   リモートの作業場所は git リポジトリを前提にした同期をするため。違えば人が `PATCH /repos/{id}` で直す
pub fn detect_repo_kind(location: &WorkspaceSpec) -> RepoKind {
    match location {
        WorkspaceSpec::Local { path, .. } => {
            if path.join(".git").exists() {
                RepoKind::Git
            } else {
                RepoKind::Dir
            }
        }
        WorkspaceSpec::Remote { .. } => RepoKind::Git,
    }
}

fn status_str(s: Status) -> &'static str {
    match s {
        Status::Draft => "draft",
        Status::Ready => "ready",
        Status::Running => "running",
        Status::Blocked => "blocked",
        Status::Reviewing => "reviewing",
        Status::Done => "done",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
    }
}

fn kind_str(k: TaskKind) -> &'static str {
    match k {
        TaskKind::Plan => "plan",
        TaskKind::Execute => "execute",
        TaskKind::Review => "review",
        TaskKind::Approval => "approval",
    }
}

/// ADR-0044 D4: `worker_hint.tier` の JSON 表現（`json_extract` の照合に使う）。
fn tier_str(t: crate::model::Tier) -> &'static str {
    match t {
        crate::model::Tier::Frontier => "frontier",
        crate::model::Tier::Standard => "standard",
        crate::model::Tier::Cheap => "cheap",
    }
}

pub(crate) fn format_rfc3339(t: OffsetDateTime) -> Result<String, StoreError> {
    Ok(t.format(&Rfc3339)?)
}

pub(crate) fn parse_rfc3339(s: &str) -> Result<OffsetDateTime, StoreError> {
    Ok(OffsetDateTime::parse(s, &Rfc3339)?)
}

/// ADR-0064 D2/D3: `src` を `dest` へ rusqlite の backup API でコピーする。既存のストア接続の
/// `Mutex` は取らない**専用の接続**を新たに開くので、進行中の書き込み・読み取りを長く待たせない
/// （WAL のバックアップはページ単位で進み、途中でも一貫したスナップショットになる）。呼び出し側
/// （`celeris` の背景バックアップ tick、`celerisctl db backup`、`relocate-db.sh`）が使う。
pub fn backup_database(
    src: &Path,
    dest: &Path,
    busy_timeout: StdDuration,
) -> Result<(), StoreError> {
    let src_conn = Connection::open(src)?;
    src_conn.busy_timeout(busy_timeout)?;
    let mut dest_conn = Connection::open(dest)?;
    let backup = rusqlite::backup::Backup::new(&src_conn, &mut dest_conn)?;
    backup.run_to_completion(100, StdDuration::from_millis(250), None)?;
    Ok(())
}

/// ADR-0064 D2: `path` の DB に対して `PRAGMA integrity_check` を実行し、`"ok"` だけが返れば
/// `true`（1 行でも別の文言があれば破損の疑い）。`relocate-db.sh` が退避先の検証に使う想定。
pub fn integrity_check(path: &Path) -> Result<bool, StoreError> {
    let conn = Connection::open(path)?;
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut ok = true;
    let mut any = false;
    for row in rows {
        any = true;
        if row? != "ok" {
            ok = false;
        }
    }
    Ok(ok && any)
}

impl SqliteStore {
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        // ADR-0064 D4: インメモリでは追加の接続が同じ状態を共有できないので、読み取りプールは作らない
        // （`with_read_conn` は書き込み接続にフォールバックする）。
        Self::from_connection(conn, &StoreOptions::default(), None)
    }

    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::open_with(path, StoreOptions::default())
    }

    /// ADR-0013 D5 / ADR-0064 D1・D4・D5: `path` の DB を `options` の PRAGMA 設定で開き、
    /// マイグレーションを適用する。DB の版数がこのバイナリの知る `SCHEMA_VERSION` より新しければ
    /// `StoreError::SchemaTooNew` を返し、DB には何も書かない。
    pub fn open_with(path: &Path, options: StoreOptions) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn, &options, Some(path))
    }

    // ---- ADR-0046 D7（Phase 59）: `celerisctl org migrate-v2` のための低レベルの書き換え。
    // 通常の経路（`org_upsert` / `update_task`）は状態機械と検証を通すが、移行は「id の付け替え」だけを
    // まとめて行うので、ここに専用の関数を置く（`celerisctl` からしか呼ばない）。

    /// ノード id を参照している行を `from` → `to` に書き換え、書き換えた行の主キーを返す。
    /// `tasks` は `assignee` 列と `json`（正本）の両方を直す。
    pub fn migrate_node_refs(
        &self,
        table: &str,
        column: &str,
        from: &str,
        to: &str,
    ) -> Result<Vec<String>, StoreError> {
        if !is_known_node_ref(table, column) {
            return Err(StoreError::Invalid(format!(
                "unknown node reference: {table}.{column}"
            )));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids: Vec<String> = {
            let sql = format!("SELECT id FROM {table} WHERE {column} = ?1");
            let mut stmt = tx.prepare(&sql)?;
            let rows = stmt.query_map(params![from], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            out
        };
        for id in &ids {
            set_node_ref(&tx, table, column, id, to)?;
        }
        tx.commit()?;
        Ok(ids)
    }

    /// `migrate_node_refs` の逆（`--rollback`）。1 行だけを元の値に戻す。
    pub fn restore_node_ref(
        &self,
        table: &str,
        column: &str,
        id: &str,
        old: &str,
    ) -> Result<(), StoreError> {
        if !is_known_node_ref(table, column) {
            return Err(StoreError::Invalid(format!(
                "unknown node reference: {table}.{column}"
            )));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        set_node_ref(&tx, table, column, id, old)?;
        tx.commit()?;
        Ok(())
    }

    /// `org_nodes` をまるごと差し替える（親が先に来る並びで渡すこと）。1 トランザクション。
    pub fn replace_org_nodes(&self, nodes: &[OrgNode]) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM org_nodes", [])?;
        let mut existing: Vec<OrgNode> = Vec::with_capacity(nodes.len());
        for node in nodes {
            crate::org::validate_upsert(&existing, node)?;
            tx.execute(
                "INSERT INTO org_nodes (id, parent_id, name, kind, genre, brief, position, created_at, updated_at, \
                 profile_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    node.id,
                    node.parent_id,
                    node.name,
                    node.kind.as_str(),
                    node.genre,
                    node.brief,
                    node.position,
                    format_rfc3339(node.created_at)?,
                    format_rfc3339(node.updated_at)?,
                    profile_json(&node.profile)?,
                ],
            )?;
            existing.push(node.clone());
        }
        tx.commit()?;
        Ok(())
    }

    /// 現在の DB のスキーマ版数（`schema_migrations` の最大 `version`。行が無ければ 0）。
    pub fn schema_version(&self) -> Result<u32, StoreError> {
        let conn = self.lock()?;
        let v: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        Ok(v as u32)
    }

    fn from_connection(
        mut conn: Connection,
        options: &StoreOptions,
        path: Option<&Path>,
    ) -> Result<Self, StoreError> {
        Self::configure_pragmas(&conn, options)?;
        Self::migrate(&mut conn)?;
        // ADR-0064 D4: ファイル DB でだけ、読み取り専用の小さな接続プールを作る（`read_pool_size = 0`
        // にすれば無効化できる）。
        let read_pool = match path {
            Some(path) if options.read_pool_size > 0 => Some(ReadPool::open(
                path,
                options.read_pool_size,
                options.busy_timeout,
            )?),
            _ => None,
        };
        Ok(Self {
            conn: Mutex::new(conn),
            read_pool,
        })
    }

    /// ADR-0013 D5 / ADR-0064 D5: WAL・busy_timeout・synchronous=NORMAL を設定する。`foreign_keys`
    /// は変えない。インメモリ DB では `journal_mode` が `memory` のまま返ることがあるが、エラーには
    /// しない。`background_checkpoint` のときは `wal_autocheckpoint=0` にし、専用の背景タスクが
    /// `PRAGMA wal_checkpoint(PASSIVE)` を打つ前提で `journal_size_limit` も設定する
    /// （fsync がリクエストや tick の中に落ちないようにするため）。
    fn configure_pragmas(conn: &Connection, options: &StoreOptions) -> Result<(), StoreError> {
        conn.busy_timeout(options.busy_timeout)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let _journal_mode: String =
            conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        if options.background_checkpoint {
            conn.pragma_update(None, "wal_autocheckpoint", 0)?;
            conn.pragma_update(
                None,
                "journal_size_limit",
                BACKGROUND_CHECKPOINT_JOURNAL_SIZE_LIMIT,
            )?;
        }
        Ok(())
    }

    /// ADR-0064 D4: 読み取り専用の接続でクロージャを実行する（読み取りプールがあればそこから、
    /// 無ければ〈インメモリ DB や `read_pool_size = 0`〉書き込み接続にフォールバックする）。
    /// `TaskStore` の SELECT だけを行うメソッドはこれを使い、書き込みを伴うメソッドは引き続き
    /// `self.lock()` を使う。
    pub(crate) fn with_read_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        match &self.read_pool {
            Some(pool) => {
                let conn = pool.checkout()?;
                let result = f(&conn);
                pool.checkin(conn);
                result
            }
            None => {
                let conn = self.lock()?;
                f(&conn)
            }
        }
    }

    fn table_exists(conn: &Connection, name: &str) -> Result<bool, StoreError> {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            params![name],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    /// ADR-0013 D5: `schema_migrations` を導入し、未適用の版を 1 つずつ 1 トランザクションで
    /// 適用する。既存 DB（`schema_migrations` が無く `tasks` がある）は版数 1 とみなす。
    fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
        let migrations_table_existed = Self::table_exists(conn, "schema_migrations")?;
        if !migrations_table_existed {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (\
                 version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
            )?;
        }

        let mut current: u32 = if migrations_table_existed {
            let v: i64 = conn.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )?;
            v as u32
        } else {
            0
        };

        if current > SCHEMA_VERSION {
            return Err(StoreError::SchemaTooNew {
                found: current,
                supported: SCHEMA_VERSION,
            });
        }

        if current == 0 && !migrations_table_existed && Self::table_exists(conn, "tasks")? {
            // 既存 DB（Phase 1〜8 で作られた、schema_migrations の無い DB）は版数 1 が
            // 適用済みとみなす。0001_init.sql は再実行しない。
            Self::mark_migration_applied(conn, 1)?;
            current = 1;
        }

        for version in (current + 1)..=SCHEMA_VERSION {
            Self::apply_migration_version(conn, version)?;
        }

        Ok(())
    }

    fn migration_sql(version: u32) -> Result<&'static str, StoreError> {
        match version {
            1 => Ok(MIGRATION_0001),
            2 => Ok(MIGRATION_0002),
            3 => Ok(MIGRATION_0003),
            4 => Ok(MIGRATION_0004),
            5 => Ok(MIGRATION_0005),
            6 => Ok(MIGRATION_0006),
            7 => Ok(MIGRATION_0007),
            8 => Ok(MIGRATION_0008),
            9 => Ok(MIGRATION_0009),
            10 => Ok(MIGRATION_0010),
            11 => Ok(MIGRATION_0011),
            12 => Ok(MIGRATION_0012),
            13 => Ok(MIGRATION_0013),
            14 => Ok(MIGRATION_0014),
            15 => Ok(MIGRATION_0015),
            16 => Ok(MIGRATION_0016),
            17 => Ok(MIGRATION_0017),
            18 => Ok(MIGRATION_0018),
            19 => Ok(MIGRATION_0019),
            20 => Ok(MIGRATION_0020),
            21 => Ok(MIGRATION_0021),
            22 => Ok(MIGRATION_0022),
            23 => Ok(MIGRATION_0023),
            24 => Ok(MIGRATION_0024),
            25 => Ok(MIGRATION_0025),
            26 => Ok(MIGRATION_0026),
            27 => Ok(MIGRATION_0027),
            28 => Ok(MIGRATION_0028),
            29 => Ok(MIGRATION_0029),
            30 => Ok(MIGRATION_0030),
            31 => Ok(MIGRATION_0031),
            32 => Ok(MIGRATION_0032),
            33 => Ok(MIGRATION_0033),
            other => Err(StoreError::Invalid(format!(
                "unknown migration version: {other}"
            ))),
        }
    }

    fn apply_migration_version(conn: &mut Connection, version: u32) -> Result<(), StoreError> {
        let sql = Self::migration_sql(version)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(sql)?;
        // ADR-0043 D1（Phase 52）: 既存の `projects.workspace` を `is_primary = 1` のリポジトリ 1 件に
        // 写す。id が ULID で、`kind` の判定にファイルシステムを見る必要があるので SQL では書けない
        // （migration 0012 のコメント参照）。同じトランザクションの中で 1 度だけ走る。
        if version == 12 {
            Self::backfill_project_repos(&tx)?;
        }
        // Phase K-1: 既存の案件に `slug` を付ける（作り方は Rust にしか無い。migration 0029 のコメント参照）。
        if version == 29 {
            Self::backfill_project_slugs(&tx)?;
        }
        let ts = format_rfc3339(OffsetDateTime::now_utc())?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![version, ts],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn mark_migration_applied(conn: &Connection, version: u32) -> Result<(), StoreError> {
        let ts = format_rfc3339(OffsetDateTime::now_utc())?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![version, ts],
        )?;
        Ok(())
    }

    // ---- ADR-0033 D1/D2: 組織・案件の行と型の間の変換（rusqlite の行変換は `rusqlite::Error` しか
    // 返せないので、解析の失敗は内側の `Result<_, StoreError>` に載せて返す）----

    fn org_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<OrgNode, StoreError>> {
        let id: String = row.get(0)?;
        let kind_col: String = row.get(3)?;
        let created_at: String = row.get(7)?;
        let updated_at: String = row.get(8)?;
        // ADR-0046 D1（Phase 59）: 10 列目は `profile_json`（NULL なら空の profile）。
        let profile_col: Option<String> = row.get(9)?;
        let Some(kind) = OrgKind::parse(&kind_col) else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid org node kind in org_nodes: {kind_col}"
            ))));
        };
        let profile = match profile_col.as_deref() {
            Some(raw) if !raw.trim().is_empty() => {
                match serde_json::from_str::<crate::profile::Profile>(raw) {
                    Ok(parsed) => parsed,
                    Err(e) => {
                        return Ok(Err(StoreError::Invalid(format!(
                            "invalid org profile for {id}: {e}"
                        ))));
                    }
                }
            }
            _ => crate::profile::Profile::default(),
        };
        Ok((|| {
            Ok(OrgNode {
                id,
                parent_id: row.get(1)?,
                name: row.get(2)?,
                kind,
                genre: row.get(4)?,
                brief: row.get(5)?,
                profile,
                position: row.get(6)?,
                created_at: parse_rfc3339(&created_at)?,
                updated_at: parse_rfc3339(&updated_at)?,
            })
        })())
    }

    fn org_list_tx(conn: &Connection) -> Result<Vec<OrgNode>, StoreError> {
        let mut stmt = conn.prepare(
            "SELECT id, parent_id, name, kind, genre, brief, position, created_at, updated_at, profile_json \
             FROM org_nodes ORDER BY position ASC, id ASC",
        )?;
        let rows = stmt.query_map([], Self::org_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    fn project_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Project, StoreError>> {
        let id: String = row.get(0)?;
        let status_col: String = row.get(3)?;
        let created_at: String = row.get(5)?;
        let updated_at: String = row.get(6)?;
        // ADR-0039 D1 / ADR-0043 D1: 7 列目は **primary のリポジトリの `location_json`**、無ければ
        // 従来の `projects.workspace` 列（`COALESCE`。導入前の行と作業場所を決めていない案件は NULL）。
        let workspace_col: Option<String> = row.get(7)?;
        // ADR-0044 D6（Phase 55）: 8 列目 `archived_at`、9 列目 `paused_from`。
        let archived_at_col: Option<String> = row.get(8)?;
        let paused_from_col: Option<String> = row.get(9)?;
        let (Ok(id), Some(status)) = (id.parse::<ProjectId>(), ProjectStatus::parse(&status_col))
        else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid project row: id={id} status={status_col}"
            ))));
        };
        let paused_from = match paused_from_col.as_deref() {
            Some(raw) => match ProjectStatus::parse(raw) {
                Some(parsed) => Some(parsed),
                None => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid project paused_from for {id}: {raw}"
                    ))));
                }
            },
            None => None,
        };
        let workspace = match workspace_col.as_deref() {
            Some(raw) => match serde_json::from_str::<WorkspaceSpec>(raw) {
                Ok(spec) => Some(spec),
                Err(e) => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid project workspace for {id}: {e}"
                    ))));
                }
            },
            None => None,
        };
        Ok((|| {
            Ok(Project {
                id,
                title: row.get(1)?,
                request: row.get(2)?,
                status,
                secretary_summary: row.get(4)?,
                workspace,
                archived_at: match archived_at_col.as_deref() {
                    Some(raw) => Some(parse_rfc3339(raw)?),
                    None => None,
                },
                paused_from,
                // ADR-0074 D3.2（Phase F4b）: 11 列目 `auto_advance`。
                auto_advance: row.get::<_, i64>(10)? != 0,
                // Phase K-1: 12 列目 `slug`。
                slug: row.get(11)?,
                created_at: parse_rfc3339(&created_at)?,
                updated_at: parse_rfc3339(&updated_at)?,
            })
        })())
    }

    /// ADR-0039 D1: 案件の作業場所を DB の列に入れる形（JSON か NULL）にする。
    fn project_workspace_column(
        workspace: Option<&WorkspaceSpec>,
    ) -> Result<Option<String>, StoreError> {
        match workspace {
            Some(spec) => serde_json::to_string(spec)
                .map(Some)
                .map_err(|e| StoreError::Invalid(format!("cannot serialize workspace: {e}"))),
            None => Ok(None),
        }
    }

    // ---- ADR-0043 D1（Phase 52）: 案件のリポジトリ（`project_repos`）----

    /// `project_repos` の 1 行。
    fn repo_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<ProjectRepo, StoreError>> {
        let id: String = row.get(0)?;
        let project_id: String = row.get(1)?;
        let kind_col: String = row.get(3)?;
        let location_col: String = row.get(4)?;
        let run_col: String = row.get(7)?;
        let created_at: String = row.get(9)?;
        let (Ok(id), Ok(project_id), Some(kind), Some(run)) = (
            id.parse::<RepoId>(),
            project_id.parse::<ProjectId>(),
            RepoKind::parse(&kind_col),
            RepoRun::parse(&run_col),
        ) else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid project_repos row: id={id} project_id={project_id} kind={kind_col} run={run_col}"
            ))));
        };
        let location = match serde_json::from_str::<WorkspaceSpec>(&location_col) {
            Ok(spec) => spec,
            Err(e) => {
                return Ok(Err(StoreError::Invalid(format!(
                    "invalid project_repos location for {id}: {e}"
                ))));
            }
        };
        let sync_col: Option<String> = row.get(6)?;
        let sync = match sync_col.as_deref() {
            None => None,
            Some(raw) => match RepoSync::parse(raw) {
                Some(v) => Some(v),
                None => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid project_repos sync for {id}: {raw:?}"
                    ))));
                }
            },
        };
        Ok((|| {
            let is_primary: i64 = row.get(8)?;
            Ok(ProjectRepo {
                id,
                project_id,
                name: row.get(2)?,
                kind,
                location,
                default_branch: row.get(5)?,
                sync,
                run,
                is_primary: is_primary != 0,
                created_at: parse_rfc3339(&created_at)?,
            })
        })())
    }

    const REPO_COLUMNS: &'static str = "id, project_id, name, kind, location_json, default_branch, sync, run, is_primary, created_at";

    fn repo_list_tx(
        conn: &Connection,
        project_id: ProjectId,
    ) -> Result<Vec<ProjectRepo>, StoreError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM project_repos WHERE project_id = ?1 \
             ORDER BY is_primary DESC, created_at ASC, id ASC",
            Self::REPO_COLUMNS
        ))?;
        let rows = stmt.query_map(params![project_id.to_string()], Self::repo_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    fn repo_get_tx(conn: &Connection, id: RepoId) -> Result<Option<ProjectRepo>, StoreError> {
        conn.query_row(
            &format!(
                "SELECT {} FROM project_repos WHERE id = ?1",
                Self::REPO_COLUMNS
            ),
            params![id.to_string()],
            Self::repo_row,
        )
        .optional()?
        .transpose()
    }

    /// 行を 1 件書く（INSERT OR REPLACE）。検証は呼び出し側で済ませておくこと。
    fn repo_write_tx(conn: &Connection, repo: &ProjectRepo) -> Result<(), StoreError> {
        let location = serde_json::to_string(&repo.location)
            .map_err(|e| StoreError::Invalid(format!("cannot serialize repo location: {e}")))?;
        conn.execute(
            &format!(
                "INSERT OR REPLACE INTO project_repos ({}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                Self::REPO_COLUMNS
            ),
            params![
                repo.id.to_string(),
                repo.project_id.to_string(),
                repo.name,
                repo.kind.as_str(),
                location,
                repo.default_branch,
                repo.sync.map(|s| s.as_str()),
                repo.run.as_str(),
                i64::from(repo.is_primary),
                format_rfc3339(repo.created_at)?,
            ],
        )?;
        Ok(())
    }

    /// 1 案件に primary は 1 つ（ADR-0043 D1）。`keep` 以外の `is_primary` を落とす。
    fn repo_clear_other_primaries_tx(
        conn: &Connection,
        project_id: ProjectId,
        keep: RepoId,
    ) -> Result<(), StoreError> {
        conn.execute(
            "UPDATE project_repos SET is_primary = 0 WHERE project_id = ?1 AND id <> ?2",
            params![project_id.to_string(), keep.to_string()],
        )?;
        Ok(())
    }

    /// `projects.workspace` 列を primary のリポジトリの写しに保つ（migration 0012 のコメント参照。
    /// ADR-0043 D1 は「書かない」だが、N-1 互換〈旧バイナリが新スキーマを読む〉のために写しを残す）。
    fn sync_project_workspace_tx(
        conn: &Connection,
        project_id: ProjectId,
    ) -> Result<(), StoreError> {
        let primary: Option<String> = conn
            .query_row(
                "SELECT location_json FROM project_repos WHERE project_id = ?1 AND is_primary = 1 \
                 ORDER BY created_at ASC, id ASC LIMIT 1",
                params![project_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        // primary が消えたら NULL に戻す（案件を「作業場所なし」に戻したとき）。
        conn.execute(
            "UPDATE projects SET workspace = ?1 WHERE id = ?2",
            params![primary, project_id.to_string()],
        )?;
        Ok(())
    }

    /// そのリポジトリを参照している**未終端**のタスク（ADR-0043 D1: `DELETE /repos/{id}` の 409）。
    /// 索引は migration 0012 で足した `tasks.repos_json`（ULID は一意なので部分一致で足りる）。
    fn repo_active_tasks_tx(conn: &Connection, id: RepoId) -> Result<Vec<TaskId>, StoreError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT id FROM tasks WHERE {} AND repos_json IS NOT NULL AND repos_json LIKE ?1 \
             ORDER BY id ASC",
            Self::NON_TERMINAL_SQL
        ))?;
        let pattern = format!("%{id}%");
        let rows = stmt.query_map(params![pattern], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(Self::parse_id(&row?)?);
        }
        Ok(out)
    }

    // ---- ADR-0043 D5（Phase 54）: 変更の取り込み（`task_integrations`）----

    const INTEGRATION_COLUMNS: &'static str = "id, task_id, repo_id, repo_name, method, state, pr_number, \
         pr_url, merged_at, detail, created_at, updated_at";

    fn integration_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<TaskIntegration, StoreError>> {
        let id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let repo_id: Option<String> = row.get(2)?;
        let method_col: String = row.get(4)?;
        let state_col: String = row.get(5)?;
        let (Ok(id), Ok(task_id), Some(method), Some(state)) = (
            id.parse::<IntegrationId>(),
            task_id.parse::<TaskId>(),
            IntegrationMethod::parse(&method_col),
            IntegrationState::parse(&state_col),
        ) else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid task_integrations row: id={id} task_id={task_id} method={method_col} state={state_col}"
            ))));
        };
        let repo_id = match repo_id.as_deref() {
            None => None,
            Some(raw) => match raw.parse::<RepoId>() {
                Ok(v) => Some(v),
                Err(_) => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid task_integrations repo_id for {id}: {raw:?}"
                    ))));
                }
            },
        };
        Ok((|| {
            let merged_at: Option<String> = row.get(8)?;
            let created_at: String = row.get(10)?;
            let updated_at: String = row.get(11)?;
            Ok(TaskIntegration {
                id,
                task_id,
                repo_id,
                repo: row.get(3)?,
                method,
                state,
                pr_number: row.get(6)?,
                pr_url: row.get(7)?,
                merged_at: merged_at.as_deref().map(parse_rfc3339).transpose()?,
                detail: row.get(9)?,
                created_at: parse_rfc3339(&created_at)?,
                updated_at: parse_rfc3339(&updated_at)?,
            })
        })())
    }

    fn integration_put_tx(
        conn: &Connection,
        integration: &TaskIntegration,
    ) -> Result<(), StoreError> {
        conn.execute(
            &format!(
                "INSERT OR REPLACE INTO task_integrations ({}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                Self::INTEGRATION_COLUMNS
            ),
            params![
                integration.id.to_string(),
                integration.task_id.to_string(),
                integration.repo_id.map(|r| r.to_string()),
                integration.repo,
                integration.method.as_str(),
                integration.state.as_str(),
                integration.pr_number,
                integration.pr_url,
                integration.merged_at.map(format_rfc3339).transpose()?,
                integration.detail,
                format_rfc3339(integration.created_at)?,
                format_rfc3339(integration.updated_at)?,
            ],
        )?;
        Ok(())
    }

    fn integration_query_tx(
        conn: &Connection,
        where_sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<TaskIntegration>, StoreError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM task_integrations WHERE {where_sql} ORDER BY created_at DESC, id DESC",
            Self::INTEGRATION_COLUMNS
        ))?;
        let rows = stmt.query_map(params, Self::integration_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    /// migration 0012 の写し（ADR-0043 D1）。`projects.workspace` がある案件ごとに `is_primary = 1` の
    /// リポジトリを 1 件作る。`kind` は「パスが git なら git、でなければ dir」だが、SQL からは
    /// ファイルシステムを見られないのでここ（Rust）で決める。
    fn backfill_project_repos(conn: &Connection) -> Result<(), StoreError> {
        let mut stmt = conn.prepare(
            "SELECT id, workspace, created_at FROM projects WHERE workspace IS NOT NULL",
        )?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for (project_id, workspace, created_at) in rows {
            let Ok(project_id) = project_id.parse::<ProjectId>() else {
                continue;
            };
            let Ok(location) = serde_json::from_str::<WorkspaceSpec>(&workspace) else {
                continue;
            };
            let repo = ProjectRepo {
                id: RepoId::new(),
                project_id,
                name: crate::repos::default_repo_name(&location),
                kind: detect_repo_kind(&location),
                location,
                default_branch: None,
                sync: None,
                run: RepoRun::Auto,
                is_primary: true,
                created_at: parse_rfc3339(&created_at)
                    .unwrap_or_else(|_| OffsetDateTime::now_utc()),
            };
            Self::repo_write_tx(conn, &repo)?;
        }
        Ok(())
    }

    /// Phase K-1: 渡された slug の検査（綴りと、他の案件との重複）。
    fn check_project_slug(conn: &Connection, id: &str, slug: &str) -> Result<(), StoreError> {
        if !crate::knowledge::is_valid_project_slug(slug) {
            return Err(StoreError::Invalid(format!(
                "project slug must be lowercase [a-z0-9-] (1..64 chars, no leading/trailing/double '-', not an id): {slug:?}"
            )));
        }
        let other: Option<String> = conn
            .query_row(
                "SELECT id FROM projects WHERE slug = ?1 AND id <> ?2",
                params![slug, id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(other) = other {
            return Err(StoreError::InUse {
                kind: "project slug",
                id: slug.to_string(),
                detail: format!("already used by project {other}"),
            });
        }
        Ok(())
    }

    /// Phase K-1（migration 0029）: `slug` の無い案件に、作った順に slug を付ける
    /// （[`crate::knowledge::derive_project_slug`]。題名 → primary リポジトリの名前 → id の末尾。
    /// 先に作った案件が先に取る。重複は `<slug>-<id の末尾 8 文字>`）。
    fn backfill_project_slugs(conn: &Connection) -> Result<(), StoreError> {
        let mut stmt = conn.prepare(
            "SELECT p.id, p.title, \
                    (SELECT r.name FROM project_repos r WHERE r.project_id = p.id AND r.is_primary = 1 \
                     ORDER BY r.created_at ASC, r.id ASC LIMIT 1) \
             FROM projects p WHERE p.slug IS NULL ORDER BY p.created_at ASC, p.id ASC",
        )?;
        let rows: Vec<(String, String, Option<String>)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for (id, title, repo) in rows {
            let slug = Self::unique_project_slug(conn, &title, &id, repo.as_deref())?;
            conn.execute(
                "UPDATE projects SET slug = ?1 WHERE id = ?2",
                params![slug, id],
            )?;
        }
        Ok(())
    }

    /// 他の案件が使っていない slug を決める（Phase K-1）。
    fn unique_project_slug(
        conn: &Connection,
        title: &str,
        id: &str,
        primary_repo: Option<&str>,
    ) -> Result<String, StoreError> {
        let mut stmt =
            conn.prepare("SELECT slug FROM projects WHERE slug IS NOT NULL AND id <> ?1")?;
        let taken: std::collections::HashSet<String> = stmt
            .query_map(params![id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(crate::knowledge::derive_project_slug(
            title,
            id,
            primary_repo,
            &|s| taken.contains(s),
        ))
    }

    /// ADR-0044 D6（Phase 55）: いま dispatch を止めている案件の id（`paused` / `cancelled` /
    /// アーカイブ済み）。`ready_tasks` が 1 tick に 1 回だけ引く。
    fn halted_projects_locked(
        conn: &Connection,
    ) -> Result<std::collections::HashSet<String>, StoreError> {
        let mut stmt = conn.prepare(
            "SELECT id FROM projects WHERE status IN ('paused', 'cancelled') OR archived_at IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = std::collections::HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// ADR-0044 D6（Phase 55）: いま dispatch を止めている途中目標の id（`paused` / `cancelled`）。
    fn halted_milestones_locked(
        conn: &Connection,
    ) -> Result<std::collections::HashSet<String>, StoreError> {
        let mut stmt =
            conn.prepare("SELECT id FROM milestones WHERE status IN ('paused', 'cancelled')")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = std::collections::HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// ADR-0079 D13（Phase R5a）: `task` が subtree の一時停止で止まっているか。自分か祖先が `paused_at` を
    /// 持つ、または祖先が `halted_projects`（paused / cancelled / archived の案件）に属するなら `true`。
    /// 祖先は `parent_id`、無ければ木の親（`tree.parent_unit.task_id`。採用で `parent_id` を書き換えない子）
    /// を辿る。循環と深すぎる鎖は `MAX_ANCESTRY` で打ち切る（壊れた行で dispatch 全体を止めない）。
    fn halted_by_ancestry_locked(
        conn: &Connection,
        task: &Task,
        halted_projects: &std::collections::HashSet<String>,
    ) -> Result<bool, StoreError> {
        const MAX_ANCESTRY: usize = 32;
        if task.paused_at.is_some() {
            return Ok(true);
        }
        let parent_of = |t: &Task| {
            t.parent_id.or_else(|| {
                t.tree
                    .as_ref()
                    .and_then(|tree| tree.parent_unit.as_ref())
                    .map(|u| u.task_id)
            })
        };
        let mut seen = std::collections::HashSet::new();
        let mut next = parent_of(task);
        while let Some(id) = next {
            if !seen.insert(id) || seen.len() > MAX_ANCESTRY {
                break;
            }
            let Some(ancestor) = Self::get_locked(conn, id)? else {
                break;
            };
            if ancestor.paused_at.is_some()
                || ancestor
                    .project_id
                    .is_some_and(|p| halted_projects.contains(&p.to_string()))
            {
                return Ok(true);
            }
            next = parent_of(&ancestor);
        }
        Ok(false)
    }

    fn milestone_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Milestone, StoreError>> {
        let id: String = row.get(0)?;
        let project_id: String = row.get(1)?;
        let status_col: String = row.get(5)?;
        let created_at: String = row.get(6)?;
        let updated_at: String = row.get(7)?;
        // ADR-0044 D6（Phase 55）: 8 列目 `paused_from`。
        let paused_from_col: Option<String> = row.get(8)?;
        let (Ok(id), Ok(project_id), Some(status)) = (
            id.parse::<MilestoneId>(),
            project_id.parse::<ProjectId>(),
            MilestoneStatus::parse(&status_col),
        ) else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid milestone row: id={id} project_id={project_id} status={status_col}"
            ))));
        };
        let paused_from = match paused_from_col.as_deref() {
            Some(raw) => match MilestoneStatus::parse(raw) {
                Some(parsed) => Some(parsed),
                None => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid milestone paused_from for {id}: {raw}"
                    ))));
                }
            },
            None => None,
        };
        Ok((|| {
            Ok(Milestone {
                id,
                project_id,
                seq: row.get(2)?,
                title: row.get(3)?,
                description: row.get(4)?,
                status,
                paused_from,
                // ADR-0074 D3.3（Phase F4b）: 10 列目 `plan_key`。
                plan_key: row.get(9)?,
                created_at: parse_rfc3339(&created_at)?,
                updated_at: parse_rfc3339(&updated_at)?,
            })
        })())
    }

    /// ADR-0033 D4（Phase 24）: `messages` の 1 行。
    fn message_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Message, StoreError>> {
        let id: String = row.get(0)?;
        let project_id: Option<String> = row.get(2)?;
        let role_col: String = row.get(3)?;
        let created_at: String = row.get(6)?;
        let (Ok(id), Some(role)) = (id.parse::<MessageId>(), MessageRole::parse(&role_col)) else {
            return Ok(Err(StoreError::Invalid(format!(
                "invalid message row: id={id} role={role_col}"
            ))));
        };
        let project_id = match project_id {
            Some(raw) => match raw.parse::<ProjectId>() {
                Ok(p) => Some(p),
                Err(_) => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid message row: id={id} project_id={raw}"
                    ))));
                }
            },
            None => None,
        };
        // migration 0007（R4）: 対話用タスクの id。導入前の行は NULL。
        let task_id: Option<String> = row.get(7)?;
        let task_id = match task_id {
            Some(raw) => match raw.parse::<TaskId>() {
                Ok(t) => Some(t),
                Err(_) => {
                    return Ok(Err(StoreError::Invalid(format!(
                        "invalid message row: id={id} task_id={raw}"
                    ))));
                }
            },
            None => None,
        };
        // ADR-0048 D3（Phase 60b）: `metadata_json`（actions の実行結果）は補助表示用なので、
        // 形が壊れていても run は落とさない（`None` として読む）。
        let metadata_json: Option<String> = row.get(8)?;
        let metadata = metadata_json.and_then(|raw| serde_json::from_str(&raw).ok());
        Ok((|| {
            Ok(Message {
                id,
                node_id: row.get(1)?,
                project_id,
                role,
                text: row.get(4)?,
                run_id: row.get(5)?,
                task_id,
                metadata,
                created_at: parse_rfc3339(&created_at)?,
            })
        })())
    }

    /// ADR-0033 D3/D5: `report.rs`・`approval.rs`（`reports`・`approvals`・`standing_rules` 表の SQL）
    /// も同じ接続を使うので crate 内に公開する。
    pub(crate) fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.conn.lock().map_err(|_| StoreError::Poisoned)
    }

    fn row_to_task(json: String) -> Result<Task, StoreError> {
        Ok(serde_json::from_str(&json)?)
    }

    /// 既にロック済みの connection を使ってタスクを取得する内部ヘルパー。
    /// `get()` が再度 Mutex をロックしないようにするために分離してある。
    pub(crate) fn get_locked(conn: &Connection, id: TaskId) -> Result<Option<Task>, StoreError> {
        let json: Option<String> = conn
            .query_row(
                "SELECT json FROM tasks WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(j) => Ok(Some(Self::row_to_task(j)?)),
            None => Ok(None),
        }
    }

    /// `insert` の本体（トランザクション内でも使えるよう `Connection` を受ける）。
    fn insert_tx(conn: &Connection, task: &Task) -> Result<(), StoreError> {
        let json = serde_json::to_string(task)?;
        let created_at = format_rfc3339(task.created_at)?;
        let updated_at = format_rfc3339(task.updated_at)?;
        let (lease_worker_run_id, lease_expires_at) = match &task.lease {
            Some(lease) => (
                Some(lease.worker_run_id.clone()),
                Some(format_rfc3339(lease.expires_at)?),
            ),
            None => (None, None),
        };
        // ADR-0043 D2: `repos_json` は索引（正は `json` の中の `repos`）。空なら NULL。
        let repos_json = if task.repos.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&task.repos)?)
        };
        conn.execute(
            "INSERT INTO tasks (id, status, kind, parent_id, priority, created_at, \
             lease_worker_run_id, lease_expires_at, json, title, updated_at, objective, genre, \
             project_id, milestone_id, assignee, repos_json, labels_json, category, skills_json, mode, \
             root_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
            params![
                task.id.to_string(),
                status_str(task.status),
                kind_str(task.kind),
                task.parent_id.map(|p| p.to_string()),
                task.priority,
                created_at,
                lease_worker_run_id,
                lease_expires_at,
                json,
                task.title,
                updated_at,
                // objective / genre は作成後に変わらないので、列を書くのは挿入時だけ（ADR-0014 D2, ADR-0027 D1）。
                task.objective,
                task.genre,
                // ADR-0033 D2: 案件・途中目標・担当も作成後に変わらないので、列を書くのは挿入時だけ。
                task.project_id.map(|p| p.to_string()),
                task.milestone_id.map(|m| m.to_string()),
                task.assignee.clone(),
                repos_json,
                // ADR-0044 D3（Phase 53）: ラベルと種類は `PATCH /tasks/{id}` で変わるので、
                // `update_task_tx` が同じ 2 列を書き直す。
                serde_json::to_string(&task.labels)?,
                task.category.as_str(),
                // ADR-0046 D2 / D4（Phase 59）: skills と mode も `PATCH /tasks/{id}` で変わるので、
                // `update_task_tx` が同じ 2 列を書き直す。
                serde_json::to_string(&task.skills)?,
                task.mode.as_str(),
                // ADR-0079 D4 (4)（migration 0031）: `Task.tree.root_id` の写し（木に属さない task は NULL）。
                task.tree.as_ref().map(|t| t.root_id.to_string()),
            ],
        )?;
        Ok(())
    }

    /// ADR-0044 D1（Phase 53）: `PATCH /tasks/{id}` の書き戻し。`json`（正本）と、絞り込みのための
    /// 写しの列を**全部**書き直す（挿入時にしか書いていなかった `objective` / `genre` / `project_id` /
    /// `milestone_id` / `assignee` も、編集で変わりうるのでここで揃える）。状態機械は通らない
    /// （`status` / `attempts` / `lease` は触らない）。
    /// `execution_plan_adopt` / `execution_plan_adopt_delegating` の共通部分（tx の中で計画・WU・events を書く）。
    fn adopt_plan_tx(
        tx: &Connection,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
    ) -> Result<(), StoreError> {
        let existing: i64 = tx.query_row(
            "SELECT COUNT(*) FROM execution_plans WHERE task_id = ?1 AND status = 'active'",
            params![task_id.to_string()],
            |r| r.get(0),
        )?;
        if existing > 0 {
            return Err(StoreError::InUse {
                kind: "execution_plan",
                id: task_id.to_string(),
                detail: "task already has an active execution plan (replan is Phase E4)"
                    .to_string(),
            });
        }
        tx.execute(
            "INSERT INTO execution_plans (id, task_id, version, origin, planner_run_id, status, \
             json, created_at, superseded_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                plan.id,
                plan.task_id,
                plan.version,
                plan.origin.as_str(),
                plan.planner_run_id,
                plan.status.as_str(),
                serde_json::to_string(&plan.spec)?,
                plan.created_at,
                plan.superseded_at,
            ],
        )?;
        for wu in &work_units {
            Self::insert_work_unit_tx(tx, wu)?;
        }
        for ev in &extra_events {
            Self::append_event_tx(tx, task_id, ev)?;
        }
        Self::append_event_tx(tx, task_id, &event)?;
        Ok(())
    }

    /// ADR-0079 D15（Phase R5b-prep）: 採用する task が今も `expect_status` で、木に属していない（`tree` が無い）か。
    fn tree_adoption_ok_tx(tx: &Connection, a: &TreeAdoption) -> Result<bool, StoreError> {
        Ok(Self::get_locked(tx, a.task.id)?
            .is_some_and(|current| current.status == a.expect_status && current.tree.is_none()))
    }

    /// ADR-0079 D15: 採用する task の `json` と絞り込みの列（`root_id`・`parent_id`）を書き、その task に event を積む。
    /// 状態・attempts・lease は読み直した値のまま（状態機械は通らない）。
    fn apply_tree_adoption_tx(tx: &Connection, a: &TreeAdoption) -> Result<(), StoreError> {
        let Some(current) = Self::get_locked(tx, a.task.id)? else {
            return Err(StoreError::Invalid(format!(
                "task not found: {}",
                a.task.id
            )));
        };
        let merged = Task {
            status: current.status,
            attempts: current.attempts,
            lease: current.lease.clone(),
            ..a.task.clone()
        };
        Self::update_task_tx(tx, &merged)?;
        Self::append_event_tx(tx, a.task.id, &a.event)?;
        Ok(())
    }

    fn update_task_tx(tx: &Connection, task: &Task) -> Result<(), StoreError> {
        let json = serde_json::to_string(task)?;
        let updated_at = format_rfc3339(task.updated_at)?;
        tx.execute(
            "UPDATE tasks SET json = ?1, title = ?2, updated_at = ?3, objective = ?4, genre = ?5, \
             priority = ?6, parent_id = ?7, project_id = ?8, milestone_id = ?9, assignee = ?10, \
             labels_json = ?11, category = ?12, skills_json = ?13, mode = ?14, root_id = ?16 \
             WHERE id = ?15",
            params![
                json,
                task.title,
                updated_at,
                task.objective,
                task.genre,
                task.priority,
                task.parent_id.map(|p| p.to_string()),
                task.project_id.map(|p| p.to_string()),
                task.milestone_id.map(|m| m.to_string()),
                task.assignee.clone(),
                serde_json::to_string(&task.labels)?,
                task.category.as_str(),
                serde_json::to_string(&task.skills)?,
                task.mode.as_str(),
                task.id.to_string(),
                task.tree.as_ref().map(|t| t.root_id.to_string()),
            ],
        )?;
        Ok(())
    }

    fn repair_apply(
        &self,
        task_id: TaskId,
        trigger: Trigger,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(plan) = &new_plan {
            tx.execute(
                "INSERT INTO execution_plans (id, task_id, version, origin, planner_run_id, \
                 status, json, created_at, superseded_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    plan.id,
                    plan.task_id,
                    plan.version,
                    plan.origin.as_str(),
                    plan.planner_run_id,
                    plan.status.as_str(),
                    serde_json::to_string(&plan.spec)?,
                    plan.created_at,
                    plan.superseded_at
                ],
            )?;
        }
        for wu in &work_units {
            Self::insert_work_unit_tx(&tx, wu)?;
        }
        let outcome = Self::apply_transition_tx(&tx, task_id, trigger, extra_events)?;
        tx.commit()?;
        Ok(outcome)
    }

    /// `apply_transition_with_events` の本体（ADR-0004 D1 / ADR-0005 D4）。`tx` 内で任意のトリガーを
    /// 検証し、tasks の更新と Event::Transitioned (+ extra_events) の追記を行う。commit は呼び出し側。
    pub(crate) fn apply_transition_tx(
        tx: &Connection,
        task_id: TaskId,
        trigger: Trigger,
        extra_events: Vec<Event>,
    ) -> Result<Outcome, StoreError> {
        // ADR-0004 D1 / ADR-0005 D4: 任意のトリガーを検証し、tasks の更新と
        // Event::Transitioned (+ extra_events) の追記を単一トランザクションで行う。

        let json: Option<String> = tx
            .query_row(
                "SELECT json FROM tasks WHERE id = ?1",
                params![task_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let json = match json {
            Some(j) => j,
            None => return Err(StoreError::Invalid(format!("task not found: {task_id}"))),
        };
        let mut task = Self::row_to_task(json)?;

        let view = StateView {
            kind: task.kind,
            status: task.status,
            attempts: task.attempts,
            max_retries: task.budget.max_retries,
        };
        let outcome = transition(&view, &trigger)?;
        // ADR-0080 D4: browser の wait が未解決の間は、一般の回答・途中確認の再開で `ready` に戻さない
        // （解除は専用の browser 操作〈`BrowserResume` / `BrowserFail`〉だけ）。
        if matches!(trigger, Trigger::Answer | Trigger::PhaseResume { .. })
            && crate::browser_wait::has_pending_wait_tx(tx, task_id)?
        {
            return Err(StoreError::InvalidTransition(
                crate::transition::InvalidTransition {
                    status: view.status,
                    kind: view.kind,
                    trigger: "browser_wait_pending",
                },
            ));
        }

        let now = OffsetDateTime::now_utc();
        // ADR-0002 D1: running から出る全遷移でリースを解放する。
        let leaving_running = view.status == Status::Running && outcome.next != Status::Running;
        task.status = outcome.next;
        task.attempts = outcome.attempts;
        task.updated_at = now;
        if leaving_running {
            task.lease = None;
        }

        let new_json = serde_json::to_string(&task)?;
        let updated_at_str = format_rfc3339(task.updated_at)?;
        if leaving_running {
            tx.execute(
                "UPDATE tasks SET status = ?1, lease_worker_run_id = NULL, \
                 lease_expires_at = NULL, json = ?2, title = ?3, updated_at = ?4 WHERE id = ?5",
                params![
                    status_str(task.status),
                    new_json,
                    task.title,
                    updated_at_str,
                    task_id.to_string()
                ],
            )?;
        } else {
            tx.execute(
                "UPDATE tasks SET status = ?1, json = ?2, title = ?3, updated_at = ?4 WHERE id = ?5",
                params![status_str(task.status), new_json, task.title, updated_at_str, task_id.to_string()],
            )?;
        }

        let mut next_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM events WHERE task_id = ?1",
            params![task_id.to_string()],
            |row| row.get(0),
        )?;

        let transitioned = Event::Transitioned {
            from: view.status,
            to: outcome.next,
            reason: outcome.reason.to_string(),
        };
        let ts = format_rfc3339(OffsetDateTime::now_utc())?;
        tx.execute(
            "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, ?3, ?4)",
            params![
                task_id.to_string(),
                next_seq,
                ts,
                serde_json::to_string(&transitioned)?
            ],
        )?;
        next_seq += 1;

        for event in extra_events {
            let ts = format_rfc3339(OffsetDateTime::now_utc())?;
            tx.execute(
                "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, ?3, ?4)",
                params![
                    task_id.to_string(),
                    next_seq,
                    ts,
                    serde_json::to_string(&event)?
                ],
            )?;
            Self::close_run_row_for_event_tx(tx, &event, &ts)?;
            Self::apply_tree_event_tx(tx, task_id, &event, &ts)?;
            next_seq += 1;
        }

        // Phase F7（ADR-0033 D5 追記 2026-09-28）: 終端になったら、このタスクの未決の認可の要求を
        // 同じトランザクションで `withdrawn` に閉じる（答える相手がいない要求を一覧・バッジに残さない）。
        // 伝播（子・後続の cancel）もこの関数を通るので、子の分も同じく閉じる。
        if !view.status.is_terminal() && outcome.next.is_terminal() {
            crate::approval::withdraw_pending_for_task_tx(
                tx,
                task_id,
                outcome.next,
                crate::approval::WITHDRAWN_BY_TRANSITION,
                now,
            )?;
        }
        // ADR-0080 D4: 終端（cancel・連鎖の中止を含む）になったら、未解決の browser wait を
        // `cancelled` に閉じる（未消費の承認も同時に失効する）。
        if !view.status.is_terminal() && outcome.next.is_terminal() {
            crate::browser_wait::cancel_open_for_task_tx(tx, task_id, now)?;
        }

        Self::cascade_after_transition_tx(tx, &task, view.status, outcome.next)?;

        Ok(outcome)
    }

    /// 終端化に伴う伝播（ADR-0010 D2）。呼び出し元と同一トランザクションで、再帰的に行う。
    /// 1. `Approval` が failed/cancelled → 終端でない直接の子を `Cancel`（ADR-0008 D1 を reject 以外にも拡張）
    /// 2. `Approval` 以外が終端 → 終端でない直接の `Approval` 子を `Cancel`（P-37）
    /// 3. failed/cancelled → 終端でない後続（`depends_on` に含むタスク）を `DependencyFailed`（P-9、推移的）
    fn cascade_after_transition_tx(
        tx: &Connection,
        task: &Task,
        from: Status,
        to: Status,
    ) -> Result<(), StoreError> {
        if from.is_terminal() || !to.is_terminal() {
            return Ok(());
        }
        let unsuccessful = matches!(to, Status::Failed | Status::Cancelled);
        if task.kind == TaskKind::Approval {
            if unsuccessful {
                for child in Self::non_terminal_children_tx(tx, task.id, None)? {
                    Self::transition_if_non_terminal_tx(tx, child, Trigger::Cancel)?;
                }
            }
        } else {
            for child in Self::non_terminal_children_tx(tx, task.id, Some(TaskKind::Approval))? {
                Self::transition_if_non_terminal_tx(tx, child, Trigger::Cancel)?;
            }
        }
        // ADR-0079 §7 R1b（D4・D16）: 木の親が中止・失敗で終わったら、親の計画の unit から作った子 task
        // （`root_id` を持つ `parent_id = 親` の task）を同じトランザクションで中止する。子の遷移がさらに
        // 孫へ連鎖する（subtree 全体）。走っている run は dispatcher の `abort_stale_runs` が止める。
        if unsuccessful {
            for child in Self::non_terminal_tree_children_tx(tx, task.id)? {
                Self::transition_if_non_terminal_tx(tx, child, Trigger::ParentCancelled)?;
            }
        }
        if unsuccessful {
            for dependent in Self::non_terminal_dependents_tx(tx, task.id)? {
                // P-78（ADR-0033 D4 / Phase 28）: 対話タスクは `DependencyFailed` の対象から外す
                // （直列化の順番待ちだけなので、前の対話タスクの失敗を理由に次を `cancelled` にしない。
                // `ready_tasks` 側が「終端に達していれば進めてよい」を見る）。
                if let Some(dep_task) = Self::get_locked(tx, dependent)?
                    && is_conversation(&dep_task)
                {
                    continue;
                }
                Self::transition_if_non_terminal_tx(tx, dependent, Trigger::DependencyFailed)?;
            }
        }
        Ok(())
    }

    /// 伝播の途中で既に終端になったタスク（例: 子でもあり後続でもある）は飛ばす。
    fn transition_if_non_terminal_tx(
        tx: &Connection,
        id: TaskId,
        trigger: Trigger,
    ) -> Result<(), StoreError> {
        match Self::get_locked(tx, id)? {
            Some(t) if !t.status.is_terminal() => {
                Self::apply_transition_tx(tx, id, trigger, vec![])?;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    const NON_TERMINAL_SQL: &'static str = "status NOT IN ('done', 'failed', 'cancelled')";

    fn parse_id(id_str: &str) -> Result<TaskId, StoreError> {
        id_str
            .parse()
            .map_err(|_| StoreError::Invalid(format!("invalid task id in tasks table: {id_str}")))
    }

    /// `parent_id` の直接の子のうち終端でないもの（`kind` 指定があればその kind だけ）。
    fn non_terminal_children_tx(
        tx: &Connection,
        parent_id: TaskId,
        kind: Option<TaskKind>,
    ) -> Result<Vec<TaskId>, StoreError> {
        let sql = format!(
            "SELECT id FROM tasks WHERE parent_id = ?1 AND {} AND (?2 IS NULL OR kind = ?2)",
            Self::NON_TERMINAL_SQL
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(params![parent_id.to_string(), kind.map(kind_str)], |row| {
            row.get::<_, String>(0)
        })?;
        let ids: Vec<String> = rows.collect::<Result<_, _>>()?;
        ids.iter().map(|s| Self::parse_id(s)).collect()
    }

    /// ADR-0079（Phase R1b）: `parent_id` の木の子（`root_id` を持つ = 計画の unit から作った子）のうち
    /// 終端でないもの。
    fn non_terminal_tree_children_tx(
        tx: &Connection,
        parent_id: TaskId,
    ) -> Result<Vec<TaskId>, StoreError> {
        let sql = format!(
            "SELECT id FROM tasks WHERE parent_id = ?1 AND root_id IS NOT NULL AND {} \
             ORDER BY created_at ASC, rowid ASC",
            Self::NON_TERMINAL_SQL
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(params![parent_id.to_string()], |row| {
            row.get::<_, String>(0)
        })?;
        let ids: Vec<String> = rows.collect::<Result<_, _>>()?;
        ids.iter().map(|s| Self::parse_id(s)).collect()
    }

    /// `depends_on` に `dep_id` を含む、終端でないタスク。JSON 列を `LIKE` で絞ってから型で確認する。
    fn non_terminal_dependents_tx(
        tx: &Connection,
        dep_id: TaskId,
    ) -> Result<Vec<TaskId>, StoreError> {
        let sql = format!(
            "SELECT json FROM tasks WHERE {} AND json LIKE ?1",
            Self::NON_TERMINAL_SQL
        );
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(params![format!("%{dep_id}%")], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for row in rows {
            let t = Self::row_to_task(row?)?;
            if t.id != dep_id && t.depends_on.contains(&dep_id) {
                out.push(t.id);
            }
        }
        Ok(out)
    }

    /// `depends_on` に `dep_id` を含むタスク（状態を問わない）。Phase 31（やり直し）が使う。
    fn dependents_of_tx(tx: &Connection, dep_id: TaskId) -> Result<Vec<TaskId>, StoreError> {
        let mut stmt = tx.prepare("SELECT json FROM tasks WHERE json LIKE ?1")?;
        let rows = stmt.query_map(params![format!("%{dep_id}%")], |row| {
            row.get::<_, String>(0)
        })?;
        let mut out = Vec::new();
        for row in rows {
            let t = Self::row_to_task(row?)?;
            if t.id != dep_id && t.depends_on.contains(&dep_id) {
                out.push(t.id);
            }
        }
        Ok(out)
    }

    /// `task_id` の最後の `Event::Transitioned` の `reason`。無ければ `None`。Phase 31 が「`cancelled` が
    /// `dependency_failed` 由来か」を見分けるのに使う。
    fn last_transitioned_reason_tx(
        tx: &Connection,
        task_id: TaskId,
    ) -> Result<Option<String>, StoreError> {
        let mut stmt = tx.prepare(
            "SELECT json FROM events WHERE task_id = ?1 AND json LIKE '%\"type\":\"transitioned\"%' ORDER BY seq DESC LIMIT 1",
        )?;
        let json: Option<String> = stmt
            .query_row(params![task_id.to_string()], |row| row.get(0))
            .optional()?;
        let Some(json) = json else {
            return Ok(None);
        };
        let event: Event = serde_json::from_str(&json)?;
        match event {
            Event::Transitioned { reason, .. } => Ok(Some(reason)),
            _ => Ok(None),
        }
    }

    /// `task` の `status` / `depends_on`（json 全体）/ `updated_at` を書き戻す（リースは変えない）。
    /// Phase 31 の張り替えが使う低レベルの書き込み（`transition()` を経由しない）。
    fn rewrite_task_tx(tx: &Connection, task: &Task) -> Result<(), StoreError> {
        let json = serde_json::to_string(task)?;
        let updated_at = format_rfc3339(task.updated_at)?;
        tx.execute(
            "UPDATE tasks SET status = ?1, json = ?2, title = ?3, updated_at = ?4 WHERE id = ?5",
            params![
                status_str(task.status),
                json,
                task.title,
                updated_at,
                task.id.to_string()
            ],
        )?;
        Ok(())
    }

    /// ADR-0044 D2: `task_comments` の 1 行を `TaskComment` にする。
    fn comment_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<TaskComment, StoreError>> {
        let id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let author_kind: String = row.get(2)?;
        let author: Option<String> = row.get(3)?;
        let body: String = row.get(4)?;
        let run_id: Option<String> = row.get(5)?;
        let created_at: String = row.get(6)?;
        Ok((|| {
            let Some(author_kind) = CommentAuthorKind::parse(&author_kind) else {
                return Err(StoreError::Invalid(format!(
                    "invalid author_kind in task_comments: {author_kind}"
                )));
            };
            Ok(TaskComment {
                id: id
                    .parse()
                    .map_err(|_| StoreError::Invalid(format!("invalid comment id: {id}")))?,
                task_id: Self::parse_id(&task_id)?,
                author_kind,
                author,
                body,
                run_id,
                created_at: parse_rfc3339(&created_at)?,
            })
        })())
    }

    // ---- ADR-0072 D5（Phase E2）: execution_plans / work_units / runs の行の変換・書き込み ----

    fn insert_work_unit_tx(tx: &Connection, wu: &WorkUnitRow) -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO work_units (id, task_id, plan_id, key, seq, kind, status, \
             blocked_reason, depends_on_json, runs, continuations, retries, last_run_id, \
             last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
             lease_expires_at, branch, base_commit, head_commit, integrated_commit, \
             child_task_id, needs_decisions_json) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,\
             ?21,?22,?23,?24,?25,?26)",
            params![
                wu.id,
                wu.task_id,
                wu.plan_id,
                wu.key,
                wu.seq,
                wu.kind.as_str(),
                wu.status.as_str(),
                wu.blocked_reason.map(|r| r.as_str()),
                serde_json::to_string(&wu.depends_on)?,
                wu.runs,
                wu.continuations,
                wu.retries,
                wu.last_run_id,
                wu.last_checkpoint_run_id,
                serde_json::to_string(&wu.spec)?,
                wu.created_at,
                wu.updated_at,
                wu.phase,
                wu.lease_run_id,
                wu.lease_expires_at,
                wu.branch,
                wu.base_commit,
                wu.head_commit,
                wu.integrated_commit,
                wu.child_task_id,
                serde_json::to_string(&wu.needs_decisions)?,
            ],
        )?;
        Ok(())
    }

    /// ADR-0072 D5（Phase E2/E2b）: `runs` へ 1 行挿入する共通ロジック（`run_index_start` と
    /// `runs_replace` の両方から呼ぶ）。`tx` はトランザクションでも素の `Connection` でもよい
    /// （`Transaction: Deref<Target = Connection>`）。
    fn insert_run_row_tx(tx: &Connection, row: &RunRow) -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO runs (run_id, task_id, work_unit_id, role, seq, status, adapter, \
             model, account, session_id, checkpoint_json, usage_json, metrics_json, \
             started_at, finished_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                row.run_id,
                row.task_id,
                row.work_unit_id,
                row.role.as_str(),
                row.seq,
                row.status.as_str(),
                row.adapter,
                row.model,
                row.account,
                row.session_id,
                row.checkpoint
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                row.usage.as_ref().map(serde_json::to_string).transpose()?,
                row.metrics
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                row.started_at,
                row.finished_at,
            ],
        )?;
        Ok(())
    }

    fn update_work_unit_tx(tx: &Connection, wu: &WorkUnitRow) -> Result<(), StoreError> {
        tx.execute(
            "UPDATE work_units SET plan_id = ?1, status = ?2, blocked_reason = ?3, \
             depends_on_json = ?4, runs = ?5, continuations = ?6, retries = ?7, \
             last_run_id = ?8, last_checkpoint_run_id = ?9, json = ?10, updated_at = ?11, \
             phase = ?13, lease_run_id = ?14, lease_expires_at = ?15, branch = ?16, \
             base_commit = ?17, head_commit = ?18, integrated_commit = ?19, \
             child_task_id = ?20, needs_decisions_json = ?21 \
             WHERE id = ?12",
            params![
                wu.plan_id,
                wu.status.as_str(),
                wu.blocked_reason.map(|r| r.as_str()),
                serde_json::to_string(&wu.depends_on)?,
                wu.runs,
                wu.continuations,
                wu.retries,
                wu.last_run_id,
                wu.last_checkpoint_run_id,
                serde_json::to_string(&wu.spec)?,
                wu.updated_at,
                wu.id,
                wu.phase,
                wu.lease_run_id,
                wu.lease_expires_at,
                wu.branch,
                wu.base_commit,
                wu.head_commit,
                wu.integrated_commit,
                wu.child_task_id,
                serde_json::to_string(&wu.needs_decisions)?,
            ],
        )?;
        Ok(())
    }

    fn row_to_execution_plan(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<ExecutionPlanRow, StoreError>> {
        let id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let version: u32 = row.get(2)?;
        let origin_s: String = row.get(3)?;
        let planner_run_id: Option<String> = row.get(4)?;
        let status_s: String = row.get(5)?;
        let json: String = row.get(6)?;
        let created_at: String = row.get(7)?;
        let superseded_at: Option<String> = row.get(8)?;
        Ok((|| -> Result<ExecutionPlanRow, StoreError> {
            let Some(origin) = PlanOrigin::parse(&origin_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid execution_plans.origin: {origin_s}"
                )));
            };
            let Some(status) = PlanStatus::parse(&status_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid execution_plans.status: {status_s}"
                )));
            };
            let spec: ExecutionPlanSpec = serde_json::from_str(&json)?;
            Ok(ExecutionPlanRow {
                id,
                task_id,
                version,
                origin,
                planner_run_id,
                status,
                spec,
                created_at,
                superseded_at,
            })
        })())
    }

    fn row_to_work_unit(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<WorkUnitRow, StoreError>> {
        let id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let plan_id: String = row.get(2)?;
        let key: String = row.get(3)?;
        let seq: u32 = row.get(4)?;
        let kind_s: String = row.get(5)?;
        let status_s: String = row.get(6)?;
        let blocked_reason_s: Option<String> = row.get(7)?;
        let depends_on_json: String = row.get(8)?;
        let runs: u32 = row.get(9)?;
        let continuations: u32 = row.get(10)?;
        let retries: u32 = row.get(11)?;
        let last_run_id: Option<String> = row.get(12)?;
        let last_checkpoint_run_id: Option<String> = row.get(13)?;
        let json: String = row.get(14)?;
        let created_at: String = row.get(15)?;
        let updated_at: String = row.get(16)?;
        let phase: Option<String> = row.get(17)?;
        let lease_run_id: Option<String> = row.get(18)?;
        let lease_expires_at: Option<String> = row.get(19)?;
        let branch: Option<String> = row.get(20)?;
        let base_commit: Option<String> = row.get(21)?;
        let head_commit: Option<String> = row.get(22)?;
        let integrated_commit: Option<String> = row.get(23)?;
        // ADR-0079（migration 0031）。
        let child_task_id: Option<String> = row.get(24)?;
        let needs_decisions_json: String = row.get(25)?;
        Ok((|| -> Result<WorkUnitRow, StoreError> {
            let Some(kind) = WorkUnitKind::parse(&kind_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid work_units.kind: {kind_s}"
                )));
            };
            let Some(status) = WorkUnitStatus::parse(&status_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid work_units.status: {status_s}"
                )));
            };
            let blocked_reason = blocked_reason_s
                .map(|s| {
                    WorkUnitBlockedReason::parse(&s).ok_or_else(|| {
                        StoreError::Invalid(format!("invalid work_units.blocked_reason: {s}"))
                    })
                })
                .transpose()?;
            let depends_on: Vec<String> = serde_json::from_str(&depends_on_json)?;
            let spec: WorkUnitSpec = serde_json::from_str(&json)?;
            let needs_decisions: Vec<String> = serde_json::from_str(&needs_decisions_json)?;
            Ok(WorkUnitRow {
                id,
                task_id,
                plan_id,
                key,
                seq,
                kind,
                status,
                blocked_reason,
                depends_on,
                runs,
                continuations,
                retries,
                last_run_id,
                last_checkpoint_run_id,
                spec,
                created_at,
                updated_at,
                phase,
                lease_run_id,
                lease_expires_at,
                branch,
                base_commit,
                head_commit,
                integrated_commit,
                child_task_id,
                needs_decisions,
            })
        })())
    }

    fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<RunRow, StoreError>> {
        let run_id: String = row.get(0)?;
        let task_id: String = row.get(1)?;
        let work_unit_id: Option<String> = row.get(2)?;
        let role_s: String = row.get(3)?;
        let seq: u32 = row.get(4)?;
        let status_s: String = row.get(5)?;
        let adapter: Option<String> = row.get(6)?;
        let model: Option<String> = row.get(7)?;
        let account: Option<String> = row.get(8)?;
        let session_id: Option<String> = row.get(9)?;
        let checkpoint_json: Option<String> = row.get(10)?;
        let usage_json: Option<String> = row.get(11)?;
        let metrics_json: Option<String> = row.get(12)?;
        let started_at: String = row.get(13)?;
        let finished_at: Option<String> = row.get(14)?;
        Ok((|| -> Result<RunRow, StoreError> {
            let Some(role) = RunIndexRole::parse(&role_s) else {
                return Err(StoreError::Invalid(format!("invalid runs.role: {role_s}")));
            };
            let Some(status) = RunIndexStatus::parse(&status_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid runs.status: {status_s}"
                )));
            };
            let checkpoint = checkpoint_json
                .map(|j| serde_json::from_str(&j))
                .transpose()?;
            let usage = usage_json.map(|j| serde_json::from_str(&j)).transpose()?;
            let metrics = metrics_json.map(|j| serde_json::from_str(&j)).transpose()?;
            Ok(RunRow {
                run_id,
                task_id,
                work_unit_id,
                role,
                seq,
                status,
                adapter,
                model,
                account,
                session_id,
                checkpoint,
                usage,
                metrics,
                started_at,
                finished_at,
            })
        })())
    }

    pub(crate) fn append_event_tx(
        conn: &Connection,
        task_id: TaskId,
        event: &Event,
    ) -> Result<u64, StoreError> {
        let next_seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM events WHERE task_id = ?1",
            params![task_id.to_string()],
            |row| row.get(0),
        )?;
        let ts = format_rfc3339(OffsetDateTime::now_utc())?;
        let json = serde_json::to_string(event)?;
        conn.execute(
            "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, ?3, ?4)",
            params![task_id.to_string(), next_seq, ts, json],
        )?;
        Self::close_run_row_for_event_tx(conn, event, &ts)?;
        Self::apply_tree_event_tx(conn, task_id, event, &ts)?;
        Ok(next_seq as u64)
    }

    /// ADR-0079 D4 (4) / D7 / D15（Phase R1a）: 木の Event の派生の書き込み（Event と同じトランザクション）。
    /// `ChildTaskCreated` / `ChildAdopted` → `work_units.child_task_id`、`DecisionRequested` /
    /// `DecisionAnswered` / `DecisionWithdrawn` → `decisions`。畳み込みは `task_core::decision::DecisionRow`
    /// の関数を通すので、`task_ops::replay::rebuild_decisions` の再構築と同じ行になる。
    fn apply_tree_event_tx(
        conn: &Connection,
        task_id: TaskId,
        event: &Event,
        ts: &str,
    ) -> Result<(), StoreError> {
        match event {
            Event::ChildTaskCreated {
                unit_key,
                child_task_id,
                ..
            }
            | Event::ChildAdopted {
                unit_key,
                child_task_id,
                ..
            } => {
                conn.execute(
                    "UPDATE work_units SET child_task_id = ?1 WHERE task_id = ?2 AND key = ?3",
                    params![child_task_id.to_string(), task_id.to_string(), unit_key],
                )?;
            }
            Event::DecisionRequested { decision } => {
                let row = crate::decision::DecisionRow::from_request(task_id, decision, ts);
                Self::insert_decision_tx(conn, &row)?;
            }
            Event::DecisionAnswered {
                id,
                option,
                note,
                by,
            } => {
                if let Some(mut row) = Self::decision_get_tx(conn, id)? {
                    row.apply_answer(option, note.as_deref(), by, ts);
                    Self::update_decision_tx(conn, &row)?;
                }
            }
            Event::DecisionWithdrawn { id, reason } => {
                if let Some(mut row) = Self::decision_get_tx(conn, id)? {
                    row.apply_withdrawal(reason);
                    Self::update_decision_tx(conn, &row)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn insert_decision_tx(
        conn: &Connection,
        row: &crate::decision::DecisionRow,
    ) -> Result<(), StoreError> {
        conn.execute(
            "INSERT INTO decisions (id, root_id, task_id, key, kind, status, needed_before_json, \
             json, created_at, answered_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                row.id,
                row.root_id.to_string(),
                row.task_id.to_string(),
                row.key,
                row.kind.as_str(),
                row.status.as_str(),
                serde_json::to_string(&row.needed_before)?,
                serde_json::to_string(&row.request)?,
                row.created_at,
                row.answered_at,
            ],
        )?;
        Ok(())
    }

    fn update_decision_tx(
        conn: &Connection,
        row: &crate::decision::DecisionRow,
    ) -> Result<(), StoreError> {
        conn.execute(
            "UPDATE decisions SET status = ?1, json = ?2, answered_at = ?3 WHERE id = ?4",
            params![
                row.status.as_str(),
                serde_json::to_string(&row.request)?,
                row.answered_at,
                row.id,
            ],
        )?;
        Ok(())
    }

    const DECISION_COLUMNS: &'static str = "id, root_id, task_id, key, kind, status, \
        needed_before_json, json, created_at, answered_at";

    fn decision_get_tx(
        conn: &Connection,
        id: &str,
    ) -> Result<Option<crate::decision::DecisionRow>, StoreError> {
        let sql = format!(
            "SELECT {} FROM decisions WHERE id = ?1",
            Self::DECISION_COLUMNS
        );
        conn.query_row(&sql, params![id], Self::row_to_decision)
            .optional()?
            .transpose()
    }

    fn row_to_decision(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<Result<crate::decision::DecisionRow, StoreError>> {
        let id: String = row.get(0)?;
        let root_id: String = row.get(1)?;
        let task_id: String = row.get(2)?;
        let key: String = row.get(3)?;
        let kind_s: String = row.get(4)?;
        let status_s: String = row.get(5)?;
        let needed_before_json: String = row.get(6)?;
        let json: String = row.get(7)?;
        let created_at: String = row.get(8)?;
        let answered_at: Option<String> = row.get(9)?;
        Ok((|| -> Result<crate::decision::DecisionRow, StoreError> {
            let Some(kind) = crate::decision::DecisionKind::parse(&kind_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid decisions.kind: {kind_s}"
                )));
            };
            let Some(status) = crate::decision::DecisionStatus::parse(&status_s) else {
                return Err(StoreError::Invalid(format!(
                    "invalid decisions.status: {status_s}"
                )));
            };
            Ok(crate::decision::DecisionRow {
                id,
                root_id: Self::parse_id(&root_id)?,
                task_id: Self::parse_id(&task_id)?,
                key,
                kind,
                status,
                needed_before: serde_json::from_str(&needed_before_json)?,
                request: serde_json::from_str(&json)?,
                created_at,
                answered_at,
            })
        })())
    }

    /// Phase F5-fix3: `WorkerFinished` を書いたら、その run の `runs` 行がまだ `running` なら同じ
    /// トランザクションで終端にする（status は `end` から、無ければ `harness_error`。`finished_at` は
    /// event の ts）。`task_ops::replay::rebuild_work_units_and_runs` と同じ規則なので `replay --check` と
    /// 食い違わない。既に `run_index_finish` で閉じた行（checkpoint 等を持つ）には触れない。
    /// dogfood 4 回目、lease 失効の requeue（`reclaim_expired_leases`）は `WorkerFinished` だけを書き、
    /// `runs` 行は `running` のまま残っていた。どの経路で run を終えても行が閉じるよう、ここで保証する。
    fn close_run_row_for_event_tx(
        conn: &Connection,
        event: &Event,
        ts: &str,
    ) -> Result<(), StoreError> {
        let Event::WorkerFinished {
            run_id,
            usage,
            metrics,
            end,
            ..
        } = event
        else {
            return Ok(());
        };
        let status = end
            .map(RunIndexStatus::from_run_end)
            .unwrap_or(RunIndexStatus::HarnessError);
        conn.execute(
            "UPDATE runs SET status = ?1, usage_json = COALESCE(?2, usage_json), \
             metrics_json = COALESCE(?3, metrics_json), finished_at = ?4 \
             WHERE run_id = ?5 AND status = 'running'",
            params![
                status.as_str(),
                usage.as_ref().map(serde_json::to_string).transpose()?,
                metrics.as_ref().map(serde_json::to_string).transpose()?,
                ts,
                run_id,
            ],
        )?;
        Ok(())
    }
}

impl TaskStore for SqliteStore {
    fn insert(&self, task: &Task) -> Result<(), StoreError> {
        let conn = self.lock()?;
        Self::insert_tx(&conn, task)
    }

    fn get(&self, id: TaskId) -> Result<Option<Task>, StoreError> {
        self.with_read_conn(|conn| Self::get_locked(conn, id))
    }

    fn list(&self, filter: Option<Status>) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            let mut tasks = Vec::new();
            match filter {
                Some(status) => {
                    let mut stmt = conn.prepare("SELECT json FROM tasks WHERE status = ?1")?;
                    let rows =
                        stmt.query_map(params![status_str(status)], |row| row.get::<_, String>(0))?;
                    for row in rows {
                        tasks.push(Self::row_to_task(row?)?);
                    }
                }
                None => {
                    let mut stmt = conn.prepare("SELECT json FROM tasks")?;
                    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                    for row in rows {
                        tasks.push(Self::row_to_task(row?)?);
                    }
                }
            }
            Ok(tasks)
        })
    }

    fn append_event(&self, task_id: TaskId, event: &Event) -> Result<u64, StoreError> {
        let mut conn = self.lock()?;
        // seq の採番（SELECT）と INSERT を 1 つの IMMEDIATE トランザクションにする。別接続（celerisctl / API）が同じタスクに
        // 追記しても seq が衝突せず、書き込みロックは busy_timeout で待つ（Phase 9 監査）。
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let seq = Self::append_event_tx(&tx, task_id, event)?;
        tx.commit()?;
        Ok(seq)
    }

    fn events_for(&self, task_id: TaskId) -> Result<Vec<(u64, Event)>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT seq, json FROM events WHERE task_id = ?1 ORDER BY seq ASC")?;
            let rows = stmt.query_map(params![task_id.to_string()], |row| {
                let seq: i64 = row.get(0)?;
                let json: String = row.get(1)?;
                Ok((seq, json))
            })?;
            let mut events = Vec::new();
            for row in rows {
                let (seq, json) = row?;
                let event: Event = serde_json::from_str(&json)?;
                events.push((seq as u64, event));
            }
            Ok(events)
        })
    }

    fn events_for_with_global_ids(&self, task_id: TaskId) -> Result<Vec<(u64, Event)>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT id, json FROM events WHERE task_id = ?1 ORDER BY id ASC")?;
            let rows = stmt.query_map(params![task_id.to_string()], |row| {
                let id: i64 = row.get(0)?;
                let json: String = row.get(1)?;
                Ok((id, json))
            })?;
            let mut events = Vec::new();
            for row in rows {
                let (id, json) = row?;
                let event: Event = serde_json::from_str(&json)?;
                events.push((id as u64, event));
            }
            Ok(events)
        })
    }

    fn acquire_lease(
        &self,
        task_id: TaskId,
        worker_run_id: &str,
        ttl: StdDuration,
    ) -> Result<bool, StoreError> {
        // ADR-0002 D2: 遷移（ready -> running, Trigger::Dispatch）の結果は
        // `Event::Transitioned` と同一トランザクションで追記する。D8: `Dispatch`
        // は `kind == Approval` では無効（Approval は running に入らない）。
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let current: Option<(String, String, String)> = tx
            .query_row(
                "SELECT status, kind, json FROM tasks WHERE id = ?1",
                params![task_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;

        let (status_col, kind_col, json) = match current {
            Some(v) => v,
            None => return Ok(false),
        };
        if status_col != status_str(Status::Ready) || kind_col == kind_str(TaskKind::Approval) {
            return Ok(false);
        }

        let mut task = Self::row_to_task(json)?;

        let dur = time::Duration::new(ttl.as_secs() as i64, ttl.subsec_nanos() as i32);
        let now = OffsetDateTime::now_utc();
        let expires_at = now + dur;

        task.status = Status::Running;
        task.lease = Some(crate::model::Lease {
            worker_run_id: worker_run_id.to_string(),
            expires_at,
        });
        task.updated_at = now;

        let new_json = serde_json::to_string(&task)?;
        let expires_at_str = format_rfc3339(expires_at)?;
        let updated_at_str = format_rfc3339(now)?;

        let affected = tx.execute(
            "UPDATE tasks SET status = ?1, lease_worker_run_id = ?2, lease_expires_at = ?3, \
             json = ?4, updated_at = ?5 WHERE id = ?6 AND status = ?7",
            params![
                status_str(Status::Running),
                worker_run_id,
                expires_at_str,
                new_json,
                updated_at_str,
                task_id.to_string(),
                status_str(Status::Ready),
            ],
        )?;

        if affected != 1 {
            return Ok(false);
        }

        let event = Event::Transitioned {
            from: Status::Ready,
            to: Status::Running,
            reason: "dispatch".to_string(),
        };
        let next_seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM events WHERE task_id = ?1",
            params![task_id.to_string()],
            |row| row.get(0),
        )?;
        let ts = format_rfc3339(OffsetDateTime::now_utc())?;
        let event_json = serde_json::to_string(&event)?;
        tx.execute(
            "INSERT INTO events (task_id, seq, ts, json) VALUES (?1, ?2, ?3, ?4)",
            params![task_id.to_string(), next_seq, ts, event_json],
        )?;

        tx.commit()?;
        Ok(true)
    }

    fn release_lease(&self, task_id: TaskId, worker_run_id: &str) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        // 読んだ json を書き戻すので、間に別接続（celerisctl / API）の書き込みが挟まらないよう IMMEDIATE で囲む（Phase 9 監査）。
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let current: Option<(Option<String>, String)> = tx
            .query_row(
                "SELECT lease_worker_run_id, json FROM tasks WHERE id = ?1",
                params![task_id.to_string()],
                |row| Ok((row.get(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        let (lease_worker_run_id, json) = match current {
            Some(v) => v,
            None => return Ok(()),
        };

        if lease_worker_run_id.as_deref() != Some(worker_run_id) {
            return Ok(());
        }

        let mut task = Self::row_to_task(json)?;
        task.lease = None;
        task.updated_at = OffsetDateTime::now_utc();
        let new_json = serde_json::to_string(&task)?;
        let updated_at_str = format_rfc3339(task.updated_at)?;

        tx.execute(
            "UPDATE tasks SET lease_worker_run_id = NULL, lease_expires_at = NULL, json = ?1, \
             updated_at = ?2 WHERE id = ?3 AND lease_worker_run_id = ?4",
            params![new_json, updated_at_str, task_id.to_string(), worker_run_id],
        )?;
        tx.commit()?;

        Ok(())
    }

    fn halted_by_pause(&self, task: &Task) -> Result<bool, StoreError> {
        if is_conversation(task) {
            return Ok(false);
        }
        self.with_read_conn(|conn| {
            let halted_projects = Self::halted_projects_locked(conn)?;
            if task
                .project_id
                .is_some_and(|p| halted_projects.contains(&p.to_string()))
            {
                return Ok(true);
            }
            Self::halted_by_ancestry_locked(conn, task, &halted_projects)
        })
    }

    fn ready_tasks(&self, limit: usize) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            // ADR-0044 D6（Phase 55）: **一時停止・中止・アーカイブされた案件／途中目標のタスクは
            // dispatch しない**（`ready` のまま。状態機械は触らない）。案件の支援 run（計画・レビュー・
            // まとめ・報告の圧縮）も同じ `tasks` の行なので、この 1 か所で全部が止まる。
            // **対話（`is_conversation`）だけは例外**: 人が「なぜ止めたのか」を秘書と話せなくなるため、
            // 止まっている案件でも対話は起こす（判断は下の Rust 側。`Task` を読まないと見分けられない）。
            let halted_projects = Self::halted_projects_locked(conn)?;
            // ADR-0079 D13（Phase R5a）: 途中目標は凍結（新しく paused / cancelled にする入口は無い）。既存の
            // paused / cancelled の途中目標に属する task の抑止だけは残す。
            let halted_milestones = Self::halted_milestones_locked(conn)?;
            // ADR-0074 D3.2 の途中目標の Go（`milestones_awaiting_go_locked`）は ADR-0079 D13（R5a）で廃止。

            // ADR-0010 D2（P-36）: dispatch されない Approval は取得件数を占有しないよう SQL 段階で除外する。
            let mut stmt = conn.prepare(
                "SELECT json FROM tasks WHERE status = ?1 AND kind != ?2 ORDER BY priority DESC, created_at ASC",
            )?;
            let rows = stmt.query_map(
                params![status_str(Status::Ready), kind_str(TaskKind::Approval)],
                |row| row.get::<_, String>(0),
            )?;

            let mut result = Vec::new();
            for row in rows {
                let task = Self::row_to_task(row?)?;

                // ADR-0044 D6（Phase 55）: 止まっている案件・途中目標のタスクは見送る（対話は除く）。
                if !is_conversation(&task) {
                    let halted = task
                        .project_id
                        .is_some_and(|p| halted_projects.contains(&p.to_string()))
                        || task
                            .milestone_id
                            .is_some_and(|m| halted_milestones.contains(&m.to_string()));
                    if halted {
                        continue;
                    }
                    // ADR-0079 D13（Phase R5a）: subtree の一時停止。自分か祖先（`parent_id` /
                    // `tree.parent_unit` の鎖）が `paused_at` を持つか、祖先が止まっている案件に属するなら見送る。
                    if Self::halted_by_ancestry_locked(conn, &task, &halted_projects)? {
                        continue;
                    }
                }

                // P-78（ADR-0033 D4 / Phase 28）: 対話タスクの `depends_on` は返事を送った順に返すための
                // 直列化だけが目的で、前の対話タスクの成否には意味が無い。前の対話タスクが終端に達していれば
                // （`done` だけでなく `failed` / `cancelled` でも）次の対話タスクへ進めてよい。
                let is_conv = is_conversation(&task);
                let mut deps_done = true;
                for dep_id in &task.depends_on {
                    match Self::get_locked(conn, *dep_id)? {
                        Some(dep) if dep.status == Status::Done => {}
                        Some(dep) if is_conv && dep.status.is_terminal() => {}
                        _ => {
                            deps_done = false;
                            break;
                        }
                    }
                }
                if !deps_done {
                    continue;
                }

                if let Some(parent_id) = task.parent_id
                    && let Some(parent) = Self::get_locked(conn, parent_id)?
                    && parent.kind == TaskKind::Approval
                    && parent.status != Status::Done
                {
                    continue;
                }

                result.push(task);
                if result.len() >= limit {
                    break;
                }
            }

            Ok(result)
        })
    }

    fn apply_transition_with_events(
        &self,
        task_id: TaskId,
        trigger: Trigger,
        extra_events: Vec<Event>,
    ) -> Result<Outcome, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let outcome = Self::apply_transition_tx(&tx, task_id, trigger, extra_events)?;
        tx.commit()?;
        Ok(outcome)
    }

    fn complete_plan(
        &self,
        plan_id: TaskId,
        verdict_events: Vec<Event>,
        children: Vec<Task>,
        accept_children: bool,
    ) -> Result<Outcome, StoreError> {
        // ADR-0007 D3: 子の insert + Created (+ Accept) と親の ReviewPass を単一トランザクションで行う。
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for child in &children {
            if child.parent_id != Some(plan_id) {
                return Err(StoreError::Invalid(format!(
                    "child {} does not belong to plan {plan_id}",
                    child.id
                )));
            }
            Self::insert_tx(&tx, child)?;
            Self::append_event_tx(
                &tx,
                child.id,
                &Event::Created {
                    task: Box::new(child.clone()),
                    origin: None,
                },
            )?;
            if accept_children {
                Self::apply_transition_tx(&tx, child.id, Trigger::Accept, vec![])?;
            }
        }
        let outcome = Self::apply_transition_tx(&tx, plan_id, Trigger::ReviewPass, verdict_events)?;
        tx.commit()?;
        Ok(outcome)
    }

    fn create_task(&self, task: &Task, extra_events: Vec<Event>) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::insert_tx(&tx, task)?;
        Self::append_event_tx(
            &tx,
            task.id,
            &Event::Created {
                task: Box::new(task.clone()),
                origin: None,
            },
        )?;
        for event in &extra_events {
            Self::append_event_tx(&tx, task.id, event)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn project_plan_decide_apply(
        &self,
        plan_task_id: TaskId,
        milestones: &[MilestoneId],
        milestone_status: MilestoneStatus,
        tasks: &[TaskId],
        trigger: Trigger,
        decided_event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = format_rfc3339(OffsetDateTime::now_utc())?;
        for id in milestones {
            let affected = tx.execute(
                "UPDATE milestones SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![milestone_status.as_str(), now, id.to_string()],
            )?;
            if affected != 1 {
                return Err(StoreError::Invalid(format!("milestone not found: {id}")));
            }
        }
        for id in tasks {
            let Some(task) = Self::get_locked(&tx, *id)? else {
                return Err(StoreError::Invalid(format!("task not found: {id}")));
            };
            // `Trigger::Cancel` は後続へカスケードする（ADR-0010 D2）ので、先に処理した兄弟の
            // cancel で既に終端になったものは飛ばす。
            if task.status != Status::Draft {
                continue;
            }
            Self::apply_transition_tx(&tx, *id, trigger.clone(), vec![])?;
        }
        Self::append_event_tx(&tx, plan_task_id, &decided_event)?;
        tx.commit()?;
        Ok(())
    }

    fn project_plan_apply(&self, apply: &ProjectPlanApply) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = format_rfc3339(OffsetDateTime::now_utc())?;
        for m in &apply.milestones {
            let affected = tx.execute(
                "UPDATE milestones SET status = ?1, updated_at = ?2, \
                 title = COALESCE(?3, title), description = COALESCE(?4, description) WHERE id = ?5",
                params![
                    m.status.as_str(),
                    now,
                    m.title.as_deref(),
                    m.description.as_deref(),
                    m.id.to_string()
                ],
            )?;
            if affected != 1 {
                return Err(StoreError::Invalid(format!(
                    "milestone not found: {}",
                    m.id
                )));
            }
        }
        for task in &apply.task_updates {
            let Some(current) = Self::get_locked(&tx, task.id)? else {
                return Err(StoreError::Invalid(format!("task not found: {}", task.id)));
            };
            if !matches!(current.status, Status::Draft | Status::Ready) || current.lease.is_some() {
                return Err(StoreError::Invalid(format!(
                    "task {} was already dispatched ({:?}); it cannot be modified",
                    task.id, current.status
                )));
            }
            let merged = Task {
                status: current.status,
                attempts: current.attempts,
                lease: current.lease.clone(),
                ..task.clone()
            };
            Self::update_task_tx(&tx, &merged)?;
            Self::append_event_tx(
                &tx,
                task.id,
                &Event::Edited {
                    fields: vec!["project_plan".to_string()],
                    by: "project-plan".to_string(),
                },
            )?;
        }
        for (id, trigger) in &apply.transitions {
            let Some(task) = Self::get_locked(&tx, *id)? else {
                return Err(StoreError::Invalid(format!("task not found: {id}")));
            };
            let applicable = match trigger {
                Trigger::Accept => task.status == Status::Draft,
                _ => !task.status.is_terminal(),
            };
            if !applicable {
                continue;
            }
            Self::apply_transition_tx(&tx, *id, trigger.clone(), vec![])?;
        }
        Self::append_event_tx(&tx, apply.plan_task_id, &apply.decided_event)?;
        tx.commit()?;
        Ok(())
    }

    fn delegate_children(
        &self,
        parent_id: TaskId,
        run_id: &str,
        children: Vec<Task>,
    ) -> Result<Vec<TaskId>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if Self::get_locked(&tx, parent_id)?.is_none() {
            return Err(StoreError::Invalid(format!("task not found: {parent_id}")));
        }
        let mut ids = Vec::with_capacity(children.len());
        for child in &children {
            if child.parent_id != Some(parent_id) {
                return Err(StoreError::Invalid(format!(
                    "child {} does not belong to task {parent_id}",
                    child.id
                )));
            }
            Self::insert_tx(&tx, child)?;
            Self::append_event_tx(
                &tx,
                child.id,
                &Event::Created {
                    task: Box::new(child.clone()),
                    origin: None,
                },
            )?;
            if child.status == Status::Draft {
                Self::apply_transition_tx(&tx, child.id, Trigger::Accept, vec![])?;
            }
            ids.push(child.id);
        }
        Self::append_event_tx(
            &tx,
            parent_id,
            &Event::Delegated {
                run_id: run_id.to_string(),
                task_ids: ids.clone(),
            },
        )?;
        tx.commit()?;
        Ok(ids)
    }

    fn retry_task(&self, original: TaskId, new_task: &Task) -> Result<Vec<TaskId>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(orig) = Self::get_locked(&tx, original)? else {
            return Err(StoreError::Invalid(format!("task not found: {original}")));
        };
        if !matches!(orig.status, Status::Failed | Status::Cancelled) {
            return Err(StoreError::InvalidTransition(InvalidTransition {
                status: orig.status,
                kind: orig.kind,
                trigger: "retry",
            }));
        }

        Self::insert_tx(&tx, new_task)?;
        Self::append_event_tx(
            &tx,
            new_task.id,
            &Event::Created {
                task: Box::new(new_task.clone()),
                origin: None,
            },
        )?;
        Self::append_event_tx(&tx, new_task.id, &Event::Retried { from: original })?;

        let mut rewired = Vec::new();
        for dep_id in Self::dependents_of_tx(&tx, original)? {
            let Some(dep) = Self::get_locked(&tx, dep_id)? else {
                continue;
            };
            let eligible = match dep.status {
                Status::Draft | Status::Ready | Status::Blocked => true,
                Status::Cancelled => {
                    Self::last_transitioned_reason_tx(&tx, dep_id)?.as_deref()
                        == Some(Trigger::DependencyFailed.name())
                }
                _ => false,
            };
            if !eligible {
                continue;
            }
            let mut updated = dep.clone();
            updated.depends_on = updated
                .depends_on
                .iter()
                .map(|d| if *d == original { new_task.id } else { *d })
                .collect();
            let was_cancelled = updated.status == Status::Cancelled;
            if was_cancelled {
                updated.status = Status::Draft;
            }
            updated.updated_at = OffsetDateTime::now_utc();
            Self::rewrite_task_tx(&tx, &updated)?;
            if was_cancelled {
                Self::append_event_tx(
                    &tx,
                    dep_id,
                    &Event::Transitioned {
                        from: Status::Cancelled,
                        to: Status::Draft,
                        reason: "retried".to_string(),
                    },
                )?;
            }
            rewired.push(dep_id);
        }

        tx.commit()?;
        Ok(rewired)
    }

    fn children(&self, parent_id: TaskId) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            // 同じトランザクションで挿入した子（created_at が同じ）は挿入順（rowid）で返す。
            let mut stmt = conn.prepare(
                "SELECT json FROM tasks WHERE parent_id = ?1 ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = stmt.query_map(params![parent_id.to_string()], |row| {
                row.get::<_, String>(0)
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(Self::row_to_task(row?)?);
            }
            Ok(out)
        })
    }

    fn renew_lease(
        &self,
        task_id: TaskId,
        worker_run_id: &str,
        ttl: StdDuration,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut task) = Self::get_locked(&tx, task_id)? else {
            return Ok(false);
        };
        let expires_at = OffsetDateTime::now_utc()
            + time::Duration::new(ttl.as_secs() as i64, ttl.subsec_nanos() as i32);
        let ours = task.status == Status::Running
            && task.lease.as_ref().map(|l| l.worker_run_id.as_str()) == Some(worker_run_id);
        if !ours {
            // ADR-0074 D1.5（Phase F2）: v2 の並列 WU の run は Task の lease の保持者ではなく
            // WU の lease（`work_units.lease_run_id`）を持つ。その WU の lease を延ばし、Task の
            // lease の期限も（短くせずに）そこまで延ばす。
            if task.status != Status::Running {
                return Ok(false);
            }
            let expires_str = format_rfc3339(expires_at)?;
            let n = tx.execute(
                "UPDATE work_units SET lease_expires_at = ?1 WHERE task_id = ?2 \
                 AND lease_run_id = ?3 AND status = 'running'",
                params![expires_str, task_id.to_string(), worker_run_id],
            )?;
            if n == 0 {
                return Ok(false);
            }
            if let Some(lease) = task.lease.as_mut()
                && lease.expires_at < expires_at
            {
                lease.expires_at = expires_at;
                tx.execute(
                    "UPDATE tasks SET lease_expires_at = ?1, json = ?2 WHERE id = ?3",
                    params![
                        expires_str,
                        serde_json::to_string(&task)?,
                        task_id.to_string(),
                    ],
                )?;
            }
            tx.commit()?;
            return Ok(true);
        }
        task.lease = Some(crate::model::Lease {
            worker_run_id: worker_run_id.to_string(),
            expires_at,
        });
        let affected = tx.execute(
            "UPDATE tasks SET lease_expires_at = ?1, json = ?2 WHERE id = ?3 AND status = ?4 AND lease_worker_run_id = ?5",
            params![
                format_rfc3339(expires_at)?,
                serde_json::to_string(&task)?,
                task_id.to_string(),
                status_str(Status::Running),
                worker_run_id,
            ],
        )?;
        tx.commit()?;
        Ok(affected == 1)
    }

    fn events_since(&self, after_id: u64, limit: usize) -> Result<Vec<EventRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, seq, ts, json FROM events WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
            )?;
            let rows =
                stmt.query_map(params![u64_to_i64(after_id), usize_to_i64(limit)], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, task_id, seq, ts, json) = row?;
                let task_id = Self::parse_id(&task_id)?;
                let event: Event = serde_json::from_str(&json)?;
                out.push(EventRow {
                    id: id as u64,
                    task_id,
                    seq: seq as u64,
                    ts,
                    event,
                });
            }
            Ok(out)
        })
    }

    fn latest_event_id(&self) -> Result<u64, StoreError> {
        self.with_read_conn(|conn| {
            let id: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |row| {
                row.get(0)
            })?;
            Ok(id as u64)
        })
    }

    fn event_rows_for(
        &self,
        task_id: TaskId,
        after_seq: Option<u64>,
        limit: usize,
    ) -> Result<Vec<EventRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, seq, ts, json FROM events WHERE task_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
            )?;
            let after: i64 = after_seq.map(u64_to_i64).unwrap_or(-1);
            let rows = stmt.query_map(
                params![task_id.to_string(), after, usize_to_i64(limit)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )?;
            let mut out = Vec::new();
            for row in rows {
                let (id, seq, ts, json) = row?;
                let event: Event = serde_json::from_str(&json)?;
                out.push(EventRow {
                    id: id as u64,
                    task_id,
                    seq: seq as u64,
                    ts,
                    event,
                });
            }
            Ok(out)
        })
    }

    fn list_page(
        &self,
        filter: &ListFilter,
        order: ListOrder,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Page<Task>, StoreError> {
        self.with_read_conn(|conn| {
            let (filter_sql, filter_params) = filter_predicate(filter);

            let total: i64 = {
                let sql = format!("SELECT COUNT(*) FROM tasks WHERE {filter_sql}");
                conn.query_row(&sql, params_from_iter(filter_params.iter()), |row| {
                    row.get(0)
                })?
            };

            let mut where_sql = format!("({filter_sql})");
            let mut query_params = filter_params;
            if let Some(c) = cursor {
                let payload = decode_cursor(c)?;
                let (keyset_sql, keyset_params) = keyset_predicate(order, &payload);
                where_sql.push_str(&format!(" AND ({keyset_sql})"));
                query_params.extend(keyset_params);
            }

            let order_sql = order_by_sql(order);
            let fetch_limit = usize_to_i64(limit.saturating_add(1));
            let sql =
                format!("SELECT json FROM tasks WHERE {where_sql} ORDER BY {order_sql} LIMIT ?");
            query_params.push(SqlValue::Integer(fetch_limit));

            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(query_params.iter()), |row| {
                row.get::<_, String>(0)
            })?;
            let mut items = Vec::new();
            for row in rows {
                items.push(Self::row_to_task(row?)?);
            }

            let has_more = items.len() > limit;
            if has_more {
                items.truncate(limit);
            }
            let next_cursor = if has_more {
                match items.last() {
                    Some(last) => Some(encode_cursor(&CursorPayload::from_task(last)?)?),
                    None => None,
                }
            } else {
                None
            };

            Ok(Page {
                items,
                next_cursor,
                total: total as u64,
            })
        })
    }

    fn count_by_status(&self) -> Result<Vec<(Status, u64)>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM tasks GROUP BY status")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (s, c) = row?;
                out.push((parse_status(&s)?, c as u64));
            }
            Ok(out)
        })
    }

    // ---- ADR-0033 D1: 組織 ----

    fn org_list(&self) -> Result<Vec<OrgNode>, StoreError> {
        self.with_read_conn(Self::org_list_tx)
    }

    fn org_get(&self, id: &str) -> Result<Option<OrgNode>, StoreError> {
        self.with_read_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, parent_id, name, kind, genre, brief, position, created_at, updated_at, profile_json \
                     FROM org_nodes WHERE id = ?1",
                    params![id],
                    Self::org_row,
                )
                .optional()?;
            row.transpose()
        })
    }

    fn org_upsert(&self, node: &OrgNode) -> Result<OrgNode, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = Self::org_list_tx(&tx)?;
        crate::org::validate_upsert(&existing, node)?;
        let previous = existing.iter().find(|n| n.id == node.id);
        let mut stored = node.clone();
        if let Some(previous) = previous {
            stored.created_at = previous.created_at;
        }
        tx.execute(
            "INSERT INTO org_nodes (id, parent_id, name, kind, genre, brief, position, created_at, updated_at, \
             profile_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(id) DO UPDATE SET parent_id = excluded.parent_id, name = excluded.name, \
             kind = excluded.kind, genre = excluded.genre, brief = excluded.brief, \
             position = excluded.position, updated_at = excluded.updated_at, \
             profile_json = excluded.profile_json",
            params![
                stored.id,
                stored.parent_id,
                stored.name,
                stored.kind.as_str(),
                stored.genre,
                stored.brief,
                stored.position,
                format_rfc3339(stored.created_at)?,
                format_rfc3339(stored.updated_at)?,
                profile_json(&stored.profile)?,
            ],
        )?;
        tx.commit()?;
        Ok(stored)
    }

    fn org_seed(&self, nodes: &[OrgNode]) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut existing = Self::org_list_tx(&tx)?;
        for node in nodes {
            crate::org::validate_upsert(&existing, node)?;
            tx.execute(
                "INSERT INTO org_nodes (id, parent_id, name, kind, genre, brief, position, created_at, updated_at, \
                 profile_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
                 ON CONFLICT(id) DO UPDATE SET parent_id = excluded.parent_id, name = excluded.name, \
                 kind = excluded.kind, genre = excluded.genre, brief = excluded.brief, \
                 position = excluded.position, updated_at = excluded.updated_at, \
                 profile_json = excluded.profile_json",
                params![
                    node.id,
                    node.parent_id,
                    node.name,
                    node.kind.as_str(),
                    node.genre,
                    node.brief,
                    node.position,
                    format_rfc3339(node.created_at)?,
                    format_rfc3339(node.updated_at)?,
                    profile_json(&node.profile)?,
                ],
            )?;
            existing.push(node.clone());
        }
        tx.commit()?;
        Ok(())
    }

    fn org_delete(&self, id: &str) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM org_nodes WHERE id = ?1)",
            params![id],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(false);
        }
        // ADR-0033 D1: 「消すときに仕事を抱えていたら 409」。抱えている＝未終了のタスクの assignee。
        let open_tasks: i64 = tx.query_row(
            &format!(
                "SELECT COUNT(*) FROM tasks WHERE assignee = ?1 AND {}",
                Self::NON_TERMINAL_SQL
            ),
            params![id],
            |row| row.get(0),
        )?;
        if open_tasks > 0 {
            return Err(StoreError::InUse {
                kind: "org node",
                id: id.to_string(),
                detail: format!("{open_tasks} task(s) assigned to it have not finished"),
            });
        }
        let children: i64 = tx.query_row(
            "SELECT COUNT(*) FROM org_nodes WHERE parent_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if children > 0 {
            return Err(StoreError::InUse {
                kind: "org node",
                id: id.to_string(),
                detail: format!("{children} child node(s) still report to it"),
            });
        }
        tx.execute("DELETE FROM org_nodes WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(true)
    }

    // ---- ADR-0033 D2: 案件と途中目標 ----

    fn project_create(&self, project: &Project) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Phase K-1: slug は案件を作るときに決める（渡されたものは検査だけ）。
        let slug = match project.slug.as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => {
                Self::check_project_slug(&tx, &project.id.to_string(), s)?;
                s.to_string()
            }
            _ => Self::unique_project_slug(
                &tx,
                &project.title,
                &project.id.to_string(),
                project
                    .workspace
                    .as_ref()
                    .map(crate::repos::default_repo_name)
                    .as_deref(),
            )?,
        };
        tx.execute(
            "INSERT INTO projects (id, title, request, status, secretary_summary, created_at, updated_at, workspace, \
             archived_at, paused_from, auto_advance, slug) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                project.id.to_string(),
                project.title,
                project.request,
                project.status.as_str(),
                project.secretary_summary,
                format_rfc3339(project.created_at)?,
                format_rfc3339(project.updated_at)?,
                Self::project_workspace_column(project.workspace.as_ref())?,
                project.archived_at.map(format_rfc3339).transpose()?,
                project.paused_from.map(|s| s.as_str()),
                i64::from(project.auto_advance),
                slug,
            ],
        )?;
        // ADR-0043 D1: 案件の作業場所は `is_primary = 1` のリポジトリ 1 件として持つ
        // （`Project.workspace` はその写し）。`POST /projects {workspace}`（従来のフォーム）も
        // これで複数リポジトリの世界に入る。
        if let Some(location) = &project.workspace {
            let repo = ProjectRepo {
                id: RepoId::new(),
                project_id: project.id,
                name: crate::repos::default_repo_name(location),
                kind: detect_repo_kind(location),
                location: location.clone(),
                default_branch: None,
                sync: None,
                run: RepoRun::Auto,
                is_primary: true,
                created_at: project.created_at,
            };
            crate::repos::validate_upsert(&[], &repo)?;
            Self::repo_write_tx(&tx, &repo)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn project_get(&self, id: ProjectId) -> Result<Option<Project>, StoreError> {
        self.with_read_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT p.id, p.title, p.request, p.status, p.secretary_summary, p.created_at, p.updated_at, \
                     COALESCE((SELECT r.location_json FROM project_repos r \
                               WHERE r.project_id = p.id AND r.is_primary = 1 \
                               ORDER BY r.created_at ASC, r.id ASC LIMIT 1), p.workspace), \
                     p.archived_at, p.paused_from, p.auto_advance, p.slug \
                     FROM projects p WHERE p.id = ?1",
                    params![id.to_string()],
                    Self::project_row,
                )
                .optional()?;
            row.transpose()
        })
    }

    fn project_list(&self) -> Result<Vec<Project>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT p.id, p.title, p.request, p.status, p.secretary_summary, p.created_at, p.updated_at, \
                     COALESCE((SELECT r.location_json FROM project_repos r \
                               WHERE r.project_id = p.id AND r.is_primary = 1 \
                               ORDER BY r.created_at ASC, r.id ASC LIMIT 1), p.workspace), \
                     p.archived_at, p.paused_from, p.auto_advance, p.slug \
                 FROM projects p ORDER BY p.created_at DESC, p.id DESC",
            )?;
            let rows = stmt.query_map([], Self::project_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn project_set_status(&self, id: ProjectId, status: ProjectStatus) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE projects SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                status.as_str(),
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        Ok(affected == 1)
    }

    /// ADR-0044 D6（Phase 55）: `pause` / `resume` / `cancel` は状態と `paused_from` を 1 回の UPDATE で書く
    /// （`resume` が「戻り先」を読んだ後に別の書き込みが割り込まないように）。
    fn project_set_lifecycle(
        &self,
        id: ProjectId,
        status: ProjectStatus,
        paused_from: Option<Option<ProjectStatus>>,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let now = format_rfc3339(OffsetDateTime::now_utc())?;
        let affected = match paused_from {
            Some(from) => conn.execute(
                "UPDATE projects SET status = ?1, paused_from = ?2, updated_at = ?3 WHERE id = ?4",
                params![
                    status.as_str(),
                    from.map(|s| s.as_str()),
                    now,
                    id.to_string()
                ],
            )?,
            None => conn.execute(
                "UPDATE projects SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![status.as_str(), now, id.to_string()],
            )?,
        };
        Ok(affected == 1)
    }

    fn project_set_archived_at(
        &self,
        id: ProjectId,
        at: Option<OffsetDateTime>,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE projects SET archived_at = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                at.map(format_rfc3339).transpose()?,
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        Ok(affected == 1)
    }

    fn project_set_slug(&self, id: ProjectId, slug: &str) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let slug = slug.trim();
        Self::check_project_slug(&tx, &id.to_string(), slug)?;
        let now = format_rfc3339(OffsetDateTime::now_utc())?;
        let n = tx.execute(
            "UPDATE projects SET slug = ?1, updated_at = ?2 WHERE id = ?3",
            params![slug, now, id.to_string()],
        )?;
        tx.commit()?;
        Ok(n > 0)
    }

    fn project_set_text(
        &self,
        id: ProjectId,
        title: Option<&str>,
        request: Option<&str>,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE projects SET title = COALESCE(?1, title), request = COALESCE(?2, request), \
             updated_at = ?3 WHERE id = ?4",
            params![
                title,
                request,
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        Ok(affected == 1)
    }

    fn project_set_workspace(
        &self,
        id: ProjectId,
        workspace: Option<&WorkspaceSpec>,
    ) -> Result<bool, StoreError> {
        let column = Self::project_workspace_column(workspace)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let affected = tx.execute(
            "UPDATE projects SET workspace = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                column,
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        if affected != 1 {
            return Ok(false);
        }
        // ADR-0043 D1: 従来の `PATCH /projects {workspace}` は **primary のリポジトリ**を書き換える。
        let existing = Self::repo_list_tx(&tx, id)?;
        let primary = existing.iter().find(|r| r.is_primary).cloned();
        match (workspace, primary) {
            // 差し替え: primary の場所（と kind）だけを直す。名前・run・default_branch は人の設定を残す。
            (Some(location), Some(mut repo)) => {
                repo.location = location.clone();
                repo.kind = detect_repo_kind(location);
                if repo.kind == RepoKind::Dir {
                    repo.default_branch = None;
                }
                if matches!(location, WorkspaceSpec::Local { .. }) {
                    repo.sync = None;
                }
                let others: Vec<ProjectRepo> = existing
                    .iter()
                    .filter(|r| r.id != repo.id)
                    .cloned()
                    .collect();
                crate::repos::validate_upsert(&others, &repo)?;
                Self::repo_write_tx(&tx, &repo)?;
            }
            // 新規: primary がまだ無い案件に作業場所を付けた。
            (Some(location), None) => {
                let mut name = crate::repos::default_repo_name(location);
                if existing.iter().any(|r| r.name == name) {
                    name = format!("{name}-2");
                }
                let repo = ProjectRepo {
                    id: RepoId::new(),
                    project_id: id,
                    name,
                    kind: detect_repo_kind(location),
                    location: location.clone(),
                    default_branch: None,
                    sync: None,
                    run: RepoRun::Auto,
                    is_primary: true,
                    created_at: OffsetDateTime::now_utc(),
                };
                crate::repos::validate_upsert(&existing, &repo)?;
                Self::repo_write_tx(&tx, &repo)?;
                Self::repo_clear_other_primaries_tx(&tx, id, repo.id)?;
            }
            // 消す: `"workspace": null` は「案件を作業場所なしに戻す」なので primary の行を消す。
            // 未終端のタスクが使っていれば 409（`DELETE /repos/{id}` と同じ規律）。
            (None, Some(repo)) => {
                let open = Self::repo_active_tasks_tx(&tx, repo.id)?;
                if !open.is_empty() {
                    return Err(StoreError::InUse {
                        kind: "project repo",
                        id: repo.id.to_string(),
                        detail: format!("{} task(s) using it have not finished", open.len()),
                    });
                }
                tx.execute(
                    "DELETE FROM project_repos WHERE id = ?1",
                    params![repo.id.to_string()],
                )?;
            }
            (None, None) => {}
        }
        Self::sync_project_workspace_tx(&tx, id)?;
        tx.commit()?;
        Ok(true)
    }

    // ---- ADR-0043 D1（Phase 52）: 案件のリポジトリ ----

    fn repo_create(&self, repo: &ProjectRepo) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            params![repo.project_id.to_string()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StoreError::Invalid(format!(
                "project not found: {}",
                repo.project_id
            )));
        }
        let existing = Self::repo_list_tx(&tx, repo.project_id)?;
        crate::repos::validate_upsert(&existing, repo)?;
        // 最初の 1 件は自動的に primary（案件に「主なリポジトリ」が無い状態を作らない）。
        let mut repo = repo.clone();
        if existing.is_empty() {
            repo.is_primary = true;
        }
        Self::repo_write_tx(&tx, &repo)?;
        if repo.is_primary {
            Self::repo_clear_other_primaries_tx(&tx, repo.project_id, repo.id)?;
        }
        Self::sync_project_workspace_tx(&tx, repo.project_id)?;
        tx.commit()?;
        Ok(())
    }

    fn repo_get(&self, id: RepoId) -> Result<Option<ProjectRepo>, StoreError> {
        self.with_read_conn(|conn| Self::repo_get_tx(conn, id))
    }

    fn repo_list(&self, project_id: ProjectId) -> Result<Vec<ProjectRepo>, StoreError> {
        self.with_read_conn(|conn| Self::repo_list_tx(conn, project_id))
    }

    fn repo_update(&self, repo: &ProjectRepo) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(current) = Self::repo_get_tx(&tx, repo.id)? else {
            return Ok(false);
        };
        // `project_id` と `created_at` は動かさない（付け替えは作り直し）。
        let repo = ProjectRepo {
            project_id: current.project_id,
            created_at: current.created_at,
            ..repo.clone()
        };
        let others: Vec<ProjectRepo> = Self::repo_list_tx(&tx, repo.project_id)?
            .into_iter()
            .filter(|r| r.id != repo.id)
            .collect();
        crate::repos::validate_upsert(&others, &repo)?;
        Self::repo_write_tx(&tx, &repo)?;
        if repo.is_primary {
            Self::repo_clear_other_primaries_tx(&tx, repo.project_id, repo.id)?;
        }
        Self::sync_project_workspace_tx(&tx, repo.project_id)?;
        tx.commit()?;
        Ok(true)
    }

    fn repo_delete(&self, id: RepoId) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(repo) = Self::repo_get_tx(&tx, id)? else {
            return Ok(false);
        };
        let open = Self::repo_active_tasks_tx(&tx, id)?;
        if !open.is_empty() {
            return Err(StoreError::InUse {
                kind: "project repo",
                id: id.to_string(),
                detail: format!("{} task(s) using it have not finished", open.len()),
            });
        }
        tx.execute(
            "DELETE FROM project_repos WHERE id = ?1",
            params![id.to_string()],
        )?;
        // primary を消したら、残りのうち一番古いものを primary にする（案件に主なリポジトリを残す）。
        if repo.is_primary
            && let Some(next) = Self::repo_list_tx(&tx, repo.project_id)?.first()
        {
            tx.execute(
                "UPDATE project_repos SET is_primary = 1 WHERE id = ?1",
                params![next.id.to_string()],
            )?;
        }
        Self::sync_project_workspace_tx(&tx, repo.project_id)?;
        tx.commit()?;
        Ok(true)
    }

    fn repo_set_primary(&self, id: RepoId) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(repo) = Self::repo_get_tx(&tx, id)? else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE project_repos SET is_primary = 1 WHERE id = ?1",
            params![id.to_string()],
        )?;
        Self::repo_clear_other_primaries_tx(&tx, repo.project_id, id)?;
        Self::sync_project_workspace_tx(&tx, repo.project_id)?;
        tx.commit()?;
        Ok(true)
    }

    fn repo_active_tasks(&self, id: RepoId) -> Result<Vec<TaskId>, StoreError> {
        self.with_read_conn(|conn| Self::repo_active_tasks_tx(conn, id))
    }

    // ---- ADR-0043 D5（Phase 54）: 変更の取り込み ----

    fn integration_put(&self, integration: &TaskIntegration) -> Result<(), StoreError> {
        let conn = self.lock()?;
        Self::integration_put_tx(&conn, integration)
    }

    fn integration_get(&self, id: IntegrationId) -> Result<Option<TaskIntegration>, StoreError> {
        self.with_read_conn(|conn| {
            Ok(
                Self::integration_query_tx(conn, "id = ?1", params![id.to_string()])?
                    .into_iter()
                    .next(),
            )
        })
    }

    fn integration_list_for_task(
        &self,
        task_id: TaskId,
    ) -> Result<Vec<TaskIntegration>, StoreError> {
        self.with_read_conn(|conn| {
            Self::integration_query_tx(conn, "task_id = ?1", params![task_id.to_string()])
        })
    }

    fn integration_latest(
        &self,
        task_id: TaskId,
        repo: &str,
    ) -> Result<Option<TaskIntegration>, StoreError> {
        self.with_read_conn(|conn| {
            Ok(Self::integration_query_tx(
                conn,
                "task_id = ?1 AND repo_name = ?2",
                params![task_id.to_string(), repo],
            )?
            .into_iter()
            .next())
        })
    }

    fn integration_list_for_project(
        &self,
        project_id: ProjectId,
        limit: usize,
    ) -> Result<Vec<TaskIntegration>, StoreError> {
        self.with_read_conn(|conn| {
            // タスク × リポジトリごとに最新の 1 件（`created_at` が同じなら `id`〈ULID〉で決める）。
            let mut stmt = conn.prepare(&format!(
                "SELECT {cols} FROM task_integrations i \
                 JOIN tasks t ON t.id = i.task_id \
                 WHERE t.project_id = ?1 \
                   AND NOT EXISTS ( \
                     SELECT 1 FROM task_integrations n \
                     WHERE n.task_id = i.task_id AND n.repo_name = i.repo_name \
                       AND (n.created_at > i.created_at OR (n.created_at = i.created_at AND n.id > i.id)) \
                   ) \
                 ORDER BY i.created_at DESC, i.id DESC LIMIT ?2",
                cols = Self::INTEGRATION_COLUMNS
                    .split(", ")
                    .map(|c| format!("i.{c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))?;
            let rows = stmt.query_map(
                params![
                    project_id.to_string(),
                    i64::try_from(limit).unwrap_or(i64::MAX)
                ],
                Self::integration_row,
            )?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn milestone_create(
        &self,
        project_id: ProjectId,
        title: &str,
        description: &str,
        status: MilestoneStatus,
    ) -> Result<Milestone, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            params![project_id.to_string()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StoreError::Invalid(format!(
                "project not found: {project_id}"
            )));
        }
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM milestones WHERE project_id = ?1",
            params![project_id.to_string()],
            |row| row.get(0),
        )?;
        let now = OffsetDateTime::now_utc();
        let milestone = Milestone {
            plan_key: None,
            id: MilestoneId::new(),
            project_id,
            seq,
            title: title.to_string(),
            description: description.to_string(),
            status,
            paused_from: None,
            created_at: now,
            updated_at: now,
        };
        tx.execute(
            "INSERT INTO milestones (id, project_id, seq, title, description, status, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                milestone.id.to_string(),
                milestone.project_id.to_string(),
                milestone.seq,
                milestone.title,
                milestone.description,
                milestone.status.as_str(),
                format_rfc3339(milestone.created_at)?,
                format_rfc3339(milestone.updated_at)?,
            ],
        )?;
        tx.commit()?;
        Ok(milestone)
    }

    fn milestone_set_plan_key(&self, id: MilestoneId, plan_key: &str) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE milestones SET plan_key = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                plan_key,
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        Ok(affected == 1)
    }

    fn milestone_list(&self, project_id: ProjectId) -> Result<Vec<Milestone>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, project_id, seq, title, description, status, created_at, updated_at, paused_from, plan_key \
                 FROM milestones WHERE project_id = ?1 ORDER BY seq ASC",
            )?;
            let rows = stmt.query_map(params![project_id.to_string()], Self::milestone_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn milestone_get(&self, id: MilestoneId) -> Result<Option<Milestone>, StoreError> {
        self.with_read_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT id, project_id, seq, title, description, status, created_at, updated_at, paused_from, plan_key \
                     FROM milestones WHERE id = ?1",
                    params![id.to_string()],
                    Self::milestone_row,
                )
                .optional()?;
            row.transpose()
        })
    }

    fn milestone_set_status(
        &self,
        id: MilestoneId,
        status: MilestoneStatus,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE milestones SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                status.as_str(),
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string()
            ],
        )?;
        Ok(affected == 1)
    }

    fn milestone_transition_status(
        &self,
        id: MilestoneId,
        from: MilestoneStatus,
        to: MilestoneStatus,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE milestones SET status = ?1, updated_at = ?2 WHERE id = ?3 AND status = ?4",
            params![
                to.as_str(),
                format_rfc3339(OffsetDateTime::now_utc())?,
                id.to_string(),
                from.as_str(),
            ],
        )?;
        Ok(affected == 1)
    }

    /// ADR-0044 D6（Phase 55）: 状態と `paused_from` を 1 回の UPDATE で書く（`project_set_lifecycle` と同じ）。
    fn milestone_set_lifecycle(
        &self,
        id: MilestoneId,
        status: MilestoneStatus,
        paused_from: Option<Option<MilestoneStatus>>,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let now = format_rfc3339(OffsetDateTime::now_utc())?;
        let affected = match paused_from {
            Some(from) => conn.execute(
                "UPDATE milestones SET status = ?1, paused_from = ?2, updated_at = ?3 WHERE id = ?4",
                params![status.as_str(), from.map(|s| s.as_str()), now, id.to_string()],
            )?,
            None => conn.execute(
                "UPDATE milestones SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![status.as_str(), now, id.to_string()],
            )?,
        };
        Ok(affected == 1)
    }

    // ---- ADR-0033 D4（Phase 24）: 対話 ----

    fn message_append(&self, message: &Message) -> Result<(), StoreError> {
        let conn = self.lock()?;
        let metadata_json = message
            .metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        conn.execute(
            "INSERT INTO messages (id, node_id, project_id, role, text, run_id, created_at, task_id, metadata_json) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                message.id.to_string(),
                message.node_id,
                message.project_id.map(|p| p.to_string()),
                message.role.as_str(),
                message.text,
                message.run_id,
                format_rfc3339(message.created_at)?,
                message.task_id.map(|t| t.to_string()),
                metadata_json,
            ],
        )?;
        Ok(())
    }

    fn message_list(
        &self,
        node_id: &str,
        project_id: Option<ProjectId>,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError> {
        self.with_read_conn(|conn| {
            // 新しい順に `limit` 件取ってから古い順に戻す（直近のやり取りを時系列で渡すため）。
            let sql = match project_id {
                Some(_) => {
                    "SELECT id, node_id, project_id, role, text, run_id, created_at, task_id, metadata_json FROM messages \
                     WHERE node_id = ?1 AND project_id = ?2 ORDER BY created_at DESC, id DESC LIMIT ?3"
                }
                None => {
                    "SELECT id, node_id, project_id, role, text, run_id, created_at, task_id, metadata_json FROM messages \
                     WHERE node_id = ?1 AND project_id IS NULL ORDER BY created_at DESC, id DESC LIMIT ?3"
                }
            };
            let mut stmt = conn.prepare(sql)?;
            let project = project_id.map(|p| p.to_string()).unwrap_or_default();
            let rows =
                stmt.query_map(params![node_id, project, limit as i64], Self::message_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            out.reverse();
            Ok(out)
        })
    }

    /// ADR-0048 D1（Phase 60a）: Console の一本の流れ用（絞り込みは任意、`after` は閉区間）。
    fn message_page(
        &self,
        node_id: Option<&str>,
        project_id: Option<ProjectId>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Message>, StoreError> {
        self.with_read_conn(|conn| {
            let mut where_sql = String::from("1 = 1");
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(node_id) = node_id {
                where_sql.push_str(" AND node_id = ?");
                args.push(Box::new(node_id.to_string()));
            }
            if let Some(project_id) = project_id {
                where_sql.push_str(" AND project_id = ?");
                args.push(Box::new(project_id.to_string()));
            }
            if let Some(after) = after {
                where_sql.push_str(" AND created_at >= ?");
                args.push(Box::new(after.to_string()));
            }
            // `after` 有り = 古い順にその先から、無し = 新しい順に `limit` 件取って戻す。
            let order = if after.is_some() { "ASC" } else { "DESC" };
            let sql = format!(
                "SELECT id, node_id, project_id, role, text, run_id, created_at, task_id, metadata_json FROM messages \
                 WHERE {where_sql} ORDER BY created_at {order}, id {order} LIMIT ?"
            );
            args.push(Box::new(limit as i64));
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(
                params_from_iter(args.iter().map(|a| a.as_ref())),
                Self::message_row,
            )?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            if after.is_none() {
                out.reverse();
            }
            Ok(out)
        })
    }

    // ---- ADR-0048 D3（Phase 60b）: CoS の actions の冪等性 ----

    fn console_action_run_claim(
        &self,
        run_id: &str,
        task_id: TaskId,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "INSERT OR IGNORE INTO console_action_runs (run_id, task_id, executed_at) \
             VALUES (?1, ?2, ?3)",
            params![run_id, task_id.to_string(), format_rfc3339(now)?],
        )?;
        Ok(affected == 1)
    }

    // ---- ADR-0040 D4（Phase 47）: `daemon_instances` ----

    // ---- ADR-0044 D1/D2（Phase 53）----

    fn update_task(&self, task: &Task, event: Event) -> Result<Task, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(current) = Self::get_locked(&tx, task.id)? else {
            return Err(StoreError::Invalid(format!("task not found: {}", task.id)));
        };
        // 状態機械が持つ 3 つ（`status` / `attempts` / `lease`）だけは**この tx の中で読んだ行**の値を使う。
        // 編集を組み立てている間にディスパッチャが `acquire_lease` を通していたら、渡された `task` は
        // 古い `ready` / `lease: None` を持っている。そのまま書くと `json` と `status` 列が食い違い、
        // そのタスクは二度と dispatch されず run の結果も捨てられる（Phase 53 の監査で発見）。
        let merged = Task {
            status: current.status,
            attempts: current.attempts,
            lease: current.lease.clone(),
            ..task.clone()
        };
        Self::update_task_tx(&tx, &merged)?;
        Self::append_event_tx(&tx, task.id, &event)?;
        tx.commit()?;
        Ok(merged)
    }

    fn comment_add(
        &self,
        comment: &TaskComment,
        transition: Option<(Trigger, Vec<Event>)>,
    ) -> Result<Option<Outcome>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx
            .query_row(
                "SELECT 1 FROM tasks WHERE id = ?1",
                params![comment.task_id.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(StoreError::Invalid(format!(
                "task not found: {}",
                comment.task_id
            )));
        }
        tx.execute(
            "INSERT INTO task_comments (id, task_id, author_kind, author, body, run_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                comment.id.to_string(),
                comment.task_id.to_string(),
                comment.author_kind.as_str(),
                comment.author.clone(),
                comment.body,
                comment.run_id.clone(),
                format_rfc3339(comment.created_at)?,
            ],
        )?;
        let outcome = match transition {
            Some((trigger, extra_events)) => Some(Self::apply_transition_tx(
                &tx,
                comment.task_id,
                trigger,
                extra_events,
            )?),
            None => None,
        };
        tx.commit()?;
        Ok(outcome)
    }

    fn comments_for(&self, task_id: TaskId) -> Result<Vec<TaskComment>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, author_kind, author, body, run_id, created_at FROM task_comments \
                 WHERE task_id = ?1 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], Self::comment_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn instance_register(&self, instance: &DaemonInstance) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR REPLACE INTO daemon_instances \
             (instance_id, \"release\", pid, role, started_at, heartbeat_at, handoff_requested_at, drained_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                instance.instance_id,
                instance.release,
                i64::from(instance.pid),
                instance.role.as_str(),
                format_rfc3339(instance.started_at)?,
                format_rfc3339(instance.heartbeat_at)?,
                instance.handoff_requested_at.map(format_rfc3339).transpose()?,
                instance.drained_at.map(format_rfc3339).transpose()?,
            ],
        )?;
        Ok(())
    }

    fn instance_heartbeat(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET heartbeat_at = ?1 WHERE instance_id = ?2",
            params![format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    fn instance_request_handoff(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET handoff_requested_at = ?1 \
             WHERE instance_id = ?2 AND handoff_requested_at IS NULL",
            params![format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    fn instance_set_role(
        &self,
        instance_id: &str,
        role: InstanceRole,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET role = ?1, heartbeat_at = ?2 WHERE instance_id = ?3",
            params![role.as_str(), format_rfc3339(at)?, instance_id],
        )?;
        Ok(affected == 1)
    }

    fn instance_mark_drained(
        &self,
        instance_id: &str,
        at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let ts = format_rfc3339(at)?;
        let affected = conn.execute(
            "UPDATE daemon_instances SET drained_at = ?1, heartbeat_at = ?2 WHERE instance_id = ?3",
            params![ts.clone(), ts, instance_id],
        )?;
        Ok(affected == 1)
    }

    fn instance_list(&self) -> Result<Vec<DaemonInstance>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(&format!(
                "{SELECT_INSTANCE} ORDER BY started_at ASC, instance_id ASC"
            ))?;
            let rows = stmt.query_map([], row_to_instance)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn instance_delete(&self, instance_id: &str) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "DELETE FROM daemon_instances WHERE instance_id = ?1",
            params![instance_id],
        )?;
        Ok(affected == 1)
    }

    fn instance_delete_stale(
        &self,
        keep: &str,
        heartbeat_before: OffsetDateTime,
    ) -> Result<Vec<String>, StoreError> {
        let mut conn = self.lock()?;
        let before = format_rfc3339(heartbeat_before)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut removed: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT instance_id FROM daemon_instances \
                 WHERE instance_id <> ?1 AND (drained_at IS NOT NULL OR heartbeat_at < ?2)",
            )?;
            let rows = stmt.query_map(params![keep, before], |row| row.get::<_, String>(0))?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row?);
            }
            ids
        };
        removed.sort();
        for id in &removed {
            tx.execute(
                "DELETE FROM daemon_instances WHERE instance_id = ?1",
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }

    fn cluster_settings_get(
        &self,
        cluster_id: &str,
    ) -> Result<Option<ClusterSettings>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT cluster_id, work_dir, updated_at FROM cluster_settings WHERE cluster_id = ?1",
                params![cluster_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
            .map(|(cluster_id, work_dir, updated_at)| {
                Ok(ClusterSettings {
                    cluster_id,
                    work_dir,
                    updated_at: parse_rfc3339(&updated_at)?,
                })
            })
            .transpose()
        })
    }

    fn cluster_settings_list(&self) -> Result<Vec<ClusterSettings>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT cluster_id, work_dir, updated_at FROM cluster_settings ORDER BY cluster_id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (cluster_id, work_dir, updated_at) = row?;
                out.push(ClusterSettings {
                    cluster_id,
                    work_dir,
                    updated_at: parse_rfc3339(&updated_at)?,
                });
            }
            Ok(out)
        })
    }

    fn cluster_settings_set(
        &self,
        cluster_id: &str,
        work_dir: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        match work_dir {
            Some(work_dir) => {
                conn.execute(
                    "INSERT INTO cluster_settings (cluster_id, work_dir, updated_at) \
                     VALUES (?1, ?2, ?3) \
                     ON CONFLICT(cluster_id) DO UPDATE SET work_dir = excluded.work_dir, \
                     updated_at = excluded.updated_at",
                    params![cluster_id, work_dir, format_rfc3339(now)?],
                )?;
            }
            None => {
                conn.execute(
                    "DELETE FROM cluster_settings WHERE cluster_id = ?1",
                    params![cluster_id],
                )?;
            }
        }
        Ok(())
    }

    fn cluster_connection_record(
        &self,
        record: &ClusterConnectionRecord,
    ) -> Result<(), StoreError> {
        let conn = self.lock()?;
        let uptime = record
            .uptime_secs
            .map(|v| i64::try_from(v).unwrap_or(i64::MAX));
        conn.execute(
            "INSERT INTO cluster_connection_log (cluster_id, kind, method, cause, uptime_secs, at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                record.cluster_id,
                record.kind,
                record.method,
                record.cause,
                uptime,
                format_rfc3339(record.at)?
            ],
        )?;
        Ok(())
    }

    fn cluster_connection_list_since(
        &self,
        since: OffsetDateTime,
    ) -> Result<Vec<ClusterConnectionRecord>, StoreError> {
        // RFC 3339 の文字列は小数秒の桁数で順序が崩れうるので、比較は読んでから時刻で行う
        // （行は接続・切断ごとに 1 行で少ない）。
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT cluster_id, kind, method, cause, uptime_secs, at FROM cluster_connection_log \
                 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (cluster_id, kind, method, cause, uptime, at) = row?;
                let at = parse_rfc3339(&at)?;
                if at < since {
                    continue;
                }
                out.push(ClusterConnectionRecord {
                    cluster_id,
                    kind,
                    method,
                    cause,
                    uptime_secs: uptime.and_then(|v| u64::try_from(v).ok()),
                    at,
                });
            }
            Ok(out)
        })
    }

    // ---- ADR-0072 D5（Phase E2）: execution_plans / work_units / runs ----

    fn execution_plan_adopt(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::adopt_plan_tx(&tx, task_id, plan, work_units, extra_events, event)?;
        tx.commit()?;
        Ok(())
    }

    fn execution_plan_adopt_delegating(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        run_id: &str,
        children: Vec<Task>,
    ) -> Result<Vec<TaskId>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::adopt_plan_tx(&tx, task_id, plan, work_units, extra_events, event)?;
        let mut ids = Vec::with_capacity(children.len());
        for child in &children {
            if child.parent_id != Some(task_id) {
                return Err(StoreError::Invalid(format!(
                    "child {} does not belong to task {task_id}",
                    child.id
                )));
            }
            Self::insert_tx(&tx, child)?;
            Self::append_event_tx(
                &tx,
                child.id,
                &Event::Created {
                    task: Box::new(child.clone()),
                    origin: None,
                },
            )?;
            if child.status == Status::Draft {
                Self::apply_transition_tx(&tx, child.id, Trigger::Accept, vec![])?;
            }
            ids.push(child.id);
        }
        if !ids.is_empty() {
            Self::append_event_tx(
                &tx,
                task_id,
                &Event::Delegated {
                    run_id: run_id.to_string(),
                    task_ids: ids.clone(),
                },
            )?;
        }
        tx.commit()?;
        Ok(ids)
    }

    fn execution_plan_active(
        &self,
        task_id: TaskId,
    ) -> Result<Option<ExecutionPlanRow>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT id, task_id, version, origin, planner_run_id, status, json, created_at, \
                 superseded_at FROM execution_plans WHERE task_id = ?1 AND status = 'active'",
                params![task_id.to_string()],
                Self::row_to_execution_plan,
            )
            .optional()?
            .transpose()
        })
    }

    fn execution_plan_list(&self, task_id: TaskId) -> Result<Vec<ExecutionPlanRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, version, origin, planner_run_id, status, json, created_at, \
                 superseded_at FROM execution_plans WHERE task_id = ?1 ORDER BY version ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], Self::row_to_execution_plan)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn work_units_for(&self, task_id: TaskId) -> Result<Vec<WorkUnitRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units \
                 WHERE task_id = ?1 ORDER BY seq ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], Self::row_to_work_unit)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn decisions_list(
        &self,
        root_id: Option<TaskId>,
    ) -> Result<Vec<crate::decision::DecisionRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut out = Vec::new();
            match root_id {
                Some(root) => {
                    let sql = format!(
                        "SELECT {} FROM decisions WHERE root_id = ?1 ORDER BY created_at, id",
                        Self::DECISION_COLUMNS
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map(params![root.to_string()], Self::row_to_decision)?;
                    for row in rows {
                        out.push(row??);
                    }
                }
                None => {
                    let sql = format!(
                        "SELECT {} FROM decisions ORDER BY created_at, id",
                        Self::DECISION_COLUMNS
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map([], Self::row_to_decision)?;
                    for row in rows {
                        out.push(row??);
                    }
                }
            }
            Ok(out)
        })
    }

    fn decision_get(&self, id: &str) -> Result<Option<crate::decision::DecisionRow>, StoreError> {
        self.with_read_conn(|conn| Self::decision_get_tx(conn, id))
    }

    fn decisions_replace(&self, rows: Vec<crate::decision::DecisionRow>) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM decisions", [])?;
        for row in &rows {
            Self::insert_decision_tx(&tx, row)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn decision_resolve_apply(
        &self,
        task_id: TaskId,
        decision_id: &str,
        expect: crate::decision::DecisionStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match Self::decision_get_tx(&tx, decision_id)? {
            Some(row) if row.status == expect && row.task_id == task_id => {}
            _ => return Ok(false),
        }
        for wu in &updated {
            Self::update_work_unit_tx(&tx, wu)?;
        }
        for ev in &events {
            Self::append_event_tx(&tx, task_id, ev)?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn work_unit_get(&self, id: &str) -> Result<Option<WorkUnitRow>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units WHERE id = ?1",
                params![id],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()
        })
    }

    fn work_unit_transition(
        &self,
        task_id: TaskId,
        updated: WorkUnitRow,
        event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::update_work_unit_tx(&tx, &updated)?;
        Self::append_event_tx(&tx, task_id, &event)?;
        tx.commit()?;
        Ok(())
    }

    fn acquire_work_unit_lease(
        &self,
        task_id: TaskId,
        work_unit_id: &str,
        run_id: &str,
        ttl: StdDuration,
        branch: Option<String>,
        base_commit: Option<String>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut task) = Self::get_locked(&tx, task_id)? else {
            return Ok(false);
        };
        if task.status != Status::Running {
            return Ok(false);
        }
        let Some(current) = tx
            .query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, \
                 child_task_id, needs_decisions_json \
                 FROM work_units WHERE id = ?1 AND task_id = ?2",
                params![work_unit_id, task_id.to_string()],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()?
        else {
            return Ok(false);
        };
        if !matches!(
            current.status,
            WorkUnitStatus::Ready | WorkUnitStatus::NeedsContinuation
        ) {
            return Ok(false);
        }
        let now = OffsetDateTime::now_utc();
        let expires_at = now + time::Duration::new(ttl.as_secs() as i64, ttl.subsec_nanos() as i32);
        let mut updated = current.clone();
        updated.status = WorkUnitStatus::Running;
        updated.blocked_reason = None;
        updated.runs += 1;
        updated.last_run_id = Some(run_id.to_string());
        updated.updated_at = format_rfc3339(now)?;
        updated.lease_run_id = Some(run_id.to_string());
        updated.lease_expires_at = Some(format_rfc3339(expires_at)?);
        if branch.is_some() {
            updated.branch = branch;
        }
        if base_commit.is_some() {
            updated.base_commit = base_commit;
        }
        Self::update_work_unit_tx(&tx, &updated)?;
        Self::append_event_tx(
            &tx,
            task_id,
            &Event::WorkUnitTransitioned {
                work_unit_id: current.id.clone(),
                key: current.key.clone(),
                from: current.status,
                to: WorkUnitStatus::Running,
                reason: "dispatch".to_string(),
                run_id: Some(run_id.to_string()),
            },
        )?;
        // Task の lease の期限を延ばす（保持者はそのまま。短くはしない）。
        if let Some(lease) = task.lease.as_mut()
            && lease.expires_at < expires_at
        {
            lease.expires_at = expires_at;
            tx.execute(
                "UPDATE tasks SET lease_expires_at = ?1, json = ?2 WHERE id = ?3",
                params![
                    format_rfc3339(expires_at)?,
                    serde_json::to_string(&task)?,
                    task_id.to_string(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn work_units_apply(
        &self,
        task_id: TaskId,
        inserted: Vec<WorkUnitRow>,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for wu in &inserted {
            Self::insert_work_unit_tx(&tx, wu)?;
        }
        for wu in &updated {
            Self::update_work_unit_tx(&tx, wu)?;
        }
        for ev in &events {
            Self::append_event_tx(&tx, task_id, ev)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn tree_tasks(&self, root_id: TaskId) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT json FROM tasks WHERE id = ?1 OR root_id = ?1 ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = stmt.query_map(params![root_id.to_string()], |row| {
                row.get::<_, String>(0)
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(Self::row_to_task(row?)?);
            }
            Ok(out)
        })
    }

    fn tasks_with_open_task_units(&self) -> Result<Vec<TaskId>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT task_id FROM work_units WHERE kind = 'task' AND status IN \
                 ('pending', 'ready', 'running', 'needs_continuation', 'blocked') ORDER BY task_id",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let ids: Vec<String> = rows.collect::<Result<_, _>>()?;
            ids.iter().map(|s| Self::parse_id(s)).collect()
        })
    }

    fn tree_child_create(
        &self,
        parent_id: TaskId,
        child: &Task,
        unit: WorkUnitRow,
        parent_events: Vec<Event>,
        replaces: Option<&str>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(parent) = Self::get_locked(&tx, parent_id)? else {
            return Ok(false);
        };
        if parent.status.is_terminal() || child.parent_id != Some(parent_id) {
            return Ok(false);
        }
        let current = tx
            .query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units WHERE id = ?1 AND task_id = ?2",
                params![unit.id, parent_id.to_string()],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()?;
        let Some(current) = current else {
            return Ok(false);
        };
        let expected = match replaces {
            None => current.status == WorkUnitStatus::Ready && current.child_task_id.is_none(),
            Some(prev) => {
                current.status == WorkUnitStatus::Running
                    && current.child_task_id.as_deref() == Some(prev)
            }
        };
        if !expected || current.kind != crate::execution_plan::WorkUnitKind::Task {
            return Ok(false);
        }
        Self::insert_tx(&tx, child)?;
        Self::append_event_tx(
            &tx,
            child.id,
            &Event::Created {
                task: Box::new(child.clone()),
                origin: Some(crate::model::CreatedOrigin::PlanUnit),
            },
        )?;
        Self::update_work_unit_tx(&tx, &unit)?;
        for ev in &parent_events {
            Self::append_event_tx(&tx, parent_id, ev)?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn execution_plan_adopt_tree(
        &self,
        task_id: TaskId,
        plan: ExecutionPlanRow,
        work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        event: Event,
        after_events: Vec<Event>,
        adoptions: Vec<TreeAdoption>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for a in &adoptions {
            if !Self::tree_adoption_ok_tx(&tx, a)? {
                return Ok(false);
            }
        }
        Self::adopt_plan_tx(&tx, task_id, plan, work_units, extra_events, event)?;
        for ev in &after_events {
            Self::append_event_tx(&tx, task_id, ev)?;
        }
        for a in &adoptions {
            Self::apply_tree_adoption_tx(&tx, a)?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn tree_adopt_apply(
        &self,
        owner_id: TaskId,
        unit_id: &str,
        expect_unit_status: WorkUnitStatus,
        updated: Vec<WorkUnitRow>,
        events: Vec<Event>,
        adoption: TreeAdoption,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(owner) = Self::get_locked(&tx, owner_id)? else {
            return Ok(false);
        };
        if owner.status.is_terminal() {
            return Ok(false);
        }
        let current = tx
            .query_row(
                "SELECT id, task_id, plan_id, key, seq, kind, status, blocked_reason, \
                 depends_on_json, runs, continuations, retries, last_run_id, \
                 last_checkpoint_run_id, json, created_at, updated_at, phase, lease_run_id, \
                 lease_expires_at, branch, base_commit, head_commit, integrated_commit, child_task_id, \
                 needs_decisions_json FROM work_units WHERE id = ?1 AND task_id = ?2",
                params![unit_id, owner_id.to_string()],
                Self::row_to_work_unit,
            )
            .optional()?
            .transpose()?;
        let Some(current) = current else {
            return Ok(false);
        };
        if current.status != expect_unit_status
            || current.child_task_id.is_some()
            || current.kind != crate::execution_plan::WorkUnitKind::Task
        {
            return Ok(false);
        }
        if !Self::tree_adoption_ok_tx(&tx, &adoption)? {
            return Ok(false);
        }
        for wu in &updated {
            Self::update_work_unit_tx(&tx, wu)?;
        }
        for ev in &events {
            Self::append_event_tx(&tx, owner_id, ev)?;
        }
        Self::apply_tree_adoption_tx(&tx, &adoption)?;
        tx.commit()?;
        Ok(true)
    }

    fn extend_task_lease(&self, task_id: TaskId, ttl: StdDuration) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(mut task) = Self::get_locked(&tx, task_id)? else {
            return Ok(false);
        };
        if task.status != Status::Running {
            return Ok(false);
        }
        let expires_at = OffsetDateTime::now_utc()
            + time::Duration::new(ttl.as_secs() as i64, ttl.subsec_nanos() as i32);
        let Some(lease) = task.lease.as_mut() else {
            return Ok(false);
        };
        if lease.expires_at < expires_at {
            lease.expires_at = expires_at;
            tx.execute(
                "UPDATE tasks SET lease_expires_at = ?1, json = ?2 WHERE id = ?3",
                params![
                    format_rfc3339(expires_at)?,
                    serde_json::to_string(&task)?,
                    task_id.to_string(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn running_tasks_with_runnable_work_units(
        &self,
        limit: usize,
    ) -> Result<Vec<Task>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.json FROM tasks t WHERE t.status = ?1 AND EXISTS ( \
                 SELECT 1 FROM work_units w WHERE w.task_id = t.id AND w.phase IS NOT NULL \
                 AND w.kind NOT IN ('integrate', 'task') AND w.status IN ('ready', 'needs_continuation')) \
                 ORDER BY t.created_at ASC LIMIT ?2",
            )?;
            let rows = stmt.query_map(
                params![status_str(Status::Running), usize_to_i64(limit)],
                |row| row.get::<_, String>(0),
            )?;
            let mut out = Vec::new();
            for row in rows {
                out.push(Self::row_to_task(row?)?);
            }
            Ok(out)
        })
    }

    fn run_index_start(&self, row: RunRow) -> Result<(), StoreError> {
        let conn = self.lock()?;
        Self::insert_run_row_tx(&conn, &row)
    }

    fn run_index_finish(
        &self,
        run_id: &str,
        status: RunIndexStatus,
        checkpoint: Option<crate::execution::Checkpoint>,
        usage: Option<crate::model::Usage>,
        metrics: Option<crate::model::RunMetrics>,
        finished_at: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE runs SET status = ?1, checkpoint_json = ?2, usage_json = ?3, \
             metrics_json = ?4, finished_at = ?5 WHERE run_id = ?6",
            params![
                status.as_str(),
                checkpoint.as_ref().map(serde_json::to_string).transpose()?,
                usage.as_ref().map(serde_json::to_string).transpose()?,
                metrics.as_ref().map(serde_json::to_string).transpose()?,
                format_rfc3339(finished_at)?,
                run_id,
            ],
        )?;
        Ok(n > 0)
    }

    fn run_index_get(&self, run_id: &str) -> Result<Option<RunRow>, StoreError> {
        self.with_read_conn(|conn| {
            conn.query_row(
                "SELECT run_id, task_id, work_unit_id, role, seq, status, adapter, model, \
                 account, session_id, checkpoint_json, usage_json, metrics_json, started_at, \
                 finished_at FROM runs WHERE run_id = ?1",
                params![run_id],
                Self::row_to_run,
            )
            .optional()?
            .transpose()
        })
    }

    fn runs_for_task(&self, task_id: TaskId) -> Result<Vec<RunRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, task_id, work_unit_id, role, seq, status, adapter, model, \
                 account, session_id, checkpoint_json, usage_json, metrics_json, started_at, \
                 finished_at FROM runs WHERE task_id = ?1 ORDER BY started_at ASC",
            )?;
            let rows = stmt.query_map(params![task_id.to_string()], Self::row_to_run)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn runs_for_work_unit(&self, work_unit_id: &str) -> Result<Vec<RunRow>, StoreError> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, task_id, work_unit_id, role, seq, status, adapter, model, \
                 account, session_id, checkpoint_json, usage_json, metrics_json, started_at, \
                 finished_at FROM runs WHERE work_unit_id = ?1 ORDER BY started_at ASC",
            )?;
            let rows = stmt.query_map(params![work_unit_id], Self::row_to_run)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
    }

    fn execution_metrics_task_rows(
        &self,
        since: Option<OffsetDateTime>,
    ) -> Result<Vec<ExecutionMetricsTaskRow>, StoreError> {
        let since_text = since.map(format_rfc3339).transpose()?;
        self.with_read_conn(|conn| {
            // Aggregate each child table before joining: raw joins multiply counts.
            let mut stmt = conn.prepare(
                "WITH eligible AS MATERIALIZED (\
                   SELECT id, status, genre, assignee, json, updated_at FROM tasks \
                   WHERE (?1 IS NULL OR julianday(updated_at) >= julianday(?1) - 1.0 / 86400000)), \
                 wu AS (\
                   SELECT w.task_id, SUM(CASE WHEN w.kind = 'repair' THEN 1 ELSE 0 END) repairs, \
                          SUM(w.continuations) continuations, SUM(w.retries) retries \
                   FROM work_units w JOIN eligible t ON t.id = w.task_id GROUP BY w.task_id), \
                 plans AS (\
                   SELECT p.task_id, SUM(CASE WHEN p.version > 1 THEN 1 ELSE 0 END) replans \
                   FROM execution_plans p JOIN eligible t ON t.id = p.task_id GROUP BY p.task_id), \
                 run_counts AS (\
                   SELECT r.task_id, COUNT(*) runs_count, \
                          SUM(CASE WHEN r.status = 'budget_exhausted' THEN 1 ELSE 0 END) budget_exhausted_runs \
                   FROM runs r JOIN eligible t ON t.id = r.task_id GROUP BY r.task_id) \
                 SELECT t.id, t.status, t.genre, t.assignee, json_extract(t.json, '$.routing'), \
                        COALESCE(wu.repairs, 0), COALESCE(plans.replans, 0), \
                        COALESCE(wu.continuations, 0), COALESCE(wu.retries, 0), \
                        COALESCE(run_counts.runs_count, 0), COALESCE(run_counts.budget_exhausted_runs, 0), \
                        latest.run_id, latest.role, latest.status, latest.adapter, latest.model, \
                        latest.metrics_json, latest.usage_json, \
                        t.updated_at, \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') IN \
                          ('execution_planned', 'work_unit_transitioned', 'worker_started', \
                           'worker_finished', 'transitioned', 'routing_decided') LIMIT 1), \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') = 'worker_finished' AND \
                          json_extract(e.json, '$.end.type') = 'budget_exhausted' LIMIT 1), \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') = 'transitioned' AND \
                          json_extract(e.json, '$.reason') IN \
                          ('continue', 'work_unit_retry', 'worker_error') LIMIT 1), \
                        EXISTS (SELECT 1 FROM events e WHERE e.task_id = t.id AND \
                          json_extract(e.json, '$.type') = 'quota_estimated' LIMIT 1) \
                 FROM eligible t \
                 LEFT JOIN wu ON wu.task_id = t.id \
                 LEFT JOIN plans ON plans.task_id = t.id \
                 LEFT JOIN run_counts ON run_counts.task_id = t.id \
                 LEFT JOIN runs latest ON latest.run_id = (\
                   SELECT r.run_id FROM runs r WHERE r.task_id = t.id \
                   ORDER BY r.started_at DESC, r.run_id DESC LIMIT 1) \
                 ORDER BY t.id",
            )?;
            let rows = stmt.query_map(params![since_text], |row| {
                Ok((
                    row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?, row.get::<_, u32>(5)?,
                    row.get::<_, u32>(6)?, row.get::<_, u32>(7)?,
                    row.get::<_, u32>(8)?, row.get::<_, u32>(9)?,
                    row.get::<_, u32>(10)?, row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?, row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?, row.get::<_, Option<String>>(15)?,
                    row.get::<_, Option<String>>(16)?, row.get::<_, Option<String>>(17)?,
                    row.get::<_, String>(18)?, row.get::<_, bool>(19)?, row.get::<_, bool>(20)?,
                    row.get::<_, bool>(21)?, row.get::<_, bool>(22)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (id, status, genre, assignee, routing_json, repairs, replans,
                    continuations, retries, runs_count, budget_exhausted_runs, run_id,
                    role, run_status, adapter, model, metrics_json, usage_json, updated_at,
                    has_execution_events, has_budget_events, has_transition_metrics,
                    has_quota_events) = row?;
                // SQLite julianday has millisecond resolution. Keep a 1 ms candidate margin in
                // SQL, then apply the original OffsetDateTime comparison exactly here.
                if let Some(since) = since && parse_rfc3339(&updated_at)? < since {
                    continue;
                }
                let latest_run = if let Some(run_id) = run_id {
                    let role = role.and_then(|s| RunIndexRole::parse(&s)).ok_or_else(|| {
                        StoreError::Invalid(format!("invalid latest runs.role for {run_id}"))
                    })?;
                    let status = run_status
                        .and_then(|s| RunIndexStatus::parse(&s))
                        .ok_or_else(|| StoreError::Invalid(format!("invalid latest runs.status for {run_id}")))?;
                    Some(ExecutionMetricsLatestRun {
                        run_id, role, status, adapter, model, metrics_json, usage_json,
                    })
                } else {
                    None
                };
                out.push(ExecutionMetricsTaskRow {
                    task_id: id.parse().map_err(|_| StoreError::Invalid(format!("invalid tasks.id: {id}")))?,
                    status: parse_status(&status)?, genre, assignee, routing_json,
                    repairs, replans, continuations, retries, runs_count,
                    budget_exhausted_runs, latest_run, has_execution_events, has_budget_events,
                    has_transition_metrics, has_quota_events,
                });
            }
            Ok(out)
        })
    }

    fn work_units_replace(
        &self,
        task_id: TaskId,
        rows: Vec<WorkUnitRow>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM work_units WHERE task_id = ?1",
            params![task_id.to_string()],
        )?;
        for wu in &rows {
            Self::insert_work_unit_tx(&tx, wu)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn runs_replace(&self, task_id: TaskId, rows: Vec<RunRow>) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM runs WHERE task_id = ?1",
            params![task_id.to_string()],
        )?;
        for r in &rows {
            Self::insert_run_row_tx(&tx, r)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn execution_plans_replace(
        &self,
        task_id: TaskId,
        rows: Vec<ExecutionPlanRow>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM execution_plans WHERE task_id = ?1",
            params![task_id.to_string()],
        )?;
        for plan in &rows {
            tx.execute(
                "INSERT INTO execution_plans (id, task_id, version, origin, planner_run_id, \
                 status, json, created_at, superseded_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    plan.id,
                    plan.task_id,
                    plan.version,
                    plan.origin.as_str(),
                    plan.planner_run_id,
                    plan.status.as_str(),
                    serde_json::to_string(&plan.spec)?,
                    plan.created_at,
                    plan.superseded_at,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---- ADR-0072 D16/D17（Phase E4）: reviewer repair / replan ----

    fn review_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError> {
        self.repair_apply(
            task_id,
            Trigger::ReviewRepair,
            extra_events,
            new_plan,
            work_units,
        )
    }

    fn delivery_repair_apply(
        &self,
        task_id: TaskId,
        extra_events: Vec<Event>,
        new_plan: Option<ExecutionPlanRow>,
        work_units: Vec<WorkUnitRow>,
    ) -> Result<Outcome, StoreError> {
        self.repair_apply(task_id, Trigger::Reopen, extra_events, new_plan, work_units)
    }

    #[allow(clippy::too_many_arguments)]
    fn execution_plan_replan(
        &self,
        task_id: TaskId,
        old_plan_id: String,
        new_plan: ExecutionPlanRow,
        updated_work_units: Vec<WorkUnitRow>,
        new_work_units: Vec<WorkUnitRow>,
        extra_events: Vec<Event>,
        plan_event: Event,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let superseded_at = format_rfc3339(OffsetDateTime::now_utc())?;
        let n = tx.execute(
            "UPDATE execution_plans SET status = 'superseded', superseded_at = ?1 \
             WHERE id = ?2 AND task_id = ?3 AND status = 'active'",
            params![superseded_at, old_plan_id, task_id.to_string()],
        )?;
        if n == 0 {
            return Err(StoreError::InUse {
                kind: "execution_plan",
                id: old_plan_id,
                detail: "no active execution plan to replan (changed concurrently?)".to_string(),
            });
        }
        tx.execute(
            "INSERT INTO execution_plans (id, task_id, version, origin, planner_run_id, \
             status, json, created_at, superseded_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                new_plan.id,
                new_plan.task_id,
                new_plan.version,
                new_plan.origin.as_str(),
                new_plan.planner_run_id,
                new_plan.status.as_str(),
                serde_json::to_string(&new_plan.spec)?,
                new_plan.created_at,
                new_plan.superseded_at,
            ],
        )?;
        for wu in &updated_work_units {
            Self::update_work_unit_tx(&tx, wu)?;
        }
        for wu in &new_work_units {
            Self::insert_work_unit_tx(&tx, wu)?;
        }
        for ev in &extra_events {
            Self::append_event_tx(&tx, task_id, ev)?;
        }
        Self::append_event_tx(&tx, task_id, &plan_event)?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "store/tests.rs"]
mod tests;
