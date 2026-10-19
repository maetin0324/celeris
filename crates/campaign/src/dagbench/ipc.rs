//! ExtSim を別 process に置くための IPC adapter（行区切り JSON over unix socket、版付き）。
//!
//! # 次の子へ渡す契約
//!
//! ## 構成
//! - [`serve_unix`] / [`serve`]: [`ExtSim`]（ledger の持ち主）を所有する独立 process の入口。制御 child が
//!   死んでも server は生き続け、`Shutdown` 要求（または [`ServerHandle`] の停止）でだけ終わる。
//!   接続ごとに thread を持ち、ExtSim は 1 つの Mutex で直列化する。
//! - [`IpcExt`]: client。[`ExtBackend`] を実装するので `Baseline::run` / `CelerisSide::with_backend` が
//!   in-process の `ExtSim` と同じ形で使える。
//!
//! ## 線形式
//! 1 行 1 JSON（改行区切り）。[`Request`] `{v, client, id, ack, op}` に対し [`Response`] `{v, id, body}` が 1 つ返る。
//! - `v` は [`PROTOCOL_VERSION`]。server は版が違う要求に `body = {"error": ...}` を返す（黙って解釈しない）。
//! - `op` は [`Op`]（`submit` / `wait_started` / `lookup` / `advance` / `record_consumed` / `now` / `due_secs` /
//!   `ack` / `shutdown`）。独立観測 runner（外部時計を進める側）も同じ `advance` を別接続から呼べる。
//!
//! ## 通知配送: seq + ack + 再送（at-least-once、重複は seq で冪等）
//! - `advance` で ExtSim が返した [`Notice`] に server が単調増加の `seq`（1 始まり）を振り、outbox に積む。
//! - `advance` の応答は outbox の **ack されていない通知を全て**（seq 昇順）含む。つまり再送は「もう一度
//!   `advance` する（同じ時刻でよい）」だけで起こる。応答を落とされた通知も次の `advance` で届く。
//! - client は受け取った通知のうち `seq > last_seq` だけを呼び出し側へ渡して `last_seq` を進める
//!   （同じ seq の再受信は捨てる = 冪等）。
//! - ack は「`seq <= ack` を呼び出し側へ渡した」の意味で、全 request の `ack` 欄に載る（piggyback）。
//!   server は `ack` 以下を outbox から捨てる。[`IpcExt::flush_ack`] は `ack` op を単独で送る。
//! - outbox は server の 1 つで、通知は `advance` を呼んだ接続に渡る（消費者は 1 人を想定）。server が死ぬと outbox は
//!   失われるが、完了の事実は ledger（`external_job_completed` / `notice_sent`）に残り、poll の `lookup` で拾える。
//!   アプリ側の冪等性は論理 job ID（baseline の `duplicate_notices_ignored`）が持つ。
//!
//! ## 要求の再送と有界性
//! - 各 request の `id` は client 内で単調増加。同じ `(client, id)` の再送には server が前回の応答を返し、
//!   ExtSim の操作を二重に実行しない。
//! - client は read/write に `io_timeout` を持ち、失敗したら接続を捨てて connector で張り直し、**同じ id** で
//!   最大 `retries` 回再送する。総待ち時間は高々 `(retries + 1) * io_timeout` + connect 時間。使い切ると
//!   `Err`（`TimedOut` / `BrokenPipe` / `ConnectionRefused` 等）。無限に待たない。
//! - server 側の接続は client の切断（EOF）で終わるだけで、ExtSim の状態には影響しない。
use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};

use super::ext::{ExtBackend, ExtConfig, ExtSim, Lookup, Notice, SubmitError, SubmitReply};

/// 線形式の版。互換のない変更で上げる（server は違う版を拒否する）。
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Submit {
        logical: String,
        job_secs: u64,
    },
    WaitStarted {
        logical: String,
    },
    Lookup {
        logical: String,
    },
    Advance {
        now_secs: u64,
    },
    RecordConsumed {
        logical: String,
    },
    Now,
    DueSecs {
        logical: String,
    },
    /// 単独の ack（他の要求にも `ack` 欄が載る）。
    Ack,
    /// server を止める。応答の後で全接続を閉じ、[`serve`] が戻る。
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub v: u32,
    pub client: String,
    pub id: u64,
    /// ここまでの通知 seq を呼び出し側に渡した。
    pub ack: u64,
    #[serde(flatten)]
    pub op: Op,
}

/// seq つきの通知。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeqNotice {
    pub seq: u64,
    pub notice: Notice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    Submit {
        reply: Result<SubmitReply, SubmitError>,
    },
    Unit,
    Lookup {
        found: Lookup,
    },
    Advance {
        notices: Vec<SeqNotice>,
    },
    Consumed {
        first: bool,
    },
    Now {
        secs: u64,
    },
    DueSecs {
        secs: Option<u64>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub v: u32,
    pub id: u64,
    pub body: Body,
}

// ---- server ----

struct State {
    sim: ExtSim,
    next_seq: u64,
    outbox: Vec<SeqNotice>,
    /// client ごとの直近の応答（同じ id の再送に返す）。
    last: BTreeMap<String, Response>,
    conns: Vec<UnixStream>,
}

/// 実行中の server への取っ手。
pub struct ServerHandle {
    path: PathBuf,
    join: std::thread::JoinHandle<io::Result<()>>,
}

impl ServerHandle {
    pub fn socket(&self) -> &Path {
        &self.path
    }

    /// server が終わるまで待つ（`Shutdown` 要求の後で呼ぶ）。
    pub fn join(self) -> io::Result<()> {
        self.join
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("ipc server thread panicked")))
    }
}

/// `socket` に listen して ExtSim を所有する server を **この thread で** 走らせ、`Shutdown` で戻る。
/// 独立 process の入口として bin / test から呼ぶ。
pub fn serve_unix(socket: &Path, ledger: &Path, cfg: ExtConfig) -> io::Result<()> {
    let sim = ExtSim::open(ledger, cfg)?;
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    serve(listener, sim)
}

/// 別 thread で [`serve_unix`] を起こす（同一 process 内の試験用。実 process 化は呼び出し側が [`serve_unix`] を呼ぶ）。
pub fn spawn_server(socket: &Path, ledger: &Path, cfg: ExtConfig) -> io::Result<ServerHandle> {
    let sim = ExtSim::open(ledger, cfg)?;
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    let path = socket.to_path_buf();
    let join = std::thread::spawn(move || serve(listener, sim));
    Ok(ServerHandle { path, join })
}

/// `listener` で受け付けて `sim` を直列に操作する。`Shutdown` 要求で全接続を閉じて戻る。
pub fn serve(listener: UnixListener, sim: ExtSim) -> io::Result<()> {
    let self_path = listener.local_addr()?.as_pathname().map(Path::to_path_buf);
    let state = Arc::new(Mutex::new(State {
        sim,
        next_seq: 0,
        outbox: Vec::new(),
        last: BTreeMap::new(),
        conns: Vec::new(),
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    for conn in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(conn) = conn else { continue };
        if let Ok(c) = conn.try_clone() {
            lock(&state).conns.push(c);
        }
        let (state, stop, path) = (Arc::clone(&state), Arc::clone(&stop), self_path.clone());
        workers.push(std::thread::spawn(move || {
            handle_conn(conn, &state, &stop, path.as_deref())
        }));
    }
    for c in lock(&state).conns.drain(..) {
        let _ = c.shutdown(std::net::Shutdown::Both);
    }
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

fn lock(s: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn handle_conn(conn: UnixStream, state: &Mutex<State>, stop: &AtomicBool, wake: Option<&Path>) {
    let Ok(read) = conn.try_clone() else { return };
    let mut write = conn;
    for line in BufReader::new(read).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let shutdown;
        let resp = match serde_json::from_str::<Request>(&line) {
            Err(e) => {
                shutdown = false;
                Response {
                    v: PROTOCOL_VERSION,
                    id: 0,
                    body: Body::Error {
                        message: format!("bad request: {e}"),
                    },
                }
            }
            Ok(req) => {
                shutdown = req.op == Op::Shutdown && req.v == PROTOCOL_VERSION;
                respond(&mut lock(state), req)
            }
        };
        let Ok(mut out) = serde_json::to_string(&resp) else {
            return;
        };
        out.push('\n');
        if write.write_all(out.as_bytes()).is_err() || write.flush().is_err() {
            return;
        }
        if shutdown {
            stop.store(true, Ordering::SeqCst);
            // accept を起こす（accept の後で stop を見て抜ける）。
            if let Some(p) = wake {
                let _ = UnixStream::connect(p);
            }
            return;
        }
    }
}

fn respond(st: &mut State, req: Request) -> Response {
    let reply = |id, body| Response {
        v: PROTOCOL_VERSION,
        id,
        body,
    };
    if req.v != PROTOCOL_VERSION {
        return reply(
            req.id,
            Body::Error {
                message: format!(
                    "protocol version {} not supported (server speaks {PROTOCOL_VERSION})",
                    req.v
                ),
            },
        );
    }
    if let Some(prev) = st.last.get(&req.client)
        && prev.id == req.id
    {
        return prev.clone();
    }
    st.outbox.retain(|n| n.seq > req.ack);
    let io_err = |e: io::Error| Body::Error {
        message: e.to_string(),
    };
    let body = match req.op {
        Op::Submit { logical, job_secs } => Body::Submit {
            reply: st.sim.submit(&logical, job_secs),
        },
        Op::WaitStarted { logical } => match st.sim.wait_started(&logical) {
            Ok(()) => Body::Unit,
            Err(e) => io_err(e),
        },
        Op::Lookup { logical } => Body::Lookup {
            found: st.sim.lookup(&logical),
        },
        Op::Advance { now_secs } => match st.sim.advance(now_secs) {
            Ok(ready) => {
                for notice in ready {
                    st.next_seq += 1;
                    let seq = st.next_seq;
                    st.outbox.push(SeqNotice { seq, notice });
                }
                Body::Advance {
                    notices: st.outbox.clone(),
                }
            }
            Err(e) => io_err(e),
        },
        Op::RecordConsumed { logical } => match st.sim.record_consumed(&logical) {
            Ok(first) => Body::Consumed { first },
            Err(e) => io_err(e),
        },
        Op::Now => Body::Now { secs: st.sim.now() },
        Op::DueSecs { logical } => Body::DueSecs {
            secs: st.sim.due_secs(&logical),
        },
        Op::Ack | Op::Shutdown => Body::Unit,
    };
    let resp = reply(req.id, body);
    st.last.insert(req.client, resp.clone());
    resp
}

// ---- client ----

type Connector = Box<dyn Fn() -> io::Result<UnixStream> + Send>;

/// client の時間設定。
#[derive(Debug, Clone, Copy)]
pub struct IpcOptions {
    /// 1 回の read / write の上限。
    pub io_timeout: Duration,
    /// 失敗後に張り直して同じ id で再送する回数。
    pub retries: u32,
}

impl Default for IpcOptions {
    fn default() -> Self {
        Self {
            io_timeout: Duration::from_secs(10),
            retries: 2,
        }
    }
}

struct Conn {
    write: UnixStream,
    read: BufReader<UnixStream>,
}

/// [`ExtBackend`] の IPC 実装。
pub struct IpcExt {
    client: String,
    connect: Connector,
    opts: IpcOptions,
    conn: Option<Conn>,
    next_id: u64,
    last_seq: u64,
}

impl IpcExt {
    /// unix socket の server に繋ぐ client（接続は最初の要求で張る）。
    pub fn connect_unix(socket: &Path, client: &str, opts: IpcOptions) -> Self {
        let path = socket.to_path_buf();
        Self::with_connector(client, opts, Box::new(move || UnixStream::connect(&path)))
    }

    pub fn with_connector(client: &str, opts: IpcOptions, connect: Connector) -> Self {
        Self {
            client: client.to_string(),
            connect,
            opts,
            conn: None,
            next_id: 0,
            last_seq: 0,
        }
    }

    /// 呼び出し側へ渡した最後の通知 seq（ack の値）。
    pub fn acked_seq(&self) -> u64 {
        self.last_seq
    }

    /// `ack` を単独で送る。
    pub fn flush_ack(&mut self) -> io::Result<()> {
        self.call(Op::Ack).map(|_| ())
    }

    /// server を止める（応答を待つ。切断済みなら Err）。
    pub fn shutdown_server(&mut self) -> io::Result<()> {
        self.call(Op::Shutdown).map(|_| ())
    }

    fn open(&self) -> io::Result<Conn> {
        let s = (self.connect)()?;
        s.set_read_timeout(Some(self.opts.io_timeout))?;
        s.set_write_timeout(Some(self.opts.io_timeout))?;
        Ok(Conn {
            read: BufReader::new(s.try_clone()?),
            write: s,
        })
    }

    fn exchange(&mut self, line: &str, id: u64) -> io::Result<Response> {
        if self.conn.is_none() {
            self.conn = Some(self.open()?);
        }
        let c = self
            .conn
            .as_mut()
            .ok_or_else(|| io::Error::other("no connection"))?;
        c.write.write_all(line.as_bytes())?;
        c.write.flush()?;
        loop {
            let mut buf = String::new();
            if c.read.read_line(&mut buf)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "ipc server closed the connection",
                ));
            }
            let resp: Response = serde_json::from_str(buf.trim_end()).map_err(io::Error::other)?;
            if resp.id == id || resp.id == 0 {
                return Ok(resp);
            }
            // 古い id の応答（再送前の遅れた応答）は読み捨てる。
        }
    }

    fn call(&mut self, op: Op) -> io::Result<Body> {
        self.next_id += 1;
        let id = self.next_id;
        let req = Request {
            v: PROTOCOL_VERSION,
            client: self.client.clone(),
            id,
            ack: self.last_seq,
            op,
        };
        let mut line = serde_json::to_string(&req).map_err(io::Error::other)?;
        line.push('\n');
        let mut last_err = io::Error::other("no attempt");
        for _ in 0..=self.opts.retries {
            match self.exchange(&line, id) {
                Ok(resp) => {
                    return match resp.body {
                        Body::Error { message } => Err(io::Error::other(message)),
                        b => Ok(b),
                    };
                }
                Err(e) => {
                    // 接続を捨てて張り直し、同じ id で再送する（server が二重実行を防ぐ）。
                    self.conn = None;
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }
}

fn unexpected(b: Body) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("unexpected ipc reply: {b:?}"),
    )
}

impl ExtBackend for IpcExt {
    fn submit(
        &mut self,
        logical: &str,
        job_secs: u64,
    ) -> io::Result<Result<SubmitReply, SubmitError>> {
        match self.call(Op::Submit {
            logical: logical.into(),
            job_secs,
        })? {
            Body::Submit { reply } => Ok(reply),
            b => Err(unexpected(b)),
        }
    }

    fn wait_started(&mut self, logical: &str) -> io::Result<()> {
        match self.call(Op::WaitStarted {
            logical: logical.into(),
        })? {
            Body::Unit => Ok(()),
            b => Err(unexpected(b)),
        }
    }

    fn lookup(&mut self, logical: &str) -> io::Result<Lookup> {
        match self.call(Op::Lookup {
            logical: logical.into(),
        })? {
            Body::Lookup { found } => Ok(found),
            b => Err(unexpected(b)),
        }
    }

    fn advance(&mut self, now_secs: u64) -> io::Result<Vec<Notice>> {
        match self.call(Op::Advance { now_secs })? {
            Body::Advance { notices } => {
                let fresh: Vec<_> = notices
                    .into_iter()
                    .filter(|n| n.seq > self.last_seq)
                    .collect();
                if let Some(m) = fresh.iter().map(|n| n.seq).max() {
                    self.last_seq = m;
                }
                Ok(fresh.into_iter().map(|n| n.notice).collect())
            }
            b => Err(unexpected(b)),
        }
    }

    fn record_consumed(&mut self, logical: &str) -> io::Result<bool> {
        match self.call(Op::RecordConsumed {
            logical: logical.into(),
        })? {
            Body::Consumed { first } => Ok(first),
            b => Err(unexpected(b)),
        }
    }

    fn now(&mut self) -> io::Result<u64> {
        match self.call(Op::Now)? {
            Body::Now { secs } => Ok(secs),
            b => Err(unexpected(b)),
        }
    }

    fn due_secs(&mut self, logical: &str) -> io::Result<Option<u64>> {
        match self.call(Op::DueSecs {
            logical: logical.into(),
        })? {
            Body::DueSecs { secs } => Ok(secs),
            b => Err(unexpected(b)),
        }
    }
}
