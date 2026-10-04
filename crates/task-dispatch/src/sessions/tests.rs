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

// ---- ADR-0140 D1: `decide_continuation`（execute continuation の同一 session resume と fallback） ----

const UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

fn wu_session(adapter: &str, account: Option<&str>, wu: &str) -> WorkUnitSession {
    WorkUnitSession::new(
        "software-engineering",
        task_core::TaskId::new(),
        Some(wu.to_string()),
        adapter,
        account.map(str::to_string),
        Some("p1".to_string()),
        Some("/wu/a/repos/r".to_string()),
        UUID,
        OffsetDateTime::now_utc(),
    )
}

fn facts<'a>(stored: Option<&'a WorkUnitSession>) -> ContinuationFacts<'a> {
    ContinuationFacts {
        role: ContinuationRole::Worker,
        work_unit_id: Some("wu-a"),
        previous_end: Some(RunEnd::BudgetExhausted {
            kind: BudgetKind::Turns,
        }),
        previous_resume_rejected: false,
        previous_comment_interrupt: false,
        fresh_requested: false,
        adapter: "claude-code",
        account: Some("acct-1"),
        provider: Some("p1"),
        cwd: Some("/wu/a/repos/r"),
        container: false,
        stored,
        rollover_tokens: 400_000,
    }
}

fn fresh(reason: ContinuationFreshReason, retire: bool) -> ContinuationDecision {
    ContinuationDecision::Fresh { reason, retire }
}

#[test]
fn session_resume_session_reuse_same_wu_budget_or_yield_resumes() {
    let s = wu_session("claude-code", Some("acct-1"), "wu-a");
    for end in [
        RunEnd::BudgetExhausted {
            kind: BudgetKind::Turns,
        },
        RunEnd::BudgetExhausted {
            kind: BudgetKind::WallClock,
        },
        RunEnd::Yielded,
        RunEnd::Waiting,
    ] {
        let f = ContinuationFacts {
            previous_end: Some(end),
            ..facts(Some(&s))
        };
        assert_eq!(
            decide_continuation(&f),
            ContinuationDecision::Resume,
            "{end:?}"
        );
    }
}

#[test]
fn session_resume_session_reuse_account_or_provider_change_falls_back() {
    let s = wu_session("claude-code", Some("acct-1"), "wu-a");
    let other_account = ContinuationFacts {
        account: Some("acct-2"),
        ..facts(Some(&s))
    };
    assert_eq!(
        decide_continuation(&other_account),
        fresh(ContinuationFreshReason::AccountChanged, true)
    );
    let other_provider = ContinuationFacts {
        provider: Some("p2"),
        ..facts(Some(&s))
    };
    assert_eq!(
        decide_continuation(&other_provider),
        fresh(ContinuationFreshReason::AccountChanged, true)
    );
    assert!(ContinuationFreshReason::AccountChanged.starts_session());
}

#[test]
fn session_resume_session_reuse_adapter_change_falls_back() {
    // 保存 session が別アダプタ（codex）で作られていた。
    let s = wu_session("codex", Some("acct-1"), "wu-a");
    assert_eq!(
        decide_continuation(&facts(Some(&s))),
        fresh(ContinuationFreshReason::AdapterChanged, true)
    );
    // 今回のアダプタが claude-code 以外（resume の対象外。session も作らない）。
    let s = wu_session("claude-code", Some("acct-1"), "wu-a");
    let codex = ContinuationFacts {
        adapter: "codex",
        ..facts(Some(&s))
    };
    assert_eq!(
        decide_continuation(&codex),
        fresh(ContinuationFreshReason::AdapterUnsupported, true)
    );
    assert!(!ContinuationFreshReason::AdapterUnsupported.starts_session());
}

#[test]
fn session_resume_session_reuse_rejected_resume_falls_back_to_checkpoint() {
    let s = wu_session("claude-code", Some("acct-1"), "wu-a");
    let rejected = ContinuationFacts {
        previous_end: Some(RunEnd::Failed { retryable: true }),
        previous_resume_rejected: true,
        ..facts(Some(&s))
    };
    assert_eq!(
        decide_continuation(&rejected),
        fresh(ContinuationFreshReason::ResumeRejected, true)
    );
    // retire 済み（行が無い）でも理由は resume_rejected のまま。
    let rejected_retired = ContinuationFacts {
        stored: None,
        ..rejected
    };
    assert_eq!(
        decide_continuation(&rejected_retired),
        fresh(ContinuationFreshReason::ResumeRejected, false)
    );
}

#[test]
fn session_resume_session_reuse_missing_or_broken_session_falls_back() {
    // daemon の restart 後に行が無い。
    assert_eq!(
        decide_continuation(&facts(None)),
        fresh(ContinuationFreshReason::SessionMissing, false)
    );
    // UUID でない id。
    let broken = WorkUnitSession {
        session_id: "01M323X6TJQSFEP0MKXABWVY78".into(),
        ..wu_session("claude-code", Some("acct-1"), "wu-a")
    };
    assert_eq!(
        decide_continuation(&facts(Some(&broken))),
        fresh(ContinuationFreshReason::SessionMissing, true)
    );
}

#[test]
fn session_resume_session_reuse_fresh_request_rollover_and_surface_fall_back() {
    let s = wu_session("claude-code", Some("acct-1"), "wu-a");
    let requested = ContinuationFacts {
        fresh_requested: true,
        ..facts(Some(&s))
    };
    assert_eq!(
        decide_continuation(&requested),
        fresh(ContinuationFreshReason::FreshRequested, true)
    );
    let context = ContinuationFacts {
        previous_end: Some(RunEnd::BudgetExhausted {
            kind: BudgetKind::Context,
        }),
        ..facts(Some(&s))
    };
    assert_eq!(
        decide_continuation(&context),
        fresh(ContinuationFreshReason::ContextRollover, true)
    );
    let big = WorkUnitSession {
        approx_tokens: 400_000,
        ..s.clone()
    };
    assert_eq!(
        decide_continuation(&facts(Some(&big))),
        fresh(ContinuationFreshReason::ContextRollover, true)
    );
    let container = ContinuationFacts {
        container: true,
        ..facts(Some(&s))
    };
    assert_eq!(
        decide_continuation(&container),
        fresh(ContinuationFreshReason::SurfaceUnsupported, true)
    );
    let moved = ContinuationFacts {
        cwd: Some("/elsewhere"),
        ..facts(Some(&s))
    };
    assert_eq!(
        decide_continuation(&moved),
        fresh(ContinuationFreshReason::SurfaceUnsupported, true)
    );
    // review fail 後の再作業など、直前が continuable でない。
    let completed = ContinuationFacts {
        previous_end: Some(RunEnd::Completed),
        ..facts(Some(&s))
    };
    assert_eq!(
        decide_continuation(&completed),
        fresh(ContinuationFreshReason::NotContinuation, true)
    );
}

/// 付記 comment-resume: 人のコメントで止めた run（`WorkerFinished` は `interrupted: comment`。`end` は
/// `Cancelled` か無し）の次は resume する。印が無ければ従来どおり `not_continuation`（修正前の振る舞い）。
/// 判断表のほかの行（account 変更・session 欠落・rollover・container・cwd・planner）はそのまま効く。
#[test]
fn session_resume_comment_interrupt_is_continuable() {
    let s = wu_session("claude-code", Some("acct-1"), "wu-a");
    for end in [RunEnd::Cancelled, RunEnd::Failed { retryable: true }] {
        let before = ContinuationFacts {
            previous_end: Some(end),
            ..facts(Some(&s))
        };
        assert_eq!(
            decide_continuation(&before),
            fresh(ContinuationFreshReason::NotContinuation, true),
            "{end:?}"
        );
        let comment = ContinuationFacts {
            previous_comment_interrupt: true,
            ..before
        };
        assert_eq!(
            decide_continuation(&comment),
            ContinuationDecision::Resume,
            "{end:?}"
        );
        let cases = [
            (
                ContinuationFacts {
                    account: Some("acct-2"),
                    ..comment
                },
                fresh(ContinuationFreshReason::AccountChanged, true),
            ),
            (
                ContinuationFacts {
                    stored: None,
                    ..comment
                },
                fresh(ContinuationFreshReason::SessionMissing, false),
            ),
            (
                ContinuationFacts {
                    previous_resume_rejected: true,
                    ..comment
                },
                fresh(ContinuationFreshReason::ResumeRejected, true),
            ),
            (
                ContinuationFacts {
                    container: true,
                    ..comment
                },
                fresh(ContinuationFreshReason::SurfaceUnsupported, true),
            ),
            (
                ContinuationFacts {
                    cwd: Some("/elsewhere"),
                    ..comment
                },
                fresh(ContinuationFreshReason::SurfaceUnsupported, true),
            ),
            (
                ContinuationFacts {
                    fresh_requested: true,
                    ..comment
                },
                fresh(ContinuationFreshReason::FreshRequested, true),
            ),
            (
                ContinuationFacts {
                    role: ContinuationRole::Planner,
                    ..comment
                },
                fresh(ContinuationFreshReason::RoleFresh, false),
            ),
        ];
        for (f, want) in cases {
            assert_eq!(decide_continuation(&f), want, "{end:?}");
        }
        let big = WorkUnitSession {
            approx_tokens: 400_000,
            ..s.clone()
        };
        assert_eq!(
            decide_continuation(&ContinuationFacts {
                stored: Some(&big),
                ..comment
            }),
            fresh(ContinuationFreshReason::ContextRollover, true)
        );
        let codex = wu_session("codex", Some("acct-1"), "wu-a");
        assert_eq!(
            decide_continuation(&ContinuationFacts {
                stored: Some(&codex),
                ..comment
            }),
            fresh(ContinuationFreshReason::AdapterChanged, true)
        );
    }
}

#[test]
fn session_resume_fresh_session_for_planner_and_reviewer() {
    let s = wu_session("claude-code", Some("acct-1"), "wu-a");
    for role in [ContinuationRole::Planner, ContinuationRole::Reviewer] {
        let f = ContinuationFacts {
            role,
            ..facts(Some(&s))
        };
        // WU の継続 session には触れない（retire もしない）、session も作らない。
        assert_eq!(
            decide_continuation(&f),
            fresh(ContinuationFreshReason::RoleFresh, false),
            "{role:?}"
        );
    }
    assert!(!ContinuationFreshReason::RoleFresh.starts_session());
}

#[test]
fn session_resume_fresh_session_for_an_independent_work_unit() {
    // 別 WU の session は引き継がず、触れもしない。
    let other = wu_session("claude-code", Some("acct-1"), "wu-b");
    assert_eq!(
        decide_continuation(&facts(Some(&other))),
        fresh(ContinuationFreshReason::IndependentWu, false)
    );
    // WU の最初の run（直前の run が無い）。次の continuation に備えて session を作る。
    let first = ContinuationFacts {
        previous_end: None,
        ..facts(None)
    };
    assert_eq!(
        decide_continuation(&first),
        fresh(ContinuationFreshReason::IndependentWu, false)
    );
    assert!(ContinuationFreshReason::IndependentWu.starts_session());
}

#[test]
fn session_resume_fresh_session_never_resumes_another_accounts_session() {
    let s = wu_session("claude-code", Some("acct-other"), "wu-a");
    let f = ContinuationFacts {
        account: Some("acct-1"),
        ..facts(Some(&s))
    };
    assert_ne!(decide_continuation(&f), ContinuationDecision::Resume);
    // プールを使わない（account 無し）run も、account 付きの session を resume しない。
    let none = ContinuationFacts {
        account: None,
        ..facts(Some(&s))
    };
    assert_eq!(
        decide_continuation(&none),
        fresh(ContinuationFreshReason::AccountChanged, true)
    );
}

#[test]
fn session_resume_container_and_adapter_are_decided_before_first_run_and_not_continuation() {
    // #5・#8（container）は #1 の直後に見る。WU の最初の run・continuation でない run・別 WU の
    // session があっても session を作らない理由になる（ADR-0140 D3）。
    let own = wu_session("claude-code", Some("acct-1"), "wu-a");
    let other = wu_session("claude-code", Some("acct-1"), "wu-b");
    let cases: [(&str, Option<RunEnd>, Option<&WorkUnitSession>, bool); 4] = [
        // WU の最初の run（従来は IndependentWu で session を作っていた）。
        ("first run", None, None, false),
        // 別 WU の session がある（触れない）。
        ("other wu", None, Some(&other), false),
        // continuation でない（従来は NotContinuation で session を作っていた）。
        (
            "not continuation",
            Some(RunEnd::Completed),
            Some(&own),
            true,
        ),
        // continuation。
        (
            "continuation",
            Some(RunEnd::BudgetExhausted {
                kind: BudgetKind::Turns,
            }),
            Some(&own),
            true,
        ),
    ];
    for (label, previous_end, stored, retire) in cases {
        let container = ContinuationFacts {
            previous_end,
            container: true,
            ..facts(stored)
        };
        let decision = decide_continuation(&container);
        assert_eq!(
            decision,
            fresh(ContinuationFreshReason::SurfaceUnsupported, retire),
            "container: {label}"
        );
        let codex = ContinuationFacts {
            previous_end,
            adapter: "codex",
            ..facts(stored)
        };
        assert_eq!(
            decide_continuation(&codex),
            fresh(ContinuationFreshReason::AdapterUnsupported, retire),
            "adapter: {label}"
        );
        // 役割（#1）はそれより先: container の reviewer も RoleFresh。
        let reviewer = ContinuationFacts {
            role: ContinuationRole::Reviewer,
            ..container
        };
        assert_eq!(
            decide_continuation(&reviewer),
            fresh(ContinuationFreshReason::RoleFresh, false),
            "reviewer: {label}"
        );
    }
    for reason in [
        ContinuationFreshReason::SurfaceUnsupported,
        ContinuationFreshReason::AdapterUnsupported,
    ] {
        assert!(!reason.starts_session(), "{reason:?}");
    }
}
