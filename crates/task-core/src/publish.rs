//! ADR 2026-11-23-project-observability-publish D3: publish manifest `celeris.publish-manifest/1` の検査と、
//! 人だけの署名の状態（純関数。store を読まない）。
//!
//! - manifest は task の成果物 `publish/<manifest_id>/v<n>.json`（D3.1）。検査は [`validate_manifest`]（M1〜M10）。
//! - 署名は決定 `publish-signoff-v<n>`（ADR-0079 D7）への人の回答だけ（D3.3）。主体の規則は [`is_signoff_actor`]、
//!   状態は [`signoff_state`] が読むたびに導く（保存しない。D3.4）。
//! - 参照の形は子 2 の [`Reference`] をそのまま使い、案件の照合は呼び出し側が strategy と同じ関数で引いた
//!   [`RefStatus`] で行う。秘密の判定も [`crate::strategy::contains_secret`] を使う。

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::decision::DecisionStatus;
use crate::strategy::{
    Acl, Actor, ArtifactPointer, Classification, Confidence, RefStatus, Reference, Source,
    SourceRevision, Stance, contains_secret,
};

pub const SCHEMA: &str = "celeris.publish-manifest/1";
pub const SIGN_OPTION: &str = "sign";
pub const REJECT_OPTION: &str = "reject";
/// 署名の決定の key の接頭辞（[`signoff_decision_key`] の唯一の形）。
pub const SIGNOFF_KEY_PREFIX: &str = "publish-signoff-v";
/// token の無い構成の主体（`Principal::Unauthenticated`）。人の token・Web の login を経ていないので署名に数えない。
pub const LOCAL_HUMAN: &str = "human:local";

pub const DOCUMENT_MAX_BYTES: usize = 262_144;
pub const ID_MAX_CHARS: usize = 40;
pub const VERSION_MAX: u32 = 64;
pub const TEXT_MAX_CHARS: usize = crate::strategy::TEXT_MAX_CHARS;
pub const VENUE_MAX_CHARS: usize = 200;
pub const CLAIMS_MAX: usize = 32;
pub const EVIDENCE_MAX: usize = 256;
pub const REFS_PER_ITEM_MAX: usize = crate::strategy::REFS_PER_ITEM_MAX;
pub const REPRODUCTION_MAX: usize = 16;
pub const COMMANDS_MAX: usize = 16;
pub const COMMAND_MAX_CHARS: usize = 2_000;
pub const LIMITATIONS_MAX: usize = 32;
pub const OPEN_QUESTIONS_MAX: usize = 32;

/// D3.2: 1 つの manifest の 1 版。署名の状態の欄は持たない（M10。署名の正本は決定）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: String,
    pub project_id: String,
    pub manifest_id: String,
    pub version: u32,
    pub supersedes: Option<ArtifactPointer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_reason: Option<String>,
    pub idempotency_key: String,
    pub source: Source,
    pub actor: Actor,
    pub source_revision: SourceRevision,
    pub timestamp: String,
    pub target: Target,
    pub acl: Acl,
    pub claims: Vec<Claim>,
    pub evidence: Vec<ManifestEvidence>,
    #[serde(default)]
    pub reproduction: Vec<Reproduction>,
    #[serde(default)]
    pub limitations: Vec<Limitation>,
    #[serde(default)]
    pub open_questions: Vec<OpenQuestion>,
    pub decision_state: ManifestState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub kind: TargetKind,
    pub venue: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Paper,
    ReleaseNote,
    Dataset,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub id: String,
    pub statement: String,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEvidence {
    pub id: String,
    pub stance: Stance,
    pub statement: String,
    #[serde(default)]
    pub references: Vec<Reference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reproduction {
    pub id: String,
    pub claims: Vec<String>,
    pub commands: Vec<String>,
    pub environment: Environment,
    #[serde(default)]
    pub data: Vec<String>,
    pub expected: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub repo: String,
    pub commit: String,
    pub host_class: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limitation {
    pub id: String,
    pub statement: String,
    #[serde(default)]
    pub claims: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenQuestion {
    pub id: String,
    pub statement: String,
    pub blocking: bool,
}

/// 作った側の状態。署名の状態（`signed` 等）は書けない（M10）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestState {
    Draft,
    Proposed,
    Withdrawn,
}

/// 検査の規則（M1〜M10 と D3.1 の 2 つの衝突）。`code()` は API の problem の code。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ManifestRule {
    /// M1・M10: 形・上限・署名の状態の自己申告。
    Shape,
    /// M2・M3・M5: 根拠の無い claim（証拠が無い・内部の参照が無い・再現条件が無い）。
    UnsupportedClaim,
    /// 参照が引けない（同じ案件の commit・成果物・文献 URL・chat・task・knowledge に解決しない）。
    UnresolvedReference,
    /// M4: 支持と反証の両方を持つ claim を確認済みにした。
    Contradicted,
    /// M6: 未確認・反証のある claim が limitations にも未解決にも載っていない。
    HiddenUncertainty,
    /// M7.
    RefuteRemoved,
    /// M8.
    Secret,
    /// M9.
    OtherProject,
    /// D3.1: 同じ name に違う sha256。
    Idempotency,
    /// D3.1: 1 つの task に 2 つ目の `manifest_id`。
    IdConflict,
}

impl ManifestRule {
    pub fn code(self) -> &'static str {
        match self {
            Self::RefuteRemoved => "publish_manifest_refute_removed",
            Self::Secret => "publish_manifest_secret",
            Self::OtherProject => "publish_manifest_other_project",
            Self::Idempotency => "publish_manifest_idempotency_conflict",
            Self::IdConflict => "publish_manifest_id_conflict",
            Self::Shape
            | Self::UnsupportedClaim
            | Self::UnresolvedReference
            | Self::Contradicted
            | Self::HiddenUncertainty => "publish_manifest_invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestViolation {
    pub rule: ManifestRule,
    pub code: &'static str,
    pub path: String,
    pub detail: String,
}

/// 照合の文脈（呼び出し側が store と workspace から集める）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManifestContext {
    /// manifest を作った task の案件。
    pub project_id: String,
    /// 記録（`ArtifactProduced`）の name。
    pub artifact_name: String,
    /// 同じ name に記録された sha256 の全件。
    pub key_sha256s: Vec<String>,
    /// 同じ task が作った `publish/<id>/…` の id の全件（この manifest の id を含めてよい）。
    pub task_manifest_ids: Vec<String>,
    /// 前の版（`version - 1`）の byte 列。引けなければ `None`。
    pub previous: Option<Vec<u8>>,
    /// 文書の参照（`url` を除く）を引いた結果。載っていない参照は引けなかったものとして扱う。
    pub references: Vec<(Reference, RefStatus)>,
}

/// 成果物の name（D3.1）。
pub fn artifact_name(manifest_id: &str, version: u32) -> String {
    format!("publish/{manifest_id}/v{version}.json")
}

/// D3.2: 文書の idempotency の key。
pub fn idempotency_key(manifest_id: &str, version: u32) -> String {
    format!("publish:{manifest_id}:v{version}")
}

/// D3.3: 署名の決定の key（決定の key の上限 32 文字に収まる）。
pub fn signoff_decision_key(version: u32) -> String {
    format!("{SIGNOFF_KEY_PREFIX}{version}")
}

/// 決定の key が署名の決定か（主体の拒否の対象）。
pub fn is_signoff_key(key: &str) -> bool {
    key.starts_with(SIGNOFF_KEY_PREFIX)
}

/// D3.3 の 4: 署名できる主体。`human:` + 名前で、`human:local`（token の無い構成）でないもの。
/// `operator:`・`mcp:`・`cos:`・接頭辞の無い古い `human` と worker の run は数えない。
pub fn is_signoff_actor(by: &str) -> bool {
    crate::is_human_principal(by) && by != LOCAL_HUMAN
}

/// D3.3: 署名の決定の問い。name と sha256 を 1 回ずつ書く（[`signoff_subject`] が読む形の唯一の定義）。
pub fn signoff_question(manifest_id: &str, version: u32, sha256: &str) -> String {
    format!(
        "{}（sha256:{sha256}）に署名しますか。{SIGN_OPTION} で署名、{REJECT_OPTION} で署名しません。署名は記録だけで、Celeris は外へ何も送りません。",
        artifact_name(manifest_id, version)
    )
}

/// 署名の決定の問いが指す manifest。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignoffSubject {
    pub manifest_id: String,
    pub version: u32,
    pub sha256: String,
}

static SUBJECT_NAME: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"publish/([a-z0-9-]{1,40})/v([0-9]{1,2})\.json").ok());
static SUBJECT_SHA: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"sha256:([0-9a-f]{64})").ok());

/// 問いから manifest の name と sha256 を読む。name と sha256 がちょうど 1 回ずつ現れるときだけ `Some`。
pub fn signoff_subject(question: &str) -> Option<SignoffSubject> {
    let (name_re, sha_re) = (SUBJECT_NAME.as_ref()?, SUBJECT_SHA.as_ref()?);
    let names: Vec<_> = name_re.captures_iter(question).collect();
    let shas: Vec<_> = sha_re.captures_iter(question).collect();
    let ([name], [sha]) = (names.as_slice(), shas.as_slice()) else {
        return None;
    };
    Some(SignoffSubject {
        manifest_id: name[1].to_string(),
        version: name[2].parse().ok()?,
        sha256: sha[1].to_string(),
    })
}

/// 成果物の name が manifest のものなら `(manifest_id, version)`。
pub fn parse_artifact_name(name: &str) -> Option<(String, u32)> {
    let rest = name.strip_prefix("publish/")?;
    let (id, file) = rest.split_once('/')?;
    let version = file
        .strip_prefix('v')?
        .strip_suffix(".json")?
        .parse()
        .ok()?;
    is_id(id).then(|| (id.to_string(), version))
}

/// 署名の状態（D3.4。保存しない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignoffState {
    Unsigned,
    Signed,
    Rejected,
    Stale,
    Superseded,
}

/// 署名の決定（manifest を作った task の `decisions` の行と、最後の `DecisionAnswered`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignoffRecord {
    pub key: String,
    pub question: String,
    pub status: DecisionStatus,
    /// 最後の回答の `(option, by)`。
    pub last_answer: Option<(String, String)>,
}

/// D3.3 の 1〜5 と D3.4 の失効。`recorded_sha256` は manifest の `ArtifactProduced` の sha256、
/// `current_sha256` は file の今の byte 列の sha256（読めなければ `None`）、`newer_signoff` は同じ
/// `manifest_id` のより新しい版の署名の決定があるか。
pub fn signoff_state(
    manifest_id: &str,
    version: u32,
    recorded_sha256: &str,
    current_sha256: Option<&str>,
    record: Option<&SignoffRecord>,
    newer_signoff: bool,
) -> SignoffState {
    if newer_signoff {
        return SignoffState::Superseded;
    }
    let Some(record) = record else {
        return SignoffState::Unsigned;
    };
    let subject = signoff_subject(&record.question);
    let matches = record.key == signoff_decision_key(version)
        && subject.as_ref().is_some_and(|s| {
            s.manifest_id == manifest_id
                && s.version == version
                && s.sha256 == bare_sha(recorded_sha256)
        });
    if !matches || record.status != DecisionStatus::Answered {
        return SignoffState::Unsigned;
    }
    let Some((option, by)) = &record.last_answer else {
        return SignoffState::Unsigned;
    };
    if option == REJECT_OPTION {
        return SignoffState::Rejected;
    }
    if option != SIGN_OPTION || !is_signoff_actor(by) {
        return SignoffState::Unsigned;
    }
    if current_sha256.map(bare_sha) != Some(bare_sha(recorded_sha256)) {
        return SignoffState::Stale;
    }
    SignoffState::Signed
}

fn bare_sha(s: &str) -> &str {
    s.strip_prefix("sha256:").unwrap_or(s)
}

fn is_id(s: &str) -> bool {
    (1..=ID_MAX_CHARS).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn push(
    out: &mut Vec<ManifestViolation>,
    rule: ManifestRule,
    path: impl Into<String>,
    detail: impl Into<String>,
) {
    out.push(ManifestViolation {
        rule,
        code: rule.code(),
        path: path.into(),
        detail: detail.into(),
    });
}

/// 署名の状態を書く欄・語（M10）。
const SIGNOFF_FIELDS: [&str; 3] = ["signoff", "signed", "approved"];

/// manifest の file の byte 列を検査する。最初の違反で止まらず、違反の全件を返す（違反が無ければ空）。
pub fn validate_manifest(bytes: &[u8], ctx: &ManifestContext) -> Vec<ManifestViolation> {
    let mut out = Vec::new();
    if bytes.len() > DOCUMENT_MAX_BYTES {
        push(
            &mut out,
            ManifestRule::Shape,
            "",
            format!("文書が {DOCUMENT_MAX_BYTES} byte を超える"),
        );
        return out;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        push(&mut out, ManifestRule::Shape, "", "UTF-8 でない");
        return out;
    };
    // M8: 形の誤りでも秘密は秘密として返す（文書の全文を見る）。
    if contains_secret(text) {
        push(
            &mut out,
            ManifestRule::Secret,
            "",
            "秘密に見える文字列（token・cookie・password・webhook）を含む",
        );
    }
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            push(
                &mut out,
                ManifestRule::Shape,
                "",
                format!("JSON でない: {e}"),
            );
            return out;
        }
    };
    // M10: 署名の状態の自己申告（欄と decision_state の語）。
    for field in SIGNOFF_FIELDS {
        if value.get(field).is_some() {
            push(
                &mut out,
                ManifestRule::Shape,
                field,
                "署名の状態は文書に書けない（署名の正本は決定 publish-signoff-v<n>）",
            );
        }
    }
    if let Some(state) = value.get("decision_state").and_then(Value::as_str)
        && !["draft", "proposed", "withdrawn"].contains(&state)
    {
        push(
            &mut out,
            ManifestRule::Shape,
            "decision_state",
            format!("decision_state {state:?} は書けない（draft・proposed・withdrawn だけ）"),
        );
    }
    let doc: Manifest = match serde_json::from_value(value) {
        Ok(d) => d,
        Err(e) => {
            if !out.iter().any(|v| v.rule == ManifestRule::Shape) {
                push(&mut out, ManifestRule::Shape, "", format!("形の誤り: {e}"));
            }
            return out;
        }
    };
    check_shape(&doc, ctx, &mut out);
    check_claims(&doc, &mut out);
    check_references(&doc, ctx, &mut out);
    check_previous(&doc, ctx, &mut out);
    out
}

fn text_ok(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.chars().count() <= max
}

/// M1 と D3.1 の 2 つの衝突。
fn check_shape(doc: &Manifest, ctx: &ManifestContext, out: &mut Vec<ManifestViolation>) {
    use ManifestRule::{IdConflict, Idempotency, Shape};
    if doc.schema != SCHEMA {
        push(out, Shape, "schema", format!("schema は {SCHEMA} だけ"));
    }
    if !is_id(&doc.manifest_id) {
        push(out, Shape, "manifest_id", "manifest_id は [a-z0-9-]{1,40}");
    }
    if !(1..=VERSION_MAX).contains(&doc.version) {
        push(
            out,
            Shape,
            "version",
            format!("version は 1〜{VERSION_MAX}"),
        );
    }
    if doc.idempotency_key != idempotency_key(&doc.manifest_id, doc.version) {
        push(
            out,
            Shape,
            "idempotency_key",
            "idempotency_key は publish:<manifest_id>:v<version>",
        );
    }
    if ctx.artifact_name != artifact_name(&doc.manifest_id, doc.version) {
        push(
            out,
            Shape,
            "manifest_id",
            "成果物の name が publish/<manifest_id>/v<version>.json と一致しない",
        );
    }
    if (doc.version > 1) != doc.supersedes.is_some() {
        push(
            out,
            Shape,
            "supersedes",
            "version 1 は supersedes を持たず、2 以上は前の版を指す",
        );
    }
    if doc.target.venue.chars().count() > VENUE_MAX_CHARS {
        push(
            out,
            Shape,
            "target.venue",
            format!("{VENUE_MAX_CHARS} 文字まで"),
        );
    }
    let distinct: BTreeSet<_> = ctx.key_sha256s.iter().map(|s| bare_sha(s)).collect();
    if distinct.len() > 1 {
        push(
            out,
            Idempotency,
            "idempotency_key",
            "同じ name の成果物に違う sha256 が記録された",
        );
    }
    if ctx
        .task_manifest_ids
        .iter()
        .any(|id| id != &doc.manifest_id)
    {
        push(
            out,
            IdConflict,
            "manifest_id",
            "1 つの task が持てる manifest_id は 1 つだけ",
        );
    }
    let lists = [
        ("claims", doc.claims.len(), 1, CLAIMS_MAX),
        ("evidence", doc.evidence.len(), 1, EVIDENCE_MAX),
        ("reproduction", doc.reproduction.len(), 0, REPRODUCTION_MAX),
        ("limitations", doc.limitations.len(), 0, LIMITATIONS_MAX),
        (
            "open_questions",
            doc.open_questions.len(),
            0,
            OPEN_QUESTIONS_MAX,
        ),
    ];
    for (path, len, min, max) in lists {
        if !(min..=max).contains(&len) {
            push(out, Shape, path, format!("{min}〜{max} 件"));
        }
    }
    let mut ids = BTreeSet::new();
    let texts = doc
        .claims
        .iter()
        .map(|c| (&c.id, &c.statement))
        .chain(doc.evidence.iter().map(|e| (&e.id, &e.statement)))
        .chain(doc.limitations.iter().map(|l| (&l.id, &l.statement)))
        .chain(doc.open_questions.iter().map(|q| (&q.id, &q.statement)))
        .chain(doc.reproduction.iter().map(|r| (&r.id, &r.expected)));
    for (id, statement) in texts {
        if !is_id(id) || !ids.insert(id.as_str()) {
            push(
                out,
                Shape,
                id.clone(),
                "id は [a-z0-9-]{1,40} で文書の中で一意",
            );
        }
        if !text_ok(statement, TEXT_MAX_CHARS) {
            push(
                out,
                Shape,
                id.clone(),
                format!("文は 1〜{TEXT_MAX_CHARS} 文字"),
            );
        }
    }
    for e in &doc.evidence {
        if e.references.len() > REFS_PER_ITEM_MAX {
            push(
                out,
                Shape,
                format!("evidence.{}", e.id),
                format!("参照は {REFS_PER_ITEM_MAX} 件まで"),
            );
        }
    }
    for r in &doc.reproduction {
        if r.commands.is_empty()
            || r.commands.len() > COMMANDS_MAX
            || r.commands.iter().any(|c| !text_ok(c, COMMAND_MAX_CHARS))
        {
            push(
                out,
                Shape,
                format!("reproduction.{}.commands", r.id),
                format!("1〜{COMMANDS_MAX} 件、各 {COMMAND_MAX_CHARS} 文字まで"),
            );
        }
        if !is_hex(&r.environment.commit, 40) {
            push(
                out,
                Shape,
                format!("reproduction.{}.environment.commit", r.id),
                "commit は 40 桁の小文字 hex",
            );
        }
    }
}

/// M2〜M6。
fn check_claims(doc: &Manifest, out: &mut Vec<ManifestViolation>) {
    use ManifestRule::{Contradicted, HiddenUncertainty, UnsupportedClaim};
    let listed: BTreeSet<&str> = doc
        .limitations
        .iter()
        .flat_map(|l| l.claims.iter().map(String::as_str))
        .chain(doc.open_questions.iter().map(|q| q.id.as_str()))
        .collect();
    let reproduced: BTreeSet<&str> = doc
        .reproduction
        .iter()
        .flat_map(|r| r.claims.iter().map(String::as_str))
        .collect();
    for claim in &doc.claims {
        let path = format!("claims.{}", claim.id);
        let evidence: Vec<&ManifestEvidence> = claim
            .evidence
            .iter()
            .filter_map(|id| doc.evidence.iter().find(|e| &e.id == id))
            .collect();
        // M2: 証拠が 1 件以上で、すべて同じ文書にある。
        if claim.evidence.is_empty() || evidence.len() != claim.evidence.len() {
            push(
                out,
                UnsupportedClaim,
                &path,
                "claim は同じ文書の証拠を 1 件以上引く",
            );
        }
        let supported = evidence.iter().any(|e| e.stance == Stance::Support);
        let refuted = evidence.iter().any(|e| e.stance == Stance::Refute);
        let confirmed = claim.confidence == Confidence::Confirmed;
        if confirmed {
            // M3: 内部の参照（commit・artifact・task）を持つ支持の証拠。
            let internal = evidence.iter().any(|e| {
                e.stance == Stance::Support
                    && e.references.iter().any(|r| {
                        matches!(
                            r,
                            Reference::Commit { .. }
                                | Reference::Artifact { .. }
                                | Reference::Task { .. }
                        )
                    })
            });
            if !internal {
                push(
                    out,
                    UnsupportedClaim,
                    &path,
                    "confirmed の claim は内部の参照（commit・artifact・task）を持つ支持の証拠が要る",
                );
            }
            // M4.
            if supported && refuted {
                push(
                    out,
                    Contradicted,
                    &path,
                    "支持と反証の両方を持つ claim は confirmed にできない",
                );
            }
            // M5.
            if !reproduced.contains(claim.id.as_str()) {
                push(
                    out,
                    UnsupportedClaim,
                    &path,
                    "confirmed の claim は reproduction のどれかに載る",
                );
            }
        }
        // M6: 未確認・反証のある claim を隠さない。
        if (!confirmed || refuted) && !listed.contains(claim.id.as_str()) {
            push(
                out,
                HiddenUncertainty,
                &path,
                "未確認か反証のある claim は limitations か open_questions に載る",
            );
        }
    }
    let known: BTreeSet<&str> = doc.claims.iter().map(|c| c.id.as_str()).collect();
    let dangling = doc
        .reproduction
        .iter()
        .flat_map(|r| &r.claims)
        .chain(doc.limitations.iter().flat_map(|l| &l.claims));
    for id in dangling {
        if !known.contains(id.as_str()) {
            push(
                out,
                ManifestRule::Shape,
                id.clone(),
                "文書に無い claim を引いた",
            );
        }
    }
}

/// 参照の解決と M9（別の案件）。
fn check_references(doc: &Manifest, ctx: &ManifestContext, out: &mut Vec<ManifestViolation>) {
    if doc.project_id != ctx.project_id {
        push(
            out,
            ManifestRule::OtherProject,
            "project_id",
            "文書の案件が manifest を作った task の案件と違う",
        );
    }
    for e in &doc.evidence {
        let path = format!("evidence.{}", e.id);
        for r in &e.references {
            if let Reference::Url { url } = r {
                if !url.starts_with("https://") || url.chars().count() > 2_048 {
                    push(
                        out,
                        ManifestRule::UnresolvedReference,
                        &path,
                        "文献の URL は https で 2,048 文字まで",
                    );
                }
                continue;
            }
            let status = ctx
                .references
                .iter()
                .find(|(seen, _)| seen == r)
                .map(|(_, s)| s);
            let project = match status {
                None | Some(RefStatus::Missing) => {
                    push(
                        out,
                        ManifestRule::UnresolvedReference,
                        &path,
                        "参照が引けない",
                    );
                    continue;
                }
                Some(RefStatus::Found { project_id }) => project_id.as_deref(),
                Some(RefStatus::Cited(m)) => m.project_id.as_deref(),
            };
            let other = match r {
                // knowledge の案件でない page（全体の知識）は他の案件の資料ではない。
                Reference::Knowledge { .. } => project.is_some_and(|p| p != ctx.project_id),
                _ => project != Some(ctx.project_id.as_str()),
            };
            if other {
                push(
                    out,
                    ManifestRule::OtherProject,
                    &path,
                    "参照が別の案件のもの",
                );
            }
        }
    }
}

/// M7: 前の版の反証の証拠は消せず、書き換えられない。
fn check_previous(doc: &Manifest, ctx: &ManifestContext, out: &mut Vec<ManifestViolation>) {
    let Some(previous) = ctx
        .previous
        .as_deref()
        .and_then(|b| serde_json::from_slice::<Manifest>(b).ok())
    else {
        return;
    };
    for old in previous
        .evidence
        .iter()
        .filter(|e| e.stance == Stance::Refute)
    {
        if !doc.evidence.iter().any(|e| e == old) {
            push(
                out,
                ManifestRule::RefuteRemoved,
                format!("evidence.{}", old.id),
                "前の版の反証の証拠を消すか書き換えた",
            );
        }
    }
}

/// 概要の強い警告（D3.2）: 機密の manifest と、公開を止める未解決の問い。
pub fn strong_warnings(doc: &Manifest) -> Vec<&'static str> {
    let mut out = Vec::new();
    if doc.acl.classification == Classification::Confidential {
        out.push("manifest_confidential");
    }
    if doc.open_questions.iter().any(|q| q.blocking) {
        out.push("manifest_blocking_question");
    }
    out
}

/// 根拠の無い claim の id（M2・M3・M5 に当たったもの）。
pub fn unsupported_claims(violations: &[ManifestViolation]) -> Vec<String> {
    let mut ids: Vec<String> = violations
        .iter()
        .filter(|v| v.rule == ManifestRule::UnsupportedClaim)
        .filter_map(|v| v.path.strip_prefix("claims.").map(str::to_string))
        .collect();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests;
