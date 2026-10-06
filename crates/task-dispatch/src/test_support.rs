//! Test executable writer: keep the writable fd out of the test process (ETXTBSY).
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub(crate) fn write_executable(path: &Path, contents: &str) {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn stub writer");
    child
        .stdin
        .take()
        .expect("stub writer stdin")
        .write_all(contents.as_bytes())
        .expect("write stub");
    assert!(child.wait().expect("wait stub writer").success());
}
