//! 試験の一時 repo で git を走らせる helper。
//!
//! `mod support;`（`script_compare` を含む）を読まない試験も使うので、`#[path]` で直接読む。
//! 読み手: `cos_delivery_suite`（`stranded_default_branch`・`stranded_integration`）・`daily_curation.rs`・
//! `src/knowledge_curation/tests.rs`。

use std::path::Path;

/// `dir` で git を試験の identity（`test <test@example.invalid>`）で走らせ、成功を assert して
/// stdout（前後の空白を除く）を返す。端末の prompt は出さない。
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}
