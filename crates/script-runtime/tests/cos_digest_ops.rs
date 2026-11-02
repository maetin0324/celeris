//! `cos_digest:list_runs`・`cos_digest:read_run` の op（境界 ADR 2026-11-02 の付記 S4-12・S4-13・S4-18）。
//!
//! 試験は固定の試験用 bundle（ESM）と、試験の中で書いた op server（host sink の受け手）で確かめる。
//! 時間に頼る待ちは使わない。op server が要求を受け取ったこと・応答口が閉じたことを出来事として待つ。

use std::collections::BTreeSet;
use std::time::Duration;

use celeris_script::dto::{Capability, HostError, HostErrorCode, HostResult};
use celeris_script::{
    HostOpCall, MAX_OPS_PER_REQUEST, ScriptCall, ScriptError, ScriptRuntimeHandle, ScriptSource,
    domain_capabilities, host_sink,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// input.op で振る舞いを変える試験用 bundle。
const FIXTURE: &str = r#"
import { cosDigestListRuns, cosDigestReadRun, HostError } from "celeris:host";

let started = 0;
let stolen = null;
let parked = null;

async function settle(p) {
  try {
    return { ok: true, value: await p };
  } catch (e) {
    return { ok: false, name: e.name, code: e.code ?? null, host_error: e instanceof HostError };
  }
}

globalThis.__celerisDispatch = async (domain, input, ctx) => {
  switch (input.op) {
    // list を読み、一覧の run を順に read する（pass の形）。
    case "pass": {
      started += 1;
      const list = await cosDigestListRuns(ctx, {});
      const runs = [];
      for (const r of list.runs) {
        runs.push(await settle(cosDigestReadRun(ctx, { thread: r.thread, run_id: r.run_id })));
      }
      return { list, runs };
    }
    case "list":
      started += 1;
      return await settle(cosDigestListRuns(ctx, input.request ?? {}));
    case "read":
      return await settle(cosDigestReadRun(ctx, input.request));
    // op を呼んだまま待たずに返す。後の要求が "parked_result" で結果を読む。
    case "park":
      stolen = ctx.token;
      parked = settle(cosDigestListRuns(ctx, {}));
      return { parked: true };
    case "parked_result":
      return await parked;
    // 前の要求の token を使って op を呼ぶ。
    case "use_stolen":
      return await settle(cosDigestListRuns({ token: stolen }, {}));
    case "steal":
      stolen = ctx.token;
      return { stolen: true };
    case "many": {
      let ok = 0;
      for (let i = 0; i < input.n; i++) {
        const r = await settle(cosDigestListRuns(ctx, {}));
        if (!r.ok) return { ok, failed_at: i + 1, code: r.code };
        ok += 1;
      }
      return { ok, failed_at: null, code: null };
    }
    case "started":
      return { started };
    default:
      throw new Error("unknown op " + input.op);
  }
};
"#;

const ALL: &[Capability] = &[
    Capability::SelftestEchoTask,
    Capability::CosDigestListRuns,
    Capability::CosDigestReadRun,
];

fn start(capabilities: &[Capability]) -> ScriptRuntimeHandle {
    ScriptRuntimeHandle::start(ScriptSource {
        name: "celeris:test/cos-digest-ops.js".into(),
        code: FIXTURE.into(),
        capabilities: capabilities.iter().copied().collect(),
    })
    .expect("start fixture")
}

fn call(
    domain: &str,
    input: Value,
    sink: Option<celeris_script::HostSink>,
    timeout: Duration,
) -> ScriptCall {
    ScriptCall {
        domain: domain.into(),
        input,
        scope_task_ids: BTreeSet::new(),
        timeout,
        host_sink: sink,
    }
}

const LONG: Duration = Duration::from_secs(30);

fn list_reply() -> Value {
    json!({"runs": [
        {"thread": "th-1", "run_id": "run-a", "run_state": "completed"},
        {"thread": "th-1", "run_id": "run-b", "run_state": "stopped"},
    ]})
}

fn read_reply(thread: &str, run_id: &str, run_state: &str) -> Value {
    json!({
        "thread": thread,
        "run_id": run_id,
        "run_state": run_state,
        "reason": null,
        "items": [{
            "item_id": "item-1",
            "thread_id": thread,
            "source_kind": "decision",
            "source_key": "k",
            "source_revision": "r1",
            "state": "running",
            "summary": "s",
            "detail": "",
            "reason": null,
            "operation_id": null,
            "run_id": run_id,
            "created_at": "2026-11-02T00:00:00Z",
            "route": "escalation",
            "decision": {
                "summary": "d",
                "background": null,
                "on_answer": null,
                "options": [{"key": "a", "label": "A", "detail": null}],
                "recommended": "a",
                "recommendation_reason": null,
                "web_path": "/inbox/1",
            },
        }],
    })
}

/// 受け取った op 呼び出しの記録（op の名前と引数）。
async fn recv(rx: &mut mpsc::Receiver<HostOpCall>) -> HostOpCall {
    tokio::time::timeout(LONG, rx.recv())
        .await
        .expect("op call within the insurance timeout")
        .expect("sink open")
}

fn ok(value: Value) -> HostResult<Value> {
    HostResult::Ok { value }
}

/// 許可表と manifest の両方に無ければ op は通らない（付記 S4-13）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cos_digest_ops_denied_without_manifest_declaration() {
    assert_eq!(
        domain_capabilities("cos_digest"),
        &[Capability::CosDigestListRuns, Capability::CosDigestReadRun]
    );
    assert!(domain_capabilities("lane_rules").is_empty());
    assert!(domain_capabilities("route_priority").is_empty());

    // manifest が宣言していない: 許可表にあっても Denied。sink には何も届かない。
    let rt = start(&[Capability::SelftestEchoTask]);
    let (sink, mut rx) = host_sink();
    let r = rt
        .call(call("cos_digest", json!({"op": "list"}), Some(sink), LONG))
        .await
        .expect("script returns");
    assert_eq!(
        r.output,
        json!({"ok": false, "name": "HostError", "code": "denied", "host_error": true})
    );
    // list だけ宣言して read を宣言しない: read だけ Denied。
    rt.shutdown();
    let rt = start(&[Capability::CosDigestListRuns]);
    let (sink2, _rx2) = host_sink();
    let r = rt
        .call(call(
            "cos_digest",
            json!({"op": "read", "request": {"thread": "th-1", "run_id": "run-a"}}),
            Some(sink2),
            LONG,
        ))
        .await
        .expect("script returns");
    assert_eq!(r.output["code"], json!("denied"));
    assert!(rx.try_recv().is_err(), "denied ops must not reach the sink");
    rt.shutdown();
}

/// op は引数を DTO で検証して sink へ送り、sink の応答を DTO で検証して script に返す（付記 S4-12）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cos_digest_ops_forward_to_host_sink_and_return_reply() {
    let rt = start(ALL);
    let (sink, mut rx) = host_sink();
    let pass = {
        let rt = rt.clone();
        let sink = sink.clone();
        tokio::spawn(async move {
            rt.call(call("cos_digest", json!({"op": "pass"}), Some(sink), LONG))
                .await
        })
    };
    let list = recv(&mut rx).await;
    assert_eq!(list.op, Capability::CosDigestListRuns);
    assert_eq!(list.request, json!({}));
    let request_id = list.request_id;
    list.reply.send(ok(list_reply())).expect("reply list");
    let a = recv(&mut rx).await;
    assert_eq!(a.op, Capability::CosDigestReadRun);
    assert_eq!(a.request, json!({"thread": "th-1", "run_id": "run-a"}));
    assert_eq!(a.request_id, request_id);
    a.reply
        .send(ok(read_reply("th-1", "run-a", "completed")))
        .expect("reply a");
    // 2 つ目の read は store の読み出しの失敗（op の error）として返す。script が catch する。
    let b = recv(&mut rx).await;
    assert_eq!(b.request, json!({"thread": "th-1", "run_id": "run-b"}));
    b.reply
        .send(HostResult::Error {
            error: HostError::new(HostErrorCode::Exception, "read failed"),
        })
        .expect("reply b");
    let out = pass.await.expect("join").expect("pass");
    assert_eq!(out.request_id, request_id);
    assert_eq!(out.output["list"], list_reply());
    assert_eq!(
        out.output["runs"],
        json!([
            {"ok": true, "value": read_reply("th-1", "run-a", "completed")},
            {"ok": false, "name": "HostError", "code": "exception", "host_error": true},
        ])
    );

    // 引数の形の誤り（未知の欄）は sink へ送らずに invalid_output。
    let r = rt
        .call(call(
            "cos_digest",
            json!({"op": "list", "request": {"limit": 100}}),
            Some(sink.clone()),
            LONG,
        ))
        .await
        .expect("script returns");
    assert_eq!(r.output["code"], json!("invalid_output"));
    let r = rt
        .call(call(
            "cos_digest",
            json!({"op": "read", "request": {"thread": "th-1"}}),
            Some(sink.clone()),
            LONG,
        ))
        .await
        .expect("script returns");
    assert_eq!(r.output["code"], json!("invalid_output"));
    assert!(
        rx.try_recv().is_err(),
        "malformed requests must not reach the sink"
    );

    // sink の応答が DTO に合わなければ exception（script に渡さない）。
    let bad = {
        let rt = rt.clone();
        let sink = sink.clone();
        tokio::spawn(async move {
            rt.call(call("cos_digest", json!({"op": "list"}), Some(sink), LONG))
                .await
        })
    };
    let c = recv(&mut rx).await;
    c.reply
        .send(ok(json!({"runs": [{"thread": "t", "run_id": "r"}]})))
        .expect("reply");
    let out = bad.await.expect("join").expect("script returns");
    assert_eq!(out.output["code"], json!("exception"));

    // sink の無い要求は exception。
    let r = rt
        .call(call("cos_digest", json!({"op": "list"}), None, LONG))
        .await
        .expect("script returns");
    assert_eq!(r.output["code"], json!("exception"));
    rt.shutdown();
}

/// op は自分の領域の要求の token でだけ通る（付記 S4-13 の 2）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cos_digest_ops_grant_bound_to_domain() {
    let rt = start(ALL);
    let (sink, mut rx) = host_sink();
    // 許可表が空の領域の要求が cos_digest の op を呼ぶ。
    for domain in ["lane_rules", "route_priority", "host_selftest"] {
        let r = rt
            .call(call(
                domain,
                json!({"op": "list"}),
                Some(sink.clone()),
                LONG,
            ))
            .await
            .expect("script returns");
        assert_eq!(r.output["code"], json!("denied"), "{domain}");
    }
    assert!(rx.try_recv().is_err(), "denied ops must not reach the sink");
    rt.shutdown();
}

/// 要求が終わった後に op の結果を渡さない（付記 S4-18 の cancel）。待っている op は terminated、終わった要求の
/// token は denied。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cos_digest_ops_terminated_after_request_end() {
    let rt = start(ALL);
    let (sink, mut rx) = host_sink();
    // op を呼んだまま要求を返す。
    let r = rt
        .call(call(
            "cos_digest",
            json!({"op": "park"}),
            Some(sink.clone()),
            LONG,
        ))
        .await
        .expect("park");
    assert_eq!(r.output, json!({"parked": true}));
    let mut held = recv(&mut rx).await;
    // 要求が終わったので op は応答を待たずに terminated で返り、応答口を落とす。
    tokio::time::timeout(LONG, held.reply.closed())
        .await
        .expect("op gives up the reply after the request ended");
    let r = rt
        .call(call("probe", json!({"op": "parked_result"}), None, LONG))
        .await
        .expect("parked result");
    assert_eq!(
        r.output,
        json!({"ok": false, "name": "HostError", "code": "terminated", "host_error": true})
    );
    // 終わった要求の token を後の要求が使う: 知らない token なので denied。sink には届かない。
    let r = rt
        .call(call(
            "cos_digest",
            json!({"op": "use_stolen"}),
            Some(sink),
            LONG,
        ))
        .await
        .expect("script returns");
    assert_eq!(r.output["code"], json!("denied"));
    assert!(rx.try_recv().is_err());

    // 呼び出し側が future を落とした（cancel）: 待っている op は応答口を落とす。
    let (sink, mut rx) = host_sink();
    let pending = {
        let rt = rt.clone();
        tokio::spawn(async move {
            rt.call(call("cos_digest", json!({"op": "list"}), Some(sink), LONG))
                .await
        })
    };
    let mut held = recv(&mut rx).await;
    pending.abort();
    tokio::time::timeout(LONG, held.reply.closed())
        .await
        .expect("op gives up the reply after cancel");
    rt.shutdown();
}

/// 要求の deadline で op の待ちを止める（付記 S4-18 の deadline）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cos_digest_ops_timeout_at_deadline() {
    let rt = start(ALL);
    let (sink, mut rx) = host_sink();
    let pending = {
        let rt = rt.clone();
        tokio::spawn(async move {
            rt.call(call(
                "cos_digest",
                json!({"op": "list"}),
                Some(sink),
                Duration::from_millis(200),
            ))
            .await
        })
    };
    // op server は受け取ったまま答えない。
    let mut held = recv(&mut rx).await;
    let r = pending.await.expect("join");
    assert_eq!(r, Err(ScriptError::Timeout));
    tokio::time::timeout(LONG, held.reply.closed())
        .await
        .expect("op gives up the reply at the deadline");
    // 後の要求は普通に動く。
    let (sink, mut rx) = host_sink();
    let next = {
        let rt = rt.clone();
        tokio::spawn(async move {
            rt.call(call("cos_digest", json!({"op": "list"}), Some(sink), LONG))
                .await
        })
    };
    let c = recv(&mut rx).await;
    c.reply.send(ok(json!({"runs": []}))).expect("reply");
    let out = next.await.expect("join").expect("next call");
    assert_eq!(out.output, json!({"ok": true, "value": {"runs": []}}));
    rt.shutdown();
}

/// generation を作り直したら、待っている op の結果を誰にも渡さない（付記 S4-18 の generation_gone）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cos_digest_ops_generation_gone_on_rebuild() {
    let rt = start(ALL);
    let (sink, mut rx) = host_sink();
    let pending = {
        let rt = rt.clone();
        tokio::spawn(async move {
            rt.call(call("cos_digest", json!({"op": "list"}), Some(sink), LONG))
                .await
        })
    };
    let mut held = recv(&mut rx).await;
    let generation = rt.restart().await.expect("restart");
    assert_eq!(generation, 1);
    assert_eq!(
        pending.await.expect("join"),
        Err(ScriptError::GenerationGone)
    );
    // 古い generation の op は落ちていて、応答しても誰にも届かない。
    tokio::time::timeout(LONG, held.reply.closed())
        .await
        .expect("old generation dropped the op");
    assert!(held.reply.send(ok(list_reply())).is_err());
    // 新しい generation の要求は通る。
    let (sink, mut rx) = host_sink();
    let next = {
        let rt = rt.clone();
        tokio::spawn(async move {
            rt.call(call("cos_digest", json!({"op": "list"}), Some(sink), LONG))
                .await
        })
    };
    let c = recv(&mut rx).await;
    c.reply.send(ok(list_reply())).expect("reply");
    let out = next.await.expect("join").expect("next");
    assert_eq!(out.generation, 1);
    assert_eq!(out.output["value"], list_reply());
    rt.shutdown();
}

/// capability を持つ要求は isolate の中で同時に 1 件まで。capability の無い要求は並行して走る（付記 S4-13 の 1）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn capability_requests_are_exclusive_per_isolate() {
    let rt = start(ALL);
    let (sink1, mut rx1) = host_sink();
    let (sink2, mut rx2) = host_sink();
    let first = {
        let rt = rt.clone();
        tokio::spawn(async move {
            rt.call(call("cos_digest", json!({"op": "list"}), Some(sink1), LONG))
                .await
        })
    };
    let held = recv(&mut rx1).await;
    // second を 1 回だけ poll して queue に積む。後の probe は FIFO なので、probe が返った時点で second は
    // actor に取り出されている（排他のため held に置かれる）。
    let second = rt.call(call("cos_digest", json!({"op": "list"}), Some(sink2), LONG));
    tokio::pin!(second);
    tokio::select! {
        biased;
        r = &mut second => panic!("second finished before first: {r:?}"),
        _ = std::future::ready(()) => {}
    }
    // capability の無い要求は first が走っている間に通る。second はまだ始まっていない（started は 1）。
    let probe = rt
        .call(call("probe", json!({"op": "started"}), None, LONG))
        .await
        .expect("pure request runs concurrently");
    assert_eq!(probe.output, json!({"started": 1}));
    assert!(
        rx2.try_recv().is_err(),
        "second must not start while first runs"
    );
    assert_eq!(rt.stats().in_flight, 1);
    // first が終わると second が始まる。
    held.reply.send(ok(json!({"runs": []}))).expect("reply 1");
    assert_eq!(
        first.await.expect("join").expect("first").output,
        json!({"ok": true, "value": {"runs": []}})
    );
    let c = recv(&mut rx2).await;
    c.reply.send(ok(list_reply())).expect("reply 2");
    assert_eq!(second.await.expect("second").output["value"], list_reply());
    let probe = rt
        .call(call("probe", json!({"op": "started"}), None, LONG))
        .await
        .expect("probe");
    assert_eq!(probe.output, json!({"started": 2}));
    rt.shutdown();
}

/// 1 回の要求の op は [`MAX_OPS_PER_REQUEST`] 回まで。超えた分は busy（付記 S4-12）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cos_digest_ops_busy_after_64_calls() {
    assert_eq!(MAX_OPS_PER_REQUEST, 64);
    let rt = start(ALL);
    let (sink, mut rx) = host_sink();
    let server = tokio::spawn(async move {
        let mut served = 0u32;
        while let Some(c) = rx.recv().await {
            served += 1;
            let _ = c.reply.send(ok(json!({"runs": []})));
        }
        served
    });
    let r = rt
        .call(call(
            "cos_digest",
            json!({"op": "many", "n": 70}),
            Some(sink),
            LONG,
        ))
        .await
        .expect("script returns");
    assert_eq!(r.output, json!({"ok": 64, "failed_at": 65, "code": "busy"}));
    rt.shutdown();
    assert_eq!(server.await.expect("join"), 64);
}
