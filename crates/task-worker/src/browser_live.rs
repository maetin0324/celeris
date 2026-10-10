//! ADR-0099 D3 / ADR-0100 / ADR-0080 H3: browser 実行に P3-C の制御 gate と live event の出口を挟む。
//!
//! - `ControlGate`: agent の browser 操作の直前に問い合わせる。`AgentRunning` 以外では新しい操作を出さない。
//!   pause は実行中の操作の完了を待って `Paused` に収束する。lease 切れ・切断で自動再開しない。
//!   `Stopped` は cancel と同じ経路（`SessionCloser`）で session を閉じる。
//! - `LiveEmitter`: status/tabs/url/console だけを、`task_core::browser_live::persistable` で
//!   scrub してから `LiveSink` に渡す。frame と tab title は型として持てない。
//!   auth section（credential 注入・identity 復元）の間は捨てる（溜めない）。
//!
//! - `FrameRelay` / `LiveFrameRegistration`（付記 2026-10-10b）: launcher の v8 frame 接続から読んだ
//!   画像を session に結び付いた容量 1 の `LatestFrameSlot` に入れ、`LiveSessions` の entry から
//!   本人向けの frame stream だけが購読する。`LiveEmitter`・`LiveSink`・`EventSink` を通らないので、
//!   events・progress・log・artifacts・tool result・agent message に frame は届かない。auth section
//!   は `LiveEmitter` の event を止めるだけで、本人向けの slot は止めない（H3 の観測停止は解かない）。
//!
//! TODO: 実 `LiveSink`（task-api への POST）は未配線。ここでは trait と、テスト用の in-memory 実装まで。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use task_core::browser_control::{
    BrowserControl, ControlCommand, ControlError, ControlOutcome, ControlPhase, ControlRequest,
};
use task_core::browser_live::{LiveEvent, PersistedLiveEvent, ScrubbedLiveEvent};

/// agent の操作の直前・直後に呼ぶ gate。
pub trait ControlGate: Send + Sync {
    fn phase(&self) -> ControlPhase;
    /// 新しい agent 操作を始めてよければ `Ok`。
    fn begin_action(&self) -> Result<(), ControlError>;
    /// 操作の完了。pause 中で最後の 1 つなら `Paused` に収束する。
    fn end_action(&self);
}

/// session を閉じる（task cancel と同じ経路）。
pub trait SessionCloser: Send + Sync {
    fn close(&self);
}

#[derive(Clone, Default)]
pub struct InMemoryGate {
    inner: Arc<Mutex<BrowserControl>>,
}

impl InMemoryGate {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BrowserControl> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 制御 command を適用する（proxy 経由で届く想定）。
    pub fn apply(&self, req: &ControlRequest, now: u64) -> Result<ControlOutcome, ControlError> {
        self.lock().apply(req, now)
    }

    pub fn version(&self) -> u64 {
        self.lock().version()
    }

    pub fn command(
        &self,
        command: ControlCommand,
        key: &str,
        now: u64,
    ) -> Result<ControlOutcome, ControlError> {
        let mut c = self.lock();
        let req = ControlRequest {
            command,
            expected_version: c.version(),
            idempotency_key: key.to_string(),
        };
        c.apply(&req, now)
    }

    /// lease 期限切れ。自動再開しない。
    pub fn expire(&self, now: u64) -> bool {
        self.lock().expire(now)
    }

    pub fn human_disconnected(&self, holder: &str) {
        self.lock().human_disconnected(holder);
    }

    pub fn enter_auth_section(&self) {
        self.lock().enter_auth_section();
    }
}

impl ControlGate for InMemoryGate {
    fn phase(&self) -> ControlPhase {
        self.lock().phase()
    }
    fn begin_action(&self) -> Result<(), ControlError> {
        self.lock().begin_agent_action()
    }
    fn end_action(&self) {
        self.lock().end_agent_action();
    }
}

/// ADR-0114 D2: store の control 状態を正とする gate（task-api の `agent/begin`・`agent/end` と同じ op）。
/// store の読み書きに失敗したら操作を出さない側に倒す。
pub struct StoreGate {
    store: Arc<dyn task_core::browser_wait::BrowserWaitStore + Send + Sync>,
    task_id: String,
    run_id: String,
    session_id: String,
}

impl StoreGate {
    pub fn new(
        store: Arc<dyn task_core::browser_wait::BrowserWaitStore + Send + Sync>,
        task_id: &str,
        run_id: &str,
        session_id: &str,
    ) -> Self {
        Self {
            store,
            task_id: task_id.into(),
            run_id: run_id.into(),
            session_id: session_id.into(),
        }
    }

    fn call(
        &self,
        op: task_core::browser_control_ops::AgentActionOp,
    ) -> Result<BrowserControl, task_core::browser_store::BrowserStoreError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.store.browser_session_agent_action(
            task_core::browser_store::BrowserSessionKey {
                task_id: &self.task_id,
                run_id: &self.run_id,
                session_id: &self.session_id,
            },
            op,
            now,
        )
    }
}

impl ControlGate for StoreGate {
    fn phase(&self) -> ControlPhase {
        self.call(task_core::browser_control_ops::AgentActionOp::Read)
            .map_or(ControlPhase::Paused, |s| s.phase())
    }
    fn begin_action(&self) -> Result<(), ControlError> {
        match self.call(task_core::browser_control_ops::AgentActionOp::Begin) {
            Ok(_) => Ok(()),
            Err(task_core::browser_store::BrowserStoreError::Control(e)) => Err(e),
            Err(_) => Err(ControlError::InvalidPhase {
                phase: ControlPhase::Paused,
            }),
        }
    }
    fn end_action(&self) {
        let _ = self.call(task_core::browser_control_ops::AgentActionOp::End);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum GatedOutcome<T> {
    Ran(T),
    /// gate が閉じている。操作は出していない。
    Blocked(ControlPhase),
    /// `Stopped`。session を閉じた。
    Closed,
}

/// agent の browser 操作を gate 越しに 1 つ実行する。
pub fn run_gated<T>(
    gate: &dyn ControlGate,
    closer: &dyn SessionCloser,
    action: impl FnOnce() -> T,
) -> GatedOutcome<T> {
    match gate.begin_action() {
        Ok(()) => {
            let out = action();
            gate.end_action();
            GatedOutcome::Ran(out)
        }
        Err(_) if gate.phase() == ControlPhase::Stopped => {
            closer.close();
            GatedOutcome::Closed
        }
        Err(_) => GatedOutcome::Blocked(gate.phase()),
    }
}

pub trait LiveSink: Send + Sync {
    fn send(&self, event: &ScrubbedLiveEvent);
}

#[derive(Default)]
pub struct CollectingSink {
    events: Mutex<Vec<PersistedLiveEvent>>,
}
impl CollectingSink {
    pub fn events(&self) -> Vec<PersistedLiveEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}
impl LiveSink for CollectingSink {
    fn send(&self, event: &ScrubbedLiveEvent) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(event.as_persisted().clone());
    }
}

pub struct LiveEmitter<S: LiveSink> {
    sink: S,
    auth_depth: Mutex<u32>,
    /// ADR-0080 H3 / ADR-0101 D4: identity の復元を受けた session。立ったら session の終わりまで
    /// 認証区間と同じく event・progress・artifact を捨てる（戻す口は無い）。
    restored: Arc<AtomicBool>,
}

/// auth section の RAII。生きている間 event は捨てられる。
pub struct AuthSection<'a, S: LiveSink> {
    emitter: &'a LiveEmitter<S>,
}
impl<S: LiveSink> Drop for AuthSection<'_, S> {
    fn drop(&mut self) {
        let mut d = self
            .emitter
            .auth_depth
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *d = d.saturating_sub(1);
    }
}

impl<S: LiveSink> LiveEmitter<S> {
    pub fn new(sink: S) -> Self {
        Self::with_observation_stop(sink, Arc::new(AtomicBool::new(false)))
    }

    /// 復元の投入口（supervisor の entry）と共有する観測停止の旗を持たせる。
    pub fn with_observation_stop(sink: S, restored: Arc<AtomicBool>) -> Self {
        Self {
            sink,
            auth_depth: Mutex::new(0),
            restored,
        }
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn auth_section(&self) -> AuthSection<'_, S> {
        let mut d = self.auth_depth.lock().unwrap_or_else(|e| e.into_inner());
        *d += 1;
        AuthSection { emitter: self }
    }

    /// 認証区間の中か。中なら呼び出し側は progress・artifact も流さない。
    pub fn in_auth_section(&self) -> bool {
        self.restored.load(Ordering::SeqCst)
            || *self.auth_depth.lock().unwrap_or_else(|e| e.into_inner()) > 0
    }

    /// scrub して送る。auth section 中・frame は送らない（溜めない）。返り値は送ったか。
    pub fn emit(&self, event: &LiveEvent) -> bool {
        if self.in_auth_section() {
            return false;
        }
        match ScrubbedLiveEvent::from_event(event) {
            Some(s) => {
                self.sink.send(&s);
                true
            }
            None => false,
        }
    }
}

// ---- 付記 2026-10-10b: Live View frame の中継（daemon 側）----

/// frame 源（launcher の frame 接続）から 1 回読んだ結果。
pub enum RelayRead {
    /// 1 枚。slot に入れる。
    Frame(task_core::browser_live_frame::LiveFrame),
    /// まだ無い（読みの期限切れ等）。停止の旗を見て読み直す。
    Idle,
    /// stream が終わった（`live_stopped`・切断・protocol 違反）。slot を閉じる。
    End,
}

/// frame 源を読む thread と、その slot。drop（または [`FrameRelay::close`]）で slot を閉じ、
/// thread には停止を知らせる（thread は次の読みの区切りで frame 源を捨てて終わる）。
pub struct FrameRelay {
    slot: Arc<task_core::browser_live_frame::LatestFrameSlot>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FrameRelay {
    /// `read` を別 thread で回し、frame を `slot` に入れる。frame は slot 以外のどこにも渡さない
    /// （log・event に出すのは何も無い）。`read` が `End` を返す・slot が閉じる・停止を知らされると
    /// 終わり、終わるときは必ず slot を閉じる。
    pub fn spawn(
        name: &str,
        mut read: impl FnMut() -> RelayRead + Send + 'static,
    ) -> std::io::Result<Self> {
        let slot = Arc::new(task_core::browser_live_frame::LatestFrameSlot::new());
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_slot, thread_stop) = (Arc::clone(&slot), Arc::clone(&stop));
        let thread = std::thread::Builder::new()
            .name(name.chars().take(15).collect())
            .spawn(move || {
                while !thread_stop.load(Ordering::SeqCst) && !thread_slot.is_closed() {
                    match read() {
                        RelayRead::Frame(frame) => {
                            if thread_slot.publish(frame)
                                == task_core::browser_live_frame::FramePublish::Closed
                            {
                                break;
                            }
                        }
                        RelayRead::Idle => {}
                        RelayRead::End => break,
                    }
                }
                thread_slot.close();
            })?;
        Ok(Self {
            slot,
            stop,
            thread: Some(thread),
        })
    }

    pub fn slot(&self) -> Arc<task_core::browser_live_frame::LatestFrameSlot> {
        Arc::clone(&self.slot)
    }

    /// slot を閉じ、thread に停止を知らせる。待たない。
    pub fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.slot.close();
    }

    /// 閉じてから thread の終わりを待つ（試験用。本番は frame 源の終わりを待たない）。
    #[cfg(test)]
    pub fn close_and_join(mut self) {
        self.close();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for FrameRelay {
    fn drop(&mut self) {
        self.close();
        // thread は frame 源の読みの期限で抜けるので、ここでは待たない（detach）。
        drop(self.thread.take());
    }
}

/// Live View を出せない理由: launcher が v8 未満（frame 接続を持たない）。`live_start` は送っていない。
pub const LIVE_NO_FRAMES_REASON: &str = "launcher_protocol_no_live_frames";
/// Live View を出せない理由: v8 の launcher が frame 接続を開かなかった。session は続ける。
pub const LIVE_STREAM_UNAVAILABLE_REASON: &str = "launcher_live_stream_unavailable";

/// launcher session の registry entry（ADR-0108 D5 の索引に載せる）。state の投入口は持たない
/// （launcher 経路では IdentityRestore を拒否する）。frame の購読口は v8 の frame 接続があり、
/// session が続いている間だけ返す。
pub struct LauncherLiveEntry {
    live_key: (String, String),
    attestation: task_core::browser_isolation::IsolationAttestation,
    frames: Option<Arc<task_core::browser_live_frame::LatestFrameSlot>>,
    unavailable: Option<&'static str>,
    open: AtomicBool,
}

impl LauncherLiveEntry {
    /// Live View を出せない理由（frame 購読口が無いとき）。
    pub fn unavailable_reason(&self) -> Option<&'static str> {
        if self.frames.is_some() && self.open.load(Ordering::SeqCst) {
            None
        } else {
            Some(self.unavailable.unwrap_or(LIVE_STREAM_UNAVAILABLE_REASON))
        }
    }
}

impl task_core::browser_isolation::LiveIsolation for LauncherLiveEntry {
    /// launcher が session の起動時に通した attestation（daemon は launcher の process を観測し直せ
    /// ない）。session が終わった後は違反を返す。
    fn current_attestation(
        &self,
    ) -> Result<
        task_core::browser_isolation::IsolationAttestation,
        Vec<task_core::browser_isolation::IsolationViolation>,
    > {
        if self.open.load(Ordering::SeqCst) {
            Ok(self.attestation.clone())
        } else {
            Err(vec![
                task_core::browser_isolation::IsolationViolation::NoProcessGroup,
            ])
        }
    }
}

impl task_core::browser_isolation::LiveSessionEntry for LauncherLiveEntry {
    fn kind(&self) -> task_core::browser_isolation::RuntimeKind {
        task_core::browser_isolation::RuntimeKind::Isolated
    }
    fn accepts_state(&self) -> bool {
        false
    }
    fn deliver_state(
        &self,
        _state: &[u8],
    ) -> Result<(), task_core::browser_isolation::StateRejected> {
        Err(task_core::browser_isolation::StateRejected)
    }
    fn live_key(&self) -> Option<(String, String)> {
        Some(self.live_key.clone())
    }
    fn live_frames(&self) -> Option<Arc<task_core::browser_live_frame::LatestFrameSlot>> {
        if !self.open.load(Ordering::SeqCst) {
            return None;
        }
        self.frames.as_ref().filter(|s| !s.is_closed()).cloned()
    }
}

/// launcher session の registry 登録と frame 中継の持ち主。drop で registry から外し、slot を閉じる
/// （run・session の終了、stop、どの早期 return でも同じ）。
pub struct LiveFrameRegistration {
    registry: Option<Arc<task_core::browser_isolation::LiveSessions>>,
    session_id: String,
    entry: Arc<LauncherLiveEntry>,
    relay: Option<FrameRelay>,
}

impl LiveFrameRegistration {
    /// `relay` が `Err(reason)` なら frame 購読口なしで登録し、理由を持たせる（他の機能は続ける）。
    pub fn register(
        registry: Option<Arc<task_core::browser_isolation::LiveSessions>>,
        session_id: &str,
        live_key: (String, String),
        attestation: task_core::browser_isolation::IsolationAttestation,
        relay: Result<FrameRelay, &'static str>,
    ) -> Self {
        let (relay, unavailable) = match relay {
            Ok(r) => (Some(r), None),
            Err(reason) => (None, Some(reason)),
        };
        let entry = Arc::new(LauncherLiveEntry {
            live_key,
            attestation,
            frames: relay.as_ref().map(FrameRelay::slot),
            unavailable,
            open: AtomicBool::new(true),
        });
        if let Some(reg) = &registry {
            reg.insert(session_id, entry.clone());
        }
        Self {
            registry,
            session_id: session_id.into(),
            entry,
            relay,
        }
    }

    pub fn entry(&self) -> &Arc<LauncherLiveEntry> {
        &self.entry
    }
}

impl Drop for LiveFrameRegistration {
    fn drop(&mut self) {
        if let Some(reg) = self.registry.take() {
            reg.remove(&self.session_id);
        }
        self.entry.open.store(false, Ordering::SeqCst);
        if let Some(slot) = &self.entry.frames {
            slot.close();
        }
        if let Some(relay) = &self.relay {
            relay.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use task_core::browser_live::LiveTab;

    #[derive(Default)]
    struct FakeCloser(AtomicU32);
    impl SessionCloser for FakeCloser {
        fn close(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn browser_live_pause_during_in_flight_converges_after_completion() {
        let gate = InMemoryGate::new();
        let closer = FakeCloser::default();
        let issued = AtomicU32::new(0);
        let out = run_gated(&gate, &closer, || {
            issued.fetch_add(1, Ordering::SeqCst);
            // pause は実行中に届く。収束していない。
            let o = gate.command(ControlCommand::Pause, "p1", 0);
            assert!(o.is_ok());
            assert_eq!(gate.phase(), ControlPhase::Pausing);
            7
        });
        assert_eq!(out, GatedOutcome::Ran(7));
        assert_eq!(gate.phase(), ControlPhase::Paused);
        let out = run_gated(&gate, &closer, || issued.fetch_add(1, Ordering::SeqCst));
        assert_eq!(out, GatedOutcome::Blocked(ControlPhase::Paused));
        assert_eq!(issued.load(Ordering::SeqCst), 1);
        let resumed = gate.command(
            ControlCommand::Resume {
                holder: "h".into(),
                fresh_snapshot: true,
                policy_origin_ok: true,
            },
            "r1",
            0,
        );
        assert!(resumed.is_ok(), "{resumed:?}");
        assert!(matches!(
            run_gated(&gate, &closer, || 1),
            GatedOutcome::Ran(1)
        ));
    }

    #[test]
    fn browser_live_human_control_blocks_and_stop_closes() {
        let gate = InMemoryGate::new();
        let closer = FakeCloser::default();
        gate.command(ControlCommand::Pause, "p", 0).ok();
        let t = gate.command(
            ControlCommand::Takeover {
                holder: "h".into(),
                ttl_secs: None,
            },
            "t",
            10,
        );
        assert!(t.is_ok(), "{t:?}");
        assert_eq!(gate.phase(), ControlPhase::HumanControl);
        let ran = AtomicU32::new(0);
        let out = run_gated(&gate, &closer, || ran.fetch_add(1, Ordering::SeqCst));
        assert_eq!(out, GatedOutcome::Blocked(ControlPhase::HumanControl));
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        assert!(gate.command(ControlCommand::Stop, "s", 11).is_ok());
        let out = run_gated(&gate, &closer, || ran.fetch_add(1, Ordering::SeqCst));
        assert_eq!(out, GatedOutcome::Closed);
        assert_eq!(closer.0.load(Ordering::SeqCst), 1);
        assert_eq!(ran.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn browser_live_events_have_no_frame_title_cookie_or_token() {
        let emitter = LiveEmitter::new(CollectingSink::default());
        assert!(!emitter.emit(&LiveEvent::Frame {
            bytes: vec![1, 2, 3]
        }));
        emitter.emit(&LiveEvent::Status {
            state: "running".into(),
        });
        emitter.emit(&LiveEvent::Tabs {
            tabs: vec![LiveTab {
                id: "t1".into(),
                url: "https://example.com/a?token=SECRETTOKEN&code=abc".into(),
                title: "SECRETTITLE".into(),
            }],
        });
        emitter.emit(&LiveEvent::Url {
            url: "https://example.com/cb?access_token=SECRETTOKEN&code=xyz#frag".into(),
        });
        emitter.emit(&LiveEvent::Console {
            level: "log".into(),
            text:
                "Set-Cookie: sid=SECRETCOOKIE; Authorization: Bearer SECRETTOKEN password=hunter2"
                    .into(),
        });
        let events = emitter.sink().events();
        assert_eq!(events.len(), 4);
        let json = serde_json::to_string(&events)
            .unwrap_or_default()
            .to_lowercase();
        for bad in [
            "frame",
            "title",
            "secrettitle",
            "secrettoken",
            "secretcookie",
            "hunter2",
            "access_token",
            "code=",
            "set-cookie",
        ] {
            assert!(!json.contains(bad), "{bad} leaked: {json}");
        }
    }

    #[test]
    fn browser_live_auth_section_drops_events_without_buffering() {
        let emitter = LiveEmitter::new(CollectingSink::default());
        {
            let _g = emitter.auth_section();
            assert!(!emitter.emit(&LiveEvent::Url {
                url: "https://example.com/login".into()
            }));
            assert!(!emitter.emit(&LiveEvent::Status {
                state: "running".into()
            }));
        }
        assert!(emitter.sink().events().is_empty());
        assert!(emitter.emit(&LiveEvent::Status {
            state: "running".into()
        }));
        assert_eq!(emitter.sink().events().len(), 1);
    }

    #[test]
    fn browser_live_lease_expiry_and_disconnect_do_not_auto_resume() {
        let gate = InMemoryGate::new();
        gate.command(ControlCommand::Pause, "p", 0).ok();
        gate.command(
            ControlCommand::Takeover {
                holder: "h".into(),
                ttl_secs: Some(5),
            },
            "t",
            10,
        )
        .ok();
        gate.expire(1000);
        assert_ne!(gate.phase(), ControlPhase::AgentRunning);
        assert!(gate.begin_action().is_err());
        gate.command(
            ControlCommand::Takeover {
                holder: "h".into(),
                ttl_secs: Some(5),
            },
            "t2",
            2000,
        )
        .ok();
        gate.human_disconnected("h");
        assert_eq!(gate.phase(), ControlPhase::Paused);
        assert!(gate.begin_action().is_err());
    }
}
