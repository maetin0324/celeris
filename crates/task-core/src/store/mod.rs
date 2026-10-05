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
//!
//! ## module map（audit.md §6.2 案A、責務別の切り出し）
//! 領域ファイルはどれも `impl SqliteStore` ブロックに `_tx`/`_locked`/行変換の内部 helper と、
//! `TaskStore` の method 本体（`<method>_impl`）を置く（可視性は `pub(super)`、`execution/` 配下は
//! `pub(in crate::store)`、既存の `pub(crate)` はそのまま）。`impl TaskStore for SqliteStore` は
//! `task_store_impl.rs` の 1 ブロックだけで、全 method が `self.<method>_impl(..)` の 1 行転送
//! （macro は使わない。method 名で grep すれば本体に届く）。
//!
//! 依存の向き: 領域 → `mod.rs` 基盤、`execution/*` → `tasks`/`events`/`transition`、
//! `approvals`/`messages` → `transition`、`transition` → `tasks`/`events`、`events` → `execution`（行変換）。
//! 例外は `migrations` → `repos::backfill_project_repos`（v12）・`projects::backfill_project_slugs`（v29）。
//!
//! 守る invariant:
//! - event と投影は同じ tx で書く（`append_event_tx` / `apply_transition_tx` を経由する）。
//! - 終端遷移の approval・browser wait・cascade は `apply_transition_tx` の同じ tx で行う。
//! - `Mutex<Connection>` は再入不可。`lock()` を取った後は `_tx`/`_locked` 関数だけを呼ぶ
//!   （`&self` の method を tx の中から呼ばない）。
//! - 状態機械が持つ `status`/`attempts`/`lease` は tx の中で読み直した値で書く（`update_task`・木の採用）。
//!
//! - `mod.rs`: `SqliteStore` の基盤（`open`/`open_with`/pragma 設定/`ReadPool`/`migrate`/`lock`/
//!   `parse_id`/`NON_TERMINAL_SQL`）、`StoreError`・`StoreOptions` などの共有型と列値の変換
//!   （`TaskStore` の実装は持たない）。
//! - `migrations.rs`: `MIGRATION_0001..0033`（`include_str!` で読む SQL 本体）・`SCHEMA_VERSION`・
//!   `apply_migration_version`/`migration_sql`（版数 → SQL の対応）。
//! - `query.rs`: `TaskStore::list_page` 系の一覧部品（`ListFilter`・`ListOrder`・`Page`・
//!   `CursorPayload`・cursor の符号化・`filter_predicate`・keyset 述語）。
//! - `task_store.rs`: `trait TaskStore` の宣言。
//! - `task_store_impl.rs`: `impl TaskStore for SqliteStore`（1 ブロック、1 行転送だけ）。
//! - `tasks.rs`: task 行の読み書き（`get_locked`/`insert_tx`/`update_task_tx`/`rewrite_task_tx`）と、
//!   task・lease の `TaskStore` 本体（`insert`/`get`/`list`/`list_page`/`count_by_status`/`children`/
//!   `create_task`/`delegate_children`/`retry_task`/`update_task`/`acquire_lease`/`renew_lease`/
//!   `extend_task_lease`/`release_lease`/`ready_tasks`/`halted_by_pause`）。
//! - `events.rs`: `append_event_tx`（events への追記）と、同じ tx で書く Event 由来の投影
//!   （`runs` 行の終端化・木の `work_units.child_task_id`・`decisions` 行と `decisions` の行変換）、
//!   events を読む/追記する `TaskStore` 本体（`append_event`/`events_for*`/`events_since`/
//!   `latest_event_id`/`event_rows_for`）。
//! - `approvals.rs`: 遷移と判断の適用（`apply_transition_with_events`、Plan の承認 `complete_plan`、
//!   案件計画の Go/No-Go `project_plan_decide_apply`/`project_plan_apply`）。どれも 1 つの IMMEDIATE tx で
//!   `apply_transition_tx`/`append_event_tx` を呼ぶ（`transition.rs` は tx 内の部品、ここは入口）。
//! - `transition.rs`: `apply_transition_tx`（状態機械の適用と Event の追記を同じ tx で行う）と
//!   終端遷移の cascade・非終端の子/依存の探索。
//! - `org.rs`: 組織ノードの行変換・`profile_json`・`celerisctl org migrate-v2` 用の低レベル書き換えと
//!   `org_*` の本体（ADR-0033 D1）。
//! - `projects.rs`: 案件・途中目標の行変換、slug の検査/backfill、停止中の祖先の判定と
//!   `project_*`/`milestone_*` の本体（ADR-0033 D2）。
//! - `repos.rs`: 案件のリポジトリ（`project_repos`）の行と backfill、`repo_*` の本体（ADR-0043 D1）。
//! - `integrations.rs`: 変更の取り込み（`task_integrations`）の行と `integration_*` の本体（ADR-0043 D5）。
//! - `messages.rs`: 対話（`messages`）とコメント（`task_comments`）の行変換、`message_*`・
//!   `comment_add`/`comments_for`（ADR-0044 D2、遷移は同じ tx の `apply_transition_tx`）と CoS actions の
//!   冪等性 `console_action_run_claim` の本体（ADR-0033 D4 / ADR-0048）。
//! - `instances.rs`: `daemon_instances`（ADR-0040 D4）の `instance_*` の本体。行変換は `crate::instance`。
//! - `cluster.rs`: `cluster_settings`・`cluster_connection_log`（ADR-0044 の cluster 設定と接続履歴）の
//!   `cluster_*` の本体。
//! - `execution/`（ADR-0072 D5 以降の実行分解）:
//!   - `plans.rs`: `execution_plans` の行変換、計画の採用 `adopt_plan_tx`、`execution_plan_adopt*`/
//!     `execution_plan_active`/`execution_plan_list`/`execution_plans_replace`。
//!   - `work_units.rs`: `work_units` の行変換と書き込み（`insert_work_unit_tx`/`update_work_unit_tx`）、
//!     `work_unit*`/`work_units_*`・WU の lease `acquire_work_unit_lease`・
//!     `running_tasks_with_runnable_work_units`。
//!   - `runs.rs`: `runs` の行変換と書き込み（`insert_run_row_tx`）、`run_index_*`/`runs_*` と
//!     実行 metrics の集計 `execution_metrics_task_rows`（ADR-0072 E6）。
//!   - `decisions.rs`: 人への決定（ADR-0079 D7）の `decisions_*`/`decision_get`/`decision_resolve_apply`。
//!   - `tree.rs`: 再帰 task の木（ADR-0079）: `tree_tasks`/`tree_child_create`/`tasks_with_open_task_units`
//!     と既存 task の木への採用（D15、`execution_plan_adopt_tree`/`tree_adopt_apply`）。
//!   - `repair.rs`: reviewer repair / delivery repair / replan（ADR-0072 D16/D17）。
//! - `tests.rs`（既存）: 単体テスト。
//!
//! store の外にある領域別の store trait（`NodeSessionStore`（sessions）・`ApprovalStore`・
//! `NotificationStore`・`McpClientStore`/`McpCallStore`・`BrowserWaitStore`・`DeliveryStore`・
//! `KnowledgeRunStore`・`ReportStore`）は、各領域 module が自分の表と `impl … for SqliteStore` を持つ。
//! `TaskStore` の method ではなく公開パスを変えないため、ここへは移さない。

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration as StdDuration;

use rusqlite::{Connection, OpenFlags, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::execution_plan::{RunIndexRole, RunIndexStatus};
use crate::model::{Event, Status, Task, TaskId, TaskKind, WorkspaceSpec};
use crate::org::{MilestoneId, MilestoneStatus, OrgError};
use crate::repos::{RepoError, RepoKind};
use crate::transition::{InvalidTransition, Trigger};

mod approvals;
mod behind_targets;
mod cluster;
mod events;
mod execution;
mod instances;
mod integrations;
mod messages;
pub(crate) mod migrations;
mod org;
mod projects;
mod query;
mod repos;
mod routing_log;
mod routing_shadow;
mod task_store;
mod task_store_impl;
mod tasks;
mod transition;
mod write_sets;

pub use migrations::SCHEMA_VERSION;
#[cfg(test)]
use migrations::{
    MIGRATION_0001, MIGRATION_0002, MIGRATION_0003, MIGRATION_0004, MIGRATION_0005, MIGRATION_0006,
    MIGRATION_0007, MIGRATION_0008, MIGRATION_0009, MIGRATION_0010, MIGRATION_0011,
};
pub use query::{ListFilter, ListOrder, Page};
pub use routing_log::RoutingCorrelation;
pub use task_store::TaskStore;

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
    /// ADR-0095 D6: `open_client`（celerisctl）が開いた DB の版数がこのバイナリより古い。クライアントは
    /// migration をしない（daemon が起動時に migrate する）。DB には何も書かない。
    #[error(
        "db schema version {found} is older than the {required} this binary uses; celerisctl never \
         migrates the database (ADR-0095). Start the daemon (celeris) on this DB to migrate it, or use \
         a celerisctl built from the same release as the running daemon"
    )]
    SchemaTooOld { found: u32, required: u32 },
    /// ADR-0095 D6: `open_client` の DB ファイルが無い（クライアントは DB を作らない）。
    #[error(
        "db {path} does not exist; celerisctl does not create or migrate databases (ADR-0095). \
         The daemon (celeris) creates it on start"
    )]
    DbMissing { path: String },
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

/// ADR-0095 D6: `SQLITE_READONLY`（読み取り専用で開いた DB・読み取り専用の mount への書き込み）かどうか。
/// celerisctl が「DB は読み取り専用」と言い換えるために使う（純粋関数）。
pub fn is_readonly_error(e: &StoreError) -> bool {
    matches!(
        e,
        StoreError::Sqlite(rusqlite::Error::SqliteFailure(inner, _))
            if inner.code == rusqlite::ErrorCode::ReadOnly
    )
}

/// ADR-0095 D6: `SqliteStore::open_client` がどう開いたか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAccess {
    /// DB の版数がこのバイナリの `SCHEMA_VERSION` と同じ。従来どおり読み書きできる（ただし DB ファイル
    /// 自体が読み取り専用なら〈ADR-0095 D1 の worker の namespace〉SQLite が読み取り専用に倒す）。
    ReadWrite,
    /// DB の版数がこのバイナリより新しい。読み取り専用の接続で開いた（書き込みは `SQLITE_READONLY`）。
    ReadOnlyNewerSchema { found: u32 },
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

/// タスクとその `events_for`（`seq`, event）の組。`TaskStore::tasks_with_events` の要素。
pub type TaskWithEvents = (Task, Vec<(u64, Event)>);

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
    /// ADR-0133 付記: 書き込み接続と読み取り接続を取った回数（回帰試験が処理件数で固定する）。
    counters: LockCounters,
}

/// ADR-0133 付記: `SqliteStore` が接続を取った回数。試験は時計ではなくこの差分で
/// 「tick・API が書き込み接続を何回取るか」を固定する。
#[derive(Debug, Default)]
struct LockCounters {
    writer: std::sync::atomic::AtomicU64,
    reader: std::sync::atomic::AtomicU64,
}

/// `SqliteStore::lock_counts` の値。`writer` は書き込み接続（`Mutex`）を取った回数、
/// `reader` は読み取り接続（プール、無ければ書き込み接続へのフォールバック）を使った回数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LockCounts {
    pub writer: u64,
    pub reader: u64,
}

impl LockCounts {
    /// `earlier` からの増分。
    pub fn since(self, earlier: LockCounts) -> LockCounts {
        LockCounts {
            writer: self.writer.saturating_sub(earlier.writer),
            reader: self.reader.saturating_sub(earlier.reader),
        }
    }
}

/// ADR-0064 D4: 読み取り専用（`query_only=ON`）の小さな接続プール。書き込み接続（`conn`）の
/// `Mutex` とは別物で、読み取りは書き込みのトランザクション中でも待たされない（WAL の性質）。
struct ReadPool {
    conns: Mutex<Vec<Connection>>,
    available: std::sync::Condvar,
}

impl ReadPool {
    fn open(
        path: &Path,
        size: usize,
        busy_timeout: StdDuration,
        flags: OpenFlags,
    ) -> Result<Self, StoreError> {
        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            let conn = Connection::open_with_flags(path, flags)?;
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

    /// ADR-0095 D6: クライアント（celerisctl）用の open。**migration をしない**。
    ///
    /// - ファイルが無い → `StoreError::DbMissing`（作らない）。
    /// - 版数を読み取り専用の接続で読み、古い → `StoreError::SchemaTooOld`（DB には何も書かない）。
    /// - 同じ → 読み書きの接続（`SQLITE_OPEN_CREATE` なし）。
    /// - 新しい → 読み取り専用の接続（`ClientAccess::ReadOnlyNewerSchema`）。書き込みは `SQLITE_READONLY`。
    ///
    /// daemon・API・MCP は従来どおり `open_with`（migration あり）を使う。
    pub fn open_client(path: &Path) -> Result<(Self, ClientAccess), StoreError> {
        Self::open_client_with(path, StoreOptions::default())
    }

    /// `open_client` の `StoreOptions` 付き（`background_checkpoint` は無視する。クライアントは立てない）。
    pub fn open_client_with(
        path: &Path,
        options: StoreOptions,
    ) -> Result<(Self, ClientAccess), StoreError> {
        if !path.exists() {
            return Err(StoreError::DbMissing {
                path: path.display().to_string(),
            });
        }
        let options = StoreOptions {
            background_checkpoint: false,
            ..options
        };
        let read_only = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let read_write = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let found = {
            let probe = Connection::open_with_flags(path, read_only)?;
            probe.busy_timeout(options.busy_timeout)?;
            Self::read_schema_version(&probe)?
        };
        if found < SCHEMA_VERSION {
            return Err(StoreError::SchemaTooOld {
                found,
                required: SCHEMA_VERSION,
            });
        }
        if found == SCHEMA_VERSION {
            let conn = Connection::open_with_flags(path, read_write)?;
            conn.busy_timeout(options.busy_timeout)?;
            // 開くまでの間に daemon が migrate したかもしれないので、使う接続で読み直す。
            let now = Self::read_schema_version(&conn)?;
            if now == SCHEMA_VERSION {
                Self::configure_client_pragmas(&conn, &options)?;
                let store = Self::from_client_connection(conn, &options, path, read_write)?;
                return Ok((store, ClientAccess::ReadWrite));
            }
        }
        let conn = Connection::open_with_flags(path, read_only)?;
        conn.busy_timeout(options.busy_timeout)?;
        let found = Self::read_schema_version(&conn)?;
        if found < SCHEMA_VERSION {
            return Err(StoreError::SchemaTooOld {
                found,
                required: SCHEMA_VERSION,
            });
        }
        // ここへ来るのは版数が新しいとき（または読み書きで開く間に daemon が migrate したとき）。
        let store = Self::from_client_connection(conn, &options, path, read_only)?;
        Ok((store, ClientAccess::ReadOnlyNewerSchema { found }))
    }

    /// ADR-0095 D6: 版数を読むだけ（表が無ければ 0。書き込みはしない）。
    fn read_schema_version(conn: &Connection) -> Result<u32, StoreError> {
        if !Self::table_exists(conn, "schema_migrations")? {
            return Ok(0);
        }
        let v: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;
        Ok(v as u32)
    }

    /// ADR-0095 D6: クライアントの接続の PRAGMA。`journal_mode` は**接続が書けるときだけ** WAL にする
    /// （読み取り専用の mount〈ADR-0095 D1〉では SQLite が読み取り専用に倒しており、`journal_mode` の
    /// 変更は書き込みになる。本番の DB は daemon が WAL にしてある）。
    fn configure_client_pragmas(
        conn: &Connection,
        options: &StoreOptions,
    ) -> Result<(), StoreError> {
        if conn.is_readonly(rusqlite::DatabaseName::Main)? {
            return Ok(());
        }
        Self::configure_pragmas(conn, options)
    }

    fn from_client_connection(
        conn: Connection,
        options: &StoreOptions,
        path: &Path,
        flags: OpenFlags,
    ) -> Result<Self, StoreError> {
        let read_pool = if options.read_pool_size > 0 {
            Some(ReadPool::open(
                path,
                options.read_pool_size,
                options.busy_timeout,
                flags,
            )?)
        } else {
            None
        };
        Ok(Self {
            conn: Mutex::new(conn),
            read_pool,
            counters: LockCounters::default(),
        })
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
                OpenFlags::default(),
            )?),
            _ => None,
        };
        Ok(Self {
            conn: Mutex::new(conn),
            read_pool,
            counters: LockCounters::default(),
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
        self.counters
            .reader
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match &self.read_pool {
            Some(pool) => {
                let conn = pool.checkout()?;
                let result = f(&conn);
                pool.checkin(conn);
                result
            }
            None => {
                let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
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

        let current: u32 = if migrations_table_existed {
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
        }

        // ADR-0133 D3.2: 版数は記録の有無で決める（`RESERVED_VERSIONS` の飛びを後から埋められるように）。
        // 記録が連続していれば従来の `(current + 1)..=SCHEMA_VERSION` と同じ。
        let applied: std::collections::HashSet<u32> = {
            let mut stmt = conn.prepare("SELECT version FROM schema_migrations")?;
            let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
            rows.map(|r| r.map(|v| v as u32))
                .collect::<Result<_, _>>()?
        };
        for version in 1..=SCHEMA_VERSION {
            if applied.contains(&version) || migrations::RESERVED_VERSIONS.contains(&version) {
                continue;
            }
            Self::apply_migration_version(conn, version)?;
        }

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

    /// ADR-0033 D3/D5: `report.rs`・`approval.rs`（`reports`・`approvals`・`standing_rules` 表の SQL）
    /// も同じ接続を使うので crate 内に公開する。
    pub(crate) fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.counters
            .writer
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.conn.lock().map_err(|_| StoreError::Poisoned)
    }

    /// ADR-0133 付記: これまでに書き込み接続・読み取り接続を取った回数（試験用の観測値）。
    pub fn lock_counts(&self) -> LockCounts {
        LockCounts {
            writer: self
                .counters
                .writer
                .load(std::sync::atomic::Ordering::Relaxed),
            reader: self
                .counters
                .reader
                .load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// 非終端（`done` / `failed` / `cancelled` 以外）の task を選ぶ SQL 断片（領域ファイル共通）。
    const NON_TERMINAL_SQL: &'static str = "status NOT IN ('done', 'failed', 'cancelled')";

    fn parse_id(id_str: &str) -> Result<TaskId, StoreError> {
        id_str
            .parse()
            .map_err(|_| StoreError::Invalid(format!("invalid task id in tasks table: {id_str}")))
    }
}

#[cfg(test)]
mod client_open_tests;
#[cfg(test)]
mod routing_log_tests;
#[cfg(test)]
mod routing_shadow_tests;
#[cfg(test)]
mod tests;
