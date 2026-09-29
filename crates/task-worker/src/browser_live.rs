//! ADR-0081 D3 / ADR-0082 / ADR-0080 H3: browser 実行に P3-C の制御 gate と live event の出口を挟む。
//!
//! - `ControlGate`: agent の browser 操作の直前に問い合わせる。`AgentRunning` 以外では新しい操作を出さない。
//!   pause は実行中の操作の完了を待って `Paused` に収束する。lease 切れ・切断で自動再開しない。
//!   `Stopped` は cancel と同じ経路（`SessionCloser`）で session を閉じる。
//! - `LiveEmitter`: status/tabs/url/console だけを、`task_core::browser_live::persistable` で
//!   scrub してから `LiveSink` に渡す。frame と tab title は型として持てない。
//!   auth section（credential 注入・identity 復元）の間は捨てる（溜めない）。
//!
//! TODO: 実 `LiveSink`（task-api への POST）は未配線。ここでは trait と、テスト用の in-memory 実装まで。

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
        Self {
            sink,
            auth_depth: Mutex::new(0),
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
        *self.auth_depth.lock().unwrap_or_else(|e| e.into_inner()) > 0
    }

    /// scrub して送る。auth section 中・frame は送らない（溜めない）。返り値は送ったか。
    pub fn emit(&self, event: &LiveEvent) -> bool {
        if *self.auth_depth.lock().unwrap_or_else(|e| e.into_inner()) > 0 {
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
