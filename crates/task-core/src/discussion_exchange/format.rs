//! D4.2: the Markdown (front matter + body) and JSON forms of one envelope and of a snapshot.
//!
//! The front matter is the small YAML subset of the ADR's example: `key: value` lines, where a value is JSON
//! (what Celeris writes) or a flow mapping / list with bare scalars (`{kind: chat, revision: 1}`, what a person
//! writes by hand), and a block list (`references:` then `  - <value>` lines). The body is everything after the
//! closing `---` line, without the one trailing newline the writer adds; `content_hash` is the SHA-256 of that body,
//! so the Markdown and the JSON of the same envelope carry the same hash. In a snapshot, each item's front matter
//! carries `body_bytes`, so a body holding a `---` line is read back exactly.

use serde_json::{Map, Value};

use super::{
    DiscussionSnapshot, ENVELOPE_SCHEMA, ExchangeEnvelope, ExchangeItem, ExchangeRefusal, INVALID,
    SNAPSHOT_SCHEMA, content_hash,
};

const FENCE: &str = "---";

fn invalid(detail: impl Into<String>) -> ExchangeRefusal {
    ExchangeRefusal::new(INVALID, detail)
}

/// Writes `fields` as front matter lines (arrays as block lists, everything else as one line of JSON).
fn write_fields(out: &mut String, fields: &Map<String, Value>) {
    for (k, v) in fields {
        match v {
            Value::Array(items) if !items.is_empty() => {
                out.push_str(&format!("{k}:\n"));
                for item in items {
                    out.push_str(&format!("  - {item}\n"));
                }
            }
            _ => out.push_str(&format!("{k}: {v}\n")),
        }
    }
}

/// Splits a flow collection's inside at top-level commas (outside quotes and nested brackets).
fn split_top(s: &str) -> Vec<&str> {
    let (mut depth, mut quote, mut start, mut out) = (0i32, None::<char>, 0usize, Vec::new());
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '{' | '[') => depth += 1,
            (None, '}' | ']') => depth -= 1,
            (None, ',') if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out.into_iter().filter(|p| !p.trim().is_empty()).collect()
}

/// One front matter value: JSON if it parses, else a flow mapping / list, else a bare (optionally quoted) string.
fn parse_value(s: &str) -> Value {
    let s = s.trim();
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        return v;
    }
    if let Some(inner) = s.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
        let mut map = Map::new();
        for part in split_top(inner) {
            let (k, v) = part.split_once(": ").unwrap_or((part, ""));
            map.insert(k.trim().trim_matches('"').to_owned(), parse_value(v));
        }
        return Value::Object(map);
    }
    if let Some(inner) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return Value::Array(split_top(inner).into_iter().map(parse_value).collect());
    }
    let unquoted = s
        .strip_prefix('\'')
        .and_then(|r| r.strip_suffix('\''))
        .unwrap_or(s);
    Value::String(unquoted.to_owned())
}

/// Reads front matter lines into a map.
fn parse_fields<'a>(
    lines: impl Iterator<Item = &'a str>,
) -> Result<Map<String, Value>, ExchangeRefusal> {
    let mut map = Map::new();
    let mut list: Option<String> = None;
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(item) = line.trim_start().strip_prefix("- ")
            && line.starts_with(' ')
        {
            let key = list
                .as_ref()
                .ok_or_else(|| invalid(format!("list item without a key: {line}")))?;
            if let Some(Value::Array(a)) = map.get_mut(key) {
                a.push(parse_value(item));
            }
            continue;
        }
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| invalid(format!("front matter line is not `key: value`: {line}")))?;
        let k = k.trim().to_owned();
        if map.contains_key(&k) {
            return Err(invalid(format!("front matter key `{k}` appears twice")));
        }
        if v.trim().is_empty() {
            map.insert(k.clone(), Value::Array(Vec::new()));
            list = Some(k);
        } else {
            map.insert(k, parse_value(v));
            list = None;
        }
    }
    Ok(map)
}

/// Splits `text` at its front matter: returns the fields and the rest after the closing fence line.
fn split_front_matter(text: &str) -> Result<(Map<String, Value>, &str), ExchangeRefusal> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or_else(|| invalid("the Markdown does not start with a `---` front matter"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == FENCE {
            let fields = parse_fields(rest[..offset].lines())?;
            return Ok((fields, &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    Err(invalid("the front matter has no closing `---`"))
}

fn without_trailing_newline(body: &str) -> &str {
    body.strip_suffix('\n').unwrap_or(body)
}

/// D4.2: the fields of a Markdown envelope (front matter plus `body`), before the typed parse, so the caller can
/// refuse self-reported keys by name.
pub(super) fn markdown_fields(text: &str) -> Result<Map<String, Value>, ExchangeRefusal> {
    let (mut fields, body) = split_front_matter(text)?;
    if fields.contains_key("body") {
        return Err(invalid(
            "`body` is the text after the front matter, not a field",
        ));
    }
    fields.insert(
        "body".into(),
        Value::String(without_trailing_newline(body).to_owned()),
    );
    Ok(fields)
}

fn to_fields<T: serde::Serialize>(v: &T) -> Map<String, Value> {
    match serde_json::to_value(v) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

/// D4.2: one envelope as Markdown (`schema` first, then the fields, then the body).
pub fn envelope_to_markdown(env: &ExchangeEnvelope) -> String {
    let mut fields = to_fields(env);
    let body = fields
        .remove("body")
        .and_then(|b| b.as_str().map(str::to_owned))
        .unwrap_or_default();
    fields.remove("schema");
    let mut out = format!("---\nschema: {ENVELOPE_SCHEMA}\n");
    write_fields(&mut out, &fields);
    out.push_str("---\n");
    out.push_str(&body);
    out.push('\n');
    out
}

/// D4.2: reads one Markdown envelope.
pub fn envelope_from_markdown(text: &str) -> Result<ExchangeEnvelope, ExchangeRefusal> {
    serde_json::from_value(Value::Object(markdown_fields(text)?))
        .map_err(|e| invalid(e.to_string()))
}

/// D4.2: one envelope as JSON.
pub fn envelope_to_json(env: &ExchangeEnvelope) -> String {
    serde_json::to_string_pretty(env).unwrap_or_default()
}

/// D4.2: reads one JSON envelope.
pub fn envelope_from_json(text: &str) -> Result<ExchangeEnvelope, ExchangeRefusal> {
    serde_json::from_str(text).map_err(|e| invalid(e.to_string()))
}

/// D1.4: a snapshot as JSON.
pub fn snapshot_to_json(s: &DiscussionSnapshot) -> String {
    serde_json::to_string_pretty(s).unwrap_or_default()
}

/// D1.4: reads a snapshot's JSON; every item's `content_hash` must match its body.
pub fn snapshot_from_json(text: &str) -> Result<DiscussionSnapshot, ExchangeRefusal> {
    let s: DiscussionSnapshot = serde_json::from_str(text).map_err(|e| invalid(e.to_string()))?;
    check_hashes(&s)?;
    Ok(s)
}

/// D1.4: a snapshot as Markdown — the snapshot's fields, then one front matter + body per item.
pub fn snapshot_to_markdown(s: &DiscussionSnapshot) -> String {
    let mut head = to_fields(s);
    head.remove("schema");
    head.remove("items");
    let mut out = format!("---\nschema: {SNAPSHOT_SCHEMA}\n");
    write_fields(&mut out, &head);
    out.push_str(&format!("items: {}\n---\n", s.items.len()));
    for item in &s.items {
        let mut fields = to_fields(item);
        let body = fields
            .remove("body")
            .and_then(|b| b.as_str().map(str::to_owned))
            .unwrap_or_default();
        fields.insert("body_bytes".into(), Value::from(body.len()));
        out.push_str("---\n");
        write_fields(&mut out, &fields);
        out.push_str("---\n");
        out.push_str(&body);
        out.push('\n');
    }
    out
}

/// D1.4: reads a snapshot's Markdown; every item's `content_hash` must match its body.
pub fn snapshot_from_markdown(text: &str) -> Result<DiscussionSnapshot, ExchangeRefusal> {
    let (mut head, mut rest) = split_front_matter(text)?;
    let count = head
        .remove("items")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| invalid("the snapshot front matter has no `items` count"))?;
    let mut items = Vec::new();
    for _ in 0..count {
        let (mut fields, after) = split_front_matter(rest)?;
        let len = fields
            .remove("body_bytes")
            .and_then(|v| v.as_u64())
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| invalid("an item has no `body_bytes`"))?;
        let body = after
            .get(..len)
            .ok_or_else(|| invalid("an item's body is shorter than `body_bytes`"))?;
        fields.insert("body".into(), Value::String(body.to_owned()));
        let item: ExchangeItem =
            serde_json::from_value(Value::Object(fields)).map_err(|e| invalid(e.to_string()))?;
        items.push(item);
        rest = after[len..].strip_prefix('\n').unwrap_or(&after[len..]);
    }
    head.insert(
        "items".into(),
        serde_json::to_value(items).map_err(|e| invalid(e.to_string()))?,
    );
    let s: DiscussionSnapshot =
        serde_json::from_value(Value::Object(head)).map_err(|e| invalid(e.to_string()))?;
    check_hashes(&s)?;
    Ok(s)
}

fn check_hashes(s: &DiscussionSnapshot) -> Result<(), ExchangeRefusal> {
    match s
        .items
        .iter()
        .find(|i| i.content_hash != content_hash(&i.body))
    {
        Some(i) => Err(ExchangeRefusal {
            code: super::RULES,
            rules: vec!["R4:content_hash"],
            detail: format!("item {} does not match its content_hash", i.message_id),
        }),
        None => Ok(()),
    }
}
