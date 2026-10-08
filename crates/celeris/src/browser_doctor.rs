//! ADR 2026-10-08-browser-prod-enablement D5: bounded, read-only process-environment probes.
use std::io::{Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use task_api::browser_readiness::BrowserReadiness;
use task_core::{BrowserAction, SqliteStore, TaskStore};
use task_worker::browser_ledger::{HostAgentBrowser, ledger_status, probe_agent_browser};

use crate::Config;

const TIMEOUT: Duration = Duration::from_secs(2);

/// Explicit paths permit deterministic tests without changing the process environment.
#[derive(Clone)]
pub struct DoctorConfig {
    pub config: Config,
    pub release: Option<String>,
    pub ledger: Option<PathBuf>,
    pub agent_browser: PathBuf,
    pub bwrap: PathBuf,
    pub bin_dir: PathBuf,
    pub resolver: Option<SocketAddr>,
}

impl DoctorConfig {
    pub fn from_process(config: Config) -> Self {
        let bin_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from))
            .unwrap_or_default();
        Self {
            resolver: config
                .browser
                .egress
                .resolver
                .map(|ip| SocketAddr::new(ip, 53)),
            config,
            release: task_worker::browser_ledger::configured_release().map(str::to_owned),
            ledger: std::env::var_os("CELERIS_BROWSER_CONFORMANCE_FILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .or_else(|| task_worker::browser_ledger::configured_path().map(PathBuf::from)),
            agent_browser: PathBuf::from("agent-browser"),
            bwrap: PathBuf::from("/usr/bin/bwrap"),
            bin_dir,
        }
    }

    pub fn inspect(&self, store: &SqliteStore) -> BrowserReadiness {
        let mut report = BrowserReadiness::default();
        let host = probe_agent_browser(&self.agent_browser);
        let ledger = ledger_status(self.ledger.as_deref(), self.release.as_deref(), &host);
        let versions = ledger
            .generated_for
            .as_ref()
            .map(|g| {
                format!(
                    "release={} agent-browser={}",
                    g.celeris_release, g.agent_browser,
                )
            })
            .unwrap_or_default();
        report.push(if ledger.code.is_ok() { "OK" } else { "NG" }, "ledger", format!(
            "{} code={} {}; 修正: release を prepare/verify/promote、または browser-ledger.sh で再生成",
            ledger.code.message(), ledger.code.as_str(), versions,
        ));
        report.push(
            if ledger.credential_backends.is_empty() {
                "WARN"
            } else {
                "OK"
            },
            "ledger-backends",
            format!(
                "public={} credential={}; 修正: release の P4-B 証拠生成ログを確認して再生成",
                ledger
                    .public_backends
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(","),
                ledger
                    .credential_backends
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        );
        report.push(if matches!(&host, HostAgentBrowser::Version(v) if v == task_worker::browser::SUPPORTED_VERSION) { "OK" } else { "NG" }, "agent-browser", format!(
            "{} {:?}; 修正: daemon の PATH に agent-browser {} を配置",
            self.agent_browser.display(), host, task_worker::browser::SUPPORTED_VERSION,
        ));
        let launcher = self.config.browser.runtime == "launcher";
        report.push(
            if matches!(self.config.browser.runtime.as_str(), "daemon" | "launcher") {
                "OK"
            } else {
                "NG"
            },
            "runtime",
            format!(
                "{}; 修正: [browser] runtime を launcher または daemon に設定",
                self.config.browser.runtime,
            ),
        );
        if launcher {
            let ok = self
                .config
                .browser
                .launcher_socket
                .as_deref()
                .is_some_and(|path| {
                    task_worker::browser_launcher::LauncherClient::connect(path, TIMEOUT)
                        .and_then(|mut client| client.hello())
                        .is_ok_and(|(version, loopback)| {
                            version == task_worker::browser_launcher::PROTOCOL_VERSION
                                && loopback.is_empty()
                        })
                });
            report.push(if ok { "OK" } else { "NG" }, "launcher", "固定 Hello IPC（本番は loopback 許可なし）; 修正: launcher_socket と launcher の許可 UID・socket/service を運用セッションで確認");
        } else {
            report.push("SKIP", "launcher", "runtime=daemon");
        }
        for (name, path) in [
            ("bwrap", self.bwrap.clone()),
            ("sandboxd", self.bin_dir.join("celeris-browser-sandboxd")),
            ("egress", self.bin_dir.join("celeris-browser-egress")),
        ] {
            report.push(if launcher { "SKIP" } else if path.is_file() { "OK" } else { "NG" }, name, format!(
                "{}; 修正: bwrap と release の runtime binary を配置（launcher 構成は launcher 側で確認）", path.display(),
            ));
        }
        let dns_ok = self.resolver.is_some_and(dns_probe);
        report.push(if dns_ok { "OK" } else { "NG" }, "egress-resolver", "固定名 example.com の DNS 応答（上限2秒）; 修正: [browser.egress] resolver と UDP/53 到達性を確認");
        let broker_ok = self
            .config
            .api
            .browser_credentiald_control_socket
            .as_deref()
            .is_some_and(|path| {
                let result = (|| -> std::io::Result<bool> {
                    let mut stream = UnixStream::connect(path)?;
                    stream.set_read_timeout(Some(TIMEOUT))?;
                    stream.set_write_timeout(Some(TIMEOUT))?;
                    stream.write_all(br#"{"op":"ping"}"#)?;
                    stream.shutdown(std::net::Shutdown::Write)?;
                    let mut bytes = Vec::new();
                    (&mut stream).take(65537).read_to_end(&mut bytes)?;
                    Ok(bytes.len() <= 65536
                        && serde_json::from_slice::<celeris_credentiald::ipc::IpcReply>(&bytes)
                            .is_ok_and(|r| r.success))
                })();
                result.unwrap_or(false)
            });
        report.push(if broker_ok { "OK" } else { "NG" }, "credentiald", "control Ping（UID/PID admission を含む、秘密・登録の変更なし）; 修正: browser_credentiald_control_socket と credentiald の daemon PID 許可を確認");
        let key_ok = self
            .config
            .api
            .browser_attestation_public_key_file
            .as_deref()
            .is_some_and(|path| task_api::browser::BrowserApiConfig::read_public_key(path).is_ok());
        report.push(if key_ok { "OK" } else { "NG" }, "attestation-key", "Ed25519 public key; 修正: web の公開鍵を browser_attestation_public_key_file に設定（秘密鍵は渡さない）");
        let policies = store.browser_site_policy_list();
        match policies {
            Ok(policies) => {
                report.push(
                    if policies.is_empty() { "NG" } else { "OK" },
                    "site-policies",
                    format!(
                        "{} 件; 修正: web /browser/settings で site policy を登録",
                        policies.len()
                    ),
                );
                for record in &policies {
                    let trusted = task_api::browser::TrustedSitePolicy::from(record.policy.clone());
                    report.push(
                        if trusted.validate().is_ok() {
                            "OK"
                        } else {
                            "NG"
                        },
                        "site-policy",
                        format!(
                            "{}; 修正: web で origin・login URL・selector を確認",
                            trusted.policy_id
                        ),
                    );
                    if self.config.api.browser_site_policies.iter().any(|seed| {
                        seed.policy_id == trusted.policy_id
                            && task_core::BrowserSitePolicy::from(seed.clone()) != record.policy
                    }) {
                        report.push("WARN", "site-policy", format!("{}: config と DB が異なる（DB が正）; 修正: 取り込み済みの config の種を削除", trusted.policy_id));
                    }
                }
                match store.org_list() {
                    Ok(nodes) => {
                        let mut count = 0;
                        for node in nodes {
                            let Some(grant) = node.profile.browser else {
                                continue;
                            };
                            count += 1;
                            let credential = grant
                                .allowed_actions
                                .as_ref()
                                .is_some_and(|a| a.contains(&BrowserAction::CredentialUse));
                            let unknown = grant
                                .credential_policy_ids
                                .iter()
                                .any(|id| !policies.iter().any(|p| &p.policy.policy_id == id));
                            report.push(if unknown { "NG" } else if !credential || grant.credential_policy_ids.is_empty() { "WARN" } else { "OK" }, "grant", format!(
                                "{}: credential_use={} credential_policy_ids={} unknown_site_policy={}; 修正: web /browser/settings で credential_use と実在する site policy を設定", node.id, credential, grant.credential_policy_ids.join(","), unknown,
                            ));
                        }
                        if count == 0 {
                            report.push("NG", "grant", "browser grant が無い; 修正: web /browser/settings で browser-execution の grant を設定");
                        }
                    }
                    Err(_) => {
                        report.push("NG", "grant", "DB を読めない; 修正: daemon DB の状態を確認")
                    }
                }
            }
            Err(_) => report.push(
                "NG",
                "site-policies",
                "DB を読めない; 修正: daemon DB の状態を確認",
            ),
        }
        report
    }
}

fn dns_probe(resolver: SocketAddr) -> bool {
    let result = (|| -> std::io::Result<bool> {
        let bind = if resolver.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = UdpSocket::bind(bind)?;
        socket.set_read_timeout(Some(TIMEOUT))?;
        socket.set_write_timeout(Some(TIMEOUT))?;
        socket.connect(resolver)?;
        // One A query, recursion desired. No user supplied name or secret leaves the process.
        let query = b"\x63\x65\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01";
        socket.send(query)?;
        let mut reply = [0u8; 4096];
        let n = socket.recv(&mut reply)?;
        Ok(n >= query.len()
            && reply[..2] == query[..2]
            && reply[2] & 0x80 != 0
            && reply[3] & 0x0f == 0
            && reply[4..6] == [0, 1]
            && reply[12..query.len()] == query[12..])
    })();
    result.unwrap_or(false)
}

#[cfg(test)]
#[path = "browser_doctor_tests.rs"]
mod tests;
