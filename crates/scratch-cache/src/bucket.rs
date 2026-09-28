//! flusher の帯域制限（token bucket）。`take` は足りない分を負債にして「待つべき時間」を返す。呼び出し側が
//! その時間だけ待ってから書けば、長さ `T` の区間に書くバイト数は `burst + rate × T` を超えない。

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct TokenBucket {
    /// byte / 秒（0 = 無制限）。
    rate: f64,
    /// 貯められる上限（byte）。
    burst: f64,
    tokens: f64,
    last: Duration,
}

impl TokenBucket {
    /// `rate_bytes_per_sec` の帯域、`burst_bytes` まで貯められる（満タンで始まる）。
    pub fn new(rate_bytes_per_sec: u64, burst_bytes: u64, now: Duration) -> Self {
        Self {
            rate: rate_bytes_per_sec as f64,
            burst: burst_bytes as f64,
            tokens: burst_bytes as f64,
            last: now,
        }
    }

    pub fn rate_bytes_per_sec(&self) -> u64 {
        self.rate as u64
    }

    fn refill(&mut self, now: Duration) {
        if now > self.last {
            let dt = (now - self.last).as_secs_f64();
            self.tokens = (self.tokens + dt * self.rate).min(self.burst);
            self.last = now;
        }
    }

    /// `n` byte を取る。返り値は書く前に待つべき時間（足りていれば 0）。
    pub fn take(&mut self, n: u64, now: Duration) -> Duration {
        if self.rate <= 0.0 {
            return Duration::ZERO;
        }
        self.refill(now);
        self.tokens -= n as f64;
        if self.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-self.tokens / self.rate)
        }
    }
}
