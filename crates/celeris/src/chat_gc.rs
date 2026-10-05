//! ADR 2026-10-05-cos-chat-home D4: CoS チャットの周期 GC（LLM なし・決定的）。
//!
//! 1 回の pass（`chat_gc_once`）は、添付の GC（未送信 `orphan_ttl_hours`・最後の参照が外れて
//! `unreferenced_retention_days`）と upload 予約の期限回収（`ChatAttachmentStore::gc`）、run 終端後の
//! text/tool の chat_events の retention（`[cos] stream_retention_days`）を回す。時刻は呼び出し側が渡す
//! （daemon は `OffsetDateTime::now_utc`、試験は注入した時計）。`db_maintenance` と同じく
//! `tokio::spawn` の背景タスクで、専用の接続を `spawn_blocking` の中で使い、失敗は WARN で継続する。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use task_core::SqliteStore;
use task_core::StoreOptions;
use task_core::chat::ChatError;
use task_core::chat::attachments::{AttachmentError, ChatAttachmentLimits, ChatAttachmentStore};
use time::OffsetDateTime;

use crate::db_maintenance::RunningDbMaintenance;

/// 既定の pass の間隔（1 時間）。期限は時間・日の単位なので、これより細かく回す理由が無い。
pub const CHAT_GC_INTERVAL: Duration = Duration::from_secs(3600);

/// 時刻の出どころ。daemon は壁時計、試験は差し替えた時計。
pub type ChatGcClock = Arc<dyn Fn() -> OffsetDateTime + Send + Sync>;

/// pass を回してよいか（daemon は active の間だけ真）。
pub type ChatGcGate = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ChatGcSettings {
    pub db_path: PathBuf,
    /// `<data_dir>/chat/attachments` の `<data_dir>`。
    pub data_dir: PathBuf,
    pub limits: ChatAttachmentLimits,
    pub stream_retention: time::Duration,
    pub busy_timeout: Duration,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChatGcReport {
    pub attachments_deleted: usize,
    pub events_removed: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ChatGcError {
    #[error("chat attachments: {0}")]
    Attachments(#[from] AttachmentError),
    #[error("chat events: {0}")]
    Events(#[from] ChatError),
    #[error("chat store: {0}")]
    Store(#[from] task_core::StoreError),
}

/// pass ごとに開き直さない接続（daemon が migration 済みの DB を使う）。
pub struct ChatGc {
    store: SqliteStore,
    attachments: ChatAttachmentStore,
    stream_retention: time::Duration,
}

impl ChatGc {
    pub fn open(settings: &ChatGcSettings) -> Result<Self, ChatGcError> {
        let store = SqliteStore::open_with(
            &settings.db_path,
            StoreOptions {
                busy_timeout: settings.busy_timeout,
                ..StoreOptions::default()
            },
        )?;
        let attachments =
            ChatAttachmentStore::open(&settings.data_dir, &settings.db_path, settings.limits)?;
        Ok(Self {
            store,
            attachments,
            stream_retention: settings.stream_retention,
        })
    }

    /// 1 回の pass。添付と予約を先に回し、続けて chat_events の retention を回す。
    pub fn run_once(&self, now: OffsetDateTime) -> Result<ChatGcReport, ChatGcError> {
        let attachments_deleted = self.attachments.gc(now)?;
        let events_removed = self
            .store
            .chat_events_retention(now, self.stream_retention)?;
        Ok(ChatGcReport {
            attachments_deleted,
            events_removed,
        })
    }
}

/// daemon 用: `interval` ごと（起動直後にも 1 回。crash 後の staging を早く回収する）に pass を回す。
pub fn spawn_chat_gc_task(
    settings: ChatGcSettings,
    interval: Duration,
    clock: ChatGcClock,
    gate: ChatGcGate,
) -> RunningDbMaintenance {
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(chat_gc_loop(
        settings,
        interval,
        clock,
        gate,
        stop_rx,
        |report| {
            if report != ChatGcReport::default() {
                tracing::info!(
                    attachments_deleted = report.attachments_deleted,
                    events_removed = report.events_removed,
                    "chat gc pass"
                );
            }
        },
    ));
    RunningDbMaintenance::from_task(stop_tx, handle, "chat_gc")
}

/// 周期の本体。`on_pass` は pass が終わるたびに結果を受ける（試験はこれで出来事を待つ）。
pub(crate) async fn chat_gc_loop(
    settings: ChatGcSettings,
    interval: Duration,
    clock: ChatGcClock,
    gate: ChatGcGate,
    mut stop_rx: tokio::sync::oneshot::Receiver<()>,
    on_pass: impl Fn(ChatGcReport) + Send + 'static,
) {
    let mut gc: Option<Arc<ChatGc>> = None;
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = &mut stop_rx => break,
            _ = ticker.tick() => {
                if !gate() {
                    continue;
                }
                let opened = match gc.clone() {
                    Some(gc) => Ok(gc),
                    None => {
                        let settings = settings.clone();
                        match tokio::task::spawn_blocking(move || ChatGc::open(&settings)).await {
                            Ok(Ok(opened)) => Ok(Arc::new(opened)),
                            Ok(Err(e)) => Err(e.to_string()),
                            Err(e) => Err(e.to_string()),
                        }
                    }
                };
                let current = match opened {
                    Ok(current) => current,
                    Err(e) => {
                        tracing::warn!(error = %e, "chat gc: cannot open stores; will retry next interval");
                        continue;
                    }
                };
                gc = Some(Arc::clone(&current));
                let now = clock();
                match tokio::task::spawn_blocking(move || current.run_once(now)).await {
                    Ok(Ok(report)) => on_pass(report),
                    Ok(Err(e)) => tracing::warn!(error = %e, "chat gc failed; will retry next interval"),
                    Err(e) => tracing::warn!(error = %e, "chat gc task panicked; will retry next interval"),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "chat_gc_tests.rs"]
mod tests;
