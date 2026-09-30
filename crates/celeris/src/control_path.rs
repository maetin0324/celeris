//! ADR-0078 D2: ssh の `ControlPath` の置き場所を起動時に検査する（warn するだけで、起動は止めない）。
//!
//! celeris は `ControlPath` を上書きしない（人の ssh・`scripts/cluster-login.sh`・celeris が同じ master を
//! 共有するため）。代わりに `ssh -G <host>`（設定の展開だけで通信しない）で実効の値を読み、
//! master が途中で消える・借りられなくなる置き場所を早めに知らせる。判定は純関数
//! [`control_path_warnings`]、実環境を読む部分は [`inspect_cluster_control_path`]。

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// unix socket の上限 108 から、ssh が作成時に付ける一時の接尾辞 17 文字を引いた余裕。
pub const MAX_CONTROL_PATH_BYTES: usize = 90;

/// 判定に使う環境の事実。純関数にするため、呼び出し側が集めて渡す。`None` は「分からなかった」。
#[derive(Debug, Clone, Default)]
pub struct ControlPathEnv {
    pub xdg_runtime_dir: Option<String>,
    /// `loginctl show-user $USER -p Linger --value` が `yes` か。
    pub linger: Option<bool>,
    /// `ControlPath` の親ディレクトリが自分の所有か。
    pub parent_owned_by_me: Option<bool>,
    /// 親ディレクトリの permission ビット（`mode & 0o777`）。
    pub parent_mode: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warn,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPathFinding {
    pub severity: Severity,
    pub message: String,
}

fn warn(message: String) -> ControlPathFinding {
    ControlPathFinding {
        severity: Severity::Warn,
        message,
    }
}

/// `ssh -G` の出力から小文字キーの値を引く（値は最初の空白の後ろ。同じキーは最初のものを採る）。
fn ssh_g_value<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        let (k, v) = line.split_once(' ')?;
        (k == key).then(|| v.trim())
    })
}

/// `ssh -G <host>` の出力と環境の事実から、`ControlPath` まわりの指摘を作る（純関数）。
pub fn control_path_warnings(ssh_g_output: &str, env: &ControlPathEnv) -> Vec<ControlPathFinding> {
    let mut out = Vec::new();
    let control_path = ssh_g_value(ssh_g_output, "controlpath");
    let control_master = ssh_g_value(ssh_g_output, "controlmaster");
    let control_persist = ssh_g_value(ssh_g_output, "controlpersist");

    let path_none = control_path.is_none_or(|p| p.eq_ignore_ascii_case("none"));
    let master_no = control_master
        .is_some_and(|m| m.eq_ignore_ascii_case("false") || m.eq_ignore_ascii_case("no"));
    if path_none || master_no {
        out.push(warn(format!(
            "ControlPath is not usable for sharing a master (controlpath={:?}, controlmaster={:?}); \
             celeris cannot borrow a master that a human opened",
            control_path.unwrap_or("(unset)"),
            control_master.unwrap_or("(unset)"),
        )));
    }

    // `%` が残る値（未展開のトークン）は長さも親ディレクトリも決められないので検査しない。
    if let Some(path) = control_path.filter(|p| !p.eq_ignore_ascii_case("none") && !p.contains('%'))
    {
        if path.len() > MAX_CONTROL_PATH_BYTES {
            out.push(warn(format!(
                "ControlPath is {} bytes (> {MAX_CONTROL_PATH_BYTES}); it may exceed the unix socket limit: {path}",
                path.len()
            )));
        }
        if env.parent_owned_by_me == Some(false) {
            out.push(warn(format!(
                "the parent directory of ControlPath is not owned by the current user: {path}"
            )));
        }
        if let Some(mode) = env.parent_mode
            && mode & 0o777 != 0o700
        {
            out.push(warn(format!(
                "the parent directory of ControlPath has mode {:04o} (expected 0700): {path}",
                mode & 0o777
            )));
        }
        if let Some(xdg) = env.xdg_runtime_dir.as_deref().filter(|x| !x.is_empty())
            && Path::new(path).starts_with(xdg)
            && env.linger != Some(true)
        {
            out.push(warn(format!(
                "ControlPath is under XDG_RUNTIME_DIR ({xdg}) but Linger is not enabled \
                 (linger={:?}); the master disappears at logout: {path}",
                env.linger
            )));
        }
    }

    if let Some(persist) = control_persist {
        out.push(ControlPathFinding {
            severity: Severity::Info,
            message: format!(
                "the human ssh config has ControlPersist={persist}; celeris overrides it with \
                 -o ControlPersist=<[[clusters]] control_persist> for the master it opens (ADR-0078 D1)"
            ),
        });
    }
    out
}

/// `ssh -G <host>` を短い timeout で走らせる。失敗・timeout は `None`。
fn run_ssh_g(host: &str, timeout: Duration) -> Option<String> {
    let mut child = Command::new("ssh")
        .args(["-G", host])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // 出力は数 KB なのでパイプが詰まることはない。timeout の間だけ終了を待つ。
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut buf = String::new();
                stdout.read_to_string(&mut buf).ok()?;
                return Some(buf);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn linger_of_current_user() -> Option<bool> {
    let user = std::env::var("USER").ok().filter(|u| !u.is_empty())?;
    let output = Command::new("loginctl")
        .args(["show-user", &user, "-p", "Linger", "--value"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim() == "yes")
}

/// 実環境を読んで [`control_path_warnings`] の入力を作り、指摘を warn / info で 1 回ずつ残す。
/// 失敗しても何もしない（起動を止めない）。ssh を 1 回起こすので、呼び出し側はバックグラウンドで呼ぶ。
pub fn inspect_cluster_control_path(cluster_id: &str, host: &str) {
    let Some(output) = run_ssh_g(host, Duration::from_secs(5)) else {
        tracing::debug!(cluster = %cluster_id, host = %host, "control_path: `ssh -G` did not give a result; skipped");
        return;
    };
    let mut env = ControlPathEnv {
        xdg_runtime_dir: std::env::var("XDG_RUNTIME_DIR").ok(),
        linger: linger_of_current_user(),
        ..ControlPathEnv::default()
    };
    if let Some(parent) = ssh_g_value(&output, "controlpath")
        .filter(|p| !p.contains('%') && !p.eq_ignore_ascii_case("none"))
        .and_then(|p| Path::new(p).parent())
        && let Ok(meta) = std::fs::metadata(parent)
    {
        env.parent_mode = Some(meta.mode() & 0o777);
        // 自分の uid は `/proc/self` の所有者から得る（unsafe / libc を使わない）。
        env.parent_owned_by_me = std::fs::metadata("/proc/self")
            .ok()
            .map(|me| me.uid() == meta.uid());
    }
    for finding in control_path_warnings(&output, &env) {
        match finding.severity {
            Severity::Warn => {
                tracing::warn!(cluster = %cluster_id, host = %host, "control_path: {}", finding.message)
            }
            Severity::Info => {
                tracing::info!(cluster = %cluster_id, host = %host, "control_path: {}", finding.message)
            }
        }
    }
}

#[cfg(test)]
#[path = "control_path/tests.rs"]
mod tests;
