use super::*;

#[test]
fn classifies_exhausted() {
    assert_eq!(
        classify_provider_failure("You've hit your usage limit"),
        Some(ProviderFailure::Exhausted)
    );
    assert_eq!(
        classify_provider_failure("Quota exceeded for this project"),
        Some(ProviderFailure::Exhausted)
    );
    assert_eq!(
        classify_provider_failure("Your credit balance is too low to access the API"),
        Some(ProviderFailure::Exhausted)
    );
}

#[test]
fn classifies_throttled_with_fixed_retry_after() {
    assert_eq!(
        classify_provider_failure("API Error: 429 rate limit exceeded"),
        Some(ProviderFailure::Throttled {
            retry_after_secs: 60
        })
    );
    assert_eq!(
        classify_provider_failure("Overloaded, please retry later"),
        Some(ProviderFailure::Throttled {
            retry_after_secs: 60
        })
    );
    assert_eq!(
        classify_provider_failure("HTTP 529: too many requests"),
        Some(ProviderFailure::Throttled {
            retry_after_secs: 60
        })
    );
    assert_eq!(
        classify_provider_failure("upstream returned rate_limit_error"),
        Some(ProviderFailure::Throttled {
            retry_after_secs: 60
        })
    );
    assert_eq!(
        classify_provider_failure("status=429"),
        Some(ProviderFailure::Throttled {
            retry_after_secs: 60
        })
    );
}

#[test]
fn classifies_auth_failed() {
    assert_eq!(
        classify_provider_failure("Invalid API key \u{b7} Please run /login"),
        Some(ProviderFailure::AuthFailed)
    );
    assert_eq!(
        classify_provider_failure("401 Unauthorized"),
        Some(ProviderFailure::AuthFailed)
    );
    assert_eq!(
        classify_provider_failure("authentication required"),
        Some(ProviderFailure::AuthFailed)
    );
    assert_eq!(
        classify_provider_failure("you are not logged in"),
        Some(ProviderFailure::AuthFailed)
    );
}

#[test]
fn exhausted_takes_priority_over_throttled_patterns() {
    // "usage limit" 自体には throttled のパターンは含まれないが、判定順（Exhausted が先）を
    // 明示的に固定するため、優先順位そのものを検証する。
    assert_eq!(
        classify_provider_failure("usage limit reached, try again tomorrow"),
        Some(ProviderFailure::Exhausted)
    );
}

#[test]
fn unmatched_text_returns_none() {
    assert_eq!(
        classify_provider_failure(
            "The 'gpt-5.4' model is not supported when using Codex with a ChatGPT account."
        ),
        None
    );
    assert_eq!(classify_provider_failure(""), None);
}

/// 監査の指摘: スタックトレースの行・列番号やバージョン番号に含まれる数字列は供給側失敗ではない。
#[test]
fn status_codes_inside_positions_or_numbers_do_not_match() {
    for text in [
        "TypeError: x is undefined\n    at run (cli.js:4291:17)",
        "at file.js:429:17",
        "node v14290.1",
        "exit code 14011",
        "request id 4015xyz",
    ] {
        assert_eq!(classify_provider_failure(text), None, "{text}");
    }
}

/// U34-1 / P-87: Python の rich トレースバックの `file.py:529` 形（`:` の後に数字が続かず、行番号
/// だけで終わる）を HTTP 529 と誤認しない。実機（Phase 34 通し）の出力そのものを使う。
#[test]
fn real_traceback_file_line_is_not_a_status_code() {
    let text = "\
Traceback (most recent call last):
  File \"/home/user/.venv/lib/python3.11/site-packages/pqa/main.py\", line 42, in run
    from PIL import Image
ModuleNotFoundError: No module named 'PIL'

During handling of the above exception, another exception occurred:

Traceback (most recent call last):
  File \"/home/user/.venv/lib/python3.11/site-packages/pypdf/_page.py\", line 529, in __getitem__
    ] pypdf/_page.py:529 in __getitem__
ImportError: cannot import name 'Image' from 'PIL' (unknown location)
";
    assert_eq!(classify_provider_failure(text), None, "{text}");
}

/// 決定に挙げた「ステータスの文脈」の形はすべて Throttled/AuthFailed に一致する。
#[test]
fn status_context_forms_match() {
    for text in [
        "HTTP 529 received",
        "status 529",
        "529 Too Many Requests",
        "Error 529",
        "got (529) back",
    ] {
        assert_eq!(
            classify_provider_failure(text),
            Some(ProviderFailure::Throttled {
                retry_after_secs: 60
            }),
            "{text}"
        );
    }
}

/// 位置情報でも数字が単独で残らない限り誤って一致しない: `[\w/.-]+\.\w{1,5}:\d+` は拡張子を問わず
/// 汎用に無視する。
#[test]
fn generic_file_extensions_before_line_numbers_are_ignored() {
    for text in [
        "at foo/bar.rs:529:1",
        "in module.go:401",
        "see script.rb:429 for details",
        "src/main.ts:529: unexpected token",
    ] {
        assert_eq!(classify_provider_failure(text), None, "{text}");
    }
}

#[test]
fn matching_is_case_insensitive() {
    assert_eq!(
        classify_provider_failure("RATE LIMIT EXCEEDED"),
        Some(ProviderFailure::Throttled {
            retry_after_secs: 60
        })
    );
    assert_eq!(
        classify_provider_failure("NOT LOGGED IN"),
        Some(ProviderFailure::AuthFailed)
    );
}

#[test]
fn resume_rejection_matches_known_phrases_case_insensitively() {
    for text in [
        "Error: No conversation found for session 01ARZ3",
        "session not found",
        "SESSION NOT FOUND",
        "invalid session id",
        "could not find session 01ARZ3",
    ] {
        assert!(looks_like_resume_rejection(text), "{text}");
    }
    assert!(!looks_like_resume_rejection("wall clock exceeded"));
    assert!(!looks_like_resume_rejection(""));
}

/// Phase 113 D4(a): 本番のタスク 01M35X86XTK84F97QW0CN5PGMR / reviewer run
/// 01M388BENASH3JEBWFS03KEQYT で観測した実機の文言そのもの。
#[test]
fn phase_113_matches_the_production_claude_code_wording() {
    assert!(looks_like_resume_rejection(
        "No conversation found with session ID: 01a0d017-e32a-4cad-b10c-0cb63869ae13"
    ));
}

/// Phase 113 D1: `could not resume` と、id が間に挟まる「session <id> not found」の形。
#[test]
fn phase_113_matches_could_not_resume_and_session_id_not_found_with_a_gap() {
    assert!(looks_like_resume_rejection(
        "Error: could not resume conversation"
    ));
    assert!(looks_like_resume_rejection(
        "session 01a0d017-e32a-4cad-b10c-0cb63869ae13 not found"
    ));
    // 「session」と「not found」が離れすぎている（無関係な文脈）ものまでは拾わない。
    assert!(!looks_like_resume_rejection(&format!(
        "session {} start ok; separately, the file was not found",
        "x".repeat(200)
    )));
}

/// Phase 67b: `--session-id`/`--resume` に渡してよい id かどうか。
#[test]
fn valid_uuid_accepts_hyphenated_hex_ignoring_case() {
    assert!(is_valid_uuid("550e8400-e29b-41d4-a716-446655440000"));
    assert!(is_valid_uuid("550E8400-E29B-41D4-A716-446655440000"));
    // version/variant の厳密な検査はしない（形式だけ見る）。
    assert!(is_valid_uuid("00000000-0000-0000-0000-000000000000"));
}

/// Phase 67b の本番事故: ULID はハイフンの位置も長さも UUID と違うので弾く。
#[test]
fn valid_uuid_rejects_a_ulid_and_other_non_uuid_shapes() {
    assert!(!is_valid_uuid("01M323X6TJQSFEP0MKXABWVY78"));
    assert!(!is_valid_uuid("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
    assert!(!is_valid_uuid(""));
    assert!(!is_valid_uuid("not-a-uuid-at-all"));
    // 長さは合っているがハイフンの位置がずれている。
    assert!(!is_valid_uuid("550e8400e29b-41d4-a716-446655440000"));
    // 長さが 1 文字短い。
    assert!(!is_valid_uuid("550e8400-e29b-41d4-a716-44665544000"));
    // 16 進数でない文字を含む。
    assert!(!is_valid_uuid("550e8400-e29b-41d4-a716-44665544000g"));
}
