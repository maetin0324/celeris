use super::{StageError, stage_message_attachments, workspace_dir};
use std::fs;
use std::io::Cursor;
use std::os::unix::fs::{PermissionsExt, symlink};

use task_core::chat::ChatMessage;
use task_core::chat::attachments::{AttachmentError, ChatAttachmentLimits, ChatAttachmentStore};
use task_core::store::SqliteStore;
use time::OffsetDateTime;

fn fixture() -> (tempfile::TempDir, ChatAttachmentStore) {
    let temp = tempfile::tempdir().expect("tempdir");
    let db = temp.path().join("test.sqlite");
    let _store = SqliteStore::open(&db).expect("migrate");
    let attachments = ChatAttachmentStore::open(temp.path(), &db, ChatAttachmentLimits::default())
        .expect("attachment store");
    (temp, attachments)
}

fn message(ids: &[String]) -> ChatMessage {
    serde_json::from_value(serde_json::json!({
        "id":"m", "thread_id":"THREAD1", "seq":1, "role":"user",
        "text":"see files", "state":"queued", "client_message_id":null,
        "reply_to_id":null, "run_id":null, "attachment_ids":ids,
        "cards":[], "created_at":"2026-10-06T00:00:00Z",
        "updated_at":"2026-10-06T00:00:00Z"
    }))
    .expect("chat message")
}

#[test]
fn cos_chat_run_attach_stages_hash_matched_read_only_image_and_file() {
    let (temp, store) = fixture();
    let png = b"\x89PNG\r\n\x1a\nimage";
    let image = store
        .upload(
            "THREAD1",
            "png",
            "screen.png",
            None,
            Cursor::new(png),
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("png");
    let pdf = store
        .upload(
            "THREAD1",
            "pdf",
            "paper.pdf",
            None,
            Cursor::new(b"%PDF-doc"),
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("pdf");
    let manifest = stage_message_attachments(
        &store,
        temp.path(),
        &message(&[image.id.clone(), pdf.id.clone()]),
    )
    .expect("stage");
    assert_eq!(manifest.len(), 2);
    assert_eq!(manifest[0].delivery, "image");
    assert_eq!(manifest[1].delivery, "file");
    assert_eq!(manifest[0].media_type, "image/png");
    assert_eq!(manifest[0].name, "screen.png");
    assert_eq!(manifest[0].sha256, image.sha256);
    assert_eq!(fs::read(&manifest[0].path).expect("staged image"), png);
    assert_eq!(
        manifest[0].path,
        temp.path()
            .join("cos/threads/THREAD1/workspace/attachments")
            .join(&image.id)
            .join("screen.png")
    );
    for entry in &manifest {
        assert_eq!(
            fs::metadata(&entry.path)
                .expect("file")
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
        assert_eq!(
            fs::metadata(entry.path.parent().expect("parent"))
                .expect("dir")
                .permissions()
                .mode()
                & 0o777,
            0o500
        );
    }
    let again = stage_message_attachments(
        &store,
        temp.path(),
        &message(&[image.id.clone(), pdf.id.clone()]),
    )
    .expect("stage same attachments for later run");
    assert_eq!(again, manifest);
}

#[test]
fn cos_chat_run_attach_rejects_missing_and_mismatched_blob_without_staging() {
    let (temp, store) = fixture();
    let good = store
        .upload(
            "THREAD1",
            "good",
            "good.txt",
            None,
            Cursor::new(b"good"),
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("good");
    let bad = store
        .upload(
            "THREAD1",
            "bad",
            "bad.txt",
            None,
            Cursor::new(b"bad"),
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("bad");
    fs::write(temp.path().join("chat").join(&bad.relative_path), b"BAD").expect("tamper");
    assert!(matches!(
        stage_message_attachments(
            &store,
            temp.path(),
            &message(&[good.id.clone(), bad.id.clone()])
        ),
        Err(StageError::Attachment {
            source: AttachmentError::HashMismatch,
            ..
        })
    ));
    assert!(
        !temp
            .path()
            .join("cos/threads/THREAD1/workspace/attachments")
            .exists()
    );
    fs::remove_file(temp.path().join("chat").join(&good.relative_path)).expect("remove");
    assert!(matches!(
        stage_message_attachments(&store, temp.path(), &message(&[good.id.clone()])),
        Err(StageError::Attachment {
            source: AttachmentError::Io(_),
            ..
        })
    ));
    assert!(
        !temp
            .path()
            .join("cos/threads/THREAD1/workspace/attachments")
            .exists()
    );
}

#[test]
fn cos_chat_run_attach_rejects_symlink_and_traversal() {
    let (temp, store) = fixture();
    let row = store
        .upload(
            "THREAD1",
            "one",
            "normal.txt",
            None,
            Cursor::new(b"hello"),
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("upload");
    let source = temp.path().join("chat").join(&row.relative_path);
    fs::remove_file(&source).expect("remove blob");
    symlink(temp.path().join("test.sqlite"), &source).expect("symlink blob");
    assert!(matches!(
        stage_message_attachments(&store, temp.path(), &message(&[row.id.clone()])),
        Err(StageError::Attachment {
            source: AttachmentError::InvalidPath,
            ..
        })
    ));
    assert!(matches!(
        workspace_dir(temp.path(), "../outside"),
        Err(StageError::UnsafePath)
    ));
    let workspace_link = temp.path().join("cos");
    symlink(temp.path().join("chat"), &workspace_link).expect("symlink workspace");
    assert!(matches!(
        workspace_dir(temp.path(), "THREAD1"),
        Err(StageError::UnsafePath)
    ));
}

#[test]
fn cos_chat_run_attach_sanitizes_original_name_and_checks_thread() {
    let (temp, store) = fixture();
    let row = store
        .upload(
            "THREAD1",
            "one",
            "folder/file name.pdf",
            None,
            Cursor::new(b"%PDF-x"),
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("upload");
    let manifest =
        stage_message_attachments(&store, temp.path(), &message(&[row.id.clone()])).expect("stage");
    assert_eq!(manifest[0].name, "folder/file name.pdf");
    assert_eq!(
        manifest[0].path.file_name().expect("filename"),
        "folder_file_name.pdf"
    );
    let other = store
        .upload(
            "OTHER",
            "two",
            "foreign.txt",
            None,
            Cursor::new(b"other"),
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("upload");
    assert!(matches!(
        stage_message_attachments(&store, temp.path(), &message(&[other.id])),
        Err(StageError::WrongThread { .. })
    ));
    let traversal = store
        .upload(
            "THREAD1",
            "three",
            "../bad.txt",
            None,
            Cursor::new(b"bad"),
            OffsetDateTime::UNIX_EPOCH,
        )
        .expect("upload");
    assert!(matches!(
        stage_message_attachments(&store, temp.path(), &message(&[traversal.id])),
        Err(StageError::UnsafePath)
    ));
}

#[test]
fn cos_chat_run_attach_delivery_covers_all_supported_image_types() {
    let (temp, store) = fixture();
    let cases: &[(&str, &[u8], &str)] = &[
        ("photo.jpg", b"\xff\xd8\xffimage", "image/jpeg"),
        ("picture.png", b"\x89PNG\r\n\x1a\nimage", "image/png"),
        ("anim.gif", b"GIF89aimage", "image/gif"),
        ("web.webp", b"RIFF1234WEBPimage", "image/webp"),
        ("unknown.bin", b"not an image", "application/octet-stream"),
    ];
    for (index, (name, bytes, media_type)) in cases.iter().enumerate() {
        let row = store
            .upload(
                "THREAD1",
                &format!("upload-{index}"),
                name,
                None,
                Cursor::new(bytes),
                OffsetDateTime::UNIX_EPOCH,
            )
            .expect("upload");
        let staged =
            stage_message_attachments(&store, temp.path(), &message(&[row.id])).expect("stage");
        assert_eq!(staged[0].media_type, *media_type);
        assert_eq!(staged[0].delivery, if index < 4 { "image" } else { "file" });
    }
}
