//! 同一要求内の fallback（ADR 2026-10-04-multi-objective-model-routing §5、Phase 2）。
//!
//! - 例外の分類（[`FailureClass`]）と分類別の retry 上限・総上限・deadline（[`FallbackBudget`]）。
//! - deployment の circuit breaker（closed → open → half_open → closed。[`Breakers`]）。half_open の
//!   試し打ちは同時 1 件だけ（[`Permit`] が持つ。drop で試し打ちの枠を返す）。
//! - 要求の制約（[`RequestConstraints`]）: fallback 先は要求が許す source に限る。
//!
//! 時刻は全部引数か注入した [`crate::reservation::Clock`] から取る（ここでは時計を読まない）。
//! 判断の束ね（候補を順に試す・stream の最初の byte の前だけ倒す）は `server.rs` が行う。

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use time::{Duration as TimeDuration, OffsetDateTime};

use crate::naming::{ModelRequest, SourceKind, SourceScope};
use crate::sources::SourceError;

/// 失敗の分類。fallback するか・breaker に数えるかはここで決まる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// 401: 資格情報の拒否（account 単位。既存の account cooldown）。
    Unauthorized,
    /// 429: rate limit（account 単位。既存の account cooldown・Retry-After）。
    RateLimited,
    /// 5xx: 上流の障害（deployment 単位。breaker に数える）。
    Server,
    /// 接続・timeout・本文の破損（deployment 単位。breaker に数える）。
    Network,
    /// その候補だけの事情（資格情報ファイルが読めない・source が無い）。
    Local,
    /// 401/429 以外の 4xx: 要求そのものの誤り。別の候補でも直らないので倒さない。
    Client,
}

impl FailureClass {
    pub fn of(e: &SourceError) -> Self {
        match e {
            SourceError::Unauthorized => Self::Unauthorized,
            SourceError::RateLimited { .. } => Self::RateLimited,
            SourceError::Upstream { status, .. } if *status >= 500 => Self::Server,
            SourceError::Upstream { .. } => Self::Client,
            SourceError::Network(_) => Self::Network,
            SourceError::Credentials(_) | SourceError::Unavailable(_) => Self::Local,
        }
    }

    /// deployment の breaker に失敗として数えるか（401/429 は account の cooldown で扱う）。
    pub fn trips_breaker(self) -> bool {
        matches!(self, Self::Server | Self::Network)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate_limited",
            Self::Server => "server",
            Self::Network => "network",
            Self::Local => "local",
            Self::Client => "client",
        }
    }
}

/// 分類別の「この分類の失敗の後に次の候補へ倒してよい回数」と、1 要求で上流へ送る総回数の上限。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryLimits {
    pub unauthorized: u32,
    pub rate_limited: u32,
    pub server: u32,
    pub network: u32,
    pub local: u32,
    pub client: u32,
    /// 1 要求で上流へ送る回数の総上限（最初の 1 回を含む）。
    pub total_attempts: u32,
}

impl Default for RetryLimits {
    fn default() -> Self {
        Self {
            unauthorized: 3,
            rate_limited: 3,
            server: 2,
            network: 2,
            local: 3,
            client: 0,
            total_attempts: 6,
        }
    }
}

impl RetryLimits {
    fn for_class(&self, class: FailureClass) -> u32 {
        match class {
            FailureClass::Unauthorized => self.unauthorized,
            FailureClass::RateLimited => self.rate_limited,
            FailureClass::Server => self.server,
            FailureClass::Network => self.network,
            FailureClass::Local => self.local,
            FailureClass::Client => self.client,
        }
    }
}

/// deployment の breaker の設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakerSettings {
    /// closed で連続この回数失敗したら open にする（0 は breaker を使わない）。
    pub failure_threshold: u32,
    /// open の長さ。過ぎた後の最初の 1 件が half_open の試し打ちになる。
    pub open_secs: i64,
}

impl Default for BreakerSettings {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            open_secs: 30,
        }
    }
}

/// fallback の設定一式（config への配線は config unit。ここは既定値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackSettings {
    pub limits: RetryLimits,
    pub breaker: BreakerSettings,
    /// 要求の受付からこの秒数を過ぎたら次の候補へ倒さない（最後の失敗を返す）。
    pub deadline_secs: i64,
}

impl Default for FallbackSettings {
    fn default() -> Self {
        Self {
            limits: RetryLimits::default(),
            breaker: BreakerSettings::default(),
            deadline_secs: 120,
        }
    }
}

/// 失敗の後の判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterFailure {
    /// 次の適格候補へ倒す。
    Fallback,
    /// 倒さずに最後の失敗を caller に返す。
    Stop(StopReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// 分類別の上限に達した（`Client` は上限 0 なので初回で止まる）。
    ClassLimit(FailureClass),
    /// 総上限に達した。
    TotalLimit,
    /// deadline を過ぎた。
    Deadline,
}

/// 1 要求の retry の帳簿。純粋（時刻は引数）。
#[derive(Debug, Clone)]
pub struct FallbackBudget {
    limits: RetryLimits,
    deadline: OffsetDateTime,
    attempts: u32,
    failures: HashMap<FailureClass, u32>,
}

impl FallbackBudget {
    pub fn new(settings: &FallbackSettings, started: OffsetDateTime) -> Self {
        Self {
            limits: settings.limits.clone(),
            deadline: started + TimeDuration::seconds(settings.deadline_secs.max(0)),
            attempts: 0,
            failures: HashMap::new(),
        }
    }

    /// 上流へ 1 回送る前に呼ぶ。総上限に達していれば false（送らない）。
    pub fn begin_attempt(&mut self) -> bool {
        if self.attempts >= self.limits.total_attempts {
            return false;
        }
        self.attempts += 1;
        true
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// 失敗を数え、次の候補へ倒してよいかを返す。
    pub fn after_failure(&mut self, class: FailureClass, now: OffsetDateTime) -> AfterFailure {
        let n = self.failures.entry(class).or_insert(0);
        *n += 1;
        if *n > self.limits.for_class(class) {
            return AfterFailure::Stop(StopReason::ClassLimit(class));
        }
        if self.attempts >= self.limits.total_attempts {
            return AfterFailure::Stop(StopReason::TotalLimit);
        }
        if now >= self.deadline {
            return AfterFailure::Stop(StopReason::Deadline);
        }
        AfterFailure::Fallback
    }
}

// ---------------------------------------------------------------------------
// 要求の制約
// ---------------------------------------------------------------------------

/// fallback が引き継ぐ要求の制約。候補がこれに合わなければ倒さない（飛ばす）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestConstraints {
    /// 要求が許す source（`claude/…` は Claude だけ、`qwen/…` は relay だけ、`celeris/…` は全部）。
    pub allowed: Vec<SourceKind>,
}

impl RequestConstraints {
    pub fn from_request(parsed: &ModelRequest) -> Self {
        let allowed = match parsed {
            ModelRequest::Explicit { source, .. } => vec![*source],
            ModelRequest::Tiered {
                scope: SourceScope::Only(kind),
                ..
            } => vec![*kind],
            ModelRequest::Tiered {
                scope: SourceScope::Any,
                ..
            } => vec![SourceKind::Claude, SourceKind::Gpt, SourceKind::Qwen],
        };
        Self { allowed }
    }

    pub fn admits(&self, kind: SourceKind) -> bool {
        self.allowed.contains(&kind)
    }
}

// ---------------------------------------------------------------------------
// deployment の circuit breaker
// ---------------------------------------------------------------------------

/// breaker の状態（表示・試験用に公開）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed { consecutive_failures: u32 },
    Open { until: OffsetDateTime },
    HalfOpen { probe_in_flight: bool },
}

/// 送ってよいかの判断の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// closed: 普通に送る。
    Normal,
    /// half_open の試し打ち（同時 1 件）。
    Probe,
}

/// deployment id → breaker 状態。in-process（プロキシ 1 つの中で共有）。
#[derive(Debug)]
pub struct Breakers {
    settings: BreakerSettings,
    states: StdMutex<HashMap<String, BreakerState>>,
}

impl Breakers {
    pub fn new(settings: BreakerSettings) -> Arc<Self> {
        Arc::new(Self {
            settings,
            states: StdMutex::new(HashMap::new()),
        })
    }

    pub fn state(&self, deployment: &str) -> BreakerState {
        self.lock()
            .get(deployment)
            .copied()
            .unwrap_or(BreakerState::Closed {
                consecutive_failures: 0,
            })
    }

    /// open で deadline 前の deployment のうち最も早く開く時刻（Retry-After の算出用）。
    pub fn earliest_reopen(
        &self,
        deployments: &[String],
        now: OffsetDateTime,
    ) -> Option<OffsetDateTime> {
        let guard = self.lock();
        deployments
            .iter()
            .filter_map(|d| match guard.get(d) {
                Some(BreakerState::Open { until }) if *until > now => Some(*until),
                _ => None,
            })
            .min()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, BreakerState>> {
        self.states.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 送ってよいなら [`Permit`] を返す。open（deadline 前）と、試し打ちが飛んでいる half_open は None。
    pub fn admit(self: &Arc<Self>, deployment: &str, now: OffsetDateTime) -> Option<Permit> {
        if self.settings.failure_threshold == 0 {
            return Some(self.permit(deployment, Admission::Normal));
        }
        let mut guard = self.lock();
        let state = guard
            .entry(deployment.to_string())
            .or_insert(BreakerState::Closed {
                consecutive_failures: 0,
            });
        let admission = match *state {
            BreakerState::Closed { .. } => Admission::Normal,
            BreakerState::Open { until } if now < until => return None,
            BreakerState::Open { .. }
            | BreakerState::HalfOpen {
                probe_in_flight: false,
            } => {
                *state = BreakerState::HalfOpen {
                    probe_in_flight: true,
                };
                Admission::Probe
            }
            BreakerState::HalfOpen {
                probe_in_flight: true,
            } => return None,
        };
        drop(guard);
        Some(self.permit(deployment, admission))
    }

    fn permit(self: &Arc<Self>, deployment: &str, admission: Admission) -> Permit {
        Permit {
            breakers: Arc::clone(self),
            deployment: deployment.to_string(),
            admission,
            settled: false,
        }
    }

    fn settle(
        &self,
        deployment: &str,
        admission: Admission,
        outcome: Outcome,
        now: OffsetDateTime,
    ) {
        if self.settings.failure_threshold == 0 {
            return;
        }
        let mut guard = self.lock();
        let state = guard
            .entry(deployment.to_string())
            .or_insert(BreakerState::Closed {
                consecutive_failures: 0,
            });
        let open = BreakerState::Open {
            until: now + TimeDuration::seconds(self.settings.open_secs.max(0)),
        };
        *state = match (admission, outcome, *state) {
            (_, Outcome::Success, _) => BreakerState::Closed {
                consecutive_failures: 0,
            },
            (Admission::Probe, Outcome::Failure, _) => open,
            (Admission::Probe, Outcome::Neutral, _) => BreakerState::HalfOpen {
                probe_in_flight: false,
            },
            (
                Admission::Normal,
                Outcome::Failure,
                BreakerState::Closed {
                    consecutive_failures,
                },
            ) => {
                let n = consecutive_failures + 1;
                if n >= self.settings.failure_threshold {
                    open
                } else {
                    BreakerState::Closed {
                        consecutive_failures: n,
                    }
                }
            }
            // 並行する要求が先に状態を変えていたら、その状態を尊重する。
            (Admission::Normal, _, current) => current,
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Success,
    Failure,
    /// 上流は応答したが成否を breaker に数えない（401/429/4xx）。
    Neutral,
}

/// 1 回の送信の許可。結果を [`Permit::success`] / [`Permit::failure`] / [`Permit::neutral`] で
/// 返す。返さずに drop したら（要求の取り消し等）neutral と同じ（試し打ちの枠を返す）。
#[derive(Debug)]
pub struct Permit {
    breakers: Arc<Breakers>,
    deployment: String,
    admission: Admission,
    settled: bool,
}

impl Permit {
    pub fn admission(&self) -> Admission {
        self.admission
    }

    pub fn success(mut self, now: OffsetDateTime) {
        self.finish(Outcome::Success, now);
    }

    pub fn failure(mut self, now: OffsetDateTime) {
        self.finish(Outcome::Failure, now);
    }

    pub fn neutral(mut self, now: OffsetDateTime) {
        self.finish(Outcome::Neutral, now);
    }

    /// 分類に応じて failure か neutral を返す。
    pub fn failed_with(self, class: FailureClass, now: OffsetDateTime) {
        if class.trips_breaker() {
            self.failure(now);
        } else {
            self.neutral(now);
        }
    }

    fn finish(&mut self, outcome: Outcome, now: OffsetDateTime) {
        if !self.settled {
            self.settled = true;
            self.breakers
                .settle(&self.deployment, self.admission, outcome, now);
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if !self.settled && self.admission == Admission::Probe {
            self.settled = true;
            let mut guard = self.breakers.lock();
            if let Some(state @ BreakerState::HalfOpen { .. }) = guard.get_mut(&self.deployment) {
                *state = BreakerState::HalfOpen {
                    probe_in_flight: false,
                };
            }
        }
    }
}

#[cfg(test)]
#[path = "fallback_tests.rs"]
mod tests;
