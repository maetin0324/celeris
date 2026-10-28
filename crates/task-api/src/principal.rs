//! 要求の主体（principal。ADR 2026-10-27-api-principals）。
//!
//! 認証（`middleware::guard`）が bearer から主体を 1 つ決め、request の extension に載せる。handler の `by`・
//! `actor` はこの値（[`Principal::actor`]）を書く。主体の名乗りを header・本文から読むことはない（MCP の
//! client id だけは、daemon 内の MCP 専用 token を示した要求に限って header から読む）。
//!
//! | 主体 | 由来 | `actor()` |
//! |---|---|---|
//! | 人（web の BFF など） | `[[api.tokens]] principal = "human:<name>"` | `human:<name>` |
//! | 外部 operator | `[[api.tokens]] principal = "operator:<name>"` | `operator:<name>` |
//! | MCP の客 | daemon が起動ごとに作る MCP 専用 token + `X-Celeris-Mcp-Client` | `mcp:<client_id>` |
//! | CoS run | `celeris-cos-run.` の run credential（`crate::cos`） | `cos:<thread_id>` |
//! | 認証の無い構成 | token が 1 本も無い loopback 構成 | `human:local` |

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::response::IntoResponse;

/// MCP の客の id を運ぶ header。MCP 専用 token と一緒のときだけ読む。
pub const MCP_CLIENT_HEADER: &str = "x-celeris-mcp-client";

/// 要求の主体。認証だけが作る。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    Human {
        name: String,
    },
    Operator {
        name: String,
    },
    Mcp {
        client_id: String,
    },
    Cos {
        thread_id: String,
        run_id: String,
    },
    /// token が 1 本も無い（loopback だけの）構成での要求。管理系は受けない。
    Unauthenticated,
}

impl Principal {
    /// `[[api.tokens]] principal` の値を読む。config の token が名乗れるのは `human:` と `operator:` だけ。
    pub fn parse_token_principal(value: &str) -> Result<Self, String> {
        let (kind, name) = value.split_once(':').ok_or_else(|| {
            format!("principal {value:?} must be human:<name> or operator:<name>")
        })?;
        if !valid_name(name) {
            return Err(format!(
                "principal {value:?}: the name must match [a-z0-9][a-z0-9._-]{{0,63}}"
            ));
        }
        match kind {
            "human" => Ok(Self::Human { name: name.into() }),
            "operator" => Ok(Self::Operator { name: name.into() }),
            "mcp" | "cos" => Err(format!(
                "principal {value:?}: {kind} principals come from the daemon, not from [[api.tokens]]"
            )),
            _ => Err(format!(
                "principal {value:?} must be human:<name> or operator:<name>"
            )),
        }
    }

    /// `by`・`actor` に書く文字列。
    pub fn actor(&self) -> String {
        match self {
            Self::Human { name } => format!("human:{name}"),
            Self::Operator { name } => format!("operator:{name}"),
            Self::Mcp { client_id } => format!("mcp:{client_id}"),
            Self::Cos { thread_id, .. } => format!("cos:{thread_id}"),
            Self::Unauthenticated => "human:local".into(),
        }
    }

    /// config の token（人か operator）か。管理系の要求はこれか CoS か MCP の読み取りに限る。
    pub fn is_token(&self) -> bool {
        matches!(self, Self::Human { .. } | Self::Operator { .. })
    }
}

/// `principal` の名前と MCP の client id の形。
pub(crate) fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// MCP の client id の形（`mcp_clients.id` は ULID 等。大文字も許す）。
pub(crate) fn valid_client_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// MCP 専用 token の接頭辞（config の token と見分けやすくする。秘密は後ろの 64 桁）。
pub const MCP_TOKEN_PREFIX: &str = "celeris-mcp.";

/// daemon が起動ごとに作る MCP 専用 token（`/dev/urandom` の 32 byte を hex にする）。
pub fn random_mcp_token() -> std::io::Result<String> {
    use std::io::Read;
    let mut random = [0_u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let mut token = String::with_capacity(MCP_TOKEN_PREFIX.len() + 64);
    token.push_str(MCP_TOKEN_PREFIX);
    for byte in random {
        token.push_str(&format!("{byte:02x}"));
    }
    Ok(token)
}

/// config から渡す名前付き token 1 本。値は Debug に出さない。
#[derive(Clone)]
pub struct ApiToken {
    pub principal: Principal,
    pub token: String,
}

impl ApiToken {
    pub fn new(principal: Principal, token: impl Into<String>) -> Self {
        Self {
            principal,
            token: token.into(),
        }
    }
}

impl std::fmt::Debug for ApiToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiToken")
            .field("principal", &self.principal.actor())
            .field("token", &"<redacted>")
            .finish()
    }
}

/// handler が主体を受け取る extractor。guard を通った要求には必ず載っている（無ければ拒否）。
#[derive(Debug, Clone)]
pub struct Caller(pub Principal);

impl Caller {
    pub fn actor(&self) -> String {
        self.0.actor()
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = axum::response::Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Caller(
            parts
                .extensions
                .get::<Principal>()
                .cloned()
                .ok_or_else(|| crate::problem::ApiProblem::unauthorized().into_response())?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_principals_parse_and_render() {
        let web = Principal::parse_token_principal("human:web").unwrap();
        assert_eq!(web.actor(), "human:web");
        let op = Principal::parse_token_principal("operator:claude-code").unwrap();
        assert_eq!(op.actor(), "operator:claude-code");
        for bad in [
            "web",
            "human:",
            "human:Web",
            "cos:t",
            "mcp:c",
            "robot:x",
            "human:a b",
        ] {
            assert!(Principal::parse_token_principal(bad).is_err(), "{bad}");
        }
        let cos = Principal::Cos {
            thread_id: "T1".into(),
            run_id: "R1".into(),
        };
        assert_eq!(cos.actor(), "cos:T1");
        assert_eq!(
            Principal::Mcp {
                client_id: "C1".into()
            }
            .actor(),
            "mcp:C1"
        );
    }

    #[tokio::test]
    async fn caller_requires_a_principal_from_the_guard() {
        let (mut parts, _) = axum::http::Request::new(()).into_parts();
        assert!(Caller::from_request_parts(&mut parts, &()).await.is_err());
        parts.extensions.insert(Principal::Unauthenticated);
        assert_eq!(
            Caller::from_request_parts(&mut parts, &())
                .await
                .unwrap()
                .actor(),
            "human:local"
        );
    }

    #[test]
    fn token_debug_is_redacted() {
        let t = ApiToken::new(Principal::Human { name: "web".into() }, "s3cret-value");
        let text = format!("{t:?}");
        assert!(!text.contains("s3cret-value"), "{text}");
        assert!(text.contains("human:web"), "{text}");
    }
}
