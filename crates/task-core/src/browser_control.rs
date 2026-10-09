//! ADR-0099 D3: browser の制御 lease（P3-C takeover）の状態機械。
//!
//! I/O も時計も持たない。呼び出し側が `now`（UNIX 秒）を渡す。ここが決めるのは
//! 「いま誰が browser を操作してよいか」だけで、ACL（誰が人として要求できるか）は
//! proxy 側（ADR-0099 D2）が先に確かめる。

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// 人の controller lease の既定の長さ（秒）。ADR-0080 H2 の lease と同じ短さに揃える。
pub const CONTROL_LEASE_DEFAULT_SECS: u64 = 60;
/// 人の controller lease の上限（秒）。延長は `Renew` で行い、1 回でこれを超えない。
pub const CONTROL_LEASE_MAX_SECS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlPhase {
    /// agent が controller。
    AgentRunning,
    /// pause を受けたが、実行中の agent 操作がまだ収束していない。
    Pausing,
    /// 誰も操作していない。切断・lease 切れの後もここに留まる（自動再開しない）。
    Paused,
    /// 人が期限付き lease を持つ。
    HumanControl,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlLease {
    pub holder: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlCommand {
    Pause,
    Takeover {
        holder: String,
        ttl_secs: Option<u64>,
    },
    Renew {
        holder: String,
        ttl_secs: Option<u64>,
    },
    /// `fresh_snapshot` と `policy_origin_ok` は呼び出し側が再確認した結果。
    Resume {
        holder: String,
        fresh_snapshot: bool,
        policy_origin_ok: bool,
    },
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlRequest {
    pub command: ControlCommand,
    pub expected_version: u64,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlOutcome {
    pub phase: ControlPhase,
    pub version: u64,
    pub lease_expires_at: Option<u64>,
    /// 同じ idempotency key の再送で、状態を変えずに前回の結果を返した。
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    MissingIdempotencyKey,
    /// 同じ key が別の command に使われた。
    IdempotencyConflict,
    VersionConflict {
        expected: u64,
        actual: u64,
    },
    InvalidPhase {
        phase: ControlPhase,
    },
    /// pause が収束していない（実行中の agent 操作が残っている）。
    NotConverged {
        in_flight: u32,
    },
    LeaseTooLong {
        requested: u64,
        max: u64,
    },
    /// lease の持ち主ではない。
    NotLeaseHolder,
    LeaseExpired,
    /// credential を注入した session（ADR-0080 H3）。人の takeover も Live View も開かない。
    AuthSectionActive,
    /// resume の前の fresh snapshot / policy・origin の再確認が済んでいない。
    ResumeNotVerified,
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingIdempotencyKey => write!(f, "idempotency_key_required"),
            Self::IdempotencyConflict => write!(f, "idempotency_key_conflict"),
            Self::VersionConflict { expected, actual } => {
                write!(f, "version_conflict: expected {expected}, actual {actual}")
            }
            Self::InvalidPhase { phase } => write!(f, "invalid_phase: {phase:?}"),
            Self::NotConverged { in_flight } => {
                write!(f, "pause_not_converged: {in_flight} in flight")
            }
            Self::LeaseTooLong { requested, max } => {
                write!(f, "lease_too_long: {requested} > {max}")
            }
            Self::NotLeaseHolder => write!(f, "not_lease_holder"),
            Self::LeaseExpired => write!(f, "lease_expired"),
            Self::AuthSectionActive => write!(f, "auth_section_active"),
            Self::ResumeNotVerified => write!(f, "resume_not_verified"),
        }
    }
}

impl std::error::Error for ControlError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserControl {
    phase: ControlPhase,
    version: u64,
    in_flight: u32,
    lease: Option<ControlLease>,
    auth_section_active: bool,
    /// ADR 2026-10-09 credential username / post-login D2-3: credential を注入した session の印。
    /// 認証区間が閉じた後（ログイン後の読み取り）も session の終わりまで残り、takeover・renew を拒否する。
    #[serde(default)]
    credential_used: bool,
    applied: BTreeMap<String, (ControlCommand, ControlOutcome)>,
}

impl Default for BrowserControl {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserControl {
    pub fn new() -> Self {
        Self {
            phase: ControlPhase::AgentRunning,
            version: 0,
            in_flight: 0,
            lease: None,
            auth_section_active: false,
            credential_used: false,
            applied: BTreeMap::new(),
        }
    }

    pub fn phase(&self) -> ControlPhase {
        self.phase
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn lease(&self) -> Option<&ControlLease> {
        self.lease.as_ref()
    }

    /// 実行中の agent 操作の数（in-flight 完了報告の待ち）。
    pub fn in_flight(&self) -> u32 {
        self.in_flight
    }

    pub fn auth_section_active(&self) -> bool {
        self.auth_section_active
    }

    /// credential を注入した session か（区間が閉じても session の終わりまで真）。
    pub fn credential_used(&self) -> bool {
        self.credential_used
    }

    /// agent の操作を 1 つ始める。agent が controller のときだけ通る。
    pub fn begin_agent_action(&mut self) -> Result<(), ControlError> {
        if self.phase != ControlPhase::AgentRunning {
            return Err(ControlError::InvalidPhase { phase: self.phase });
        }
        self.in_flight = self.in_flight.saturating_add(1);
        Ok(())
    }

    /// agent の操作が 1 つ終わった。pause 中で残りが無くなれば `Paused` に収束する。
    pub fn end_agent_action(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if self.phase == ControlPhase::Pausing && self.in_flight == 0 {
            self.transition(ControlPhase::Paused);
        }
    }

    /// 人の操作を 1 つ通してよいか。lease の持ち主・期限内・`HumanControl` の全てを要する。
    pub fn authorize_human_action(&mut self, holder: &str, now: u64) -> Result<(), ControlError> {
        self.check_holder(holder, now)
    }

    /// credential を注入した。session の終わりまで戻らない（ADR-0080 H3）。
    /// 人が lease を持っていれば取り上げる。
    pub fn enter_auth_section(&mut self) {
        self.auth_section_active = true;
        self.credential_used = true;
        if self.phase == ControlPhase::HumanControl {
            self.lease = None;
            self.transition(ControlPhase::Paused);
        }
    }

    /// credential 注入（認証区間）が終わった。takeover・renew を再び受け付ける。
    /// 取り上げた lease は戻さない（人が改めて takeover する）。
    pub fn leave_auth_section(&mut self) {
        self.auth_section_active = false;
    }

    /// 人の接続が切れた。lease を失効させ、agent は自動再開しない。
    pub fn human_disconnected(&mut self, holder: &str) {
        let held = self.lease.as_ref().is_some_and(|l| l.holder == holder);
        if self.phase == ControlPhase::HumanControl && held {
            self.lease = None;
            self.transition(ControlPhase::Paused);
        }
    }

    /// 期限の切れた lease を失効させる。失効したら true。
    pub fn expire(&mut self, now: u64) -> bool {
        let expired = self.lease.as_ref().is_some_and(|l| l.expires_at <= now);
        if expired {
            self.lease = None;
            if self.phase == ControlPhase::HumanControl {
                self.transition(ControlPhase::Paused);
            }
        }
        expired
    }

    pub fn apply(
        &mut self,
        req: &ControlRequest,
        now: u64,
    ) -> Result<ControlOutcome, ControlError> {
        if req.idempotency_key.trim().is_empty() {
            return Err(ControlError::MissingIdempotencyKey);
        }
        if let Some((command, outcome)) = self.applied.get(&req.idempotency_key) {
            if *command != req.command {
                return Err(ControlError::IdempotencyConflict);
            }
            return Ok(ControlOutcome {
                replayed: true,
                ..outcome.clone()
            });
        }
        self.expire(now);
        if req.expected_version != self.version {
            return Err(ControlError::VersionConflict {
                expected: req.expected_version,
                actual: self.version,
            });
        }
        match &req.command {
            ControlCommand::Pause => self.pause()?,
            ControlCommand::Takeover { holder, ttl_secs } => {
                self.takeover(holder, *ttl_secs, now)?
            }
            ControlCommand::Renew { holder, ttl_secs } => self.renew(holder, *ttl_secs, now)?,
            ControlCommand::Resume {
                holder,
                fresh_snapshot,
                policy_origin_ok,
            } => self.resume(holder, *fresh_snapshot && *policy_origin_ok, now)?,
            ControlCommand::Stop => self.stop()?,
        }
        let outcome = ControlOutcome {
            phase: self.phase,
            version: self.version,
            lease_expires_at: self.lease.as_ref().map(|l| l.expires_at),
            replayed: false,
        };
        self.applied.insert(
            req.idempotency_key.clone(),
            (req.command.clone(), outcome.clone()),
        );
        Ok(outcome)
    }

    fn transition(&mut self, phase: ControlPhase) {
        self.phase = phase;
        self.version = self.version.saturating_add(1);
    }

    fn lease_ttl(ttl_secs: Option<u64>) -> Result<u64, ControlError> {
        let ttl = match ttl_secs {
            None | Some(0) => CONTROL_LEASE_DEFAULT_SECS,
            Some(t) => t,
        };
        if ttl > CONTROL_LEASE_MAX_SECS {
            return Err(ControlError::LeaseTooLong {
                requested: ttl,
                max: CONTROL_LEASE_MAX_SECS,
            });
        }
        Ok(ttl)
    }

    fn check_holder(&mut self, holder: &str, now: u64) -> Result<(), ControlError> {
        if self.expire(now) {
            return Err(ControlError::LeaseExpired);
        }
        if self.phase != ControlPhase::HumanControl {
            return Err(ControlError::InvalidPhase { phase: self.phase });
        }
        match &self.lease {
            Some(l) if l.holder == holder => Ok(()),
            _ => Err(ControlError::NotLeaseHolder),
        }
    }

    fn pause(&mut self) -> Result<(), ControlError> {
        if self.phase != ControlPhase::AgentRunning {
            return Err(ControlError::InvalidPhase { phase: self.phase });
        }
        let next = if self.in_flight == 0 {
            ControlPhase::Paused
        } else {
            ControlPhase::Pausing
        };
        self.transition(next);
        Ok(())
    }

    fn takeover(
        &mut self,
        holder: &str,
        ttl_secs: Option<u64>,
        now: u64,
    ) -> Result<(), ControlError> {
        if self.auth_section_active || self.credential_used {
            return Err(ControlError::AuthSectionActive);
        }
        match self.phase {
            ControlPhase::Paused => {}
            ControlPhase::Pausing => {
                return Err(ControlError::NotConverged {
                    in_flight: self.in_flight,
                });
            }
            ControlPhase::HumanControl => return Err(ControlError::NotLeaseHolder),
            phase => return Err(ControlError::InvalidPhase { phase }),
        }
        let ttl = Self::lease_ttl(ttl_secs)?;
        self.lease = Some(ControlLease {
            holder: holder.to_string(),
            expires_at: now.saturating_add(ttl),
        });
        self.transition(ControlPhase::HumanControl);
        Ok(())
    }

    fn renew(&mut self, holder: &str, ttl_secs: Option<u64>, now: u64) -> Result<(), ControlError> {
        if self.auth_section_active || self.credential_used {
            return Err(ControlError::AuthSectionActive);
        }
        self.check_holder(holder, now)?;
        let ttl = Self::lease_ttl(ttl_secs)?;
        self.lease = Some(ControlLease {
            holder: holder.to_string(),
            expires_at: now.saturating_add(ttl),
        });
        self.version = self.version.saturating_add(1);
        Ok(())
    }

    fn resume(&mut self, holder: &str, verified: bool, now: u64) -> Result<(), ControlError> {
        match self.phase {
            ControlPhase::Paused => {}
            ControlPhase::HumanControl => self.check_holder(holder, now)?,
            phase => return Err(ControlError::InvalidPhase { phase }),
        }
        if !verified {
            return Err(ControlError::ResumeNotVerified);
        }
        self.lease = None;
        self.transition(ControlPhase::AgentRunning);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ControlError> {
        if self.phase == ControlPhase::Stopped {
            return Err(ControlError::InvalidPhase { phase: self.phase });
        }
        self.lease = None;
        self.transition(ControlPhase::Stopped);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(command: ControlCommand, version: u64, key: &str) -> ControlRequest {
        ControlRequest {
            command,
            expected_version: version,
            idempotency_key: key.to_string(),
        }
    }

    fn takeover(holder: &str, ttl: Option<u64>) -> ControlCommand {
        ControlCommand::Takeover {
            holder: holder.to_string(),
            ttl_secs: ttl,
        }
    }

    fn resume(holder: &str, ok: bool) -> ControlCommand {
        ControlCommand::Resume {
            holder: holder.to_string(),
            fresh_snapshot: ok,
            policy_origin_ok: ok,
        }
    }

    fn paused() -> BrowserControl {
        let mut c = BrowserControl::new();
        c.apply(&req(ControlCommand::Pause, 0, "p"), 100).unwrap();
        c
    }

    fn human(holder: &str) -> BrowserControl {
        let mut c = paused();
        c.apply(&req(takeover(holder, None), 1, "t"), 100).unwrap();
        c
    }

    #[test]
    fn pause_waits_for_in_flight_actions_before_takeover() {
        let mut c = BrowserControl::new();
        c.begin_agent_action().unwrap();
        let out = c.apply(&req(ControlCommand::Pause, 0, "p"), 100).unwrap();
        assert_eq!(out.phase, ControlPhase::Pausing);
        // 新しい agent 操作は止まる。
        assert_eq!(
            c.begin_agent_action(),
            Err(ControlError::InvalidPhase {
                phase: ControlPhase::Pausing
            })
        );
        // 収束前の takeover は拒否。
        assert_eq!(
            c.apply(&req(takeover("owner", None), 1, "t0"), 100),
            Err(ControlError::NotConverged { in_flight: 1 })
        );
        c.end_agent_action();
        assert_eq!(c.phase(), ControlPhase::Paused);
        let out = c
            .apply(&req(takeover("owner", None), 2, "t1"), 100)
            .unwrap();
        assert_eq!(out.phase, ControlPhase::HumanControl);
        assert_eq!(out.lease_expires_at, Some(100 + CONTROL_LEASE_DEFAULT_SECS));
    }

    #[test]
    fn lease_is_exclusive_between_humans_and_against_the_agent() {
        let mut c = human("owner");
        let v = c.version();
        assert_eq!(
            c.apply(&req(takeover("other", None), v, "t2"), 110),
            Err(ControlError::NotLeaseHolder)
        );
        assert_eq!(
            c.authorize_human_action("other", 110),
            Err(ControlError::NotLeaseHolder)
        );
        assert!(c.begin_agent_action().is_err());
        assert_eq!(c.authorize_human_action("owner", 110), Ok(()));
        assert_eq!(c.lease().map(|l| l.holder.as_str()), Some("owner"));
    }

    #[test]
    fn lease_ttl_is_bounded() {
        let mut c = paused();
        assert_eq!(
            c.apply(&req(takeover("owner", Some(301)), 1, "t"), 100),
            Err(ControlError::LeaseTooLong {
                requested: 301,
                max: CONTROL_LEASE_MAX_SECS
            })
        );
        let out = c
            .apply(&req(takeover("owner", Some(300)), 1, "t300"), 100)
            .unwrap();
        assert_eq!(out.lease_expires_at, Some(400));
        let renew = ControlCommand::Renew {
            holder: "owner".to_string(),
            ttl_secs: Some(301),
        };
        assert!(matches!(
            c.apply(&req(renew, out.version, "r"), 120),
            Err(ControlError::LeaseTooLong { .. })
        ));
    }

    #[test]
    fn expired_lease_falls_back_to_paused_without_resuming_the_agent() {
        let mut c = human("owner");
        assert_eq!(
            c.authorize_human_action("owner", 160),
            Err(ControlError::LeaseExpired)
        );
        assert_eq!(c.phase(), ControlPhase::Paused);
        assert!(c.lease().is_none());
        assert!(c.begin_agent_action().is_err());
    }

    #[test]
    fn disconnect_revokes_the_lease_and_does_not_resume() {
        let mut c = human("owner");
        // 他人の切断では何も起きない。
        c.human_disconnected("other");
        assert_eq!(c.phase(), ControlPhase::HumanControl);
        c.human_disconnected("owner");
        assert_eq!(c.phase(), ControlPhase::Paused);
        assert!(c.lease().is_none());
        assert!(c.begin_agent_action().is_err());
        assert_eq!(
            c.authorize_human_action("owner", 110),
            Err(ControlError::InvalidPhase {
                phase: ControlPhase::Paused
            })
        );
    }

    #[test]
    fn stale_version_loses_the_race() {
        let mut c = paused();
        c.apply(&req(takeover("owner", None), 1, "a"), 100).unwrap();
        // 同じ version を見ていた競合相手。
        assert_eq!(
            c.apply(&req(resume("x", true), 1, "b"), 100),
            Err(ControlError::VersionConflict {
                expected: 1,
                actual: 2
            })
        );
        assert_eq!(c.phase(), ControlPhase::HumanControl);
    }

    #[test]
    fn idempotent_retry_does_not_act_twice() {
        let mut c = paused();
        let first = c.apply(&req(takeover("owner", None), 1, "k"), 100).unwrap();
        let again = c.apply(&req(takeover("owner", None), 1, "k"), 130).unwrap();
        assert!(again.replayed);
        assert_eq!(again.version, first.version);
        // 再送で lease は延びない。
        assert_eq!(again.lease_expires_at, first.lease_expires_at);
        assert_eq!(c.version(), first.version);
        assert_eq!(
            c.apply(&req(ControlCommand::Stop, first.version, "k"), 130),
            Err(ControlError::IdempotencyConflict)
        );
        assert_eq!(
            c.apply(&req(ControlCommand::Stop, first.version, " "), 130),
            Err(ControlError::MissingIdempotencyKey)
        );
    }

    #[test]
    fn resume_requires_reverification_and_returns_control_to_the_agent() {
        let mut c = human("owner");
        let v = c.version();
        assert_eq!(
            c.apply(&req(resume("owner", false), v, "r0"), 110),
            Err(ControlError::ResumeNotVerified)
        );
        assert_eq!(
            c.apply(&req(resume("other", true), v, "r1"), 110),
            Err(ControlError::NotLeaseHolder)
        );
        let out = c.apply(&req(resume("owner", true), v, "r2"), 110).unwrap();
        assert_eq!(out.phase, ControlPhase::AgentRunning);
        assert_eq!(out.lease_expires_at, None);
        assert_eq!(c.begin_agent_action(), Ok(()));
        assert!(c.authorize_human_action("owner", 110).is_err());
    }

    #[test]
    fn stop_revokes_the_lease_and_ends_all_control() {
        let mut c = human("owner");
        let v = c.version();
        let out = c.apply(&req(ControlCommand::Stop, v, "s"), 110).unwrap();
        assert_eq!(out.phase, ControlPhase::Stopped);
        assert!(c.lease().is_none());
        assert!(c.begin_agent_action().is_err());
        assert!(c.authorize_human_action("owner", 110).is_err());
        assert!(
            c.apply(&req(resume("owner", true), out.version, "r"), 110)
                .is_err()
        );
        assert!(
            c.apply(&req(ControlCommand::Stop, out.version, "s2"), 110)
                .is_err()
        );
    }

    #[test]
    fn auth_section_revokes_human_control_and_blocks_takeover() {
        let mut c = human("owner");
        c.enter_auth_section();
        assert_eq!(c.phase(), ControlPhase::Paused);
        assert!(c.lease().is_none());
        let v = c.version();
        assert_eq!(
            c.apply(&req(takeover("owner", None), v, "t2"), 110),
            Err(ControlError::AuthSectionActive)
        );
        assert!(c.auth_section_active());
    }

    /// ADR 2026-10-09 credential username / post-login D2-3: 区間が閉じた後（ログイン後の読み取り）も
    /// credential を使った session は takeover・renew を拒否し、agent の操作だけが戻る。
    #[test]
    fn post_login_credential_session_keeps_takeover_refused_after_auth_section() {
        let mut c = BrowserControl::new();
        c.enter_auth_section();
        assert!(c.begin_agent_action().is_ok());
        c.end_agent_action();
        c.leave_auth_section();
        assert!(!c.auth_section_active());
        assert!(c.credential_used());
        assert!(c.begin_agent_action().is_ok());
        c.end_agent_action();
        let v = c.version();
        let out = c.apply(&req(ControlCommand::Pause, v, "p"), 110).unwrap();
        assert_eq!(
            c.apply(&req(takeover("owner", None), out.version, "t"), 110),
            Err(ControlError::AuthSectionActive)
        );
        // 旧い state（欄なし）は credential を使っていない session として読める。
        let mut json = serde_json::to_value(BrowserControl::new()).unwrap();
        json.as_object_mut().unwrap().remove("credential_used");
        let old: BrowserControl = serde_json::from_value(json).unwrap();
        assert!(!old.credential_used());
    }
}
