//! ADR 2026-11-21-chat-discussion-mode D3・D6: the check of the draft and summary files a CoS
//! reply would attach (`strategy/<id>/v<n>.json`, `discussion/summary/<name>.json`).
//!
//! 決定的（LLM も CoS の申告も使わない）。通らない file は添付せず、code を返す（呼び出し側が run の
//! status に出す）。通った file の引用の列は、添付と同じ transaction で返事の `metadata_json.citations`
//! に 1 回だけ書く（`chat_message_attach_workspace_files` がその transaction で引用を検査し直す）。
//! - 案: 子 2 の `task_core::strategy::validate_document`（intent は記録。`approved`・`rejected` は
//!   `strategy_invalid`、同じ path に別の sha256 があれば `strategy_idempotency_conflict`）と D3 の固定値
//!   （source・actor・project_id）、各 `chat` 参照の [`verify_citation`]。discuss の run の案は
//!   `discussion_mode_forbidden`。
//! - 要約: `celeris.discussion-summary/1` の [`check_discussion_summary`]。
//!
//! `dispatcher::cos_chat::workspace_files::attach` は run ごとに [`AttachGate`] を 1 つ作り、各 file を
//! [`AttachGate::upload`] で保存し、最後に [`AttachGate::finish`] で返事に pin する（dispatcher の外に置く。
//! repo health の `size.responsibility-growth`）。

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;

use task_core::SqliteStore;
use task_core::chat::attachments::{
    AttachmentError, ChatAttachmentLimits, ChatAttachmentStore, StoredAttachment,
};
use task_core::chat::{
    ChatDiscussionMode, ChatError, ChatFileCitation, ChatStatusPhase, ChatWorkspaceFile,
};
use task_core::chat_citation::{
    ChatCitation, CitedMessage, DiscussionSummary, SummaryContext, check_discussion_summary,
    verify_citation,
};
use task_core::org::ProjectId;
use task_core::strategy::{
    self, Actor, ActorKind, ArtifactFacts, CheckContext, DecisionState, Intent, RefStatus,
    Reference, SourceKind, StrategyDocument,
};

/// D2.3 の code（discuss の run の案）。
pub(crate) const DISCUSSION_MODE_FORBIDDEN: &str = "discussion_mode_forbidden";
/// D3 の固定値（source・actor・project_id）と違う案。
pub(crate) const STRATEGY_CHAT_FIXED: &str = "strategy_invalid";
/// 案の置けない thread（案件の無い統括の thread）。
pub(crate) const STRATEGY_NO_PROJECT: &str = "strategy_other_project";
/// 検査に要る事実が読めない（fail closed）。
pub(crate) const CHECK_UNAVAILABLE: &str = "discussion_file_check_unavailable";

/// file の種類（workspace の相対 path から決まる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiscussionFile {
    /// `strategy/<id>/v<n>.json`（D3）。
    Strategy,
    /// `discussion/summary/<name>.json`（D6）。
    Summary,
}

pub(crate) fn kind_of(rel: &str) -> Option<DiscussionFile> {
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        ["strategy", id, file] if !id.is_empty() && file.ends_with(".json") => {
            Some(DiscussionFile::Strategy)
        }
        ["discussion", "summary", file]
            if file.len() > ".json".len() && file.ends_with(".json") =>
        {
            Some(DiscussionFile::Summary)
        }
        _ => None,
    }
}

/// 1 つの run の workspace file の添付。案と要約は検査を通ったものだけ保存し、その引用を添付と一緒に書く。
/// 他の file は今までどおり保存する。
pub(crate) struct AttachGate<'a> {
    store: &'a SqliteStore,
    attachments: &'a ChatAttachmentStore,
    thread_id: &'a str,
    run_id: &'a str,
    limits: ChatAttachmentLimits,
    facts: Option<ReplyFacts>,
    citations: Vec<ChatFileCitation>,
    refused: Vec<(String, String)>,
}

impl<'a> AttachGate<'a> {
    pub(crate) fn new(
        store: &'a SqliteStore,
        attachments: &'a ChatAttachmentStore,
        thread_id: &'a str,
        run_id: &'a str,
        limits: ChatAttachmentLimits,
    ) -> Self {
        Self {
            store,
            attachments,
            thread_id,
            run_id,
            limits,
            facts: None,
            citations: Vec::new(),
            refused: Vec::new(),
        }
    }

    /// `file`（workspace の `rel`）を保存する。案・要約で検査を通らない、または読めないなら保存せず
    /// `None`（通らない code は [`Self::finish`] が run の status に出す）。
    pub(crate) fn upload(
        &mut self,
        rel: &str,
        key: &str,
        name: &str,
        size: u64,
        file: File,
        now: time::OffsetDateTime,
    ) -> Option<Result<StoredAttachment, AttachmentError>> {
        let (thread_id, attachments) = (self.thread_id, self.attachments);
        let Some(kind) = kind_of(rel) else {
            return Some(attachments.upload(thread_id, key, name, Some(size), file, now));
        };
        let mut bytes = Vec::new();
        if let Err(error) = file
            .take(self.limits.max_file_bytes)
            .read_to_end(&mut bytes)
        {
            tracing::debug!(%rel, %error, "CoS workspace file unreadable; not attached");
            return None;
        }
        let sha256 = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&bytes));
        let (store, run_id) = (self.store, self.run_id);
        let facts = self
            .facts
            .get_or_insert_with(|| ReplyFacts::read(store, thread_id, run_id));
        let cited = match check(store, facts, kind, rel, &bytes, &sha256) {
            Ok(cited) => cited,
            Err(code) => {
                self.refused.push((rel.to_string(), code.to_string()));
                return None;
            }
        };
        let uploaded = attachments.upload(thread_id, key, name, Some(size), &bytes[..], now);
        if uploaded.is_ok() {
            self.citations.extend(cited);
        }
        Some(uploaded)
    }

    /// `files` を出力 message に pin し、引用を同じ transaction で書く。その間に引いた発言が変わって
    /// いれば（409）引用を持つ file を外して pin し直す。外した file と検査を通らなかった file は code を
    /// run の status に出す。`files` は pin した file だけになる。
    pub(crate) fn finish(
        mut self,
        message_id: &str,
        files: &mut Vec<ChatWorkspaceFile>,
        now: time::OffsetDateTime,
    ) -> Result<(), String> {
        let (store, thread_id) = (self.store, self.thread_id);
        let attach = |files: &[ChatWorkspaceFile], citations: &[ChatFileCitation]| {
            if files.is_empty() {
                return Ok(());
            }
            store
                .chat_message_attach_workspace_files(thread_id, message_id, files, citations, now)
                .map(drop)
        };
        match attach(files, &self.citations) {
            Err(ChatError::Conflict(reason)) if !self.citations.is_empty() => {
                let cited: HashSet<&str> = self.citations.iter().map(|c| c.path.as_str()).collect();
                for path in &cited {
                    self.refused.push(((*path).to_string(), reason.clone()));
                }
                files.retain(|f| !cited.contains(f.path.as_str()));
                attach(files, &[]).map_err(|e| format!("attach workspace files: {e}"))?;
            }
            other => other.map_err(|e| format!("attach workspace files: {e}"))?,
        }
        for (rel, code) in self.refused {
            let summary = format!("{code}: {rel} は添付しなかった");
            if let Err(error) =
                store.chat_run_status(self.run_id, ChatStatusPhase::Working, &summary, now)
            {
                tracing::warn!(%error, run_id = %self.run_id, "CoS discussion file refusal not recorded");
            }
        }
        Ok(())
    }
}

/// 返事の thread と run の事実（run ごとに 1 回読む）。
pub(crate) struct ReplyFacts {
    pub thread_id: String,
    pub run_id: String,
    pub project_id: Option<String>,
    /// D1.2 の run の mode（読めなければ discuss とみなす: fail closed）。
    pub mode: ChatDiscussionMode,
}

impl ReplyFacts {
    pub(crate) fn read(store: &SqliteStore, thread_id: &str, run_id: &str) -> Self {
        let project_id = store
            .chat_thread_get(thread_id)
            .ok()
            .flatten()
            .and_then(|t| t.project_id);
        let mode = store
            .chat_run_discussion_mode(thread_id, run_id)
            .map(|m| m.mode)
            .unwrap_or(ChatDiscussionMode::Discuss);
        Self {
            thread_id: thread_id.to_string(),
            run_id: run_id.to_string(),
            project_id,
            mode,
        }
    }
}

/// `bytes`（`rel` の file、Celeris が計算した `sha256`）を添付してよいか。よければ引用の列。
pub(crate) fn check(
    store: &SqliteStore,
    facts: &ReplyFacts,
    kind: DiscussionFile,
    rel: &str,
    bytes: &[u8],
    sha256: &str,
) -> Result<Vec<ChatFileCitation>, &'static str> {
    let citations = match kind {
        DiscussionFile::Strategy => check_strategy(store, facts, rel, bytes, sha256)?,
        DiscussionFile::Summary => check_summary(store, facts, bytes)?,
    };
    Ok(citations
        .into_iter()
        .map(|citation| ChatFileCitation {
            path: rel.to_string(),
            citation,
        })
        .collect())
}

fn cited(store: &SqliteStore, c: &ChatCitation) -> Result<Option<CitedMessage>, &'static str> {
    store
        .chat_cited_message(&c.thread_id, &c.message_id)
        .map_err(|_| CHECK_UNAVAILABLE)
}

fn check_summary(
    store: &SqliteStore,
    facts: &ReplyFacts,
    bytes: &[u8],
) -> Result<Vec<ChatCitation>, &'static str> {
    let summary = DiscussionSummary::parse(bytes).map_err(|v| v.code)?;
    let mut now = Vec::new();
    for c in &summary.citations {
        now.extend(cited(store, c)?);
    }
    let ctx = SummaryContext {
        thread_id: &facts.thread_id,
        project_id: facts.project_id.as_deref(),
        cited: &now,
    };
    match check_discussion_summary(&summary, &ctx).first() {
        Some(v) => Err(v.code),
        None => Ok(summary.citations),
    }
}

fn check_strategy(
    store: &SqliteStore,
    facts: &ReplyFacts,
    rel: &str,
    bytes: &[u8],
    sha256: &str,
) -> Result<Vec<ChatCitation>, &'static str> {
    if facts.mode == ChatDiscussionMode::Discuss {
        return Err(DISCUSSION_MODE_FORBIDDEN);
    }
    let Some(project_id) = facts.project_id.clone() else {
        return Err(STRATEGY_NO_PROJECT);
    };
    let project = project_id
        .parse::<ProjectId>()
        .ok()
        .and_then(|id| store.project_get(id).ok().flatten())
        .ok_or(CHECK_UNAVAILABLE)?;
    let roadmap_version = store
        .roadmap_approved(&project_id)
        .map_err(|_| CHECK_UNAVAILABLE)?
        .map(|v| v.version);
    let doc: Option<StrategyDocument> = serde_json::from_slice(bytes).ok();
    let mut references = Vec::new();
    let mut citations = Vec::new();
    if let Some(doc) = &doc {
        fixed_values(doc, facts, &project_id)?;
        for reference in doc_references(doc) {
            if let Reference::Chat(c) = reference {
                let now = cited(store, c)?;
                verify_citation(&facts.thread_id, c, now.as_ref()).map_err(|r| r.code())?;
                if let Some(m) = now {
                    references.push((reference.clone(), RefStatus::Cited(m)));
                }
                if !citations.contains(c) {
                    citations.push(c.clone());
                }
            }
        }
    }
    let ctx = CheckContext {
        intent: Intent::Record,
        project_id: project_id.clone(),
        request_sha256: task_core::project_proposal::sha256_hex(&project.request),
        roadmap_version,
        artifact: ArtifactFacts {
            name: rel.to_string(),
            sha256: sha256.to_string(),
            run_id: facts.run_id.clone(),
        },
        key_sha256s: store
            .chat_workspace_file_sha256s(&facts.thread_id, rel)
            .map_err(|_| CHECK_UNAVAILABLE)?,
        chain: Vec::new(),
        approval: None,
        latest_approved_version: None,
        references,
    };
    match strategy::validate_document(bytes, &ctx).first() {
        Some(v) => Err(v.rule.code()),
        None => Ok(citations),
    }
}

/// D3: a chat draft is CoS's, of this thread, of the thread's project, and not decided.
fn fixed_values(
    doc: &StrategyDocument,
    facts: &ReplyFacts,
    project_id: &str,
) -> Result<(), &'static str> {
    let source_ok = doc.source.as_ref().is_some_and(|s| {
        s.kind == SourceKind::CelerisCosThread
            && s.thread_id.as_deref() == Some(facts.thread_id.as_str())
    });
    let actor = Actor {
        kind: ActorKind::Cos,
        id: format!("cos:{}", facts.thread_id),
    };
    let state_ok = matches!(
        doc.decision_state,
        DecisionState::Discussion | DecisionState::Proposed
    );
    if !state_ok || !source_ok || doc.actor != actor || doc.project_id != project_id {
        return Err(STRATEGY_CHAT_FIXED);
    }
    Ok(())
}

fn doc_references(doc: &StrategyDocument) -> impl Iterator<Item = &Reference> {
    doc.research_question
        .references
        .iter()
        .chain(doc.evidence.iter().flat_map(|e| e.references.iter()))
}

#[cfg(test)]
#[path = "discussion_files_tests.rs"]
mod tests;
