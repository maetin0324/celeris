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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Latest artifact link; immutable attempt directories are retained next to it.
    pub fn artifact_dir(&self, campaign: &str) -> PathBuf {
        self.stage_root.join(campaign).join(&self.node)
    }

    /// `attempt`（0 始まり）の成果物を `dir` に作る。
    pub fn materialize(&self, attempt: u32, dir: &Path) -> std::io::Result<()> {
        let attempts = dir.with_extension("attempts");
        fs::create_dir_all(&attempts)?;
        let saved = attempts.join(attempt.to_string());
        // Never overwrite an earlier attempt, including one interrupted by a crash.
        fs::create_dir(&saved)?;
        let payload = saved.join("artifact");
        copy_dir(&self.good_artifact, &payload)?;
        if fs::symlink_metadata(dir).is_ok() {
            if dir.is_symlink() {
                fs::remove_file(dir)?;
            } else {
                fs::remove_dir_all(dir)?;
            }
        }
        std::os::unix::fs::symlink(fs::canonicalize(&payload)?, dir)?;
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

    /// Content pins, independent of arm-specific output paths.
    pub fn pin(&self) -> Result<serde_json::Value, String> {
        use sha2::{Digest, Sha256};
        fn hash(p: &Path) -> Result<String, String> {
            Ok(format!(
                "{:x}",
                Sha256::digest(fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?)
            ))
        }
        fn tree(
            root: &Path,
            at: &Path,
            out: &mut std::collections::BTreeMap<String, String>,
        ) -> Result<(), String> {
            for entry in fs::read_dir(at).map_err(|e| e.to_string())? {
                let p = entry.map_err(|e| e.to_string())?.path();
                if p.is_symlink() {
                    continue;
                }
                if p.is_dir() {
                    tree(root, &p, out)?;
                } else {
                    out.insert(
                        p.strip_prefix(root).unwrap().to_string_lossy().into_owned(),
                        hash(&p)?,
                    );
                }
            }
            Ok(())
        }
        let mut inputs = std::collections::BTreeMap::new();
        tree(&self.good_artifact, &self.good_artifact, &mut inputs)?;
        Ok(
            serde_json::json!({"contract": "validated-attempts/1", "node": self.node,
            "case_sha256": hash(&self.case)?, "case": self.case,
            "grader_version": env!("CARGO_PKG_VERSION"),
            "grader_sha256": hash(&self.grader_bin)?, "inputs": inputs,
            "observed_sha256": self.observed.as_ref().map(|p| hash(p)).transpose()?,
            "first_attempt_overrides": self.first_attempt_overrides, "max_attempts": self.max_attempts}),
        )
    }

    pub fn attach_baseline(&self, baseline: &mut super::baseline::Baseline) {
        baseline.set_graded_node(self.node.clone());
        let g = self.clone();
        baseline.set_verifier(
            Box::new(move |id, attempt| {
                let (campaign, node) = id.split_once('/').unwrap_or(("c0", id));
                if node != g.node {
                    return Ok(());
                }
                let dir = g.artifact_dir(campaign);
                g.materialize(attempt, &dir).map_err(|e| e.to_string())?;
                g.grade(&dir)
            }),
            self.max_attempts,
        );
    }

    /// WorkUnit の `checks` に置く shell 文字列（各引数を shell quote する）。
    pub fn check_cmd(&self, campaign: &str) -> String {
        self.argv(&self.artifact_dir(campaign))
            .iter()
            .map(|a| format!("'{}'", a.replace('\'', "'\"'\"'")))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// grader を実際に走らせる。合格で Ok、不合格で Err（exit 1 の stdout にある判定 JSON）、
    /// 判定を作れないエラー（exit 2 など）は Err（stderr の理由）。
    pub fn grade(&self, artifact: &Path) -> Result<(), String> {
        fs::create_dir_all(&self.work).map_err(|e| e.to_string())?;
        let a = self.argv(artifact);
        let out = Command::new(&a[0])
            .args(&a[1..])
            .output()
            .map_err(|e| format!("{}: {e}", a[0]))?;
        let record = serde_json::json!({
            "schema": "celeris.dagbench-grade/1",
            "passed": out.status.success(), "exit": out.status.code(),
            "stdout": String::from_utf8_lossy(&out.stdout),
            "stderr": String::from_utf8_lossy(&out.stderr),
        });
        // The first independent verdict remains attached to this attempt.
        let canonical = fs::canonicalize(artifact).map_err(|e| e.to_string())?;
        let path = if canonical.file_name().is_some_and(|n| n == "artifact") {
            canonical
                .parent()
                .expect("attempt parent")
                .join("grade.json")
        } else {
            canonical.with_extension("grade.json")
        };
        if !path.exists() {
            fs::write(
                &path,
                serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
        match out.status.code() {
            Some(0) => Ok(()),
            Some(1) => Err(String::from_utf8_lossy(&out.stdout).into_owned()),
            _ => Err(String::from_utf8_lossy(&out.stderr).into_owned()),
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
