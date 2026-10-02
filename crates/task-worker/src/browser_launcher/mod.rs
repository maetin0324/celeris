//! ADR-0115 / ADR-0116: 権限分離 browser launcher（専用 host user `celeris-browser` の service）。
//!
//! daemon は Unix socket の固定 protocol（[`protocol`]）で session の起動・操作・観測・停止を頼み、
//! launcher（[`server`]）は `SO_PEERCRED` の許可 UID・lease・接続の持ち主を検査してから
//! [`SessionBackend`] に振り分ける。session は [`registry`] が状態 dir に starttime・instance id 付きで
//! 記録し、切断・lease 失効・stop・shutdown で process group ごと止めて回収する。daemon 側は
//! [`client`] の同期 client を使う。
//!
//! 起動の中身（userns 生成・bwrap・Chrome）は [`SessionBackend`] の実装に閉じ、この module は
//! FD・argv・path を一切受け渡さない（ADR-0116 D2/D4）。

pub mod backend;
pub mod client;
pub mod protocol;
pub mod registry;
pub mod server;
pub mod userns;

pub use client::{ClientError, LauncherClient, StartedSession};
pub use protocol::{
    ActionArgs, ErrorCode, Observation, Outcome, PROTOCOL_VERSION, Receipt, Request, Response,
    SessionBinding, SessionFacts, SessionPolicy, SessionState, Verb,
};
pub use registry::{Registry, SessionRecord};
pub use server::{
    BackendSession, Launched, LauncherLimits, LauncherServer, ServerConfig, ServerHandle,
    SessionBackend, StartRequest,
};

/// `/dev/urandom` から 16 byte を読んで 32 桁の hex にする（session id・instance id）。
pub fn random_id() -> std::io::Result<String> {
    use std::io::Read;
    let mut buf = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// UNIX epoch からの ms。
pub(crate) fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
