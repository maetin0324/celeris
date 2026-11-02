//! actor へ要求を送る handle と DTO（境界 ADR 2026-11-02 (b)(c)）。
//!
//! - 要求は bounded `tokio::sync::mpsc`（容量 [`QUEUE_CAPACITY`]）で送り、応答は要求ごとの `oneshot` で受ける。
//! - `try_send` が満杯なら待たずに [`ScriptError::Busy`] を返す。
//! - 入力（domain と input の JSON）が [`RuntimeLimits::input_limit_bytes`] を超えたら送らずに
//!   [`ScriptError::InputTooLarge`]。
//! - deadline を過ぎたら呼び出し側は oneshot の受け手を落とし、actor は結果を捨てる。同期の無限ループは
//!   actor の watchdog が同じ deadline で V8 を止める。呼び出し側が future を落とした場合（cancel）も同じく結果を捨てる。
//! - [`ScriptRuntimeHandle::restart`] は今の generation を捨てて isolate を作り直す。未完の要求は
//!   [`ScriptError::GenerationGone`]。
//! - 最後の handle の Drop か [`ScriptRuntimeHandle::shutdown`] で actor thread を止めて join する。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use ulid::Ulid;

use crate::actor::{self, ActorRequest, Control as ActorControl};
use crate::error::ScriptError;

/// 要求 queue の容量（境界 ADR (b)）。
pub const QUEUE_CAPACITY: usize = 64;
/// actor が同時に抱える未完の要求の上限（境界 ADR (c)）。これを超える分は queue に留まる。
pub const MAX_IN_FLIGHT: usize = 64;
/// 入力の上限の既定値（1 MiB、境界 ADR (c)）。
pub const DEFAULT_INPUT_LIMIT_BYTES: usize = 1024 * 1024;
/// V8 heap の上限の既定値（64 MiB、境界 ADR (c)）。
pub const DEFAULT_HEAP_LIMIT_BYTES: usize = 64 * 1024 * 1024;
/// script が定義する入口の関数名。`globalThis[DISPATCH_FUNCTION](domain, input)` を呼ぶ。
pub const DISPATCH_FUNCTION: &str = "__celerisDispatch";

/// actor が起動時（と generation を作り直すたび）に評価する script（classic script）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSource {
    /// stack trace に出る名前（例 `celeris:test/fixture.js`）。
    pub name: String,
    /// script の本文。`globalThis.__celerisDispatch` を関数として定義する。
    pub code: String,
}

/// runtime の上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLimits {
    /// 1 回の入力（domain と input を JSON にした byte 数の和）の上限。
    pub input_limit_bytes: usize,
    /// V8 heap の上限。近づいたら near_heap_limit_callback がその generation の実行を止める。
    pub heap_limit_bytes: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            input_limit_bytes: DEFAULT_INPUT_LIMIT_BYTES,
            heap_limit_bytes: DEFAULT_HEAP_LIMIT_BYTES,
        }
    }
}

/// 1 回の呼び出しの入力。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptCall {
    /// 判定の領域（例 `lane_policy`）。script の入口へ第 1 引数で渡す。
    pub domain: String,
    /// 入力 DTO（JSON）。script の入口へ第 2 引数で渡す。
    pub input: serde_json::Value,
    /// 送ってから応答までの上限。watchdog も同じ deadline で V8 を止める。
    pub timeout: Duration,
}

/// 1 回の呼び出しの結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptReply {
    /// 要求ごとの ID（ULID）。
    pub request_id: Ulid,
    /// 結果を出した generation。
    pub generation: u64,
    /// script の返した値（Promise なら解決後の値）。
    pub output: serde_json::Value,
}

/// 監視用の数値。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStats {
    /// 今の generation の番号（起動時 0、作り直すたびに 1 増える）。
    pub generation: u64,
    /// actor が script に渡して結果を待っている要求の数。
    pub in_flight: usize,
    /// queue に積まれて actor がまだ取り出していない要求の数（[`ScriptRuntimeHandle::stats`] だけが埋める）。
    pub queued: usize,
    /// actor が完了（成功・失敗・破棄）させた要求の累計。
    pub completed: u64,
    /// 呼び出し側が待つのをやめた（cancel・timeout）ので actor が結果を捨てた要求の累計。
    pub discarded: u64,
    /// watchdog が V8 を止めた回数。
    pub watchdog_terminations: u64,
    /// heap 上限で V8 を止めた回数。
    pub heap_limit_hits: u64,
    /// generation を作り直した回数（restart・watchdog・heap 上限）。
    pub restarts: u64,
    /// [`ScriptError::Busy`] で返した要求の累計（handle 側）。
    pub busy_rejections: u64,
    /// [`ScriptError::Timeout`] で返した要求の累計（handle 側）。
    pub timeouts: u64,
    /// Busy・Timeout 以外の誤りで返した要求の累計（handle 側。InputTooLarge を含む）。
    pub failures: u64,
}

/// ScriptRuntime actor の handle。`Clone + Send + Sync`。
#[derive(Clone)]
pub struct ScriptRuntimeHandle {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Counters {
    busy: AtomicU64,
    timeouts: AtomicU64,
    failures: AtomicU64,
}

struct Inner {
    tx: mpsc::Sender<ActorRequest>,
    ctl: mpsc::UnboundedSender<ActorControl>,
    stats: watch::Receiver<RuntimeStats>,
    limits: RuntimeLimits,
    counters: Counters,
    control: Mutex<Control>,
}

struct Control {
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl ScriptRuntimeHandle {
    /// 既定の上限で [`Self::start_with_limits`] を呼ぶ。
    pub fn start(source: ScriptSource) -> Result<Self, ScriptError> {
        Self::start_with_limits(source, RuntimeLimits::default())
    }

    /// actor thread を起こし、`source` を評価して入口の関数を確かめるまで待つ。
    ///
    /// 同期の関数で、評価が終わるまで呼び出し元の thread を塞ぐ。
    pub fn start_with_limits(
        source: ScriptSource,
        limits: RuntimeLimits,
    ) -> Result<Self, ScriptError> {
        let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
        let (ctl_tx, ctl_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (stats_tx, stats_rx) = watch::channel(RuntimeStats::default());
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let channels = actor::Channels {
            rx,
            ctl: ctl_rx,
            shutdown: shutdown_rx,
            stats: stats_tx,
        };
        let thread = std::thread::Builder::new()
            .name("celeris-script".into())
            .spawn(move || actor::run(source, limits, channels, ready_tx))
            .map_err(|_| ScriptError::Terminated)?;
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                let _ = thread.join();
                return Err(err);
            }
            // actor が ready を返さずに終わった（panic など）。
            Err(_) => {
                let _ = thread.join();
                return Err(ScriptError::Terminated);
            }
        }
        Ok(Self {
            inner: Arc::new(Inner {
                tx,
                ctl: ctl_tx,
                stats: stats_rx,
                limits,
                counters: Counters::default(),
                control: Mutex::new(Control {
                    shutdown: Some(shutdown_tx),
                    thread: Some(thread),
                }),
            }),
        })
    }

    /// script の入口を呼び、結果を待つ。queue が満杯なら待たずに [`ScriptError::Busy`]。
    ///
    /// この future を途中で落とすと、actor は後から来た結果を捨てる（cancel）。
    pub async fn call(&self, call: ScriptCall) -> Result<ScriptReply, ScriptError> {
        let result = self.call_inner(call).await;
        let counter = match &result {
            Ok(_) => None,
            Err(ScriptError::Busy) => Some(&self.inner.counters.busy),
            Err(ScriptError::Timeout) => Some(&self.inner.counters.timeouts),
            Err(_) => Some(&self.inner.counters.failures),
        };
        if let Some(c) = counter {
            c.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    async fn call_inner(&self, call: ScriptCall) -> Result<ScriptReply, ScriptError> {
        let size = input_size(&call.domain, &call.input);
        let limit = self.inner.limits.input_limit_bytes;
        if size > limit {
            return Err(ScriptError::InputTooLarge { size, limit });
        }
        let deadline = Instant::now() + call.timeout;
        let request_id = Ulid::new();
        let (reply_tx, reply_rx) = oneshot::channel();
        let request = ActorRequest {
            request_id,
            domain: call.domain,
            input: call.input,
            deadline: deadline.into_std(),
            reply: reply_tx,
        };
        match self.inner.tx.try_send(request) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => return Err(ScriptError::Busy),
            Err(mpsc::error::TrySendError::Closed(_)) => return Err(ScriptError::Terminated),
        }
        match tokio::time::timeout_at(deadline, reply_rx).await {
            Ok(Ok(Ok((generation, output)))) => Ok(ScriptReply {
                request_id,
                generation,
                output,
            }),
            Ok(Ok(Err(err))) => Err(err),
            Ok(Err(_)) => Err(ScriptError::Terminated),
            Err(_) => Err(ScriptError::Timeout),
        }
    }

    /// 今の generation を捨て、同じ script で isolate を作り直す。新しい generation の番号を返す。
    ///
    /// 未完の要求（Promise・async op を待っているもの）は [`ScriptError::GenerationGone`] で返り、
    /// 古い generation の結果は誰にも渡らない。queue に残っている要求は新しい generation で動く。
    pub async fn restart(&self) -> Result<u64, ScriptError> {
        let (done_tx, done_rx) = oneshot::channel();
        self.inner
            .ctl
            .send(ActorControl::Restart { done: done_tx })
            .map_err(|_| ScriptError::Terminated)?;
        done_rx.await.map_err(|_| ScriptError::Terminated)?
    }

    /// 監視用の数値の今の値（actor 側の数と handle 側の数を合わせる）。
    pub fn stats(&self) -> RuntimeStats {
        let mut s = *self.inner.stats.borrow();
        s.queued = self.queued();
        let c = &self.inner.counters;
        s.busy_rejections = c.busy.load(Ordering::Relaxed);
        s.timeouts = c.timeouts.load(Ordering::Relaxed);
        s.failures = c.failures.load(Ordering::Relaxed);
        s
    }

    /// actor 側の数の変化を待つ受け手。handle 側の数（`queued`・`busy_rejections`・`timeouts`・`failures`）は
    /// 0 のままなので [`Self::stats`] で読む。
    pub fn watch_stats(&self) -> watch::Receiver<RuntimeStats> {
        self.inner.stats.clone()
    }

    /// queue に積まれて actor がまだ取り出していない要求の数。
    pub fn queued(&self) -> usize {
        self.inner.tx.max_capacity() - self.inner.tx.capacity()
    }

    /// actor を止めて thread を join する。未完と queue の要求は [`ScriptError::Terminated`] で返る。
    /// 2 回目以降の呼び出しと、他の clone からの呼び出しは何もしない。
    pub fn shutdown(&self) {
        self.inner.stop();
    }
}

/// 入力の大きさ: domain の byte 数と input の JSON の byte 数の和。
fn input_size(domain: &str, input: &serde_json::Value) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += buf.len();
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(domain.len());
    // Value の直列化は失敗しない（失敗したら上限超えとして扱う）。
    if serde_json::to_writer(&mut count, input).is_err() {
        return usize::MAX;
    }
    count.0
}

impl Inner {
    fn stop(&self) {
        let (shutdown, thread) = match self.control.lock() {
            Ok(mut control) => (control.shutdown.take(), control.thread.take()),
            Err(poisoned) => {
                let mut control = poisoned.into_inner();
                (control.shutdown.take(), control.thread.take())
            }
        };
        if let Some(shutdown) = shutdown {
            let _ = shutdown.send(());
        }
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop();
    }
}
