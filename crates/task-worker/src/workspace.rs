//! `Workspace`（作業場所。docs/SPEC.md §3.7）。本プロジェクトは `LocalWorkspace` のみ実装し、
//! `RemoteWorkspace` は接続層プロジェクトが実装するまで骨組みのみ（ADR-0005 D3）。
//!
//! 1 インスタンス = 1 タスクの作業ディレクトリ。ディスパッチャが run ごとに
//! `LocalWorkspace::new(dir)` を作る。

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use task_core::{ArtifactRef, Task};

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("input artifact not found: {0}")]
    InputNotFound(String),
    #[error("remote workspace is not implemented (cluster={0})")]
    RemoteUnsupported(String),
    /// ADR-0018 D5: ssh / rsync 自体の失敗（接続が無い・認証を求められた・宛先に届かない）。供給側失敗として扱う。
    #[error("cluster is unreachable: {0}")]
    Unreachable(String),
    /// リモート側の準備に失敗した（ディレクトリが作れない・rsync が異常終了した）。
    #[error("remote error: {0}")]
    Remote(String),
    /// ADR-0059 D3: `sync = "worktree"` の準備でリモートの `path` が git リポジトリでなかった（exit 65）。
    /// `Remote` から独立したバリアントにして、呼び出し側（`run_worker`）が文字列を見ずに「自動で
    /// `shared` へ格下げしてよいか」を型で判定できるようにする。
    #[error("not a git repository: {0}")]
    NotAGitRepository(String),
}

/// `Workspace::exec` の結果。`exit` は signal 終了・タイムアウト時 `None`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub exit: Option<i32>,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub timed_out: bool,
}

/// 作業場所の trait（docs/SPEC.md §3.7）。
#[async_trait]
pub trait Workspace: Send + Sync {
    /// 作業ディレクトリと成果物ディレクトリ（ADR-0036: `artifacts/` または
    /// `.taskd/artifacts/<task_id>/`）、`inputs/`, `runs/` を作り、`task.inputs` を `inputs/<name>` に配置する。
    /// 既存ファイルは消さない。戻り値は作業ディレクトリの絶対パス。
    async fn prepare(&self, task: &Task) -> Result<PathBuf, WorkspaceError>;
    /// `sh -c <cmd>` を作業ディレクトリで実行する。`timeout` 超過は kill して `timed_out=true`。
    /// stdout/stderr は末尾 4 KiB を返す。
    async fn exec(&self, cmd: &str, timeout: Duration) -> Result<ExecResult, WorkspaceError>;
    /// そのタスクの成果物ディレクトリ（ADR-0036）配下のファイルを再帰的に列挙し、sha256 付きの
    /// `ArtifactRef` を返す（`path` は作業ディレクトリ相対、`name` はファイル名、`kind` は拡張子）。
    async fn collect(&self, task: &Task) -> Result<Vec<ArtifactRef>, WorkspaceError>;
}

/// ローカルファイルシステム上の作業ディレクトリ。
#[derive(Debug, Clone)]
pub struct LocalWorkspace {
    dir: PathBuf,
    /// ADR-0041 D1: コマンドを実行する場所（worktree）。`None` なら `dir` と同じ（従来どおり）。
    work_dir: Option<PathBuf>,
    /// ADR-0043 D3（Phase 56）: `Some` なら `exec` をコンテナの中で走らせる（`[commands] setup` が
    /// 「その実行環境で」走るために要る）。`None` はホスト実行（従来どおり）。
    container: Option<crate::container::SharedPlan>,
    /// ADR-0074 F5-fix: `exec`（判定コマンド）に足す環境変数（`CARGO_TARGET_DIR` など）。空なら従来どおり。
    env: Vec<(String, String)>,
    /// 2026-10-04 統合の検査の進み具合 D2: `Some` なら `exec` の stdout/stderr を出た順にこのファイルへ逐次追記する
    /// （[`OUTPUT_LOG_CAP_BYTES`] まで）。`None` は従来どおり（末尾だけを返す）。
    output_log: Option<PathBuf>,
}

/// 2026-10-04 統合の検査の進み具合 D2: `with_output_log` のファイル 1 件の上限。超えたら以降は書かず、その旨を 1 行残す。
pub const OUTPUT_LOG_CAP_BYTES: u64 = 8 * 1024 * 1024;

/// `with_output_log` のファイルへ追記する口（上限を数える）。stdout と stderr の読み手が共有する。
struct OutputLog {
    file: std::fs::File,
    written: u64,
    truncated: bool,
}

impl OutputLog {
    fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            file,
            written,
            truncated: false,
        })
    }

    fn write(&mut self, chunk: &[u8]) {
        use std::io::Write;
        if self.truncated {
            return;
        }
        let room = OUTPUT_LOG_CAP_BYTES.saturating_sub(self.written);
        if (chunk.len() as u64) <= room {
            if self.file.write_all(chunk).is_ok() {
                self.written += chunk.len() as u64;
            }
            return;
        }
        let head = &chunk[..room as usize];
        let _ = self.file.write_all(head);
        let _ = self.file.write_all(
            format!("\n[celeris: output truncated at {OUTPUT_LOG_CAP_BYTES} bytes]\n").as_bytes(),
        );
        self.written += room;
        self.truncated = true;
    }
}

/// 子の出力を読み切りながら、`log` があればそこへも逐次書く。
async fn read_teed<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    log: Option<&std::sync::Mutex<OutputLog>>,
) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = tokio::io::AsyncReadExt::read(&mut reader, &mut chunk).await?;
        if n == 0 {
            return Ok(buf);
        }
        if let Some(log) = log
            && let Ok(mut log) = log.lock()
        {
            log.write(&chunk[..n]);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

impl LocalWorkspace {
    /// `dir` はそのタスクの作業ディレクトリ（ADR-0005 D3）。
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            work_dir: None,
            container: None,
            env: Vec::new(),
            output_log: None,
        }
    }

    /// 2026-10-04 統合の検査の進み具合 D2: `exec` の出力をこのファイルにも逐次追記する（親ディレクトリは作る）。
    pub fn with_output_log(mut self, path: impl Into<PathBuf>) -> Self {
        self.output_log = Some(path.into());
        self
    }

    /// ADR-0074 F5-fix: 判定コマンドに環境変数を足す（WU の run と同じ `CARGO_TARGET_DIR` で検査するため）。
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    /// run と同じ `CargoEnv`（`set` を重ねる。ADR-0129 (1): 継いだ env は外さない）。
    pub fn with_cargo_env(self, env: crate::scratch::CargoEnv) -> Self {
        self.with_env(env.set)
    }

    /// ADR-0041 D1: `runs/` `inputs/` `artifacts/` は `dir`、コマンドは `work_dir`（worktree）で動かす。
    pub fn with_work_dir(mut self, work_dir: impl Into<PathBuf>) -> Self {
        self.work_dir = Some(work_dir.into());
        self
    }

    /// ADR-0043 D3: `exec` をコンテナの中で走らせる（`setup` 用）。
    pub fn with_container(mut self, plan: Option<crate::container::SharedPlan>) -> Self {
        self.container = plan;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 実際にコマンドを動かす場所（worktree があればそこ）。
    pub fn work_dir(&self) -> &Path {
        self.work_dir.as_deref().unwrap_or(&self.dir)
    }
}

/// 接続層プロジェクトが実装する。型のみ。
#[derive(Debug, Clone)]
pub struct RemoteWorkspace {
    pub cluster: String,
    pub path: PathBuf,
}

// ---- 以下は implementer（unit B）が実装する ----

const TAIL_BYTES: usize = 4096;

pub(crate) fn tail_utf8_lossy(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[async_trait]
impl Workspace for LocalWorkspace {
    async fn prepare(&self, task: &Task) -> Result<PathBuf, WorkspaceError> {
        tokio::fs::create_dir_all(&self.dir).await?;
        // ADR-0036 D1: 成果物ディレクトリはタスクごと（共有 workspace では `.taskd/artifacts/<task_id>/`）。
        tokio::fs::create_dir_all(task_core::artifacts::artifacts_dir_for(task, &self.dir)).await?;
        tokio::fs::create_dir_all(self.dir.join("inputs")).await?;
        tokio::fs::create_dir_all(self.dir.join("runs")).await?;

        for input in &task.inputs {
            let src = Path::new(&input.path);
            if src.is_absolute() {
                let dest = self.dir.join("inputs").join(&input.name);
                tokio::fs::copy(src, &dest)
                    .await
                    .map_err(|_| WorkspaceError::InputNotFound(input.name.clone()))?;
            } else if self.dir.join(src).exists() {
                // すでに配置済み。何もしない。
            } else {
                return Err(WorkspaceError::InputNotFound(input.name.clone()));
            }
        }

        let canon = self.dir.canonicalize()?;
        Ok(canon)
    }

    async fn exec(&self, cmd: &str, timeout: Duration) -> Result<ExecResult, WorkspaceError> {
        use std::process::Stdio;

        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(cmd);
        // ADR-0019 D1 6. / ADR-0041 D1: 判定コマンドは worktree の中で実行する。
        command.current_dir(self.work_dir());
        command.envs(self.env.iter().cloned());
        // ★ ADR-0043 D3 の差し込み点（コンテナ実行）。`None` ならそのまま（ホスト実行は変わらない）。
        let mut command = crate::db_guard::launch(command, self.container.as_deref());
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.kill_on_drop(true);
        #[cfg(unix)]
        {
            command.process_group(0);
        }

        let mut child = command.spawn()?;
        let mut stdout = child.stdout.take().ok_or_else(|| {
            WorkspaceError::Io(std::io::Error::other("child stdout not captured"))
        })?;
        let mut stderr = child.stderr.take().ok_or_else(|| {
            WorkspaceError::Io(std::io::Error::other("child stderr not captured"))
        })?;

        // 開けなければログ無しで続ける（判定は従来どおり。ログは見るためだけのもの）。
        let log = match &self.output_log {
            Some(path) => match OutputLog::open(path) {
                Ok(log) => Some(std::sync::Mutex::new(log)),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "cannot open the exec output log");
                    None
                }
            },
            None => None,
        };
        let read_stdout = read_teed(&mut stdout, log.as_ref());
        let read_stderr = read_teed(&mut stderr, log.as_ref());

        let wait_fut = child.wait();
        let combined = async {
            let (out, err, status) = tokio::join!(read_stdout, read_stderr, wait_fut);
            (out, err, status)
        };

        match tokio::time::timeout(timeout, combined).await {
            Ok((out, err, status)) => {
                let out = out?;
                let err = err?;
                let status = status?;
                Ok(ExecResult {
                    exit: status.code(),
                    stdout_tail: tail_utf8_lossy(&out),
                    stderr_tail: tail_utf8_lossy(&err),
                    timed_out: false,
                })
            }
            Err(_elapsed) => {
                if let Some(pid) = child.id() {
                    #[cfg(unix)]
                    {
                        let _ = nix::sys::signal::killpg(
                            nix::unistd::Pid::from_raw(pid as i32),
                            nix::sys::signal::Signal::SIGKILL,
                        );
                    }
                }
                let _ = child.wait().await;
                Ok(ExecResult {
                    exit: None,
                    stdout_tail: String::new(),
                    stderr_tail: String::new(),
                    timed_out: true,
                })
            }
        }
    }

    async fn collect(&self, task: &Task) -> Result<Vec<ArtifactRef>, WorkspaceError> {
        let artifacts_dir = task_core::artifacts::artifacts_dir_for(task, &self.dir);
        if !artifacts_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        collect_files_recursive(&artifacts_dir, &mut files)?;

        let mut refs = Vec::with_capacity(files.len());
        for file in &files {
            let rel = file.strip_prefix(&self.dir).map_err(|_| {
                WorkspaceError::Io(std::io::Error::other("artifact path outside workspace"))
            })?;
            let rel_str = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let kind = file
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".to_string());
            let sha256 = crate::artifact::sha256_file(file)?;
            refs.push(ArtifactRef {
                name,
                path: rel_str,
                sha256,
                kind,
                declared: true,
            });
        }

        refs.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(refs)
    }
}

fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), WorkspaceError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files_recursive(&path, out)?;
        } else if file_type.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

#[async_trait]
impl Workspace for RemoteWorkspace {
    async fn prepare(&self, _task: &Task) -> Result<PathBuf, WorkspaceError> {
        unimplemented!("RemoteWorkspace is implemented by the connection layer project")
    }

    async fn exec(&self, _cmd: &str, _timeout: Duration) -> Result<ExecResult, WorkspaceError> {
        unimplemented!("RemoteWorkspace is implemented by the connection layer project")
    }

    async fn collect(&self, _task: &Task) -> Result<Vec<ArtifactRef>, WorkspaceError> {
        unimplemented!("RemoteWorkspace is implemented by the connection layer project")
    }
}

#[cfg(test)]
mod tests;
