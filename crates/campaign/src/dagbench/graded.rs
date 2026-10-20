//! 独立 grader で成功を判定する node（`docs/ops/dag-fault-comparison.md` の「独立判定付き case」）。
//!
//! scenario の 1 node（例 `analyze`）を「実成果物を作り、評価側の grader（`celeris-campaign grade`）が通す」仕事に
//! する。成果物は固定入力から作った固定の dir（`good_artifact`）で、最初の試行だけ `first_attempt_overrides` で
//! 改竄（数値の捏造など）してある。grader は成果物を実際に検査する（成功宣言・常に true の check では通らない）。
//! - Celeris: 台本 worker が成果物を `artifact_dir` に書き、WorkUnit の `checks` に grader を置く。不合格は既存の
//!   check 不合格 → 次の run へ `previous_check_failures`（既存の repair）。直した run は `previous_check_failures`
//!   を受け取ったときの試行（2 回目）。
//! - baseline: 同じ成果物の列を同じ grader で検査し、不合格なら同じ node を最初から再実行する
//!   （[`super::baseline::Baseline::set_verifier`]）。試行上限は同じ `max_attempts`。
//!
//! 両系とも同じ入力・同じ grader・同じ試行上限・同じ成功判定を与える（baseline を弱くしない）。
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone)]
pub struct Grading {
    /// 判定付きにする scenario の node id。
    pub node: String,
    /// `celeris-campaign` の実行ファイル（`grade` サブコマンド）。
    pub grader_bin: PathBuf,
    pub case: PathBuf,
    /// 評価側が取得した scheduler の job 記録（成果物の外）。
    pub observed: Option<PathBuf>,
    /// grader の作業 dir（評価側が持つ）。
    pub work: PathBuf,
    /// 成果物の置き場（campaign・attempt ごとに別 dir）。
    pub stage_root: PathBuf,
    /// 固定入力から作った正しい成果物 dir。
    pub good_artifact: PathBuf,
    /// 最初の試行だけ上書きする (成果物内の相対 path, 内容)。空なら最初から正しい成果物。
    pub first_attempt_overrides: Vec<(String, String)>,
    /// node あたりの試行上限（最初を含む）。
    pub max_attempts: u32,
}

impl Grading {
    /// campaign の成果物 dir（再実行は同じ dir を上書きする）。
    pub fn artifact_dir(&self, campaign: &str) -> PathBuf {
        self.stage_root.join(campaign).join(&self.node)
    }

    /// `attempt`（0 始まり）の成果物を `dir` に作る。
    pub fn materialize(&self, attempt: u32, dir: &Path) -> std::io::Result<()> {
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        copy_dir(&self.good_artifact, dir)?;
        if attempt == 0 {
            for (rel, body) in &self.first_attempt_overrides {
                let p = dir.join(rel);
                if let Some(d) = p.parent() {
                    fs::create_dir_all(d)?;
                }
                fs::write(p, body)?;
            }
        }
        Ok(())
    }

    fn argv(&self, artifact: &Path) -> Vec<String> {
        let mut a = vec![
            self.grader_bin.to_string_lossy().into_owned(),
            "grade".into(),
            "--case".into(),
            self.case.to_string_lossy().into_owned(),
            "--artifact".into(),
            artifact.to_string_lossy().into_owned(),
            "--work".into(),
            self.work.to_string_lossy().into_owned(),
        ];
        if let Some(o) = &self.observed {
            a.push("--observed".into());
            a.push(o.to_string_lossy().into_owned());
        }
        a
    }

    /// WorkUnit の `checks` に置く shell 文字列（空白を含まない path 前提。測定側が作る dir だけを渡す）。
    pub fn check_cmd(&self, campaign: &str) -> String {
        self.argv(&self.artifact_dir(campaign)).join(" ")
    }

    /// grader を実際に走らせる。合格で Ok、不合格で Err（判定 JSON の stderr）。
    pub fn grade(&self, artifact: &Path) -> Result<(), String> {
        fs::create_dir_all(&self.work).map_err(|e| e.to_string())?;
        let a = self.argv(artifact);
        let out = Command::new(&a[0])
            .args(&a[1..])
            .output()
            .map_err(|e| format!("{}: {e}", a[0]))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let dst = to.join(e.file_name());
        let ft = e.file_type()?;
        if ft.is_symlink() {
            // 実 raw には dir への symlink（例 `run_0 -> server_0`）がある。そのまま作り直す。
            std::os::unix::fs::symlink(fs::read_link(e.path())?, dst)?;
        } else if ft.is_dir() {
            copy_dir(&e.path(), &dst)?;
        } else {
            fs::copy(e.path(), dst)?;
        }
    }
    Ok(())
}
