//! ADR 2026-10-08-browser-prod-enablement D1.4: 適合台帳の状態の判定（`ledger_status`）と、daemon が
//! 決めた台帳の path（`configure_conformance`）。dispatcher の gate（D2）・`celerisctl`・doctor が同じ判定を使う。
//! 決定的な file の読み取りと版の比較だけで、LLM は呼ばない。

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use task_core::browser_backend::{self, Capability};
use task_core::browser_prerequisite::BrowserPrerequisiteCode;

use crate::browser::{BROWSER_BACKEND_IDS, SUPPORTED_VERSION};

/// 台帳の `generated_for`（D1.2）。runner が書く。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedFor {
    pub celeris_release: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub celeris_sha: Option<String>,
    pub agent_browser: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
}

/// daemon が決めた台帳の path と release（process 全体の設定。`std::env::set_var` は使わない）。
#[derive(Debug, Clone)]
struct ConformanceConfig {
    path: PathBuf,
    release: Option<String>,
}

static CONFIGURED: OnceLock<ConformanceConfig> = OnceLock::new();

/// daemon が起動時に 1 回呼ぶ。後の呼び出しは無視する（最初の設定が勝つ）。`release` は
/// `--release <sha12>` で動いているときの sha12（`stale_release` の判定に使う）。
pub fn configure_conformance(path: PathBuf, release: Option<String>) {
    let _ = CONFIGURED.set(ConformanceConfig { path, release });
}

pub fn configured_path() -> Option<&'static Path> {
    CONFIGURED.get().map(|c| c.path.as_path())
}

pub fn configured_release() -> Option<&'static str> {
    CONFIGURED.get().and_then(|c| c.release.as_deref())
}

/// host の agent-browser の状態。`NotChecked` は版を見ない（試験・開発）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAgentBrowser {
    NotChecked,
    Missing,
    Version(String),
}

/// `executable --version` を 1 回起動して版を読む（5 秒で打ち切り）。tick ごとには呼ばない。
pub fn probe_agent_browser(executable: &Path) -> HostAgentBrowser {
    let Ok(mut child) = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return HostAgentBrowser::Missing;
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    let _ = stdout.read_to_string(&mut out);
                }
                if !status.success() {
                    return HostAgentBrowser::Missing;
                }
                let version = out
                    .trim()
                    .strip_prefix("agent-browser")
                    .unwrap_or(out.trim())
                    .trim()
                    .to_string();
                return HostAgentBrowser::Version(version);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return HostAgentBrowser::Missing;
            }
        }
    }
}

/// 台帳の判定結果。`public_backends`・`credential_backends` は certify された backend id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerStatus {
    pub code: BrowserPrerequisiteCode,
    pub generated_for: Option<GeneratedFor>,
    pub public_backends: BTreeSet<String>,
    pub credential_backends: BTreeSet<String>,
}

impl LedgerStatus {
    fn code(code: BrowserPrerequisiteCode) -> Self {
        Self {
            code,
            generated_for: None,
            public_backends: BTreeSet::new(),
            credential_backends: BTreeSet::new(),
        }
    }

    /// task ごとの判定: 台帳が有効でも、`credential_use` の task は credential 注入を certify された
    /// backend が無ければ `ledger_lacks_credential`。
    pub fn task_code(&self, credential_use: bool) -> BrowserPrerequisiteCode {
        if !self.code.is_ok() {
            self.code
        } else if credential_use && self.credential_backends.is_empty() {
            BrowserPrerequisiteCode::LedgerLacksCredential
        } else {
            BrowserPrerequisiteCode::Ok
        }
    }
}

/// D1.4 の表の判定。`expected_release` は daemon の release（`None` の開発 daemon では見ない）。
pub fn ledger_status(
    path: Option<&Path>,
    expected_release: Option<&str>,
    host: &HostAgentBrowser,
) -> LedgerStatus {
    let Some(path) = path else {
        return LedgerStatus::code(BrowserPrerequisiteCode::Missing);
    };
    if std::fs::symlink_metadata(path).is_err() {
        return LedgerStatus::code(BrowserPrerequisiteCode::Missing);
    }
    let Ok((results, generated_for)) = crate::browser::load_ledger(path) else {
        return LedgerStatus::code(BrowserPrerequisiteCode::Invalid);
    };
    let with_meta = |code| LedgerStatus {
        generated_for: generated_for.clone(),
        ..LedgerStatus::code(code)
    };
    if let Some(release) = expected_release
        && generated_for.as_ref().map(|g| g.celeris_release.as_str()) != Some(release)
    {
        return with_meta(BrowserPrerequisiteCode::StaleRelease);
    }
    let ledger_versions_current = generated_for
        .as_ref()
        .is_none_or(|g| g.agent_browser == SUPPORTED_VERSION)
        && results.values().all(|r| r.version == SUPPORTED_VERSION);
    if !ledger_versions_current {
        return with_meta(BrowserPrerequisiteCode::StaleAgentBrowser);
    }
    match host {
        HostAgentBrowser::NotChecked => {}
        HostAgentBrowser::Missing => {
            return with_meta(BrowserPrerequisiteCode::AgentBrowserMissing);
        }
        HostAgentBrowser::Version(v) if v != SUPPORTED_VERSION => {
            return with_meta(BrowserPrerequisiteCode::StaleAgentBrowser);
        }
        HostAgentBrowser::Version(_) => {}
    }
    let certified = |declared: BTreeSet<Capability>| -> BTreeSet<String> {
        BROWSER_BACKEND_IDS
            .into_iter()
            .filter(|id| {
                let backend = browser_backend::BackendDescriptor {
                    id: (*id).into(),
                    kind: if *id == "browser-specialist" {
                        browser_backend::BackendKind::BrowserSpecialist
                    } else {
                        browser_backend::BackendKind::ExistingLoop
                    },
                    version: SUPPORTED_VERSION.into(),
                    declared: declared.clone(),
                    enabled: true,
                };
                browser_backend::certify(&backend, results.get(*id))
                    .is_ok_and(|got| got == backend.declared)
            })
            .map(str::to_string)
            .collect()
    };
    let public = crate::browser::public_capabilities();
    let public_backends = certified(public.clone());
    let mut with_credential = public;
    with_credential.insert(Capability::CredentialInjection);
    let credential_backends = certified(with_credential);
    let code = if public_backends.is_empty() {
        BrowserPrerequisiteCode::NoConformantBackend
    } else {
        BrowserPrerequisiteCode::Ok
    };
    LedgerStatus {
        code,
        generated_for,
        public_backends,
        credential_backends,
    }
}

/// 台帳の path と、その file の `(mtime, size)`（無ければ `None`）。
type LedgerKey = (Option<PathBuf>, Option<(SystemTime, u64)>);

/// D1.4: 判定を cache し、台帳 file の `(mtime, size)` が変わったときだけ計算し直す（tick ごとに
/// process を起こさない）。host の agent-browser の版は計算し直すときだけ見る。
#[derive(Debug)]
pub struct LedgerWatch {
    source: LedgerSource,
    agent_browser: Option<PathBuf>,
    key: Option<LedgerKey>,
    status: LedgerStatus,
}

#[derive(Debug)]
enum LedgerSource {
    /// daemon の設定（`conformance_record_path` と `configured_release`）を毎回引く。
    Process,
    Fixed {
        path: Option<PathBuf>,
        release: Option<String>,
    },
}

impl LedgerWatch {
    /// 本番: daemon が `configure_conformance` した path・release と、PATH の `agent-browser`。
    pub fn from_process() -> Self {
        Self::new(LedgerSource::Process, Some(PathBuf::from("agent-browser")))
    }

    /// 試験・点検用: 固定の path・release。`agent_browser = None` は host の版を見ない。
    pub fn fixed(
        path: Option<PathBuf>,
        release: Option<String>,
        agent_browser: Option<PathBuf>,
    ) -> Self {
        Self::new(LedgerSource::Fixed { path, release }, agent_browser)
    }

    fn new(source: LedgerSource, agent_browser: Option<PathBuf>) -> Self {
        Self {
            source,
            agent_browser,
            key: None,
            status: LedgerStatus::code(BrowserPrerequisiteCode::Missing),
        }
    }

    /// 必要なら計算し直す。結果が前回と変わったら `true`（初回は常に `true`）。
    pub fn refresh(&mut self) -> bool {
        let (path, release) = match &self.source {
            LedgerSource::Process => (
                crate::browser::conformance_record_path(),
                configured_release().map(str::to_string),
            ),
            LedgerSource::Fixed { path, release } => (path.clone(), release.clone()),
        };
        let stamp = path.as_deref().and_then(|p| {
            let m = std::fs::symlink_metadata(p).ok()?;
            Some((m.modified().ok()?, m.len()))
        });
        let key = (path.clone(), stamp);
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        let first = self.key.is_none();
        self.key = Some(key);
        let host = match &self.agent_browser {
            // 台帳が無いなら host の版は見ない（process を起こさない）。
            Some(exe) if stamp.is_some() => probe_agent_browser(exe),
            _ => HostAgentBrowser::NotChecked,
        };
        let status = ledger_status(path.as_deref(), release.as_deref(), &host);
        let changed = first || status != self.status;
        self.status = status;
        changed
    }

    pub fn status(&self) -> &LedgerStatus {
        &self.status
    }
}

#[cfg(test)]
#[path = "browser_ledger_tests.rs"]
mod tests;
