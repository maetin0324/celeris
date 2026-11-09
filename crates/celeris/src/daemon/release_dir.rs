//! 実行中の binary の release dir（`<releases>/<sha12>`）を求める daemon の共通の関数。
//!
//! 適合台帳（`browser_ledger_path`。ADR 2026-10-08-browser-prod-enablement D1.4）と同梱の検査器
//! （`repo_health_path`。ADR 2026-11-08-health-enforcement-loop D1.4）が同じ求め方を使う。各利用側は
//! release dir の下の自分の path だけを決め、この式を写さない。

use std::path::{Path, PathBuf};

/// `--release <sha12>` で動くとき、`exe`（`<releases>/<sha12>/bin/celeris`）の親の親（`<releases>/<sha12>`）。
/// `release` が無い・空（開発の起動）か、`exe` が分からなければ `None`。
pub(crate) fn release_dir_for(exe: Option<&Path>, release: Option<&str>) -> Option<PathBuf> {
    release.map(str::trim).filter(|r| !r.is_empty())?;
    Some(exe?.parent()?.parent()?.to_path_buf())
}

/// 実行中の binary の path（[`release_dir_for`] の `exe`）。読めなければ `None`。
pub(crate) fn running_exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}
