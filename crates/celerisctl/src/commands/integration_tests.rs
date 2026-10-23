#[cfg(test)]
#[path = "../../../../tests/support/http_sync.rs"]
mod fake_http;

use super::*;
use clap::Parser;
use std::io::Write;
use std::net::TcpListener;
use std::thread;

fn parse(args: &[&str]) -> Result<IntegrationCommand, clap::Error> {
    crate::Cli::try_parse_from(std::iter::once("celerisctl").chain(args.iter().copied())).map(
        |cli| match cli.command {
            crate::Command::Integration { command, .. } => command,
            other => panic!("not an integration command: {other:?}"),
        },
    )
}

fn task() -> String {
    task_core::TaskId::new().to_string()
}

#[test]
fn integration_api_list_paths_and_filters() {
    let id = task();
    let (method, path, body) = request_of(&parse(&["integration", "list"]).unwrap()).unwrap();
    assert_eq!(
        (method, path.as_str(), body),
        ("GET", "/integrations", None)
    );

    let (_, path, _) =
        request_of(&parse(&["integration", "list", "--state", "held"]).unwrap()).unwrap();
    assert_eq!(path, "/integrations?state=held");
    let (_, path, _) =
        request_of(&parse(&["integration", "list", "--state", "not_applicable"]).unwrap()).unwrap();
    assert_eq!(path, "/integrations?state=not_applicable");

    let (_, path, _) =
        request_of(&parse(&["integration", "list", "--task", &id]).unwrap()).unwrap();
    assert_eq!(path, format!("/tasks/{id}/integrations"));

    assert!(parse(&["integration", "list", "--state", "merged"]).is_err());
    let bad_task = parse(&["integration", "list", "--task", "not-a-task"]).unwrap();
    assert!(
        request_of(&bad_task).is_err(),
        "a bad task id is refused before any request"
    );
}

#[test]
fn integration_api_actions_build_paths_and_bodies() {
    let id = task();
    let cases: [(&[&str], &str, Option<Value>); 4] = [
        (
            &["integration", "merge", &id, "pluvio"],
            "merge",
            Some(json!({})),
        ),
        (
            &["integration", "reopen", &id, "pluvio"],
            "reopen",
            Some(json!({})),
        ),
        (
            &[
                "integration",
                "discard",
                &id,
                "pluvio",
                "--reason",
                "obsolete",
            ],
            "discard",
            Some(json!({"reason": "obsolete"})),
        ),
        (
            &[
                "integration",
                "hold",
                &id,
                "pluvio",
                "--reason",
                "wait for review",
            ],
            "hold",
            Some(json!({"reason": "wait for review"})),
        ),
    ];
    for (args, action, expected_body) in cases {
        let (method, path, body) = request_of(&parse(args).unwrap()).unwrap();
        assert_eq!(method, "POST", "{action}");
        assert_eq!(path, format!("/tasks/{id}/integrations/pluvio/{action}"));
        assert_eq!(body, expected_body, "{action}");
    }
}

#[test]
fn integration_api_discard_and_hold_require_reason() {
    let id = task();
    for action in ["discard", "hold"] {
        assert!(
            parse(&["integration", action, &id, "pluvio"]).is_err(),
            "{action} without --reason must fail to parse"
        );
        assert!(
            parse(&["integration", action, &id, "pluvio", "--reason", ""]).is_ok(),
            "an empty reason is refused by the daemon (400), not by the parser"
        );
    }
    // The global `--reason` (CoS audit) must not swallow the integration reason.
    match parse(&["integration", "hold", &id, "pluvio", "--reason", "why"]).unwrap() {
        IntegrationCommand::Hold(a) => assert_eq!(a.reason, "why"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn integration_api_rejects_unknown_state_at_parse_time() {
    let err = parse(&["integration", "list", "--state", "bogus"]).unwrap_err();
    assert!(
        err.to_string().contains("unknown integration state"),
        "{err}"
    );
}

#[test]
fn integration_api_filter_and_table_render_items() {
    let response = json!({"items": [
        {"task_id": "T1", "repo_id": "R1", "state": "unintegrated", "branch": "b1", "head_sha": "h1", "reason": null},
        {"task_id": "T2", "repo_id": "R1", "state": "held", "branch": "b2", "head_sha": "h2", "reason": "wait"},
    ]});
    assert_eq!(filter_items(&response, None).len(), 2);
    let held = filter_items(&response, Some(TaskRepoIntegrationStatus::Held));
    assert_eq!(held.len(), 1);
    assert_eq!(held[0]["task_id"], "T2");
    assert_eq!(render_table(&held), "T2\tR1\theld\tb2\th2\twait");
    assert_eq!(
        render_table(&filter_items(
            &response,
            Some(TaskRepoIntegrationStatus::Discarded)
        )),
        "no integration rows"
    );
    let all = render_table(&filter_items(&response, None));
    assert!(all.contains("T1\tR1\tunintegrated\tb1\th1\t-"), "{all}");
}

/// The HTTP call: the method, path, body and admin bearer reach the daemon, and its JSON reply is
/// returned. A local listener only; no external network.
fn exercise(command: IntegrationCommand, expected_method: &str, expected_path: &str) -> Value {
    let (method, path, body) = request_of(&command).unwrap();
    assert_eq!((method, path.as_str()), (expected_method, expected_path));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let expected_first = format!("{method} /api/v1{path} HTTP/1.1");
    let server = thread::spawn(move || {
        let mut parts = expected_first.split_whitespace();
        let (mut stream, request) =
            fake_http::accept(&listener, parts.next().unwrap(), parts.next().unwrap());
        assert!(request.starts_with(&expected_first), "{request}");
        assert!(request.contains("Authorization: Bearer admin-token\r\n"));
        let reply = r#"{"integrated":false}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
            reply.len()
        )
        .unwrap();
        request
    });
    let api = ApiConfig {
        base_url: format!("http://{addr}/api/v1"),
        token: Some("admin-token".to_string()),
    };
    let reply = request(&api, method, &path, body.clone()).unwrap();
    let sent = server.join().unwrap();
    if let Some(body) = body {
        let actual: Value = serde_json::from_str(sent.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(actual, body);
    }
    reply
}

#[test]
fn integration_api_http_calls_reach_the_daemon_route() {
    let id = task();
    let reply = exercise(
        parse(&[
            "integration",
            "discard",
            &id,
            "pluvio",
            "--reason",
            "obsolete",
        ])
        .unwrap(),
        "POST",
        &format!("/tasks/{id}/integrations/pluvio/discard"),
    );
    assert_eq!(reply["integrated"], false);
    exercise(
        parse(&["integration", "merge", &id, "pluvio"]).unwrap(),
        "POST",
        &format!("/tasks/{id}/integrations/pluvio/merge"),
    );
    exercise(
        parse(&["integration", "list", "--state", "unintegrated"]).unwrap(),
        "GET",
        "/integrations?state=unintegrated",
    );
}
