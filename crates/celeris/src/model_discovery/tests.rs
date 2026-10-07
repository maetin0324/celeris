use super::*;
use task_core::SqliteStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// path ごとの固定応答を返す偽 HTTP server。受けた要求の頭（ヘッダ込み）を `seen` に溜める。
async fn serve(
    routes: Vec<(&'static str, u16, String)>,
) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let routes = routes.clone();
            let seen = Arc::clone(&seen2);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .split('?')
                    .next()
                    .unwrap_or("")
                    .to_string();
                seen.lock().unwrap().push(head);
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map(|(_, s, b)| (*s, b.clone()))
                    .unwrap_or((404, "{}".to_string()));
                let resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), seen)
}

/// ETXTBSY 対策（ADR-0010 D10）で別プロセスに書かせる。
fn write_executable(path: &Path, script: &str) {
    use std::io::Write;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(format!("#!/bin/sh\n{script}\n").as_bytes())
        .unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

fn ids(models: &[DiscoveredModel]) -> Vec<&str> {
    models.iter().map(|m| m.model_id.as_str()).collect()
}

#[tokio::test]
async fn opencode_go_uses_the_gateway_models_url() {
    let (base, _) = serve(vec![(
        "/zen/go/v1/models",
        200,
        r#"{"object":"list","data":[{"id":"glm-5"},{"id":"kimi-k2.5"},{"id":"minimax-m2.5"}]}"#
            .to_string(),
    )])
    .await;
    let target = DiscoveryTarget::OpencodeGo {
        models_url: format!("{base}/zen/go/v1/models"),
        cli_command: "/nonexistent/opencode".into(),
    };
    let got = discover_one(&target).await.unwrap();
    assert_eq!(ids(&got), ["glm-5", "kimi-k2.5", "minimax-m2.5"]);
}

#[tokio::test]
async fn opencode_go_falls_back_to_the_cli_stdout() {
    let (base, _) = serve(vec![("/zen/go/v1/models", 500, "{}".to_string())]).await;
    let dir = tempfile::tempdir().unwrap();
    let cli = dir.path().join("opencode");
    write_executable(
        &cli,
        r#"[ "$1 $2" = "models opencode-go" ] || exit 2
echo "INFO booting"
echo "opencode-go/glm-5"
echo "opencode-go/kimi-k2.5"
echo "other/skip""#,
    );
    let target = DiscoveryTarget::OpencodeGo {
        models_url: format!("{base}/zen/go/v1/models"),
        cli_command: cli.to_string_lossy().into_owned(),
    };
    let got = discover_one(&target).await.unwrap();
    assert_eq!(ids(&got), ["glm-5", "kimi-k2.5"]);
}

#[tokio::test]
async fn opencode_go_reports_both_failures_without_touching_the_catalog() {
    let (base, _) = serve(vec![("/m", 503, "{}".to_string())]).await;
    let target = DiscoveryTarget::OpencodeGo {
        models_url: format!("{base}/m"),
        cli_command: "/nonexistent/opencode".into(),
    };
    let store = SqliteStore::open_in_memory().unwrap();
    let src = target.source();
    store
        .model_catalog_apply(&src, &[DiscoveredModel::new("old")], 1)
        .unwrap();
    let out = run_targets(&store, &[target], None, 2).await;
    assert!(!out[0].ok);
    let err = out[0].error.as_deref().unwrap();
    assert!(err.contains("HTTP 503") && err.contains("cli:"), "{err}");
    let list = store.model_catalog_list().unwrap();
    assert!(list[0].available, "failure must not mark models removed");
    let rec = store.model_catalog_discovery_records().unwrap();
    assert!(!rec[0].ok);
}

#[tokio::test]
async fn openai_compatible_sends_the_key_and_parses() {
    let (base, seen) = serve(vec![(
        "/v1/models",
        200,
        r#"{"data":[{"id":"qwen3-30b"},{"id":"qwen3-8b"}]}"#.to_string(),
    )])
    .await;
    let target = DiscoveryTarget::OpenaiCompatible {
        id: "qwen".into(),
        base_url: format!("{base}/v1"),
        api_key: Some("sk-secret".into()),
    };
    assert_eq!(target.source().as_str(), "openai-compatible:qwen");
    let got = discover_one(&target).await.unwrap();
    assert_eq!(ids(&got), ["qwen3-30b", "qwen3-8b"]);
    let head = seen.lock().unwrap()[0].to_ascii_lowercase();
    assert!(head.contains("authorization: bearer sk-secret"), "{head}");
    // Debug は鍵を出さない。
    assert!(!format!("{target:?}").contains("sk-secret"));
}

#[tokio::test]
async fn claude_uses_the_oauth_bearer_and_beta_header() {
    let (base, seen) = serve(vec![(
        "/v1/models",
        200,
        r#"{"data":[{"id":"claude-opus-x","display_name":"Opus X"}],"has_more":false}"#.to_string(),
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let acct = dir.path().join("a1");
    std::fs::create_dir_all(&acct).unwrap();
    let far = (time::OffsetDateTime::now_utc().unix_timestamp() + 3600) * 1000;
    std::fs::write(
        acct.join(".credentials.json"),
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"tok-abc","refreshToken":"r","expiresAt":{far}}}}}"#
        ),
    )
    .unwrap();
    let target = DiscoveryTarget::ClaudeOauth {
        base_url: base,
        account_dir: first_logged_in(dir.path(), ".credentials.json"),
    };
    let got = discover_one(&target).await.unwrap();
    assert_eq!(got[0].display_name.as_deref(), Some("Opus X"));
    let head = seen.lock().unwrap()[0].to_ascii_lowercase();
    assert!(head.contains("authorization: bearer tok-abc"), "{head}");
    assert!(head.contains("anthropic-beta: oauth-2025-04-20"), "{head}");
}

#[tokio::test]
async fn claude_without_a_logged_in_account_or_with_an_expired_token_fails_cleanly() {
    let target = DiscoveryTarget::ClaudeOauth {
        base_url: "http://127.0.0.1:1".into(),
        account_dir: None,
    };
    assert!(
        discover_one(&target)
            .await
            .unwrap_err()
            .contains("no logged-in")
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"tok-old","refreshToken":"r","expiresAt":1000}}"#,
    )
    .unwrap();
    let target = DiscoveryTarget::ClaudeOauth {
        base_url: "http://127.0.0.1:1".into(),
        account_dir: Some(dir.path().to_path_buf()),
    };
    let err = discover_one(&target).await.unwrap_err();
    assert!(err.contains("expired") && !err.contains("tok-old"), "{err}");
}

#[tokio::test]
async fn codex_pages_through_model_list_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let codex = dir.path().join("codex");
    // initialize(id 1) → model/list(id 2, cursor 無し) → model/list(id 3, cursor p2)。
    write_executable(
        &codex,
        r#"[ "$1" = "app-server" ] || exit 2
[ -n "$CODEX_HOME" ] || exit 3
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"id":1,"result":{"userAgent":"x"}}' ;;
    *'"method":"model/list"'*)
      case "$line" in
        *'"includeHidden":false'*) ;;
        *) exit 4 ;;
      esac
      case "$line" in
        *'"cursor":"p2"'*) echo '{"id":3,"result":{"data":[{"id":"gpt-c"}],"nextCursor":null}}' ;;
        *) echo '{"method":"noise","params":{}}'; echo '{"id":2,"result":{"data":[{"id":"gpt-a","displayName":"A"},{"id":"hid","hidden":true},{"model":"gpt-b"}],"nextCursor":"p2"}}' ;;
      esac ;;
  esac
done"#,
    );
    let acct = dir.path().join("acct");
    std::fs::create_dir_all(&acct).unwrap();
    let target = DiscoveryTarget::CodexOauth {
        command: codex.to_string_lossy().into_owned(),
        account_dir: Some(acct),
    };
    let got = discover_one(&target).await.unwrap();
    assert_eq!(ids(&got), ["gpt-a", "gpt-b", "gpt-c"]);
    assert_eq!(got[0].display_name.as_deref(), Some("A"));
}

#[tokio::test]
async fn run_discovery_applies_and_reports_deltas_for_one_source() {
    let (base, _) = serve(vec![(
        "/v1/models",
        200,
        r#"{"data":[{"id":"m1"},{"id":"m2"}]}"#.to_string(),
    )])
    .await;
    let targets = vec![
        DiscoveryTarget::OpenaiCompatible {
            id: "qwen".into(),
            base_url: format!("{base}/v1"),
            api_key: None,
        },
        DiscoveryTarget::OpenaiCompatible {
            id: "other".into(),
            base_url: "http://127.0.0.1:1/v1".into(),
            api_key: None,
        },
    ];
    let store = SqliteStore::open_in_memory().unwrap();
    let out = run_targets(&store, &targets, Some("openai-compatible:qwen"), 10).await;
    assert_eq!(out.len(), 1);
    assert!(out[0].ok);
    assert_eq!(out[0].count, 2);
    assert_eq!(out[0].delta.added, ["m1", "m2"]);
    assert_eq!(store.model_catalog_list().unwrap().len(), 2);
}

#[tokio::test]
async fn ticker_respects_interval_disable_and_overlap() {
    let mut t = DiscoveryTicker::default();
    assert!(t.due(3600, 100));
    assert!(!t.due(0, 100));
    t.last_run = Some(100);
    assert!(!t.due(3600, 3699));
    assert!(t.due(3600, 3700));
    // 実行中は due にならない。
    t.in_flight.store(true, Ordering::SeqCst);
    assert!(!t.due(3600, 99_999));
}

#[tokio::test]
async fn ticker_spawns_once_and_clears_the_flag() {
    let (base, _) = serve(vec![(
        "/v1/models",
        200,
        r#"{"data":[{"id":"m1"}]}"#.to_string(),
    )])
    .await;
    let text = format!(
        "[[llm_proxy.sources.openai_compatible]]\nid = \"qwen\"\nbase_url = \"{base}/v1\"\n[model_catalog]\nrefresh_interval_seconds = 60\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n"
    );
    let config: Config = toml::from_str(&text).unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut t = DiscoveryTicker::default();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = t
        .tick(Arc::clone(&store), &config, 1000, move |s| {
            let _ = tx.send(s.len());
        })
        .expect("started");
    // 同じ周期の中では 2 つ目は始まらない。
    assert!(t.tick(Arc::clone(&store), &config, 1001, |_| {}).is_none());
    let summaries = handle.await.unwrap();
    assert!(summaries[0].ok);
    assert_eq!(rx.await.unwrap(), 1);
    assert_eq!(store.model_catalog_list().unwrap().len(), 1);
    assert!(!t.in_flight.load(Ordering::SeqCst));
    assert!(t.tick(Arc::clone(&store), &config, 1030, |_| {}).is_none());
    assert!(t.tick(Arc::clone(&store), &config, 1060, |_| {}).is_some());
}

#[test]
fn targets_come_from_the_config_and_the_defaults_are_set() {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    std::fs::create_dir_all(claude.join("a1")).unwrap();
    std::fs::write(claude.join("a1/.credentials.json"), "{}").unwrap();
    std::fs::create_dir_all(claude.join("a0")).unwrap(); // 未ログイン（目印なし）
    let text = format!(
        "[accounts]\nclaude_dir = {claude:?}\nopencode_dir = \"/x/opencode\"\n[[llm_proxy.sources.openai_compatible]]\nid = \"qwen\"\nbase_url = \"http://h/v1\"\napi_key = \"k\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n"
    );
    let config: Config = toml::from_str(&text).unwrap();
    assert_eq!(config.model_catalog.refresh_interval_seconds, 3600);
    let targets = targets_from_config(&config);
    let sources: Vec<String> = targets.iter().map(|t| t.source().0).collect();
    assert_eq!(
        sources,
        ["opencode-go", "claude-oauth", "openai-compatible:qwen"]
    );
    match &targets[0] {
        DiscoveryTarget::OpencodeGo {
            models_url,
            cli_command,
        } => {
            assert_eq!(models_url, "https://opencode.ai/zen/go/v1/models");
            assert_eq!(cli_command, "opencode");
        }
        other => panic!("{other:?}"),
    }
    match &targets[1] {
        DiscoveryTarget::ClaudeOauth {
            base_url,
            account_dir,
        } => {
            assert_eq!(base_url, "https://api.anthropic.com");
            assert_eq!(account_dir.as_deref(), Some(claude.join("a1").as_path()));
        }
        other => panic!("{other:?}"),
    }
    let off: Config = toml::from_str("[model_catalog]\nrefresh_interval_seconds = 0\nopencode_go_models_url = \"http://x/m\"\nopencode_cli = \"oc\"\n[[providers]]\nid = \"x\"\nadapter = \"fake\"\n").unwrap();
    assert_eq!(off.model_catalog.refresh_interval_seconds, 0);
    assert_eq!(off.model_catalog.opencode_cli, "oc");
    assert!(targets_from_config(&off).is_empty());
}
