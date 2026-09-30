//! 秘密ファイルの読み書き（ADR-0030）。1 秘密 = 1 ファイル（0600、ファイル名 = id）。**値は決して
//! ログにも応答にも出さない**（`fingerprint` は値の sha256 の先頭 8 桁で、値そのものは復元できない）。

use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// `^[A-Za-z0-9_-]{1,64}$`（プロバイダ/アカウント id と同じ規則。ADR-0030 D1）。
pub fn valid_secret_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn secret_file_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(id)
}

/// 末尾の改行（`\n` / `\r`）を落とす（ADR-0030 D1: 「値 1 行。末尾の改行は落とす」）。
pub fn trim_secret_value(text: &str) -> &str {
    text.trim_end_matches(['\n', '\r'])
}

/// 値の sha256 の先頭 8 桁（16 進）。値そのものは復元できない（ADR-0030 D3）。
pub fn fingerprint(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum SecretFileError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// `dir` を 0700 で作る（ADR-0030 D1）。celeris 本体は `Config::ensure_secrets_dir` で先に作るが、
/// そこを通らない経路（運用中にディレクトリが消えた等）でも 0755 にならないようにここでも権限を付ける。
fn create_secrets_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        match std::fs::DirBuilder::new()
            .recursive(false)
            .mode(0o700)
            .create(dir)
        {
            Ok(()) => Ok(()),
            // 親が無い場合だけ recursive に作り直す（作った親は umask のまま。`dir` 自身は 0700 にする）。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(dir)?;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

/// `dir`（無ければ作る）の下に `id` という名前で `value` を 0600・atomic temp+rename で書く（ADR-0030 D1）。
/// `dir` は呼び出し側で `valid_secret_id(id)` を確かめてから渡すこと（ここではパストラバーサルを検査しない）。
pub fn write_secret_file(dir: &Path, id: &str, value: &str) -> Result<(), SecretFileError> {
    create_secrets_dir(dir).map_err(|source| SecretFileError::Write {
        path: dir.to_path_buf(),
        source,
    })?;
    let path = secret_file_path(dir, id);
    let tmp_path = dir.join(format!(".{id}.tmp-{}", ulid::Ulid::new()));
    let result = (|| -> std::io::Result<()> {
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp_path)?
        };
        #[cfg(not(unix))]
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        file.write_all(value.as_bytes())?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(source) = result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(SecretFileError::Write {
            path: tmp_path,
            source,
        });
    }
    std::fs::rename(&tmp_path, &path).map_err(|source| SecretFileError::Write { path, source })
}

pub fn delete_secret_file(dir: &Path, id: &str) -> std::io::Result<()> {
    std::fs::remove_file(secret_file_path(dir, id))
}

/// `GET /secrets` の `items[]` の 1 要素分のメタデータ（値は含まない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMeta {
    pub id: String,
    pub updated_at: String,
    pub fingerprint: String,
}

/// `dir` 配下の秘密ファイルを id 昇順で返す。`dir` が無ければ空（`[secrets]` はあるがまだ 1 つも
/// 追加していない状態）。一時ファイル（`.` 始まり）や `valid_secret_id` を満たさない名前は飛ばす。
pub fn list_secret_files(dir: &Path) -> Result<Vec<SecretMeta>, SecretFileError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|source| SecretFileError::Read {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries.flatten() {
        let path = entry.path();
        // シンボリックリンクは追わない（リンク先の mtime や fingerprint を管理 API に出さない）。
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_file() => {}
            _ => continue,
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !valid_secret_id(name) {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|source| SecretFileError::Read {
            path: path.clone(),
            source,
        })?;
        let value = trim_secret_value(&text);
        let metadata = std::fs::metadata(&path).map_err(|source| SecretFileError::Read {
            path: path.clone(),
            source,
        })?;
        let updated_at = metadata
            .modified()
            .ok()
            .map(OffsetDateTime::from)
            .and_then(|t| t.format(&Rfc3339).ok())
            .unwrap_or_default();
        out.push(SecretMeta {
            id: name.to_string(),
            updated_at,
            fingerprint: fingerprint(value),
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

#[cfg(test)]
#[path = "secrets_tests.rs"]
mod tests;
