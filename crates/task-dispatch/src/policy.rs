//! `ProviderPolicy` と `StaticPolicy`（DESIGN §5.5, ADR-0005 D6, ADR-0012 D2）。

use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::Arc;
use std::time::{Duration, Instant};

use task_core::{Tier, WorkerHint};

pub type AdapterId = String;
pub type ProviderId = String;

/// `report` に渡す供給側の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderOutcome {
    Ok,
    Throttled { retry_after: Duration },
    AuthFailed,
    Exhausted,
}

/// 設定表の 1 行（`[[providers]]`）。上から順に優先。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSpec {
    pub id: ProviderId,
    pub adapter: AdapterId,
    pub tiers: Vec<Tier>,
    pub concurrency: usize,
    pub model: String,
}

/// cooldown に入った理由（ADR-0013 D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CooldownReason {
    Throttled,
    AuthFailed,
    Exhausted,
}

/// `ProviderPolicy::cooldowns` の 1 件（ADR-0013 D4）。デーモン状態として API に公開する観測値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cooldown {
    pub provider: ProviderId,
    pub until: Instant,
    pub reason: CooldownReason,
}

/// `ProviderPolicy::select` の結果（ADR-0012 D2, P-20 / P-33）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Picked {
        adapter: AdapterId,
        provider: ProviderId,
    },
    /// 条件（adapter 指定・tier）に合うプロバイダはあるが、全て cooldown 中か除外されている。一時的。
    Busy,
    /// 条件に合うプロバイダが設定に 1 つも無い。設定を直さない限り解消しない。
    NoMatchingProvider,
}

/// DESIGN §5.5 の trait。既存 3 メソッドのシグネチャは変えない（供給層との境界）。
pub trait ProviderPolicy: Send {
    fn pick(&self, hint: &WorkerHint, now: Instant) -> Option<(AdapterId, ProviderId)>;
    fn report(&mut self, provider: ProviderId, outcome: &ProviderOutcome);
    fn concurrency_limit(&self, provider: ProviderId) -> usize;

    /// ADR-0012 D2: `excluded`（並列度の上限に達したプロバイダ等）を除いて選ぶ。既定実装は `pick` から導くので
    /// 「候補なし」と「一時的に不可」を区別できず、選べなければ `Busy`（従来どおり待つ）を返す。
    fn select(&self, hint: &WorkerHint, now: Instant, excluded: &HashSet<ProviderId>) -> Selection {
        match self.pick(hint, now) {
            Some((adapter, provider)) if !excluded.contains(&provider) => {
                Selection::Picked { adapter, provider }
            }
            _ => Selection::Busy,
        }
    }

    /// ADR-0013 D4: `now` 時点で cooldown 中のプロバイダ（デーモン状態として API に公開する）。既定実装は空
    /// （cooldown を持たない、または公開しないポリシーはそのまま動く）。
    fn cooldowns(&self, _now: Instant) -> Vec<Cooldown> {
        Vec::new()
    }

    /// ADR-0132 付記 L2 (a): 設定行 `provider` がこの `hint` に合うか（adapter の固定と専用アダプタの
    /// 除外を含む）。cooldown は見ない。既定実装は `false`（ローカル優先の前段を使わない）。
    fn offers(&self, _provider: &str, _hint: &WorkerHint) -> bool {
        false
    }

    /// ADR-0132 付記 L2: 設定行 `provider` のアダプタ名。既定実装は `None`。
    fn adapter_of(&self, _provider: &str) -> Option<AdapterId> {
        None
    }

    /// Legacy optimizer input in config order. Custom policies without an enumerable
    /// config keep their existing selector behavior.
    fn legacy_specs(&self, _hint: &WorkerHint) -> Vec<ProviderSpec> {
        Vec::new()
    }
}

/// 設定表の優先順位どおりに選ぶ。Throttled は cooldown まで除外。
pub struct StaticPolicy {
    providers: Vec<ProviderSpec>,
    error_cooldown: Duration,
    cooldown_until: HashMap<ProviderId, (Instant, CooldownReason)>,
    #[cfg(test)]
    test_clock: Option<Arc<std::sync::Mutex<Instant>>>,
}

impl std::fmt::Debug for StaticPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticPolicy")
            .field("providers", &self.providers)
            .field("error_cooldown", &self.error_cooldown)
            .field("cooldown_until", &self.cooldown_until)
            .finish_non_exhaustive()
    }
}

impl StaticPolicy {
    pub fn new(providers: Vec<ProviderSpec>, error_cooldown: Duration) -> Self {
        Self {
            providers,
            error_cooldown,
            cooldown_until: HashMap::new(),
            #[cfg(test)]
            test_clock: None,
        }
    }

    pub fn providers(&self) -> &[ProviderSpec] {
        &self.providers
    }

    #[cfg(test)]
    pub fn set_test_clock(&mut self, clock: Arc<std::sync::Mutex<Instant>>) {
        self.test_clock = Some(clock);
    }

    #[cfg(test)]
    pub fn advance_test_clock(&mut self, by: Duration) {
        if let Some(clock) = &self.test_clock {
            let mut now = clock.lock().unwrap();
            *now += by;
        }
    }

    fn report_now(&self) -> Instant {
        #[cfg(test)]
        if let Some(clock) = &self.test_clock {
            return *clock.lock().unwrap();
        }
        Instant::now()
    }

    /// 指定プロバイダの `model`（`WorkerStarted.model` 用）。
    pub fn model_of(&self, provider: &str) -> Option<&str> {
        self.providers
            .iter()
            .find(|p| p.id == provider)
            .map(|p| p.model.as_str())
    }

    fn matches(p: &ProviderSpec, hint: &WorkerHint) -> bool {
        let compatible = match hint.adapter.as_deref() {
            Some(adapter) => p.adapter == adapter,
            // 専用契約のアダプタへ対話・計画・通常の作業を渡さない（ADR-0049）。
            None => !matches!(
                p.adapter.as_str(),
                "paperqa" | "local-deep-research" | "langmem"
            ),
        };
        compatible && p.tiers.contains(&hint.tier)
    }

    fn cooling_down(&self, p: &ProviderSpec, now: Instant) -> bool {
        self.cooldown_until
            .get(&p.id)
            .is_some_and(|(until, _)| *until > now)
    }
}

impl ProviderPolicy for StaticPolicy {
    fn pick(&self, hint: &WorkerHint, now: Instant) -> Option<(AdapterId, ProviderId)> {
        self.providers
            .iter()
            .find(|p| Self::matches(p, hint) && !self.cooling_down(p, now))
            .map(|p| {
                tracing::debug!(provider = %p.id, adapter = %p.adapter, "policy: picked provider");
                (p.adapter.clone(), p.id.clone())
            })
    }

    fn report(&mut self, provider: ProviderId, outcome: &ProviderOutcome) {
        match outcome {
            ProviderOutcome::Ok => {}
            ProviderOutcome::Throttled { retry_after } => {
                let until = self.report_now() + *retry_after;
                tracing::debug!(%provider, ?until, "policy: throttled");
                self.cooldown_until
                    .insert(provider, (until, CooldownReason::Throttled));
            }
            ProviderOutcome::AuthFailed | ProviderOutcome::Exhausted => {
                let until = self.report_now() + self.error_cooldown;
                let reason = if *outcome == ProviderOutcome::AuthFailed {
                    CooldownReason::AuthFailed
                } else {
                    CooldownReason::Exhausted
                };
                tracing::debug!(%provider, ?until, ?outcome, "policy: error cooldown");
                self.cooldown_until.insert(provider, (until, reason));
            }
        }
    }

    fn cooldowns(&self, now: Instant) -> Vec<Cooldown> {
        let mut out: Vec<Cooldown> = self
            .cooldown_until
            .iter()
            .filter(|(_, (until, _))| *until > now)
            .map(|(provider, (until, reason))| Cooldown {
                provider: provider.clone(),
                until: *until,
                reason: *reason,
            })
            .collect();
        out.sort_by(|a, b| a.provider.cmp(&b.provider));
        out
    }

    fn concurrency_limit(&self, provider: ProviderId) -> usize {
        self.providers
            .iter()
            .find(|p| p.id == provider)
            .map(|p| p.concurrency)
            .unwrap_or(0)
    }

    fn offers(&self, provider: &str, hint: &WorkerHint) -> bool {
        self.providers
            .iter()
            .any(|p| p.id == provider && Self::matches(p, hint))
    }

    fn adapter_of(&self, provider: &str) -> Option<AdapterId> {
        self.providers
            .iter()
            .find(|p| p.id == provider)
            .map(|p| p.adapter.clone())
    }

    fn legacy_specs(&self, hint: &WorkerHint) -> Vec<ProviderSpec> {
        self.providers
            .iter()
            .filter(|p| Self::matches(p, hint))
            .cloned()
            .collect()
    }

    /// ADR-0012 D2: 設定表の順に、条件に合い cooldown 中でも除外されてもいない最初の行。
    fn select(&self, hint: &WorkerHint, now: Instant, excluded: &HashSet<ProviderId>) -> Selection {
        let mut any_match = false;
        for p in &self.providers {
            if !Self::matches(p, hint) {
                continue;
            }
            any_match = true;
            if self.cooling_down(p, now) || excluded.contains(&p.id) {
                continue;
            }
            return Selection::Picked {
                adapter: p.adapter.clone(),
                provider: p.id.clone(),
            };
        }
        if any_match {
            Selection::Busy
        } else {
            Selection::NoMatchingProvider
        }
    }
}

#[cfg(test)]
mod tests;
