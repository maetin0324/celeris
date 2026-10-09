//! Supervisor-only credential segment (ADR-0080 D2/D3). The harness never sees a lease,
//! binding token or secret: it only learns `success` or a fixed failure code.
#[cfg(test)]
use std::os::fd::{AsRawFd, OwnedFd};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(test)]
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
#[cfg(test)]
use std::time::Duration;

use celeris_credentiald::{CredentialPolicy, CredentialRef, LeaseRequest, ipc};
use serde_json::json;
use task_core::browser_wait::ConsumedBrowserApproval;
use zeroize::Zeroizing;

/// Lease TTL (ADR-0080 D2: default 60 s, never above 300 s).
const LEASE_TTL_SECS: u64 = 60;

/// Trusted control channel to celeris-credentiald. Only the daemon process may use it.
pub trait CredentialLeaseBroker: Send + Sync {
    /// Registers the task/run/session/origin binding and returns the unpredictable token.
    fn bind(
        &self,
        binding: celeris_credentiald::Binding,
    ) -> Result<Zeroizing<String>, &'static str>;
    fn grant(&self, request: LeaseRequest) -> Result<String, &'static str>;
    fn revoke(&self, lease_id: &str, actor_id: &str);
}

pub struct UnixLeaseBroker {
    pub control_socket: PathBuf,
}

impl UnixLeaseBroker {
    fn call(&self, request: serde_json::Value) -> Result<ipc::IpcReply, &'static str> {
        let bytes = Zeroizing::new(serde_json::to_vec(&request).map_err(|_| "invalid_request")?);
        let reply = ipc::call(&self.control_socket, &bytes).map_err(|e| e.code())?;
        if reply.success {
            Ok(reply)
        } else {
            Err("broker_denied")
        }
    }
}

impl CredentialLeaseBroker for UnixLeaseBroker {
    fn bind(
        &self,
        binding: celeris_credentiald::Binding,
    ) -> Result<Zeroizing<String>, &'static str> {
        self.call(json!({"op":"bind","binding":binding}))?
            .binding_token
            .map(Zeroizing::new)
            .ok_or("broker_denied")
    }
    fn grant(&self, request: LeaseRequest) -> Result<String, &'static str> {
        self.call(json!({"op":"grant","request":request}))?
            .lease_id
            .ok_or("broker_denied")
    }
    fn revoke(&self, lease_id: &str, actor_id: &str) {
        let _ = self.call(json!({"op":"revoke","lease_id":lease_id,"actor_id":actor_id}));
    }
}

/// Where the supervisor finds credentiald. Configured once by the daemon at startup.
#[derive(Clone)]
pub struct CredentialSupervisor {
    pub broker: Arc<dyn CredentialLeaseBroker>,
    /// `celeris-credentiald` executable, run as `bridge` by the substrate's plugin host.
    pub bridge: PathBuf,
    /// `XDG_RUNTIME_DIR` under which credentiald serves `celeris-credentiald/resolve.sock`.
    /// `None` inherits the daemon's own value.
    pub runtime_dir: Option<PathBuf>,
}

static CONFIGURED: OnceLock<CredentialSupervisor> = OnceLock::new();

/// Called by the daemon. Later calls are ignored (the first configuration wins).
pub fn configure(supervisor: CredentialSupervisor) {
    let _ = CONFIGURED.set(supervisor);
}

pub fn configured() -> Option<&'static CredentialSupervisor> {
    CONFIGURED.get()
}

/// Read the non-secret login intent from the broker's registered site policy.
pub(crate) fn describe_policy(
    sup: &CredentialSupervisor,
    reference: &task_core::browser_wait::CredentialRef,
    origin: &str,
) -> Result<task_core::browser_wait::TrustedLogin, &'static str> {
    let runtime = sup
        .runtime_dir
        .clone()
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from))
        .ok_or("policy_changed")?;
    let request = serde_json::to_vec(&json!({
        "op":"describe_policy", "reference":reference,
        "origin":origin,
    }))
    .map_err(|_| "policy_changed")?;
    let reply = ipc::call(&runtime.join("celeris-credentiald/control.sock"), &request)
        .map_err(|_| "policy_changed")?;
    if !reply.success {
        return Err("policy_changed");
    }
    reply.trusted_login.ok_or("policy_changed")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the substrate commands need; no secret lives here.
#[cfg(test)]
pub(crate) struct Segment<'a> {
    pub executable: &'a Path,
    pub credentiald_runtime: Option<&'a Path>,
    pub runtime: &'a Path,
    pub session_id: &'a str,
    pub allowed_domains: &'a [String],
}

/// Credential-segment action policy: lifecycle, the trusted login navigation and the two
/// credential actions. Never shown to or reused by the harness.
pub(crate) fn segment_policy() -> Vec<u8> {
    let mut allow = vec![
        "auth_login",
        "close",
        "launch",
        "navigate",
        "url",
        task_core::browser::CREDENTIAL_PLUGIN_ACTION,
    ];
    allow.sort_unstable();
    serde_json::to_vec(&json!({"default":"deny","allow":allow})).unwrap_or_default()
}

#[cfg(test)]
fn private_pipe_with(token: &str) -> std::io::Result<OwnedFd> {
    use std::io::Write;
    let (read, write) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC)?;
    let mut writer = std::fs::File::from(write);
    // 64 hex bytes fit in the pipe buffer; the write end closes before the child starts.
    writer.write_all(token.as_bytes())?;
    drop(writer);
    Ok(read)
}

#[cfg(test)]
fn login_argv<'a>(lease_id: &'a str, login_url: &'a str) -> [&'a str; 10] {
    [
        "auth",
        "login",
        "celeris-credential",
        "--credential-provider",
        "celeris-credential",
        "--item",
        lease_id,
        "--no-navigate",
        "--url",
        login_url,
    ]
}

#[cfg(test)]
async fn substrate(
    seg: &Segment<'_>,
    command: &[&str],
    token: Option<&str>,
) -> Result<serde_json::Value, &'static str> {
    let mut cmd = tokio::process::Command::new(seg.executable);
    cmd.env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .env("AGENT_BROWSER_NAMESPACE", "celeris")
        .current_dir(seg.runtime)
        .arg("--config")
        .arg(seg.runtime.join("upstream.json"))
        .arg("--session")
        .arg(seg.session_id)
        .arg("--action-policy")
        .arg(seg.runtime.join("policy.json"))
        .arg("--allowed-domains")
        .arg(
            seg.allowed_domains
                .iter()
                .filter_map(|d| crate::browser_policy::origin_host_port(d))
                .map(|(host, _)| host)
                .collect::<Vec<_>>()
                .join(","),
        )
        .args(["--content-boundaries", "--max-output", "16000", "--json"])
        .args(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match seg.credentiald_runtime {
        Some(dir) => {
            cmd.env("XDG_RUNTIME_DIR", dir);
        }
        None => {
            if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
                cmd.env("XDG_RUNTIME_DIR", xdg);
            }
        }
    }
    let pipe = token.map(private_pipe_with).transpose().map_err(|_| "io")?;
    if let Some(fd) = pipe.as_ref().map(|p| p.as_raw_fd()) {
        // SAFETY: only async-signal-safe dup2/fcntl run between fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                if fd == 3 {
                    let flags = libc_fcntl(3, nix::libc::F_GETFD, 0)?;
                    libc_fcntl(3, nix::libc::F_SETFD, flags & !nix::libc::FD_CLOEXEC)?;
                } else if nix::libc::dup2(fd, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let child = cmd.spawn().map_err(|_| "substrate_unavailable")?;
    drop(pipe);
    let output = tokio::time::timeout(Duration::from_secs(45), child.wait_with_output())
        .await
        .map_err(|_| "substrate_timeout")?
        .map_err(|_| "substrate_unavailable")?;
    // Never parse beyond the fixed shape; stdout may reflect page content.
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|_| "substrate_failed")?;
    if !output.status.success() || value.get("success") != Some(&serde_json::Value::Bool(true)) {
        return Err("substrate_failed");
    }
    Ok(value)
}

#[cfg(test)]
fn libc_fcntl(fd: i32, cmd: i32, arg: i32) -> std::io::Result<i32> {
    // SAFETY: plain fcntl on a known descriptor.
    let rc = unsafe { nix::libc::fcntl(fd, cmd, arg) };
    if rc < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

/// Issue a lease for the pinned site policy without starting the retired bridge.
/// The lease is consumed only by the injection IPC after its target checks.
///
/// `live_session_id` is the session registered with credentiald (`register_live_session`), which
/// every injection request names and `consume_for_injection` compares with the lease. On the
/// daemon runtime it is the wait's session; on the launcher runtime it is the launcher-assigned
/// session id (ADR 2026-10-09 付記「launcher の Authenticate 経路」1).
pub(crate) fn grant_h3_lease(
    sup: &CredentialSupervisor,
    approval: &ConsumedBrowserApproval,
    task_id: &str,
    live_session_id: &str,
) -> Result<String, &'static str> {
    let wait = &approval.wait;
    let trusted = approval
        .trusted_login
        .as_ref()
        .ok_or("trusted_selector_missing")?;
    approval.injection_selector(None)?;
    if trusted.revision != approval.credential.credential_revision
        || trusted.policy_id != approval.credential.policy_id
    {
        return Err("policy_changed");
    }
    let now = unix_now();
    let approval_expires = u64::try_from(wait.deadline.unix_timestamp()).unwrap_or(0);
    let expires = (now + LEASE_TTL_SECS).min(approval_expires.max(now + 1));
    let policy_hash = wait
        .policy_hash
        .strip_prefix("sha256:")
        .unwrap_or(&wait.policy_hash)
        .to_string();
    let _binding = sup.broker.bind(celeris_credentiald::Binding {
        token: String::new(),
        task_id: task_id.into(),
        run_id: wait.run_id.clone(),
        session_id: live_session_id.into(),
        exact_origin: wait.origin.clone(),
        policy_hash: policy_hash.clone(),
        expires_at: expires,
    })?;
    sup.broker.grant(LeaseRequest {
        reference: CredentialRef {
            credential_id: approval.credential.credential_id.clone(),
            provider: approval.credential.provider.clone(),
            policy_id: approval.credential.policy_id.clone(),
        },
        policy: CredentialPolicy {
            policy_id: trusted.policy_id.clone(),
            revision: trusted.revision,
            exact_origin: wait.origin.clone(),
            task_id: task_id.into(),
            max_ttl_seconds: LEASE_TTL_SECS,
            require_approval: true,
            allow_persistence: false,
            login_url: Some(trusted.login_url.clone()),
            password_selector: Some(trusted.password_selector.clone()),
            submit_selector: trusted.submit_selector.clone(),
        },
        credential_revision: approval.credential.credential_revision,
        task_id: task_id.into(),
        run_id: wait.run_id.clone(),
        session_id: live_session_id.into(),
        approval_id: wait.approval_id.clone().ok_or("approval record missing")?,
        approved_by: approval.approved_by.clone(),
        policy_hash,
        idempotency_key: format!("lease-{}", wait.wait_id),
        ttl_seconds: LEASE_TTL_SECS,
        approval_expires_at: approval_expires,
        session_expires_at: expires,
    })
}

/// Upstream config for the credential segment: the only plugin is the fixed bridge.
#[cfg(test)]
pub(crate) fn segment_upstream_config(bridge: &Path) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "idleTimeout": "5m",
        "noWebmcp": true,
        "plugins": [{"name":"celeris-credential", "command":bridge,
                     "args":["bridge"], "capabilities":["credential.read"]}],
    }))
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_is_an_explicit_credential_read_provider() {
        let config: serde_json::Value =
            serde_json::from_slice(&segment_upstream_config(Path::new("/bin/bridge"))).unwrap();
        assert_eq!(
            config["plugins"],
            json!([{
                "name":"celeris-credential", "command":"/bin/bridge",
                "args":["bridge"], "capabilities":["credential.read"]
            }])
        );
    }

    #[test]
    fn auth_login_uses_name_and_item_reference() {
        assert_eq!(
            login_argv("lease-1", "https://example.com/"),
            [
                "auth",
                "login",
                "celeris-credential",
                "--credential-provider",
                "celeris-credential",
                "--item",
                "lease-1",
                "--no-navigate",
                "--url",
                "https://example.com/",
            ]
        );
    }

    #[test]
    fn origin_probe_is_explicitly_allowed_but_observation_is_denied() {
        let value: serde_json::Value = serde_json::from_slice(&segment_policy()).unwrap();
        assert_eq!(value["default"], "deny");
        let allow = value["allow"].as_array().unwrap();
        assert!(allow.contains(&json!("url")));
        assert!(!allow.contains(&json!("snapshot")));
        assert!(!allow.contains(&json!("gettext")));
    }

    #[tokio::test]
    async fn session_starter_receives_private_fd3_and_uses_stable_paths() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("fake-browser.py");
        std::fs::write(
            &executable,
            concat!(
                "#!/usr/bin/env python3\n",
                "import json, os, sys\n",
                "args = sys.argv[1:]\n",
                "assert args[args.index('--config') + 1].endswith('/upstream.json')\n",
                "assert args[args.index('--action-policy') + 1].endswith('/policy.json')\n",
                "assert os.read(3, 128) == b'binding-token'\n",
                "print(json.dumps({'success': True, 'data': {'url': 'https://example.com/'}}))\n",
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let domains = vec!["example.com".to_owned()];
        let segment = Segment {
            executable: &executable,
            credentiald_runtime: None,
            runtime: temp.path(),
            session_id: "session-test",
            allowed_domains: &domains,
        };
        assert!(
            substrate(
                &segment,
                &["open", "https://example.com/"],
                Some("binding-token")
            )
            .await
            .is_ok()
        );
    }

    #[test]
    fn export_worker_settings_for_real_browser_check() {
        let Some(output) = std::env::var_os("BROWSER_WIRING_EXPORT") else {
            return;
        };
        let output = PathBuf::from(output);
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(
            output.join("upstream.json"),
            segment_upstream_config(Path::new("/configured/by/check/celeris-credentiald")),
        )
        .unwrap();
        std::fs::write(output.join("policy.json"), segment_policy()).unwrap();
        std::fs::write(
            output.join("auth-argv.json"),
            serde_json::to_vec(&login_argv("LEASE", "ORIGIN/")).unwrap(),
        )
        .unwrap();
    }
}
