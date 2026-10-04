//! クラスタへの接続を GUI から張る（ADR-0032）。
//!
//! `SshWorkspace`（`ssh.rs`）は「人が張った ControlMaster を借りる」だけだった（ADR-0018 D2）。
//! ここではその借り先を celeris 自身が用意する: `ssh -M -N` の子プロセスを celeris が**保持し続ける**ことで
//! master を張る（`-f` は使わない。`ControlPersist` に依存しないため。ADR-0032 D2）。
//!
//! - `auth = "publickey"`（`interactive = false`）: `BatchMode=yes` で鍵だけの接続を試みる。
//! - `auth = "totp"`（`interactive = true`）: `SSH_ASKPASS` 経由でプロンプトと検証コードを GUI と中継する
//!   （ADR-0032 D4）。コードはメモリと FIFO（カーネルのパイプバッファ）だけを通り、ディスクには残らない。
//!
//! ADR-0060: master は既定で `systemd-run --user --scope` を使って celeris（`celeris@<sha12>` unit）の
//! cgroup の外の scope で起こす（[`MasterLauncher`]）。celeris の unit が `KillMode=control-group`（既定）で
//! 止まっても、別 scope にいる master は巻き込まれない。接続が成立した後は celeris の終了・再起動で
//! master を殺さない（[`ClusterMaster`] には Drop を持たせない。明示的な切断だけが [`ClusterMaster::kill`]
//! を呼ぶ）。

use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nix::sys::signal::Signal;
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

use crate::claude_account::{READER_JOIN_TIMEOUT, join_with_timeout, pump_reader, truncate_detail};
use crate::ssh::control_master_alive_blocking;
use crate::subprocess::send_signal_to_group;

/// `-O check` をポーリングする間隔（ADR-0032 §1「実機で確かめた事実」: `-O check` は即座に返る）。
const CHECK_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// celeris が保持する ssh master。
///
/// ADR-0060（Phase 103 の訂正）: 以前はここに `impl Drop` があり、`ClusterMaster` を落とすと
/// プロセスグループごと SIGKILL していた。celeris の通常の終了（`SIGTERM` → `tick_loop` が
/// `Ok(Exit::..)` を返す → `main()` から戻る）でもこの構造体はスタック巻き戻しで drop されるため、
/// **celeris を再起動するたびに、繋がっていたはずの master まで道連れに殺していた**（本番の観測、
/// `agent-docs/PROGRESS.md` P-100-1）。今は**Drop で殺さない**（`child` は `kill_on_drop(false)` で spawn
/// してあるので、ただ drop してもプロセスは生きたまま。reap は tokio のオーファンキューが後で行う）。
/// 明示的な切断（`DELETE /clusters/{id}/connect`）だけが [`ClusterMaster::kill`] を呼ぶ。
pub struct ClusterMaster {
    child: Child,
    /// ADR-0062 A（Phase 107）: master の stderr の蓄積（`spawn_master` の汲み出しタスクと共有）。
    /// 明示的な切断を経ずに master が終了したとき、`stderr_tail` で人に見せる手がかりにする。
    stderr_buf: Arc<Mutex<Vec<u8>>>,
}

impl std::fmt::Debug for ClusterMaster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterMaster")
            .field("pid", &self.child.id())
            .finish()
    }
}

impl ClusterMaster {
    /// 明示的な切断でだけ呼ぶ（ADR-0032 D5 `DELETE /clusters/{id}/connect`）。`ssh -O exit` が
    /// 効かなかったときの保険として、celeris が持っている子だけをプロセスグループごと落とす
    /// （ADR-0060: 通常の Drop はもう殺さない。これは意図した切断のときだけの経路）。
    pub async fn kill(mut self) {
        send_signal_to_group(&self.child, Signal::SIGKILL);
        let _ = self.child.wait().await;
    }

    /// ADR-0062 A（Phase 107）: master プロセスが終了したかを非破壊に調べる（ブロックしない）。
    /// `Some(exit_code)` なら終了済み（シグナルで落ちた場合は `exit_code == None`）。まだ生きて
    /// いる、または既に reap 済みで判定できない場合は `None` を返す（呼び出し側は「まだ生きている」
    /// として扱ってよい。celeris はこれを毎 tick 呼んで、終了した master をマップから取り除く）。
    pub fn try_wait_exit(&mut self) -> Option<Option<i32>> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.code()),
            _ => None,
        }
    }

    /// stderr の末尾 `max_bytes` バイト（UTF-8 の文字境界を壊さない）。ADR-0032 D4 のとおり、
    /// 検証コードは ssh の stderr にはそもそも入らない。
    pub fn stderr_tail(&self, max_bytes: usize) -> String {
        let buf = self
            .stderr_buf
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let text = String::from_utf8_lossy(&buf);
        if text.len() <= max_bytes {
            return text.into_owned();
        }
        let mut start = text.len() - max_bytes;
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
        text[start..].to_string()
    }
}

/// `start_connect` の結果。
#[derive(Debug)]
pub enum ClusterConnectStart {
    /// コード無しで接続できた。人が張った master を見つけた場合は `None`。
    Connected(Option<ClusterMaster>),
    /// ssh がプロンプトを出した。`prompt` をそのまま GUI に見せる。
    NeedsCode {
        prompt: String,
        session: ClusterConnectSession,
    },
}

/// 進行中の TOTP 接続セッション（ADR-0032 D4）。一時ディレクトリ・FIFO・askpass の子プロセスを保持する。
pub struct ClusterConnectSession {
    child: Option<Child>,
    ssh_command: Vec<String>,
    host: String,
    code_fifo: PathBuf,
    /// 0700 の一時ディレクトリ（FIFO と askpass スクリプトを置く）。Drop で消す。
    dir: PathBuf,
    started_at: Instant,
    stderr_buf: Arc<Mutex<Vec<u8>>>,
    err_task: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for ClusterConnectSession {
    /// プロンプト文字列はここには保持していないが、慣習として `<redacted>` を出す
    /// （`claude_account.rs::LoginSession` の `Debug` と同じ規律。ADR-0032 D4）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterConnectSession")
            .field("host", &self.host)
            .field("started_at", &self.started_at)
            .field("prompt", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl Drop for ClusterConnectSession {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            send_signal_to_group(&child, Signal::SIGKILL);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl ClusterConnectSession {
    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    /// コードを FIFO へ渡し、`wait` 以内の `-O check` の成功を待つ（ADR-0032 D4）。
    ///
    /// N6 相当: `trim` して空、または制御文字を含むコードは ssh に渡さずに `InvalidCode` を返す
    /// （`claude_account.rs::LoginSession::submit_code` と同じ注入防止）。この場合 `self` はここで
    /// 消費され、`Drop` がプロセスグループを kill し一時ディレクトリを消す（ssh には何も渡らない）。
    /// 戻り値が `None` なのは失敗ではない。ssh が `ControlPersist` で自分を切り離したため
    /// **保持すべき子プロセスが無い**という意味（接続自体は成立している。上の実機の注記を見よ）。
    pub async fn submit_code(
        mut self,
        code: &str,
        wait: Duration,
    ) -> Result<Option<ClusterMaster>, ClusterConnectError> {
        let trimmed = code.trim();
        if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) {
            return Err(ClusterConnectError::InvalidCode);
        }

        let code_fifo = self.code_fifo.clone();
        let payload = trimmed.to_string();
        let write_ok = matches!(
            tokio::task::spawn_blocking(move || write_code_fifo(&code_fifo, &payload)).await,
            Ok(Ok(()))
        );
        if !write_ok {
            let detail = self.stderr_detail();
            self.cancel().await;
            return Err(ClusterConnectError::Failed(detail));
        }

        let ssh_command = self.ssh_command.clone();
        let host = self.host.clone();
        let Some(child) = self.child.as_mut() else {
            let detail = self.stderr_detail();
            self.cancel().await;
            return Err(ClusterConnectError::Failed(detail));
        };
        let outcome = poll_until_connected_or_timeout(&ssh_command, &host, child, wait).await;
        match outcome {
            PollOutcome::Connected => Ok(self.into_master()),
            PollOutcome::TimedOut | PollOutcome::ChildExited => {
                let detail = self.stderr_detail();
                self.cancel().await;
                Err(ClusterConnectError::Failed(detail))
            }
        }
    }

    /// 進行中の接続を取り消す（ADR-0032 D5: `DELETE /clusters/{id}/connect`）。
    /// プロセスグループを落とし、一時ディレクトリを消す。
    pub async fn cancel(mut self) {
        if let Some(mut child) = self.child.take() {
            send_signal_to_group(&child, Signal::SIGKILL);
            let _ = child.wait().await;
        }
        if let Some(t) = self.err_task.take() {
            join_with_timeout(t, READER_JOIN_TIMEOUT).await;
        }
        let dir = self.dir.clone();
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&dir)).await;
    }

    /// 認証済みの child を `ClusterMaster` として取り出す。一時ディレクトリは（`self` の残りとともに）
    /// この関数を抜けたときの `Drop` で消える（child は既に取り出しているので kill はされない）。
    /// 生きている子だけを `ClusterMaster` にする。ssh が切り離した後の抜け殻を掴むと、
    /// `Drop` が既に終了したプロセスグループへ signal を送るだけの無意味な保持になる。
    fn into_master(mut self) -> Option<ClusterMaster> {
        let stderr_buf = self.stderr_buf.clone();
        self.child
            .take()
            .and_then(|child| master_if_alive(child, stderr_buf))
    }

    fn stderr_detail(&self) -> String {
        let buf = self
            .stderr_buf
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        truncate_detail(&String::from_utf8_lossy(&buf))
    }
}

/// `start_connect` の失敗。
#[derive(Debug)]
pub enum ClusterConnectError {
    Spawn(String),
    Timeout,
    Failed(String),
    InvalidCode,
}

impl std::fmt::Display for ClusterConnectError {
    /// ADR-0032 D4: **検証コードは決してここに現れない**（`InvalidCode` は値を持たず、`Failed` に入れるのは
    /// ssh の stderr だけ）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(detail) => write!(f, "could not start ssh: {detail}"),
            Self::Timeout => write!(f, "timed out waiting for ssh"),
            Self::Failed(detail) => write!(f, "ssh did not connect: {detail}"),
            Self::InvalidCode => write!(
                f,
                "the verification code was empty or contained control characters"
            ),
        }
    }
}

impl std::error::Error for ClusterConnectError {}

/// ADR-0060: master の起こし方。celeris（`celeris@<sha12>` unit）の cgroup の外で起こすかどうか。
///
/// `[[clusters]] master_launcher` の値（`"auto"` / `"systemd-run"` / `"inline"`）から
/// [`resolve_master_launcher`] が解決する。`"auto"` は環境（`systemd-run` が PATH にあるか、
/// `XDG_RUNTIME_DIR` が設定されているか）で決まる。
///
/// Phase 105: 判断そのもの（[`resolve_master_launcher`]）と argv の組み立て
/// （[`launch_master_command`]）は `crate::detach`（[`crate::detach::DetachLauncher`]）に共通化した。
/// `MasterLauncher` はここでの呼び名を保つための型別名（挙動もテストも変えていない）。
pub type MasterLauncher = crate::detach::DetachLauncher;

/// `master_launcher` の設定値と環境から、実際に使う起こし方を決める（ADR-0060）。純関数。
/// 実体は [`crate::detach::resolve_detach_launcher`]。
pub fn resolve_master_launcher(
    configured: &str,
    has_systemd_run: bool,
    has_xdg_runtime_dir: bool,
) -> MasterLauncher {
    crate::detach::resolve_detach_launcher(configured, has_systemd_run, has_xdg_runtime_dir)
}

/// `path_env`（`PATH` の値）に実行可能な `name` があるか（`which name` 相当）。
pub use crate::detach::path_has_executable;

/// 実際のプロセス環境から `systemd-run` が `PATH` にあるか調べる（`resolve_master_launcher` の
/// 呼び出し側が使う、非純粋な便利関数。判定そのものは [`resolve_master_launcher`] が純関数で持つ）。
pub use crate::detach::systemd_run_on_path;

/// 実際のプロセス環境から `XDG_RUNTIME_DIR` が（空でなく）設定されているか調べる。
pub use crate::detach::xdg_runtime_dir_is_set;

/// `launcher` に応じて、実際に spawn する `(program, args)` を組み立てる（ADR-0060）。純関数。
///
/// `Inline` はそのまま。`SystemdRun` は
/// `<program> --user --scope --quiet --unit celeris-ssh-master-<id>-<乱数> --description "celeris ssh
/// master (<id>)" -- <program> <args...>` を組み立てる（`crate::detach::wrap_command` と
/// `crate::detach::scope_unit_name` に委譲。ADR-0060）。`--scope` は指定したコマンドを exec する
/// だけなので、ssh は celeris の子プロセスのままで、cgroup だけが新しい scope に移る（環境変数は
/// `Command::envs` で渡したものがそのまま届く。exec は環境を消さない）。
fn launch_master_command(
    launcher: &MasterLauncher,
    program: &str,
    args: &[String],
    cluster_id: &str,
) -> (String, Vec<String>) {
    let unit = crate::detach::scope_unit_name("celeris-ssh-master", cluster_id);
    let description = format!("celeris ssh master ({cluster_id})");
    crate::detach::wrap_command(launcher, program, args, &unit, &description)
}

/// 接続を開始する（ADR-0032 D2/D3/D4、ADR-0060）。
///
/// 1. 既に人（または以前の celeris）が張った master が生きていれば、何もせず `Connected(None)`。
/// 2. `interactive == false`（`auth = "publickey"`）: `BatchMode=yes` で `ssh -M -N` を張り、
///    `connect_timeout` 以内に `-O check` が通れば `Connected(Some(master))`。
/// 3. `interactive == true`（`auth = "totp"`）: `SSH_ASKPASS` を使って `ssh -M -N` を張り、
///    プロンプトが出れば `NeedsCode`。`prompt_timeout` 内に出なければ、鍵だけで入れたか確かめる。
///
/// `cluster_id` は `SystemdRun` のときの scope unit 名にだけ使う。`launcher` は
/// [`resolve_master_launcher`] で解決した値を渡す。
///
/// ADR-0062 A（Phase 107）: `keepalive_secs` が 0 でなければ、master の argv に
/// `-o ServerAliveInterval=<keepalive_secs> -o ServerAliveCountMax=3 -o TCPKeepAlive=yes` を足す
/// （publickey / totp 両経路）。コマンドラインの `-o` は `~/.ssh/config` より優先されるので、
/// 人の設定を変えずに効く。sirius のように NAT / ファイアウォールの idle timeout で TCP が黙って
/// 死ぬホストでも、OS の keepalive プローブが先に切断を検出できるようにする。
///
/// ADR-0078 D1: `control_persist` は master の argv に足す `-o ControlPersist=<v>`（既定 `"yes"`、
/// 空文字なら足さない。publickey / totp 両経路）。
#[allow(clippy::too_many_arguments)]
pub async fn start_connect(
    ssh_command: &[String],
    host: &str,
    cluster_id: &str,
    launcher: &MasterLauncher,
    interactive: bool,
    prompt_timeout: Duration,
    connect_timeout: Duration,
    keepalive_secs: u64,
    control_persist: &str,
) -> Result<ClusterConnectStart, ClusterConnectError> {
    if check_master(ssh_command, host).await {
        return Ok(ClusterConnectStart::Connected(None));
    }
    if interactive {
        start_totp(
            ssh_command,
            host,
            cluster_id,
            launcher,
            prompt_timeout,
            keepalive_secs,
            control_persist,
        )
        .await
    } else {
        start_publickey(
            ssh_command,
            host,
            cluster_id,
            launcher,
            connect_timeout,
            keepalive_secs,
            control_persist,
        )
        .await
    }
}

/// ADR-0062 A: master の argv に足す keepalive の `-o` オプション（`keepalive_secs == 0` なら空）。
fn keepalive_args(keepalive_secs: u64) -> Vec<String> {
    if keepalive_secs == 0 {
        return Vec::new();
    }
    vec![
        "-o".to_string(),
        format!("ServerAliveInterval={keepalive_secs}"),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
        "-o".to_string(),
        "TCPKeepAlive=yes".to_string(),
    ]
}

/// ADR-0078 D1: master の argv に足す `-o ControlPersist=<v>`（空文字なら足さない）。
/// 既定の `"yes"` にすると、最後の client が離れても master は idle で終わらない
/// （終わるのは明示切断・keepalive による断の検出・相手側の切断だけ）。
/// コマンドラインの `-o` は `~/.ssh/config` より優先されるので、人の設定を変えずに効く。
pub fn persist_args(control_persist: &str) -> Vec<String> {
    if control_persist.is_empty() {
        return Vec::new();
    }
    vec![
        "-o".to_string(),
        format!("ControlPersist={control_persist}"),
    ]
}

/// 接続を切る。`ssh -O exit <host>` を `BatchMode=yes` で呼ぶ（ADR-0032 D5: `DELETE /clusters/{id}/connect`）。
pub async fn disconnect(ssh_command: &[String], host: &str) -> Result<(), ClusterConnectError> {
    let (program, rest) = ssh_command
        .split_first()
        .ok_or_else(|| ClusterConnectError::Spawn("empty ssh_command".to_string()))?;
    let mut args: Vec<String> = rest.to_vec();
    args.push("-o".into());
    args.push("BatchMode=yes".into());
    args.push("-O".into());
    args.push("exit".into());
    args.push(host.to_string());
    let output = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| ClusterConnectError::Spawn(e.to_string()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(ClusterConnectError::Failed(truncate_detail(
            &String::from_utf8_lossy(&output.stderr),
        )))
    }
}

/// ADR-0032 D3: `auth = "publickey"`。`BatchMode=yes` で askpass を使わずに張る。
/// ADR-0062 A: `keepalive_secs` があれば master の argv に keepalive の `-o` を足す。
async fn start_publickey(
    ssh_command: &[String],
    host: &str,
    cluster_id: &str,
    launcher: &MasterLauncher,
    connect_timeout: Duration,
    keepalive_secs: u64,
    control_persist: &str,
) -> Result<ClusterConnectStart, ClusterConnectError> {
    let (program, rest) = ssh_command
        .split_first()
        .ok_or_else(|| ClusterConnectError::Spawn("empty ssh_command".to_string()))?;
    let mut args: Vec<String> = rest.to_vec();
    args.push("-o".into());
    args.push("BatchMode=yes".into());
    args.extend(keepalive_args(keepalive_secs));
    args.extend(persist_args(control_persist));
    args.push("-M".into());
    args.push("-N".into());
    args.push(host.to_string());
    let (exec_program, exec_args) = launch_master_command(launcher, program, &args, cluster_id);

    let (mut child, stderr_buf, err_task) = spawn_master(&exec_program, &exec_args, &[])?;
    match poll_until_connected_or_timeout(ssh_command, host, &mut child, connect_timeout).await {
        PollOutcome::Connected => {
            // 接続できたので、stderr の中継タスクは master の寿命の間そのまま走らせておく
            // （もう監視する必要はない。参照を手放すだけで abort はしない）。
            drop(err_task);
            // ssh が自分を切り離した（`ControlPersist` あり）場合、この子はもう終了している。
            // そのときは持つべき master が無い（接続は切り離された側が持っている）ので `None` を返す。
            // 切るときは `ssh -O exit` を使う（`disconnect`）。
            Ok(ClusterConnectStart::Connected(master_if_alive(
                child, stderr_buf,
            )))
        }
        PollOutcome::TimedOut | PollOutcome::ChildExited => {
            let detail = finish_stderr(&mut child, stderr_buf, err_task).await;
            Err(ClusterConnectError::Failed(detail))
        }
    }
}

/// ADR-0032 D4: `auth = "totp"`。`SSH_ASKPASS` でプロンプトとコードを中継する。
/// ADR-0062 A: `keepalive_secs` があれば master の argv に keepalive の `-o` を足す。
async fn start_totp(
    ssh_command: &[String],
    host: &str,
    cluster_id: &str,
    launcher: &MasterLauncher,
    prompt_timeout: Duration,
    keepalive_secs: u64,
    control_persist: &str,
) -> Result<ClusterConnectStart, ClusterConnectError> {
    let dir =
        make_secure_tempdir().map_err(|e| ClusterConnectError::Spawn(format!("tempdir: {e}")))?;
    let prompt_fifo = dir.join("prompt");
    let code_fifo = dir.join("code");
    if let Err(e) = make_fifo(&prompt_fifo).and_then(|()| make_fifo(&code_fifo)) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    let askpass = match write_askpass_script(&dir) {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
    };

    let (program, rest) = match ssh_command.split_first() {
        Some(v) => v,
        None => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(ClusterConnectError::Spawn("empty ssh_command".to_string()));
        }
    };
    let mut args: Vec<String> = rest.to_vec();
    args.extend(keepalive_args(keepalive_secs));
    args.extend(persist_args(control_persist));
    args.push("-M".into());
    args.push("-N".into());
    args.push(host.to_string());
    // BatchMode=yes は付けない（付けると askpass が呼ばれない。ADR-0032 §1「実機で確かめた事実」）。
    let envs = vec![
        (
            "SSH_ASKPASS".to_string(),
            askpass.to_string_lossy().into_owned(),
        ),
        ("SSH_ASKPASS_REQUIRE".to_string(), "force".to_string()),
        ("DISPLAY".to_string(), String::new()),
        (
            "CELERIS_PROMPT_FIFO".to_string(),
            prompt_fifo.to_string_lossy().into_owned(),
        ),
        (
            "CELERIS_CODE_FIFO".to_string(),
            code_fifo.to_string_lossy().into_owned(),
        ),
    ];
    let (exec_program, exec_args) = launch_master_command(launcher, program, &args, cluster_id);

    let (child, stderr_buf, err_task) = match spawn_master(&exec_program, &exec_args, &envs) {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
    };

    let session = ClusterConnectSession {
        child: Some(child),
        ssh_command: ssh_command.to_vec(),
        host: host.to_string(),
        code_fifo,
        dir,
        started_at: Instant::now(),
        stderr_buf,
        err_task: Some(err_task),
    };

    // N: `read_prompt_blocking` は自前の `deadline` でポーリングして戻ってくる（`prompt_timeout` を
    // 超えて OS スレッドがブロックされたまま残ることがない）。`tokio::select!` で外側から競わせて
    // 見捨てる方式は、tokio のブロッキングスレッドが（呼び手が待つのをやめても）決して終わらないままに
    // なりうる（`ssh` が二度と askpass を呼ばない場合。実際にテストで確認した）ので採らない。
    let deadline = Instant::now() + prompt_timeout;
    let read_prompt_path = prompt_fifo;
    let read_result =
        tokio::task::spawn_blocking(move || read_prompt_blocking(&read_prompt_path, deadline))
            .await;

    match read_result {
        Ok(Ok(Some(prompt))) => Ok(ClusterConnectStart::NeedsCode { prompt, session }),
        Ok(Ok(None)) => {
            // プロンプトが所定の時間来なかった: 鍵だけで入れたかもしれないので `-O check` で判定する
            // （ADR-0032 D4 手順 3）。ここでは 1 回だけ見る（ポーリングはしない）。
            if check_master(&session.ssh_command, &session.host).await {
                match session.into_master() {
                    Some(master) => Ok(ClusterConnectStart::Connected(Some(master))),
                    None => Err(ClusterConnectError::Failed(
                        "cluster connect session lost its child process".to_string(),
                    )),
                }
            } else {
                session.cancel().await;
                Err(ClusterConnectError::Timeout)
            }
        }
        _ => {
            // FIFO を開けなかった／読めなかった: 接続の試み自体を失敗として扱う。
            let detail = session.stderr_detail();
            session.cancel().await;
            Err(ClusterConnectError::Failed(detail))
        }
    }
}

/// `spawn_master` の戻り値: 子プロセス・stderr の蓄積バッファ・それを汲み出すタスクのハンドル。
type MasterSpawn = (Child, Arc<Mutex<Vec<u8>>>, JoinHandle<()>);

/// `ssh -M -N`（または ADR-0060 の `systemd-run --user --scope -- ssh -M -N`）を起動し、stderr を
/// 非同期に汲み出す（`stdout` は使わないので捨てる）。
///
/// ADR-0060: `kill_on_drop(false)`。以前は `true` にしていたが、`ClusterMaster`/`ClusterConnectSession`
/// の側で必要なときは明示的に `send_signal_to_group` を呼んでいる（保留中の取り消し・明示的な切断）ので
/// 冗長だった上、接続成立後に `Child` が何らかの理由で drop されただけで master を巻き込んで殺す
/// 副作用があった。tokio はいずれにせよオーファンキューで reap するので、zombie は残らない。
fn spawn_master(
    program: &str,
    args: &[String],
    envs: &[(String, String)],
) -> Result<MasterSpawn, ClusterConnectError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(envs.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| ClusterConnectError::Spawn(e.to_string()))?;
    let stderr = child.stderr.take();
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let task_buf = buf.clone();
    let err_task = tokio::spawn(async move {
        if let Some(stderr) = stderr {
            pump_reader(stderr, task_buf).await;
        }
    });
    Ok((child, buf, err_task))
}

/// 失敗として終える: プロセスグループを落とし、stderr の読み取りタスクを（上限付きで）待ってから
/// 人が読める一行の手がかりにする（ADR-0032 D4: コードは含めない。ssh の stderr にはそもそも入らない）。
async fn finish_stderr(
    child: &mut Child,
    stderr_buf: Arc<Mutex<Vec<u8>>>,
    err_task: JoinHandle<()>,
) -> String {
    send_signal_to_group(child, Signal::SIGKILL);
    let _ = child.wait().await;
    join_with_timeout(err_task, READER_JOIN_TIMEOUT).await;
    let buf = stderr_buf.lock().unwrap_or_else(|e| e.into_inner()).clone();
    truncate_detail(&String::from_utf8_lossy(&buf))
}

enum PollOutcome {
    Connected,
    TimedOut,
    ChildExited,
}

/// `-O check` を `CHECK_POLL_INTERVAL` 間隔でポーリングしつつ、子の終了も同時に見る（ADR-0032 D2/D4）。
/// 子がまだ生きていれば `ClusterMaster` として保持する。既に終了していれば `None`
/// （ssh が `ControlPersist` で master を切り離した後。接続は生きているが、こちらに持ち物は無い）。
fn master_if_alive(mut child: Child, stderr_buf: Arc<Mutex<Vec<u8>>>) -> Option<ClusterMaster> {
    match child.try_wait() {
        Ok(Some(_)) => None,
        _ => Some(ClusterMaster { child, stderr_buf }),
    }
}

async fn poll_until_connected_or_timeout(
    ssh_command: &[String],
    host: &str,
    child: &mut Child,
    timeout: Duration,
) -> PollOutcome {
    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let _ = status;
            // **実機で判明（2026-09-17、sirius）**: `~/.ssh/config` に `ControlPersist` があると、
            // ssh は認証が済んだ時点で**自分をバックグラウンドへ切り離す**（master は setsid して PPID 1 になり、
            // こちらが持っていた子プロセスは終了する）。つまり「子が終了した」は失敗とは限らない。
            // 必ずもう一度 `-O check` を見てから判定する（これを見ないと、接続できているのに失敗を返す）。
            return if check_master(ssh_command, host).await {
                PollOutcome::Connected
            } else {
                PollOutcome::ChildExited
            };
        }
        if check_master(ssh_command, host).await {
            return PollOutcome::Connected;
        }
        if start.elapsed() >= timeout {
            return PollOutcome::TimedOut;
        }
        tokio::time::sleep(CHECK_POLL_INTERVAL).await;
    }
}

/// `ssh.rs::control_master_alive_blocking` をブロッキングスレッドで呼ぶ（新しい async 版は書かない。
/// 判定は一本化する。ADR-0032）。
async fn check_master(ssh_command: &[String], host: &str) -> bool {
    let ssh_command = ssh_command.to_vec();
    let host = host.to_string();
    tokio::task::spawn_blocking(move || control_master_alive_blocking(&ssh_command, &host))
        .await
        .unwrap_or(false)
}

/// 0700 の一時ディレクトリを作る（ADR-0032 D4）。
fn make_secure_tempdir() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "celeris-cluster-connect-{}",
        task_core::TaskId::new()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok(dir)
}

/// FIFO を作る。`mkfifo(3)` のモードは umask に削られるので、作った後に `chmod 0600` で確定させる。
fn make_fifo(path: &Path) -> Result<(), ClusterConnectError> {
    nix::unistd::mkfifo(
        path,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .map_err(|e| ClusterConnectError::Spawn(format!("mkfifo {}: {e}", path.display())))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| ClusterConnectError::Spawn(format!("chmod {}: {e}", path.display())))
}

/// askpass スクリプト（ADR-0032 §1「実機で確かめた事実」で検証済みの中身と等価）。
fn write_askpass_script(dir: &Path) -> Result<PathBuf, ClusterConnectError> {
    let path = dir.join("askpass.sh");
    let script =
        "#!/bin/sh\nprintf '%s' \"$1\" > \"$CELERIS_PROMPT_FIFO\"\ncat \"$CELERIS_CODE_FIFO\"\n";
    std::fs::write(&path, script)
        .map_err(|e| ClusterConnectError::Spawn(format!("askpass script: {e}")))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| ClusterConnectError::Spawn(format!("askpass permissions: {e}")))?;
    Ok(path)
}

/// プロンプト FIFO を読む。**実装上の判断**: 素朴に `File::open` + ブロッキング `read` で書くと、
/// askpass が一度も呼ばれないケース（`-O check` だけで判定がついてしまう等）で「誰も書き込まない FIFO の
/// open」が永遠にブロックし、その `spawn_blocking` のスレッドを誰も回収できなくなる
/// （tokio の runtime は drop 時にブロッキングタスクの終了を待つため、テストがハングした）。
/// そこで FIFO を `O_RDWR | O_NONBLOCK` で開く（Linux 拡張: `O_RDWR` は書き手が居なくても即座に返る。
/// 自分自身も書き手として保持することで、askpass がまだ繋がっていない間の `read` が
/// 「書き手が居ない＝EOF」と誤認されない。POSIX: 書き手が 1 つでもあれば、データが無いときの
/// `read` は `EAGAIN`）。`deadline` まで `EAGAIN` をポーリングし、来なければ `Ok(None)` で戻る
/// （呼び出し側は `-O check` で判定する）。プロンプトは askpass の 1 回の `printf` で書かれる想定
/// （ADR-0032 §1）なので、最初に読めたところで確定する。
fn read_prompt_blocking(path: &Path, deadline: Instant) -> std::io::Result<Option<String>> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::fcntl::OFlag::O_NONBLOCK.bits())
        .open(path)?;
    let mut chunk = [0u8; 4096];
    loop {
        match file.read(&mut chunk) {
            Ok(n) => return Ok(Some(String::from_utf8_lossy(&chunk[..n]).into_owned())),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}

/// コード FIFO に書いて、即座に unlink する（ADR-0032 D4: コードをディスクに残さない）。
fn write_code_fifo(path: &Path, code: &str) -> std::io::Result<()> {
    {
        let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
        file.write_all(code.as_bytes())?;
        file.flush()?;
    }
    let _ = std::fs::remove_file(path);
    Ok(())
}

#[cfg(test)]
mod tests;
