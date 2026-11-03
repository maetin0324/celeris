//! generation の登録簿（境界 ADR 2026-11-02 の付記 S5-1・S5-2・S5-5）。
//!
//! bundle は試験の中で書いた手書きの JS（業務 TS ではない）。時間は [`ManualClock`] を進めて数え、
//! 実時間の sleep・CPU を使う待ちは使わない。in-flight の要求は「解決しない Promise」か
//! 「試験が答えるまで待つ host op」で作り、試験は op server が要求を受け取った出来事を待つ。

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use celeris_script::dto::HostResult;
use celeris_script::generation::{
    DRAIN_CUTOFF, MAX_IN_FLIGHT_PER_GENERATION, MAX_LIVE_GENERATIONS, check_checkpoint_versions,
    checkpoint_versions_cover,
};
use celeris_script::manifest::{
    BUNDLE_FILE, MANIFEST_FILE, VerifiedBundle, deploy_bundle, host_schema_sha256, sha256_hex,
    verify_bundle,
};
use celeris_script::{
    GenerationError, GenerationRegistry, HostOpCall, ManualClock, RetireReason, RuntimeLimits,
    ScriptCall, ScriptError, host_sink,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// 試験用 bundle。`__TAG__` を generation ごとに差し替え、bundle の hash を変える。
const FIXTURE: &str = r#"
import { hostVersion, selftestEcho, cosDigestListRuns } from "celeris:host";

const TAG = "__TAG__";
let count = 0;

globalThis.__celerisDispatch = async (domain, input, ctx) => {
  if (domain === "host_selftest") {
    return { host_api: hostVersion(), reply: await selftestEcho(ctx, input) };
  }
  if (domain === "lane_rules") {
    switch (input?.op) {
      case "tag":
        return { tag: TAG };
      case "count":
        count += 1;
        return { tag: TAG, count };
      // 解決しない Promise。CPU は使わない（event loop は待つだけ）。
      case "hang":
        return await new Promise(() => {});
      default:
        throw new Error("bad input");
    }
  }
  if (domain === "cos_digest") {
    if (input === null) throw new Error("bad input");
    const list = await cosDigestListRuns(ctx, {});
    return { tag: TAG, list };
  }
  throw new Error(`unknown domain: ${domain}`);
};
"#;

const ALL_DOMAINS: &[&str] = &["cos_digest", "host_selftest", "lane_rules"];
const ALL_CAPS: &[&str] = &[
    "cos_digest:list_runs",
    "cos_digest:read_run",
    "selftest:echo_task",
];

/// 1 回の要求の上限。試験は時計を進めて打ち切るので、これは保険（実時間）。
const LONG: Duration = Duration::from_secs(600);

fn manifest(sha: &str, domains: &[&str], caps: &[&str], checkpoints: &[u32]) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "celeris.script-manifest/1",
        "bundle_sha256": sha,
        "host_api": "^1",
        "schema_sha256": host_schema_sha256(),
        "toolchain": {"typescript": "fixture", "esbuild": "fixture", "node": "fixture"},
        "source_commit": "fixture",
        "domains": domains,
        "capabilities": caps,
        "checkpoint_versions": checkpoints,
    }))
    .expect("manifest json")
}

fn code(tag: &str) -> String {
    FIXTURE.replace("__TAG__", tag)
}

fn bundle_with(tag: &str, domains: &[&str], caps: &[&str], checkpoints: &[u32]) -> VerifiedBundle {
    let code = code(tag);
    let sha = sha256_hex(code.as_bytes());
    verify_bundle(&manifest(&sha, domains, caps, checkpoints), code.as_bytes())
        .expect("fixture verifies")
}

fn bundle(tag: &str) -> VerifiedBundle {
    bundle_with(tag, ALL_DOMAINS, ALL_CAPS, &[])
}

fn registry() -> (GenerationRegistry, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new());
    let reg = GenerationRegistry::with_clock(RuntimeLimits::default(), clock.clone());
    (reg, clock)
}

fn lane(op: &str) -> ScriptCall {
    ScriptCall {
        domain: "lane_rules".into(),
        input: json!({ "op": op }),
        scope_task_ids: BTreeSet::new(),
        timeout: LONG,
    }
}

fn digest() -> ScriptCall {
    ScriptCall {
        domain: "cos_digest".into(),
        input: json!({}),
        scope_task_ids: BTreeSet::new(),
        timeout: LONG,
    }
}

async fn recv(rx: &mut mpsc::Receiver<HostOpCall>) -> HostOpCall {
    tokio::time::timeout(LONG, rx.recv())
        .await
        .expect("op call within the insurance timeout")
        .expect("sink open")
}

fn empty_list() -> HostResult<Value> {
    HostResult::Ok {
        value: json!({ "runs": [] }),
    }
}

/// in-flight の数。枠は `call` を呼んだ時点で取る（future を poll する前）ので、待たずに読める。
fn in_flight(reg: &GenerationRegistry, id: &str) -> usize {
    reg.get(id).map(|g| g.in_flight).unwrap_or(usize::MAX)
}

/// 登録は generation ごとに新しい isolate を作る: module の状態（`count`）が generation の間で共有されない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_load_uses_new_isolate() {
    let (reg, _) = registry();
    reg.register("gen-a", bundle("a"))
        .await
        .expect("register a");
    for expected in 1..=2 {
        let out = reg
            .call("gen-a", lane("count"), None)
            .await
            .expect("call a");
        assert_eq!(out.reply.output, json!({"tag": "a", "count": expected}));
    }
    // 同じ本文の bundle でも、別の generation は 0 から数える（既存の isolate を使わない）。
    reg.register("gen-b", bundle("b"))
        .await
        .expect("register b");
    let out = reg
        .call("gen-b", lane("count"), None)
        .await
        .expect("call b");
    assert_eq!(out.reply.output, json!({"tag": "b", "count": 1}));
    assert_eq!(out.generation_id, "gen-b");
    reg.shutdown().await;
}

/// 検証に落ちた bundle は登録しない（未知の領域・許されない capability・checkpoint の形・入口が無い・selftest 無し）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_load_rejects_unknown_domain_capability_or_checkpoint() {
    let (reg, _) = registry();
    let cases: Vec<(&str, VerifiedBundle)> = vec![
        (
            "unknown domain",
            bundle_with(
                "u",
                &["host_selftest", "nope"],
                &["selftest:echo_task"],
                &[],
            ),
        ),
        (
            "capability of an undeclared domain",
            bundle_with(
                "c",
                &["host_selftest", "lane_rules"],
                &["selftest:echo_task", "cos_digest:list_runs"],
                &[],
            ),
        ),
        (
            "checkpoint version unknown to the host",
            bundle_with("k", ALL_DOMAINS, ALL_CAPS, &[2]),
        ),
        (
            "checkpoint versions not ascending",
            bundle_with("k2", ALL_DOMAINS, ALL_CAPS, &[1, 1]),
        ),
        (
            "no selftest domain",
            bundle_with("s", &["lane_rules"], &[], &[]),
        ),
        (
            "entry missing for a declared domain",
            bundle_with(
                "e",
                &["host_selftest", "route_priority"],
                &["selftest:echo_task"],
                &[],
            ),
        ),
    ];
    for (name, b) in cases {
        let err = reg
            .register("gen-x", b)
            .await
            .expect_err(&format!("{name}: must be rejected"));
        assert!(err.is_invalid_bundle(), "{name}: {err}");
        assert_eq!(reg.live_count(), 0, "{name}: nothing registered");
    }
    // 入口が無い場合の誤りの種類。
    let err = reg
        .register(
            "gen-x",
            bundle_with(
                "e",
                &["host_selftest", "route_priority"],
                &["selftest:echo_task"],
                &[],
            ),
        )
        .await
        .expect_err("entry missing");
    assert_eq!(
        err,
        GenerationError::EntryMissing {
            domain: "route_priority".into()
        }
    );
    // 検証に落ちた登録は上限の枠を残さない。
    reg.register("gen-a", bundle("a")).await.expect("a");
    reg.register("gen-b", bundle("b")).await.expect("b");
    // 落ちた id への要求は GenerationGone。
    assert_eq!(
        reg.call("gen-x", lane("tag"), None).await,
        Err(ScriptError::GenerationGone)
    );
    reg.shutdown().await;
}

/// 配備先の名前と hash の照合（`register_deployed`）。壊れた配備は登録しない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_register_deployed_checks_name_and_hash() {
    let dist = tempfile::tempdir().expect("dist");
    let state = tempfile::tempdir().expect("state");
    let b = bundle("d");
    std::fs::write(dist.path().join(BUNDLE_FILE), &b.code).expect("bundle");
    std::fs::write(
        dist.path().join(MANIFEST_FILE),
        manifest(&b.manifest.bundle_sha256, ALL_DOMAINS, ALL_CAPS, &[]),
    )
    .expect("manifest");
    deploy_bundle(dist.path(), state.path()).expect("deploy");
    let (reg, _) = registry();
    let other = sha256_hex(b"not deployed");
    let err = reg
        .register_deployed("gen-x", state.path(), &other)
        .await
        .expect_err("missing deploy");
    assert!(matches!(err, GenerationError::Manifest(_)), "{err}");
    let info = reg
        .register_deployed("gen-d", state.path(), &b.manifest.bundle_sha256)
        .await
        .expect("deployed bundle registers");
    assert_eq!(info.bundle_sha256, b.manifest.bundle_sha256);
    reg.shutdown().await;
}

/// 要求は呼んだ時点の generation に固定される。切り替え（drain）の後の要求は新しい generation へ行き、
/// 切り替えの前に受けた要求は古い generation で最後まで走る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_call_pins_requested_generation() {
    let (reg, _) = registry();
    reg.register("gen-a", bundle("a")).await.expect("a");
    let (sink, mut rx) = host_sink();
    // 切り替えの前に A へ送った要求。future を作った時点で A に固定される。
    let pinned = reg.call("gen-a", digest(), Some(sink));
    let pinned = tokio::spawn(pinned);
    let op = recv(&mut rx).await;
    // 新しい generation を足し、A から B へ切り替える（呼び出し側は以後 B を読む）。
    reg.register("gen-b", bundle("b")).await.expect("b");
    reg.drain("gen-a").expect("drain a");
    assert_eq!(
        reg.call("gen-a", lane("tag"), None).await,
        Err(ScriptError::GenerationGone),
        "new requests to the draining generation are refused"
    );
    let b = reg.call("gen-b", lane("tag"), None).await.expect("b call");
    assert_eq!(
        (b.generation_id.as_str(), b.reply.output),
        ("gen-b", json!({"tag": "b"}))
    );
    // A の要求は A の isolate で最後まで走る。
    let _ = op.reply.send(empty_list());
    let a = pinned.await.expect("join").expect("pinned call completes");
    assert_eq!(a.generation_id, "gen-a");
    assert_eq!(a.reply.output, json!({"tag": "a", "list": {"runs": []}}));
    reg.shutdown().await;
}

/// drain の完了: in-flight が 0 になったら sweep が退役させ、打ち切りは 0 件。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_drain_completes_when_in_flight_reaches_zero() {
    let (reg, clock) = registry();
    reg.register("gen-a", bundle("a")).await.expect("a");
    let (sink, mut rx) = host_sink();
    let in_flight = tokio::spawn(reg.call("gen-a", digest(), Some(sink)));
    let op = recv(&mut rx).await;
    reg.drain("gen-a").expect("drain");
    // in-flight が残る間は退役しない（30 秒の手前まで時計を進めても）。
    clock.advance(DRAIN_CUTOFF - Duration::from_millis(1));
    assert!(reg.sweep().await.is_empty());
    let info = reg.get("gen-a").expect("still live");
    assert!(info.draining);
    assert_eq!(info.in_flight, 1);
    let _ = op.reply.send(empty_list());
    let done = in_flight
        .await
        .expect("join")
        .expect("completes in the old generation");
    assert_eq!(done.generation_id, "gen-a");
    let retired = reg.sweep().await;
    assert_eq!(retired.len(), 1);
    assert_eq!(retired[0].generation_id, "gen-a");
    assert_eq!(retired[0].reason, RetireReason::Drained);
    assert_eq!(retired[0].cancelled_in_flight, 0);
    assert!(reg.get("gen-a").is_none());
    assert_eq!(
        reg.call("gen-a", lane("tag"), None).await,
        Err(ScriptError::GenerationGone)
    );
    reg.shutdown().await;
}

/// drain から 30 秒（注入した時計）で残った純粋判定を打ち切り、`GenerationGone` を返す。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_drain_cuts_pure_call_at_30s() {
    let (reg, clock) = registry();
    reg.register("gen-a", bundle("a")).await.expect("a");
    let hanging = tokio::spawn(reg.call("gen-a", lane("hang"), None));
    assert_eq!(in_flight(&reg, "gen-a"), 1);
    reg.drain("gen-a").expect("drain");
    clock.advance(DRAIN_CUTOFF - Duration::from_millis(1));
    assert!(reg.sweep().await.is_empty(), "not cut before 30 s");
    clock.advance(Duration::from_millis(1));
    let retired = reg.sweep().await;
    assert_eq!(retired.len(), 1);
    assert_eq!(retired[0].reason, RetireReason::Cutoff);
    assert_eq!(retired[0].cancelled_in_flight, 1);
    assert_eq!(
        hanging.await.expect("join"),
        Err(ScriptError::GenerationGone),
        "the caller answers with the Rust reference"
    );
    assert_eq!(reg.live_count(), 0);
}

/// 退役した generation への要求は即 `GenerationGone`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_call_to_retired_is_generation_gone() {
    let (reg, _) = registry();
    reg.register("gen-a", bundle("a")).await.expect("a");
    let retired = reg.retire("gen-a").await.expect("retire loaded");
    assert_eq!(retired.reason, RetireReason::Retired);
    assert_eq!(
        reg.call("gen-a", lane("tag"), None).await,
        Err(ScriptError::GenerationGone)
    );
    assert_eq!(
        reg.call("never", lane("tag"), None).await,
        Err(ScriptError::GenerationGone)
    );
    // retired の bundle は新しい isolate で登録し直せる（known-good への戻り道）。
    reg.register("gen-a2", bundle("a"))
        .await
        .expect("re-register");
    let out = reg.call("gen-a2", lane("count"), None).await.expect("call");
    assert_eq!(out.reply.output, json!({"tag": "a", "count": 1}));
    reg.shutdown().await;
}

/// 生きた generation は 2 つまで。drain 中も数える。3 つ目は拒み、退役の後は通る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_limit_two_live() {
    assert_eq!(MAX_LIVE_GENERATIONS, 2);
    let (reg, _) = registry();
    reg.register("gen-a", bundle("a")).await.expect("a");
    reg.register("gen-b", bundle("b")).await.expect("b");
    assert_eq!(
        reg.register("gen-c", bundle("c")).await,
        Err(GenerationError::Limit),
        "the third live generation is refused"
    );
    let hanging = tokio::spawn(reg.call("gen-a", lane("hang"), None));
    assert_eq!(in_flight(&reg, "gen-a"), 1);
    reg.drain("gen-a").expect("drain");
    assert!(reg.sweep().await.is_empty());
    assert_eq!(
        reg.register("gen-c", bundle("c")).await,
        Err(GenerationError::Limit),
        "a draining generation still counts"
    );
    assert_eq!(
        reg.retire("gen-a").await,
        Err(GenerationError::InFlight {
            id: "gen-a".into(),
            in_flight: 1
        })
    );
    reg.invalidate("gen-a").await.expect("invalidate");
    assert_eq!(
        hanging.await.expect("join"),
        Err(ScriptError::GenerationGone)
    );
    reg.register("gen-c", bundle("c"))
        .await
        .expect("c after a is gone");
    assert_eq!(
        reg.register("gen-d", bundle("d")).await,
        Err(GenerationError::Limit)
    );
    assert_eq!(
        reg.register("gen-b", bundle("b")).await,
        Err(GenerationError::Duplicate { id: "gen-b".into() })
    );
    reg.shutdown().await;
}

/// generation ごとに in-flight は 64 まで。65 件目は待たずに `Busy`。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_in_flight_64_then_busy() {
    assert_eq!(MAX_IN_FLIGHT_PER_GENERATION, 64);
    let (reg, _) = registry();
    reg.register("gen-a", bundle("a")).await.expect("a");
    reg.register("gen-b", bundle("b")).await.expect("b");
    let held: Vec<_> = (0..MAX_IN_FLIGHT_PER_GENERATION)
        .map(|_| tokio::spawn(reg.call("gen-a", lane("hang"), None)))
        .collect();
    assert_eq!(in_flight(&reg, "gen-a"), MAX_IN_FLIGHT_PER_GENERATION);
    assert_eq!(
        reg.call("gen-a", lane("tag"), None).await,
        Err(ScriptError::Busy)
    );
    // 他の generation の枠は別に数える。
    let b = reg.call("gen-b", lane("tag"), None).await.expect("b");
    assert_eq!(b.reply.output, json!({"tag": "b"}));
    reg.invalidate("gen-a").await.expect("invalidate");
    for h in held {
        assert_eq!(h.await.expect("join"), Err(ScriptError::GenerationGone));
    }
    reg.shutdown().await;
}

/// checkpoint の版の照合（付記 S5-5。作り物の保存済みの版 V で確かめる）。
#[test]
fn checkpoint_versions_must_cover_stored() {
    let none = BTreeSet::new();
    let v1 = BTreeSet::from([1]);
    let v12 = BTreeSet::from([1, 2]);
    assert_eq!(checkpoint_versions_cover(&none, &[]), Ok(()));
    assert_eq!(checkpoint_versions_cover(&v1, &[1]), Ok(()));
    assert_eq!(
        checkpoint_versions_cover(&v1, &[]),
        Err(GenerationError::CheckpointIncompatible { missing: vec![1] })
    );
    assert_eq!(
        checkpoint_versions_cover(&v12, &[1]),
        Err(GenerationError::CheckpointIncompatible { missing: vec![2] })
    );
    assert_eq!(check_checkpoint_versions(&[]), Ok(()));
    assert_eq!(check_checkpoint_versions(&[1]), Ok(()));
    assert!(check_checkpoint_versions(&[2]).is_err());
    assert!(check_checkpoint_versions(&[1, 1]).is_err());
}

/// 本物の bundle（`control-ts` の build）が新しい isolate の検証（selftest・各領域の入口）に通る。
///
/// bundle は付記 S5-9 の `CELERIS_SCRIPT_BUNDLE_DIR` が指す dir から読む。無ければ SKIP し、
/// `CELERIS_SCRIPT_SKIP_LOG` があればそこへ 1 行足す（`test-affected.sh` が数える）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generation_registers_real_bundle() {
    const NAME: &str = "generation_registers_real_bundle";
    let Some(dir) = std::env::var_os("CELERIS_SCRIPT_BUNDLE_DIR") else {
        let reason = "CELERIS_SCRIPT_BUNDLE_DIR is not set";
        eprintln!("SKIP {NAME}: {reason}");
        if let Some(log) = std::env::var_os("CELERIS_SCRIPT_SKIP_LOG") {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .expect("open skip log");
            writeln!(f, "{NAME}\t{reason}").expect("write skip log");
        }
        return;
    };
    let b = celeris_script::manifest::load_bundle_dir(std::path::Path::new(&dir))
        .expect("the bundle in CELERIS_SCRIPT_BUNDLE_DIR verifies");
    let (reg, _) = registry();
    let info = reg
        .register("gen-real", b)
        .await
        .expect("real bundle registers");
    for domain in [
        "cos_digest",
        "host_selftest",
        "lane_rules",
        "route_priority",
    ] {
        assert!(info.domains.iter().any(|d| d == domain), "{domain}");
    }
    reg.shutdown().await;
}
