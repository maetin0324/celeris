//! ADR 2026-11-12-knowledge-usage-per-run D2・D3: worker run の KB の操作の台帳。
//!
//! - run の前（`dispatcher::followups` の `clear_stale_followups`）: `<artifacts_dir>/knowledge-usage.jsonl` を
//!   空にして置く（この run を計測する印）。env は followups の env と一緒に渡す（どちらも run の中の
//!   `celerisctl` の書き先）。前の run の数を今回のものと誤読しない（ADR-0098 D1 と同じ）。
//! - run の後: 台帳があれば合計（0 回を含む）を `RunMetrics.knowledge` に入れる。無ければ `None`
//!   （計測していない run）。どの run の数かは daemon が持つ run id で決める（台帳に run id は無い）。

use std::path::Path;

use task_core::KnowledgeUse;
use task_core::knowledge::usage::{self, ENV_USAGE_FILE, USAGE_FILE_NAME};

/// run の前: `measure` なら台帳を空にして置く（この run を計測する印）。`measure` でなければ前の run の台帳を
/// 消す（run の後の `read` が `None` になり、計測していない run を 0 回と数えない）。置けなければ計測しない。
pub(crate) async fn reset(artifacts_dir: &Path, measure: bool) {
    let path = artifacts_dir.join(USAGE_FILE_NAME);
    if !measure {
        let _ = tokio::fs::remove_file(&path).await;
        return;
    }
    let written = async {
        tokio::fs::create_dir_all(artifacts_dir).await?;
        tokio::fs::write(&path, b"").await
    }
    .await;
    if let Err(e) = written {
        tracing::warn!(path = %path.display(), error = %e, "knowledge usage ledger: could not prepare; this run is not measured (ADR 2026-11-12-knowledge-usage-per-run D2)");
        let _ = tokio::fs::remove_file(&path).await;
    }
}

/// 入口（`celerisctl knowledge`）に渡す env。
pub(crate) fn env(artifacts_dir: &Path) -> (String, String) {
    (
        ENV_USAGE_FILE.to_string(),
        artifacts_dir.join(USAGE_FILE_NAME).display().to_string(),
    )
}

/// worker run の後の合計。台帳は run を起こしたときの artifacts（WU の run は WU の artifacts。
/// `work_unit_checkpoint_site` と同じ規則）にある。
pub(crate) fn run_use(
    dispatcher: &crate::dispatcher::Dispatcher,
    task: &task_core::Task,
    wu: Option<&task_core::WorkUnitRow>,
) -> Option<KnowledgeUse> {
    let artifacts = match wu {
        Some(wu) => dispatcher.work_unit_checkpoint_site(task, wu).0,
        None => dispatcher
            .task_dir(task)
            .map(|dir| dispatcher.artifacts_dir(task, &dir)),
    };
    read(artifacts.as_deref())
}

/// run の後の合計。台帳が無ければ `None`（計測していない run）。
pub(crate) fn read(artifacts_dir: Option<&Path>) -> Option<KnowledgeUse> {
    let path = artifacts_dir?.join(USAGE_FILE_NAME);
    path.is_file().then(|| usage::read(&path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 守る不変条件: 前の run の数を次の run に持ち越さず、計測した run は 0 回でも `Some`、計測しない run は `None`。
    #[tokio::test]
    async fn reset_clears_the_previous_run_and_read_distinguishes_unmeasured_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let artifacts = dir.path().join("artifacts");
        assert_eq!(read(Some(&artifacts)), None);
        reset(&artifacts, true).await;
        let (key, ledger) = env(&artifacts);
        assert_eq!(key, ENV_USAGE_FILE);
        assert_eq!(read(Some(&artifacts)), Some(KnowledgeUse::default()));
        usage::append(
            std::path::Path::new(&ledger),
            task_core::KnowledgeOp::Get,
            true,
        )
        .expect("append");
        assert_eq!(read(Some(&artifacts)).map(|u| u.get), Some(1));
        reset(&artifacts, true).await;
        assert_eq!(read(Some(&artifacts)), Some(KnowledgeUse::default()));
        reset(&artifacts, false).await;
        assert_eq!(read(Some(&artifacts)), None);
        assert_eq!(read(None), None);
    }
}
