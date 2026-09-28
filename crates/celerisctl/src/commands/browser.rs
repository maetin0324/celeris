//! ADR-0080 D6: `celerisctl browser owner-session approve <challenge>`。
//!
//! GUI が発行した非秘密の challenge を、GUI の Unix control socket（runtime directory 0700 / socket 0600）へ
//! 渡して、その GUI ログイン session を browser の本人（owner）に束縛する。一般 HTTP bearer API は使わない。
//! DB は開かない。cookie・cookie ID は扱わない。

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Subcommand};

use crate::error::CliError;

/// GUI と同じ環境変数（`gui/app/browser-owner.server.ts` の `OWNER_SOCKET_ENV`）。
pub const OWNER_SOCKET_ENV: &str = "CELERIS_GUI_OWNER_SOCKET";

#[derive(Subcommand, Debug)]
pub enum BrowserCommand {
    /// 本人（owner）の GUI session を確定する。
    OwnerSession {
        #[command(subcommand)]
        command: OwnerSessionCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum OwnerSessionCommand {
    /// GUI の「このセッションを本人として登録」で表示された challenge を承認する（一回限り・5 分）。
    Approve(ApproveArgs),
}

#[derive(Args, Debug)]
pub struct ApproveArgs {
    /// GUI が表示した challenge（12 桁の hex）。
    pub challenge: String,
    /// GUI の control socket。既定は `$CELERIS_GUI_OWNER_SOCKET`。
    #[arg(long)]
    pub socket: Option<PathBuf>,
}

fn valid_challenge(s: &str) -> bool {
    s.len() == 12 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// socket と親 directory が自分の所有で、group/other に開いていないこと。
fn check_socket(path: &Path) -> Result<(), CliError> {
    let uid = current_uid();
    let dir = path
        .parent()
        .ok_or_else(|| CliError::msg("owner socket path has no parent directory"))?;
    for (p, what) in [(dir, "directory"), (path, "socket")] {
        let meta = std::fs::metadata(p)
            .map_err(|e| CliError::msg(format!("cannot stat owner {what} {}: {e}", p.display())))?;
        if meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 {
            return Err(CliError::msg(format!(
                "owner {what} {} must be owned by this user and not accessible to group/other",
                p.display()
            )));
        }
    }
    Ok(())
}

fn current_uid() -> u32 {
    // std に getuid が無いので `/proc/self` の所有者（この process の uid）を使う。
    std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(u32::MAX)
}

pub fn run(command: BrowserCommand) -> Result<ExitCode, CliError> {
    match command {
        BrowserCommand::OwnerSession {
            command: OwnerSessionCommand::Approve(args),
        } => approve(args),
    }
}

fn approve(args: ApproveArgs) -> Result<ExitCode, CliError> {
    let challenge = args.challenge.trim().to_ascii_uppercase();
    if !valid_challenge(&challenge) {
        return Err(CliError::msg("challenge must be 12 hex characters"));
    }
    let socket = match args.socket {
        Some(p) => p,
        None => std::env::var_os(OWNER_SOCKET_ENV)
            .map(PathBuf::from)
            .ok_or_else(|| CliError::msg(format!("--socket or {OWNER_SOCKET_ENV} is required")))?,
    };
    check_socket(&socket)?;
    let mut stream = UnixStream::connect(&socket)
        .map_err(|e| CliError::msg(format!("cannot connect to {}: {e}", socket.display())))?;
    let timeout = Some(Duration::from_secs(5));
    stream
        .set_read_timeout(timeout)
        .and_then(|()| stream.set_write_timeout(timeout))
        .map_err(|e| CliError::msg(format!("socket setup failed: {e}")))?;
    let request = serde_json::json!({ "op": "approve", "challenge": challenge });
    stream
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|e| CliError::msg(format!("write failed: {e}")))?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| CliError::msg(format!("read failed: {e}")))?;
    let reply: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|_| CliError::msg("unexpected reply from the GUI control socket"))?;
    let code = reply
        .get("code")
        .and_then(|c| c.as_str())
        .unwrap_or("error");
    if reply.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        println!("approved: this GUI session is now the browser owner");
        Ok(ExitCode::SUCCESS)
    } else {
        Err(CliError::msg(format!(
            "owner-session approve failed: {code}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn challenge_format() {
        assert!(valid_challenge("ABCDEF012345"));
        assert!(!valid_challenge("ABCDEF01234"));
        assert!(!valid_challenge("ABCDEF01234Z"));
    }

    #[test]
    fn approve_talks_one_line_to_a_private_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = dir.path().join("run");
        std::fs::create_dir(&run).expect("mkdir");
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let sock = run.join("owner.sock");
        let listener = UnixListener::bind(&sock).expect("bind");
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        let server = std::thread::spawn(move || {
            let (conn, _) = listener.accept().expect("accept");
            let mut line = String::new();
            BufReader::new(&conn).read_line(&mut line).expect("read");
            let mut w = &conn;
            w.write_all(b"{\"ok\":true,\"code\":\"approved\"}\n")
                .expect("write");
            line
        });
        let code = approve(ApproveArgs {
            challenge: "abcdef012345".into(),
            socket: Some(sock.clone()),
        })
        .expect("approve");
        assert_eq!(code, ExitCode::SUCCESS);
        let sent = server.join().expect("join");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(sent.trim()).expect("json"),
            serde_json::json!({"op": "approve", "challenge": "ABCDEF012345"})
        );
        // group/other に開いた socket は使わない
        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o666)).expect("chmod");
        assert!(check_socket(&sock).is_err());
    }
}
