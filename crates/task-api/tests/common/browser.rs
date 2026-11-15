//! browser の結合試験の共通部品（`browser_e2e.rs`・`browser_h3_injection.rs`）。

use std::path::{Path, PathBuf};

/// `celeris-credentiald` の binary の path。試験の deps dir の隣に無ければ `cargo build` で作る。
pub fn bridge_binary() -> PathBuf {
    // `cargo test --workspace` builds the broker binary next to the test's deps directory.
    let exe = std::env::current_exe().expect("test exe");
    let bin = exe
        .parent()
        .and_then(Path::parent)
        .expect("target dir")
        .join("celeris-credentiald");
    if !bin.exists() {
        let status = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "-q",
                "-p",
                "celeris-credentiald",
                "--bin",
                "celeris-credentiald",
            ])
            .status()
            .expect("cargo build");
        assert!(status.success());
    }
    assert!(bin.exists(), "missing {}", bin.display());
    bin
}
