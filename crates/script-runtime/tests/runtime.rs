//! ScriptRuntime actor の統合試験。業務 TS は使わず、固定の試験用 JS 文字列で確かめる。

use std::time::Duration;

use celeris_script::{
    MAX_IN_FLIGHT, QUEUE_CAPACITY, ScriptCall, ScriptError, ScriptRuntimeHandle, ScriptSource,
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
