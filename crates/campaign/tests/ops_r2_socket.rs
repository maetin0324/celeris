//! ops-r2-sandbox.sh の `--socket`（ADR 2026-10-31-ops-r2-preflight-fixes 付記 D-socket）。
//! socket 1 つだけを bind し（親 dir は見せない）、socket 以外は拒み、preflight の許可表がこの bind を含むことを確かめる。
//! 包みの中からの connect は実 bwrap を使うので CELERIS_USERNS_TESTS=1 のときだけ走る。外部 network には出ない。
use std::{os::unix::net::UnixListener, path::PathBuf, process::Command};

use celeris_campaign::{preflight_isolation::allow_sources, runtime_probe::mounts_outside_allow};

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/campaign/ops-r2-sandbox.sh")
}

struct Fx {
    _d: tempfile::TempDir,
    _m: tempfile::TempDir,
    root: PathBuf,
    mux: PathBuf,
    sock: PathBuf,
    _l: UnixListener,
}

fn fx() -> Fx {
    // workspaces・state の下は拒まれるので cargo の target の一時 dir を使う。
    let d = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let root = d.path().to_path_buf();
    for p in ["state", "release", "account"] {
        std::fs::create_dir_all(root.join(p)).unwrap();
    }
    // unix socket の path は 108 byte 未満（SUN_LEN）。target の深さに依らないよう socket の dir だけを /tmp 直下に置く
    // （/tmp は workspaces・state の下ではないので ops-r2-sandbox.sh に拒まれない）。
    let m = tempfile::Builder::new()
        .prefix("r2s")
        .tempdir_in("/tmp")
        .unwrap();
    let mux = m.path().to_path_buf();
    std::fs::write(mux.join("other"), "sibling").unwrap();
    let sock = mux.join("master.sock");
    let l = UnixListener::bind(&sock).unwrap();
    Fx {
        _d: d,
        _m: m,
        root,
        mux,
        sock,
        _l: l,
    }
}

fn base(f: &Fx) -> Command {
    let mut c = Command::new("sh");
    c.arg(script());
    for p in ["state", "release", "account"] {
        c.arg(format!("--{p}")).arg(f.root.join(p));
    }
    c
}

#[test]
fn socket_flag_rejects_non_sockets() {
    let f = fx();
    for bad in [f.mux.join("other"), f.mux.clone(), f.mux.join("none")] {
        let o = base(&f)
            .arg("--socket")
            .arg(&bad)
            .args(["--print-args", "--", "true"])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{}", bad.display());
        assert!(String::from_utf8_lossy(&o.stderr).contains("not a unix socket"));
    }
    // --ro は socket を受けない（通常 file・dir だけ）。
    let o = base(&f)
        .arg("--ro")
        .arg(&f.sock)
        .args(["--print-args", "--", "true"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn socket_bind_is_only_the_socket_and_in_the_allow_table() {
    let f = fx();
    let o = base(&f)
        .arg("--socket")
        .arg(&f.sock)
        .args(["--print-args", "--", "true"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let args: Vec<String> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let s = f.sock.to_string_lossy().into_owned();
    assert!(
        args.windows(3)
            .any(|w| w[0] == "--bind" && w[1] == s && w[2] == s),
        "{args:?}"
    );
    let mux = f.mux.to_string_lossy().into_owned();
    assert!(
        !args.contains(&mux),
        "parent dir must not be bound: {args:?}"
    );
    // 許可表（allow_sources）に socket の載る filesystem が入り、その mount は外扱いにならない。
    let host = std::fs::read_to_string("/proc/self/mountinfo").unwrap();
    let allow = allow_sources(&args, &host);
    let canon = std::fs::canonicalize(&f.sock).unwrap();
    let line = host
        .lines()
        .filter(|l| {
            let mp = l.split_whitespace().nth(4).unwrap_or("");
            canon.starts_with(mp)
        })
        .max_by_key(|l| l.split_whitespace().nth(4).unwrap_or("").len())
        .unwrap();
    assert_eq!(
        mounts_outside_allow(line, &allow).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn connect_to_socket_inside_wrap_and_parent_hidden() {
    let setting = std::env::var("CELERIS_USERNS_TESTS").unwrap_or_default();
    if setting != "1" && setting != "require" {
        eprintln!("skip: requires CELERIS_USERNS_TESTS=1 (or require)");
        return;
    }
    let f = fx();
    let code = format!(
        "import os,socket;s=socket.socket(socket.AF_UNIX);s.connect({:?});print(os.path.exists({:?}))",
        f.sock.to_string_lossy(),
        f.mux.join("other").to_string_lossy()
    );
    let o = base(&f)
        .arg("--socket")
        .arg(&f.sock)
        .args(["--", "python3", "-I", "-c", &code])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "False");
}
