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

/// The CoS thread of a new project titled `key` (ADR 2026-10-22-cos-fixed-threads D1): tests that
/// need several CoS threads get one per project.
pub(crate) fn project_cos_thread(store: &task_core::SqliteStore, key: &str) -> String {
    let now = time::OffsetDateTime::now_utc();
    let project = task_core::Project {
        id: task_core::ProjectId::new(),
        title: key.into(),
        request: key.into(),
        status: task_core::ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        auto_advance: false,
        auto_integrate: false,
        slug: None,
        created_at: now,
        updated_at: now,
    };
    task_core::TaskStore::project_create(store, &project).expect("project");
    store
        .cos_thread_open(
            &task_core::chat::ChatOpenThreadRequest {
                project_id: Some(project.id.to_string()),
            },
            now,
        )
        .expect("project thread")
        .thread
        .id
}
