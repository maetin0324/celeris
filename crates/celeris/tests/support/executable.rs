//! 実行 file を書く helper。`#[path]` で読む。読み手: `cap_probe_real_daemon.rs`・`ops_r1_measure.rs`。

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// `body` をそのまま実行 file として書く。`/bin/sh` の子 process が書いて `chmod` するので、
/// 試験 process が書き込み用に開いた fd を持ったまま exec して `ETXTBSY` になることが無い。
pub fn write_executable(path: &Path, body: &str) {
    let mut c = Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn");
    c.stdin
        .take()
        .expect("stdin")
        .write_all(body.as_bytes())
        .expect("write");
    assert!(c.wait().expect("wait").success());
}
