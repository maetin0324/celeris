//! toolchain が PATH に無いときの sh/dash の「<name>: not found」を env_failure に数える（ADR 2026-10-30-ops-r2-isolation-principal 付記 H3）。

use celeris_campaign::grader::CommandOutput;
use celeris_campaign::grader::implementation::output_is_env_failure;

fn out(stderr: &str) -> CommandOutput {
    CommandOutput {
        exit: Some(127),
        stdout: String::new(),
        stderr: stderr.into(),
    }
}

#[test]
fn dash_cargo_not_found_is_env_failure() {
    assert!(output_is_env_failure(&out("sh: 1: cargo: not found\n")));
    assert!(output_is_env_failure(&out(
        ".grader/run.sh: 3: cargo: not found"
    )));
    assert!(output_is_env_failure(&out(
        "bash: cargo: command not found\n"
    )));
}

#[test]
fn model_failure_with_not_found_text_stays_model_failure() {
    // test の FAILED があれば、not found の行があっても成果物の失敗。
    assert!(!output_is_env_failure(&out(
        "test restart ... FAILED\nsh: 1: cargo: not found\n"
    )));
    // 行の途中の not found（成果物の出力）は環境の印にしない。
    assert!(!output_is_env_failure(&out("xattr key not found in map\n")));
}
