//! ScriptRuntime actor の統合試験。業務 TS は使わず、固定の試験用 JS 文字列で確かめる。

use std::time::Duration;

use celeris_script::{
    DEFAULT_INPUT_LIMIT_BYTES, MAX_IN_FLIGHT, QUEUE_CAPACITY, RuntimeStats, ScriptCall,
    ScriptError, ScriptRuntimeHandle, ScriptSource,
};
use serde_json::json;

/// 試験用の script。domain ごとに振る舞いを変える。
const FIXTURE: &str = r#"
let gate = null;
let gateResolve = null;
function openGate() {
  if (gate === null) {
    gate = new Promise((resolve) => { gateResolve = resolve; });
  }
  return gate;
}
globalThis.__celerisDispatch = (domain, input) => {
  switch (domain) {
    case "add":
      return { sum: input.a + input.b };
    case "async_add":
      return (async () => {
        await null;
        await Promise.resolve();
        return { sum: input.a + input.b };
      })();
    // 別の要求（release）が来るまで解決しない。
    case "wait_gate":
      return openGate().then((v) => ({ released: v, tag: input.tag }));
    case "release":
      openGate();
      gateResolve(input.value);
      return { ok: true };
    case "hold":
      return new Promise(() => {});
    case "throw":
      throw new Error("boom: " + input.why);
    case "reject":
      return Promise.reject(new Error("rejected: " + input.why));
    case "undefined":
      return undefined;
    case "set_mark":
      globalThis.mark = input.mark;
      return { ok: true };
    case "get_mark":
      return { mark: globalThis.mark ?? null };
    // 同期の無限ループ。watchdog が止めるまで戻らない（試験で 1 本だけ使う）。
    case "spin":
      globalThis.mark = input.mark;
      for (;;) {}
    // 小さな割り当てを続けて heap 上限に当てる。上限で止まるまで戻らない。
    case "grow": {
      const keep = [];
      for (;;) { keep.push(new Array(1024).fill(input.n)); }
    }
    // 1 回の巨大な割り当て（cons string の平坦化）。heap 上限の猶予でも足りず V8 が process を落とす。
    case "oom": {
      const s = "ab".repeat(input.half_len).toUpperCase();
      return { c: s.charCodeAt(s.length - 1) };
    }
    case "len":
      return { len: input.s.length };
    default:
      throw new Error("unknown domain " + domain);
  }
};
"#;

fn start() -> ScriptRuntimeHandle {
    ScriptRuntimeHandle::start(ScriptSource {
        name: "celeris:test/fixture.js".into(),
        code: FIXTURE.into(),
    })
    .expect("start runtime")
}

fn call(domain: &str, input: serde_json::Value) -> ScriptCall {
    ScriptCall {
        domain: domain.into(),
        input,
        timeout: Duration::from_secs(30),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_sync() {
    let rt = start();
    let a = rt
        .call(call("add", json!({"a": 2, "b": 40})))
        .await
        .unwrap();
    assert_eq!(a.output, json!({"sum": 42}));
    let b = rt.call(call("add", json!({"a": 1, "b": 1}))).await.unwrap();
    assert_eq!(b.output, json!({"sum": 2}));
    // 要求ごとに別の ULID。
    assert_ne!(a.request_id, b.request_id);
    // 状態は呼び出しの間で isolate に残る（同じ runtime を使い続ける）。
    assert_eq!(rt.stats().in_flight, 0);
    rt.shutdown();
    assert_eq!(
        rt.call(call("add", json!({"a": 1, "b": 1}))).await,
        Err(ScriptError::Terminated)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_async() {
    let rt = start();
    let r = rt
        .call(call("async_add", json!({"a": 3, "b": 4})))
        .await
        .unwrap();
    assert_eq!(r.output, json!({"sum": 7}));

    // 先の要求が Promise で待っている間も actor は次の要求を受け、event loop を回し続ける。
    let waiter = {
        let rt = rt.clone();
        tokio::spawn(async move { rt.call(call("wait_gate", json!({"tag": "w"}))).await })
    };
    let mut stats = rt.watch_stats();
    tokio::time::timeout(
        Duration::from_secs(30),
        stats.wait_for(|s| s.in_flight == 1),
    )
    .await
    .expect("waiter in flight")
    .unwrap();
    let rel = rt
        .call(call("release", json!({"value": "go"})))
        .await
        .unwrap();
    assert_eq!(rel.output, json!({"ok": true}));
    let w = waiter.await.unwrap().unwrap();
    assert_eq!(w.output, json!({"released": "go", "tag": "w"}));
    rt.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_exception() {
    let rt = start();
    match rt.call(call("throw", json!({"why": "sync"}))).await {
        Err(ScriptError::Exception { message }) => {
            assert!(message.contains("boom: sync"), "{message}")
        }
        other => panic!("unexpected {other:?}"),
    }
    match rt.call(call("reject", json!({"why": "async"}))).await {
        Err(ScriptError::Exception { message }) => {
            assert!(message.contains("rejected: async"), "{message}")
        }
        other => panic!("unexpected {other:?}"),
    }
    match rt.call(call("undefined", json!(null))).await {
        Err(ScriptError::InvalidOutput { .. }) => {}
        other => panic!("unexpected {other:?}"),
    }
    // 例外の後も runtime は使える。
    let ok = rt.call(call("add", json!({"a": 5, "b": 5}))).await.unwrap();
    assert_eq!(ok.output, json!({"sum": 10}));

    // 起動時の script の例外と入口の欠落は start が返す。
    let bad = ScriptRuntimeHandle::start(ScriptSource {
        name: "celeris:test/bad.js".into(),
        code: "throw new Error('load failed');".into(),
    });
    match bad {
        Err(ScriptError::Exception { message }) => assert!(message.contains("load failed")),
        Err(other) => panic!("unexpected {other:?}"),
        Ok(_) => panic!("start must fail"),
    }
    let missing = ScriptRuntimeHandle::start(ScriptSource {
        name: "celeris:test/missing.js".into(),
        code: "globalThis.x = 1;".into(),
    });
    assert!(matches!(missing, Err(ScriptError::InvalidOutput { .. })));
    rt.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_busy() {
    let rt = start();
    // 解決しない要求で in-flight を上限まで埋める。
    let mut held = Vec::new();
    for _ in 0..MAX_IN_FLIGHT {
        let rt = rt.clone();
        held.push(tokio::spawn(async move {
            rt.call(call("hold", json!(null))).await
        }));
    }
    let mut stats = rt.watch_stats();
    tokio::time::timeout(
        Duration::from_secs(30),
        stats.wait_for(|s| s.in_flight == MAX_IN_FLIGHT),
    )
    .await
    .expect("in-flight reaches the limit")
    .unwrap();

    // actor はもう queue から取り出さないので、queue が容量まで埋まる。
    let mut queued = Vec::new();
    for _ in 0..QUEUE_CAPACITY {
        let rt = rt.clone();
        queued.push(tokio::spawn(async move {
            rt.call(call("add", json!({"a": 1, "b": 2}))).await
        }));
    }
    for _ in 0..1000 {
        if rt.queued() == QUEUE_CAPACITY {
            break;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(rt.queued(), QUEUE_CAPACITY);

    // 満杯なら待たずに Busy。
    assert_eq!(
        rt.call(call("add", json!({"a": 1, "b": 2}))).await,
        Err(ScriptError::Busy)
    );

    // shutdown で未完と queue の要求は Terminated で返る。
    rt.shutdown();
    for h in held.into_iter().chain(queued) {
        assert_eq!(h.await.unwrap(), Err(ScriptError::Terminated));
    }
    assert_eq!(rt.stats().in_flight, 0);
}

/// 監視の数が条件を満たすまで待つ（出来事待ち。上限は保険）。
async fn wait_stats(rt: &ScriptRuntimeHandle, f: impl FnMut(&RuntimeStats) -> bool) {
    let mut stats = rt.watch_stats();
    tokio::time::timeout(Duration::from_secs(60), stats.wait_for(f))
        .await
        .expect("stats condition")
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_cancel() {
    let rt = start();
    // 解決を待つ要求を送り、actor が受け取ってから呼び出し側の future を落とす。
    let waiter = {
        let rt = rt.clone();
        tokio::spawn(async move { rt.call(call("wait_gate", json!({"tag": "c"}))).await })
    };
    wait_stats(&rt, |s| s.in_flight == 1).await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    // actor は oneshot の受け手が落ちたのを見て結果を捨てる。
    wait_stats(&rt, |s| s.in_flight == 0 && s.discarded == 1).await;
    // 捨てた要求の Promise を解いても誰にも届かず、runtime は使い続けられる。
    let rel = rt
        .call(call("release", json!({"value": "late"})))
        .await
        .unwrap();
    assert_eq!(rel.output, json!({"ok": true}));
    let s = rt.stats();
    assert_eq!((s.in_flight, s.discarded, s.generation), (0, 1, 0));
    rt.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_timeout_sync_loop() {
    let rt = start();
    rt.call(call("set_mark", json!({"mark": "before"})))
        .await
        .unwrap();
    // 同期の無限ループ。deadline（注入した 200 ms）で watchdog が V8 を止める。
    let r = rt
        .call(ScriptCall {
            domain: "spin".into(),
            input: json!({"mark": "spinning"}),
            timeout: Duration::from_millis(200),
        })
        .await;
    assert_eq!(r, Err(ScriptError::Timeout));
    wait_stats(&rt, |s| s.watchdog_terminations == 1 && s.generation == 1).await;
    assert_eq!(rt.stats().heap_limit_hits, 0);
    // 止めた後の要求は通る。止めた isolate は再利用せず作り直したので global は初期状態。
    let after = rt
        .call(call("add", json!({"a": 20, "b": 22})))
        .await
        .unwrap();
    assert_eq!(after.output, json!({"sum": 42}));
    assert_eq!(after.generation, 1);
    let mark = rt.call(call("get_mark", json!(null))).await.unwrap();
    assert_eq!(mark.output, json!({"mark": null}));
    let s = rt.stats();
    assert_eq!((s.restarts, s.timeouts, s.in_flight), (1, 1, 0));
    rt.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_restart() {
    let rt = start();
    rt.call(call("set_mark", json!({"mark": "g0"})))
        .await
        .unwrap();
    let g0 = rt.call(call("get_mark", json!(null))).await.unwrap();
    assert_eq!((g0.generation, g0.output), (0, json!({"mark": "g0"})));
    assert_eq!(rt.restart().await, Ok(1));
    let g1 = rt.call(call("get_mark", json!(null))).await.unwrap();
    assert_eq!((g1.generation, g1.output), (1, json!({"mark": null})));
    assert_eq!(rt.restart().await, Ok(2));
    let s = rt.stats();
    assert_eq!(
        (s.generation, s.restarts, s.watchdog_terminations),
        (2, 2, 0)
    );
    rt.shutdown();
    assert_eq!(rt.restart().await, Err(ScriptError::Terminated));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_generation_discard() {
    let rt = start();
    // generation 0 で Promise を待つ要求を 2 本。
    let waiters: Vec<_> = (0..2)
        .map(|i| {
            let rt = rt.clone();
            tokio::spawn(async move { rt.call(call("wait_gate", json!({"tag": i}))).await })
        })
        .collect();
    wait_stats(&rt, |s| s.in_flight == 2).await;
    assert_eq!(rt.restart().await, Ok(1));
    // 古い generation の未完の要求は結果を受け取らず GenerationGone。
    for w in waiters {
        assert_eq!(w.await.unwrap(), Err(ScriptError::GenerationGone));
    }
    // 新しい generation で gate を開けても古い要求の結果は出ない。新しい要求は新しい gate で動く。
    let fresh = {
        let rt = rt.clone();
        tokio::spawn(async move { rt.call(call("wait_gate", json!({"tag": "new"}))).await })
    };
    wait_stats(&rt, |s| s.in_flight == 1 && s.generation == 1).await;
    rt.call(call("release", json!({"value": "g1"})))
        .await
        .unwrap();
    let f = fresh.await.unwrap().unwrap();
    assert_eq!(
        (f.generation, f.output),
        (1, json!({"released": "g1", "tag": "new"}))
    );
    let s = rt.stats();
    assert_eq!((s.in_flight, s.failures), (0, 2));
    rt.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_heap_limit() {
    let rt = start();
    rt.call(call("set_mark", json!({"mark": "g0"})))
        .await
        .unwrap();
    // 64 MiB の heap 上限に当たると near_heap_limit_callback が実行を止める。process は落ちない。
    let r = rt.call(call("grow", json!({"n": 1.5}))).await;
    assert_eq!(r, Err(ScriptError::HeapLimit));
    wait_stats(&rt, |s| s.generation == 1).await;
    let s = rt.stats();
    assert_eq!(
        (s.heap_limit_hits, s.generation, s.watchdog_terminations),
        (1, 1, 0)
    );
    // 作り直した generation で要求が通る。
    let after = rt.call(call("get_mark", json!(null))).await.unwrap();
    assert_eq!((after.generation, after.output), (1, json!({"mark": null})));
    rt.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_input_limit() {
    let rt = start();
    // domain "len" (3 byte) + JSON {"s":"…"}（8 byte + 本文）がちょうど上限。
    let fits = "x".repeat(DEFAULT_INPUT_LIMIT_BYTES - 3 - 8);
    let ok = rt.call(call("len", json!({"s": fits}))).await.unwrap();
    assert_eq!(ok.output, json!({"len": DEFAULT_INPUT_LIMIT_BYTES - 11}));
    // 1 byte 超えると actor に送らずに拒む。
    let over = "x".repeat(DEFAULT_INPUT_LIMIT_BYTES - 3 - 8 + 1);
    assert_eq!(
        rt.call(call("len", json!({"s": over}))).await,
        Err(ScriptError::InputTooLarge {
            size: DEFAULT_INPUT_LIMIT_BYTES + 1,
            limit: DEFAULT_INPUT_LIMIT_BYTES,
        })
    );
    let s = rt.stats();
    assert_eq!((s.failures, s.completed), (1, 1));
    rt.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_inflight_limit() {
    let rt = start();
    // 解決を待つ要求で in-flight を上限まで埋める。
    let mut waiters = Vec::new();
    for i in 0..MAX_IN_FLIGHT {
        let rt = rt.clone();
        waiters.push(tokio::spawn(async move {
            rt.call(call("wait_gate", json!({"tag": i}))).await
        }));
    }
    wait_stats(&rt, |s| s.in_flight == MAX_IN_FLIGHT).await;
    // 上限を超える要求は actor が取り出さず queue に留まる（release もまだ動かない）。
    let release = {
        let rt = rt.clone();
        tokio::spawn(async move { rt.call(call("release", json!({"value": "open"}))).await })
    };
    for _ in 0..1000 {
        if rt.queued() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let s = rt.stats();
    assert_eq!((s.in_flight, s.queued), (MAX_IN_FLIGHT, 1));
    assert!(!release.is_finished());
    // 1 本 cancel すると枠が空き、queue の release が動いて残り全部が解ける。
    waiters.remove(0).abort();
    assert_eq!(release.await.unwrap().unwrap().output, json!({"ok": true}));
    for w in waiters {
        assert_eq!(w.await.unwrap().unwrap().output["released"], json!("open"));
    }
    let s = rt.stats();
    assert_eq!((s.in_flight, s.queued, s.discarded), (0, 0, 1));
    rt.shutdown();
}

const OOM_CHILD_ENV: &str = "CELERIS_SCRIPT_OOM_CHILD";

/// 破壊的な OOM は試験 binary 自身を子 process で起こし、親は子の異常終了だけを確かめる。
#[test]
fn runtime_oom_child_process() {
    if std::env::var_os(OOM_CHILD_ENV).is_some() {
        oom_child();
        return;
    }
    let exe = std::env::current_exe().expect("test binary path");
    let out = std::process::Command::new(exe)
        .args([
            "--exact",
            "runtime_oom_child_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(OOM_CHILD_ENV, "1")
        .output()
        .expect("spawn child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let detail = format!("status={:?}\nstdout={stdout}\nstderr={stderr}", out.status);
    assert!(stdout.contains("OOM_CHILD_READY"), "{detail}");
    assert!(!stdout.contains("OOM_CHILD_SURVIVED"), "{detail}");
    assert!(!out.status.success(), "{detail}");
    // panic（exit 101）ではなく V8 が process を signal で落とした（Fatal JavaScript out of memory）。
    assert_ne!(out.status.code(), Some(101), "{detail}");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert!(out.status.signal().is_some(), "{detail}");
    }
}

fn oom_child() {
    use std::io::Write;
    let rt = start();
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio");
    println!("OOM_CHILD_READY");
    let _ = std::io::stdout().flush();
    // 256 Mi 文字の平坦化と大文字化 = 256 MiB の割り当て。上限 64 MiB と 1 回だけの猶予 128 MiB を超える。
    let r = tokio_rt.block_on(rt.call(call("oom", json!({"half_len": 1u64 << 27}))));
    println!("OOM_CHILD_SURVIVED {r:?}");
    let _ = std::io::stdout().flush();
}
