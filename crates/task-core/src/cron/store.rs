//! ADR-0131 D1: `cron_jobs` / `cron_job_runs`（migration 0039）の読み書き。
//!
//! `TaskStore` の supertrait（`NotificationStore` 等と同じ形）なので、ディスパッチャの `Arc<dyn TaskStore>` から
//! 使える。発火の規則（D2・D3）は持たず、行の読み書きだけを行う（規則は `task_ops::cron`）。
//! 時刻は文字列比較せず、読んだ後に `OffsetDateTime` で比べる（RFC3339 の小数秒の有無で辞書順が崩れるため）。

use rusqlite::{Connection, Row, TransactionBehavior, params};
use time::OffsetDateTime;

use super::{
    CronCatchUp, CronJob, CronJobId, CronJobRun, CronJobRunId, CronOverlap, CronRunOutcome,
    CronTaskTemplate, CronTrigger,
};
use crate::model::TaskId;
use crate::store::{SqliteStore, StoreError, format_rfc3339, parse_rfc3339};

/// `cron_job_runs` の 1 行を書き換える値（`queued` → `created` 等、ADR-0131 D2）。
#[derive(Debug, Clone, PartialEq)]
pub struct CronJobRunUpdate {
    pub outcome: CronRunOutcome,
    pub task_id: Option<TaskId>,
    pub detail: Option<String>,
    pub recorded_at: OffsetDateTime,
}

pub trait CronJobStore: Send + Sync {
    /// job を 1 件足す。`name` が既にあれば `StoreError::InUse`（API は 409）。
    fn cron_job_insert(&self, job: &CronJob) -> Result<(), StoreError>;
    fn cron_job_get(&self, id: CronJobId) -> Result<Option<CronJob>, StoreError>;
    fn cron_job_get_by_name(&self, name: &str) -> Result<Option<CronJob>, StoreError>;
    /// 全 job（`name` 昇順）。
    fn cron_job_list(&self) -> Result<Vec<CronJob>, StoreError>;
    /// `id` の行の可変欄（`id`・`created_at` 以外）を書き換える。行が無ければ `Ok(false)`。
    /// `name` が他の job と重なれば `StoreError::InUse`。
    fn cron_job_update(&self, job: &CronJob) -> Result<bool, StoreError>;
    /// job と履歴を消す（作った task は残る）。行が無ければ `Ok(false)`。
    fn cron_job_delete(&self, id: CronJobId) -> Result<bool, StoreError>;
    /// `enabled` で `next_fire_at <= now` の job（`next_fire_at` 昇順、同時刻は `name` 順）。
    fn cron_job_due(&self, now: OffsetDateTime) -> Result<Vec<CronJob>, StoreError>;
    /// 1 回の発火の記録: 履歴 `runs` を足し、job の `next_fire_at` と `updated_at` を書き換える（同じ
    /// トランザクション）。`(job_id, scheduled_for, trigger)` が既にあれば何も書かず `StoreError::InUse`
    /// （二重発火の最後の砦）。job が無ければ `StoreError::Invalid`。
    fn cron_job_record(
        &self,
        job_id: CronJobId,
        next_fire_at: Option<OffsetDateTime>,
        updated_at: OffsetDateTime,
        runs: &[CronJobRun],
    ) -> Result<(), StoreError>;
    /// 履歴 1 行を書き換える（`queued` を `created` / `skipped_overlap` に閉じる）。行が無ければ `Ok(false)`。
    fn cron_job_run_update(
        &self,
        run_id: CronJobRunId,
        update: &CronJobRunUpdate,
    ) -> Result<bool, StoreError>;
    /// 履歴（記録した順の新しい順）。`limit = None` なら全件。
    fn cron_job_runs(
        &self,
        job_id: CronJobId,
        limit: Option<usize>,
    ) -> Result<Vec<CronJobRun>, StoreError>;
    /// 溜まっている `queued` の行（高々 1 件の想定。複数あれば最も古いもの）。
    fn cron_job_run_queued(&self, job_id: CronJobId) -> Result<Option<CronJobRun>, StoreError>;
    /// 最後に task を作った行（`outcome = created` の最新。D2 の重ね掛け判定に使う）。
    fn cron_job_run_last_created(
        &self,
        job_id: CronJobId,
    ) -> Result<Option<CronJobRun>, StoreError>;
}

const JOB_COLUMNS: &str = "id, name, enabled, schedule, timezone, overlap, catch_up, template_json, \
                           next_fire_at, created_at, updated_at";
const RUN_COLUMNS: &str =
    "id, job_id, scheduled_for, trigger, outcome, task_id, detail, recorded_at";

/// 読んだ 1 行（parse 前）。
struct JobRaw {
    id: String,
    name: String,
    enabled: bool,
    schedule: String,
    timezone: String,
    overlap: String,
    catch_up: String,
    template_json: String,
    next_fire_at: Option<String>,
    created_at: String,
    updated_at: String,
}

fn job_raw(row: &Row<'_>) -> rusqlite::Result<JobRaw> {
    Ok(JobRaw {
        id: row.get(0)?,
        name: row.get(1)?,
        enabled: row.get::<_, i64>(2)? != 0,
        schedule: row.get(3)?,
        timezone: row.get(4)?,
        overlap: row.get(5)?,
        catch_up: row.get(6)?,
        template_json: row.get(7)?,
        next_fire_at: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn invalid(what: &str, e: impl std::fmt::Display) -> StoreError {
    StoreError::Invalid(format!("{what}: {e}"))
}

impl JobRaw {
    fn into_job(self) -> Result<CronJob, StoreError> {
        Ok(CronJob {
            id: self.id.parse().map_err(|e| invalid("cron_jobs.id", e))?,
            name: self.name,
            enabled: self.enabled,
            schedule: self.schedule,
            timezone: self.timezone,
            overlap: self
                .overlap
                .parse::<CronOverlap>()
                .map_err(|e| invalid("cron_jobs.overlap", e))?,
            catch_up: self
                .catch_up
                .parse::<CronCatchUp>()
                .map_err(|e| invalid("cron_jobs.catch_up", e))?,
            template: serde_json::from_str::<CronTaskTemplate>(&self.template_json)?,
            next_fire_at: self
                .next_fire_at
                .as_deref()
                .map(parse_rfc3339)
                .transpose()?,
            created_at: parse_rfc3339(&self.created_at)?,
            updated_at: parse_rfc3339(&self.updated_at)?,
        })
    }
}

struct RunRaw {
    id: String,
    job_id: String,
    scheduled_for: String,
    trigger: String,
    outcome: String,
    task_id: Option<String>,
    detail: Option<String>,
    recorded_at: String,
}

fn run_raw(row: &Row<'_>) -> rusqlite::Result<RunRaw> {
    Ok(RunRaw {
        id: row.get(0)?,
        job_id: row.get(1)?,
        scheduled_for: row.get(2)?,
        trigger: row.get(3)?,
        outcome: row.get(4)?,
        task_id: row.get(5)?,
        detail: row.get(6)?,
        recorded_at: row.get(7)?,
    })
}

impl RunRaw {
    fn into_run(self) -> Result<CronJobRun, StoreError> {
        Ok(CronJobRun {
            id: self
                .id
                .parse()
                .map_err(|e| invalid("cron_job_runs.id", e))?,
            job_id: self
                .job_id
                .parse()
                .map_err(|e| invalid("cron_job_runs.job_id", e))?,
            scheduled_for: parse_rfc3339(&self.scheduled_for)?,
            trigger: self
                .trigger
                .parse::<CronTrigger>()
                .map_err(|e| invalid("cron_job_runs.trigger", e))?,
            outcome: self
                .outcome
                .parse::<CronRunOutcome>()
                .map_err(|e| invalid("cron_job_runs.outcome", e))?,
            task_id: self
                .task_id
                .as_deref()
                .map(|s| s.parse::<TaskId>())
                .transpose()
                .map_err(|e| invalid("cron_job_runs.task_id", e))?,
            detail: self.detail,
            recorded_at: parse_rfc3339(&self.recorded_at)?,
        })
    }
}

fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(inner, _)
            if inner.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                || inner.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
    )
}

fn map_unique(e: rusqlite::Error, kind: &'static str, id: String, detail: &str) -> StoreError {
    if is_unique_violation(&e) {
        StoreError::InUse {
            kind,
            id,
            detail: detail.to_string(),
        }
    } else {
        StoreError::Sqlite(e)
    }
}

fn query_jobs(
    conn: &Connection,
    where_sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<CronJob>, StoreError> {
    let sql = format!("SELECT {JOB_COLUMNS} FROM cron_jobs {where_sql}");
    let mut stmt = conn.prepare(&sql)?;
    let raws = stmt
        .query_map(args, job_raw)?
        .collect::<Result<Vec<_>, _>>()?;
    raws.into_iter().map(JobRaw::into_job).collect()
}

fn query_runs(
    conn: &Connection,
    where_sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<CronJobRun>, StoreError> {
    let sql = format!("SELECT {RUN_COLUMNS} FROM cron_job_runs {where_sql}");
    let mut stmt = conn.prepare(&sql)?;
    let raws = stmt
        .query_map(args, run_raw)?
        .collect::<Result<Vec<_>, _>>()?;
    raws.into_iter().map(RunRaw::into_run).collect()
}

fn insert_run(conn: &Connection, run: &CronJobRun) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO cron_job_runs (id, job_id, scheduled_for, trigger, outcome, task_id, detail, \
         recorded_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            run.id.to_string(),
            run.job_id.to_string(),
            format_rfc3339(run.scheduled_for)?,
            run.trigger.as_str(),
            run.outcome.as_str(),
            run.task_id.map(|t| t.to_string()),
            run.detail,
            format_rfc3339(run.recorded_at)?,
        ],
    )
    .map_err(|e| {
        map_unique(
            e,
            "cron_job_run",
            format!("{}@{}", run.job_id, run.trigger.as_str()),
            "a run for this job, scheduled time and trigger is already recorded",
        )
    })?;
    Ok(())
}

impl CronJobStore for SqliteStore {
    fn cron_job_insert(&self, job: &CronJob) -> Result<(), StoreError> {
        let conn = self.lock()?;
        conn.execute(
            &format!(
                "INSERT INTO cron_jobs ({JOB_COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
            ),
            params![
                job.id.to_string(),
                job.name,
                i64::from(job.enabled),
                job.schedule,
                job.timezone,
                job.overlap.as_str(),
                job.catch_up.as_str(),
                serde_json::to_string(&job.template)?,
                job.next_fire_at.map(format_rfc3339).transpose()?,
                format_rfc3339(job.created_at)?,
                format_rfc3339(job.updated_at)?,
            ],
        )
        .map_err(|e| map_unique(e, "cron_job", job.name.clone(), "name already exists"))?;
        Ok(())
    }

    fn cron_job_get(&self, id: CronJobId) -> Result<Option<CronJob>, StoreError> {
        self.with_read_conn(|conn| {
            Ok(query_jobs(conn, "WHERE id = ?1", &[&id.to_string()])?
                .into_iter()
                .next())
        })
    }

    fn cron_job_get_by_name(&self, name: &str) -> Result<Option<CronJob>, StoreError> {
        self.with_read_conn(|conn| {
            Ok(query_jobs(conn, "WHERE name = ?1", &[&name])?
                .into_iter()
                .next())
        })
    }

    fn cron_job_list(&self) -> Result<Vec<CronJob>, StoreError> {
        self.with_read_conn(|conn| query_jobs(conn, "ORDER BY name ASC", &[]))
    }

    fn cron_job_update(&self, job: &CronJob) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let n = conn
            .execute(
                "UPDATE cron_jobs SET name = ?2, enabled = ?3, schedule = ?4, timezone = ?5, \
                 overlap = ?6, catch_up = ?7, template_json = ?8, next_fire_at = ?9, updated_at = ?10 \
                 WHERE id = ?1",
                params![
                    job.id.to_string(),
                    job.name,
                    i64::from(job.enabled),
                    job.schedule,
                    job.timezone,
                    job.overlap.as_str(),
                    job.catch_up.as_str(),
                    serde_json::to_string(&job.template)?,
                    job.next_fire_at.map(format_rfc3339).transpose()?,
                    format_rfc3339(job.updated_at)?,
                ],
            )
            .map_err(|e| map_unique(e, "cron_job", job.name.clone(), "name already exists"))?;
        Ok(n > 0)
    }

    fn cron_job_delete(&self, id: CronJobId) -> Result<bool, StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // `foreign_keys` は有効にしていないので ON DELETE CASCADE に頼らず消す。
        tx.execute(
            "DELETE FROM cron_job_runs WHERE job_id = ?1",
            params![id.to_string()],
        )?;
        let n = tx.execute(
            "DELETE FROM cron_jobs WHERE id = ?1",
            params![id.to_string()],
        )?;
        tx.commit()?;
        Ok(n > 0)
    }

    fn cron_job_due(&self, now: OffsetDateTime) -> Result<Vec<CronJob>, StoreError> {
        let mut due: Vec<CronJob> = self.with_read_conn(|conn| {
            query_jobs(conn, "WHERE enabled = 1 AND next_fire_at IS NOT NULL", &[])
        })?;
        due.retain(|j| j.next_fire_at.is_some_and(|t| t <= now));
        due.sort_by(|a, b| {
            a.next_fire_at
                .cmp(&b.next_fire_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(due)
    }

    fn cron_job_record(
        &self,
        job_id: CronJobId,
        next_fire_at: Option<OffsetDateTime>,
        updated_at: OffsetDateTime,
        runs: &[CronJobRun],
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let n = tx.execute(
            "UPDATE cron_jobs SET next_fire_at = ?2, updated_at = ?3 WHERE id = ?1",
            params![
                job_id.to_string(),
                next_fire_at.map(format_rfc3339).transpose()?,
                format_rfc3339(updated_at)?,
            ],
        )?;
        if n == 0 {
            return Err(StoreError::Invalid(format!("cron job {job_id} not found")));
        }
        for run in runs {
            if run.job_id != job_id {
                return Err(StoreError::Invalid(format!(
                    "cron run {} belongs to job {}, not {job_id}",
                    run.id, run.job_id
                )));
            }
            insert_run(&tx, run)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn cron_job_run_update(
        &self,
        run_id: CronJobRunId,
        update: &CronJobRunUpdate,
    ) -> Result<bool, StoreError> {
        let conn = self.lock()?;
        let n = conn.execute(
            "UPDATE cron_job_runs SET outcome = ?2, task_id = ?3, detail = ?4, recorded_at = ?5 \
             WHERE id = ?1",
            params![
                run_id.to_string(),
                update.outcome.as_str(),
                update.task_id.map(|t| t.to_string()),
                update.detail,
                format_rfc3339(update.recorded_at)?,
            ],
        )?;
        Ok(n > 0)
    }

    fn cron_job_runs(
        &self,
        job_id: CronJobId,
        limit: Option<usize>,
    ) -> Result<Vec<CronJobRun>, StoreError> {
        let limit = limit.map_or(-1i64, |n| i64::try_from(n).unwrap_or(i64::MAX));
        self.with_read_conn(|conn| {
            query_runs(
                conn,
                "WHERE job_id = ?1 ORDER BY rowid DESC LIMIT ?2",
                &[&job_id.to_string(), &limit],
            )
        })
    }

    fn cron_job_run_queued(&self, job_id: CronJobId) -> Result<Option<CronJobRun>, StoreError> {
        self.with_read_conn(|conn| {
            Ok(query_runs(
                conn,
                "WHERE job_id = ?1 AND outcome = 'queued' ORDER BY rowid ASC LIMIT 1",
                &[&job_id.to_string()],
            )?
            .into_iter()
            .next())
        })
    }

    fn cron_job_run_last_created(
        &self,
        job_id: CronJobId,
    ) -> Result<Option<CronJobRun>, StoreError> {
        self.with_read_conn(|conn| {
            Ok(query_runs(
                conn,
                "WHERE job_id = ?1 AND outcome = 'created' ORDER BY rowid DESC LIMIT 1",
                &[&job_id.to_string()],
            )?
            .into_iter()
            .next())
        })
    }
}
