//! prepare 時の入力と worker の写しを照合する。DB・ネットワークは使わない。

use super::{content_hash, known_task_ids_from_inbox_json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const SNAPSHOT_FILE: &str = "curation-inputs.json";

/// daemon はこれを作業場所外の状態にも保存する。worker は生成・更新しない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSnapshot {
    version: u32,
    kb_hashes: BTreeMap<String, String>,
    inbox_hash: String,
    pub inbox_task_ids: BTreeSet<String>,
}

fn file_hashes(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(format!(
            "入力 KB の directory が不正です: {}",
            dir.display()
        ));
    }
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            file_hashes(root, &path, out)?;
        } else if kind.is_file() {
            let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
            let name = relative
                .to_str()
                .ok_or("入力 KB の path が UTF-8 ではありません")?;
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            out.insert(name.to_string(), format!("{:x}", Sha256::digest(&bytes)));
        } else {
            return Err(format!(
                "入力 KB に symlink または特殊ファイルがあります: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn read_regular(path: &Path) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("{} を読めない: {e}", path.display()))?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(format!(
            "入力は通常ファイルである必要があります: {}",
            path.display()
        ));
    }
    std::fs::read_to_string(path).map_err(|e| format!("{} を読めない: {e}", path.display()))
}

impl InputSnapshot {
    /// daemon の prepare 専用。検証時は snapshot を作り直さない。
    pub fn capture(kb: &Path, inbox: &Path) -> Result<Self, String> {
        let mut kb_hashes = BTreeMap::new();
        file_hashes(kb, kb, &mut kb_hashes)?;
        let inbox_raw = read_regular(inbox)?;
        Ok(Self {
            version: 1,
            kb_hashes,
            inbox_hash: content_hash(&inbox_raw),
            inbox_task_ids: known_task_ids_from_inbox_json(&inbox_raw)?,
        })
    }

    pub fn verify(&self, kb: &Path, inbox: &Path) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!(
                "未対応の curation input snapshot version: {}",
                self.version
            ));
        }
        let actual = Self::capture(kb, inbox)?;
        if self.kb_hashes != actual.kb_hashes {
            return Err("inputs/kb が prepare 時の snapshot と一致しません。入力を復元し、変更後の本文は計画の content に書いてください".into());
        }
        if self.inbox_hash != actual.inbox_hash || self.inbox_task_ids != actual.inbox_task_ids {
            return Err("inputs/inbox.json が prepare 時の snapshot と一致しません".into());
        }
        Ok(())
    }
}

/// snapshot は必須。欠落・旧 run では可変の写しだけで成功にしない。
pub fn verify_inputs(kb: &Path, inbox: &Path, snapshot: &Path) -> Result<InputSnapshot, String> {
    let raw = read_regular(snapshot).map_err(|e| {
        format!("curation input snapshot: {e}（daemon による入力の再準備が必要です）")
    })?;
    let snapshot: InputSnapshot = serde_json::from_str(&raw)
        .map_err(|e| format!("curation input snapshot の形が違う: {e}"))?;
    snapshot.verify(kb, inbox)?;
    Ok(snapshot)
}
