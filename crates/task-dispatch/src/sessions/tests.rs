use super::*;

/// Phase 67b: `session_id` は `"sess-1"` ではなく有効な UUID を使う（`claude-code` を対象にした
/// テストが多く、Phase 67b の「`session_id` が UUID でなければ自己修復する」チェックに毎回
/// 引っかからないようにするため）。id の形式そのものを見るテストは別に用意する。
fn session(adapter: &str, account: Option<&str>, tokens: u64) -> NodeSession {
    NodeSession {
        approx_tokens: tokens as i64,
        ..NodeSession::new(
            "cos",
            task_core::SessionKind::Conversation,
            None,
            adapter,
            account.map(str::to_string),
            "550e8400-e29b-41d4-a716-446655440000",
            OffsetDateTime::now_utc(),
        )
    }
}

#[test]
fn no_active_session_is_fresh_without_a_summary() {
    let action = decide(None, "claude-code", Some("a"), 400_000, false);
    assert_eq!(action, SessionAction::Fresh(FreshReason::NoActive));
    assert!(!FreshReason::NoActive.needs_summary());
}

#[test]
fn a_matching_session_under_the_rollover_threshold_resumes() {
    let s = session("claude-code", Some("a"), 100);
    let action = decide(Some(&s), "claude-code", Some("a"), 400_000, false);
    assert_eq!(action, SessionAction::Resume);
}

#[test]
fn rollover_exceeded_is_fresh_with_summary() {
    let s = session("claude-code", Some("a"), 400_000);
    let action = decide(Some(&s), "claude-code", Some("a"), 400_000, false);
    assert_eq!(action, SessionAction::Fresh(FreshReason::RolloverExceeded));
    assert!(FreshReason::RolloverExceeded.needs_summary());

    // 境界のすぐ下は resume。
    let under = session("claude-code", Some("a"), 399_999);
    assert_eq!(
        decide(Some(&under), "claude-code", Some("a"), 400_000, false),
        SessionAction::Resume
    );
}

#[test]
fn account_change_is_fresh() {
    let s = session("claude-code", Some("acct-a"), 10);
    assert_eq!(
        decide(Some(&s), "claude-code", Some("acct-b"), 400_000, false),
        SessionAction::Fresh(FreshReason::AccountChanged)
    );
    // プールを使わない → 使う（あるいはその逆）も account change 扱い。
    assert_eq!(
        decide(Some(&s), "claude-code", None, 400_000, false),
        SessionAction::Fresh(FreshReason::AccountChanged)
    );
}

#[test]
fn adapter_change_is_fresh() {
    let s = session("claude-code", Some("a"), 10);
    assert_eq!(
        decide(Some(&s), "codex", Some("a"), 400_000, false),
        SessionAction::Fresh(FreshReason::AdapterChanged)
    );
}

#[test]
fn a_failed_resume_is_fresh_even_if_nothing_else_changed() {
    let s = session("claude-code", Some("a"), 10);
    assert_eq!(
        decide(Some(&s), "claude-code", Some("a"), 400_000, true),
        SessionAction::Fresh(FreshReason::ResumeFailed)
    );
}

/// ADR-0054 Phase 67b 追記: 本番事故の再現。`node_sessions` に ULID の `session_id` を持つ
/// `claude-code` の行が残っていたら（Phase 67 の産物）、他の条件が resume 可能でも自己修復する。
#[test]
fn a_claude_code_session_with_a_non_uuid_id_self_heals() {
    let mut s = session("claude-code", Some("claude_max_lab"), 10);
    s.session_id = "01M323X6TJQSFEP0MKXABWVY78".to_string(); // 本番で観測された ULID。
    let action = decide(
        Some(&s),
        "claude-code",
        Some("claude_max_lab"),
        400_000,
        false,
    );
    assert_eq!(action, SessionAction::Fresh(FreshReason::InvalidSessionId));
    assert!(FreshReason::InvalidSessionId.needs_summary());
}

/// 同じ ULID の `session_id` でも、`codex`/`acp` では形式を問わない（アダプタが決める id なので）。
#[test]
fn a_non_uuid_session_id_is_fine_for_non_claude_code_adapters() {
    for adapter in ["codex", "acp"] {
        let mut s = session(adapter, Some("a"), 10);
        s.session_id = "01M323X6TJQSFEP0MKXABWVY78".to_string();
        assert_eq!(
            decide(Some(&s), adapter, Some("a"), 400_000, false),
            SessionAction::Resume,
            "adapter={adapter}"
        );
    }
}

/// ADR-0054 Phase 67c: 空文字はどのアダプタでも無効（`codex`/`acp` が id を確定できないまま次の
/// run に来ても、それを resume しようとしない）。
#[test]
fn an_empty_session_id_self_heals_for_every_adapter() {
    for adapter in ["claude-code", "codex", "acp"] {
        let mut s = session(adapter, Some("a"), 10);
        s.session_id = String::new();
        let action = decide(Some(&s), adapter, Some("a"), 400_000, false);
        assert_eq!(
            action,
            SessionAction::Fresh(FreshReason::InvalidSessionId),
            "adapter={adapter}"
        );
        assert!(!session_id_is_valid_for_adapter(adapter, ""));
    }
}

#[test]
fn session_id_is_valid_for_adapter_only_requires_uuid_for_claude_code() {
    assert!(!session_id_is_valid_for_adapter(
        "claude-code",
        "01M323X6TJQSFEP0MKXABWVY78"
    ));
    assert!(session_id_is_valid_for_adapter(
        "claude-code",
        "550e8400-e29b-41d4-a716-446655440000"
    ));
    assert!(session_id_is_valid_for_adapter(
        "codex",
        "01M323X6TJQSFEP0MKXABWVY78"
    ));
    assert!(session_id_is_valid_for_adapter(
        "acp",
        "whatever-the-agent-returns"
    ));
}

/// ADR-0054 Phase 67b 追記: `new_session_id` は `claude-code` にだけ UUID を発行する。他のアダプタは
/// run の途中でアダプタ自身が確定させるので、celeris は id を先取りしない（空文字のまま）。
#[test]
fn new_session_id_mints_a_uuid_only_for_claude_code() {
    let id = new_session_id("claude-code");
    assert!(
        task_worker::provider::is_valid_uuid(&id),
        "{id} is not a valid UUID"
    );
    assert_eq!(new_session_id("codex"), "");
    assert_eq!(new_session_id("acp"), "");
    assert_eq!(new_session_id("unknown-future-adapter"), "");
}

/// 呼ぶたびに違う id になる（衝突しない）。
#[test]
fn new_session_id_is_not_constant() {
    assert_ne!(new_session_id("claude-code"), new_session_id("claude-code"));
}

#[test]
fn diff_lines_keeps_only_items_strictly_after_since() {
    let since = OffsetDateTime::now_utc();
    let before = since - time::Duration::seconds(10);
    let after = since + time::Duration::seconds(10);
    let lines = diff_lines(
        &[
            (before, MessageRole::User, "old question".into()),
            (after, MessageRole::User, "new question".into()),
        ],
        &[(after, "実装 done: 完了".into())],
        &[(after, "cluster-write once".into())],
        &[(after, "Pluvio".into())],
        since,
    );
    assert_eq!(
        lines,
        vec![
            "人: new question".to_string(),
            "タスク終了: 実装 done: 完了".to_string(),
            "認可: cluster-write once".to_string(),
            "新しい案件: Pluvio".to_string(),
        ]
    );
}

#[test]
fn diff_lines_is_empty_when_nothing_changed() {
    let since = OffsetDateTime::now_utc();
    let before = since - time::Duration::seconds(1);
    let lines = diff_lines(
        &[(before, MessageRole::User, "old".into())],
        &[],
        &[],
        &[],
        since,
    );
    assert!(lines.is_empty());
}

#[test]
fn summary_lines_keeps_only_the_last_n_entries() {
    let history: Vec<(MessageRole, String)> = (0..25)
        .map(|i| (MessageRole::User, format!("msg {i}")))
        .collect();
    let summary = summary_lines(&history, 20);
    assert_eq!(summary.len(), 20);
    assert_eq!(summary[0], "人: msg 5");
    assert_eq!(summary[19], "人: msg 24");
}

#[test]
fn summary_lines_handles_fewer_entries_than_the_limit() {
    let history = vec![(MessageRole::User, "hi".to_string())];
    assert_eq!(summary_lines(&history, 20), vec!["人: hi".to_string()]);
}

// ---- Phase 67c: decide_sticky ----

#[test]
fn tier_rank_orders_frontier_above_standard_above_cheap() {
    assert!(tier_rank(task_core::Tier::Frontier) > tier_rank(task_core::Tier::Standard));
    assert!(tier_rank(task_core::Tier::Standard) > tier_rank(task_core::Tier::Cheap));
}

/// 受け入れ条件 1: セッションが使える（アカウント・tier とも問題なし）→ stick。
#[test]
fn a_usable_session_sticks() {
    let s = session("claude-code", Some("claude_max_lab"), 10);
    assert_eq!(
        decide_sticky(Some(&s), 400_000, true, true),
        StickyDecision::Stick
    );
}

/// 受け入れ条件 1: アカウントが cooldown（枯渇・未ログイン等を含む） → 通常のランキングへ
/// フォールバックする（このセッションはそのランキングの結果次第で `decide` が retire する）。
#[test]
fn an_account_in_cooldown_falls_back() {
    let s = session("claude-code", Some("claude_max_lab"), 10);
    assert_eq!(
        decide_sticky(Some(&s), 400_000, false, true),
        StickyDecision::FallBack
    );
}

/// 受け入れ条件 1: プロバイダが設定から消えた（tier を提供する行が無い）→ フォールバック。
#[test]
fn a_provider_removed_from_config_falls_back() {
    let s = session("claude-code", Some("claude_max_lab"), 10);
    assert_eq!(
        decide_sticky(Some(&s), 400_000, true, false),
        StickyDecision::FallBack
    );
}

#[test]
fn no_active_session_never_sticks() {
    assert_eq!(
        decide_sticky(None, 400_000, true, true),
        StickyDecision::FallBack
    );
}

/// rollover 済みのセッションは、アカウント・tier が問題無くても留まらない（次の run で作り直す方が
/// 一貫している。前置きの要約もそちらの経路が付ける）。
#[test]
fn a_session_past_rollover_falls_back_even_if_otherwise_usable() {
    let s = session("claude-code", Some("claude_max_lab"), 400_000);
    assert_eq!(
        decide_sticky(Some(&s), 400_000, true, true),
        StickyDecision::FallBack
    );
}

/// プールを使わないセッション（`account_id = None`）は、アカウントの状態を問わず provider さえ
/// あれば stick できる（呼び出し側が `account_usable = true` を渡す想定どおり）。
#[test]
fn a_poolless_session_sticks_without_an_account_check() {
    let s = session("acp", None, 10);
    assert_eq!(
        decide_sticky(Some(&s), 400_000, true, true),
        StickyDecision::Stick
    );
}
