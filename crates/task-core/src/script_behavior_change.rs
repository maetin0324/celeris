//! ADR 2026-11-03-ts-intended-behavior-change: 人が承認した方針改善（behavior change）の仕様と、その説明の判定。
//!
//! 説明は正規化ではない。仕様の編集を **Rust の出力** に当てて期待出力を作り、TS の出力と serde_json の直列化の
//! byte 列で比べる（D1）。I/O を持たない。承認の記録は `store::script_behavior_changes`。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::script_compare::{ScriptDomain, sha256_digest};

/// 仕様の `schema`。
pub const BEHAVIOR_CHANGE_SCHEMA: &str = "celeris.script-behavior-change/1";
/// `when`・`edits` の個数の上限（狭い改善だけを許す）。
pub const BEHAVIOR_CHANGE_PARTS_MAX: usize = 4;
/// `lit` の上限（byte）。
pub const BEHAVIOR_CHANGE_LIT_MAX: usize = 1024;
/// `summary` の上限（byte）。
pub const BEHAVIOR_CHANGE_SUMMARY_MAX: usize = 512;
/// `truncate_chars.max` の上限。
pub const BEHAVIOR_CHANGE_TRUNCATE_MAX: u32 = 100_000;

/// 改善仕様（D1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BehaviorChangeSpec {
    pub schema: String,
    pub id: String,
    pub version: u32,
    pub domain: ScriptDomain,
    pub summary: String,
    /// 前の版の `spec_sha256`（最初の版は `null`）。
    pub supersedes: Option<String>,
    pub when: Vec<InputCondition>,
    pub edits: Vec<OutputEdit>,
    /// 期待差分 fixture の file の byte 列の sha256（64 桁 hex）。
    pub cases_sha256: String,
}

/// 入力条件 1 個。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputCondition {
    /// 入力 DTO の JSON pointer（`*` は書かない）。
    pub path: String,
    pub op: ConditionOp,
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOp {
    /// JSON 値の一致。
    Equals,
    /// 文字列の code point 数が `value` 以上。
    MinChars,
    /// 文字列の code point 数が `value` 以下。
    MaxChars,
}

/// 期待差分の述語 1 個: 出力の `path` の文字列の中の `find` を `replace` にする。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputEdit {
    /// 出力の JSON pointer（`*` は配列の全要素）。
    pub path: String,
    pub find: Vec<TextPart>,
    pub replace: Vec<TextPart>,
}

/// `find`・`replace` の部品。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum TextPart {
    Lit(String),
    /// 入力の文字列（JSON pointer）。
    Input(String),
    /// 入力の文字列を `max` code point までにし、超えたら `ellipsis` を足す。
    TruncateChars {
        input: String,
        max: u32,
        ellipsis: String,
    },
}

/// どの仕様で説明したか（記録の `explained_by`）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExplainedBy {
    pub id: String,
    pub version: u32,
    pub spec_sha256: String,
}

/// 承認済みの仕様 1 件（store が読む）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedBehaviorChange {
    pub spec: BehaviorChangeSpec,
    pub spec_sha256: String,
}

/// 仕様の不備（承認の 422 `behavior_change_invalid`・runner の読み込みの失敗）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("behavior change spec invalid: {0}")]
pub struct BehaviorChangeInvalid(pub String);

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_pointer(p: &str, wildcard: bool) -> bool {
    p.starts_with('/')
        && p.len() <= 256
        && p[1..]
            .split('/')
            .all(|seg| !seg.is_empty() && (wildcard || seg != "*"))
}

fn valid_part(part: &TextPart) -> Result<(), String> {
    match part {
        TextPart::Lit(s) if s.len() > BEHAVIOR_CHANGE_LIT_MAX => Err(format!(
            "lit is longer than {BEHAVIOR_CHANGE_LIT_MAX} bytes"
        )),
        TextPart::Lit(_) => Ok(()),
        TextPart::Input(p) if !valid_pointer(p, false) => Err(format!("input pointer {p:?}")),
        TextPart::Input(_) => Ok(()),
        TextPart::TruncateChars {
            input,
            max,
            ellipsis,
        } => {
            if !valid_pointer(input, false) {
                return Err(format!("truncate_chars input pointer {input:?}"));
            }
            if *max == 0 || *max > BEHAVIOR_CHANGE_TRUNCATE_MAX {
                return Err(format!(
                    "truncate_chars max must be 1..={BEHAVIOR_CHANGE_TRUNCATE_MAX}"
                ));
            }
            if ellipsis.len() > BEHAVIOR_CHANGE_LIT_MAX {
                return Err("truncate_chars ellipsis is too long".into());
            }
            Ok(())
        }
    }
}

impl BehaviorChangeSpec {
    /// 形の検証（D1 の「仕様の検証」）。領域の対応は [`domain_supported`] で別に見る。
    pub fn validate(&self) -> Result<(), BehaviorChangeInvalid> {
        let bad = |m: String| Err(BehaviorChangeInvalid(m));
        if self.schema != BEHAVIOR_CHANGE_SCHEMA {
            return bad(format!("schema must be {BEHAVIOR_CHANGE_SCHEMA}"));
        }
        if self.id.is_empty()
            || self.id.len() > 64
            || !self
                .id
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'))
        {
            return bad("id must match [a-z0-9-]{1,64}".into());
        }
        if self.version == 0 {
            return bad("version must be at least 1".into());
        }
        if self.summary.trim().is_empty() || self.summary.len() > BEHAVIOR_CHANGE_SUMMARY_MAX {
            return bad(format!(
                "summary must be 1..={BEHAVIOR_CHANGE_SUMMARY_MAX} bytes"
            ));
        }
        if let Some(s) = &self.supersedes
            && !is_hex64(s)
        {
            return bad("supersedes must be 64 lowercase hex".into());
        }
        if self.version == 1 && self.supersedes.is_some() {
            return bad("version 1 has no previous version: supersedes must be null".into());
        }
        if !is_hex64(&self.cases_sha256) {
            return bad("cases_sha256 must be 64 lowercase hex".into());
        }
        if self.when.is_empty() || self.when.len() > BEHAVIOR_CHANGE_PARTS_MAX {
            return bad(format!("when must have 1..={BEHAVIOR_CHANGE_PARTS_MAX}"));
        }
        if self.edits.is_empty() || self.edits.len() > BEHAVIOR_CHANGE_PARTS_MAX {
            return bad(format!("edits must have 1..={BEHAVIOR_CHANGE_PARTS_MAX}"));
        }
        for c in &self.when {
            if !valid_pointer(&c.path, false) {
                return bad(format!("when pointer {:?}", c.path));
            }
            if matches!(c.op, ConditionOp::MinChars | ConditionOp::MaxChars) && !c.value.is_u64() {
                return bad("min_chars/max_chars value must be a non-negative integer".into());
            }
        }
        for e in &self.edits {
            if !valid_pointer(&e.path, true) {
                return bad(format!("edit pointer {:?}", e.path));
            }
            if e.find.is_empty() || e.replace.is_empty() {
                return bad("find and replace must not be empty".into());
            }
            if e.find == e.replace {
                return bad("find and replace must differ".into());
            }
            for p in e.find.iter().chain(&e.replace) {
                valid_part(p).map_err(BehaviorChangeInvalid)?;
            }
        }
        Ok(())
    }

    /// `spec_sha256`（D1）: struct の宣言順の serde_json の直列化の sha256（64 桁 hex）。
    pub fn sha256(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        sha256_digest(&bytes)
            .trim_start_matches("sha256:")
            .to_owned()
    }

    /// `when` がすべて成り立つか。
    pub fn applies_to(&self, input: &Value) -> bool {
        self.when.iter().all(|c| {
            let Some(v) = input.pointer(&c.path) else {
                return false;
            };
            match c.op {
                ConditionOp::Equals => *v == c.value,
                ConditionOp::MinChars => match (v.as_str(), c.value.as_u64()) {
                    (Some(s), Some(n)) => s.chars().count() as u64 >= n,
                    _ => false,
                },
                ConditionOp::MaxChars => match (v.as_str(), c.value.as_u64()) {
                    (Some(s), Some(n)) => s.chars().count() as u64 <= n,
                    _ => false,
                },
            }
        })
    }

    /// `edits` を Rust の出力に当てた期待出力（D1 の 3）。作れなければ `None`。
    pub fn expected_output(&self, input: &Value, rust: &Value) -> Option<Value> {
        let mut out = rust.clone();
        let mut replaced = 0usize;
        for e in &self.edits {
            let find = render(&e.find, input)?;
            let replace = render(&e.replace, input)?;
            if find.is_empty() || find == replace {
                return None;
            }
            let segs: Vec<&str> = e.path[1..].split('/').collect();
            replaced += edit_at(&mut out, &segs, &find, &replace)?;
        }
        (replaced > 0).then_some(out)
    }

    /// この仕様が `rust` と `ts` の差分を説明するか（D1 の 2〜4）。
    pub fn explains(&self, input: &Value, rust: &Value, ts: &Value) -> bool {
        if !self.applies_to(input) {
            return false;
        }
        match self.expected_output(input, rust) {
            Some(expected) => same_bytes(&expected, ts),
            None => false,
        }
    }
}

fn same_bytes(a: &Value, b: &Value) -> bool {
    match (serde_json::to_vec(a), serde_json::to_vec(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn render(parts: &[TextPart], input: &Value) -> Option<String> {
    let mut s = String::new();
    for p in parts {
        match p {
            TextPart::Lit(l) => s.push_str(l),
            TextPart::Input(ptr) => s.push_str(input.pointer(ptr)?.as_str()?),
            TextPart::TruncateChars {
                input: ptr,
                max,
                ellipsis,
            } => {
                let v = input.pointer(ptr)?.as_str()?;
                let max = *max as usize;
                if v.chars().count() <= max {
                    s.push_str(v);
                } else {
                    s.extend(v.chars().take(max));
                    s.push_str(ellipsis);
                }
            }
        }
    }
    Some(s)
}

/// `segs` の先の文字列で `find` がちょうど 1 回なら置き換える。2 回以上は曖昧なので `None`。
fn edit_at(v: &mut Value, segs: &[&str], find: &str, replace: &str) -> Option<usize> {
    let Some((head, rest)) = segs.split_first() else {
        return match v {
            Value::String(s) => match s.matches(find).count() {
                0 => Some(0),
                1 => {
                    *s = s.replacen(find, replace, 1);
                    Some(1)
                }
                _ => None,
            },
            _ => Some(0),
        };
    };
    let head = head.replace("~1", "/").replace("~0", "~");
    match v {
        Value::Array(items) if head == "*" => {
            let mut n = 0;
            for item in items {
                n += edit_at(item, rest, find, replace)?;
            }
            Some(n)
        }
        Value::Array(items) => match head.parse::<usize>().ok().and_then(|i| items.get_mut(i)) {
            Some(item) => edit_at(item, rest, find, replace),
            None => Some(0),
        },
        Value::Object(map) => match map.get_mut(&head) {
            Some(item) => edit_at(item, rest, find, replace),
            None => Some(0),
        },
        _ => Some(0),
    }
}

/// 承認できる領域か（D4。比較の結線がある領域だけ）。
pub fn domain_supported(domain: ScriptDomain) -> bool {
    matches!(domain, ScriptDomain::CosDigest)
}

/// 承認済みの仕様のうち、最初に説明できたもの（`id` の順）。
pub fn explain(
    approved: &[ApprovedBehaviorChange],
    domain: ScriptDomain,
    input: &Value,
    rust: &Value,
    ts: &Value,
) -> Option<ExplainedBy> {
    approved
        .iter()
        .filter(|a| a.spec.domain == domain)
        .find(|a| a.spec.explains(input, rust, ts))
        .map(|a| ExplainedBy {
            id: a.spec.id.clone(),
            version: a.spec.version,
            spec_sha256: a.spec_sha256.clone(),
        })
}

/// 期待差分 fixture の 1 行（D3）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BehaviorChangeCase {
    pub name: String,
    pub input: Value,
    pub rust: Value,
    pub expected: Value,
    pub expect: BehaviorChangeExpect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorChangeExpect {
    /// 差分があり、仕様が説明する。
    Explained,
    /// 改善が当たらない（TS は Rust と同じであるべき）。
    Equal,
}

#[cfg(test)]
#[path = "script_behavior_change_tests.rs"]
mod tests;

/// 承認の記録の状態（D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorChangeState {
    Approved,
    Revoked,
    Superseded,
}

impl BehaviorChangeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Revoked => "revoked",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "approved" => Some(Self::Approved),
            "revoked" => Some(Self::Revoked),
            "superseded" => Some(Self::Superseded),
            _ => None,
        }
    }
}

/// `script_behavior_changes` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct BehaviorChangeRow {
    pub spec_sha256: String,
    pub id: String,
    pub version: u32,
    pub domain: ScriptDomain,
    pub state: BehaviorChangeState,
    pub spec: BehaviorChangeSpec,
    pub approved_by: String,
    pub approved_at: String,
    pub updated_by: String,
    pub updated_at: String,
}

/// `script_behavior_change_events` の 1 行（追記専用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct BehaviorChangeEvent {
    pub id: u64,
    pub ts: String,
    pub spec_sha256: String,
    pub change_id: String,
    pub version: u32,
    pub action: BehaviorChangeState,
    pub actor: String,
    pub reason: String,
}

/// 承認・取り消しの拒否（D4 の code に写す）。
#[derive(Debug, thiserror::Error)]
pub enum BehaviorChangeError {
    #[error("approving a behavior change needs a human actor (got `{0}`)")]
    RequiresHuman(String),
    #[error(transparent)]
    Invalid(#[from] BehaviorChangeInvalid),
    #[error("behavior changes are not supported for domain `{0}`")]
    DomainUnsupported(String),
    #[error("spec_sha256 mismatch: computed {computed}, given {given}")]
    HashMismatch { computed: String, given: String },
    #[error("behavior change `{id}` version {version} is bound to another spec")]
    VersionTaken { id: String, version: u32 },
    #[error("behavior change `{id}` version {version} is not newer than version {latest}")]
    VersionNotNewer {
        id: String,
        version: u32,
        latest: u32,
    },
    #[error("behavior change `{sha}` is {}; raise the version instead", .state.as_str())]
    Retired {
        sha: String,
        state: BehaviorChangeState,
    },
    #[error("behavior change `{0}` not found")]
    NotFound(String),
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
}

impl From<rusqlite::Error> for BehaviorChangeError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(crate::store::StoreError::from(e))
    }
}

impl From<serde_json::Error> for BehaviorChangeError {
    fn from(e: serde_json::Error) -> Self {
        Self::Store(crate::store::StoreError::from(e))
    }
}
