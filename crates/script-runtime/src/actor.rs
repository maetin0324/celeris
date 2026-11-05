//! ScriptRuntime actor（境界 ADR 2026-11-02 (b)(c)）。
//!
//! 専用の `std::thread` 1 本が `JsRuntime` を生成・所有・破棄する。その thread の中で current-thread の
//! tokio runtime が 1 つの `poll_fn` を回し、次を同じ poll の中で繰り返す。
//! 1. shutdown の合図を見る。
//! 2. restart の要求を見て、来ていれば generation を作り直す。
//! 3. 未完の要求が [`MAX_IN_FLIGHT`] 未満なら queue から要求を取り出し、script の入口を呼ぶ。
//! 4. `JsRuntime::poll_event_loop` で event loop を進める（op・microtask・Promise の解決）。
//! 5. 未完の要求の結果を見て、解決したものを oneshot で返す。受け手が落ちた要求（cancel・timeout）は結果を捨てる。
//!
//! capability を 1 つ以上持つ要求（op で読み出す要求）は、isolate の中で同時に 1 件までしか走らせない
//! （境界 ADR の付記 S4-13 の 1。要求ごとに V8 context を分けない代わり）。走っている間に来た別の capability 付きの
//! 要求は `held` に置き、前の要求が終わってから来た順に始める。capability の無い要求は並行して走る。
//!
//! V8 に入る 2 か所（3 の入口の呼び出しと 4 の event loop）は watchdog に deadline を渡してから入る。
//! watchdog が V8 を止めた・heap 上限で止めた generation は再利用せず作り直す（理由は進捗
//! `agent-docs/progress/2026-11-02-ts-control-layer-s1/runtime-guard.md`。試験は下の `tests`）。
//!
//! V8 の値（`v8::Global` 等）はこの thread から出さない。

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use deno_core::error::JsError;
use deno_core::futures::FutureExt;
use deno_core::{JsRuntime, ModuleSpecifier, PollEventLoopOptions, RuntimeOptions, serde_v8, v8};
use tokio::sync::{mpsc, oneshot, watch};
use ulid::Ulid;

use crate::dto::{HostError, HostErrorCode};
use crate::error::ScriptError;
use crate::handle::{
    DISPATCH_FUNCTION, HeapStats, HostSink, MAX_IN_FLIGHT, RuntimeLimits, RuntimeStats,
    ScriptSource,
};
use crate::loader::BundleLoader;
use crate::ops::{self, Grant, GrantGuard, GrantSpec, Grants};
use crate::watchdog::Watchdog;

type Reply = oneshot::Sender<Result<(u64, serde_json::Value), ScriptError>>;

/// handle から actor へ送る要求。
pub(crate) struct ActorRequest {
    pub(crate) request_id: Ulid,
    pub(crate) domain: String,
    pub(crate) input: serde_json::Value,
    /// 要求に結び付いた scope（op が読んでよい task の id）。
    pub(crate) scope_task_ids: BTreeSet<String>,
    /// capability 付きの op の送り先（付記 S4-12）。
    pub(crate) host_sink: Option<HostSink>,
    /// 呼び出し側の deadline。watchdog が V8 を止める時刻。
    pub(crate) deadline: Instant,
    pub(crate) reply: Reply,
}

/// handle から actor への制御（queue の満杯に関係なく届く）。
pub(crate) enum Control {
    /// generation を作り直す。新しい generation の番号を返す。
    Restart {
        done: oneshot::Sender<Result<u64, ScriptError>>,
    },
    /// 今の generation の V8 heap の統計を読む（長時間の計測。P1-I）。generation が無ければ None。
    HeapStats {
        done: oneshot::Sender<Option<HeapStats>>,
    },
}

/// actor が受け取る channel の束。
pub(crate) struct Channels {
    pub(crate) rx: mpsc::Receiver<ActorRequest>,
    pub(crate) ctl: mpsc::UnboundedReceiver<Control>,
    pub(crate) shutdown: oneshot::Receiver<()>,
    pub(crate) stats: watch::Sender<RuntimeStats>,
}

/// actor の起動の結果。inspector を有効にしたかを handle に伝える（常に false。`runtime_no_inspector`）。
pub(crate) struct Started {
    pub(crate) inspector: bool,
    /// isolate に登録した op の名前（`runtime_no_ambient_io` が fs・network 等の op が無いことを確かめる）。
    pub(crate) op_names: Vec<String>,
}

type CallFuture = Pin<Box<dyn Future<Output = Result<v8::Global<v8::Value>, ScriptError>>>>;

/// script に渡して結果を待っている要求。
struct Pending {
    #[allow(dead_code)]
    request_id: Ulid,
    deadline: Instant,
    future: CallFuture,
    reply: Reply,
    /// capability を持つ要求（isolate の中で排他）。
    exclusive: bool,
    /// 要求が終わったら op の権限表から消す。
    _grant: GrantGuard,
}

/// 1 つの isolate とその上で評価した script。作り直すたびに番号が 1 増える。
///
/// field は宣言順に drop されるので、`dispatch`（V8 の handle）を `runtime` より先に置く。
struct Generation {
    dispatch: v8::Global<v8::Function>,
    runtime: JsRuntime,
    /// 要求 token → 権限。op と共有する。
    grants: Grants,
    /// near_heap_limit_callback が立てる。
    heap_hit: Arc<AtomicBool>,
}

static V8_PLATFORM: Once = Once::new();

/// V8 platform を process で 1 回だけ初期化する。
fn init_platform() {
    V8_PLATFORM.call_once(|| JsRuntime::init_platform(None));
}

/// heap 上限で止めた後、V8 が実行を畳むまでの猶予として上限を何倍にするか（generation ごとに 1 回）。
const HEAP_GRACE_FACTOR: usize = 2;

/// bundle の評価（top-level）の上限。超えたら watchdog が止め、起動・作り直しは失敗する。
const LOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// 静的 bootstrap（Deno.* の除去・時計と乱数の禁止・celeris:host への op の受け渡し）。
const BOOTSTRAP_JS: &str = include_str!("bootstrap.js");

/// inspector は有効にしない（境界 ADR (c)。debugger の接続口を作らない）。
pub(crate) const INSPECTOR: bool = false;

/// generation 1 つ分の JsRuntime の設定。
fn runtime_options(
    limits: &RuntimeLimits,
    loader: BundleLoader,
    grants: &Grants,
) -> RuntimeOptions {
    let params = v8::CreateParams::default().heap_limits(0, limits.heap_limit_bytes);
    RuntimeOptions {
        create_params: Some(params),
        module_loader: Some(Rc::new(loader)),
        extensions: vec![ops::extension(Rc::clone(grants))],
        inspector: INSPECTOR,
        ..Default::default()
    }
}

/// isolate を作り、heap 上限と watchdog を付けて script を評価する。
fn new_generation(
    source: &ScriptSource,
    limits: &RuntimeLimits,
    watchdog: &Watchdog,
) -> Result<Generation, ScriptError> {
    let specifier = ModuleSpecifier::parse(&source.name).map_err(|e| ScriptError::Exception {
        message: format!("invalid bundle specifier {:?}: {e}", source.name),
    })?;
    let loader = BundleLoader::new(specifier.clone(), source.code.clone()).map_err(|e| {
        ScriptError::Exception {
            message: e.to_string(),
        }
    })?;
    let grants: Grants = Rc::new(RefCell::new(HashMap::new()));
    let mut runtime = JsRuntime::try_new(runtime_options(limits, loader, &grants))
        .map_err(|_| ScriptError::Terminated)?;
    let isolate = runtime.v8_isolate().thread_safe_handle();
    let heap_hit = Arc::new(AtomicBool::new(false));
    {
        let heap_hit = Arc::clone(&heap_hit);
        let isolate = isolate.clone();
        // 上限に近づいたら実行を止め、畳むための猶予だけ上限を広げる。猶予の中でも 1 回の巨大な割り当ては
        // 止められず V8 は process を落とす（runtime_oom_child_process が子 process で確かめる）。
        // 猶予は generation ごとに 1 回だけ。2 回目は上限を広げず、V8 が process を落とす。
        runtime.add_near_heap_limit_callback(move |current, _initial| {
            if heap_hit.swap(true, Ordering::SeqCst) {
                return current;
            }
            isolate.terminate_execution();
            current.saturating_mul(HEAP_GRACE_FACTOR)
        });
    }
    watchdog.set_isolate(isolate);
    watchdog.arm(Instant::now() + LOAD_TIMEOUT);
    let loaded = load(&mut runtime, &specifier, source);
    if watchdog.disarm() {
        return Err(ScriptError::Timeout);
    }
    let dispatch = loaded?;
    Ok(Generation {
        dispatch,
        runtime,
        grants,
        heap_hit,
    })
}

/// actor thread の本体。`ready` には起動の成否を 1 回だけ送る。
pub(crate) fn run(
    source: ScriptSource,
    limits: RuntimeLimits,
    channels: Channels,
    ready: std::sync::mpsc::SyncSender<Result<Started, ScriptError>>,
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
    let watchdog = match Watchdog::spawn() {
        Ok(w) => w,
        Err(_) => {
            let _ = ready.send(Err(ScriptError::Terminated));
            return;
        }
    };
    init_platform();
    // JsRuntime は tokio runtime の文脈の中で作り、同じ thread で破棄する。
    let _guard = tokio_rt.enter();
    let generation = match new_generation(&source, &limits, &watchdog) {
        Ok(g) => g,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    let op_names = generation
        .runtime
        .op_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    if ready
        .send(Ok(Started {
            inspector: INSPECTOR,
            op_names,
        }))
        .is_err()
    {
        return;
    }

    let Channels {
        rx,
        ctl,
        shutdown,
        stats,
    } = channels;
    let mut actor = Actor {
        source,
        limits,
        watchdog,
        generation: Some(generation),
        pending: Vec::new(),
        held: VecDeque::new(),
        rx,
        ctl,
        shutdown,
        stats_tx: stats,
        stats: RuntimeStats::default(),
    };
    tokio_rt.block_on(std::future::poll_fn(|cx| actor.drive(cx)));
    actor.finish();
}

struct Actor {
    source: ScriptSource,
    limits: RuntimeLimits,
    watchdog: Watchdog,
    /// 作り直しの途中（と作り直しに失敗した後）だけ None。
    generation: Option<Generation>,
    pending: Vec<Pending>,
    /// 別の capability 付きの要求が走っているので始めずに置いている要求（来た順）。
    held: VecDeque<ActorRequest>,
    rx: mpsc::Receiver<ActorRequest>,
    ctl: mpsc::UnboundedReceiver<Control>,
    shutdown: oneshot::Receiver<()>,
    stats_tx: watch::Sender<RuntimeStats>,
    stats: RuntimeStats,
}

/// generation を作り直す理由。
enum Recycle {
    /// handle の restart。
    Restart,
    /// watchdog が V8 を止めた。`culprit` は deadline を過ぎた要求で、Timeout を返す。
    Watchdog { culprit: Option<Pending> },
    /// heap 上限で止めた。`culprit` が分かればその要求に、分からなければ未完の全部に HeapLimit を返す。
    Heap { culprit: Option<Pending> },
}

impl Actor {
    fn drive(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        loop {
            // shutdown の合図（送り手が落ちた場合も止める）。
            if Pin::new(&mut self.shutdown).poll(cx).is_ready() {
                return Poll::Ready(());
            }
            let mut progressed = false;

            // restart の要求。
            while let Poll::Ready(Some(control)) = self.ctl.poll_recv(cx) {
                match control {
                    Control::Restart { done } => {
                        let result = self.recycle(Recycle::Restart);
                        let failed = result.is_err();
                        let _ = done.send(result);
                        if failed {
                            return Poll::Ready(());
                        }
                    }
                    Control::HeapStats { done } => {
                        let _ = done.send(self.heap_stats());
                    }
                }
                progressed = true;
            }

            // 置いていた capability 付きの要求を、走っている排他の要求が無ければ 1 件始める。
            match self.start_held() {
                Ok(true) => progressed = true,
                Ok(false) => {}
                Err(_) => return Poll::Ready(()),
            }

            // 要求を取り出して script に渡す。未完が上限なら queue に残す（backpressure）。
            while self.pending.len() + self.held.len() < MAX_IN_FLIGHT {
                match self.rx.poll_recv(cx) {
                    Poll::Ready(Some(req)) => {
                        progressed = true;
                        if req.reply.is_closed() {
                            // 送った側が既に待つのをやめている。
                            self.stats.completed += 1;
                            self.stats.discarded += 1;
                            continue;
                        }
                        if self.needs_exclusive(&req)
                            && (self.exclusive_running() || !self.held.is_empty())
                        {
                            self.held.push_back(req);
                            continue;
                        }
                        if self.start(req).is_err() {
                            return Poll::Ready(());
                        }
                    }
                    // 全 handle が落ちた。shutdown の合図も同時に来る。
                    Poll::Ready(None) => return Poll::Ready(()),
                    Poll::Pending => break,
                }
            }

            if self.poll_event_loop(cx).is_err() {
                return Poll::Ready(());
            }
            if self.collect(cx) {
                progressed = true;
            }

            self.publish();
            if !progressed {
                return Poll::Pending;
            }
        }
    }

    /// 要求が capability を持つ（排他にする）か。有効な capability は宣言 ∩ 許可表。
    fn needs_exclusive(&self, req: &ActorRequest) -> bool {
        ops::domain_capabilities(&req.domain)
            .iter()
            .any(|c| self.source.capabilities.contains(c))
    }

    /// capability 付きの要求が走っているか。
    fn exclusive_running(&self) -> bool {
        self.pending.iter().any(|p| p.exclusive)
    }

    /// 置いていた要求を 1 件始める。始めた（または待つのをやめた要求を捨てた）なら true。
    fn start_held(&mut self) -> Result<bool, ScriptError> {
        let mut progressed = false;
        // 待つのをやめた要求（cancel・timeout）は捨てる。
        let before = self.held.len();
        self.held.retain(|r| !r.reply.is_closed());
        let dropped = before - self.held.len();
        if dropped > 0 {
            self.stats.completed += dropped as u64;
            self.stats.discarded += dropped as u64;
            progressed = true;
        }
        if self.exclusive_running() {
            return Ok(progressed);
        }
        match self.held.pop_front() {
            Some(req) => {
                self.start(req)?;
                Ok(true)
            }
            None => Ok(progressed),
        }
    }

    /// script の入口を呼ぶ。同期の部分はこの中で走るので watchdog で守る。
    /// Err は generation を作り直せなかったこと（actor を止める）。
    fn start(&mut self, req: ActorRequest) -> Result<(), ScriptError> {
        let ActorRequest {
            request_id,
            domain,
            input,
            scope_task_ids,
            host_sink,
            deadline,
            reply,
        } = req;
        let generation = self.generation.as_mut().ok_or(ScriptError::Terminated)?;
        let token = request_id.to_string();
        let (grant, end) = Grant::new(GrantSpec {
            declared: &self.source.capabilities,
            domain: &domain,
            scope_task_ids: &scope_task_ids,
            request_id,
            deadline,
            sink: host_sink,
        });
        let exclusive = grant.is_exclusive();
        let grant = GrantGuard::insert(&generation.grants, token.clone(), grant, end);
        self.watchdog.arm(deadline);
        let future = call_dispatch(generation, &domain, &input, &token);
        let fired = self.watchdog.disarm();
        let heap = generation.heap_hit.load(Ordering::SeqCst);
        let pending = Pending {
            request_id,
            deadline,
            future,
            reply,
            exclusive,
            _grant: grant,
        };
        if heap {
            self.recycle(Recycle::Heap {
                culprit: Some(pending),
            })?;
        } else if fired {
            self.recycle(Recycle::Watchdog {
                culprit: Some(pending),
            })?;
        } else {
            self.pending.push(pending);
        }
        Ok(())
    }

    /// event loop を進める。Ready(Ok) は「待つ op が無い」だけで、actor は止めない。
    fn poll_event_loop(&mut self, cx: &mut Context<'_>) -> Result<(), ScriptError> {
        // deadline を過ぎた要求は先に Timeout で返す（その要求のために他の要求の実行を止めない）。
        let now = Instant::now();
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].deadline <= now {
                let p = self.pending.swap_remove(i);
                self.stats.completed += 1;
                self.publish();
                let _ = p.reply.send(Err(ScriptError::Timeout));
            } else {
                i += 1;
            }
        }
        let earliest = self
            .pending
            .iter()
            .enumerate()
            .min_by_key(|(_, p)| p.deadline)
            .map(|(i, p)| (i, p.deadline));
        let generation = self.generation.as_mut().ok_or(ScriptError::Terminated)?;
        if let Some((_, deadline)) = earliest {
            self.watchdog.arm(deadline);
        }
        let result = generation
            .runtime
            .poll_event_loop(cx, PollEventLoopOptions::default());
        let fired = earliest.is_some() && self.watchdog.disarm();
        let heap = generation.heap_hit.load(Ordering::SeqCst);
        if heap {
            return self.recycle(Recycle::Heap { culprit: None }).map(|_| ());
        }
        if fired {
            let culprit = earliest.map(|(i, _)| self.pending.swap_remove(i));
            return self.recycle(Recycle::Watchdog { culprit }).map(|_| ());
        }
        if let Poll::Ready(Err(err)) = result {
            // event loop の誤りは未完の要求全部に返す（どの要求の誤りか分けられない）。
            let message = err.to_string();
            for p in self.pending.drain(..) {
                let _ = p.reply.send(Err(ScriptError::Exception {
                    message: message.clone(),
                }));
                self.stats.completed += 1;
            }
        }
        Ok(())
    }

    /// 結果を集める。何か終わったら true。
    fn collect(&mut self, cx: &mut Context<'_>) -> bool {
        let mut progressed = false;
        let number = self.stats.generation;
        let mut i = 0;
        while i < self.pending.len() {
            let p = &mut self.pending[i];
            if p.reply.poll_closed(cx).is_ready() {
                // cancel・timeout: 結果を捨てる。
                self.pending.swap_remove(i);
                self.stats.completed += 1;
                self.stats.discarded += 1;
                progressed = true;
                continue;
            }
            match p.future.as_mut().poll(cx) {
                Poll::Ready(result) => {
                    let p = self.pending.swap_remove(i);
                    let out = match self.generation.as_mut() {
                        Some(g) => result.and_then(|value| to_json(&mut g.runtime, value)),
                        None => Err(ScriptError::GenerationGone),
                    };
                    self.stats.completed += 1;
                    // 呼び出し側が結果を受け取った時点で stats が完了を映しているよう、返す前に公開する。
                    self.publish();
                    let _ = p.reply.send(out.map(|v| (number, v)));
                    progressed = true;
                }
                Poll::Pending => i += 1,
            }
        }
        progressed
    }

    /// 今の generation を捨てて作り直す。古い generation の未完の要求には結果を渡さない。
    /// 新しい generation の番号を返す。作り直せなければ Err（actor を止める）。
    fn recycle(&mut self, why: Recycle) -> Result<u64, ScriptError> {
        let (culprit, culprit_err, rest_err) = match why {
            Recycle::Restart => (
                None,
                ScriptError::GenerationGone,
                ScriptError::GenerationGone,
            ),
            Recycle::Watchdog { culprit } => {
                self.stats.watchdog_terminations += 1;
                (culprit, ScriptError::Timeout, ScriptError::GenerationGone)
            }
            Recycle::Heap { culprit } => {
                self.stats.heap_limit_hits += 1;
                let rest = if culprit.is_some() {
                    ScriptError::GenerationGone
                } else {
                    ScriptError::HeapLimit
                };
                (culprit, ScriptError::HeapLimit, rest)
            }
        };
        // 返事は stats を公開してから送る（受け取った側が見る stats に完了が映っているように）。
        let mut replies = Vec::with_capacity(self.pending.len() + 1);
        if let Some(p) = culprit {
            replies.push((p.reply, culprit_err));
            self.stats.completed += 1;
        }
        // 未完の future は古い isolate の Promise を持つので、isolate より先に落とす。
        for p in self.pending.drain(..) {
            replies.push((p.reply, rest_err.clone()));
            self.stats.completed += 1;
        }
        // isolate は作成と逆順に drop しなければならないので、古い方を落としてから新しい方を作る。
        drop(self.generation.take());
        let result = match new_generation(&self.source, &self.limits, &self.watchdog) {
            Ok(next) => {
                self.generation = Some(next);
                self.stats.generation += 1;
                self.stats.restarts += 1;
                Ok(self.stats.generation)
            }
            Err(err) => Err(err),
        };
        self.publish();
        for (reply, err) in replies {
            let _ = reply.send(Err(err));
        }
        result
    }

    /// shutdown: 未完と queue に残った要求へ Terminated を返し、isolate を落とす。
    fn finish(mut self) {
        self.rx.close();
        for p in self.pending.drain(..) {
            let _ = p.reply.send(Err(ScriptError::Terminated));
            self.stats.completed += 1;
        }
        for req in self.held.drain(..) {
            let _ = req.reply.send(Err(ScriptError::Terminated));
            self.stats.completed += 1;
        }
        while let Ok(req) = self.rx.try_recv() {
            let _ = req.reply.send(Err(ScriptError::Terminated));
            self.stats.completed += 1;
        }
        self.ctl.close();
        while let Ok(control) = self.ctl.try_recv() {
            match control {
                Control::Restart { done } => {
                    let _ = done.send(Err(ScriptError::Terminated));
                }
                Control::HeapStats { done } => {
                    let _ = done.send(None);
                }
            }
        }
        self.publish();
        drop(self.generation.take());
    }

    /// 今の generation の isolate の heap の統計（V8 の `GetHeapStatistics`。actor の thread で読む）。
    fn heap_stats(&mut self) -> Option<HeapStats> {
        let generation = self.generation.as_mut()?;
        let s = generation.runtime.v8_isolate().get_heap_statistics();
        Some(HeapStats {
            used_heap_size: s.used_heap_size() as u64,
            total_heap_size: s.total_heap_size() as u64,
            total_physical_size: s.total_physical_size() as u64,
            heap_size_limit: s.heap_size_limit() as u64,
            external_memory: s.external_memory() as u64,
            malloced_memory: s.malloced_memory() as u64,
            number_of_native_contexts: s.number_of_native_contexts() as u64,
            generation: self.stats.generation,
        })
    }

    fn publish(&mut self) {
        self.stats.in_flight = self.pending.len();
        let next = self.stats;
        self.stats_tx.send_if_modified(|s| {
            if *s == next {
                false
            } else {
                *s = next;
                true
            }
        });
    }
}

/// bootstrap を評価し、bundle を main module として load・評価して、入口の関数を取り出す。
fn load(
    runtime: &mut JsRuntime,
    specifier: &ModuleSpecifier,
    source: &ScriptSource,
) -> Result<v8::Global<v8::Function>, ScriptError> {
    runtime
        .execute_script("celeris:bootstrap.js", BOOTSTRAP_JS)
        .map_err(|e| js_error_to_script_error(&e))?;
    // loader は同期で返すので、load と評価は外の event loop を待たずに終わる。
    deno_core::futures::executor::block_on(async {
        let id = runtime
            .load_main_es_module_from_code(specifier, source.code.clone())
            .await
            .map_err(core_error)?;
        let evaluated = runtime.mod_evaluate(id);
        runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await
            .map_err(core_error)?;
        evaluated.await.map_err(core_error)
    })?;
    deno_core::scope!(scope, runtime);
    let global = scope.get_current_context().global(scope);
    let key = v8::String::new(scope, DISPATCH_FUNCTION).ok_or(ScriptError::Terminated)?;
    let value = global.get(scope, key.into()).ok_or_else(missing_dispatch)?;
    let function = v8::Local::<v8::Function>::try_from(value).map_err(|_| missing_dispatch())?;
    Ok(v8::Global::new(scope, function))
}

/// load・評価の誤り。JS の例外なら [`js_error_to_script_error`] で写す。
fn core_error(e: deno_core::error::CoreError) -> ScriptError {
    match *e.0 {
        deno_core::error::CoreErrorKind::Js(js) => js_error_to_script_error(&js),
        other => ScriptError::Exception {
            message: other.to_string(),
        },
    }
}

/// JS の例外を写す。`HostError { code, message }` は code の種類へ、それ以外は `Exception`。
pub(crate) fn js_error_to_script_error(e: &JsError) -> ScriptError {
    if e.name.as_deref() == Some("HostError") {
        let code = e
            .additional_properties
            .iter()
            .find(|(k, _)| k == "code")
            .and_then(|(_, v)| {
                serde_json::from_value::<HostErrorCode>(serde_json::Value::String(v.clone())).ok()
            });
        if let Some(code) = code {
            return HostError::new(code, e.message.clone().unwrap_or_default()).into();
        }
    }
    ScriptError::Exception {
        message: e.exception_message.clone(),
    }
}

fn missing_dispatch() -> ScriptError {
    ScriptError::InvalidOutput {
        reason: format!("script did not define globalThis.{DISPATCH_FUNCTION} as a function"),
    }
}

/// script の入口を呼ぶ。入力の変換に失敗したら、その場で解決する future を返す。
fn call_dispatch(
    generation: &mut Generation,
    domain: &str,
    input: &serde_json::Value,
    token: &str,
) -> CallFuture {
    let runtime = &mut generation.runtime;
    deno_core::scope!(scope, runtime);
    let args = (|| {
        let domain = serde_v8::to_v8(scope, domain).ok()?;
        let input = serde_v8::to_v8(scope, input).ok()?;
        let ctx = serde_v8::to_v8(scope, serde_json::json!({ "token": token })).ok()?;
        Some([
            v8::Global::new(scope, domain),
            v8::Global::new(scope, input),
            v8::Global::new(scope, ctx),
        ])
    })();
    match args {
        Some(args) => Box::pin(
            JsRuntime::scoped_call_with_args(scope, &generation.dispatch, &args)
                .map(|r| r.map_err(|e| js_error_to_script_error(&e))),
        ),
        // 入力が V8 の値にできない。
        None => Box::pin(std::future::ready(Err(ScriptError::Exception {
            message: "failed to convert input to a V8 value".into(),
        }))),
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

#[cfg(test)]
mod tests {
    //! terminate 後の isolate を再利用するかの判断の根拠になる試験（進捗 runtime-guard.md）。
    use super::*;
    use std::time::Duration;

    /// watchdog で同期ループを止めた isolate は、`cancel_terminate_execution` の後なら技術的には再利用できる。
    /// ただし止める前に script が書き換えた global の状態が途中のまま残る（不変条件を保てない）。
    #[test]
    fn terminate_then_reuse_keeps_half_written_state() {
        init_platform();
        let watchdog = Watchdog::spawn().expect("watchdog");
        let mut runtime = JsRuntime::new(RuntimeOptions::default());
        watchdog.set_isolate(runtime.v8_isolate().thread_safe_handle());
        // 2 つの global を対で書く途中で無限ループに入る script。
        watchdog.arm(Instant::now() + Duration::from_millis(100));
        let r = runtime.execute_script(
            "celeris:test/spin.js",
            "globalThis.pairA = 1; for (;;) {} globalThis.pairB = 1;",
        );
        assert!(watchdog.disarm(), "watchdog must fire");
        assert!(r.is_err());
        runtime.v8_isolate().cancel_terminate_execution();
        let v = runtime
            .execute_script(
                "celeris:test/probe.js",
                "JSON.stringify([globalThis.pairA ?? null, globalThis.pairB ?? null])",
            )
            .expect("reuse after cancel_terminate_execution");
        deno_core::scope!(scope, &mut runtime);
        let local = v8::Local::new(scope, v);
        let s = local.to_rust_string_lossy(scope);
        // pairA だけが書かれ pairB は無い: 半端な状態が残る。
        assert_eq!(s, "[1,null]");
    }
}
