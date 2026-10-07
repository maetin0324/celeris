//! ADR 2026-10-07-browser-trusted-devices: ブラウザの信頼できる端末の型と純粋な規則。
//!
//! 秘密の値は web の外に出ない。ここと store（`store::trusted_devices`、表 `browser_trusted_devices`）が扱うのは
//! web が計算した SHA-256 hex だけで、events にはその hash も載せない（D2・D5）。時刻は UNIX 秒で、呼び手が
//! 注入する（試験は偽の時計で期限を決める）。I/O も LLM 呼び出しも持たない。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::TaskId;

/// 期限の延長幅（人の決定 device-policy: 最後の使用から 90 日）。
pub const TRUSTED_DEVICE_TTL_SECS: i64 = 90 * 24 * 60 * 60;
/// 有効な（未失効・未期限切れの）端末の上限数（人の決定 device-policy）。
pub const TRUSTED_DEVICE_LIMIT: usize = 5;
/// 名前の上限（文字数）。
pub const TRUSTED_DEVICE_NAME_MAX_CHARS: usize = 64;

/// 端末の event を追記する疑似 task の id。events は task ごとの列なので、task に属さない端末の操作は
/// この 1 本に集める（`model_catalog::catalog_event_task_id()` の nil とは別の列）。
pub fn trusted_device_event_task_id() -> TaskId {
    TaskId(ulid::Ulid::from_parts(0, 0x7d))
}

/// 端末の秘密の方式（D1）。今回は `Cookie` だけ。https 化の後に passkey を足す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrustedDeviceMethod {
    Cookie,
}

impl TrustedDeviceMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            TrustedDeviceMethod::Cookie => "cookie",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "cookie" => Some(TrustedDeviceMethod::Cookie),
            _ => None,
        }
    }
}

/// 失効の理由（D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrustedDeviceRevokeReason {
    /// 人が一覧から失効させた。
    Owner,
    /// 回転前の秘密が再提示された（盗用・複製とみなす）。
    Reuse,
}

impl TrustedDeviceRevokeReason {
    pub fn as_str(self) -> &'static str {
        match self {
            TrustedDeviceRevokeReason::Owner => "owner",
            TrustedDeviceRevokeReason::Reuse => "reuse",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(TrustedDeviceRevokeReason::Owner),
            "reuse" => Some(TrustedDeviceRevokeReason::Reuse),
            _ => None,
        }
    }
}

/// 拒否の理由（D4・D5）。HTTP の応答では区別せず、events にだけ残す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrustedDeviceRejectReason {
    /// 未知の端末 id。
    Unknown,
    /// hash が現行とも回転前とも一致しない。
    Mismatch,
    /// 失効済み。
    Revoked,
    /// 期限切れ。
    Expired,
    /// 回転前の秘密の再提示（この拒否と同時に端末を失効させる）。
    Reuse,
    /// 登録の上限超過。
    Limit,
}

impl TrustedDeviceRejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            TrustedDeviceRejectReason::Unknown => "unknown",
            TrustedDeviceRejectReason::Mismatch => "mismatch",
            TrustedDeviceRejectReason::Revoked => "revoked",
            TrustedDeviceRejectReason::Expired => "expired",
            TrustedDeviceRejectReason::Reuse => "reuse",
            TrustedDeviceRejectReason::Limit => "limit",
        }
    }
}

/// 一覧に出す 1 行。hash は持たない（API の応答にも返さない。D2）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrustedDevice {
    pub id: String,
    pub name: String,
    pub method: TrustedDeviceMethod,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub expires_at: i64,
    /// `None` = 絶対上限なし。
    pub absolute_expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub revoked_reason: Option<TrustedDeviceRevokeReason>,
    pub actor: String,
}

impl TrustedDevice {
    /// `now` の時点で使えるか（未失効かつ期限内）。
    pub fn is_active(&self, now: i64) -> bool {
        self.revoked_at.is_none() && !is_expired(self.expires_at, self.absolute_expires_at, now)
    }
}

/// 登録の入力。`secret_hash` は web が計算した SHA-256 hex（64 文字）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTrustedDevice<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub method: TrustedDeviceMethod,
    pub secret_hash: &'a str,
    /// `None` = 絶対上限なし（人の決定 device-policy の既定）。
    pub absolute_expires_at: Option<i64>,
    pub actor: &'a str,
}

/// 検証の結果。拒否は `Err` ではなく値で返す（拒否の event は store が同じ transaction で書く）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustedDeviceVerdict {
    /// 一致した。`verify_and_rotate` では回転・延長済みの行、`verify_readonly` では読んだままの行。
    Accepted(TrustedDevice),
    Rejected(TrustedDeviceRejectReason),
}

/// 期限切れか（`expires_at` ≤ now、または絶対上限があってそれ ≤ now）。
pub fn is_expired(expires_at: i64, absolute_expires_at: Option<i64>, now: i64) -> bool {
    expires_at <= now || absolute_expires_at.is_some_and(|abs| abs <= now)
}

/// 使ったときの新しい期限（`now + TTL`。絶対上限があれば越えない）。
pub fn extended_expiry(now: i64, absolute_expires_at: Option<i64>) -> i64 {
    let sliding = now.saturating_add(TRUSTED_DEVICE_TTL_SECS);
    match absolute_expires_at {
        Some(abs) => sliding.min(abs),
        None => sliding,
    }
}

/// SHA-256 hex（小文字 64 文字）か。
pub fn is_valid_secret_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
