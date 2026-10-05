//! ADR 2026-10-04 §10 Phase 5: 外部 estimator sidecar の非同期 client（`POST /estimate` v1）。
//!
//! 契約（型と値検証）は task-core の `model_router::estimator::sidecar` が持ち、ここは transport・
//! timeout・同時実行上限・payload 上限・circuit・privacy gate だけを持つ。結果は比較用の
//! [`SidecarEstimateSnapshot`] か、理由付きの評価不能（[`SidecarUnavailable`]）で、どちらも
//! heuristic の primary 判断には触れない（決定権を持たない）。
//!
//! 送る前に止める gate（送信 0）: circuit open・同時実行上限・descriptor の版・prompt 必須なのに
//! 送れない・descriptor が network/外部 embeddings 依存を申告して許可が無い・request が上限超過。
//! `send_prompt=false` なら `optional_prompt` は必ず落としてから送る。
//! 時計は注入する（circuit の開閉。試験は `tokio::time::pause`）。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use task_core::model_router::estimator::sidecar::{
    EstimateRequestV1, EstimateResponseV1, EstimatorDescriptor, SidecarEstimateSnapshot,
    SidecarProtocolError,
};
use tokio::sync::Semaphore;
use tokio::time::Instant;

/// circuit 用の単調時計。
pub trait MonotonicClock: Send + Sync {
    fn now(&self) -> Instant;
}

/// tokio の時計（`tokio::time::pause` に従う）。
pub struct TokioClock;

impl MonotonicClock for TokioClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// client の設定。daemon の `[model_routing.estimator.sidecar]` から組み立てる（検証済みの値）。
#[derive(Debug, Clone)]
pub struct SidecarClientConfig {
    /// sidecar の base URL（`/estimate` を足す）。
    pub endpoint: reqwest::Url,
    /// 期待する estimator の識別・版・依存申告。応答はこれと一致しなければ評価不能。
    pub descriptor: EstimatorDescriptor,
    pub timeout: Duration,
    pub max_inflight: usize,
    pub max_payload_bytes: usize,
    pub send_prompt: bool,
    /// descriptor が network / 外部 embeddings 依存を申告していても呼んでよいか（既定 false）。
    pub allow_external_dependencies: bool,
    /// 連続失敗がこの回数に達したら circuit を開く。
    pub circuit_failure_threshold: u32,
    /// circuit を開いておく時間。過ぎたら 1 件だけ試す（失敗で再び開く）。
    pub circuit_open_for: Duration,
}

impl SidecarClientConfig {
    pub fn new(endpoint: reqwest::Url, descriptor: EstimatorDescriptor) -> Self {
        Self {
            endpoint,
            descriptor,
            timeout: Duration::from_millis(1_000),
            max_inflight: 4,
            max_payload_bytes: 65_536,
            send_prompt: false,
            allow_external_dependencies: false,
            circuit_failure_threshold: 3,
            circuit_open_for: Duration::from_secs(30),
        }
    }
}

/// 評価不能の理由。`reason()` は監査・shadow 記録に載せる安定した短い語。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SidecarUnavailable {
    #[error("circuit open")]
    CircuitOpen,
    #[error("max_inflight reached")]
    Inflight,
    #[error("timeout")]
    Timeout,
    #[error("transport error: {0}")]
    Transport(String),
    #[error("http status {0}")]
    Status(u16),
    #[error("payload exceeds {0} bytes")]
    PayloadTooLarge(usize),
    #[error("invalid response body: {0}")]
    Decode(String),
    #[error("estimator protocol or version mismatch: {0}")]
    VersionMismatch(SidecarProtocolError),
    #[error("estimator requires a prompt that may not be sent")]
    PromptRequired,
    #[error("estimator declares network or external embeddings dependencies")]
    DependenciesNotAllowed,
    #[error("estimator dependencies differ from descriptor")]
    DependencyMismatch,
    #[error("invalid estimate: {0}")]
    Protocol(SidecarProtocolError),
}

impl SidecarUnavailable {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::CircuitOpen => "circuit_open",
            Self::Inflight => "max_inflight",
            Self::Timeout => "timeout",
            Self::Transport(_) => "transport",
            Self::Status(_) => "http_status",
            Self::PayloadTooLarge(_) => "payload_too_large",
            Self::Decode(_) => "decode",
            Self::VersionMismatch(_) => "version_mismatch",
            Self::PromptRequired => "prompt_required",
            Self::DependenciesNotAllowed => "dependencies_not_allowed",
            Self::DependencyMismatch => "dependency_mismatch",
            Self::Protocol(_) => "invalid_estimate",
        }
    }

    /// sidecar 側の失敗として circuit に数えるか（送る前の gate は数えない）。
    fn counts_as_failure(&self) -> bool {
        !matches!(
            self,
            Self::CircuitOpen
                | Self::Inflight
                | Self::PromptRequired
                | Self::DependenciesNotAllowed
                | Self::PayloadTooLarge(_)
        )
    }
}

fn classify(error: SidecarProtocolError) -> SidecarUnavailable {
    match error {
        SidecarProtocolError::ProtocolVersion(_) | SidecarProtocolError::EstimatorVersion => {
            SidecarUnavailable::VersionMismatch(error)
        }
        SidecarProtocolError::Dependencies => SidecarUnavailable::DependencyMismatch,
        other => SidecarUnavailable::Protocol(other),
    }
}

#[derive(Debug, Default)]
struct Circuit {
    consecutive_failures: u32,
    open_until: Option<Instant>,
}

/// 非同期 sidecar client。状態は circuit と最後の検証済み snapshot だけ（タスクの状態は持たない）。
pub struct SidecarEstimatorClient {
    config: SidecarClientConfig,
    url: reqwest::Url,
    http: reqwest::Client,
    inflight: Arc<Semaphore>,
    clock: Arc<dyn MonotonicClock>,
    circuit: Mutex<Circuit>,
    last: Mutex<Option<Arc<SidecarEstimateSnapshot>>>,
}

impl SidecarEstimatorClient {
    /// redirect は追わず、環境の proxy も使わない（送信先は endpoint だけ）。
    pub fn new(
        config: SidecarClientConfig,
        clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, String> {
        if !matches!(config.endpoint.scheme(), "http" | "https") {
            return Err("sidecar endpoint must use http or https".into());
        }
        if config.max_inflight == 0 || config.max_payload_bytes == 0 {
            return Err("max_inflight and max_payload_bytes must be positive".into());
        }
        let mut url = config.endpoint.clone();
        url.set_path(&format!("{}/estimate", url.path().trim_end_matches('/')));
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("cannot build sidecar http client: {e}"))?;
        Ok(Self {
            inflight: Arc::new(Semaphore::new(config.max_inflight)),
            config,
            url,
            http,
            clock,
            circuit: Mutex::new(Circuit::default()),
            last: Mutex::new(None),
        })
    }

    pub fn descriptor(&self) -> &EstimatorDescriptor {
        &self.config.descriptor
    }

    /// 最後に検証を通った snapshot（無ければ `None`）。
    pub fn cached_snapshot(&self) -> Option<Arc<SidecarEstimateSnapshot>> {
        self.last.lock().ok().and_then(|last| last.clone())
    }

    pub fn circuit_open(&self) -> bool {
        let now = self.clock.now();
        self.circuit
            .lock()
            .ok()
            .and_then(|c| c.open_until)
            .is_some_and(|until| now < until)
    }

    /// `POST /estimate` を 1 回呼ぶ。失敗はすべて理由付きの評価不能で返す。
    pub async fn estimate(
        &self,
        mut request: EstimateRequestV1,
    ) -> Result<Arc<SidecarEstimateSnapshot>, SidecarUnavailable> {
        let descriptor = &self.config.descriptor;
        descriptor
            .validate()
            .map_err(SidecarUnavailable::VersionMismatch)?;
        let deps = &descriptor.dependencies;
        if (deps.needs_network || deps.external_embeddings)
            && !self.config.allow_external_dependencies
        {
            return Err(SidecarUnavailable::DependenciesNotAllowed);
        }
        if !self.config.send_prompt {
            request.optional_prompt = None;
        }
        if descriptor.needs_prompt && request.optional_prompt.is_none() {
            return Err(SidecarUnavailable::PromptRequired);
        }
        match request.validate(self.config.max_payload_bytes) {
            Ok(()) => {}
            Err(SidecarProtocolError::Size { limit, .. }) => {
                return Err(SidecarUnavailable::PayloadTooLarge(limit));
            }
            Err(other) => return Err(SidecarUnavailable::Protocol(other)),
        }
        if self.circuit_open() {
            return Err(SidecarUnavailable::CircuitOpen);
        }
        let Ok(_permit) = self.inflight.clone().try_acquire_owned() else {
            return Err(SidecarUnavailable::Inflight);
        };
        let result = match tokio::time::timeout(self.config.timeout, self.call(&request)).await {
            Ok(result) => result,
            Err(_) => Err(SidecarUnavailable::Timeout),
        };
        self.record(&result);
        result
    }

    async fn call(
        &self,
        request: &EstimateRequestV1,
    ) -> Result<Arc<SidecarEstimateSnapshot>, SidecarUnavailable> {
        let limit = self.config.max_payload_bytes;
        let mut response = self
            .http
            .post(self.url.clone())
            .json(request)
            .send()
            .await
            .map_err(|e| SidecarUnavailable::Transport(e.without_url().to_string()))?;
        if !response.status().is_success() {
            return Err(SidecarUnavailable::Status(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|len| len > limit as u64)
        {
            return Err(SidecarUnavailable::PayloadTooLarge(limit));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| SidecarUnavailable::Transport(e.without_url().to_string()))?
        {
            if body.len() + chunk.len() > limit {
                return Err(SidecarUnavailable::PayloadTooLarge(limit));
            }
            body.extend_from_slice(&chunk);
        }
        let parsed: EstimateResponseV1 =
            serde_json::from_slice(&body).map_err(|e| SidecarUnavailable::Decode(e.to_string()))?;
        let snapshot = SidecarEstimateSnapshot::from_response(
            request,
            &parsed,
            &self.config.descriptor,
            limit,
        )
        .map_err(classify)?;
        Ok(Arc::new(snapshot))
    }

    fn record(&self, result: &Result<Arc<SidecarEstimateSnapshot>, SidecarUnavailable>) {
        let Ok(mut circuit) = self.circuit.lock() else {
            return;
        };
        match result {
            Ok(snapshot) => {
                *circuit = Circuit::default();
                if let Ok(mut last) = self.last.lock() {
                    *last = Some(snapshot.clone());
                }
            }
            Err(error) if error.counts_as_failure() => {
                circuit.consecutive_failures = circuit.consecutive_failures.saturating_add(1);
                if circuit.consecutive_failures >= self.config.circuit_failure_threshold.max(1) {
                    circuit.open_until = Some(self.clock.now() + self.config.circuit_open_for);
                }
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_send_gates_do_not_count_as_sidecar_failures() {
        assert!(!SidecarUnavailable::PromptRequired.counts_as_failure());
        assert!(!SidecarUnavailable::CircuitOpen.counts_as_failure());
        assert!(SidecarUnavailable::Timeout.counts_as_failure());
        assert!(SidecarUnavailable::DependencyMismatch.counts_as_failure());
    }
}
