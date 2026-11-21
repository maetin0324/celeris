//! 子 0 の設計 brief §5 の共有フォーマット（discussion envelope）の検査（規則 R1〜R13）。
//!
//! 参照の検査器は `agent-docs/progress/2026-11-21-strategy-v1-0/fixtures/validate.py`。議論・引用の 1 件（CoS thread の
//! message・人の貼り付け・MCP の client・Markdown の import）を受け取るときの形で、子 3・4 が使う。source と actor の組・consent・
//! 秘密の形・人の主体は戦略の文書（[`super::validate_document`]）と同じ関数を使う。

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{
    ActorKind, Confidence, Consent, DecisionState, SourceKind, actor_allowed, consent_granted,
    contains_secret,
};
use crate::project_proposal::sha256_hex;

/// envelope の本文の上限（byte、UTF-8）。
pub const BODY_MAX_BYTES: usize = 16_384;

const REQUIRED: [&str; 12] = [
    "project_id",
    "source",
    "actor",
    "source_revision",
    "timestamp",
    "content_hash",
    "consent",
    "idempotency_key",
    "decision_state",
    "confidence",
    "references",
    "body",
];

static INJECT: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(ignore (all |the )?previous|you are now|承認したことに|以前の指示を無視|approve (this|the) (decision|plan)|publish (it|this) now)",
    )
    .ok()
});

/// 照合の文脈: thread の案件と、既に受けた idempotency の key → content_hash（`sha256:<hex>`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct EnvelopeContext {
    pub project_id: String,
    #[serde(default)]
    pub idempotency: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Approval {
    #[serde(default)]
    by: String,
    #[serde(default)]
    via: String,
}

fn field<T: DeserializeOwned>(env: &Value, path: &[&str]) -> Option<T> {
    let v = path.iter().try_fold(env, |v, k| v.get(*k))?;
    serde_json::from_value(v.clone()).ok()
}

/// envelope を検査し、違反した規則の id（`R4:content_hash` など）を昇順で返す。空なら受理。
pub fn validate_envelope(env: &Value, ctx: &EnvelopeContext) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut fail = |hit: bool, rule: &'static str| {
        if hit {
            out.push(rule);
        }
    };
    let source: Option<SourceKind> = field(env, &["source", "kind"]);
    let actor: Option<ActorKind> = field(env, &["actor", "kind"]);
    let state: Option<DecisionState> = field(env, &["decision_state"]);
    let confidence: Option<Confidence> = field(env, &["confidence"]);
    let body: String = field(env, &["body"]).unwrap_or_default();
    let content_hash: Option<String> = field(env, &["content_hash"]);
    let approval: Option<Approval> = field(env, &["approval"]);
    let external = source.is_some_and(SourceKind::is_external);
    let open_state = matches!(
        state,
        Some(DecisionState::Discussion | DecisionState::Proposed)
    );
    let human_approval = |vias: &[&str]| {
        approval.as_ref().is_some_and(|a| {
            crate::is_human_principal(&a.by) && (vias.is_empty() || vias.contains(&a.via.as_str()))
        })
    };

    fail(
        REQUIRED.iter().any(|k| env.get(*k).is_none()),
        "R1:required",
    );
    // envelope の source は 4 種だけ（`celeris_task_run` は戦略の文書の source）。state・confidence は決めた語だけ。
    fail(
        source.is_none_or(|s| s == SourceKind::CelerisTaskRun)
            || state.is_none()
            || confidence.is_none(),
        "R2:source_kind",
    );
    fail(
        !matches!((source, actor), (Some(s), Some(a)) if actor_allowed(s, a)),
        "R3:actor_mismatch",
    );
    let own_hash = format!("sha256:{}", sha256_hex(&body));
    fail(
        content_hash.as_deref() != Some(own_hash.as_str()),
        "R4:content_hash",
    );
    fail(body.len() > BODY_MAX_BYTES, "R5:body_too_long");
    fail(
        field::<String>(env, &["project_id"]).as_deref() != Some(ctx.project_id.as_str()),
        "R6:project_mismatch",
    );
    let key: Option<String> = field(env, &["idempotency_key"]);
    let seen = key.and_then(|k| ctx.idempotency.get(&k));
    fail(
        seen.is_some_and(|h| Some(h) != content_hash.as_ref()),
        "R7:idempotency_conflict",
    );
    let consent: Option<Consent> = field(env, &["consent"]);
    fail(
        external && !consent.as_ref().is_some_and(consent_granted),
        "R8:consent",
    );
    fail(contains_secret(&body), "R9:secret");
    fail(
        state == Some(DecisionState::Approved) && !human_approval(&["web", "api_human_token"]),
        "R10:approval_needs_human",
    );
    let injected = INJECT.as_ref().is_none_or(|re| re.is_match(&body));
    fail(
        (external || injected) && !open_state,
        "R11:external_cannot_decide",
    );
    let refs: Vec<Value> = field(env, &["references"]).unwrap_or_default();
    fail(
        confidence == Some(Confidence::Confirmed) && refs.is_empty(),
        "R12:unsupported_claim",
    );
    let outbound = field::<String>(env, &["direction"]).as_deref() == Some("outbound");
    let confidential = field::<String>(env, &["classification"]).as_deref() == Some("confidential");
    fail(
        outbound && confidential && !human_approval(&[]),
        "R13:outbound_confidential",
    );
    out.sort_unstable();
    out.dedup();
    out
}
