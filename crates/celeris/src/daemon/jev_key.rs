//! Jev の API key を secrets dir から読む `ApiKeyProvider`（ADR 2026-10-15-jev-wire D2）。
//!
//! 呼び出しのたびにファイルを読み直す（人の登録・再登録が再起動なしで次の分類に反映される）。
//! 未登録・空・読めない場合は `None`（`JevError::KeyMissing` → 既存の credential 待ちへ）。値は log・Debug・エラーに出さない。

#[cfg(test)]
#[allow(clippy::duplicate_mod)] // 同じ支援 file を試験 crate ごとに読む
#[path = "../../../../tests/support/http_async.rs"]
mod fake_http;

use std::fmt;
use std::path::PathBuf;

use jev_classifier::{ApiKey, ApiKeyProvider};

pub struct SecretsDirKeyProvider {
    secrets_dir: Option<PathBuf>,
    secret_id: String,
}

impl SecretsDirKeyProvider {
    pub fn new(secrets_dir: Option<PathBuf>, secret_id: impl Into<String>) -> Self {
        Self {
            secrets_dir,
            secret_id: secret_id.into(),
        }
    }
}

impl fmt::Debug for SecretsDirKeyProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretsDirKeyProvider")
            .field("secret_id", &self.secret_id)
            .finish_non_exhaustive()
    }
}

impl ApiKeyProvider for SecretsDirKeyProvider {
    fn api_key(&self) -> Option<ApiKey> {
        let dir = self.secrets_dir.as_deref()?;
        let text = std::fs::read_to_string(dir.join(&self.secret_id)).ok()?;
        let value = text.trim();
        if value.is_empty() {
            None
        } else {
            Some(ApiKey::new(value))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::classify_stage::ClassificationStage;
    use jev_classifier::{ClassifyInput, JevClassifier, JevConfig};
    use std::sync::Arc;
    use task_core::route_select::ClassifierStatus;
    use task_dispatch::{ClassificationBoard, CredentialGate};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    const SECRET: &str = "sk-test-SECRET-value-123456";

    #[test]
    fn unregistered_registered_and_reregistered_are_read_each_call() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = SecretsDirKeyProvider::new(Some(dir.path().to_path_buf()), "jev_api_key");
        assert!(p.api_key().is_none());
        std::fs::write(dir.path().join("jev_api_key"), format!("{SECRET}\n")).expect("w");
        assert!(p.api_key().is_some());
        std::fs::write(dir.path().join("jev_api_key"), "  \n").expect("w");
        assert!(p.api_key().is_none(), "empty is unregistered");
        std::fs::write(dir.path().join("jev_api_key"), "sk-other").expect("w");
        assert!(p.api_key().is_some());
        let none = SecretsDirKeyProvider::new(None, "jev_api_key");
        assert!(none.api_key().is_none());
    }

    #[test]
    fn value_never_appears_in_debug_output() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(dir.path().join("jev_api_key"), SECRET).expect("w");
        let p = SecretsDirKeyProvider::new(Some(dir.path().to_path_buf()), "jev_api_key");
        let key = p.api_key().expect("key");
        for s in [format!("{p:?}"), format!("{p:#?}"), format!("{key:?}")] {
            assert!(!s.contains(SECRET), "leaked: {s}");
        }
        assert!(format!("{key:?}").contains("redacted"));
    }

    /// 1 回だけ応答する fake server。受けた request 全文と status を返す。
    async fn serve_once(status: u16) -> (String, tokio::task::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = l.local_addr().expect("addr");
        let h = tokio::spawn(async move {
            let (mut sock, request) = fake_http::accept(&l, "POST", "/decisions").await;
            let resp =
                format!("HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            let _ = sock.write_all(resp.as_bytes()).await;
            request
        });
        (format!("http://{addr}/decisions"), h)
    }

    fn input() -> ClassifyInput {
        ClassifyInput {
            instruction: "fix".into(),
            acceptance: vec![],
            required_artifacts: vec![],
            dependency_summaries: vec![],
            recent_failure: None,
        }
    }

    async fn run_stage(
        status: Option<u16>,
        register_key: bool,
    ) -> (task_dispatch::BoardItem, Option<String>) {
        let dir = tempfile::tempdir().expect("tmp");
        if register_key {
            std::fs::write(dir.path().join("jev_api_key"), SECRET).expect("w");
        }
        let (endpoint, handle) = match status {
            Some(s) => {
                let (e, h) = serve_once(s).await;
                (e, Some(h))
            }
            None => ("http://127.0.0.1:9/decisions".to_string(), None),
        };
        let cfg = JevConfig {
            endpoint,
            max_retries: 0,
            ..JevConfig::default()
        };
        let classifier = Arc::new(JevClassifier::new(cfg).expect("classifier"));
        let keys = Arc::new(SecretsDirKeyProvider::new(
            Some(dir.path().to_path_buf()),
            "jev_api_key",
        ));
        let board = ClassificationBoard::default();
        let stage = ClassificationStage::new(
            classifier,
            keys,
            CredentialGate {
                secret_id: "jev_api_key".into(),
                provider: "jev".into(),
                purpose: "Jev で依頼を分類する".into(),
            },
            board,
        );
        let item = stage.classify_subject("wu-1", &input()).await;
        let seen = match handle {
            Some(h) => Some(h.await.expect("server")),
            None => None,
        };
        (item, seen)
    }

    #[tokio::test]
    async fn unregistered_key_enters_credential_wait_without_a_request() {
        let (item, _) = run_stage(None, false).await;
        assert!(item.credential.is_some());
        let dbg = format!("{item:?}");
        assert!(!dbg.contains(SECRET));
    }

    #[tokio::test]
    async fn auth_failures_401_403_request_credential_and_the_key_is_sent_as_bearer() {
        for status in [401u16, 403] {
            let (item, seen) = run_stage(Some(status), true).await;
            assert!(item.credential.is_some(), "{status} is a key problem");
            let seen = seen.expect("request");
            assert!(seen.contains(&format!("Bearer {SECRET}")));
            let dbg = format!("{item:?}");
            assert!(!dbg.contains(SECRET), "leaked in board item: {dbg}");
        }
    }

    #[tokio::test]
    async fn rate_limit_429_and_server_errors_do_not_request_credential() {
        for status in [429u16, 402, 500] {
            let (item, _) = run_stage(Some(status), true).await;
            assert!(
                item.credential.is_none(),
                "{status} must not ask the human to re-register the key"
            );
            assert_ne!(item.entry.status, ClassifierStatus::JevOk);
            assert!(!format!("{item:?}").contains(SECRET));
        }
    }
}
