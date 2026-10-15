//! Jev（OpenRouter の Decisions API）用の `CredentialVerifier`（ADR 2026-10-15-mlsys-min-decision-assisted D4）。
//!
//! 最小費用の検証: `state` 1 語・`noul` 質問 1 つだけを送る。結果は `classify_http_status` で分類する。
//! 値は Authorization header にだけ使い、ログ・エラー文面に出さない（reqwest の Error は URL を含むので文面を使わない）。
//! `verify` は同期 trait なので、専用 thread の current-thread runtime で 1 回だけ HTTP を行う。

use std::time::Duration;

use task_dispatch::credential_resume::{classify_http_status, CredentialVerifier, VerifyOutcome};

pub struct JevVerifier {
    pub endpoint: String,
    pub model: String,
    pub timeout: Duration,
}

impl JevVerifier {
    pub fn openrouter_default() -> Self {
        Self {
            endpoint: "https://openrouter.ai/api/alpha/decisions".to_string(),
            model: "typesafe/jev-1.14".to_string(),
            timeout: Duration::from_secs(20),
        }
    }

    async fn post(&self, value: &str) -> VerifyOutcome {
        let Ok(client) = reqwest::Client::builder().timeout(self.timeout).build() else {
            return VerifyOutcome::Transient;
        };
        let body = serde_json::json!({
            "model": self.model,
            "state": "ping",
            "questions": {"ok": {"type": "noul", "instructions": "Is this a ping?", "criteria": "yes"}},
        });
        match client
            .post(&self.endpoint)
            .bearer_auth(value)
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => classify_http_status(resp.status().as_u16()),
            Err(_) => VerifyOutcome::Transient,
        }
    }
}

impl CredentialVerifier for JevVerifier {
    fn verify(&self, _secret_id: &str, value: &str) -> VerifyOutcome {
        std::thread::scope(|s| {
            s.spawn(|| {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt.block_on(self.post(value)),
                    Err(_) => VerifyOutcome::Transient,
                }
            })
            .join()
            .unwrap_or(VerifyOutcome::Transient)
        })
    }
}
