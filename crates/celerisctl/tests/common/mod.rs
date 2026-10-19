//! 試験が起動する実 `celerisctl` の解決。
//!
//! `env!("CARGO_BIN_EXE_celerisctl")` は試験 binary のコンパイル時に埋め込まれる絶対 path で、再利用 cache（seed）
//! 由来の試験 binary では古い別 target を指しうる。nextest は実行時に現在 build した binary の path を同名の環境変数で
//! 渡すので、それを優先し、無い（通常の `cargo test`）ときだけ埋込み値を使う。PATH や別 target は探さない。
#![allow(dead_code)]

use std::ffi::OsString;
use std::path::PathBuf;

const ENV_NAME: &str = "CARGO_BIN_EXE_celerisctl";

/// 実行時 mapping（あれば）を優先して CLI の path を決める。指す先が実在する file でなければ理由付きで失敗する。
pub fn resolve_cli(runtime: Option<OsString>, embedded: &str) -> Result<PathBuf, String> {
    let (source, path) = match runtime {
        Some(v) if v.is_empty() => {
            return Err(format!("{ENV_NAME} (runtime mapping) is set but empty"));
        }
        Some(v) => ("runtime mapping", PathBuf::from(v)),
        None => ("embedded at compile time", PathBuf::from(embedded)),
    };
    if !path.is_absolute() {
        return Err(format!(
            "{ENV_NAME} ({source}) is not an absolute path: {}",
            path.display()
        ));
    }
    if !path.is_file() {
        return Err(format!(
            "{ENV_NAME} ({source}) does not point to an existing file: {}",
            path.display()
        ));
    }
    Ok(path)
}

/// 現在 build された実 `celerisctl` の path。解決できなければ panic（誤った binary へ黙って落とさない）。
pub fn celerisctl_path() -> PathBuf {
    resolve_cli(std::env::var_os(ENV_NAME), env!("CARGO_BIN_EXE_celerisctl"))
        .unwrap_or_else(|e| panic!("cannot locate celerisctl: {e}"))
}
