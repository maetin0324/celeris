//! ADR-0081 D3: worker・人の接続・task cancel からの control 状態の遷移。
//!
//! どれも 1 つの IMMEDIATE トランザクションで読み・遷移・書き戻しを行う。

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::browser_control::{
    BrowserControl, ControlCommand, ControlError, ControlPhase, ControlRequest,
};
use crate::browser_store::{BrowserSessionKey, BrowserStoreError};
use crate::store::{SqliteStore, StoreError};

impl SqliteStore {
    /// control 状態を読み、`f` で遷移させ、成功したときだけ書き戻す。
    pub fn browser_control_mutate<T>(
        &self,
        key: BrowserSessionKey<'_>,
        f: impl FnOnce(&mut BrowserControl) -> Result<T, ControlError>,
    ) -> Result<T, BrowserStoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let json: Option<String> = tx
            .query_row(
                "SELECT state_json FROM browser_control_state WHERE task_id=?1 AND run_id=?2 AND session_id=?3",
                params![key.task_id, key.run_id, key.session_id],
                |r| r.get(0),
            )
            .optional()?;
        let mut state: BrowserControl = match json {
            Some(json) => serde_json::from_str(&json).map_err(StoreError::from)?,
            None => BrowserControl::new(),
        };
        let value = f(&mut state)?;
        let phase = serde_json::to_string(&state.phase()).map_err(StoreError::from)?;
        let state_json = serde_json::to_string(&state).map_err(StoreError::from)?;
        tx.execute(
            "INSERT INTO browser_control_state(task_id,run_id,session_id,phase,version,lease_holder,lease_expires_at,state_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(task_id,run_id,session_id) DO UPDATE SET phase=excluded.phase,version=excluded.version,lease_holder=excluded.lease_holder,lease_expires_at=excluded.lease_expires_at,state_json=excluded.state_json",
            params![
                key.task_id,
                key.run_id,
                key.session_id,
                phase,
                state.version(),
                state.lease().map(|l| l.holder.as_str()),
                state.lease().map(|l| l.expires_at),
                state_json
            ],
        )?;
        tx.commit()?;
        Ok(value)
    }

    /// ADR-0080 H3: 認証区間（credential 注入）の開始/終了。task-api の `auth-section`
    /// endpoint と worker の supervisor（`EventSink::browser_auth_section`）が共に使う唯一の口。
    pub fn browser_control_auth_section(
        &self,
        key: BrowserSessionKey<'_>,
        active: bool,
    ) -> Result<BrowserControl, BrowserStoreError> {
        self.browser_control_mutate(key, |s| {
            if active {
                s.enter_auth_section();
            } else {
                s.leave_auth_section();
            }
            Ok(s.clone())
        })
    }

    /// task cancel: その task の全 session を `Stopped` にし、lease を失効させる。
    /// 止めた session の数を返す。
    pub fn browser_control_stop_task(
        &self,
        task_id: &str,
        now: u64,
    ) -> Result<usize, BrowserStoreError> {
        let sessions: Vec<(String, String)> = {
            let conn = self.lock()?;
            let mut stmt = conn
                .prepare("SELECT run_id, session_id FROM browser_control_state WHERE task_id=?1")?;
            stmt.query_map(params![task_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?
        };
        let mut stopped = 0;
        for (run_id, session_id) in &sessions {
            let key = BrowserSessionKey {
                task_id,
                run_id,
                session_id,
            };
            let changed = self.browser_control_mutate(key, |s| {
                s.expire(now);
                if s.phase() == ControlPhase::Stopped {
                    return Ok(false);
                }
                let req = ControlRequest {
                    command: ControlCommand::Stop,
                    expected_version: s.version(),
                    idempotency_key: format!("task-cancel:{}", s.version()),
                };
                s.apply(&req, now).map(|_| true)
            })?;
            stopped += usize::from(changed);
        }
        Ok(stopped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser_control::{CONTROL_LEASE_DEFAULT_SECS, CONTROL_LEASE_MAX_SECS};

    const KEY: BrowserSessionKey<'static> = BrowserSessionKey {
        task_id: "T1",
        run_id: "R1",
        session_id: "S1",
    };

    fn req(command: ControlCommand, v: u64, k: &str) -> ControlRequest {
        ControlRequest {
            command,
            expected_version: v,
            idempotency_key: k.into(),
        }
    }
    fn takeover(holder: &str, ttl: Option<u64>) -> ControlCommand {
        ControlCommand::Takeover {
            holder: holder.into(),
            ttl_secs: ttl,
        }
    }
    fn control_err(e: BrowserStoreError) -> Option<ControlError> {
        match e {
            BrowserStoreError::Control(c) => Some(c),
            _ => None,
        }
    }

    #[test]
    fn conflict_replay_key_reuse_and_other_lease() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        let p = store.browser_control_apply(KEY, &req(ControlCommand::Pause, 0, "p"), 100)?;
        assert_eq!(p.phase, ControlPhase::Paused);
        // 古い version は version_conflict。
        let e = store.browser_control_apply(KEY, &req(takeover("me", None), 0, "t"), 100);
        assert!(matches!(
            e.err().and_then(control_err),
            Some(ControlError::VersionConflict { .. })
        ));
        let t =
            store.browser_control_apply(KEY, &req(takeover("me", None), p.version, "t"), 100)?;
        assert_eq!(t.lease_expires_at, Some(100 + CONTROL_LEASE_DEFAULT_SECS));
        // 同じ key の再送は前回の結果を返し、lease を延ばさない。
        let again =
            store.browser_control_apply(KEY, &req(takeover("me", None), p.version, "t"), 150)?;
        assert!(again.replayed);
        assert_eq!(again.lease_expires_at, t.lease_expires_at);
        assert_eq!(store.browser_control_get(KEY)?.version(), t.version);
        // 同じ key を別 command に流用すると拒否。
        let e = store.browser_control_apply(KEY, &req(ControlCommand::Stop, t.version, "t"), 150);
        assert_eq!(
            e.err().and_then(control_err),
            Some(ControlError::IdempotencyConflict)
        );
        // 他人は lease を延長・奪取できない。
        let renew = ControlCommand::Renew {
            holder: "other".into(),
            ttl_secs: None,
        };
        let e = store.browser_control_apply(KEY, &req(renew, t.version, "r"), 150);
        assert_eq!(
            e.err().and_then(control_err),
            Some(ControlError::NotLeaseHolder)
        );
        let e =
            store.browser_control_apply(KEY, &req(takeover("other", None), t.version, "o"), 150);
        assert_eq!(
            e.err().and_then(control_err),
            Some(ControlError::NotLeaseHolder)
        );
        Ok(())
    }

    #[test]
    fn lease_limits_expiry_and_no_auto_resume() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        let p = store.browser_control_apply(KEY, &req(ControlCommand::Pause, 0, "p"), 100)?;
        let e = store.browser_control_apply(
            KEY,
            &req(
                takeover("me", Some(CONTROL_LEASE_MAX_SECS + 1)),
                p.version,
                "long",
            ),
            100,
        );
        assert!(matches!(
            e.err().and_then(control_err),
            Some(ControlError::LeaseTooLong { .. })
        ));
        let t =
            store.browser_control_apply(KEY, &req(takeover("me", None), p.version, "t"), 100)?;
        assert_eq!(t.lease_expires_at, Some(160));
        // 切断相当: lease 切れで Paused に戻り、agent は自動再開しない。
        let expired = store.browser_control_mutate(KEY, |s| Ok(s.expire(161)))?;
        assert!(expired);
        let s = store.browser_control_get(KEY)?;
        assert_eq!(s.phase(), ControlPhase::Paused);
        assert!(s.lease().is_none());
        let begin = store.browser_control_mutate(KEY, |s| s.begin_agent_action());
        assert!(matches!(
            begin.err().and_then(control_err),
            Some(ControlError::InvalidPhase { .. })
        ));
        // 明示的な切断も同じ。
        let v = store.browser_control_get(KEY)?.version();
        store.browser_control_apply(KEY, &req(takeover("me", None), v, "t2"), 200)?;
        store.browser_control_mutate(KEY, |s| {
            s.human_disconnected("me");
            Ok(())
        })?;
        assert_eq!(
            store.browser_control_get(KEY)?.phase(),
            ControlPhase::Paused
        );
        Ok(())
    }

    #[test]
    fn auth_section_revokes_lease_and_rejects_takeover() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        let p = store.browser_control_apply(KEY, &req(ControlCommand::Pause, 0, "p"), 100)?;
        store.browser_control_apply(KEY, &req(takeover("me", None), p.version, "t"), 100)?;
        store.browser_control_mutate(KEY, |s| {
            s.enter_auth_section();
            Ok(())
        })?;
        let s = store.browser_control_get(KEY)?;
        assert_eq!(s.phase(), ControlPhase::Paused);
        assert!(s.lease().is_none());
        let e =
            store.browser_control_apply(KEY, &req(takeover("me", None), s.version(), "t2"), 101);
        assert_eq!(
            e.err().and_then(control_err),
            Some(ControlError::AuthSectionActive)
        );
        Ok(())
    }

    #[test]
    fn task_cancel_stops_and_rejects_later_ops() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        let p = store.browser_control_apply(KEY, &req(ControlCommand::Pause, 0, "p"), 100)?;
        store.browser_control_apply(KEY, &req(takeover("me", None), p.version, "t"), 100)?;
        assert_eq!(store.browser_control_stop_task("T1", 110)?, 1);
        let s = store.browser_control_get(KEY)?;
        assert_eq!(s.phase(), ControlPhase::Stopped);
        assert!(s.lease().is_none());
        // 二度目は何もしない。
        assert_eq!(store.browser_control_stop_task("T1", 111)?, 0);
        let renew = ControlCommand::Renew {
            holder: "me".into(),
            ttl_secs: None,
        };
        assert!(
            store
                .browser_control_apply(KEY, &req(renew, s.version(), "r"), 112)
                .is_err()
        );
        let e =
            store.browser_control_apply(KEY, &req(takeover("me", None), s.version(), "t9"), 112);
        assert!(matches!(
            e.err().and_then(control_err),
            Some(ControlError::InvalidPhase {
                phase: ControlPhase::Stopped
            })
        ));
        assert!(
            store
                .browser_control_mutate(KEY, |s| s.begin_agent_action())
                .is_err()
        );
        Ok(())
    }
}

/// worker の agent 操作 gate の op（ADR-0094 D2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentActionOp {
    /// 状態を読むだけ。
    Read,
    /// 操作を 1 つ始める。lease 期限切れを先に失効させ、`AgentRunning` 以外と auth_section 中は拒否する。
    Begin,
    /// 操作が 1 つ終わった。pause 中で最後なら `Paused` に収束する。
    End,
}

impl SqliteStore {
    /// ADR-0094 D2: task-api の `agent/begin`・`agent/end` と同じ遷移（auth_section 中の begin は拒否）。
    pub fn browser_control_agent_action(
        &self,
        key: BrowserSessionKey<'_>,
        op: AgentActionOp,
        now: u64,
    ) -> Result<BrowserControl, BrowserStoreError> {
        match op {
            AgentActionOp::Read => Ok(self.browser_control_get(key)?),
            AgentActionOp::Begin => self.browser_control_mutate(key, |s| {
                s.expire(now);
                if s.auth_section_active() {
                    return Err(ControlError::AuthSectionActive);
                }
                s.begin_agent_action()?;
                Ok(s.clone())
            }),
            AgentActionOp::End => self.browser_control_mutate(key, |s| {
                s.end_agent_action();
                Ok(s.clone())
            }),
        }
    }
}
