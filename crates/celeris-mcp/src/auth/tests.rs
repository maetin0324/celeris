use super::*;

#[test]
fn generate_token_is_long_and_not_repeated() {
    let a = generate_token();
    let b = generate_token();
    assert_ne!(a, b);
    assert!(a.len() >= 60, "{a}");
    assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
}
use task_core::{McpClient, McpClientStore, SqliteStore};

fn store() -> SqliteStore {
    SqliteStore::open_in_memory().expect("open")
}

fn client(id: &str, token: Option<&str>, scopes: Vec<McpScope>, revoked: bool) -> McpClient {
    let now = OffsetDateTime::now_utc();
    McpClient {
        id: id.to_string(),
        name: id.to_string(),
        token_hash: token.map(hash_token),
        scopes,
        created_at: now,
        last_used_at: None,
        revoked_at: if revoked { Some(now) } else { None },
    }
}

#[test]
fn token_listener_accepts_a_matching_bearer_token() {
    let store = store();
    store
        .mcp_client_create(&client(
            "c1",
            Some("secret"),
            vec![McpScope::KnowledgeRead],
            false,
        ))
        .unwrap();
    let authed = authenticate(
        &store,
        &ListenerAuth::Token,
        Some("secret"),
        OffsetDateTime::now_utc(),
    )
    .expect("authed");
    assert_eq!(authed.id, "c1");
    assert!(authed.has_scope(McpScope::KnowledgeRead));
}

#[test]
fn token_listener_rejects_missing_wrong_or_revoked_token() {
    let store = store();
    store
        .mcp_client_create(&client("c1", Some("secret"), vec![], false))
        .unwrap();
    store
        .mcp_client_create(&client("c2", Some("secret2"), vec![], false))
        .unwrap();
    store
        .mcp_client_revoke("c2", OffsetDateTime::now_utc())
        .unwrap();
    assert_eq!(
        authenticate(
            &store,
            &ListenerAuth::Token,
            None,
            OffsetDateTime::now_utc()
        ),
        Err(AuthError::MissingToken)
    );
    assert_eq!(
        authenticate(
            &store,
            &ListenerAuth::Token,
            Some("wrong"),
            OffsetDateTime::now_utc()
        ),
        Err(AuthError::InvalidToken)
    );
    assert_eq!(
        authenticate(
            &store,
            &ListenerAuth::Token,
            Some("secret2"),
            OffsetDateTime::now_utc()
        ),
        Err(AuthError::Revoked)
    );
}

#[test]
fn a_no_token_client_can_never_authenticate_on_a_token_listener() {
    let store = store();
    store
        .mcp_client_create(&client("c1", None, vec![], false))
        .unwrap();
    // トークンが無いので、どんな値を当てても一致しない（ハッシュ比較が `NULL` に当たらない）。
    assert_eq!(
        authenticate(
            &store,
            &ListenerAuth::Token,
            Some(""),
            OffsetDateTime::now_utc()
        ),
        Err(AuthError::MissingToken)
    );
    assert!(matches!(
        authenticate(
            &store,
            &ListenerAuth::Token,
            Some("anything"),
            OffsetDateTime::now_utc()
        ),
        Err(AuthError::InvalidToken)
    ));
}

#[test]
fn fixed_listener_binds_to_the_named_client_ignoring_bearer() {
    let store = store();
    store
        .mcp_client_create(&client(
            "chatgpt",
            None,
            vec![McpScope::KnowledgePropose],
            false,
        ))
        .unwrap();
    let authed = authenticate(
        &store,
        &ListenerAuth::Fixed("chatgpt".to_string()),
        None,
        OffsetDateTime::now_utc(),
    )
    .expect("authed");
    assert_eq!(authed.id, "chatgpt");
    assert!(authed.has_scope(McpScope::KnowledgePropose));
}

#[test]
fn fixed_listener_with_unknown_or_revoked_client_fails() {
    let store = store();
    assert_eq!(
        authenticate(
            &store,
            &ListenerAuth::Fixed("nope".to_string()),
            None,
            OffsetDateTime::now_utc()
        ),
        Err(AuthError::UnknownFixedClient)
    );
    store
        .mcp_client_create(&client("chatgpt", None, vec![], false))
        .unwrap();
    store
        .mcp_client_revoke("chatgpt", OffsetDateTime::now_utc())
        .unwrap();
    assert_eq!(
        authenticate(
            &store,
            &ListenerAuth::Fixed("chatgpt".to_string()),
            None,
            OffsetDateTime::now_utc()
        ),
        Err(AuthError::Revoked)
    );
}

#[test]
fn bearer_token_parses_the_header_value() {
    assert_eq!(bearer_token(Some("Bearer abc")), Some("abc"));
    assert_eq!(bearer_token(Some("Bearer  abc")), Some("abc"));
    assert_eq!(bearer_token(Some("abc")), None);
    assert_eq!(bearer_token(Some("Bearer ")), None);
    assert_eq!(bearer_token(None), None);
}
