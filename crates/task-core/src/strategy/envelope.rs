//! 子 0 の設計 brief §5 の共有フォーマット（discussion envelope）の検査（規則 R1〜R13）。
//!
//! 議論・引用の 1 件（CoS thread の message・人の貼り付け・MCP の client・Markdown の import）を受け取る・外へ出すときの形で、
//! 子 3・4 が使う。source と actor の組・consent・秘密の形・人の主体は戦略の文書（[`super::validate_document`]）と同じ関数を使う。
//!
//! 承認（R10・R13）と主体（R3・R8）は envelope の中の自己申告から読まず、呼び出し側が store から集めて [`EnvelopeContext`] に
//! 渡した記録（決定の `DecisionAnswered`・人の共有の event・認証した主体）から導く（ADR 2026-11-22-external-llm-discussion-exchange D5）。
//! R8・R9・R11・R13 とマスクは、取り込み・書き出し・MCP が同じ規則を使うように公開の純関数にしてある（規約 2）。

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub use super::contains_secret;
use super::{
    APPROVE_OPTION, ActorKind, ApprovalRecord, Confidence, Consent, DecisionState, SECRET,
    SourceKind, actor_allowed, approval_decision_key, approval_subject, bare_sha, consent_granted,
};
use crate::decision::DecisionStatus;
use crate::is_human_principal;
use crate::project_proposal::sha256_hex;

/// envelope の本文の上限（byte、UTF-8）。
pub const BODY_MAX_BYTES: usize = 16_384;

/// MCP の client が案を記録するのに要る scope（ADR D1.1 の `discussion:propose`）。
pub const MCP_PROPOSE_SCOPE: &str = "discussion:propose";

/// MCP の client の propose に Celeris が埋める consent の scope（ADR D1.6）。
pub const MCP_PROPOSE_CONSENT_SCOPE: &str = "mcp_scope:discussion:propose";

/// マスクした秘密の置き換え（ADR D2.4）。
pub const MASK: &str = "[masked]";

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

/// 人の共有の event（ADR D2.1 の `exchange_shared`）から呼び出し側が導いた、外へ出すことの許可。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutboundShare {
    /// 共有した主体（event の `by`）。
    pub by: String,
    /// 共有した内容の sha256（`sha256:<hex>` か `<hex>`）。
    pub sha256s: Vec<String>,
    pub include_confidential: bool,
}

/// 照合の文脈（呼び出し側が store と認証から集める。envelope の中の自己申告は使わない）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvelopeContext {
    /// thread の案件。
    pub project_id: String,
    /// 既に受けた idempotency の key → content_hash（`sha256:<hex>`）。
    pub idempotency: BTreeMap<String, String>,
    /// 認証した主体（`human:<名前>` か `mcp:<client_id>`）。受け取り（`direction` が `outbound` でない）の外部の source で使う。
    pub caller: String,
    /// MCP の client の scope（MCP のとき）。
    pub caller_scopes: Vec<String>,
    /// envelope の `content_hash` を承認する決定（`decisions` の行と最後の `DecisionAnswered`）。
    pub approval: Option<ApprovalRecord>,
    /// 外へ出すときの人の共有。
    pub outbound_share: Option<OutboundShare>,
}

/// マスクの結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Masked {
    pub text: String,
    /// 置き換えた箇所の数。
    pub masked: usize,
}

fn field<T: DeserializeOwned>(env: &Value, path: &[&str]) -> Option<T> {
    let v = path.iter().try_fold(env, |v, k| v.get(*k))?;
    serde_json::from_value(v.clone()).ok()
}

fn is_mcp_principal(by: &str) -> bool {
    by.strip_prefix("mcp:").is_some_and(|rest| !rest.is_empty())
}

/// 注入文（以前の指示の無視・承認や publish の指示）の形を含むか。正規表現が組めなければ fail-closed で `true`。
pub fn contains_injection(text: &str) -> bool {
    INJECT.as_ref().is_none_or(|re| re.is_match(text))
}

/// R11: 外部の source の文と注入文を含む文は `discussion`・`proposed` から進めない。その状態を持てるなら `true`。
pub fn state_allowed(source: SourceKind, body: &str, state: DecisionState) -> bool {
    matches!(state, DecisionState::Discussion | DecisionState::Proposed)
        || !(source.is_external() || contains_injection(body))
}

/// R10: `content_hash`（`sha256:<hex>`）を、人（`human:<名前>`）が承認の決定で承認したか。決定は D2.2 の
/// `strategy-approve-v<n>` で、問いの sha256 が `content_hash` と一致し、answered で、最後の回答が `approve` であること。
/// `mcp:`・`cos:`・`operator:` の回答は承認にならない。
pub fn human_approved(approval: Option<&ApprovalRecord>, content_hash: &str) -> bool {
    let Some(a) = approval else {
        return false;
    };
    let Some(subject) = approval_subject(&a.question) else {
        return false;
    };
    a.key == approval_decision_key(subject.version)
        && a.status == DecisionStatus::Answered
        && content_hash.strip_prefix("sha256:") == Some(subject.sha256.as_str())
        && matches!(&a.last_answer, Some((option, by)) if option == APPROVE_OPTION && is_human_principal(by))
}

/// R13: confidential の内容（`content_hash`）を外へ出してよいか。人が `include_confidential` でその内容を共有したか、
/// R10 と同じ人の承認の決定があるとき。
pub fn outbound_allowed(
    content_hash: &str,
    share: Option<&OutboundShare>,
    approval: Option<&ApprovalRecord>,
) -> bool {
    let shared = share.is_some_and(|s| {
        is_human_principal(&s.by)
            && s.include_confidential
            && s.sha256s
                .iter()
                .any(|h| bare_sha(h) == bare_sha(content_hash))
    });
    shared || human_approved(approval, content_hash)
}

/// R8: 外部の source（[`SourceKind::is_external`]）の consent。記録の形は、人の貼り付け・Markdown の取り込みなら人の
/// 抜粋か要約の許可（[`consent_granted`]）、MCP の client なら `mcp:` の主体と [`MCP_PROPOSE_CONSENT_SCOPE`]。
/// 受け取り（`inbound` が `Some((caller, caller_scopes))`）では、さらに `granted_by` が認証した主体と一致し、MCP の client は
/// [`MCP_PROPOSE_SCOPE`] を持つこと（client や文書が書いた consent を信じない）。外部でない source は `true`。
pub fn consent_ok(
    source: SourceKind,
    consent: &Consent,
    inbound: Option<(&str, &[String])>,
) -> bool {
    let recorded = match source {
        SourceKind::HumanPaste | SourceKind::MarkdownImport => consent_granted(consent),
        SourceKind::McpClient => {
            is_mcp_principal(&consent.granted_by) && consent.scope == MCP_PROPOSE_CONSENT_SCOPE
        }
        SourceKind::CelerisCosThread | SourceKind::CelerisTaskRun => return true,
    };
    recorded
        && inbound.is_none_or(|(caller, scopes)| {
            consent.granted_by == caller
                && (source != SourceKind::McpClient
                    || scopes.iter().any(|s| s == MCP_PROPOSE_SCOPE))
        })
}

/// ADR D2.4: 秘密の形（R9。[`contains_secret`] と同じ正規表現）を見つけた所から行末までを [`MASK`] に置き換える。
/// `password:`・`cookie:` のように値が後ろに続く形も値ごと消す。正規表現が組めなければ fail-closed で本文の全体を置き換える。
pub fn mask_secrets(text: &str) -> Masked {
    let Some(re) = SECRET.as_ref() else {
        return Masked {
            text: MASK.to_string(),
            masked: usize::from(!text.is_empty()),
        };
    };
    let mut out = String::with_capacity(text.len());
    let mut masked = 0;
    let mut rest = text;
    while let Some(m) = re.find(rest) {
        out.push_str(&rest[..m.start()]);
        out.push_str(MASK);
        masked += 1;
        let tail = &rest[m.start()..];
        rest = tail.find('\n').map_or("", |i| &tail[i..]);
    }
    out.push_str(rest);
    Masked { text: out, masked }
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
    let actor_id: Option<String> = field(env, &["actor", "id"]);
    let state: Option<DecisionState> = field(env, &["decision_state"]);
    let confidence: Option<Confidence> = field(env, &["confidence"]);
    let body: String = field(env, &["body"]).unwrap_or_default();
    let content_hash: String = field(env, &["content_hash"]).unwrap_or_default();
    let external = source.is_some_and(SourceKind::is_external);
    let outbound = field::<String>(env, &["direction"]).as_deref() == Some("outbound");
    // 受け取る外部の文だけが、認証した主体と照らす対象（外へ出す item の actor・consent は元の記録の主体）。
    let inbound_external = external && !outbound;

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
        !matches!((source, actor), (Some(s), Some(a)) if actor_allowed(s, a))
            || (inbound_external && actor_id.as_deref() != Some(ctx.caller.as_str())),
        "R3:actor_mismatch",
    );
    let own_hash = format!("sha256:{}", sha256_hex(&body));
    fail(content_hash != own_hash, "R4:content_hash");
    fail(body.len() > BODY_MAX_BYTES, "R5:body_too_long");
    fail(
        field::<String>(env, &["project_id"]).as_deref() != Some(ctx.project_id.as_str()),
        "R6:project_mismatch",
    );
    let key: Option<String> = field(env, &["idempotency_key"]);
    let seen = key.and_then(|k| ctx.idempotency.get(&k));
    fail(
        seen.is_some_and(|h| *h != content_hash),
        "R7:idempotency_conflict",
    );
    let consent: Option<Consent> = field(env, &["consent"]);
    let inbound = (!outbound).then_some((ctx.caller.as_str(), ctx.caller_scopes.as_slice()));
    fail(
        external && !matches!((source, &consent), (Some(s), Some(c)) if consent_ok(s, c, inbound)),
        "R8:consent",
    );
    fail(contains_secret(&body), "R9:secret");
    // 承認は決定の記録からだけ導く。envelope が `approval` を書いていれば、状態によらず自己申告として拒否する。
    fail(
        env.get("approval").is_some()
            || (state == Some(DecisionState::Approved)
                && !human_approved(ctx.approval.as_ref(), &content_hash)),
        "R10:approval_needs_human",
    );
    fail(
        matches!((source, state), (Some(s), Some(st)) if !state_allowed(s, &body, st)),
        "R11:external_cannot_decide",
    );
    let refs: Vec<Value> = field(env, &["references"]).unwrap_or_default();
    fail(
        confidence == Some(Confidence::Confirmed) && refs.is_empty(),
        "R12:unsupported_claim",
    );
    let confidential = field::<String>(env, &["classification"]).as_deref() == Some("confidential");
    fail(
        outbound
            && confidential
            && !outbound_allowed(
                &content_hash,
                ctx.outbound_share.as_ref(),
                ctx.approval.as_ref(),
            ),
        "R13:outbound_confidential",
    );
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests;
