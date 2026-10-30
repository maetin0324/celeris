//! Direct Claude Code trial evidence, without invoking a harness or reading a DB.
use crate::operator_usage::{self, UsageRow};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const SCHEMA: &str = "ops-r1-a-raw/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    Exited,
    Cutoff,
    TokenCap,
    OperatorStop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClaudeCode {
    pub version: String,
    pub sha256: String,
}

/// Facts recorded by the operator. The transcript does not establish why a process stopped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub trial_id: String,
    pub condition_id: String,
    pub started_at: String,
    pub ended_at: String,
    pub stop: Stop,
    pub exit_code: Option<i32>,
    pub claude_code: ClaudeCode,
    pub cost_usd: Option<f64>,
    pub workspace_sha256_initial: String,
    pub session_id: String,
}

/// Each missing counter remains null, including in a transcript without assistant messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TokenUsage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_creation: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Raw {
    pub schema: String,
    #[serde(flatten)]
    pub metadata: Metadata,
    pub models: Vec<String>,
    pub token_usage: TokenUsage,
}

fn validate(m: &Metadata) -> Result<(), String> {
    for s in [
        &m.trial_id,
        &m.condition_id,
        &m.session_id,
        &m.claude_code.version,
    ] {
        if s.trim().is_empty() {
            return Err("empty trial/condition/session/version".into());
        }
    }
    for s in [&m.claude_code.sha256, &m.workspace_sha256_initial] {
        if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid sha256".into());
        }
    }
    let start = OffsetDateTime::parse(&m.started_at, &Rfc3339).map_err(|e| e.to_string())?;
    let end = OffsetDateTime::parse(&m.ended_at, &Rfc3339).map_err(|e| e.to_string())?;
    if end < start {
        return Err("ended_at precedes started_at".into());
    }
    if m.stop == Stop::Exited && m.exit_code.is_none() {
        return Err("exited requires exit_code".into());
    }
    if m.cost_usd.is_some_and(|v| !v.is_finite() || v < 0.0) {
        return Err("invalid cost_usd".into());
    }
    Ok(())
}

/// Reuse the operator transcript reader: repeated snapshots of one message count once.
pub fn build(transcript: &str, metadata: Metadata) -> Result<Raw, String> {
    validate(&metadata)?;
    let rows = operator_usage::convert(transcript, &[])?;
    let rows: Vec<_> = rows
        .iter()
        .filter(|r| r.session_id == metadata.session_id)
        .collect();
    let total = |field: fn(&UsageRow) -> Option<u64>| -> Result<Option<u64>, String> {
        if rows.is_empty() || rows.iter().any(|r| field(r).is_none()) {
            return Ok(None);
        }
        rows.iter().try_fold(Some(0u64), |acc, r| {
            acc.zip(field(r))
                .and_then(|(a, b)| a.checked_add(b))
                .map(Some)
                .ok_or_else(|| "token sum overflow".into())
        })
    };
    let models: BTreeSet<_> = rows.iter().filter_map(|r| r.model.clone()).collect();
    Ok(Raw {
        schema: SCHEMA.into(),
        token_usage: TokenUsage {
            input: total(|r| r.input_tokens)?,
            output: total(|r| r.output_tokens)?,
            cache_read: total(|r| r.cache_read_tokens)?,
            cache_creation: total(|r| r.cache_creation_tokens)?,
        },
        models: models.into_iter().collect(),
        metadata,
    })
}

impl Raw {
    pub fn render(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self)
            .map(|s| s + "\n")
            .map_err(|e| e.to_string())
    }
}

/// `ops-r1-a-raw --transcript <jsonl> --metadata <json> --out <json>`.
pub fn cli(args: &[String]) -> Result<String, String> {
    let mut opts = BTreeMap::new();
    let mut rest = args.iter().skip(1);
    while let Some(key) = rest.next() {
        if !["--transcript", "--metadata", "--out"].contains(&key.as_str()) {
            return Err(format!("unknown option {key}"));
        }
        let value = rest
            .next()
            .filter(|v| !v.starts_with("--"))
            .ok_or("missing option value")?;
        if opts.insert(key.as_str(), value).is_some() {
            return Err(format!("duplicate {key}"));
        }
    }
    let get = |k| {
        opts.get(k)
            .copied()
            .ok_or_else(|| format!("{k} is required"))
    };
    let transcript = Path::new(get("--transcript")?);
    let metadata = Path::new(get("--metadata")?);
    let out = Path::new(get("--out")?);
    if let Ok(dest) = out.canonicalize() {
        for input in [transcript, metadata] {
            if input.canonicalize().map_err(|e| e.to_string())? == dest {
                return Err("output must differ from inputs".into());
            }
        }
    }
    let read = |p| std::fs::read_to_string(p).map_err(|e| e.to_string());
    let raw = build(
        &read(transcript)?,
        serde_json::from_str(&read(metadata)?).map_err(|e| e.to_string())?,
    )?;
    std::fs::write(out, raw.render()?).map_err(|e| e.to_string())?;
    Ok(SCHEMA.into())
}
