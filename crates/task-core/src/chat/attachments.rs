//! Local, durable chat attachment blobs (ADR CoS chat D4).
//!
//! The database is the index. The blob name is generated here, never derived from a
//! client filename or path. A second SQLite connection lets reservation transactions
//! serialize with the conversation store on the same database.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime, UtcOffset};
use ulid::Ulid;

#[derive(Debug, Clone, Copy)]
pub struct ChatAttachmentLimits {
    pub max_file_bytes: u64,
    pub max_message_bytes: u64,
    pub max_files_per_message: usize,
    pub max_storage_bytes: u64,
    pub orphan_ttl_hours: i64,
    pub unreferenced_retention_days: i64,
}

impl Default for ChatAttachmentLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 25 * 1024 * 1024,
            max_message_bytes: 100 * 1024 * 1024,
            max_files_per_message: 10,
            max_storage_bytes: 10 * 1024 * 1024 * 1024,
            orphan_ttl_hours: 24,
            unreferenced_retention_days: 30,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    #[error("invalid attachment identifier or path")]
    InvalidPath,
    #[error("attachment limit exceeded")]
    Limit,
    #[error("invalid attachment limits")]
    InvalidLimits,
    #[error("attachment conflicts with an existing upload or reference")]
    Conflict,
    #[error("attachment not found")]
    NotFound,
    #[error("attachment hash mismatch")]
    HashMismatch,
    #[error("attachment store lock poisoned")]
    Poisoned,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Time(#[from] time::error::Format),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAttachment {
    pub id: String,
    pub thread_id: String,
    pub original_name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub relative_path: String,
}

pub struct ChatAttachmentStore {
    root: PathBuf,
    conn: Mutex<Connection>,
    limits: ChatAttachmentLimits,
}

impl ChatAttachmentStore {
    /// The caller opens/migrates the database first. `data_dir` is trusted daemon
    /// configuration, never an API parameter.
    pub fn open(
        data_dir: &Path,
        db_path: &Path,
        limits: ChatAttachmentLimits,
    ) -> Result<Self, AttachmentError> {
        if limits.max_file_bytes > i64::MAX as u64
            || limits.max_storage_bytes > i64::MAX as u64
            || limits.max_message_bytes > i64::MAX as u64
            || limits.orphan_ttl_hours <= 0
            || limits.unreferenced_retention_days <= 0
        {
            return Err(AttachmentError::InvalidLimits);
        }
        let root = data_dir.join("chat");
        reject_symlinks(data_dir)?;
        if !data_dir.is_dir() {
            return Err(AttachmentError::InvalidPath);
        }
        ensure_private_dir(&root)?;
        ensure_private_dir(&root.join("attachments"))?;
        ensure_private_dir(&root.join("staging"))?;
        let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        Ok(Self {
            root,
            conn: Mutex::new(conn),
            limits,
        })
    }

    /// Streams from a reader with bounded memory. `declared_bytes` controls the
    /// reservation only; the counted stream remains authoritative.
    pub fn upload(
        &self,
        thread_id: &str,
        client_upload_id: &str,
        original_name: &str,
        declared_bytes: Option<u64>,
        mut source: impl Read,
        now: OffsetDateTime,
    ) -> Result<StoredAttachment, AttachmentError> {
        if thread_id.is_empty() || client_upload_id.is_empty() || original_name.is_empty() {
            return Err(AttachmentError::InvalidPath);
        }
        let reserved = declared_bytes.unwrap_or(self.limits.max_file_bytes);
        if reserved > self.limits.max_file_bytes || reserved > i64::MAX as u64 {
            return Err(AttachmentError::Limit);
        }
        let now_text = stamp(now)?;
        let lease = stamp(now + Duration::hours(1))?;
        let reservation_id = Ulid::new().to_string();
        let existing = {
            let mut conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "DELETE FROM chat_upload_reservations WHERE lease_expires_at <= ?1",
                [&now_text],
            )?;
            let existing: Option<String> = tx.query_row(
                "SELECT result_id FROM chat_client_requests WHERE kind='upload' AND scope_id=?1 AND key=?2",
                params![thread_id, client_upload_id], |r| r.get(0),
            ).optional()?;
            if existing.is_none() {
                let active: Option<String> = tx.query_row(
                    "SELECT id FROM chat_upload_reservations WHERE thread_id=?1 AND client_upload_id=?2",
                    params![thread_id, client_upload_id], |r| r.get(0),
                ).optional()?;
                if active.is_some() {
                    return Err(AttachmentError::Conflict);
                }
                let used: i64 = tx.query_row(
                    "SELECT COALESCE((SELECT SUM(size_bytes) FROM chat_attachments WHERE state='ready'),0) + \
                     COALESCE((SELECT SUM(reserved_bytes) FROM chat_upload_reservations WHERE lease_expires_at > ?1),0)",
                    [&now_text], |r| r.get(0),
                )?;
                let available = self
                    .limits
                    .max_storage_bytes
                    .saturating_sub(used.max(0) as u64);
                if reserved > available {
                    return Err(AttachmentError::Limit);
                }
                tx.execute(
                    "INSERT INTO chat_upload_reservations(id,thread_id,client_upload_id,reserved_bytes,lease_expires_at,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
                    params![reservation_id, thread_id, client_upload_id, reserved as i64, lease, now_text],
                )?;
            }
            tx.commit()?;
            existing
        };
        let stage = self.root.join("staging").join(&reservation_id);
        let result = (|| {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&stage)?;
            let mut hash = Sha256::new();
            let mut size = 0u64;
            let mut magic = [0u8; 16];
            let mut magic_len = 0usize;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let count = source.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                size = size
                    .checked_add(count as u64)
                    .ok_or(AttachmentError::Limit)?;
                if size > self.limits.max_file_bytes || size > reserved && existing.is_none() {
                    return Err(AttachmentError::Limit);
                }
                let take = (magic.len() - magic_len).min(count);
                magic[magic_len..magic_len + take].copy_from_slice(&buffer[..take]);
                magic_len += take;
                hash.update(&buffer[..count]);
                output.write_all(&buffer[..count])?;
            }
            output.sync_all()?;
            drop(output);
            let digest = format!("{:x}", hash.finalize());
            if let Some(id) = existing.as_ref() {
                let row = self.get(id).map_err(|e| match e {
                    AttachmentError::NotFound => AttachmentError::Conflict,
                    other => other,
                })?;
                if row.thread_id == thread_id
                    && row.original_name == original_name
                    && row.size_bytes == size
                    && row.sha256 == digest
                {
                    return Ok(row);
                }
                return Err(AttachmentError::Conflict);
            }
            let id = reservation_id.clone();
            let relative_path = format!("attachments/{id}/blob");
            let directory = self.root.join("attachments").join(&id);
            fs::create_dir(&directory)?;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
            let final_path = directory.join("blob");
            fs::rename(&stage, &final_path)?;
            File::open(&directory)?.sync_all()?;
            File::open(self.root.join("attachments"))?.sync_all()?;
            let row = StoredAttachment {
                id,
                thread_id: thread_id.to_owned(),
                original_name: original_name.to_owned(),
                media_type: detect_media_type(&magic[..magic_len]).to_owned(),
                size_bytes: size,
                sha256: digest,
                relative_path,
            };
            let db_result =
                self.finish_upload(&row, client_upload_id, &reservation_id, &now_text, now);
            if db_result.is_err() {
                let _ = fs::remove_file(&final_path);
                let _ = fs::remove_dir(&directory);
            }
            db_result.map(|()| row)
        })();
        let _ = fs::remove_file(&stage);
        if result.is_err() && existing.is_none() {
            let _ = self.abort(&reservation_id);
        }
        result
    }

    fn finish_upload(
        &self,
        row: &StoredAttachment,
        client_upload_id: &str,
        reservation_id: &str,
        now_text: &str,
        now: OffsetDateTime,
    ) -> Result<(), AttachmentError> {
        let expires = stamp(now + Duration::hours(self.limits.orphan_ttl_hours))?;
        let mut conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let lease: Option<String> = tx
            .query_row(
                "SELECT lease_expires_at FROM chat_upload_reservations WHERE id=?1",
                [reservation_id],
                |r| r.get(0),
            )
            .optional()?;
        if lease.as_deref().is_none_or(|s| s <= now_text) {
            return Err(AttachmentError::Conflict);
        }
        tx.execute(
            "INSERT INTO chat_attachments(id,thread_id,original_name,media_type,size_bytes,sha256,relative_path,state,created_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,'ready',?8,?9)",
            params![row.id, row.thread_id, row.original_name, row.media_type, row.size_bytes as i64,
                row.sha256, row.relative_path, now_text, expires],
        )?;
        tx.execute(
            "INSERT INTO chat_client_requests(kind,scope_id,key,request_hash,result_id,created_at) VALUES('upload',?1,?2,?3,?4,?5)",
            params![row.thread_id, client_upload_id, row.sha256, row.id, now_text],
        )?;
        tx.execute(
            "DELETE FROM chat_upload_reservations WHERE id=?1",
            [reservation_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn abort(&self, reservation_id: &str) -> Result<(), AttachmentError> {
        let conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
        conn.execute(
            "DELETE FROM chat_upload_reservations WHERE id=?1",
            [reservation_id],
        )?;
        if valid_id(reservation_id) {
            let _ = fs::remove_file(self.root.join("staging").join(reservation_id));
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<StoredAttachment, AttachmentError> {
        if !valid_id(id) {
            return Err(AttachmentError::InvalidPath);
        }
        let conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
        conn.query_row(
            "SELECT id,thread_id,original_name,media_type,size_bytes,sha256,relative_path FROM chat_attachments WHERE id=?1 AND state='ready'",
            [id],
            |r| Ok(StoredAttachment {
                id: r.get(0)?, thread_id: r.get(1)?, original_name: r.get(2)?,
                media_type: r.get(3)?, size_bytes: r.get::<_, i64>(4)? as u64,
                sha256: r.get(5)?, relative_path: r.get(6)?,
            }),
        ).optional()?.ok_or(AttachmentError::NotFound)
    }

    /// Verifies the bytes before handing them to a worker. The relative path is
    /// checked against the generated shape even if the database was tampered with.
    pub fn read_verified(&self, id: &str) -> Result<File, AttachmentError> {
        let row = self.get(id)?;
        if row.relative_path != format!("attachments/{id}/blob") {
            return Err(AttachmentError::InvalidPath);
        }
        let path = self.root.join(&row.relative_path);
        reject_symlinks(&path)?;
        let mut file = File::open(&path)?;
        let mut hash = Sha256::new();
        let mut count = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            count = count.checked_add(n as u64).ok_or(AttachmentError::Limit)?;
            if count > row.size_bytes {
                return Err(AttachmentError::HashMismatch);
            }
            hash.update(&buffer[..n]);
        }
        if count != row.size_bytes || format!("{:x}", hash.finalize()) != row.sha256 {
            return Err(AttachmentError::HashMismatch);
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }

    pub fn validate_message(&self, thread_id: &str, ids: &[String]) -> Result<(), AttachmentError> {
        if ids.len() > self.limits.max_files_per_message {
            return Err(AttachmentError::Limit);
        }
        let mut total = 0u64;
        for id in ids {
            let row = self.get(id)?;
            if row.thread_id != thread_id {
                return Err(AttachmentError::Conflict);
            }
            total = total
                .checked_add(row.size_bytes)
                .ok_or(AttachmentError::Limit)?;
            if total > self.limits.max_message_bytes {
                return Err(AttachmentError::Limit);
            }
        }
        Ok(())
    }

    pub fn add_ref(
        &self,
        id: &str,
        owner_kind: &str,
        owner_id: &str,
        now: OffsetDateTime,
    ) -> Result<(), AttachmentError> {
        if !valid_id(id)
            || !matches!(owner_kind, "message" | "task" | "knowledge_inbox")
            || owner_id.is_empty()
        {
            return Err(AttachmentError::InvalidPath);
        }
        let mut conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_attachments WHERE id=?1 AND state='ready')",
            [id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(AttachmentError::NotFound);
        }
        tx.execute(
            "INSERT OR IGNORE INTO chat_attachment_refs(attachment_id,owner_kind,owner_id,created_at) VALUES(?1,?2,?3,?4)",
            params![id, owner_kind, owner_id, stamp(now)?],
        )?;
        tx.execute(
            "UPDATE chat_attachments SET expires_at=NULL WHERE id=?1",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_ref(
        &self,
        id: &str,
        owner_kind: &str,
        owner_id: &str,
        now: OffsetDateTime,
    ) -> Result<(), AttachmentError> {
        if !valid_id(id)
            || !matches!(owner_kind, "message" | "task" | "knowledge_inbox")
            || owner_id.is_empty()
        {
            return Err(AttachmentError::InvalidPath);
        }
        let mut conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if owner_kind == "message" {
            let active: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_runs WHERE (input_message_id=?1 OR output_message_id=?1) AND state IN ('queued','running','stopping'))",
                [owner_id], |r| r.get(0),
            )?;
            if active {
                return Err(AttachmentError::Conflict);
            }
        }
        tx.execute("DELETE FROM chat_attachment_refs WHERE attachment_id=?1 AND owner_kind=?2 AND owner_id=?3", params![id, owner_kind, owner_id])?;
        tx.execute(
            "UPDATE chat_attachments SET expires_at=?2 WHERE id=?1 AND state='ready' AND NOT EXISTS (SELECT 1 FROM chat_attachment_refs WHERE attachment_id=?1)",
            params![id, stamp(now + Duration::days(self.limits.unreferenced_retention_days))?],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_unreferenced(&self, id: &str) -> Result<(), AttachmentError> {
        if !valid_id(id) {
            return Err(AttachmentError::InvalidPath);
        }
        let mut conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let refs: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_attachment_refs WHERE attachment_id=?1)",
            [id],
            |r| r.get(0),
        )?;
        if refs {
            return Err(AttachmentError::Conflict);
        }
        let changed = tx.execute(
            "UPDATE chat_attachments SET state='deleted' WHERE id=?1 AND state='ready'",
            [id],
        )?;
        tx.commit()?;
        if changed != 0 {
            self.remove_blob(id)?;
        }
        Ok(())
    }

    fn remove_blob(&self, id: &str) -> Result<(), AttachmentError> {
        let path = self.root.join("attachments").join(id);
        reject_symlinks(&path)?;
        match fs::remove_file(path.join("blob")) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let _ = fs::remove_dir(&path);
        Ok(())
    }

    fn delete_expired(&self, id: &str, now_text: &str) -> Result<bool, AttachmentError> {
        let mut conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE chat_attachments SET state='deleted' WHERE id=?1 AND state='ready' AND expires_at <= ?2 AND NOT EXISTS (SELECT 1 FROM chat_attachment_refs WHERE attachment_id=?1)",
            params![id, now_text],
        )?;
        tx.commit()?;
        if changed != 0 {
            self.remove_blob(id)?;
        }
        Ok(changed != 0)
    }

    pub fn gc(&self, now: OffsetDateTime) -> Result<usize, AttachmentError> {
        let now_text = stamp(now)?;
        let expired: Vec<String> = {
            let conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
            let mut stmt = conn.prepare(
                "SELECT id FROM chat_attachments WHERE state='ready' AND expires_at <= ?1 AND NOT EXISTS (SELECT 1 FROM chat_attachment_refs WHERE attachment_id=chat_attachments.id)",
            )?;
            stmt.query_map([&now_text], |r| r.get(0))?
                .collect::<Result<_, _>>()?
        };
        let mut deleted = 0;
        for id in &expired {
            deleted += usize::from(self.delete_expired(id, &now_text)?);
        }
        let stale: Vec<String> = {
            let conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
            let mut stmt = conn
                .prepare("SELECT id FROM chat_upload_reservations WHERE lease_expires_at <= ?1")?;
            stmt.query_map([&now_text], |r| r.get(0))?
                .collect::<Result<_, _>>()?
        };
        for id in &stale {
            self.abort(id)?;
        }
        // A crash may leave either a staged file after its reservation was
        // removed, or a blob after the row became deleted. Both are safe to
        // retry because a live reservation/ready row is checked first.
        for entry in fs::read_dir(self.root.join("staging"))? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if !valid_id(&id) {
                continue;
            }
            let active: bool = {
                let conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM chat_upload_reservations WHERE id=?1 AND lease_expires_at > ?2)",
                    params![id, now_text], |r| r.get(0),
                )?
            };
            if !active {
                match fs::remove_file(entry.path()) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e.into()),
                }
            }
        }
        for entry in fs::read_dir(self.root.join("attachments"))? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().into_owned();
            if !valid_id(&id) {
                continue;
            }
            let mut conn = self.conn.lock().map_err(|_| AttachmentError::Poisoned)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let live: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_attachments WHERE id=?1 AND state='ready') OR EXISTS(SELECT 1 FROM chat_upload_reservations WHERE id=?1 AND lease_expires_at > ?2)",
                params![id, now_text], |r| r.get(0),
            )?;
            if !live {
                self.remove_blob(&id)?;
            }
            tx.commit()?;
        }
        Ok(deleted)
    }
}

fn stamp(now: OffsetDateTime) -> Result<String, AttachmentError> {
    // Fixed precision keeps SQLite TEXT ordering correct even when a caller's
    // injected clock has fractional seconds.
    let now = now.to_offset(UtcOffset::UTC);
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.nanosecond(),
    ))
}

fn valid_id(id: &str) -> bool {
    id.len() == 26 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn ensure_private_dir(path: &Path) -> Result<(), AttachmentError> {
    reject_symlinks(path)?;
    if path.exists() {
        if !path.is_dir() {
            return Err(AttachmentError::InvalidPath);
        }
    } else {
        fs::create_dir(path)?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn reject_symlinks(path: &Path) -> Result<(), AttachmentError> {
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(AttachmentError::InvalidPath);
        }
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(AttachmentError::InvalidPath),
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn detect_media_type(head: &[u8]) -> &'static str {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if head.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        "image/gif"
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        "image/webp"
    } else if head.starts_with(b"%PDF-") {
        "application/pdf"
    } else if head.starts_with(b"PK\x03\x04") {
        "application/zip"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SqliteStore;
    use std::io::Cursor;
    use std::os::unix::fs::symlink;
    use std::sync::{Arc, Barrier};

    fn fixture(limits: ChatAttachmentLimits) -> (tempfile::TempDir, ChatAttachmentStore) {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("db.sqlite");
        let _store = SqliteStore::open(&db).expect("migrate");
        let attachments = ChatAttachmentStore::open(temp.path(), &db, limits).expect("attachments");
        (temp, attachments)
    }

    fn now() -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH
    }

    fn small_limits() -> ChatAttachmentLimits {
        ChatAttachmentLimits {
            max_file_bytes: 8,
            max_storage_bytes: 16,
            ..ChatAttachmentLimits::default()
        }
    }

    #[test]
    fn chat_attach_stores_blob_with_hash_and_modes() {
        let (temp, store) = fixture(small_limits());
        let bytes = b"%PDF-123";
        let row = store
            .upload(
                "thread",
                "upload",
                "../paper.txt",
                None,
                Cursor::new(bytes),
                now(),
            )
            .expect("upload");
        assert_eq!(row.media_type, "application/pdf");
        assert_eq!(row.sha256, format!("{:x}", Sha256::digest(bytes)));
        assert_eq!(row.relative_path, format!("attachments/{}/blob", row.id));
        let path = temp.path().join("chat").join(&row.relative_path);
        assert_eq!(
            fs::metadata(path.parent().expect("parent"))
                .expect("dir mode")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).expect("file mode").permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(temp.path().join("chat/staging"))
                .expect("stage mode")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let mut copy = Vec::new();
        store
            .read_verified(&row.id)
            .expect("verified")
            .read_to_end(&mut copy)
            .expect("read");
        assert_eq!(copy, bytes);
    }

    #[test]
    fn chat_attach_file_limit_exact_and_over() {
        let (temp, store) = fixture(small_limits());
        let row = store
            .upload(
                "thread",
                "exact",
                "x",
                None,
                Cursor::new(b"12345678"),
                now(),
            )
            .expect("exact limit");
        assert_eq!(row.size_bytes, 8);
        assert!(matches!(
            store.upload(
                "thread",
                "too-large",
                "x",
                None,
                Cursor::new(b"123456789"),
                now()
            ),
            Err(AttachmentError::Limit)
        ));
        assert!(matches!(
            store.upload(
                "thread",
                "understated",
                "x",
                Some(1),
                Cursor::new(b"12"),
                now()
            ),
            Err(AttachmentError::Limit)
        ));
        assert_eq!(
            fs::read_dir(temp.path().join("chat/staging"))
                .expect("staging")
                .count(),
            0
        );
    }

    #[test]
    fn chat_attach_unknown_magic_is_octet_stream() {
        let (_temp, store) = fixture(small_limits());
        let row = store
            .upload(
                "thread",
                "plain",
                "a.png",
                None,
                Cursor::new(b"hello"),
                now(),
            )
            .expect("upload");
        assert_eq!(row.media_type, "application/octet-stream");
    }

    #[test]
    fn chat_attach_read_verified_rejects_tampered_blob() {
        let (temp, store) = fixture(small_limits());
        let row = store
            .upload("thread", "key", "x", None, Cursor::new(b"abc"), now())
            .expect("upload");
        let path = temp.path().join("chat").join(&row.relative_path);
        fs::write(&path, b"tampered!").expect("tamper");
        assert!(matches!(
            store.read_verified(&row.id),
            Err(AttachmentError::HashMismatch)
        ));
    }

    #[test]
    fn chat_attach_idempotent_client_upload_id() {
        let (_temp, store) = fixture(small_limits());
        let first = store
            .upload("thread", "key", "a", None, Cursor::new(b"abc"), now())
            .expect("first");
        let retry = store
            .upload("thread", "key", "a", None, Cursor::new(b"abc"), now())
            .expect("retry");
        assert_eq!(first.id, retry.id);
        assert!(matches!(
            store.upload("thread", "key", "b", None, Cursor::new(b"abc"), now()),
            Err(AttachmentError::Conflict)
        ));
        assert!(matches!(
            store.upload("thread", "key", "a", None, Cursor::new(b"abd"), now()),
            Err(AttachmentError::Conflict)
        ));
        let second = store
            .upload("thread", "other", "a", None, Cursor::new(b"abc"), now())
            .expect("same hash separate id");
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn chat_attach_message_count_and_thread_checks() {
        let limits = ChatAttachmentLimits {
            max_file_bytes: 8,
            max_message_bytes: 8,
            max_files_per_message: 1,
            ..ChatAttachmentLimits::default()
        };
        let (_temp, store) = fixture(limits);
        let first = store
            .upload("thread", "a", "a", None, Cursor::new(b"abc"), now())
            .expect("first");
        let second = store
            .upload("thread", "b", "b", None, Cursor::new(b"abc"), now())
            .expect("second");
        store
            .validate_message("thread", std::slice::from_ref(&first.id))
            .expect("one file");
        assert!(matches!(
            store.validate_message("thread", &[first.id.clone(), second.id]),
            Err(AttachmentError::Limit)
        ));
        assert!(matches!(
            store.validate_message("another", std::slice::from_ref(&first.id)),
            Err(AttachmentError::Conflict)
        ));
    }

    #[test]
    fn chat_attach_refs_block_delete_and_gc_by_injected_clock() {
        let (_temp, store) = fixture(small_limits());
        let first = store
            .upload("thread", "key", "a", None, Cursor::new(b"abc"), now())
            .expect("first");
        let orphan = store
            .upload("thread", "other", "a", None, Cursor::new(b"abc"), now())
            .expect("orphan");
        store
            .add_ref(&first.id, "task", "task-1", now())
            .expect("pin");
        assert!(matches!(
            store.delete_unreferenced(&first.id),
            Err(AttachmentError::Conflict)
        ));
        assert_eq!(store.gc(now() + Duration::hours(23)).expect("early gc"), 0);
        assert_eq!(store.gc(now() + Duration::hours(25)).expect("orphan gc"), 1);
        assert!(matches!(
            store.get(&orphan.id),
            Err(AttachmentError::NotFound)
        ));
        store
            .remove_ref(&first.id, "task", "task-1", now() + Duration::hours(25))
            .expect("unpin");
        assert_eq!(store.gc(now() + Duration::days(29)).expect("retention"), 0);
        assert_eq!(
            store.gc(now() + Duration::days(32)).expect("retention gc"),
            1
        );
        store
            .delete_unreferenced(&first.id)
            .expect("idempotent delete");
    }

    #[test]
    fn chat_attach_message_byte_limit_exact_and_over() {
        let limits = ChatAttachmentLimits {
            max_file_bytes: 8,
            max_message_bytes: 5,
            max_files_per_message: 2,
            ..ChatAttachmentLimits::default()
        };
        let (_temp, store) = fixture(limits);
        let a = store
            .upload("thread", "a", "a", None, Cursor::new(b"abc"), now())
            .expect("a");
        let b = store
            .upload("thread", "b", "b", None, Cursor::new(b"de"), now())
            .expect("b");
        let c = store
            .upload("thread", "c", "c", None, Cursor::new(b"def"), now())
            .expect("c");
        store
            .validate_message("thread", &[a.id.clone(), b.id])
            .expect("exact message bytes");
        assert!(matches!(
            store.validate_message("thread", &[a.id, c.id]),
            Err(AttachmentError::Limit)
        ));
    }

    #[test]
    fn chat_attach_rejects_parent_symlink_and_cross_thread_reuse() {
        let (temp, store) = fixture(ChatAttachmentLimits::default());
        let row = store
            .upload("thread", "key", "x", None, Cursor::new(b"x"), now())
            .expect("upload");
        assert!(matches!(
            store
                .upload("other", "key", "x", None, Cursor::new(b"x"), now())
                .map(|r| r.id == row.id),
            Ok(false)
        ));
        assert!(matches!(
            store.get("../blob"),
            Err(AttachmentError::InvalidPath)
        ));
        let link = temp.path().join("link");
        symlink(temp.path(), &link).expect("symlink");
        let db = temp.path().join("db.sqlite");
        assert!(matches!(
            ChatAttachmentStore::open(&link, &db, ChatAttachmentLimits::default()),
            Err(AttachmentError::InvalidPath)
        ));
        let path = temp.path().join("chat").join(&row.relative_path);
        fs::remove_file(&path).expect("remove blob");
        symlink(temp.path().join("db.sqlite"), &path).expect("replace blob with link");
        assert!(matches!(
            store.read_verified(&row.id),
            Err(AttachmentError::InvalidPath)
        ));
    }

    #[test]
    fn chat_attach_concurrent_reservation_counts_in_flight() {
        struct WaitingReader {
            entered: Arc<Barrier>,
            release: Arc<Barrier>,
            done: bool,
        }
        impl Read for WaitingReader {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if self.done {
                    return Ok(0);
                }
                self.entered.wait();
                self.release.wait();
                out[0] = b'a';
                self.done = true;
                Ok(1)
            }
        }
        let limits = ChatAttachmentLimits {
            max_file_bytes: 8,
            max_storage_bytes: 8,
            ..ChatAttachmentLimits::default()
        };
        let (_temp, store) = fixture(limits);
        let store = Arc::new(store);
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let worker = {
            let store = Arc::clone(&store);
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            std::thread::spawn(move || {
                store.upload(
                    "thread",
                    "first",
                    "a",
                    None,
                    WaitingReader {
                        entered,
                        release,
                        done: false,
                    },
                    now(),
                )
            })
        };
        entered.wait();
        assert!(matches!(
            store.upload("thread", "second", "b", None, Cursor::new(b"b"), now()),
            Err(AttachmentError::Limit)
        ));
        release.wait();
        assert!(worker.join().expect("thread").is_ok());
    }

    #[test]
    fn chat_attach_running_run_keeps_message_ref_and_fractional_gc_boundary() {
        let (temp, store) = fixture(ChatAttachmentLimits::default());
        let t = now() + Duration::milliseconds(1);
        let row = store
            .upload("thread", "key", "x", None, Cursor::new(b"x"), t)
            .expect("upload");
        store
            .add_ref(&row.id, "message", "message", t)
            .expect("pin");
        let conn = Connection::open(temp.path().join("db.sqlite")).expect("db");
        conn.execute(
            "INSERT INTO chat_threads(id,kind,title,status,created_at,updated_at) VALUES('thread','human','t','open',?1,?1)",
            [stamp(t).expect("time")],
        )
        .expect("thread");
        conn.execute(
            "INSERT INTO chat_messages(id,thread_id,seq,role,text,state,created_at,updated_at) VALUES('message','thread',1,'user','x','running',?1,?1)",
            [stamp(t).expect("time")],
        )
        .expect("message");
        conn.execute(
            "INSERT INTO chat_runs(run_id,thread_id,input_message_id,state) VALUES('run','thread','message','running')",
            [],
        )
        .expect("run");
        assert!(matches!(
            store.remove_ref(&row.id, "message", "message", t),
            Err(AttachmentError::Conflict)
        ));
        assert_eq!(store.gc(t + Duration::days(40)).expect("pinned gc"), 0);
        conn.execute(
            "UPDATE chat_runs SET state='completed' WHERE run_id='run'",
            [],
        )
        .expect("complete");
        store
            .remove_ref(&row.id, "message", "message", t)
            .expect("unpin");
        assert_eq!(
            store
                .gc(t + Duration::days(30) - Duration::nanoseconds(1))
                .expect("before boundary"),
            0
        );
        assert_eq!(store.gc(t + Duration::days(30)).expect("at boundary"), 1);
    }

    #[test]
    fn chat_attach_gc_releases_expired_reservation_and_staging() {
        let limits = ChatAttachmentLimits {
            max_file_bytes: 8,
            max_storage_bytes: 8,
            ..ChatAttachmentLimits::default()
        };
        let (temp, store) = fixture(limits);
        let id = Ulid::new().to_string();
        let db = Connection::open(temp.path().join("db.sqlite")).expect("db");
        db.execute(
            "INSERT INTO chat_upload_reservations(id,thread_id,client_upload_id,reserved_bytes,lease_expires_at,created_at) VALUES(?1,'thread','aborted',8,?2,?3)",
            params![id, stamp(now() + Duration::hours(1)).expect("lease"), stamp(now()).expect("created")],
        ).expect("reserve");
        let stage = temp.path().join("chat/staging").join(&id);
        fs::write(&stage, b"incomplete").expect("stage");
        assert!(matches!(
            store.upload("thread", "new", "x", None, Cursor::new(b"x"), now()),
            Err(AttachmentError::Limit)
        ));
        assert_eq!(store.gc(now() + Duration::hours(1)).expect("gc"), 0);
        assert!(!stage.exists());
        store
            .upload(
                "thread",
                "new",
                "x",
                None,
                Cursor::new(b"x"),
                now() + Duration::hours(1),
            )
            .expect("capacity released");
    }
}
