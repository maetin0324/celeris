//! ADR-0126 A: `install_worker_db_guard` が使う P の組み立てと判定（userns を使わない）。

use std::path::{Path, PathBuf};

use task_worker::db_guard::{
    DaemonPaths, GuardDecision, PRODUCTION_IN_WORKER_RUN, WorkerRunMarker,
};

use super::{
    WorkerDbGuardAction, enforce_worker_db_guard, worker_db_guard_action, worker_db_guard_decision,
    worker_db_guard_precheck, worker_db_guard_protected_set,
};

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

// ---------------------------------------------------------------------------
// ADR-0126 A2（final review 指摘）: worker run の中で本番に当たる daemon は probe せずに拒否する。
// ---------------------------------------------------------------------------

const PROVIDER: &str =
    "\n[adapters.fake]\ncommand = [\"true\"]\n\n[[providers]]\nid = \"p1\"\nadapter = \"fake\"\n";

/// 試験 daemon の config（`db` だけを指定。worker_read_only は既定の true）。
fn daemon_config(e: &Env, db: &Path) -> crate::Config {
    let path = e.test.join("daemon.toml");
    std::fs::write(&path, format!("db = \"{}\"\n{PROVIDER}", db.display())).unwrap();
    crate::Config::load(&path).unwrap()
}

#[track_caller]
fn assert_refuse_action(d: &GuardDecision) {
    assert_refused_inside(d);
    match worker_db_guard_action(d) {
        WorkerDbGuardAction::Refuse(message) => {
            assert!(message.contains(PRODUCTION_IN_WORKER_RUN), "{message}");
            assert!(!message.contains("production-token"), "{message}");
        }
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn worker_db_guard_refuse_action_never_probes_inside_worker_run() {
    let e = env(PROD_CONFIG);
    let config = daemon_config(&e, &e.db);
    let decision = GuardDecision::RefuseProduction {
        reason: "db is the production db".into(),
        inside_worker_run: true,
    };
    let mut probed = false;
    let err = enforce_worker_db_guard(&config, &decision, |_| {
        probed = true;
        Err(crate::DaemonError::DbGuard("probe must not run".into()))
    })
    .err()
    .unwrap();
    assert!(
        !probed,
        "probe ran for RefuseProduction inside a worker run"
    );
    let text = err.to_string();
    assert!(text.contains(PRODUCTION_IN_WORKER_RUN), "{text}");
    assert!(text.contains("db is the production db"), "{text}");
}

#[test]
fn worker_db_guard_refuse_action_table() {
    let refuse = |inside| GuardDecision::RefuseProduction {
        reason: "r".into(),
        inside_worker_run: inside,
    };
    assert_eq!(
        worker_db_guard_action(&refuse(true)),
        WorkerDbGuardAction::Refuse(format!("{PRODUCTION_IN_WORKER_RUN}: r"))
    );
    // worker run の外の本番 daemon（本番そのもの）は従来どおり probe。
    assert_eq!(
        worker_db_guard_action(&refuse(false)),
        WorkerDbGuardAction::Probe
    );
    assert_eq!(
        worker_db_guard_action(&GuardDecision::RequireUserns { reason: "r".into() }),
        WorkerDbGuardAction::Probe
    );
    assert_eq!(
        worker_db_guard_action(&GuardDecision::Exempt),
        WorkerDbGuardAction::Exempt
    );
    // Exempt も probe しない。Probe のときだけ probe を呼ぶ。
    let e = env(PROD_CONFIG);
    let config = daemon_config(&e, &e.test.join("t.sqlite3"));
    let mut probed = 0;
    let got = enforce_worker_db_guard(&config, &GuardDecision::Exempt, |_| {
        probed += 1;
        Err(crate::DaemonError::DbGuard("x".into()))
    });
    assert!(matches!(got, Ok(None)));
    let got = enforce_worker_db_guard(&config, &refuse(false), |_| {
        probed += 1;
        Err(crate::DaemonError::DbGuard("probe failed".into()))
    });
    assert!(got.is_err());
    assert_eq!(probed, 1);
}

#[test]
fn worker_db_guard_refuse_production_db() {
    let e = env(PROD_CONFIG);
    let marker = WorkerRunMarker::ReadOnly(e.home.clone());
    let daemon = DaemonPaths {
        db: e.db.clone(),
        state_dir: None,
        token_file: None,
    };
    assert_refuse_action(&decide(Some(&e.home), &daemon, &marker));
}

#[test]
fn worker_db_guard_refuse_state_dir_overlap() {
    let e = env("db = \"@LIB@/celeris.sqlite3\"\nstate_dir = \"~/state\"\n");
    let state = e.home.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let marker = WorkerRunMarker::ReadOnly(e.home.join(".config"));
    // daemon の DB が本番 state_dir の中。
    let db = state.join("t.sqlite3");
    std::fs::write(&db, b"t").unwrap();
    let daemon = DaemonPaths {
        db,
        state_dir: None,
        token_file: None,
    };
    assert_refuse_action(&decide(Some(&e.home), &daemon, &marker));
    // daemon の state_dir が本番 state_dir を含む（祖先）。
    let mut daemon = test_daemon(&e);
    daemon.state_dir = Some(e.home.clone());
    assert_refuse_action(&decide(Some(&e.home), &daemon, &marker));
}

#[test]
fn worker_db_guard_refuse_production_token() {
    let e = env(PROD_CONFIG);
    let marker = WorkerRunMarker::ReadOnly(e.home.join(".config"));
    let mut daemon = test_daemon(&e);
    daemon.token_file = Some(e.token.clone());
    assert_refuse_action(&decide(Some(&e.home), &daemon, &marker));
    // 同じ内容の写し（path は別）。
    let copy = e.test.join("copy.token");
    std::fs::write(&copy, "production-token\n").unwrap();
    daemon.token_file = Some(copy);
    assert_refuse_action(&decide(Some(&e.home), &daemon, &marker));
}

#[test]
fn worker_db_guard_refuse_via_symlink() {
    let e = env(PROD_CONFIG);
    let marker = WorkerRunMarker::ReadOnly(e.home.join(".config"));
    // 本番 DB への symlink。
    let link = e.test.join("link.sqlite3");
    std::os::unix::fs::symlink(&e.db, &link).unwrap();
    let daemon = DaemonPaths {
        db: link,
        state_dir: None,
        token_file: None,
    };
    assert_refuse_action(&decide(Some(&e.home), &daemon, &marker));
    // 本番 DB のディレクトリへの symlink を通した path。
    let dir_link = e.test.join("libdir");
    std::os::unix::fs::symlink(&e.lib, &dir_link).unwrap();
    let daemon = DaemonPaths {
        db: dir_link.join("celeris.sqlite3"),
        state_dir: None,
        token_file: None,
    };
    assert_refuse_action(&decide(Some(&e.home), &daemon, &marker));
    // 本番 token への symlink。
    let token_link = e.test.join("token.link");
    std::os::unix::fs::symlink(&e.token, &token_link).unwrap();
    let mut daemon = test_daemon(&e);
    daemon.token_file = Some(token_link);
    assert_refuse_action(&decide(Some(&e.home), &daemon, &marker));
}

#[test]
fn worker_db_guard_refuse_relative_path_with_dotdot() {
    // 本番 config の相対 db（config のディレクトリ基準、`..` を含む）。
    let e = env("db = \"../../../lib/celeris.sqlite3\"\n");
    let marker = WorkerRunMarker::ReadOnly(e.home.join(".config"));
    let daemon = DaemonPaths {
        db: e.db.clone(),
        state_dir: None,
        token_file: None,
    };
    assert_refuse_action(&decide(Some(&e.home), &daemon, &marker));
    // daemon 側の `..` を含む path。
    let daemon = DaemonPaths {
        db: e.test.join("../lib/./celeris.sqlite3"),
        state_dir: None,
        token_file: None,
    };
    assert_refuse_action(&decide(Some(&e.home), &daemon, &marker));
    // 試験 daemon の config に書いた相対 path（config 基準で解ける）。
    let path = e.test.join("rel.toml");
    std::fs::write(
        &path,
        format!("db = \"../lib/celeris.sqlite3\"\n{PROVIDER}"),
    )
    .unwrap();
    let config = crate::Config::load(&path).unwrap();
    let daemon = super::worker_db_guard_daemon_paths(&config);
    assert_refuse_action(&decide(Some(&e.home), &daemon, &marker));
}

#[test]
fn worker_db_guard_refuse_under_marker_path() {
    // 本番 config が無くても、印の path（guard が読み取り専用にした本番 DB の dir）配下は拒否。
    let e = env(PROD_CONFIG);
    std::fs::remove_file(e.home.join(".config/celeris/config.toml")).unwrap();
    let sub = e.lib.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let db = sub.join("t.sqlite3");
    std::fs::write(&db, b"t").unwrap();
    let daemon = DaemonPaths {
        db,
        state_dir: None,
        token_file: None,
    };
    assert_refuse_action(&decide(Some(&e.home), &daemon, &inside(&e)));
}

#[test]
fn worker_db_guard_refuse_precheck_before_opening_db() {
    let e = env(PROD_CONFIG);
    // 既にある本番 DB: DB を開く前に止める。
    let config = daemon_config(&e, &e.db);
    let action = worker_db_guard_precheck(Some(&e.home), &config, &inside(&e));
    assert!(
        matches!(&action, WorkerDbGuardAction::Refuse(m) if m.contains(PRODUCTION_IN_WORKER_RUN)),
        "{action:?}"
    );
    // まだ無い DB でも、印の path 配下なら止める（読み取り専用の dir に試験 DB は作れない）。
    let config = daemon_config(&e, &e.lib.join("new/t.sqlite3"));
    let action = worker_db_guard_precheck(Some(&e.home), &config, &inside(&e));
    assert!(
        matches!(&action, WorkerDbGuardAction::Refuse(m) if m.contains(PRODUCTION_IN_WORKER_RUN)),
        "{action:?}"
    );
    // 印の外のまだ無い試験 DB・worker run の外は、ここでは止めない。
    let config = daemon_config(&e, &e.test.join("fresh.sqlite3"));
    assert_eq!(
        worker_db_guard_precheck(Some(&e.home), &config, &inside(&e)),
        WorkerDbGuardAction::Probe
    );
    let config = daemon_config(&e, &e.db);
    assert_eq!(
        worker_db_guard_precheck(Some(&e.home), &config, &WorkerRunMarker::Absent),
        WorkerDbGuardAction::Probe
    );
}
