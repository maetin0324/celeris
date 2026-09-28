//! ADR-0075 §5 G2（U1）: 本物の sccache を使う手動の確認（`#[ignore]`）。
//!
//! ```sh
//! CELERIS_E2E_SCCACHE=$HOME/.cargo/bin/sccache \
//!   cargo test -p task-worker --test scratch_sccache_e2e -- --ignored --nocapture
//! ```
//!
//! 一時ディレクトリの scratch pool と専用の port（空いている port を選ぶ）で sccache の server を起こし、依存（registry の
//! `cfg-if` / `itoa`。`--offline` で手元の registry の cache だけを使う）を持つ小さな crate を、別の owner の target で 2 回
//! ビルドする。`task_worker::scratch::cargo_env_with`（Celeris の run と同じ env。`RUSTC_WRAPPER` は `<scratch>/bin/sccache`）
//! なら 2 回目の依存が hit し、素の sccache（`CARGO_TARGET_DIR` が rustc の env に残る）なら hit しないことを確かめる。
//! 外部ネットワークには出ない（loopback と手元の registry だけ）。証跡は `docs/progress/phase-G.md`。

use std::path::{Path, PathBuf};
use std::process::Command;

use task_worker::scratch::{
    self, CargoTuning, Owner, SccacheSettings, SccacheState, ScratchSettings,
};

fn sccache_bin() -> PathBuf {
    std::env::var_os("CELERIS_E2E_SCCACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cargo/bin/sccache")
        })
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn write_crate(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"e2e-sccache\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ncfg-if = \"=1.0.4\"\nitoa = \"=1.0.18\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn f(n: u64) -> String { cfg_if::cfg_if! { if #[cfg(unix)] { itoa::Buffer::new().format(n).to_string() } else { String::new() } } }\n",
    )
    .unwrap();
}

/// `(rust hits, rust misses)`（`--zero-stats` からの差分）。
fn rust_stats(settings: &ScratchSettings) -> (u64, u64) {
    let out = Command::new(&settings.sccache.binary)
        .args(["--show-stats", "--stats-format=json"])
        .envs(scratch::sccache_server_env(settings))
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let get = |k: &str| v["stats"][k]["counts"]["Rust"].as_u64().unwrap_or(0);
    (get("cache_hits"), get("cache_misses"))
}

fn build(src: &Path, env: &[(String, String)], settings: &ScratchSettings) -> (u64, u64, u128) {
    let zero = Command::new(&settings.sccache.binary)
        .arg("--zero-stats")
        .envs(scratch::sccache_server_env(settings))
        .output()
        .unwrap();
    assert!(zero.status.success());
    let t = std::time::Instant::now();
    let st = Command::new("cargo")
        .args(["build", "--offline", "--quiet"])
        .current_dir(src)
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_TARGET_DIR")
        .envs(env.iter().cloned())
        .status()
        .unwrap();
    assert!(st.success(), "cargo build failed with {env:?}");
    let (h, m) = rust_stats(settings);
    (h, m, t.elapsed().as_millis())
}

#[test]
#[ignore = "manual: needs a real sccache binary (CELERIS_E2E_SCCACHE) and the crates in ~/.cargo/registry"]
fn dependencies_hit_across_owner_targets_only_through_the_wrapper() {
    let bin = sccache_bin();
    assert!(bin.is_file(), "no sccache at {}", bin.display());
    let tmp = tempfile::tempdir().unwrap();
    let settings = ScratchSettings {
        l1_max_bytes: 2 * scratch::GIB,
        sccache: SccacheSettings {
            enabled: true,
            binary: bin.clone(),
            server_port: free_port(),
        },
        cargo: CargoTuning::default(),
        ..ScratchSettings::with_dir(tmp.path().join("scratch"))
    };
    // server は Celeris の run の外（ここではテスト自身）が起こす。
    let start = Command::new(&bin)
        .arg("--start-server")
        .envs(scratch::sccache_server_env(&settings))
        .output()
        .unwrap();
    assert!(start.status.success(), "{start:?}");
    let src = tmp.path().join("src");
    write_crate(&src);

    let state = scratch::resolve_sccache(&settings, scratch::server_listening);
    assert!(matches!(state, SccacheState::Ready { .. }), "{state:?}");
    // Celeris の env（wrapper 経由）で owner a → b。
    let env_a = scratch::cargo_env_with(&settings, &Owner::parse("agent-e2e-a").unwrap(), &state);
    let env_b = scratch::cargo_env_with(&settings, &Owner::parse("agent-e2e-b").unwrap(), &state);
    let a = build(&src, &env_a, &settings);
    let b = build(&src, &env_b, &settings);
    // 素の sccache（`RUSTC_WRAPPER` = 本物）で owner c → d。`CARGO_TARGET_DIR` が key に入る。
    let plain = |owner: &str| {
        let mut env = scratch::cargo_env_with(&settings, &Owner::parse(owner).unwrap(), &state);
        for (k, v) in env.iter_mut() {
            if k == "RUSTC_WRAPPER" {
                *v = bin.display().to_string();
            }
        }
        env
    };
    let c = build(&src, &plain("agent-e2e-c"), &settings);
    let d = build(&src, &plain("agent-e2e-d"), &settings);
    println!("wrapper a: rust hits {} misses {} ({} ms)", a.0, a.1, a.2);
    println!("wrapper b: rust hits {} misses {} ({} ms)", b.0, b.1, b.2);
    println!("plain   c: rust hits {} misses {} ({} ms)", c.0, c.1, c.2);
    println!("plain   d: rust hits {} misses {} ({} ms)", d.0, d.1, d.2);
    let stop = Command::new(&bin)
        .arg("--stop-server")
        .envs(scratch::sccache_server_env(&settings))
        .output();
    assert!(stop.is_ok());
    // 依存 2 つ（cfg-if / itoa）は wrapper 経由なら 2 回目に hit する。workspace のメンバー（e2e-sccache）は
    // `CARGO_TARGET_DIR` に依らないので同じソースのパスなら hit しうる。
    assert!(
        b.0 >= 2,
        "wrapper: dependencies should hit across owners: {b:?}"
    );
    assert_eq!(
        b.1, 0,
        "wrapper: nothing should miss on the second owner: {b:?}"
    );
    // 素の sccache は owner ごとに key が変わる（U1 の原因）。
    assert_eq!(d.0, 0, "plain sccache should not hit across owners: {d:?}");
}
