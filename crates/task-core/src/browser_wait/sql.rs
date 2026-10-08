//! Durable wait storage and transaction handling.

use super::*;

// ---- SQL ----

const SELECT_WAIT: &str = "SELECT wait_id, task_id, work_unit_id, run_id, session_id, reason, origin, \
     purpose, credential_id, credential_provider, credential_policy_id, operation_intent_id, action, \
     args_digest, approval_id, policy_revision, policy_hash, owner_id, deadline, resume_key, version, \
     state, resolution_code, created_at, resolved_at, trusted_login_json FROM browser_waits";

type RawWait = (
    [String; 7],
    [Option<String>; 8],
    (i64, String, Option<String>, String, String, i64, String),
    (Option<String>, String, Option<String>),
    Option<String>,
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
        row.get(25)?,
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
        trusted_login_json,
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
    let trusted_login = trusted_login_json
        .as_deref()
        .map(serde_json::from_str::<TrustedLogin>)
        .transpose()
        .map_err(|e| StoreError::Invalid(format!("invalid browser wait trusted login: {e}")))?;
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
        trusted_login,
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
    fn browser_session_auth_section(
        &self,
        key: crate::browser_store::BrowserSessionKey<'_>,
        active: bool,
    ) -> Result<(), crate::browser_store::BrowserStoreError> {
        self.browser_control_auth_section(key, active).map(|_| ())
    }
    fn browser_session_agent_action(
        &self,
        key: crate::browser_store::BrowserSessionKey<'_>,
        op: crate::browser_control_ops::AgentActionOp,
        now: u64,
    ) -> Result<crate::browser_control::BrowserControl, crate::browser_store::BrowserStoreError>
    {
        self.browser_control_agent_action(key, op, now)
    }
    fn browser_session_live_append(
        &self,
        key: crate::browser_store::BrowserSessionKey<'_>,
        event: &crate::browser_live::ScrubbedLiveEvent,
    ) -> Result<u64, StoreError> {
        self.browser_live_append(key, event)
    }
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
            // ADR 2026-10-08-browser-prod-enablement D4: D2 の前提 gate で止めた task も編集を受ける
            // （人待ちの `blocked` は binding hash があるので変えない）。
            Some("blocked")
                if SqliteStore::last_transitioned_reason_tx(&tx, task_id)?.as_deref()
                    == Some(crate::browser_prerequisite::REASON_BLOCKED) => {}
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
                trusted_login: new.trusted_login.clone(),
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
            let trusted_login_json = wait
                .trusted_login
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| BrowserWaitError::Invalid {
                    field: "trusted_login",
                })?;
            tx.execute(
                "INSERT INTO browser_waits (wait_id, task_id, work_unit_id, run_id, session_id, reason, \
                 origin, purpose, credential_id, credential_provider, credential_policy_id, \
                 operation_intent_id, action, args_digest, approval_id, policy_revision, policy_hash, \
                 owner_id, deadline, resume_key, version, state, resolution_code, created_at, \
                 resolved_at, trusted_login_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
                 ?11, ?12, ?13, ?14, NULL, ?15, ?16, ?17, ?18, ?19, ?20, ?21, NULL, ?22, NULL, ?23)",
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
                    trusted_login_json,
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
