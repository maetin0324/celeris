use std::sync::Mutex;
use std::time::Duration;

/// 連続失敗で開く circuit breaker。時計は呼び出し側が渡す（注入可能）。
/// cooldown が過ぎたら 1 回試す（half-open）。失敗すれば直ちに再び開く。
#[derive(Debug)]
pub struct CircuitBreaker {
    threshold: u32,
    cooldown: Duration,
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    consecutive_failures: u32,
    open_until: Option<Duration>,
}

impl CircuitBreaker {
    pub fn new(threshold: u32, cooldown: Duration) -> Self {
        Self {
            threshold,
            cooldown,
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn allow(&self, now: Duration) -> bool {
        match self.lock().open_until {
            Some(until) => now >= until,
            None => true,
        }
    }

    pub fn record_success(&self) {
        let mut s = self.lock();
        s.consecutive_failures = 0;
        s.open_until = None;
    }

    pub fn record_failure(&self, now: Duration) {
        let mut s = self.lock();
        s.consecutive_failures = s.consecutive_failures.saturating_add(1);
        if s.consecutive_failures >= self.threshold {
            s.open_until = Some(now + self.cooldown);
        }
    }

    pub fn is_open(&self, now: Duration) -> bool {
        !self.allow(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn opens_after_threshold_and_recovers_after_cooldown() {
        let b = CircuitBreaker::new(2, s(10));
        b.record_failure(s(0));
        assert!(b.allow(s(0)));
        b.record_failure(s(1));
        assert!(!b.allow(s(5)));
        assert!(b.allow(s(11)));
    }

    #[test]
    fn half_open_failure_reopens_immediately() {
        let b = CircuitBreaker::new(2, s(10));
        b.record_failure(s(0));
        b.record_failure(s(0));
        assert!(b.allow(s(10)));
        b.record_failure(s(10));
        assert!(!b.allow(s(11)));
    }

    #[test]
    fn success_resets() {
        let b = CircuitBreaker::new(2, s(10));
        b.record_failure(s(0));
        b.record_success();
        b.record_failure(s(1));
        assert!(b.allow(s(1)));
    }
}
