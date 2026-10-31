//! ops-r2-sandbox.sh の `--ssh-config`（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-ssh）。
//! 専用 ssh config 1 file だけを /etc/ssh/ssh_config に bind し（親 dir は見せない）、/etc/ssh は空の tmpfs で
//! 覆って ssh_config.d（Bad owner の原因）を隠す。/etc の bind より後に置かれることを print-args で確かめる。
//! 実 ssh・実 sirius には出ない。
use std::{path::PathBuf, process::Command};

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/campaign/ops-r2-sandbox.sh")
}

fn print_args(root: &std::path::Path, extra: &[&std::ffi::OsStr]) -> std::process::Output {
    let mut c = Command::new("sh");
    c.arg(script());
    for p in ["state", "release", "account"] {
        c.arg(format!("--{p}")).arg(root.join(p));
    }
    c.args(extra).args(["--print-args", "--", "ssh", "sirius"]);
    c.output().unwrap()
}

fn fixture() -> tempfile::TempDir {
    let d = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    for p in ["state", "release", "account", "ssh"] {
        std::fs::create_dir_all(d.path().join(p)).unwrap();
    }
    std::fs::write(d.path().join("ssh/config"), "Host sirius\n").unwrap();
    std::fs::write(d.path().join("ssh/other"), "sibling").unwrap();
    d
}

#[test]
fn ssh_config_is_bound_as_system_config_after_etc() {
    let d = fixture();
    let cfg = d.path().join("ssh/config");
    let o = print_args(d.path(), &["--ssh-config".as_ref(), cfg.as_os_str()]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let args: Vec<String> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let c = cfg.to_string_lossy().into_owned();
    let bind = args
        .windows(3)
        .position(|w| w[0] == "--ro-bind" && w[1] == c && w[2] == "/etc/ssh/ssh_config")
        .expect("ssh config bind");
    let tmpfs = args
        .windows(2)
        .position(|w| w[0] == "--tmpfs" && w[1] == "/etc/ssh")
        .expect("tmpfs over /etc/ssh");
    let etc = args
        .windows(3)
        .position(|w| w[0] == "--ro-bind" && w[1] == "/etc" && w[2] == "/etc")
        .expect("/etc bind");
    assert!(etc < tmpfs && tmpfs < bind, "{args:?}");
    let parent = d.path().join("ssh").to_string_lossy().into_owned();
    assert!(
        !args.contains(&parent),
        "parent dir must not be bound: {args:?}"
    );
    // -F は command に足されない（試行に教えない）。
    assert!(!args.iter().any(|a| a == "-F"), "{args:?}");
}

#[test]
fn ssh_config_is_the_same_for_a_and_b_shapes() {
    // A と B は state・account が違うだけで、ssh の bind は同じ 3 語になる。
    let d = fixture();
    let cfg = d.path().join("ssh/config");
    let tail = |o: std::process::Output| -> Vec<String> {
        let a: Vec<String> = String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        let i = a.iter().position(|x| x == "/etc/ssh").unwrap();
        a[i - 1..i + 4].to_vec()
    };
    let a = tail(print_args(
        d.path(),
        &["--ssh-config".as_ref(), cfg.as_os_str()],
    ));
    let b = tail(print_args(
        d.path(),
        &["--ssh-config".as_ref(), cfg.as_os_str()],
    ));
    assert_eq!(a, b);
}

#[test]
fn ssh_config_rejects_dirs_and_forbidden_paths() {
    let d = fixture();
    let o = print_args(
        d.path(),
        &["--ssh-config".as_ref(), d.path().join("ssh").as_os_str()],
    );
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("not a file"));
}
