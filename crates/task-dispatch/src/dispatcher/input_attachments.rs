//! ADR 2026-10-05 cos-chat-home D4: task に pin されたチャット添付を、その task の worker run
//! （atomic task の run と、その task の WorkUnit の run）の開始時に参照から stage する。
//!
//! - 読むのは `chat_attachment_refs` の owner_kind=`task`・owner_id=その task だけ。子孫 task には継がない
//!   （pin は「この task の入力」という明示の引渡しで、子は親の planner が objective に書いた範囲で
//!   動く。要る子には CoS が子 task へ pin する。継ぐと範囲外の入力が黙って広がる）。
//! - planner・reviewer の run には stage しない（`spawn_worker` が planner を除き、reviewer は
//!   `review.rs` の別経路）。stage の対象は実際に作業する run だけ。
//! - 元チャットの run path や CoS の thread workspace は使わない。原本は
//!   `<data_dir>/chat/attachments/<id>/blob` で、`read_verified` が DB の hash・size を照合する。
//! - stage 先は作業ツリーの外（`<task_dir>/attachments/<id>/<name>`、WU なら
//!   `<task_dir>/wu/<key>/attachments/…`。`repos/` と `artifacts/` の兄弟）。dir 0500・file 0400。
//! - 照合できない添付は渡さず、manifest に `delivery = unavailable` と理由で載せる。

use std::fs::{self, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use task_core::chat::attachments::{ChatAttachmentLimits, ChatAttachmentStore, StoredAttachment};
use task_worker::protocol::{InputAttachment, InputAttachmentDelivery};

use super::cos_chat::attachments::{
    StageError, private_dir, reject_symlinks, safe_filename, safe_id, verify_staged,
};

/// 添付の原本の場所（daemon の `[cos]` から決まる。API の引数ではない）。
#[derive(Debug, Clone)]
pub(crate) struct InputAttachmentSource {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub limits: ChatAttachmentLimits,
}

impl super::Dispatcher {
    /// 原本の場所は CoS chat の設定（`set_cos_chat_launch`）と同じ data dir・DB。CoS chat を
    /// 配線していない daemon には添付が存在しないので `None`。
    pub(super) fn input_attachment_source(&self) -> Option<InputAttachmentSource> {
        self.cos_chat_launch
            .as_ref()
            .map(|chat| InputAttachmentSource {
                data_dir: chat.config.data_dir.clone(),
                db_path: chat.config.db_path.clone(),
                limits: chat.config.attachment_limits,
            })
    }
}

/// stage 先の親（`repos/` と `artifacts/` の兄弟）。WU の run は WU ごとの成果物の置き場の親、
/// それ以外は task の作業 dir。
pub(crate) fn stage_base(task_dir: &Path, artifacts_dir_override: Option<&Path>) -> PathBuf {
    artifacts_dir_override
        .and_then(Path::parent)
        .map_or_else(|| task_dir.to_path_buf(), Path::to_path_buf)
}

fn unavailable(row: StoredAttachment, reason: impl Into<String>) -> InputAttachment {
    InputAttachment {
        id: row.id,
        name: row.original_name,
        media_type: row.media_type,
        size_bytes: row.size_bytes,
        sha256: row.sha256,
        path: None,
        delivery: InputAttachmentDelivery::Unavailable,
        reason: Some(reason.into()),
    }
}

/// task に pin された ready の添付を読み、`base/attachments/` へ照合つきで stage して manifest を返す。
/// pin が無ければ何も作らず空。`remote` の run は作業場所が別 host なので stage せず unavailable。
/// store を開けない・pin を読めないときは `Err`（呼び出し側は run を落とさず警告に留める）。
pub(crate) fn stage_task_input_attachments(
    source: &InputAttachmentSource,
    task_id: &str,
    base: &Path,
    remote: bool,
) -> Result<Vec<InputAttachment>, String> {
    let store = ChatAttachmentStore::open(&source.data_dir, &source.db_path, source.limits)
        .map_err(|e| format!("attachment store unavailable: {e}"))?;
    let rows = store
        .list_for_owner("task", task_id)
        .map_err(|e| format!("pinned attachments unreadable: {e}"))?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    if remote {
        return Ok(rows
            .into_iter()
            .map(|row| {
                unavailable(
                    row,
                    "remote workspace: pinned attachments are staged only for local runs",
                )
            })
            .collect());
    }
    let root = base.join("attachments");
    let mut manifest = Vec::with_capacity(rows.len());
    for row in rows {
        manifest.push(match stage_one(&store, &root, &row) {
            Ok(path) => InputAttachment {
                delivery: InputAttachmentDelivery::for_media_type(&row.media_type),
                id: row.id,
                name: row.original_name,
                media_type: row.media_type,
                size_bytes: row.size_bytes,
                sha256: row.sha256,
                path: Some(path),
                reason: None,
            },
            Err(error) => {
                let reason = error.to_string();
                unavailable(row, reason)
            }
        });
    }
    // 親も書けなくする（worker が別の file を紛れ込ませない）。次の run は stage_one の前に戻す。
    if root.is_dir() {
        let _ = fs::set_permissions(&root, fs::Permissions::from_mode(0o500));
    }
    Ok(manifest)
}

fn stage_one(
    store: &ChatAttachmentStore,
    root: &Path,
    row: &StoredAttachment,
) -> Result<PathBuf, StageError> {
    if !safe_id(&row.id) {
        return Err(StageError::UnsafePath);
    }
    let filename = safe_filename(&row.original_name)?;
    reject_symlinks(root)?;
    if root.is_dir() {
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    private_dir(root)?;
    let directory = root.join(&row.id);
    private_dir(&directory)?;
    let path = directory.join(&filename);
    reject_symlinks(&path)?;
    // 原本を先に照合する（不一致・欠落なら古い写しも残さない）。
    let mut file = match store.read_verified(&row.id) {
        Ok(file) => file,
        Err(source) => {
            let _ = fs::remove_file(&path);
            let _ = fs::set_permissions(&directory, fs::Permissions::from_mode(0o500));
            return Err(StageError::Attachment {
                id: row.id.clone(),
                source,
            });
        }
    };
    if fs::symlink_metadata(&path).is_ok() {
        // 前の run の写しは、原本の記録と一致するときだけ使い回す。違えば作り直す。
        let reuse = fs::symlink_metadata(&path)?.is_file()
            && verify_staged(&path, &row.id, row.size_bytes, &row.sha256).is_ok();
        if !reuse {
            fs::remove_file(&path)?;
        }
    }
    if fs::symlink_metadata(&path).is_err() {
        let result = (|| -> Result<(), StageError> {
            let io = |source| StageError::Io {
                id: row.id.clone(),
                source,
            };
            let mut target = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(io)?;
            std::io::copy(&mut file, &mut target).map_err(io)?;
            target.sync_all().map_err(io)?;
            verify_staged(&path, &row.id, row.size_bytes, &row.sha256)
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&path);
            let _ = fs::set_permissions(&directory, fs::Permissions::from_mode(0o500));
            return Err(error);
        }
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))?;
    Ok(path)
}
