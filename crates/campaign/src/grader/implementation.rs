//! 検査 (1) 実装テスト: pristine な評価側テストを成果物 repo に重ねて実行する。
//! 成果物側の評価テストの削除・改変（sha256 不一致）は失格。改変時は弱体化の印も数える。
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use super::{CheckResult, GradeOptions, case::ImplementationSpec, safe_join, sha256_file, tail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// 評価 command の実行器。本番は [`ProcessRunner`]、試験は stub を注入する。
pub trait CommandRunner {
    fn run(&self, argv: &[String], cwd: &Path) -> Result<CommandOutput, String>;
}

pub struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run(&self, argv: &[String], cwd: &Path) -> Result<CommandOutput, String> {
        let (prog, rest) = argv.split_first().ok_or("empty command")?;
        let out = Command::new(prog)
            .args(rest)
            .current_dir(cwd)
            .output()
            .map_err(|e| format!("spawn {prog}: {e}"))?;
        Ok(CommandOutput {
            exit: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// 試験 file の弱体化の印。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TestMarks {
    pub tests: usize,
    pub asserts: usize,
    pub ignores: usize,
    pub skips: usize,
}

pub fn marks(text: &str) -> TestMarks {
    let mut m = TestMarks::default();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        m.tests += t.matches("#[test]").count() + t.matches("#[tokio::test").count();
        m.asserts += t.matches("assert").count();
        m.ignores += t.matches("#[ignore").count();
        m.skips += [
            "#[cfg(any())]",
            "#[cfg(FALSE)]",
            "#![cfg(any())]",
            "@pytest.mark.skip",
            "pytest.skip(",
        ]
        .iter()
        .map(|k| t.matches(k).count())
        .sum::<usize>();
    }
    m
}

fn marks_json(m: TestMarks) -> Value {
    json!({"tests": m.tests, "asserts": m.asserts, "ignores": m.ignores, "skips": m.skips})
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    for e in fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let name = e.file_name();
        if name == ".git" || name == "target" {
            continue;
        }
        let ft = e.file_type().map_err(|e| e.to_string())?;
        let to = dst.join(&name);
        if ft.is_dir() {
            copy_tree(&e.path(), &to)?;
        } else if ft.is_file() {
            fs::copy(e.path(), &to).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        // symlink は写さない（成果物外の参照を作業 dir に持ち込まない）。
    }
    Ok(())
}

fn work_dir(root: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    root.join(format!("celeris-grade-{}-{nanos}", std::process::id()))
}

pub fn check(spec: &ImplementationSpec, artifact: &Path, opts: &GradeOptions) -> CheckResult {
    let mut reasons = Vec::new();
    let mut files = Vec::new();
    let repo = match safe_join(artifact, &spec.repo) {
        Ok(p) if p.is_dir() => p,
        Ok(p) => {
            return CheckResult::from_reasons(
                vec![format!("artifact repo {} is missing", p.display())],
                Value::Null,
            );
        }
        Err(e) => return CheckResult::from_reasons(vec![e], Value::Null),
    };
    let pristine_root = opts.case_dir.join(&spec.pristine_dir);
    let mut pristine = Vec::new();
    for t in &spec.tests {
        let pp = pristine_root.join(&t.path);
        let psha = match sha256_file(&pp) {
            Ok(s) => s,
            Err(e) => {
                reasons.push(format!("grader pristine test unreadable: {e}"));
                continue;
            }
        };
        if psha != t.sha256 {
            reasons.push(format!(
                "grader pristine test {} does not match the case sha256 (grader-side copy altered)",
                t.path
            ));
            continue;
        }
        let ptext = fs::read_to_string(&pp).unwrap_or_default();
        let pm = marks(&ptext);
        let ap = match safe_join(&repo, &t.path) {
            Ok(p) => p,
            Err(e) => {
                reasons.push(e);
                continue;
            }
        };
        let mut entry = json!({"path": t.path, "pristine": marks_json(pm)});
        if !ap.is_file() {
            reasons.push(format!(
                "evaluation test {} was deleted from the artifact repo",
                t.path
            ));
            entry["state"] = json!("deleted");
        } else {
            let asha = sha256_file(&ap).unwrap_or_default();
            if asha == t.sha256 {
                entry["state"] = json!("pristine");
            } else {
                let am = marks(&fs::read_to_string(&ap).unwrap_or_default());
                let mut weak = Vec::new();
                if am.tests < pm.tests {
                    weak.push(format!("#[test] {} -> {}", pm.tests, am.tests));
                }
                if am.asserts < pm.asserts {
                    weak.push(format!("assert {} -> {}", pm.asserts, am.asserts));
                }
                if am.ignores > pm.ignores {
                    weak.push(format!("#[ignore] {} -> {}", pm.ignores, am.ignores));
                }
                if am.skips > pm.skips {
                    weak.push(format!("skip markers {} -> {}", pm.skips, am.skips));
                }
                reasons.push(format!(
                    "evaluation test {} differs from pristine (sha256 mismatch){}",
                    t.path,
                    if weak.is_empty() {
                        String::new()
                    } else {
                        format!("; weakened: {}", weak.join(", "))
                    }
                ));
                entry["state"] = json!(if weak.is_empty() {
                    "modified"
                } else {
                    "weakened"
                });
                entry["artifact"] = marks_json(am);
                entry["weakened"] = json!(weak);
            }
        }
        files.push(entry);
        pristine.push((t.path.clone(), pp));
    }
    if !reasons.is_empty() {
        // 改変・削除があれば失格。評価 command は走らせない。
        return CheckResult::from_reasons(reasons, json!({"tests": files, "ran": false}));
    }
    let wd = work_dir(&opts.work_root);
    let result = (|| -> Result<CommandOutput, String> {
        copy_tree(&repo, &wd)?;
        for (rel, pp) in &pristine {
            let to = wd.join(rel);
            if let Some(d) = to.parent() {
                fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            fs::copy(pp, &to).map_err(|e| format!("{}: {e}", to.display()))?;
        }
        opts.runner.run(&spec.command, &wd)
    })();
    let _ = fs::remove_dir_all(&wd);
    let out = match result {
        Ok(o) => o,
        Err(e) => {
            return CheckResult::from_reasons(
                vec![format!("evaluation command could not run: {e}")],
                json!({"tests": files, "ran": false}),
            );
        }
    };
    if out.exit != Some(0) {
        reasons.push(format!("evaluation command exited with {:?}", out.exit));
    }
    let mut missing = Vec::new();
    for name in &spec.must_pass {
        let ok = out.stdout.lines().any(|l| {
            l.trim()
                .strip_prefix("test ")
                .and_then(|r| r.strip_suffix("... ok"))
                .map(str::trim)
                .is_some_and(|t| t == name || t.ends_with(&format!("::{name}")))
        });
        if !ok {
            missing.push(name.clone());
        }
    }
    if !missing.is_empty() {
        reasons.push(format!(
            "required tests did not report ok: {}",
            missing.join(", ")
        ));
    }
    CheckResult::from_reasons(
        reasons,
        json!({
            "tests": files,
            "ran": true,
            "exit": out.exit,
            "missing_ok": missing,
            "stdout_tail": tail(&out.stdout, 2048),
            "stderr_tail": tail(&out.stderr, 2048),
        }),
    )
}
