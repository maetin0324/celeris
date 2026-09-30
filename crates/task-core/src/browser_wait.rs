//! ADR-0080 D4/D5: browser の人待ち（credential の登録依頼・credential 使用/高リスク操作の承認）の耐久記録。
//!
//! - task の Status は増やさない。待っている task は `Blocked`、wait の `reason` が
//!   `waiting_for_auth` / `waiting_for_approval` を運ぶ（`BrowserRunState` に写すのは呼び出し側）。
//! - wait の作成・解決・終端化は task の遷移と `Event` と**同じトランザクション**で確定する。
//! - 解決は `version` の CAS と `resume_key` で重複を排除する。期限切れは一度だけ終端化する。
//! - **秘密（username/password・鍵・lease の中身・dashboard token）はどの列・どの event にも置かない**。
//!   登録された秘密は celeris-credentiald の vault だけにあり、ここには credential の参照だけが入る。
//!
//! 実装は `SqliteStore` のみ（`TaskStore` の supertrait）。LLM は呼ばない。

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

use crate::browser::BrowserTaskPolicy;
use crate::store::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};
use crate::{Event, Status, TaskId, Trigger};

/// 登録待ちの既定・上限（ADR-0080 D4: 既定 24 時間、管理者上限 24 時間）。
pub const AUTH_WAIT_MAX_SECS: u64 = 24 * 60 * 60;
/// 承認待ちの既定・上限（ADR-0080 D4: 既定・上限 5 分）。
pub const APPROVAL_WAIT_MAX_SECS: u64 = 5 * 60;
/// 目的（`purpose`）の最大文字数（ADR-0080 D5）。
pub const PURPOSE_MAX_CHARS: usize = 500;
/// 識別子・digest・nonce などの最大長。
pub const TOKEN_MAX_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserWaitReason {
    WaitingForAuth,
    WaitingForApproval,
}

impl BrowserWaitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WaitingForAuth => "waiting_for_auth",
            Self::WaitingForApproval => "waiting_for_approval",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "waiting_for_auth" => Some(Self::WaitingForAuth),
            "waiting_for_approval" => Some(Self::WaitingForApproval),
            _ => None,
        }
    }

    /// 対応する `BrowserRunState`。
    pub fn run_state(self) -> crate::BrowserRunState {
        match self {
            Self::WaitingForAuth => crate::BrowserRunState::WaitingForAuth,
            Self::WaitingForApproval => crate::BrowserRunState::WaitingForApproval,
        }
    }

    fn max_secs(self) -> u64 {
        match self {
            Self::WaitingForAuth => AUTH_WAIT_MAX_SECS,
            Self::WaitingForApproval => APPROVAL_WAIT_MAX_SECS,
        }
    }
}

/// wait の状態。`pending` と `approved`（未消費）が「開いている」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserWaitState {
    Pending,
    /// 登録依頼に人が credential を登録した（使用承認は兼ねない）。
    Registered,
    /// 人が一回だけの実行を承認した（未消費）。
    Approved,
    /// 承認の一回の実行権を worker が消費した。
    Resumed,
    Denied,
    Expired,
    Cancelled,
    Revoked,
    Invalidated,
}

impl BrowserWaitState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Registered => "registered",
            Self::Approved => "approved",
            Self::Resumed => "resumed",
            Self::Denied => "denied",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Revoked => "revoked",
            Self::Invalidated => "invalidated",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "registered" => Self::Registered,
            "approved" => Self::Approved,
            "resumed" => Self::Resumed,
            "denied" => Self::Denied,
            "expired" => Self::Expired,
            "cancelled" => Self::Cancelled,
            "revoked" => Self::Revoked,
            "invalidated" => Self::Invalidated,
            _ => return None,
        })
    }

    pub fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::Approved)
    }
}

/// credential の参照（ADR-0080 D2 `CredentialRef`）。秘密ではない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialRef {
    pub credential_id: String,
    pub provider: String,
    pub policy_id: String,
}

/// 承認を求める操作 intent（内部 action 名と、非秘密の引数 digest）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationIntent {
    pub intent_id: String,
    /// agent-browser の内部 action 名（例: `click`・`download`・`credential_use`）。
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_digest: Option<String>,
}

/// 耐久の wait 1 件（ADR-0080 D4 `BrowserWait`）。**秘密を持たない**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserWait {
    pub wait_id: String,
    pub task_id: TaskId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_unit_id: Option<String>,
    pub run_id: String,
    pub session_id: String,
    pub reason: BrowserWaitReason,
    /// trusted exact HTTPS origin。
    pub origin: String,
    /// 何のためか（untrusted な plain text として表示する）。
    pub purpose: String,
    /// 登録待ちが要求する credential policy、または承認待ちが使う credential の policy。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_policy_id: Option<String>,
    /// 登録済み（または使用する）credential の参照。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<OperationIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    pub policy_revision: u64,
    pub policy_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub deadline: OffsetDateTime,
    pub resume_key: String,
    pub version: u64,
    pub state: BrowserWaitState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_code: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub created_at: OffsetDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    #[schemars(with = "Option<String>")]
    pub resolved_at: Option<OffsetDateTime>,
}

/// wait を開く要求（trusted supervisor が組む）。秘密の欄は無い。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewBrowserWait {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_unit_id: Option<String>,
    pub run_id: String,
    pub session_id: String,
    pub reason: BrowserWaitReason,
    pub origin: String,
    pub purpose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<OperationIntent>,
    pub policy_revision: u64,
    pub policy_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    /// 待つ秒数。省略・上限超えは reason ごとの上限に丸める。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_secs: Option<u64>,
    pub resume_key: String,
}

/// 登録された credential の台帳行（ADR-0080 D3: id・provider・policy・origin だけ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialRecord {
    pub credential_id: String,
    pub provider: String,
    pub policy_id: String,
    pub credential_revision: u64,
    pub origin: String,
    /// broker が発行した登録 receipt（秘密ではない）。
    pub receipt_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDecision {
    ApproveOnce,
    Deny,
    Revoke,
}

impl BrowserDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApproveOnce => "approve_once",
            Self::Deny => "deny",
            Self::Revoke => "revoke",
        }
    }
}

/// 人の決定（署名付き human attestation を API 層が検証した後の値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanDecision {
    pub decision: BrowserDecision,
    pub expected_version: u64,
    pub actor_id: String,
    pub owner_session_hash: String,
    pub policy_hash: String,
    pub nonce: String,
    pub idempotency_key: String,
}

/// 決定の記録（`browser_approvals` の 1 行）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BrowserApprovalRecord {
    pub approval_id: String,
    pub wait_id: String,
    pub task_id: TaskId,
    pub decision: BrowserDecision,
    pub actor_id: String,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    pub decided_at: OffsetDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    #[schemars(with = "Option<String>")]
    pub consumed_at: Option<OffsetDateTime>,
}

/// `browser_wait_open` の結果。`created = false` は同じ `resume_key` の再送。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserWaitOpen {
    pub wait: BrowserWait,
    pub created: bool,
}

/// 解決系の結果。`replayed = true` は同じ決定・同じ登録の冪等な再送（何も書いていない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserWaitResolution {
    pub wait: BrowserWait,
    pub task_status: Status,
    pub replayed: bool,
}

/// wait の操作が断られた理由（固定コード。API は status に写す）。
#[derive(Debug, thiserror::Error)]
pub enum BrowserWaitError {
    #[error("browser wait not found")]
    NotFound,
    /// 入力の検証に落ちた（API は 422）。`field` は欄の名前だけで、値を含めない。
    #[error("invalid browser wait field: {field}")]
    Invalid { field: &'static str },
    /// `expected_version` が合わない（409）。
    #[error("browser wait version conflict")]
    VersionConflict,
    /// その状態・種類の wait には許されない操作（409）。
    #[error("browser wait is not in a state that allows {op}")]
    InvalidState { op: &'static str },
    /// 期限切れ・失効（410）。
    #[error("browser wait expired or revoked")]
    Gone,
    /// attestation の nonce の再利用（403）。
    #[error("attestation nonce already used")]
    Replay,
    /// 対象の task が待てる状態に無い（409）。
    #[error("task is not running")]
    TaskNotRunning,
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl From<rusqlite::Error> for BrowserWaitError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(StoreError::Sqlite(e))
    }
}

impl BrowserWaitError {
    /// API の problem の `code`（固定の語）。
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "browser_wait_not_found",
            Self::Invalid { .. } => "browser_wait_invalid",
            Self::VersionConflict => "browser_wait_version_conflict",
            Self::InvalidState { .. } => "browser_wait_state",
            Self::Gone => "browser_wait_expired",
            Self::Replay => "attestation_replayed",
            Self::TaskNotRunning => "task_not_running",
            Self::Store(_) => "internal",
        }
    }
}

/// `BrowserWait` の読み書き。`TaskStore` の supertrait。
pub trait BrowserWaitStore: Send + Sync {
    /// A missing policy is a deny. Only a trusted administrator may write this record.
    fn browser_task_policy_get(
        &self,
        task_id: TaskId,
    ) -> Result<Option<BrowserTaskPolicy>, StoreError>;
    fn browser_task_policy_set(
        &self,
        task_id: TaskId,
        policy: &BrowserTaskPolicy,
    ) -> Result<(), StoreError>;
    /// running の task に wait を開く（task → `Blocked`、worker の lease を解放、`BrowserWaitOpened`）。
    /// 同じ `resume_key` の再送は既存の wait を返す（`created = false`）。
    fn browser_wait_open(
        &self,
        task_id: TaskId,
        new: &NewBrowserWait,
        now: OffsetDateTime,
    ) -> Result<BrowserWaitOpen, BrowserWaitError>;
    fn browser_wait_get(&self, wait_id: &str) -> Result<Option<BrowserWait>, StoreError>;
    /// task の wait（新しい順ではなく作成順）。
    fn browser_waits_for_task(&self, task_id: TaskId) -> Result<Vec<BrowserWait>, StoreError>;
    /// 人の対応を待つ wait（`pending`）の全件。inbox・承認一覧に出す。作成順。
    fn browser_waits_pending(&self) -> Result<Vec<BrowserWait>, StoreError>;
    /// 登録待ちに credential の参照を結び付けて一度だけ解決する（task → `Ready`）。使用承認は兼ねない。
    fn browser_wait_register(
        &self,
        task_id: TaskId,
        wait_id: &str,
        expected_version: u64,
        credential: &CredentialRecord,
        actor_id: &str,
        now: OffsetDateTime,
    ) -> Result<BrowserWaitResolution, BrowserWaitError>;
    /// 人の `approve_once` / `deny` / `revoke`。
    fn browser_wait_decide(
        &self,
        task_id: TaskId,
        wait_id: &str,
        decision: &HumanDecision,
        now: OffsetDateTime,
    ) -> Result<BrowserWaitResolution, BrowserWaitError>;
    /// worker が承認済みの wait の一回の実行権を消費する（同じ run/session・`resume_key` だけ）。
    /// 期限切れなら `expired` にして `Gone`。
    fn browser_wait_consume(
        &self,
        task_id: TaskId,
        wait_id: &str,
        resume_key: &str,
        run_id: &str,
        session_id: &str,
        now: OffsetDateTime,
    ) -> Result<BrowserWait, BrowserWaitError>;
    /// 期限の過ぎた開いている wait を一度だけ終端化する（`pending` は task を `Failed` に）。
    /// daemon の起動時と tick で呼ぶ。終端化した wait を返す。
    fn browser_waits_expire(&self, now: OffsetDateTime) -> Result<Vec<BrowserWait>, StoreError>;
    fn browser_approvals_for_wait(
        &self,
        wait_id: &str,
    ) -> Result<Vec<BrowserApprovalRecord>, StoreError>;
    fn browser_credential_get(
        &self,
        credential_id: &str,
    ) -> Result<Option<CredentialRecord>, StoreError>;
}

/// 承認済み credential 使用の一回消費の結果（trusted supervisor が broker へ lease を求める材料）。
/// 秘密は持たない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumedBrowserApproval {
    pub wait: BrowserWait,
    pub credential: CredentialRecord,
    pub approved_by: String,
}

/// 承認済みの credential 使用 wait を、それを開いた論理 run/session の continuation として一度だけ消費する。
/// dispatch ごとの run id は変わるので、continuation の照合には wait 自身の run/session を使う
/// （lease と browser session もその session に結び付く）。承認記録と credential 台帳が揃わなければ拒否する。
pub fn consume_credential_approval<S: BrowserWaitStore + ?Sized>(
    store: &S,
    task_id: TaskId,
    wait: &BrowserWait,
    now: OffsetDateTime,
) -> Result<ConsumedBrowserApproval, &'static str> {
    if wait.reason != BrowserWaitReason::WaitingForApproval
        || wait
            .operation
            .as_ref()
            .is_none_or(|o| o.action != "credential_use")
    {
        return Err("browser approval is not a credential use");
    }
    let consumed = store
        .browser_wait_consume(
            task_id,
            &wait.wait_id,
            &wait.resume_key,
            &wait.run_id,
            &wait.session_id,
            now,
        )
        .map_err(|e| e.code())?;
    let reference = consumed
        .credential
        .clone()
        .ok_or("approved credential reference missing")?;
    let credential = store
        .browser_credential_get(&reference.credential_id)
        .map_err(|_| "browser credential store unavailable")?
        .filter(|c| {
            c.provider == reference.provider
                && c.policy_id == reference.policy_id
                && c.origin == consumed.origin
        })
        .ok_or("approved credential record missing")?;
    let approval_id = consumed
        .approval_id
        .clone()
        .ok_or("approval record missing")?;
    let approved_by = store
        .browser_approvals_for_wait(&consumed.wait_id)
        .map_err(|_| "browser approval store unavailable")?
        .into_iter()
        .find(|a| {
            a.approval_id == approval_id
                && a.decision == BrowserDecision::ApproveOnce
                && a.consumed_at.is_some()
        })
        .map(|a| a.actor_id)
        .ok_or("approval record missing")?;
    Ok(ConsumedBrowserApproval {
        wait: consumed,
        credential,
        approved_by,
    })
}

// ---- 検証 ----

fn valid_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= TOKEN_MAX_LEN
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'-'))
}

fn check_token(s: &str, field: &'static str) -> Result<(), BrowserWaitError> {
    if valid_token(s) {
        Ok(())
    } else {
        Err(BrowserWaitError::Invalid { field })
    }
}

fn valid_dns_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

/// canonical exact HTTPS origin（`https://host` か `https://host:port`、port は 443 以外）。
/// 大文字・userinfo・path・query・fragment・wildcard・既定 port の明示は受け付けない
/// （呼び出し側が canonical にしてから渡す）。
pub fn valid_exact_origin(origin: &str) -> bool {
    let Some(authority) = origin.strip_prefix("https://") else {
        return false;
    };
    let host = match authority.split_once(':') {
        Some((host, port)) => {
            if port.is_empty()
                || port.starts_with('0')
                || !port.bytes().all(|c| c.is_ascii_digit())
                || !matches!(port.parse::<u16>(), Ok(p) if p != 443 && p != 0)
            {
                return false;
            }
            host
        }
        None => authority,
    };
    valid_dns_host(host)
}

/// 目的は 1〜500 文字の plain text（制御文字なし）。
pub fn valid_purpose(purpose: &str) -> bool {
    let n = purpose.chars().count();
    (1..=PURPOSE_MAX_CHARS).contains(&n)
        && !purpose.trim().is_empty()
        && !purpose.chars().any(char::is_control)
}

fn check_credential_ref(c: &CredentialRef) -> Result<(), BrowserWaitError> {
    check_token(&c.credential_id, "credential.credential_id")?;
    check_token(&c.provider, "credential.provider")?;
    check_token(&c.policy_id, "credential.policy_id")
}

impl NewBrowserWait {
    pub fn validate(&self) -> Result<(), BrowserWaitError> {
        if let Some(wu) = &self.work_unit_id {
            check_token(wu, "work_unit_id")?;
        }
        check_token(&self.run_id, "run_id")?;
        check_token(&self.session_id, "session_id")?;
        check_token(&self.resume_key, "resume_key")?;
        check_token(&self.policy_hash, "policy_hash")?;
        if let Some(owner) = &self.owner_id {
            check_token(owner, "owner_id")?;
        }
        if !valid_exact_origin(&self.origin) {
            return Err(BrowserWaitError::Invalid { field: "origin" });
        }
        if !valid_purpose(&self.purpose) {
            return Err(BrowserWaitError::Invalid { field: "purpose" });
        }
        if let Some(p) = &self.credential_policy_id {
            check_token(p, "credential_policy_id")?;
        }
        if let Some(c) = &self.credential {
            check_credential_ref(c)?;
        }
        if let Some(op) = &self.operation {
            check_token(&op.intent_id, "operation.intent_id")?;
            check_token(&op.action, "operation.action")?;
            if let Some(d) = &op.args_digest {
                check_token(d, "operation.args_digest")?;
            }
        }
        match self.reason {
            // 登録依頼: どの policy の credential が要るかは必須。登録済みの参照や操作は持たない。
            BrowserWaitReason::WaitingForAuth => {
                if self.credential_policy_id.is_none() {
                    return Err(BrowserWaitError::Invalid {
                        field: "credential_policy_id",
                    });
                }
                if self.credential.is_some() || self.operation.is_some() {
                    return Err(BrowserWaitError::Invalid {
                        field: "credential",
                    });
                }
            }
            // 承認待ち: 操作 intent は必須（credential 使用なら credential の参照も要る）。
            BrowserWaitReason::WaitingForApproval => {
                let Some(op) = &self.operation else {
                    return Err(BrowserWaitError::Invalid { field: "operation" });
                };
                if op.action == "credential_use" && self.credential.is_none() {
                    return Err(BrowserWaitError::Invalid {
                        field: "credential",
                    });
                }
            }
        }
        Ok(())
    }

    fn ttl(&self) -> Duration {
        let max = self.reason.max_secs();
        let secs = self.ttl_secs.unwrap_or(max).clamp(1, max);
        Duration::seconds(i64::try_from(secs).unwrap_or(i64::MAX))
    }
}

mod sql;

pub(crate) use sql::{cancel_open_for_task_tx, has_pending_wait_tx};

#[cfg(test)]
#[path = "browser_wait/tests.rs"]
mod tests;
