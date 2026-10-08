//! ADR 2026-10-08-browser-prod-enablement D1.4: daemon が起動時に適合台帳の path を決め、
//! `task_worker::browser_ledger::configure_conformance` で worker に渡す（`std::env::set_var` は使わない）。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 試験・開発用の上書き（worker の `conformance_record_path` も同じ名前を先に見る）。
pub(crate) const CONFORMANCE_FILE_ENV: &str = "CELERIS_BROWSER_CONFORMANCE_FILE";

/// 台帳の path を決める。順は (1) env `CELERIS_BROWSER_CONFORMANCE_FILE`（空は無い扱い）
/// (2) `--release <sha12>` で動くとき `current_exe()` の親の親（release dir）の `browser/conformance.json`
/// (3) どちらも無ければ `None`（未配置。daemon は起動し、gate が browser task だけを止める）。
pub(crate) fn conformance_path_for(
    env: Option<OsString>,
    exe: Option<&Path>,
    release: Option<&str>,
) -> Option<PathBuf> {
    if let Some(value) = env.filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(value));
    }
    release.map(str::trim).filter(|r| !r.is_empty())?;
    // `<releases>/<sha12>/bin/celeris` → `<releases>/<sha12>`
    let release_dir = exe?.parent()?.parent()?;
    Some(release_dir.join("browser").join("conformance.json"))
}

/// 起動時に 1 回呼ぶ。`cli_release` は `--release` の値（`CELERIS_RELEASE` env ではない。D1.4 の 2）。
pub(crate) fn configure_from_process(cli_release: Option<&str>) {
    let release = cli_release
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_string);
    let exe = std::env::current_exe().ok();
    match conformance_path_for(
        std::env::var_os(CONFORMANCE_FILE_ENV),
        exe.as_deref(),
        release.as_deref(),
    ) {
        Some(path) => {
            tracing::info!(path = %path.display(), release = ?release, "browser conformance ledger path");
            task_worker::browser_ledger::configure_conformance(path, release);
        }
        None => {
            tracing::info!(
                "browser conformance ledger: not placed (no {CONFORMANCE_FILE_ENV}, no --release); \
                 browser tasks are held by the ledger gate"
            );
        }
    }
}

#[cfg(test)]
#[path = "browser_ledger_path_tests.rs"]
mod tests;
