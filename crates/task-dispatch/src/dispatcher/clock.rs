//! dispatcher の時計（試験は `test_hooks.now` / `test_hooks.policy_clock` で差し替える）。

use super::*;

impl Dispatcher {
    pub(super) fn now_utc(&self) -> OffsetDateTime {
        #[cfg(test)]
        if let Some(clock) = &self.test_hooks.now {
            return *clock.lock().expect("test clock mutex");
        }
        OffsetDateTime::now_utc()
    }

    pub(super) fn monotonic_now(&self) -> Instant {
        (self.monotonic_clock())()
    }

    /// `monotonic_now` と同じ時計を、`Dispatcher` を借りない形で返す（owner の method に渡す）。
    pub(super) fn monotonic_clock(&self) -> impl Fn() -> Instant + 'static {
        #[cfg(test)]
        let test_clock = self.test_hooks.policy_clock.clone();
        move || {
            #[cfg(test)]
            if let Some(clock) = &test_clock {
                return *clock.lock().expect("test monotonic clock mutex");
            }
            Instant::now()
        }
    }
}
