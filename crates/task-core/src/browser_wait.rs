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

// ---- SQL ----

const SELECT_WAIT: &str = "SELECT wait_id, task_id, work_unit_id, run_id, session_id, reason, origin, \
     purpose, credential_id, credential_provider, credential_policy_id, operation_intent_id, action, \
     args_digest, approval_id, policy_revision, policy_hash, owner_id, deadline, resume_key, version, \
     state, resolution_code, created_at, resolved_at FROM browser_waits";

type RawWait = (
    [String; 7],
    [Option<String>; 8],
    (i64, String, Option<String>, String, String, i64, String),
    (Option<String>, String, Option<String>),
);

fn raw_wait(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawWait> {
    Ok((
        [
            row.get(0)?,
            row.get(1)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
        ],
        [
            row.get(2)?,
            row.get(8)?,
            row.get(9)?,
            row.get(10)?,
            row.get(11)?,
            row.get(12)?,
            row.get(13)?,
            row.get(14)?,
        ],
        (
            row.get(15)?,
            row.get(16)?,
            row.get(17)?,
            row.get(18)?,
            row.get(19)?,
            row.get(20)?,
            row.get(21)?,
        ),
        (row.get(22)?, row.get(23)?, row.get(24)?),
    ))
}

fn wait_from_raw(raw: RawWait) -> Result<BrowserWait, StoreError> {
    let (
        [
            wait_id,
            task_id,
            run_id,
            session_id,
            reason,
            origin,
            purpose,
        ],
        opt,
        mid,
        tail,
    ) = raw;
    let [
        work_unit_id,
        credential_id,
        credential_provider,
        credential_policy_id,
        intent_id,
        action,
        args_digest,
        approval_id,
    ] = opt;
    let (policy_revision, policy_hash, owner_id, deadline, resume_key, version, state) = mid;
    let (resolution_code, created_at, resolved_at) = tail;
    let task_id = task_id
        .parse::<TaskId>()
        .map_err(|_| StoreError::Invalid(format!("invalid browser wait task id: {task_id}")))?;
    let reason = BrowserWaitReason::parse(&reason)
        .ok_or_else(|| StoreError::Invalid(format!("invalid browser wait reason: {reason}")))?;
    let state = BrowserWaitState::parse(&state)
        .ok_or_else(|| StoreError::Invalid(format!("invalid browser wait state: {state}")))?;
    let credential = match (
        credential_id,
        credential_provider,
        credential_policy_id.clone(),
    ) {
        (Some(credential_id), Some(provider), Some(policy_id)) => Some(CredentialRef {
            credential_id,
            provider,
            policy_id,
        }),
        _ => None,
    };
    let operation = match (intent_id, action) {
        (Some(intent_id), Some(action)) => Some(OperationIntent {
            intent_id,
            action,
            args_digest,
        }),
        _ => None,
    };
    Ok(BrowserWait {
        wait_id,
        task_id,
        work_unit_id,
        run_id,
        session_id,
        reason,
        origin,
        purpose,
        credential_policy_id,
        credential,
        operation,
        approval_id,
        policy_revision: u64::try_from(policy_revision).unwrap_or(0),
        policy_hash,
        owner_id,
        deadline: parse_rfc3339(&deadline)?,
        resume_key,
        version: u64::try_from(version).unwrap_or(0),
        state,
        resolution_code,
        created_at: parse_rfc3339(&created_at)?,
        resolved_at: resolved_at.as_deref().map(parse_rfc3339).transpose()?,
    })
}

fn query_waits(
    conn: &Connection,
    where_sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<BrowserWait>, StoreError> {
    let mut stmt = conn.prepare(&format!("{SELECT_WAIT} {where_sql}"))?;
    let rows = stmt
        .query_map(args, raw_wait)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(wait_from_raw).collect()
}

fn wait_by_id_tx(conn: &Connection, wait_id: &str) -> Result<Option<BrowserWait>, StoreError> {
    Ok(query_waits(conn, "WHERE wait_id = ?1", &[&wait_id])?
        .into_iter()
        .next())
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// `apply_transition_tx` の迂回防止: 人の対応を待っている（`pending`）wait があるか。
pub(crate) fn has_pending_wait_tx(conn: &Connection, task_id: TaskId) -> Result<bool, StoreError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM browser_waits WHERE task_id = ?1 AND state = 'pending'",
        params![task_id.to_string()],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

/// 状態を CAS で書き換える（`version` が合うときだけ。1 行変わったら `true`）。
fn set_state_tx(
    conn: &Connection,
    wait: &BrowserWait,
    state: BrowserWaitState,
    code: &str,
    now: OffsetDateTime,
) -> Result<bool, StoreError> {
    let changed = conn.execute(
        "UPDATE browser_waits SET state = ?1, resolution_code = ?2, resolved_at = ?3, \
         version = version + 1 WHERE wait_id = ?4 AND version = ?5 AND state = ?6",
        params![
            state.as_str(),
            code,
            format_rfc3339(now)?,
            wait.wait_id,
            to_i64(wait.version),
            wait.state.as_str()
        ],
    )?;
    Ok(changed == 1)
}

fn resolved_event(
    wait: &BrowserWait,
    state: BrowserWaitState,
    code: &str,
    actor_id: Option<&str>,
) -> Event {
    Event::BrowserWaitResolved {
        wait_id: wait.wait_id.clone(),
        reason: wait.reason,
        state,
        code: code.to_string(),
        version: wait.version + 1,
        approval_id: wait.approval_id.clone(),
        credential_id: wait.credential.as_ref().map(|c| c.credential_id.clone()),
        actor_id: actor_id.map(str::to_string),
    }
}

fn task_status_tx(conn: &Connection, task_id: TaskId) -> Result<Option<Status>, StoreError> {
    Ok(SqliteStore::get_locked(conn, task_id)?.map(|t| t.status))
}

/// 期限切れの終端化（`pending` は task を `Failed` に、`approved` は wait だけを失効）。一度だけ。
fn expire_tx(
    conn: &Connection,
    wait: &BrowserWait,
    now: OffsetDateTime,
) -> Result<bool, StoreError> {
    if !set_state_tx(
        conn,
        wait,
        BrowserWaitState::Expired,
        "browser_wait_expired",
        now,
    )? {
        return Ok(false);
    }
    let event = resolved_event(
        wait,
        BrowserWaitState::Expired,
        "browser_wait_expired",
        None,
    );
    if wait.state == BrowserWaitState::Pending
        && task_status_tx(conn, wait.task_id)? == Some(Status::Blocked)
    {
        SqliteStore::apply_transition_tx(
            conn,
            wait.task_id,
            Trigger::BrowserFail { expired: true },
            vec![event],
        )?;
    } else {
        SqliteStore::append_event_tx(conn, wait.task_id, &event)?;
    }
    Ok(true)
}

/// task が終端になったとき（`apply_transition_tx` の中）: 開いている wait を `cancelled` に閉じる。
pub(crate) fn cancel_open_for_task_tx(
    conn: &Connection,
    task_id: TaskId,
    now: OffsetDateTime,
) -> Result<(), StoreError> {
    let open = query_waits(
        conn,
        "WHERE task_id = ?1 AND state IN ('pending', 'approved') ORDER BY created_at, wait_id",
        &[&task_id.to_string()],
    )?;
    for wait in open {
        if set_state_tx(
            conn,
            &wait,
            BrowserWaitState::Cancelled,
            "task_terminal",
            now,
        )? {
            let event = resolved_event(&wait, BrowserWaitState::Cancelled, "task_terminal", None);
            SqliteStore::append_event_tx(conn, task_id, &event)?;
        }
    }
    Ok(())
}

fn check_version_and_deadline(
    conn: &Connection,
    wait: &BrowserWait,
    expected_version: u64,
    now: OffsetDateTime,
) -> Result<(), BrowserWaitError> {
    if wait.version != expected_version {
        return Err(BrowserWaitError::VersionConflict);
    }
    if wait.deadline <= now {
        expire_tx(conn, wait, now)?;
        return Err(BrowserWaitError::Gone);
    }
    Ok(())
}

impl SqliteStore {
    /// 期限切れの判定を commit してから断る（`Gone` を返すときも期限切れの終端化は残す）。
    fn browser_tx<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, BrowserWaitError>,
    ) -> Result<T, BrowserWaitError> {
        let mut conn = self.lock()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StoreError::from)?;
        match f(&tx) {
            Ok(v) => {
                tx.commit().map_err(StoreError::from)?;
                Ok(v)
            }
            Err(BrowserWaitError::Gone) => {
                tx.commit().map_err(StoreError::from)?;
                Err(BrowserWaitError::Gone)
            }
            Err(e) => Err(e),
        }
    }
}

fn load_for_task(
    conn: &Connection,
    task_id: TaskId,
    wait_id: &str,
) -> Result<BrowserWait, BrowserWaitError> {
    match wait_by_id_tx(conn, wait_id)? {
        Some(w) if w.task_id == task_id => Ok(w),
        _ => Err(BrowserWaitError::NotFound),
    }
}

fn approval_rows(
    conn: &Connection,
    where_sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<BrowserApprovalRecord>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT approval_id, wait_id, task_id, decision, actor_id, decided_at, consumed_at \
         FROM browser_approvals {where_sql}"
    ))?;
    let rows = stmt
        .query_map(args, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(
            |(approval_id, wait_id, task_id, decision, actor_id, decided_at, consumed_at)| {
                let decision = match decision.as_str() {
                    "approve_once" => BrowserDecision::ApproveOnce,
                    "deny" => BrowserDecision::Deny,
                    "revoke" => BrowserDecision::Revoke,
                    other => {
                        return Err(StoreError::Invalid(format!(
                            "invalid browser decision: {other}"
                        )));
                    }
                };
                Ok(BrowserApprovalRecord {
                    approval_id,
                    wait_id,
                    task_id: task_id.parse::<TaskId>().map_err(|_| {
                        StoreError::Invalid(format!("invalid approval task id: {task_id}"))
                    })?,
                    decision,
                    actor_id,
                    decided_at: parse_rfc3339(&decided_at)?,
                    consumed_at: consumed_at.as_deref().map(parse_rfc3339).transpose()?,
                })
            },
        )
        .collect()
}

impl BrowserWaitStore for SqliteStore {
    fn browser_task_policy_get(
        &self,
        task_id: TaskId,
    ) -> Result<Option<BrowserTaskPolicy>, StoreError> {
        self.with_read_conn(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT policy_json FROM browser_task_policies WHERE task_id = ?1",
                    params![task_id.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            raw.map(|value| {
                BrowserTaskPolicy::from_json(&value)
                    .map_err(|e| StoreError::Invalid(e.code().into()))
            })
            .transpose()
        })
    }

    fn browser_task_policy_set(
        &self,
        task_id: TaskId,
        policy: &BrowserTaskPolicy,
    ) -> Result<(), StoreError> {
        policy
            .validate()
            .map_err(|e| StoreError::Invalid(e.code().into()))?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state: Option<String> = tx
            .query_row(
                "SELECT status FROM tasks WHERE id = ?1",
                params![task_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        match state.as_deref() {
            Some("draft" | "ready") => {}
            Some(_) => {
                return Err(StoreError::Invalid(
                    "browser policy can only change while draft or ready".into(),
                ));
            }
            None => return Err(StoreError::Invalid("task not found".into())),
        }
        let raw = serde_json::to_string(policy)?;
        tx.execute(
            "INSERT INTO browser_task_policies (task_id, policy_json) VALUES (?1, ?2)
            ON CONFLICT(task_id) DO UPDATE SET policy_json = excluded.policy_json",
            params![task_id.to_string(), raw],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn browser_wait_open(
        &self,
        task_id: TaskId,
        new: &NewBrowserWait,
        now: OffsetDateTime,
    ) -> Result<BrowserWaitOpen, BrowserWaitError> {
        new.validate()?;
        self.browser_tx(|tx| {
            // resume_key の再送: 同じ task・同じ run/session なら既存の wait を返す（何も書かない）。
            let existing = query_waits(tx, "WHERE resume_key = ?1", &[&new.resume_key])?;
            if let Some(wait) = existing.into_iter().next() {
                if wait.task_id == task_id
                    && wait.run_id == new.run_id
                    && wait.session_id == new.session_id
                    && wait.reason == new.reason
                {
                    return Ok(BrowserWaitOpen {
                        wait,
                        created: false,
                    });
                }
                return Err(BrowserWaitError::Invalid {
                    field: "resume_key",
                });
            }
            match task_status_tx(tx, task_id)? {
                None => return Err(BrowserWaitError::NotFound),
                Some(Status::Running) => {}
                Some(_) => return Err(BrowserWaitError::TaskNotRunning),
            }
            let wait = BrowserWait {
                wait_id: ulid::Ulid::new().to_string(),
                task_id,
                work_unit_id: new.work_unit_id.clone(),
                run_id: new.run_id.clone(),
                session_id: new.session_id.clone(),
                reason: new.reason,
                origin: new.origin.clone(),
                purpose: new.purpose.clone(),
                credential_policy_id: new
                    .credential_policy_id
                    .clone()
                    .or_else(|| new.credential.as_ref().map(|c| c.policy_id.clone())),
                credential: new.credential.clone(),
                operation: new.operation.clone(),
                approval_id: None,
                policy_revision: new.policy_revision,
                policy_hash: new.policy_hash.clone(),
                owner_id: new.owner_id.clone(),
                deadline: now + new.ttl(),
                resume_key: new.resume_key.clone(),
                version: 1,
                state: BrowserWaitState::Pending,
                resolution_code: None,
                created_at: now,
                resolved_at: None,
            };
            tx.execute(
                "INSERT INTO browser_waits (wait_id, task_id, work_unit_id, run_id, session_id, reason, \
                 origin, purpose, credential_id, credential_provider, credential_policy_id, \
                 operation_intent_id, action, args_digest, approval_id, policy_revision, policy_hash, \
                 owner_id, deadline, resume_key, version, state, resolution_code, created_at, \
                 resolved_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                 NULL, ?15, ?16, ?17, ?18, ?19, ?20, ?21, NULL, ?22, NULL)",
                params![
                    wait.wait_id,
                    task_id.to_string(),
                    wait.work_unit_id,
                    wait.run_id,
                    wait.session_id,
                    wait.reason.as_str(),
                    wait.origin,
                    wait.purpose,
                    wait.credential.as_ref().map(|c| c.credential_id.clone()),
                    wait.credential.as_ref().map(|c| c.provider.clone()),
                    wait.credential_policy_id,
                    wait.operation.as_ref().map(|o| o.intent_id.clone()),
                    wait.operation.as_ref().map(|o| o.action.clone()),
                    wait.operation.as_ref().and_then(|o| o.args_digest.clone()),
                    to_i64(wait.policy_revision),
                    wait.policy_hash,
                    wait.owner_id,
                    format_rfc3339(wait.deadline)?,
                    wait.resume_key,
                    to_i64(wait.version),
                    wait.state.as_str(),
                    format_rfc3339(now)?,
                ],
            )
            .map_err(|e| match e {
                // 1 task に開いている wait は 1 件だけ（部分 UNIQUE 索引）。
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    BrowserWaitError::InvalidState { op: "open" }
                }
                other => BrowserWaitError::from(other),
            })?;
            SqliteStore::apply_transition_tx(
                tx,
                task_id,
                Trigger::BrowserWait {
                    approval: wait.reason == BrowserWaitReason::WaitingForApproval,
                },
                vec![Event::BrowserWaitOpened {
                    wait: Box::new(wait.clone()),
                }],
            )?;
            Ok(BrowserWaitOpen {
                wait,
                created: true,
            })
        })
    }

    fn browser_wait_get(&self, wait_id: &str) -> Result<Option<BrowserWait>, StoreError> {
        let conn = self.lock()?;
        wait_by_id_tx(&conn, wait_id)
    }

    fn browser_waits_for_task(&self, task_id: TaskId) -> Result<Vec<BrowserWait>, StoreError> {
        let conn = self.lock()?;
        query_waits(
            &conn,
            "WHERE task_id = ?1 ORDER BY created_at, wait_id",
            &[&task_id.to_string()],
        )
    }

    fn browser_waits_pending(&self) -> Result<Vec<BrowserWait>, StoreError> {
        let conn = self.lock()?;
        query_waits(
            &conn,
            "WHERE state = 'pending' ORDER BY created_at, wait_id",
            &[],
        )
    }

    fn browser_wait_register(
        &self,
        task_id: TaskId,
        wait_id: &str,
        expected_version: u64,
        credential: &CredentialRecord,
        actor_id: &str,
        now: OffsetDateTime,
    ) -> Result<BrowserWaitResolution, BrowserWaitError> {
        check_token(&credential.credential_id, "credential_id")?;
        check_token(&credential.provider, "provider")?;
        check_token(&credential.policy_id, "policy_id")?;
        check_token(&credential.receipt_id, "receipt_id")?;
        check_token(actor_id, "actor_id")?;
        self.browser_tx(|tx| {
            let wait = load_for_task(tx, task_id, wait_id)?;
            if wait.reason != BrowserWaitReason::WaitingForAuth {
                return Err(BrowserWaitError::InvalidState { op: "register" });
            }
            // 同じ credential の再送は冪等（二段階の登録の片方だけ届いた再送を吸収する）。
            if wait.state == BrowserWaitState::Registered {
                if wait
                    .credential
                    .as_ref()
                    .is_some_and(|c| c.credential_id == credential.credential_id)
                {
                    let task_status = task_status_tx(tx, task_id)?.unwrap_or(Status::Ready);
                    return Ok(BrowserWaitResolution {
                        wait,
                        task_status,
                        replayed: true,
                    });
                }
                return Err(BrowserWaitError::InvalidState { op: "register" });
            }
            if wait.state != BrowserWaitState::Pending {
                return Err(if wait.state == BrowserWaitState::Expired {
                    BrowserWaitError::Gone
                } else {
                    BrowserWaitError::InvalidState { op: "register" }
                });
            }
            check_version_and_deadline(tx, &wait, expected_version, now)?;
            // 登録先の origin/policy は保存済みの wait が正（フォームからの差し替えを拒否する）。
            if credential.origin != wait.origin {
                return Err(BrowserWaitError::Invalid { field: "origin" });
            }
            if wait.credential_policy_id.as_deref() != Some(credential.policy_id.as_str()) {
                return Err(BrowserWaitError::Invalid { field: "policy_id" });
            }
            tx.execute(
                "INSERT INTO browser_credentials (credential_id, provider, policy_id, \
                 credential_revision, origin, receipt_id, wait_id, registered_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(credential_id) DO UPDATE SET credential_revision = excluded.credential_revision, \
                 receipt_id = excluded.receipt_id, wait_id = excluded.wait_id, \
                 registered_at = excluded.registered_at \
                 WHERE provider = excluded.provider AND policy_id = excluded.policy_id \
                 AND origin = excluded.origin",
                params![
                    credential.credential_id,
                    credential.provider,
                    credential.policy_id,
                    to_i64(credential.credential_revision),
                    credential.origin,
                    credential.receipt_id,
                    wait.wait_id,
                    format_rfc3339(now)?,
                ],
            )?;
            tx.execute(
                "UPDATE browser_waits SET credential_id = ?1, credential_provider = ?2 WHERE wait_id = ?3",
                params![credential.credential_id, credential.provider, wait.wait_id],
            )?;
            if !set_state_tx(
                tx,
                &wait,
                BrowserWaitState::Registered,
                "registered",
                now,
            )? {
                return Err(BrowserWaitError::VersionConflict);
            }
            let mut after = wait.clone();
            after.credential = Some(CredentialRef {
                credential_id: credential.credential_id.clone(),
                provider: credential.provider.clone(),
                policy_id: credential.policy_id.clone(),
            });
            let event = resolved_event(
                &after,
                BrowserWaitState::Registered,
                "registered",
                Some(actor_id),
            );
            let outcome =
                SqliteStore::apply_transition_tx(tx, task_id, Trigger::BrowserResume, vec![event])?;
            let wait = wait_by_id_tx(tx, wait_id)?.ok_or(BrowserWaitError::NotFound)?;
            Ok(BrowserWaitResolution {
                wait,
                task_status: outcome.next,
                replayed: false,
            })
        })
    }

    fn browser_wait_decide(
        &self,
        task_id: TaskId,
        wait_id: &str,
        decision: &HumanDecision,
        now: OffsetDateTime,
    ) -> Result<BrowserWaitResolution, BrowserWaitError> {
        check_token(&decision.actor_id, "actor_id")?;
        check_token(&decision.owner_session_hash, "owner_session_hash")?;
        check_token(&decision.nonce, "nonce")?;
        check_token(&decision.idempotency_key, "idempotency_key")?;
        self.browser_tx(|tx| {
            let wait = load_for_task(tx, task_id, wait_id)?;
            // 冪等な再送: 同じ idempotency_key・同じ wait・同じ決定なら現在の状態を返す。
            let prior = approval_rows(
                tx,
                "WHERE idempotency_key = ?1",
                &[&decision.idempotency_key],
            )?;
            if let Some(prior) = prior.into_iter().next() {
                if prior.wait_id == wait.wait_id && prior.decision == decision.decision {
                    let task_status = task_status_tx(tx, task_id)?.unwrap_or(Status::Failed);
                    return Ok(BrowserWaitResolution {
                        wait,
                        task_status,
                        replayed: true,
                    });
                }
                return Err(BrowserWaitError::Replay);
            }
            let nonce_used: i64 = tx.query_row(
                "SELECT COUNT(*) FROM browser_approvals WHERE nonce = ?1",
                params![decision.nonce],
                |row| row.get(0),
            )?;
            if nonce_used > 0 {
                return Err(BrowserWaitError::Replay);
            }
            let expected_state = match decision.decision {
                BrowserDecision::ApproveOnce | BrowserDecision::Deny => BrowserWaitState::Pending,
                BrowserDecision::Revoke => BrowserWaitState::Approved,
            };
            if wait.state != expected_state {
                return Err(match wait.state {
                    BrowserWaitState::Expired | BrowserWaitState::Revoked => BrowserWaitError::Gone,
                    _ => BrowserWaitError::InvalidState {
                        op: decision.decision.as_str(),
                    },
                });
            }
            // 登録依頼は「一回だけの実行」を承認できない（登録は使用承認を兼ねない）。拒否だけ。
            if decision.decision == BrowserDecision::ApproveOnce
                && wait.reason != BrowserWaitReason::WaitingForApproval
            {
                return Err(BrowserWaitError::InvalidState { op: "approve_once" });
            }
            if decision.policy_hash != wait.policy_hash {
                return Err(BrowserWaitError::Invalid {
                    field: "policy_hash",
                });
            }
            check_version_and_deadline(tx, &wait, decision.expected_version, now)?;
            let approval_id = ulid::Ulid::new().to_string();
            tx.execute(
                "INSERT INTO browser_approvals (approval_id, wait_id, task_id, decision, actor_id, \
                 owner_session_hash, policy_hash, nonce, idempotency_key, decided_at, consumed_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL)",
                params![
                    approval_id,
                    wait.wait_id,
                    task_id.to_string(),
                    decision.decision.as_str(),
                    decision.actor_id,
                    decision.owner_session_hash,
                    decision.policy_hash,
                    decision.nonce,
                    decision.idempotency_key,
                    format_rfc3339(now)?,
                ],
            )?;
            let (state, code) = match decision.decision {
                BrowserDecision::ApproveOnce => (BrowserWaitState::Approved, "approved"),
                BrowserDecision::Deny => (BrowserWaitState::Denied, "approval_denied"),
                BrowserDecision::Revoke => (BrowserWaitState::Revoked, "revoked"),
            };
            if decision.decision == BrowserDecision::ApproveOnce {
                tx.execute(
                    "UPDATE browser_waits SET approval_id = ?1 WHERE wait_id = ?2",
                    params![approval_id, wait.wait_id],
                )?;
            }
            if !set_state_tx(tx, &wait, state, code, now)? {
                return Err(BrowserWaitError::VersionConflict);
            }
            let mut after = wait.clone();
            if decision.decision == BrowserDecision::ApproveOnce {
                after.approval_id = Some(approval_id.clone());
            }
            let event = resolved_event(&after, state, code, Some(&decision.actor_id));
            let task_status = match decision.decision {
                BrowserDecision::ApproveOnce => {
                    SqliteStore::apply_transition_tx(
                        tx,
                        task_id,
                        Trigger::BrowserResume,
                        vec![event],
                    )?
                    .next
                }
                BrowserDecision::Deny => {
                    SqliteStore::apply_transition_tx(
                        tx,
                        task_id,
                        Trigger::BrowserFail { expired: false },
                        vec![event],
                    )?
                    .next
                }
                // 承認の取り消し: task は既に再開待ち（`ready`）。一回の実行権だけを失効させ、
                // worker の `browser_wait_consume` が `Gone` で断る。
                BrowserDecision::Revoke => {
                    SqliteStore::append_event_tx(tx, task_id, &event)?;
                    task_status_tx(tx, task_id)?.unwrap_or(Status::Ready)
                }
            };
            let wait = wait_by_id_tx(tx, wait_id)?.ok_or(BrowserWaitError::NotFound)?;
            Ok(BrowserWaitResolution {
                wait,
                task_status,
                replayed: false,
            })
        })
    }

    fn browser_wait_consume(
        &self,
        task_id: TaskId,
        wait_id: &str,
        resume_key: &str,
        run_id: &str,
        session_id: &str,
        now: OffsetDateTime,
    ) -> Result<BrowserWait, BrowserWaitError> {
        self.browser_tx(|tx| {
            let wait = load_for_task(tx, task_id, wait_id)?;
            if wait.resume_key != resume_key {
                return Err(BrowserWaitError::Invalid {
                    field: "resume_key",
                });
            }
            match wait.state {
                BrowserWaitState::Approved => {}
                BrowserWaitState::Expired | BrowserWaitState::Revoked => {
                    return Err(BrowserWaitError::Gone);
                }
                _ => return Err(BrowserWaitError::InvalidState { op: "consume" }),
            }
            // 同じ論理 run/session の continuation だけ（別の session で旧承認を使わない）。
            if wait.run_id != run_id || wait.session_id != session_id {
                if set_state_tx(
                    tx,
                    &wait,
                    BrowserWaitState::Invalidated,
                    "session_mismatch",
                    now,
                )? {
                    let event = resolved_event(
                        &wait,
                        BrowserWaitState::Invalidated,
                        "session_mismatch",
                        None,
                    );
                    SqliteStore::append_event_tx(tx, task_id, &event)?;
                }
                return Err(BrowserWaitError::Gone);
            }
            if wait.deadline <= now {
                expire_tx(tx, &wait, now)?;
                return Err(BrowserWaitError::Gone);
            }
            let changed = tx.execute(
                "UPDATE browser_approvals SET consumed_at = ?1 \
                 WHERE approval_id = ?2 AND consumed_at IS NULL",
                params![format_rfc3339(now)?, wait.approval_id],
            )?;
            if changed != 1 || !set_state_tx(tx, &wait, BrowserWaitState::Resumed, "consumed", now)?
            {
                return Err(BrowserWaitError::InvalidState { op: "consume" });
            }
            let event = resolved_event(&wait, BrowserWaitState::Resumed, "consumed", None);
            SqliteStore::append_event_tx(tx, task_id, &event)?;
            wait_by_id_tx(tx, wait_id)?.ok_or(BrowserWaitError::NotFound)
        })
    }

    fn browser_waits_expire(&self, now: OffsetDateTime) -> Result<Vec<BrowserWait>, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let due = query_waits(
            &tx,
            "WHERE state IN ('pending', 'approved') AND deadline <= ?1 ORDER BY deadline, wait_id",
            &[&format_rfc3339(now)?],
        )?;
        let mut expired = Vec::new();
        for wait in due {
            if expire_tx(&tx, &wait, now)?
                && let Some(w) = wait_by_id_tx(&tx, &wait.wait_id)?
            {
                expired.push(w);
            }
        }
        tx.commit()?;
        Ok(expired)
    }

    fn browser_approvals_for_wait(
        &self,
        wait_id: &str,
    ) -> Result<Vec<BrowserApprovalRecord>, StoreError> {
        let conn = self.lock()?;
        approval_rows(
            &conn,
            "WHERE wait_id = ?1 ORDER BY decided_at, approval_id",
            &[&wait_id],
        )
    }

    fn browser_credential_get(
        &self,
        credential_id: &str,
    ) -> Result<Option<CredentialRecord>, StoreError> {
        let conn = self.lock()?;
        Ok(conn
            .query_row(
                "SELECT credential_id, provider, policy_id, credential_revision, origin, receipt_id \
                 FROM browser_credentials WHERE credential_id = ?1",
                params![credential_id],
                |row| {
                    Ok(CredentialRecord {
                        credential_id: row.get(0)?,
                        provider: row.get(1)?,
                        policy_id: row.get(2)?,
                        credential_revision: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
                        origin: row.get(4)?,
                        receipt_id: row.get(5)?,
                    })
                },
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskStore;
    use crate::model::{Budget, Task, TaskKind, Tier, WorkerHint, WorkspaceSpec};

    const SENTINEL: &str = "SENTINEL-p4ssw0rd-9f3a";

    fn task(status: Status) -> Task {
        let now = OffsetDateTime::now_utc();
        Task {
            routing: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            id: TaskId::new(),
            parent_id: None,
            kind: TaskKind::Execute,
            title: "browser".into(),
            objective: "o".into(),
            acceptance: vec![],
            inputs: vec![],
            depends_on: vec![],
            status,
            priority: 0,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            workspace: WorkspaceSpec::Local {
                path: "ws".into(),
                mode: None,
            },
            budget: Budget {
                max_turns: 1,
                max_wall_secs: 1,
                max_retries: 2,
            },
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: None,
            aggregate: false,
            project_id: None,
            milestone_id: None,
            assignee: None,
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
            tree: None,
            paused_at: None,
        }
    }

    fn running(store: &SqliteStore) -> TaskId {
        let t = task(Status::Ready);
        store.insert(&t).expect("insert");
        assert!(
            store
                .acquire_lease(t.id, "run-1", std::time::Duration::from_secs(60))
                .expect("lease")
        );
        t.id
    }

    fn auth_request(key: &str) -> NewBrowserWait {
        NewBrowserWait {
            work_unit_id: None,
            run_id: "run-1".into(),
            session_id: "sess-1".into(),
            reason: BrowserWaitReason::WaitingForAuth,
            origin: "https://login.example.com".into(),
            purpose: "Sign in to read the build dashboard".into(),
            credential_policy_id: Some("pol-example".into()),
            credential: None,
            operation: None,
            policy_revision: 3,
            policy_hash: "sha256-abc".into(),
            owner_id: Some("owner".into()),
            ttl_secs: None,
            resume_key: key.into(),
        }
    }

    fn approval_request(key: &str) -> NewBrowserWait {
        NewBrowserWait {
            reason: BrowserWaitReason::WaitingForApproval,
            credential_policy_id: None,
            credential: Some(CredentialRef {
                credential_id: "cred-1".into(),
                provider: "manual".into(),
                policy_id: "pol-example".into(),
            }),
            operation: Some(OperationIntent {
                intent_id: "intent-1".into(),
                action: "credential_use".into(),
                args_digest: Some("sha256-args".into()),
            }),
            ttl_secs: Some(10_000),
            ..auth_request(key)
        }
    }

    fn decision(kind: BrowserDecision, version: u64, nonce: &str) -> HumanDecision {
        HumanDecision {
            decision: kind,
            expected_version: version,
            actor_id: "owner".into(),
            owner_session_hash: "sess-hash".into(),
            policy_hash: "sha256-abc".into(),
            nonce: nonce.into(),
            idempotency_key: format!("idem-{nonce}"),
        }
    }

    fn status(store: &SqliteStore, id: TaskId) -> Status {
        store.get(id).expect("get").expect("task").status
    }

    fn record() -> CredentialRecord {
        CredentialRecord {
            credential_id: "cred-1".into(),
            provider: "manual".into(),
            policy_id: "pol-example".into(),
            credential_revision: 1,
            origin: "https://login.example.com".into(),
            receipt_id: "rcpt-1".into(),
        }
    }

    #[test]
    fn open_blocks_task_releases_lease_and_is_idempotent_by_resume_key() {
        let store = SqliteStore::open_in_memory().expect("open");
        let id = running(&store);
        let now = OffsetDateTime::now_utc();
        let open = store
            .browser_wait_open(id, &auth_request("rk-1"), now)
            .expect("open");
        assert!(open.created);
        assert_eq!(open.wait.state, BrowserWaitState::Pending);
        assert_eq!(open.wait.deadline, now + Duration::hours(24));
        let t = store.get(id).expect("get").expect("task");
        assert_eq!(t.status, Status::Blocked);
        assert!(t.lease.is_none(), "worker slot (lease) must be released");
        // 同じ resume_key の再送は同じ wait。
        let again = store
            .browser_wait_open(id, &auth_request("rk-1"), now)
            .expect("again");
        assert!(!again.created);
        assert_eq!(again.wait.wait_id, open.wait.wait_id);
        assert_eq!(store.browser_waits_for_task(id).expect("list").len(), 1);
        // 別の task が同じ resume_key を使うのは拒否。
        let other = running(&store);
        assert!(matches!(
            store.browser_wait_open(other, &auth_request("rk-1"), now),
            Err(BrowserWaitError::Invalid {
                field: "resume_key"
            })
        ));
        // events: transitioned(waiting_for_auth) + browser_wait_opened が同じ遷移で入る。
        let events: Vec<Event> = store
            .events_for(id)
            .expect("events")
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        assert!(events.iter().any(|e| matches!(e, Event::Transitioned { to: Status::Blocked, reason, .. } if reason == "waiting_for_auth")));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::BrowserWaitOpened { .. }))
        );
    }

    #[test]
    fn open_rejects_non_running_task_and_bad_fields() {
        let store = SqliteStore::open_in_memory().expect("open");
        let t = task(Status::Ready);
        store.insert(&t).expect("insert");
        let now = OffsetDateTime::now_utc();
        assert!(matches!(
            store.browser_wait_open(t.id, &auth_request("rk"), now),
            Err(BrowserWaitError::TaskNotRunning)
        ));
        let id = running(&store);
        for (mutate, field) in [
            (
                Box::new(|r: &mut NewBrowserWait| r.origin = "https://*.example.com".into())
                    as Box<dyn Fn(&mut NewBrowserWait)>,
                "origin",
            ),
            (
                Box::new(|r: &mut NewBrowserWait| r.origin = "https://u:p@example.com".into()),
                "origin",
            ),
            (
                Box::new(|r: &mut NewBrowserWait| r.origin = "https://example.com:443".into()),
                "origin",
            ),
            (
                Box::new(|r: &mut NewBrowserWait| r.origin = "https://example.com/login".into()),
                "origin",
            ),
            (
                Box::new(|r: &mut NewBrowserWait| r.origin = "http://example.com".into()),
                "origin",
            ),
            (
                Box::new(|r: &mut NewBrowserWait| r.purpose = "x".repeat(501)),
                "purpose",
            ),
            (
                Box::new(|r: &mut NewBrowserWait| r.purpose = "a\nb".into()),
                "purpose",
            ),
            (
                Box::new(|r: &mut NewBrowserWait| r.credential_policy_id = None),
                "credential_policy_id",
            ),
        ] {
            let mut r = auth_request("rk-bad");
            mutate(&mut r);
            match store.browser_wait_open(id, &r, now) {
                Err(BrowserWaitError::Invalid { field: got }) => assert_eq!(got, field),
                other => panic!("expected invalid {field}: {other:?}"),
            }
        }
        assert!(valid_exact_origin("https://example.com:8443"));
        assert_eq!(status(&store, id), Status::Running);
    }

    #[test]
    fn register_resolves_once_and_does_not_approve_use() {
        let store = SqliteStore::open_in_memory().expect("open");
        let id = running(&store);
        let now = OffsetDateTime::now_utc();
        let wait = store
            .browser_wait_open(id, &auth_request("rk-1"), now)
            .expect("open")
            .wait;
        // 一般の回答・途中確認の再開では ready に戻せない。
        assert!(store.apply_transition(id, Trigger::Answer, None).is_err());
        assert_eq!(status(&store, id), Status::Blocked);
        // origin の差し替え・古い version は拒否。
        let mut wrong = record();
        wrong.origin = "https://evil.example.com".into();
        assert!(matches!(
            store.browser_wait_register(id, &wait.wait_id, wait.version, &wrong, "owner", now),
            Err(BrowserWaitError::Invalid { field: "origin" })
        ));
        assert!(matches!(
            store.browser_wait_register(id, &wait.wait_id, 99, &record(), "owner", now),
            Err(BrowserWaitError::VersionConflict)
        ));
        // 登録依頼は approve_once できない。
        assert!(matches!(
            store.browser_wait_decide(
                id,
                &wait.wait_id,
                &decision(BrowserDecision::ApproveOnce, wait.version, "n0"),
                now
            ),
            Err(BrowserWaitError::InvalidState { .. })
        ));
        let r = store
            .browser_wait_register(id, &wait.wait_id, wait.version, &record(), "owner", now)
            .expect("register");
        assert!(!r.replayed);
        assert_eq!(r.task_status, Status::Ready);
        assert_eq!(r.wait.state, BrowserWaitState::Registered);
        assert_eq!(r.wait.version, 2);
        assert!(r.wait.approval_id.is_none(), "registration is not approval");
        // 再送は冪等。
        let again = store
            .browser_wait_register(id, &wait.wait_id, wait.version, &record(), "owner", now)
            .expect("replay");
        assert!(again.replayed);
        assert_eq!(
            store
                .browser_credential_get("cred-1")
                .expect("get")
                .expect("credential")
                .origin,
            "https://login.example.com"
        );
    }

    #[test]
    fn approve_once_consumes_once_and_deny_fails_task() {
        let store = SqliteStore::open_in_memory().expect("open");
        let id = running(&store);
        let now = OffsetDateTime::now_utc();
        let wait = store
            .browser_wait_open(id, &approval_request("rk-a"), now)
            .expect("open")
            .wait;
        // 承認待ちは既定・上限 5 分（要求の 10000 秒は丸める）。
        assert_eq!(wait.deadline, now + Duration::minutes(5));
        assert!(matches!(
            store.browser_wait_decide(
                id,
                &wait.wait_id,
                &decision(BrowserDecision::ApproveOnce, 7, "n1"),
                now
            ),
            Err(BrowserWaitError::VersionConflict)
        ));
        let mut bad_policy = decision(BrowserDecision::ApproveOnce, wait.version, "n1");
        bad_policy.policy_hash = "sha256-other".into();
        assert!(matches!(
            store.browser_wait_decide(id, &wait.wait_id, &bad_policy, now),
            Err(BrowserWaitError::Invalid {
                field: "policy_hash"
            })
        ));
        let d = decision(BrowserDecision::ApproveOnce, wait.version, "n1");
        let r = store
            .browser_wait_decide(id, &wait.wait_id, &d, now)
            .expect("approve");
        assert_eq!(r.task_status, Status::Ready);
        assert_eq!(r.wait.state, BrowserWaitState::Approved);
        // 同じ idempotency_key の再送は冪等、別の決定で同じ nonce は replay。
        assert!(
            store
                .browser_wait_decide(id, &wait.wait_id, &d, now)
                .expect("replay")
                .replayed
        );
        let mut reuse = decision(BrowserDecision::Deny, r.wait.version, "n1");
        reuse.idempotency_key = "idem-other".into();
        assert!(matches!(
            store.browser_wait_decide(id, &wait.wait_id, &reuse, now),
            Err(BrowserWaitError::Replay)
        ));
        let consumed = store
            .browser_wait_consume(id, &wait.wait_id, "rk-a", "run-1", "sess-1", now)
            .expect("consume");
        assert_eq!(consumed.state, BrowserWaitState::Resumed);
        assert!(matches!(
            store.browser_wait_consume(id, &wait.wait_id, "rk-a", "run-1", "sess-1", now),
            Err(BrowserWaitError::InvalidState { op: "consume" })
        ));
        let approvals = store
            .browser_approvals_for_wait(&wait.wait_id)
            .expect("approvals");
        assert_eq!(approvals.len(), 1);
        assert!(approvals[0].consumed_at.is_some());

        // 拒否: task は failed（自動 retry なし）。
        let id2 = running(&store);
        let w2 = store
            .browser_wait_open(id2, &approval_request("rk-b"), now)
            .expect("open")
            .wait;
        let r2 = store
            .browser_wait_decide(
                id2,
                &w2.wait_id,
                &decision(BrowserDecision::Deny, w2.version, "n2"),
                now,
            )
            .expect("deny");
        assert_eq!(r2.task_status, Status::Failed);
        assert_eq!(r2.wait.state, BrowserWaitState::Denied);
        assert_eq!(r2.wait.resolution_code.as_deref(), Some("approval_denied"));
        let t2 = store.get(id2).expect("get").expect("task");
        assert_eq!(t2.attempts, 0);
    }

    #[test]
    fn consume_in_other_session_invalidates_and_revoke_blocks_consume() {
        let store = SqliteStore::open_in_memory().expect("open");
        let now = OffsetDateTime::now_utc();
        let id = running(&store);
        let w = store
            .browser_wait_open(id, &approval_request("rk-s"), now)
            .expect("open")
            .wait;
        store
            .browser_wait_decide(
                id,
                &w.wait_id,
                &decision(BrowserDecision::ApproveOnce, w.version, "n1"),
                now,
            )
            .expect("approve");
        assert!(matches!(
            store.browser_wait_consume(id, &w.wait_id, "rk-s", "run-1", "sess-2", now),
            Err(BrowserWaitError::Gone)
        ));
        assert_eq!(
            store
                .browser_wait_get(&w.wait_id)
                .expect("get")
                .expect("wait")
                .state,
            BrowserWaitState::Invalidated
        );

        let id2 = running(&store);
        let w2 = store
            .browser_wait_open(id2, &approval_request("rk-r"), now)
            .expect("open")
            .wait;
        let approved = store
            .browser_wait_decide(
                id2,
                &w2.wait_id,
                &decision(BrowserDecision::ApproveOnce, w2.version, "n2"),
                now,
            )
            .expect("approve");
        let revoked = store
            .browser_wait_decide(
                id2,
                &w2.wait_id,
                &decision(BrowserDecision::Revoke, approved.wait.version, "n3"),
                now,
            )
            .expect("revoke");
        assert_eq!(revoked.wait.state, BrowserWaitState::Revoked);
        assert!(matches!(
            store.browser_wait_consume(id2, &w2.wait_id, "rk-r", "run-1", "sess-1", now),
            Err(BrowserWaitError::Gone)
        ));
    }

    #[test]
    fn expiry_terminates_exactly_once_and_cancel_closes_waits() {
        let store = SqliteStore::open_in_memory().expect("open");
        let now = OffsetDateTime::now_utc();
        let id = running(&store);
        let w = store
            .browser_wait_open(id, &approval_request("rk-e"), now)
            .expect("open")
            .wait;
        assert!(
            store
                .browser_waits_expire(now + Duration::minutes(4))
                .expect("not yet")
                .is_empty()
        );
        let later = now + Duration::minutes(6);
        // 期限後の承認は 410 相当で、wait は期限切れとして終端化される。
        assert!(matches!(
            store.browser_wait_decide(
                id,
                &w.wait_id,
                &decision(BrowserDecision::ApproveOnce, w.version, "n1"),
                later
            ),
            Err(BrowserWaitError::Gone)
        ));
        assert_eq!(status(&store, id), Status::Failed);
        // 照合でもう一度終端化しない。
        assert!(
            store
                .browser_waits_expire(later)
                .expect("expire")
                .is_empty()
        );
        let resolved = store
            .events_for(id)
            .expect("events")
            .into_iter()
            .filter(|(_, e)| matches!(e, Event::BrowserWaitResolved { code, .. } if code == "browser_wait_expired"))
            .count();
        assert_eq!(resolved, 1);

        // 照合（tick）による期限切れ。
        let id2 = running(&store);
        store
            .browser_wait_open(id2, &auth_request("rk-e2"), now)
            .expect("open");
        let expired = store
            .browser_waits_expire(now + Duration::hours(25))
            .expect("expire");
        assert_eq!(expired.len(), 1);
        assert_eq!(status(&store, id2), Status::Failed);
        assert!(
            store
                .browser_waits_expire(now + Duration::hours(26))
                .expect("again")
                .is_empty()
        );

        // cancel: 開いている wait は cancelled。
        let id3 = running(&store);
        let w3 = store
            .browser_wait_open(id3, &auth_request("rk-c"), now)
            .expect("open")
            .wait;
        store
            .apply_transition(id3, Trigger::Cancel, None)
            .expect("cancel");
        let w3 = store
            .browser_wait_get(&w3.wait_id)
            .expect("get")
            .expect("wait");
        assert_eq!(w3.state, BrowserWaitState::Cancelled);
        assert!(store.browser_waits_pending().expect("pending").is_empty());
    }

    #[test]
    fn waits_survive_restart_and_resume() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("celeris.db");
        let now = OffsetDateTime::now_utc();
        let (id, wait) = {
            let store = SqliteStore::open(&path).expect("open");
            let id = running(&store);
            let wait = store
                .browser_wait_open(id, &approval_request("rk-restart"), now)
                .expect("open")
                .wait;
            (id, wait)
        };
        let store = SqliteStore::open(&path).expect("reopen");
        // 再起動時の照合: 期限内なので残る。
        assert!(store.browser_waits_expire(now).expect("expire").is_empty());
        let pending = store.browser_waits_pending().expect("pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].wait_id, wait.wait_id);
        let r = store
            .browser_wait_decide(
                id,
                &wait.wait_id,
                &decision(BrowserDecision::ApproveOnce, wait.version, "n-restart"),
                now,
            )
            .expect("approve after restart");
        assert_eq!(r.task_status, Status::Ready);
        store
            .browser_wait_consume(id, &wait.wait_id, "rk-restart", "run-1", "sess-1", now)
            .expect("consume after restart");
    }

    /// 秘密を受け取る型がここには無いこと、DB の全行・event に秘密が混ざらないこと（API 層の試験と
    /// 対になる）: 秘密っぽい値を他の欄に入れようとしても検証で落ち、どこにも書かれない。
    #[test]
    fn secret_like_values_never_reach_rows_or_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("celeris.db");
        let store = SqliteStore::open(&path).expect("open");
        let now = OffsetDateTime::now_utc();
        let id = running(&store);
        let mut r = auth_request("rk-x");
        r.policy_hash = format!("{SENTINEL}!");
        assert!(store.browser_wait_open(id, &r, now).is_err());
        let w = store
            .browser_wait_open(id, &auth_request("rk-x"), now)
            .expect("open")
            .wait;
        store
            .browser_wait_register(id, &w.wait_id, w.version, &record(), "owner", now)
            .expect("register");
        drop(store);
        let raw = std::fs::read(&path).expect("db");
        let hay = String::from_utf8_lossy(&raw);
        assert!(!hay.contains(SENTINEL));
    }
}
