//! ADR-0088 D3: exercise the daemon's actual instance startup function with real processes.
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use celeris::{Config, InstanceIdentity, instance, start_instance};
use task_core::{DaemonInstance, InstanceRole, SharedRole, SqliteStore, TaskStore};
use task_worker::browser_runtime::{RecordedProcess, process_starttime, write_record};
use time::OffsetDateTime;

fn spawn_record(dir: &std::path::Path, name: &str) -> (std::process::Child, i32) {
    let child = Command::new("/bin/sleep")
        .arg("60")
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    write_record(
        dir,
        name,
        &[RecordedProcess {
            pid,
            starttime: process_starttime(pid).unwrap(),
            role: "bwrap".into(),
        }],
    )
    .unwrap();
    (child, pid)
}

fn row(id: &str, pid: u32) -> DaemonInstance {
    let now = OffsetDateTime::now_utc();
    DaemonInstance {
        instance_id: id.into(),
        release: id.into(),
        pid,
        role: InstanceRole::Standby,
        started_at: now,
        heartbeat_at: now,
        handoff_requested_at: None,
        drained_at: None,
    }
}

#[test]
fn startup_reaps_dead_instance_and_preserves_live_instance() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        "db = \"celeris.sqlite3\"\nworkspace_root = \"ws\"\n[[providers]]\nid = \"p1\"\nadapter = \"fake\"\n",
    )
    .unwrap();
    let config = Config::load(&config_path).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open(&config.db.path).unwrap());
    let root = temp.path().join("browser-runtime");
    let dead_dir = root.join("dead-instance");
    let live_dir = root.join("live-instance");
    std::fs::create_dir_all(&dead_dir).unwrap();
    std::fs::create_dir_all(&live_dir).unwrap();
    let (mut dead, dead_pid) = spawn_record(&dead_dir, "old-session");
    let (mut live, _live_pid) = spawn_record(&live_dir, "live-session");
    store
        .instance_register(&row("dead-instance", 999_999_999))
        .unwrap();
    store
        .instance_register(&row("live-instance", std::process::id()))
        .unwrap();

    let identity = InstanceIdentity::new(Some("startup-test"));
    let role = SharedRole::new(InstanceRole::Active);
    let started = start_instance(store, identity, role, &config).unwrap();
    assert!(matches!(started, instance::Started::Running(_)));
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while process_starttime(dead_pid).is_some() && dead.try_wait().unwrap().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "dead runtime survived startup"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!dead_dir.exists(), "dead instance records remain");
    assert!(
        live.try_wait().unwrap().is_none(),
        "live runtime was killed"
    );
    assert!(live_dir.join("live-session.pid").exists());
    live.kill().unwrap();
    live.wait().unwrap();
    dead.wait().unwrap();
}
