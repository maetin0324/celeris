//! ADR 2026-11-12-knowledge-usage-per-run D2・D3: worker run の KB の操作の台帳。
//!
//! - run の前: `<artifacts_dir>/knowledge-usage.jsonl` を空にして置き（この run を計測する印）、入口
//!   （`celerisctl knowledge`）に渡す env を返す。前の run の数を今回のものと誤読しない（ADR-0098 D1 と同じ）。
//! - run の後: 台帳があれば合計（0 回を含む）を `RunMetrics.knowledge` に入れる。無ければ `None`
//!   （計測していない run）。どの run の数かは daemon が持つ run id で決める（台帳に run id は無い）。

use std::path::Path;

use task_core::KnowledgeUse;
use task_core::knowledge::usage::{self, ENV_USAGE_FILE, USAGE_FILE_NAME};

/// run の前に台帳を空にし、渡す env を返す。置けなければ計測しない（run は止めない）。
pub(super) async fn prepare(artifacts_dir: &Path) -> Option<Vec<(String, String)>> {
    let path = artifacts_dir.join(USAGE_FILE_NAME);
    let written = async {
        tokio::fs::create_dir_all(artifacts_dir).await?;
        tokio::fs::write(&path, b"").await
    }
    .await;
    if let Err(e) = written {
        tracing::warn!(path = %path.display(), error = %e, "knowledge usage ledger: could not prepare; this run is not measured (ADR 2026-11-12-knowledge-usage-per-run D2)");
        return None;
    }
    Some(vec![(
        ENV_USAGE_FILE.to_string(),
        path.display().to_string(),
    )])
}

impl super::Dispatcher {
    /// worker run の後の合計（WU の run は WU の artifacts、それ以外は task の artifacts。run を起こした
    /// ときの置き場と同じ規則の `work_unit_checkpoint_site` を使う）。
    pub(super) fn run_knowledge_use(
        &self,
        task: &task_core::Task,
        wu: Option<&task_core::WorkUnitRow>,
    ) -> Option<KnowledgeUse> {
        let artifacts = match wu {
            Some(wu) => self.work_unit_checkpoint_site(task, wu).0,
            None => self.task_dir(task).map(|d| self.artifacts_dir(task, &d)),
        };
        read(artifacts.as_deref())
    }
}

/// run の後の合計。台帳が無ければ `None`。
pub(super) fn read(artifacts_dir: Option<&Path>) -> Option<KnowledgeUse> {
    let path = artifacts_dir?.join(USAGE_FILE_NAME);
    path.is_file().then(|| usage::read(&path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 守る不変条件: 前の run の数を次の run に持ち越さず、計測した run は 0 回でも `Some`。
    #[tokio::test]
    async fn prepare_resets_the_ledger_and_read_distinguishes_unmeasured_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let artifacts = dir.path().join("artifacts");
        assert_eq!(read(Some(&artifacts)), None);
        let env = prepare(&artifacts).await.expect("env");
        assert_eq!(env[0].0, ENV_USAGE_FILE);
        let ledger = std::path::PathBuf::from(&env[0].1);
        assert_eq!(read(Some(&artifacts)), Some(KnowledgeUse::default()));
        usage::append(&ledger, task_core::KnowledgeOp::Get, true).expect("append");
        assert_eq!(read(Some(&artifacts)).map(|u| u.get), Some(1));
        prepare(&artifacts).await.expect("env");
        assert_eq!(read(Some(&artifacts)), Some(KnowledgeUse::default()));
        assert_eq!(read(None), None);
    }
}
