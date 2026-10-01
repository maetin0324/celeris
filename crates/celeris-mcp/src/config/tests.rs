use super::*;

fn addr(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

#[test]
fn default_config_has_no_listeners() {
    let cfg = McpConfig::default();
    assert!(!cfg.effective_enabled());
    assert_eq!(cfg.resolve_listeners().unwrap(), vec![]);
}

#[test]
fn sugar_listen_defaults_to_token_auth() {
    let cfg = McpConfig {
        listen: Some(addr("127.0.0.1:18200")),
        ..Default::default()
    };
    assert!(cfg.effective_enabled());
    let listeners = cfg.resolve_listeners().unwrap();
    assert_eq!(listeners.len(), 1);
    assert_eq!(listeners[0].auth, ListenerAuth::Token);
}

#[test]
fn multiple_listeners_mix_token_and_fixed_none() {
    let cfg = McpConfig {
        listeners: vec![
            McpListenerConfig {
                listen: addr("127.0.0.1:18200"),
                auth: "token".to_string(),
                client: None,
            },
            McpListenerConfig {
                listen: addr("127.0.0.1:18201"),
                auth: "none".to_string(),
                client: Some("chatgpt".to_string()),
            },
        ],
        ..Default::default()
    };
    let listeners = cfg.resolve_listeners().unwrap();
    assert_eq!(listeners.len(), 2);
    assert_eq!(listeners[0].auth, ListenerAuth::Token);
    assert_eq!(
        listeners[1].auth,
        ListenerAuth::Fixed("chatgpt".to_string())
    );
}

#[test]
fn none_on_non_loopback_is_a_config_error() {
    let cfg = McpConfig {
        listeners: vec![McpListenerConfig {
            listen: addr("0.0.0.0:18201"),
            auth: "none".to_string(),
            client: Some("chatgpt".to_string()),
        }],
        ..Default::default()
    };
    assert_eq!(
        cfg.resolve_listeners(),
        Err(McpConfigError::NoneNotLoopback {
            listen: addr("0.0.0.0:18201")
        })
    );
}

#[test]
fn none_without_client_is_a_config_error() {
    let cfg = McpConfig {
        listeners: vec![McpListenerConfig {
            listen: addr("127.0.0.1:18201"),
            auth: "none".to_string(),
            client: None,
        }],
        ..Default::default()
    };
    assert_eq!(
        cfg.resolve_listeners(),
        Err(McpConfigError::NoneWithoutClient {
            listen: addr("127.0.0.1:18201")
        })
    );
}

#[test]
fn token_with_client_is_a_config_error() {
    let cfg = McpConfig {
        listeners: vec![McpListenerConfig {
            listen: addr("127.0.0.1:18200"),
            auth: "token".to_string(),
            client: Some("chatgpt".to_string()),
        }],
        ..Default::default()
    };
    assert_eq!(
        cfg.resolve_listeners(),
        Err(McpConfigError::ClientWithoutNone {
            listen: addr("127.0.0.1:18200")
        })
    );
}

#[test]
fn unknown_auth_is_a_config_error() {
    let cfg = McpConfig {
        listeners: vec![McpListenerConfig {
            listen: addr("127.0.0.1:18200"),
            auth: "bogus".to_string(),
            client: None,
        }],
        ..Default::default()
    };
    assert_eq!(
        cfg.resolve_listeners(),
        Err(McpConfigError::UnknownAuth("bogus".to_string()))
    );
}
