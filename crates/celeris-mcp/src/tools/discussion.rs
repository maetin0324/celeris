//! ADR 2026-11-22-external-llm-discussion-exchange D1: the vendor-neutral discussion tools of an external LLM
//! (ChatGPT・Muse・any MCP client) — `discussion_snapshot`・`discussion_evidence_search`・`discussion_propose`.
//!
//! The tools only translate the MCP arguments and errors. What a client may read (a person's share and its own
//! proposals, masked) and what a propose may record (an unapproved `discuss` draft by `mcp:<client_id>`) are decided by
//! `task_core::discussion_exchange` and the pure rules of `task_core::strategy::envelope` (rule 2: one implementation
//! shared with the REST exchange). Authentication, the per-client scope, `mcp_calls` and the rate limit are the existing
//! ones of `crate::rpc`; the bodies are not copied into `mcp_calls.request` (`super::audit_request` leaves them out).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use task_core::McpScope;
use task_core::chat::ChatCitation;
use task_core::discussion_exchange::{
    DiscussionSnapshot, ExchangeError, ExchangeItem, ExchangeRefusal, ExchangeTarget, ItemKind,
    ProposeRequest, SNAPSHOT_ITEMS_MAX, SnapshotQuery,
};
use task_core::strategy::{Actor, Confidence, DecisionState, Source};

use super::{ToolDef, ToolError, ToolOutput, clamp_limit, schema};
use crate::auth::AuthedClient;
use crate::state::McpState;

/// D1.5: `celeris.discussion-evidence/1`.
pub const EVIDENCE_SCHEMA: &str = "celeris.discussion-evidence/1";
/// D1.3: the search `query` is 1–512 characters.
pub const QUERY_MAX_CHARS: usize = 512;
/// D1.5: a hit's snippet is at most 400 characters (of the masked body).
pub const SNIPPET_MAX_CHARS: usize = 400;
/// D1.6: a `chat` reference that is not an item of the caller's snapshot now.
pub const CITATION_INVALID: &str = "citation_invalid";
/// How many snapshot pages a search or a citation check reads (each ≤ [`SNAPSHOT_ITEMS_MAX`] items).
const SNAPSHOT_PAGES_MAX: usize = 50;

/// The principal of the client in chat and envelope records (D1.6: `mcp:<client_id>`; never `human:`).
fn principal(client: &AuthedClient) -> String {
    format!("mcp:{}", client.id)
}

/// D1.3・D1.6: a refusal of the exchange is a bad argument (`-32602`) or a failed deterministic check (`-32002`);
/// the message carries the code (`thread_project_mismatch`, `R7:idempotency_conflict`, …) and never the body.
fn exchange_error(error: ExchangeError) -> ToolError {
    match error {
        ExchangeError::Refused(r) => refusal_error(&r),
        ExchangeError::Chat(e) => match e.http_status() {
            404 => ToolError::not_found(e.to_string()),
            500.. => ToolError::internal(e.to_string()),
            _ => ToolError::invalid_params(e.to_string()),
        },
    }
}

fn refusal_error(r: &ExchangeRefusal) -> ToolError {
    use task_core::discussion_exchange::{
        INVALID, THREAD_KIND_NOT_PROJECT, THREAD_PROJECT_MISMATCH, TOO_LONG,
    };
    if [
        INVALID,
        TOO_LONG,
        THREAD_KIND_NOT_PROJECT,
        THREAD_PROJECT_MISMATCH,
    ]
    .contains(&r.code)
    {
        ToolError::invalid_params(r.to_string())
    } else {
        ToolError::rejected(r.to_string())
    }
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, ToolError> {
    serde_json::from_value(args).map_err(|e| ToolError::invalid_params(e.to_string()))
}

/// Reads every page of the caller's snapshot of the target (the range a search or a citation may use).
async fn full_snapshot(
    state: &Arc<McpState>,
    viewer: String,
    target: ExchangeTarget,
) -> Result<(DiscussionSnapshot, Vec<ExchangeItem>), ToolError> {
    state
        .blocking(move |store| {
            let now = time::OffsetDateTime::now_utc();
            let mut query = SnapshotQuery {
                target,
                after_seq: 0,
                limit: Some(SNAPSHOT_ITEMS_MAX),
                share_id: None,
            };
            let mut items = Vec::new();
            for page_no in 1.. {
                let mut page = store.chat_exchange_snapshot(&query, &viewer, &[], now)?;
                let more = page.items.len() == SNAPSHOT_ITEMS_MAX
                    && page.next_after_seq > query.after_seq
                    && page_no < SNAPSHOT_PAGES_MAX;
                query.after_seq = page.next_after_seq;
                items.append(&mut page.items);
                if !more {
                    return Ok((page, items));
                }
            }
            unreachable!("the page loop returns")
        })
        .await
        .map_err(exchange_error)
}

// ---- discussion_snapshot ----

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotArgs {
    /// The project; alone it selects the project's fixed CoS thread.
    #[serde(default)]
    pub project_id: Option<String>,
    /// A `project` thread; with `project_id` both must agree.
    #[serde(default)]
    pub thread_id: Option<String>,
    /// Items after this message seq (`next_after_seq` of the previous call).
    #[serde(default)]
    pub after_seq: Option<u64>,
    /// At most this many items (default 50, at most 200).
    #[serde(default)]
    pub limit: Option<usize>,
}

async fn snapshot_impl(
    state: &Arc<McpState>,
    client: &AuthedClient,
    args: Value,
) -> Result<ToolOutput, ToolError> {
    let args: SnapshotArgs = parse(args)?;
    let query = SnapshotQuery {
        target: ExchangeTarget {
            project_id: args.project_id,
            thread_id: args.thread_id,
        },
        after_seq: args.after_seq.unwrap_or(0),
        limit: args.limit,
        share_id: None,
    };
    let viewer = principal(client);
    let snapshot = state
        .blocking(move |store| {
            store.chat_exchange_snapshot(&query, &viewer, &[], time::OffsetDateTime::now_utc())
        })
        .await
        .map_err(exchange_error)?;
    ToolOutput::from_serialize(&snapshot)
}

fn snapshot_call<'a>(
    state: &'a Arc<McpState>,
    client: &'a AuthedClient,
    args: Value,
) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
    Box::pin(snapshot_impl(state, client, args))
}

pub fn snapshot_def() -> ToolDef {
    ToolDef {
        name: "discussion_snapshot",
        description: "案件の CoS thread のうち、人が外部の LLM に共有した message と、この client 自身の propose だけを返す（celeris.discussion-snapshot/1）。project_id だけなら案件の固定の thread。各 item は citation・content_hash・source・actor・consent・confidence・decision_state を持ち、秘密の形は [masked]。共有の無い thread は items が空。続きは next_after_seq を after_seq に渡す。",
        scope: McpScope::DiscussionRead,
        input_schema: schema::<SnapshotArgs>,
        call: snapshot_call,
    }
}

// ---- discussion_evidence_search ----

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    /// 1–512 characters; every whitespace-separated word must appear (case-insensitive).
    pub query: String,
    /// At most this many hits (default 20, at most 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One hit of [`EVIDENCE_SCHEMA`] (D1.5).
#[derive(Debug, Serialize)]
pub struct EvidenceHit {
    pub kind: ItemKind,
    pub citation: ChatCitation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub snippet: String,
    pub references: Vec<Value>,
    pub confidence: Confidence,
    pub decision_state: DecisionState,
    pub actor: Actor,
    pub source: Source,
    pub content_hash: String,
}

#[derive(Debug, Serialize)]
pub struct EvidenceOutput {
    pub schema: &'static str,
    pub project_id: String,
    pub thread_id: String,
    pub hits: Vec<EvidenceHit>,
}

/// D1.5: the snippet of `body` around the first word of `terms` (all lowercase), at most [`SNIPPET_MAX_CHARS`].
/// `None` unless every word appears. The body is the masked one of the snapshot, so a search cannot find a
/// masked secret.
fn snippet(body: &str, terms: &[String]) -> Option<String> {
    let lowered = body.to_lowercase();
    let mut first = usize::MAX;
    for term in terms {
        first = first.min(lowered.find(term.as_str())?);
    }
    // Lowercasing keeps the char count for almost every text; if not, start the snippet at the beginning.
    let at = if lowered.chars().count() == body.chars().count() {
        lowered[..first].chars().count()
    } else {
        0
    };
    Some(
        body.chars()
            .skip(at.saturating_sub(SNIPPET_MAX_CHARS / 4))
            .take(SNIPPET_MAX_CHARS)
            .collect(),
    )
}

async fn search_impl(
    state: &Arc<McpState>,
    client: &AuthedClient,
    args: Value,
) -> Result<ToolOutput, ToolError> {
    let args: SearchArgs = parse(args)?;
    let query = args.query.trim();
    if query.is_empty() || args.query.chars().count() > QUERY_MAX_CHARS {
        return Err(ToolError::invalid_params(format!(
            "query is 1-{QUERY_MAX_CHARS} characters"
        )));
    }
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let limit = clamp_limit(args.limit);
    let target = ExchangeTarget {
        project_id: args.project_id,
        thread_id: args.thread_id,
    };
    let (snapshot, items) = full_snapshot(state, principal(client), target).await?;
    let hits = items
        .into_iter()
        .filter_map(|item| {
            let snippet = snippet(&item.body, &terms)?;
            Some(EvidenceHit {
                kind: item.kind,
                citation: item.citation,
                path: item.path,
                snippet,
                references: item.references,
                confidence: item.confidence,
                decision_state: item.decision_state,
                actor: item.actor,
                source: item.source,
                content_hash: item.content_hash,
            })
        })
        .take(limit)
        .collect();
    ToolOutput::from_serialize(&EvidenceOutput {
        schema: EVIDENCE_SCHEMA,
        project_id: snapshot.project_id,
        thread_id: snapshot.thread_id,
        hits,
    })
}

fn search_call<'a>(
    state: &'a Arc<McpState>,
    client: &'a AuthedClient,
    args: Value,
) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
    Box::pin(search_impl(state, client, args))
}

pub fn search_def() -> ToolDef {
    ToolDef {
        name: "discussion_evidence_search",
        description: "discussion_snapshot と同じ範囲（人が共有した message とこの client の propose）だけを語で探し、出典（citation）・snippet（400 文字まで、秘密は [masked]）・references・confidence・decision_state 付きで返す（celeris.discussion-evidence/1）。共有の外の message と knowledge は返さない。",
        scope: McpScope::DiscussionRead,
        input_schema: schema::<SearchArgs>,
        call: search_call,
    }
}

// ---- discussion_propose ----

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProposeArgs {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    /// 1–128 of `[A-Za-z0-9._:-]`. The same key and body record one draft; another body with the key is refused.
    pub idempotency_key: String,
    /// 1–64 of `[a-z0-9._-]` (e.g. `chatgpt`, `muse`).
    pub source_label: String,
    /// The draft (at most 16,384 bytes). Secrets are refused.
    pub body: String,
    /// At most 32 objects. A `{"kind":"chat","thread_id","message_id","revision","sha256"}` reference must be an
    /// item of this client's snapshot now; a `{"kind":"url","url","note"}` is kept as text and never fetched.
    #[serde(default)]
    pub references: Vec<Value>,
    /// `confirmed`・`estimated`・`contradicted`・`needs_decision`.
    pub confidence: String,
    /// `discussion` or `proposed` (an external text is never `approved`).
    pub decision_state: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProposeOutput {
    pub message_id: String,
    pub thread_id: String,
    pub project_id: String,
    pub seq: u64,
    pub content_hash: String,
    pub decision_state: String,
    pub duplicate: bool,
}

fn enum_field<T: serde::de::DeserializeOwned>(name: &str, raw: &str) -> Result<T, ToolError> {
    serde_json::from_value(Value::String(raw.to_owned()))
        .map_err(|_| ToolError::invalid_params(format!("{name} {raw:?} is not a known value")))
}

/// D1.6: every `chat` reference names an item of the caller's snapshot at its revision and sha256.
fn citations_ok(references: &[Value], items: &[ExchangeItem]) -> Result<(), ToolError> {
    for (i, r) in references.iter().enumerate() {
        if r.get("kind").and_then(Value::as_str) != Some("chat") {
            continue;
        }
        let cited: ChatCitation = {
            let mut r = r.clone();
            if let Some(obj) = r.as_object_mut() {
                obj.remove("kind");
            }
            serde_json::from_value(r).map_err(|e| {
                ToolError::rejected(format!("{CITATION_INVALID}: references[{i}]: {e}"))
            })?
        };
        if !items.iter().any(|item| item.citation == cited) {
            return Err(ToolError::rejected(format!(
                "{CITATION_INVALID}: references[{i}] is not a message of this client's snapshot at that revision"
            )));
        }
    }
    Ok(())
}

async fn propose_impl(
    state: &Arc<McpState>,
    client: &AuthedClient,
    args: Value,
) -> Result<ToolOutput, ToolError> {
    let args: ProposeArgs = parse(args)?;
    let req = ProposeRequest {
        project_id: args.project_id,
        thread_id: args.thread_id,
        idempotency_key: args.idempotency_key,
        source_label: args.source_label,
        body: args.body,
        references: args.references,
        confidence: enum_field("confidence", &args.confidence)?,
        decision_state: enum_field("decision_state", &args.decision_state)?,
    };
    let caller = principal(client);
    if req
        .references
        .iter()
        .any(|r| r.get("kind").and_then(Value::as_str) == Some("chat"))
    {
        let target = ExchangeTarget {
            project_id: req.project_id.clone(),
            thread_id: req.thread_id.clone(),
        };
        let (_, items) = full_snapshot(state, caller.clone(), target).await?;
        citations_ok(&req.references, &items)?;
    }
    let scopes: Vec<String> = client
        .scopes
        .iter()
        .map(|s| s.as_str().to_owned())
        .collect();
    let decision_state = args.decision_state;
    let received = state
        .blocking(move |store| {
            store.chat_exchange_propose(&caller, &scopes, &req, time::OffsetDateTime::now_utc())
        })
        .await
        .map_err(exchange_error)?;
    ToolOutput::from_serialize(&ProposeOutput {
        message_id: received.message.id,
        thread_id: received.message.thread_id,
        project_id: received.project_id,
        seq: received.message.seq,
        content_hash: received.content_hash,
        decision_state,
        duplicate: received.duplicate,
    })
}

fn propose_call<'a>(
    state: &'a Arc<McpState>,
    client: &'a AuthedClient,
    args: Value,
) -> Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send + 'a>> {
    Box::pin(propose_impl(state, client, args))
}

pub fn propose_def() -> ToolDef {
    ToolDef {
        name: "discussion_propose",
        description: "案件の CoS thread に未承認の案（draft）を 1 件記録する。author・actor は mcp:<client_id> で、人の発言・承認には数えない。mode は常に discuss で、これに答える CoS run は task の起票・Roadmap の変更・決定への回答・publish をできない。decision_state は discussion か proposed だけ。同じ idempotency_key と本文の繰り返しは 1 件（duplicate: true）、別の本文は R7:idempotency_conflict。秘密を含む本文は拒否。",
        scope: McpScope::DiscussionPropose,
        input_schema: schema::<ProposeArgs>,
        call: propose_call,
    }
}
