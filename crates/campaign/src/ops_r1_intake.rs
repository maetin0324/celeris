//! ops-r1・ops-r2 B の CoS 受付の記録（inventory の g-b-cos-intake）。B の依頼は task API へ直接は入れず、隔離した
//! 測定案件の案件 CoS thread へ、認可済みの operator（または MCP）principal の relay として入る
//! （ADR 2026-10-30-ops-r2-isolation-principal D5）。この道具はその message を trial と人の指示の出典に結ぶ:
//! - `--relay`: operator principal の token で固定依頼文を `POST …/relays` に送る（`client_message_id` は trial から
//!   決めるので再実行しても 1 件。`said_at` は出典の時刻）。
//! - `--bind-message <id>`: 既に relay された message を探して結ぶ。
//!
//! どちらも `--source <source.json>`（人の指示・承認の出典）が要る。message は `relay` を持つ `role=user` で、
//! `author`（daemon が認証した主体）が `operator:`・`mcp:` で `relay.relayed_by` と同じ、`relay.said_at` が出典の
//! `said_at` と同じときだけ記録する。`author` が `human:` の message（人の token を誰が使ったか記録で区別できない）と、
//! relay の無い operator の message（出典が無い）は人の依頼として数えない。人の token は使わない。
//! 呼ぶのは `GET /api/v1/chat/threads/{t}`・`GET …/messages`・`POST …/relays` だけで、`POST /api/v1/tasks` は
//! 呼ばない（起票は CoS がする）。記録は API の値と出典の file だけから作るので、同じ入力からは byte 一致で再生成できる。
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    celeris_exec::Transport,
    human_ops::{Actor, ActorKind},
};

pub const SCHEMA: &str = "ops-r1-intake/2";
/// K4（ops-r1）の記録の schema。読み手（`ops-r1-assemble`）が過去の試行の記録を読むためだけに残す。この道具は書かない。
pub const SCHEMA_V1: &str = "ops-r1-intake/1";
/// `client_message_id` の接頭辞（daemon の上限 128 byte に収める）。
const CLIENT_KEY_PREFIX: &str = "ops-r1-intake:";
const CLIENT_KEY_MAX_BYTES: usize = 128;
/// `GET …/messages` の 1 頁（API の上限）。
const PAGE_LIMIT: u32 = 200;

/// 受付の記録として読める schema か（今の `/2` と、K4 の `/1`）。
pub fn is_intake_schema(s: &str) -> bool {
    s == SCHEMA || s == SCHEMA_V1
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 既に relay された message を結ぶ。
    Bind(String),
    /// operator principal の token で固定依頼文を relay する。
    Relay,
}

/// 人の指示の出典の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// 人が承認票で承認した固定依頼文（`decision_id` が承認の id）。
    ApprovedRequest,
    /// 人の発言の原文（`thread_id`・`message_id` が人の message）。
    ChatMessage,
}

/// `--source` の file（人が承認票・本番 daemon の記録から書く）。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceInput {
    pub kind: SourceKind,
    /// 人の指示がある daemon。`production` だけ。
    pub daemon: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub decision_id: Option<String>,
    /// 人が言った・承認した時刻（RFC 3339 UTC）。relay の `said_at` に使う。
    pub said_at: String,
    /// 承認票・人の message に書かれた依頼文の sha256。
    pub request_sha256: String,
    /// 出典の主体（`human:<name>`）。
    pub decided_by: String,
    /// 有料試行を承認した主体（`human:<name>`）。
    pub approved_by: String,
}

/// 記録に残す出典。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Source {
    pub kind: SourceKind,
    pub daemon: String,
    pub thread_id: Option<String>,
    pub message_id: Option<String>,
    pub decision_id: Option<String>,
    pub said_at: String,
    pub request_sha256: String,
}

/// human-ops /2 の relay の op 行に写す値（`celeris-campaign human-op record` の引数と同じ書式）。
/// 判断者・実行者・承認者を別の欄に置く。人の能動時間には数えない（`actor` が人でない）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HumanOpRoles {
    /// `--actor-kind`・`--actor`（= `executed_by`）。
    pub actor_kind: String,
    pub actor: String,
    /// `--decided-by`・`--executed-by`・`--approved-by`（`<actor_kind>:<id>`）。
    pub decided_by: String,
    pub executed_by: String,
    pub approved_by: String,
    /// `--credential`（relay を認証した principal。値は書かない）。
    pub credential: String,
    /// `--request-ref`・`--request-revision`（relay の message と seq）。
    pub request_ref: String,
    pub request_revision: String,
    /// `--policy-ref`（出典）。
    pub policy_ref: String,
    /// `--celeris-event-ref`（relay の message id）。
    pub celeris_event_ref: String,
}

#[derive(Clone, Debug)]
pub struct IntakeInput {
    pub thread_id: String,
    pub trial_id: String,
    pub condition_id: String,
    /// 固定依頼文（`--request-file` の bytes。UTF-8 であること）。
    pub request_text: String,
    pub mode: Mode,
    pub source: SourceInput,
    /// 実験専用 operator session の id（`--operator-session`）。human-ops の `actor` に使う。無ければ relay の author。
    pub operator_session: Option<String>,
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
    /// 依頼文（`--request-file`）の sha256。message の本文・出典の sha256 と一致したときだけ記録する。
    pub request_sha256: String,
    pub request_bytes: usize,
    /// `bind`（relay 済みの message を結んだ）か `relay`（この道具が operator principal で relay した）。
    pub intake_mode: String,
    pub client_message_id: Option<String>,
    /// message を記録した主体（daemon が認証した `author`）。`executed_by` と同じ。
    pub actor: Actor,
    /// 人の指示・承認の出典。
    pub source: Source,
    /// 判断者（出典の主体。人）。
    pub decided_by: Actor,
    /// 実行者（relay の `author`。daemon が認証した値）。
    pub executed_by: Actor,
    /// 承認者（有料試行を承認した人。承認の出典は `source.decision_id`）。
    pub approved_by: Actor,
    /// relay の `relay.relayed_by`。
    pub relayed_by: String,
    pub human_op: HumanOpRoles,
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

fn human(name: &str, v: &str) -> Result<Actor, String> {
    ident(name, v)?;
    let a = actor_of(Some(v));
    if a.kind != ActorKind::Human || v.len() <= "human:".len() {
        return Err(format!("{name} must be human:<name>, got {v:?}"));
    }
    Ok(a)
}

fn rfc3339(s: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

fn is_rfc3339_utc(s: &str) -> bool {
    rfc3339(s).is_some_and(|t| t.offset().is_utc()) && s.ends_with('Z')
}

/// 出典を検査して記録の形にする。依頼文の sha256 と一致しなければ記録しない。
fn source_of(s: &SourceInput, request_sha256: &str) -> Result<(Source, Actor, Actor), String> {
    if s.daemon != "production" {
        return Err(format!(
            "--source: daemon must be \"production\" (the daemon where the person instructed), got {:?}",
            s.daemon
        ));
    }
    for (name, v) in [
        ("source.thread_id", &s.thread_id),
        ("source.message_id", &s.message_id),
        ("source.decision_id", &s.decision_id),
    ] {
        if let Some(v) = v {
            ident(name, v)?;
        }
    }
    match s.kind {
        SourceKind::ApprovedRequest if s.decision_id.is_none() => {
            return Err("--source: approved_request needs decision_id (the approval)".into());
        }
        SourceKind::ChatMessage if s.thread_id.is_none() || s.message_id.is_none() => {
            return Err(
                "--source: chat_message needs thread_id and message_id (the person's message)"
                    .into(),
            );
        }
        _ => {}
    }
    if !is_rfc3339_utc(&s.said_at) {
        return Err(format!(
            "--source: said_at must be RFC 3339 UTC (…Z), got {:?}",
            s.said_at
        ));
    }
    if s.request_sha256 != request_sha256 {
        return Err(format!(
            "--source: request_sha256 {} differs from --request-file sha256 {request_sha256}; not recorded",
            s.request_sha256
        ));
    }
    let decided = human("source.decided_by", &s.decided_by)?;
    let approved = human("source.approved_by", &s.approved_by)?;
    Ok((
        Source {
            kind: s.kind,
            daemon: s.daemon.clone(),
            thread_id: s.thread_id.clone(),
            message_id: s.message_id.clone(),
            decision_id: s.decision_id.clone(),
            said_at: s.said_at.clone(),
            request_sha256: s.request_sha256.clone(),
        },
        decided,
        approved,
    ))
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

/// `POST …/relays`（ADR 2026-10-27-relayed-instructions D2）。task は指定しない（案件全体への依頼）。
fn post_relay(
    t: &dyn Transport,
    thread_id: &str,
    key: &str,
    text: &str,
    said_at: &str,
) -> Result<Value, String> {
    let path = format!("/api/v1/chat/threads/{thread_id}/relays");
    let body = json!({
        "client_message_id": key,
        "text": text,
        "said_at": said_at,
        "task_ids": [],
        "expires_at": null,
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

/// relay の主体を確かめる: `author` が operator か MCP で、`relay.relayed_by` と同じ、`said_at` が出典と同じ。
/// 返すのは (`author` の actor, `relayed_by`)。
fn relay_author(
    message: &Value,
    message_id: &str,
    said_at: &str,
) -> Result<(Actor, String), String> {
    let actor = actor_of(message.get("author").and_then(Value::as_str));
    match actor.kind {
        ActorKind::ExternalOperator | ActorKind::McpClient => {}
        ActorKind::Human => {
            return Err(format!(
                "message {message_id} was posted with a person's token ({}); ops-r2 takes the request \
                 only as an operator/MCP relay with its source; not recorded",
                actor.id
            ));
        }
        _ => {
            return Err(format!(
                "message {message_id} was posted by {} ({}), not by an operator/MCP principal; not recorded",
                actor.id,
                actor.kind.as_str()
            ));
        }
    }
    let Some(relay) = message.get("relay").filter(|r| r.is_object()) else {
        return Err(format!(
            "message {message_id} by {} has no relay (no source of a person's instruction); \
             not counted as the person's request",
            actor.id
        ));
    };
    let relayed_by = str_field(relay, "relayed_by", "message.relay")?;
    if relayed_by != actor.id {
        return Err(format!(
            "message {message_id}: relay.relayed_by {relayed_by} differs from author {}",
            actor.id
        ));
    }
    let relay_said = str_field(relay, "said_at", "message.relay")?;
    // daemon は時刻を正規化して返す（`…:03Z` → `…:03.000Z`）ので、同じ時点かで比べる。
    if rfc3339(relay_said).is_none() || rfc3339(relay_said) != rfc3339(said_at) {
        return Err(format!(
            "message {message_id}: relay.said_at {relay_said} differs from source.said_at {said_at}"
        ));
    }
    Ok((actor, relayed_by.to_string()))
}

/// 受付を記録する。出典と message の検査（thread・role・状態・本文・relay の主体）が全部通ったときだけ記録を返す。
pub fn intake(t: &dyn Transport, input: &IntakeInput) -> Result<IntakeRecord, String> {
    ident("--thread", &input.thread_id)?;
    ident("--trial-id", &input.trial_id)?;
    ident("--condition-id", &input.condition_id)?;
    if let Some(s) = &input.operator_session {
        ident("--operator-session", s)?;
    }
    if input.request_text.trim().is_empty() {
        return Err("--request-file is empty".into());
    }
    let request_sha256 = sha256_hex(input.request_text.as_bytes());
    // 出典は送る前に確かめる（出典の無い依頼は relay しない）。
    let (source, decided_by, approved_by) = source_of(&input.source, &request_sha256)?;
    let key = client_message_id(&input.trial_id)?;
    let project_id = project_thread(t, &input.thread_id)?;
    let (message, mode) = match &input.mode {
        Mode::Bind(id) => {
            ident("--bind-message", id)?;
            (find_message(t, &input.thread_id, id)?, "bind")
        }
        Mode::Relay => (
            post_relay(
                t,
                &input.thread_id,
                &key,
                &input.request_text,
                &source.said_at,
            )?,
            "relay",
        ),
    };
    let message_id = str_field(&message, "id", "message")?.to_string();
    if str_field(&message, "thread_id", "message")? != input.thread_id {
        return Err(format!("message {message_id} belongs to another thread"));
    }
    let role = str_field(&message, "role", "message")?;
    if role != "user" {
        return Err(format!(
            "message {message_id} has role {role}; a relayed request is role=user"
        ));
    }
    let state = str_field(&message, "state", "message")?;
    if state == "cancelled" {
        return Err(format!("message {message_id} was cancelled"));
    }
    let text_sha256 = sha256_hex(str_field(&message, "text", "message")?.as_bytes());
    if text_sha256 != request_sha256 {
        return Err(format!(
            "message {message_id} text sha256 {text_sha256} differs from --request-file sha256 {request_sha256}"
        ));
    }
    let (actor, relayed_by) = relay_author(&message, &message_id, &source.said_at)?;
    let message_seq = message
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or("message: missing seq")?;
    let policy_ref = match (&source.decision_id, &source.message_id) {
        (Some(d), _) if source.kind == SourceKind::ApprovedRequest => format!("decision:{d}"),
        (_, Some(m)) => format!("chat-message:{m}"),
        (Some(d), None) => format!("decision:{d}"),
        (None, None) => return Err("--source: no thread/message/decision id".into()),
    };
    let role_str = |a: &Actor| format!("{}:{}", a.kind.as_str(), a.id);
    let op_actor = input
        .operator_session
        .clone()
        .unwrap_or_else(|| actor.id.clone());
    let op_executor = Actor {
        kind: actor.kind,
        id: op_actor.clone(),
    };
    let human_op = HumanOpRoles {
        actor_kind: actor.kind.as_str().into(),
        actor: op_actor,
        decided_by: role_str(&decided_by),
        executed_by: role_str(&op_executor),
        approved_by: role_str(&approved_by),
        credential: actor.id.clone(),
        request_ref: format!("chat-message:{message_id}"),
        request_revision: message_seq.to_string(),
        policy_ref,
        celeris_event_ref: message_id.clone(),
    };
    Ok(IntakeRecord {
        schema: SCHEMA.into(),
        trial_id: input.trial_id.clone(),
        condition_id: input.condition_id.clone(),
        thread_id: input.thread_id.clone(),
        project_id,
        message_id,
        message_seq,
        accepted_at: str_field(&message, "created_at", "message")?.to_string(),
        request_sha256,
        request_bytes: input.request_text.len(),
        intake_mode: mode.into(),
        client_message_id: message
            .get("client_message_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        executed_by: actor.clone(),
        actor,
        source,
        decided_by,
        approved_by,
        relayed_by,
        human_op,
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
--thread <thread_id> --trial-id <id> --condition-id <id> --request-file <path> --source <source.json> \
[--operator-session <id>] (--bind-message <message_id> | --relay) --out <intake.json>";

/// `celeris-campaign ops-r1-intake …`。transport は呼び出し側が作る（試験は偽 API を渡す）。
pub fn cli_with(args: &[String], t: &dyn Transport) -> Result<String, String> {
    let o = Options::parse(args)?;
    let text = std::fs::read(&o.request_file).map_err(|e| format!("{}: {e}", o.request_file))?;
    let request_text =
        String::from_utf8(text).map_err(|_| format!("{}: not UTF-8", o.request_file))?;
    let raw = std::fs::read(&o.source).map_err(|e| format!("{}: {e}", o.source))?;
    let source: SourceInput =
        serde_json::from_slice(&raw).map_err(|e| format!("{}: {e}", o.source))?;
    let rec = intake(
        t,
        &IntakeInput {
            thread_id: o.thread,
            trial_id: o.trial_id,
            condition_id: o.condition_id,
            request_text,
            mode: o.mode,
            source,
            operator_session: o.operator_session,
        },
    )?;
    write_record(Path::new(&o.out), &rec)
}

/// 引数（`--api-url`・`--token-env` は transport を作るために `main` も読む）。
#[derive(Debug)]
pub struct Options {
    pub api_url: String,
    pub token_env: String,
    pub thread: String,
    pub trial_id: String,
    pub condition_id: String,
    pub request_file: String,
    pub source: String,
    pub operator_session: Option<String>,
    pub mode: Mode,
    pub out: String,
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut v: std::collections::BTreeMap<&str, String> = Default::default();
        let mut relay = false;
        let mut rest = args.iter().skip(1);
        while let Some(k) = rest.next() {
            let k = k.as_str();
            if k == "--relay" {
                if relay {
                    return Err("duplicate --relay".into());
                }
                relay = true;
                continue;
            }
            const KEYS: [&str; 10] = [
                "--api-url",
                "--token-env",
                "--thread",
                "--trial-id",
                "--condition-id",
                "--request-file",
                "--source",
                "--operator-session",
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
            source: req("--source")?,
            out: req("--out")?,
            operator_session: None,
            mode: Mode::Relay,
        };
        o.operator_session = v.remove("--operator-session");
        o.mode = match (v.remove("--bind-message"), relay) {
            (Some(m), false) => Mode::Bind(m),
            (None, true) => Mode::Relay,
            _ => {
                return Err(format!(
                    "give exactly one of --bind-message or --relay\n{USAGE}"
                ));
            }
        };
        Ok(o)
    }
}

#[cfg(test)]
#[path = "ops_r1_intake_tests.rs"]
mod tests;
