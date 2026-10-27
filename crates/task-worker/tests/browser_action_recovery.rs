//! Exercise the sandbox Python runner without starting a browser or entering userns.
#[test]
fn timed_out_download_adoption_recovers_before_the_next_snapshot() {
    let output = std::process::Command::new("python3")
        .args(["-c", include_str!("browser_action_recovery.py")])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/browser_action.py"
        ))
        .output()
        .expect("python3");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
