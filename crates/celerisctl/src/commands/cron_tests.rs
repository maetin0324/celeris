use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

fn exercise(
    command: CronCommand,
    expected_method: &str,
    expected_path: &str,
    expected_body: Option<&str>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let path = expected_path.to_string();
    let method = expected_method.to_string();
    let body = expected_body.map(str::to_string);
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
            let request = String::from_utf8_lossy(&bytes);
            if let Some((head, content)) = request.split_once("\r\n\r\n") {
                let length = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|x| x.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if content.len() >= length {
                    break;
                }
            }
        }
        let request = String::from_utf8_lossy(&bytes);
        let first = request.lines().next().unwrap();
        assert!(
            first.starts_with(&format!("{method} {path} HTTP/1.1")),
            "{first}"
        );
        if let Some(body) = body {
            let actual = request.split_once("\r\n\r\n").unwrap().1;
            let expected: Value = serde_json::from_str(&body).unwrap();
            let actual: Value = serde_json::from_str(actual).unwrap();
            assert_eq!(actual, expected);
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}").unwrap();
    });
    let api = ApiConfig {
        base_url: format!("http://{addr}"),
        token: None,
    };
    let actual = request(&api, expected_method, expected_path, request_body(&command)).unwrap();
    assert_eq!(actual["ok"], true);
    server.join().unwrap();
}

fn request_body(command: &CronCommand) -> Option<Value> {
    match command {
        CronCommand::Create(a) => Some(
            json!({"name":a.name,"schedule":a.schedule,"timezone":a.timezone,"overlap":a.overlap,"catch_up":a.catch_up,"enabled":a.enabled,"template":parse_object(&a.template).unwrap()}),
        ),
        CronCommand::Update(a) => {
            let mut m = Map::new();
            if let Some(v) = &a.name {
                m.insert("name".into(), json!(v));
            }
            if let Some(v) = &a.schedule {
                m.insert("schedule".into(), json!(v));
            }
            if let Some(v) = &a.timezone {
                m.insert("timezone".into(), json!(v));
            }
            if let Some(v) = &a.overlap {
                m.insert("overlap".into(), json!(v));
            }
            if let Some(v) = &a.catch_up {
                m.insert("catch_up".into(), json!(v));
            }
            if let Some(v) = &a.template {
                m.insert("template".into(), parse_object(v).unwrap());
            }
            Some(Value::Object(m))
        }
        CronCommand::Pause(_) | CronCommand::Resume(_) | CronCommand::Run(_) => Some(json!({})),
        _ => None,
    }
}

#[test]
fn cron_list_calls_api() {
    exercise(CronCommand::List, "GET", "/cron-jobs", None);
}
#[test]
fn cron_show_calls_api() {
    exercise(
        CronCommand::Show(KeyArgs { id: "job-x".into() }),
        "GET",
        "/cron-jobs/job-x",
        None,
    );
}
#[test]
fn cron_create_parses_template_and_calls_api() {
    exercise(
        CronCommand::Create(CreateArgs {
            name: "daily".into(),
            schedule: "@daily".into(),
            timezone: "UTC".into(),
            overlap: "skip".into(),
            catch_up: "latest".into(),
            enabled: true,
            template: "{\"title\":\"hello\"}".into(),
        }),
        "POST",
        "/cron-jobs",
        Some(
            r#"{"name":"daily","schedule":"@daily","timezone":"UTC","overlap":"skip","catch_up":"latest","enabled":true,"template":{"title":"hello"}}"#,
        ),
    );
}
#[test]
fn cron_update_calls_api() {
    exercise(
        CronCommand::Update(UpdateArgs {
            id: "job-x".into(),
            name: Some("renamed".into()),
            schedule: None,
            timezone: None,
            overlap: None,
            catch_up: None,
            template: None,
        }),
        "PATCH",
        "/cron-jobs/job-x",
        Some(r#"{"name":"renamed"}"#),
    );
}
#[test]
fn cron_pause_calls_api() {
    exercise(
        CronCommand::Pause(KeyArgs { id: "job-x".into() }),
        "POST",
        "/cron-jobs/job-x/pause",
        Some("{}"),
    );
}
#[test]
fn cron_resume_calls_api() {
    exercise(
        CronCommand::Resume(KeyArgs { id: "job-x".into() }),
        "POST",
        "/cron-jobs/job-x/resume",
        Some("{}"),
    );
}
#[test]
fn cron_run_calls_api() {
    exercise(
        CronCommand::Run(KeyArgs { id: "job-x".into() }),
        "POST",
        "/cron-jobs/job-x/run",
        Some("{}"),
    );
}
#[test]
fn cron_history_calls_api() {
    exercise(
        CronCommand::History(HistoryArgs {
            id: "job-x".into(),
            limit: 7,
        }),
        "GET",
        "/cron-jobs/job-x/runs?limit=7",
        None,
    );
}

#[test]
fn cron_template_must_be_json_object() {
    assert!(parse_object("[]").is_err());
    assert!(parse_object("not json").is_err());
}

/// 本番の `ApiConfig`（`http://<listen>/api/v1`）でも区切りの `/` を落とさない（2026-10-03、
/// `/api/v1cron-jobs` へ送って 404 になっていた）。
#[test]
fn cron_request_keeps_the_api_v1_prefix_separator() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0; 4096];
        while !String::from_utf8_lossy(&bytes).contains("\r\n\r\n") {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"ok\":true}").unwrap();
        String::from_utf8_lossy(&bytes)
            .lines()
            .next()
            .unwrap()
            .to_string()
    });
    let api = ApiConfig {
        base_url: format!("http://{addr}/api/v1"),
        token: None,
    };
    let actual = request(&api, "GET", "/cron-jobs", None).unwrap();
    assert_eq!(actual["ok"], true);
    let first = server.join().unwrap();
    assert!(
        first.starts_with("GET /api/v1/cron-jobs HTTP/1.1"),
        "{first}"
    );
}
