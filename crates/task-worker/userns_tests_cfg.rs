//! userns の要る試験を走らせるかの規則（ADR-0126 B3、ADR 2026-11-21-userns-tests-explicit-skip D1）。
//! env を読む規則はこの file だけに置く。build script 専用で、`crates/task-worker/build.rs` と
//! `crates/celeris-credentiald/build.rs` が `#[path]` で読む。
//!
//! 有効なら cfg `celeris_userns_tests` を出す。試験は `#[cfg_attr(not(celeris_userns_tests), ignore = "…")]` で、
//! env 無しの build では harness の ignored（skipped）と数えられ、env 有りの build では本体を走らせる。
//!
//! 優先順位（上が強い）:
//! 1. `CELERIS_ISOLATION_TESTS=skip` → 無効。
//! 2. `CELERIS_USERNS_TESTS=1`・`CELERIS_ISOLATION_TESTS=require`・crate 固有の `<var>=require` のどれか → 有効。
//! 3. どれも無い → 無効（既定）。
//!
//! env を変えると build script が走り直し、cfg が変われば crate とその逆依存を compile し直す（ADR の D3）。

const CFG: &str = "celeris_userns_tests";
const ISOLATION: &str = "CELERIS_ISOLATION_TESTS";
const USERNS: &str = "CELERIS_USERNS_TESTS";

fn enabled(crate_require_var: &str) -> bool {
    let is = |var: &str, value: &str| std::env::var(var).as_deref() == Ok(value);
    if is(ISOLATION, "skip") {
        return false;
    }
    is(USERNS, "1") || is(ISOLATION, "require") || is(crate_require_var, "require")
}

/// build script の出力を書く。`crate_require_var` は crate 固有の opt-in の変数、`rule_file` はこの file の
/// package root からの path（`cargo:rerun-if-changed` に渡す）。
pub fn emit(crate_require_var: &str, rule_file: &str) {
    for var in [ISOLATION, USERNS, crate_require_var] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={rule_file}");
    println!("cargo:rustc-check-cfg=cfg({CFG})");
    if enabled(crate_require_var) {
        println!("cargo:rustc-cfg={CFG}");
    }
}
