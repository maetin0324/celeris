//! ADR 2026-10-05 D2: CoS chat session の rollover（retire → fresh）と、resume 拒否後の fresh 再試行。
//!
//! - 起動時（`choose_session`）: 照合 key・cache・前回の run の終わり方（resume 拒否・context 枯渇）・
//!   `[sessions] rollover_tokens` を [`decide_cos_chat_session`] に渡し、resume か retire→fresh かを決めて
//!   `chat_runs` に session_mode と理由を残す。前回の run の終わり方は DB（`chat_runs.reason` と
//!   `session_row_id`）から読む。dispatcher のメモリには持たない。
//! - run の中（`run_attempts`）: resume した run が resume 拒否（`session_resume_failed`）で終わったら、
//!   同じ chat run のまま session を retire し、summary＋未要約履歴で fresh session を作って**同じ入力**に
//!   対して 1 回だけ再試行する。入力 message は claim し直さない（配送記録 `input_message_id` と
//!   `message.state` はこの run のまま）ので二重に配送されない。run credential も同じなので、worker が
//!   同じ冪等 key で出した `cos_operations` は既存の記録が返り、二重に適用されない。2 回目の拒否・失敗は
//!   理由付きの failed。
//! - summary の水位（`summary_through_seq`）は配送 cursor（入力 message）と別に扱う。再試行の直前に
//!   thread の summary を読み直し、1 回目の worker が checkpoint で水位を上げていればそれ以降だけを
//!   未要約履歴として渡す。入力は変えない。
//! - 終端で context 枯渇（harness の event か `BudgetExhausted{kind: Context}`）を観測したら run の理由に
//!   [`CONTEXT_EXHAUSTED_MARK`] を残し、次の run を fresh（`context_exhausted`）にする。
//!
//! LLM 呼び出しは無い（summary は worker が checkpoint で書いたものだけを使う）。

use std::path::Path;
use std::sync::Arc;

use task_core::SqliteStore;
use task_core::chat::{
    ChatMessageQuery, ChatRunSessionMode, ChatRunState, ChatSession, ChatSessionKey,
    ChatStatusPhase,
};
use task_worker::adapter::{AdapterError, RunLimits, RunOutcome, Terminal, WorkerAdapter};
use task_worker::protocol::{CosChatHistory, CosChatHistoryMessage, RunRequest, SessionHandle};
use time::OffsetDateTime;

use super::sink::{ChatClock, ChatFinish, ChatRunSink, ChatStopIntent, chat_finish_for};
use crate::sessions::cos_chat::{
    CosChatSessionDecision, CosChatSessionFacts, decide_cos_chat_session,
};

/// run の理由の先頭に付ける印。次の run の起動時にこれを見て fresh にする。
pub(crate) const CONTEXT_EXHAUSTED_MARK: &str = "context exhausted";
/// resume 拒否で終わった run の理由の先頭（再試行できなかった場合。次の run を fresh にする）。
pub(crate) const RESUME_REFUSED_MARK: &str = "resume refused";

/// 起動時に決めた session。
#[derive(Debug, Clone)]
pub(crate) struct SessionChoice {
    pub session: ChatSession,
    pub mode: ChatRunSessionMode,
    pub reason: Option<&'static str>,
}

/// 前回の run（この thread の最新の終端 run）の終わり方。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PreviousRunSignals {
    pub resume_refused: bool,
    pub context_exhausted: bool,
}

/// `run_id` を除く、この thread の最新の run の終わり方を DB から読む。現役 session（`session_row_id`）
/// で走った run でなければ（既に retire 済みの session の run）何も無いとみなす。
pub(crate) fn previous_run_signals(
    store: &SqliteStore,
    thread_id: &str,
    current_run_id: &str,
    session_row_id: &str,
) -> Result<PreviousRunSignals, String> {
    let mut before = None;
    loop {
        let page = store
            .chat_message_list(
                thread_id,
                &ChatMessageQuery {
                    before_seq: before,
                    limit: Some(200),
                    ..ChatMessageQuery::default()
                },
            )
            .map_err(|e| e.to_string())?;
        let mut items = page.items;
        items.sort_by_key(|m| std::cmp::Reverse(m.seq));
        for message in items {
            let Some(run_id) = message.run_id.as_deref() else {
                continue;
            };
            if run_id == current_run_id {
                continue;
            }
            let run = store
                .chat_run_get(thread_id, run_id)
                .map_err(|e| e.to_string())?;
            if !task_core::chat::chat_run_state_is_terminal(run.state) {
                continue;
            }
            let record = store
                .chat_run_session_record(run_id)
                .map_err(|e| e.to_string())?;
            if record.session_row_id.as_deref() != Some(session_row_id) {
                return Ok(PreviousRunSignals::default());
            }
            let reason = run.reason.as_deref().unwrap_or("");
            return Ok(PreviousRunSignals {
                resume_refused: reason.starts_with(RESUME_REFUSED_MARK),
                context_exhausted: reason.starts_with(CONTEXT_EXHAUSTED_MARK),
            });
        }
        before = page.next_before_seq;
        if before.is_none() {
            return Ok(PreviousRunSignals::default());
        }
    }
}

/// 起動時の session の選択（resume か retire→fresh）と `chat_runs` への記録。
pub(crate) fn choose_session(
    store: &SqliteStore,
    run_id: &str,
    key: ChatSessionKey,
    rollover_tokens: u64,
    now: OffsetDateTime,
) -> Result<SessionChoice, String> {
    let thread_id = key.thread_id.clone();
    let existing = store
        .chat_session_active(&thread_id)
        .map_err(|e| e.to_string())?;
    let previous = match existing.as_ref() {
        Some(old) => previous_run_signals(store, &thread_id, run_id, &old.id)?,
        None => PreviousRunSignals::default(),
    };
    let decision = decide_cos_chat_session(&CosChatSessionFacts {
        key: &key,
        stored: existing.as_ref(),
        cache_present: existing
            .as_ref()
            .is_some_and(|old| !old.session_id.is_empty()),
        previous_resume_refused: previous.resume_refused,
        context_exhausted: previous.context_exhausted,
        rollover_tokens,
    });
    let choice = match (decision, existing) {
        (CosChatSessionDecision::Resume, Some(old)) => SessionChoice {
            session: old,
            mode: ChatRunSessionMode::Resumed,
            reason: None,
        },
        _ => SessionChoice {
            session: rotate_fresh(store, key, now)?,
            mode: decision.run_mode(),
            reason: decision.reason(),
        },
    };
    store
        .chat_run_record_session(run_id, choice.mode, &choice.session.id, choice.reason, now)
        .map_err(|e| e.to_string())?;
    Ok(choice)
}

/// 旧 session を retire し、thread の summary の水位から始まる fresh session を作る。
fn rotate_fresh(
    store: &SqliteStore,
    key: ChatSessionKey,
    now: OffsetDateTime,
) -> Result<ChatSession, String> {
    let (_, summary_through) = store
        .chat_thread_summary(&key.thread_id)
        .map_err(|e| e.to_string())?;
    let mut next = ChatSession::new(
        key.clone(),
        crate::sessions::new_session_id(&key.harness),
        now,
    );
    next.summary_through_seq = i64::try_from(summary_through).unwrap_or(i64::MAX);
    store
        .chat_session_rotate(&next, now)
        .map_err(|e| e.to_string())?;
    Ok(next)
}

/// thread の summary を読み直し、入力 `input_seq` より前の未要約履歴を作る（配送 cursor と別）。
pub(crate) fn history_since_summary(
    store: &SqliteStore,
    thread_id: &str,
    input_seq: u64,
) -> Result<(Option<String>, u64, CosChatHistory), String> {
    let (summary, through) = store
        .chat_thread_summary(thread_id)
        .map_err(|e| e.to_string())?;
    let from = through.saturating_add(1);
    let until = input_seq.saturating_sub(1);
    let mut messages = Vec::new();
    let mut after = through;
    while after < until {
        let page = store
            .chat_message_list(
                thread_id,
                &ChatMessageQuery {
                    after_seq: Some(after),
                    limit: Some(200),
                    ..ChatMessageQuery::default()
                },
            )
            .map_err(|e| e.to_string())?;
        let Some(last) = page.items.iter().map(|m| m.seq).max() else {
            break;
        };
        messages.extend(page.items.into_iter().filter(|m| m.seq <= until).map(|m| {
            CosChatHistoryMessage {
                id: m.id,
                seq: m.seq as i64,
                role: format!("{:?}", m.role).to_lowercase(),
                text: m.text,
            }
        }));
        if last <= after {
            break;
        }
        after = last;
    }
    messages.sort_by_key(|m| m.seq);
    messages.dedup_by_key(|m| m.seq);
    Ok((
        (!summary.is_empty()).then_some(summary),
        through,
        CosChatHistory {
            from_seq: from as i64,
            through_seq: until as i64,
            messages,
        },
    ))
}

/// run の usage（input+output）。session の `approx_tokens` に積む（rollover_tokens の材料）。
pub(crate) fn usage_tokens(outcome: &Result<RunOutcome, AdapterError>) -> i64 {
    let usage = match outcome {
        Ok(RunOutcome {
            terminal:
                Terminal::Done { usage, .. }
                | Terminal::Yielded { usage, .. }
                | Terminal::BudgetExhausted { usage, .. }
                | Terminal::Waiting { usage, .. },
            ..
        }) => usage.as_ref(),
        _ => None,
    };
    usage
        .map(|u| u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0))
        .map_or(0, |t| i64::try_from(t).unwrap_or(i64::MAX))
}

/// 1 回の試行の後始末: harness の決めた session id と usage を session 行に残す。
fn record_attempt(
    store: &SqliteStore,
    session_row_id: &str,
    sink: &ChatRunSink,
    outcome: &Result<RunOutcome, AdapterError>,
    run_id: &str,
    now: OffsetDateTime,
) {
    if let Some(session_id) = sink.signals().established
        && let Err(error) = store.chat_session_set_id(session_row_id, &session_id)
    {
        tracing::warn!(%error, %run_id, "CoS session id was not saved");
    }
    if let Err(error) = store.chat_session_touch(session_row_id, usage_tokens(outcome), now) {
        tracing::warn!(%error, %run_id, "CoS session usage was not saved");
    }
}

/// worker を走らせるのに要る値（launch が組む）。
pub(crate) struct ChatAttempt {
    pub store: Arc<SqliteStore>,
    pub adapter: Arc<dyn WorkerAdapter>,
    pub req: RunRequest,
    pub thread_id: String,
    pub run_id: String,
    pub session_row_id: String,
    pub secrets: Vec<String>,
    pub limits: RunLimits,
    pub clock: ChatClock,
}

/// worker を走らせ、resume 拒否なら fresh で 1 回だけ再試行する。終端を書くのは呼び出し側
/// （返した sink の `finish`）。返す `ChatFinish` には context 枯渇の印と再試行の理由が入っている。
pub(crate) async fn run_attempts(a: ChatAttempt) -> (ChatRunSink, ChatFinish) {
    let resumed = a
        .req
        .context
        .session
        .as_ref()
        .is_some_and(|session| session.resume);
    let sink = ChatRunSink::new(
        Arc::clone(&a.store),
        &a.thread_id,
        &a.run_id,
        Arc::clone(&a.clock),
        a.secrets.clone(),
    );
    let outcome = a
        .adapter
        .run(a.req.clone(), &a.run_id, a.limits, &sink)
        .await;
    record_attempt(
        &a.store,
        &a.session_row_id,
        &sink,
        &outcome,
        &a.run_id,
        (a.clock)(),
    );
    let refused = sink.signals().resume_failed;
    let Some(refusal) = refused.filter(|_| resumed) else {
        let finish = mark_context(chat_finish_for(&outcome, ChatStopIntent::None), &sink);
        return (sink, finish);
    };
    let live = a
        .store
        .chat_run_get(&a.thread_id, &a.run_id)
        .is_ok_and(|run| run.state == ChatRunState::Running);
    if !live {
        // 人の stop・割り込みが先に来ていれば再試行しない（終端は呼び出し側の意図に任せる）。
        let finish = chat_finish_for(&outcome, ChatStopIntent::None);
        return (sink, finish);
    }
    match prepare_fresh_retry(&a, &refusal) {
        Ok((req, session_row_id)) => {
            let retry = ChatRunSink::new(
                Arc::clone(&a.store),
                &a.thread_id,
                &a.run_id,
                Arc::clone(&a.clock),
                a.secrets.clone(),
            );
            let outcome = a.adapter.run(req, &a.run_id, a.limits, &retry).await;
            record_attempt(
                &a.store,
                &session_row_id,
                &retry,
                &outcome,
                &a.run_id,
                (a.clock)(),
            );
            let mut finish = mark_context(chat_finish_for(&outcome, ChatStopIntent::None), &retry);
            if let Some(again) = retry.signals().resume_failed {
                finish.state = ChatRunState::Failed;
                finish.final_text = None;
                finish.reason = Some(format!(
                    "{RESUME_REFUSED_MARK} again after the single fresh retry: {again}"
                ));
            } else if finish.state == ChatRunState::Failed {
                finish.reason = Some(format!(
                    "fresh retry after {RESUME_REFUSED_MARK} failed: {}",
                    finish.reason.as_deref().unwrap_or("unknown")
                ));
            }
            (retry, finish)
        }
        Err(error) => {
            let finish = ChatFinish {
                state: ChatRunState::Failed,
                final_text: None,
                reason: Some(format!(
                    "{RESUME_REFUSED_MARK} ({refusal}); fresh retry could not start: {error}"
                )),
                context_exhausted: false,
            };
            (sink, finish)
        }
    }
}

/// context 枯渇を観測した run の理由に印を付ける（次の run を fresh にする）。
fn mark_context(mut finish: ChatFinish, sink: &ChatRunSink) -> ChatFinish {
    if finish.context_exhausted || sink.signals().context_exhausted {
        finish.context_exhausted = true;
        finish.reason = Some(match finish.reason.take() {
            Some(reason) if reason.starts_with(CONTEXT_EXHAUSTED_MARK) => reason,
            Some(reason) => format!("{CONTEXT_EXHAUSTED_MARK}: {reason}"),
            None => format!("{CONTEXT_EXHAUSTED_MARK}: the next run starts a fresh session"),
        });
    }
    finish
}

/// resume を拒否された session を retire し、fresh session と同じ入力で request を組み直す。
fn prepare_fresh_retry(a: &ChatAttempt, refusal: &str) -> Result<(RunRequest, String), String> {
    let now = (a.clock)();
    let old = a
        .store
        .chat_session_get(&a.session_row_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("session {} vanished", a.session_row_id))?;
    let fresh = rotate_fresh(&a.store, old.key, now)?;
    a.store
        .chat_run_record_session(
            &a.run_id,
            ChatRunSessionMode::FreshAfterRefusal,
            &fresh.id,
            Some(crate::sessions::cos_chat::CosChatRetireReason::ResumeRefused.as_str()),
            now,
        )
        .map_err(|e| e.to_string())?;
    if let Err(error) = a.store.chat_run_status(
        &a.run_id,
        ChatStatusPhase::Working,
        &format!("{RESUME_REFUSED_MARK}; retrying once with a fresh session ({refusal})"),
        now,
    ) {
        tracing::warn!(%error, run_id = %a.run_id, "CoS fresh retry status failed");
    }
    let mut req = a.req.clone();
    if let Some(chat) = req.context.cos_chat.as_mut() {
        let input_seq = chat
            .inputs
            .iter()
            .map(|input| input.seq)
            .min()
            .and_then(|seq| u64::try_from(seq).ok())
            .unwrap_or(0);
        let (summary, through, history) = history_since_summary(&a.store, &a.thread_id, input_seq)?;
        chat.summary = summary;
        chat.summary_through_seq = i64::try_from(through).unwrap_or(i64::MAX);
        chat.unsummarized = history;
    }
    req.context.session = Some(SessionHandle {
        adapter: fresh.key.harness.clone(),
        session_id: fresh.session_id.clone(),
        resume: false,
    });
    clear_attempt_result(&req.artifacts_dir);
    Ok((req, fresh.id))
}

/// 1 回目の試行の result.json を再試行に持ち越さない（actions のカードを二重に出さない）。
fn clear_attempt_result(artifacts_dir: &Path) {
    let path = artifacts_dir.join("result.json");
    if let Err(error) = std::fs::remove_file(&path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, path = %path.display(), "CoS retry could not clear result.json");
    }
}
