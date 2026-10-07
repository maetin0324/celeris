//! ADR 2026-10-07-browser-trusted-devices D2・D4・D5: 表 `browser_trusted_devices`（migration 0057）。
//!
//! 秘密は hash（SHA-256 hex）だけを扱う。登録・検証と回転・失効は、読み切り→判定→更新→event を
//! 1 つの IMMEDIATE transaction で行う（同じ秘密の並行した提示が両方通らない）。events に秘密も hash も
//! 載せない。時刻 `now` は呼び手が注入する（UNIX 秒）。probe 用の `trusted_device_verify_readonly` は
//! 何も書かない（最終使用も期限も event も）。

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use crate::model::Event;
use crate::trusted_device::{
    NewTrustedDevice, TRUSTED_DEVICE_LIMIT, TRUSTED_DEVICE_NAME_MAX_CHARS, TrustedDevice,
    TrustedDeviceMethod, TrustedDeviceRejectReason, TrustedDeviceRevokeReason,
    TrustedDeviceVerdict, extended_expiry, is_expired, is_valid_secret_hash,
    trusted_device_event_task_id,
};

use super::{SqliteStore, StoreError};

const COLUMNS: &str = "id, name, method, created_at, last_used_at, expires_at, absolute_expires_at, \
                       revoked_at, revoked_reason, actor, secret_hash, prev_secret_hash";

/// 行と、判定にだけ使う hash（外へは出さない）。
struct StoredDevice {
    device: TrustedDevice,
    secret_hash: String,
    prev_secret_hash: Option<String>,
}

fn read_row(r: &Row<'_>) -> rusqlite::Result<(StoredDevice, String, Option<String>)> {
    let method: String = r.get(2)?;
    let revoked_reason: Option<String> = r.get(8)?;
    Ok((
        StoredDevice {
            device: TrustedDevice {
                id: r.get(0)?,
                name: r.get(1)?,
                // 未知の方式・理由は読めるように既定へ倒す（変換の誤りで一覧が落ちないように）。
                method: TrustedDeviceMethod::parse(&method).unwrap_or(TrustedDeviceMethod::Cookie),
                created_at: r.get(3)?,
                last_used_at: r.get(4)?,
                expires_at: r.get(5)?,
                absolute_expires_at: r.get(6)?,
                revoked_at: r.get(7)?,
                revoked_reason: revoked_reason
                    .as_deref()
                    .and_then(TrustedDeviceRevokeReason::parse),
                actor: r.get(9)?,
            },
            secret_hash: r.get(10)?,
            prev_secret_hash: r.get(11)?,
        },
        method,
        revoked_reason,
    ))
}

fn load(conn: &Connection, id: &str) -> Result<Option<StoredDevice>, StoreError> {
    let row = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM browser_trusted_devices WHERE id = ?1"),
            params![id],
            read_row,
        )
        .optional()?;
    Ok(row.map(|(d, _, _)| d))
}

fn active_count(conn: &Connection, now: i64) -> Result<usize, StoreError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM browser_trusted_devices WHERE revoked_at IS NULL AND expires_at > ?1 \
         AND (absolute_expires_at IS NULL OR absolute_expires_at > ?1)",
        params![now],
        |r| r.get(0),
    )?;
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

fn check_hash(label: &str, hash: &str) -> Result<(), StoreError> {
    if is_valid_secret_hash(hash) {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!(
            "trusted device {label} must be a lowercase SHA-256 hex"
        )))
    }
}

impl SqliteStore {
    /// 端末を登録する（D2・D4）。有効な端末が既に上限（5 台）なら登録せず `Rejected(Limit)` を返し、
    /// `TrustedDeviceRejected` を残す。期限は `now + 90 日`（絶対上限があれば越えない）。
    pub fn trusted_device_register(
        &self,
        new: &NewTrustedDevice<'_>,
        now: i64,
    ) -> Result<TrustedDeviceVerdict, StoreError> {
        let name = new.name.trim();
        if new.id.is_empty() || new.id.len() > 64 {
            return Err(StoreError::Invalid(
                "trusted device id is empty or too long".into(),
            ));
        }
        if name.is_empty() || name.chars().count() > TRUSTED_DEVICE_NAME_MAX_CHARS {
            return Err(StoreError::Invalid(format!(
                "trusted device name must be 1..={TRUSTED_DEVICE_NAME_MAX_CHARS} characters"
            )));
        }
        if new.actor.is_empty() {
            return Err(StoreError::Invalid("trusted device actor is empty".into()));
        }
        check_hash("secret_hash", new.secret_hash)?;
        if new.absolute_expires_at.is_some_and(|abs| abs <= now) {
            return Err(StoreError::Invalid(
                "trusted device absolute_expires_at must be in the future".into(),
            ));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if active_count(&tx, now)? >= TRUSTED_DEVICE_LIMIT {
            Self::append_event_tx(
                &tx,
                trusted_device_event_task_id(),
                &Event::TrustedDeviceRejected {
                    device_id: None,
                    actor: new.actor.to_string(),
                    reason: TrustedDeviceRejectReason::Limit,
                },
            )?;
            tx.commit()?;
            return Ok(TrustedDeviceVerdict::Rejected(
                TrustedDeviceRejectReason::Limit,
            ));
        }
        let expires_at = extended_expiry(now, new.absolute_expires_at);
        tx.execute(
            "INSERT INTO browser_trusted_devices (id, name, method, secret_hash, prev_secret_hash, \
             created_at, last_used_at, expires_at, absolute_expires_at, revoked_at, revoked_reason, actor) \
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, NULL, ?6, ?7, NULL, NULL, ?8)",
            params![
                new.id,
                name,
                new.method.as_str(),
                new.secret_hash,
                now,
                expires_at,
                new.absolute_expires_at,
                new.actor
            ],
        )?;
        Self::append_event_tx(
            &tx,
            trusted_device_event_task_id(),
            &Event::TrustedDeviceRegistered {
                device_id: new.id.to_string(),
                name: name.to_string(),
                method: new.method,
                actor: new.actor.to_string(),
                expires_at,
                absolute_expires_at: new.absolute_expires_at,
            },
        )?;
        let device = load(&tx, new.id)?
            .ok_or_else(|| StoreError::Invalid("trusted device vanished after insert".into()))?
            .device;
        tx.commit()?;
        Ok(TrustedDeviceVerdict::Accepted(device))
    }

    /// 提示された秘密の hash を検証し、一致すれば `next_hash` へ回転して期限を延長する（D3・D4）。
    ///
    /// - 現行の hash と一致: `prev_secret_hash` ← 現行、`secret_hash` ← `next_hash`、`last_used_at` ← now、
    ///   `expires_at` ← `now + 90 日`（絶対上限があれば越えない）。`TrustedDeviceUsed`。
    /// - 回転前の hash と一致: 使い回しとして端末を失効（`reuse`）させ拒否する。`Revoked` と `Rejected`。
    /// - 未知の id・不一致・失効済み・期限切れ: 拒否する。`Rejected`。
    pub fn trusted_device_verify_and_rotate(
        &self,
        device_id: &str,
        presented_hash: &str,
        next_hash: &str,
        actor: &str,
        now: i64,
    ) -> Result<TrustedDeviceVerdict, StoreError> {
        check_hash("next_hash", next_hash)?;
        if next_hash == presented_hash {
            return Err(StoreError::Invalid(
                "trusted device next_hash must differ from the presented hash".into(),
            ));
        }
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = load(&tx, device_id)?;
        let reject = |tx: &Connection,
                      device_id: Option<&str>,
                      reason: TrustedDeviceRejectReason|
         -> Result<TrustedDeviceVerdict, StoreError> {
            Self::append_event_tx(
                tx,
                trusted_device_event_task_id(),
                &Event::TrustedDeviceRejected {
                    device_id: device_id.map(str::to_string),
                    actor: actor.to_string(),
                    reason,
                },
            )?;
            Ok(TrustedDeviceVerdict::Rejected(reason))
        };
        let verdict = match stored {
            None => reject(&tx, None, TrustedDeviceRejectReason::Unknown)?,
            Some(s) if s.device.revoked_at.is_some() => {
                reject(&tx, Some(device_id), TrustedDeviceRejectReason::Revoked)?
            }
            Some(s) if is_expired(s.device.expires_at, s.device.absolute_expires_at, now) => {
                reject(&tx, Some(device_id), TrustedDeviceRejectReason::Expired)?
            }
            Some(s) if s.secret_hash == presented_hash => {
                let expires_at = extended_expiry(now, s.device.absolute_expires_at);
                tx.execute(
                    "UPDATE browser_trusted_devices SET prev_secret_hash = secret_hash, \
                     secret_hash = ?2, last_used_at = ?3, expires_at = ?4 WHERE id = ?1",
                    params![device_id, next_hash, now, expires_at],
                )?;
                Self::append_event_tx(
                    &tx,
                    trusted_device_event_task_id(),
                    &Event::TrustedDeviceUsed {
                        device_id: device_id.to_string(),
                        actor: actor.to_string(),
                        expires_at,
                    },
                )?;
                let device = load(&tx, device_id)?
                    .ok_or_else(|| StoreError::Invalid("trusted device vanished".into()))?
                    .device;
                TrustedDeviceVerdict::Accepted(device)
            }
            Some(s) if s.prev_secret_hash.as_deref() == Some(presented_hash) => {
                Self::revoke_tx(&tx, device_id, actor, TrustedDeviceRevokeReason::Reuse, now)?;
                reject(&tx, Some(device_id), TrustedDeviceRejectReason::Reuse)?
            }
            Some(_) => reject(&tx, Some(device_id), TrustedDeviceRejectReason::Mismatch)?,
        };
        tx.commit()?;
        Ok(verdict)
    }

    /// probe 用の検証（D7）。判定は `trusted_device_verify_and_rotate` と同じだが、何も書かない
    /// （回転・最終使用・期限の延長・使い回しの失効・event のいずれも無し）。
    pub fn trusted_device_verify_readonly(
        &self,
        device_id: &str,
        presented_hash: &str,
        now: i64,
    ) -> Result<TrustedDeviceVerdict, StoreError> {
        let conn = self.lock()?;
        let verdict = match load(&conn, device_id)? {
            None => TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Unknown),
            Some(s) if s.device.revoked_at.is_some() => {
                TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Revoked)
            }
            Some(s) if is_expired(s.device.expires_at, s.device.absolute_expires_at, now) => {
                TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Expired)
            }
            Some(s) if s.secret_hash == presented_hash => TrustedDeviceVerdict::Accepted(s.device),
            Some(s) if s.prev_secret_hash.as_deref() == Some(presented_hash) => {
                TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Reuse)
            }
            Some(_) => TrustedDeviceVerdict::Rejected(TrustedDeviceRejectReason::Mismatch),
        };
        Ok(verdict)
    }

    /// 登録端末の一覧（失効・期限切れも含む。新しい順）。hash は返さない。
    pub fn trusted_device_list(&self) -> Result<Vec<TrustedDevice>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM browser_trusted_devices ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map([], read_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?.0.device);
        }
        Ok(out)
    }

    /// 端末 1 台（hash は返さない）。
    pub fn trusted_device_get(&self, device_id: &str) -> Result<Option<TrustedDevice>, StoreError> {
        let conn = self.lock()?;
        Ok(load(&conn, device_id)?.map(|s| s.device))
    }

    /// 人の失効（D4。`reason = owner`）。未知の id・既に失効済みなら何もせず `false`。
    pub fn trusted_device_revoke(
        &self,
        device_id: &str,
        actor: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let revoked = match load(&tx, device_id)? {
            Some(s) if s.device.revoked_at.is_none() => {
                Self::revoke_tx(&tx, device_id, actor, TrustedDeviceRevokeReason::Owner, now)?;
                true
            }
            _ => false,
        };
        tx.commit()?;
        Ok(revoked)
    }

    fn revoke_tx(
        conn: &Connection,
        device_id: &str,
        actor: &str,
        reason: TrustedDeviceRevokeReason,
        now: i64,
    ) -> Result<(), StoreError> {
        conn.execute(
            "UPDATE browser_trusted_devices SET revoked_at = ?2, revoked_reason = ?3 \
             WHERE id = ?1 AND revoked_at IS NULL",
            params![device_id, now, reason.as_str()],
        )?;
        Self::append_event_tx(
            conn,
            trusted_device_event_task_id(),
            &Event::TrustedDeviceRevoked {
                device_id: device_id.to_string(),
                actor: actor.to_string(),
                reason,
            },
        )?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "trusted_devices_tests.rs"]
mod tests;
