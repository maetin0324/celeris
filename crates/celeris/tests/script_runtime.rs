//! `[script]` の daemon への結線（境界 ADR 2026-11-02 (b)(d)、付記 S5-2 の起動時の復元）。`celeris::run` を試験 process の
//! 中で動かす。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use celeris::{Config, Exit, RunOptions};
use celeris_script::manifest::{deploy_bundle, deployed_dir, host_schema_sha256, sha256_hex};
use serde_json::json;
use task_core::model::Event;
use task_core::script_compare::{
    NewScriptGeneration, ScriptDomain, ScriptGenerationState, ScriptMode, ScriptModeChange,
};
use task_core::script_inbox::ScriptInboxStore;
use task_core::{DaemonMode, SqliteStore};
use time::OffsetDateTime;
/// actor thread の名前（`celeris_script::handle`）。
const THREAD_NAME: &str = "celeris-script";
/// `cargo test` は同じ process で試験を並べる。thread の観察が混ざらないよう 1 本ずつ動かす。
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// sampler の間隔。
const SAMPLE_INTERVAL: Duration = Duration::from_millis(5);

/// 1 回の観察。process の全 thread の `(tid, comm)`。
type Sample = Vec<(u32, String)>;

fn take_sample() -> Sample {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return Vec::new();
    };
    tasks
        .flatten()
        .filter_map(|t| {
            let tid = t.file_name().to_str()?.parse().ok()?;
            let comm = std::fs::read_to_string(t.path().join("comm")).ok()?;
            Some((tid, comm.trim_end().to_owned()))
        })
        .collect()
}

fn named_script(sample: &Sample) -> usize {
    sample.iter().filter(|(_, c)| c == THREAD_NAME).count()
}

fn script_threads() -> usize {
    named_script(&take_sample())
}

/// 標本の列から actor thread の本数を数える。tid ごとに最後に読めた名前が `THREAD_NAME` のものが actor。
///
/// Linux の新しい thread は親の comm を引き継ぐ。actor が起こした子は、自分の名前を付けるまで actor と同じ名前で
/// 読めるが、後の標本で別の名前になるので数えない。逆向きの変化（別の名前から `THREAD_NAME`）は actor 自身の
/// 起動の瞬間（spawn した thread の comm を引き継いだ後に改名する）なので、これは数える。
/// 同じ名前のまま走る thread は 1 本ずつ数えるので、actor が 2 本走れば 2 を返す。
fn census_actors(samples: &[Sample]) -> usize {
    let mut last: HashMap<u32, &str> = HashMap::new();
    for (tid, comm) in samples.iter().flatten() {
        last.insert(*tid, comm);
    }
    last.values().filter(|comm| **comm == THREAD_NAME).count()
}

/// `run` の間 actor thread を観察する。
struct Observed {
    samples: usize,
    actors: usize,
}

async fn run_daemon(config: Config) -> (Exit, Observed) {
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut samples = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                samples.push(take_sample());
                std::thread::sleep(SAMPLE_INTERVAL);
            }
            samples
        })
    };
    let exit = celeris::run(
        config,
        RunOptions {
            until_idle: false,
            max_ticks: 3,
            mode: DaemonMode::Normal,
            release: None,
        },
    )
    .await
    .expect("daemon run");
    stop.store(true, Ordering::Relaxed);
    let samples = sampler.join().expect("sampler");
    (
        exit,
        Observed {
            samples: samples.len(),
            actors: census_actors(&samples),
        },
    )
}

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn config(&self, script: &str) -> Config {
        let path = self.dir.path().join("config.toml");
        std::fs::write(
            &path,
            format!("db = \"t.sqlite3\"\nworkspace_root = \"ws\"\ntick_ms = 100\n{script}"),
        )
        .expect("config");
        Config::load(&path).expect("config load")
    }

    fn script_config(&self) -> Config {
        self.config("[script]\nstate_dir = \"state\"\n")
    }

    fn store(&self) -> SqliteStore {
        SqliteStore::open(&self.dir.path().join("t.sqlite3")).expect("store")
    }

    /// `bundle_sha256` の generation を `loaded` で記録する（mode は変えない）。
    fn point_loaded(&self, sha256: &str) -> String {
        self.store()
            .script_generation_record(
                &NewScriptGeneration {
                    bundle_sha256: sha256.to_owned(),
                    host_api: celeris_script::dto::HOST_API_VERSION.to_owned(),
                    schema_sha256: "0".repeat(64),
                    manifest_json: json!({ "domains": ["host_selftest", "lane_rules"] })
                        .to_string(),
                    state: ScriptGenerationState::Loaded,
                },
                OffsetDateTime::now_utc(),
            )
            .expect("generation")
            .id
    }

    /// `domain` を `bundle_sha256` の generation の `compare` にする（人の actor。daemon は起動時にこれを load する）。
    fn point(&self, sha256: &str, domain: ScriptDomain) -> String {
        let store = self.store();
        let generation = store
            .script_generation_record(
                &NewScriptGeneration {
                    bundle_sha256: sha256.to_owned(),
                    host_api: celeris_script::dto::HOST_API_VERSION.to_owned(),
                    schema_sha256: "0".repeat(64),
                    manifest_json: json!({ "domains": ["host_selftest", domain.as_str()] })
                        .to_string(),
                    state: ScriptGenerationState::Loaded,
                },
                OffsetDateTime::now_utc(),
            )
            .expect("generation");
        let row = store.script_domain_mode(domain.as_str()).expect("mode");
        store
            .script_domain_set_mode(
                &ScriptModeChange {
                    domain: domain.as_str().to_owned(),
                    to_mode: ScriptMode::Compare,
                    to_generation: Some(generation.id.clone()),
                    expected_version: row.version,
                    actor: "human:test".into(),
                    reason: "script_runtime test".into(),
                },
                OffsetDateTime::now_utc(),
            )
            .expect("set mode");
        generation.id
    }

    /// 試験用の bundle を配備して hash を返す（`host_selftest` だけの手書きの bundle）。
    fn deploy_fixture(&self) -> String {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../script-runtime/tests/fixtures/selftest-bundle");
        deploy_bundle(&fixture, &self.state())
            .expect("deploy")
            .manifest
            .bundle_sha256
    }

    /// manifest の hash を合わせた本文を、`domains`・`capabilities`・`host_api` を指定して配備先に直接書く
    /// （`deploy_bundle` が拒む manifest も置ける）。
    fn deploy_raw(&self, code: &str, domains: &[&str], caps: &[&str], host_api: &str) -> String {
        let sha256 = sha256_hex(code.as_bytes());
        let manifest = json!({
            "schema": "celeris.script-manifest/1",
            "bundle_sha256": sha256,
            "host_api": host_api,
            "schema_sha256": host_schema_sha256(),
            "toolchain": {"typescript": "fixture", "esbuild": "fixture", "node": "fixture"},
            "source_commit": "fixture",
            "domains": domains,
            "capabilities": caps,
            "checkpoint_versions": [],
        });
        let dir = deployed_dir(&self.state(), &sha256);
        std::fs::create_dir_all(&dir).expect("deployed dir");
        std::fs::write(dir.join("bundle.mjs"), code).expect("bundle");
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).expect("manifest"),
        )
        .expect("manifest");
        sha256
    }

    /// `lane_rules` と `host_selftest` に答える bundle を配備する。
    fn deploy_code(&self, code: &str) -> String {
        self.deploy_raw(
            code,
            &["host_selftest", "lane_rules"],
            &["selftest:echo_task"],
            "^1",
        )
    }

    /// daemon が起動を続け、actor thread を残さず、`domain` を `system:script-startup` で `rust` に戻し、event と
    /// 受信箱の項目（`script_guard` が event から作る）に理由を残したことを確かめる（付記 S5-2・S4-14）。
    async fn assert_rolled_back(&self, domain: ScriptDomain, code: &str, needle: &str) {
        let (exit, _) = run_daemon(self.script_config()).await;
        assert_eq!(exit, Exit::MaxTicks);
        assert_eq!(script_threads(), 0);
        let store = self.store();
        let row = store.script_domain_mode(domain.as_str()).expect("mode");
        assert_eq!(row.mode, ScriptMode::Rust);
        assert_eq!(row.updated_by, celeris::SCRIPT_STARTUP_ACTOR);
        let events = store.script_domain_events(Some(domain), 1).expect("events");
        let Some(Event::ScriptDomainModeChanged { actor, reason, .. }) =
            events.first().map(|e| &e.event)
        else {
            panic!("no mode change event: {events:?}");
        };
        assert_eq!(actor, celeris::SCRIPT_STARTUP_ACTOR);
        assert!(reason.starts_with(code), "{reason}");
        assert!(reason.contains(needle), "{reason}");
        // 受信箱の取り込みは daemon の起動直後に 1 回走る（blocking の thread）。終わるまで待つ（実時間の保険 30 秒）。
        let started = std::time::Instant::now();
        loop {
            let items = store.script_inbox_open().expect("inbox");
            if let Some(item) = items
                .iter()
                .find(|i| i.domain.as_deref() == Some(domain.as_str()))
            {
                assert_eq!(item.actor, celeris::SCRIPT_STARTUP_ACTOR);
                assert_eq!(item.code, code, "{item:?}");
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "no inbox item: {items:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// `lane_rules` に答える手書きの bundle（業務 TS ではない）。
const LANE_BUNDLE: &str = r#"import { hostVersion, selftestEcho } from "celeris:host";
globalThis.__celerisDispatch = async (domain, input, ctx) => {
  if (domain === "host_selftest") return { host_api: hostVersion(), reply: await selftestEcho(ctx, input) };
  if (domain === "lane_rules") throw new Error("bad input");
  throw new Error(`unknown domain: ${domain}`);
};
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_absent_no_thread() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let config = env.config("");
    assert_eq!(config.script, None);
    let (exit, seen) = run_daemon(config).await;
    assert_eq!(exit, Exit::MaxTicks);
    assert!(seen.samples > 0);
    assert_eq!(seen.actors, 0, "no isolate thread without [script]");
}

/// `[script]` が在っても、mode の行が generation を指さなければ isolate を作らない（付記 S5-1）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_no_pointed_generation_no_thread() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    env.deploy_fixture();
    let (exit, seen) = run_daemon(env.script_config()).await;
    assert_eq!(exit, Exit::MaxTicks);
    assert_eq!(seen.actors, 0, "nothing to restore");
}

/// 付記 S5-2: 指された generation を起動時に新しい isolate で load し、mode を保つ。停止で thread を join する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_daemon_restore_keeps_loaded_modes() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let sha256 = env.deploy_code(LANE_BUNDLE);
    let id = env.point(&sha256, ScriptDomain::LaneRules);
    for _ in 0..2 {
        let (exit, seen) = run_daemon(env.script_config()).await;
        assert_eq!(exit, Exit::MaxTicks);
        assert_eq!(
            seen.actors, 1,
            "the restored generation ran during the daemon"
        );
        assert_eq!(script_threads(), 0, "shutdown joined the actor thread");
        let store = env.store();
        let row = store.script_domain_mode("lane_rules").expect("mode");
        assert_eq!(row.mode, ScriptMode::Compare);
        assert_eq!(row.generation_id.as_deref(), Some(id.as_str()));
        let g = store
            .script_generation_get(&id)
            .expect("read")
            .expect("row");
        assert_eq!(g.state, ScriptGenerationState::Active);
    }
}

/// 付記 S5-2: 起動時に、mode の行から指されない `draining` は drain の終わり（in-flight 0）として、isolate の無い
/// `loaded` は退役として `retired` になる。指された generation は残る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_daemon_restore_retires_unpointed_draining() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let a = env.deploy_code(LANE_BUNDLE);
    let b = env.deploy_code(&format!("{LANE_BUNDLE}// b\n"));
    let a_id = env.point(&a, ScriptDomain::LaneRules);
    // B へ移すと A は `draining`（daemon の外の CAS なので in-flight は 0）。
    let b_id = env.point(&b, ScriptDomain::LaneRules);
    let store = env.store();
    let state = |id: &str| {
        store
            .script_generation_get(id)
            .expect("read")
            .expect("row")
            .state
    };
    assert_eq!(state(&a_id), ScriptGenerationState::Draining);

    let (exit, seen) = run_daemon(env.script_config()).await;
    assert_eq!(exit, Exit::MaxTicks);
    assert_eq!(seen.actors, 1, "only the pointed generation is loaded");
    assert_eq!(state(&a_id), ScriptGenerationState::Retired);
    assert_eq!(state(&b_id), ScriptGenerationState::Active);
    let retired = |id: &str| {
        store
            .script_generation_events_after(0, 1000)
            .expect("events")
            .into_iter()
            .rev()
            .find(|e| {
                e.generation_id == id && e.event.to_state == Some(ScriptGenerationState::Retired)
            })
            .expect("a retired event")
            .event
    };
    let event = retired(&a_id);
    assert_eq!(event.actor, celeris::SCRIPT_STARTUP_ACTOR);
    assert_eq!(event.cancelled_in_flight, Some(0));
    assert!(event.reason.starts_with("drained"), "{event:?}");

    // 指されない `loaded`（前の process で登録した isolate は消えている）も `retired` にする。登録し直せば戻る（S5-1 の 6）。
    let c = env.deploy_code(&format!("{LANE_BUNDLE}// c\n"));
    let c_id = env.point_loaded(&c);
    assert_eq!(state(&c_id), ScriptGenerationState::Loaded);
    let (exit, _) = run_daemon(env.script_config()).await;
    assert_eq!(exit, Exit::MaxTicks);
    assert_eq!(state(&c_id), ScriptGenerationState::Retired);
    assert_eq!(state(&b_id), ScriptGenerationState::Active);
    assert!(retired(&c_id).reason.starts_with("startup_no_isolate"));
}

/// 付記 S5-2 の起動時の失敗: file が無い。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_restore_missing_bundle_rolls_back() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    env.point(&"b".repeat(64), ScriptDomain::LaneRules);
    env.assert_rolled_back(
        ScriptDomain::LaneRules,
        "startup_bundle_unusable",
        "manifest.json",
    )
    .await;
}

/// 付記 S5-2 の起動時の失敗: hash が違う（配備先の本文が書き換わった）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_restore_tampered_bundle_rolls_back() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let sha256 = env.deploy_code(LANE_BUNDLE);
    env.point(&sha256, ScriptDomain::LaneRules);
    let bundle = deployed_dir(&env.state(), &sha256).join("bundle.mjs");
    let mut code = std::fs::read(&bundle).expect("bundle");
    code.extend_from_slice(b"\n// tampered\n");
    std::fs::write(&bundle, code).expect("tamper");
    env.assert_rolled_back(
        ScriptDomain::LaneRules,
        "startup_bundle_unusable",
        "does not match",
    )
    .await;
}

/// 付記 S5-2 の起動時の失敗: `host_api` が合わない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_restore_host_api_mismatch_rolls_back() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let sha256 = env.deploy_raw(
        LANE_BUNDLE,
        &["host_selftest", "lane_rules"],
        &["selftest:echo_task"],
        "^99",
    );
    env.point(&sha256, ScriptDomain::LaneRules);
    env.assert_rolled_back(ScriptDomain::LaneRules, "startup_bundle_unusable", "^99")
        .await;
}

/// 付記 S5-2 の起動時の失敗: その領域の capability が足りない（`cos_digest` に `cos_digest:*` が無い）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_restore_missing_capability_rolls_back() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let code = LANE_BUNDLE.replace(
        r#"if (domain === "lane_rules") throw"#,
        r#"if (domain === "lane_rules" || domain === "cos_digest") throw"#,
    );
    let sha256 = env.deploy_raw(
        &code,
        &["host_selftest", "cos_digest"],
        &["selftest:echo_task"],
        "^1",
    );
    env.point(&sha256, ScriptDomain::CosDigest);
    env.assert_rolled_back(
        ScriptDomain::CosDigest,
        "startup_capability_missing",
        "cos_digest:list_runs",
    )
    .await;
}

/// load で例外を投げる bundle（検証に落ちる）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_restore_throwing_bundle_rolls_back() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let sha256 = env.deploy_code("throw new Error(\"boom at load\");\n");
    env.point(&sha256, ScriptDomain::LaneRules);
    env.assert_rolled_back(
        ScriptDomain::LaneRules,
        "startup_bundle_unusable",
        "boom at load",
    )
    .await;
}

/// 解決しない top-level await で daemon の起動が止まらない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_restore_pending_top_level_await_rolls_back() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let sha256 = env.deploy_code("await new Promise(() => {});\n");
    env.point(&sha256, ScriptDomain::LaneRules);
    let started = std::time::Instant::now();
    env.assert_rolled_back(ScriptDomain::LaneRules, "startup_bundle_unusable", &sha256)
        .await;
    assert!(started.elapsed() < Duration::from_secs(60));
}

fn sample(threads: &[(u32, &str)]) -> Sample {
    threads
        .iter()
        .map(|(tid, comm)| (*tid, (*comm).to_owned()))
        .collect()
}

#[test]
fn script_thread_census_excludes_child_before_naming() {
    let samples = [
        sample(&[(1, "main"), (10, THREAD_NAME), (11, THREAD_NAME)]),
        sample(&[(1, "main"), (10, THREAD_NAME), (11, "celeris-script-")]),
    ];
    assert_eq!(census_actors(&samples), 1);
}

/// 子が 1 回も改名前に読まれなければ、そのまま 1 本と数える。
#[test]
fn script_thread_census_counts_child_only_seen_renamed() {
    let samples = [sample(&[(10, THREAD_NAME), (11, "V8 DefaultWorke")])];
    assert_eq!(census_actors(&samples), 1);
}

/// actor 自身も、改名する前は spawn した thread の名前で読める。これを除くと 0 本になる。
#[test]
fn script_thread_census_counts_actor_read_before_its_own_naming() {
    let samples = [
        sample(&[(1, "main"), (10, "main")]),
        sample(&[(1, "main"), (10, THREAD_NAME)]),
    ];
    assert_eq!(census_actors(&samples), 1);
}

#[test]
fn script_thread_census_counts_two_actors_as_two() {
    let samples = [
        sample(&[(10, THREAD_NAME), (20, THREAD_NAME)]),
        sample(&[(10, THREAD_NAME), (20, THREAD_NAME)]),
    ];
    assert_eq!(census_actors(&samples), 2);
}

/// 先に終わった actor の後に別の actor が走っても、同時でなくても 2 本と数える。
#[test]
fn script_thread_census_counts_actors_in_turn_as_two() {
    let samples = [sample(&[(10, THREAD_NAME)]), sample(&[(20, THREAD_NAME)])];
    assert_eq!(census_actors(&samples), 2);
}

#[test]
fn script_thread_census_no_thread_is_zero() {
    assert_eq!(census_actors(&[]), 0);
    assert_eq!(census_actors(&[Vec::new()]), 0);
    assert_eq!(census_actors(&[sample(&[(1, "main")])]), 0);
}

/// 本物の thread で、親の名前を引き継いだ子を再現する。順序は channel だけで決める。
#[test]
fn script_thread_census_real_child_inherits_then_renames() {
    let _serial = SERIAL.blocking_lock();
    let (spawned_tx, spawned_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (renamed_tx, renamed_rx) = mpsc::channel();
    let parent = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || {
            // 名前なしの子。名前を付けるまで親の comm を引き継いだまま止まる。
            let child = std::thread::Builder::new()
                .spawn(move || {
                    go_rx.recv().expect("rename signal");
                    std::fs::write("/proc/thread-self/comm", "census-child").expect("rename");
                    renamed_tx.send(()).expect("renamed");
                    go_rx.recv().expect("finish signal");
                })
                .expect("spawn child");
            spawned_tx.send(()).expect("spawned");
            child.join().expect("child");
        })
        .expect("spawn parent");

    spawned_rx.recv().expect("spawned");
    let before = take_sample();
    go_tx.send(()).expect("rename signal");
    renamed_rx.recv().expect("renamed");
    let after = take_sample();
    go_tx.send(()).expect("finish signal");
    parent.join().expect("parent");

    assert_eq!(
        named_script(&before),
        2,
        "the unnamed child inherits the parent's comm: {before:?}"
    );
    assert_eq!(named_script(&after), 1, "{after:?}");
    assert_eq!(census_actors(std::slice::from_ref(&before)), 2);
    assert_eq!(census_actors(&[before, after]), 1);
}

/// 検出力: daemon の actor の他にもう 1 本 `THREAD_NAME` の thread が走っていれば、sampler は 2 本と数える。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_thread_census_daemon_sampler_detects_second_actor() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let sha256 = env.deploy_code(LANE_BUNDLE);
    env.point(&sha256, ScriptDomain::LaneRules);
    let (named_tx, named_rx) = mpsc::channel();
    let (finish_tx, finish_rx) = mpsc::channel::<()>();
    let second = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || {
            named_tx.send(()).expect("named");
            // 送り手が落ちるまで止まる。
            let _ = finish_rx.recv();
        })
        .expect("spawn second");
    named_rx.recv().expect("named");

    let (exit, seen) = run_daemon(env.script_config()).await;

    drop(finish_tx);
    second.join().expect("second");
    assert_eq!(exit, Exit::MaxTicks);
    assert_eq!(seen.actors, 2);
}
