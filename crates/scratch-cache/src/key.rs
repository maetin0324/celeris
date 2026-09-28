//! key の検査とパス。sccache の key は 64 桁の 16 進（sha256）で、webdav の path の最後の要素（U5）。

/// sccache の server が起動時に読み書きする storage check の object（U5）。L2 には流さない。
pub const CHECK_KEY: &str = ".sccache_check";

/// key の長さの上限。
pub const MAX_KEY_LEN: usize = 200;

/// `[0-9A-Za-z_-]` だけで 1〜`MAX_KEY_LEN` 文字。
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_KEY_LEN
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// シャードのディレクトリ名（先頭 2 文字。`valid_key` の key は ASCII）。
pub fn shard(key: &str) -> &str {
    &key[..key.len().min(2)]
}
