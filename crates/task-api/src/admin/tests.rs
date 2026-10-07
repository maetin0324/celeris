use super::*;

#[test]
fn provider_ids_reject_path_traversal_and_empty() {
    assert!(valid_provider_id("acct-b"));
    assert!(valid_provider_id("acct_2"));
    assert!(!valid_provider_id(""));
    assert!(!valid_provider_id("../escape"));
    assert!(!valid_provider_id("a/b"));
    assert!(!valid_provider_id(&"x".repeat(65)));
}

#[test]
fn create_body_fills_defaults_like_provider_config() {
    let body = ProviderCreateBody {
        kind: None,
        llm_source: None,
        credential_refs: Default::default(),
        tier_models: Default::default(),
        account_id: None,
        id: "acct-b".into(),
        adapter: "fake".into(),
        tiers: None,
        concurrency: None,
        model: None,
        env: HashMap::new(),
        account_pool: false.into(),
    };
    let file = body.into_file();
    assert_eq!(file.tiers, default_tiers());
    assert_eq!(file.concurrency, 1);
    assert_eq!(file.model, "");
    assert!(!file.account_pool.is_on());
}

#[test]
fn patch_only_overwrites_provided_fields() {
    let file = ProviderConfigFile {
        kind: None,
        llm_source: None,
        tier_models: Default::default(),
        account_id: None,
        id: "acct-b".into(),
        adapter: "fake".into(),
        tiers: vec![Tier::Standard],
        concurrency: 2,
        model: "m1".into(),
        env: HashMap::from([("K".to_string(), "v".to_string())]),
        env_from_secrets: HashMap::new(),
        account_pool: false.into(),
        command: None,
        args: None,
        settings: None,
    };
    let patch = ProviderPatchBody {
        credential_refs: None,
        tier_models: None,
        account_id: None,
        concurrency: Some(5),
        ..Default::default()
    };
    let patched = patch.apply(file.clone());
    assert_eq!(patched.concurrency, 5);
    assert_eq!(patched.tiers, vec![Tier::Standard]);
    assert_eq!(patched.model, "m1");
    assert_eq!(patched.env.get("K"), Some(&"v".to_string()));
    assert!(!patched.account_pool.is_on());

    let pool_patch = ProviderPatchBody {
        account_pool: Some(true.into()),
        ..Default::default()
    };
    assert!(pool_patch.apply(file).account_pool.is_on());
}

#[test]
fn write_then_read_round_trips() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let file = ProviderConfigFile {
        kind: None,
        llm_source: None,
        tier_models: Default::default(),
        account_id: None,
        id: "acct-b".into(),
        adapter: "claude-code".into(),
        tiers: vec![Tier::Frontier],
        concurrency: 3,
        model: "".into(),
        env: HashMap::from([("CLAUDE_CONFIG_DIR".to_string(), "/x".to_string())]),
        env_from_secrets: HashMap::from([("K".to_string(), "secret-id".to_string())]),
        account_pool: true.into(),
        command: None,
        args: None,
        settings: None,
    };
    write_provider_file(dir.path(), &file).unwrap_or_else(|e| panic!("write: {e}"));
    let read = read_provider_file(&provider_file_path(dir.path(), "acct-b"))
        .unwrap_or_else(|e| panic!("read: {e}"));
    assert_eq!(read.id, "acct-b");
    assert_eq!(read.concurrency, 3);
    assert_eq!(read.env.get("CLAUDE_CONFIG_DIR"), Some(&"/x".to_string()));
    assert_eq!(
        read.env_from_secrets.get("K"),
        Some(&"secret-id".to_string())
    );
    assert!(read.account_pool.is_on());
}

/// ADR-0026 D7 / ADR-0030 D2: `command`/`args`/`env_from_secrets` は API から書かないが、人が
/// `providers.d/<id>.toml` に手で足した値は `PATCH`（`read_provider_file` → `apply` →
/// `write_provider_file`）を経ても消えない（素通り）。
#[test]
fn patch_round_trip_preserves_hand_edited_command_and_args() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let file = ProviderConfigFile {
        kind: None,
        llm_source: None,
        tier_models: Default::default(),
        account_id: None,
        id: "opencode-qwen".into(),
        adapter: "acp".into(),
        tiers: vec![Tier::Standard],
        concurrency: 1,
        model: "qwen-local/qwen3.8-27b".into(),
        env: HashMap::new(),
        env_from_secrets: HashMap::from([("SOME_KEY".to_string(), "some-secret".to_string())]),
        account_pool: false.into(),
        command: Some("opencode".into()),
        args: Some(vec!["acp".into()]),
        settings: None,
    };
    write_provider_file(dir.path(), &file).unwrap_or_else(|e| panic!("write: {e}"));

    let path = provider_file_path(dir.path(), "opencode-qwen");
    let current = read_provider_file(&path).unwrap_or_else(|e| panic!("read: {e}"));
    assert_eq!(current.command.as_deref(), Some("opencode"));
    assert_eq!(current.args.as_deref(), Some(&["acp".to_string()][..]));
    assert_eq!(
        current.env_from_secrets.get("SOME_KEY"),
        Some(&"some-secret".to_string())
    );

    let patch = ProviderPatchBody {
        credential_refs: None,
        tier_models: None,
        account_id: None,
        concurrency: Some(2),
        ..Default::default()
    };
    let updated = patch.apply(current);
    write_provider_file(dir.path(), &updated).unwrap_or_else(|e| panic!("write: {e}"));

    let after = read_provider_file(&path).unwrap_or_else(|e| panic!("read: {e}"));
    assert_eq!(after.concurrency, 2);
    assert_eq!(
        after.command.as_deref(),
        Some("opencode"),
        "PATCH must not drop hand-edited command"
    );
    assert_eq!(
        after.args.as_deref(),
        Some(&["acp".to_string()][..]),
        "PATCH must not drop hand-edited args"
    );
    assert_eq!(
        after.env_from_secrets.get("SOME_KEY"),
        Some(&"some-secret".to_string()),
        "PATCH must not drop hand-edited env_from_secrets"
    );
}
