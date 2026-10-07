//! ADR 2026-10-05 cos-chat-home D4: task に pin された添付を task run の入力として stage する試験
//! （偽ハーネス・一時 SQLite。外部の CLI も時刻待ちも使わない）。

use super::*;
use crate::dispatcher::input_attachments::InputAttachmentSource;
use std::os::unix::fs::PermissionsExt;
use task_core::chat::ChatMessage;
use task_core::chat::attachments::{ChatAttachmentLimits, ChatAttachmentStore};
use task_core::store::SqliteStore;
use task_worker::protocol::{InputAttachment, InputAttachmentDelivery};

/// 1 run 分の控え: 渡された入力 manifest と、各項目の path から読めた bytes。
type SeenRun = (Vec<InputAttachment>, Vec<Option<Vec<u8>>>);

/// 渡された入力 manifest と、run の最中に stage 先から読めた bytes を控えるアダプタ。
#[derive(Default)]
struct CaptureAdapter {
    seen: StdMutex<Vec<SeenRun>>,
}

#[async_trait]
impl WorkerAdapter for CaptureAdapter {
    fn id(&self) -> &str {
        "capture"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        let manifest = req.context.input_attachments.clone();
        let bytes = manifest
            .iter()
            .map(|item| item.path.as_ref().and_then(|p| std::fs::read(p).ok()))
            .collect();
        self.seen.lock().expect("seen").push((manifest, bytes));
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

struct Fixture {
    temp: crate::test_support::WritableTempDir,
    store: Arc<SqliteStore>,
    attachments: ChatAttachmentStore,
    source: InputAttachmentSource,
    task_dir: PathBuf,
    task: Task,
}

fn fixture() -> Fixture {
    let temp = crate::test_support::WritableTempDir::new();
    let data_dir = temp.path().join("data");
    std::fs::create_dir(&data_dir).expect("data dir");
    let db_path = data_dir.join("celeris.db");
    let store = Arc::new(SqliteStore::open(&db_path).expect("store"));
    let attachments =
        ChatAttachmentStore::open(&data_dir, &db_path, ChatAttachmentLimits::default())
            .expect("attachments");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).expect("workspace");
    let task_dir = temp.path().join("task");
    std::fs::create_dir(&task_dir).expect("task dir");
    let task = new_task(
        &workspace,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).expect("insert");
    Fixture {
        source: InputAttachmentSource {
            data_dir,
            db_path,
            limits: ChatAttachmentLimits::default(),
        },
        temp,
        store,
        attachments,
        task_dir,
        task,
    }
}

async fn run_task(f: &Fixture, adapter: Arc<CaptureAdapter>) {
    let outcome = run_worker(
        f.store.clone(),
        adapter,
        Vec::new(),
        f.task.id,
        Tier::Standard,
        f.task_dir.clone(),
        "run-attach",
        RunLimits {
            wall_clock: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(5),
            kill_grace: Duration::from_millis(100),
        },
        LeaseRenewal {
            ttl: Duration::from_secs(60),
            every: Duration::from_secs(30),
        },
        None,
        None,
        RunExtras {
            input_attachment_source: Some(f.source.clone()),
            ..RunExtras::default()
        },
        Vec::new(),
        Vec::new(),
        DelegationLimits::default(),
        None,
        None,
        ContainerDecision::Host,
        CargoTargetPlan::None,
    )
    .await
    .expect("run");
    assert!(matches!(outcome.terminal, Terminal::Done { .. }));
}

/// 0500 の dir を消せるように戻してから消す（試験の後始末と「chat の workspace を消す」操作）。
fn force_remove(path: &Path) {
    if let Ok(meta) = std::fs::symlink_metadata(path)
        && meta.is_dir()
    {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
        for entry in std::fs::read_dir(path).expect("read dir").flatten() {
            force_remove(&entry.path());
        }
        std::fs::remove_dir(path).expect("remove dir");
    } else if std::fs::symlink_metadata(path).is_ok() {
        std::fs::remove_file(path).expect("remove file");
    }
}

fn chat_message(thread: &str, ids: &[String]) -> ChatMessage {
    serde_json::from_value(serde_json::json!({
        "id":"m", "thread_id":thread, "seq":1, "role":"user",
        "text":"この画面を直して", "state":"completed", "client_message_id":null,
        "reply_to_id":null, "run_id":null, "attachment_ids":ids,
        "cards":[], "created_at":"2026-10-07T00:00:00Z",
        "updated_at":"2026-10-07T00:00:00Z"
    }))
    .expect("chat message")
}

#[tokio::test]
async fn cos_chat_attach_handoff_task_run_stages_pinned_attachment() {
    let f = fixture();
    let bytes = b"\x89PNG\r\n\x1a\nscreenshot-bytes".to_vec();
    let thread = "THREAD1";
    let image = f
        .attachments
        .upload(
            thread,
            "upload-1",
            "screen shot.png",
            None,
            std::io::Cursor::new(bytes.clone()),
            OffsetDateTime::now_utc(),
        )
        .expect("upload");
    // CoS chat run が自分の thread workspace に stage した写し（後続の task run は使わない）。
    let chat_manifest = crate::dispatcher::cos_chat::attachments::stage_message_attachments(
        &f.attachments,
        &f.source.data_dir,
        &chat_message(thread, std::slice::from_ref(&image.id)),
    )
    .expect("chat stage");
    assert!(chat_manifest[0].path.exists());
    // pin（CoS が references API で task へ）→ chat の一時 file・workspace を消す → run 開始。
    f.attachments
        .add_ref(
            &image.id,
            "task",
            &f.task.id.to_string(),
            OffsetDateTime::now_utc(),
        )
        .expect("pin");
    force_remove(&f.source.data_dir.join("cos"));
    assert!(!chat_manifest[0].path.exists());

    let adapter = Arc::new(CaptureAdapter::default());
    run_task(&f, adapter.clone()).await;
    // 2 回目の run（continuation 等）も同じ写しを照合して使い回せる。
    run_task(&f, adapter.clone()).await;

    let seen = adapter.seen.lock().expect("seen");
    assert_eq!(seen.len(), 2);
    for (manifest, read) in seen.iter() {
        assert_eq!(manifest.len(), 1, "{manifest:?}");
        let item = &manifest[0];
        assert_eq!(item.id, image.id);
        assert_eq!(item.name, "screen shot.png");
        assert_eq!(item.media_type, "image/png");
        assert_eq!(item.size_bytes, bytes.len() as u64);
        assert_eq!(item.sha256, image.sha256);
        assert_eq!(item.delivery, InputAttachmentDelivery::Image);
        assert!(item.reason.is_none());
        let path = item.path.as_ref().expect("staged path");
        assert_eq!(
            path,
            &f.task_dir
                .join("attachments")
                .join(&image.id)
                .join("screen_shot.png")
        );
        // 作業ツリー（task の workspace）の外で、元チャットの workspace でもない。
        assert!(!path.starts_with(f.temp.path().join("workspace")));
        assert!(!path.starts_with(f.source.data_dir.join("cos")));
        assert_eq!(read[0].as_deref(), Some(bytes.as_slice()));
    }
    let path = seen[0].0[0].path.clone().expect("path");
    drop(seen);
    assert_eq!(
        task_worker::artifact::sha256_file(&path).expect("hash"),
        image.sha256
    );
    let mode = |p: &Path| std::fs::metadata(p).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode(&path), 0o400);
    assert_eq!(mode(path.parent().expect("id dir")), 0o500);
    assert_eq!(mode(&f.task_dir.join("attachments")), 0o500);
    force_remove(&f.task_dir.join("attachments"));
}

#[tokio::test]
async fn cos_chat_attach_handoff_task_run_hash_mismatch_not_delivered() {
    let f = fixture();
    let bytes = b"%PDF-1.7 paper".to_vec();
    let pdf = f
        .attachments
        .upload(
            "THREAD2",
            "upload-1",
            "paper.pdf",
            None,
            std::io::Cursor::new(bytes.clone()),
            OffsetDateTime::now_utc(),
        )
        .expect("upload");
    f.attachments
        .add_ref(
            &pdf.id,
            "task",
            &f.task.id.to_string(),
            OffsetDateTime::now_utc(),
        )
        .expect("pin");
    let adapter = Arc::new(CaptureAdapter::default());
    // 1 回目は照合が通って渡る。
    run_task(&f, adapter.clone()).await;
    let staged = f
        .task_dir
        .join("attachments")
        .join(&pdf.id)
        .join("paper.pdf");
    assert!(staged.exists());
    // 原本を同じ長さの別 bytes に書き換える（DB の sha256 と食い違う）。
    let blob = f
        .source
        .data_dir
        .join("chat/attachments")
        .join(&pdf.id)
        .join("blob");
    std::fs::write(&blob, b"%PDF-1.7 PAPER").expect("tamper");
    run_task(&f, adapter.clone()).await;

    let seen = adapter.seen.lock().expect("seen");
    assert_eq!(seen[0].0[0].delivery, InputAttachmentDelivery::File);
    let item = &seen[1].0[0];
    assert_eq!(item.id, pdf.id);
    assert_eq!(item.delivery, InputAttachmentDelivery::Unavailable);
    assert!(item.path.is_none(), "{item:?}");
    assert_eq!(item.sha256, pdf.sha256);
    let reason = item.reason.as_deref().expect("reason");
    assert!(reason.contains("hash mismatch"), "{reason}");
    assert_eq!(seen[1].1[0], None);
    drop(seen);
    // 前の run の写しも残さない（読めたと装わない）。
    assert!(!staged.exists());
    force_remove(&f.task_dir.join("attachments"));
}

#[tokio::test]
async fn cos_chat_attach_handoff_task_run_without_pins_is_unchanged() {
    let f = fixture();
    // 別 owner（message・別 task）への pin は、この task の入力にならない。
    let other = f
        .attachments
        .upload(
            "THREAD3",
            "upload-1",
            "a.txt",
            None,
            std::io::Cursor::new(b"x".to_vec()),
            OffsetDateTime::now_utc(),
        )
        .expect("upload");
    f.attachments
        .add_ref(&other.id, "message", "m1", OffsetDateTime::now_utc())
        .expect("message pin");
    f.attachments
        .add_ref(&other.id, "task", "another-task", OffsetDateTime::now_utc())
        .expect("other task pin");
    let adapter = Arc::new(CaptureAdapter::default());
    run_task(&f, adapter.clone()).await;
    assert!(adapter.seen.lock().expect("seen")[0].0.is_empty());
    assert!(!f.task_dir.join("attachments").exists());
}

#[tokio::test]
async fn cos_chat_attach_handoff_remote_run_marks_unavailable() {
    let f = fixture();
    let row = f
        .attachments
        .upload(
            "THREAD4",
            "upload-1",
            "a.png",
            None,
            std::io::Cursor::new(b"\x89PNG\r\n\x1a\n".to_vec()),
            OffsetDateTime::now_utc(),
        )
        .expect("upload");
    f.attachments
        .add_ref(
            &row.id,
            "task",
            &f.task.id.to_string(),
            OffsetDateTime::now_utc(),
        )
        .expect("pin");
    let manifest = crate::dispatcher::input_attachments::stage_task_input_attachments(
        &f.source,
        &f.task.id.to_string(),
        &f.task_dir,
        true,
    )
    .expect("manifest");
    assert_eq!(manifest[0].delivery, InputAttachmentDelivery::Unavailable);
    assert!(manifest[0].path.is_none());
    assert!(!f.task_dir.join("attachments").exists());
}
