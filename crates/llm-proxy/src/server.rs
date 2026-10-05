//! axum サーバ（ADR-0053 D1）: `GET /v1/models`、`POST /v1/chat/completions`（非 stream / stream）、
//! `POST /v1/embeddings`（501）、`GET /healthz`。loopback のみ想定。Bearer は `[api] token` と同じもの。
//!
//! 判断（選択・cooldown・やり直し）はここに集める。`sources::*` は「送る」だけ、`selection` は
//! 「並べる」だけ、という分業をここで束ねる。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{Json, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use task_core::AccountAdapter;
use task_core::SharedRole;
use task_core::model_router::context_registry::RoutingContextRegistry;
use task_core::model_router::feedback::RequestSourceAttempt;
use task_core::store::RoutingCorrelation;
use task_dispatch::accounts::{
    AccountBook, AccountCooldownReason, AccountDir, cooldown_for_failure, scan_accounts,
};
use ulid::Ulid;

use crate::config::{LlmProxyConfig, OpenAiCompatibleConfig};
use crate::estimator_shadow::{EstimatorShadow, EstimatorShadowInput};
use crate::fallback::{
    AfterFailure, Breakers, FailureClass, FallbackBudget, FallbackSettings, RequestConstraints,
};
use crate::log::{self, RequestLogRow};
use crate::naming::{self, ModelRequest, SourceKind, SourceScope};
use crate::openai::ChatCompletionRequest;
use crate::reservation::{Clock, SystemClock};
use crate::routing_context::{
    self as rctx, InFlightContexts, InFlightGuard, ProxyEventSink, ResolvedRouting,
};
use crate::selection::{self, PoolInput, SelectedAccount};
use crate::shadow::{ProxyShadow, ShadowCandidate, ShadowEvent, ShadowJob};
use crate::sources::{SendOutcome, SourceError, claude, codex, relay};

const X_SOURCE: &str = "x-celeris-source";
const X_ACCOUNT: &str = "x-celeris-account";

/// ADR-0063 Phase 109b C1: 候補が一時的に全部無くなったときの再走査の回数と待ち（合計
/// `NO_SOURCE_MAX_RESCANS * NO_SOURCE_RESCAN_DELAY` = 2 秒。3 秒以内の要件を満たす）。
const NO_SOURCE_MAX_RESCANS: u32 = 2;
const NO_SOURCE_RESCAN_DELAY: Duration = Duration::from_secs(1);

/// relay の probe 結果のキャッシュ 1 件: (probe した時刻, 結果)。`Err` は人が読む理由。
type ProbeCacheEntry = (Instant, Result<(), String>);

/// プロキシが動くのに要るもの一式。`Arc` で共有する。
pub struct ProxyState {
    pub config: LlmProxyConfig,
    pub client: reqwest::Client,
    /// `[accounts] claude_dir` 直下の `.celeris-usage.json`。CLI ワーカーの dispatcher と**同じ帳簿**
    /// （cooldown・観測値を共有する。ADR-0053: 「既存のアカウントプールを使う」）。
    pub claude_book: Option<Arc<StdMutex<AccountBook>>>,
    pub codex_book: Option<Arc<StdMutex<AccountBook>>>,
    pub token: Option<String>,
    pub role: SharedRole,
    pub db_path: Option<PathBuf>,
    pub busy_timeout: Duration,
    /// 供給元 id → (probe した時刻, 結果)。`Err` は人が読む理由（`relay::probe`）。
    probe_cache: StdMutex<HashMap<String, ProbeCacheEntry>>,
    in_use: StdMutex<HashMap<String, usize>>,
    /// 同一要求内 fallback の上限・breaker の設定（ADR 2026-10-04 §5）。
    fallback: FallbackSettings,
    /// deployment（`<source>/<upstream model>`）ごとの closed/open/half_open。
    breakers: Arc<Breakers>,
    /// fallback の deadline と breaker の遷移に使う時計（試験で差し替える）。
    clock: Arc<dyn Clock>,
    /// daemon が登録した `context_ref` の解決先（ADR 2026-10-04 §10 Phase 3）。`None` の proxy は
    /// `x-celeris-routing-context` を信頼できないので、header 付きの要求を 400 にする。
    routing_registry: Option<Arc<dyn RoutingContextRegistry>>,
    /// task に相関できる要求の受け口。`None` なら何もしない（proxy log だけ）。
    event_sink: Option<Arc<dyn ProxyEventSink>>,
    /// run ごとの in-flight 要求数（ref を外す前に daemon が見る）。
    in_flight: Arc<InFlightContexts>,
    /// ADR 2026-10-04 §7.1・Phase 4: decision / execution shadow。`None`（既定）なら何もしない。
    shadow: Option<ProxyShadow>,
    /// ADR 2026-10-04 §10 Phase 5: sidecar estimator の shadow。`None`（既定）なら何もしない。
    estimator_shadow: Option<Arc<EstimatorShadow>>,
}

impl ProxyState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: LlmProxyConfig,
        client: reqwest::Client,
        claude_book: Option<Arc<StdMutex<AccountBook>>>,
        codex_book: Option<Arc<StdMutex<AccountBook>>>,
        token: Option<String>,
        role: SharedRole,
        db_path: Option<PathBuf>,
        busy_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            client,
            claude_book,
            codex_book,
            token,
            role,
            db_path,
            busy_timeout,
            probe_cache: StdMutex::new(HashMap::new()),
            in_use: StdMutex::new(HashMap::new()),
            breakers: Breakers::new(FallbackSettings::default().breaker),
            fallback: FallbackSettings::default(),
            clock: Arc::new(SystemClock),
            routing_registry: None,
            event_sink: None,
            in_flight: Arc::new(InFlightContexts::default()),
            shadow: None,
            estimator_shadow: None,
        })
    }

    /// shadow を差し込む（作った直後、まだ共有していない `Arc` にだけ効く）。
    pub fn with_shadow(mut self: Arc<Self>, shadow: Option<ProxyShadow>) -> Arc<Self> {
        match Arc::get_mut(&mut self) {
            Some(state) => state.shadow = shadow,
            None => tracing::warn!("llm-proxy: shadow ignored (state already shared)"),
        }
        self
    }

    /// estimator shadow を差し込む（作った直後、まだ共有していない `Arc` にだけ効く）。
    pub fn with_estimator_shadow(
        mut self: Arc<Self>,
        shadow: Option<Arc<EstimatorShadow>>,
    ) -> Arc<Self> {
        match Arc::get_mut(&mut self) {
            Some(state) => state.estimator_shadow = shadow,
            None => tracing::warn!("llm-proxy: estimator shadow ignored (state already shared)"),
        }
        self
    }

    /// shadow（decision / execution / estimator）のどれかが差し込まれているか。
    fn any_shadow(&self) -> bool {
        self.shadow.is_some() || self.estimator_shadow.is_some()
    }

    /// primary が成功した後に shadow を走らせる。decision shadow は記録だけ、実行 shadow は要求の
    /// コピーを queue に入れるだけで、どちらも primary の応答を待たせない（同期で返る）。
    fn shadow_after_primary(
        &self,
        routing: &ResolvedRouting,
        request_id: &str,
        lane: &str,
        candidates: &[ShadowCandidate],
        primary: &ShadowCandidate,
        req: &ChatCompletionRequest,
    ) {
        self.estimator_after_primary(routing, request_id, lane, candidates, primary, req);
        let Some(shadow) = &self.shadow else {
            return;
        };
        let decision_id = routing.decision_id.clone();
        let mut chosen: Option<ShadowCandidate> = None;
        if let Some(policy) = &shadow.decision {
            let record = crate::shadow::decision_record(
                policy.as_ref(),
                format!("shd_{}", Ulid::new()),
                decision_id.clone(),
                routing.run_id().map(str::to_owned),
                Some(request_id.to_string()),
                candidates,
                primary,
            );
            chosen = record
                .candidate_source
                .clone()
                .zip(record.candidate_model.clone())
                .map(|(source, model)| ShadowCandidate { source, model });
            shadow.sink.record(ShadowEvent {
                task_id: routing.task_id().map(str::to_owned),
                record,
            });
        }
        let Some(queue) = &shadow.execution else {
            return;
        };
        // 実行する候補: 比較 policy の選択が primary と違えばそれ、無ければ primary 以外の先頭。
        let candidate = chosen
            .filter(|c| c != primary)
            .or_else(|| candidates.iter().find(|c| *c != primary).cloned());
        let Some(candidate) = candidate else {
            return;
        };
        let ctx = &routing.context;
        let input_estimate = ctx.input_tokens.unwrap_or_else(|| {
            let chars: usize = req
                .messages
                .iter()
                .filter_map(|m| m.content.as_ref())
                .map(|c| c.as_text().len())
                .sum();
            (chars as u64).div_ceil(4)
        });
        let output_cap = req
            .max_tokens
            .map(u64::from)
            .or(ctx.output_reserve)
            .unwrap_or(shadow.default_output_reserve);
        let worst_tokens = input_estimate.saturating_add(output_cap);
        let worst_effective_usd = shadow
            .cost
            .as_ref()
            .and_then(|f| f(&candidate.source, &candidate.model, worst_tokens));
        let outcome = queue.submit(ShadowJob {
            shadow_id: format!("shx_{}", Ulid::new()),
            primary_decision_id: decision_id,
            task_id: routing.task_id().map(str::to_owned),
            run_id: routing.run_id().map(str::to_owned),
            request_id: Some(request_id.to_string()),
            target: task_core::model_router::shadow::ShadowTarget {
                task_kind: ctx.task_kind.clone().unwrap_or_default(),
                role: ctx.role.clone().unwrap_or_default(),
                lane: lane.to_string(),
                source: candidate.source.clone(),
            },
            candidate_model: candidate.model,
            primary_source: primary.source.clone(),
            primary_resource_group: None,
            candidate_resource_group: None,
            request: req.clone(),
            worst_tokens,
            worst_effective_usd,
        });
        tracing::debug!(?outcome, "llm-proxy: execution shadow submitted");
    }

    /// sidecar estimator の比較を始める（同期で返り、sidecar の完了を待たない）。primary は変えない。
    fn estimator_after_primary(
        &self,
        routing: &ResolvedRouting,
        request_id: &str,
        lane: &str,
        candidates: &[ShadowCandidate],
        primary: &ShadowCandidate,
        req: &ChatCompletionRequest,
    ) {
        let Some(estimator) = &self.estimator_shadow else {
            return;
        };
        // estimator の比較は lane の policy で回す（明示 model の要求は対象外）。
        let Ok(tier) = serde_json::from_value::<task_core::Tier>(serde_json::json!(lane)) else {
            return;
        };
        let ctx = &routing.context;
        let target = task_core::model_router::shadow::ShadowTarget {
            task_kind: ctx.task_kind.clone().unwrap_or_default(),
            role: ctx.role.clone().unwrap_or_default(),
            lane: lane.to_string(),
            source: primary.source.clone(),
        };
        let prompt = estimator.prompt_allowed(&target).then(|| {
            req.messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .and_then(|m| m.content.as_ref())
                .map(|c| c.as_text())
                .unwrap_or_default()
        });
        let outcome = estimator.submit(EstimatorShadowInput {
            primary_decision_id: routing.decision_id.clone(),
            task_id: routing.task_id().map(str::to_owned),
            run_id: routing.run_id().map(str::to_owned),
            request_id: Some(request_id.to_string()),
            lane: tier,
            target,
            context: ctx.clone(),
            candidates: candidates.to_vec(),
            primary: primary.clone(),
            prompt,
        });
        tracing::debug!(?outcome, "llm-proxy: estimator shadow submitted");
    }

    /// primary が候補待ち（予約待ち）に入った: 未開始の実行 shadow を落とす。
    fn shadow_primary_pressure(&self) {
        if let Some(queue) = self.shadow.as_ref().and_then(|s| s.execution.as_ref()) {
            queue.primary_pressure();
        }
    }

    /// `context_ref` の registry と routing event の sink を差し込む（作った直後、まだ共有していない
    /// `Arc` にだけ効く。[`Self::with_fallback`] と同じ流儀）。
    pub fn with_routing_context(
        mut self: Arc<Self>,
        registry: Option<Arc<dyn RoutingContextRegistry>>,
        sink: Option<Arc<dyn ProxyEventSink>>,
    ) -> Arc<Self> {
        match Arc::get_mut(&mut self) {
            Some(state) => {
                state.routing_registry = registry;
                state.event_sink = sink;
            }
            None => tracing::warn!("llm-proxy: routing context ignored (state already shared)"),
        }
        self
    }

    /// run ごとの in-flight 要求数（daemon は run 終了後、これが 0 になってから ref を外す）。
    pub fn in_flight_contexts(&self) -> &Arc<InFlightContexts> {
        &self.in_flight
    }

    /// fallback の設定と時計を差し替える（作った直後、まだ共有していない `Arc` にだけ効く）。
    /// breaker の状態はここで作り直す。
    pub fn with_fallback(
        mut self: Arc<Self>,
        settings: FallbackSettings,
        clock: Arc<dyn Clock>,
    ) -> Arc<Self> {
        match Arc::get_mut(&mut self) {
            Some(state) => {
                state.breakers = Breakers::new(settings.breaker.clone());
                state.fallback = settings;
                state.clock = clock;
            }
            None => tracing::warn!("llm-proxy: fallback settings ignored (state already shared)"),
        }
        self
    }

    /// deployment の breaker（表示・試験用）。
    pub fn breakers(&self) -> &Arc<Breakers> {
        &self.breakers
    }

    fn in_use_count(&self, key: &str) -> usize {
        self.in_use
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .copied()
            .unwrap_or(0)
    }

    fn in_use_start(&self, key: &str) {
        let mut guard = self.in_use.lock().unwrap_or_else(|e| e.into_inner());
        *guard.entry(key.to_string()).or_insert(0) += 1;
    }

    fn in_use_end(&self, key: &str) {
        let mut guard = self.in_use.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = guard.get_mut(key) {
            *v = v.saturating_sub(1);
        }
    }

    /// `GET <base_url>/models` の到達性を probe し、`probe_cache_secs` の間はキャッシュする。
    pub(crate) async fn reachable(&self, cfg: &OpenAiCompatibleConfig) -> bool {
        self.probe_relay(cfg).await.is_ok()
    }

    /// [`Self::reachable`] と同じだが、届かないときの理由も返す。キャッシュは成功・失敗とも
    /// `probe_cache_secs` で切れる（一度落ちた供給元も、寿命が過ぎれば次の要求・表示で probe し直す）。
    pub(crate) async fn probe_relay(&self, cfg: &OpenAiCompatibleConfig) -> Result<(), String> {
        let ttl = Duration::from_secs(self.config.probe_cache_secs);
        if let Some((at, result)) = self
            .probe_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&cfg.id)
            .cloned()
            && at.elapsed() < ttl
        {
            return result;
        }
        let result = relay::probe(&self.client, cfg).await;
        if let Err(reason) = &result {
            tracing::debug!(source = %cfg.id, reason, "llm-proxy: relay probe failed");
        }
        self.probe_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(cfg.id.clone(), (Instant::now(), result.clone()));
        result
    }

    fn claude_dirs(&self) -> Vec<AccountDir> {
        self.config
            .sources
            .claude_oauth
            .as_ref()
            .filter(|c| c.enabled)
            .map(|c| scan_accounts(&c.accounts_dir, AccountAdapter::ClaudeCode))
            .unwrap_or_default()
    }

    fn codex_dirs(&self) -> Vec<AccountDir> {
        self.config
            .sources
            .codex_oauth
            .as_ref()
            .filter(|c| c.enabled)
            .map(|c| scan_accounts(&c.accounts_dir, AccountAdapter::Codex))
            .unwrap_or_default()
    }

    fn rank_claude(&self, dirs: &[AccountDir], now: i64) -> Vec<SelectedAccount> {
        let Some(book) = &self.claude_book else {
            return vec![];
        };
        let guard = book.lock().unwrap_or_else(|e| e.into_inner());
        let input = PoolInput {
            dirs,
            book: &guard,
            in_use: &|id| self.in_use_count(&format!("claude-oauth:{id}")),
            max_concurrent_per_account: self.config.max_concurrent_per_account,
        };
        selection::rank_pool(SourceKind::Claude, &input, now)
    }

    fn rank_codex(&self, dirs: &[AccountDir], now: i64) -> Vec<SelectedAccount> {
        let Some(book) = &self.codex_book else {
            return vec![];
        };
        let guard = book.lock().unwrap_or_else(|e| e.into_inner());
        let input = PoolInput {
            dirs,
            book: &guard,
            in_use: &|id| self.in_use_count(&format!("codex-oauth:{id}")),
            max_concurrent_per_account: self.config.max_concurrent_per_account,
        };
        selection::rank_pool(SourceKind::Gpt, &input, now)
    }

    fn rank_cross(
        &self,
        claude_dirs: &[AccountDir],
        codex_dirs: &[AccountDir],
        now: i64,
    ) -> Vec<SelectedAccount> {
        let claude_guard = self
            .claude_book
            .as_ref()
            .map(|b| b.lock().unwrap_or_else(|e| e.into_inner()));
        let codex_guard = self
            .codex_book
            .as_ref()
            .map(|b| b.lock().unwrap_or_else(|e| e.into_inner()));
        let claude_in_use = |id: &str| self.in_use_count(&format!("claude-oauth:{id}"));
        let codex_in_use = |id: &str| self.in_use_count(&format!("codex-oauth:{id}"));
        let claude_input = claude_guard.as_ref().map(|g| PoolInput {
            dirs: claude_dirs,
            book: g,
            in_use: &claude_in_use,
            max_concurrent_per_account: self.config.max_concurrent_per_account,
        });
        let codex_input = codex_guard.as_ref().map(|g| PoolInput {
            dirs: codex_dirs,
            book: g,
            in_use: &codex_in_use,
            max_concurrent_per_account: self.config.max_concurrent_per_account,
        });
        selection::rank_across_pools(claude_input.as_ref(), codex_input.as_ref(), now)
    }

    async fn rank_relays(&self, sources: &[OpenAiCompatibleConfig]) -> Vec<OpenAiCompatibleConfig> {
        let mut reachability = HashMap::new();
        for s in sources {
            reachability.insert(s.id.clone(), self.reachable(s).await);
        }
        selection::rank_relays(sources, |id| reachability.get(id).copied().unwrap_or(false))
            .into_iter()
            .cloned()
            .collect()
    }

    /// 429/401 を受けたアカウントに cooldown を付ける（既存の観測と同じ表。ADR-0053 D1）。
    fn record_failure(&self, source: SourceKind, account_id: &str, error: &SourceError, now: i64) {
        let reason = match error {
            SourceError::Unauthorized => AccountCooldownReason::AuthFailed,
            SourceError::RateLimited { .. } => AccountCooldownReason::Throttled,
            _ => return,
        };
        let book = match source {
            SourceKind::Claude => self.claude_book.as_ref(),
            SourceKind::Gpt => self.codex_book.as_ref(),
            SourceKind::Qwen => None,
        };
        let Some(book) = book else { return };
        let mut guard = book.lock().unwrap_or_else(|e| e.into_inner());
        let cooldown = cooldown_for_failure(
            guard.state(account_id),
            reason,
            now,
            self.config.cooldown_fallback_secs,
        );
        guard.set_cooldown(account_id, cooldown, now);
        if let Err(e) = guard.save() {
            tracing::warn!(error = %e, "llm-proxy: could not persist the account cooldown");
        }
    }
}

// ---------------------------------------------------------------------------
// 候補の組み立て
// ---------------------------------------------------------------------------

enum Attempt {
    Claude(SelectedAccount),
    Codex(SelectedAccount),
    Relay(OpenAiCompatibleConfig),
}

impl Attempt {
    fn source_label(&self) -> String {
        match self {
            Attempt::Claude(_) => "claude-oauth".to_string(),
            Attempt::Codex(_) => "codex-oauth".to_string(),
            Attempt::Relay(cfg) => format!("openai-compatible:{}", cfg.id),
        }
    }
    fn account_label(&self) -> Option<String> {
        match self {
            Attempt::Claude(a) | Attempt::Codex(a) => Some(a.account_id.clone()),
            Attempt::Relay(_) => None,
        }
    }
    fn source_kind(&self) -> SourceKind {
        match self {
            Attempt::Claude(_) => SourceKind::Claude,
            Attempt::Codex(_) => SourceKind::Gpt,
            Attempt::Relay(_) => SourceKind::Qwen,
        }
    }
}

impl ProxyState {
    async fn attempts_for(&self, parsed: &ModelRequest, now: i64) -> Vec<(Attempt, String)> {
        match parsed {
            ModelRequest::Explicit { source, model } => match source {
                SourceKind::Claude => self
                    .rank_claude(&self.claude_dirs(), now)
                    .into_iter()
                    .map(|a| (Attempt::Claude(a), model.clone()))
                    .collect(),
                SourceKind::Gpt => self
                    .rank_codex(&self.codex_dirs(), now)
                    .into_iter()
                    .map(|a| (Attempt::Codex(a), model.clone()))
                    .collect(),
                SourceKind::Qwen => self
                    .rank_relays(&self.config.sources.openai_compatible)
                    .await
                    .into_iter()
                    .map(|cfg| (Attempt::Relay(cfg), model.clone()))
                    .collect(),
            },
            ModelRequest::Tiered { scope, tier } => {
                let catalog = crate::legacy_catalog::normalize_legacy_config(&self.config);
                let deployments =
                    selection::legacy_deployments(&catalog, *scope, *tier, self.config.prefer_free);
                let model_for = |source: &str| -> Option<String> {
                    deployments
                        .iter()
                        .find(|d| d.source_ref == source)
                        .map(|d| d.upstream_model.clone())
                };
                match scope {
                    SourceScope::Only(SourceKind::Claude) => {
                        let Some(model) = model_for("claude-oauth") else {
                            return vec![];
                        };
                        self.rank_claude(&self.claude_dirs(), now)
                            .into_iter()
                            .map(|a| (Attempt::Claude(a), model.clone()))
                            .collect()
                    }
                    SourceScope::Only(SourceKind::Gpt) => {
                        let Some(model) = model_for("codex-oauth") else {
                            return vec![];
                        };
                        self.rank_codex(&self.codex_dirs(), now)
                            .into_iter()
                            .map(|a| (Attempt::Codex(a), model.clone()))
                            .collect()
                    }
                    SourceScope::Only(SourceKind::Qwen) => {
                        if deployments.is_empty() {
                            return vec![];
                        }
                        self.rank_relays(&self.config.sources.openai_compatible)
                            .await
                            .into_iter()
                            .filter_map(|cfg| {
                                model_for(&format!("openai-compatible:{}", cfg.id))
                                    .map(|model| (Attempt::Relay(cfg), model))
                            })
                            .collect()
                    }
                    SourceScope::Any => {
                        let mut attempts = Vec::new();
                        if deployments
                            .iter()
                            .any(|d| d.source_ref.starts_with("openai-compatible:"))
                        {
                            let relays = self
                                .rank_relays(&self.config.sources.openai_compatible)
                                .await;
                            attempts.extend(relays.into_iter().filter_map(|cfg| {
                                model_for(&format!("openai-compatible:{}", cfg.id))
                                    .map(|model| (Attempt::Relay(cfg), model))
                            }));
                        }
                        let claude_dirs = self.claude_dirs();
                        let codex_dirs = self.codex_dirs();
                        attempts.extend(
                            self.rank_cross(&claude_dirs, &codex_dirs, now)
                                .into_iter()
                                .filter_map(|a| match a.source {
                                    SourceKind::Claude => {
                                        model_for("claude-oauth").map(|m| (Attempt::Claude(a), m))
                                    }
                                    SourceKind::Gpt => {
                                        model_for("codex-oauth").map(|m| (Attempt::Codex(a), m))
                                    }
                                    SourceKind::Qwen => None,
                                }),
                        );
                        attempts
                    }
                }
            }
        }
    }

    /// ADR-0053 D4（Phase 66）: `celeris/<tier>` が今どこに解決するか（表示専用。実際の要求は送らない。
    /// `attempts_for` と同じ決定的な選択をなぞるだけ）。候補が無ければ `None`。
    pub(crate) async fn resolves_tier(&self, tier: task_core::Tier, now: i64) -> Option<String> {
        self.attempts_for(
            &ModelRequest::Tiered {
                scope: SourceScope::Any,
                tier,
            },
            now,
        )
        .await
        .into_iter()
        .next()
        .map(|(attempt, _)| attempt.source_label())
    }
}

// ---------------------------------------------------------------------------
// 記録
// ---------------------------------------------------------------------------

/// 要求 1 件の記録に共通するもの（id・時刻・routing の相関・試した source・in-flight の数え）。
/// 終わり方ごとに [`RequestScope::log`] で [`LogHandle`] に変えて 1 回だけ書く。
struct RequestScope {
    id: String,
    ts: i64,
    requested_model: String,
    started: Instant,
    db_path: Option<PathBuf>,
    busy_timeout: Duration,
    routing: ResolvedRouting,
    attempts: Vec<RequestSourceAttempt>,
    sink: Option<Arc<dyn ProxyEventSink>>,
    /// 要求の間だけ持つ（drop で in-flight の数が戻る）。
    _in_flight: Option<InFlightGuard>,
}

impl RequestScope {
    fn log(
        self,
        source: Option<String>,
        account: Option<String>,
        upstream_model: Option<String>,
    ) -> LogHandle {
        LogHandle {
            source,
            account,
            upstream_model,
            scope: self,
        }
    }
}

struct LogHandle {
    source: Option<String>,
    account: Option<String>,
    upstream_model: Option<String>,
    scope: RequestScope,
}

impl LogHandle {
    fn write(
        &self,
        status: &'static str,
        usage: (Option<u64>, Option<u64>),
        error_kind: Option<String>,
    ) {
        let scope = &self.scope;
        if let (Some(sink), Some(event)) = (
            &scope.sink,
            rctx::build_event(&scope.routing, &scope.id, &scope.attempts),
        ) {
            sink.record(event);
        }
        let Some(db_path) = &scope.db_path else {
            return;
        };
        let row = RequestLogRow {
            id: scope.id.clone(),
            ts: scope.ts,
            source: self.source.clone(),
            account: self.account.clone(),
            requested_model: scope.requested_model.clone(),
            upstream_model: self.upstream_model.clone(),
            prompt_tokens: usage.0,
            completion_tokens: usage.1,
            latency_ms: scope.started.elapsed().as_millis() as u64,
            status,
            error_kind,
        };
        let conn = match log::open(db_path, scope.busy_timeout) {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!(error = %e, "llm-proxy: could not open the request log db");
                return;
            }
        };
        // daemon 登録の context を持つ要求は 0048 の相関欄（run 側 decision・run・task・実 source・model）
        // も書く。standalone は従来の欄だけ。
        let result = if scope.routing.registered {
            let correlation = RoutingCorrelation {
                decision_id: scope.routing.parent_decision_id.clone(),
                snapshot_id: None,
                run_id: scope.routing.run_id().map(str::to_owned),
                task_id: scope.routing.task_id().map(str::to_owned),
                source_id: self.source.clone(),
                model: self.upstream_model.clone(),
                account: None,
            };
            log::insert_routed(&conn, &row, &correlation)
        } else {
            log::insert(&conn, &row)
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, "llm-proxy: could not record the request log row");
        }
        // in-flight の数えはここで戻る（`self` の drop で guard も落ちる）。
    }
}

/// 次の候補へ倒した理由の reason code（ADR §6 の安定値に寄せる）。
fn fallback_reason(class: FailureClass) -> &'static str {
    match class {
        FailureClass::Unauthorized => "auth_failed",
        FailureClass::RateLimited => "rate_limit",
        FailureClass::Server | FailureClass::Network => "health_down",
        FailureClass::Local => "local",
        FailureClass::Client => "client",
    }
}

fn error_kind(e: &SourceError) -> &'static str {
    match e {
        SourceError::Unauthorized => "unauthorized",
        SourceError::RateLimited { .. } => "rate_limited",
        SourceError::Upstream { .. } => "upstream",
        SourceError::Network(_) => "network",
        SourceError::Credentials(_) => "credentials",
        SourceError::Unavailable(_) => "unavailable",
    }
}

fn usage_from_value(v: &Value) -> (Option<u64>, Option<u64>) {
    let usage = v.get("usage");
    let prompt = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|x| x.as_u64());
    let completion = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|x| x.as_u64());
    (prompt, completion)
}

// ---------------------------------------------------------------------------
// 送信 1 回
// ---------------------------------------------------------------------------

enum AttemptOutcome {
    NonStreamJson(Value),
    Stream(BoxStream<'static, Result<Bytes, SourceError>>),
}

async fn run_attempt(
    state: &ProxyState,
    attempt: &Attempt,
    upstream_model: &str,
    req: &ChatCompletionRequest,
) -> Result<AttemptOutcome, SourceError> {
    match attempt {
        Attempt::Claude(a) => {
            let cfg = state
                .config
                .sources
                .claude_oauth
                .as_ref()
                .ok_or_else(|| SourceError::Unavailable("claude-oauth".to_string()))?;
            match claude::send(&state.client, cfg, &a.dir, req, upstream_model).await? {
                SendOutcome::NonStream(resp) => {
                    let mut value = serde_json::to_value(resp).unwrap_or(Value::Null);
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert("model".to_string(), json!(req.model));
                    }
                    Ok(AttemptOutcome::NonStreamJson(value))
                }
                SendOutcome::Stream(s) => Ok(AttemptOutcome::Stream(encode_chunk_stream(
                    s,
                    req.model.clone(),
                ))),
            }
        }
        Attempt::Codex(a) => {
            let cfg = state
                .config
                .sources
                .codex_oauth
                .as_ref()
                .ok_or_else(|| SourceError::Unavailable("codex-oauth".to_string()))?;
            match codex::send(&state.client, cfg, &a.dir, req, upstream_model).await? {
                SendOutcome::NonStream(resp) => {
                    let mut value = serde_json::to_value(resp).unwrap_or(Value::Null);
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert("model".to_string(), json!(req.model));
                    }
                    Ok(AttemptOutcome::NonStreamJson(value))
                }
                SendOutcome::Stream(s) => Ok(AttemptOutcome::Stream(encode_chunk_stream(
                    s,
                    req.model.clone(),
                ))),
            }
        }
        Attempt::Relay(cfg) => {
            let mut body = serde_json::to_value(req).unwrap_or(Value::Null);
            if let Some(obj) = body.as_object_mut() {
                obj.insert("model".to_string(), json!(upstream_model));
            }
            let raw = relay::send_raw(&state.client, cfg, &body).await?;
            classify_relay_status(raw.status, raw.retry_after)?;
            if req.stream {
                let requested_model = req.model.clone();
                let byte_stream = raw.body.bytes_stream().map(move |r| {
                    r.map_err(|e| SourceError::Network(crate::neterr::safe_reqwest_error(&e)))
                });
                Ok(AttemptOutcome::Stream(rewrite_relay_stream_model(
                    byte_stream,
                    requested_model,
                )))
            } else {
                let mut value: Value = raw
                    .body
                    .json()
                    .await
                    .map_err(|e| SourceError::Network(crate::neterr::safe_reqwest_error(&e)))?;
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("model".to_string(), json!(req.model));
                }
                Ok(AttemptOutcome::NonStreamJson(value))
            }
        }
    }
}

fn classify_relay_status(status: u16, retry_after: Option<u64>) -> Result<(), SourceError> {
    if status == 401 {
        return Err(SourceError::Unauthorized);
    }
    if status == 429 {
        return Err(SourceError::RateLimited { retry_after });
    }
    if !(200..300).contains(&status) {
        return Err(SourceError::Upstream {
            status,
            summary: "relay upstream error".to_string(),
        });
    }
    Ok(())
}

/// Claude/Codex の chunk stream を OpenAI SSE の bytes へ写す。末尾に `data: [DONE]` を付ける。
fn encode_chunk_stream(
    inner: BoxStream<'static, Result<crate::openai::ChatCompletionChunk, SourceError>>,
    requested_model: String,
) -> BoxStream<'static, Result<Bytes, SourceError>> {
    let mut inner = inner;
    let mut done = false;
    Box::pin(futures_util::stream::poll_fn(move |cx| {
        if done {
            return std::task::Poll::Ready(None);
        }
        match inner.as_mut().poll_next(cx) {
            std::task::Poll::Ready(Some(Ok(mut chunk))) => {
                chunk.model = requested_model.clone();
                let json = serde_json::to_string(&chunk).unwrap_or_else(|_| "{}".to_string());
                std::task::Poll::Ready(Some(Ok(Bytes::from(crate::sse::encode_data(&json)))))
            }
            std::task::Poll::Ready(Some(Err(e))) => std::task::Poll::Ready(Some(Err(e))),
            std::task::Poll::Ready(None) => {
                done = true;
                std::task::Poll::Ready(Some(Ok(Bytes::from(crate::sse::DONE))))
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }))
}

/// relay（openai-compatible）の SSE をそのまま中継しつつ、各イベントの `model` だけ書き換える。
/// パースできない行（`[DONE]` 等）はそのまま通す。
fn rewrite_relay_stream_model(
    inner: impl Stream<Item = Result<Bytes, SourceError>> + Send + 'static,
    requested_model: String,
) -> BoxStream<'static, Result<Bytes, SourceError>> {
    let mut inner = Box::pin(inner);
    let mut decoder = crate::sse::SseDecoder::new();
    let mut pending: std::collections::VecDeque<Bytes> = std::collections::VecDeque::new();
    Box::pin(futures_util::stream::poll_fn(move |cx| {
        loop {
            if let Some(b) = pending.pop_front() {
                return std::task::Poll::Ready(Some(Ok(b)));
            }
            match inner.as_mut().poll_next(cx) {
                std::task::Poll::Ready(Some(Ok(bytes))) => {
                    for ev in decoder.push(&bytes) {
                        if ev.data.trim() == "[DONE]" {
                            pending.push_back(Bytes::from(crate::sse::DONE));
                            continue;
                        }
                        let rewritten = match serde_json::from_str::<Value>(&ev.data) {
                            Ok(mut v) => {
                                if let Some(obj) = v.as_object_mut() {
                                    obj.insert("model".to_string(), json!(requested_model));
                                }
                                serde_json::to_string(&v).unwrap_or(ev.data.clone())
                            }
                            Err(_) => ev.data.clone(),
                        };
                        pending.push_back(Bytes::from(crate::sse::encode_data(&rewritten)));
                    }
                    continue;
                }
                std::task::Poll::Ready(Some(Err(e))) => {
                    return std::task::Poll::Ready(Some(Err(e)));
                }
                std::task::Poll::Ready(None) => return std::task::Poll::Ready(None),
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }))
}

/// 送信済みバイトの後は切断するだけ（やり直さない。ADR-0053 D1）。ログはここで確定させる。
fn finalize_stream(
    inner: BoxStream<'static, Result<Bytes, SourceError>>,
    log: LogHandle,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    let mut inner = inner;
    let mut logged = false;
    let mut log = Some(log);
    futures_util::stream::poll_fn(move |cx| match inner.as_mut().poll_next(cx) {
        std::task::Poll::Ready(Some(Ok(b))) => std::task::Poll::Ready(Some(Ok(b))),
        std::task::Poll::Ready(Some(Err(e))) => {
            if !logged {
                logged = true;
                if let Some(log) = log.take() {
                    log.write("error", (None, None), Some(error_kind(&e).to_string()));
                }
            }
            tracing::warn!(
                kind = error_kind(&e),
                "llm-proxy: stream terminated after bytes were sent"
            );
            std::task::Poll::Ready(None)
        }
        std::task::Poll::Ready(None) => {
            if !logged {
                logged = true;
                if let Some(log) = log.take() {
                    log.write("ok", (None, None), None);
                }
            }
            std::task::Poll::Ready(None)
        }
        std::task::Poll::Pending => std::task::Poll::Pending,
    })
}

// ---------------------------------------------------------------------------
// ハンドラ
// ---------------------------------------------------------------------------

fn problem(status: StatusCode, code: &str, detail: &str) -> Response {
    (
        status,
        Json(json!({"error": {"type": code, "message": detail}})),
    )
        .into_response()
}

async fn healthz() -> Response {
    (StatusCode::OK, Json(json!({"status": "ok"}))).into_response()
}

async fn embeddings_not_implemented() -> Response {
    problem(
        StatusCode::NOT_IMPLEMENTED,
        "not_implemented",
        "POST /v1/embeddings is not implemented by this proxy",
    )
}

async fn list_models(State(state): State<Arc<ProxyState>>) -> Response {
    let created = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut data = Vec::new();
    for tier in ["frontier", "standard", "cheap"] {
        data.push(crate::openai::ModelInfo {
            id: format!("celeris/{tier}"),
            object: "model".to_string(),
            created,
            owned_by: "celeris".to_string(),
        });
    }
    if state
        .config
        .sources
        .claude_oauth
        .as_ref()
        .is_some_and(|c| c.enabled)
    {
        for tier in ["frontier", "standard", "cheap"] {
            data.push(crate::openai::ModelInfo {
                id: format!("claude/{tier}"),
                object: "model".to_string(),
                created,
                owned_by: "claude-oauth".to_string(),
            });
        }
    }
    if state
        .config
        .sources
        .codex_oauth
        .as_ref()
        .is_some_and(|c| c.enabled)
    {
        for tier in ["frontier", "standard", "cheap"] {
            data.push(crate::openai::ModelInfo {
                id: format!("gpt/{tier}"),
                object: "model".to_string(),
                created,
                owned_by: "codex-oauth".to_string(),
            });
        }
    }
    if state
        .config
        .sources
        .openai_compatible
        .iter()
        .any(|c| c.enabled)
        && selection::qwen_tier_model(&state.config.models.qwen, task_core::Tier::Cheap).is_some()
    {
        data.push(crate::openai::ModelInfo {
            id: "qwen/cheap".to_string(),
            object: "model".to_string(),
            created,
            owned_by: "openai-compatible".to_string(),
        });
    }
    (
        StatusCode::OK,
        Json(crate::openai::ModelsResponse {
            object: "list".to_string(),
            data,
        }),
    )
        .into_response()
}

fn attach_source_headers(headers: &mut HeaderMap, source: &str, account: Option<&str>) {
    if let Ok(v) = HeaderValue::from_str(source) {
        headers.insert(header::HeaderName::from_static(X_SOURCE), v);
    }
    if let Some(account) = account
        && let Ok(v) = HeaderValue::from_str(account)
    {
        headers.insert(header::HeaderName::from_static(X_ACCOUNT), v);
    }
}

fn error_response(e: &SourceError) -> Response {
    match e {
        SourceError::Unauthorized => problem(
            StatusCode::BAD_GATEWAY,
            "unauthorized",
            "upstream rejected the credentials",
        ),
        SourceError::RateLimited { retry_after } => {
            let mut resp = problem(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "upstream rate-limited the request",
            );
            if let Some(secs) = retry_after
                && let Ok(v) = HeaderValue::from_str(&secs.to_string())
            {
                resp.headers_mut().insert(header::RETRY_AFTER, v);
            }
            resp
        }
        SourceError::Upstream { status, summary } => {
            let code = StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY);
            let code = if code.is_client_error() || code.is_server_error() {
                code
            } else {
                StatusCode::BAD_GATEWAY
            };
            problem(code, "upstream_error", summary)
        }
        SourceError::Network(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "network_error",
            "could not reach the upstream",
        ),
        SourceError::Credentials(msg) => {
            problem(StatusCode::INTERNAL_SERVER_ERROR, "credentials_error", msg)
        }
        SourceError::Unavailable(id) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "source_unavailable",
            &format!("source unavailable: {id}"),
        ),
    }
}

async fn chat_completions(
    State(state): State<Arc<ProxyState>>,
    headers: HeaderMap,
    Json(req): Json<ChatCompletionRequest>,
) -> Response {
    let started = Instant::now();
    let request_id = Ulid::new().to_string();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();

    // ADR 2026-10-04 §10 Phase 3: 文脈は daemon 登録の ref からだけ取る（本文の自己申告は採らない）。
    // header は以後どこにも渡さない（上流への要求は `req` から組み直す）。
    let (context, registered) =
        match rctx::resolve(&headers, state.routing_registry.as_deref(), &req, started) {
            Ok(resolved) => resolved,
            Err(e) => {
                RequestScope {
                    id: request_id,
                    ts: now,
                    requested_model: req.model.clone(),
                    started,
                    db_path: state.db_path.clone(),
                    busy_timeout: state.busy_timeout,
                    routing: ResolvedRouting {
                        context: task_core::model_router::context::standalone_context(
                            "llm-proxy:rejected",
                        ),
                        registered: false,
                        decision_id: format!("pdec_{}", Ulid::new()),
                        parent_decision_id: None,
                    },
                    attempts: Vec::new(),
                    sink: None,
                    _in_flight: None,
                }
                .log(None, None, None)
                .write("error", (None, None), Some(e.code().to_string()));
                return problem(
                    StatusCode::BAD_REQUEST,
                    e.code(),
                    "the x-celeris-routing-context reference is not registered or has expired",
                );
            }
        };
    let parent_decision_id = match (&state.event_sink, context.run_id.as_deref()) {
        (Some(sink), Some(run_id)) if registered => sink.parent_decision(run_id),
        _ => None,
    };
    let in_flight = match (registered, context.run_id.as_deref()) {
        (true, Some(run_id)) => Some(state.in_flight.enter(run_id)),
        _ => None,
    };
    let mut scope = RequestScope {
        id: request_id,
        ts: now,
        requested_model: req.model.clone(),
        started,
        db_path: state.db_path.clone(),
        busy_timeout: state.busy_timeout,
        routing: ResolvedRouting {
            context,
            registered,
            decision_id: format!("pdec_{}", Ulid::new()),
            parent_decision_id,
        },
        attempts: Vec::new(),
        sink: state.event_sink.clone(),
        _in_flight: in_flight,
    };

    let parsed = match naming::parse_model(&req.model) {
        Ok(p) => p,
        Err(e) => {
            scope.log(None, None, None).write(
                "error",
                (None, None),
                Some("invalid_model".to_string()),
            );
            return problem(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_model",
                &e.to_string(),
            );
        }
    };

    let mut attempts = state.attempts_for(&parsed, now).await;
    // ADR-0063 Phase 109b C1: 起動直後・アカウントのローテーションの一瞬など、候補が一時的に全部
    // 無くなる瞬間がある（実機の観測、2026-09-23: claude-oauth が 429/cooldown で codex に倒れる
    // 過程で一瞬すべての候補が無くなり `no_source_available` を返した）。即 503 を返す前に短い待ち
    // を挟んで候補列をもう 1 周する（最大 2 周、合計 3 秒以内）。
    let mut rescans = 0u32;
    if attempts.is_empty() {
        state.shadow_primary_pressure();
    }
    while attempts.is_empty() && rescans < NO_SOURCE_MAX_RESCANS {
        tokio::time::sleep(NO_SOURCE_RESCAN_DELAY).await;
        rescans += 1;
        let rescan_now = time::OffsetDateTime::now_utc().unix_timestamp();
        attempts = state.attempts_for(&parsed, rescan_now).await;
    }
    if attempts.is_empty() {
        scope.log(None, None, None).write(
            "unavailable",
            (None, None),
            Some("no_source_available".to_string()),
        );
        let mut resp = problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_source_available",
            "no reachable llm source is configured for this model",
        );
        if let Ok(v) = HeaderValue::from_str("5") {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
        return resp;
    }

    // ADR 2026-10-04 §5: 同一要求内 fallback。要求の制約に合う候補へだけ倒し、stream は最初の
    // byte を受け取る前だけ次の候補へ送る。分類別・総数の上限と deadline は budget が、deployment
    // の closed/open/half_open は breaker が決める（時刻は注入した時計）。
    let constraints = RequestConstraints::from_request(&parsed);
    let shadow_lane = match &parsed {
        ModelRequest::Tiered { tier, .. } => serde_json::to_value(tier)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        ModelRequest::Explicit { .. } => "explicit".to_string(),
    };
    let shadow_candidates: Vec<ShadowCandidate> = if state.any_shadow() {
        attempts
            .iter()
            .filter(|(a, _)| constraints.admits(a.source_kind()))
            .map(|(a, m)| ShadowCandidate {
                source: a.source_label(),
                model: m.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut budget = FallbackBudget::new(&state.fallback, state.clock.now());
    let mut breaker_skipped: Vec<String> = Vec::new();
    let mut last_error = None;
    for (attempt, upstream_model) in &attempts {
        let source_label = attempt.source_label();
        let account_label = attempt.account_label();
        if !constraints.admits(attempt.source_kind()) {
            tracing::debug!(source = %source_label, "llm-proxy: candidate outside the request constraints; skipped");
            continue;
        }
        let deployment = format!("{source_label}/{upstream_model}");
        let Some(permit) = state.breakers.admit(&deployment, state.clock.now()) else {
            tracing::debug!(%deployment, "llm-proxy: deployment breaker is open (or probing); skipped");
            breaker_skipped.push(deployment);
            continue;
        };
        if !budget.begin_attempt() {
            drop(permit);
            break;
        }
        let in_use_key = account_label
            .as_ref()
            .map(|a| format!("{source_label}:{a}"));
        if let Some(k) = &in_use_key {
            state.in_use_start(k);
        }
        scope.attempts.push(RequestSourceAttempt {
            source_id: source_label.clone(),
            model: Some(upstream_model.clone()),
            account_id: account_label.clone(),
            fallback_reason: None,
        });
        let result = match run_attempt(&state, attempt, upstream_model, &req).await {
            // 最初の byte が来るまでは失敗を「送る前」と同じに扱う（まだ caller へ何も返していない）。
            Ok(AttemptOutcome::Stream(mut stream)) => match stream.next().await {
                Some(Ok(first)) => Ok(AttemptOutcome::Stream(Box::pin(
                    futures_util::stream::once(async move { Ok(first) }).chain(stream),
                ))),
                Some(Err(e)) => Err(e),
                None => Ok(AttemptOutcome::Stream(stream)),
            },
            other => other,
        };
        if let Some(k) = &in_use_key {
            state.in_use_end(k);
        }
        match result {
            Ok(AttemptOutcome::NonStreamJson(value)) => {
                permit.success(state.clock.now());
                state.shadow_after_primary(
                    &scope.routing,
                    &scope.id,
                    &shadow_lane,
                    &shadow_candidates,
                    &ShadowCandidate {
                        source: source_label.clone(),
                        model: upstream_model.clone(),
                    },
                    &req,
                );
                let usage = usage_from_value(&value);
                scope
                    .log(
                        Some(source_label.clone()),
                        account_label.clone(),
                        Some(upstream_model.clone()),
                    )
                    .write("ok", usage, None);
                let mut resp = (StatusCode::OK, Json(value)).into_response();
                attach_source_headers(resp.headers_mut(), &source_label, account_label.as_deref());
                return resp;
            }
            Ok(AttemptOutcome::Stream(stream)) => {
                // 最初の byte を受け取った。以後の失敗は caller に返す（再送しない）。
                permit.success(state.clock.now());
                state.shadow_after_primary(
                    &scope.routing,
                    &scope.id,
                    &shadow_lane,
                    &shadow_candidates,
                    &ShadowCandidate {
                        source: source_label.clone(),
                        model: upstream_model.clone(),
                    },
                    &req,
                );
                let log = scope.log(
                    Some(source_label.clone()),
                    account_label.clone(),
                    Some(upstream_model.clone()),
                );
                let body = Body::from_stream(finalize_stream(stream, log));
                let mut resp = Response::new(body);
                resp.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/event-stream"),
                );
                resp.headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
                attach_source_headers(resp.headers_mut(), &source_label, account_label.as_deref());
                return resp;
            }
            Err(e) => {
                let class = FailureClass::of(&e);
                let failed_at = state.clock.now();
                permit.failed_with(class, failed_at);
                if let (Some(account), true) = (
                    &account_label,
                    matches!(
                        e,
                        SourceError::Unauthorized | SourceError::RateLimited { .. }
                    ),
                ) {
                    state.record_failure(attempt.source_kind(), account, &e, now);
                }
                if let Some(last) = scope.attempts.last_mut() {
                    last.fallback_reason = Some(fallback_reason(class).to_string());
                }
                let decision = budget.after_failure(class, failed_at);
                tracing::warn!(source = %source_label, account = ?account_label, kind = error_kind(&e), class = class.as_str(), ?decision, "llm-proxy: candidate failed before any bytes were sent");
                last_error = Some((source_label, account_label, upstream_model.clone(), e));
                if let AfterFailure::Stop(_) = decision {
                    break;
                }
            }
        }
    }

    if last_error.is_none() {
        // 候補は居たが、制約に合わないか breaker が開いていて 1 回も送らなかった。
        scope.log(None, None, None).write(
            "unavailable",
            (None, None),
            Some("no_source_available".to_string()),
        );
        let mut resp = problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_source_available",
            "every candidate for this model is temporarily unavailable",
        );
        let clock_now = state.clock.now();
        let retry_after = state
            .breakers
            .earliest_reopen(&breaker_skipped, clock_now)
            .map(|at| (at - clock_now).whole_seconds().max(1))
            .unwrap_or(5);
        if let Ok(v) = HeaderValue::from_str(&retry_after.to_string()) {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
        return resp;
    }

    let (source_label, account_label, upstream_model, e) = last_error.unwrap_or((
        "none".to_string(),
        None,
        String::new(),
        SourceError::Unavailable("no candidate succeeded".to_string()),
    ));
    // 最後に試した source は次へ倒していない（理由は error_kind に残る）。
    if let Some(last) = scope.attempts.last_mut() {
        last.fallback_reason = None;
    }
    scope
        .log(Some(source_label), account_label, Some(upstream_model))
        .write("error", (None, None), Some(error_kind(&e).to_string()));
    error_response(&e)
}

// ---------------------------------------------------------------------------
// ミドルウェア・ルータ
// ---------------------------------------------------------------------------

async fn guard(State(state): State<Arc<ProxyState>>, req: Request, next: Next) -> Response {
    if req.uri().path() == "/healthz" {
        return next.run(req).await;
    }
    if !state.role.accepts_admin() {
        return problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "standby",
            "this instance is not active",
        );
    }
    if let Some(token) = &state.token {
        let digest = crate::auth::token_digest(token);
        if !crate::auth::check_bearer(req.headers(), &digest) {
            return problem(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "missing or invalid bearer token",
            );
        }
    }
    next.run(req).await
}

pub fn router(state: Arc<ProxyState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/embeddings", post(embeddings_not_implemented))
        .layer(axum::middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// `listen` に bind して動かす（loopback のみを想定。呼び出し側が bind アドレスを検証する）。
pub async fn serve(
    listener: tokio::net::TcpListener,
    state: Arc<ProxyState>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let app = router(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
}
