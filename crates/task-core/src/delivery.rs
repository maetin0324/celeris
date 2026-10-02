//! ADR-0051: 上司の判定とリリース準備の永続状態。LLM・git・プロセス起動は含まない。
use crate::{MessageId, ProjectId, RepoId, SqliteStore, StoreError, TaskId};
use rusqlite::OptionalExtension;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Reviewing,
    MergeQueued,
    Merging,
    Preparing,
    Ready,
    Blocked,
}

/// ADR-0119 D2: 対象案件の root で delivery を作れなかった理由（`Event::DeliverySkipped.reason`）。
/// 並びは判定の順（同時に複数あれば先のものを記録する）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DeliverySkipReason {
    MultipleRepos,
    NoMarker,
    MarkerRepoMismatch,
    RepoRowMissing,
    RepoNotLocal,
    RepoPathMismatch,
    NotGit,
    NoBranch,
    BranchNameMismatch,
    RefsUnresolvable,
    DepartmentUnresolved,
}

impl DeliverySkipReason {
    /// 機械可読コード（serde の値と同じ）。
    pub fn code(self) -> &'static str {
        match self {
            Self::MultipleRepos => "multiple_repos",
            Self::NoMarker => "no_marker",
            Self::MarkerRepoMismatch => "marker_repo_mismatch",
            Self::RepoRowMissing => "repo_row_missing",
            Self::RepoNotLocal => "repo_not_local",
            Self::RepoPathMismatch => "repo_path_mismatch",
            Self::NotGit => "not_git",
            Self::NoBranch => "no_branch",
            Self::BranchNameMismatch => "branch_name_mismatch",
            Self::RefsUnresolvable => "refs_unresolvable",
            Self::DepartmentUnresolved => "department_unresolved",
        }
    }

    /// 人が読む 1 行（受信箱の文面）。
    pub fn label(self) -> &'static str {
        match self {
            Self::MultipleRepos => "リポジトリがちょうど 1 つではありません",
            Self::NoMarker => "作業ツリーの目印（worktree.json）が読めません",
            Self::MarkerRepoMismatch => "作業ツリーのリポジトリが task のリポジトリと一致しません",
            Self::RepoRowMissing => "案件にリポジトリの登録がありません",
            Self::RepoNotLocal => "登録リポジトリがローカルではありません",
            Self::RepoPathMismatch => "登録リポジトリの場所が取り込み先と一致しません",
            Self::NotGit => "Git のリポジトリではありません",
            Self::NoBranch => "作業ブランチがありません",
            Self::BranchNameMismatch => "ブランチ名が task ID で終わりません",
            Self::RefsUnresolvable => "既定ブランチか作業ブランチを解決できません",
            Self::DepartmentUnresolved => "取り込みを判定する部署が決まりません",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Delivery {
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub repo_id: RepoId,
    pub repo: String,
    pub branch: String,
    pub base: String,
    pub head: String,
    pub default_branch: String,
    pub department: String,
    pub review_run: String,
    pub worker_run: String,
    pub criterion_idx: usize,
    pub decision: Option<bool>,
    pub state: DeliveryState,
    pub detail: String,
    pub release: Option<String>,
    pub prepare_pid: Option<u32>,
    pub notification: Option<MessageId>,
    /// ADR-0051 Phase 106追記: merge直後にoriginへpushした時刻（成功または「既に同じかそれより先」で
    /// 省略したとき）。push機能を使わない（`[selfdeploy] push = false`）ときは常に`None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "crate::node_session::opt_rfc3339")]
    #[schemars(with = "Option<String>")]
    pub pushed_at: Option<OffsetDateTime>,
    /// push失敗の理由（stderr末尾500バイト）。1度だけ再試行し、再試行後もなお失敗したものには
    /// `[retried] ` を前置して以後は触らない目印にする（通知文には前置詞を外して出す）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_error: Option<String>,
}

pub trait DeliveryStore: Send + Sync {
    fn delivery_get(&self, task_id: TaskId) -> Result<Option<Delivery>, StoreError>;
    fn delivery_list(&self) -> Result<Vec<Delivery>, StoreError>;
    /// Compare-and-swap。ワーカー判定と tick が古い状態を上書きしない。
    fn delivery_save(
        &self,
        previous: Option<&Delivery>,
        next: &Delivery,
    ) -> Result<bool, StoreError>;
}
impl DeliveryStore for SqliteStore {
    fn delivery_get(&self, task_id: TaskId) -> Result<Option<Delivery>, StoreError> {
        let json: Option<String> = self
            .lock()?
            .query_row(
                "SELECT json FROM deliveries WHERE task_id=?1",
                [task_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|s| serde_json::from_str(&s).map_err(|e| StoreError::Invalid(e.to_string())))
            .transpose()
    }
    fn delivery_list(&self) -> Result<Vec<Delivery>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare("SELECT json FROM deliveries ORDER BY task_id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| serde_json::from_str(&r?).map_err(|e| StoreError::Invalid(e.to_string())))
            .collect()
    }
    fn delivery_save(
        &self,
        previous: Option<&Delivery>,
        next: &Delivery,
    ) -> Result<bool, StoreError> {
        let json = serde_json::to_string(next).map_err(|e| StoreError::Invalid(e.to_string()))?;
        let conn = self.lock()?;
        let changed = match previous {
            Some(old) => {
                let old_json =
                    serde_json::to_string(old).map_err(|e| StoreError::Invalid(e.to_string()))?;
                conn.execute(
                    "UPDATE deliveries SET json=?1 WHERE task_id=?2 AND json=?3",
                    rusqlite::params![json, next.task_id.to_string(), old_json],
                )?
            }
            None => conn.execute(
                "INSERT OR IGNORE INTO deliveries (task_id,json) VALUES (?1,?2)",
                rusqlite::params![next.task_id.to_string(), json],
            )?,
        };
        Ok(changed == 1)
    }
}
