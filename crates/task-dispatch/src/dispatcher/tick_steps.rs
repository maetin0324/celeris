//! 第 4 フェーズ P4-A（ADR 2026-11-05-phase4-p4a-baseline-contract D3）: tick の段ごとの所要時間。
//! blocked の task の数に比例して伸びる段を特定するための計測の口で、判断には使わない
//! （`TickReport::step_us`、`crates/celeris/tests/tick_scale_baseline.rs`）。遅い tick の警告もこの値を出す。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 段ごとの glibc `mallinfo2` 差分を log に出す計測（daemon-rss 2026-11-06）。環境変数 `CELERIS_TICK_MALLOC_TRACE=1`
/// のときだけ働く。`mallinfo2` は arena の lock を取って全 bin を走査するため、本番では既定で無効にする。
fn malloc_trace_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CELERIS_TICK_MALLOC_TRACE").is_some_and(|v| v == "1"))
}

/// 前の tick の終わりの arena（bytes）。tick の外（API・バックグラウンドの task）の増分を出すのに使う。
static LAST_TICK_END_ARENA: AtomicI64 = AtomicI64::new(-1);

/// (arena, in_use, free) の bytes。
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn malloc_now() -> Option<(i64, i64, i64)> {
    // SAFETY: `mallinfo2` は引数を取らず値で構造体を返す（glibc 2.33 以降）。
    let m = unsafe { libc::mallinfo2() };
    Some((m.arena as i64, m.uordblks as i64, m.fordblks as i64))
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn malloc_now() -> Option<(i64, i64, i64)> {
    None
}

/// 遅い tick の内訳を警告に出す閾値（ADR-0015 D2）。
pub(super) const SLOW_TICK: Duration = Duration::from_secs(1);

pub(super) struct TickSteps {
    started: Instant,
    at: Instant,
    out: Vec<(&'static str, u64)>,
    /// 計測が有効なときの前の印の `mallinfo2`。
    malloc_at: Option<(i64, i64, i64)>,
    /// tick の開始時の arena と、前の tick の終わりからの差（tick の外の増分）。
    outside_arena_delta: i64,
    /// 段ごとの (名前, arena 差, in_use 差, free 差)。arena が動いた段だけ。
    malloc_steps: Vec<(&'static str, i64, i64, i64)>,
}

impl TickSteps {
    pub(super) fn start() -> Self {
        let now = Instant::now();
        let malloc_at = if malloc_trace_enabled() {
            malloc_now()
        } else {
            None
        };
        let prev_end = LAST_TICK_END_ARENA.load(Ordering::Relaxed);
        let outside_arena_delta = match malloc_at {
            Some((arena, _, _)) if prev_end >= 0 => arena - prev_end,
            _ => 0,
        };
        Self {
            started: now,
            at: now,
            out: Vec::new(),
            malloc_at,
            outside_arena_delta,
            malloc_steps: Vec::new(),
        }
    }

    /// 前の印からの時間を `name` の段として積む。
    pub(super) fn mark(&mut self, name: &'static str) {
        let us = u64::try_from(self.at.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.out.push((name, us));
        if let Some(prev) = self.malloc_at {
            if let Some(cur) = malloc_now() {
                let d = (cur.0 - prev.0, cur.1 - prev.1, cur.2 - prev.2);
                if d.0 != 0 {
                    self.malloc_steps.push((name, d.0, d.1, d.2));
                }
                self.malloc_at = Some(cur);
            }
        }
        self.at = Instant::now();
    }

    /// 段の内側の小段を、現在の `mark` とは別に `name.sub` の名前で積む（時間は足さない。内訳の観測用）。
    pub(super) fn sub(&mut self, parts: Vec<(&'static str, u64)>) {
        self.out.extend(parts);
    }

    /// `mark` と `sub` を 1 回で行う。
    pub(super) fn mark_with(&mut self, name: &'static str, parts: Vec<(&'static str, u64)>) {
        self.mark(name);
        self.sub(parts);
    }

    /// 段の列を返し、tick が遅ければ内訳を警告に出す。
    pub(super) fn finish(self) -> Vec<(&'static str, u64)> {
        let total = self.started.elapsed();
        if let Some((arena, _, _)) = self.malloc_at {
            LAST_TICK_END_ARENA.store(arena, Ordering::Relaxed);
        }
        if !self.malloc_steps.is_empty() || self.outside_arena_delta != 0 {
            let arena_delta: i64 = self.malloc_steps.iter().map(|s| s.1).sum();
            tracing::info!(
                total_ms = total.as_millis() as u64,
                arena_delta,
                outside_arena_delta = self.outside_arena_delta,
                steps = ?self.malloc_steps,
                "tick malloc phases"
            );
        }
        if total >= SLOW_TICK {
            tracing::warn!(
                total_ms = total.as_millis() as u64,
                steps_us = ?self.out,
                "slow tick phases"
            );
        }
        self.out
    }
}
