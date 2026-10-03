//! ADR-0116 D6: launcher の session 記録と回収。
//!
//! session ごとに `<dir>/<session_id>.json` を tmp+rename・mode 0600 で書く。記録には process group
//! の leader の pid・starttime と launcher の instance id を入れ、signal を送る前に必ず
//! `same_process_alive(pid, starttime)` で本人かを確かめる（PID 再利用先を殺さない）。

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nix::libc;
use serde::{Deserialize, Serialize};

use crate::browser_runtime::same_process_alive;

/// 1 session の記録。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
    pub session_id: String,
    pub instance_id: String,
    /// process group の leader（bwrap）の pid。
    pub pid: i32,
    pub pgid: i32,
    pub starttime: u64,
    pub peer_pid: i32,
    pub peer_starttime: u64,
    pub lease_id: String,
    pub lease_deadline_unix_ms: u64,
}

/// 状態 dir の session 記録。`instance_id` は launcher 起動ごとの乱数。
#[derive(Debug, Clone)]
pub struct Registry {
    dir: PathBuf,
    instance_id: String,
}

fn valid_session_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Registry {
    /// 状態 dir を（無ければ 0700 で）用意する。
    pub fn open(dir: impl Into<PathBuf>, instance_id: impl Into<String>) -> std::io::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            dir,
            instance_id: instance_id.into(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn path(&self, session_id: &str) -> std::io::Result<PathBuf> {
        if !valid_session_id(session_id) {
            return Err(std::io::Error::other("invalid session id"));
        }
        Ok(self.dir.join(format!("{session_id}.json")))
    }

    /// 記録を tmp+rename で書く（mode 0600）。
    pub fn write(&self, rec: &SessionRecord) -> std::io::Result<()> {
        let path = self.path(&rec.session_id)?;
        let tmp = self.dir.join(format!(".{}.json.tmp", rec.session_id));
        let body = serde_json::to_vec(rec).map_err(std::io::Error::other)?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&body)?;
        f.sync_all()?;
        std::fs::rename(tmp, path)
    }

    /// 記録を消す（無ければ何もしない）。
    pub fn remove(&self, session_id: &str) -> std::io::Result<()> {
        match std::fs::remove_file(self.path(session_id)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// 全記録を読む。壊れた記録は（名前とともに）捨てる。
    pub fn read_all(&self) -> std::io::Result<Vec<SessionRecord>> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(&self.dir)? {
            let path = e?.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let parsed = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice::<SessionRecord>(&b).ok());
            match parsed {
                Some(r) if valid_session_id(&r.session_id) => out.push(r),
                _ => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        Ok(out)
    }

    /// launcher 起動時の回収: 他 instance の記録のうち、leader が記録した本人（starttime 一致）で
    /// まだ生きているものだけ process group ごと SIGKILL する。他 instance の記録は全部消す。
    /// 現 instance の記録には触らない。戻り値は signal を送った leader の pid。
    pub fn reap_orphans(&self) -> std::io::Result<Vec<i32>> {
        let mut killed = Vec::new();
        for rec in self.read_all()? {
            if rec.instance_id == self.instance_id {
                continue;
            }
            if signal_group_if_same(&rec, libc::SIGKILL) {
                killed.push(rec.pid);
            }
            self.remove(&rec.session_id)?;
        }
        Ok(killed)
    }
}

/// leader が記録の本人なら process group に `sig` を送る。送ったら true。
pub fn signal_group_if_same(rec: &SessionRecord, sig: i32) -> bool {
    if rec.pid <= 1 || rec.pgid <= 1 || !same_process_alive(rec.pid, rec.starttime) {
        return false;
    }
    // SAFETY: 直前に leader の starttime が記録と一致した。pgid は launcher が起動時に記録した値。
    unsafe { libc::kill(-rec.pgid, sig) == 0 }
}

/// process group を SIGTERM → `grace` 待ち → SIGKILL で止める。leader が本人でなければ何もしない。
/// leader が終わった（または zombie になった）ら true。
pub fn stop_group(rec: &SessionRecord, grace: Duration) -> bool {
    if !signal_group_if_same(rec, libc::SIGTERM) {
        return !same_process_alive(rec.pid, rec.starttime);
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if !same_process_alive(rec.pid, rec.starttime) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    signal_group_if_same(rec, libc::SIGKILL);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !same_process_alive(rec.pid, rec.starttime) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}
