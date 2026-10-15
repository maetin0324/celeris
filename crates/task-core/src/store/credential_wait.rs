//! credential 待ちの永続状態（ADR 2026-10-15-mlsys-min-decision-assisted D4）。
//!
//! 外部 provider の鍵が未登録で止まった work unit を `credential_waits` に 1 行ずつ持つ。
//! **秘密の値はどの列にも置かない**（`secret_id` と fingerprint だけ）。遷移は条件付き UPDATE で、
//! `waiting|rejected → verified → resumed` の一方向、`rejected` は再登録待ち、`cancelled` は終端。
//! `claim_resume` は一度だけ `true` を返す。LLM 呼び出しは無い。
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::model::TaskId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialWaitState {
    /// 鍵が未登録（`waiting_for_auth`）。
    Waiting,
    /// 登録された鍵が誤り／権限不足。同じ通知を更新して再登録を待つ。
    Rejected,
    /// 検証に成功した。再開の claim 待ち。
    Verified,
    /// 再開を claim 済み（以後 resume しない）。
    Resumed,
    /// task が cancel され、再開しない。
    Cancelled,
}

impl CredentialWaitState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Rejected => "rejected",
            Self::Verified => "verified",
            Self::Resumed => "resumed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(s: &str) -> Result<Self, StoreError> {
        Ok(match s {
            "waiting" => Self::Waiting,
            "rejected" => Self::Rejected,
            "verified" => Self::Verified,
            "resumed" => Self::Resumed,
            "cancelled" => Self::Cancelled,
            other => {
                return Err(StoreError::Invalid(format!(
                    "credential wait state {other}"
                )));
            }
        })
    }

    /// 開いている（まだ再開も cancel もされていない）。
    pub fn is_open(self) -> bool {
        matches!(self, Self::Waiting | Self::Rejected | Self::Verified)
    }
}

/// 検証結果の種別（値は持たない）。429・予算不足・5xx は再登録要求にしないので、ここには無い。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialVerifyKind {
    Valid,
    InvalidKey,
    InsufficientPermission,
}

impl CredentialVerifyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::InvalidKey => "invalid_key",
            Self::InsufficientPermission => "insufficient_permission",
        }
    }

    fn parse(s: &str) -> Result<Self, StoreError> {
        Ok(match s {
            "valid" => Self::Valid,
            "invalid_key" => Self::InvalidKey,
            "insufficient_permission" => Self::InsufficientPermission,
            other => {
                return Err(StoreError::Invalid(format!(
                    "credential verify kind {other}"
                )));
            }
        })
    }
}

/// `credential_waits` の 1 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialWait {
    pub id: i64,
    pub work_unit_id: String,
    pub task_id: TaskId,
    pub secret_id: String,
    pub provider: String,
    pub purpose: String,
    pub state: CredentialWaitState,
    pub aggregation_key: String,
    pub notice_item_id: Option<String>,
    pub seen_fingerprint: Option<String>,
    pub last_verify_kind: Option<CredentialVerifyKind>,
    pub resumed_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

/// `enter_wait` の入力（値は参照名と用途だけ）。
#[derive(Debug, Clone)]
pub struct CredentialWaitEnter<'a> {
    pub work_unit_id: &'a str,
    pub task_id: TaskId,
    pub secret_id: &'a str,
    pub provider: &'a str,
    pub purpose: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterWaitOutcome {
    pub wait: CredentialWait,
    /// この呼び出しで新しく作った（既に待機中なら `false`）。
    pub created: bool,
    /// 同じ集約キーで他に開いている wait が無かった（= 通知を新規に 1 件出す番）。
    pub first_for_key: bool,
}

/// 同じ秘密を待つ wait を束ねる集約キー。
pub fn credential_aggregation_key(secret_id: &str) -> String {
    format!("credential:{secret_id}")
}

const COLS: &str = "id, work_unit_id, task_id, secret_id, provider, purpose, state, aggregation_key, \
    notice_item_id, seen_fingerprint, last_verify_kind, resumed_at, created_at";

fn parse_ts(s: &str) -> Result<OffsetDateTime, StoreError> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .map_err(|e| StoreError::Invalid(e.to_string()))
}

fn from_row(r: &rusqlite::Row<'_>) -> Result<CredentialWait, StoreError> {
    let task: String = r.get(2)?;
    let state: String = r.get(6)?;
    let kind: Option<String> = r.get(10)?;
    let resumed: Option<String> = r.get(11)?;
    let created: String = r.get(12)?;
    Ok(CredentialWait {
        id: r.get(0)?,
        work_unit_id: r.get(1)?,
        task_id: task
            .parse::<TaskId>()
            .map_err(|e| StoreError::Invalid(format!("credential wait task id: {e}")))?,
        secret_id: r.get(3)?,
        provider: r.get(4)?,
        purpose: r.get(5)?,
        state: CredentialWaitState::parse(&state)?,
        aggregation_key: r.get(7)?,
        notice_item_id: r.get(8)?,
        seen_fingerprint: r.get(9)?,
        last_verify_kind: kind
            .as_deref()
            .map(CredentialVerifyKind::parse)
            .transpose()?,
        resumed_at: resumed.as_deref().map(parse_ts).transpose()?,
        created_at: parse_ts(&created)?,
    })
}

fn query_waits(
    conn: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<CredentialWait>, StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(args)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(from_row(r)?);
    }
    Ok(out)
}

fn open_wait_tx(
    conn: &Connection,
    work_unit_id: &str,
    secret_id: &str,
) -> Result<Option<CredentialWait>, StoreError> {
    Ok(query_waits(
        conn,
        &format!(
            "SELECT {COLS} FROM credential_waits WHERE work_unit_id = ?1 AND secret_id = ?2 \
             AND state IN ('waiting', 'rejected', 'verified')"
        ),
        &[&work_unit_id, &secret_id],
    )?
    .into_iter()
    .next())
}

impl SqliteStore {
    /// work unit を credential 待ちへ入れる。同じ (work unit, secret) が既に開いていれば何もせず返す（冪等）。
    pub fn credential_wait_enter(
        &self,
        input: &CredentialWaitEnter<'_>,
        now: OffsetDateTime,
    ) -> Result<EnterWaitOutcome, StoreError> {
        let at = format_rfc3339(now)?;
        let key = credential_aggregation_key(input.secret_id);
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(wait) = open_wait_tx(&tx, input.work_unit_id, input.secret_id)? {
            let first_for_key = false;
            tx.commit()?;
            return Ok(EnterWaitOutcome {
                wait,
                created: false,
                first_for_key,
            });
        }
        let others: i64 = tx.query_row(
            "SELECT COUNT(*) FROM credential_waits WHERE aggregation_key = ?1 \
             AND state IN ('waiting', 'rejected', 'verified')",
            params![key],
            |r| r.get(0),
        )?;
        // 既に同じ鍵の登録を見ていれば、その fingerprint から始める（登録済みなら検証待ち）。
        let seen: Option<String> = tx
            .query_row(
                "SELECT fingerprint FROM credential_wait_events WHERE secret_id = ?1 ORDER BY id DESC LIMIT 1",
                params![input.secret_id],
                |r| r.get(0),
            )
            .optional()?;
        let notice: Option<String> = tx
            .query_row(
                "SELECT notice_item_id FROM credential_waits WHERE aggregation_key = ?1 \
                 AND notice_item_id IS NOT NULL ORDER BY id DESC LIMIT 1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute(
            "INSERT INTO credential_waits (work_unit_id, task_id, secret_id, provider, purpose, state, \
             aggregation_key, notice_item_id, seen_fingerprint, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'waiting', ?6, ?7, ?8, ?9, ?9)",
            params![
                input.work_unit_id,
                input.task_id.to_string(),
                input.secret_id,
                input.provider,
                input.purpose,
                key,
                notice,
                seen,
                at
            ],
        )?;
        let wait = open_wait_tx(&tx, input.work_unit_id, input.secret_id)?
            .ok_or_else(|| StoreError::Invalid("credential wait vanished".into()))?;
        tx.commit()?;
        Ok(EnterWaitOutcome {
            wait,
            created: true,
            first_for_key: others == 0,
        })
    }

    /// 秘密の保存・更新の完了を追記専用の event（`secret_stored`。id と fingerprint のみ）にし、
    /// その鍵を待つ wait の `seen_fingerprint` を更新する。検証が要る wait（fingerprint が変わった
    /// `waiting`/`rejected`）の work unit id を返す。同じ (secret, fingerprint) の重複は何も足さず空を返す。
    pub fn credential_record_secret_saved(
        &self,
        secret_id: &str,
        fingerprint: &str,
        now: OffsetDateTime,
    ) -> Result<Vec<CredentialWait>, StoreError> {
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last: Option<String> = tx
            .query_row(
                "SELECT fingerprint FROM credential_wait_events WHERE secret_id = ?1 ORDER BY id DESC LIMIT 1",
                params![secret_id],
                |r| r.get(0),
            )
            .optional()?;
        if last.as_deref() == Some(fingerprint) {
            tx.commit()?;
            return Ok(Vec::new());
        }
        tx.execute(
            "INSERT INTO credential_wait_events (ts, kind, secret_id, fingerprint) \
             VALUES (?1, 'secret_stored', ?2, ?3)",
            params![at, secret_id, fingerprint],
        )?;
        tx.execute(
            "UPDATE credential_waits SET seen_fingerprint = ?2, updated_at = ?3 \
             WHERE secret_id = ?1 AND state IN ('waiting', 'rejected') \
             AND COALESCE(seen_fingerprint, '') <> ?2",
            params![secret_id, fingerprint, at],
        )?;
        let need = query_waits(
            &tx,
            &format!(
                "SELECT {COLS} FROM credential_waits WHERE secret_id = ?1 \
                 AND state IN ('waiting', 'rejected') ORDER BY id"
            ),
            &[&secret_id],
        )?;
        tx.commit()?;
        Ok(need)
    }

    /// 検証成功。`waiting|rejected → verified`。`fingerprint` が最後に見た登録と違えば（古い検証）変えない。
    pub fn credential_wait_mark_verified(
        &self,
        work_unit_id: &str,
        secret_id: &str,
        fingerprint: &str,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let n = self.lock()?.execute(
            "UPDATE credential_waits SET state = 'verified', last_verify_kind = 'valid', updated_at = ?4 \
             WHERE work_unit_id = ?1 AND secret_id = ?2 AND state IN ('waiting', 'rejected') \
             AND seen_fingerprint = ?3",
            params![work_unit_id, secret_id, fingerprint, format_rfc3339(now)?],
        )?;
        Ok(n > 0)
    }

    /// 誤キー／権限不足。`waiting|rejected → rejected`（再登録待ち）。`Valid` は受け付けない。
    pub fn credential_wait_mark_rejected(
        &self,
        work_unit_id: &str,
        secret_id: &str,
        fingerprint: &str,
        kind: CredentialVerifyKind,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        if kind == CredentialVerifyKind::Valid {
            return Err(StoreError::Invalid(
                "mark_rejected needs a failure kind".into(),
            ));
        }
        let n = self.lock()?.execute(
            "UPDATE credential_waits SET state = 'rejected', last_verify_kind = ?4, updated_at = ?5 \
             WHERE work_unit_id = ?1 AND secret_id = ?2 AND state IN ('waiting', 'rejected') \
             AND seen_fingerprint = ?3",
            params![
                work_unit_id,
                secret_id,
                fingerprint,
                kind.as_str(),
                format_rfc3339(now)?
            ],
        )?;
        Ok(n > 0)
    }

    /// 再開の claim。`verified → resumed` を条件付き UPDATE で一度だけ成功させる。task が cancel 済みなら
    /// `cancelled` にして `false`（再開しない）。
    pub fn credential_wait_claim_resume(
        &self,
        work_unit_id: &str,
        secret_id: &str,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cancelled: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM credential_waits w JOIN tasks t ON t.id = w.task_id \
                 WHERE w.work_unit_id = ?1 AND w.secret_id = ?2 AND w.state = 'verified' \
                 AND t.status = 'cancelled'",
                params![work_unit_id, secret_id],
                |r| r.get(0),
            )
            .optional()?;
        if cancelled.is_some() {
            tx.execute(
                "UPDATE credential_waits SET state = 'cancelled', updated_at = ?3 \
                 WHERE work_unit_id = ?1 AND secret_id = ?2 AND state = 'verified'",
                params![work_unit_id, secret_id, at],
            )?;
            tx.commit()?;
            return Ok(false);
        }
        let n = tx.execute(
            "UPDATE credential_waits SET state = 'resumed', resumed_at = ?3, updated_at = ?3 \
             WHERE work_unit_id = ?1 AND secret_id = ?2 AND state = 'verified'",
            params![work_unit_id, secret_id, at],
        )?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// 再開の claim を取り消す（`resumed → verified`）。claim の後の再キューが失敗したときだけ使い、
    /// 次の tick で再び claim できるようにする。
    pub fn credential_wait_release_claim(
        &self,
        work_unit_id: &str,
        secret_id: &str,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let n = self.lock()?.execute(
            "UPDATE credential_waits SET state = 'verified', resumed_at = NULL, updated_at = ?3 \
             WHERE work_unit_id = ?1 AND secret_id = ?2 AND state = 'resumed'",
            params![work_unit_id, secret_id, format_rfc3339(now)?],
        )?;
        Ok(n > 0)
    }

    /// task の cancel。開いている wait を `cancelled` にする（以後 claim_resume は成功しない）。件数を返す。
    pub fn credential_wait_cancel(
        &self,
        task_id: TaskId,
        now: OffsetDateTime,
    ) -> Result<usize, StoreError> {
        Ok(self.lock()?.execute(
            "UPDATE credential_waits SET state = 'cancelled', updated_at = ?2 \
             WHERE task_id = ?1 AND state IN ('waiting', 'rejected', 'verified')",
            params![task_id.to_string(), format_rfc3339(now)?],
        )?)
    }

    /// 通知（受信箱 item）の id を、同じ集約キーの開いている wait 全部に記録する。
    pub fn credential_wait_set_notice_item(
        &self,
        aggregation_key: &str,
        item_id: &str,
        now: OffsetDateTime,
    ) -> Result<usize, StoreError> {
        Ok(self.lock()?.execute(
            "UPDATE credential_waits SET notice_item_id = ?2, updated_at = ?3 \
             WHERE aggregation_key = ?1 AND state IN ('waiting', 'rejected', 'verified')",
            params![aggregation_key, item_id, format_rfc3339(now)?],
        )?)
    }

    /// 開いている wait（`waiting`/`rejected`/`verified`）を古い順に返す。再起動復元・reconcile 用。
    pub fn credential_waits_list_waiting(&self) -> Result<Vec<CredentialWait>, StoreError> {
        let conn = self.lock()?;
        query_waits(
            &conn,
            &format!(
                "SELECT {COLS} FROM credential_waits \
                 WHERE state IN ('waiting', 'rejected', 'verified') ORDER BY id"
            ),
            &[],
        )
    }

    /// 1 つの wait（work unit と secret で、開いているものを優先して最新）。
    pub fn credential_wait_get(
        &self,
        work_unit_id: &str,
        secret_id: &str,
    ) -> Result<Option<CredentialWait>, StoreError> {
        let conn = self.lock()?;
        Ok(query_waits(
            &conn,
            &format!(
                "SELECT {COLS} FROM credential_waits WHERE work_unit_id = ?1 AND secret_id = ?2 \
                 ORDER BY id DESC LIMIT 1"
            ),
            &[&work_unit_id, &secret_id],
        )?
        .into_iter()
        .next())
    }
}

#[cfg(test)]
#[path = "credential_wait_tests.rs"]
mod tests;
