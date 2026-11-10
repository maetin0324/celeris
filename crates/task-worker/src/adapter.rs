//! `WorkerAdapter` / `EventSink`（DESIGN §5.4, ADR-0003, ADR-0005 D1/D4）。
//! アダプタはワーカー固有の事情を閉じ込め、結果を `RunOutcome` に正規化して返す。
//! 状態遷移の判断はアダプタでは行わない（task-dispatch の責務）。

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use task_core::{ArtifactRef, ProgressFields, RateLimitObservation, Usage};
use tokio::process::Child;
use tokio::task::JoinHandle;
use tracing::warn;

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
        /// question で終わるまでに観測できた usage（取れた値だけ。無ければ `None`）。
        usage: Option<Usage>,
    },
    Error {
        message: String,
        retryable: bool,
        /// run の途中までに観測できた usage（取れた値だけ。無ければ `None`）。
        usage: Option<Usage>,
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

/// ADR-0090 D1: `result.json` の値が wait の申告なら、その終端（不正なら `Error{retryable: true}`）。
/// 申告が無ければ `None`（呼び出し側は従来の `question` / `summary` / `yield` の読み方に進む）。
/// harness adapter の結果ファイルは結果契約の正本（`result_report::interpret_result_text`）がこれを呼ぶ。
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
            usage: None,
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
/// AccountBook の (source_id, account_id) cooldown にも記録する（ADR-0005 D4/D6）。
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

/// 監視 loop の締め切り（ADR-0003 D4、ADR-0072 D7）。超過していなければ次に待つ時間（wall clock と idle の
/// 残りの小さい方）。wall clock の超過は予算切れ `BudgetExhausted{WallClock}`（continuation の対象。文言は
/// dispatcher の `classify_budget_kind_from_text` が見る語）、idle の超過は harness_error に分類変更しない
/// retryable な `Error`（ADR-0072 §7 U7）。両方が超過なら wall clock。usage は載せない（観測できる adapter が
/// 自分で載せる）。`idle_timeout` は値で受ける（adapter ごとに縮めることがある。ADR-0047 D4）。終端は
/// `Terminal` が大きいので箱に入れる（clippy `result_large_err`）。
pub(crate) fn deadline_terminal(
    start: Instant,
    last_activity: Instant,
    wall_clock: Duration,
    idle_timeout: Duration,
) -> Result<Duration, Box<Terminal>> {
    let wall_elapsed = start.elapsed();
    if wall_elapsed >= wall_clock {
        return Err(Box::new(Terminal::BudgetExhausted {
            kind: task_core::BudgetKind::WallClock,
            message: "wall clock exceeded".into(),
            usage: None,
        }));
    }
    let idle_elapsed = last_activity.elapsed();
    if idle_elapsed >= idle_timeout {
        return Err(Box::new(Terminal::Error {
            message: "idle timeout".into(),
            retryable: true,
            usage: None,
        }));
    }
    Ok((wall_clock - wall_elapsed).min(idle_timeout - idle_elapsed))
}

/// harness の stderr を `path` に写す task（終わった後の供給側失敗の分類は `subprocess::read_tail` で
/// その file を読む。ADR-0010 D5）。stderr が pipe でなければ `AdapterError::Other`。
pub(crate) fn spawn_capture_stderr(
    child: &mut Child,
    path: &Path,
    run_id: &str,
) -> Result<JoinHandle<()>, AdapterError> {
    let mut reader = child
        .stderr
        .take()
        .ok_or_else(|| AdapterError::Other("worker stderr was not piped".into()))?;
    let path = path.to_path_buf();
    let run_id = run_id.to_string();
    Ok(tokio::spawn(async move {
        match tokio::fs::File::create(&path).await {
            Ok(mut file) => {
                if let Err(e) = tokio::io::copy(&mut reader, &mut file).await {
                    warn!("run {run_id}: failed to write worker stderr.log: {e}");
                }
            }
            Err(e) => warn!("run {run_id}: failed to create worker stderr.log: {e}"),
        }
    }))
}

/// usage に cost が無く model が分かるときだけ、静的単価表（`task_core::estimate_cost_usd`。ADR-0061）で
/// 埋める。harness が報告した実測の cost は上書きしない。単価表に無い model・token の欠けは `None` のまま
/// （不明は不明のまま。予算の会計を推測で埋めない）。
pub(crate) fn with_estimated_cost(usage: Option<Usage>, model: Option<&str>) -> Option<Usage> {
    usage.map(|mut u| {
        if u.cost_usd.is_none()
            && let Some(model) = model
        {
            u.cost_usd = task_core::estimate_cost_usd(model, &u);
        }
        u
    })
}

/// run 途中のイベント受け口。ディスパッチャがストアへ `WorkerProgress` / `ArtifactProduced` を追記する。
/// 同期 API（ストアは `Mutex<Connection>` で直列化されるため）。
pub trait EventSink: Send + Sync {
    /// 起動前通知（ADR 2026-11-06-cos-run-launch-accounting-and-drain D1）。アダプタは harness の
    /// process を spawn する**同じ関数の直前**にこれを 1 回呼ぶ。`Err` なら spawn しない（run は
    /// `AdapterError::Other` で終わる）。spawn が失敗しても通知は取り消さない。route の failover で
    /// 2 本目以降の起動でも呼ばれるので、実装は再送で初回の記録を書き換えないこと（冪等）。
    /// 既定は何もしない（task run・`celerisctl worker run` など、起動の印を持たない文脈）。
    fn before_spawn(&self) -> Result<(), String> {
        Ok(())
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
    /// Adapter-observed completed context compaction / rollover, never inferred from model prose.
    fn context_compacted(&self) {
        self.progress_with(
            "context compacted",
            &ProgressFields::of(task_core::ProgressKind::Status)
                .with_tool(task_core::tree::CONTEXT_COMPACTION_TOOL),
        );
    }
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
    /// run 内で累積した usage（累積値。非終端）。観測できる adapter だけが呼ぶ。
    fn usage_progress(&self, _usage: &Usage) {}
    /// claude-code アダプタが stream-json の `rate_limit_event` を解析するたびに呼ぶ（ADR-0024 D4）。
    /// ディスパッチャはプールのアカウントで走っている run のシンクから、この値を `AccountBook` に記録する。
    /// 既定は何もしない（`fake` アダプタや `celerisctl worker run` の観測用途など）。
    fn rate_limit(&self, _obs: RateLimitObservation) {}
    /// ADR 2026-10-07-worker-no-subagents-no-llm-cli D5: adapter が `tool_use` の写像で subagent 道具・別の LLM CLI・
    /// LLM API の起動を検出した（警告。run は止めない）。ディスパッチャは `Event::WorkerPolicyViolation` を追記し、
    /// reviewer に渡す（D6）。既定は何もしない（`fake` アダプタ・`celerisctl worker run` など DB を変えない文脈）。
    fn policy_violation(&self, _violation: &crate::tool_policy::ToolPolicyViolation) {}
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

/// DESIGN §5.4 `trait WorkerAdapter`。`run_id` は成果物・ログのひも付け用（`runs/<run_id>/`）。
#[async_trait]
pub trait WorkerAdapter: Send + Sync {
    /// アダプタ識別子（設定の `adapter` と一致。例: `"fake"`）。
    fn id(&self) -> &str;
    fn model_namespace(&self) -> Option<&str> {
        None
    }
    fn account_id(&self) -> Option<&str> {
        None
    }
    async fn run(
        &self,
        req: RunRequest,
        run_id: &str,
        limits: RunLimits,
        sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError>;

    /// ADR 2026-11-10-worker-capability-ports D3: `settings` を 1 回で適用した複製と、受けられない項目を
    /// 返す。各 adapter は受ける項目を [`AdapterSettings::configured`] に渡して宣言する。
    /// 既定は何も受けない（複製なし、指定された項目を全部 `unsupported` に入れる）。許可側に倒れない。
    fn configure(&self, settings: &AdapterSettings) -> Configured {
        Configured {
            adapter: None,
            unsupported: settings.requested(),
        }
    }
}

/// ADR 2026-11-10-worker-capability-ports D3: run ごと・route ごとに adapter へ施す設定。空の欄は
/// 「変えない」。
#[derive(Clone, Default)]
pub struct AdapterSettings {
    /// 選んだ route の model（ADR 2026-10-11 llm-source-harness-selection）。
    pub model: Option<String>,
    /// 選んだ source の OpenAI 互換の endpoint。受けられない harness の route は呼び出し側が fail closed にする。
    pub llm_endpoint: Option<LlmEndpoint>,
    /// ADR-0072 D14: CLI の permission-mode（例: `"plan"`）。`Some("")` は渡さない（呼び出し側が `None` にする）。
    pub permission_mode: Option<String>,
    /// 環境の末尾に重ねる env（ADR-0024 D2: celeris の環境 < adapter の環境 < プロバイダの環境 < アカウント）。
    /// 同名は後勝ちなので、呼び出し側は重ねる順に並べる。
    pub env: Vec<(String, String)>,
    /// ADR-0043 D3: adapter が起こすコマンドをコンテナの中で走らせる計画（差し込み点は `container::wrap`）。
    pub container: Option<crate::container::SharedPlan>,
}

/// 選んだ source の endpoint。`api_key` は credential の参照から解決済みの値（protocol には載らない）。
#[derive(Clone)]
pub struct LlmEndpoint {
    pub base_url: String,
    pub api_key: Option<String>,
}

/// [`AdapterSettings`] の項目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdapterSetting {
    Model,
    LlmEndpoint,
    PermissionMode,
    Env,
    Container,
}

impl AdapterSetting {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::LlmEndpoint => "llm_endpoint",
            Self::PermissionMode => "permission_mode",
            Self::Env => "env",
            Self::Container => "container",
        }
    }
}

impl std::fmt::Display for AdapterSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AdapterSettings {
    /// env だけを重ねる設定（reviewer の cargo env・run の TMPDIR・eval の target・CoS の run credential）。
    pub fn env_only(env: Vec<(String, String)>) -> Self {
        Self {
            env,
            ..Self::default()
        }
    }

    /// 指定された項目（[`AdapterSetting`] の順）。
    pub fn requested(&self) -> Vec<AdapterSetting> {
        [
            (self.model.is_some(), AdapterSetting::Model),
            (self.llm_endpoint.is_some(), AdapterSetting::LlmEndpoint),
            (
                self.permission_mode.is_some(),
                AdapterSetting::PermissionMode,
            ),
            (!self.env.is_empty(), AdapterSetting::Env),
            (self.container.is_some(), AdapterSetting::Container),
        ]
        .into_iter()
        .filter_map(|(set, item)| set.then_some(item))
        .collect()
    }

    /// adapter の `configure` の共通の判定。`accepts` の外の指定を `unsupported` に入れ、`accepts` の
    /// 中の指定が 1 つでもあれば `build` で複製を作る（無ければ複製しない）。
    pub fn configured(
        &self,
        accepts: &[AdapterSetting],
        build: impl FnOnce() -> Arc<dyn WorkerAdapter>,
    ) -> Configured {
        let (applied, unsupported): (Vec<_>, Vec<_>) = self
            .requested()
            .into_iter()
            .partition(|item| accepts.contains(item));
        Configured {
            adapter: (!applied.is_empty()).then(build),
            unsupported,
        }
    }

    /// `model` が指定されていれば `slot` を置き換える（空の欄は変えない）。
    pub fn apply_model(&self, slot: &mut Option<String>) {
        if let Some(model) = &self.model {
            *slot = Some(model.clone());
        }
    }

    /// `env` を `env` の末尾に足す。同名は後勝ちなので、足した値が既存の同名キーに勝つ。
    pub fn apply_env(&self, env: &mut Vec<(String, String)>) {
        env.extend(self.env.iter().cloned());
    }

    /// `container` が指定されていれば `slot` を置き換える（空の欄は変えない）。
    pub fn apply_container(&self, slot: &mut Option<crate::container::SharedPlan>) {
        if let Some(plan) = &self.container {
            *slot = Some(Arc::clone(plan));
        }
    }
}

/// [`WorkerAdapter::configure`] の結果。
pub struct Configured {
    /// 受けた項目を適用した複製。何も適用しなかったら `None`。
    pub adapter: Option<Arc<dyn WorkerAdapter>>,
    /// 指定されたが受けられなかった項目（空 = 指定を全部適用した）。
    pub unsupported: Vec<AdapterSetting>,
}

impl Configured {
    /// 別の adapter を包む adapter の転送: 内側の複製があれば `wrap` で包み直し、`unsupported` は内側の
    /// 報告をそのまま返す。
    pub fn map_adapter(
        self,
        wrap: impl FnOnce(Arc<dyn WorkerAdapter>) -> Arc<dyn WorkerAdapter>,
    ) -> Self {
        Self {
            adapter: self.adapter.map(wrap),
            unsupported: self.unsupported,
        }
    }

    /// 呼び出し側の受け取り方。複製が無ければ `base` を使う。
    pub fn resolve(
        self,
        base: &Arc<dyn WorkerAdapter>,
    ) -> (Arc<dyn WorkerAdapter>, Vec<AdapterSetting>) {
        (
            self.adapter.unwrap_or_else(|| Arc::clone(base)),
            self.unsupported,
        )
    }
}

#[cfg(test)]
#[path = "adapter_tests.rs"]
mod run_common_tests;

/// ADR 2026-11-10-worker-capability-ports D3・D5 5b: `configure` の unsupported の報告と browser-specialist
/// の転送（名前は `adapter_settings_` で始める）。守る不変条件: 受けられない項目は必ず `unsupported` に
/// 出て（呼び出し側の fail closed・log の根拠）、受けた項目は 1 回の適用で複製に載る。既存の試験は
/// adapter ごとの「受ける」側（`with_env_overrides_…` など）だけを見ており、受けない側の報告・既定実装・
/// browser-specialist の転送を見る試験は無かった。
#[cfg(test)]
mod settings_tests {
    use super::*;
    use crate::container::{ContainerPlan, Runtime};
    use crate::protocol::{PROTOCOL_VERSION, RunContext, sample_task_for_tests};
    use AdapterSetting as S;
    use std::sync::Mutex;

    const ALL: [AdapterSetting; 5] = [
        S::Model,
        S::LlmEndpoint,
        S::PermissionMode,
        S::Env,
        S::Container,
    ];

    fn kv(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    /// `items` の欄だけを埋めた設定。
    fn settings_of(items: &[AdapterSetting]) -> AdapterSettings {
        let has = |item| items.contains(&item);
        AdapterSettings {
            model: has(S::Model).then(|| "route-model".to_string()),
            llm_endpoint: has(S::LlmEndpoint).then(|| LlmEndpoint {
                base_url: "http://127.0.0.1:18000/v1".into(),
                api_key: Some("k".into()),
            }),
            permission_mode: has(S::PermissionMode).then(|| "plan".to_string()),
            env: if has(S::Env) {
                vec![kv("ROUTING_ACCOUNT", "account-a")]
            } else {
                Vec::new()
            },
            container: has(S::Container).then(|| {
                Arc::new(ContainerPlan {
                    runtime: Runtime::Podman,
                    program: "podman".into(),
                    image: "celeris-worker:latest".into(),
                    task_dir: "/w/01TASK".into(),
                    dir_repos: vec![],
                    creds: vec![],
                    extra_mounts: vec![],
                    knowledge_root: None,
                    env: vec![],
                    task_id: "01TASK".into(),
                    uid: 1001,
                    gid: 1001,
                })
            }),
        }
    }

    /// 設定を何も受けない adapter（`configure` の既定実装）。
    struct Bare;

    #[async_trait]
    impl WorkerAdapter for Bare {
        fn id(&self) -> &str {
            "bare"
        }
        async fn run(
            &self,
            _req: RunRequest,
            _run_id: &str,
            _limits: RunLimits,
            _sink: &dyn EventSink,
        ) -> Result<RunOutcome, AdapterError> {
            Err(AdapterError::Other("bare".into()))
        }
    }

    /// `accepts` だけを受け、run の時点の env を `log` に残す adapter。
    #[derive(Clone)]
    struct Probe {
        accepts: Vec<AdapterSetting>,
        env: Vec<(String, String)>,
        log: Arc<Mutex<Vec<Vec<(String, String)>>>>,
    }

    #[async_trait]
    impl WorkerAdapter for Probe {
        fn id(&self) -> &str {
            "probe"
        }
        async fn run(
            &self,
            _req: RunRequest,
            _run_id: &str,
            _limits: RunLimits,
            _sink: &dyn EventSink,
        ) -> Result<RunOutcome, AdapterError> {
            self.log.lock().unwrap().push(self.env.clone());
            Err(AdapterError::Other("probe".into()))
        }
        fn configure(&self, settings: &AdapterSettings) -> Configured {
            settings.configured(&self.accepts, || {
                let mut next = self.clone();
                settings.apply_env(&mut next.env);
                Arc::new(next)
            })
        }
    }

    #[test]
    fn adapter_settings_default_configure_reports_every_requested_item() {
        let configured = Bare.configure(&settings_of(&ALL));
        assert!(configured.adapter.is_none());
        assert_eq!(configured.unsupported, ALL);
        let empty = Bare.configure(&AdapterSettings::default());
        assert!(empty.adapter.is_none());
        assert!(empty.unsupported.is_empty());
        let base: Arc<dyn WorkerAdapter> = Arc::new(Bare);
        let (adapter, unsupported) = base.configure(&settings_of(&ALL)).resolve(&base);
        assert!(
            Arc::ptr_eq(&adapter, &base),
            "nothing applied keeps the base"
        );
        assert_eq!(unsupported, ALL);
    }

    /// ADR D3 3a の表: adapter ごとに受けない項目だけが報告され、受けた項目があれば複製が返る
    /// （id は変わらない）。受けない項目だけを指定したときは複製を作らない。
    #[test]
    fn adapter_settings_each_harness_reports_what_it_cannot_take() {
        let no_container = vec![S::LlmEndpoint, S::PermissionMode, S::Container];
        let cases: Vec<(Arc<dyn WorkerAdapter>, Vec<AdapterSetting>)> = vec![
            (
                Arc::new(crate::AcpAdapter::new(Default::default())),
                vec![S::LlmEndpoint, S::PermissionMode],
            ),
            (
                Arc::new(crate::AiderAdapter::new(Default::default())),
                vec![S::LlmEndpoint, S::PermissionMode],
            ),
            (
                Arc::new(crate::ClaudeCodeAdapter::new(Default::default())),
                vec![S::LlmEndpoint],
            ),
            (
                Arc::new(crate::CodexAdapter::new(Default::default())),
                vec![S::LlmEndpoint, S::PermissionMode],
            ),
            (
                Arc::new(crate::FakeAdapter::default()),
                vec![S::LlmEndpoint, S::PermissionMode],
            ),
            (
                Arc::new(crate::LangMemAdapter::new(Default::default())),
                no_container.clone(),
            ),
            (
                Arc::new(crate::LdrAdapter::new(Default::default())),
                no_container.clone(),
            ),
            (
                Arc::new(crate::PaperQaAdapter::new(Default::default())),
                no_container,
            ),
            (
                Arc::new(crate::PiAdapter::new(Default::default())),
                vec![S::PermissionMode],
            ),
        ];
        for (adapter, expected) in cases {
            let id = adapter.id().to_string();
            let configured = adapter.configure(&settings_of(&ALL));
            assert_eq!(configured.unsupported, expected, "{id}");
            let copy = configured.adapter.expect("an accepted item makes a copy");
            assert_eq!(copy.id(), id);
            let only_unsupported = adapter.configure(&settings_of(&expected));
            assert!(only_unsupported.adapter.is_none(), "{id}");
            assert_eq!(only_unsupported.unsupported, expected, "{id}");
        }
    }

    /// `BrowserSpecialistAdapter` は `configure` を内側へ 1 回転送し、内側の `unsupported` をそのまま返し、
    /// 複製の id は `browser-specialist` のまま。run は設定を受けた内側へ届く。内側が何も受けなければ
    /// 複製を作らない。
    #[tokio::test]
    async fn adapter_settings_browser_specialist_forwards_and_keeps_identity() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let inner = Probe {
            accepts: vec![S::Model, S::Env],
            env: vec![kv("A", "1")],
            log: Arc::clone(&log),
        };
        let specialist = crate::BrowserSpecialistAdapter::new(Arc::new(inner));
        let mut settings = settings_of(&ALL);
        settings.env = vec![kv("A", "2"), kv("B", "x")];
        let configured = specialist.configure(&settings);
        assert_eq!(
            configured.unsupported,
            [S::LlmEndpoint, S::PermissionMode, S::Container]
        );
        let copy = configured
            .adapter
            .expect("the inner harness took model and env");
        assert_eq!(copy.id(), crate::BrowserSpecialistAdapter::ID);
        let mut task = sample_task_for_tests();
        task.skills = vec![task_core::browser::BROWSER_SKILL.to_string()];
        let dir = std::path::PathBuf::from("/nonexistent-adapter-settings");
        let req = RunRequest {
            protocol: PROTOCOL_VERSION,
            task,
            workspace: dir.clone(),
            artifacts_dir: dir.join("artifacts"),
            work_dir: None,
            context: RunContext::default(),
            cargo_target_dir: None,
            llm_route: None,
        };
        let limits = RunLimits {
            wall_clock: LONG,
            idle_timeout: LONG,
            kill_grace: Duration::from_millis(1),
        };
        let _ = copy.run(req, "r1", limits, &NullSink).await;
        assert_eq!(
            log.lock().unwrap().as_slice(),
            &[vec![kv("A", "1"), kv("A", "2"), kv("B", "x")]]
        );

        let bare = crate::BrowserSpecialistAdapter::new(Arc::new(Bare));
        let configured = bare.configure(&settings_of(&ALL));
        assert!(configured.adapter.is_none());
        assert_eq!(configured.unsupported, ALL);
    }

    /// 呼び出し側は env を重ねる順に並べ、adapter は末尾に足す（同名は後勝ち）。空の欄は変えない。
    #[test]
    fn adapter_settings_env_is_appended_in_order_and_empty_fields_change_nothing() {
        let settings = AdapterSettings {
            env: vec![kv("A", "cargo"), kv("B", "x"), kv("A", "tmp")],
            ..AdapterSettings::default()
        };
        let mut env = vec![kv("A", "base")];
        settings.apply_env(&mut env);
        assert_eq!(
            env,
            [
                kv("A", "base"),
                kv("A", "cargo"),
                kv("B", "x"),
                kv("A", "tmp")
            ]
        );
        let mut model = Some("configured".to_string());
        settings.apply_model(&mut model);
        assert_eq!(model.as_deref(), Some("configured"));
        let mut container = None;
        settings.apply_container(&mut container);
        assert!(container.is_none());
        assert_eq!(settings.requested(), [S::Env]);
    }

    const LONG: Duration = Duration::from_secs(60);
}
