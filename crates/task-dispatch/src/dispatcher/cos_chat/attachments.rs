//! Verified, read-only delivery of chat attachments into a CoS thread workspace.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use task_core::chat::ChatMessage;
use task_core::chat::attachments::{AttachmentError, ChatAttachmentStore};

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error("unsafe CoS workspace or attachment path")]
    UnsafePath,
    #[error("attachment {id} belongs to another thread")]
    WrongThread { id: String },
    #[error("attachment {id}: {source}")]
    Attachment {
        id: String,
        #[source]
        source: AttachmentError,
    },
    #[error("attachment {id} changed while being staged")]
    HashMismatch { id: String },
    #[error("attachment {id}: {source}")]
    Io {
        id: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    WorkspaceIo(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentManifest {
    pub id: String,
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub path: PathBuf,
    pub delivery: String,
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn reject_symlinks(path: &Path) -> Result<(), StageError> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(StageError::UnsafePath);
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(StageError::UnsafePath),
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(StageError::WorkspaceIo(e)),
        }
    }
    Ok(())
}

fn private_dir(path: &Path) -> Result<(), StageError> {
    reject_symlinks(path)?;
    if path.exists() {
        if !path.is_dir() {
            return Err(StageError::UnsafePath);
        }
    } else {
        fs::create_dir(path)?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Create the stable working directory for one thread. Thread ids are path components,
/// never arbitrary caller-supplied paths.
pub fn workspace_dir(data_dir: &Path, thread_id: &str) -> Result<PathBuf, StageError> {
    if !safe_id(thread_id) {
        return Err(StageError::UnsafePath);
    }
    reject_symlinks(data_dir)?;
    if !data_dir.is_dir() {
        return Err(StageError::UnsafePath);
    }
    let cos = data_dir.join("cos");
    let threads = cos.join("threads");
    let thread = threads.join(thread_id);
    let workspace = thread.join("workspace");
    for path in [&cos, &threads, &thread, &workspace] {
        private_dir(path)?;
    }
    Ok(workspace)
}

fn safe_filename(name: &str) -> Result<String, StageError> {
    if name.is_empty() || name.contains("..") {
        return Err(StageError::UnsafePath);
    }
    // Do not use the supplied name as a path: keep only a conservative set of
    // characters in the leaf, with the attachment id as its parent component.
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() || safe == "." || safe == ".." {
        return Err(StageError::UnsafePath);
    }
    Ok(safe)
}

/// Verify every source before creating any staged attachment. The source store checks
/// its DB metadata, generated blob path, symlinks, byte count and SHA-256.
pub fn stage_message_attachments(
    store: &ChatAttachmentStore,
    data_dir: &Path,
    message: &ChatMessage,
) -> Result<Vec<AttachmentManifest>, StageError> {
    if !safe_id(&message.thread_id) {
        return Err(StageError::UnsafePath);
    }
    let mut sources = Vec::with_capacity(message.attachment_ids.len());
    for id in &message.attachment_ids {
        let row = store.get(id).map_err(|source| StageError::Attachment {
            id: id.clone(),
            source,
        })?;
        if row.thread_id != message.thread_id {
            return Err(StageError::WrongThread { id: id.clone() });
        }
        let filename = safe_filename(&row.original_name)?;
        let file = store
            .read_verified(id)
            .map_err(|source| StageError::Attachment {
                id: id.clone(),
                source,
            })?;
        sources.push((row, filename, file));
    }
    let workspace = workspace_dir(data_dir, &message.thread_id)?;
    let attachments = workspace.join("attachments");
    private_dir(&attachments)?;
    let mut manifest = Vec::with_capacity(sources.len());
    for (row, filename, mut source) in sources {
        let directory = attachments.join(&row.id);
        reject_symlinks(&directory)?;
        if directory.exists() {
            if !directory.is_dir() {
                return Err(StageError::UnsafePath);
            }
        } else {
            private_dir(&directory)?;
        }
        let path = directory.join(&filename);
        reject_symlinks(&path)?;
        if path.exists() {
            // A later run in this thread may receive the same attachment again.
            // Reuse it only if its bytes still match the durable source record.
            let mut existing = File::open(&path).map_err(|source| StageError::Io {
                id: row.id.clone(),
                source,
            })?;
            if !existing
                .metadata()
                .map_err(|source| StageError::Io {
                    id: row.id.clone(),
                    source,
                })?
                .is_file()
            {
                return Err(StageError::UnsafePath);
            }
            let mut hash = Sha256::new();
            let mut size = 0u64;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = existing.read(&mut buf).map_err(|source| StageError::Io {
                    id: row.id.clone(),
                    source,
                })?;
                if n == 0 {
                    break;
                }
                size = size
                    .checked_add(n as u64)
                    .ok_or_else(|| StageError::HashMismatch { id: row.id.clone() })?;
                hash.update(&buf[..n]);
            }
            if size != row.size_bytes || format!("{:x}", hash.finalize()) != row.sha256 {
                return Err(StageError::HashMismatch { id: row.id });
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))?;
        } else {
            let result = (|| -> Result<(), StageError> {
                let mut target = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)
                    .map_err(|source| StageError::Io {
                        id: row.id.clone(),
                        source,
                    })?;
                let mut hash = Sha256::new();
                let mut size = 0u64;
                let mut buf = [0u8; 64 * 1024];
                loop {
                    let n = source.read(&mut buf).map_err(|source| StageError::Io {
                        id: row.id.clone(),
                        source,
                    })?;
                    if n == 0 {
                        break;
                    }
                    size = size
                        .checked_add(n as u64)
                        .ok_or_else(|| StageError::HashMismatch { id: row.id.clone() })?;
                    hash.update(&buf[..n]);
                    target
                        .write_all(&buf[..n])
                        .map_err(|source| StageError::Io {
                            id: row.id.clone(),
                            source,
                        })?;
                }
                if size != row.size_bytes || format!("{:x}", hash.finalize()) != row.sha256 {
                    return Err(StageError::HashMismatch { id: row.id.clone() });
                }
                target.sync_all().map_err(|source| StageError::Io {
                    id: row.id.clone(),
                    source,
                })?;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))?;
                Ok(())
            })();
            if let Err(error) = result {
                let _ = fs::remove_file(&path);
                return Err(error);
            }
        }
        let delivery = if matches!(
            row.media_type.as_str(),
            "image/jpeg" | "image/png" | "image/webp" | "image/gif"
        ) {
            "image"
        } else {
            "file"
        };
        manifest.push(AttachmentManifest {
            id: row.id,
            name: row.original_name,
            media_type: row.media_type,
            size_bytes: row.size_bytes,
            sha256: row.sha256,
            path,
            delivery: delivery.to_owned(),
        });
    }
    Ok(manifest)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
