//! grade の評価 command を実際に走らせる実行器（ADR 2026-10-16-mlsys-pilot-wiring-fixes D9）。
//! 判定の library（grader.rs）は子 process を起こさず、CLI がこれを注入する。command は grade の作業 dir
//! （成果物 repo の複製に pristine 試験を重ねたもの）を cwd にして走り、`timeout` を超えたら process group ごと止める。
use std::{
    fs::{self, File},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::grader::{CommandOutput, CommandRunner};

/// 評価 command 1 回の既定の上限。
pub const DEFAULT_GRADE_TIMEOUT: Duration = Duration::from_secs(1800);

pub struct ProcessRunner {
    pub timeout: Duration,
}

impl Default for ProcessRunner {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_GRADE_TIMEOUT,
        }
    }
}

impl CommandRunner for ProcessRunner {
    fn run(&self, argv: &[String], cwd: &Path) -> Result<CommandOutput, String> {
        let (prog, rest) = argv.split_first().ok_or("empty evaluation command")?;
        // 出力は作業 dir の外（隣）の file に受ける（pipe が詰まって止まらないように）。
        let log = |ext: &str| cwd.with_extension(ext);
        let (out_path, err_path) = (log("stdout"), log("stderr"));
        let open = |p: &Path| File::create(p).map_err(|e| format!("{}: {e}", p.display()));
        let mut child = Command::new(prog)
            .args(rest)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(open(&out_path)?)
            .stderr(open(&err_path)?)
            .process_group(0)
            .spawn()
            .map_err(|e| format!("spawn {prog}: {e}"))?;
        let start = Instant::now();
        let status = loop {
            if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
                break Some(st);
            }
            if start.elapsed() > self.timeout {
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .stderr(Stdio::null())
                    .status();
                let _ = child.wait();
                break None;
            }
            thread::sleep(Duration::from_millis(100));
        };
        let read = |p: &Path| {
            let s = String::from_utf8_lossy(&fs::read(p).unwrap_or_default()).into_owned();
            let _ = fs::remove_file(p);
            s
        };
        let mut stderr = read(&err_path);
        if status.is_none() {
            stderr.push_str(&format!(
                "\n[grade] killed after {}s timeout",
                self.timeout.as_secs()
            ));
        }
        Ok(CommandOutput {
            exit: status.and_then(|s| s.code()),
            stdout: read(&out_path),
            stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_in_cwd_and_captures_output_and_exit() {
        let d = tempfile::tempdir().expect("dir");
        let wd = d.path().join("work");
        fs::create_dir_all(&wd).expect("mk");
        fs::write(wd.join("marker"), "x").expect("w");
        let r = ProcessRunner::default();
        let argv = ["sh", "-c", "ls; echo err >&2; exit 3"].map(String::from);
        let o = r.run(&argv, &wd).expect("run");
        assert_eq!(o.exit, Some(3));
        assert!(o.stdout.contains("marker"), "{o:?}");
        assert!(o.stderr.contains("err"), "{o:?}");
        // 出力の受け file は残さない。
        assert!(!wd.with_extension("stdout").exists());
    }

    #[test]
    fn timeout_kills_and_reports_no_exit_code() {
        let d = tempfile::tempdir().expect("dir");
        let wd = d.path().join("work");
        fs::create_dir_all(&wd).expect("mk");
        let r = ProcessRunner {
            timeout: Duration::from_millis(200),
        };
        // 待つだけ（CPU を使わない）。
        let argv = ["sleep", "30"].map(String::from);
        let o = r.run(&argv, &wd).expect("run");
        assert_eq!(o.exit, None);
        assert!(o.stderr.contains("timeout"));
    }
}
