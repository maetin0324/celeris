//! ADR 2026-10-08-cos-workspace-files-in-chat D1: files the CoS wrote in its thread workspace
//! become attachments of its reply.
//!
//! 決定的（LLM も CoS の申告も使わない）:
//! - run の起動直前に workspace を走査（`snapshot`）し、終端の直前にもう一度走査する。
//!   新しく現れたか、大きさ・mtime が変わった通常 file が候補。
//! - 返事の本文が触れた path（`…/workspace/<rel>` の絶対 path、`<rel>`・`./<rel>` の相対 path）の file も候補。
//!   本文の順が先、差分は path 順で後。
//! - 走査は dot で始まる名前（`.taskd/` など run の内部物）と最上位の `attachments/`（人の添付の写し）を除き、
//!   adapter の run 記録 `runs/` も除き、symlink を辿らず、深さ `MAX_DEPTH`・`MAX_ENTRIES` 項目で打ち切る。
//! - 保存は既存の `ChatAttachmentStore::upload`（hash・MIME・権限は D4 と同じ）。上限を超える file は付けない。

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use task_core::SqliteStore;
use task_core::chat::attachments::{ChatAttachmentLimits, ChatAttachmentStore};
use task_core::chat::{ChatMessage, ChatMessageQuery, ChatWorkspaceFile};
use time::OffsetDateTime;

/// 走査する深さ（workspace 直下が 1）。
pub(crate) const MAX_DEPTH: usize = 6;
/// 走査で見る項目（file と dir）の上限。
pub(crate) const MAX_ENTRIES: usize = 4096;

/// 最上位で除く dir: 人の添付を stage する `attachments/`（`attachments.rs`）と、adapter が run ごとの
/// 記録（request.json・stdout.jsonl・prompt.txt）を書く `runs/`。どちらも CoS の作った文書ではない。
const EXCLUDED_TOP: [&str; 2] = ["attachments", "runs"];

/// workspace の通常 file の `(相対 path → (大きさ, mtime))`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WorkspaceSnapshot {
    files: BTreeMap<String, (u64, Option<SystemTime>)>,
}

/// 起動時に覚えて終端で使う、添付化に要るもの一式。
#[derive(Debug, Clone)]
pub(crate) struct WorkspaceCapture {
    pub workspace: PathBuf,
    pub before: WorkspaceSnapshot,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub limits: ChatAttachmentLimits,
}

pub(crate) fn snapshot(workspace: &Path) -> WorkspaceSnapshot {
    let mut snap = WorkspaceSnapshot::default();
    let mut seen = 0usize;
    walk(workspace, "", 1, &mut seen, &mut snap);
    snap
}

fn walk(dir: &Path, prefix: &str, depth: usize, seen: &mut usize, snap: &mut WorkspaceSnapshot) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok().map(|n| (n, e.path())))
        .collect();
    names.sort();
    for (name, path) in names {
        if *seen >= MAX_ENTRIES {
            return;
        }
        *seen += 1;
        if name.starts_with('.') || (depth == 1 && EXCLUDED_TOP.contains(&name.as_str())) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let rel = format!("{prefix}{name}");
        if meta.file_type().is_dir() {
            walk(&path, &format!("{rel}/"), depth + 1, seen, snap);
        } else if meta.file_type().is_file() {
            snap.files.insert(rel, (meta.len(), meta.modified().ok()));
        }
    }
}

/// path の一部になり得る文字（境界の判定に使う）。ASCII だけ: 日本語の本文は path に句読点なしで続く。
fn path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '~' | '+' | '@' | '%')
}

/// 本文で `rel` に最初に触れた位置。前は path の外か `…/workspace/`・`./`、後ろは path の外
/// （文末の `.` などは外とみなす）。
pub(crate) fn first_mention(text: &str, rel: &str) -> Option<usize> {
    for (at, _) in text.match_indices(rel) {
        let before = &text[..at];
        let start_ok = match before.chars().next_back() {
            None => true,
            Some('/') => {
                before.ends_with("/workspace/")
                    || before.ends_with("./") && {
                        let b = &before[..before.len() - 2];
                        b.chars().next_back().is_none_or(|c| !path_char(c))
                    }
            }
            Some(c) => !path_char(c),
        };
        if !start_ok {
            continue;
        }
        let after = &text[at + rel.len()..];
        let mut rest = after.chars();
        let end_ok = match rest.next() {
            None => true,
            Some('.') => rest.next().is_none_or(|c| !path_char(c) || c == '.'),
            Some(c) => !path_char(c),
        };
        if end_ok {
            return Some(at);
        }
    }
    None
}

/// 添付にする相対 path（本文で触れた順 → 差分の path 順。重複なし）。
pub(crate) fn candidates(
    before: &WorkspaceSnapshot,
    after: &WorkspaceSnapshot,
    text: &str,
) -> Vec<String> {
    let mut mentioned: Vec<(usize, &String)> = after
        .files
        .keys()
        .filter_map(|rel| first_mention(text, rel).map(|at| (at, rel)))
        .collect();
    // 同じ位置なら長い path（`a/b.md` と `b.md` の両方に当たるとき）を先にする。
    mentioned.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.len().cmp(&a.1.len())));
    let mut out: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for (_, rel) in mentioned {
        if seen.insert(rel.clone()) {
            out.push(rel.clone());
        }
    }
    for (rel, meta) in &after.files {
        if before.files.get(rel) != Some(meta) && seen.insert(rel.clone()) {
            out.push(rel.clone());
        }
    }
    out
}

/// 通常 file だけを開く。開いた file と path の lstat が同じ inode であることも確かめる
/// （開く前後で symlink に差し替わっていない）。
fn open_regular(path: &Path) -> std::io::Result<File> {
    let link = fs::symlink_metadata(path)?;
    if !link.file_type().is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.ino() != link.ino() || meta.dev() != link.dev() {
        return Err(std::io::Error::other("file changed while opening"));
    }
    Ok(file)
}

/// 候補を保存して出力 message に pin する。付けた file を返す（無ければ何も書かない）。
pub(crate) fn attach(
    store: &SqliteStore,
    capture: &WorkspaceCapture,
    thread_id: &str,
    message_id: &str,
    run_id: &str,
    text: &str,
    now: OffsetDateTime,
) -> Result<Vec<ChatWorkspaceFile>, String> {
    let after = snapshot(&capture.workspace);
    let rels = candidates(&capture.before, &after, text);
    if rels.is_empty() {
        return Ok(Vec::new());
    }
    let attachments =
        ChatAttachmentStore::open(&capture.data_dir, &capture.db_path, capture.limits)
            .map_err(|e| format!("attachment store unavailable: {e}"))?;
    let limits = capture.limits;
    let mut total = 0u64;
    let mut files = Vec::new();
    for rel in rels {
        if files.len() >= limits.max_files_per_message {
            break;
        }
        let Some(&(size, _)) = after.files.get(&rel) else {
            continue;
        };
        if size > limits.max_file_bytes || total.saturating_add(size) > limits.max_message_bytes {
            tracing::debug!(%rel, size, "CoS workspace file over the attachment limit; not attached");
            continue;
        }
        // 途中の dir が symlink に差し替わっていないか（走査の後の変化）も見る。
        let path = capture.workspace.join(&rel);
        if super::attachments::reject_symlinks(&path).is_err() {
            continue;
        }
        let file = match open_regular(&path) {
            Ok(file) => file,
            Err(error) => {
                tracing::debug!(%rel, %error, "CoS workspace file unreadable; not attached");
                continue;
            }
        };
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let key = format!("cos-workspace:{run_id}:{rel}");
        match attachments.upload(thread_id, &key, &name, Some(size), file, now) {
            Ok(row) => {
                total = total.saturating_add(row.size_bytes);
                files.push(ChatWorkspaceFile {
                    path: rel,
                    attachment_id: row.id,
                });
            }
            Err(error) => {
                tracing::debug!(%rel, %error, "CoS workspace file not attached");
            }
        }
    }
    if files.is_empty() {
        return Ok(files);
    }
    store
        .chat_message_attach_workspace_files(thread_id, message_id, &files, now)
        .map_err(|e| format!("attach workspace files: {e}"))?;
    Ok(files)
}

/// 出力 message の今の本文（終端で `final_text` が無いときに使う）。
pub(crate) fn message_text(
    store: &SqliteStore,
    thread_id: &str,
    message_id: &str,
) -> Option<String> {
    let mut before = None;
    loop {
        let page = store
            .chat_message_list(
                thread_id,
                &ChatMessageQuery {
                    before_seq: before,
                    limit: Some(200),
                    ..ChatMessageQuery::default()
                },
            )
            .ok()?;
        if let Some(message) = page
            .items
            .into_iter()
            .find(|m: &ChatMessage| m.id == message_id)
        {
            return Some(message.text);
        }
        before = page.next_before_seq;
        before?;
    }
}

#[cfg(test)]
#[path = "workspace_files_tests.rs"]
mod tests;
