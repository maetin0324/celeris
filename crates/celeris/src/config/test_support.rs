//! `config` の子 module の unit 試験が共有する読み込み helper。

use super::{Config, ConfigError};

/// `db` の行を先に置いた `extra` を一時 dir の `config.toml` に書き、`Config::load` で読む。
pub(super) fn load(extra: &str) -> Result<Config, ConfigError> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, format!("db = \"t.sqlite3\"\n{extra}")).expect("write config");
    Config::load(&path)
}
