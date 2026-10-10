mod common;
use celeris_credentiald::{CredentialRef, ManualProvider};
use common::*;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::json;
use std::sync::Arc;
use task_api::browser::{
    BrokerFailure, BrokerReceipt, BrowserApiConfig, CredentialBrokerControl, ManualRegistration,
};
use task_api::browser_credentials::SavedCredentialItem;

struct Vault(ManualProvider);
impl CredentialBrokerControl for Vault {
    fn register(&self, _: ManualRegistration) -> Result<BrokerReceipt, BrokerFailure> {
        unreachable!()
    }
    fn verify_receipt(&self, _: &str, _: &BrokerReceipt) -> Result<bool, BrokerFailure> {
        unreachable!()
    }
    fn list_saved(&self, owner: &str) -> Result<Vec<SavedCredentialItem>, BrokerFailure> {
        Ok(self
            .0
            .list_owned(owner)
            .unwrap()
            .into_iter()
            .map(|(r, created_at, expires_at)| SavedCredentialItem {
                credential_id: r.credential_id,
                policy_id: r.policy_id,
                created_at,
                expires_at,
            })
            .collect())
    }
    fn delete_saved(&self, owner: &str, id: &str) -> Result<(), BrokerFailure> {
        self.0
            .remove_owned(
                &CredentialRef {
                    credential_id: id.into(),
                    provider: "manual".into(),
                    policy_id: "site".into(),
                },
                owner,
            )
            .map_err(|_| BrokerFailure::Rejected("denied"))
    }
}

#[tokio::test]
async fn saved_credential_management_is_attested_owner_scoped_and_removes_the_vault_entry() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(
        dir.path(),
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
    )
    .unwrap();
    let manual = ManualProvider::open(dir.path().join("keys"), dir.path().join("vault")).unwrap();
    manual.initialize_key().unwrap();
    let reference = CredentialRef {
        credential_id: "saved".into(),
        provider: "manual".into(),
        policy_id: "site".into(),
    };
    manual
        .register_owned(
            &reference,
            &celeris_credentiald::CredentialPolicy {
                policy_id: "site".into(),
                revision: 1,
                exact_origin: "https://example.com".into(),
                task_id: "old-task".into(),
                max_ttl_seconds: 60,
                require_approval: true,
                allow_persistence: false,
                login_url: Some("https://example.com/login".into()),
                password_selector: Some("#password".into()),
                submit_selector: None,
                username_selector: None,
                post_login: None,
                consent: None,
            },
            1,
            &celeris_credentiald::SecretEnvelope {
                username: "SECRET-USER".into(),
                password: "SECRET-PASSWORD".into(),
            },
            Some("owner"),
        )
        .unwrap();
    let env = admin_env();
    let key = Ed25519KeyPair::from_seed_unchecked(&[17; 32]).unwrap();
    let app = task_api::router(
        env.state
            .clone()
            .with_browser(BrowserApiConfig {
                attestation_public_key: Some(key.public_key().as_ref().to_vec()),
                broker: Some(Arc::new(Vault(manual.clone()))),
            })
            .with_clock(Arc::new(|| 1_800_000_000)),
    );
    let base = "/api/v1/browser/credentials";
    assert_eq!(send(&app, get_admin(base)).await.status, 403);
    let headers = |owner: &str, purpose: &str, id: Option<&str>, expires: i64| {
        let payload = json!({"purpose":purpose,"owner_session_id":"session","owner_session":true,
            "actor_id":owner,"credential_id":id,"expires_at":expires})
        .to_string();
        let sig: String = key
            .sign(payload.as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        vec![
            ("authorization", "Bearer s3cret-token-value".to_owned()),
            ("x-celeris-assertion-payload", payload),
            ("x-celeris-assertion-signature", sig),
        ]
    };
    for (owner, count) in [("owner", 1), ("other-owner", 0)] {
        let h = headers(owner, "credential_list", None, 1_800_000_020);
        let h: Vec<_> = h.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let result = send(&app, get_with(base, &h)).await;
        assert_eq!(result.status, 200);
        assert_eq!(result.header("cache-control"), Some("no-store"));
        let body = result.json();
        assert_eq!(body["items"].as_array().unwrap().len(), count);
        for secret in ["SECRET-USER", "SECRET-PASSWORD", "ciphertext"] {
            assert!(!body.to_string().contains(secret));
        }
    }
    for (owner, purpose, id, expiry, status) in [
        (
            "owner",
            "credential_list",
            Some("saved"),
            1_800_000_020,
            403,
        ),
        (
            "owner",
            "credential_delete",
            Some("another-id"),
            1_800_000_020,
            403,
        ),
        (
            "owner",
            "credential_delete",
            Some("saved"),
            1_799_999_999,
            403,
        ),
        (
            "other-owner",
            "credential_delete",
            Some("saved"),
            1_800_000_020,
            503,
        ),
        (
            "owner",
            "credential_delete",
            Some("saved"),
            1_800_000_020,
            204,
        ),
    ] {
        let h = headers(owner, purpose, id, expiry);
        let h: Vec<_> = h.iter().map(|(k, v)| (*k, v.as_str())).collect();
        assert_eq!(
            send(&app, delete_with(&format!("{base}/saved"), &h))
                .await
                .status,
            status
        );
    }
    assert!(manual.list_owned("owner").unwrap().is_empty());
    assert!(
        manual
            .describe_registered(&reference, "https://example.com", None)
            .is_err()
    );
}
