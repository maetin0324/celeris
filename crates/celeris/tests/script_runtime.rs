//! `[script]` の daemon への結線（境界 ADR 2026-11-02 (b)(d)）。`celeris::run` を試験 process の中で動かす。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use celeris::{Config, Exit, RunOptions};
use celeris_script::manifest::{deploy_bundle, deployed_dir, sha256_hex};
use task_core::{DaemonMode, NoticeKind, NoticeQuery, NoticeStore, SqliteStore};

/// 通知の束ね key（`daemon::script_layer`）。
const GROUP_KEY: &str = "script_bundle_rejected";
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

    fn script_config(&self, sha256: &str) -> Config {
        self.config(&format!(
            "[script]\nstate_dir = \"state\"\nactive_bundle_sha256 = \"{sha256}\"\n"
        ))
    }

    fn rejected(&self) -> Vec<task_core::Notice> {
        let store = SqliteStore::open(&self.dir.path().join("t.sqlite3")).expect("store");
        store
            .notice_list(&NoticeQuery {
                unread_only: false,
                kinds: vec![NoticeKind::BadNews],
                limit: 0,
                offset: 0,
            })
            .expect("notices")
            .items
            .into_iter()
            .filter(|n| n.group_key == GROUP_KEY)
            .collect()
    }

    /// 試験用の bundle を配備して hash を返す。
    fn deploy_fixture(&self) -> String {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../script-runtime/tests/fixtures/selftest-bundle");
        deploy_bundle(&fixture, &self.state())
            .expect("deploy")
            .manifest
            .bundle_sha256
    }

    /// manifest の hash を合わせた本文を配備する（load の段まで届く bundle を作る）。
    fn deploy_code(&self, code: &str) -> String {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../script-runtime/tests/fixtures/selftest-bundle/manifest.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture).expect("manifest")).expect("json");
        let sha256 = sha256_hex(code.as_bytes());
        manifest["bundle_sha256"] = serde_json::Value::String(sha256.clone());
        let dist = self.dir.path().join(format!("dist-{sha256}"));
        std::fs::create_dir_all(&dist).expect("dist");
        std::fs::write(dist.join("bundle.mjs"), code).expect("bundle");
        std::fs::write(
            dist.join("manifest.json"),
            serde_json::to_vec(&manifest).expect("manifest"),
        )
        .expect("manifest");
        deploy_bundle(&dist, &self.state()).expect("deploy");
        sha256
    }

    /// daemon が起動を続け、actor thread を残さず、通知に理由を残したことを確かめる。
    async fn assert_fail_closed(&self, sha256: &str, reason: &str) {
        let (exit, _) = run_daemon(self.script_config(sha256)).await;
        assert_eq!(exit, Exit::MaxTicks);
        assert_eq!(script_threads(), 0);
        let notices = self.rejected();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].summary.contains(sha256),
            "{}",
            notices[0].summary
        );
        assert!(
            notices[0].summary.contains(reason),
            "{}",
            notices[0].summary
        );
    }
}

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
    assert!(env.rejected().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_valid_bundle_loads() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let sha256 = env.deploy_fixture();
    let (exit, seen) = run_daemon(env.script_config(&sha256)).await;
    assert_eq!(exit, Exit::MaxTicks);
    assert_eq!(seen.actors, 1, "the actor thread ran during the daemon");
    assert_eq!(script_threads(), 0, "shutdown joined the actor thread");
    assert!(env.rejected().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_bad_bundle_fail_closed() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let sha256 = env.deploy_fixture();
    let bundle = deployed_dir(&env.state(), &sha256).join("bundle.mjs");
    let mut code = std::fs::read(&bundle).expect("bundle");
    code.extend_from_slice(b"\n// tampered\n");
    std::fs::write(&bundle, code).expect("tamper");
    env.assert_fail_closed(&sha256, "does not match").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_missing_bundle_fail_closed() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    env.assert_fail_closed(&"b".repeat(64), "manifest.json")
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_throwing_bundle_fail_closed() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let sha256 = env.deploy_code("throw new Error(\"boom at load\");\n");
    env.assert_fail_closed(&sha256, "boom at load").await;
}

/// 解決しない top-level await で daemon の起動が止まらない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_pending_top_level_await_fail_closed() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let sha256 = env.deploy_code("await new Promise(() => {});\n");
    let started = std::time::Instant::now();
    env.assert_fail_closed(&sha256, &sha256).await;
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
    let sha256 = env.deploy_fixture();
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

    let (exit, seen) = run_daemon(env.script_config(&sha256)).await;

    drop(finish_tx);
    second.join().expect("second");
    assert_eq!(exit, Exit::MaxTicks);
    assert_eq!(seen.actors, 2);
}
