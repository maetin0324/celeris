//! アカウントのレート制限の観測値（ADR-0024 D4, ADR-0025 D1/D3）。
//!
//! Claude Code / codex が stream-json の実測値をそのまま持つための型。推定はしない。
//! タスクの真実（DB・イベント）ではなく観測値で、replay の対象外。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// アカウントプールのアダプタの種類（ADR-0025 D1）。`(adapter, id)` でアカウントを識別する。
///
/// task-core に置くのは、task-dispatch / task-worker / task-api / celeris のいずれからも参照できる
/// 基底クレートだからで、依存を増やさない（ADR-0025 の指示）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AccountAdapter {
    ClaudeCode,
    Codex,
    /// opencode go の subscription（ADR 2026-10-06 D2）。
    OpencodeGo,
}

impl AccountAdapter {
    pub const ALL: [AccountAdapter; 3] = [
        AccountAdapter::ClaudeCode,
        AccountAdapter::Codex,
        AccountAdapter::OpencodeGo,
    ];

    /// `"claude-code"` / `"codex"`（設定の `adapter` や API の `?adapter=` と同じ文字列）。
    pub fn as_str(self) -> &'static str {
        match self {
            AccountAdapter::ClaudeCode => "claude-code",
            AccountAdapter::Codex => "codex",
            AccountAdapter::OpencodeGo => "opencode-go",
        }
    }

    /// 設定・クエリの文字列から解決する。既知でなければ `None`。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "claude-code" => Some(AccountAdapter::ClaudeCode),
            "codex" => Some(AccountAdapter::Codex),
            "opencode-go" => Some(AccountAdapter::OpencodeGo),
            _ => None,
        }
    }

    /// ログイン済みかどうかを示すファイル名（ADR-0025 D1: 中身は読まない、存在だけを見る）。
    pub fn credentials_marker(self) -> &'static str {
        match self {
            AccountAdapter::ClaudeCode => ".credentials.json",
            AccountAdapter::Codex => "auth.json",
            AccountAdapter::OpencodeGo => "opencode/auth.json",
        }
    }

    /// プールで選んだアカウントの根ディレクトリを渡す環境変数名（ADR-0025 D2）。
    pub fn env_var(self) -> &'static str {
        match self {
            AccountAdapter::ClaudeCode => "CLAUDE_SECURESTORAGE_CONFIG_DIR",
            AccountAdapter::Codex => "CODEX_HOME",
            AccountAdapter::OpencodeGo => "XDG_DATA_HOME",
        }
    }
}

impl std::fmt::Display for AccountAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// provider 行の `account_pool`（ADR-0024 D2 / ADR 2026-10-06 D2）。
///
/// `false` / `true` に加え、pool の adapter 名（`"opencode-go"` 等）を文字列で書ける。`true` は
/// 「行の adapter と同じ名前の pool」（claude-code / codex）。ACP のように行の adapter が pool の
/// adapter と一致しない行は、pool の名前を文字列で明示する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountPoolSetting {
    /// pool を使わない。
    #[default]
    Off,
    /// 行の adapter と同名の pool。
    On,
    /// 名前で指定した pool。
    Adapter(AccountAdapter),
}

impl AccountPoolSetting {
    /// pool を使う行か。
    pub fn is_on(self) -> bool {
        !matches!(self, AccountPoolSetting::Off)
    }

    /// 行の adapter 名 `row_adapter` に対する pool の adapter。`Off` や解決できなければ `None`。
    pub fn pool_adapter(self, row_adapter: &str) -> Option<AccountAdapter> {
        match self {
            AccountPoolSetting::Off => None,
            AccountPoolSetting::On => AccountAdapter::parse(row_adapter),
            AccountPoolSetting::Adapter(a) => Some(a),
        }
    }
}

impl From<bool> for AccountPoolSetting {
    fn from(on: bool) -> Self {
        if on {
            AccountPoolSetting::On
        } else {
            AccountPoolSetting::Off
        }
    }
}

impl Serialize for AccountPoolSetting {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            AccountPoolSetting::Off => serializer.serialize_bool(false),
            AccountPoolSetting::On => serializer.serialize_bool(true),
            AccountPoolSetting::Adapter(a) => serializer.serialize_str(a.as_str()),
        }
    }
}

impl<'de> Deserialize<'de> for AccountPoolSetting {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Flag(bool),
            Name(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Flag(on) => Ok(on.into()),
            Raw::Name(name) => AccountAdapter::parse(&name)
                .map(AccountPoolSetting::Adapter)
                .ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "unknown account_pool {name:?} (true, false, \"claude-code\", \"codex\" or \"opencode-go\")"
                    ))
                }),
        }
    }
}

/// 1 つの枠（5 時間 / 7 日 / 1 か月）の観測値。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RateWindow {
    /// 0.0〜1.0（`unifiedWindows.<w>.utilization`）
    pub utilization: f64,
    /// 枠がリセットされる時刻（Unix 秒、`resetsAt`）
    pub resets_at: i64,
}

/// `rate_limit_event` 1 行分の観測値。枠は欠けることがある。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RateLimitObservation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<RateWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<RateWindow>,
    /// 1 か月窓（opencode go の `monthly`。ADR 2026-10-06 D1）。観測できなければ `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_month: Option<RateWindow>,
    /// `rate_limit_info.status`（`allowed` / `allowed_warning` / `rejected` 等。未知の値もそのまま）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// `rate_limit_info.resetsAt`（`status` が指す枠のリセット時刻、Unix 秒）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// 観測した時刻（Unix 秒。celeris の壁時計）
    pub observed_at: i64,
}

impl RateLimitObservation {
    /// stream-json の 1 行（`{"type":"rate_limit_event","rate_limit_info":{...}}`）を解析する。
    /// `rate_limit_event` でない、または枠も status も無ければ `None`。
    pub fn from_stream_json(line: &serde_json::Value, observed_at: i64) -> Option<Self> {
        if line.get("type").and_then(|v| v.as_str()) != Some("rate_limit_event") {
            return None;
        }
        let info = line.get("rate_limit_info")?;
        let window = |name: &str| -> Option<RateWindow> {
            let w = info.get("unifiedWindows")?.get(name)?;
            Some(RateWindow {
                utilization: w.get("utilization")?.as_f64()?,
                resets_at: w.get("resetsAt")?.as_i64()?,
            })
        };
        let obs = Self {
            five_hour: window("five_hour"),
            seven_day: window("seven_day"),
            one_month: None,
            status: info
                .get("status")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            resets_at: info.get("resetsAt").and_then(|v| v.as_i64()),
            observed_at,
        };
        if obs.five_hour.is_none() && obs.seven_day.is_none() && obs.status.is_none() {
            return None;
        }
        Some(obs)
    }

    /// Codex rollout の観測値。リセット時刻は絶対秒と相対秒を区別する。
    pub fn from_codex_token_count(line: &serde_json::Value, observed_at: i64) -> Option<Self> {
        if line.get("type").and_then(|v| v.as_str()) != Some("token_count") {
            return None;
        }
        Self::from_codex_limits(line.get("rate_limits")?, observed_at)
    }

    /// account/rateLimits/read の result。複数 bucket があれば Codex の枠だけを読む。
    pub fn from_codex_account_limits(result: &serde_json::Value, observed_at: i64) -> Option<Self> {
        let limits = result
            .get("rateLimitsByLimitId")
            .and_then(|buckets| buckets.get("codex"))
            .or_else(|| result.get("rateLimits"))?;
        if limits
            .get("limitId")
            .and_then(|v| v.as_str())
            .is_some_and(|id| id != "codex")
        {
            return None;
        }
        Self::from_codex_limits(limits, observed_at)
    }

    fn from_codex_limits(limits: &serde_json::Value, observed_at: i64) -> Option<Self> {
        let mut obs = Self {
            five_hour: None,
            seven_day: None,
            one_month: None,
            status: None,
            resets_at: None,
            observed_at,
        };
        for key in ["primary", "secondary"] {
            if let Some((window, kind)) = limits.get(key).and_then(|w| codex_window(w, observed_at))
            {
                match kind {
                    CodexWindowKind::Short => obs.five_hour = Some(window),
                    CodexWindowKind::Long => obs.seven_day = Some(window),
                    CodexWindowKind::Month => obs.one_month = Some(window),
                }
            }
        }
        (obs.five_hour.is_some() || obs.seven_day.is_some() || obs.one_month.is_some())
            .then_some(obs)
    }
}

/// codex の枠を窓の長さで分けた種別。
enum CodexWindowKind {
    /// 1440 分以下（5 時間窓）
    Short,
    /// 1440 分超 〜 7 日の 1.5 倍以下（週窓）
    Long,
    /// 7 日の 1.5 倍超（月窓）
    Month,
}

fn codex_window(
    value: &serde_json::Value,
    observed_at: i64,
) -> Option<(RateWindow, CodexWindowKind)> {
    let used_percent = value
        .get("usedPercent")
        .or_else(|| value.get("used_percent"))?
        .as_f64()?;
    let minutes = value
        .get("windowDurationMins")
        .or_else(|| value.get("window_minutes"))?
        .as_i64()?;
    if !used_percent.is_finite() || !(0.0..=100.0).contains(&used_percent) || minutes <= 0 {
        return None;
    }
    let resets_at = if let Some(absolute) = value.get("resetsAt").or_else(|| value.get("resets_at"))
    {
        absolute.as_i64()?
    } else {
        let seconds = value
            .get("resets_in_seconds")
            .or_else(|| value.get("reset_after_seconds"))?
            .as_i64()?;
        if seconds < 0 {
            return None;
        }
        observed_at.checked_add(seconds)?
    };
    if resets_at < 0 {
        return None;
    }
    Some((
        RateWindow {
            utilization: used_percent / 100.0,
            resets_at,
        },
        if minutes <= 1440 {
            CodexWindowKind::Short
        } else if minutes > 7 * 24 * 60 * 3 / 2 {
            CodexWindowKind::Month
        } else {
            CodexWindowKind::Long
        },
    ))
}

/// opencode go の `GET /zen/go/v1/usage` の本文を観測値にする（ADR 2026-10-06 D2）。
///
/// `rolling → five_hour`、`weekly → seven_day`、`monthly → one_month`。`percent / 100` を
/// 0..1 に丸めて utilization にし、`status == "rate-limited"` の窓は 1.0。`resetsAt`（RFC3339）は
/// Unix 秒にする。窓が無い・解析できない場合はその窓だけ `None`（0 にしない）。
/// JSON でない、`usage` が無い、どの窓も読めない場合は `Err`。
pub fn parse_opencode_go_usage(body: &str, now: i64) -> Result<RateLimitObservation, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("usage 応答が JSON ではない: {e}"))?;
    let usage = value
        .get("usage")
        .filter(|u| u.is_object())
        .ok_or_else(|| "usage 応答に usage が無い".to_owned())?;
    let window = |name: &str| -> Option<RateWindow> {
        let w = usage.get(name)?;
        let percent = w.get("percent")?.as_f64()?;
        let limited = w.get("status").and_then(|s| s.as_str()) == Some("rate-limited");
        let resets_at = time::OffsetDateTime::parse(
            w.get("resetsAt")?.as_str()?,
            &time::format_description::well_known::Rfc3339,
        )
        .ok()?
        .unix_timestamp();
        let utilization = if limited || percent.is_nan() {
            if limited { 1.0 } else { return None }
        } else {
            (percent / 100.0).clamp(0.0, 1.0)
        };
        Some(RateWindow {
            utilization,
            resets_at,
        })
    };
    let obs = RateLimitObservation {
        five_hour: window("rolling"),
        seven_day: window("weekly"),
        one_month: window("monthly"),
        status: None,
        resets_at: None,
        observed_at: now,
    };
    if obs.five_hour.is_none() && obs.seven_day.is_none() && obs.one_month.is_none() {
        return Err("usage 応答にどの窓も読めない".to_owned());
    }
    Ok(obs)
}

#[cfg(test)]
mod tests {
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

        let other: serde_json::Value =
            serde_json::from_str(r#"{"type":"assistant"}"#).expect("json");
        assert!(RateLimitObservation::from_stream_json(&other, 1).is_none());
        let empty: serde_json::Value =
            serde_json::from_str(r#"{"type":"rate_limit_event","rate_limit_info":{}}"#)
                .expect("json");
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
        let over = r#"{"usage":{"monthly":{"status":"ok","percent":250,"resetsAt":"2026-11-01T00:00:00Z"}}}"#;
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
}
