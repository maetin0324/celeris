//! ADR-0126 A: `install_worker_db_guard` が使う P の組み立てと判定（userns を使わない）。

use std::path::{Path, PathBuf};

use task_worker::db_guard::{
    DaemonPaths, GuardDecision, PRODUCTION_IN_WORKER_RUN, WorkerRunMarker,
};

use super::{
    WorkerDbGuardAction, enforce_worker_db_guard, launcher_test_loopback_refusal,
    worker_db_guard_action, worker_db_guard_decision, worker_db_guard_opt_out_action,
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
fn worker_db_guard_test_db_is_exempt_outside_worker_run() {
    // ADR-0126 付記2 の 2 で期待を変えた（旧: RequireUserns）。本番に当たらず P が決まっている daemon は、
    // 印が無くても probe しない（印を運ばない試験の daemon が worker sandbox で落ちていたため）。
    let e = env(PROD_CONFIG);
    let d = decide(Some(&e.home), &test_daemon(&e), &WorkerRunMarker::Absent);
    assert_eq!(d, GuardDecision::Exempt);
    // 印が裏付けられない（偽の印）ときも同じ。
    let fake = WorkerRunMarker::Unverified {
        value: e.test.clone(),
        reason: "not read-only".into(),
    };
    assert_eq!(
        decide(Some(&e.home), &test_daemon(&e), &fake),
        GuardDecision::Exempt
    );
    // P が決まらなければ従来どおり probe（fail closed）。
    let d = decide(None, &test_daemon(&e), &WorkerRunMarker::Absent);
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

// ---------------------------------------------------------------------------
// ADR-0126 付記2: 本番に当たらない daemon は印なしでも Exempt、本番に当たる daemon は印なしでも Probe、
// worker run の中では opt-out（worker_read_only = false）でも本番一致を拒否する。
// ---------------------------------------------------------------------------

/// 試験 daemon の config（`worker_read_only = false` の opt-out。token_file は任意）。
fn opt_out_config(e: &Env, db: &Path, token: Option<&Path>) -> crate::Config {
    let path = e.test.join("opt-out.toml");
    let api = token
        .map(|t| format!("\n[api]\ntoken_file = \"{}\"\n", t.display()))
        .unwrap_or_default();
    std::fs::write(
        &path,
        format!(
            "[db]\npath = \"{}\"\nworker_read_only = false\n{api}{PROVIDER}",
            db.display()
        ),
    )
    .unwrap();
    let config = crate::Config::load(&path).unwrap();
    assert!(!config.db.worker_read_only);
    config
}

#[test]
fn worker_guard_exempt_test_db_without_marker() {
    let e = env(PROD_CONFIG);
    let mut daemon = test_daemon(&e);
    let token = e.test.join("api.token");
    std::fs::write(&token, "test-only-token\n").unwrap();
    daemon.token_file = Some(token);
    let decision = decide(Some(&e.home), &daemon, &WorkerRunMarker::Absent);
    assert_eq!(decision, GuardDecision::Exempt);
    let config = daemon_config(&e, &daemon.db);
    let mut probed = false;
    let got = enforce_worker_db_guard(&config, &decision, |_| {
        probed = true;
        Err(crate::DaemonError::DbGuard("probe must not run".into()))
    });
    assert!(matches!(got, Ok(None)), "{got:?}");
    assert!(!probed, "probe ran for a test DB without the marker");
    // 本番 config が無い host でも同じ。
    std::fs::remove_file(e.home.join(".config/celeris/config.toml")).unwrap();
    assert_eq!(
        decide(Some(&e.home), &daemon, &WorkerRunMarker::Absent),
        GuardDecision::Exempt
    );
}

#[test]
fn worker_guard_probe_production_without_marker() {
    let e = env(PROD_CONFIG);
    let check = |daemon: &DaemonPaths| {
        let decision = decide(Some(&e.home), daemon, &WorkerRunMarker::Absent);
        assert!(
            matches!(
                decision,
                GuardDecision::RefuseProduction {
                    inside_worker_run: false,
                    ..
                }
            ),
            "{decision:?}"
        );
        assert_eq!(
            worker_db_guard_action(&decision),
            WorkerDbGuardAction::Probe
        );
        let config = daemon_config(&e, &daemon.db);
        let mut probed = 0;
        let got = enforce_worker_db_guard(&config, &decision, |_| {
            probed += 1;
            Err(crate::DaemonError::DbGuard("userns unavailable".into()))
        });
        // probe が呼ばれ、失敗なら起動しない。
        assert_eq!(probed, 1);
        assert!(got.is_err());
    };
    // 本番 config の DB。
    check(&DaemonPaths {
        db: e.db.clone(),
        state_dir: None,
        token_file: None,
    });
    // 本番 token（写し）を使う daemon。
    let copy = e.test.join("copy.token");
    std::fs::write(&copy, "production-token\n").unwrap();
    let mut daemon = test_daemon(&e);
    daemon.token_file = Some(copy);
    check(&daemon);
}

#[test]
fn worker_guard_opt_out_refuses_production_inside_worker_run() {
    let e = env(PROD_CONFIG);
    let set = worker_db_guard_protected_set(Some(&e.home));
    let marker = WorkerRunMarker::ReadOnly(e.home.join(".config"));
    let prod = DaemonPaths {
        db: e.db.clone(),
        state_dir: None,
        token_file: None,
    };
    // 印の中・本番 DB: opt-out でも拒否（token の値は出さない）。
    match worker_db_guard_opt_out_action(&set, &prod, &marker) {
        WorkerDbGuardAction::Refuse(m) => {
            assert!(m.contains(PRODUCTION_IN_WORKER_RUN), "{m}");
        }
        other => panic!("not refused: {other:?}"),
    }
    // 印が裏付けられない（偽の印）中でも拒否。
    let fake = WorkerRunMarker::Unverified {
        value: e.test.clone(),
        reason: "not read-only".into(),
    };
    assert!(matches!(
        worker_db_guard_opt_out_action(&set, &prod, &fake),
        WorkerDbGuardAction::Refuse(_)
    ));
    // 印の中・本番 token の写し。
    let copy = e.test.join("copy.token");
    std::fs::write(&copy, "production-token\n").unwrap();
    let mut daemon = test_daemon(&e);
    daemon.token_file = Some(copy.clone());
    match worker_db_guard_opt_out_action(&set, &daemon, &marker) {
        WorkerDbGuardAction::Refuse(m) => assert!(!m.contains("production-token"), "{m}"),
        other => panic!("not refused: {other:?}"),
    }
    // 印の中でも本番に当たらない daemon は opt-out どおり guard なしで起動してよい。
    assert_eq!(
        worker_db_guard_opt_out_action(&set, &test_daemon(&e), &marker),
        WorkerDbGuardAction::Exempt
    );
    // 印の中で P が決まらなければ拒否（opt-out では probe で止める道が無い）。
    let unknown = worker_db_guard_protected_set(None);
    assert!(matches!(
        worker_db_guard_opt_out_action(&unknown, &test_daemon(&e), &marker),
        WorkerDbGuardAction::Refuse(_)
    ));
    // 印が無ければ opt-out が効く（本番 DB でも判定しない = worker run の外の運用）。
    assert_eq!(
        worker_db_guard_opt_out_action(&set, &prod, &WorkerRunMarker::Absent),
        WorkerDbGuardAction::Exempt
    );
    assert_eq!(
        worker_db_guard_opt_out_action(&unknown, &prod, &WorkerRunMarker::Absent),
        WorkerDbGuardAction::Exempt
    );
}

#[test]
fn worker_guard_opt_out_precheck_refuses_before_opening_db() {
    let e = env(PROD_CONFIG);
    let refused = |a: &WorkerDbGuardAction| matches!(a, WorkerDbGuardAction::Refuse(_));
    // 既にある本番 DB（symlink を通しても）。
    let link = e.test.join("link.sqlite3");
    std::os::unix::fs::symlink(&e.db, &link).unwrap();
    for db in [&e.db, &link] {
        let config = opt_out_config(&e, db, None);
        let a = worker_db_guard_precheck(Some(&e.home), &config, &inside(&e));
        assert!(refused(&a), "{db:?}: {a:?}");
    }
    // まだ無い DB が印の配下（相対 path・`..` を含む）。
    let config = opt_out_config(&e, &e.test.join("../lib/new/t.sqlite3"), None);
    let a = worker_db_guard_precheck(Some(&e.home), &config, &inside(&e));
    assert!(refused(&a), "{a:?}");
    // まだ無い DB でも本番 DB のディレクトリの中（印は別）。
    let marker = WorkerRunMarker::ReadOnly(e.home.join(".config"));
    let config = opt_out_config(&e, &e.lib.join("fresh.sqlite3"), None);
    let a = worker_db_guard_precheck(Some(&e.home), &config, &marker);
    assert!(refused(&a), "{a:?}");
    // まだ無い試験 DB でも本番 token の写しを使うなら DB を作る前に止める。
    let copy = e.test.join("copy.token");
    std::fs::write(&copy, "production-token\n").unwrap();
    let config = opt_out_config(&e, &e.test.join("fresh.sqlite3"), Some(&copy));
    let a = worker_db_guard_precheck(Some(&e.home), &config, &marker);
    assert!(refused(&a), "{a:?}");
    // 印の中で P が決まらない opt-out も止める。worker_read_only = true なら後の probe に任せる。
    let config = opt_out_config(&e, &e.test.join("fresh.sqlite3"), None);
    let a = worker_db_guard_precheck(None, &config, &marker);
    assert!(refused(&a), "{a:?}");
    let config = daemon_config(&e, &e.test.join("fresh.sqlite3"));
    assert_eq!(
        worker_db_guard_precheck(None, &config, &marker),
        WorkerDbGuardAction::Probe
    );
    // 本番に当たらない試験 DB・worker run の外は止めない。
    let config = opt_out_config(&e, &e.test.join("fresh.sqlite3"), None);
    assert_eq!(
        worker_db_guard_precheck(Some(&e.home), &config, &marker),
        WorkerDbGuardAction::Probe
    );
    let config = opt_out_config(&e, &e.db, None);
    assert_eq!(
        worker_db_guard_precheck(Some(&e.home), &config, &WorkerRunMarker::Absent),
        WorkerDbGuardAction::Probe
    );
}

// ---- 付記 E2（daemon 側）: 本番の config・DB の daemon は試験専用 loopback 許可の launcher を拒否する ----

#[test]
fn egress_test_loopback_production_db_or_config_daemon_refuses_test_launcher() {
    let e = env(PROD_CONFIG);
    let prod_config = e.home.join(".config/celeris/config.toml");
    let own_config = e.test.join("config.toml");
    std::fs::write(&own_config, "").unwrap();

    // 試験用 DB・別 config の daemon は拒否しない。
    assert_eq!(
        launcher_test_loopback_refusal(Some(&e.home), Some(&own_config), &test_daemon(&e)),
        None
    );
    // 本番 DB の daemon。
    let prod_db = DaemonPaths {
        db: e.db.clone(),
        state_dir: None,
        token_file: None,
    };
    let reason = launcher_test_loopback_refusal(Some(&e.home), Some(&own_config), &prod_db)
        .expect("production db");
    assert!(reason.contains("production db"), "{reason}");
    // 本番 token の daemon（中身が同じ別 file も）。
    let copy = e.test.join("api.token");
    std::fs::write(&copy, "production-token\n").unwrap();
    let mut prod_token = test_daemon(&e);
    prod_token.token_file = Some(copy);
    assert!(
        launcher_test_loopback_refusal(Some(&e.home), Some(&own_config), &prod_token).is_some()
    );
    let mut prod_token_file = test_daemon(&e);
    prod_token_file.token_file = Some(e.token.clone());
    assert!(
        launcher_test_loopback_refusal(Some(&e.home), Some(&own_config), &prod_token_file)
            .is_some()
    );
    // 本番 config を読んだ daemon（verify の staging のように DB だけ差し替えても）。
    let reason =
        launcher_test_loopback_refusal(Some(&e.home), Some(&prod_config), &test_daemon(&e))
            .expect("production config");
    assert!(reason.contains("production config"), "{reason}");
    // symlink 経由の本番 config も同じ。
    let link = e.test.join("config-link.toml");
    std::os::unix::fs::symlink(&prod_config, &link).unwrap();
    assert!(launcher_test_loopback_refusal(Some(&e.home), Some(&link), &test_daemon(&e)).is_some());
    // home が決まらない・本番 config が壊れている（判定不能）は拒否。
    assert!(launcher_test_loopback_refusal(None, Some(&own_config), &test_daemon(&e)).is_some());
    std::fs::write(&prod_config, "db = [").unwrap();
    assert!(
        launcher_test_loopback_refusal(Some(&e.home), Some(&own_config), &test_daemon(&e))
            .is_some()
    );
}
