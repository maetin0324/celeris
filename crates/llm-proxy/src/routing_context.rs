//! ADR 2026-10-04 §10 Phase 3: `x-celeris-routing-context` の参照解決と要求ごとの routing 記録。
//!
//! - header の値は daemon が [`RoutingContextRegistry`] に登録した opaque な ref。proxy はそこから
//!   context を引くだけで、要求本文の org/priority/privacy 等の自己申告は採らない（本文で制約を緩め
//!   られない）。本文から取るのは測れる量（入力長・tools・stream・max_tokens）だけで、登録済みの値より
//!   厳しい側（大きい token 数・`true` の要求能力）にしか動かさない。
//! - 無効・期限切れの ref は 400（既定の context に倒さない。fail-closed）。registry が無い proxy に
//!   header が来たときも、信頼できる登録が無いので無効と同じに扱う。
//! - header 無しは `origin=standalone` の最小 context。
//! - header と内部の run ID は upstream に出さない（上流への要求は `ChatCompletionRequest` から組み直す
//!   ので、受けた header は構造的に転送されない。run ID は log と event sink にだけ渡す）。
//! - 解決した context は要求の間（stream の終わりまで）[`InFlightGuard`] で数え、daemon が run の
//!   終了後も in-flight の要求が終わるまで ref を外さずに済むようにする（[`InFlightContexts::count`]）。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use axum::http::HeaderMap;
use task_core::model_router::context::{RoutingContext, standalone_context};
use task_core::model_router::context_registry::{ContextRefError, RoutingContextRegistry};
use task_core::model_router::feedback::{
    FeatureStage, ROUTING_CONTEXT_VERSION, RequestSourceAttempt, RoutingFeaturesRecord,
    RoutingRequestRecord,
};

use crate::openai::ChatCompletionRequest;

/// 参照を運ぶ header 名（worker の adapter が付ける。upstream には出さない）。
pub const ROUTING_CONTEXT_HEADER: &str = "x-celeris-routing-context";

/// 本文から測った欄の出自。
const REQUEST_FIELDS_PROVENANCE: &str = "llm-proxy:request-fields";

/// task に相関できる要求 1 件（daemon の sink が task event として追記する）。
#[derive(Debug, Clone, PartialEq)]
pub struct ProxyRoutingEvent {
    pub task_id: String,
    /// proxy の更新時点の特徴（stage = proxy）。
    pub features: RoutingFeaturesRecord,
    /// 要求 1 件の trace（run 側 decision を親に持つ）。
    pub request: RoutingRequestRecord,
}

/// proxy の routing 記録の受け口。daemon が配線する。未設定なら proxy は何も渡さない。
/// 実装は request/decision ID で冪等に記録する（ADR §6）。
pub trait ProxyEventSink: Send + Sync {
    /// run 側の decision（dispatch の `routing_decided` の decision_id）。分からなければ `None`
    /// （推定で結び付けない。audit は `audit_incomplete` を見せる）。
    fn parent_decision(&self, _run_id: &str) -> Option<String> {
        None
    }
    /// task に相関できる要求 1 件を受け取る。呼び出し側を待たせない実装にする。
    fn record(&self, event: ProxyRoutingEvent);
}

/// ref の解決に失敗した理由（400 の本文に出す安定値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextHeaderError {
    Invalid,
    Expired,
}

impl ContextHeaderError {
    pub fn code(self) -> &'static str {
        match self {
            ContextHeaderError::Invalid => "invalid_routing_context",
            ContextHeaderError::Expired => "expired_routing_context",
        }
    }
}

/// 要求 1 件で使う context と相関 ID。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRouting {
    pub context: RoutingContext,
    /// daemon の登録から来たか（`false` は standalone）。
    pub registered: bool,
    /// この要求の proxy 側 decision。
    pub decision_id: String,
    /// run 側 decision（sink が知っていれば）。
    pub parent_decision_id: Option<String>,
}

impl ResolvedRouting {
    pub fn run_id(&self) -> Option<&str> {
        self.context.run_id.as_deref()
    }
    pub fn task_id(&self) -> Option<&str> {
        self.context.task_id.as_deref()
    }
}

/// header から context を決める。header 無しは standalone、値があれば registry で解決する。
pub fn resolve(
    headers: &HeaderMap,
    registry: Option<&dyn RoutingContextRegistry>,
    req: &ChatCompletionRequest,
    now: Instant,
) -> Result<(RoutingContext, bool), ContextHeaderError> {
    let Some(raw) = headers.get(ROUTING_CONTEXT_HEADER) else {
        let mut ctx = standalone_context("llm-proxy:standalone");
        apply_request_fields(&mut ctx, req);
        return Ok((ctx, false));
    };
    let reference = raw
        .to_str()
        .map(str::trim)
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or(ContextHeaderError::Invalid)?;
    let registry = registry.ok_or(ContextHeaderError::Invalid)?;
    let mut ctx = registry.resolve(reference, now).map_err(|e| match e {
        ContextRefError::Invalid => ContextHeaderError::Invalid,
        ContextRefError::Expired => ContextHeaderError::Expired,
    })?;
    apply_request_fields(&mut ctx, req);
    Ok((ctx, true))
}

/// 本文から測れる量だけを context に足す。登録済みの値より緩い側には動かさない。
fn apply_request_fields(ctx: &mut RoutingContext, req: &ChatCompletionRequest) {
    let chars: usize = req
        .messages
        .iter()
        .filter_map(|m| m.content.as_ref())
        .map(|c| c.as_text().chars().count())
        .sum();
    let measured_input = chars.div_ceil(4) as u64;
    let mut touched: Vec<&str> = Vec::new();
    if ctx.input_tokens.is_none_or(|v| v < measured_input) {
        ctx.input_tokens = Some(measured_input);
        touched.push("input_tokens");
    }
    if let Some(max) = req.max_tokens.map(u64::from)
        && ctx.output_reserve.is_none_or(|v| v < max)
    {
        ctx.output_reserve = Some(max);
        touched.push("output_reserve");
    }
    if req.tools.as_ref().is_some_and(|t| !t.is_empty()) && !ctx.required_tools {
        ctx.required_tools = true;
        touched.push("required_tools");
    }
    if req.stream && !ctx.required_streaming {
        ctx.required_streaming = true;
        touched.push("required_streaming");
    }
    for field in touched {
        ctx.field_provenance
            .insert(field.to_string(), REQUEST_FIELDS_PROVENANCE.to_string());
        ctx.missing_fields.retain(|m| m != field);
    }
}

/// run ごとの in-flight 要求数。
#[derive(Default)]
pub struct InFlightContexts {
    counts: StdMutex<HashMap<String, usize>>,
}

impl InFlightContexts {
    /// run `run_id` の in-flight 要求数（daemon は 0 になってから ref を外す）。
    pub fn count(&self, run_id: &str) -> usize {
        self.counts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(run_id)
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn enter(self: &Arc<Self>, run_id: &str) -> InFlightGuard {
        *self
            .counts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(run_id.to_string())
            .or_insert(0) += 1;
        InFlightGuard {
            owner: Arc::clone(self),
            run_id: run_id.to_string(),
        }
    }
}

/// 要求の終わり（応答を返し終えた・stream が閉じた・落とされた）で数を戻す。
pub struct InFlightGuard {
    owner: Arc<InFlightContexts>,
    run_id: String,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let mut counts = self.owner.counts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = counts.get_mut(&self.run_id) {
            *v = v.saturating_sub(1);
            if *v == 0 {
                counts.remove(&self.run_id);
            }
        }
    }
}

/// task に相関できる要求なら sink 用の event を組む（standalone・task 不明は `None`。proxy log のみ）。
pub fn build_event(
    routing: &ResolvedRouting,
    request_id: &str,
    attempts: &[RequestSourceAttempt],
) -> Option<ProxyRoutingEvent> {
    if !routing.registered {
        return None;
    }
    let task_id = routing.task_id()?.to_string();
    let ctx = &routing.context;
    let features = RoutingFeaturesRecord {
        decision_id: routing.decision_id.clone(),
        context_version: ROUTING_CONTEXT_VERSION.to_string(),
        features: serde_json::to_value(ctx).unwrap_or(serde_json::Value::Null),
        provenance: ctx
            .field_provenance
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
        missing_fields: ctx.missing_fields.clone(),
        run_id: ctx.run_id.clone(),
        request_id: Some(request_id.to_string()),
        stage: Some(FeatureStage::Proxy),
    };
    let fallback_reason = attempts
        .iter()
        .rev()
        .skip(1)
        .find_map(|a| a.fallback_reason.clone());
    let request = RoutingRequestRecord {
        request_id: request_id.to_string(),
        decision_id: routing.decision_id.clone(),
        parent_decision_id: routing.parent_decision_id.clone(),
        run_id: ctx.run_id.clone(),
        trace: None,
        attempts: attempts.to_vec(),
        fallback_reason,
    };
    Some(ProxyRoutingEvent {
        task_id,
        features,
        request,
    })
}
