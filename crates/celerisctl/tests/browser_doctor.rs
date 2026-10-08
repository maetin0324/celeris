//! Doctor reads a fake daemon in a temporary configuration, never a production DB/socket.
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};

fn serve(dir: &std::path::Path, items: Value) -> (std::path::PathBuf, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = dir.join("config.toml");
    std::fs::write(dir.join("token"), "test-doctor-token\n").unwrap();
    std::fs::write(
        &config,
        format!(
            "[api]\nlisten = '{}'\ntoken_file = 'token'\n[db]\npath = 'must-not-create.db'\n[[providers]]\nid = 'fake-local'\nadapter = 'fake'\n",
            listener.local_addr().unwrap()
        ),
    )
    .unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut req = Vec::new();
        let mut byte = [0];
        while !req.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            req.push(byte[0]);
        }
        let req = String::from_utf8(req).unwrap();
        assert!(req.starts_with("GET /api/v1/browser/readiness HTTP/1.1\r\n"));
        assert!(req.contains("Authorization: Bearer test-doctor-token\r\n"));
        let body = json!({"items":items}).to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    (config, server)
}

fn doctor(config: &std::path::Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .env_remove("CELERIS_COS_RUN_CREDENTIAL")
        .env_remove("CELERIS_API_URL")
        .env_remove("CELERIS_API_TOKEN_FILE")
        .args(["browser", "doctor", "--config"])
        .arg(config)
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn browser_doctor_prints_one_line_per_missing_item_and_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    let checks = [
        "ledger",
        "agent-browser",
        "launcher",
        "egress-resolver",
        "credentiald",
        "attestation-key",
        "site-policies",
        "grant",
    ];
    let items: Vec<_> = checks
        .iter()
        .map(|check| json!({"status":"NG","check":check,"detail":"missing; 修正: 設定を確認"}))
        .collect();
    let (config, server) = serve(dir.path(), json!(items));
    let out = doctor(&config, &[]);
    server.join().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stderr.is_empty());
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), checks.len());
    for (line, check) in text.lines().zip(checks) {
        assert!(line.starts_with("NG  "));
        assert!(line.contains(check));
        assert!(line.contains("修正:"));
    }
    assert!(!dir.path().join("must-not-create.db").exists());
}

#[test]
fn browser_doctor_json_ok_warn_and_skip_exit_zero_with_env_config() {
    let dir = tempfile::tempdir().unwrap();
    let items = json!([
        {"status":"OK","check":"ledger","detail":"release=abc agent-browser=0.38.1"},
        {"status":"WARN","check":"grant","detail":"credential_use=off; 修正: web で設定"},
        {"status":"SKIP","check":"bwrap","detail":"runtime=launcher"}
    ]);
    let (config, server) = serve(dir.path(), items.clone());
    let out = Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .env_remove("CELERIS_COS_RUN_CREDENTIAL")
        .env_remove("CELERIS_API_URL")
        .env_remove("CELERIS_API_TOKEN_FILE")
        .env("CELERIS_CONFIG", config)
        .args(["browser", "doctor", "--json"])
        .output()
        .unwrap();
    server.join().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report, json!({"items":items}));
}

#[test]
fn browser_doctor_daemon_unavailable_is_one_line_exit_two() {
    let dir = tempfile::tempdir().unwrap();
    // Missing temporary configuration gives the same actionable daemon row as failed transport.
    let out = doctor(&dir.path().join("absent.toml"), &[]);
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8(out.stdout).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.starts_with("NG   daemon "));
    assert!(text.contains("修正:"));
    let out = doctor(&dir.path().join("absent.toml"), &["--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["items"][0]["check"],
        "daemon"
    );
}

#[test]
fn browser_doctor_api_url_and_token_file_overrides_config() {
    let dir = tempfile::tempdir().unwrap();
    let (config, server) = serve(
        dir.path(),
        json!([{"status":"OK","check":"runtime","detail":"launcher"}]),
    );
    let raw = std::fs::read_to_string(config).unwrap();
    let listen = raw
        .lines()
        .find_map(|l| {
            l.strip_prefix("listen = '")
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .env_remove("CELERIS_COS_RUN_CREDENTIAL")
        .env_remove("CELERIS_API_URL")
        .args([
            "--api-url",
            &format!("http://{listen}/api/v1"),
            "browser",
            "doctor",
            "--config",
        ])
        .arg(dir.path().join("absent.toml"))
        .arg("--token-file")
        .arg(dir.path().join("token"))
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
