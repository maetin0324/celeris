//! Exercise real deployment scripts with fake host commands and isolated state.
#[test]
fn deployment_scripts_require_provenance_and_preserve_history() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::process::Command::new("sh")
        .arg(repo.join("scripts/selfdeploy/tests/promote_authorization_marker.sh"))
        .output()
        .expect("run deployment script fixture");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
