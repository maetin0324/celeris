//! ADR-0131 D2〜D5: 定期実行（cron job）の発火と操作。
//!
//! job 1 件を `now`（引数の時計）で評価し、D1〜D3 の規則で `cron_job_runs` を書き、雛形から
//! `task_ops::add` の経路で通常の task を作る（routing・計画・review は通常どおり）。daemon の tick
//! （[`fire_due`]）と API の手動実行（[`run_now`]）は同じ関数を通る。LLM・外部 process・network は呼ばない。
//!
//! 書き込みの順（二重発火の防止）: 先に task を組み立てて id を決め、`cron_job_record` で履歴
//! （`outcome = created`、`task_id` 付き）と `next_fire_at` を 1 transaction で書く（`UNIQUE (job_id,
//! scheduled_for, trigger)` が最後の砦）。それが通ってから task を挿入し、挿入に失敗したら履歴を `error`
//! に書き換える。

use task_core::cron::{CronJobRunUpdate, due_fires, render_title};
use task_core::{
    CronCatchUp, CronJob, CronJobId, CronJobRun, CronJobRunId, CronOverlap, CronRunOutcome,
    CronSchedule, CronTaskTemplate, CronTrigger, CronTz, GenreSpec, ProjectId, RoleSpec, Status,
    StoreError, Task, TaskId, TaskStore,
};
use time::{Duration, OffsetDateTime};

use crate::add::{CriterionSpec, NewTaskSpec, PriorityInput, build_task_with_roles};
use crate::error::OpsError;

/// cron が作る task に付けるラベル（一覧で見分けるため）。
pub const CRON_TASK_LABEL: &str = "cron";

/// ADR-0131 付記 D10 (3): 雛形の `extra["mode"]` を発火時に task へ写したラベル。task の `Created` event に
/// 入るので、後の job PATCH で既存 task の mode は変わらない（`Task.mode` とは別の値）。
pub const MODE_DRY_RUN_LABEL: &str = "mode-dry-run";
pub const MODE_APPLY_LABEL: &str = "mode-apply";

/// 雛形の `extra["mode"]` を発火時のラベルに写す（`mode` が無い雛形は `None`）。
pub fn template_mode_label(template: &CronTaskTemplate) -> Option<&'static str> {
    let mode = template.extra.get("mode")?;
    // validate_job が新規・更新時の値を検証する。古い保存済み job に不正値が残っていても、
    // 誤って apply しないよう安全側の dry-run として扱う。
    Some(if mode.as_str() == Some("apply") {
        MODE_APPLY_LABEL
    } else {
        MODE_DRY_RUN_LABEL
    })
}

/// cron 由来の task に発火時に固定された mode（`dry_run` | `apply`）。ラベルが無ければ `dry_run`。
pub fn task_mode(task: &Task) -> &'static str {
    if task.labels.iter().any(|l| l == MODE_APPLY_LABEL) {
        "apply"
    } else {
        "dry_run"
    }
}

/// 予定時刻からの遅れがこれ以内なら通常運転（`trigger = schedule`）とみなす（ADR-0131 D3「1 tick 以内」）。
/// tick の既定間隔（数秒）に余裕を持たせた値。
pub const DEFAULT_ON_TIME_GRACE: Duration = Duration::minutes(2);

/// 発火に使う設定（役割・分野の既定と、通常運転とみなす遅れ）。
#[derive(Debug, Clone, Copy)]
pub struct CronFireContext<'a> {
    pub roles: &'a [RoleSpec],
    pub genres: &'a [GenreSpec],
    pub on_time_grace: Duration,
}

impl Default for CronFireContext<'_> {
    fn default() -> Self {
        Self {
            roles: &[],
            genres: &[],
            on_time_grace: DEFAULT_ON_TIME_GRACE,
        }
    }
}

/// 1 job の 1 回の評価の結果（tick の log と API の応答に使う）。
#[derive(Debug, Clone, PartialEq)]
pub struct CronFireOutcome {
    pub job_id: CronJobId,
    pub job_name: String,
    /// この評価で書いた（または書き換えた）履歴。古い順。
    pub runs: Vec<CronJobRun>,
    /// 作った task（高々 1 件）。
    pub task_id: Option<TaskId>,
}

// ---- 雛形 ----

/// 雛形を `NewTaskSpec` に写す（`title` は呼び出し側で `{date}` を置き換えたもの）。初期状態は `ready`
/// （人が登録した job の定時の発火で、人の Go を挟まない）。`lane` は書いたときだけ人の明示として効く。
pub fn template_to_spec(
    store: &dyn TaskStore,
    template: &CronTaskTemplate,
    title: String,
) -> Result<NewTaskSpec, OpsError> {
    let acceptance = template
        .acceptance
        .iter()
        .enumerate()
        .map(|(i, v)| {
            serde_json::from_value::<CriterionSpec>(v.clone())
                .map_err(|e| OpsError::Validation(format!("template.acceptance[{i}]: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let priority = template
        .priority
        .clone()
        .map(serde_json::from_value::<PriorityInput>)
        .transpose()
        .map_err(|e| OpsError::Validation(format!("template.priority: {e}")))?;
    let project_id = template
        .project
        .as_deref()
        .map(|p| resolve_project(store, p))
        .transpose()?;
    let mut spec: NewTaskSpec = serde_json::from_value(serde_json::json!({
        "title": title,
        "objective": template.objective,
        "acceptance": [],
    }))
    .map_err(|e| OpsError::Validation(format!("template: {e}")))?;
    spec.acceptance = acceptance;
    spec.tier = template.lane;
    spec.priority = priority;
    spec.assignee = template.assignee.clone();
    spec.genre = template.harness.clone();
    spec.project_id = project_id;
    spec.repos = template.repos.clone();
    spec.labels = vec![CRON_TASK_LABEL.to_string()];
    if let Some(label) = template_mode_label(template) {
        spec.labels.push(label.to_string());
    }
    spec.status = Some(Status::Ready);
    Ok(spec)
}

/// 雛形の `project` は ULID か案件の題名。
fn resolve_project(store: &dyn TaskStore, key: &str) -> Result<ProjectId, OpsError> {
    if let Ok(id) = key.parse::<ProjectId>() {
        if store.project_get(id)?.is_some() {
            return Ok(id);
        }
        return Err(OpsError::ProjectNotFound(id));
    }
    let matches: Vec<ProjectId> = store
        .project_list()?
        .into_iter()
        .filter(|p| p.title == key)
        .map(|p| p.id)
        .collect();
    match matches.as_slice() {
        [id] => Ok(*id),
        [] => Err(OpsError::Validation(format!(
            "template.project: no project named {key:?}"
        ))),
        _ => Err(OpsError::Validation(format!(
            "template.project: {key:?} matches more than one project; use its id"
        ))),
    }
}

fn build_task(
    store: &dyn TaskStore,
    ctx: &CronFireContext<'_>,
    job: &CronJob,
    fired_at: OffsetDateTime,
    now: OffsetDateTime,
) -> Result<Task, OpsError> {
    let tz = parse_tz(job)?;
    let title = render_title(&job.template.title, &tz, fired_at)
        .map_err(|e| OpsError::Validation(e.to_string()))?;
    let spec = template_to_spec(store, &job.template, title)?;
    build_task_with_roles(store, spec, ctx.roles, ctx.genres, now)
}

fn parse_schedule(job: &CronJob) -> Result<CronSchedule, OpsError> {
    job.schedule
        .parse::<CronSchedule>()
        .map_err(|e| OpsError::Validation(e.to_string()))
}

fn parse_tz(job: &CronJob) -> Result<CronTz, OpsError> {
    CronTz::iana(&job.timezone).map_err(|e| OpsError::Validation(e.to_string()))
}

// ---- 操作（作成・更新・一時停止・再開・削除） ----

/// 作成の入力（API の `POST /cron-jobs`・`celerisctl cron add`）。
#[derive(Debug, Clone, PartialEq)]
pub struct NewCronJob {
    pub name: String,
    pub schedule: String,
    pub timezone: String,
    pub overlap: CronOverlap,
    pub catch_up: CronCatchUp,
    pub enabled: bool,
    pub template: CronTaskTemplate,
}

/// 更新の入力（書いた欄だけ変える）。`enabled` は [`pause_job`] / [`resume_job`] で変える。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CronJobPatch {
    pub name: Option<String>,
    pub schedule: Option<String>,
    pub timezone: Option<String>,
    pub overlap: Option<CronOverlap>,
    pub catch_up: Option<CronCatchUp>,
    pub template: Option<CronTaskTemplate>,
}

/// job を検証する: 名前・cron 式・タイムゾーン・次回時刻があること・雛形が task に組み立てられること。
pub fn validate_job(
    store: &dyn TaskStore,
    ctx: &CronFireContext<'_>,
    job: &CronJob,
    now: OffsetDateTime,
) -> Result<(), OpsError> {
    if let Some(mode) = job.template.extra.get("mode")
        && !matches!(mode.as_str(), Some("dry_run" | "apply"))
    {
        return Err(OpsError::Validation(
            "template extra.mode must be \"dry_run\" or \"apply\"".to_string(),
        ));
    }
    if job.name.trim().is_empty() {
        return Err(OpsError::Validation("name must not be blank".to_string()));
    }
    if job.name.parse::<CronJobId>().is_ok() {
        return Err(OpsError::Validation(
            "name must not look like a job id (ULID)".to_string(),
        ));
    }
    if job
        .compute_next_after(now)
        .map_err(|e| OpsError::Validation(e.to_string()))?
        .is_none()
    {
        return Err(OpsError::Validation(format!(
            "schedule {:?} never fires within {} years",
            job.schedule,
            task_core::cron::SEARCH_YEARS
        )));
    }
    build_task(store, ctx, job, now, now).map(|_| ())
}

/// `{id}` は ULID か `name` のどちらでも引ける（ADR-0131 D5）。
pub fn resolve_job(store: &dyn TaskStore, key: &str) -> Result<Option<CronJob>, OpsError> {
    if let Ok(id) = key.parse::<CronJobId>()
        && let Some(job) = store.cron_job_get(id)?
    {
        return Ok(Some(job));
    }
    Ok(store.cron_job_get_by_name(key)?)
}

fn require_job(store: &dyn TaskStore, id: CronJobId) -> Result<CronJob, OpsError> {
    store
        .cron_job_get(id)?
        .ok_or_else(|| OpsError::Validation(format!("cron job {id} not found")))
}

pub fn create_job(
    store: &dyn TaskStore,
    ctx: &CronFireContext<'_>,
    new: NewCronJob,
    now: OffsetDateTime,
) -> Result<CronJob, OpsError> {
    let mut job = CronJob {
        id: CronJobId::new(),
        name: new.name,
        enabled: new.enabled,
        schedule: new.schedule,
        timezone: new.timezone,
        overlap: new.overlap,
        catch_up: new.catch_up,
        template: new.template,
        next_fire_at: None,
        created_at: now,
        updated_at: now,
    };
    validate_job(store, ctx, &job, now)?;
    if job.enabled {
        job.next_fire_at = job
            .compute_next_after(now)
            .map_err(|e| OpsError::Validation(e.to_string()))?;
    }
    store.cron_job_insert(&job)?;
    Ok(job)
}

/// 更新。schedule / timezone を変えたら（有効なら）`next_fire_at` を `now` から計算し直す。
pub fn update_job(
    store: &dyn TaskStore,
    ctx: &CronFireContext<'_>,
    id: CronJobId,
    patch: CronJobPatch,
    now: OffsetDateTime,
) -> Result<CronJob, OpsError> {
    let mut job = require_job(store, id)?;
    let reschedule = patch.schedule.as_ref().is_some_and(|s| *s != job.schedule)
        || patch.timezone.as_ref().is_some_and(|t| *t != job.timezone);
    if let Some(name) = patch.name {
        job.name = name;
    }
    if let Some(schedule) = patch.schedule {
        job.schedule = schedule;
    }
    if let Some(timezone) = patch.timezone {
        job.timezone = timezone;
    }
    if let Some(overlap) = patch.overlap {
        job.overlap = overlap;
    }
    if let Some(catch_up) = patch.catch_up {
        job.catch_up = catch_up;
    }
    if let Some(template) = patch.template {
        job.template = template;
    }
    validate_job(store, ctx, &job, now)?;
    if reschedule && job.enabled {
        job.next_fire_at = job
            .compute_next_after(now)
            .map_err(|e| OpsError::Validation(e.to_string()))?;
    }
    job.updated_at = now;
    store.cron_job_update(&job)?;
    Ok(job)
}

/// 一時停止: `next_fire_at` を消し、溜まっている `queued` を `skipped_overlap`（detail "paused"）に閉じる。
pub fn pause_job(
    store: &dyn TaskStore,
    id: CronJobId,
    now: OffsetDateTime,
) -> Result<CronJob, OpsError> {
    let mut job = require_job(store, id)?;
    if let Some(queued) = store.cron_job_run_queued(id)? {
        store.cron_job_run_update(
            queued.id,
            &CronJobRunUpdate {
                outcome: CronRunOutcome::SkippedOverlap,
                task_id: None,
                detail: Some("paused".to_string()),
                recorded_at: now,
            },
        )?;
    }
    job.enabled = false;
    job.next_fire_at = None;
    job.updated_at = now;
    store.cron_job_update(&job)?;
    Ok(job)
}

/// 再開: 一時停止中に過ぎた時刻は取りこぼしにしない（`next_fire_at = next_after(now)`、ADR-0131 D3）。
/// 既に有効な job は何も変えない。
pub fn resume_job(
    store: &dyn TaskStore,
    id: CronJobId,
    now: OffsetDateTime,
) -> Result<CronJob, OpsError> {
    let mut job = require_job(store, id)?;
    if job.enabled {
        return Ok(job);
    }
    job.enabled = true;
    job.next_fire_at = job
        .compute_next_after(now)
        .map_err(|e| OpsError::Validation(e.to_string()))?;
    job.updated_at = now;
    store.cron_job_update(&job)?;
    Ok(job)
}

/// 削除（履歴も消える。作った task は残る）。無ければ `Ok(false)`。
pub fn delete_job(store: &dyn TaskStore, id: CronJobId) -> Result<bool, OpsError> {
    Ok(store.cron_job_delete(id)?)
}

// ---- 発火 ----

/// tick の段: 有効な job をすべて評価する（`queued` の消化と、`next_fire_at <= now` の発火）。1 job の失敗は
/// その job の結果に `error` として残すか `Err` を集めて、他の job は続ける。
pub fn fire_due(
    store: &dyn TaskStore,
    now: OffsetDateTime,
) -> Vec<Result<CronFireOutcome, OpsError>> {
    fire_due_with(store, &CronFireContext::default(), now)
}

pub fn fire_due_with(
    store: &dyn TaskStore,
    ctx: &CronFireContext<'_>,
    now: OffsetDateTime,
) -> Vec<Result<CronFireOutcome, OpsError>> {
    let jobs = match store.cron_job_list() {
        Ok(jobs) => jobs,
        Err(e) => return vec![Err(e.into())],
    };
    jobs.into_iter()
        .filter(|j| j.enabled)
        .map(|job| fire_job(store, ctx, &job, now))
        .filter(|r| !matches!(r, Ok(o) if o.runs.is_empty()))
        .collect()
}

/// job 1 件を `now` で評価する: (1) `queued` があり前回 task が終端なら task を作る、(2) 予定時刻が
/// 過ぎていれば D2・D3 の規則で発火する。何もしなければ `runs` が空の結果を返す。
pub fn fire_job(
    store: &dyn TaskStore,
    ctx: &CronFireContext<'_>,
    job: &CronJob,
    now: OffsetDateTime,
) -> Result<CronFireOutcome, OpsError> {
    let mut out = CronFireOutcome {
        job_id: job.id,
        job_name: job.name.clone(),
        runs: Vec::new(),
        task_id: None,
    };
    if job.enabled {
        drain_queued(store, ctx, job, now, &mut out)?;
    }
    let Some(next_fire_at) = job.next_fire_at.filter(|t| job.enabled && *t <= now) else {
        return Ok(out);
    };
    let due = match parse_schedule(job)
        .and_then(|s| parse_tz(job).map(|tz| (s, tz)))
        .and_then(|(s, tz)| {
            due_fires(&s, &tz, next_fire_at, now).map_err(|e| OpsError::Validation(e.to_string()))
        }) {
        Ok(Some(due)) => due,
        Ok(None) => return Ok(out),
        Err(e) => {
            // 式・タイムゾーンが読めない: `error` を残し、`next_fire_at` も `enabled` も変えない（次の tick で
            // 再試行。同じ予定時刻の `error` は UNIQUE で 1 行だけ）。
            let run = new_run(
                job.id,
                next_fire_at,
                CronTrigger::Schedule,
                CronRunOutcome::Error,
                None,
                Some(e.to_string()),
                now,
            );
            match store.cron_job_record(job.id, job.next_fire_at, now, std::slice::from_ref(&run)) {
                Ok(()) => out.runs.push(run),
                Err(StoreError::InUse { .. }) => {}
                Err(e) => return Err(e.into()),
            }
            return Ok(out);
        }
    };

    let on_time = now - due.latest <= ctx.on_time_grace;
    let mut runs = Vec::new();
    // 過ぎた予定時刻のうち発火しないもの（ADR-0131 D3）。
    let (missed, fire) = match (on_time, job.catch_up) {
        (true, _) => (due.count - 1, Some(CronTrigger::Schedule)),
        (false, CronCatchUp::Latest) => (due.count - 1, Some(CronTrigger::CatchUp)),
        (false, CronCatchUp::Skip) => (due.count, None),
    };
    if missed > 0 {
        let more = if due.count_capped { "+" } else { "" };
        let last_missed = if fire.is_some() {
            "before the latest".to_string()
        } else {
            format_time(due.latest)
        };
        runs.push(new_run(
            job.id,
            due.earliest,
            CronTrigger::CatchUp,
            CronRunOutcome::SkippedMissed,
            None,
            Some(format!(
                "missed {missed}{more} scheduled time(s) from {} to {last_missed}",
                format_time(due.earliest)
            )),
            now,
        ));
    }
    let mut task = None;
    if let Some(trigger) = fire {
        let (run, built) = decide_fire(store, ctx, job, due.latest, trigger, now)?;
        runs.push(run);
        task = built;
    }
    record_and_create(store, job, due.next, runs, task, now, &mut out)?;
    Ok(out)
}

/// 手動実行（`trigger = manual`、`scheduled_for = now`）。D2 の重ね掛けの規則に従い、`next_fire_at` は
/// 変えない。同じ時刻に 2 回押したら `StoreError::InUse`（API は 409）。一時停止中の job も実行できる
/// （ただし `queue` は溜めず `skipped_overlap`。一時停止中は `queued` を消化しないため）。
pub fn run_now(
    store: &dyn TaskStore,
    job_id: CronJobId,
    now: OffsetDateTime,
) -> Result<CronFireOutcome, OpsError> {
    run_now_with(store, &CronFireContext::default(), job_id, now)
}

pub fn run_now_with(
    store: &dyn TaskStore,
    ctx: &CronFireContext<'_>,
    job_id: CronJobId,
    now: OffsetDateTime,
) -> Result<CronFireOutcome, OpsError> {
    let job = require_job(store, job_id)?;
    let mut out = CronFireOutcome {
        job_id: job.id,
        job_name: job.name.clone(),
        runs: Vec::new(),
        task_id: None,
    };
    let (run, task) = decide_fire(store, ctx, &job, now, CronTrigger::Manual, now)?;
    record_and_create(
        store,
        &job,
        job.next_fire_at,
        vec![run],
        task,
        now,
        &mut out,
    )?;
    Ok(out)
}

/// 前回の task がまだ動いているか（ADR-0131 D2）。動いていればその task を返す。作った task が消えて
/// いれば動いていないとみなす。
fn running_previous(store: &dyn TaskStore, job: &CronJob) -> Result<Option<Task>, OpsError> {
    let Some(last) = store.cron_job_run_last_created(job.id)? else {
        return Ok(None);
    };
    let Some(task_id) = last.task_id else {
        return Ok(None);
    };
    Ok(store.get(task_id)?.filter(|t| !t.status.is_terminal()))
}

/// 1 回の発火を決める: 重ならなければ task を組み立てて `created`、重なれば `skip` / `queue` の規則。
fn decide_fire(
    store: &dyn TaskStore,
    ctx: &CronFireContext<'_>,
    job: &CronJob,
    scheduled_for: OffsetDateTime,
    trigger: CronTrigger,
    now: OffsetDateTime,
) -> Result<(CronJobRun, Option<Task>), OpsError> {
    if let Some(prev) = running_previous(store, job)? {
        let busy = format!("previous task {} is still {:?}", prev.id, prev.status);
        let queue_ok = job.overlap == CronOverlap::Queue && job.enabled;
        let (outcome, detail) = if !queue_ok {
            let detail = if job.overlap == CronOverlap::Queue {
                format!("{busy}; paused job does not queue")
            } else {
                busy
            };
            (CronRunOutcome::SkippedOverlap, detail)
        } else if let Some(q) = store.cron_job_run_queued(job.id)? {
            (
                CronRunOutcome::SkippedOverlap,
                format!(
                    "{busy}; run for {} is already queued",
                    format_time(q.scheduled_for)
                ),
            )
        } else {
            (CronRunOutcome::Queued, busy)
        };
        let run = new_run(
            job.id,
            scheduled_for,
            trigger,
            outcome,
            None,
            Some(detail),
            now,
        );
        return Ok((run, None));
    }
    match build_task(store, ctx, job, scheduled_for, now) {
        Ok(task) => {
            let run = new_run(
                job.id,
                scheduled_for,
                trigger,
                CronRunOutcome::Created,
                Some(task.id),
                None,
                now,
            );
            Ok((run, Some(task)))
        }
        // 雛形が今は組み立てられない（案件が消えた等）: この予定時刻は `error` として進める。
        Err(OpsError::Store(e)) => Err(OpsError::Store(e)),
        Err(e) => {
            let run = new_run(
                job.id,
                scheduled_for,
                trigger,
                CronRunOutcome::Error,
                None,
                Some(e.to_string()),
                now,
            );
            Ok((run, None))
        }
    }
}

/// 履歴と `next_fire_at` を 1 transaction で書き、通ってから task を挿入する。
fn record_and_create(
    store: &dyn TaskStore,
    job: &CronJob,
    next_fire_at: Option<OffsetDateTime>,
    runs: Vec<CronJobRun>,
    task: Option<Task>,
    now: OffsetDateTime,
    out: &mut CronFireOutcome,
) -> Result<(), OpsError> {
    store.cron_job_record(job.id, next_fire_at, now, &runs)?;
    out.runs.extend(runs);
    if let Some(task) = task {
        insert_created(store, task, now, out)?;
    }
    Ok(())
}

/// `created` の履歴を書いた後に task を挿入する。失敗したら履歴を `error` に書き換える。
fn insert_created(
    store: &dyn TaskStore,
    task: Task,
    now: OffsetDateTime,
    out: &mut CronFireOutcome,
) -> Result<(), OpsError> {
    let task_id = task.id;
    match store.create_task(&task, Vec::new()) {
        Ok(()) => {
            out.task_id = Some(task_id);
            Ok(())
        }
        Err(e) => {
            let detail = format!("creating task failed: {e}");
            if let Some(run) = out.runs.iter_mut().find(|r| r.task_id == Some(task_id)) {
                store.cron_job_run_update(
                    run.id,
                    &CronJobRunUpdate {
                        outcome: CronRunOutcome::Error,
                        task_id: None,
                        detail: Some(detail.clone()),
                        recorded_at: now,
                    },
                )?;
                run.outcome = CronRunOutcome::Error;
                run.task_id = None;
                run.detail = Some(detail);
                run.recorded_at = now;
            }
            Err(e.into())
        }
    }
}

/// `queued` の行があり前回 task が終端なら、その行を `created` にして task を作る（ADR-0131 D2）。
fn drain_queued(
    store: &dyn TaskStore,
    ctx: &CronFireContext<'_>,
    job: &CronJob,
    now: OffsetDateTime,
    out: &mut CronFireOutcome,
) -> Result<(), OpsError> {
    let Some(queued) = store.cron_job_run_queued(job.id)? else {
        return Ok(());
    };
    if running_previous(store, job)?.is_some() {
        return Ok(());
    }
    let (update, task) = match build_task(store, ctx, job, queued.scheduled_for, now) {
        Ok(task) => (
            CronJobRunUpdate {
                outcome: CronRunOutcome::Created,
                task_id: Some(task.id),
                detail: None,
                recorded_at: now,
            },
            Some(task),
        ),
        Err(OpsError::Store(e)) => return Err(OpsError::Store(e)),
        Err(e) => (
            CronJobRunUpdate {
                outcome: CronRunOutcome::Error,
                task_id: None,
                detail: Some(e.to_string()),
                recorded_at: now,
            },
            None,
        ),
    };
    store.cron_job_run_update(queued.id, &update)?;
    out.runs.push(CronJobRun {
        outcome: update.outcome,
        task_id: update.task_id,
        detail: update.detail,
        recorded_at: now,
        ..queued
    });
    if let Some(task) = task {
        insert_created(store, task, now, out)?;
    }
    Ok(())
}

fn new_run(
    job_id: CronJobId,
    scheduled_for: OffsetDateTime,
    trigger: CronTrigger,
    outcome: CronRunOutcome,
    task_id: Option<TaskId>,
    detail: Option<String>,
    now: OffsetDateTime,
) -> CronJobRun {
    CronJobRun {
        id: CronJobRunId::new(),
        job_id,
        scheduled_for,
        trigger,
        outcome,
        task_id,
        detail,
        recorded_at: now,
    }
}

fn format_time(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| t.to_string())
}

#[cfg(test)]
mod tests;
