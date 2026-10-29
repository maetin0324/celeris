//! ops-r1 B の CoS 受付の記録（inventory の g-b-cos-intake）。B の依頼は task API へ直接は入れず、隔離した
//! 測定案件の案件 CoS thread へ人の message として入る。この道具はその message を trial に結ぶ:
//! - `--bind-message <id>`: 人が web・celerisctl で送った message を探して結ぶ。
//! - `--send`: 人の token で固定依頼文を送る（`client_message_id` は trial から決めるので再実行しても 1 件）。
//!
//! どちらも message の `author`（daemon が認証した主体）を actor として記録し、人（`human:`）以外は拒否する。
//! 呼ぶのは `GET /api/v1/chat/threads/{t}`・`GET …/messages`・`POST …/messages` だけで、`POST /api/v1/tasks` は
//! 呼ばない（起票は CoS がする）。記録は API の値だけから作るので、同じ message からは byte 一致で再生成できる。
use std::path::Path;

use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    celeris_exec::Transport,
    human_ops::{Actor, ActorKind},
};

pub const SCHEMA: &str = "ops-r1-intake/1";
/// `client_message_id` の接頭辞（daemon の上限 128 byte に収める）。
const CLIENT_KEY_PREFIX: &str = "ops-r1-intake:";
const CLIENT_KEY_MAX_BYTES: usize = 128;
/// `GET …/messages` の 1 頁（API の上限）。
const PAGE_LIMIT: u32 = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 人が送った message を結ぶ。
    Bind(String),
    /// 人の token で固定依頼文を送る。
    Send,
}

#[derive(Clone, Debug)]
pub struct IntakeInput {
    pub thread_id: String,
    pub trial_id: String,
    pub condition_id: String,
    /// 固定依頼文（`--request-file` の bytes。UTF-8 であること）。
    pub request_text: String,
    pub mode: Mode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IntakeRecord {
    pub schema: String,
    pub trial_id: String,
    pub condition_id: String,
    pub thread_id: String,
    pub project_id: String,
    pub message_id: String,
    pub message_seq: u64,
    /// 受付時刻 = daemon が message を記録した時刻（`created_at`、RFC 3339 UTC）。
    pub accepted_at: String,
    /// 依頼文（`--request-file`）の sha256。message の本文の sha256 と一致したときだけ記録する。
    pub request_sha256: String,
    pub request_bytes: usize,
    /// `bind`（人が送った message を結んだ）か `send`（この道具が人の token で送った）。
    pub intake_mode: String,
    pub client_message_id: Option<String>,
    /// message を記録した主体（daemon が認証した `author`）。
    pub actor: Actor,
}

pub fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// trial から決める `client_message_id`（同じ trial の再送は daemon が同じ message として返す）。
pub fn client_message_id(trial_id: &str) -> Result<String, String> {
    let key = format!("{CLIENT_KEY_PREFIX}{trial_id}");
    if key.len() > CLIENT_KEY_MAX_BYTES {
        return Err(format!(
            "--trial-id is too long for client_message_id ({} > {CLIENT_KEY_MAX_BYTES} bytes)",
            key.len()
        ));
    }
    Ok(key)
}

/// daemon の `author`（`human:<name>` 等）を human-ops の actor 語彙にする。
pub fn actor_of(author: Option<&str>) -> Actor {
    let Some(a) = author else {
        return Actor {
            kind: ActorKind::Unknown,
            id: "unknown".into(),
        };
    };
    let kind = match a.split_once(':').map(|(k, _)| k) {
        Some("human") => ActorKind::Human,
        Some("operator") => ActorKind::ExternalOperator,
        Some("mcp") => ActorKind::McpClient,
        _ => ActorKind::Unknown,
    };
    Actor { kind, id: a.into() }
}

fn ident(name: &str, v: &str) -> Result<(), String> {
    if v.is_empty()
        || !v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return Err(format!(
            "{name} must be non-empty [A-Za-z0-9._:-], got {v:?}"
        ));
    }
    Ok(())
}

fn get(t: &dyn Transport, path: &str) -> Result<Value, String> {
    let r = t.request("GET", path, None)?;
    if r.status != 200 {
        return Err(format!("GET {path}: HTTP {} {}", r.status, r.body));
    }
    Ok(r.body)
}

fn str_field<'a>(v: &'a Value, f: &str, what: &str) -> Result<&'a str, String> {
    v.get(f)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{what}: missing {f}"))
}

/// thread が開いた案件 CoS thread であることを確かめ、project_id を返す。
fn project_thread(t: &dyn Transport, thread_id: &str) -> Result<String, String> {
    let d = get(t, &format!("/api/v1/chat/threads/{thread_id}"))?;
    let th = d.get("thread").ok_or("thread detail: missing thread")?;
    if str_field(th, "id", "thread")? != thread_id {
        return Err("thread detail: id does not match --thread".into());
    }
    let kind = str_field(th, "kind", "thread")?;
    if kind != "project" {
        return Err(format!(
            "thread {thread_id} is a {kind} thread; B's request enters a project CoS thread"
        ));
    }
    let status = str_field(th, "status", "thread")?;
    if status != "open" {
        return Err(format!("thread {thread_id} is {status}, not open"));
    }
    Ok(str_field(th, "project_id", "thread")?.to_string())
}

/// thread の message を新しい方から頁送りで探す。
fn find_message(t: &dyn Transport, thread_id: &str, message_id: &str) -> Result<Value, String> {
    let mut before: Option<u64> = None;
    loop {
        let mut path = format!("/api/v1/chat/threads/{thread_id}/messages?limit={PAGE_LIMIT}");
        if let Some(b) = before {
            path.push_str(&format!("&before_seq={b}"));
        }
        let page = get(t, &path)?;
        let items = page
            .get("items")
            .and_then(Value::as_array)
            .ok_or("messages: missing items")?;
        if let Some(m) = items
            .iter()
            .find(|m| m.get("id").and_then(Value::as_str) == Some(message_id))
        {
            return Ok(m.clone());
        }
        match page.get("next_before_seq").and_then(Value::as_u64) {
            Some(b) if !items.is_empty() && Some(b) != before => before = Some(b),
            _ => return Err(format!("message {message_id} is not in thread {thread_id}")),
        }
    }
}

fn post_message(
    t: &dyn Transport,
    thread_id: &str,
    key: &str,
    text: &str,
) -> Result<Value, String> {
    let path = format!("/api/v1/chat/threads/{thread_id}/messages");
    let body = json!({
        "client_message_id": key,
        "text": text,
        "attachment_ids": [],
        "reply_to_id": null,
        "mode": "queue",
        "resume_queue": false,
    });
    let r = t.request("POST", &path, Some(&body))?;
    if !(200..300).contains(&r.status) {
        return Err(format!("POST {path}: HTTP {} {}", r.status, r.body));
    }
    r.body
        .get("message")
        .cloned()
        .ok_or_else(|| format!("POST {path}: response has no message"))
}

/// 受付を記録する。message の検査（thread・role・状態・本文・主体）が全部通ったときだけ記録を返す。
pub fn intake(t: &dyn Transport, input: &IntakeInput) -> Result<IntakeRecord, String> {
    ident("--thread", &input.thread_id)?;
    ident("--trial-id", &input.trial_id)?;
    ident("--condition-id", &input.condition_id)?;
    if input.request_text.trim().is_empty() {
        return Err("--request-file is empty".into());
    }
    let key = client_message_id(&input.trial_id)?;
    let project_id = project_thread(t, &input.thread_id)?;
    let (message, mode) = match &input.mode {
        Mode::Bind(id) => {
            ident("--bind-message", id)?;
            (find_message(t, &input.thread_id, id)?, "bind")
        }
        Mode::Send => (
            post_message(t, &input.thread_id, &key, &input.request_text)?,
            "send",
        ),
    };
    let message_id = str_field(&message, "id", "message")?.to_string();
    if str_field(&message, "thread_id", "message")? != input.thread_id {
        return Err(format!("message {message_id} belongs to another thread"));
    }
    let role = str_field(&message, "role", "message")?;
    if role != "user" {
        return Err(format!(
            "message {message_id} has role {role}; a person's message is role=user"
        ));
    }
    let state = str_field(&message, "state", "message")?;
    if state == "cancelled" {
        return Err(format!("message {message_id} was cancelled"));
    }
    let request_sha256 = sha256_hex(input.request_text.as_bytes());
    let text_sha256 = sha256_hex(str_field(&message, "text", "message")?.as_bytes());
    if text_sha256 != request_sha256 {
        return Err(format!(
            "message {message_id} text sha256 {text_sha256} differs from --request-file sha256 {request_sha256}"
        ));
    }
    let actor = actor_of(message.get("author").and_then(Value::as_str));
    if actor.kind != ActorKind::Human {
        return Err(format!(
            "message {message_id} was posted by {} ({}), not by a person's token; not recorded",
            actor.id,
            actor.kind.as_str()
        ));
    }
    Ok(IntakeRecord {
        schema: SCHEMA.into(),
        trial_id: input.trial_id.clone(),
        condition_id: input.condition_id.clone(),
        thread_id: input.thread_id.clone(),
        project_id,
        message_id,
        message_seq: message
            .get("seq")
            .and_then(Value::as_u64)
            .ok_or("message: missing seq")?,
        accepted_at: str_field(&message, "created_at", "message")?.to_string(),
        request_sha256,
        request_bytes: input.request_text.len(),
        intake_mode: mode.into(),
        client_message_id: message
            .get("client_message_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        actor,
    })
}

/// 記録を `out` に書く。既にあれば、同じ bytes のときだけ受ける（別の受付で上書きしない）。
pub fn write_record(out: &Path, rec: &IntakeRecord) -> Result<String, String> {
    let mut s = serde_json::to_string_pretty(rec).map_err(|e| e.to_string())?;
    s.push('\n');
    match std::fs::read(out) {
        Ok(old) if old == s.as_bytes() => return Ok(s),
        Ok(_) => {
            return Err(format!(
                "{} already holds a different intake record; not overwritten",
                out.display()
            ));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", out.display())),
    }
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(out, &s).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(s)
}

pub const USAGE: &str = "usage: celeris-campaign ops-r1-intake --api-url <origin> --token-env <ENV_NAME> \
--thread <thread_id> --trial-id <id> --condition-id <id> --request-file <path> \
(--bind-message <message_id> | --send) --out <intake.json>";

/// `celeris-campaign ops-r1-intake …`。transport は呼び出し側が作る（試験は偽 API を渡す）。
pub fn cli_with(args: &[String], t: &dyn Transport) -> Result<String, String> {
    let o = Options::parse(args)?;
    let text = std::fs::read(&o.request_file).map_err(|e| format!("{}: {e}", o.request_file))?;
    let request_text =
        String::from_utf8(text).map_err(|_| format!("{}: not UTF-8", o.request_file))?;
    let rec = intake(
        t,
        &IntakeInput {
            thread_id: o.thread,
            trial_id: o.trial_id,
            condition_id: o.condition_id,
            request_text,
            mode: o.mode,
        },
    )?;
    write_record(Path::new(&o.out), &rec)
}

/// 引数（`--api-url`・`--token-env` は transport を作るために `main` も読む）。
pub struct Options {
    pub api_url: String,
    pub token_env: String,
    pub thread: String,
    pub trial_id: String,
    pub condition_id: String,
    pub request_file: String,
    pub mode: Mode,
    pub out: String,
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut v: std::collections::BTreeMap<&str, String> = Default::default();
        let mut send = false;
        let mut rest = args.iter().skip(1);
        while let Some(k) = rest.next() {
            let k = k.as_str();
            if k == "--send" {
                if send {
                    return Err("duplicate --send".into());
                }
                send = true;
                continue;
            }
            const KEYS: [&str; 8] = [
                "--api-url",
                "--token-env",
                "--thread",
                "--trial-id",
                "--condition-id",
                "--request-file",
                "--bind-message",
                "--out",
            ];
            if !KEYS.contains(&k) {
                return Err(format!("unknown option {k}\n{USAGE}"));
            }
            let val = rest
                .next()
                .filter(|x| !x.starts_with("--"))
                .ok_or_else(|| format!("{k} needs a value"))?;
            if v.insert(k, val.clone()).is_some() {
                return Err(format!("duplicate {k}"));
            }
        }
        let mut req = |k: &str| {
            v.remove(k)
                .ok_or_else(|| format!("{k} is required\n{USAGE}"))
        };
        let mut o = Self {
            api_url: req("--api-url")?,
            token_env: req("--token-env")?,
            thread: req("--thread")?,
            trial_id: req("--trial-id")?,
            condition_id: req("--condition-id")?,
            request_file: req("--request-file")?,
            out: req("--out")?,
            mode: Mode::Send,
        };
        o.mode = match (v.remove("--bind-message"), send) {
            (Some(m), false) => Mode::Bind(m),
            (None, true) => Mode::Send,
            _ => {
                return Err(format!(
                    "give exactly one of --bind-message or --send\n{USAGE}"
                ));
            }
        };
        Ok(o)
    }
}

#[cfg(test)]
#[path = "ops_r1_intake_tests.rs"]
mod tests;
