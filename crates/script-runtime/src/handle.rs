//! actor へ要求を送る handle と DTO（境界 ADR 2026-11-02 (b)）。
//!
//! - 要求は bounded `tokio::sync::mpsc`（容量 [`QUEUE_CAPACITY`]）で送り、応答は要求ごとの `oneshot` で受ける。
//! - `try_send` が満杯なら待たずに [`ScriptError::Busy`] を返す。
//! - deadline を過ぎたら呼び出し側は oneshot の受け手を落とし、actor は結果を捨てる。
//! - 最後の handle の Drop か [`ScriptRuntimeHandle::shutdown`] で actor thread を止めて join する。

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use ulid::Ulid;

use crate::actor::{self, ActorRequest};
use crate::error::ScriptError;

/// 要求 queue の容量（境界 ADR (b)）。
pub const QUEUE_CAPACITY: usize = 64;
/// actor が同時に抱える未完の要求の上限（境界 ADR (c)）。これを超える分は queue に留まる。
pub const MAX_IN_FLIGHT: usize = 64;
/// script が定義する入口の関数名。`globalThis[DISPATCH_FUNCTION](domain, input)` を呼ぶ。
pub const DISPATCH_FUNCTION: &str = "__celerisDispatch";

/// actor が起動時に評価する script（classic script）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSource {
    /// stack trace に出る名前（例 `celeris:test/fixture.js`）。
    pub name: String,
    /// script の本文。`globalThis.__celerisDispatch` を関数として定義する。
    pub code: String,
}

/// 1 回の呼び出しの入力。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptCall {
    /// 判定の領域（例 `lane_policy`）。script の入口へ第 1 引数で渡す。
    pub domain: String,
    /// 入力 DTO（JSON）。script の入口へ第 2 引数で渡す。
    pub input: serde_json::Value,
    /// 送ってから応答までの上限。
    pub timeout: Duration,
}

/// 1 回の呼び出しの結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptReply {
    /// 要求ごとの ID（ULID）。
    pub request_id: Ulid,
    /// script の返した値（Promise なら解決後の値）。
    pub output: serde_json::Value,
}

/// 監視用の数値。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStats {
    /// actor が script に渡して結果を待っている要求の数。
    pub in_flight: usize,
    /// actor が完了（成功・失敗・破棄）させた要求の累計。
    pub completed: u64,
}

/// ScriptRuntime actor の handle。`Clone + Send + Sync`。
#[derive(Clone)]
pub struct ScriptRuntimeHandle {
    inner: Arc<Inner>,
}

struct Inner {
    tx: mpsc::Sender<ActorRequest>,
    stats: watch::Receiver<RuntimeStats>,
    control: Mutex<Control>,
}

struct Control {
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl ScriptRuntimeHandle {
    /// actor thread を起こし、`source` を評価して入口の関数を確かめるまで待つ。
    ///
    /// 同期の関数で、評価が終わるまで呼び出し元の thread を塞ぐ。
    pub fn start(source: ScriptSource) -> Result<Self, ScriptError> {
        let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (stats_tx, stats_rx) = watch::channel(RuntimeStats::default());
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("celeris-script".into())
            .spawn(move || actor::run(source, rx, shutdown_rx, stats_tx, ready_tx))
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
                stats: stats_rx,
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
            Ok(Ok(Ok(output))) => Ok(ScriptReply { request_id, output }),
            Ok(Ok(Err(err))) => Err(err),
            Ok(Err(_)) => Err(ScriptError::Terminated),
            Err(_) => Err(ScriptError::Timeout),
        }
    }

    /// 監視用の数値の今の値。
    pub fn stats(&self) -> RuntimeStats {
        *self.inner.stats.borrow()
    }

    /// 監視用の数値の変化を待つ受け手。
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
