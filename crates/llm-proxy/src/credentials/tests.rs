use super::*;
use std::fs;

fn claude_fixture() -> serde_json::Value {
    serde_json::json!({
        "claudeAiOauth": {
            "accessToken": "fake-access-abc",
            "refreshToken": "fake-refresh-abc",
            "expiresAt": 1_800_000_000_000i64,
            "scopes": ["user:inference"],
            "subscriptionType": "max"
        }
    })
}

fn codex_fixture() -> serde_json::Value {
    serde_json::json!({
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": "fake-id",
            "access_token": "fake-access",
            "refresh_token": "fake-refresh",
            "account_id": "acct-123"
        },
        "last_refresh": "2026-09-21T00:00:00Z"
    })
}

#[test]
fn parses_claude_fixture() {
    let tokens = parse_claude_tokens(&claude_fixture()).unwrap();
    assert_eq!(tokens.access_token, "fake-access-abc");
    assert_eq!(tokens.expires_at_ms, 1_800_000_000_000);
    // Debug は値を出さない。
    assert!(!format!("{tokens:?}").contains("fake-access-abc"));
}

#[test]
fn parses_codex_fixture() {
    let tokens = parse_codex_tokens(&codex_fixture()).unwrap();
    assert_eq!(tokens.account_id, "acct-123");
    assert_eq!(tokens.access_token, "fake-access");
    assert!(!format!("{tokens:?}").contains("fake-access"));
}

#[test]
fn missing_field_is_a_shape_error() {
    let bad = serde_json::json!({"claudeAiOauth": {"accessToken": "a"}});
    assert!(matches!(
        parse_claude_tokens(&bad),
        Err(CredentialError::Shape("claudeAiOauth.refreshToken"))
    ));
}

#[test]
fn write_back_preserves_unknown_fields_and_updates_only_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(CLAUDE_CREDENTIALS_FILE);
    write_json_atomic(&path, &claude_fixture()).unwrap();
    assert!(
        !dir.path()
            .join(format!("{CLAUDE_CREDENTIALS_FILE}.tmp"))
            .exists()
    );

    let mut value = read_json(&path).unwrap();
    let new_tokens = ClaudeTokens {
        access_token: "new-access".into(),
        refresh_token: "new-refresh".into(),
        expires_at_ms: 1_900_000_000_000,
    };
    apply_claude_tokens(&mut value, &new_tokens);
    write_json_atomic(&path, &value).unwrap();

    let reloaded = read_json(&path).unwrap();
    assert_eq!(reloaded["claudeAiOauth"]["accessToken"], "new-access");
    assert_eq!(reloaded["claudeAiOauth"]["subscriptionType"], "max");
    assert_eq!(
        reloaded["claudeAiOauth"]["scopes"],
        serde_json::json!(["user:inference"])
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn codex_write_back_updates_last_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(CODEX_CREDENTIALS_FILE);
    write_json_atomic(&path, &codex_fixture()).unwrap();
    let mut value = read_json(&path).unwrap();
    let tokens = CodexTokens {
        access_token: "new-access".into(),
        refresh_token: "new-refresh".into(),
        account_id: "acct-123".into(),
        id_token: "new-id".into(),
    };
    apply_codex_tokens(&mut value, &tokens, "2026-09-21T01:00:00Z");
    write_json_atomic(&path, &value).unwrap();
    let reloaded = read_json(&path).unwrap();
    assert_eq!(reloaded["tokens"]["access_token"], "new-access");
    assert_eq!(reloaded["last_refresh"], "2026-09-21T01:00:00Z");
    assert_eq!(reloaded["OPENAI_API_KEY"], serde_json::Value::Null);
}
