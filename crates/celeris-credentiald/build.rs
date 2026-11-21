//! userns の要る試験の cfg（ADR 2026-11-21-userns-tests-explicit-skip）。規則は task-worker の
//! `userns_tests_cfg.rs` にある（この crate は task-worker に依存しないので、file を `#[path]` で読む）。

#[path = "../task-worker/userns_tests_cfg.rs"]
mod userns_tests_cfg;

fn main() {
    userns_tests_cfg::emit(
        "CELERIS_LAUNCHER_TESTS",
        "../task-worker/userns_tests_cfg.rs",
    );
}
