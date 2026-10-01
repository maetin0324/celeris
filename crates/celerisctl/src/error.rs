//! celerisctl 全コマンド共通のエラー型とヘルパ。

use task_core::{StoreError, TaskId};
use task_ops::OpsError;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Message(String),
}

impl CliError {
    pub fn msg(s: impl Into<String>) -> Self {
        CliError::Message(s.into())
    }
}

/// `task-ops` のエラーを `CliError` に写す（ADR-0013 D7）。`OpsError::Store` は
/// `CliError::Store` に、それ以外は `Display` の文面をそのまま `CliError::Message` にする
/// （stderr の文面と exit code を変えない）。
impl From<OpsError> for CliError {
    fn from(e: OpsError) -> Self {
        match e {
            OpsError::Store(se) => CliError::Store(se),
            other => CliError::Message(other.to_string()),
        }
    }
}

/// CLI から受け取った文字列を `TaskId` にパースする。失敗はユーザー向けメッセージにする。
pub fn parse_task_id(s: &str) -> Result<TaskId, CliError> {
    s.parse::<TaskId>()
        .map_err(|e| CliError::msg(format!("invalid task id '{s}': {e}")))
}

/// ADR-0095 D6: 書き込みが `SQLITE_READONLY` で落ちたときの言い換え（worker の run の namespace で
/// 本番 DB が読み取り専用、または DB の schema がこの celerisctl より新しくて読み取り専用で開いた）。
pub const READ_ONLY_HINT: &str = "the database is open read-only, so celerisctl cannot write to it \
     (ADR-0095): inside a Celeris worker run the production DB is read-only, and a DB whose schema is \
     newer than this celerisctl is opened read-only. Reads such as `show`/`ls` still work; ask for \
     changes through the HTTP API or the human. To create a follow-up task from a worker run, run \
     `celerisctl add` without `--db` inside the run (it is queued in the run's followups.json and \
     created in the run's project when the run ends; ADR-0098)";

/// stderr に出す 1 行（`error: ` は呼び出し側が付ける）。読み取り専用の書き込み失敗には
/// `READ_ONLY_HINT` を足す（task-ops が文面に写したものも SQLite の文言で拾う）。
pub fn render(e: &CliError) -> String {
    let readonly = match e {
        CliError::Store(se) => task_core::is_readonly_error(se),
        CliError::Message(m) => m.contains("attempt to write a readonly database"),
    };
    if readonly {
        format!("{e}\n{READ_ONLY_HINT}")
    } else {
        e.to_string()
    }
}
