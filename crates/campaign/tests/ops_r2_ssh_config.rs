//! ops-r2-sandbox.sh の `--ssh-config`（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-ssh）。
//! 専用 ssh config 1 file だけを /etc/ssh/ssh_config に bind し（親 dir は見せない）、/etc/ssh は空の tmpfs で
//! 覆って ssh_config.d（Bad owner の原因）を隠す。/etc の bind より後に置かれることを print-args で確かめる。
//! preflight の隔離検査（`preflight_isolation::run`）も B の daemon と同じ `--ssh-config` を包みに渡す（付記 D-bssh）。
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

/// preflight の `--b-ssh-config`（`ScriptSandbox.ssh_config`）が、許可表を作る `--print-args` と probe の起動の
/// 両方に `--ssh-config <file>` として渡ることを、argv を記録する偽の sandbox script で確かめる（bwrap は使わない）。
#[test]
fn preflight_passes_ssh_config_to_print_args_and_the_probe() {
    use celeris_campaign::preflight_isolation::{ProbeRequest, ScriptSandbox, Wrap, run};
    let d = fixture();
    let log = d.path().join("sandbox-argv.txt");
    let fake = d.path().join("fake-sandbox.sh");
    // 呼ばれるたびに argv を 1 行（NUL 区切りでなく改行区切り、呼び出しの区切りは空行）で記録する。
    // --print-args なら bwrap の引数の形を、そうでなければ probe の JSON を出す。
    std::fs::write(
        &fake,
        format!(
            "for a in \"$@\"; do printf '%s\\n' \"$a\"; done >> '{}'\necho >> '{}'\n\
             for a in \"$@\"; do [ \"$a\" = --print-args ] && {{ printf '%s\\n' --ro-bind /usr /usr; exit 0; }}; done\n\
             printf '%s' '{{\"schema\":\"celeris.runtime-probe/1\",\"principal\":\"B\",\"uid\":1,\"paths\":[],\"mounts_outside_allow\":[],\"ok\":true}}'\n",
            log.display(),
            log.display()
        ),
    )
    .unwrap();
    let cfg = d.path().join("ssh/config");
    let req = ProbeRequest {
        principal: "B",
        program: fake.clone(),
        wrap: Wrap::Script {
            script: fake.clone(),
            sandbox: ScriptSandbox {
                state: Some(d.path().join("state")),
                release: Some(d.path().join("release")),
                account: Some(d.path().join("account")),
                ro: Vec::new(),
                sockets: Vec::new(),
                ssh_config: Some(cfg.clone()),
            },
        },
        deny: vec![d.path().join("ssh/other")],
    };
    let probe = run(&req).expect("probe");
    assert!(probe.ok);
    let text = std::fs::read_to_string(&log).unwrap();
    let calls: Vec<Vec<&str>> = text
        .split("\n\n")
        .filter(|c| !c.trim().is_empty())
        .map(|c| c.lines().collect())
        .collect();
    assert_eq!(calls.len(), 2, "print-args and the probe: {text}");
    let c = cfg.to_string_lossy();
    for call in &calls {
        let sep = call.iter().position(|a| *a == "--").expect("--");
        let at = call
            .windows(2)
            .position(|w| w[0] == "--ssh-config" && w[1] == c)
            .unwrap_or_else(|| panic!("--ssh-config {c} in {call:?}"));
        assert!(at < sep, "{call:?}");
    }
    assert!(calls[0].contains(&"--print-args"));
    assert!(calls[1].contains(&"probe-read"));
}
