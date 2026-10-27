//! External operator accounting. See ADR 2026-10-27-operator-usage.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fs, io::Write, path::Path};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const SCHEMA: &str = "celeris.operator-usage/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Prep,
    Observe,
    Intervene,
    Shared,
    SharedUnattributable,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub session_id: String,
    pub start: Option<String>,
    pub end: Option<String>,
    pub category: Category,
    pub trial_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageRow {
    pub schema: String,
    pub dedup_key: String,
    pub session_id: String,
    pub message_id: String,
    pub model: Option<String>,
    pub is_subagent: bool,
    pub agent_id: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub category: Category,
    pub trial_key: Option<String>,
}

fn timestamp(s: &str) -> Result<OffsetDateTime, String> {
    OffsetDateTime::parse(s, &Rfc3339).map_err(|e| format!("invalid timestamp {s}: {e}"))
}

fn bound(s: &Option<String>) -> Result<Option<OffsetDateTime>, String> {
    s.as_deref().map(timestamp).transpose()
}

fn validate_map(map: &[Mapping]) -> Result<(), String> {
    for (i, m) in map.iter().enumerate() {
        if m.session_id.trim().is_empty()
            || m.category == Category::SharedUnattributable
            || (m.category == Category::Shared) != m.trial_key.is_none()
            || m.trial_key.as_ref().is_some_and(|s| s.trim().is_empty())
        {
            return Err(format!("mapping {i}: invalid session/category/trial_key"));
        }
        let (start, end) = (bound(&m.start)?, bound(&m.end)?);
        if start.zip(end).is_some_and(|(s, e)| s >= e) {
            return Err(format!("mapping {i}: start must precede end"));
        }
        for other in &map[..i] {
            if other.session_id == m.session_id
                && !end.zip(bound(&other.start)?).is_some_and(|(e, s)| e <= s)
                && !bound(&other.end)?.zip(start).is_some_and(|(e, s)| e <= s)
            {
                return Err(format!("mapping {i}: overlapping intervals"));
            }
        }
    }
    Ok(())
}

fn optional_text(v: &Value, key: &str) -> Result<Option<String>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
        _ => Err(format!("{key}: expected nonempty string or null")),
    }
}

fn required_text(v: &Value, key: &str) -> Result<String, String> {
    optional_text(v, key)?.ok_or_else(|| format!("missing {key}"))
}

fn token(v: &Value, key: &str) -> Result<Option<u64>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(n) => n.as_u64().map(Some).ok_or_else(|| format!("invalid {key}")),
    }
}

fn response(v: &Value) -> Result<UsageRow, String> {
    let session = required_text(v, "sessionId")?;
    let message = &v["message"];
    let id = required_text(message, "id")?;
    let at = optional_text(v, "timestamp")?;
    if let Some(t) = &at {
        timestamp(t)?;
    }
    let agent_id = optional_text(v, "agentId")?;
    let sidechain = match v.get("isSidechain") {
        None => false,
        Some(Value::Bool(b)) => *b,
        _ => return Err("isSidechain must be boolean".into()),
    };
    let usage = &message["usage"];
    if !usage.is_null() && !usage.is_object() {
        return Err("usage must be an object or null".into());
    }
    Ok(UsageRow {
        schema: SCHEMA.into(),
        dedup_key: format!("op:{session}:{id}"),
        session_id: session,
        message_id: id,
        model: optional_text(message, "model")?,
        is_subagent: sidechain || agent_id.is_some(),
        agent_id,
        start: at.clone(),
        end: at,
        input_tokens: token(usage, "input_tokens")?,
        output_tokens: token(usage, "output_tokens")?,
        cache_read_tokens: token(usage, "cache_read_input_tokens")?,
        cache_creation_tokens: token(usage, "cache_creation_input_tokens")?,
        category: Category::SharedUnattributable,
        trial_key: None,
    })
}

fn merge(row: &mut UsageRow, next: UsageRow) -> Result<(), String> {
    if row.model != next.model
        || row.is_subagent != next.is_subagent
        || row.agent_id != next.agent_id
    {
        return Err(format!("{}: inconsistent message identity", row.dedup_key));
    }
    row.input_tokens = row.input_tokens.max(next.input_tokens);
    row.output_tokens = row.output_tokens.max(next.output_tokens);
    row.cache_read_tokens = row.cache_read_tokens.max(next.cache_read_tokens);
    row.cache_creation_tokens = row.cache_creation_tokens.max(next.cache_creation_tokens);
    match (&row.start, &row.end, &next.start, &next.end) {
        (Some(s), Some(e), Some(ns), Some(ne)) => {
            if timestamp(ns)? < timestamp(s)? {
                row.start = next.start;
            }
            if timestamp(ne)? > timestamp(e)? {
                row.end = next.end;
            }
        }
        _ => {
            row.start = None;
            row.end = None;
        }
    }
    Ok(())
}

/// Convert complete transcript snapshots without reading prompt/content fields.
pub fn convert(text: &str, map: &[Mapping]) -> Result<Vec<UsageRow>, String> {
    validate_map(map)?;
    let mut rows: BTreeMap<String, UsageRow> = BTreeMap::new();
    for (i, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let v: Value = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        if v["type"] != "assistant" {
            continue;
        }
        let row = response(&v).map_err(|e| format!("line {}: {e}", i + 1))?;
        if let Some(old) = rows.get_mut(&row.dedup_key) {
            merge(old, row)?;
        } else {
            rows.insert(row.dedup_key.clone(), row);
        }
    }
    for row in rows.values_mut() {
        if let (Some(start), Some(end)) = (bound(&row.start)?, bound(&row.end)?) {
            for m in map.iter().filter(|m| m.session_id == row.session_id) {
                if bound(&m.start)?.is_none_or(|s| start >= s)
                    && bound(&m.end)?.is_none_or(|e| end < e)
                {
                    row.category = m.category;
                    row.trial_key.clone_from(&m.trial_key);
                }
            }
        }
    }
    Ok(rows.into_values().collect())
}

/// Serialized, atomic merge. Conflicts never overwrite previously collected evidence.
pub fn import(transcript: &Path, session_map: &Path, out: &Path) -> Result<usize, String> {
    let error = |e: std::io::Error| e.to_string();
    if let Ok(dest) = out.canonicalize() {
        for input in [transcript, session_map] {
            if input.canonicalize().map_err(error)? == dest {
                return Err("output must differ from inputs".into());
            }
        }
    }
    let map: Vec<Mapping> = serde_json::from_str(&fs::read_to_string(session_map).map_err(error)?)
        .map_err(|e| e.to_string())?;
    let incoming = convert(&fs::read_to_string(transcript).map_err(error)?, &map)?;
    let mut lock_name = out.as_os_str().to_os_string();
    lock_name.push(".lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_name)
        .map_err(error)?;
    lock.lock().map_err(error)?;
    let mut rows: BTreeMap<String, UsageRow> = BTreeMap::new();
    let existing = match fs::read_to_string(out) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(error(e)),
    };
    for line in existing.lines().filter(|l| !l.trim().is_empty()) {
        let row: UsageRow = serde_json::from_str(line).map_err(|e| e.to_string())?;
        if row.schema != SCHEMA
            || row.dedup_key != format!("op:{}:{}", row.session_id, row.message_id)
            || rows.insert(row.dedup_key.clone(), row).is_some()
        {
            return Err("invalid existing operator usage ledger".into());
        }
    }
    let before = rows.len();
    for row in incoming {
        if let Some(old) = rows.get(&row.dedup_key) {
            if old != &row {
                return Err(format!(
                    "{}: conflict; regenerate to a new output",
                    row.dedup_key
                ));
            }
        } else {
            rows.insert(row.dedup_key.clone(), row);
        }
    }
    let added = rows.len() - before;
    if added > 0 || !out.exists() {
        let parent = out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(error)?;
        for row in rows.values() {
            serde_json::to_writer(&mut tmp, row).map_err(|e| e.to_string())?;
            writeln!(tmp).map_err(error)?;
        }
        tmp.as_file().sync_all().map_err(error)?;
        tmp.persist(out).map_err(|e| e.to_string())?;
    }
    Ok(added)
}

pub fn cli(args: &[String]) -> Result<String, String> {
    let mut options = BTreeMap::new();
    let mut rest = args[1..].iter();
    while let Some(key) = rest.next() {
        if !["--transcript", "--session-map", "--out"].contains(&key.as_str()) {
            return Err(format!("unknown option {key}"));
        }
        let value = rest
            .next()
            .filter(|v| !v.starts_with("--"))
            .ok_or("missing option value")?;
        if options.insert(key.as_str(), value).is_some() {
            return Err(format!("duplicate {key}"));
        }
    }
    let transcript = options
        .get("--transcript")
        .ok_or("--transcript is required")?;
    let map = Path::new(
        options
            .get("--session-map")
            .ok_or("--session-map is required")?,
    );
    let default = map.with_file_name("operator-usage.jsonl");
    let out = options.get("--out").map(Path::new).unwrap_or(&default);
    let added = import(Path::new(transcript), map, out)?;
    Ok(serde_json::json!({"added": added, "out": out}).to_string())
}
