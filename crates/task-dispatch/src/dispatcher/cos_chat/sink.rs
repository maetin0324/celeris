//! ADR 2026-10-05 D2: CoS chat run の worker の進行を `chat_events` に写す `ChatRunSink`。
//!
//! 写像（判断も LLM も無い。要約は worker が作ったものだけを使う）:
//! - `text` → `chat_run_append_text`（offset は追記前の UTF-8 byte 長。store が数える）
//! - `tool_use` / `tool_result` → `chat_run_tool`。progress に call id が無いので、sink が
//!   `call-<n>` を振り、`tool_result` は同じ道具名の最も古い未完の call（無ければ最も古い call）を閉じる。
//!   summary / detail は既存の redact（`browser_live` の秘密判定を行ごと）と既知の秘密の置換を通し、
//!   detail は store が 4 KiB で切る（status の要約は既知の秘密の置換だけ）
//! - `thinking` → `status`（phase=thinking）。worker の公開要約（`summary`）だけを使い、本文（`detail`）は流さない
//! - `status`・構造の無い `progress` → `status`（phase=working）
//! - `heartbeat` → 何もしない（events を増やさない）
//!
//! session の確立・resume 拒否・context の圧縮は `ChatSessionSignals` に残し、呼び出し側（launch /
//! rollover）が `signals()` で読む。終端は `chat_finish_for` で `RunOutcome` を写し、`finish` が
//! 未完の tool を failed にし、result の `actions` があれば実行せずに notice の card を出してから
//! `chat_run_finish` を呼ぶ。終端の後は（store の上で他者が終端にした場合も）本文差分を送らない。

// launch 葉が dispatcher の tick に配線するまでは試験からだけ使う。
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use task_core::browser_live::{LIVE_CONSOLE_MAX_CHARS, LiveEvent, PersistedLiveEvent, REDACTED};
use task_core::chat::{
    ChatActor, ChatCard, ChatCardKind, ChatError, ChatRun, ChatRunState, ChatStatusPhase,
    ChatToolData, ChatToolState,
};
use task_core::{ArtifactRef, BudgetKind, ProgressFields, ProgressKind, SqliteStore, Usage};
use task_worker::adapter::{AdapterError, EventSink, RunOutcome, Terminal};
use task_worker::result_report::ParsedActions;
use time::OffsetDateTime;

/// 注入する時計（試験は固定の時刻を渡す）。
pub(crate) type ChatClock = Arc<dyn Fn() -> OffsetDateTime + Send + Sync>;

/// run の途中で adapter が報告した session の出来事（呼び出し側が読む）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChatSessionSignals {
    /// `session_established` の id（最後の 1 件）。
    pub established: Option<String>,
    /// `session_resume_failed` の理由。
    pub resume_failed: Option<String>,
    /// adapter が観測した context の圧縮の回数。
    pub compacted: u32,
    /// context の上限で終わった（`BudgetExhausted{kind: Context}`）。
    pub context_exhausted: bool,
}

/// 終端の原因のうち、`RunOutcome` からは分からない呼び出し側の意図。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatStopIntent {
    /// 人の stop も割り込みも無い。
    None,
    /// 人の stop（`chat_run_stop` で stopping になった run）→ stopped。
    Stop,
    /// 割り込み・再起動回収などで止めた run → interrupted。
    Interrupt,
}

/// `chat_run_finish` に渡す終端の値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatFinish {
    pub state: ChatRunState,
    /// 出力 message の最終本文（`None` は途中まで流した本文のまま）。
    pub final_text: Option<String>,
    pub reason: Option<String>,
    /// context の上限で終わった（rollover の判断材料）。
    pub context_exhausted: bool,
}

/// `RunOutcome`（または adapter の失敗）と呼び出し側の意図を chat run の終端に写す。
pub(crate) fn chat_finish_for(
    outcome: &Result<RunOutcome, AdapterError>,
    intent: ChatStopIntent,
) -> ChatFinish {
    let finish = |state, final_text: Option<String>, reason: Option<String>| ChatFinish {
        state,
        final_text,
        reason,
        context_exhausted: false,
    };
    match intent {
        ChatStopIntent::Stop => return finish(ChatRunState::Stopped, None, None),
        ChatStopIntent::Interrupt => {
            return finish(
                ChatRunState::Interrupted,
                None,
                Some("interrupted".to_string()),
            );
        }
        ChatStopIntent::None => {}
    }
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => return finish(ChatRunState::Failed, None, Some(format!("worker: {e}"))),
    };
    let non_empty = |s: &str| (!s.trim().is_empty()).then(|| s.to_string());
    match &outcome.terminal {
        Terminal::Done { summary, .. } => finish(ChatRunState::Completed, non_empty(summary), None),
        // CoS の問いは会話の返事として人に届ける（人はそのまま次の発言で答える）。
        Terminal::Question { text } => finish(ChatRunState::Completed, non_empty(text), None),
        Terminal::Error { message, .. } => finish(
            ChatRunState::Failed,
            None,
            Some(format!("worker error: {message}")),
        ),
        Terminal::Yielded { .. } => finish(
            ChatRunState::Interrupted,
            None,
            Some("worker yielded before finishing".to_string()),
        ),
        Terminal::BudgetExhausted { kind, message, .. } => ChatFinish {
            state: ChatRunState::Interrupted,
            final_text: None,
            reason: Some(format!("budget exhausted ({kind:?}): {message}")),
            context_exhausted: *kind == BudgetKind::Context,
        },
        Terminal::Waiting { .. } => finish(
            ChatRunState::Failed,
            None,
            Some("cluster job wait is not supported in a CoS chat run".to_string()),
        ),
    }
}

/// 既存の redact（`browser_live` の console と同じ秘密判定）を行ごとに当てる。既知の秘密の値
/// （run credential など）は判定より先に置き換える。返り値の bool は行を切り詰めたか。
pub(crate) fn redact_text(text: &str, secrets: &[String]) -> (String, bool) {
    let mut cut = false;
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let line = replace_secrets(line, secrets);
            if line.chars().count() > LIVE_CONSOLE_MAX_CHARS {
                cut = true;
            }
            match task_core::browser_live::persistable(&LiveEvent::Console {
                level: "log".to_string(),
                text: line,
            }) {
                Some(PersistedLiveEvent::Console { text, .. }) => text,
                _ => REDACTED.to_string(),
            }
        })
        .collect();
    (lines.join("\n"), cut)
}

/// 既知の秘密の値だけを置き換える（status の要約用。行ごとの判定は tool の表示だけに当てる）。
pub(crate) fn replace_secrets(text: &str, secrets: &[String]) -> String {
    let mut text = text.to_string();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        if text.contains(secret.as_str()) {
            text = text.replace(secret.as_str(), REDACTED);
        }
    }
    text
}

#[derive(Debug, Default)]
struct SinkState {
    finished: bool,
    usage: Option<Usage>,
    skill_reads: u64,
    first_output_at: Option<OffsetDateTime>,
    /// 出力 message を持たない run（triage）。本文の追記は送らない（終端ではない）。
    text_rejected: bool,
    next_call: u64,
    /// 未完の tool call（`(call_id, name)`）。古い順。
    open_calls: VecDeque<(String, String)>,
    signals: ChatSessionSignals,
}

/// CoS chat run 1 本分の `EventSink`。
pub(crate) struct ChatRunSink {
    store: Arc<SqliteStore>,
    thread_id: String,
    run_id: String,
    clock: ChatClock,
    /// tool の表示から消す既知の秘密の値（run credential の token など）。
    secrets: Vec<String>,
    state: Mutex<SinkState>,
}

impl ChatRunSink {
    pub(crate) fn new(
        store: Arc<SqliteStore>,
        thread_id: impl Into<String>,
        run_id: impl Into<String>,
        clock: ChatClock,
        secrets: Vec<String>,
    ) -> Self {
        Self {
            store,
            thread_id: thread_id.into(),
            run_id: run_id.into(),
            clock,
            secrets,
            state: Mutex::new(SinkState::default()),
        }
    }

    fn state(&self) -> MutexGuard<'_, SinkState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Copy only telemetry across the existing fresh retry; session behavior is unchanged.
    pub(crate) fn inherit_telemetry(&self, previous: &Self) {
        let old = previous.state();
        let mut state = self.state();
        state.usage = old.usage;
        state.skill_reads = old.skill_reads;
        state.first_output_at = old.first_output_at;
    }

    pub(crate) fn record_usage(&self, usage: Option<Usage>) {
        let Some(usage) = usage else { return };
        let mut state = self.state();
        let Some(old) = state.usage else {
            state.usage = Some(usage);
            return;
        };
        fn add<T: Copy>(a: Option<T>, b: Option<T>, sum: impl FnOnce(T, T) -> T) -> Option<T> {
            match (a, b) {
                (Some(a), Some(b)) => Some(sum(a, b)),
                _ => None,
            }
        }
        state.usage = Some(Usage {
            input_tokens: add(old.input_tokens, usage.input_tokens, u64::saturating_add),
            output_tokens: add(old.output_tokens, usage.output_tokens, u64::saturating_add),
            cache_read_tokens: add(
                old.cache_read_tokens,
                usage.cache_read_tokens,
                u64::saturating_add,
            ),
            cache_creation_tokens: add(
                old.cache_creation_tokens,
                usage.cache_creation_tokens,
                u64::saturating_add,
            ),
            cost_usd: add(old.cost_usd, usage.cost_usd, |a, b| a + b),
            duplicate_reads: add(
                old.duplicate_reads,
                usage.duplicate_reads,
                u32::saturating_add,
            ),
            session_resumed: usage.session_resumed,
        });
    }

    /// run 途中の session の出来事の写し。
    pub(crate) fn signals(&self) -> ChatSessionSignals {
        self.state().signals.clone()
    }

    /// この sink が終端を書いたか、store の上で run が終端になっていたのを見たか。
    pub(crate) fn is_finished(&self) -> bool {
        self.state().finished
    }

    /// store への 1 件の書き込み。終端後は書かない。run が既に終端（Conflict）なら以後を止める。
    fn write<T>(&self, state: &mut SinkState, f: impl FnOnce() -> Result<T, ChatError>) {
        if state.finished {
            return;
        }
        match f() {
            Ok(_) => {}
            Err(ChatError::Conflict(msg)) => {
                // ADR 2026-10-07-cos-live-fixes D4: Conflict は「終端」だけではない（出力 message を持たない
                // triage run への本文の追記も Conflict）。store の上で終端を確かめた時だけ以後を止める。
                // 非終端のまま閉じると `finish` が終端を書かず、handle の終了後に orphan takeover が走る。
                if self.run_is_terminal() {
                    tracing::debug!(run_id = %self.run_id, %msg, "chat run already finished; dropping progress");
                    state.finished = true;
                } else {
                    tracing::debug!(run_id = %self.run_id, %msg, "chat run progress rejected; run is still live");
                    state.text_rejected |= msg.contains("no output message");
                }
            }
            Err(e) => {
                tracing::warn!(run_id = %self.run_id, error = %e, "failed to record chat run progress");
            }
        }
    }

    /// store の上で run が終端か。読めなければ非終端とみなす（`finish` が終端を書きにいく側に倒す。
    /// 既に終端なら `chat_run_finish` が Conflict を返すだけ）。
    fn run_is_terminal(&self) -> bool {
        self.store
            .chat_run_get(&self.thread_id, &self.run_id)
            .is_ok_and(|run| task_core::chat::chat_run_state_is_terminal(run.state))
    }

    fn status(&self, state: &mut SinkState, phase: ChatStatusPhase, summary: &str) {
        let summary = replace_secrets(summary, &self.secrets);
        let now = (self.clock)();
        self.write(state, || {
            self.store
                .chat_run_status(&self.run_id, phase, &summary, now)
        });
    }

    fn tool(
        &self,
        state: &mut SinkState,
        call_id: String,
        name: String,
        tool_state: ChatToolState,
        fields: &ProgressFields,
    ) {
        let (summary, _) = redact_text(fields.summary.as_deref().unwrap_or(""), &self.secrets);
        let (detail, cut) = match fields.detail.as_deref() {
            Some(d) => {
                let (d, cut) = redact_text(d, &self.secrets);
                (Some(d), cut)
            }
            None => (None, false),
        };
        let tool = ChatToolData {
            call_id,
            name,
            state: tool_state,
            summary,
            detail,
            error: fields.error || tool_state == ChatToolState::Failed,
            truncated: fields.truncated || cut,
        };
        let now = (self.clock)();
        self.write(state, || self.store.chat_run_tool(&self.run_id, &tool, now));
    }

    fn tool_use(&self, state: &mut SinkState, fields: &ProgressFields) {
        if !state.finished && skill_read(fields) {
            state.skill_reads = state.skill_reads.saturating_add(1);
        }
        state.next_call += 1;
        let call_id = format!("call-{}", state.next_call);
        let name = fields.tool.clone().unwrap_or_else(|| "tool".to_string());
        state.open_calls.push_back((call_id.clone(), name.clone()));
        self.tool(state, call_id, name, ChatToolState::Running, fields);
    }

    fn tool_result(&self, state: &mut SinkState, fields: &ProgressFields) {
        let by_name = fields
            .tool
            .as_deref()
            .and_then(|t| state.open_calls.iter().position(|(_, n)| n == t));
        let open = by_name
            .or((!state.open_calls.is_empty()).then_some(0))
            .and_then(|i| state.open_calls.remove(i));
        let (call_id, name) = match open {
            Some(call) => call,
            None => {
                state.next_call += 1;
                let name = fields.tool.clone().unwrap_or_else(|| "tool".to_string());
                (format!("call-{}", state.next_call), name)
            }
        };
        let tool_state = if fields.error {
            ChatToolState::Failed
        } else {
            ChatToolState::Completed
        };
        self.tool(state, call_id, name, tool_state, fields);
    }

    /// 終端を書く。未完の tool は failed にし、`actions` があれば実行せずに notice の card を出す。
    /// 既に終端なら何もしない（`Ok(None)`）。
    pub(crate) fn finish(
        &self,
        finish: &ChatFinish,
        actions: &ParsedActions,
    ) -> Result<Option<ChatRun>, ChatError> {
        let mut state = self.state();
        if state.finished {
            return Ok(None);
        }
        if finish.context_exhausted {
            state.signals.context_exhausted = true;
        }
        let open: Vec<(String, String)> = state.open_calls.drain(..).collect();
        for (call_id, name) in open {
            let fields = ProgressFields::of(ProgressKind::ToolResult)
                .with_summary("run ended before the tool result")
                .with_error(true);
            self.tool(&mut state, call_id, name, ChatToolState::Failed, &fields);
        }
        if !state.finished && !actions.is_empty() {
            self.actions_card(actions)?;
        }
        if state.finished {
            return Ok(None);
        }
        let now = (self.clock)();
        let result = self.store.chat_run_finish_with_telemetry(
            &self.run_id,
            finish.state,
            finish.final_text.as_deref(),
            finish.reason.as_deref(),
            now,
            state.usage.as_ref(),
            state.skill_reads,
            state.first_output_at,
        );
        state.finished = true;
        result.map(Some)
    }

    /// D3: 新 CoS run は `result.actions` を実行しない。黙って無視せず、明示エラーの card にする。
    fn actions_card(&self, actions: &ParsedActions) -> Result<(), ChatError> {
        let kinds: Vec<&str> = actions.valid.iter().map(|a| a.kind()).collect();
        let count = actions.valid.len() + actions.malformed.len();
        let reason = format!(
            "CoS chat run は result.actions を実行しない（ADR 2026-10-05 D3）。{count} 件を実行せずに捨てた{}。操作は celerisctl / API（/cos/operations）で行う",
            if kinds.is_empty() {
                String::new()
            } else {
                format!("（{}）", kinds.join(", "))
            }
        );
        let card = ChatCard {
            kind: ChatCardKind::Notice,
            id: format!("{}:actions", self.run_id),
            title: "actions は実行しない".to_string(),
            state: "error".to_string(),
            href: format!("/?thread={}", self.thread_id),
            actor: ChatActor::System,
            reason: Some(reason),
            operation_id: None,
        };
        self.store
            .chat_system_message_add(
                &self.thread_id,
                "result.actions は実行しなかった",
                &[card],
                (self.clock)(),
            )
            .map(|_| ())
    }
}

/// Inputs are the adapter's structured tool_use detail, before display redaction/truncation.
fn skill_read(fields: &ProgressFields) -> bool {
    match fields.tool.as_deref() {
        Some("Skill") => true,
        Some("Read") => {
            let input = fields
                .detail
                .as_deref()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
            let path = input
                .as_ref()
                .and_then(|v| v.get("file_path").or_else(|| v.get("path")))
                .and_then(serde_json::Value::as_str);
            let Some(path) = path else { return false };
            let mut parts = Vec::new();
            for part in path.split('/') {
                match part {
                    "" | "." => {}
                    ".." => {
                        parts.pop();
                    }
                    part => parts.push(part),
                }
            }
            parts.last() == Some(&"SKILL.md")
                && parts.windows(2).any(|p| p == [".claude", "skills"])
        }
        _ => false,
    }
}

impl EventSink for ChatRunSink {
    fn progress(&self, msg: &str) {
        let mut state = self.state();
        self.status(&mut state, ChatStatusPhase::Working, msg);
    }

    fn progress_with(&self, msg: &str, fields: &ProgressFields) {
        let mut state = self.state();
        match fields.kind {
            Some(ProgressKind::Text) => {
                let text = fields.detail.as_deref().unwrap_or(msg);
                if text.is_empty() || state.text_rejected {
                    return;
                }
                let now = (self.clock)();
                if !state.finished {
                    state.first_output_at.get_or_insert(now);
                }
                self.write(&mut state, || {
                    self.store.chat_run_append_text(&self.run_id, text, now)
                });
            }
            Some(ProgressKind::ToolUse) => self.tool_use(&mut state, fields),
            Some(ProgressKind::ToolResult) => self.tool_result(&mut state, fields),
            Some(ProgressKind::Thinking) => {
                let summary = fields.summary.clone().unwrap_or_default();
                self.status(&mut state, ChatStatusPhase::Thinking, &summary);
            }
            Some(ProgressKind::Status) => {
                if fields.tool.as_deref() == Some(task_core::tree::CONTEXT_COMPACTION_TOOL) {
                    state.signals.compacted += 1;
                }
                let summary = fields.summary.as_deref().unwrap_or(msg);
                self.status(&mut state, ChatStatusPhase::Working, summary);
            }
            None => self.status(&mut state, ChatStatusPhase::Working, msg),
        }
    }

    fn artifact(&self, _artifact: &ArtifactRef) {}

    fn heartbeat(&self) {}

    fn session_established(&self, session_id: &str) {
        self.state().signals.established = Some(session_id.to_string());
    }

    fn session_resume_failed(&self, reason: &str) {
        self.state().signals.resume_failed = Some(reason.to_string());
    }
}

#[cfg(test)]
#[path = "sink_tests.rs"]
mod tests;
