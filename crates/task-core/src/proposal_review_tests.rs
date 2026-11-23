//! ADR 2026-11-23-project-observability-publish D2.2・D2.4: the pure parts (kind, external mark, state, fixed text).

use serde_json::json;

use super::*;
use crate::chat_citation::{ChatCitation, CitedMessage};
use crate::proposal_review_wire::{
    ProposalKind, ProposalReviewRecord, ProposalReviewState, ReviewMessage, external_mark,
    proposal_kind, proposal_review_state,
};

fn now(revision: u64, sha: &str) -> CitedMessage {
    CitedMessage {
        thread_id: "t".into(),
        message_id: "p".into(),
        project_id: None,
        general_thread: false,
        seq: 0,
        revision,
        sha256: sha.into(),
        retracted: false,
    }
}

fn review(id: &str, author: &str, verdict: ProposalVerdict, sha: &str) -> ReviewMessage {
    ReviewMessage {
        message_id: id.into(),
        author: author.into(),
        created_at: "2026-11-23T00:00:00Z".into(),
        record: ProposalReviewRecord {
            verdict,
            target: ChatCitation {
                thread_id: "t".into(),
                message_id: "p".into(),
                revision: 1,
                sha256: sha.into(),
            },
            attachment: None,
        },
    }
}

/// Invariant: only a person's review counts (a record under another author never adopts, T1・T2), the last adopt /
/// reject decides over later comments, only comments read `commented`, and a review of another body is `stale`.
#[test]
fn proposal_review_state_follows_last_verdict_of_a_person() {
    let sha = "a".repeat(64);
    let state = |rs: &[ReviewMessage], s: &str| {
        let s = s.to_owned();
        proposal_review_state(ProposalKind::ExternalLlm, rs, move |_| Some(now(1, &s))).review_state
    };
    assert_eq!(state(&[], &sha), ProposalReviewState::Pending);
    let forged = review("r0", "mcp:chatgpt", ProposalVerdict::Adopt, &sha);
    assert_eq!(
        state(std::slice::from_ref(&forged), &sha),
        ProposalReviewState::Pending
    );
    let adopt = review("r1", "human:alice", ProposalVerdict::Adopt, &sha);
    let reject = review("r2", "human:alice", ProposalVerdict::Reject, &sha);
    let comment = review("r3", "human:alice", ProposalVerdict::Comment, &sha);
    assert_eq!(
        state(&[adopt.clone(), comment.clone()], &sha),
        ProposalReviewState::Adopted
    );
    assert_eq!(
        state(&[adopt.clone(), reject, comment.clone()], &sha),
        ProposalReviewState::Rejected
    );
    assert_eq!(
        state(std::slice::from_ref(&comment), &sha),
        ProposalReviewState::Commented
    );
    assert_eq!(state(&[adopt], &"b".repeat(64)), ProposalReviewState::Stale);
}

/// Invariant: kinds and marks come from the record (`role`, `author`, `metadata_json.exchange`, the workspace
/// files), never from the text; the general CoS's `cos_proposal` and a person's own message are not proposals.
#[test]
fn proposal_review_kind_and_mark_come_from_the_record() {
    let mcp = json!({"exchange": {"source": {"kind": "mcp_client", "label": "muse"}}});
    assert_eq!(
        proposal_kind("user", Some("mcp:muse"), &mcp),
        Some(ProposalKind::ExternalLlm)
    );
    assert_eq!(
        external_mark(&mcp).map(|m| (m.source_label, m.untrusted)),
        Some(("muse".into(), true))
    );
    let paste = json!({"exchange": {"source": {"kind": "human_paste", "label": "chatgpt"}}});
    assert_eq!(
        proposal_kind("user", Some("human:alice"), &paste),
        Some(ProposalKind::HumanImport)
    );
    let draft = json!({"workspace_files": [{"path": "strategy/s1/v2.json", "attachment_id": "a"}]});
    assert_eq!(
        proposal_kind("assistant", None, &draft),
        Some(ProposalKind::CosStrategy)
    );
    let summary =
        json!({"workspace_files": [{"path": "discussion/summary/x.json", "attachment_id": "a"}]});
    assert_eq!(proposal_kind("assistant", None, &summary), None);
    let cos_proposal = json!({"cos_proposal": {"proposed_by": "cos:g"}});
    assert_eq!(proposal_kind("system", None, &cos_proposal), None);
    assert_eq!(proposal_kind("user", Some("human:alice"), &json!({})), None);
    assert!(external_mark(&json!({})).is_none());
}

/// Invariant: the review text is Rust's fixed sentence; an adopt says it is not an approval and never copies the
/// proposal's words.
#[test]
fn proposal_review_text_is_fixed_and_adopt_is_not_approval() {
    let target = ChatCitation {
        thread_id: "t".into(),
        message_id: "p1".into(),
        revision: 2,
        sha256: "a".repeat(64),
    };
    let adopt = review_text(
        ProposalVerdict::Adopt,
        &target,
        Some("strategy/s1/v3.json"),
        None,
    );
    assert!(adopt.contains("strategy/s1/v3.json") && adopt.contains("strategy-approve-v3"));
    assert!(adopt.contains("これは承認ではない") && adopt.contains("rev 2"));
    let comment = review_text(ProposalVerdict::Comment, &target, None, Some("根拠は?"));
    assert!(comment.ends_with("\n\n根拠は?"));
}
