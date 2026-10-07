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

/// A `tempfile::TempDir` that is safe to drop even when the test made entries read-only
/// (cos_chat attachments are 0o500 dirs / 0o400 files): permissions are restored before removal.
pub(crate) struct WritableTempDir(tempfile::TempDir);

impl WritableTempDir {
    pub(crate) fn new() -> Self {
        Self(tempfile::tempdir().expect("tempdir"))
    }
}

impl std::ops::Deref for WritableTempDir {
    type Target = tempfile::TempDir;
    fn deref(&self) -> &tempfile::TempDir {
        &self.0
    }
}

impl Drop for WritableTempDir {
    fn drop(&mut self) {
        restore_write(self.0.path());
    }
}

fn restore_write(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            restore_write(&entry.path());
        }
    }
}
