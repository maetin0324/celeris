//! ADR 2026-11-21-chat-discussion-mode D3・D6・D8.1 (WU cite-attach): the run-end attach of draft
//! and summary files, through the real `workspace_files::attach`, a file-backed store and the
//! attachment blob store.
//!
//! 守る不変条件: 要約・案の file は検査（引用・D3 の固定値・子 2 の検査器・mode）を通ったものだけが返事に付き、
//! 通った file の引用の列は返事の `metadata_json.citations` に状態なしで 1 回だけ書かれる。GET と SSE の
//! replay は引いた message の今の revision から状態を導く。discuss の run の案・`approved` の案・別の案件の
//! 発言を引いた要約・同じ path の別の sha256 は付かず、code が run の status に出る。
//! 既存の試験では検出できない: base の添付は file の中身を見ずに全部付け、返事は引用を持たない。

use std::path::PathBuf;

use serde_json::{Value, json};
use task_core::SqliteStore;
use task_core::chat::attachments::ChatAttachmentLimits;
use task_core::chat::*;
use task_core::org::{Project, ProjectId, ProjectStatus};
use time::OffsetDateTime;

use super::super::workspace_files::{WorkspaceCapture, attach, snapshot};

const ME: &str = "human:alice";
const BASELINE: &str = include_str!("../../../../task-core/src/strategy/fixtures/baseline-v1.json");

fn at(secs: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_158_400 + secs).expect("valid ts")
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: SqliteStore,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let store = SqliteStore::open(&root.join("celeris.db")).expect("store");
    std::fs::create_dir_all(root.join("data")).expect("data dir");
    Fixture {
        _dir: dir,
        root,
        store,
    }
}

impl Fixture {
    fn project_thread(&self, title: &str) -> (Project, String) {
        let now = OffsetDateTime::now_utc();
        let project = Project {
            auto_advance: false,
            slug: None,
            archived_at: None,
            paused_from: None,
            id: ProjectId::new(),
            title: title.into(),
            request: format!("{title} の依頼"),
            status: ProjectStatus::Active,
            workspace: None,
            created_at: now,
            updated_at: now,
        };
        self.store.project_create(&project).expect("project");
        let thread = self
            .store
            .cos_thread_open(
                &ChatOpenThreadRequest {
                    project_id: Some(project.id.to_string()),
                },
                at(0),
            )
            .expect("thread")
            .thread
            .id;
        (project, thread)
    }

    fn post(
        &self,
        t: &str,
        key: &str,
        text: &str,
        mode: Option<ChatDiscussionMode>,
    ) -> ChatMessage {
        self.store
            .chat_message_post_as(
                t,
                &ChatPostMessageRequest {
                    client_message_id: key.into(),
                    text: text.into(),
                    attachment_ids: vec![],
                    reply_to_id: None,
                    mode: ChatSendMode::Queue,
                    resume_queue: false,
                    discussion_mode: mode,
                },
                ME,
                at(1),
            )
            .expect("post")
            .response
            .message
    }

    /// Claims the next run of `t`; returns `(run_id, output message id, workspace capture)`.
    fn run(&self, t: &str, run: &str) -> (String, WorkspaceCapture) {
        let claimed = self
            .store
            .chat_run_claim_next(t, run, &json!({"harness": "claude-code"}), at(2))
            .expect("claim")
            .expect("claimed");
        let workspace = self.root.join("ws").join(t);
        std::fs::create_dir_all(&workspace).expect("workspace");
        let capture = WorkspaceCapture {
            before: snapshot(&workspace),
            workspace,
            data_dir: self.root.join("data"),
            db_path: self.root.join("celeris.db"),
            limits: ChatAttachmentLimits::default(),
        };
        (claimed.output_message_id.expect("output message"), capture)
    }

    fn finish(&self, run: &str) {
        self.store
            .chat_run_finish(run, ChatRunState::Completed, Some("ok"), None, at(4))
            .expect("finish");
    }

    fn citation(&self, t: &str, m: &str) -> ChatCitation {
        let f = self
            .store
            .chat_cited_message(t, m)
            .expect("read")
            .expect("message");
        ChatCitation {
            thread_id: f.thread_id,
            message_id: f.message_id,
            revision: f.revision,
            sha256: f.sha256,
        }
    }

    /// The run's `status` summaries (the refusal codes are there).
    fn statuses(&self, t: &str) -> Vec<String> {
        self.store
            .chat_events_page(t, &ChatEventQuery::default())
            .expect("events")
            .items
            .into_iter()
            .filter_map(|e| match e.data {
                ChatEventData::Status(s) => Some(s.summary),
                _ => None,
            })
            .collect()
    }
}

fn write(capture: &WorkspaceCapture, rel: &str, body: &str) {
    let path = capture.workspace.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, body).expect("write");
}

fn summary_file(project: &Project, t: &str, citations: &[ChatCitation]) -> String {
    json!({
        "schema": "celeris.discussion-summary/1",
        "project_id": project.id.to_string(),
        "thread_id": t,
        "through_seq": 100,
        "body": "## 要約\n- 最小の辺の集合を先に測る案が出た",
        "citations": citations,
    })
    .to_string()
}

/// 子 2 の baseline を、この thread の CoS の案（D3 の固定値）に直し、参照をすべて `cite` にする。
fn draft(project: &Project, t: &str, cite: &ChatCitation, state: &str) -> String {
    fn refs(v: &mut Value, cite: &Value, t: &str) {
        match v {
            Value::Object(map) => {
                for (k, child) in map.iter_mut() {
                    if k == "references" {
                        *child = json!([cite]);
                    } else if k == "source" {
                        *child = json!({"kind": "celeris_cos_thread", "label": "案件 thread", "thread_id": t});
                    } else {
                        refs(child, cite, t);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|i| refs(i, cite, t)),
            _ => {}
        }
    }
    let mut doc: Value = serde_json::from_str(BASELINE).expect("baseline");
    let mut reference = serde_json::to_value(cite).expect("cite");
    reference["kind"] = json!("chat");
    refs(&mut doc, &reference, t);
    doc["project_id"] = json!(project.id.to_string());
    doc["actor"] = json!({"kind": "cos", "id": format!("cos:{t}")});
    doc["source_revision"] = json!({
        "request_sha256": task_core::project_proposal::sha256_hex(&project.request),
        "roadmap_version": null,
    });
    doc["decision_state"] = json!(state);
    serde_json::to_string_pretty(&doc).expect("json")
}

fn attach_now(
    f: &Fixture,
    capture: &WorkspaceCapture,
    t: &str,
    out: &str,
    run: &str,
) -> Vec<String> {
    attach(&f.store, capture, t, out, run, "要約と案を添えた", at(3))
        .expect("attach")
        .into_iter()
        .map(|w| w.path)
        .collect()
}

fn reply(f: &Fixture, t: &str, out: &str) -> ChatMessage {
    f.store.chat_message_get(t, out).expect("reply")
}

const SUMMARY: &str = "discussion/summary/arch.json";
const DRAFT: &str = "strategy/arch-v4-cycle/v1.json";

#[test]
fn chat_discussion_summary_attached_with_citations() {
    let f = fixture();
    let (project, t) = f.project_thread("引用");
    let said = f.post(
        &t,
        "m1",
        "最小の辺を先に測ろう",
        Some(ChatDiscussionMode::Discuss),
    );
    let (out, capture) = f.run(&t, "run-1");
    let cite = f.citation(&t, &said.id);
    write(
        &capture,
        SUMMARY,
        &summary_file(&project, &t, std::slice::from_ref(&cite)),
    );
    write(&capture, "notes.md", "ふつうの file");

    let attached = attach_now(&f, &capture, &t, &out, "run-1");
    assert_eq!(attached, vec![SUMMARY.to_string(), "notes.md".to_string()]);
    let message = reply(&f, &t, &out);
    assert_eq!(
        message.citations,
        vec![ChatCitationView {
            path: SUMMARY.into(),
            citation: cite.clone(),
            state: ChatCitationState::Current,
        }]
    );
    // 状態は保存しない: `metadata_json.citations` は path と引用だけ。
    let raw: String = rusqlite::Connection::open(f.root.join("celeris.db"))
        .expect("read the db")
        .query_row(
            "SELECT metadata_json FROM chat_messages WHERE id=?1",
            [&out],
            |r| r.get(0),
        )
        .expect("metadata");
    let meta: Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(
        meta["citations"],
        json!([{"path": SUMMARY, "citation": cite}])
    );
    assert!(!raw.contains("\"state\""), "{raw}");
}

#[test]
fn chat_discussion_summary_edit_makes_get_and_sse_edited() {
    let f = fixture();
    let (project, t) = f.project_thread("編集");
    let said = f.post(
        &t,
        "m1",
        "最小の辺を先に測ろう",
        Some(ChatDiscussionMode::Propose),
    );
    let (out, capture) = f.run(&t, "run-1");
    let cite = f.citation(&t, &said.id);
    write(
        &capture,
        SUMMARY,
        &summary_file(&project, &t, std::slice::from_ref(&cite)),
    );
    assert_eq!(
        attach_now(&f, &capture, &t, &out, "run-1"),
        vec![SUMMARY.to_string()]
    );
    f.finish("run-1");

    f.store
        .chat_message_edit(
            &t,
            &said.id,
            ME,
            &ChatMessageEditRequest {
                text: "やはり pilot を続けよう".into(),
                expected_revision: 1,
            },
            at(5),
        )
        .expect("edit");

    assert_eq!(
        reply(&f, &t, &out).citations[0].state,
        ChatCitationState::Edited
    );
    // GET messages（`chat_message_list`）も今の revision から導く。
    let listed = f
        .store
        .chat_message_list(&t, &ChatMessageQuery::default())
        .expect("list")
        .items
        .into_iter()
        .find(|m| m.id == out)
        .expect("reply listed");
    assert_eq!(listed.citations[0].state, ChatCitationState::Edited);
    // SSE の replay（`chat_events_page`）も今の状態を導く。
    let replayed: Vec<ChatCitationState> = f
        .store
        .chat_events_page(&t, &ChatEventQuery::default())
        .expect("events")
        .items
        .into_iter()
        .filter_map(|e| match e.data {
            ChatEventData::Message(d) if d.message.id == out => {
                d.message.citations.first().map(|c| c.state)
            }
            _ => None,
        })
        .collect();
    assert!(!replayed.is_empty(), "the reply has a message event");
    assert!(
        replayed.iter().all(|s| *s == ChatCitationState::Edited),
        "{replayed:?}"
    );
}

#[test]
fn chat_discussion_summary_other_project_citation_not_attached() {
    let f = fixture();
    let (project, t) = f.project_thread("こちら");
    let (_, other) = f.project_thread("別の案件");
    let foreign = f.post(&other, "x1", "別の案件の発言", None);
    let foreign_cite = f.citation(&other, &foreign.id);
    f.post(&t, "m1", "要約して", Some(ChatDiscussionMode::Discuss));
    let (out, capture) = f.run(&t, "run-1");
    write(
        &capture,
        SUMMARY,
        &summary_file(&project, &t, &[foreign_cite]),
    );

    assert!(attach_now(&f, &capture, &t, &out, "run-1").is_empty());
    let message = reply(&f, &t, &out);
    assert!(message.workspace_files.is_empty() && message.citations.is_empty());
    let statuses = f.statuses(&t);
    assert!(
        statuses
            .iter()
            .any(|s| s.starts_with("chat_citation_other_thread: discussion/summary/arch.json")),
        "{statuses:?}"
    );
}

#[test]
fn discussion_mode_propose_attaches_valid_strategy_draft() {
    let f = fixture();
    let (project, t) = f.project_thread("案");
    let said = f.post(
        &t,
        "m1",
        "最小の辺を先に測る案を出して",
        Some(ChatDiscussionMode::Propose),
    );
    let (out, capture) = f.run(&t, "run-1");
    let cite = f.citation(&t, &said.id);
    write(&capture, DRAFT, &draft(&project, &t, &cite, "proposed"));

    assert_eq!(
        attach_now(&f, &capture, &t, &out, "run-1"),
        vec![DRAFT.to_string()],
        "{:?}",
        f.statuses(&t)
    );
    let message = reply(&f, &t, &out);
    assert_eq!(message.citations.len(), 1);
    assert_eq!(message.citations[0].path, DRAFT);
    assert_eq!(message.citations[0].state, ChatCitationState::Current);
    let sha = f
        .store
        .chat_workspace_file_sha256s(&t, DRAFT)
        .expect("sha256s");
    assert_eq!(sha.len(), 1);
    assert_eq!(sha[0].len(), 64);
}

#[test]
fn discussion_mode_propose_refuses_approved_state_draft() {
    let f = fixture();
    let (project, t) = f.project_thread("承認済みと書いた案");
    let said = f.post(&t, "m1", "案を出して", Some(ChatDiscussionMode::Propose));
    let (out, capture) = f.run(&t, "run-1");
    let cite = f.citation(&t, &said.id);
    write(&capture, DRAFT, &draft(&project, &t, &cite, "approved"));

    assert!(attach_now(&f, &capture, &t, &out, "run-1").is_empty());
    assert!(reply(&f, &t, &out).citations.is_empty());
    let statuses = f.statuses(&t);
    assert!(
        statuses
            .iter()
            .any(|s| s.starts_with(&format!("strategy_invalid: {DRAFT}"))),
        "{statuses:?}"
    );
}

#[test]
fn discussion_mode_discuss_refuses_strategy_draft() {
    let f = fixture();
    let (project, t) = f.project_thread("議論だけ");
    let said = f.post(
        &t,
        "m1",
        "議論だけしよう",
        Some(ChatDiscussionMode::Discuss),
    );
    let (out, capture) = f.run(&t, "run-1");
    let cite = f.citation(&t, &said.id);
    write(&capture, DRAFT, &draft(&project, &t, &cite, "proposed"));
    write(
        &capture,
        SUMMARY,
        &summary_file(&project, &t, std::slice::from_ref(&cite)),
    );

    // 要約は付くが、discuss の run の案は付かない。
    assert_eq!(
        attach_now(&f, &capture, &t, &out, "run-1"),
        vec![SUMMARY.to_string()]
    );
    let statuses = f.statuses(&t);
    assert!(
        statuses
            .iter()
            .any(|s| s.starts_with(&format!("discussion_mode_forbidden: {DRAFT}"))),
        "{statuses:?}"
    );
}

#[test]
fn discussion_mode_propose_same_name_other_sha_refused() {
    let f = fixture();
    let (project, t) = f.project_thread("版の衝突");
    let said = f.post(&t, "m1", "案を出して", Some(ChatDiscussionMode::Propose));
    let (out, capture) = f.run(&t, "run-1");
    let cite = f.citation(&t, &said.id);
    write(&capture, DRAFT, &draft(&project, &t, &cite, "proposed"));
    assert_eq!(
        attach_now(&f, &capture, &t, &out, "run-1"),
        vec![DRAFT.to_string()]
    );
    f.finish("run-1");

    f.post(&t, "m2", "もう一度", Some(ChatDiscussionMode::Propose));
    let (out2, capture2) = f.run(&t, "run-2");
    write(&capture2, DRAFT, &draft(&project, &t, &cite, "discussion"));
    assert!(attach_now(&f, &capture2, &t, &out2, "run-2").is_empty());
    let statuses = f.statuses(&t);
    assert!(
        statuses
            .iter()
            .any(|s| s.starts_with(&format!("strategy_idempotency_conflict: {DRAFT}"))),
        "{statuses:?}"
    );
}
