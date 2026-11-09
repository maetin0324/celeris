//! ADR 2026-11-08-health-enforcement-loop D1.4 の daemon 側: Celeris の release に同梱した repo health の検査器
//! （`<release>/repo-health/`。release.sh の `sd_bundle_repo_health` が書く）を、起動時に 1 回
//! `task_dispatch::health_review::gate::configure_bundled_checker` で process に設定する。
//!
//! release dir は `super::release_dir`（適合台帳と同じ求め方）で求める。所在の上書きは置かない（同梱の検査器は
//! 実行中の release の物だけ）。`CHECKER_TREE`（`repo-health <tree id>`・`source-size-report <blob id>`）の読み取りと
//! task_core の `checker_sha256` による版の計算は gate の `read_bundled_checker` が持ち、ここでは写さない。
//! 同梱が無い・読めないときは設定しない（同梱なし）。base に検査器の無い repo は適用判定で `checker_missing` の
//! `Unavailable` になり、合格にはならない。

use std::path::Path;

use task_dispatch::health_review::gate::{self, BundledChecker};

/// release dir の下の同梱の検査器の dir（release.sh の `$STAGE/repo-health`）。
pub(crate) const BUNDLE_DIR: &str = "repo-health";

/// 同梱の検査器を使わない理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoBundle {
    /// `--release` で起動していない（開発の起動）か、実行中の binary の path が分からない。
    NoReleaseDir,
    /// release dir はあるが、同梱の検査器を読めない（`repo-health/` か `repo_health.py` が無い、`CHECKER_TREE` が
    /// 無い・壊れている）。gate の理由の文を持つ。
    Unreadable(String),
}

impl std::fmt::Display for NoBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoReleaseDir => f.write_str("release の指定が無い（--release で起動していない）"),
            Self::Unreadable(why) => f.write_str(why),
        }
    }
}

/// `exe`（実行中の binary）と `release`（`--release` の値）から同梱の検査器を読む。process の設定には触れない。
/// 返す [`BundledChecker::id`] は `CheckerSource::Bundled` で、`sha256` は base の版と同じ式（`checker_sha256`）の値。
pub(crate) fn bundled_checker_for(
    exe: Option<&Path>,
    release: Option<&str>,
) -> Result<BundledChecker, NoBundle> {
    let release_dir =
        super::release_dir::release_dir_for(exe, release).ok_or(NoBundle::NoReleaseDir)?;
    gate::read_bundled_checker(&release_dir.join(BUNDLE_DIR)).map_err(NoBundle::Unreadable)
}

/// 起動時に 1 回呼ぶ。`cli_release` は `--release` の値。同梱が無ければ設定せず、理由を log に出す。
pub(crate) fn configure_from_process(cli_release: Option<&str>) {
    let exe = super::release_dir::running_exe();
    let found = match bundled_checker_for(exe.as_deref(), cli_release) {
        Ok(found) => found,
        Err(why @ NoBundle::NoReleaseDir) => {
            tracing::info!(
                reason = %why,
                "repo health: no bundled checker; repos without the checker at base are checker_missing"
            );
            return;
        }
        Err(why @ NoBundle::Unreadable(_)) => {
            tracing::warn!(
                reason = %why,
                "repo health: bundled checker unreadable; repos without the checker at base are checker_missing"
            );
            return;
        }
    };
    match gate::configure_bundled_checker(&found.dir) {
        Ok(set) => tracing::info!(
            dir = %set.dir.display(),
            checker_sha256 = %set.id.sha256,
            "repo health: bundled checker"
        ),
        Err(why) => tracing::warn!(
            reason = %why,
            "repo health: bundled checker not configured; repos without the checker at base are checker_missing"
        ),
    }
}

#[cfg(test)]
#[path = "repo_health_path_tests.rs"]
mod tests;
