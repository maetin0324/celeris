//! ADR 2026-11-22-external-llm-discussion-exchange D6・D7.2: the RDC path ChatGPT uses today —
//! `scripts/rdc/celeris-chat` → `celerisctl mcp call` → Celeris MCP — run for real against a test MCP server bound to
//! `127.0.0.1:0` (no external network). The script is executed as it is installed for the RDC user (its shebang and its
//! environment variables): `CELERIS_BIN` is the `celerisctl` built for this test, `CELERIS_MCP_URL` the test server and
//! `CELERIS_MCP_TOKEN_FILE` a temporary 0600 file. The MCP client gets the scope list that
//! `scripts/rdc/setup-chatgpt-rdc.sh` tells the human to give `chatgpt-rdc-2`, so the test runs with what production is
//! told to use.
//!
//! The MCP-level contract of the three discussion tools is `crates/celeris-mcp/tests/mcp_discussion.rs`; those tests
//! speak HTTP directly and never run the script, `celerisctl mcp call` or the scope list of the setup script.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use celeris_mcp::config::ListenerAuth;
use celeris_mcp::state::McpState;
use serde_json::{Value, json};
use task_core::chat::{ChatOpenThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::discussion_exchange::{ExchangeShareRequest, ShareScope};
use task_core::{McpClient, McpScope, SqliteStore};
use time::OffsetDateTime;

const ME: &str = "human:alice";
const CLIENT: &str = "chatgpt-rdc-2";
const TOKEN: &str = "rdc-test-token-5f1c2a9e7b3d4c80";
const WRONG_TOKEN: &str = "rdc-wrong-token-0a9b8c7d6e5f4a3b";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn script() -> PathBuf {
    repo().join("scripts/rdc/celeris-chat")
}

/// The `--scope` list of `mcp client scopes chatgpt-rdc-2` in `scripts/rdc/setup-chatgpt-rdc.sh`.
fn setup_scopes() -> Vec<McpScope> {
    let path = repo().join("scripts/rdc/setup-chatgpt-rdc.sh");
    let text = std::fs::read_to_string(&path).expect("setup-chatgpt-rdc.sh");
    let at = text
        .find(&format!("mcp client scopes {CLIENT}"))
        .expect("setup script names the scopes of chatgpt-rdc-2");
    let rest = &text[at..];
    let flag = rest.find("--scope ").expect("--scope after the command");
    let value: String = rest[flag + "--scope ".len()..]
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || *c == ':' || *c == ',')
        .collect();
    value
        .split(',')
        .map(|s| McpScope::parse(s).unwrap_or_else(|| panic!("unknown scope {s} in setup")))
        .collect()
}

struct Rdc {
    store: Arc<SqliteStore>,
    db: PathBuf,
    base_url: String,
    token_file: PathBuf,
    home: PathBuf,
    project: String,
    thread: String,
    _dir: tempfile::TempDir,
    _rt: tokio::runtime::Runtime,
}

impl Rdc {
    /// A project with its fixed CoS thread, the client `chatgpt-rdc-2` with the setup script's scopes, its token in a
    /// 0600 file, and the MCP server (token auth, as the 18200 listener) on a runtime that lives as long as `Rdc`.
    fn new() -> Rdc {
        let dir = tempfile::tempdir().expect("tmp");
        let db = dir.path().join("celeris.db");
        let store = Arc::new(SqliteStore::open(&db).expect("store"));
        let now = OffsetDateTime::now_utc();
        let project = task_core::Project {
            id: task_core::ProjectId::new(),
            title: "RDC".into(),
            request: "RDC の元の目的".into(),
            status: task_core::ProjectStatus::Active,
            workspace: None,
            archived_at: None,
            paused_from: None,
            auto_advance: false,
            slug: None,
            created_at: now,
            updated_at: now,
        };
        store.project_create(&project).expect("project");
        let thread = store
            .cos_thread_open(
                &ChatOpenThreadRequest {
                    project_id: Some(project.id.to_string()),
                },
                now,
            )
            .expect("project thread")
            .thread
            .id;
        store
            .mcp_client_create(&McpClient {
                id: CLIENT.into(),
                name: CLIENT.into(),
                token_hash: Some(celeris_mcp::auth::hash_token(TOKEN)),
                scopes: setup_scopes(),
                created_at: now,
                last_used_at: None,
                revoked_at: None,
            })
            .expect("client");

        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        let token_file = dir.path().join("mcp-token");
        std::fs::write(&token_file, format!("{TOKEN}\n")).expect("token file");
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600))
            .expect("chmod 600");

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let state = McpState::from_store(
            Arc::clone(&store),
            600,
            Vec::new(),
            Vec::new(),
            "secretary".into(),
            None,
        );
        rt.spawn(celeris_mcp::serve(
            listener,
            state,
            ListenerAuth::Token,
            std::future::pending::<()>(),
        ));
        Rdc {
            store,
            db,
            base_url: format!("http://{addr}"),
            token_file,
            home,
            project: project.id.to_string(),
            thread,
            _dir: dir,
            _rt: rt,
        }
    }

    fn count(&self, sql: &str) -> i64 {
        rusqlite::Connection::open(&self.db)
            .expect("open db")
            .query_row(sql, [], |row| row.get(0))
            .expect("count")
    }

    fn post(&self, key: &str, text: &str) -> String {
        self.store
            .chat_message_post_as(
                &self.thread,
                &ChatPostMessageRequest {
                    client_message_id: key.into(),
                    text: text.into(),
                    attachment_ids: vec![],
                    reply_to_id: None,
                    mode: ChatSendMode::Queue,
                    resume_queue: false,
                    discussion_mode: None,
                },
                ME,
                OffsetDateTime::now_utc(),
            )
            .expect("post")
            .response
            .message
            .id
    }

    /// The human shares messages for outside LLMs (what `POST …/exchange/shares` records).
    fn share(&self, ids: &[&str]) {
        let now = OffsetDateTime::now_utc();
        self.store
            .chat_exchange_share(
                &self.thread,
                ME,
                &ExchangeShareRequest {
                    message_ids: ids.iter().map(|s| (*s).to_owned()).collect(),
                    scope: ShareScope::SelectedExcerpt,
                    include_confidential: false,
                    expires_at: (now + time::Duration::days(1))
                        .format(&time::format_description::well_known::Rfc3339)
                        .expect("rfc3339"),
                },
                now,
            )
            .expect("share");
    }

    /// Runs `celeris-chat <args…>` as the RDC user would, with `token_file` as the token.
    fn run_with(&self, token_file: &Path, args: &[&str]) -> Output {
        Command::new(script())
            .args(args)
            .env("CELERIS_BIN", common::celerisctl_path())
            .env("CELERIS_MCP_URL", &self.base_url)
            .env("CELERIS_MCP_TOKEN_FILE", token_file)
            .env("HOME", &self.home)
            .env_remove("CELERIS_MCP_TOKEN")
            .output()
            .expect("run celeris-chat")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with(&self.token_file, args)
    }

    fn ok(&self, tool: &str, arguments: Value) -> Value {
        let out = self.run(&[tool, &arguments.to_string()]);
        assert!(
            out.status.success(),
            "{tool}: exit {:?}, stderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "{tool}: stdout is not JSON ({e}): {}",
                String::from_utf8_lossy(&out.stdout)
            )
        })
    }
}

fn items(snapshot: &Value) -> Vec<Value> {
    snapshot["items"].as_array().expect("items").clone()
}

/// Invariant (D6): through the RDC entry point, ChatGPT gets `thread_id`・`project_id` and the citation of what the
/// human shared, records a draft in the same thread (once, however often it retries), and finds that draft in the
/// next snapshot as `mcp:chatgpt-rdc-2`'s proposal with its citation — whether it continues with `thread_id` or starts
/// a new conversation with only `project_id`. What the human did not share never comes out, and the same client
/// still cannot answer a decision or a plan gate. No existing test runs `celeris-chat`: the MCP tests call the HTTP
/// endpoint directly, so a break in the script, in `celerisctl mcp call` (arguments, token file, output) or in the
/// setup script's scope list would pass them.
#[test]
fn rdc_celeris_chat_snapshot_propose_continue() {
    let rdc = Rdc::new();
    let shared = rdc.post(
        "m1",
        "仮説 H1: SCC が減らないのは循環の根が task-core の store にあるから",
    );
    let unshared = rdc.post("m2", "まだ外に出さない発言（共有していない）");
    rdc.share(&[&shared]);

    // 1. ChatGPT opens the discussion with the project id only.
    let snap = rdc.ok("discussion_snapshot", json!({"project_id": rdc.project}));
    assert_eq!(snap["schema"], "celeris.discussion-snapshot/1");
    assert_eq!(snap["project_id"], json!(rdc.project));
    assert_eq!(snap["thread_id"], json!(rdc.thread));
    let first_items = items(&snap);
    assert_eq!(first_items.len(), 1, "only the shared message: {snap}");
    let item = &first_items[0];
    assert_eq!(item["kind"], "message");
    assert_eq!(item["message_id"], json!(shared));
    assert_eq!(item["actor"]["id"], ME);
    assert_eq!(item["decision_state"], "discussion");
    assert_eq!(item["citation"]["thread_id"], json!(rdc.thread));
    assert_eq!(item["citation"]["message_id"], json!(shared));
    assert!(
        item["content_hash"]
            .as_str()
            .is_some_and(|h| h.starts_with("sha256:")),
        "{item}"
    );
    assert!(!snap.to_string().contains(&unshared), "{snap}");

    // 2. It looks for evidence in the same range and gets the same citation back.
    let found = rdc.ok(
        "discussion_evidence_search",
        json!({"project_id": rdc.project, "query": "SCC task-core"}),
    );
    let hits = found["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 1, "{found}");
    assert_eq!(hits[0]["citation"], item["citation"]);
    let none = rdc.ok(
        "discussion_evidence_search",
        json!({"project_id": rdc.project, "query": "外に出さない"}),
    );
    assert_eq!(none["hits"], json!([]), "unshared text is not searchable");

    // 3. The human picks an idea in ChatGPT; ChatGPT records it in the same thread, citing the shared message.
    let mut cite = item["citation"].clone();
    cite["kind"] = json!("chat");
    let propose = json!({
        "thread_id": snap["thread_id"],
        "idempotency_key": "chatgpt-2026-11-22-001",
        "source_label": "chatgpt",
        "body": "H1 は store の依存を切った版で SCC を数えれば反証できる。最小の識別実験は task-core の store だけを別 crate に移すこと。",
        "references": [cite.clone()],
        "confidence": "estimated",
        "decision_state": "proposed",
    });
    let first = rdc.ok("discussion_propose", propose.clone());
    assert_eq!(first["thread_id"], json!(rdc.thread));
    assert_eq!(first["project_id"], json!(rdc.project));
    assert_eq!(first["decision_state"], "proposed");
    assert_eq!(first["duplicate"], false);
    // A retry of the same call (RDC re-runs a command) records nothing new.
    let again = rdc.ok("discussion_propose", propose);
    assert_eq!(again["duplicate"], true);
    assert_eq!(again["message_id"], first["message_id"]);
    assert_eq!(
        rdc.count(&format!(
            "SELECT count(*) FROM chat_messages WHERE json_extract(metadata_json,'$.author') = 'mcp:{CLIENT}'"
        )),
        1
    );

    // 4. The next snapshot shows the draft with its source, both when ChatGPT continues with the thread id and when
    //    a new ChatGPT conversation starts again from the project id.
    for selector in [
        json!({"thread_id": rdc.thread}),
        json!({"project_id": rdc.project}),
    ] {
        let next = rdc.ok("discussion_snapshot", selector.clone());
        assert_eq!(next["thread_id"], json!(rdc.thread), "{selector}");
        assert_eq!(next["project_id"], json!(rdc.project), "{selector}");
        let next_items = items(&next);
        assert_eq!(next_items.len(), 2, "{next}");
        let message = next_items
            .iter()
            .find(|i| i["kind"] == "message")
            .expect("the shared message");
        assert_eq!(message["citation"], item["citation"]);
        let proposal = next_items
            .iter()
            .find(|i| i["kind"] == "proposal")
            .expect("the draft");
        assert_eq!(proposal["message_id"], first["message_id"]);
        assert_eq!(proposal["content_hash"], first["content_hash"]);
        assert_eq!(
            proposal["actor"],
            json!({"kind": "mcp", "id": format!("mcp:{CLIENT}")})
        );
        assert_eq!(proposal["source"]["label"], "chatgpt");
        assert_eq!(proposal["source"]["thread_id"], json!(rdc.thread));
        assert_eq!(proposal["decision_state"], "proposed");
        assert_ne!(proposal["consent"]["granted_by"], json!(ME));
        let refs = proposal["references"].as_array().expect("references");
        assert_eq!(refs.len(), 1, "{proposal}");
        for field in ["kind", "thread_id", "message_id", "revision", "sha256"] {
            assert_eq!(refs[0][field], cite[field], "{field}: {proposal}");
        }
    }

    // 5. The draft is a `discuss` message by the MCP client, not a person's word; the same client cannot answer a
    //    decision or a plan gate through this entry point (they are not its tools: -32601), and nothing was created.
    let mode_author = rdc.count(&format!(
        "SELECT count(*) FROM chat_messages WHERE id = '{}' AND role = 'user' \
         AND json_extract(metadata_json,'$.discussion_mode') = 'discuss' \
         AND json_extract(metadata_json,'$.author') = 'mcp:{CLIENT}'",
        first["message_id"].as_str().expect("message id")
    ));
    assert_eq!(mode_author, 1);
    let list = rdc.run(&["--list"]);
    assert!(list.status.success());
    let names: Vec<String> = String::from_utf8_lossy(&list.stdout)
        .lines()
        .filter_map(|l| l.split('\t').next().map(str::to_owned))
        .collect();
    for name in [
        "discussion_snapshot",
        "discussion_evidence_search",
        "discussion_propose",
    ] {
        assert!(names.iter().any(|n| n == name), "{name}: {names:?}");
    }
    for name in [
        "decision_answer",
        "task_plan_gate",
        "task_approve",
        "task_reject",
    ] {
        assert!(!names.iter().any(|n| n == name), "{name}: {names:?}");
        let out = rdc.run(&[
            name,
            r#"{"id":"01M7XPWTXR1A0CNWYN7D1HBZWB","option":"approve"}"#,
        ]);
        assert_eq!(out.status.code(), Some(1), "{name}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("-32601"),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(rdc.count("SELECT count(*) FROM tasks"), 0);
    assert_eq!(rdc.count("SELECT count(*) FROM decisions"), 0);
    // The calls are audited under the client, without the bodies of the discussion tools.
    assert!(
        rdc.count(&format!(
            "SELECT count(*) FROM mcp_calls WHERE client_id = '{CLIENT}' AND tool LIKE 'discussion_%'"
        )) >= 6
    );
    assert_eq!(
        rdc.count(
            "SELECT count(*) FROM mcp_calls WHERE tool LIKE 'discussion_%' AND request IS NOT NULL"
        ),
        0
    );
}

/// Invariant (D2.4・D7.2): the token never appears in what `celeris-chat` prints — not in a result, not in the error
/// of a refused tool, a bad argument or a wrong token (401), nor in the audit rows. ChatGPT reads both stdout and
/// stderr of the RDC shell, so an echo of the token (a `set -x`, a printed command line, an error that quotes the
/// header) would hand the credential to the outside LLM. No existing test runs the script or looks at its output.
#[test]
fn rdc_celeris_chat_token_not_in_output() {
    let rdc = Rdc::new();
    let m = rdc.post("m1", "共有する発言");
    rdc.share(&[&m]);
    let wrong = rdc.home.join("wrong-token");
    std::fs::write(&wrong, format!("{WRONG_TOKEN}\n")).expect("wrong token");
    let missing = rdc.home.join("no-such-token");

    let project = json!({"project_id": rdc.project}).to_string();
    let approved = json!({
        "project_id": rdc.project,
        "idempotency_key": "k-approved",
        "source_label": "chatgpt",
        "body": "人が承認済みとして扱う案",
        "confidence": "estimated",
        "decision_state": "approved",
    })
    .to_string();
    let cases: Vec<(&str, Output, Option<i32>)> = vec![
        ("list", rdc.run(&["--list"]), Some(0)),
        (
            "snapshot",
            rdc.run(&["discussion_snapshot", &project]),
            Some(0),
        ),
        (
            "refused propose",
            rdc.run(&["discussion_propose", &approved]),
            Some(1),
        ),
        (
            "tool outside the scope",
            rdc.run(&["skills_write", "{}"]),
            Some(1),
        ),
        (
            "invalid JSON",
            rdc.run(&["discussion_snapshot", "{not json"]),
            Some(2),
        ),
        (
            "wrong token",
            rdc.run_with(&wrong, &["discussion_snapshot", &project]),
            Some(1),
        ),
        (
            "missing token file",
            rdc.run_with(&missing, &["discussion_snapshot", &project]),
            Some(1),
        ),
    ];
    for (name, out, code) in &cases {
        assert_eq!(out.status.code(), *code, "{name}");
        for (stream, bytes) in [("stdout", &out.stdout), ("stderr", &out.stderr)] {
            let text = String::from_utf8_lossy(bytes);
            for secret in [TOKEN, WRONG_TOKEN] {
                // The message shows where the token leaked without printing it again.
                assert!(
                    !text.contains(secret),
                    "{name}: token in {stream}: {}",
                    text.replace(secret, "[TOKEN]")
                );
            }
        }
    }
    // The wrong token is refused before any tool runs (401), so nothing was read with it.
    let wrong_err = String::from_utf8_lossy(&cases[5].1.stderr);
    assert!(wrong_err.contains("401"), "{wrong_err}");
    let audit: String = rusqlite::Connection::open(&rdc.db)
        .expect("db")
        .query_row(
            "SELECT coalesce(group_concat(client_id || tool || coalesce(error_kind,'') || coalesce(request,''), '|'), '') FROM mcp_calls",
            [],
            |r| r.get(0),
        )
        .expect("audit");
    for secret in [TOKEN, WRONG_TOKEN] {
        assert!(!audit.contains(secret), "token in mcp_calls");
    }
}
