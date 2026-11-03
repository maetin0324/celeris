//! ADR 2026-11-02-ts-control-layer-boundary 付記 S4-16（unit traces-async）: digest pass の trace。
//!
//! 各場面は本物の dispatcher の経路で triage run を終える（FakeAdapter の harness と記録済みの CoS の
//! 判断。LLM は呼ばない）。同じ場面を mode `rust`（script 無し）と mode `ts` で 1 回ずつ走らせる。
//! - mode `ts` の script（[`TracePass`]）は op の送り口越しに list と各 run の read を行い、dispatcher の
//!   pass の memo が返した要求と応答の組を、呼んだ順に記録する。これが trace の `ops` になる。
//! - trace の `output` は、その `ops` から組んだ読み出しに Rust の参照実装 `cos_digest_pass_decide` を
//!   当てたもの。dispatcher の記録（`script_compare_log`）が `applied_by = ts`・`outcome = match` で
//!   あることも確かめる（dispatcher の中の Rust の判定と TS の出力が一致した）。
//! - trace の `writes` は pass の間に実際に書かれた digest message と requeue（試験 DB の trigger の log）。
//!   [`verify_writes`] が `output` の commands と照合する。mode `rust` の書き込みとも一致を確かめる。
//!
//! 行は `control-ts/fixtures/cos_digest/pass_traces.jsonl` になり、byte 一致で比べる。
//! `CELERIS_UPDATE_FIXTURES=1` で書き直す。時計は注入した試験の時計、ULID は初出順の固定の id に置き換える。

use super::*;
use crate::script_policy::test_pass::op;
use crate::script_policy::{
    CosDigestOp, CosDigestOpReply, CosDigestPassCall, PolicyFuture, PolicyReply, PolicyScript,
    PolicyScriptBundle, PolicyScriptError,
};
use std::collections::{BTreeSet, HashMap};
use task_core::chat::digest::{
    CosDigestCommand, CosDigestInput, CosDigestListRunsReply, CosDigestListRunsRequest,
    CosDigestPassOutput, CosDigestPassRead, CosDigestPassRun, CosDigestReadRunRequest,
    cos_digest_decide, cos_digest_pass_decide,
};
use task_core::chat::{ChatPostMessageRequest, ChatRunState, ChatSendMode};
use task_core::script_compare::{
    CompareOutcome, CosDigestAppliedBy, LaneRulesInput, NewScriptGeneration, RoutePriorityInput,
    ScriptGenerationState, ScriptMode, ScriptModeChange,
};

const FAIL_SCRIPT: &str = "cat >/dev/null; echo boom >&2; exit 3";
const FIXTURES_ENV: &str = "CELERIS_UPDATE_FIXTURES";
const DIGEST_KEY: &str = task_core::chat::triage::COS_TRIAGE_DIGEST_KEY_PREFIX;
const BUNDLE_SHA: &str = "7a";
const HOST_API: &str = "1.1.0";

fn pass_traces_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../control-ts/fixtures/cos_digest/pass_traces.jsonl")
}

// ---------------------------------------------------------------- trace の op

/// trace の op 1 回（付記 S4-16 の形）。直列化の欄順を serde_json の feature（`preserve_order`）に
/// 依らず固定するため、`serde_json::Value` でなく型で持つ。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceOp {
    op: String,
    request: TraceRequest,
    reply: TraceReply,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum TraceRequest {
    ReadRun(CosDigestReadRunRequest),
    ListRuns(CosDigestListRunsRequest),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TraceReply {
    Ok { value: TraceValue },
    Error { error: TraceError },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum TraceValue {
    ReadRun(CosDigestInput),
    ListRuns(CosDigestListRunsReply),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceError {
    code: String,
    message: String,
}

impl TraceOp {
    fn new(which: CosDigestOp, request: &serde_json::Value, reply: &CosDigestOpReply) -> Self {
        Self {
            op: which.as_str().to_owned(),
            request: serde_json::from_value(request.clone()).expect("trace request"),
            reply: serde_json::from_value(reply.to_json()).expect("trace reply"),
        }
    }

    fn error_code(&self) -> Option<&str> {
        match &self.reply {
            TraceReply::Error { error } => Some(error.code.as_str()),
            TraceReply::Ok { .. } => None,
        }
    }
}

// ---------------------------------------------------------------- 記録する script

/// mode `ts` の偽の TS。TS の `cosDigestPass` と同じ手順（list → 各 run の read → `decide`。read の
/// `exception` は飛ばし、ほかの error は投げる）を op の送り口越しに行い、op の組を記録する。
#[derive(Default)]
struct TracePass {
    ops: std::sync::Mutex<Vec<TraceOp>>,
}

impl TracePass {
    fn take(&self) -> Vec<TraceOp> {
        std::mem::take(&mut *self.ops.lock().expect("lock"))
    }

    async fn call(
        &self,
        sink: &crate::script_policy::CosDigestOpSink,
        which: CosDigestOp,
        request: serde_json::Value,
    ) -> CosDigestOpReply {
        let reply = op(sink, which, request.clone()).await;
        self.ops
            .lock()
            .expect("lock")
            .push(TraceOp::new(which, &request, &reply));
        reply
    }
}

fn script_error(e: impl ToString) -> PolicyScriptError {
    PolicyScriptError::new("exception", e.to_string())
}

impl PolicyScript for TracePass {
    fn bundle(&self) -> PolicyScriptBundle {
        PolicyScriptBundle {
            bundle_sha256: BUNDLE_SHA.repeat(32),
            host_api: HOST_API.into(),
        }
    }

    fn lane_rules(&self, _input: LaneRulesInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "not this domain")) })
    }

    fn route_priority(&self, _input: RoutePriorityInput, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async { Err(PolicyScriptError::new("exception", "not this domain")) })
    }

    fn cos_digest_pass(&self, call: CosDigestPassCall, _deadline: Duration) -> PolicyFuture<'_> {
        Box::pin(async move {
            let list = match self
                .call(&call.sink, CosDigestOp::ListRuns, serde_json::json!({}))
                .await
            {
                CosDigestOpReply::Ok(v) => {
                    serde_json::from_value::<CosDigestListRunsReply>(v).map_err(script_error)?
                }
                CosDigestOpReply::Error { code, message } => {
                    return Err(script_error(format!("{code}: {message}")));
                }
            };
            let mut runs = Vec::new();
            for run in list.runs {
                let request = serde_json::to_value(CosDigestReadRunRequest {
                    thread: run.thread.clone(),
                    run_id: run.run_id.clone(),
                })
                .map_err(script_error)?;
                match self.call(&call.sink, CosDigestOp::ReadRun, request).await {
                    CosDigestOpReply::Ok(v) => {
                        let input: CosDigestInput =
                            serde_json::from_value(v).map_err(script_error)?;
                        runs.push(CosDigestPassRun {
                            run_id: input.run_id.clone(),
                            commands: cos_digest_decide(&input),
                        });
                    }
                    CosDigestOpReply::Error { code, .. } if code == "exception" => {}
                    CosDigestOpReply::Error { code, message } => {
                        return Err(script_error(format!("{code}: {message}")));
                    }
                }
            }
            Ok(PolicyReply {
                output: serde_json::to_value(CosDigestPassOutput { runs }).map_err(script_error)?,
                isolate_epoch: 1,
            })
        })
    }
}

// ---------------------------------------------------------------- 場面の器

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Rust,
    Ts,
}

/// 1 場面の試験 DB と、mode `ts` なら記録する script。
struct Scene {
    f: Fixture,
    script: Option<Arc<TracePass>>,
}

fn scene(mode: Mode, enabled: bool, harness: &str) -> Scene {
    let f = fixture_script(enabled, harness);
    // requeue（`running`→`pending`）の実行 log。
    rusqlite::Connection::open(&f.db_path)
        .expect("db")
        .execute_batch(
            "CREATE TABLE test_trace_requeue(item_id TEXT NOT NULL, run_id TEXT); \
             CREATE TRIGGER test_trace_requeue_t AFTER UPDATE OF state ON cos_inbox_items \
             WHEN OLD.state='running' AND NEW.state='pending' BEGIN \
             INSERT INTO test_trace_requeue VALUES(OLD.id, OLD.run_id); END;",
        )
        .expect("requeue log");
    let mut s = Scene { f, script: None };
    s.f.d.set_script_domain_store(s.f.store.clone());
    if mode == Mode::Ts {
        set_mode_ts(&s.f);
        let script = Arc::new(TracePass::default());
        s.f.d
            .set_policy_script(Some(script.clone() as Arc<dyn PolicyScript>));
        s.script = Some(script);
    }
    s
}

fn cos_digest_version(f: &Fixture) -> u64 {
    f.store
        .script_domain_modes()
        .expect("modes")
        .into_iter()
        .find(|r| r.domain.as_str() == "cos_digest")
        .expect("cos_digest row")
        .version
}

/// generation を記録し、`compare` を経て同じ generation の `ts` にする（人の actor）。
fn set_mode_ts(f: &Fixture) {
    let now = f.d.now_utc();
    let generation = f
        .store
        .script_generation_record(
            &NewScriptGeneration {
                bundle_sha256: BUNDLE_SHA.repeat(32),
                host_api: HOST_API.into(),
                schema_sha256: "0".repeat(64),
                manifest_json: r#"{"domains":["cos_digest"]}"#.into(),
                state: ScriptGenerationState::Loaded,
            },
            now,
        )
        .expect("generation");
    for to in [ScriptMode::Compare, ScriptMode::Ts] {
        f.store
            .script_domain_set_mode(
                &ScriptModeChange {
                    domain: "cos_digest".into(),
                    to_mode: to,
                    to_generation: Some(generation.id.clone()),
                    expected_version: cos_digest_version(f),
                    actor: "human:test".into(),
                    reason: "cos_digest pass trace".into(),
                },
                now,
            )
            .expect("mode");
    }
}

// ---------------------------------------------------------------- 書き込みと照合

/// pass の間に書かれた digest message 1 件。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct WrittenMessage {
    thread: String,
    idempotency_key: String,
    role: String,
    text: String,
    cards: Vec<task_core::chat::ChatCard>,
    run_id: Option<String>,
}

/// requeue の実行 log の 1 行。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Requeued {
    item_id: String,
    run_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Writes {
    messages: Vec<WrittenMessage>,
    requeued: Vec<Requeued>,
}

/// 書き込みの目印（pass の前に読み、後に差を取る）。
struct Mark {
    messages: BTreeSet<String>,
    requeues: i64,
}

fn mark(f: &Fixture) -> Mark {
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    let mut stmt = conn
        .prepare("SELECT id FROM chat_messages WHERE client_message_id LIKE ?1")
        .expect("prepare");
    let messages = stmt
        .query_map([format!("{DIGEST_KEY}%")], |r| r.get(0))
        .expect("ids")
        .collect::<Result<_, _>>()
        .expect("ids");
    let requeues = conn
        .query_row("SELECT COUNT(*) FROM test_trace_requeue", [], |r| r.get(0))
        .expect("count");
    Mark { messages, requeues }
}

/// `before` の後に書かれた digest message（書いた順）と requeue。
fn writes_since(f: &Fixture, before: &Mark) -> Writes {
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    let mut stmt = conn
        .prepare(
            "SELECT id,thread_id FROM chat_messages WHERE client_message_id LIKE ?1 ORDER BY rowid",
        )
        .expect("prepare");
    let new: Vec<(String, String)> = stmt
        .query_map([format!("{DIGEST_KEY}%")], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("messages")
        .collect::<Result<Vec<_>, _>>()
        .expect("messages")
        .into_iter()
        .filter(|(id, _)| !before.messages.contains(id))
        .collect();
    let messages = new
        .into_iter()
        .map(|(id, thread)| {
            let m = f
                .store
                .chat_message_list(
                    &thread,
                    &ChatMessageQuery {
                        limit: Some(200),
                        ..Default::default()
                    },
                )
                .expect("list")
                .items
                .into_iter()
                .find(|m| m.id == id)
                .expect("message");
            WrittenMessage {
                thread,
                idempotency_key: m.client_message_id.unwrap_or_default(),
                role: serde_json::to_value(m.role)
                    .expect("role")
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                text: m.text,
                cards: m.cards,
                run_id: m.run_id,
            }
        })
        .collect();
    let mut stmt = conn
        .prepare("SELECT item_id,run_id FROM test_trace_requeue WHERE rowid>?1 ORDER BY rowid")
        .expect("prepare");
    let requeued = stmt
        .query_map([before.requeues], |r| {
            Ok(Requeued {
                item_id: r.get(0)?,
                run_id: r.get(1)?,
            })
        })
        .expect("requeues")
        .collect::<Result<_, _>>()
        .expect("requeues");
    Writes { messages, requeued }
}

/// trace の `ops` から pass の読み出し（list の順）を組む。
fn pass_reads(ops: &[TraceOp]) -> Result<Vec<CosDigestPassRead>, String> {
    let list = ops
        .iter()
        .find_map(|o| match (&o.request, &o.reply) {
            (
                TraceRequest::ListRuns(_),
                TraceReply::Ok {
                    value: TraceValue::ListRuns(list),
                },
            ) => Some(list.clone()),
            _ => None,
        })
        .ok_or("no list_runs reply")?;
    Ok(list
        .runs
        .into_iter()
        .map(|run| {
            let input = ops.iter().find_map(|o| match (&o.request, &o.reply) {
                (
                    TraceRequest::ReadRun(r),
                    TraceReply::Ok {
                        value: TraceValue::ReadRun(input),
                    },
                ) if r.thread == run.thread && r.run_id == run.run_id => Some(input.clone()),
                _ => None,
            });
            CosDigestPassRead { run, input }
        })
        .collect())
}

/// `output` の commands と実際の書き込みを照合する（trace の 1 行だけで決まる）。
/// - `post_digest` 1 件ごとに、同じ thread・key・text・cards・run の assistant message がちょうど 1 件
/// - `requeue_run` のある run では、read で `running` だった item がちょうど requeue され、無い run では 0 件
/// - commands で説明できない message・requeue が無い
fn verify_writes(
    ops: &[TraceOp],
    output: &CosDigestPassOutput,
    writes: &Writes,
) -> Result<(), String> {
    let reads = pass_reads(ops)?;
    let mut messages_used = 0;
    let mut requeues_used = 0;
    for run in &output.runs {
        let input = reads
            .iter()
            .find(|r| r.run.run_id == run.run_id)
            .and_then(|r| r.input.as_ref())
            .ok_or_else(|| format!("run {} has no read", run.run_id))?;
        let mut requeue = false;
        for command in &run.commands {
            match command {
                CosDigestCommand::PostDigest {
                    thread,
                    idempotency_key,
                    text,
                    cards,
                    run_id,
                } => {
                    let same: Vec<_> = writes
                        .messages
                        .iter()
                        .filter(|m| &m.idempotency_key == idempotency_key && &m.thread == thread)
                        .collect();
                    let [m] = same.as_slice() else {
                        return Err(format!("{} messages for {idempotency_key}", same.len()));
                    };
                    messages_used += 1;
                    if m.role != "assistant" {
                        return Err(format!("role {}", m.role));
                    }
                    if &m.text != text {
                        return Err(format!("text differs: {:?} != {text:?}", m.text));
                    }
                    if &m.cards != cards {
                        return Err(format!("cards differ: {:?} != {cards:?}", m.cards));
                    }
                    if m.run_id.as_deref() != Some(run_id.as_str()) {
                        return Err(format!("run id differs: {:?}", m.run_id));
                    }
                }
                CosDigestCommand::RequeueRun { .. } => requeue = true,
            }
        }
        let mut written: Vec<&str> = writes
            .requeued
            .iter()
            .filter(|r| r.run_id.as_deref() == Some(run.run_id.as_str()))
            .map(|r| r.item_id.as_str())
            .collect();
        requeues_used += written.len();
        let mut expected: Vec<&str> = if requeue {
            input
                .items
                .iter()
                .filter(|i| i.state == "running")
                .map(|i| i.item_id.as_str())
                .collect()
        } else {
            Vec::new()
        };
        written.sort_unstable();
        expected.sort_unstable();
        if written != expected {
            return Err(format!(
                "run {}: requeued {written:?}, requeue command: {requeue}, running items {expected:?}",
                run.run_id
            ));
        }
    }
    if messages_used != writes.messages.len() {
        return Err(format!(
            "{} digest messages without a command",
            writes.messages.len() - messages_used
        ));
    }
    if requeues_used != writes.requeued.len() {
        return Err(format!(
            "{} requeues without a command",
            writes.requeued.len() - requeues_used
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- 正規化

/// ULID を初出順の固定の id に、一時 data dir を固定の path に置き換える（id の長さは変えない）。
fn trace_normalize(
    text: &str,
    dir: &str,
    scenario: usize,
    ids: &mut HashMap<String, String>,
) -> String {
    const CROCKFORD: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let text = text.replace(dir, "/celeris-trace-data");
    let chars: Vec<char> = text.chars().collect();
    let is_id = |c: char| CROCKFORD.contains(c);
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let end = chars[i..]
            .iter()
            .position(|c| !is_id(*c))
            .map_or(chars.len(), |n| i + n);
        if end == i {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let token: String = chars[i..end].iter().collect();
        let boundary = i == 0 || !chars[i - 1].is_ascii_alphanumeric();
        if token.len() == 26 && boundary {
            let next = ids.len() + 1;
            out.push_str(
                ids.entry(token)
                    .or_insert_with(|| format!("01TRACE{scenario:02}{next:017}")),
            );
        } else {
            out.push_str(&token);
        }
        i = end;
    }
    out
}

/// 質問の `source_revision` は task の event の壁時計の時刻（試験の時計は task store に届かない）。
/// digest は読まないので、read の応答の中で item の固定の時刻に置き換える。
fn fix_revisions(ops: &mut [TraceOp]) {
    for o in ops.iter_mut() {
        if let TraceReply::Ok {
            value: TraceValue::ReadRun(input),
        } = &mut o.reply
        {
            for item in &mut input.items {
                if item.source_revision.contains('T') {
                    item.source_revision = item.created_at.clone();
                }
            }
        }
    }
}

#[derive(serde::Serialize)]
struct PassTraceLine<'a> {
    name: &'a str,
    ops: &'a [TraceOp],
    writes: &'a Writes,
    output: &'a CosDigestPassOutput,
}

fn trace_line(
    name: &str,
    ops: &[TraceOp],
    writes: &Writes,
    output: &CosDigestPassOutput,
) -> String {
    serde_json::to_string(&PassTraceLine {
        name,
        ops,
        writes,
        output,
    })
    .expect("trace line")
}

// ---------------------------------------------------------------- 記録

/// 1 pass の結果。`line` は mode `ts` だけ。`writes` は場面の中で正規化した書き込み（mode の比較用）。
#[derive(Debug)]
struct PassCapture {
    line: Option<String>,
    writes: String,
}

#[derive(Default)]
struct Recorder {
    scenarios: usize,
    /// 場面の名前と pass ごとの結果。
    passes: Vec<(String, Vec<PassCapture>)>,
}

impl Recorder {
    /// 場面を終える: run を待ち、digest が空になるまで pass を回して pass ごとに記録・照合する。
    /// 起動は止めておく（requeue した item をすぐ取り直さない）。
    async fn record(&mut self, s: &mut Scene, name: &str) {
        self.record_with(s, name, |_, _| {}).await;
    }

    /// [`Self::record`] と同じ。各 pass の前に `before_pass(fixture, pass の番号（0 始まり）)` を呼ぶ
    /// （pass の間に DB を壊す・戻す場面）。
    async fn record_with(
        &mut self,
        s: &mut Scene,
        name: &str,
        mut before_pass: impl FnMut(&Fixture, usize),
    ) {
        trace_finish_runs(&mut s.f).await;
        self.scenarios += 1;
        let scenario = self.scenarios;
        s.f.d.accepting_new_work = false;
        let dir = s.f.dir.path().display().to_string();
        let mut ids = HashMap::new();
        let mut write_ids = HashMap::new();
        let mut captures = Vec::new();
        loop {
            if let Some(script) = &s.script {
                script.take();
            }
            before_pass(&s.f, captures.len());
            let pending =
                !s.f.store
                    .cos_triage_runs_without_digest(1)
                    .expect("scan")
                    .is_empty();
            if !pending {
                break;
            }
            assert!(captures.len() < 5, "{name}: the digest does not drain");
            s.f.d
                .cos_chat_launch
                .as_mut()
                .expect("launch")
                .triage
                .digest_due = true;
            let before = mark(&s.f);
            let records_before = s.f.store.script_compare_log(None).expect("log").len();
            s.f.d.ticks += 1;
            s.f.tick();
            let writes = writes_since(&s.f, &before);
            let writes_norm = trace_normalize(
                &serde_json::to_string(&writes).expect("json"),
                &dir,
                scenario,
                &mut write_ids,
            );
            let line = match &s.script {
                None => None,
                Some(script) => {
                    let mut ops = script.take();
                    assert!(!ops.is_empty(), "{name}: the TS pass was not called");
                    let raw = cos_digest_pass_decide(&pass_reads(&ops).expect("reads"));
                    verify_writes(&ops, &raw, &writes).unwrap_or_else(|e| panic!("{name}: {e}"));
                    // The dispatcher applied TS's output after its own Rust decision matched it.
                    let records = s.f.store.script_compare_log(None).expect("log");
                    let new = &records[records_before..];
                    assert_eq!(new.len(), raw.runs.len(), "{name}: one record per run");
                    for r in new {
                        let r = &r.record;
                        assert_eq!(
                            (r.applied_by, r.outcome, r.fallback.as_ref()),
                            (CosDigestAppliedBy::Ts, CompareOutcome::Match, None),
                            "{name}: {r:?}"
                        );
                    }
                    fix_revisions(&mut ops);
                    let output = cos_digest_pass_decide(&pass_reads(&ops).expect("reads"));
                    assert_eq!(
                        output, raw,
                        "{name}: the digest does not read source_revision"
                    );
                    let line_name = format!("{name}/pass-{}", captures.len() + 1);
                    let raw_line = trace_line(&line_name, &ops, &writes, &output);
                    let line = trace_normalize(&raw_line, &dir, scenario, &mut ids);
                    // The normalized line must replay: it is what the TS side is compared with.
                    let value: serde_json::Value = serde_json::from_str(&line).expect("json");
                    let ops: Vec<TraceOp> =
                        serde_json::from_value(value["ops"].clone()).expect("ops");
                    let writes: Writes =
                        serde_json::from_value(value["writes"].clone()).expect("writes");
                    let output: CosDigestPassOutput =
                        serde_json::from_value(value["output"].clone()).expect("output");
                    assert_eq!(
                        cos_digest_pass_decide(&pass_reads(&ops).expect("reads")),
                        output,
                        "{line_name}"
                    );
                    verify_writes(&ops, &output, &writes)
                        .unwrap_or_else(|e| panic!("{line_name}: {e}"));
                    assert_eq!(trace_line(&line_name, &ops, &writes, &output), line);
                    Some(line)
                }
            };
            captures.push(PassCapture {
                line,
                writes: writes_norm,
            });
        }
        assert!(!captures.is_empty(), "{name}: nothing to digest");
        // Idempotent: another pass writes nothing more.
        let before = mark(&s.f);
        s.f.d
            .cos_chat_launch
            .as_mut()
            .expect("launch")
            .triage
            .digest_due = true;
        s.f.d.ticks += 1;
        s.f.tick();
        assert_eq!(
            writes_since(&s.f, &before),
            Writes::default(),
            "{name}: digest repeated"
        );
        self.passes.push((name.to_owned(), captures));
    }
}

/// 生きている CoS の run をすべて待つ（worker の task は await している間だけ進む）。
async fn trace_finish_runs(f: &mut Fixture) {
    let launch = f.d.cos_chat_launch.as_mut().expect("launch");
    let handles: Vec<_> = launch.running.drain().map(|(_, handle)| handle).collect();
    for handle in handles {
        tokio::time::timeout(std::time::Duration::from_secs(30), handle)
            .await
            .expect("run deadline")
            .expect("worker did not panic");
    }
}

/// 待ちを 1 tick ずつ離して入れ（`created_at` が report の順を決める）、全部を 1 つの run で始める。
/// report の順の item id を返す。
fn trace_start_run(f: &mut Fixture, waits: &[&str]) -> Vec<String> {
    f.tick();
    f.d.accepting_new_work = false;
    for wait in waits {
        match wait.strip_prefix("notice:") {
            Some(key) => {
                f.notice(key, NoticeKind::Report);
            }
            None => {
                f.question(wait, vec![]);
            }
        }
        f.tick();
    }
    f.d.accepting_new_work = true;
    f.tick();
    assert_eq!(f.inbox_runs(), 1);
    trace_items(f)
}

fn trace_items(f: &Fixture) -> Vec<String> {
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    let mut stmt = conn
        .prepare("SELECT id FROM cos_inbox_items ORDER BY created_at,id")
        .expect("prepare");
    stmt.query_map([], |r| r.get(0))
        .expect("items")
        .collect::<Result<_, _>>()
        .expect("items")
}

fn trace_latest_run(f: &Fixture) -> (String, String) {
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    conn.query_row(
        "SELECT thread_id,run_id FROM chat_runs ORDER BY started_at DESC,run_id DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .expect("run")
}

/// CoS の `escalate` の記録済みの結果: outbox の packet、次に item の状態。
fn trace_escalate(f: &Fixture, item: &str, reason: &str, packet: serde_json::Value) {
    let now = f.d.now_utc();
    f.store
        .cos_triage_outbox_claim(item, "escalation", &packet.to_string(), now)
        .expect("claim")
        .expect("routed");
    assert!(
        f.store
            .cos_triage_resolve(item, "escalated", Some("op-escalate"), Some(reason), now)
            .expect("resolve")
    );
}

fn trace_packet() -> serde_json::Value {
    serde_json::json!({
        "summary": "公開してよいか",
        "background": "依頼「資料の公開」の task が公開先を決められず止まった",
        "on_answer": "選んだ方で task が再開する",
        "options": [{"key":"publish","label":"公開する"},{"key":"hold","label":"保留"}],
        "recommended": "hold",
        "recommendation_reason": "公開先が未確認",
        "web_path": "/tasks/recorded",
    })
}

fn trace_resolve(f: &Fixture, item: &str, outcome: &str, op: Option<&str>, reason: Option<&str>) {
    assert!(
        f.store
            .cos_triage_resolve(item, outcome, op, reason, f.d.now_utc())
            .expect("resolve")
    );
}

fn trace_stop(f: &Fixture) {
    let (thread, run) = trace_latest_run(f);
    let stop = f
        .store
        .chat_run_stop(&thread, &run, f.d.now_utc())
        .expect("stop");
    assert!(stop.accepted);
}

/// 人の発言が run に割り込む。
fn trace_interrupt(f: &Fixture) {
    let inbox = f.inbox_thread().expect("inbox thread");
    f.store
        .chat_message_post(
            &inbox,
            &ChatPostMessageRequest {
                client_message_id: "h-interrupt".into(),
                text: "(b) にして".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Interrupt,
                resume_queue: false,
            },
            f.d.now_utc(),
        )
        .expect("post");
}

/// store で直接 run `id` に知らせ 1 件を渡して終える（20 件を超える drain 用。harness は起こさない）。
fn trace_direct_run(f: &Fixture, id: &str, state: ChatRunState) {
    let now = f.d.now_utc();
    f.store
        .cos_triage_ingest_batch(
            "digest-pass-trace",
            id,
            &[task_core::chat::triage::CosTriageSource {
                source_kind: "notice".into(),
                source_key: id.into(),
                source_revision: "1".into(),
                source_event_id: None,
                operation_id: None,
                summary: format!("知らせ {id}"),
                detail: String::new(),
                policy_version: "1".into(),
                task_id: None,
            }],
            now,
        )
        .expect("ingest");
    let general = f
        .store
        .cos_thread_open(
            &task_core::chat::ChatOpenThreadRequest { project_id: None },
            now,
        )
        .expect("general")
        .thread
        .id;
    f.store
        .cos_triage_claim(&general, id, now)
        .expect("claim")
        .expect("run");
    f.store
        .chat_run_finish(id, state, None, Some("人が止めた"), now)
        .expect("finish");
}

fn sql(f: &Fixture, sql: &str) {
    rusqlite::Connection::open(&f.db_path)
        .expect("db")
        .execute_batch(sql)
        .expect("sql");
}

// ---------------------------------------------------------------- 場面

/// 全場面を file の順に。場面ごとに試験 DB を作る。tick を数えるので時刻は固定。
async fn trace_scenarios(mode: Mode) -> Vec<(String, Vec<PassCapture>)> {
    let mut out = Recorder::default();

    // completed: CoS escalated with a decision packet.
    let mut s = scene(mode, true, DONE);
    let items = trace_start_run(&mut s.f, &["deploy"]);
    trace_escalate(&s.f, &items[0], "外部公開なので人の判断", trace_packet());
    out.record(&mut s, "completed/escalated-packet").await;

    // completed: the packet carries CoS's text for each option.
    let mut s = scene(mode, true, DONE);
    let items = trace_start_run(&mut s.f, &["deploy"]);
    trace_escalate(
        &s.f,
        &items[0],
        "どちらも戻せない",
        serde_json::json!({
            "summary": "どちらの host に入れるか",
            "options": [
                {"key":"a","label":"host A","detail":"既存の job が止まる"},
                {"key":"b","label":"host B"},
            ],
            "recommended": "a",
            "web_path": "/tasks/recorded",
        }),
    );
    out.record(&mut s, "completed/escalated-option-details")
        .await;

    // completed: CoS did not touch the item; the fallback of the same tick takes it.
    let mut s = scene(mode, true, DONE);
    trace_start_run(&mut s.f, &["deploy"]);
    out.record(&mut s, "completed/running-unhandled").await;

    // completed: two handled items, a third left running.
    let mut s = scene(mode, true, DONE);
    let items = trace_start_run(&mut s.f, &["deploy", "notice:report:1", "publish"]);
    trace_resolve(
        &s.f,
        &items[0],
        "answered",
        Some("op-answer"),
        Some("方針どおり"),
    );
    trace_resolve(&s.f, &items[1], "observed", None, Some("知らせだけ"));
    out.record(&mut s, "completed/answered-observed-running")
        .await;

    // completed: the unavailable fallback routed the item while the run was still live.
    let mut s = scene(mode, true, DONE);
    trace_start_run(&mut s.f, &["deploy"]);
    s.f.advance(130);
    s.f.tick();
    assert_eq!(
        s.f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='fallback'"),
        1
    );
    out.record(&mut s, "completed/fallback-while-live").await;

    // completed: the wait closed while the run was live.
    let mut s = scene(mode, true, DONE);
    s.f.tick();
    let task = s.f.question("deploy", vec![]);
    s.f.tick();
    assert_eq!(s.f.inbox_runs(), 1);
    s.f.store
        .apply_transition_with_events(
            task,
            Trigger::Answer,
            vec![Event::Answered {
                by: None,
                question: "deploy?".into(),
                answer: "A".into(),
            }],
        )
        .expect("answer");
    s.f.tick();
    assert_eq!(
        s.f.count("SELECT COUNT(*) FROM cos_inbox_items WHERE state='resolved'"),
        1
    );
    out.record(&mut s, "completed/resolved-source-closed").await;

    // completed: a stored state the digest does not know (`handled`, as in the cases fixture).
    // The table's CHECK does not list it, so this one row bypasses the CHECK on purpose.
    let mut s = scene(mode, true, DONE);
    let items = trace_start_run(&mut s.f, &["deploy"]);
    trace_finish_runs(&mut s.f).await;
    let conn = rusqlite::Connection::open(&s.f.db_path).expect("db");
    conn.pragma_update(None, "ignore_check_constraints", true)
        .expect("pragma");
    conn.execute(
        "UPDATE cos_inbox_items SET state='handled',reason='CoS が処理した' WHERE id=?1",
        [&items[0]],
    )
    .expect("handled");
    drop(conn);
    out.record(&mut s, "completed/handled-unknown-state").await;

    // failed: the harness exits non-zero with the item unhandled.
    let mut s = scene(mode, true, FAIL_SCRIPT);
    trace_start_run(&mut s.f, &["deploy"]);
    out.record(&mut s, "failed/running-unhandled").await;

    // failed: CoS escalated one item, then the run failed.
    let mut s = scene(mode, true, FAIL_SCRIPT);
    let items = trace_start_run(&mut s.f, &["deploy", "publish"]);
    trace_escalate(&s.f, &items[0], "人の判断", trace_packet());
    out.record(&mut s, "failed/escalated-and-running").await;

    // failed: the unavailable fallback routed the item while the run was still live.
    let mut s = scene(mode, true, FAIL_SCRIPT);
    trace_start_run(&mut s.f, &["deploy"]);
    s.f.advance(130);
    s.f.tick();
    out.record(&mut s, "failed/fallback-while-live").await;

    // stopped: a person stopped the run; the item goes back to the queue.
    let mut s = scene(mode, true, DONE);
    trace_start_run(&mut s.f, &["deploy"]);
    trace_stop(&s.f);
    out.record(&mut s, "stopped/running-requeued").await;

    // stopped: only the item still running goes back.
    let mut s = scene(mode, true, DONE);
    let items = trace_start_run(&mut s.f, &["deploy", "publish", "notice:report:1"]);
    trace_escalate(&s.f, &items[0], "人の判断", trace_packet());
    trace_resolve(&s.f, &items[2], "observed", None, Some("知らせだけ"));
    trace_stop(&s.f);
    out.record(&mut s, "stopped/escalated-observed-running")
        .await;

    // stopped: two items still running are both requeued by one command.
    let mut s = scene(mode, true, DONE);
    trace_start_run(&mut s.f, &["deploy", "publish"]);
    trace_stop(&s.f);
    out.record(&mut s, "stopped/two-running-requeued").await;

    // interrupted: a human message interrupted the run before it polled.
    let mut s = scene(mode, true, DONE);
    trace_start_run(&mut s.f, &["deploy"]);
    trace_interrupt(&s.f);
    out.record(&mut s, "interrupted/running-requeued").await;

    // interrupted: handled items stay, the running one goes back.
    let mut s = scene(mode, true, DONE);
    let items = trace_start_run(&mut s.f, &["deploy", "publish"]);
    trace_resolve(
        &s.f,
        &items[0],
        "answered",
        Some("op-answer"),
        Some("方針どおり"),
    );
    trace_interrupt(&s.f);
    out.record(&mut s, "interrupted/answered-running").await;

    // interrupted: an escalated item with the fallback of a second one.
    let mut s = scene(mode, true, DONE);
    let items = trace_start_run(&mut s.f, &["deploy", "publish"]);
    trace_escalate(&s.f, &items[0], "人の判断", trace_packet());
    s.f.advance(130);
    s.f.tick();
    trace_interrupt(&s.f);
    out.record(&mut s, "interrupted/escalated-fallback").await;

    // Two threads in one pass: a run of the general CoS and a run of a project CoS.
    let mut s = scene(mode, true, DONE);
    s.f.d.config.execution.max_cos_runs = 2;
    s.f.tick();
    s.f.question("general wait", vec![]);
    s.f.tick();
    crate::test_support::project_cos_thread(&s.f.store, "案件A");
    let project: task_core::ProjectId =
        s.f.count_text("SELECT project_id FROM chat_threads WHERE kind='project'")
            .parse()
            .expect("project id");
    s.f.question_in("project wait", Some(project), None, vec![]);
    s.f.tick();
    assert_eq!(s.f.count("SELECT COUNT(*) FROM chat_runs"), 2);
    out.record(&mut s, "completed/two-threads").await;

    // The chat run cannot be read (its usage is not a usage object): the digest has no reason (`null`).
    let mut s = scene(mode, true, DONE);
    trace_start_run(&mut s.f, &["deploy"]);
    trace_stop(&s.f);
    trace_finish_runs(&mut s.f).await;
    let (_, run) = trace_latest_run(&s.f);
    sql(
        &s.f,
        &format!("UPDATE chat_runs SET usage_json='\"not usage\"' WHERE run_id='{run}'"),
    );
    out.record(&mut s, "stopped/run-unreadable-reason-null")
        .await;

    // More than 20 finished runs: the first pass takes 20 (the bound), the next pass the rest.
    // Each fourth run is stopped or interrupted, so the drain requeues as well.
    let mut s = scene(mode, false, "true");
    let states = [
        ChatRunState::Completed,
        ChatRunState::Stopped,
        ChatRunState::Failed,
        ChatRunState::Interrupted,
    ];
    for i in 0..23 {
        trace_direct_run(&s.f, &format!("drain-run-{i:02}"), states[i % 4]);
    }
    out.record(&mut s, "drain/23-runs").await;

    // `read_run` fails for one run of the pass (an item's summary is a blob, so its report cannot be
    // read: op error `exception`). The pass digests the other run and skips that one; it is
    // digested in the next pass after the summary is repaired.
    let mut s = scene(mode, false, "true");
    trace_direct_run(&s.f, "read-ok", ChatRunState::Completed);
    trace_direct_run(&s.f, "read-fail", ChatRunState::Stopped);
    out.record_with(
        &mut s,
        "read-failure/skip-then-digest",
        |f, pass| match pass {
            0 => sql(
                f,
                "UPDATE cos_inbox_items SET summary=X'FF' WHERE run_id='read-fail';",
            ),
            1 => sql(
                f,
                "UPDATE cos_inbox_items SET summary='知らせ read-fail' WHERE run_id='read-fail';",
            ),
            _ => {}
        },
    )
    .await;

    out.passes
}

/// `read_run` が失敗する pass（report の表が読めない）。TS も Rust も その run を飛ばし、何も書かない。
/// list は読めるので、表を戻した次の pass で digest される。
async fn trace_read_failure(mode: Mode) -> (Vec<TraceOp>, Writes, Writes) {
    let mut s = scene(mode, true, DONE);
    trace_start_run(&mut s.f, &["deploy"]);
    trace_stop(&s.f);
    trace_finish_runs(&mut s.f).await;
    s.f.d.accepting_new_work = false;
    if let Some(script) = &s.script {
        script.take();
    }
    sql(
        &s.f,
        "ALTER TABLE cos_notification_routes RENAME TO test_routes_away;",
    );
    let before = mark(&s.f);
    s.f.d.ticks += 1;
    s.f.tick();
    let failed = writes_since(&s.f, &before);
    let ops = s.script.as_ref().map(|t| t.take()).unwrap_or_default();
    sql(
        &s.f,
        "ALTER TABLE test_routes_away RENAME TO cos_notification_routes;",
    );
    let before = mark(&s.f);
    s.f.d.ticks += 1;
    s.f.tick();
    (ops, failed, writes_since(&s.f, &before))
}

// ---------------------------------------------------------------- 試験

#[tokio::test]
async fn cos_digest_pass_trace_fixture_is_current() {
    let rust = trace_scenarios(Mode::Rust).await;
    let ts = trace_scenarios(Mode::Ts).await;
    assert_eq!(rust.len(), ts.len());
    let mut lines = Vec::new();
    for ((name, rust), (ts_name, ts)) in rust.iter().zip(&ts) {
        assert_eq!(name, ts_name);
        assert_eq!(rust.len(), ts.len(), "{name}: passes");
        for (r, t) in rust.iter().zip(ts) {
            assert!(r.line.is_none());
            assert_eq!(
                r.writes, t.writes,
                "{name}: mode ts wrote what mode rust wrote"
            );
            lines.push(t.line.clone().expect("ts line"));
        }
    }

    let values: Vec<serde_json::Value> = lines
        .iter()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    let names: BTreeSet<&str> = values
        .iter()
        .map(|v| v["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names.len(), lines.len(), "names are unique");
    let reads: Vec<&serde_json::Value> = values
        .iter()
        .flat_map(|v| v["ops"].as_array().expect("ops"))
        .filter(|o| o["op"] == "cos_digest:read_run" && o["reply"]["kind"] == "ok")
        .map(|o| &o["reply"]["value"])
        .collect();
    let run_states: BTreeSet<&str> = reads
        .iter()
        .map(|i| i["run_state"].as_str().expect("run state"))
        .collect();
    for state in ["completed", "failed", "stopped", "interrupted"] {
        assert!(
            run_states.contains(state),
            "run state {state}: {run_states:?}"
        );
    }
    let item_states: BTreeSet<&str> = reads
        .iter()
        .flat_map(|i| i["items"].as_array().expect("items"))
        .map(|i| i["state"].as_str().expect("state"))
        .collect();
    for state in [
        "escalated",
        "fallback",
        "running",
        "handled",
        "answered",
        "observed",
        "resolved",
    ] {
        assert!(
            item_states.contains(state),
            "item state {state}: {item_states:?}"
        );
    }
    // A pass over 20 runs (the bound) and the rest in the next pass.
    let listed: Vec<usize> = values
        .iter()
        .map(|v| {
            v["ops"][0]["reply"]["value"]["runs"]
                .as_array()
                .expect("runs")
                .len()
        })
        .collect();
    assert!(listed.contains(&20) && listed.contains(&3), "{listed:?}");
    // Requeues were written, and a run without a reason (`chat_run_get` failed).
    assert!(
        values
            .iter()
            .any(|v| !v["writes"]["requeued"].as_array().expect("r").is_empty())
    );
    assert!(
        reads
            .iter()
            .any(|i| i["run_state"] == "stopped" && i["reason"].is_null())
    );
    // A pass where `read_run` failed (`exception`) and the run was skipped, while another run of
    // the same pass was digested.
    assert!(
        values.iter().any(|v| {
            let ops = v["ops"].as_array().expect("ops");
            let failed: Vec<&str> = ops
                .iter()
                .filter(|o| o["op"] == "cos_digest:read_run" && o["reply"]["kind"] == "error")
                .map(|o| {
                    assert_eq!(o["reply"]["error"]["code"], "exception", "{o}");
                    o["request"]["run_id"].as_str().expect("run id")
                })
                .collect();
            let digested: Vec<&str> = v["output"]["runs"]
                .as_array()
                .expect("runs")
                .iter()
                .map(|r| r["run_id"].as_str().expect("run id"))
                .collect();
            !failed.is_empty()
                && !digested.is_empty()
                && failed.iter().all(|id| !digested.contains(id))
        }),
        "no pass skips a run whose read_run failed"
    );

    let generated = lines.join("\n") + "\n";
    let path = pass_traces_path();
    if std::env::var(FIXTURES_ENV).as_deref() == Ok("1") {
        std::fs::write(&path, &generated).expect("write pass traces");
    }
    let committed = std::fs::read_to_string(&path).expect("read pass traces");
    assert!(
        committed == generated,
        "pass_traces.jsonl differs from the generated traces; rewrite with {FIXTURES_ENV}=1\n\
         first difference at line {:?}",
        committed
            .lines()
            .zip(generated.lines())
            .position(|(a, b)| a != b)
    );
}

/// `read_run` の失敗: TS の op は `exception` を受けて run を飛ばし、どちらの mode も何も書かない。
#[tokio::test]
async fn cos_digest_pass_trace_read_failure_skips_the_run_in_both_modes() {
    let (_, rust_failed, rust_next) = trace_read_failure(Mode::Rust).await;
    let (ops, ts_failed, ts_next) = trace_read_failure(Mode::Ts).await;
    assert_eq!(rust_failed, Writes::default());
    assert_eq!(ts_failed, Writes::default());
    let kinds: Vec<(&str, &str)> = ops
        .iter()
        .map(|o| (o.op.as_str(), o.error_code().unwrap_or("ok")))
        .collect();
    assert_eq!(
        kinds,
        [
            ("cos_digest:list_runs", "ok"),
            ("cos_digest:read_run", "exception")
        ]
    );
    let reads = pass_reads(&ops).expect("reads");
    assert!(reads[0].input.is_none());
    assert_eq!(
        cos_digest_pass_decide(&reads),
        CosDigestPassOutput { runs: vec![] }
    );
    // The next pass digests the run (one message, one requeue) in both modes.
    for next in [&rust_next, &ts_next] {
        assert_eq!(
            (next.messages.len(), next.requeued.len()),
            (1, 1),
            "{next:?}"
        );
    }
}

/// 照合（[`verify_writes`]）は commands と書き込みの食い違いを見つける。
#[tokio::test]
async fn cos_digest_pass_trace_verify_detects_mismatch() {
    let mut out = Recorder::default();
    let mut s = scene(Mode::Ts, true, DONE);
    trace_start_run(&mut s.f, &["deploy"]);
    trace_stop(&s.f);
    out.record(&mut s, "stopped").await;
    let line = out.passes[0].1[0].line.clone().expect("line");
    let value: serde_json::Value = serde_json::from_str(&line).expect("json");
    let ops: Vec<TraceOp> = serde_json::from_value(value["ops"].clone()).expect("ops");
    let writes: Writes = serde_json::from_value(value["writes"].clone()).expect("writes");
    let output: CosDigestPassOutput =
        serde_json::from_value(value["output"].clone()).expect("output");
    verify_writes(&ops, &output, &writes).expect("the recorded pass matches");
    assert_eq!(writes.requeued.len(), 1);

    let err = |output: &CosDigestPassOutput, writes: &Writes| {
        verify_writes(&ops, output, writes).expect_err("mismatch")
    };
    // A different text.
    let mut o = output.clone();
    if let CosDigestCommand::PostDigest { text, .. } = &mut o.runs[0].commands[0] {
        text.push('x');
    }
    assert!(err(&o, &writes).contains("text differs"));
    // Different cards.
    let mut o = output.clone();
    if let CosDigestCommand::PostDigest { cards, .. } = &mut o.runs[0].commands[0] {
        cards.clear();
    }
    assert!(err(&o, &writes).contains("cards differ"));
    // Items were requeued although no command said so.
    let mut o = output.clone();
    o.runs[0].commands.truncate(1);
    assert!(err(&o, &writes).contains("requeue command"));
    // A requeue command without the write.
    let mut w = writes.clone();
    w.requeued.clear();
    assert!(err(&output, &w).contains("requeue command"));
    // A second message with the same key, and a message no command explains.
    let mut w = writes.clone();
    w.messages.push(w.messages[0].clone());
    assert!(err(&output, &w).contains("2 messages"));
    let mut w = writes.clone();
    let mut extra = w.messages[0].clone();
    extra.idempotency_key = format!("{DIGEST_KEY}other");
    w.messages.push(extra);
    assert!(err(&output, &w).contains("without a command"));
    // The run is not in the list (no read).
    let mut o = output.clone();
    o.runs[0].run_id = "run-outside".into();
    assert!(err(&o, &writes).contains("has no read"));
}

#[test]
fn cos_digest_pass_trace_normalize_fixes_ids_in_first_seen_order() {
    let a = "01M6EVP9GT07K2G01EMW0E6HNC";
    let b = "01M6EYH4MAHQPXZZ221BVBTG2E";
    let mut ids = HashMap::new();
    let text = format!("/data/x {b}/{a} question-{b} X{a} {b}");
    let out = trace_normalize(&text, "/data/x", 7, &mut ids);
    let first = "01TRACE0700000000000000001";
    let second = "01TRACE0700000000000000002";
    assert_eq!(first.len(), 26);
    assert_eq!(
        out,
        format!("/celeris-trace-data {first}/{second} question-{first} X{a} {first}")
    );
}
