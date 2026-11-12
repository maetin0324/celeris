//! ADR 2026-11-12-host-check-slots: host 共通の重い検査の枠（check slot）。
//!
//! 枠は `<dir>/slot-<i>.lock` の `flock(2)` の排他 lock（D1）。daemon の検査は [`SlottedWorkspace`] が、
//! script（test-affected.sh・test-parallel.sh）は `scripts/dev/lib/check-slot.sh` が同じ file を取り合う。
//! 枠を待つ時間は検査の timeout に数えない（D3）。待ちの理由は 1 回だけ log と `waiting/` に残す（D5）。

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use nix::fcntl::{Flock, FlockArg};
use task_core::{ArtifactRef, Task};

use crate::workspace::{ExecResult, Workspace, WorkspaceError};

/// 枠の dir を差し替える環境変数（D1）。無ければ `$HOME/.cache/celeris/check-slots`。
pub const SLOTS_DIR_ENV: &str = "CELERIS_CHECK_SLOTS_DIR";
/// 枠の数を差し替える環境変数（D2）。
pub const SLOTS_ENV: &str = "CELERIS_CHECK_SLOTS";
/// 取った枠（lock の path）を command に渡す環境変数（D3）。script はこれがあれば取り直さない。
pub const HELD_ENV: &str = "CELERIS_CHECK_SLOT";
/// 枠 1 本あたりの core 数（D2。`scripts/dev/lib/check-slot.sh` と同じ値）。
pub const CORES_PER_SLOT: usize = 12;
/// 枠が空くのを確かめ直す間隔（D5）。
pub const POLL: Duration = Duration::from_secs(10);

/// D2: 枠の数の既定（`max(1, core 数 / 12)`）。
pub fn default_slots(cores: usize) -> usize {
    (cores / CORES_PER_SLOT).max(1)
}

/// D4: 枠を取る重い command か（`cargo`・`cargo-nextest`・`nextest` の語、または test-affected・test-parallel）。
pub fn is_heavy(cmd: &str) -> bool {
    cmd.split(|c: char| {
        c.is_whitespace() || matches!(c, ';' | '&' | '|' | '(' | ')' | '`' | '"' | '\'')
    })
    .any(|word| {
        let base = word.rsplit('/').next().unwrap_or(word);
        matches!(base, "cargo" | "cargo-nextest" | "nextest")
            || word.contains("test-affected")
            || word.contains("test-parallel")
    })
}

/// host の枠の集まり（dir と数）。
#[derive(Debug, Clone)]
pub struct CheckSlots {
    dir: PathBuf,
    slots: usize,
    poll: Duration,
}

/// 枠を 1 つ持っている間の印。drop で lock を外し、holder の file を消す。
#[derive(Debug)]
pub struct CheckSlotGuard {
    _lock: Flock<File>,
    path: PathBuf,
    holder: PathBuf,
}

impl CheckSlotGuard {
    /// 取った枠の lock の path（[`HELD_ENV`] で command に渡す）。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CheckSlotGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.holder);
    }
}

/// 待っている間だけ `waiting/` に置く file（drop で消す）。
struct WaitingMark(PathBuf);

impl Drop for WaitingMark {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

static WAITER_SEQ: AtomicU64 = AtomicU64::new(0);

/// 枠の使用中の 1 件（status 用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotHolder {
    pub slot: usize,
    pub label: String,
    pub since: String,
}

/// 枠を待っている 1 件（status 用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotWaiter {
    pub pid: u32,
    pub label: String,
    pub since: String,
    pub reason: String,
}

/// D6: 枠の status。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotStatus {
    pub dir: PathBuf,
    pub slots: usize,
    pub held: Vec<SlotHolder>,
    pub waiting: Vec<SlotWaiter>,
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

fn read_fields(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn field(fields: &[(String, String)], key: &str) -> String {
    fields
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

impl CheckSlots {
    pub fn new(dir: impl Into<PathBuf>, slots: usize) -> Self {
        Self {
            dir: dir.into(),
            slots: slots.max(1),
            poll: POLL,
        }
    }

    /// 試験用: 確かめ直す間隔を変える。
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// D1/D2: 環境変数と core 数から決める。dir を決められない（`HOME` も無い）なら `None`。
    pub fn from_env() -> Option<Self> {
        let dir = match std::env::var_os(SLOTS_DIR_ENV) {
            Some(d) if !d.is_empty() => PathBuf::from(d),
            _ => PathBuf::from(std::env::var_os("HOME")?).join(".cache/celeris/check-slots"),
        };
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let slots = std::env::var(SLOTS_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or_else(|| default_slots(cores));
        Some(Self::new(dir, slots))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn slots(&self) -> usize {
        self.slots
    }

    fn lock_path(&self, i: usize) -> PathBuf {
        self.dir.join(format!("slot-{i}.lock"))
    }

    fn holder_path(&self, i: usize) -> PathBuf {
        self.dir.join(format!("slot-{i}.holder"))
    }

    fn open_lock(&self, i: usize) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.lock_path(i))
    }

    /// 空いている枠を 1 つ取る（待たない）。全部使用中なら `None`。
    pub fn try_take(&self, label: &str) -> io::Result<Option<CheckSlotGuard>> {
        std::fs::create_dir_all(&self.dir)?;
        for i in 0..self.slots {
            let file = self.open_lock(i)?;
            match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Ok(lock) => {
                    let holder = self.holder_path(i);
                    let _ = std::fs::write(
                        &holder,
                        format!(
                            "pid={}\nlabel={}\nsince={}\n",
                            std::process::id(),
                            one_line(label),
                            now_rfc3339()
                        ),
                    );
                    return Ok(Some(CheckSlotGuard {
                        _lock: lock,
                        path: self.lock_path(i),
                        holder,
                    }));
                }
                Err((_, nix::errno::Errno::EWOULDBLOCK)) => continue,
                Err((_, e)) => return Err(io::Error::from(e)),
            }
        }
        Ok(None)
    }

    /// 枠が空くまで待って取る。待ちの始めに 1 回だけ `on_wait(理由)` を呼び、`waiting/` に理由を置く（D5）。
    pub async fn acquire(
        &self,
        label: &str,
        mut on_wait: impl FnMut(&str),
    ) -> io::Result<CheckSlotGuard> {
        let mut mark: Option<WaitingMark> = None;
        loop {
            if let Some(guard) = self.try_take(label)? {
                drop(mark);
                return Ok(guard);
            }
            if mark.is_none() {
                let holders: Vec<String> =
                    self.status().held.into_iter().map(|h| h.label).collect();
                let reason = format!(
                    "host の検査の枠 {} 本がすべて使用中（{}）",
                    self.slots,
                    if holders.is_empty() {
                        "使用中の検査は不明".to_string()
                    } else {
                        holders.join("、")
                    }
                );
                let waiting = self.dir.join("waiting");
                std::fs::create_dir_all(&waiting)?;
                let path = waiting.join(format!(
                    "{}-{}",
                    std::process::id(),
                    WAITER_SEQ.fetch_add(1, Ordering::Relaxed)
                ));
                let _ = std::fs::write(
                    &path,
                    format!(
                        "pid={}\nlabel={}\nsince={}\nreason={}\n",
                        std::process::id(),
                        one_line(label),
                        now_rfc3339(),
                        one_line(&reason)
                    ),
                );
                mark = Some(WaitingMark(path));
                on_wait(&reason);
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    /// D6: 使用中の枠と、生きている待ちの一覧。空きの判定は lock で行う（holder の file は表示だけ）。
    pub fn status(&self) -> SlotStatus {
        let mut held = Vec::new();
        for i in 0..self.slots {
            let Ok(file) = OpenOptions::new().read(true).open(self.lock_path(i)) else {
                continue;
            };
            if Flock::lock(file, FlockArg::LockSharedNonblock).is_ok() {
                continue;
            }
            let fields = read_fields(&self.holder_path(i));
            held.push(SlotHolder {
                slot: i,
                label: field(&fields, "label"),
                since: field(&fields, "since"),
            });
        }
        let mut waiting = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.dir.join("waiting")) {
            for entry in entries.flatten() {
                let fields = read_fields(&entry.path());
                let Ok(pid) = field(&fields, "pid").parse::<u32>() else {
                    continue;
                };
                if !Path::new(&format!("/proc/{pid}")).exists() {
                    continue;
                }
                waiting.push(SlotWaiter {
                    pid,
                    label: field(&fields, "label"),
                    since: field(&fields, "since"),
                    reason: field(&fields, "reason"),
                });
            }
        }
        waiting.sort_by(|a, b| a.since.cmp(&b.since).then(a.label.cmp(&b.label)));
        SlotStatus {
            dir: self.dir.clone(),
            slots: self.slots,
            held,
            waiting,
        }
    }
}

/// 待ちの理由を受け取る口（既定は log）。
pub type WaitNote = Arc<dyn Fn(&str, &str) + Send + Sync>;

fn log_wait() -> WaitNote {
    Arc::new(|label: &str, reason: &str| {
        tracing::info!(label, reason, "check waits for a host check slot");
    })
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// D3: daemon の検査の作業場所を包み、重い command だけ枠を取ってから timeout 付きで実行する。
pub struct SlottedWorkspace {
    inner: Box<dyn Workspace>,
    slots: CheckSlots,
    label: String,
    note: WaitNote,
}

impl SlottedWorkspace {
    pub fn new(inner: Box<dyn Workspace>, slots: CheckSlots, label: impl Into<String>) -> Self {
        Self {
            inner,
            slots,
            label: label.into(),
            note: log_wait(),
        }
    }

    /// 試験用: 待ちの理由の受け取り口を差し替える。
    pub fn with_wait_note(mut self, note: WaitNote) -> Self {
        self.note = note;
        self
    }

    /// host の枠（[`CheckSlots::from_env`]）で包む。枠の dir を決められなければ包まずに返す。
    pub fn wrap(inner: Box<dyn Workspace>, label: impl Into<String>) -> Box<dyn Workspace> {
        match CheckSlots::from_env() {
            Some(slots) => Box::new(Self::new(inner, slots, label)),
            None => {
                tracing::warn!(
                    "check slots: neither CELERIS_CHECK_SLOTS_DIR nor HOME is set; checks run without a host slot"
                );
                inner
            }
        }
    }
}

/// D3: daemon の local の検査の作業場所を host の枠で包む（[`SlottedWorkspace::wrap`]）。
pub fn slotted(ws: crate::LocalWorkspace, label: impl Into<String>) -> Box<dyn Workspace> {
    SlottedWorkspace::wrap(Box::new(ws), label)
}

#[async_trait]
impl Workspace for SlottedWorkspace {
    async fn prepare(&self, task: &Task) -> Result<PathBuf, WorkspaceError> {
        self.inner.prepare(task).await
    }

    async fn exec(&self, cmd: &str, timeout: Duration) -> Result<ExecResult, WorkspaceError> {
        if !is_heavy(cmd) {
            return self.inner.exec(cmd, timeout).await;
        }
        let label = format!("{}: {}", self.label, cmd);
        let note = self.note.clone();
        let guard = self
            .slots
            .acquire(&label, |reason| note(&label, reason))
            .await?;
        let held = format!(
            "{HELD_ENV}={}; export {HELD_ENV}; {cmd}",
            shell_quote(&guard.path().to_string_lossy())
        );
        let result = self.inner.exec(&held, timeout).await;
        drop(guard);
        result
    }

    async fn collect(&self, task: &Task) -> Result<Vec<ArtifactRef>, WorkspaceError> {
        self.inner.collect(task).await
    }
}

#[cfg(test)]
mod tests;
