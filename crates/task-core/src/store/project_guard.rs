//! ADR 2026-11-04-project-controller A6: 長時間の運用の store の口（phase の同期・最後の入力の時刻・token の和）。
//! 判定（どの guard が効くか）は `task_dispatch::dispatcher::project_guard`、予算の数え方は
//! `task_ops::project_budget` が持つ。ここは読み書きだけ。

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::project_wake::ControllerPhase;

/// [`SqliteStore::project_controller_sync_phases`] が変えた 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseSync {
    pub project_id: String,
    pub from: ControllerPhase,
    pub to: ControllerPhase,
    /// `paused`・`cancelled` にしたときに消費した（評価せずに畳んだ）wake の数。
    pub held_wakes: usize,
}

/// [`SqliteStore::project_run_usage_between`] の 1 行（予算に数える終わった run）。
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRunUsage {
    pub model: Option<String>,
    /// 予算に使う usage。`None` は費用不明（usage が無い・読めない、または chat run の attempt に usage 欠測がある）。
    pub usage: Option<crate::model::Usage>,
    /// 読めた usage（attempt 欠測で `usage` が `None` でも、既知の分は消さずに残す。予算には使わない）。
    pub known_usage: Option<crate::model::Usage>,
}

/// 起動前に終わった chat run の判定（reason 無しの NULL で `NOT` が NULL にならないよう COALESCE する。`cost_state = not_launched`、`chat::run_store::cost_state_of` と同じ条件）を
/// 表す SQL の述語。`r` は `chat_runs`。和 [`SqliteStore::project_run_usage_between`] の除外と
/// 件数 [`SqliteStore::project_not_launched_runs_between`] がこの 1 つを共有する。
fn not_launched_sql() -> String {
    use crate::chat::ChatPrelaunchReason as R;
    // match が網羅なので、列挙を足すとここがコンパイルエラーになる（SQL の列挙の更新漏れを防ぐ）。
    let name = |r: R| match r {
        R::RouteUnavailable => "route_unavailable",
        R::SkillsUnavailable => "skills_unavailable",
        R::WallExpiredBeforeLaunch => "wall_expired_before_launch",
        R::SlotWaitExpired => "slot_wait_expired",
        R::DaemonRestartBeforeLaunch => "daemon_restart_before_launch",
        R::StoppedBeforeLaunch => "stopped_before_launch",
    };
    let list = [
        R::RouteUnavailable,
        R::SkillsUnavailable,
        R::WallExpiredBeforeLaunch,
        R::SlotWaitExpired,
        R::DaemonRestartBeforeLaunch,
        R::StoppedBeforeLaunch,
    ]
    .map(|r| format!("'{}'", name(r)))
    .join(",");
    format!(
        "(r.finished_at IS NOT NULL AND r.launch_tracked = 1 AND r.launched_at IS NULL \
         AND COALESCE(r.prelaunch_reason, '') IN ({list}))"
    )
}

/// [`SqliteStore::project_controller_resume`] の入力。読んだときの状態と、判断の行に残す根拠。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectResume {
    pub project_id: String,
    pub revision: i64,
    pub human_epoch: i64,
    pub blocked_decision_id: String,
    pub decision_id: String,
    pub request_sha256: String,
    pub rule_id: String,
    pub record_json: String,
    pub effect_json: String,
}

/// `Project.status`（と `archived_at`）から決まる phase。`None` は「controller の判断に任せる」。
fn phase_of_project(status: &str, archived: bool) -> Option<ControllerPhase> {
    if archived {
        return Some(ControllerPhase::Cancelled);
    }
    match status {
        "paused" => Some(ControllerPhase::Paused),
        "cancelled" | "done" => Some(ControllerPhase::Cancelled),
        _ => None,
    }
}

impl SqliteStore {
    /// A6: controller の phase を `Project.status` に合わせる（D12「`paused`・`cancelled` は `Project.status`
    /// に従う」）。Task の running とは独立に、案件の状態だけを読む。
    /// - 案件が paused → `paused`、cancelled・done・archived → `cancelled`。未消費の wake は `hold:<phase>` で
    ///   消費し、timer（`next_wake_at`・`coalesce_until`）を消す（評価 0。止まっている間に積もった wake で
    ///   再開の後に何度も評価しない）。
    /// - 案件が active（proposed）に戻ったのに phase が `paused`・`cancelled` → `active`、`next_wake_at = now`
    ///   （再開の後に 1 回だけ評価する）。
    /// - mode `off` の案件は触らない（I13）。1 回の transaction で全案件を見る。
    pub fn project_controller_sync_phases(
        &self,
        now: OffsetDateTime,
    ) -> Result<Vec<PhaseSync>, StoreError> {
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rows: Vec<(String, String, String, bool)> = {
            let mut st = tx.prepare(
                "SELECT s.project_id, s.phase, p.status, p.archived_at IS NOT NULL \
                 FROM project_controller_state s JOIN projects p ON p.id = s.project_id \
                 WHERE s.mode != 'off'",
            )?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<Result<_, _>>()?
        };
        let mut out = Vec::new();
        for (project_id, phase, status, archived) in rows {
            let Some(from) = ControllerPhase::parse(&phase) else {
                continue;
            };
            let wanted = phase_of_project(&status, archived);
            let to = match wanted {
                Some(p) => p,
                None if matches!(from, ControllerPhase::Paused | ControllerPhase::Cancelled) => {
                    ControllerPhase::Active
                }
                None => from,
            };
            let holds = matches!(to, ControllerPhase::Paused | ControllerPhase::Cancelled);
            // 止まっている間に来た wake も評価せずに畳む（毎 tick。再開の後に積もった分で何度も評価しない）。
            let held_wakes = if holds {
                tx.execute(
                    "UPDATE project_wakes SET consumed_by = ?2 \
                     WHERE project_id = ?1 AND consumed_by IS NULL",
                    params![project_id, format!("hold:{}", to.as_str())],
                )?
            } else {
                0
            };
            if to == from {
                continue;
            }
            if holds {
                tx.execute(
                    "UPDATE project_controller_state SET phase = ?2, next_wake_at = NULL, \
                     coalesce_until = NULL, updated_at = ?3, updated_by = 'daemon' \
                     WHERE project_id = ?1",
                    params![project_id, to.as_str(), at],
                )?;
            } else {
                tx.execute(
                    "UPDATE project_controller_state SET phase = ?2, next_wake_at = ?3, \
                     updated_at = ?3, updated_by = 'daemon' WHERE project_id = ?1",
                    params![project_id, to.as_str(), at],
                )?;
            }
            out.push(PhaseSync {
                project_id,
                from,
                to,
                held_wakes,
            });
        }
        tx.commit()?;
        Ok(out)
    }

    /// A6 の TTL: 案件に最後に「外からの入力」（[`crate::project_wake::is_external_input`] の wake。消費済みを
    /// 含む）が来た時刻。CoS 自身の brief の更新と daemon の内部の wake は数えない（人を待つ TTL を延ばさない）。
    /// 無ければ最初の判断の時刻。どちらも無ければ `None`。
    pub fn project_controller_last_input_at(
        &self,
        project_id: &str,
    ) -> Result<Option<String>, StoreError> {
        self.with_read_conn(|c| {
            let wake: Option<String> = c
                .query_row(
                    "SELECT MAX(created_at) FROM project_wakes \
                     WHERE project_id = ?1 AND kind NOT IN ('cron', 'stagnation') \
                       AND wake_key NOT LIKE 'brief:%' AND wake_key NOT LIKE 'authority-denied:%'",
                    [project_id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            if wake.is_some() {
                return Ok(wake);
            }
            Ok(c.query_row(
                "SELECT MIN(created_at) FROM project_controller_decisions WHERE project_id = ?1",
                [project_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
        })
    }

    /// 案件の task run と CoS chat run（`since <= started_at < until`）のうち、予算に数える終わった run の
    /// model と usage（ADR 2026-11-06-budget-usd-caps D2-3）。走っている run（`finished_at` 無し・`running`）と、
    /// harness を起動する前に終わった task run（usage・model・adapter・account・session・metrics が全部無い）と、
    /// 起動前に終わった chat run（`finished_at` あり・`launched_at` NULL・境界を記録する世代・列挙内の
    /// `prelaunch_reason` の全部。ADR 2026-11-06-cos-run-launch-accounting-and-drain D1-2）は返さない。
    /// 旧記録・未知の reason・reason 無し・起動後の usage 欠測・attempt の欠測は返し、`usage: None`（費用不明）にする。usage が無い・読めない行は `usage: None` で返す（費用不明として数える）。model は task run が
    /// `runs.model`、CoS chat run が `resolved_config_json` の `model`。
    pub fn project_run_usage_between(
        &self,
        project_id: &str,
        since: &str,
        until: &str,
    ) -> Result<Vec<ProjectRunUsage>, StoreError> {
        let not_launched = not_launched_sql();
        self.with_read_conn(|c| {
            let sql = format!(
                "SELECT r.model, r.usage_json, 0 FROM runs r JOIN tasks t ON t.id = r.task_id \
                 WHERE t.project_id = ?1 AND r.started_at >= ?2 AND r.started_at < ?3 \
                   AND r.finished_at IS NOT NULL AND r.status <> 'running' \
                   AND (r.usage_json IS NOT NULL OR r.model IS NOT NULL OR r.adapter IS NOT NULL \
                        OR r.account IS NOT NULL OR r.session_id IS NOT NULL OR r.metrics_json IS NOT NULL) \
                 UNION ALL SELECT \
                   json_extract(CASE WHEN json_valid(r.resolved_config_json) THEN r.resolved_config_json END, '$.model'), \
                   r.usage_json, r.attempts_usage_missing FROM chat_runs r \
                 JOIN chat_threads t ON t.id = r.thread_id \
                 WHERE t.project_id = ?1 AND r.started_at >= ?2 AND r.started_at < ?3 \
                   AND r.finished_at IS NOT NULL AND NOT {not_launched}"
            );
            let mut st = c.prepare(&sql)?;
            let raw: Vec<(Option<String>, Option<String>, i64)> = st
                .query_map(params![project_id, since, until], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<Result<_, _>>()?;
            Ok(raw
                .into_iter()
                .map(|(model, usage, missing)| {
                    let known_usage: Option<crate::model::Usage> =
                        usage.and_then(|s| serde_json::from_str(&s).ok());
                    ProjectRunUsage {
                        model: model.filter(|m| !m.is_empty()),
                        usage: if missing > 0 { None } else { known_usage.clone() },
                        known_usage,
                    }
                })
                .collect())
        })
    }

    /// [`Self::project_run_usage_between`] が費用の和から除いた「起動前に終わった chat run」の数（同じ案件・同じ
    /// `since <= started_at < until`・同じ判定）。task run は数えない。
    pub fn project_not_launched_runs_between(
        &self,
        project_id: &str,
        since: &str,
        until: &str,
    ) -> Result<u32, StoreError> {
        let not_launched = not_launched_sql();
        self.with_read_conn(|c| {
            Ok(c.query_row(
                &format!(
                    "SELECT COUNT(*) FROM chat_runs r JOIN chat_threads t ON t.id = r.thread_id \
                     WHERE t.project_id = ?1 AND r.started_at >= ?2 AND r.started_at < ?3 \
                       AND {not_launched}"
                ),
                params![project_id, since, until],
                |r| r.get(0),
            )?)
        })
    }

    /// 案件 CoS の起動数（`since <= started_at < until`）。queued の行は起動していないため数えない。
    pub fn project_chat_starts_between(
        &self,
        project_id: &str,
        since: &str,
        until: &str,
    ) -> Result<u32, StoreError> {
        self.with_read_conn(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM chat_runs r JOIN chat_threads t ON t.id = r.thread_id
                 WHERE t.project_id = ?1 AND r.started_at >= ?2 AND r.started_at < ?3",
                params![project_id, since, until],
                |r| r.get(0),
            )?)
        })
    }

    /// P1-G: `blocked` の案件を根拠つきで再開する（1 つの transaction）。止めた判断の行と `revision`・`human_epoch`
    /// が読んだ時と同じで、案件が動いていて lease が無いときだけ書く（CAS）。判断の行（効果なし）に根拠を残し、
    /// `revision + 1`・phase `active`・`next_wake_at = now`。積もった wake は消費せず、次の評価が 1 回で畳む。
    /// 書いたら `true`。人の操作か他の instance が先なら何も書かず `false`。
    pub fn project_controller_resume(
        &self,
        resume: &ProjectResume,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let open: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM project_controller_state s \
               JOIN projects p ON p.id = s.project_id \
              WHERE s.project_id = ?1 AND s.phase = 'blocked' AND s.mode IN ('shadow', 'enabled') \
                AND s.revision = ?2 AND s.human_epoch = ?3 AND s.last_decision_id = ?4 \
                AND s.lease_owner IS NULL \
                AND p.status IN ('active', 'proposed') AND p.archived_at IS NULL)",
            params![
                resume.project_id,
                resume.revision,
                resume.human_epoch,
                resume.blocked_decision_id
            ],
            |r| r.get(0),
        )?;
        if !open {
            return Ok(false);
        }
        tx.execute(
            "INSERT INTO project_controller_decisions (id, project_id, based_on_revision, human_epoch, \
               snapshot_sha256, request_sha256, generation_id, mode, proposal_json, validation, \
               effect_json, effect_state, effect_ref, progress_epoch, created_at, applied_at) \
             SELECT ?1, project_id, revision, human_epoch, '', ?3, NULL, mode, ?4, ?5, ?6, 'none', NULL, \
               progress_epoch, ?7, NULL FROM project_controller_state WHERE project_id = ?2",
            params![
                resume.decision_id,
                resume.project_id,
                resume.request_sha256,
                resume.record_json,
                resume.rule_id,
                resume.effect_json,
                at
            ],
        )?;
        tx.execute(
            "UPDATE project_controller_state SET phase = 'active', revision = revision + 1, \
               next_wake_at = ?2, coalesce_until = NULL, retry_count = 0, backoff_until = NULL, \
               last_decision_id = ?3, updated_at = ?2, updated_by = 'daemon' WHERE project_id = ?1",
            params![resume.project_id, at, resume.decision_id],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// P1-G: 翌日に再評価しないと決めた予算の止まり。notice 1 件と、期限（`next_wake_at`）の取り消しを同じ
    /// transaction で書く。以後は人が mode を置き直すまで `blocked` のまま。止めた判断が同じときだけ書く。
    pub fn project_controller_budget_hold(
        &self,
        project_id: &str,
        blocked_decision_id: &str,
        notice: &crate::feed::NoticeEvent,
        now: OffsetDateTime,
    ) -> Result<bool, StoreError> {
        let at = format_rfc3339(now)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE project_controller_state SET next_wake_at = NULL, updated_at = ?3, updated_by = 'daemon' \
             WHERE project_id = ?1 AND phase = 'blocked' AND last_decision_id = ?2",
            params![project_id, blocked_decision_id, at],
        )?;
        if changed > 0 {
            crate::feed::record_in_tx(&tx, notice)?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    /// A6（crash の B5）: 問いの notice（`source_key`）が既に記録されているか。`escalate` は notice と
    /// phase `blocked` を同じ tx で書くので、記録があれば効果は効いている。
    pub fn project_controller_notice_recorded(&self, source_key: &str) -> Result<bool, StoreError> {
        self.with_read_conn(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM feed_sources WHERE source_key = ?1)",
                [source_key],
                |r| r.get(0),
            )?)
        })
    }
}
