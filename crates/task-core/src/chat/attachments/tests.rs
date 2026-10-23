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
fn cos_chat_attach_handoff_list_for_owner_returns_ready_pins_in_pin_order() {
    let (_temp, store) = fixture(ChatAttachmentLimits::default());
    let first = store
        .upload("thread", "k1", "a.png", None, Cursor::new(b"abc"), now())
        .expect("first");
    let second = store
        .upload("thread", "k2", "b.pdf", None, Cursor::new(b"%PDF"), now())
        .expect("second");
    let other = store
        .upload("thread", "k3", "c", None, Cursor::new(b"x"), now())
        .expect("other");
    store
        .add_ref(&second.id, "task", "T1", now())
        .expect("pin second");
    store
        .add_ref(&first.id, "task", "T1", now() + Duration::seconds(1))
        .expect("pin first");
    store
        .add_ref(&other.id, "knowledge_inbox", "T1", now())
        .expect("other owner kind");
    let ids: Vec<String> = store
        .list_for_owner("task", "T1")
        .expect("list")
        .into_iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(ids, vec![second.id.clone(), first.id.clone()]);
    assert!(
        store
            .list_for_owner("task", "T2")
            .expect("empty")
            .is_empty()
    );
    assert!(matches!(
        store.list_for_owner("thread", "T1"),
        Err(AttachmentError::InvalidPath)
    ));
    store
        .remove_ref(&second.id, "task", "T1", now())
        .expect("unpin");
    assert_eq!(store.list_for_owner("task", "T1").expect("list").len(), 1);
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
        "INSERT INTO chat_threads(id,kind,title,status,created_at,updated_at) VALUES('thread','general','t','open',?1,?1)",
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
