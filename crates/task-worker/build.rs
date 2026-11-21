//! userns の要る試験の cfg（ADR 2026-11-21-userns-tests-explicit-skip）。規則は `userns_tests_cfg.rs` にある。

#[path = "userns_tests_cfg.rs"]
mod userns_tests_cfg;

fn main() {
    userns_tests_cfg::emit("CELERIS_DB_GUARD_TESTS", "userns_tests_cfg.rs");
}
