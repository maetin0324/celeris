//! ADR 2026-11-04-project-controller D12・A1: `project_controller_state`・`project_wakes`・
//! `project_controller_decisions`（migration 0112）の読み書き。LLM 呼び出しは無い。
//!
//! 書き込みはどれも 1 つの IMMEDIATE transaction で、再起動後は表の行から続く:
//! - 取り込み: wake の `INSERT OR IGNORE`（`UNIQUE(project_id, wake_key)`）と全体の cursor（`feed_cursor`）を
//!   同じ transaction で書く。同じ event の再配送は 1 行に束なる。
//! - lease: 期限つき。切れた lease は他の instance が引き継ぐ。
//! - tx1（[`SqliteStore::project_controller_commit`]）: `revision`・`human_epoch`・lease を照合し（CAS、I6）、
//!   判断の行・wake の消費・cursor・`revision + 1`・効果 `pending` を 1 回で書く。
//! - tx2: 効果を冪等に実行し（root は `task_idempotency_keys`、CoS 入力は `client_message_id`）、`applied` へ進める。

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use time::OffsetDateTime;

use super::project_wake_effect::effect_gate_open_tx;
use super::{IdempotencyClaim, IdempotencyLookup, SqliteStore, StoreError, format_rfc3339};
use crate::model::Task;
use crate::project_wake::{
    ControllerMode, ControllerPhase, DRY_RUN_WAKE_PREFIX, EffectState, NewProjectWake,
    ProjectControllerDecision, ProjectControllerState, ProjectWake, ProjectWakeKind, StaleReason,
    backoff_secs, human_wake_key,
};

/// lease で一度に畳む wake の上限。残りは次の評価に回る。
pub const WAKES_PER_EVALUATION: usize = 256;

const STATE_COLS: &str = "project_id, mode, phase, revision, event_cursor, next_wake_at, \
    coalesce_until, lease_owner, lease_expires_at, retry_count, backoff_until, last_decision_id, \
    human_epoch, progress_epoch, updated_at, updated_by";
const WAKE_COLS: &str =
    "id, project_id, wake_key, kind, source_event_id, source_ref, created_at, consumed_by";
const DECISION_COLS: &str = "id, project_id, based_on_revision, human_epoch, snapshot_sha256, \
    request_sha256, generation_id, mode, proposal_json, validation, effect_json, effect_state, \
    effect_ref, progress_epoch, created_at, applied_at";

fn invalid(what: &str, v: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("project controller {what}: {v}").into(),
    )
}

fn state_row(r: &Row<'_>) -> rusqlite::Result<ProjectControllerState> {
    let mode: String = r.get(1)?;
    let phase: String = r.get(2)?;
    Ok(ProjectControllerState {
        project_id: r.get(0)?,
        mode: ControllerMode::parse(&mode).ok_or_else(|| invalid("mode", &mode))?,
        phase: ControllerPhase::parse(&phase).ok_or_else(|| invalid("phase", &phase))?,
        revision: r.get(3)?,
        event_cursor: r.get(4)?,
        next_wake_at: r.get(5)?,
        coalesce_until: r.get(6)?,
        lease_owner: r.get(7)?,
        lease_expires_at: r.get(8)?,
        retry_count: r.get::<_, i64>(9)? as u32,
        backoff_until: r.get(10)?,
        last_decision_id: r.get(11)?,
        human_epoch: r.get(12)?,
        progress_epoch: r.get(13)?,
        updated_at: r.get(14)?,
        updated_by: r.get(15)?,
    })
}

fn wake_row(r: &Row<'_>) -> rusqlite::Result<ProjectWake> {
    let kind: String = r.get(3)?;
    Ok(ProjectWake {
        id: r.get(0)?,
        project_id: r.get(1)?,
        wake_key: r.get(2)?,
        kind: ProjectWakeKind::parse(&kind).ok_or_else(|| invalid("wake kind", &kind))?,
        source_event_id: r.get(4)?,
        source_ref: r.get(5)?,
        created_at: r.get(6)?,
        consumed_by: r.get(7)?,
    })
}

fn decision_row(r: &Row<'_>) -> rusqlite::Result<ProjectControllerDecision> {
    let mode: String = r.get(7)?;
    let state: String = r.get(11)?;
    Ok(ProjectControllerDecision {
        id: r.get(0)?,
        project_id: r.get(1)?,
        based_on_revision: r.get(2)?,
        human_epoch: r.get(3)?,
        snapshot_sha256: r.get(4)?,
        request_sha256: r.get(5)?,
        generation_id: r.get(6)?,
        mode: ControllerMode::parse(&mode).ok_or_else(|| invalid("mode", &mode))?,
        proposal_json: r.get(8)?,
        validation: r.get(9)?,
        effect_json: r.get(10)?,
        effect_state: EffectState::parse(&state).ok_or_else(|| invalid("effect", &state))?,
        effect_ref: r.get(12)?,
        progress_epoch: r.get(13)?,
        created_at: r.get(14)?,
        applied_at: r.get(15)?,
    })
}

fn parse_at(s: &str) -> Result<OffsetDateTime, StoreError> {
    super::parse_rfc3339(s)
}

fn at_or_before(value: Option<&str>, now: OffsetDateTime) -> Result<bool, StoreError> {
    Ok(match value {
        None => false,
        Some(v) => parse_at(v)? <= now,
    })
}

/// 行が無い案件の既定（mode `off`。I13）。
pub fn default_state(project_id: &str, now: &str) -> ProjectControllerState {
    ProjectControllerState {
        project_id: project_id.into(),
        mode: ControllerMode::Off,
        phase: ControllerPhase::Active,
        revision: 0,
        event_cursor: 0,
        next_wake_at: None,
        coalesce_until: None,
        lease_owner: None,
        lease_expires_at: None,
        retry_count: 0,
        backoff_until: None,
        last_decision_id: None,
        human_epoch: 0,
        progress_epoch: 0,
        updated_at: now.into(),
        updated_by: "-".into(),
    }
}

/// [`SqliteStore::project_controller_set_mode`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeChange {
    Changed(Box<ProjectControllerState>),
    /// `expected_revision` が今の `revision` と違う（409）。
    Stale {
        current: i64,
    },
}

/// lease を取れたときの評価の材料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLease {
    pub state: ProjectControllerState,
    pub wakes: Vec<ProjectWake>,
}

/// tx1 に渡す判断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProjectDecision {
    pub id: String,
    pub based_on_revision: i64,
    pub human_epoch: i64,
    pub snapshot_sha256: String,
    pub request_sha256: String,
    pub generation_id: Option<String>,
    pub proposal_json: String,
    pub validation: String,
    pub effect_json: String,
    /// 効果があるか（`false` なら `effect_state = none`）。
    pub has_effect: bool,
    pub next_wake_at: Option<OffsetDateTime>,
    pub phase: ControllerPhase,
    /// 第 3 フェーズ P0-B（I1）: 判断の前に読んだ案件の root ごとの最後の event の seq
    /// （[`SqliteStore::project_root_event_mark`]）。phase `achieved` を書くときだけ commit の tx で
    /// 照合し、違えば `Stale(Evidence)`（decide の間の reopen で根拠の無い達成を書かない）。`None` は照合しない。
    pub evidence_mark: Option<RootEventMark>,
}

/// 案件の root（`parent_id` の無い task）ごとの最後の event の seq（task id 順）。
pub type RootEventMark = Vec<(String, i64)>;

/// tx1 の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    Committed {
        decision_id: String,
        effect_state: EffectState,
    },
    /// CAS で適用しなかった。wake は消費しない（次の tick で評価し直す。I7）。
    Stale(StaleReason),
}

/// 判断の失敗（decide の Err）の後の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecideFailure {
    pub retry_count: u32,
    pub backoff_secs: Option<u64>,
    pub blocked: bool,
}

/// root の冪等な作成の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootCreate {
    Created(String),
    /// 同じキーの root が既にある（I5。中身が違っても既存を返す）。
    Existing(String),
    /// 効果の門が閉じていた（人の操作の後の古い判断、`off` など）。何も書いていない。
    Refused,
    /// A7: 判断の後に許可が切れた（取り消し・期限・予算）。root は作らず、拒否の監査だけを書いた。
    AuthorityDenied(String),
}

impl SqliteStore {
    fn pc_state_tx(
        conn: &Connection,
        project_id: &str,
    ) -> Result<Option<ProjectControllerState>, StoreError> {
        Ok(conn
            .query_row(
                &format!("SELECT {STATE_COLS} FROM project_controller_state WHERE project_id = ?1"),
                [project_id],
                state_row,
            )
            .optional()?)
    }

    /// 状態の行（無ければ `None`。表示は [`default_state`]）。
    pub fn project_controller_state(
        &self,
        project_id: &str,
    ) -> Result<Option<ProjectControllerState>, StoreError> {
        self.with_read_conn(|c| Self::pc_state_tx(c, project_id))
    }

    /// `dry_run`・`shadow`・`enabled` の案件の状態（tick の評価の候補）。
    pub fn project_controller_active(&self) -> Result<Vec<ProjectControllerState>, StoreError> {
        self.with_read_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {STATE_COLS} FROM project_controller_state \
                 WHERE mode IN ('dry_run', 'shadow', 'enabled') ORDER BY project_id"
            ))?;
            let rows = stmt.query_map([], state_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// mode を変える（CAS: `expected_revision` が今の `revision` と同じときだけ。行が無ければ 0）。
    /// `revision + 1`。`off` 以外へ上げたら phase を `active` に戻し、backoff を消す（人の置き直し）。
    pub fn project_controller_set_mode(
        &self,
        project_id: &str,
        mode: ControllerMode,
        expected_revision: Option<i64>,
        by: &str,
        now: OffsetDateTime,
    ) -> Result<ModeChange, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = Self::project_controller_set_mode_tx(
            &tx,
            project_id,
            mode,
            expected_revision,
            by,
            now,
        )?;
        tx.commit()?;
        Ok(out)
    }

    /// [`Self::project_controller_set_mode`] を呼び出し側の transaction で（CoS の operation の監査と同じ tx）。
    pub fn project_controller_set_mode_tx(
        tx: &Connection,
        project_id: &str,
        mode: ControllerMode,
        expected_revision: Option<i64>,
        by: &str,
        now: OffsetDateTime,
    ) -> Result<ModeChange, StoreError> {
        let at = format_rfc3339(now)?;
        let current = Self::pc_state_tx(tx, project_id)?;
        let revision = current.as_ref().map_or(0, |s| s.revision);
        if let Some(expected) = expected_revision
            && expected != revision
        {
            return Ok(ModeChange::Stale { current: revision });
        }
        tx.execute(
            "INSERT INTO project_controller_state (project_id, mode, phase, revision, updated_at, updated_by) \
             VALUES (?1, ?2, 'active', 1, ?3, ?4) \
             ON CONFLICT(project_id) DO UPDATE SET mode = excluded.mode, revision = revision + 1, \
               phase = CASE WHEN excluded.mode = 'off' THEN phase ELSE 'active' END, \
               retry_count = CASE WHEN excluded.mode = 'off' THEN retry_count ELSE 0 END, \
               backoff_until = CASE WHEN excluded.mode = 'off' THEN backoff_until ELSE NULL END, \
               updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            params![project_id, mode.as_str(), at, by],
        )?;
        let state = Self::pc_state_tx(tx, project_id)?
            .ok_or_else(|| StoreError::Invalid("controller state vanished".into()))?;
        // `dry_run` に置いたら 1 回だけ評価を頼む（付記 A3 の 7）。coalesce を待たず次の tick で評価する。
        if mode == ControllerMode::DryRun {
            Self::pc_wake_insert_tx(
                tx,
                &NewProjectWake {
                    project_id: project_id.into(),
                    wake_key: format!("{DRY_RUN_WAKE_PREFIX}{}", state.revision),
                    kind: ProjectWakeKind::HumanChanged,
                    source_event_id: None,
                    source_ref: None,
                },
                &at,
                &at,
            )?;
        }
        Ok(ModeChange::Changed(Box::new(state)))
    }

    /// wake 1 件を呼び出し側の transaction で入れる（API の wake・CoS の operation）。入ったら `true`。
    pub fn project_wake_insert_tx(
        tx: &Connection,
        wake: &NewProjectWake,
        coalesce_secs: u64,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let at = format_rfc3339(now)?;
        let until = format_rfc3339(now + time::Duration::seconds(coalesce_secs as i64))?;
        Ok(Self::pc_wake_insert_tx(tx, wake, &at, &until)? > 0)
    }

    /// 状態の行を読む（呼び出し側の transaction）。
    pub fn project_controller_state_tx(
        tx: &Connection,
        project_id: &str,
    ) -> Result<Option<ProjectControllerState>, StoreError> {
        Self::pc_state_tx(tx, project_id)
    }

    /// 取り込み: wake を入れ（`shadow`・`enabled` の案件だけ）、`cursor` があれば同じ transaction で進める。
    /// 新しく入った wake の数を返す。入った案件は `coalesce_until` を `now + coalesce_secs` に置く
    /// （既に置いてあれば変えない）。
    pub fn project_wake_ingest(
        &self,
        wakes: &[NewProjectWake],
        cursor: Option<(&str, u64)>,
        coalesce_secs: u64,
        now: OffsetDateTime,
    ) -> Result<usize, StoreError> {
        let at = format_rfc3339(now)?;
        let until = format_rfc3339(now + time::Duration::seconds(coalesce_secs as i64))?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut inserted = 0;
        for w in wakes {
            inserted += Self::pc_wake_insert_tx(&tx, w, &at, &until)?;
        }
        if let Some((name, value)) = cursor {
            tx.execute(
                "INSERT INTO feed_cursor (name, value) VALUES (?1, ?2) \
                 ON CONFLICT(name) DO UPDATE SET value = excluded.value",
                params![name, value.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(inserted)
    }

    fn pc_wake_insert_tx(
        tx: &Connection,
        w: &NewProjectWake,
        at: &str,
        until: &str,
    ) -> Result<usize, StoreError> {
        let row: Option<(String, i64)> = tx
            .query_row(
                "SELECT mode, event_cursor FROM project_controller_state WHERE project_id = ?1",
                [&w.project_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((mode, event_cursor)) = row else {
            return Ok(0);
        };
        if !ControllerMode::parse(&mode)
            .is_some_and(|m| m.accepts_wake(w.kind, &w.wake_key, w.source_event_id.is_some()))
        {
            return Ok(0);
        }
        // 案件別 cursor（付記 A1 の 12）: 判断が既に畳んだ event 以下の再配送は wake にしない。
        // 取り込みは event id の昇順・1 tx なので、cursor 以下の event の wake は既に入れたか、
        // 入れなかった（その時 off だった）もの。source_event_id の無い wake（人・cron・停滞）は対象外。
        if w.source_event_id.is_some_and(|id| id <= event_cursor) {
            return Ok(0);
        }
        let n = tx.execute(
            "INSERT OR IGNORE INTO project_wakes \
             (project_id, wake_key, kind, source_event_id, source_ref, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                w.project_id,
                w.wake_key,
                w.kind.as_str(),
                w.source_event_id,
                w.source_ref,
                at
            ],
        )?;
        if n > 0 {
            tx.execute(
                "UPDATE project_controller_state SET coalesce_until = COALESCE(coalesce_until, ?2) \
                 WHERE project_id = ?1",
                params![w.project_id, until],
            )?;
        }
        Ok(n)
    }

    /// brief の内容変更は人の回答ではない。入力 epoch と wake を保存側の tx で更新し、
    /// 古い snapshot と未実行の効果を拒否する。停止 phase は解除しない。
    pub(crate) fn project_controller_brief_changed_tx(
        tx: &Connection,
        project_id: &str,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        let Some(state) = Self::pc_state_tx(tx, project_id)? else {
            return Ok(());
        };
        if !state.mode.wakes() {
            return Ok(());
        }
        let epoch = state.human_epoch + 1;
        tx.execute(
            "UPDATE project_controller_state SET human_epoch = ?2 WHERE project_id = ?1",
            params![project_id, epoch],
        )?;
        Self::project_wake_insert_tx(
            tx,
            &NewProjectWake {
                project_id: project_id.into(),
                wake_key: format!("{}{epoch}", crate::project_wake::BRIEF_WAKE_PREFIX),
                kind: ProjectWakeKind::ExternalReleased,
                source_event_id: None,
                source_ref: Some(format!("project_brief:{project_id}:{epoch}")),
            },
            0,
            now,
        )?;
        Ok(())
    }

    /// 人の操作（D12 `human_epoch`）。呼び出し側の transaction の中で `human_epoch + 1` と wake
    /// `human:<epoch>` を書く。状態の行が無い・`off` の案件では何もしない（`None`。I13）。
    pub fn project_controller_human_bump_tx(
        tx: &Connection,
        project_id: &str,
        now: OffsetDateTime,
        coalesce_secs: u64,
    ) -> Result<Option<i64>, StoreError> {
        let Some(state) = Self::pc_state_tx(tx, project_id)? else {
            return Ok(None);
        };
        if state.mode == ControllerMode::Off {
            return Ok(None);
        }
        let epoch = state.human_epoch + 1;
        tx.execute(
            "UPDATE project_controller_state SET human_epoch = ?2 WHERE project_id = ?1",
            params![project_id, epoch],
        )?;
        let at = format_rfc3339(now)?;
        let until = format_rfc3339(now + time::Duration::seconds(coalesce_secs as i64))?;
        Self::pc_wake_insert_tx(
            tx,
            &NewProjectWake {
                project_id: project_id.into(),
                wake_key: human_wake_key(epoch),
                kind: ProjectWakeKind::HumanChanged,
                source_event_id: None,
                source_ref: None,
            },
            &at,
            &until,
        )?;
        Ok(Some(epoch))
    }

    /// `human_epoch` を数える案件（呼び出し側の transaction の中で task の案件を読む）。
    pub fn task_project_id_tx(
        tx: &Connection,
        task_id: crate::model::TaskId,
    ) -> Result<Option<crate::ProjectId>, StoreError> {
        Ok(Self::get_locked(tx, task_id)?.and_then(|task| task.project_id))
    }

    /// 未消費の wake（古い順）。
    pub fn project_wakes_pending(&self, project_id: &str) -> Result<Vec<ProjectWake>, StoreError> {
        self.with_read_conn(|c| Self::pc_wakes_pending_tx(c, project_id, usize::MAX))
    }

    fn pc_wakes_pending_tx(
        c: &Connection,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<ProjectWake>, StoreError> {
        let mut stmt = c.prepare(&format!(
            "SELECT {WAKE_COLS} FROM project_wakes WHERE project_id = ?1 AND consumed_by IS NULL \
             ORDER BY id LIMIT ?2"
        ))?;
        let rows = stmt.query_map(
            params![project_id, i64::try_from(limit).unwrap_or(i64::MAX)],
            wake_row,
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 全 wake（試験・診断）。
    pub fn project_wakes_all(&self, project_id: &str) -> Result<Vec<ProjectWake>, StoreError> {
        self.with_read_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {WAKE_COLS} FROM project_wakes WHERE project_id = ?1 ORDER BY id"
            ))?;
            let rows = stmt.query_map([project_id], wake_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// 評価の期限が来た案件を古い wake の順に最大 `limit` 件（backpressure、D12）。
    /// 条件: mode が wake を積む・phase が止まっていない・backoff が過ぎた・lease が空いている（切れている）・
    /// (未消費の wake があり `coalesce_until` が過ぎた) か `next_wake_at` が過ぎた。
    pub fn project_controller_due(
        &self,
        now: OffsetDateTime,
        limit: usize,
    ) -> Result<Vec<String>, StoreError> {
        let states = self.project_controller_active()?;
        let mut due: Vec<(i64, String)> = Vec::new();
        for s in states {
            if s.phase.halted() {
                continue;
            }
            if s.backoff_until.as_deref().is_some()
                && !at_or_before(s.backoff_until.as_deref(), now)?
            {
                continue;
            }
            if s.lease_owner.is_some() && !at_or_before(s.lease_expires_at.as_deref(), now)? {
                continue;
            }
            let oldest: Option<i64> = self.with_read_conn(|c| {
                Ok(c.query_row(
                    "SELECT MIN(id) FROM project_wakes WHERE project_id = ?1 AND consumed_by IS NULL",
                    [&s.project_id],
                    |r| r.get(0),
                )?)
            })?;
            let wake_due = oldest.is_some()
                && (s.coalesce_until.is_none() || at_or_before(s.coalesce_until.as_deref(), now)?);
            let timer_due = at_or_before(s.next_wake_at.as_deref(), now)?;
            if wake_due || timer_due {
                due.push((oldest.unwrap_or(i64::MAX), s.project_id));
            }
        }
        due.sort();
        Ok(due.into_iter().take(limit).map(|(_, p)| p).collect())
    }

    /// lease を取る（空いている・切れている・自分のときだけ）。取れたら状態と未消費の wake を返す。
    pub fn project_controller_lease(
        &self,
        project_id: &str,
        owner: &str,
        lease_secs: u64,
        now: OffsetDateTime,
    ) -> Result<Option<ProjectLease>, StoreError> {
        let expires = format_rfc3339(now + time::Duration::seconds(lease_secs as i64))?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(state) = Self::pc_state_tx(&tx, project_id)? else {
            return Ok(None);
        };
        if !state.mode.evaluates() || state.phase.halted() {
            return Ok(None);
        }
        let held_by_other = state.lease_owner.as_deref().is_some_and(|o| o != owner)
            && !at_or_before(state.lease_expires_at.as_deref(), now)?;
        if held_by_other {
            return Ok(None);
        }
        tx.execute(
            "UPDATE project_controller_state SET lease_owner = ?2, lease_expires_at = ?3 \
             WHERE project_id = ?1",
            params![project_id, owner, expires],
        )?;
        let wakes = Self::pc_wakes_pending_tx(&tx, project_id, WAKES_PER_EVALUATION)?;
        let state = Self::pc_state_tx(&tx, project_id)?
            .ok_or_else(|| StoreError::Invalid("controller state vanished".into()))?;
        tx.commit()?;
        Ok(Some(ProjectLease { state, wakes }))
    }

    /// decide が失敗した: `retry_count + 1`。`max_retries` を超えたら `blocked`（以後 decide しない）、
    /// それまでは `retry_base_secs × 2^(n-1)` の backoff。wake は消費しない。lease を放す。
    /// `blocked` にするときは `exhausted_notice(&state, n)` の notice を同じ transaction で記録する
    /// （付記 A1 の 12。notice と blocked は一緒に残るか一緒に残らない。source_key が同じなら 1 件）。
    pub fn project_controller_decide_failed(
        &self,
        project_id: &str,
        owner: &str,
        (base, max, max_retries): (u64, u64, u32),
        now: OffsetDateTime,
        exhausted_notice: &dyn Fn(&ProjectControllerState, u32) -> crate::feed::NoticeEvent,
    ) -> Result<DecideFailure, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = Self::pc_state_tx(&tx, project_id)?
            .ok_or_else(|| StoreError::Invalid("controller state missing".into()))?;
        if state.lease_owner.as_deref() != Some(owner) {
            return Ok(DecideFailure {
                retry_count: state.retry_count,
                backoff_secs: None,
                blocked: state.phase == ControllerPhase::Blocked,
            });
        }
        let n = state.retry_count + 1;
        let blocked = n > max_retries;
        let wait = (!blocked).then(|| backoff_secs(n, base, max));
        let until = match wait {
            Some(secs) => Some(format_rfc3339(now + time::Duration::seconds(secs as i64))?),
            None => None,
        };
        if blocked {
            crate::feed::record_in_tx(&tx, &exhausted_notice(&state, n))?;
        }
        tx.execute(
            "UPDATE project_controller_state SET retry_count = ?2, backoff_until = ?3, \
             phase = CASE WHEN ?4 THEN 'blocked' ELSE phase END, \
             lease_owner = NULL, lease_expires_at = NULL WHERE project_id = ?1",
            params![project_id, n, until, blocked],
        )?;
        tx.commit()?;
        Ok(DecideFailure {
            retry_count: n,
            backoff_secs: wait,
            blocked,
        })
    }

    /// lease を放す（評価しなかったとき）。
    pub fn project_controller_release(
        &self,
        project_id: &str,
        owner: &str,
    ) -> Result<(), StoreError> {
        self.lock()?.execute(
            "UPDATE project_controller_state SET lease_owner = NULL, lease_expires_at = NULL \
             WHERE project_id = ?1 AND lease_owner = ?2",
            params![project_id, owner],
        )?;
        Ok(())
    }

    /// tx1: CAS（`revision`・`human_epoch`・lease・mode）を照合し、一致すれば判断の行・wake の消費・
    /// cursor・`revision + 1`・`next_wake_at`・phase を 1 回で書く。`shadow` の判断の効果は `suppressed`。
    /// 一致しなければ何も書かず（lease だけ放す）`Stale` を返す。
    pub fn project_controller_commit(
        &self,
        project_id: &str,
        owner: &str,
        wake_ids: &[i64],
        d: &NewProjectDecision,
        now: OffsetDateTime,
    ) -> Result<CommitOutcome, StoreError> {
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = Self::pc_state_tx(&tx, project_id)?
            .ok_or_else(|| StoreError::Invalid("controller state missing".into()))?;
        let stale = if state.lease_owner.as_deref() != Some(owner) {
            Some(StaleReason::Lease)
        } else if !state.mode.evaluates() {
            Some(StaleReason::Mode)
        } else if state.human_epoch != d.human_epoch {
            Some(StaleReason::HumanEpoch)
        } else if state.revision != d.based_on_revision {
            Some(StaleReason::Revision)
        } else if state.mode != ControllerMode::DryRun
            && d.phase == ControllerPhase::Achieved
            && match &d.evidence_mark {
                Some(mark) => Self::root_event_mark_tx(&tx, project_id)? != *mark,
                None => false,
            }
        {
            Some(StaleReason::Evidence)
        } else {
            None
        };
        if let Some(reason) = stale {
            tx.execute(
                "UPDATE project_controller_state SET lease_owner = NULL, lease_expires_at = NULL \
                 WHERE project_id = ?1 AND lease_owner = ?2",
                params![project_id, owner],
            )?;
            tx.commit()?;
            return Ok(CommitOutcome::Stale(reason));
        }
        let effect_state = match (d.has_effect, state.mode) {
            (false, _) => EffectState::None,
            (true, ControllerMode::Enabled) => EffectState::Pending,
            (true, _) => EffectState::Suppressed,
        };
        tx.execute(
            &format!(
                "INSERT INTO project_controller_decisions ({DECISION_COLS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, ?13, ?14, NULL)"
            ),
            params![
                d.id,
                project_id,
                d.based_on_revision,
                d.human_epoch,
                d.snapshot_sha256,
                d.request_sha256,
                d.generation_id,
                state.mode.as_str(),
                d.proposal_json,
                d.validation,
                d.effect_json,
                effect_state.as_str(),
                state.progress_epoch,
                at
            ],
        )?;
        let mut cursor = state.event_cursor;
        for id in wake_ids {
            tx.execute(
                "UPDATE project_wakes SET consumed_by = ?3 \
                 WHERE id = ?1 AND project_id = ?2 AND consumed_by IS NULL",
                params![id, project_id, d.id],
            )?;
            let src: Option<i64> = tx
                .query_row(
                    "SELECT source_event_id FROM project_wakes WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            cursor = cursor.max(src.unwrap_or(0));
        }
        // `dry_run` は頼まれたときだけ評価する: timer を置かず、phase も変えない（判断の記録だけ。付記 A3 の 7）。
        let dry_run = state.mode == ControllerMode::DryRun;
        let next = match d.next_wake_at {
            Some(t) if !dry_run => Some(format_rfc3339(t)?),
            _ => None,
        };
        let phase = if dry_run { state.phase } else { d.phase };
        // 畳み切れなかった wake が残れば coalesce を待たずに次の tick で続ける。
        let remaining: i64 = tx.query_row(
            "SELECT COUNT(*) FROM project_wakes WHERE project_id = ?1 AND consumed_by IS NULL",
            [project_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE project_controller_state SET revision = revision + 1, event_cursor = ?2, \
             next_wake_at = ?3, coalesce_until = CASE WHEN ?4 > 0 THEN ?5 ELSE NULL END, \
             lease_owner = NULL, lease_expires_at = NULL, retry_count = 0, backoff_until = NULL, \
             last_decision_id = ?6, phase = ?7, updated_at = ?5, updated_by = 'daemon' \
             WHERE project_id = ?1",
            params![
                project_id,
                cursor,
                next,
                remaining,
                at,
                d.id,
                phase.as_str()
            ],
        )?;
        tx.commit()?;
        Ok(CommitOutcome::Committed {
            decision_id: d.id.clone(),
            effect_state,
        })
    }

    /// 案件の root（`parent_id` の無い task。会話などの task も含む上位集合）ごとの最後の event の seq。
    /// 判断の前に読み、phase `achieved` の commit の tx で同じものを読み直して照合する（I1）。
    pub fn project_root_event_mark(&self, project_id: &str) -> Result<RootEventMark, StoreError> {
        self.with_read_conn(|c| Self::root_event_mark_tx(c, project_id))
    }

    pub(super) fn root_event_mark_tx(
        c: &Connection,
        project_id: &str,
    ) -> Result<RootEventMark, StoreError> {
        let mut stmt = c.prepare(
            "SELECT t.id, COALESCE((SELECT MAX(e.seq) FROM events e WHERE e.task_id = t.id), 0) \
             FROM tasks t WHERE t.project_id = ?1 AND t.parent_id IS NULL ORDER BY t.id",
        )?;
        let rows = stmt.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 効果が `pending` のまま残っている判断（古い順。crash の B4・B5 の回復）。
    pub fn project_controller_pending_effects(
        &self,
    ) -> Result<Vec<ProjectControllerDecision>, StoreError> {
        self.with_read_conn(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {DECISION_COLS} FROM project_controller_decisions \
                 WHERE effect_state = 'pending' ORDER BY created_at, id"
            ))?;
            let rows = stmt.query_map([], decision_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// 効果を `pending` から `applied`・`failed` へ進める（1 方向。変わったら `true`）。
    pub fn project_controller_effect_done(
        &self,
        decision_id: &str,
        state: EffectState,
        effect_ref: Option<&str>,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        if !matches!(state, EffectState::Applied | EffectState::Failed) {
            return Err(StoreError::Invalid(format!(
                "effect can only advance to applied or failed, not {}",
                state.as_str()
            )));
        }
        let at = format_rfc3339(now)?;
        let n = self.lock()?.execute(
            "UPDATE project_controller_decisions SET effect_state = ?2, effect_ref = ?3, applied_at = ?4 \
             WHERE id = ?1 AND effect_state = 'pending'",
            params![decision_id, state.as_str(), effect_ref, at],
        )?;
        Ok(n > 0)
    }

    /// 判断の履歴（新しい順。`before` は判断 id より古いもの）。
    pub fn project_controller_decisions(
        &self,
        project_id: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ProjectControllerDecision>, StoreError> {
        self.with_read_conn(|c| {
            let lim = i64::try_from(limit).unwrap_or(i64::MAX);
            let mut stmt = c.prepare(&format!(
                "SELECT {DECISION_COLS} FROM project_controller_decisions WHERE project_id = ?1 \
                 AND (?2 IS NULL OR (created_at, id) < \
                   (SELECT created_at, id FROM project_controller_decisions WHERE id = ?2)) \
                 ORDER BY created_at DESC, id DESC LIMIT ?3"
            ))?;
            let rows = stmt.query_map(params![project_id, before, lim], decision_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// 1 件の判断。
    pub fn project_controller_decision(
        &self,
        decision_id: &str,
    ) -> Result<Option<ProjectControllerDecision>, StoreError> {
        self.with_read_conn(|c| {
            Ok(c.query_row(
                &format!("SELECT {DECISION_COLS} FROM project_controller_decisions WHERE id = ?1"),
                [decision_id],
                decision_row,
            )
            .optional()?)
        })
    }

    /// root を冪等に作る（I5）: 同じ transaction でキーを照合し、無ければ task と
    /// `task_idempotency_keys` の行を書く。キーが既にあれば（中身が違っても）既存の task を返す。
    /// キーが無いときは同じ transaction で効果の門を読み、閉じていれば作らない（I7、付記 A1 の 11）。
    #[allow(clippy::too_many_arguments)]
    pub fn project_controller_create_root(
        &self,
        task: &Task,
        claim: &IdempotencyClaim,
        created_by: &str,
        result_json: Option<&str>,
        decision_human_epoch: i64,
        authority: Option<&crate::authority::GrantScope>,
        now: OffsetDateTime,
    ) -> Result<RootCreate, StoreError> {
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match Self::idempotency_lookup_tx(&tx, claim)? {
            IdempotencyLookup::Same { task_id, .. } | IdempotencyLookup::Conflict { task_id } => {
                return Ok(RootCreate::Existing(task_id));
            }
            IdempotencyLookup::New => {}
        }
        let project_id = task
            .project_id
            .ok_or_else(|| StoreError::Invalid("controller root without a project".into()))?;
        if !effect_gate_open_tx(&tx, &project_id.to_string(), decision_human_epoch)? {
            return Ok(RootCreate::Refused);
        }
        if let Some(scope) = authority {
            let row = crate::authority::NewAuthorityAudit {
                project_id: project_id.to_string(),
                action_class: crate::authority::ActionClass::LaunchRoot
                    .as_str()
                    .to_owned(),
                scope: scope.clone(),
                verdict: crate::authority::AuthorityVerdict::denied("pending"),
                suggestion: serde_json::json!({
                    "by": "project_controller",
                    "action": "launch_root",
                    "idempotency_key": claim.key,
                }),
                actual_actor: created_by.to_owned(),
                human_decision_ref: None,
                decision_id: None,
                dedupe_key: format!("pc-use:{}", claim.key),
                outcome_state: "applied",
                observed_outcome: Some(serde_json::json!({"task_id": task.id.to_string()})),
            };
            let (verdict, _) = Self::authority_use_tx(&tx, &row, now)?;
            if !verdict.allow {
                let why = verdict.reason;
                tx.commit()?;
                return Ok(RootCreate::AuthorityDenied(why));
            }
        }
        Self::create_task_tx(&tx, task, None, created_by, vec![])?;
        Self::idempotency_record_tx(&tx, claim, &task.id.to_string(), result_json, &at)?;
        tx.commit()?;
        Ok(RootCreate::Created(task.id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> OffsetDateTime {
        OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).unwrap()
    }

    fn wake(key: &str) -> NewProjectWake {
        NewProjectWake {
            project_id: "p1".into(),
            wake_key: key.into(),
            kind: ProjectWakeKind::RootDone,
            source_event_id: Some(7),
            source_ref: None,
        }
    }

    #[test]
    fn project_wake_store_off_project_takes_no_wake() {
        let s = SqliteStore::open_in_memory().unwrap();
        let now = t("2026-11-04T00:00:00Z");
        assert_eq!(
            s.project_wake_ingest(&[wake("a")], None, 30, now).unwrap(),
            0
        );
        s.project_controller_set_mode("p1", ControllerMode::Shadow, Some(0), "human:t", now)
            .unwrap();
        assert_eq!(
            s.project_wake_ingest(&[wake("a")], None, 30, now).unwrap(),
            1
        );
        assert_eq!(
            s.project_wake_ingest(&[wake("a")], None, 30, now).unwrap(),
            0
        );
        assert_eq!(s.project_wakes_pending("p1").unwrap().len(), 1);
    }

    #[test]
    fn project_wake_store_decisions_are_append_only() {
        let s = SqliteStore::open_in_memory().unwrap();
        let now = t("2026-11-04T00:00:00Z");
        s.project_controller_set_mode("p1", ControllerMode::Enabled, None, "human:t", now)
            .unwrap();
        s.project_wake_ingest(&[wake("a")], None, 0, now).unwrap();
        let lease = s
            .project_controller_lease("p1", "i1", 60, now)
            .unwrap()
            .unwrap();
        let d = NewProjectDecision {
            id: "d1".into(),
            based_on_revision: lease.state.revision,
            human_epoch: lease.state.human_epoch,
            snapshot_sha256: "s".into(),
            request_sha256: "r".into(),
            generation_id: None,
            proposal_json: "{}".into(),
            validation: "ok".into(),
            effect_json: "{}".into(),
            has_effect: true,
            next_wake_at: None,
            phase: ControllerPhase::Active,
            evidence_mark: None,
        };
        let ids: Vec<i64> = lease.wakes.iter().map(|w| w.id).collect();
        assert!(matches!(
            s.project_controller_commit("p1", "i1", &ids, &d, now)
                .unwrap(),
            CommitOutcome::Committed {
                effect_state: EffectState::Pending,
                ..
            }
        ));
        let conn = s.lock().unwrap();
        assert!(
            conn.execute("DELETE FROM project_controller_decisions", [])
                .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE project_controller_decisions SET validation = 'x'",
                []
            )
            .is_err()
        );
        // 再試行の欄は pending のまま前進だけ（回数は減らない）。
        conn.execute(
            "UPDATE project_controller_decisions SET effect_attempts = 2, effect_failures = 1",
            [],
        )
        .unwrap();
        assert!(
            conn.execute(
                "UPDATE project_controller_decisions SET effect_failures = 0",
                []
            )
            .is_err()
        );
        drop(conn);
        assert!(
            s.project_controller_effect_done("d1", EffectState::Applied, Some("t"), now)
                .unwrap()
        );
        assert!(
            !s.project_controller_effect_done("d1", EffectState::Applied, Some("t"), now)
                .unwrap()
        );
    }
}
