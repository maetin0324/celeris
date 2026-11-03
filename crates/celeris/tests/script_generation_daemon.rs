//! 付記 S5-1・S5-2: 実行中の daemon の generation の登録・切替・drain（`celeris::ScriptGenerations`）。
//!
//! bundle は試験の中で書いた手書きの JS（業務 TS ではない）。in-flight の要求は「試験が答えるまで待つ host op」で作り、
//! 試験は op server が要求を受け取った出来事を待つ。drain の 30 秒は `ManualClock` を進めて数える。CPU 負荷はかけない。
//! 外部ネットワークに出ない（API は loopback）。

mod daemon_port;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use celeris::{Config, Exit, RunOptions, ScriptGenerations};
use celeris_script::manifest::{deployed_dir, host_schema_sha256, sha256_hex};
use celeris_script::{GenerationRegistry, ManualClock, RuntimeLimits};
use serde_json::{Value, json};
use task_api::script_generations::ScriptGenerationError;
use task_core::script_compare::{
    LaneRulesInput, NewScriptGeneration, ScriptDomainError, ScriptGenerationState, ScriptMode,
    ScriptModeChange,
};
use task_core::{DaemonMode, SqliteStore};
use task_dispatch::script_policy::{
    CosDigestOpCall, CosDigestOpReply, CosDigestPassCall, PolicyScript, PolicyScriptError,
};
use time::OffsetDateTime;
use tokio::sync::mpsc;

const FIXTURE: &str = r#"
import { hostVersion, selftestEcho, cosDigestListRuns } from "celeris:host";

const TAG = "__TAG__";
let count = 0;

globalThis.__celerisDispatch = async (domain, input, ctx) => {
  if (domain === "host_selftest") {
    return { host_api: hostVersion(), reply: await selftestEcho(ctx, input) };
  }
  if (domain === "lane_rules") {
    if (input === null) throw new Error("bad input");
    count += 1;
    return { tag: TAG, count };
  }
  if (domain === "cos_digest") {
    if (input === null) throw new Error("bad input");
    const list = await cosDigestListRuns(ctx, {});
    return { tag: TAG, list };
  }
  throw new Error(`unknown domain: ${domain}`);
};
"#;

/// 1 回の要求の上限（実時間の保険。試験は出来事を待つ）。
const LONG: Duration = Duration::from_secs(600);
const TOKEN: &str = "script-generation-test-token";

struct Env {
    dir: tempfile::TempDir,
    db: Arc<SqliteStore>,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(SqliteStore::open(&dir.path().join("t.sqlite3")).expect("store"));
        Self { dir, db }
    }

    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    /// 手書きの bundle を配備先に置き、hash を返す。
    fn deploy(&self, tag: &str, domains: &[&str]) -> String {
        deploy_raw(&self.state(), &FIXTURE.replace("__TAG__", tag), domains)
    }

    async fn generations(&self, registry: GenerationRegistry) -> Arc<ScriptGenerations> {
        let g = ScriptGenerations::new(registry, self.state(), Arc::clone(&self.db));
        g.restore().await.expect("restore");
        g
    }

    fn set_compare(&self, sha256: &str) {
        let generation = self
            .db
            .script_generation_by_sha256(sha256)
            .expect("read")
            .expect("registered");
        let row = self.db.script_domain_mode("lane_rules").expect("mode");
        self.db
            .script_domain_set_mode(
                &ScriptModeChange {
                    domain: "lane_rules".into(),
                    to_mode: ScriptMode::Compare,
                    to_generation: Some(generation.id),
                    expected_version: row.version,
                    actor: "human:test".into(),
                    reason: "script_generation test".into(),
                },
                OffsetDateTime::now_utc(),
            )
            .expect("set mode");
    }

    fn state_of(&self, sha256: &str) -> ScriptGenerationState {
        self.db
            .script_generation_by_sha256(sha256)
            .expect("read")
            .expect("row")
            .state
    }

    fn pinned(&self, g: &ScriptGenerations, sha256: &str) -> Result<Arc<dyn PolicyScript>, String> {
        let row = self
            .db
            .script_generation_by_sha256(sha256)
            .expect("read")
            .expect("row");
        task_dispatch::script_policy::PolicyScriptSource::pinned(g, &row)
    }
}

fn deploy_raw(state: &Path, code: &str, domains: &[&str]) -> String {
    let sha256 = sha256_hex(code.as_bytes());
    let manifest = json!({
        "schema": "celeris.script-manifest/1",
        "bundle_sha256": sha256,
        "host_api": "^1",
        "schema_sha256": host_schema_sha256(),
        "toolchain": {"typescript": "fixture", "esbuild": "fixture", "node": "fixture"},
        "source_commit": "fixture",
        "domains": domains,
        "capabilities": ["cos_digest:list_runs", "cos_digest:read_run", "selftest:echo_task"],
        "checkpoint_versions": [],
    });
    let dir = deployed_dir(state, &sha256);
    std::fs::create_dir_all(&dir).expect("deployed dir");
    std::fs::write(dir.join("bundle.mjs"), code).expect("bundle");
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).expect("manifest"),
    )
    .expect("manifest");
    sha256
}

const ALL: &[&str] = &["cos_digest", "host_selftest", "lane_rules"];

/// blocking の thread から登録する（API と同じ呼び方）。
async fn register(
    g: &Arc<ScriptGenerations>,
    sha256: &str,
) -> Result<task_core::script_compare::ScriptGeneration, ScriptGenerationError> {
    let control = g.control();
    let sha256 = sha256.to_owned();
    tokio::task::spawn_blocking(move || control.register(&sha256))
        .await
        .expect("register task")
}

/// 入口の答えは入力に依らない（手書きの bundle）。9 軸はすべて `medium`。
fn lane_input() -> LaneRulesInput {
    serde_json::from_value(json!({"features": {
        "judgment": "medium", "ambiguity": "medium", "verifiability": "medium",
        "reversibility": "medium", "consequence": "medium", "context_size": "medium",
        "tool_intensity": "medium", "expected_length": "medium", "cross_cutting": "medium"
    }}))
    .expect("lane input")
}

async fn lane(script: &Arc<dyn PolicyScript>) -> Result<Value, PolicyScriptError> {
    script
        .lane_rules(lane_input(), LONG)
        .await
        .map(|r| r.output)
}

/// `cos_digest` の pass を別の task で始め、op（`list_runs`）を受け取ったところで返す。返した op に答えるまで pass は
/// in-flight のまま。
async fn start_pass(
    script: Arc<dyn PolicyScript>,
) -> (
    tokio::task::JoinHandle<Result<Value, PolicyScriptError>>,
    CosDigestOpCall,
) {
    let (sink, mut rx) = mpsc::channel(4);
    let pass = tokio::spawn(async move {
        script
            .cos_digest_pass(
                CosDigestPassCall {
                    pass_id: "pass-1".into(),
                    sink,
                },
                LONG,
            )
            .await
            .map(|r| r.output)
    });
    let op = rx.recv().await.expect("the pass asked for an op");
    (pass, op)
}

/// 付記 S5-1: 登録は新しい isolate で検証してから `loaded` で記録する。module の状態は generation の間で共有されない。
/// 検証に落ちれば `invalid`、store が拒めば isolate を捨てる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_daemon_registers_generation_in_new_isolate() {
    let env = Env::new();
    let g = env
        .generations(GenerationRegistry::new(RuntimeLimits::default()))
        .await;
    let a = env.deploy("A", ALL);
    let b = env.deploy("B", ALL);
    let row_a = register(&g, &a).await.expect("register A");
    assert_eq!(row_a.state, ScriptGenerationState::Loaded);
    assert_eq!(row_a.bundle_sha256, a);
    let row_b = register(&g, &b).await.expect("register B");
    assert_ne!(row_a.id, row_b.id);
    assert_eq!(g.registry().live_count(), 2);
    // 同じ hash の登録し直しは今の行を返す（isolate を増やさない）。
    assert_eq!(register(&g, &a).await.expect("again").id, row_a.id);
    assert_eq!(g.registry().live_count(), 2);

    // 別の isolate: A の数は A だけで増える。
    env.set_compare(&a);
    env.set_compare(&b);
    // compare を B に移したので A は draining。登録簿の中の A は見回りまで受ける。
    let pa = env.pinned(&g, &a).expect("pinned A");
    let pb = env.pinned(&g, &b).expect("pinned B");
    assert_eq!(lane(&pa).await.expect("A")["count"], 1);
    assert_eq!(lane(&pa).await.expect("A")["count"], 2);
    assert_eq!(lane(&pb).await.expect("B"), json!({"tag": "B", "count": 1}));

    // 3 つ目は上限で拒む（表も登録簿も変えない）。
    let c = env.deploy("C", ALL);
    let err = register(&g, &c).await.expect_err("limit");
    assert!(
        matches!(
            err,
            ScriptGenerationError::Domain(ScriptDomainError::GenerationLimit { .. })
        ),
        "{err:?}"
    );
    assert!(
        env.db
            .script_generation_by_sha256(&c)
            .expect("read")
            .is_none()
    );
    assert_eq!(g.registry().live_count(), 2);

    // 検証に落ちる bundle（入口の無い領域）は `invalid` で記録し、isolate を残さない。
    g.reconcile().await;
    assert_eq!(env.state_of(&a), ScriptGenerationState::Retired);
    let broken = deploy_raw(
        &env.state(),
        "import { hostVersion, selftestEcho } from \"celeris:host\";\n\
         globalThis.__celerisDispatch = async (d, i, ctx) => {\n\
           if (d === \"host_selftest\") return { host_api: hostVersion(), reply: await selftestEcho(ctx, i) };\n\
           throw new Error(`unknown domain: ${d}`);\n\
         };\n",
        ALL,
    );
    let err = register(&g, &broken).await.expect_err("invalid");
    assert!(
        matches!(err, ScriptGenerationError::Invalid { .. }),
        "{err:?}"
    );
    assert_eq!(env.state_of(&broken), ScriptGenerationState::Invalid);
    assert_eq!(g.registry().live_count(), 1, "only B is live");
    assert!(g.registry().get(&broken).is_none());
}

/// 付記 S5-1: 登録簿は受けたが store が拒んだ（表の生きた行が上限）→ isolate を捨てる。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_daemon_store_refusal_discards_isolate() {
    let env = Env::new();
    let g = env
        .generations(GenerationRegistry::new(RuntimeLimits::default()))
        .await;
    // 別の process が登録した生きた行が 2 つ（この登録簿は持たない）。
    for n in 0..2 {
        env.db
            .script_generation_record(
                &NewScriptGeneration {
                    bundle_sha256: format!("{n}").repeat(64),
                    host_api: celeris_script::dto::HOST_API_VERSION.to_owned(),
                    schema_sha256: "0".repeat(64),
                    manifest_json: json!({"domains": ["lane_rules"]}).to_string(),
                    state: ScriptGenerationState::Loaded,
                },
                OffsetDateTime::now_utc(),
            )
            .expect("seed");
    }
    let a = env.deploy("A", ALL);
    let err = register(&g, &a).await.expect_err("store refuses");
    assert!(
        matches!(
            err,
            ScriptGenerationError::Domain(ScriptDomainError::GenerationLimit { .. })
        ),
        "{err:?}"
    );
    assert_eq!(g.registry().live_count(), 0, "the isolate was discarded");
    assert!(
        env.db
            .script_generation_by_sha256(&a)
            .expect("read")
            .is_none()
    );
}

/// 付記 S5-2: 実行中に 2 つ目を登録して切り替える。切替の前に送った要求は旧 generation で終わり、後の要求は新へ行く。
/// 旧は drain の後 in-flight 0 で `retired`（打ち切り 0）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_daemon_switch_drains_old_generation() {
    let env = Env::new();
    let g = env
        .generations(GenerationRegistry::new(RuntimeLimits::default()))
        .await;
    let a = env.deploy("A", ALL);
    register(&g, &a).await.expect("register A");
    env.set_compare(&a);
    assert_eq!(env.state_of(&a), ScriptGenerationState::Active);
    let pa = env.pinned(&g, &a).expect("pinned A");
    let (pass, op) = start_pass(Arc::clone(&pa)).await;
    assert_eq!(g.registry().get(&a).expect("A").in_flight, 1);

    // 実行中に B を登録して切り替える（mode の CAS）。
    let b = env.deploy("B", ALL);
    register(&g, &b).await.expect("register B");
    env.set_compare(&b);
    assert_eq!(env.state_of(&a), ScriptGenerationState::Draining);
    assert_eq!(env.state_of(&b), ScriptGenerationState::Active);
    g.reconcile().await;
    // drain 中: in-flight が残るので退役しない。新しい要求は受けない（dispatcher は Rust の参照実装で答える）。
    let info = g.registry().get(&a).expect("A still holds its isolate");
    assert!(info.draining);
    assert_eq!(info.in_flight, 1);
    assert!(
        env.pinned(&g, &a).is_err(),
        "a draining generation is not handed out"
    );
    assert_eq!(
        lane(&pa).await.expect_err("draining").kind,
        "generation_gone"
    );
    let pb = env.pinned(&g, &b).expect("pinned B");
    assert_eq!(lane(&pb).await.expect("B")["tag"], "B");

    // 旧の in-flight は旧で終わる。
    let _ = op.reply.send(CosDigestOpReply::Ok(json!({"runs": []})));
    let out = pass.await.expect("pass task").expect("A finished");
    assert_eq!(out["tag"], "A");
    g.reconcile().await;
    assert!(g.registry().get(&a).is_none());
    assert_eq!(env.state_of(&a), ScriptGenerationState::Retired);
    let events = env
        .db
        .script_generation_events_after(0, 1000)
        .expect("events");
    let retired = events
        .iter()
        .rev()
        .find(|e| {
            e.event.bundle_sha256.as_deref() == Some(a.as_str())
                && e.event.to_state == Some(ScriptGenerationState::Retired)
        })
        .expect("retired event");
    assert_eq!(retired.event.actor, "system:script-runtime");
    assert_eq!(retired.event.cancelled_in_flight, Some(0));
}

/// 付記 S5-2・(d): drain から 30 秒を過ぎて残る要求は打ち切り、`GenerationGone`。`cancelled_in_flight` 1 を記録する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_daemon_drain_cutoff_30s_generation_gone() {
    let env = Env::new();
    let clock = Arc::new(ManualClock::new());
    let g = env
        .generations(GenerationRegistry::with_clock(
            RuntimeLimits::default(),
            clock.clone(),
        ))
        .await;
    let a = env.deploy("A", ALL);
    let b = env.deploy("B", ALL);
    register(&g, &a).await.expect("register A");
    register(&g, &b).await.expect("register B");
    env.set_compare(&a);
    let (pass, _op) = start_pass(env.pinned(&g, &a).expect("pinned A")).await;
    env.set_compare(&b);
    g.reconcile().await;
    clock.advance(Duration::from_millis(29_999));
    g.reconcile().await;
    assert!(g.registry().get(&a).is_some(), "not cut before 30 s");
    clock.advance(Duration::from_millis(1));
    g.reconcile().await;
    let err = pass.await.expect("pass task").expect_err("cut");
    assert_eq!(err.kind, "generation_gone");
    assert_eq!(env.state_of(&a), ScriptGenerationState::Retired);
    let events = env
        .db
        .script_generation_events_after(0, 1000)
        .expect("events");
    let retired = events
        .iter()
        .rev()
        .find(|e| {
            e.event.bundle_sha256.as_deref() == Some(a.as_str())
                && e.event.to_state == Some(ScriptGenerationState::Retired)
        })
        .expect("retired event");
    assert_eq!(retired.event.cancelled_in_flight, Some(1));
    assert!(
        retired.event.reason.starts_with("drain_cutoff"),
        "{:?}",
        retired.event
    );
}

/// 付記 S5-1・S5-2: 動いている daemon に API で 2 つの generation を登録し、mode を A→B に切り替えると A は退役する。
/// 再起動すると B の compare を復元する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_daemon_api_registers_and_switches_while_running() {
    let env = Env::new();
    let a = env.deploy("A", ALL);
    let b = env.deploy("B", ALL);
    std::fs::write(env.dir.path().join("api.token"), format!("{TOKEN}\n")).expect("token");
    let reservation = daemon_port::PortReservation::new();
    let addr = reservation.addr();
    let config_path = env.dir.path().join("config.toml");
    let write_config = |listen: bool| {
        let api = if listen {
            format!(
                "[api]\nlisten = \"{addr}\"\n\n[[api.tokens]]\nprincipal = \"human:test\"\ntoken_file = \"api.token\"\n"
            )
        } else {
            String::new()
        };
        std::fs::write(
            &config_path,
            format!(
                "db = \"t.sqlite3\"\nworkspace_root = \"ws\"\ntick_ms = 50\n\n{api}\n[script]\nstate_dir = \"state\"\n"
            ),
        )
        .expect("config");
        Config::load(&config_path).expect("config load")
    };
    let run = |config: Config, max_ticks: u64| {
        tokio::spawn(celeris::run(
            config,
            RunOptions {
                until_idle: false,
                max_ticks,
                mode: DaemonMode::Normal,
                release: None,
            },
        ))
    };
    // 400 tick（50 ms ごと、20 秒以上）で自分で止まる。下の手順は数秒で終わる。
    let mut daemon = run(write_config(true), 400);
    let base = format!("http://{addr}/api/v1");
    let client = reqwest::Client::new();
    daemon_port::wait_api_ready(
        &client,
        &format!("{base}/script/generations"),
        TOKEN,
        &mut daemon,
        Duration::from_secs(5),
        Duration::from_secs(180),
    )
    .await
    .unwrap_or_else(|e| panic!("daemon API did not become ready: {e}"));
    let post = |path: String, body: Value| {
        let client = client.clone();
        async move {
            let response = client
                .post(path)
                .bearer_auth(TOKEN)
                .json(&body)
                .send()
                .await
                .expect("POST");
            let status = response.status().as_u16();
            (
                status,
                response.json::<Value>().await.unwrap_or(Value::Null),
            )
        }
    };
    // 復元の前（起動の直後）は 503 になりうる。復元は起動時に終わっているので 200 を待つ必要は無いが、保険に数回試す。
    let mut registered = None;
    for _ in 0..40 {
        let (status, body) = post(
            format!("{base}/script/generations"),
            json!({"bundle_sha256": a}),
        )
        .await;
        if status == 200 {
            registered = Some(body);
            break;
        }
        assert_eq!(status, 503, "{body}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let ga = registered.expect("registered A");
    assert_eq!(ga["state"], "loaded");
    let (status, gb) = post(
        format!("{base}/script/generations"),
        json!({"bundle_sha256": b}),
    )
    .await;
    assert_eq!(status, 200, "{gb}");
    let put_mode = |sha: &str| {
        let client = client.clone();
        let body = json!({
            "mode": "compare",
            "expected_version": env.db.script_domain_mode("lane_rules").expect("mode").version,
            "generation": sha,
            "reason": "script_generation test",
        });
        let url = format!("{base}/script/domains/lane_rules/mode");
        async move {
            let response = client
                .put(url)
                .bearer_auth(TOKEN)
                .json(&body)
                .send()
                .await
                .expect("PUT");
            let status = response.status().as_u16();
            assert_eq!(status, 200, "{}", response.text().await.unwrap_or_default());
        }
    };
    put_mode(&a).await;
    assert_eq!(env.state_of(&a), ScriptGenerationState::Active);
    put_mode(&b).await;
    assert_eq!(env.state_of(&b), ScriptGenerationState::Active);
    // 見回り（1 秒ごと）が A を drain して退役させる（出来事待ち + 長い保険）。
    tokio::time::timeout(Duration::from_secs(60), async {
        while env.state_of(&a) != ScriptGenerationState::Retired {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("A retired after the drain");
    assert_eq!(
        daemon.await.expect("daemon task").expect("daemon run"),
        Exit::MaxTicks
    );
    drop(reservation);

    // 再起動: B の compare を新しい isolate で復元する（API 無し）。
    let restarted = run(write_config(false), 3)
        .await
        .expect("daemon task")
        .expect("run");
    assert_eq!(restarted, Exit::MaxTicks);
    let row = env.db.script_domain_mode("lane_rules").expect("mode");
    assert_eq!(row.mode, ScriptMode::Compare);
    assert_eq!(row.generation_id.as_deref(), gb["id"].as_str());
    assert_eq!(env.state_of(&b), ScriptGenerationState::Active);
}
