//! A daemon bootstrap and dispatch tick must tolerate durable events written by the
//! pre-removal routing implementation.
use celeris::{ClusterMasters, Config, build_dispatcher};

#[test]
fn startup_and_dispatch_accept_database_with_legacy_routing_event() {
    let temp = tempfile::tempdir().expect("temporary database directory");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "db = {:?}\nworkspace_root = {:?}\n",
            temp.path().join("celeris.sqlite3").display().to_string(),
            temp.path().join("workspaces").display().to_string()
        ),
    )
    .expect("write config");
    let config = Config::load(&config_path).expect("load config");

    // Use the exact bootstrap entry point used by daemon startup, followed by its
    // normal first dispatch tick. The task-core legacy_routing_event test exercises
    // deserialization of the old record itself through Event.
    let mut dispatcher = build_dispatcher(&config, ClusterMasters::default())
        .expect("daemon bootstrap accepts the durable event schema");
    dispatcher.tick().expect("first dispatch tick");
}
