//! dispatcher の時計（試験は `test_now` / `test_policy_clock` で差し替える）。

use super::*;

impl Dispatcher {
    pub(super) fn now_utc(&self) -> OffsetDateTime {
        #[cfg(test)]
        if let Some(clock) = &self.test_now {
            return *clock.lock().expect("test clock mutex");
        }
        OffsetDateTime::now_utc()
    }

    pub(super) fn monotonic_now(&self) -> Instant {
        #[cfg(test)]
        if let Some(clock) = &self.test_policy_clock {
            return *clock.lock().expect("test monotonic clock mutex");
        }
        Instant::now()
    }
}
