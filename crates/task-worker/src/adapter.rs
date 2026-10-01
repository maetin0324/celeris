//! `WorkerAdapter` / `EventSink`（DESIGN §5.4, ADR-0003, ADR-0005 D1/D4）。
//! アダプタはワーカー固有の事情を閉じ込め、結果を `RunOutcome` に正規化して返す。
//! 状態遷移の判断はアダプタでは行わない（task-dispatch の責務）。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use task_core::{ArtifactRef, ProgressFields, RateLimitObservation, Usage};

use crate::protocol::{Evidence, ProviderFailure, RunRequest};

/// run の終端結果（ADR-0003 D3）。タイムアウト・終端無し exit も `Error{retryable:true}` に正規化する（D4）。
///
/// ADR-0072 D7（Phase E1）: `Yielded` / `BudgetExhausted` を追加した。アダプタが構造化して返せない
/// 古い経路のためには、dispatcher 側に `task_core::is_budget_outcome` を使う「二重の安全網」がある
/// （`Terminal::Error` のままでも予算切れとして拾われる）。
#[derive(Debug, Clone, PartialEq)]
pub enum Terminal {
    Done {
        summary: String,
        evidence: Vec<Evidence>,
        usage: Option<Usage>,
    },
    Question {
        text: String,
    },
    Error {
        message: String,
        retryable: bool,
    },
    /// ADR-0072 D9/D10: result.json の `{"yield": {...}}`（graceful yield）。`checkpoint` は
    /// checkpoint の意味の欄（`task_core::WorkerCheckpointInput` と同じ形）を寛容に持つ生の JSON。
    Yielded {
        checkpoint: serde_json::Value,
        usage: Option<Usage>,
    },
    /// ADR-0072 D7: turn / wall-clock / context の上限に当たった（予算切れでも usage を運ぶ）。
    BudgetExhausted {
        kind: task_core::BudgetKind,
        message: String,
        usage: Option<Usage>,
    },
    /// ADR-0090 D1: result.json の `{"type": "wait", "kind": "cluster_job", ...}`（クラスタ job の終了待ち）。
    /// `checkpoint` は `yield` と同じ checkpoint の意味の欄を寛容に持つ生の JSON（無ければ `None`）。
    Waiting {
        request: task_core::cluster_job::ClusterJobWaitRequest,
        checkpoint: Option<serde_json::Value>,
        usage: Option<Usage>,
    },
}

/// ADR-0090 D1: `result.json` の本文が wait の申告で、`question` を持たなければその終端（優先順位は
/// `question` > `wait` > `summary` > `yield`。入れ子の形 `{"summary": .., "wait": {..}}` は `summary` を一緒に持つ）。
/// JSON として読めない・申告が無ければ `None`。
pub fn result_file_wait(text: &str, usage: Option<Usage>) -> Option<Terminal> {
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    if value.get("question").is_some_and(|q| q.is_string()) {
        return None;
    }
    wait_terminal(&value, usage)
}

/// ADR-0090 D1: `result.json` の値が wait の申告なら、その終端（不正なら `Error{retryable: true}`）。
/// 申告が無ければ `None`（呼び出し側は従来の `question` / `summary` / `yield` の読み方に進む）。
pub fn wait_terminal(value: &serde_json::Value, usage: Option<Usage>) -> Option<Terminal> {
    match task_core::cluster_job::parse_wait_request(value)? {
        Ok((request, checkpoint)) => Some(Terminal::Waiting {
            request,
            checkpoint,
            usage,
        }),
        Err(reason) => Some(Terminal::Error {
            message: format!("result.json has an invalid cluster job wait: {reason}"),
            retryable: true,
        }),
    }
}

/// アダプタが返す run の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    pub terminal: Terminal,
    /// サブプロセスの exit code（取れた場合）。状態遷移には使わない（ADR-0003 D3）。
    pub exit_code: Option<i32>,
}

/// 生存監視の上限（ADR-0003 D4）。`wall_clock` は `task.budget.max_wall_secs`、
/// `idle_timeout` / `kill_grace` はアダプタ設定から。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    pub wall_clock: Duration,
    pub idle_timeout: Duration,
    pub kill_grace: Duration,
}

/// アダプタ内部の失敗（プロトコル上の `error` ではなく、起動不能など）。
/// ディスパッチャは `WorkerError{retryable:true}` に写し、`Throttled`/`AuthFailed`/`Exhausted` は
/// `ProviderPolicy::report` にも渡す（ADR-0005 D4/D6）。
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("failed to spawn worker: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("provider throttled (retry after {retry_after:?})")]
    Throttled { retry_after: Duration },
    #[error("provider auth failed: {0}")]
    AuthFailed(String),
    #[error("provider exhausted: {0}")]
    Exhausted(String),
    #[error("{0}")]
    Other(String),
}

impl AdapterError {
    /// `error.provider_failure`（プロトコル）や、CLI 系アダプタのエラー文面の分類結果を `AdapterError` に写す
    /// （ADR-0010 D5）。遷移の判断はディスパッチャが行う。
    pub fn from_provider_failure(failure: ProviderFailure, message: &str) -> Self {
        match failure {
            // `retry_after_secs: 0` で毎 tick 再 dispatch されるホットループを避けるため最低 1 秒（Phase 7 監査）。
            ProviderFailure::Throttled { retry_after_secs } => AdapterError::Throttled {
                retry_after: Duration::from_secs(retry_after_secs.max(1)),
            },
            ProviderFailure::AuthFailed => AdapterError::AuthFailed(message.to_string()),
            ProviderFailure::Exhausted => AdapterError::Exhausted(message.to_string()),
        }
    }
}

/// run 途中のイベント受け口。ディスパッチャがストアへ `WorkerProgress` / `ArtifactProduced` を追記する。
/// 同期 API（ストアは `Mutex<Connection>` で直列化されるため）。
pub trait EventSink: Send + Sync {
    /// Capability lifecycle, produced by the supervisor, never parsed from model text.
    fn browser_updated(&self, _browser: &task_core::BrowserRun) {}
    /// Trusted browser supervisor only. Implementations persist a wait and release the worker lease.
    fn browser_wait_open(
        &self,
        _request: &task_core::browser_wait::NewBrowserWait,
    ) -> Result<(), String> {
        Err("browser wait store unavailable".into())
    }
    fn browser_waits(&self) -> Result<Vec<task_core::browser_wait::BrowserWait>, String> {
        Ok(Vec::new())
    }
    /// Trusted browser supervisor only: consume an approved credential use exactly once.
    fn browser_approval_consume(
        &self,
        _wait: &task_core::browser_wait::BrowserWait,
    ) -> Result<task_core::browser_wait::ConsumedBrowserApproval, String> {
        Err("browser wait store unavailable".into())
    }
    /// Trusted browser supervisor only (ADR-0080 H3): the credential-injection section of a
    /// browser session starts (`true`) / ends (`false`). Implementations record it in the
    /// session's control state through the same store op as task-api's `auth-section`
    /// endpoint, so takeover/renew are refused while it is active. Failing closed is the
    /// caller's job.
    fn browser_auth_section(
        &self,
        _run_id: &str,
        _session_id: &str,
        _active: bool,
    ) -> Result<(), String> {
        Err("browser control store unavailable".into())
    }
    /// Trusted browser supervisor only (ADR-0113 D3): the control gate every shim-issued agent
    /// browser action passes before it reaches the browser. `None` (the default) makes the
    /// browser run refuse to start (fail closed).
    fn browser_control_gate(
        &self,
        _run_id: &str,
        _session_id: &str,
    ) -> Option<std::sync::Arc<dyn crate::browser_live::ControlGate>> {
        None
    }
    /// Trusted browser supervisor only (ADR-0100): one scrubbed live event for the session.
    fn browser_live(
        &self,
        _run_id: &str,
        _session_id: &str,
        _event: &task_core::browser_live::ScrubbedLiveEvent,
    ) {
    }
    fn progress(&self, msg: &str);
    /// ADR-0048 D2（Phase 60a）: 構造化した進行（`kind` / `tool` / `summary` / `detail`）。
    /// 既定は `msg` だけを `progress` に流す（この口を実装していないシンクでも従来どおり動く）。
    /// アダプタごとの写像はアダプタ側にあり、ここには何の判断も無い。
    fn progress_with(&self, msg: &str, _fields: &ProgressFields) {
        self.progress(msg);
    }
    /// パス検査と sha256 計算済みの成果物（`crate::artifact::resolve` を通したもの）。
    fn artifact(&self, artifact: &ArtifactRef);
    /// ワーカーの stdout から 1 行読むたびにアダプタが呼ぶ生存通知。ディスパッチャはこれでリースを延長する
    /// （ADR-0010 D7, P-7）。既定は何もしない。
    fn heartbeat(&self) {}
    /// `delegate` メッセージ（LLM アダプタでは `artifacts/delegate.json`）の提案（ADR-0016 D2）。ディスパッチャが検証して
    /// 子タスクを挿入する。既定は何もしない（`celerisctl worker run` など DB を変えない文脈）。
    fn delegate(&self, _tasks: &[task_core::DelegateTask]) {}
    /// ADR-0044 D2（Phase 53）: `{"type":"comment","body":"…"}`（任意回、非終端）。ディスパッチャは
    /// `task_comments` に `author_kind = node` で残す。既定は何もしない（`celerisctl worker run` など
    /// DB を変えない文脈）。
    fn comment(&self, _body: &str) {}
    /// claude-code アダプタが stream-json の `rate_limit_event` を解析するたびに呼ぶ（ADR-0024 D4）。
    /// ディスパッチャはプールのアカウントで走っている run のシンクから、この値を `AccountBook` に記録する。
    /// 既定は何もしない（`fake` アダプタや `celerisctl worker run` の観測用途など）。
    fn rate_limit(&self, _obs: RateLimitObservation) {}
    /// ADR-0054 D1（Phase 67）: アダプタが**実際に使った／割り当てられた**セッション id を報告する
    /// （高々 1 回。claude-code は `--session-id`/`--resume` に渡した値そのもの、codex は
    /// `thread.started` で観測した thread id、acp は `session/new`/`session/load` の応答の
    /// `sessionId`）。ディスパッチャはこれを `node_sessions` に記録する（新規なら作る、確認なら触らない）。
    /// `context.session` が無い run では呼ばれない。既定は何もしない（`fake` アダプタ・
    /// `celerisctl worker run` など DB を変えない文脈）。
    fn session_established(&self, _session_id: &str) {}
    /// ADR-0054 D1（Phase 67）: `context.session` で resume を頼んだのに、アダプタがそのセッションを
    /// 拒否した（見つからない・失効した）ことを報告する（高々 1 回）。ディスパッチャはこれを見て
    /// `node_sessions` の該当行を retire する（次の run からは新しいセッションになる。ADR-0054 D1
    /// 「失敗も同じ経路で作り直す」）。この run 自身の成否には関わらない（`RunOutcome` は通常どおり返す）。
    /// 既定は何もしない。
    fn session_resume_failed(&self, _reason: &str) {}
}

/// 何もしないシンク（テスト・デバッグ用）。
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl EventSink for NullSink {
    fn progress(&self, _msg: &str) {}
    fn artifact(&self, _artifact: &ArtifactRef) {}
}

/// ADR-0075 G3-fix1: `with_env_removed` の共通部。設定の `env` から `keys` を消し、`remove`（子プロセスで
/// `env_remove` する key）に足す（重複なし）。
pub(crate) fn remove_env_keys(
    env: &mut Vec<(String, String)>,
    remove: &mut Vec<String>,
    keys: &[String],
) {
    env.retain(|(k, _)| !keys.contains(k));
    for key in keys {
        if !remove.contains(key) {
            remove.push(key.clone());
        }
    }
}

/// ADR-0075 G3-fix1: 子プロセスの `Command` から `keys` を外す（`envs` より先に呼ぶ。後から足した値は残る）。
pub(crate) fn apply_env_removal(command: &mut tokio::process::Command, keys: &[String]) {
    for key in keys {
        command.env_remove(key);
    }
}

/// DESIGN §5.4 `trait WorkerAdapter`。`run_id` は成果物・ログのひも付け用（`runs/<run_id>/`）。
#[async_trait]
pub trait WorkerAdapter: Send + Sync {
    /// アダプタ識別子（設定の `adapter` と一致。例: `"fake"`）。
    fn id(&self) -> &str;
    fn account_id(&self) -> Option<&str> {
        None
    }
    fn model_for_tier(&self, _tier: task_core::Tier) -> Result<Option<String>, String> {
        Ok(None)
    }
    /// ADR-0069 D4（Phase 114）: その lane の設定上の reasoning effort（監査記録用。既定は無し）。
    fn reasoning_effort_for_tier(&self, _tier: task_core::Tier) -> Option<String> {
        None
    }
    fn with_model(&self, _model: &str) -> Option<Arc<dyn WorkerAdapter>> {
        None
    }

    /// ADR-0072 D14（Phase E4b 項目3）: `mode`（例: `"plan"`）を CLI の permission-mode に使う複製を
    /// 返す（`with_model` と同じ形）。既定は `None` = この経路を持たないアダプタ（呼び出し側は元の
    /// アダプタのまま実行する。permission-mode の概念が無い研究系ハーネスなどはこの既定のまま）。
    fn with_permission_mode(&self, _mode: &str) -> Option<Arc<dyn WorkerAdapter>> {
        None
    }

    /// ADR-0069 Phase 118 D1: このアダプタは reasoning effort を実際に CLI へ渡せるか
    /// （`with_reasoning_effort` が `Some` を返しうるか）。既定 `false`（対応する引数・環境変数が
    /// 無い。`claude-code` はこの既定のまま）。
    fn supports_reasoning_effort(&self) -> bool {
        false
    }
    /// ADR-0069 Phase 118 D1: 指定した reasoning effort を持つ複製を返す（`with_model` と同じ形）。
    /// 対応しないアダプタは既定の `None`（呼び出し側は元のアダプタのまま実行する）。
    fn with_reasoning_effort(&self, _effort: &str) -> Option<Arc<dyn WorkerAdapter>> {
        None
    }

    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError>;

    /// `extra` を環境の末尾に重ねた複製を返す（ADR-0024 D2: celeris の環境 < アダプタの環境 < プロバイダの環境 <
    /// アカウント）。プール（`account_pool = true`）で選んだアカウントの `CLAUDE_SECURESTORAGE_CONFIG_DIR` を
    /// 足すために使う。既定は `None`（この経路をサポートしないアダプタ）。
    fn with_env(&self, _extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        None
    }

    /// ADR-0075 G3-fix1: `keys` を子プロセスの環境から**外す**複製を返す（親〈daemon〉から継いだ値も、
    /// アダプタの設定の `env` にある同名の値も外す。`Command::env_remove`）。この後に `with_env` で足した値は残る。
    /// 既定は `None`（この経路を持たないアダプタ。dispatcher は `RUSTC_WRAPPER` などを空の値で上書きして代える）。
    fn with_env_removed(&self, _keys: &[String]) -> Option<Arc<dyn WorkerAdapter>> {
        None
    }

    /// ADR-0043 D3（Phase 56）: このアダプタが起こすコマンドを**コンテナの中で**走らせる複製を返す。
    /// 差し込み点は `crate::container::wrap` の 1 関数だけで、アダプタは受け取った計画を
    /// `Command` を組み立てた直後に渡すだけである（stdio 契約は変わらない）。
    ///
    /// 既定は `None` = **この経路を持たないアダプタ**（`paperqa` / `local-deep-research` は道具立てが
    /// ホストの venv にあるので、そもそも `container::decide` がコンテナを選ばない）。
    /// `None` が返ったらディスパッチャはホストで走らせる。
    fn with_container(
        &self,
        _plan: crate::container::SharedPlan,
    ) -> Option<Arc<dyn WorkerAdapter>> {
        None
    }
}
