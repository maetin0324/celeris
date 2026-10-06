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

/// ADR-0121 D2: 対象案件の root で delivery を作れなかった理由（`Event::DeliverySkipped.reason`）。
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
    /// Target ref read for this review attempt. NULL means no candidate was checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_candidate_sha: Option<String>,
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
        let json: Option<String> = self.with_read_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT json FROM deliveries WHERE task_id=?1",
                    [task_id.to_string()],
                    |row| row.get(0),
                )
                .optional()?)
        })?;
        json.map(|s| serde_json::from_str(&s).map_err(|e| StoreError::Invalid(e.to_string())))
            .transpose()
    }
    fn delivery_list(&self) -> Result<Vec<Delivery>, StoreError> {
        // ADR-0133 付記: 読むだけなので読み取り接続（tick ごとの通知同期が書き込み接続を取らない）。
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare("SELECT json FROM deliveries ORDER BY task_id")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.map(|r| serde_json::from_str(&r?).map_err(|e| StoreError::Invalid(e.to_string())))
                .collect()
        })
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
                    "UPDATE deliveries SET json=?1, target_sha=?4, reviewed_sha=?5, merge_candidate_sha=?6 WHERE task_id=?2 AND json=?3",
                    rusqlite::params![json, next.task_id.to_string(), old_json, next.target_sha, next.reviewed_sha, next.merge_candidate_sha],
                )?
            }
            None => conn.execute(
                "INSERT OR IGNORE INTO deliveries (task_id,json,target_sha,reviewed_sha,merge_candidate_sha) VALUES (?1,?2,?3,?4,?5)",
                rusqlite::params![next.task_id.to_string(), json, next.target_sha, next.reviewed_sha, next.merge_candidate_sha],
            )?,
        };
        Ok(changed == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, params};

    fn columns(conn: &Connection) -> (Option<String>, Option<String>, Option<String>) {
        conn.query_row(
            "SELECT target_sha, reviewed_sha, merge_candidate_sha FROM deliveries",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    }

    #[test]
    fn migration_0042_reads_existing_0036_delivery_and_saves_candidate_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("delivery.sqlite3");
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        for version in 1..=36 {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
        let task_id = TaskId::new();
        conn.execute(
            "INSERT INTO tasks (id,status,kind,priority,created_at,json) VALUES (?1,'ready','execute',0,'2026-10-02T00:00:00Z','{}')",
            [task_id.to_string()],
        )
        .unwrap();
        let legacy = serde_json::json!({
            "task_id": task_id,
            "project_id": ProjectId::new(),
            "repo_id": RepoId::new(),
            "repo": "example", "branch": "feature", "base": "base", "head": "head",
            "default_branch": "main", "department": "engineering",
            "review_run": "review-1", "worker_run": "worker-1", "criterion_idx": 0,
            "decision": null, "state": "reviewing", "detail": "",
            "release": null, "prepare_pid": null, "notification": null
        });
        conn.execute(
            "INSERT INTO deliveries (task_id,json) VALUES (?1,?2)",
            params![
                task_id.to_string(),
                serde_json::to_string(&serde_json::from_value::<Delivery>(legacy).unwrap())
                    .unwrap()
            ],
        )
        .unwrap();
        drop(conn);

        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), crate::SCHEMA_VERSION);
        let old = store.delivery_get(task_id).unwrap().unwrap();
        assert_eq!(old.target_sha, None);
        assert_eq!(old.reviewed_sha, None);
        assert_eq!(old.merge_candidate_sha, None);
        assert_eq!(store.delivery_list().unwrap(), vec![old.clone()]);
        let conn = Connection::open(&path).unwrap();
        assert_eq!(columns(&conn), (None, None, None));
        drop(conn);

        let mut next = old.clone();
        next.target_sha = Some("target".into());
        next.reviewed_sha = Some("reviewed".into());
        next.merge_candidate_sha = Some("reviewed".into());
        assert!(store.delivery_save(Some(&old), &next).unwrap());
        assert_eq!(store.delivery_get(task_id).unwrap(), Some(next.clone()));
        let conn = Connection::open(&path).unwrap();
        assert_eq!(
            columns(&conn),
            (
                Some("target".into()),
                Some("reviewed".into()),
                Some("reviewed".into())
            )
        );
        assert!(!store.delivery_save(Some(&old), &next).unwrap());
    }

    /// review sync: main の schema 41 の DB（1〜37 と 41 が当たり、38〜40 は予約で飛び、0042 は未知）を開くと
    /// 38〜40 は飛んだままで、振り直した 0042〜0045 が当たり、deliveries に検査対象の列ができる。
    #[test]
    fn migration_0042_fills_gaps_in_main_schema_41_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main41.sqlite3");
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .unwrap();
        for version in (1..=37).chain([41]) {
            SqliteStore::apply_migration_version(&mut conn, version).unwrap();
        }
        drop(conn);

        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), crate::SCHEMA_VERSION);
        assert_eq!(crate::SCHEMA_VERSION, 51);
        let conn = Connection::open(&path).unwrap();
        let mut stmt = conn
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .unwrap();
        let versions: Vec<u32> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            versions,
            (1..=37)
                .chain([41, 42, 43, 44, 45, 46, 47, 51])
                .collect::<Vec<u32>>()
        );
        let mut stmt = conn.prepare("PRAGMA table_info(deliveries)").unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for name in ["target_sha", "reviewed_sha", "merge_candidate_sha"] {
            assert!(names.iter().any(|column| column == name), "{name}");
        }
    }

    #[test]
    fn migration_0042_creates_columns_in_new_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.sqlite3");
        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), crate::SCHEMA_VERSION);
        let conn = Connection::open(&path).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(deliveries)").unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for name in ["target_sha", "reviewed_sha", "merge_candidate_sha"] {
            assert!(names.iter().any(|column| column == name));
        }
    }
}
