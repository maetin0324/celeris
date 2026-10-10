//! ADR 2026-10-10-browser-launcher-live-view-frames 付記 2026-10-10b: Live View の揮発 frame。
//!
//! `LiveFrame` は launcher の screencast から gateway まで本人に届ける画像 1 枚で、どの永続層
//! （DB・WAL・events・progress・log・artifacts・tmp）にも残さない。そのため `Serialize` /
//! `Deserialize` / `Debug` / `Clone` を実装せず、persistable な `LiveEvent` /
//! `ScrubbedLiveEvent` / `PersistedLiveEvent` への変換も持たない。診断に出してよいのは
//! [`LiveFrameDiag`]（byte 長・seq・寸法・encoding）だけ。
//!
//! ```compile_fail
//! use task_core::browser_live_frame::{LiveFrame, LiveFrameEncoding};
//! let f = LiveFrame::new(1, 2, 2, LiveFrameEncoding::Jpeg, vec![1]).ok();
//! let _ = serde_json::to_string(&f);
//! ```
//!
//! ```compile_fail
//! use task_core::browser_live_frame::{LiveFrame, LiveFrameEncoding};
//! let f = LiveFrame::new(1, 2, 2, LiveFrameEncoding::Jpeg, vec![1]).ok();
//! let _ = format!("{f:?}");
//! ```
//!
//! ```compile_fail
//! use task_core::browser_live::ScrubbedLiveEvent;
//! use task_core::browser_live_frame::{LiveFrame, LiveFrameEncoding};
//! fn to_event(f: LiveFrame) -> ScrubbedLiveEvent { f.into() }
//! ```
//!
//! [`LatestFrameSlot`] は容量 1 の受け渡し口。下流が詰まれば古い frame を上書きし、queue・再送・
//! disk を持たない。close は保持中の frame を捨て、待ち手を起こす。

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

/// 1 frame の body 上限（付記 2026-10-10b v8 wire contract）。
pub const LIVE_FRAME_MAX_BYTES: usize = 2 * 1024 * 1024;

/// frame の画像 encoding。CDP `Page.startScreencast` の format に合わせる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveFrameEncoding {
    Jpeg,
    Png,
}

impl LiveFrameEncoding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "jpeg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            _ => None,
        }
    }
}

/// frame を作れなかった理由。body の中身は持たない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveFrameRejected {
    Empty,
    TooLarge { len: usize },
    ZeroDimension,
}

/// Live View の画像 1 枚（揮発専用）。永続化・Debug 表示・複製の手段を持たない。
pub struct LiveFrame {
    seq: u64,
    width: u32,
    height: u32,
    encoding: LiveFrameEncoding,
    body: Vec<u8>,
}

/// 診断に出してよい frame の事実。画像本体は含まない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveFrameDiag {
    pub byte_len: usize,
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub encoding: LiveFrameEncoding,
}

impl LiveFrame {
    pub fn new(
        seq: u64,
        width: u32,
        height: u32,
        encoding: LiveFrameEncoding,
        body: Vec<u8>,
    ) -> Result<Self, LiveFrameRejected> {
        if body.is_empty() {
            return Err(LiveFrameRejected::Empty);
        }
        if body.len() > LIVE_FRAME_MAX_BYTES {
            return Err(LiveFrameRejected::TooLarge { len: body.len() });
        }
        if width == 0 || height == 0 {
            return Err(LiveFrameRejected::ZeroDimension);
        }
        Ok(Self {
            seq,
            width,
            height,
            encoding,
            body,
        })
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn encoding(&self) -> LiveFrameEncoding {
        self.encoding
    }

    /// 本人向けの送出（gateway への書き出し）にだけ使う。log・event に渡さない。
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn diag(&self) -> LiveFrameDiag {
        LiveFrameDiag {
            byte_len: self.body.len(),
            seq: self.seq,
            width: self.width,
            height: self.height,
            encoding: self.encoding,
        }
    }
}

/// `LatestFrameSlot::publish` の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePublish {
    /// 空の slot に入った。
    Stored,
    /// 下流が受け取る前の frame を上書きした（背圧）。
    Replaced,
    /// slot は閉じている。frame は捨てた。
    Closed,
}

#[derive(Default)]
struct SlotState {
    frame: Option<LiveFrame>,
    closed: bool,
    replaced: u64,
    wakers: Vec<Waker>,
}

/// 容量 1 の最新 frame の受け渡し口。上流（daemon の frame 接続）は `publish`、下流（frame
/// stream）は `next` / `try_take` で受け取る。古い frame は上書きで捨て、溜めない。
#[derive(Default)]
pub struct LatestFrameSlot {
    state: Mutex<SlotState>,
}

impl std::fmt::Debug for LatestFrameSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (pending, closed, replaced) = self
            .state
            .lock()
            .map(|s| (s.frame.as_ref().map(LiveFrame::diag), s.closed, s.replaced))
            .unwrap_or((None, true, 0));
        f.debug_struct("LatestFrameSlot")
            .field("pending", &pending)
            .field("closed", &closed)
            .field("replaced", &replaced)
            .finish()
    }
}

impl LatestFrameSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// 最新 frame を置く。未受領の frame があれば上書きする。待ち手を起こす。
    pub fn publish(&self, frame: LiveFrame) -> FramePublish {
        let Ok(mut s) = self.state.lock() else {
            return FramePublish::Closed;
        };
        if s.closed {
            return FramePublish::Closed;
        }
        let outcome = if s.frame.replace(frame).is_some() {
            s.replaced = s.replaced.saturating_add(1);
            FramePublish::Replaced
        } else {
            FramePublish::Stored
        };
        let wakers = std::mem::take(&mut s.wakers);
        drop(s);
        wakers.into_iter().for_each(Waker::wake);
        outcome
    }

    /// 保持中の frame があれば取り出す。待たない。
    pub fn try_take(&self) -> Option<LiveFrame> {
        self.state.lock().ok()?.frame.take()
    }

    /// 次の frame を待つ。閉じていれば `None`。
    pub fn next(&self) -> NextFrame<'_> {
        NextFrame { slot: self }
    }

    /// 閉じる。保持中の frame を捨て、待ち手を起こす。以後の `publish` は捨てる。
    pub fn close(&self) {
        let wakers = match self.state.lock() {
            Ok(mut s) => {
                s.closed = true;
                s.frame = None;
                std::mem::take(&mut s.wakers)
            }
            Err(_) => return,
        };
        wakers.into_iter().for_each(Waker::wake);
    }

    pub fn is_closed(&self) -> bool {
        self.state.lock().map(|s| s.closed).unwrap_or(true)
    }

    /// 上書きで捨てた frame の数（診断用）。
    pub fn replaced_count(&self) -> u64 {
        self.state.lock().map(|s| s.replaced).unwrap_or(0)
    }
}

/// `LatestFrameSlot::next` の future。
pub struct NextFrame<'a> {
    slot: &'a LatestFrameSlot,
}

impl Future for NextFrame<'_> {
    type Output = Option<LiveFrame>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Ok(mut s) = self.slot.state.lock() else {
            return Poll::Ready(None);
        };
        if let Some(frame) = s.frame.take() {
            return Poll::Ready(Some(frame));
        }
        if s.closed {
            return Poll::Ready(None);
        }
        if !s.wakers.iter().any(|w| w.will_wake(cx.waker())) {
            s.wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_isolation::{
        IsolationAttestation, IsolationViolation, LiveIsolation, LiveSessionEntry, RuntimeKind,
        StateRejected,
    };
    use crate::browser_live::{LiveEvent, PersistedLiveEvent, ScrubbedLiveEvent};
    use std::marker::PhantomData;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    fn frame(seq: u64) -> LiveFrame {
        LiveFrame::new(seq, 4, 3, LiveFrameEncoding::Jpeg, vec![seq as u8; 8]).expect("valid frame")
    }

    struct CountWaker(AtomicUsize);
    impl Wake for CountWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn poll_next(slot: &LatestFrameSlot, waker: &Waker) -> Poll<Option<LiveFrame>> {
        let mut fut = slot.next();
        Pin::new(&mut fut).poll(&mut Context::from_waker(waker))
    }

    #[test]
    fn browser_live_frame_backpressure_keeps_latest_only() {
        let slot = LatestFrameSlot::new();
        let counter = Arc::new(CountWaker(AtomicUsize::new(0)));
        let waker = Waker::from(counter.clone());

        // 空なら待つ（出来事待ち: waker が登録される）。
        assert!(poll_next(&slot, &waker).is_pending());

        // 下流が詰まっている間に 5 枚届く: 最新 1 枚だけ残る。
        assert_eq!(slot.publish(frame(1)), FramePublish::Stored);
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        for seq in 2..=5 {
            assert_eq!(slot.publish(frame(seq)), FramePublish::Replaced);
        }
        assert_eq!(slot.replaced_count(), 4);

        let Poll::Ready(Some(got)) = poll_next(&slot, &waker) else {
            panic!("latest frame expected");
        };
        assert_eq!(got.seq(), 5);
        assert_eq!(got.body(), &[5u8; 8]);
        // 古い frame は再送しない。
        assert!(slot.try_take().is_none());
        assert!(poll_next(&slot, &waker).is_pending());

        // close は待ち手を起こし、保持中の frame を捨て、以後の frame を受けない。
        let before = counter.0.load(Ordering::SeqCst);
        slot.close();
        assert_eq!(counter.0.load(Ordering::SeqCst), before + 1);
        assert!(matches!(poll_next(&slot, &waker), Poll::Ready(None)));
        assert_eq!(slot.publish(frame(6)), FramePublish::Closed);
        assert!(slot.try_take().is_none());

        let held = LatestFrameSlot::new();
        held.publish(frame(7));
        held.close();
        assert!(held.try_take().is_none());
    }

    #[test]
    fn browser_live_frame_rejects_oversize_and_empty() {
        let big = vec![0u8; LIVE_FRAME_MAX_BYTES + 1];
        assert_eq!(
            LiveFrame::new(1, 1, 1, LiveFrameEncoding::Png, big).err(),
            Some(LiveFrameRejected::TooLarge {
                len: LIVE_FRAME_MAX_BYTES + 1
            })
        );
        assert_eq!(
            LiveFrame::new(1, 1, 1, LiveFrameEncoding::Png, Vec::new()).err(),
            Some(LiveFrameRejected::Empty)
        );
        assert_eq!(
            LiveFrame::new(1, 0, 1, LiveFrameEncoding::Png, vec![1]).err(),
            Some(LiveFrameRejected::ZeroDimension)
        );
        assert!(
            LiveFrame::new(
                1,
                1,
                1,
                LiveFrameEncoding::Png,
                vec![0; LIVE_FRAME_MAX_BYTES]
            )
            .is_ok()
        );
        assert_eq!(
            LiveFrameEncoding::parse("jpeg"),
            Some(LiveFrameEncoding::Jpeg)
        );
        assert_eq!(LiveFrameEncoding::parse("webp"), None);
    }

    // 自動参照による特殊化: 境界を満たす型では固有 method が、満たさなければ trait の既定が選ばれる。
    struct Probe<T>(PhantomData<T>);
    trait Fallback {
        fn serializable(&self) -> bool {
            false
        }
        fn deserializable(&self) -> bool {
            false
        }
        fn debuggable(&self) -> bool {
            false
        }
        fn cloneable(&self) -> bool {
            false
        }
        fn converts_to_live_event(&self) -> bool {
            false
        }
        fn converts_to_scrubbed(&self) -> bool {
            false
        }
        fn converts_to_persisted(&self) -> bool {
            false
        }
    }
    impl<T> Fallback for Probe<T> {}
    impl<T: serde::Serialize> Probe<T> {
        fn serializable(&self) -> bool {
            true
        }
    }
    impl<T: serde::de::DeserializeOwned> Probe<T> {
        fn deserializable(&self) -> bool {
            true
        }
    }
    impl<T: std::fmt::Debug> Probe<T> {
        fn debuggable(&self) -> bool {
            true
        }
    }
    impl<T: Clone> Probe<T> {
        fn cloneable(&self) -> bool {
            true
        }
    }
    impl<T: Into<LiveEvent>> Probe<T> {
        fn converts_to_live_event(&self) -> bool {
            true
        }
    }
    impl<T: Into<ScrubbedLiveEvent>> Probe<T> {
        fn converts_to_scrubbed(&self) -> bool {
            true
        }
    }
    impl<T: Into<PersistedLiveEvent>> Probe<T> {
        fn converts_to_persisted(&self) -> bool {
            true
        }
    }

    #[test]
    fn browser_live_frame_not_serializable() {
        // 探査器そのものが効くこと（陽性対照）。
        let control = Probe::<PersistedLiveEvent>(PhantomData);
        assert!(control.serializable() && control.deserializable() && control.debuggable());
        assert!(control.converts_to_persisted());
        assert!(control.cloneable());
        assert!(Probe::<LiveEvent>(PhantomData).converts_to_live_event());
        assert!(Probe::<ScrubbedLiveEvent>(PhantomData).converts_to_scrubbed());

        let p = Probe::<LiveFrame>(PhantomData);
        assert!(!p.serializable(), "LiveFrame must not implement Serialize");
        assert!(
            !p.deserializable(),
            "LiveFrame must not implement Deserialize"
        );
        assert!(!p.debuggable(), "LiveFrame must not implement Debug");
        assert!(!p.cloneable(), "LiveFrame must not implement Clone");
        assert!(
            !p.converts_to_live_event(),
            "LiveFrame must not convert to LiveEvent"
        );
        assert!(
            !p.converts_to_scrubbed(),
            "LiveFrame must not convert to ScrubbedLiveEvent"
        );
        assert!(
            !p.converts_to_persisted(),
            "LiveFrame must not convert to PersistedLiveEvent"
        );

        // slot の Debug は frame の診断事実だけを出し、body を出さない。
        let slot = LatestFrameSlot::new();
        let secret = b"SECRET-PIXELS".to_vec();
        slot.publish(LiveFrame::new(9, 640, 480, LiveFrameEncoding::Png, secret).expect("frame"));
        let shown = format!("{slot:?}");
        assert!(
            shown.contains("byte_len: 13") && shown.contains("seq: 9"),
            "{shown}"
        );
        assert!(
            !shown.contains("SECRET") && !shown.contains("83, 69, 67"),
            "{shown}"
        );
    }

    struct Plain;
    impl LiveIsolation for Plain {
        fn current_attestation(&self) -> Result<IsolationAttestation, Vec<IsolationViolation>> {
            Err(Vec::new())
        }
    }
    impl LiveSessionEntry for Plain {
        fn kind(&self) -> RuntimeKind {
            RuntimeKind::NotIsolated
        }
        fn accepts_state(&self) -> bool {
            false
        }
        fn deliver_state(&self, _state: &[u8]) -> Result<(), StateRejected> {
            Err(StateRejected)
        }
    }

    #[test]
    fn browser_live_frame_entry_has_no_frames_by_default() {
        assert!(Plain.live_frames().is_none());
    }
}
