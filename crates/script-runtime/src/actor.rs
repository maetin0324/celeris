//! ScriptRuntime actor（境界 ADR 2026-11-02 (b)）。
//!
//! 専用の `std::thread` 1 本が `JsRuntime` を生成・所有・破棄する。その thread の中で current-thread の
//! tokio runtime が 1 つの `poll_fn` を回し、次を同じ poll の中で繰り返す。
//! 1. shutdown の合図を見る。
//! 2. 未完の要求が [`MAX_IN_FLIGHT`] 未満なら queue から要求を取り出し、script の入口を呼ぶ。
//! 3. `JsRuntime::poll_event_loop` で event loop を進める（op・microtask・Promise の解決）。
//! 4. 未完の要求の結果を見て、解決したものを oneshot で返す。受け手が落ちた要求（cancel・timeout）は結果を捨てる。
//!
//! V8 の値（`v8::Global` 等）はこの thread から出さない。

use std::future::Future;
use std::pin::Pin;
use std::sync::Once;
use std::task::{Context, Poll};
use std::time::Instant;

use deno_core::futures::FutureExt;
use deno_core::{JsRuntime, PollEventLoopOptions, RuntimeOptions, serde_v8, v8};
use tokio::sync::{mpsc, oneshot, watch};
use ulid::Ulid;

use crate::error::ScriptError;
use crate::handle::{DISPATCH_FUNCTION, MAX_IN_FLIGHT, RuntimeStats, ScriptSource};

/// handle から actor へ送る要求。
pub(crate) struct ActorRequest {
    pub(crate) request_id: Ulid,
    pub(crate) domain: String,
    pub(crate) input: serde_json::Value,
    /// 呼び出し側の deadline。watchdog（V8 の中断）が読む。
    #[allow(dead_code)]
    pub(crate) deadline: Instant,
    pub(crate) reply: oneshot::Sender<Result<serde_json::Value, ScriptError>>,
}

type CallFuture = Pin<Box<dyn Future<Output = Result<v8::Global<v8::Value>, ScriptError>>>>;

/// script に渡して結果を待っている要求。
struct Pending {
    #[allow(dead_code)]
    request_id: Ulid,
    future: CallFuture,
    reply: oneshot::Sender<Result<serde_json::Value, ScriptError>>,
}

static V8_PLATFORM: Once = Once::new();

/// V8 platform を process で 1 回だけ初期化する。
fn init_platform() {
    V8_PLATFORM.call_once(|| JsRuntime::init_platform(None));
}

/// actor thread の本体。`ready` には起動の成否を 1 回だけ送る。
pub(crate) fn run(
    source: ScriptSource,
    mut rx: mpsc::Receiver<ActorRequest>,
    mut shutdown: oneshot::Receiver<()>,
    stats: watch::Sender<RuntimeStats>,
    ready: std::sync::mpsc::SyncSender<Result<(), ScriptError>>,
) {
    let tokio_rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => {
            let _ = ready.send(Err(ScriptError::Terminated));
            return;
        }
    };
    init_platform();
    // JsRuntime は tokio runtime の文脈の中で作り、同じ thread で破棄する。
    let _guard = tokio_rt.enter();
    let mut runtime = match JsRuntime::try_new(RuntimeOptions::default()) {
        Ok(rt) => rt,
        Err(_) => {
            let _ = ready.send(Err(ScriptError::Terminated));
            return;
        }
    };
    let dispatch = match load(&mut runtime, source) {
        Ok(f) => f,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        return;
    }

    let mut pending: Vec<Pending> = Vec::new();
    let mut completed: u64 = 0;
    tokio_rt.block_on(std::future::poll_fn(|cx| {
        drive(
            cx,
            &mut runtime,
            &dispatch,
            &mut rx,
            &mut shutdown,
            &mut pending,
            &mut completed,
            &stats,
        )
    }));

    // shutdown: 未完と queue に残った要求へ Terminated を返す。
    rx.close();
    for p in pending.drain(..) {
        let _ = p.reply.send(Err(ScriptError::Terminated));
        completed += 1;
    }
    while let Ok(req) = rx.try_recv() {
        let _ = req.reply.send(Err(ScriptError::Terminated));
        completed += 1;
    }
    publish(&stats, 0, completed);
    drop(dispatch);
    drop(runtime);
}

/// script を評価し、入口の関数を取り出す。
fn load(
    runtime: &mut JsRuntime,
    source: ScriptSource,
) -> Result<v8::Global<v8::Function>, ScriptError> {
    runtime
        .execute_script(source.name, source.code)
        .map_err(|e| ScriptError::Exception {
            message: e.exception_message.clone(),
        })?;
    deno_core::scope!(scope, runtime);
    let global = scope.get_current_context().global(scope);
    let key = v8::String::new(scope, DISPATCH_FUNCTION).ok_or(ScriptError::Terminated)?;
    let value = global.get(scope, key.into()).ok_or_else(missing_dispatch)?;
    let function = v8::Local::<v8::Function>::try_from(value).map_err(|_| missing_dispatch())?;
    Ok(v8::Global::new(scope, function))
}

fn missing_dispatch() -> ScriptError {
    ScriptError::InvalidOutput {
        reason: format!("script did not define globalThis.{DISPATCH_FUNCTION} as a function"),
    }
}

#[allow(clippy::too_many_arguments)]
fn drive(
    cx: &mut Context<'_>,
    runtime: &mut JsRuntime,
    dispatch: &v8::Global<v8::Function>,
    rx: &mut mpsc::Receiver<ActorRequest>,
    shutdown: &mut oneshot::Receiver<()>,
    pending: &mut Vec<Pending>,
    completed: &mut u64,
    stats: &watch::Sender<RuntimeStats>,
) -> Poll<()> {
    loop {
        // shutdown の合図（送り手が落ちた場合も止める）。
        if Pin::new(&mut *shutdown).poll(cx).is_ready() {
            return Poll::Ready(());
        }

        // 要求を取り出して script に渡す。未完が上限なら queue に残す（backpressure）。
        let mut progressed = false;
        while pending.len() < MAX_IN_FLIGHT {
            match rx.poll_recv(cx) {
                Poll::Ready(Some(req)) => {
                    if req.reply.is_closed() {
                        // 送った側が既に待つのをやめている。
                        *completed += 1;
                        continue;
                    }
                    pending.push(start(runtime, dispatch, req));
                    progressed = true;
                }
                // 全 handle が落ちた。shutdown の合図も同時に来る。
                Poll::Ready(None) => return Poll::Ready(()),
                Poll::Pending => break,
            }
        }

        // event loop を進める。Ready(Ok) は「待つ op が無い」だけで、actor は止めない。
        if let Poll::Ready(Err(err)) = runtime.poll_event_loop(cx, PollEventLoopOptions::default())
        {
            // event loop の誤りは未完の要求全部に返す（どの要求の誤りか分けられない）。
            let message = err.to_string();
            for p in pending.drain(..) {
                let _ = p.reply.send(Err(ScriptError::Exception {
                    message: message.clone(),
                }));
                *completed += 1;
            }
        }

        // 結果を集める。
        let mut i = 0;
        while i < pending.len() {
            let p = &mut pending[i];
            if p.reply.poll_closed(cx).is_ready() {
                // cancel・timeout: 結果を捨てる。
                pending.swap_remove(i);
                *completed += 1;
                progressed = true;
                continue;
            }
            match p.future.as_mut().poll(cx) {
                Poll::Ready(result) => {
                    let p = pending.swap_remove(i);
                    let out = result.and_then(|value| to_json(runtime, value));
                    let _ = p.reply.send(out);
                    *completed += 1;
                    progressed = true;
                }
                Poll::Pending => i += 1,
            }
        }

        publish(stats, pending.len(), *completed);
        if !progressed {
            return Poll::Pending;
        }
    }
}

/// script の入口を呼ぶ。入力の変換に失敗したら、その場で解決する future を返す。
fn start(
    runtime: &mut JsRuntime,
    dispatch: &v8::Global<v8::Function>,
    req: ActorRequest,
) -> Pending {
    let ActorRequest {
        request_id,
        domain,
        input,
        reply,
        ..
    } = req;
    deno_core::scope!(scope, runtime);
    let args = (|| {
        let domain = serde_v8::to_v8(scope, &domain).ok()?;
        let input = serde_v8::to_v8(scope, &input).ok()?;
        Some([
            v8::Global::new(scope, domain),
            v8::Global::new(scope, input),
        ])
    })();
    let future: CallFuture = match args {
        Some(args) => Box::pin(
            JsRuntime::scoped_call_with_args(scope, dispatch, &args).map(|r| {
                r.map_err(|e| ScriptError::Exception {
                    message: e.exception_message.clone(),
                })
            }),
        ),
        // 入力が V8 の値にできない。
        None => Box::pin(std::future::ready(Err(ScriptError::Exception {
            message: "failed to convert input to a V8 value".into(),
        }))),
    };
    Pending {
        request_id,
        future,
        reply,
    }
}

/// script の返した値を JSON に写す。`undefined` と JSON にできない値は InvalidOutput。
fn to_json(
    runtime: &mut JsRuntime,
    value: v8::Global<v8::Value>,
) -> Result<serde_json::Value, ScriptError> {
    deno_core::scope!(scope, runtime);
    let local = v8::Local::new(scope, value);
    if local.is_undefined() {
        return Err(ScriptError::InvalidOutput {
            reason: "script returned undefined (use null)".into(),
        });
    }
    serde_v8::from_v8::<serde_json::Value>(scope, local).map_err(|e| ScriptError::InvalidOutput {
        reason: e.to_string(),
    })
}

fn publish(stats: &watch::Sender<RuntimeStats>, in_flight: usize, completed: u64) {
    stats.send_if_modified(|s| {
        let next = RuntimeStats {
            in_flight,
            completed,
        };
        if *s == next {
            false
        } else {
            *s = next;
            true
        }
    });
}
