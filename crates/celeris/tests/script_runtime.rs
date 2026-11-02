//! `[script]` の daemon への結線（境界 ADR 2026-11-02 (b)(d)）。`celeris::run` を試験 process の中で動かす。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

fn script_threads() -> usize {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return 0;
    };
    tasks
        .flatten()
        .filter(|t| {
            std::fs::read_to_string(t.path().join("comm"))
                .is_ok_and(|comm| comm.trim_end() == THREAD_NAME)
        })
        .count()
}

/// `run` の間 actor thread を観察する。
struct Observed {
    samples: usize,
    max_threads: usize,
}

async fn run_daemon(config: Config) -> (Exit, Observed) {
    let stop = Arc::new(AtomicBool::new(false));
    let max_threads = Arc::new(AtomicUsize::new(0));
    let sampler = {
        let (stop, max_threads) = (Arc::clone(&stop), Arc::clone(&max_threads));
        std::thread::spawn(move || {
            let mut samples = 0;
            while !stop.load(Ordering::Relaxed) {
                max_threads.fetch_max(script_threads(), Ordering::Relaxed);
                samples += 1;
                std::thread::sleep(Duration::from_millis(5));
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
            samples,
            max_threads: max_threads.load(Ordering::Relaxed),
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
    assert_eq!(seen.max_threads, 0, "no isolate thread without [script]");
    assert!(env.rejected().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_runtime_valid_bundle_loads() {
    let _serial = SERIAL.lock().await;
    let env = Env::new();
    let sha256 = env.deploy_fixture();
    let (exit, seen) = run_daemon(env.script_config(&sha256)).await;
    assert_eq!(exit, Exit::MaxTicks);
    assert_eq!(
        seen.max_threads, 1,
        "the actor thread ran during the daemon"
    );
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
