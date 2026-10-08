//! ADR 2026-10-05 D2: CoS chat thread session の判断（純粋関数）。

use task_core::chat::{ChatRunSessionMode, ChatSession, ChatSessionKey};
use time::OffsetDateTime;

use super::*;

const UUID: &str = "11111111-1111-4111-8111-111111111111";

fn key() -> ChatSessionKey {
    ChatSessionKey {
        thread_id: "t1".into(),
        harness: "claude-code".into(),
        provider: Some("claude-pool".into()),
        llm_source: Some("claude_oauth".into()),
        account_id: Some("a1".into()),
        cwd: Some("/data/cos/threads/t1/workspace".into()),
        model: Some("m1".into()),
    }
}

fn stored(tokens: i64) -> ChatSession {
    let mut s = ChatSession::new(key(), UUID, OffsetDateTime::UNIX_EPOCH);
    s.approx_tokens = tokens;
    s
}

fn facts<'a>(key: &'a ChatSessionKey, stored: Option<&'a ChatSession>) -> CosChatSessionFacts<'a> {
    CosChatSessionFacts {
        key,
        stored,
        cache_present: true,
        previous_resume_refused: false,
        context_exhausted: false,
        rollover_tokens: 1000,
    }
}

#[test]
fn cos_chat_run_session_first_run_is_new_and_match_resumes() {
    let k = key();
    let d = decide_cos_chat_session(&facts(&k, None));
    assert_eq!(d, CosChatSessionDecision::New);
    assert_eq!(d.run_mode(), ChatRunSessionMode::New);
    assert_eq!(d.reason(), None);
    let s = stored(999);
    let d = decide_cos_chat_session(&facts(&k, Some(&s)));
    assert_eq!(d, CosChatSessionDecision::Resume);
    assert_eq!(d.run_mode(), ChatRunSessionMode::Resumed);
}

#[test]
fn cos_chat_run_session_any_key_change_retires() {
    let s = stored(0);
    let changes: [fn(&mut ChatSessionKey); 7] = [
        |k| k.thread_id = "t2".into(),
        |k| k.harness = "codex".into(),
        |k| k.provider = Some("other".into()),
        |k| k.llm_source = None,
        |k| k.account_id = Some("a2".into()),
        |k| k.cwd = Some("/elsewhere".into()),
        |k| k.model = Some("m2".into()),
    ];
    for change in changes {
        let mut k = key();
        change(&mut k);
        let d = decide_cos_chat_session(&facts(&k, Some(&s)));
        assert_eq!(
            d,
            CosChatSessionDecision::Retire(CosChatRetireReason::KeyChanged),
            "{k:?}"
        );
        assert_eq!(d.run_mode(), ChatRunSessionMode::Fresh);
        assert_eq!(d.reason(), Some("key_changed"));
    }
}

#[test]
fn cos_chat_run_session_cache_missing_or_bad_id_retires() {
    let k = key();
    let s = stored(0);
    let mut f = facts(&k, Some(&s));
    f.cache_present = false;
    assert_eq!(
        decide_cos_chat_session(&f),
        CosChatSessionDecision::Retire(CosChatRetireReason::CacheMissing)
    );
    // claude-code needs a UUID; an empty (never established) id is unusable for any harness.
    for (harness, id) in [("claude-code", "not-a-uuid"), ("codex", "")] {
        let mut k = key();
        k.harness = harness.into();
        let mut s = ChatSession::new(k.clone(), id, OffsetDateTime::UNIX_EPOCH);
        s.approx_tokens = 0;
        let d = decide_cos_chat_session(&facts(&k, Some(&s)));
        assert_eq!(
            d,
            CosChatSessionDecision::Retire(CosChatRetireReason::CacheMissing),
            "{harness}"
        );
        assert_eq!(d.reason(), Some("cache_missing"));
    }
}

#[test]
fn cos_chat_run_session_resume_refusal_retires_as_fresh_after_refusal() {
    let k = key();
    let s = stored(0);
    let mut f = facts(&k, Some(&s));
    f.previous_resume_refused = true;
    let d = decide_cos_chat_session(&f);
    assert_eq!(
        d,
        CosChatSessionDecision::Retire(CosChatRetireReason::ResumeRefused)
    );
    assert_eq!(d.run_mode(), ChatRunSessionMode::FreshAfterRefusal);
    assert_eq!(d.reason(), Some("resume_refused"));
}

#[test]
fn cos_chat_run_session_context_limit_retires() {
    let k = key();
    let at_limit = stored(1000);
    let d = decide_cos_chat_session(&facts(&k, Some(&at_limit)));
    assert_eq!(
        d,
        CosChatSessionDecision::Retire(CosChatRetireReason::TokenRollover)
    );
    assert_eq!(d.reason(), Some("token_rollover"));
    assert_eq!(d.run_mode(), ChatRunSessionMode::Fresh);

    let small = stored(10);
    let mut f = facts(&k, Some(&small));
    f.context_exhausted = true;
    let d = decide_cos_chat_session(&f);
    assert_eq!(
        d,
        CosChatSessionDecision::Retire(CosChatRetireReason::ContextExhausted)
    );
    assert_eq!(d.reason(), Some("context_exhausted"));
}

#[test]
fn cos_chat_run_session_reason_precedence_is_fixed() {
    // Key change wins over everything; refusal over cache/context; cache over context.
    let s = stored(5000);
    let mut k = key();
    k.model = Some("m2".into());
    let mut f = facts(&k, Some(&s));
    f.previous_resume_refused = true;
    f.cache_present = false;
    f.context_exhausted = true;
    assert_eq!(decide_cos_chat_session(&f).reason(), Some("key_changed"));
    let k = key();
    let mut f = facts(&k, Some(&s));
    f.previous_resume_refused = true;
    f.cache_present = false;
    f.context_exhausted = true;
    assert_eq!(decide_cos_chat_session(&f).reason(), Some("resume_refused"));
    f.previous_resume_refused = false;
    assert_eq!(decide_cos_chat_session(&f).reason(), Some("cache_missing"));
    f.cache_present = true;
    assert_eq!(
        decide_cos_chat_session(&f).reason(),
        Some("context_exhausted")
    );
    f.context_exhausted = false;
    assert_eq!(decide_cos_chat_session(&f).reason(), Some("token_rollover"));
}

#[test]
fn cos_chat_run_rollover_measure_prefers_context_occupancy() {
    let k = key();
    // Occupancy known: it decides, whatever the cumulative approx_tokens says.
    let mut s = stored(5000);
    s.last_context_tokens = Some(999);
    assert_eq!(rollover_measure(&s), 999);
    assert_eq!(
        decide_cos_chat_session(&facts(&k, Some(&s))),
        CosChatSessionDecision::Resume
    );
    s.last_context_tokens = Some(1000);
    s.approx_tokens = 1;
    assert_eq!(
        decide_cos_chat_session(&facts(&k, Some(&s))).reason(),
        Some("token_rollover")
    );
    // The billing-equivalent cumulative input is never compared.
    s.last_context_tokens = Some(1);
    s.billed_input_tokens = 1_000_000;
    assert_eq!(
        decide_cos_chat_session(&facts(&k, Some(&s))),
        CosChatSessionDecision::Resume
    );
}

#[test]
fn cos_chat_run_rollover_measure_falls_back_for_legacy_rows() {
    let k = key();
    // Pre-0061 row / harness without occupancy: last_context_tokens is None → approx_tokens.
    let s = stored(1000);
    assert_eq!(s.last_context_tokens, None);
    assert_eq!(rollover_measure(&s), 1000);
    assert_eq!(
        decide_cos_chat_session(&facts(&k, Some(&s))).reason(),
        Some("token_rollover")
    );
    assert_eq!(rollover_measure(&stored(-5)), 0);
}
