//! ADR-0082 D2: browser の live proxy（P3-B）の ACL と記録の規則。
//!
//! I/O も時計も持たない。呼び出し側が `now`（UNIX 秒）を渡す。ここが決めるのは
//! 「誰がどの task/run の live を見てよいか」と「何を永続 event に残すか」だけで、
//! 操作の権利（controller lease）は `browser_control`（ADR-0081 D3）が決める。

use serde::{Deserialize, Serialize};

/// live の grant の長さ（秒）。ADR-0080 H2 の lease と同じ短さに揃える。
pub const LIVE_GRANT_TTL_SECS: u64 = 60;
pub const LIVE_CONSOLE_MAX_CHARS: usize = 2000;
pub const LIVE_URL_MAX_CHARS: usize = 2048;
pub const LIVE_TABS_MAX: usize = 32;
pub const REDACTED: &str = "[redacted]";

/// これ以上続く token 様の文字の連続は秘密とみなす。
const TOKEN_RUN_CHARS: usize = 24;
const SENSITIVE_MARKERS: [&str; 12] = [
    "cookie",
    "authorization",
    "bearer",
    "token",
    "secret",
    "password",
    "passwd",
    "session",
    "api_key",
    "apikey",
    "api-key",
    "sid=",
];
const STATUS_WORDS: [&str; 8] = [
    "connecting",
    "connected",
    "reconnecting",
    "disconnected",
    "paused",
    "human_control",
    "stopped",
    "observation_stopped",
];
const CONSOLE_LEVELS: [&str; 5] = ["debug", "log", "info", "warn", "error"];

/// 見る人。ADR-0080 D6 の検査の結果を呼び出し側が詰める。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveViewer {
    pub auth_enabled: bool,
    pub single_owner_instance: bool,
    /// 有効な cookie の session。無ければ未認証。
    pub session_id: Option<String>,
    pub owner_session: bool,
    pub origin_ok: bool,
}

/// browser session の束縛。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveTarget {
    pub task_id: String,
    pub run_id: String,
    pub run_active: bool,
    /// credential を注入した session（ADR-0080 H3）。
    pub credential_interval: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveGrant {
    pub task_id: String,
    pub run_id: String,
    pub session_id: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveDenied {
    LiveViewDisabled,
    Unauthenticated,
    NotOwnerSession,
    OriginMismatch,
    OtherTask,
    RunEnded,
    ObservationStopped,
    GrantExpired,
}

impl LiveDenied {
    pub fn code(&self) -> &'static str {
        match self {
            Self::LiveViewDisabled => "live_view_disabled",
            Self::Unauthenticated => "unauthenticated",
            Self::NotOwnerSession => "not_owner_session",
            Self::OriginMismatch => "origin_mismatch",
            Self::OtherTask => "other_task",
            Self::RunEnded => "run_ended",
            Self::ObservationStopped => "observation_stopped",
            Self::GrantExpired => "grant_expired",
        }
    }

    pub fn http_status(&self) -> u16 {
        match self {
            Self::Unauthenticated => 401,
            Self::RunEnded | Self::GrantExpired => 410,
            _ => 403,
        }
    }
}

fn check_target(target: &LiveTarget) -> Result<(), LiveDenied> {
    if !target.run_active {
        return Err(LiveDenied::RunEnded);
    }
    if target.credential_interval {
        return Err(LiveDenied::ObservationStopped);
    }
    Ok(())
}

/// 接続（と再接続）の入口。`task_id`/`run_id` は要求の経路のもの。
pub fn authorize_live(
    viewer: &LiveViewer,
    target: &LiveTarget,
    task_id: &str,
    run_id: &str,
    now: u64,
) -> Result<LiveGrant, LiveDenied> {
    if !viewer.auth_enabled || !viewer.single_owner_instance {
        return Err(LiveDenied::LiveViewDisabled);
    }
    let Some(session_id) = viewer.session_id.as_deref().filter(|s| !s.is_empty()) else {
        return Err(LiveDenied::Unauthenticated);
    };
    if !viewer.owner_session {
        return Err(LiveDenied::NotOwnerSession);
    }
    if !viewer.origin_ok {
        return Err(LiveDenied::OriginMismatch);
    }
    if target.task_id != task_id || target.run_id != run_id {
        return Err(LiveDenied::OtherTask);
    }
    check_target(target)?;
    Ok(LiveGrant {
        task_id: target.task_id.clone(),
        run_id: target.run_id.clone(),
        session_id: session_id.to_string(),
        expires_at: now.saturating_add(LIVE_GRANT_TTL_SECS),
    })
}

/// 中継のたびに呼ぶ。接続時の 1 回の判定に頼らない。
pub fn check_connection(
    grant: &LiveGrant,
    session_id: &str,
    target: &LiveTarget,
    now: u64,
) -> Result<(), LiveDenied> {
    if grant.session_id != session_id {
        return Err(LiveDenied::NotOwnerSession);
    }
    if grant.task_id != target.task_id || grant.run_id != target.run_id {
        return Err(LiveDenied::OtherTask);
    }
    if now >= grant.expires_at {
        return Err(LiveDenied::GrantExpired);
    }
    check_target(target)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiveResume {
    /// `after_seq` より後の event を送り直す。
    Replay { after_seq: u64 },
    /// 続きを作れない。現在の状態から送り直す。
    Reset { latest_seq: u64 },
}

pub fn resume_plan(last_seen: Option<u64>, oldest_retained: u64, latest_seq: u64) -> LiveResume {
    match last_seen {
        Some(seen) if seen <= latest_seq && seen.saturating_add(1) >= oldest_retained => {
            LiveResume::Replay { after_seq: seen }
        }
        _ => LiveResume::Reset { latest_seq },
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveTab {
    pub id: String,
    pub url: String,
    pub title: String,
}

/// browser から届く live の出来事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveEvent {
    Frame { bytes: Vec<u8> },
    Status { state: String },
    Tabs { tabs: Vec<LiveTab> },
    Url { url: String },
    Console { level: String, text: String },
}

/// 永続 event に残す形。frame と tab の title は持たない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistedLiveEvent {
    Status {
        state: String,
    },
    Tabs {
        count: usize,
        origins: Vec<String>,
    },
    Url {
        url: String,
    },
    Console {
        level: String,
        text: String,
        redacted: bool,
    },
}

fn contains_sensitive(text: &str) -> bool {
    let lower = text.to_lowercase();
    if SENSITIVE_MARKERS.iter().any(|m| lower.contains(m)) || text.contains("eyJ") {
        return true;
    }
    let mut run = 0usize;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '=' | '+' | '%') {
            run += 1;
            if run >= TOKEN_RUN_CHARS {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// http(s) の URL を (scheme, host, path) に分ける。query・fragment・userinfo は捨てる。
fn split_http(url: &str) -> Option<(String, String, &str)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let host = authority.rsplit('@').next().unwrap_or("");
    if host.is_empty() {
        return None;
    }
    Some((scheme, host.to_ascii_lowercase(), path))
}

pub fn scrub_origin(url: &str) -> String {
    match split_http(url) {
        Some((scheme, host, _)) => format!("{scheme}://{host}"),
        None => REDACTED.to_string(),
    }
}

pub fn scrub_url(url: &str) -> String {
    if url.trim() == "about:blank" {
        return "about:blank".to_string();
    }
    let Some((scheme, host, path)) = split_http(url) else {
        return REDACTED.to_string();
    };
    let mut out = format!("{scheme}://{host}");
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        out.push('/');
        if contains_sensitive(segment) {
            out.push_str(REDACTED);
        } else {
            out.push_str(segment);
        }
    }
    out.chars().take(LIVE_URL_MAX_CHARS).collect()
}

/// 永続 event に残す形へ落とす。残さないものは `None`。
pub fn persistable(event: &LiveEvent) -> Option<PersistedLiveEvent> {
    match event {
        LiveEvent::Frame { .. } => None,
        LiveEvent::Status { state } => Some(PersistedLiveEvent::Status {
            state: STATUS_WORDS
                .iter()
                .find(|w| **w == state.as_str())
                .copied()
                .unwrap_or("unknown")
                .to_string(),
        }),
        LiveEvent::Tabs { tabs } => Some(PersistedLiveEvent::Tabs {
            count: tabs.len(),
            origins: tabs
                .iter()
                .take(LIVE_TABS_MAX)
                .map(|t| scrub_origin(&t.url))
                .collect(),
        }),
        LiveEvent::Url { url } => Some(PersistedLiveEvent::Url {
            url: scrub_url(url),
        }),
        LiveEvent::Console { level, text } => {
            let level = CONSOLE_LEVELS
                .iter()
                .find(|l| **l == level.as_str())
                .copied()
                .unwrap_or("log")
                .to_string();
            let redacted = contains_sensitive(text);
            let text = if redacted {
                REDACTED.to_string()
            } else {
                text.chars().take(LIVE_CONSOLE_MAX_CHARS).collect()
            };
            Some(PersistedLiveEvent::Console {
                level,
                text,
                redacted,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> LiveViewer {
        LiveViewer {
            auth_enabled: true,
            single_owner_instance: true,
            session_id: Some("s-owner".into()),
            owner_session: true,
            origin_ok: true,
        }
    }

    fn target(task: &str) -> LiveTarget {
        LiveTarget {
            task_id: task.into(),
            run_id: "r1".into(),
            run_active: true,
            credential_interval: false,
        }
    }

    #[test]
    fn owner_gets_short_grant_bound_to_task_and_run() {
        let grant = authorize_live(&owner(), &target("a"), "a", "r1", 1000).unwrap();
        assert_eq!(grant.expires_at, 1000 + LIVE_GRANT_TTL_SECS);
        assert_eq!((grant.task_id.as_str(), grant.run_id.as_str()), ("a", "r1"));
        assert_eq!(
            check_connection(&grant, "s-owner", &target("a"), 1059),
            Ok(())
        );
    }

    #[test]
    fn viewer_checks_follow_adr_0080_d6() {
        let t = target("a");
        let mut v = owner();
        v.session_id = None;
        let denied = authorize_live(&v, &t, "a", "r1", 0).unwrap_err();
        assert_eq!(
            (denied, denied.http_status()),
            (LiveDenied::Unauthenticated, 401)
        );
        let mut v = owner();
        v.owner_session = false;
        let denied = authorize_live(&v, &t, "a", "r1", 0).unwrap_err();
        assert_eq!(
            (denied, denied.http_status()),
            (LiveDenied::NotOwnerSession, 403)
        );
        let mut v = owner();
        v.single_owner_instance = false;
        assert_eq!(
            authorize_live(&v, &t, "a", "r1", 0),
            Err(LiveDenied::LiveViewDisabled)
        );
        let mut v = owner();
        v.auth_enabled = false;
        assert_eq!(
            authorize_live(&v, &t, "a", "r1", 0),
            Err(LiveDenied::LiveViewDisabled)
        );
        let mut v = owner();
        v.origin_ok = false;
        assert_eq!(
            authorize_live(&v, &t, "a", "r1", 0),
            Err(LiveDenied::OriginMismatch)
        );
    }

    #[test]
    fn other_task_and_other_run_are_denied() {
        assert_eq!(
            authorize_live(&owner(), &target("a"), "b", "r1", 0),
            Err(LiveDenied::OtherTask)
        );
        assert_eq!(
            authorize_live(&owner(), &target("a"), "a", "r2", 0),
            Err(LiveDenied::OtherTask)
        );
        let grant = authorize_live(&owner(), &target("a"), "a", "r1", 0).unwrap();
        assert_eq!(
            check_connection(&grant, "s-owner", &target("b"), 1),
            Err(LiveDenied::OtherTask)
        );
        assert_eq!(
            check_connection(&grant, "s-other", &target("a"), 1),
            Err(LiveDenied::NotOwnerSession)
        );
    }

    #[test]
    fn ended_run_and_expired_grant_are_gone() {
        let mut t = target("a");
        let grant = authorize_live(&owner(), &t, "a", "r1", 0).unwrap();
        let denied = check_connection(&grant, "s-owner", &t, LIVE_GRANT_TTL_SECS).unwrap_err();
        assert_eq!(
            (denied, denied.http_status()),
            (LiveDenied::GrantExpired, 410)
        );
        t.run_active = false;
        assert_eq!(
            check_connection(&grant, "s-owner", &t, 1),
            Err(LiveDenied::RunEnded)
        );
        assert_eq!(
            authorize_live(&owner(), &t, "a", "r1", 0),
            Err(LiveDenied::RunEnded)
        );
    }

    #[test]
    fn credential_interval_stops_new_and_existing_connections() {
        let mut t = target("a");
        let grant = authorize_live(&owner(), &t, "a", "r1", 0).unwrap();
        t.credential_interval = true;
        assert_eq!(
            check_connection(&grant, "s-owner", &t, 1),
            Err(LiveDenied::ObservationStopped)
        );
        assert_eq!(
            authorize_live(&owner(), &t, "a", "r1", 1),
            Err(LiveDenied::ObservationStopped)
        );
    }

    #[test]
    fn reconnect_replays_or_resets() {
        assert_eq!(
            resume_plan(Some(7), 3, 10),
            LiveResume::Replay { after_seq: 7 }
        );
        assert_eq!(
            resume_plan(Some(10), 3, 10),
            LiveResume::Replay { after_seq: 10 }
        );
        assert_eq!(
            resume_plan(Some(2), 3, 10),
            LiveResume::Replay { after_seq: 2 }
        );
        assert_eq!(
            resume_plan(Some(1), 3, 10),
            LiveResume::Reset { latest_seq: 10 }
        );
        assert_eq!(
            resume_plan(Some(11), 3, 10),
            LiveResume::Reset { latest_seq: 10 }
        );
        assert_eq!(
            resume_plan(None, 3, 10),
            LiveResume::Reset { latest_seq: 10 }
        );
    }

    #[test]
    fn frames_and_tab_titles_are_not_persisted() {
        assert_eq!(persistable(&LiveEvent::Frame { bytes: vec![1, 2] }), None);
        let tabs = LiveEvent::Tabs {
            tabs: vec![LiveTab {
                id: "1".into(),
                url: "https://user:pw@Example.com/inbox?sid=SENTINEL".into(),
                title: "SENTINEL title".into(),
            }],
        };
        assert_eq!(
            persistable(&tabs),
            Some(PersistedLiveEvent::Tabs {
                count: 1,
                origins: vec!["https://example.com".into()]
            })
        );
    }

    #[test]
    fn url_drops_query_fragment_userinfo_and_token_segments() {
        assert_eq!(
            scrub_url("https://u:p@example.com/a/b?token=x#frag"),
            "https://example.com/a/b"
        );
        assert_eq!(
            scrub_url("https://example.com/reset/0123456789abcdef01234567/done"),
            "https://example.com/reset/[redacted]/done"
        );
        assert_eq!(scrub_url("about:blank"), "about:blank");
        assert_eq!(scrub_url("data:text/html,secret"), REDACTED);
        assert_eq!(scrub_url("https://@/x"), REDACTED);
    }

    #[test]
    fn console_with_cookie_or_token_is_replaced_whole() {
        for text in [
            "Set-Cookie: sid=SENTINEL",
            "Authorization: Bearer SENTINEL",
            "jwt eyJhbGciOi.SENTINEL",
            "key 0123456789abcdef01234567 SENTINEL",
        ] {
            let event = LiveEvent::Console {
                level: "error".into(),
                text: text.into(),
            };
            let persisted = persistable(&event).unwrap();
            let json = serde_json::to_string(&persisted).unwrap();
            assert!(!json.contains("SENTINEL"), "{json}");
            assert!(json.contains(REDACTED));
        }
    }

    #[test]
    fn plain_console_is_kept_and_bounded() {
        let event = LiveEvent::Console {
            level: "shout".into(),
            text: "あ ".repeat(LIVE_CONSOLE_MAX_CHARS),
        };
        let Some(PersistedLiveEvent::Console {
            level,
            text,
            redacted,
        }) = persistable(&event)
        else {
            panic!("console is persisted");
        };
        assert_eq!(level, "log");
        assert!(!redacted);
        assert_eq!(text.chars().count(), LIVE_CONSOLE_MAX_CHARS);
    }

    #[test]
    fn status_is_a_fixed_word() {
        let persisted = persistable(&LiveEvent::Status {
            state: "page says SENTINEL".into(),
        });
        assert_eq!(
            persisted,
            Some(PersistedLiveEvent::Status {
                state: "unknown".into()
            })
        );
        let persisted = persistable(&LiveEvent::Status {
            state: "reconnecting".into(),
        });
        assert_eq!(
            persisted,
            Some(PersistedLiveEvent::Status {
                state: "reconnecting".into()
            })
        );
    }
}
