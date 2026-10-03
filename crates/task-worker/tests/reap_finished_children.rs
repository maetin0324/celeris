//! The reaper uses waitpid(-1), which can steal the status of any child in
//! this process. Keep this regression test in its own test binary so parallel
//! task-worker unit tests can wait on their own subprocesses safely.

use task_worker::process_group::reap_finished_children;

#[test]
fn collects_an_unwaited_zombie() {
    let child = std::process::Command::new("true")
        .spawn()
        .unwrap_or_else(|e| panic!("spawn: {e}"));
    let pid = child.id();
    // Do not call wait(): the child must remain a zombie for the reaper.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if matches!(
            std::fs::read_to_string(format!("/proc/{pid}/stat")),
            Ok(s) if s.split(' ').nth(2) == Some("Z")
        ) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "child {pid} did not become a zombie"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let reaped = reap_finished_children();
    assert!(reaped >= 1, "expected to reap the zombie");
    assert!(
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .map(|s| !s.success())
            .unwrap_or(true),
        "the zombie should be gone from /proc after reaping"
    );
    drop(child);
}
