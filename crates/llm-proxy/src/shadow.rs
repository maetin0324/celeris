//! ADR 2026-10-04 §7.1・§10 Phase 4: llm-proxy の shadow（decision / execution）。
//!
//! - **decision shadow**: primary が決まった後、別 policy（[`DecisionPolicy`]）が同じ候補列から何を
//!   選ぶかだけを記録する。upstream は呼ばない（追加 HTTP 0）。
//! - **execution shadow**: `ShadowPolicy.execute = true` かつ allowlist・標本化（`ShadowPolicy::admit`）を
//!   通った要求の**コピー**だけを [`ShadowQueue`] に入れ、候補 source/model へ送る。primary の応答経路は
//!   `submit` が同期で返るだけで shadow を待たない（実行は `tokio::spawn`）。queue は
//!   `max_queue_depth`、同時実行は `max_concurrency`、1 件の期限は `timeout_ms` で縛る。
//!   出力は SHA-256 と tokens だけを `routing_shadow_recorded` に残し、tool call は実行しない
//!   （出力を解釈しない。会話の続きも送らない）。
//! - dropped の理由: queue 満杯（`queue_full`）、queue 内で期限切れ（`timeout`）、primary と同じ
//!   非分離 resource group（`resource_group_shared`）、primary の予約待ち発生時の未開始分
//!   （`primary_pressure`）、日次予約の拒否（`cap_exceeded` / `unknown_cost`）。
//! - 日次予約は [`ShadowBudget`] で抽象化する。共有 DB（migration 0049）の実装は
//!   [`crate::shadow_budget::StoreShadowBudget`]。ここの [`AllowAllBudget`] は試験用の偽（費用が
//!   分かれば常に許可）。
//! - 実行枠: [`ShadowQueue::with_capacity`] で primary と同じ [`ReservationTable`] を渡すと、shadow は
//!   開始時に候補の resource group の枠を**待たずに**取る（primary が先に取った残りだけを使う）。
//!   埋まっていれば日次予約の前に `concurrency_limit` で dropped にする（予約・送信 0）。

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde_json::{Value, json};
use task_core::model_router::shadow::{
    SHADOW_RECORD_VERSION, ShadowKind, ShadowPolicy, ShadowReason, ShadowRecord, ShadowReservation,
    ShadowReservationRequest, ShadowSettlement, ShadowStatus, ShadowTarget, output_sha256,
};
use time::OffsetDateTime;
use tokio::time::Instant;

use crate::config::OpenAiCompatibleConfig;
use crate::openai::ChatCompletionRequest;
use crate::reservation::{Clock, ReservationTable, SlotKey};
use crate::sources::relay;

/// openai-compatible の source ref の接頭辞（`Attempt::source_label` と同じ形）。
const RELAY_PREFIX: &str = "openai-compatible:";

// ---------------------------------------------------------------------------
// 受け口・予約・実行の抽象
// ---------------------------------------------------------------------------

/// shadow 1 件の記録。`task_id` があれば daemon の sink が task event（`routing_shadow_recorded`）
/// として追記する。無ければ proxy 側の記録だけ。
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowEvent {
    pub task_id: Option<String>,
    pub record: ShadowRecord,
}

/// shadow の記録の受け口（daemon が配線する）。呼び出し側を待たせない実装にする。
pub trait ShadowSink: Send + Sync {
    fn record(&self, event: ShadowEvent);
}

/// 実行 shadow の日次予約（UTC 日、開始前に最悪消費を原子的に予約する。§7.1）。
pub trait ShadowBudget: Send + Sync {
    fn reserve(&self, request: &ShadowReservationRequest, now: OffsetDateTime)
    -> ShadowReservation;
    fn settle(&self, reservation_id: &str, settlement: ShadowSettlement);
}

/// 試験用の偽の予約: 費用が分かれば常に許可する（未知の費用は本物と同じく `unknown_cost`）。
#[derive(Debug, Default)]
pub struct AllowAllBudget {
    settled: StdMutex<Vec<(String, ShadowSettlement)>>,
}

impl AllowAllBudget {
    /// 確定した予約（試験で見る）。
    pub fn settled(&self) -> Vec<(String, ShadowSettlement)> {
        self.settled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl ShadowBudget for AllowAllBudget {
    fn reserve(
        &self,
        request: &ShadowReservationRequest,
        now: OffsetDateTime,
    ) -> ShadowReservation {
        match request.worst_effective_usd {
            Some(usd) if usd.is_finite() && usd >= 0.0 => ShadowReservation::Reserved {
                reservation_id: format!("fake-{}", request.shadow_id),
                day: task_core::model_router::shadow::utc_day(now),
            },
            _ => ShadowReservation::Denied(ShadowReason::UnknownCost),
        }
    }

    fn settle(&self, reservation_id: &str, settlement: ShadowSettlement) {
        self.settled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((reservation_id.to_string(), settlement));
    }
}

/// 候補で生成した結果。本文は hash にするためだけに受け取り、記録には残さない。
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowOutput {
    pub body: Vec<u8>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// 実測の effective 費用（分からなければ予約した最悪値で確定する）。
    pub effective_usd: Option<f64>,
}

pub type ShadowFuture = Pin<Box<dyn Future<Output = Result<ShadowOutput, String>> + Send>>;

/// 候補へ要求のコピーを 1 回だけ送る。tool call は実行しない（出力を解釈しない）。
pub trait ShadowExecutor: Send + Sync {
    fn execute(&self, job: &ShadowJob) -> ShadowFuture;
}

/// openai-compatible（self-hosted / relay）へ送る実行器。`stream = false` に固定して 1 回だけ送り、
/// 応答本文はそのまま hash 用に返す（tool_calls が入っていても何もしない）。OAuth の source は
/// 対象にしない（`unsupported_source` で failed）。
pub struct RelayShadowExecutor {
    client: reqwest::Client,
    sources: Vec<OpenAiCompatibleConfig>,
}

impl RelayShadowExecutor {
    pub fn new(client: reqwest::Client, sources: Vec<OpenAiCompatibleConfig>) -> Self {
        Self { client, sources }
    }
}

impl ShadowExecutor for RelayShadowExecutor {
    fn execute(&self, job: &ShadowJob) -> ShadowFuture {
        let cfg = job
            .target
            .source
            .strip_prefix(RELAY_PREFIX)
            .and_then(|id| self.sources.iter().find(|s| s.id == id))
            .cloned();
        let client = self.client.clone();
        let body = job.upstream_body();
        Box::pin(async move {
            let cfg = cfg.ok_or_else(|| "unsupported_source".to_string())?;
            let raw = relay::send_raw(&client, &cfg, &body)
                .await
                .map_err(|e| format!("send: {}", error_code(&e)))?;
            if !(200..300).contains(&raw.status) {
                return Err(format!("status {}", raw.status));
            }
            let bytes = raw
                .body
                .bytes()
                .await
                .map_err(|e| format!("body: {}", crate::neterr::safe_reqwest_error(&e)))?;
            let usage = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|v| v.get("usage").cloned());
            let tokens = |k: &str| {
                usage
                    .as_ref()
                    .and_then(|u| u.get(k))
                    .and_then(Value::as_u64)
            };
            Ok(ShadowOutput {
                input_tokens: tokens("prompt_tokens"),
                output_tokens: tokens("completion_tokens"),
                body: bytes.to_vec(),
                effective_usd: None,
            })
        })
    }
}

fn error_code(e: &crate::sources::SourceError) -> &'static str {
    use crate::sources::SourceError;
    match e {
        SourceError::Unauthorized => "unauthorized",
        SourceError::RateLimited { .. } => "rate_limited",
        SourceError::Upstream { .. } => "upstream",
        SourceError::Network(_) => "network",
        SourceError::Credentials(_) => "credentials",
        SourceError::Unavailable(_) => "unavailable",
    }
}

// ---------------------------------------------------------------------------
// 実行 shadow の queue
// ---------------------------------------------------------------------------

/// 実行 shadow 1 件（primary の要求のコピーと、候補・対象判定の属性）。
#[derive(Debug, Clone)]
pub struct ShadowJob {
    pub shadow_id: String,
    pub primary_decision_id: String,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub request_id: Option<String>,
    /// allowlist の判定に使う属性（`source` は候補＝shadow で呼ぶ側）。
    pub target: ShadowTarget,
    pub candidate_model: String,
    /// primary が実際に使った source。
    pub primary_source: String,
    pub primary_resource_group: Option<String>,
    pub candidate_resource_group: Option<String>,
    /// primary の要求のコピー（上流へは `stream = false`・候補 model で組み直す）。
    pub request: ChatCompletionRequest,
    /// output 上限込みの最悪 tokens。
    pub worst_tokens: u64,
    /// 最悪の effective 費用。`None` は未知（予約されず `unknown_cost` で dropped）。
    pub worst_effective_usd: Option<f64>,
}

impl ShadowJob {
    /// primary と同じ非分離の resource group を使うか。group が両方分かればその一致、分からなければ
    /// 同じ source（同じ subscription pool / 同じ self-hosted endpoint）を非分離とみなす。
    pub fn shares_resource_group(&self) -> bool {
        match (&self.primary_resource_group, &self.candidate_resource_group) {
            (Some(p), Some(c)) => p == c,
            _ => self.primary_source == self.target.source,
        }
    }

    /// 上流へ送る本文（要求のコピー、`stream = false`、候補 model）。
    pub fn upstream_body(&self) -> Value {
        let mut body = serde_json::to_value(&self.request).unwrap_or(Value::Null);
        if let Some(obj) = body.as_object_mut() {
            obj.insert("model".to_string(), json!(self.candidate_model));
            obj.insert("stream".to_string(), json!(false));
        }
        body
    }

    fn record(&self, status: ShadowStatus, reason: Option<ShadowReason>) -> ShadowRecord {
        ShadowRecord {
            shadow_id: self.shadow_id.clone(),
            primary_decision_id: self.primary_decision_id.clone(),
            kind: ShadowKind::Execution,
            status,
            reason,
            detail: None,
            policy_version: execution_policy_version(),
            run_id: self.run_id.clone(),
            request_id: self.request_id.clone(),
            candidate_model: Some(self.candidate_model.clone()),
            candidate_source: Some(self.target.source.clone()),
            input_tokens: None,
            output_tokens: None,
            output_sha256: None,
            cash_usd: None,
            effective_usd: None,
            latency_ms: None,
            reservation_id: None,
        }
    }
}

fn execution_policy_version() -> String {
    format!("llm-proxy:execution-shadow/v{SHADOW_RECORD_VERSION}")
}

/// queue の上限（`ShadowPolicy` の検証済みの値から作る）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowQueueSettings {
    pub max_queue_depth: usize,
    pub max_concurrency: usize,
    pub timeout: Duration,
}

impl ShadowQueueSettings {
    /// `execute = true` で検証を通る設定だけ `Some`（それ以外は実行 shadow を作らない）。
    pub fn from_policy(policy: &ShadowPolicy) -> Option<Self> {
        policy.daily_caps()?;
        Some(Self {
            max_queue_depth: usize::try_from(policy.max_queue_depth?).ok()?,
            max_concurrency: usize::try_from(policy.max_concurrency?).ok()?,
            timeout: Duration::from_millis(policy.timeout_ms?),
        })
    }
}

/// `submit` の結果（primary の経路はこれを見るだけで、shadow の完了を待たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// 入口の判定（off・allowlist 外・標本外）で落ちた。記録しない（送信 0）。
    NotAdmitted(ShadowReason),
    /// dropped として記録した。
    Dropped(ShadowReason),
    /// 空き枠があり実行を始めた。
    Started,
    /// 枠が空くまで queue で待つ。
    Queued,
}

struct Queued {
    job: ShadowJob,
    enqueued: Instant,
}

#[derive(Default)]
struct Inner {
    queued: VecDeque<Queued>,
    inflight: usize,
}

/// queue の差し替えられる設定（reload で 1 回の書き込みで丸ごと入れ替える）。
struct QueueConfig {
    policy: ShadowPolicy,
    settings: ShadowQueueSettings,
    budget: Arc<dyn ShadowBudget>,
}

/// 実行 shadow の bounded queue。
pub struct ShadowQueue {
    config: std::sync::RwLock<Arc<QueueConfig>>,
    /// 予約の持ち主（instance id。監査用）。
    owner: String,
    executor: Arc<dyn ShadowExecutor>,
    sink: Arc<dyn ShadowSink>,
    clock: Arc<dyn Clock>,
    /// primary と共有する実行枠の表（`None` なら枠を数えない）。
    capacity: Option<Arc<ReservationTable>>,
    inner: StdMutex<Inner>,
}

impl ShadowQueue {
    /// `policy` が実行 shadow を許さない（off・不完全）なら `None`。
    pub fn new(
        policy: ShadowPolicy,
        owner: impl Into<String>,
        executor: Arc<dyn ShadowExecutor>,
        budget: Arc<dyn ShadowBudget>,
        sink: Arc<dyn ShadowSink>,
        clock: Arc<dyn Clock>,
    ) -> Option<Arc<Self>> {
        let settings = ShadowQueueSettings::from_policy(&policy)?;
        Some(Arc::new(Self {
            config: std::sync::RwLock::new(Arc::new(QueueConfig {
                policy,
                settings,
                budget,
            })),
            owner: owner.into(),
            executor,
            sink,
            clock,
            capacity: None,
            inner: StdMutex::new(Inner::default()),
        }))
    }

    /// primary と共有する実行枠の表を差し込む（作った直後、まだ共有していない `Arc` にだけ効く）。
    pub fn with_capacity(mut self: Arc<Self>, table: Arc<ReservationTable>) -> Arc<Self> {
        match Arc::get_mut(&mut self) {
            Some(queue) => queue.capacity = Some(table),
            None => tracing::warn!("llm-proxy: shadow capacity ignored (queue already shared)"),
        }
        self
    }

    pub fn settings(&self) -> ShadowQueueSettings {
        self.config().settings
    }

    /// いま効いている policy。
    pub fn policy(&self) -> ShadowPolicy {
        self.config().policy.clone()
    }

    fn config(&self) -> Arc<QueueConfig> {
        Arc::clone(&self.config.read().unwrap_or_else(|e| e.into_inner()))
    }

    /// reload: policy・上限・日次予約先を 1 回で差し替える（読む側は常に新旧どちらか一方の組を見る）。
    /// 実行中の分は始めた時の予約先で確定する。`policy` が実行を許さない（off・不完全）なら上限は
    /// 旧値のまま、以後の `submit` は `NotAdmitted(Off)`、queue で待っている未開始分は `dropped/off`。
    /// `budget` が `None` なら旧い予約先を使い続ける。
    pub fn reconfigure(&self, policy: ShadowPolicy, budget: Option<Arc<dyn ShadowBudget>>) {
        let executable = ShadowQueueSettings::from_policy(&policy);
        {
            let mut slot = self.config.write().unwrap_or_else(|e| e.into_inner());
            let next = QueueConfig {
                settings: executable.unwrap_or(slot.settings),
                budget: budget.unwrap_or_else(|| Arc::clone(&slot.budget)),
                policy,
            };
            *slot = Arc::new(next);
        }
        if executable.is_none() {
            let drained: Vec<Queued> = self.lock().queued.drain(..).collect();
            for q in &drained {
                self.drop_job(
                    &q.job,
                    ShadowReason::Off,
                    Some("reconfigured_off".to_string()),
                );
            }
        }
    }

    /// (queue で待つ数, 実行中の数)。
    pub fn depth(&self) -> (usize, usize) {
        let inner = self.lock();
        (inner.queued.len(), inner.inflight)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 要求のコピーを受け取る。同期で返り、実行は spawn する（primary はこれを待たない）。
    /// tokio の runtime の中から呼ぶ。
    pub fn submit(self: &Arc<Self>, job: ShadowJob) -> SubmitOutcome {
        let config = self.config();
        if let Err(reason) = config.policy.admit(&job.target, &job.primary_decision_id) {
            return SubmitOutcome::NotAdmitted(reason);
        }
        if job.shares_resource_group() {
            self.drop_job(&job, ShadowReason::ResourceGroupShared, None);
            return SubmitOutcome::Dropped(ShadowReason::ResourceGroupShared);
        }
        let mut inner = self.lock();
        if inner.queued.is_empty() && inner.inflight < config.settings.max_concurrency {
            inner.inflight += 1;
            drop(inner);
            self.spawn_run(job);
            return SubmitOutcome::Started;
        }
        if inner.queued.len() < config.settings.max_queue_depth {
            inner.queued.push_back(Queued {
                job,
                enqueued: Instant::now(),
            });
            return SubmitOutcome::Queued;
        }
        drop(inner);
        self.drop_job(&job, ShadowReason::QueueFull, None);
        SubmitOutcome::Dropped(ShadowReason::QueueFull)
    }

    /// primary の予約待ちが発生した: 未開始の shadow を全部落とす（送信済みの分は止められない）。
    /// 落とした数を返す。
    pub fn primary_pressure(&self) -> usize {
        let drained: Vec<Queued> = self.lock().queued.drain(..).collect();
        for q in &drained {
            self.drop_job(&q.job, ShadowReason::PrimaryPressure, None);
        }
        drained.len()
    }

    fn drop_job(&self, job: &ShadowJob, reason: ShadowReason, detail: Option<String>) {
        let mut record = job.record(ShadowStatus::Dropped, Some(reason));
        record.detail = detail;
        self.emit(job, record);
    }

    fn emit(&self, job: &ShadowJob, record: ShadowRecord) {
        if let Err(e) = record.validate() {
            tracing::warn!(error = %e, "llm-proxy: invalid shadow record; not recorded");
            return;
        }
        self.sink.record(ShadowEvent {
            task_id: job.task_id.clone(),
            record,
        });
    }

    fn spawn_run(self: &Arc<Self>, job: ShadowJob) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.run(job).await;
            this.finish_one();
        });
    }

    /// 1 件の実行が終わった: 枠を返し、queue の先頭から期限内のものを始める。
    fn finish_one(self: &Arc<Self>) {
        let mut expired = Vec::new();
        let mut start = Vec::new();
        let settings = self.config().settings;
        {
            let mut inner = self.lock();
            inner.inflight = inner.inflight.saturating_sub(1);
            while inner.inflight < settings.max_concurrency {
                let Some(q) = inner.queued.pop_front() else {
                    break;
                };
                if q.enqueued.elapsed() >= settings.timeout {
                    expired.push(q.job);
                    continue;
                }
                inner.inflight += 1;
                start.push(q.job);
            }
        }
        for job in &expired {
            self.drop_job(
                job,
                ShadowReason::Timeout,
                Some("expired_in_queue".to_string()),
            );
        }
        for job in start {
            self.spawn_run(job);
        }
    }

    async fn run(&self, job: ShadowJob) {
        // 始めた時点の設定で予約・確定する（途中の reload で予約先が変わっても同じ先で確定する）。
        let config = self.config();
        let budget = Arc::clone(&config.budget);
        // primary が先に取った残りの枠だけを使う（待たない）。枠は実行が終わるまで持つ。
        let _slot = match (&self.capacity, &job.candidate_resource_group) {
            (Some(table), Some(group)) => match table.try_reserve(&[SlotKey::group(group.clone())])
            {
                Ok(slot) => Some(slot),
                Err(_) => {
                    self.drop_job(
                        &job,
                        ShadowReason::ConcurrencyLimit,
                        Some(format!("resource_group_full:{group}")),
                    );
                    return;
                }
            },
            _ => None,
        };
        let request = ShadowReservationRequest {
            shadow_id: job.shadow_id.clone(),
            owner: self.owner.clone(),
            worst_tokens: job.worst_tokens,
            worst_effective_usd: job.worst_effective_usd,
        };
        let reservation_id = match budget.reserve(&request, self.clock.now()) {
            ShadowReservation::Reserved { reservation_id, .. } => reservation_id,
            ShadowReservation::Denied(reason) => {
                self.drop_job(&job, reason, None);
                return;
            }
        };
        let started = Instant::now();
        let result =
            tokio::time::timeout(config.settings.timeout, self.executor.execute(&job)).await;
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut record = match result {
            Ok(Ok(output)) => {
                let tokens = output
                    .input_tokens
                    .unwrap_or(0)
                    .saturating_add(output.output_tokens.unwrap_or(0));
                let effective = output
                    .effective_usd
                    .or(job.worst_effective_usd)
                    .unwrap_or(0.0);
                budget.settle(
                    &reservation_id,
                    ShadowSettlement::Completed {
                        tokens,
                        effective_usd: effective,
                    },
                );
                let mut r = job.record(ShadowStatus::Completed, None);
                r.input_tokens = output.input_tokens;
                r.output_tokens = output.output_tokens;
                r.output_sha256 = Some(output_sha256(&output.body));
                r.effective_usd = Some(effective);
                r
            }
            Ok(Err(detail)) => {
                budget.settle(
                    &reservation_id,
                    ShadowSettlement::Failed {
                        tokens: None,
                        effective_usd: None,
                    },
                );
                let mut r = job.record(ShadowStatus::Failed, Some(ShadowReason::UpstreamError));
                r.detail = Some(detail);
                r
            }
            Err(_) => {
                budget.settle(&reservation_id, ShadowSettlement::TimedOut);
                job.record(ShadowStatus::Failed, Some(ShadowReason::Timeout))
            }
        };
        record.latency_ms = Some(latency_ms);
        record.reservation_id = Some(reservation_id);
        self.emit(&job, record);
    }
}

// ---------------------------------------------------------------------------
// decision shadow
// ---------------------------------------------------------------------------

/// 候補 1 件（primary の候補列と同じ順）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowCandidate {
    pub source: String,
    pub model: String,
}

/// 比較する別 policy。候補列から 1 件を選ぶだけで、送信・状態の変更はしない。
pub trait DecisionPolicy: Send + Sync {
    fn version(&self) -> String;
    fn choose(&self, candidates: &[ShadowCandidate]) -> Option<usize>;
}

/// 既定の比較 policy: self-hosted（openai-compatible）を最優先し、無ければ先頭。
#[derive(Debug, Default, Clone, Copy)]
pub struct PreferSelfHosted;

impl DecisionPolicy for PreferSelfHosted {
    fn version(&self) -> String {
        "llm-proxy:prefer-self-hosted/v1".to_string()
    }
    fn choose(&self, candidates: &[ShadowCandidate]) -> Option<usize> {
        candidates
            .iter()
            .position(|c| c.source.starts_with(RELAY_PREFIX))
            .or(if candidates.is_empty() { None } else { Some(0) })
    }
}

/// decision shadow の記録を作る（upstream は呼ばない）。`detail` は primary との比較
/// （`same_as_primary` / `differs_from_primary` / `no_candidate`）。
pub fn decision_record(
    policy: &dyn DecisionPolicy,
    shadow_id: String,
    primary_decision_id: String,
    run_id: Option<String>,
    request_id: Option<String>,
    candidates: &[ShadowCandidate],
    primary: &ShadowCandidate,
) -> ShadowRecord {
    let chosen = policy.choose(candidates).and_then(|i| candidates.get(i));
    let detail = match chosen {
        None => "no_candidate",
        Some(c) if c == primary => "same_as_primary",
        Some(_) => "differs_from_primary",
    };
    ShadowRecord {
        shadow_id,
        primary_decision_id,
        kind: ShadowKind::Decision,
        status: ShadowStatus::Completed,
        reason: None,
        detail: Some(detail.to_string()),
        policy_version: policy.version(),
        run_id,
        request_id,
        candidate_model: chosen.map(|c| c.model.clone()),
        candidate_source: chosen.map(|c| c.source.clone()),
        input_tokens: None,
        output_tokens: None,
        output_sha256: None,
        cash_usd: None,
        effective_usd: None,
        latency_ms: None,
        reservation_id: None,
    }
}

/// 最悪の effective 費用の見積もり（source, model, worst_tokens）。分からなければ `None`。
pub type ShadowCostFn = dyn Fn(&str, &str, u64) -> Option<f64> + Send + Sync;

/// `ProxyState` に差し込む shadow 一式。既定（`ProxyState::new`）は無し＝何もしない。
#[derive(Clone)]
pub struct ProxyShadow {
    /// decision shadow の比較 policy（`None` なら記録しない）。
    pub decision: Option<Arc<dyn DecisionPolicy>>,
    /// 実行 shadow の queue（`ShadowPolicy.execute = false` なら `None`）。
    pub execution: Option<Arc<ShadowQueue>>,
    /// decision shadow の記録先（実行 shadow は queue が自分の sink を持つ）。
    pub sink: Arc<dyn ShadowSink>,
    /// 最悪費用の見積もり。`None` なら全て未知（`unknown_cost` で dropped）。
    pub cost: Option<Arc<ShadowCostFn>>,
    /// 出力上限が要求に無いときの最悪 output tokens。
    pub default_output_reserve: u64,
}

#[cfg(test)]
#[path = "shadow_tests.rs"]
mod tests;
