//! `Workspace`（DESIGN §5.8）。本プロジェクトは `LocalWorkspace` のみ実装し、
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

/// DESIGN §5.8 の trait。
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
    /// ADR-0075 G3-fix1: `exec` の子プロセスから外す環境変数（親から継いだ `RUSTC_WRAPPER` / `SCCACHE_*` など。
    /// `env` より先に `Command::env_remove` する）。
    env_remove: Vec<String>,
}

impl LocalWorkspace {
    /// `dir` はそのタスクの作業ディレクトリ（ADR-0005 D3）。
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            work_dir: None,
            container: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        }
    }

    /// ADR-0074 F5-fix: 判定コマンドに環境変数を足す（WU の run と同じ `CARGO_TARGET_DIR` で検査するため）。
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    /// ADR-0075 G3-fix1: 判定コマンドの子プロセスから外す環境変数。
    pub fn with_env_removed(mut self, keys: Vec<String>) -> Self {
        self.env_remove = keys;
        self
    }

    /// ADR-0075 G3-fix1: run と同じ `CargoEnv`（`set` を重ね、`remove` を外す）。
    pub fn with_cargo_env(self, env: crate::scratch::CargoEnv) -> Self {
        self.with_env(env.set).with_env_removed(env.remove)
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
        for key in &self.env_remove {
            command.env_remove(key);
        }
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

        let read_stdout = async {
            let mut buf = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut buf).await?;
            Ok::<Vec<u8>, std::io::Error>(buf)
        };
        let read_stderr = async {
            let mut buf = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf).await?;
            Ok::<Vec<u8>, std::io::Error>(buf)
        };

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
