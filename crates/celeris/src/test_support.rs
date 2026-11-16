//! celeris の unit 試験が共有する helper。

use std::io::Write;
use std::path::Path;

/// `#!/bin/sh` と `script` を実行 file として書く。`sh` の子 process が書いて `chmod` するので、
/// 試験 process が書き込み用に開いた fd を持ったまま exec して `ETXTBSY` になることが無い。
pub(crate) fn write_executable(path: &Path, script: &str) {
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(format!("#!/bin/sh\n{script}\n").as_bytes())
        .unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
