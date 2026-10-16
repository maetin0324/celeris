//! task-api への実 HTTP（`curl` を子 process として使う。新しい依存を足さない）。
//! token は環境変数名だけを受け取り、値は curl の設定を標準入力で渡す（argv・出力・台帳に出さない）。
use std::{
    io::Write,
    process::{Command, Stdio},
};

use serde_json::Value;

use crate::celeris_exec::{HttpResponse, Transport};

pub struct CurlTransport {
    base_url: String,
    token_env: String,
}

impl CurlTransport {
    /// token の値はここで読んで保持せず、要求のたびに環境から読む。値が無ければ Err（値は含めない）。
    pub fn new(base_url: &str, token_env: &str) -> Result<Self, String> {
        validate_target(base_url, token_env)?;
        if std::env::var_os(token_env).is_none() {
            return Err(format!("environment variable {token_env} is not set"));
        }
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token_env: token_env.into(),
        })
    }
}

/// URL と環境変数「名」の形だけを検査する（値は読まない。preflight も同じ検査を使う）。
pub fn validate_target(base_url: &str, token_env: &str) -> Result<(), String> {
    {
        if token_env.is_empty()
            || !token_env
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err("--token-env must be an environment variable name".into());
        }
        let Some(rest) = base_url
            .strip_prefix("http://")
            .or_else(|| base_url.strip_prefix("https://"))
        else {
            return Err("--api-url must start with http:// or https://".into());
        };
        // 実行器は `/api/v1/...` を自分で足す。path 付き（`.../api/v1` など）は二重になるので受けない。
        if rest.trim_end_matches('/').contains('/') || rest.contains(['?', '#']) {
            return Err(format!(
                "--api-url must be the server origin without a path (e.g. http://127.0.0.1:8080); \
                 the executor adds /api/v1 itself, got {base_url}"
            ));
        }
        Ok(())
    }
}

/// curl config の引用符つき文字列。
fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Transport for CurlTransport {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        let token = std::env::var(&self.token_env)
            .map_err(|_| format!("environment variable {} is not set", self.token_env))?;
        if token.contains(['\n', '\r']) {
            return Err("token contains a newline".into());
        }
        let mut cfg = format!(
            "url = {}\nrequest = {}\nheader = {}\nsilent\nshow-error\nwrite-out = \"\\n%{{http_code}}\"\n",
            q(&format!("{}{path}", self.base_url)),
            q(method),
            q(&format!("Authorization: Bearer {token}")),
        );
        if let Some(b) = body {
            // body も config に埋め込む（stdin は config 専用。argv に出さない）。
            cfg.push_str(&format!(
                "header = \"Content-Type: application/json\"\ndata = {}\n",
                q(&b.to_string())
            ));
        }
        let mut child = Command::new("curl")
            .args(["--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("curl: {e}"))?;
        child
            .stdin
            .take()
            .ok_or("curl stdin")?
            .write_all(cfg.as_bytes())
            .map_err(|e| format!("curl stdin: {e}"))?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            // stderr に token は出ない（header は stdin 経由）。
            return Err(format!(
                "curl failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let (payload, code) = text
            .rsplit_once('\n')
            .ok_or("curl: no status in response")?;
        let status: u16 = code.trim().parse().map_err(|_| "curl: bad status")?;
        let body = if payload.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(payload).unwrap_or(Value::String(payload.to_string()))
        };
        Ok(HttpResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_names_and_urls_without_leaking() {
        assert!(CurlTransport::new("http://x", "A B").is_err());
        assert!(CurlTransport::new("ftp://x", "PATH").is_err());
        // path 付きは拒否（executor が /api/v1 を足すので二重になる）。origin と末尾 / は受ける。
        let e = CurlTransport::new("http://127.0.0.1:8080/api/v1", "PATH").err();
        assert!(e.is_some_and(|e| e.contains("without a path")));
        assert!(CurlTransport::new("http://127.0.0.1:8080/api/v1/", "PATH").is_err());
        let t = CurlTransport::new("http://127.0.0.1:8080/", "PATH").expect("origin");
        assert_eq!(t.base_url, "http://127.0.0.1:8080");
        let e = CurlTransport::new("http://x", "CELERIS_NO_SUCH_TOKEN_VAR").err();
        assert!(e.is_some_and(|e| e.contains("CELERIS_NO_SUCH_TOKEN_VAR")));
    }

    #[test]
    fn quoting_escapes() {
        assert_eq!(q("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
