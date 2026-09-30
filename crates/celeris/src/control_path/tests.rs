use super::*;

const GOOD_PATH: &str = "/run/user/1001/ssh-mux-rmaeda@pegasus:22";

fn out(path: &str, master: &str, persist: Option<&str>) -> String {
    let mut s = format!("host pegasus\nuser rmaeda\ncontrolmaster {master}\ncontrolpath {path}\n");
    if let Some(p) = persist {
        s.push_str(&format!("controlpersist {p}\n"));
    }
    s
}

fn good_env() -> ControlPathEnv {
    ControlPathEnv {
        xdg_runtime_dir: Some("/run/user/1001".into()),
        linger: Some(true),
        parent_owned_by_me: Some(true),
        parent_mode: Some(0o700),
    }
}

fn warns(findings: &[ControlPathFinding]) -> Vec<&ControlPathFinding> {
    findings
        .iter()
        .filter(|f| f.severity == Severity::Warn)
        .collect()
}

#[test]
fn a_normal_setup_has_no_warnings_only_the_persist_info() {
    let f = control_path_warnings(&out(GOOD_PATH, "auto", Some("10m")), &good_env());
    assert!(warns(&f).is_empty(), "{f:?}");
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].severity, Severity::Info);
    assert!(f[0].message.contains("ControlPersist=10m"), "{f:?}");
}

#[test]
fn controlpath_none_is_a_warning() {
    let f = control_path_warnings(&out("none", "auto", None), &good_env());
    assert_eq!(warns(&f).len(), 1, "{f:?}");
}

#[test]
fn controlmaster_no_is_a_warning() {
    for m in ["no", "false"] {
        let f = control_path_warnings(&out(GOOD_PATH, m, None), &good_env());
        assert_eq!(warns(&f).len(), 1, "{m}: {f:?}");
    }
}

#[test]
fn a_path_over_90_bytes_is_a_warning_and_exactly_90_is_not() {
    let base = "/run/user/1001/";
    let long = format!("{base}{}", "a".repeat(91 - base.len()));
    assert_eq!(long.len(), 91);
    let f = control_path_warnings(&out(&long, "auto", None), &good_env());
    assert_eq!(warns(&f).len(), 1, "{f:?}");
    let ok = format!("{base}{}", "a".repeat(90 - base.len()));
    let f = control_path_warnings(&out(&ok, "auto", None), &good_env());
    assert!(warns(&f).is_empty(), "{f:?}");
}

#[test]
fn linger_off_under_xdg_runtime_dir_is_a_warning() {
    for linger in [Some(false), None] {
        let env = ControlPathEnv {
            linger,
            ..good_env()
        };
        let f = control_path_warnings(&out(GOOD_PATH, "auto", None), &env);
        assert_eq!(warns(&f).len(), 1, "{linger:?}: {f:?}");
    }
    // XDG_RUNTIME_DIR の外なら Linger は関係ない。
    let env = ControlPathEnv {
        linger: Some(false),
        ..good_env()
    };
    let f = control_path_warnings(&out("/home/u/.ssh/mux-pegasus", "auto", None), &env);
    assert!(warns(&f).is_empty(), "{f:?}");
}

#[test]
fn a_parent_dir_not_mine_or_not_0700_is_a_warning() {
    let env = ControlPathEnv {
        parent_owned_by_me: Some(false),
        ..good_env()
    };
    let f = control_path_warnings(&out(GOOD_PATH, "auto", None), &env);
    assert_eq!(warns(&f).len(), 1, "{f:?}");
    let env = ControlPathEnv {
        parent_mode: Some(0o755),
        ..good_env()
    };
    let f = control_path_warnings(&out(GOOD_PATH, "auto", None), &env);
    assert_eq!(warns(&f).len(), 1, "{f:?}");
}

#[test]
fn an_unexpanded_token_skips_the_length_and_directory_checks() {
    let path = format!("/run/user/1001/{}%C", "a".repeat(100));
    let env = ControlPathEnv {
        linger: Some(false),
        parent_mode: Some(0o777),
        ..good_env()
    };
    let f = control_path_warnings(&out(&path, "auto", None), &env);
    assert!(warns(&f).is_empty(), "{f:?}");
}
