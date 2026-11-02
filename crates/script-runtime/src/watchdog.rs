//! 外部 watchdog（境界 ADR 2026-11-02 (b)(c)）。
//!
//! actor thread が V8 に入る直前に、その間に守る deadline を [`Watchdog::arm`] で渡し、戻ったら
//! [`Watchdog::disarm`] で外す。watchdog thread は deadline を過ぎても外されなければ、その generation の
//! `IsolateHandle::terminate_execution` で V8 を止める。同期の無限ループは V8 の中から戻らないので、
//! actor 自身では止められない。
//!
//! 時間は actor が渡す deadline だけで決まる（watchdog は自分で時間を決めない）。

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

use deno_core::v8;

/// watchdog thread と actor が共有する状態。
struct State {
    /// 今の generation の isolate。作り直しのたびに差し替える。
    isolate: Option<v8::IsolateHandle>,
    /// 今守っている deadline と、その arm の番号（古い arm の発火を避ける）。
    armed: Option<(Instant, u64)>,
    next_token: u64,
    /// arm の間に terminate したか。
    fired: bool,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    cond: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// actor が持つ watchdog の操作口。Drop で watchdog thread を止めて join する。
pub(crate) struct Watchdog {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Watchdog {
    pub(crate) fn spawn() -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                isolate: None,
                armed: None,
                next_token: 0,
                fired: false,
                stop: false,
            }),
            cond: Condvar::new(),
        });
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("celeris-script-watchdog".into())
                .spawn(move || watch(&shared))?
        };
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// 今の generation の isolate を渡す。
    pub(crate) fn set_isolate(&self, isolate: v8::IsolateHandle) {
        let mut st = self.shared.lock();
        st.isolate = Some(isolate);
        st.armed = None;
        st.fired = false;
    }

    /// これから V8 に入る。`deadline` を過ぎて [`Self::disarm`] が来なければ V8 を止める。
    pub(crate) fn arm(&self, deadline: Instant) {
        let mut st = self.shared.lock();
        st.next_token += 1;
        st.armed = Some((deadline, st.next_token));
        st.fired = false;
        self.shared.cond.notify_all();
    }

    /// V8 から戻った。arm の間に terminate したかを返す。
    pub(crate) fn disarm(&self) -> bool {
        let mut st = self.shared.lock();
        st.armed = None;
        std::mem::take(&mut st.fired)
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        {
            let mut st = self.shared.lock();
            st.stop = true;
            st.isolate = None;
            self.shared.cond.notify_all();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn watch(shared: &Shared) {
    let mut st = shared.lock();
    loop {
        if st.stop {
            return;
        }
        let Some((deadline, token)) = st.armed else {
            st = match shared.cond.wait(st) {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            continue;
        };
        let now = Instant::now();
        if now < deadline {
            st = match shared.cond.wait_timeout(st, deadline - now) {
                Ok((g, _)) => g,
                Err(p) => p.into_inner().0,
            };
            continue;
        }
        // deadline を過ぎても同じ arm のまま: V8 を止める。
        if st.armed.map(|(_, t)| t) == Some(token) && !st.fired {
            if let Some(isolate) = &st.isolate {
                isolate.terminate_execution();
            }
            st.fired = true;
        }
        // 発火済み。actor の disarm か次の arm を待つ。
        st = match shared.cond.wait(st) {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
    }
}
