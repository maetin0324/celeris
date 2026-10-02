//! ADR-0075 §5 G3 の手動 e2e（`#[ignore]`）: 本物の sccache 0.18 を in-process の cache server（loopback）に向け、
//! miss → PUT → L1 hit → L1 を消して L2 hit（promote）→ cache server 停止中もコンパイルが続く、を確かめる。
//!
//! ```text
//! CELERIS_E2E_SCCACHE=$HOME/.cargo/bin/sccache CELERIS_E2E_DIR=/var/lib/celeris/scratch/targets/agent-g3-e2e \
//!   cargo test -p scratch-cache --test sccache_webdav_e2e -- --ignored --nocapture
//! ```

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::{request, start_on};
use scratch_cache::{StoreConfig, TieredStore};

const TOKEN: &str = "e2e-token";

fn store(root: &Path) -> TieredStore {
    std::fs::create_dir_all(root.join("l2")).unwrap();
    let mut c = StoreConfig::new(root.join("l1"), Some(root.join("l2")));
    c.l2_gc_interval = Duration::ZERO;
    TieredStore::open(c).unwrap()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn write_project(dir: &Path) {
    let members = ["e2ea", "e2eb", "e2ec"];
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"e2ea\", \"e2eb\", \"e2ec\", \"e2ebin\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    for (i, m) in members.iter().enumerate() {
        std::fs::create_dir_all(dir.join(m).join("src")).unwrap();
        std::fs::write(
            dir.join(m).join("Cargo.toml"),
            format!("[package]\nname = \"{m}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join(m).join("src/lib.rs"),
            format!(
                "pub fn f(x: u64) -> u64 {{ (0..x).map(|i| i * {}).sum() }}\n",
                i + 2
            ),
        )
        .unwrap();
    }
    std::fs::create_dir_all(dir.join("e2ebin/src")).unwrap();
    std::fs::write(
        dir.join("e2ebin/Cargo.toml"),
        "[package]\nname = \"e2ebin\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\ne2ea = { path = \"../e2ea\" }\ne2eb = { path = \"../e2eb\" }\ne2ec = { path = \"../e2ec\" }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("e2ebin/src/main.rs"),
        "fn main() { println!(\"{}\", e2ea::f(3) + e2eb::f(4) + e2ec::f(5)); }\n",
    )
    .unwrap();
}

struct Sccache {
    bin: PathBuf,
    /// `task_worker::scratch::wrapper_script` の G2 の形（`CARGO_TARGET_DIR` を外してから本物を exec。G2 の U1）。R7-7 の
    /// 「server に届かなければ compiler を直接」は省く（この試験では server が常に居る）。
    wrapper: PathBuf,
    port: u16,
    endpoint_port: u16,
    root: PathBuf,
}

impl Sccache {
    fn cmd(&self) -> Command {
        let mut c = Command::new(&self.bin);
        c.env("SCCACHE_SERVER_PORT", self.port.to_string())
            .env(
                "SCCACHE_WEBDAV_ENDPOINT",
                format!("http://127.0.0.1:{}", self.endpoint_port),
            )
            .env("SCCACHE_WEBDAV_KEY_PREFIX", "sccache")
            .env("SCCACHE_WEBDAV_TOKEN", TOKEN)
            .env("SCCACHE_IDLE_TIMEOUT", "0")
            .env("SCCACHE_DIR", self.root.join("unused-disk"))
            .env_remove("SCCACHE_LOG");
        c
    }
    fn build(&self, project: &Path, target: &Path) -> (bool, Duration) {
        let _ = std::fs::remove_dir_all(target);
        let started = Instant::now();
        let st = Command::new("cargo")
            .args(["build", "--offline", "-q"])
            .current_dir(project)
            .env("CARGO_TARGET_DIR", target)
            .env("CARGO_INCREMENTAL", "0")
            .env("RUSTC_WRAPPER", &self.wrapper)
            .env("SCCACHE_SERVER_PORT", self.port.to_string())
            .status()
            .unwrap();
        (st.success(), started.elapsed())
    }
    fn rust_hits_misses(&self) -> (u64, u64) {
        let out = self
            .cmd()
            .args(["--show-stats", "--stats-format=json"])
            .output()
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let s = &v["stats"];
        (
            s["cache_hits"]["counts"]["Rust"].as_u64().unwrap_or(0),
            s["cache_misses"]["counts"]["Rust"].as_u64().unwrap_or(0),
        )
    }
    fn zero(&self) {
        let _ = self.cmd().arg("--zero-stats").output();
    }
}

fn stats(port: u16) -> serde_json::Value {
    serde_json::from_slice(&request(port, "GET", "/stats", &[], b"").body).unwrap()
}

#[test]
#[ignore = "manual: needs a real sccache 0.18 (CELERIS_E2E_SCCACHE) and cargo; evidence goes to docs/progress/phase-G.md"]
fn real_sccache_uses_the_tiered_cache_server() {
    let Some(bin) = std::env::var_os("CELERIS_E2E_SCCACHE").map(PathBuf::from) else {
        eprintln!("CELERIS_E2E_SCCACHE is not set; skipping");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = std::env::var_os("CELERIS_E2E_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| tmp.path().to_path_buf());
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let project = root.join("proj");
    write_project(&project);

    let cache_port = free_port();
    let s1 = store(&root.join("cache"));
    let srv = start_on(s1.clone(), Some(TOKEN), cache_port);
    let wrapper = root.join("bin/sccache");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nunset CARGO_TARGET_DIR CARGO_BUILD_TARGET_DIR\nexec '{}' \"$@\"\n",
                bin.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let sc = Sccache {
        bin,
        wrapper,
        port: free_port(),
        endpoint_port: cache_port,
        root: root.clone(),
    };
    let out = sc.cmd().arg("--start-server").output().unwrap();
    assert!(
        out.status.success(),
        "sccache --start-server: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // A: 空の cache（miss → PUT）。
    let (ok, t) = sc.build(&project, &root.join("target-a"));
    assert!(ok);
    let (h, m) = sc.rust_hits_misses();
    let st = stats(cache_port);
    eprintln!(
        "A (cold): {t:?} rust hits {h} misses {m} · server puts {} gets {} misses {}",
        st["puts"], st["gets"], st["misses"]
    );
    assert_eq!(h, 0);
    assert!(m >= 3);
    let deadline = Instant::now() + Duration::from_secs(60);
    while stats(cache_port)["flush_queue_len"] != 0 {
        assert!(Instant::now() < deadline, "flusher did not drain");
        std::thread::sleep(Duration::from_millis(100));
    }
    let st = stats(cache_port);
    eprintln!(
        "   flushed to L2: written {} ({} bytes) at {} MB/s cap",
        st["flush_written"], st["flush_written_bytes"], st["flush_mbps"]
    );

    // B: 別の target（L1 hit）。
    sc.zero();
    let (ok, t) = sc.build(&project, &root.join("target-b"));
    assert!(ok);
    let (h, m) = sc.rust_hits_misses();
    let st = stats(cache_port);
    eprintln!(
        "B (L1): {t:?} rust hits {h} misses {m} · server l1_hits {} l2_hits {}",
        st["l1_hits"], st["l2_hits"]
    );
    assert!(h >= 3 && m == 0);

    // C: cache server を止めて L1 を消し、同じ port で起こし直す（L2 hit → promote）。
    srv.stop();
    s1.shutdown();
    std::fs::remove_dir_all(root.join("cache/l1")).unwrap();
    let s2 = store(&root.join("cache"));
    let srv = start_on(s2.clone(), Some(TOKEN), cache_port);
    sc.zero();
    let (ok, t) = sc.build(&project, &root.join("target-c"));
    assert!(ok);
    let (h, m) = sc.rust_hits_misses();
    let st = stats(cache_port);
    eprintln!(
        "C (L2 → promote): {t:?} rust hits {h} misses {m} · server l2_hits {} promotes {}",
        st["l2_hits"], st["promotes"]
    );
    assert!(h >= 3 && m == 0);
    assert!(st["promotes"].as_u64().unwrap_or(0) >= 3);

    // D: cache server が止まっている（U5 の down-mid）。コンパイルは続く。
    srv.stop();
    s2.shutdown();
    sc.zero();
    let (ok, t) = sc.build(&project, &root.join("target-d"));
    let (h, m) = sc.rust_hits_misses();
    eprintln!("D (cache server down): ok={ok} {t:?} rust hits {h} misses {m}");
    assert!(ok, "the build must succeed without the cache server");

    let _ = sc.cmd().arg("--stop-server").output();
    if std::env::var_os("CELERIS_E2E_DIR").is_some() {
        let _ = std::fs::remove_dir_all(&root);
    }
}
