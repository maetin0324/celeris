//! ADR-0126 A: `install_worker_db_guard` が使う P の組み立てと判定（userns を使わない）。

use std::path::{Path, PathBuf};

use task_worker::db_guard::{
    DaemonPaths, GuardDecision, PRODUCTION_IN_WORKER_RUN, WorkerRunMarker,
};

use super::{worker_db_guard_decision, worker_db_guard_protected_set};

/// 本番の home（`.config/celeris/config.toml`）と本番 DB・token、試験用の別 dir。
struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    lib: PathBuf,
    db: PathBuf,
    token: PathBuf,
    test: PathBuf,
}

fn env(config: &str) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.join("home");
    let lib = root.join("lib");
    let test = root.join("test");
    for d in [&home.join(".config/celeris"), &lib, &test] {
        std::fs::create_dir_all(d).unwrap();
    }
    let db = lib.join("celeris.sqlite3");
    std::fs::write(&db, b"prod").unwrap();
    let token = home.join(".config/celeris/api.token");
    std::fs::write(&token, "production-token\n").unwrap();
    let config = config.replace("@LIB@", &lib.display().to_string());
    std::fs::write(home.join(".config/celeris/config.toml"), config).unwrap();
    Env {
        _tmp: tmp,
        home,
        lib,
        db,
        token,
        test,
    }
}

const PROD_CONFIG: &str =
    "[db]\npath = \"@LIB@/celeris.sqlite3\"\n\n[api]\ntoken_file = \"api.token\"\n";

/// guard の効いた worker run の中（印 = 本番 DB のディレクトリ）。
fn inside(e: &Env) -> WorkerRunMarker {
    WorkerRunMarker::ReadOnly(e.lib.clone())
}

fn test_daemon(e: &Env) -> DaemonPaths {
    let db = e.test.join("celeris.sqlite3");
    std::fs::write(&db, b"test").unwrap();
    DaemonPaths {
        db,
        state_dir: None,
        token_file: None,
    }
}

fn decide(home: Option<&Path>, daemon: &DaemonPaths, marker: &WorkerRunMarker) -> GuardDecision {
    worker_db_guard_decision(&worker_db_guard_protected_set(home), daemon, marker)
}

#[track_caller]
fn assert_refused_inside(d: &GuardDecision) {
    assert!(
        matches!(
            d,
            GuardDecision::RefuseProduction {
                inside_worker_run: true,
                ..
            }
        ),
        "{d:?}"
    );
    assert_eq!(d.refusal_message(), Some(PRODUCTION_IN_WORKER_RUN));
}

#[test]
fn worker_db_guard_test_db_is_exempt_with_production_config() {
    let e = env(PROD_CONFIG);
    let set = worker_db_guard_protected_set(Some(&e.home));
    assert!(set.unknown_reasons().is_empty(), "{set:?}");
    let mut daemon = test_daemon(&e);
    let token = e.test.join("api.token");
    std::fs::write(&token, "test-only-token\n").unwrap();
    daemon.token_file = Some(token);
    assert_eq!(
        worker_db_guard_decision(&set, &daemon, &inside(&e)),
        GuardDecision::Exempt
    );
}

#[test]
fn worker_db_guard_test_db_is_exempt_without_production_config() {
    // 本番 config が無い host（P は印だけ）。
    let e = env(PROD_CONFIG);
    std::fs::remove_file(e.home.join(".config/celeris/config.toml")).unwrap();
    let set = worker_db_guard_protected_set(Some(&e.home));
    assert!(set.unknown_reasons().is_empty(), "{set:?}");
    assert_eq!(
        worker_db_guard_decision(&set, &test_daemon(&e), &inside(&e)),
        GuardDecision::Exempt
    );
}

#[test]
fn worker_db_guard_test_db_requires_userns_outside_worker_run() {
    let e = env(PROD_CONFIG);
    let d = decide(Some(&e.home), &test_daemon(&e), &WorkerRunMarker::Absent);
    assert!(matches!(d, GuardDecision::RequireUserns { .. }), "{d:?}");
}

#[test]
fn worker_db_guard_refuses_production_db_from_production_config() {
    // 印とは別の dir に本番 DB を置き、P が本番 config から来ることを確かめる。
    let e = env(PROD_CONFIG);
    let marker = WorkerRunMarker::ReadOnly(e.home.clone());
    let daemon = DaemonPaths {
        db: e.db.clone(),
        state_dir: None,
        token_file: None,
    };
    assert_refused_inside(&decide(Some(&e.home), &daemon, &marker));
    // 本番 config が無ければ同じ DB でも P に入らない（印の dir の外）。
    std::fs::remove_file(e.home.join(".config/celeris/config.toml")).unwrap();
    assert_eq!(
        decide(Some(&e.home), &daemon, &marker),
        GuardDecision::Exempt
    );
}

#[test]
fn worker_db_guard_refuses_production_token_relative_to_production_config() {
    let e = env(PROD_CONFIG);
    let marker = WorkerRunMarker::ReadOnly(e.home.clone());
    // 本番 token file そのもの（config 基準の相対 path が解けている）。
    let mut daemon = test_daemon(&e);
    daemon.token_file = Some(e.token.clone());
    assert_refused_inside(&decide(Some(&e.home), &daemon, &marker));
    // 内容の写し。
    let copy = e.test.join("copied.token");
    std::fs::write(&copy, "production-token").unwrap();
    daemon.token_file = Some(copy);
    assert_refused_inside(&decide(Some(&e.home), &daemon, &marker));
}

#[test]
fn worker_db_guard_production_db_tilde_uses_passwd_home_not_env_home() {
    // `~` は渡した home（getpwuid の home）で展開する。`$HOME` を書き換えた試験 daemon にだまされない。
    let e = env("db = \"~/state/celeris.sqlite3\"\n");
    let state = e.home.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let db = state.join("celeris.sqlite3");
    std::fs::write(&db, b"prod").unwrap();
    let daemon = DaemonPaths {
        db,
        state_dir: None,
        token_file: None,
    };
    assert_refused_inside(&decide(Some(&e.home), &daemon, &inside(&e)));
}

#[test]
fn worker_db_guard_production_config_unparsable_is_unknown_for_test_db() {
    let e = env("[db\npath = ");
    let set = worker_db_guard_protected_set(Some(&e.home));
    assert!(!set.unknown_reasons().is_empty(), "{set:?}");
    let d = worker_db_guard_decision(&set, &test_daemon(&e), &inside(&e));
    assert!(
        matches!(&d, GuardDecision::RequireUserns { reason } if reason.contains("unknown")),
        "{d:?}"
    );
}

#[test]
fn worker_db_guard_production_config_unreadable_is_unknown_for_test_db() {
    // 存在するのに読めない（dir になっている）。root でも決定的に失敗させるため chmod は使わない。
    let e = env(PROD_CONFIG);
    let config = e.home.join(".config/celeris/config.toml");
    std::fs::remove_file(&config).unwrap();
    std::fs::create_dir(&config).unwrap();
    let set = worker_db_guard_protected_set(Some(&e.home));
    assert!(!set.unknown_reasons().is_empty(), "{set:?}");
    let d = worker_db_guard_decision(&set, &test_daemon(&e), &inside(&e));
    assert!(matches!(d, GuardDecision::RequireUserns { .. }), "{d:?}");
}

#[test]
fn worker_db_guard_production_config_invalid_db_is_unknown_for_test_db() {
    let e = env("db = 3\n");
    let set = worker_db_guard_protected_set(Some(&e.home));
    assert!(!set.unknown_reasons().is_empty(), "{set:?}");
    let d = worker_db_guard_decision(&set, &test_daemon(&e), &inside(&e));
    assert!(matches!(d, GuardDecision::RequireUserns { .. }), "{d:?}");
}

#[test]
fn worker_db_guard_unknown_home_is_unknown_for_test_db() {
    let e = env(PROD_CONFIG);
    let set = worker_db_guard_protected_set(None);
    assert!(!set.unknown_reasons().is_empty(), "{set:?}");
    let d = worker_db_guard_decision(&set, &test_daemon(&e), &inside(&e));
    assert!(matches!(d, GuardDecision::RequireUserns { .. }), "{d:?}");
}

#[test]
fn worker_db_guard_refuses_production_state_dir_from_production_config() {
    let e = env("db = \"@LIB@/celeris.sqlite3\"\nstate_dir = \"~/state\"\n");
    let state = e.home.join("state");
    std::fs::create_dir_all(state.join("sub")).unwrap();
    let mut daemon = test_daemon(&e);
    daemon.state_dir = Some(state.join("sub"));
    assert_refused_inside(&decide(Some(&e.home), &daemon, &inside(&e)));
}
