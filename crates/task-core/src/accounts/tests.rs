use super::*;

#[test]
fn parses_the_line_observed_on_claude_2_1_273() {
    let line: serde_json::Value = serde_json::from_str(
        r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1789605600,"rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"out_of_credits","isUsingOverage":false,"unifiedWindows":{"five_hour":{"utilization":0.14,"resetsAt":1789605600},"seven_day":{"utilization":0.24,"resetsAt":1790031600}}},"uuid":"u","session_id":"s"}"#,
    )
    .expect("json");
    let obs = RateLimitObservation::from_stream_json(&line, 100).expect("observation");
    assert_eq!(
        obs.five_hour,
        Some(RateWindow {
            utilization: 0.14,
            resets_at: 1789605600
        })
    );
    assert_eq!(
        obs.seven_day,
        Some(RateWindow {
            utilization: 0.24,
            resets_at: 1790031600
        })
    );
    assert_eq!(obs.status.as_deref(), Some("allowed"));
    assert_eq!(obs.resets_at, Some(1789605600));
    assert_eq!(obs.observed_at, 100);
}

#[test]
fn missing_windows_and_other_types_are_tolerated() {
    let partial: serde_json::Value = serde_json::from_str(
        r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":5}}"#,
    )
    .expect("json");
    let obs = RateLimitObservation::from_stream_json(&partial, 1).expect("status only");
    assert!(obs.five_hour.is_none() && obs.seven_day.is_none());
    assert_eq!(obs.status.as_deref(), Some("rejected"));

    let other: serde_json::Value = serde_json::from_str(r#"{"type":"assistant"}"#).expect("json");
    assert!(RateLimitObservation::from_stream_json(&other, 1).is_none());
    let empty: serde_json::Value =
        serde_json::from_str(r#"{"type":"rate_limit_event","rate_limit_info":{}}"#).expect("json");
    assert!(RateLimitObservation::from_stream_json(&empty, 1).is_none());
}

// ---- AccountAdapter (ADR-0025 D1) ----

#[test]
fn account_adapter_string_round_trip_and_markers() {
    assert_eq!(AccountAdapter::ClaudeCode.as_str(), "claude-code");
    assert_eq!(AccountAdapter::Codex.as_str(), "codex");
    assert_eq!(
        AccountAdapter::parse("claude-code"),
        Some(AccountAdapter::ClaudeCode)
    );
    assert_eq!(AccountAdapter::parse("codex"), Some(AccountAdapter::Codex));
    assert_eq!(
        AccountAdapter::parse("opencode-go"),
        Some(AccountAdapter::OpencodeGo)
    );
    assert_eq!(
        AccountAdapter::OpencodeGo.credentials_marker(),
        "opencode/auth.json"
    );
    assert_eq!(AccountAdapter::OpencodeGo.env_var(), "XDG_DATA_HOME");
    assert_eq!(AccountAdapter::ALL.len(), 3);
    assert_eq!(AccountAdapter::parse("fake"), None);
    assert_eq!(
        AccountAdapter::ClaudeCode.credentials_marker(),
        ".credentials.json"
    );
    assert_eq!(AccountAdapter::Codex.credentials_marker(), "auth.json");
    assert_eq!(
        AccountAdapter::ClaudeCode.env_var(),
        "CLAUDE_SECURESTORAGE_CONFIG_DIR"
    );
    assert_eq!(AccountAdapter::Codex.env_var(), "CODEX_HOME");
    assert_eq!(
        serde_json::to_string(&AccountAdapter::Codex).expect("json"),
        "\"codex\""
    );
    assert_eq!(
        serde_json::to_string(&AccountAdapter::ClaudeCode).expect("json"),
        "\"claude-code\""
    );
}

// ---- RateLimitObservation::from_codex_token_count (ADR-0025 D3) ----

/// `resets_in_seconds` を使う形（実測でこの名前が使われている場合）。
#[test]
fn codex_token_count_with_resets_in_seconds_field() {
    let line: serde_json::Value = serde_json::from_str(
        r#"{"type":"token_count","rate_limits":{"primary":{"used_percent":14.0,"window_minutes":300,"resets_in_seconds":3600},"secondary":{"used_percent":24.0,"window_minutes":10080,"resets_in_seconds":432000}}}"#,
    )
    .expect("json");
    let obs = RateLimitObservation::from_codex_token_count(&line, 1_000).expect("observation");
    assert_eq!(
        obs.five_hour,
        Some(RateWindow {
            utilization: 0.14,
            resets_at: 1_000 + 3_600
        })
    );
    assert_eq!(
        obs.seven_day,
        Some(RateWindow {
            utilization: 0.24,
            resets_at: 1_000 + 432_000
        })
    );
    assert_eq!(obs.observed_at, 1_000);
}

/// `reset_after_seconds` を使う形。
#[test]
fn codex_token_count_with_reset_after_seconds_field() {
    let line: serde_json::Value = serde_json::from_str(
        r#"{"type":"token_count","rate_limits":{"primary":{"used_percent":50.0,"window_minutes":300,"reset_after_seconds":7200}}}"#,
    )
    .expect("json");
    let obs = RateLimitObservation::from_codex_token_count(&line, 500).expect("observation");
    assert_eq!(
        obs.five_hour,
        Some(RateWindow {
            utilization: 0.5,
            resets_at: 500 + 7_200
        })
    );
    assert!(obs.seven_day.is_none());
}

/// `resets_at` は Unix 絶対秒。
#[test]
fn codex_token_count_with_absolute_resets_at() {
    let line: serde_json::Value = serde_json::from_str(
        r#"{"type":"token_count","rate_limits":{"secondary":{"used_percent":10.0,"window_minutes":10080,"resets_at":86400}}}"#,
    )
    .expect("json");
    let obs = RateLimitObservation::from_codex_token_count(&line, 2_000).expect("observation");
    assert_eq!(
        obs.seven_day,
        Some(RateWindow {
            utilization: 0.1,
            resets_at: 86_400
        })
    );
    assert!(obs.five_hour.is_none());
}

#[test]
fn codex_missing_or_invalid_windows_are_unknown() {
    for primary in [
        serde_json::json!({"usedPercent": 20, "windowDurationMins": 300}),
        serde_json::json!({"usedPercent": -1, "windowDurationMins": 300, "resetsAt": 1000}),
        serde_json::json!({"usedPercent": 101, "windowDurationMins": 300, "resetsAt": 1000}),
        serde_json::json!({"usedPercent": 20, "windowDurationMins": 0, "resetsAt": 1000}),
    ] {
        assert!(
            RateLimitObservation::from_codex_account_limits(
                &serde_json::json!({"rateLimits": {"primary": primary}}),
                100
            )
            .is_none()
        );
    }
}

#[test]
fn codex_app_server_prefers_codex_bucket_and_absolute_time() {
    let result = serde_json::json!({
        "rateLimits": {"primary": {"usedPercent": 99, "windowDurationMins": 300, "resetsAt": 2000}},
        "rateLimitsByLimitId": {"codex": {
            "primary": {"usedPercent": 25, "windowDurationMins": 300, "resetsAt": 2000},
            "secondary": {"usedPercent": 40, "windowDurationMins": 10080, "resetsAt": 9000}
        }}
    });
    let obs = RateLimitObservation::from_codex_account_limits(&result, 1000).unwrap();
    assert_eq!(
        obs.five_hour,
        Some(RateWindow {
            utilization: 0.25,
            resets_at: 2000
        })
    );
    assert_eq!(
        obs.seven_day,
        Some(RateWindow {
            utilization: 0.4,
            resets_at: 9000
        })
    );
    assert!(RateLimitObservation::from_codex_account_limits(&serde_json::json!({"rateLimits": {"limitId": "other", "primary": {"usedPercent": 25, "windowDurationMins": 300, "resetsAt": 2000}}}), 1000).is_none());
}

/// `window_minutes` による枠の割り当て: 1440 以下は five_hour（短い枠）、それより長ければ seven_day（長い枠）。
/// `primary`/`secondary` という名前ではなく窓の長さで判断する（境界値もテストする）。
#[test]
fn codex_token_count_window_classification_is_by_length_not_by_key_name() {
    let boundary: serde_json::Value = serde_json::from_str(
        r#"{"type":"token_count","rate_limits":{"primary":{"used_percent":1.0,"window_minutes":1440,"resets_in_seconds":1}}}"#,
    )
    .expect("json");
    let obs = RateLimitObservation::from_codex_token_count(&boundary, 0).expect("observation");
    assert!(
        obs.five_hour.is_some(),
        "1440 minutes is still the short window"
    );
    assert!(obs.seven_day.is_none());

    // 名前が "secondary" でも window_minutes が短ければ five_hour 枠に入る。
    let swapped: serde_json::Value = serde_json::from_str(
        r#"{"type":"token_count","rate_limits":{"secondary":{"used_percent":5.0,"window_minutes":60,"resets_in_seconds":10}}}"#,
    )
    .expect("json");
    let obs2 = RateLimitObservation::from_codex_token_count(&swapped, 0).expect("observation");
    assert!(obs2.five_hour.is_some());
    assert!(obs2.seven_day.is_none());

    let long: serde_json::Value = serde_json::from_str(
        r#"{"type":"token_count","rate_limits":{"primary":{"used_percent":1.0,"window_minutes":1441,"resets_in_seconds":1}}}"#,
    )
    .expect("json");
    let obs3 = RateLimitObservation::from_codex_token_count(&long, 0).expect("observation");
    assert!(obs3.five_hour.is_none());
    assert!(obs3.seven_day.is_some());
}

#[test]
fn codex_token_count_wrong_type_or_missing_rate_limits_is_none() {
    let wrong_type: serde_json::Value =
        serde_json::from_str(r#"{"type":"item.started"}"#).expect("json");
    assert!(RateLimitObservation::from_codex_token_count(&wrong_type, 0).is_none());

    let no_limits: serde_json::Value =
        serde_json::from_str(r#"{"type":"token_count"}"#).expect("json");
    assert!(RateLimitObservation::from_codex_token_count(&no_limits, 0).is_none());

    let empty_limits: serde_json::Value =
        serde_json::from_str(r#"{"type":"token_count","rate_limits":{}}"#).expect("json");
    assert!(RateLimitObservation::from_codex_token_count(&empty_limits, 0).is_none());
}

// ---- opencode go usage（ADR 2026-10-06 D2） ----

#[test]
fn opencode_go_usage_parses_all_three_windows() {
    let body = r#"{"usage":{"rolling":{"status":"ok","percent":12,"resetsAt":"2026-10-06T12:00:00.000Z"},"weekly":{"status":"ok","percent":40,"resetsAt":"2026-10-08T00:00:00Z"},"monthly":{"status":"ok","percent":55,"resetsAt":"2026-11-01T00:00:00Z"}}}"#;
    let obs = parse_opencode_go_usage(body, 7).expect("obs");
    assert_eq!(obs.observed_at, 7);
    let five = obs.five_hour.expect("five");
    assert!((five.utilization - 0.12).abs() < 1e-9);
    assert_eq!(five.resets_at, 1_791_288_000);
    assert!((obs.seven_day.expect("week").utilization - 0.4).abs() < 1e-9);
    assert!((obs.one_month.expect("month").utilization - 0.55).abs() < 1e-9);
}

#[test]
fn opencode_go_usage_missing_window_is_none_and_rate_limited_is_full() {
    let body = r#"{"usage":{"rolling":{"status":"rate-limited","percent":80,"resetsAt":"2026-10-06T12:00:00Z"},"weekly":{"status":"ok","percent":"x","resetsAt":"2026-10-08T00:00:00Z"}}}"#;
    let obs = parse_opencode_go_usage(body, 0).expect("obs");
    assert_eq!(obs.five_hour.expect("five").utilization, 1.0);
    assert!(obs.seven_day.is_none(), "unparsable window is unknown");
    assert!(obs.one_month.is_none());
    let over =
        r#"{"usage":{"monthly":{"status":"ok","percent":250,"resetsAt":"2026-11-01T00:00:00Z"}}}"#;
    assert_eq!(
        parse_opencode_go_usage(over, 0)
            .expect("obs")
            .one_month
            .expect("m")
            .utilization,
        1.0
    );
}

#[test]
fn opencode_go_usage_garbage_is_err() {
    assert!(parse_opencode_go_usage("not json", 0).is_err());
    assert!(parse_opencode_go_usage("{}", 0).is_err());
    assert!(parse_opencode_go_usage(r#"{"usage":{}}"#, 0).is_err());
}

#[test]
fn codex_month_window_goes_to_one_month() {
    let result = serde_json::json!({"rateLimits": {
        "primary": {"usedPercent": 10, "windowDurationMins": 300, "resetsAt": 100},
        "secondary": {"usedPercent": 20, "windowDurationMins": 43200, "resetsAt": 900}
    }});
    let obs = RateLimitObservation::from_codex_account_limits(&result, 0).expect("obs");
    assert!(obs.five_hour.is_some() && obs.seven_day.is_none());
    assert_eq!(obs.one_month.expect("month").resets_at, 900);
}

#[test]
fn account_pool_setting_accepts_bool_or_adapter_name() {
    #[derive(Deserialize)]
    struct Row {
        account_pool: AccountPoolSetting,
    }
    let parse = |t: &str| toml::from_str::<Row>(t).map(|r| r.account_pool);
    assert_eq!(
        parse("account_pool = false").ok(),
        Some(AccountPoolSetting::Off)
    );
    assert_eq!(
        parse("account_pool = true").ok(),
        Some(AccountPoolSetting::On)
    );
    assert_eq!(
        parse("account_pool = \"opencode-go\"").ok(),
        Some(AccountPoolSetting::Adapter(AccountAdapter::OpencodeGo))
    );
    assert!(parse("account_pool = \"nope\"").is_err());
    assert_eq!(
        AccountPoolSetting::On.pool_adapter("codex"),
        Some(AccountAdapter::Codex)
    );
    assert_eq!(AccountPoolSetting::On.pool_adapter("acp"), None);
    assert_eq!(
        AccountPoolSetting::Adapter(AccountAdapter::OpencodeGo).pool_adapter("acp"),
        Some(AccountAdapter::OpencodeGo)
    );
    assert_eq!(
        serde_json::to_string(&AccountPoolSetting::Adapter(AccountAdapter::OpencodeGo))
            .expect("json"),
        "\"opencode-go\""
    );
    assert_eq!(
        serde_json::to_string(&AccountPoolSetting::On).expect("json"),
        "true"
    );
}
