//! `task-ops` 全体で使うエラー型（ADR-0013 D7）。
//!
//! `Display` は現在の `celerisctl` の各コマンドのエラー文面をそのまま保つ（挙動を変えない）。
//! `celerisctl` 側は `OpsError` を `CliError` に写し、stderr の文面と exit code を変えない。

use task_core::{MilestoneId, ProjectId, Status, StoreError, TaskId};

#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    /// 指定した `TaskId` が存在しない。
    #[error("task not found: {0}")]
    NotFound(TaskId),

    /// 現在の状態では要求された操作ができない。
    /// `context` は `kind=.., status=..` や `status=..`、`action` は `approved` や
    /// `answered; only blocked tasks accept an answer` のように、元のメッセージの
    /// `cannot be <action>` 部分をそのまま埋め込む。
    #[error("task {id} ({context}) cannot be {action}")]
    InvalidState {
        id: TaskId,
        context: String,
        action: String,
    },

    /// 入力の検証エラー（受け入れ条件が無い、依存先が不正など）。
    #[error("{0}")]
    Validation(String),

    /// 呼び出し側が期待した `Status` と現在の `Status` が食い違う（`expected` 引数付き呼び出し）。
    #[error("expected status {expected:?} but task has status {actual:?}")]
    Conflict { expected: Status, actual: Status },

    /// ADR-0044 D6（Phase 55）: その id の案件が無い（API は 404）。
    #[error("project not found: {0}")]
    ProjectNotFound(ProjectId),

    /// ADR-0044 D6（Phase 55）: その id の途中目標が無い（API は 404）。
    #[error("milestone not found: {0}")]
    MilestoneNotFound(MilestoneId),

    /// ADR-0044 D6（Phase 55）: いまの状態ではその操作ができない（API は 409）。
    /// `subject` は `project <ULID>` / `milestone <ULID>`、`context` は `status=..`、
    /// `action` は `cancelled` / `paused` / `resumed` / `archived`。
    #[error("{subject} ({context}) cannot be {action}")]
    InvalidLifecycle {
        subject: String,
        context: String,
        action: String,
    },

    /// ADR-0074 D3.3（Phase F4a (c)）: その案件にその版の案件計画（マイルストーン DAG）の提案が無い
    /// （API は 404）。
    #[error("project {project_id} has no project plan proposal version {version}")]
    ProjectPlanProposalNotFound { project_id: ProjectId, version: u32 },

    /// ADR-0074 D3.3（Phase F4a (c)）: その版の案件計画は既に承認/却下済み（API は 409）。
    #[error("project {project_id} project plan version {version} was already decided")]
    ProjectPlanAlreadyDecided { project_id: ProjectId, version: u32 },

    /// ADR-0074 D3.4（Phase F4b (e)）: その案件には既に動いている案件計画 run か、未決の提案がある
    /// （同じ案件への二重の計画依頼。API は 409）。
    #[error("project {project_id} already has a project plan in flight: {detail}")]
    ProjectPlanInFlight {
        project_id: ProjectId,
        detail: String,
    },

    /// ADR-0074 D3.4（Phase F4b (e)）: 差分の提案が、提案の後に変わった現行の計画にもう当てはまらない
    /// （変える対象のマイルストーンが dispatch された、など。API は 409。replan をやり直す）。
    #[error("project {project_id} project plan version {version} no longer applies: {detail}")]
    ProjectPlanStale {
        project_id: ProjectId,
        version: u32,
        detail: String,
    },

    #[error(transparent)]
    Store(#[from] StoreError),
}
