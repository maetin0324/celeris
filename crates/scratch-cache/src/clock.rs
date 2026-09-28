//! 時計（flusher の token bucket と L2 の切り離しのバックオフが使う）。テストは仮想の時計に差し替える。

use std::time::{Duration, Instant, SystemTime};

pub trait Clock: Send + Sync {
    /// 単調な経過時間（起点は実装が決める）。
    fn now(&self) -> Duration;
    /// 壁時計（stats の時刻、L1 / L2 の mtime）。
    fn wall(&self) -> SystemTime;
    /// `d` だけ待つ（flusher が token bucket の不足分を待つ）。
    fn sleep(&self, d: Duration);
}

/// 本物の時計。
pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }
    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }
    fn sleep(&self, d: Duration) {
        if !d.is_zero() {
            std::thread::sleep(d);
        }
    }
}
