//! ADR 2026-11-08-health-enforcement-loop (6) D6.1・D6.3〜D6.5: 全体棚卸し（cron の種類 `health_inventory`）の runner。
//!
//! - [`post`]: daemon の tick の sink（`dispatcher::cos_chat::review::CosReviewSink`）が 1 行で委譲する口。同じ job の
//!   前回の棚卸しが走っている間は [`CronInputPost::Busy`]（履歴は `skipped_overlap`）。走っていなければ案件の Git の
//!   repo を読み、[`InventoryFire::run`] を tick の外（`spawn_blocking`）で始める。`CronJobRun.detail` には始めたこと
//!   （案件と repo の数）だけを書き、評価・省略・対象外の数の正本は表 `health_inventory_runs` の行の集計
//!   （`SqliteStore::health_inventory_fire_counts`）。
//! - [`InventoryRuns`]: Busy の印（job id → 実行中の `JoinHandle`）。Dispatcher の欄で、`in_flight` に数えるので、
//!   draining の instance は棚卸しの終わりを待ってから exit する。
//! - [`InventoryFire::inventory_repo`]: 1 repo の差分検査。既定の branch の HEAD を読む（ref・作業ツリーは変えない）。
//!   検査器の選択と remote workspace の除外は `repo_facts` → `applicability` だけで決める（`celeris::delivery` の
//!   `check_required_evidence` と同じ）。base は最後に評価できた行（verdict が Error でない `evaluated`）の head_sha、
//!   初回は base = head。`(head_tree, checker.sha256)` が最後に評価できた行と同じなら検査器を走らせない。
//!   検査器は 1 repo につき 1 回（`gate::run_health_check_with_details`）。
//! - [`InventoryFire::file`]（D6.6・D6.7）: 同じ検査器の実行が返した `HealthEvidence` と `HealthBlockingDetail` を
//!   そのまま `task_ops::health_debt::file_health_debts` に渡す（検査器を走らせ直さず、report を読み直さない）。
//!   起票の枠 [`MAX_DEBTS_PER_FIRE`] は 1 回の発火の repo をまたいで数える。起票した task id・既存で省いた数・
//!   上限で残した数は、その repo の行に入れて 1 回だけ追記する（行は更新しない）。
//!
//! LLM・run・chat を起こさない（I4）。起票は draft の task だけで、計画・子 task・run を作らない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use task_core::health_inventory::{HealthInventoryOutcome, HealthInventoryRun};
use task_core::required_checks::health::{
    CheckerId, HealthApplicability, HealthBlockingDetail, HealthEvidence, HealthVerdict,
    applicability, repo_facts,
};
use task_core::{
    CronJob, CronJobId, GenreSpec, ProjectId, ProjectRepo, RepoId, RepoKind, RoleSpec, SqliteStore,
    TaskStore, WorkspaceSpec,
};
use task_ops::cron_jobs::{CronFireContext, CronInputPost};
use task_ops::health_debt::{
    FiledHealthDebts, HealthDebtInput, MAX_DEBTS_PER_FIRE, file_health_debts,
};
use time::OffsetDateTime;
use tokio::task::JoinHandle;
use ulid::Ulid;

use crate::dispatcher::cos_chat::review::CosReviewSink;
use crate::health_review::gate::{bundled_checker, run_health_check_with_details};
use crate::health_review::input::HEALTH_INPUT_TIMEOUT;
use crate::health_review::policy::git_stdout;

/// Busy の印: job ごとの実行中の棚卸し（Dispatcher の欄 `health_inventory`）。プロセス内メモリのみ（再起動の後は
/// 走っていない。次の発火で同じ予定時刻の行が無い repo から評価し直す）。
#[derive(Debug, Default)]
pub struct InventoryRuns {
    running: HashMap<CronJobId, JoinHandle<()>>,
}

impl InventoryRuns {
    /// 終わっていない棚卸しの数（`Dispatcher::in_flight` の 1 項）。
    pub fn in_flight(&self) -> usize {
        self.running.values().filter(|h| !h.is_finished()).count()
    }

    /// `job` の棚卸しが走っているか（終わった印はここで消す）。
    fn is_running(&mut self, job: CronJobId) -> bool {
        self.running.retain(|_, handle| !handle.is_finished());
        self.running.contains_key(&job)
    }

    /// tick の sink に渡す口（印と、起票に使う `[[roles]]`・`[[genres]]` を持つ tick の文脈）。
    pub(crate) fn port<'a>(&'a mut self, ctx: &'a CronFireContext<'a>) -> Port<'a> {
        Port { runs: self, ctx }
    }

    /// `work` を tick の外（tokio の blocking pool）で始め、`job` の印にする。
    pub(crate) fn start(
        &mut self,
        job: CronJobId,
        work: impl FnOnce() + Send + 'static,
    ) -> Result<(), String> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|e| format!("no tokio runtime for the health inventory: {e}"))?;
        self.running.insert(job, runtime.spawn_blocking(work));
        Ok(())
    }

    /// 試験用: 走っている棚卸しの終わりを待つ。
    #[cfg(test)]
    pub(crate) async fn join_all(&mut self) {
        for (_, handle) in self.running.drain() {
            let _ = handle.await;
        }
    }
}

/// `CosReviewSink` の欄 `inventory`: Busy の印と tick の文脈（起票は cron の task と同じ role・genre の既定を使う）。
pub(crate) struct Port<'a> {
    pub runs: &'a mut InventoryRuns,
    pub ctx: &'a CronFireContext<'a>,
}

/// `CronInputSink::post_health_inventory` の daemon の実装（D6.1。`CosReviewSink` が 1 行で委譲する）。
pub(crate) fn post(
    sink: &mut CosReviewSink<'_>,
    job: &CronJob,
    scheduled_for: OffsetDateTime,
    project_id: &str,
) -> Result<CronInputPost, String> {
    let store = sink.launch.map(|launch| &launch.store);
    start_inventory(
        store,
        &mut *sink.inventory.runs,
        sink.inventory.ctx,
        job,
        scheduled_for,
        project_id,
        sink.now,
    )
}

/// 棚卸しを始める。`store` は daemon の SQLite store（CoS chat の launch が持つもの。無ければ発火は `error`）。
pub(crate) fn start_inventory(
    store: Option<&Arc<SqliteStore>>,
    runs: &mut InventoryRuns,
    ctx: &CronFireContext<'_>,
    job: &CronJob,
    scheduled_for: OffsetDateTime,
    project_id: &str,
    now: OffsetDateTime,
) -> Result<CronInputPost, String> {
    let Some(store) = store else {
        return Err("health inventory needs the daemon store (CoS chat is not configured)".into());
    };
    if runs.is_running(job.id) {
        return Ok(CronInputPost::Busy(
            "the previous health inventory of this job is still running".into(),
        ));
    }
    let project: ProjectId = project_id
        .parse()
        .map_err(|e| format!("health inventory project_id {project_id:?}: {e}"))?;
    let repos: Vec<ProjectRepo> = store
        .repo_list(project)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|r| r.kind == RepoKind::Git)
        .collect();
    let detail = format!(
        "started the health inventory of project {project}: {} git repo(s)",
        repos.len()
    );
    let fire = InventoryFire {
        job_id: job.id,
        project_id: project,
        scheduled_for,
        now,
        bundled: bundled_checker().map(|b| b.id.clone()),
        timeout: HEALTH_INPUT_TIMEOUT,
        roles: ctx.roles.to_vec(),
        genres: ctx.genres.to_vec(),
    };
    let store = Arc::clone(store);
    runs.start(job.id, move || fire.run(&store, &repos))?;
    Ok(CronInputPost::Posted(detail))
}

/// 1 回の発火の棚卸し（tick の外で走る）。
#[derive(Debug, Clone)]
pub(crate) struct InventoryFire {
    pub job_id: CronJobId,
    /// 起票の task の案件（job の `extra.project_id`）。
    pub project_id: ProjectId,
    pub scheduled_for: OffsetDateTime,
    /// 行の `created_at`（発火の時刻。注入した時計の値）。
    pub now: OffsetDateTime,
    /// release に同梱した検査器（`repo_facts` に渡す）。
    pub bundled: Option<CheckerId>,
    /// 1 repo の git の読み取りと検査器の実行の上限（review の health 文脈と同じ）。
    pub timeout: Duration,
    /// 起票の組み立て（`POST /tasks` と同じ既定）に使う `[[roles]]`・`[[genres]]` の写し（tick の外へ持ち出す）。
    pub roles: Vec<RoleSpec>,
    pub genres: Vec<GenreSpec>,
}

/// 1 repo の棚卸しの結果（行に書く前の形）。`details` は同じ検査器の実行の blocking の説明（起票の文面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoInventory {
    pub outcome: HealthInventoryOutcome,
    pub evidence: Option<HealthEvidence>,
    pub details: Vec<HealthBlockingDetail>,
}

/// 既定の branch の HEAD（commit と tree）。
struct Head {
    sha: String,
    tree: String,
}

impl InventoryFire {
    /// repo の順に評価・起票し、repo ごとに 1 行を追記する。起票の枠は repo をまたいで数える。1 repo の失敗は
    /// log に残して次の repo へ進む。起票に失敗した repo は行を書かない（次の発火が同じ base から評価し直し、
    /// 作れた debt は冪等キーで `existing_skipped` になる）。
    pub(crate) fn run(&self, store: &SqliteStore, repos: &[ProjectRepo]) {
        let mut remaining = MAX_DEBTS_PER_FIRE;
        for repo in repos {
            let done = self.inventory_repo(store, repo).and_then(|inventory| {
                let filed = self.file(store, &inventory, remaining)?;
                remaining = remaining.saturating_sub(filed.task_ids.len());
                self.record(store, repo.id, inventory, &filed)
            });
            if let Err(error) = done {
                tracing::warn!(job_id = %self.job_id, repo = %repo.name, error = %error, "health inventory of a repo failed");
            }
        }
    }

    /// 1 repo の評価から debt の draft を起票する（D6.6・D6.7）。証跡と説明は `inventory_repo` の同じ検査器の
    /// 実行のもの。評価しなかった repo（`unchanged`・`not_applicable`）は何もしない。
    pub(crate) fn file(
        &self,
        store: &SqliteStore,
        inventory: &RepoInventory,
        remaining: usize,
    ) -> Result<FiledHealthDebts, String> {
        let Some(evidence) = &inventory.evidence else {
            return Ok(FiledHealthDebts::default());
        };
        let input = HealthDebtInput {
            project_id: self.project_id,
            evidence,
            details: &inventory.details,
            remaining,
        };
        let filed = file_health_debts(store, &input, &self.roles, &self.genres, self.now)
            .map_err(|e| format!("health debt filing: {e}"))?;
        if filed.missing_detail > 0 {
            tracing::warn!(job_id = %self.job_id, repo_id = %evidence.repo_id, missing_detail = filed.missing_detail, "health inventory: blocking findings without a path were not filed");
        }
        Ok(filed)
    }

    /// 1 repo の差分検査（D6.3・D6.4）。
    pub(crate) fn inventory_repo(
        &self,
        store: &SqliteStore,
        repo: &ProjectRepo,
    ) -> Result<RepoInventory, String> {
        let deadline = Instant::now() + self.timeout;
        let (path, integrated) = match &repo.location {
            WorkspaceSpec::Local { path, .. } => (path.clone(), true),
            WorkspaceSpec::Remote { .. } => (PathBuf::new(), false),
        };
        let previous = store
            .health_inventory_latest_evaluated(self.job_id, repo.id)
            .map_err(|e| e.to_string())?
            .and_then(|row| row.evidence);
        let head = integrated.then(|| read_head(&path, repo.default_branch.as_deref(), deadline));
        let base = match (&previous, &head) {
            (Some(previous), _) => previous.head_sha.clone(),
            (None, Some(Ok(head))) => head.sha.clone(),
            _ => String::new(),
        };
        if let Some(Err(why)) = &head {
            return Ok(error_inventory(repo.id, &base, None, why.clone()));
        }
        let facts = repo_facts(integrated, &base, self.bundled.as_ref(), |args| {
            git_stdout(&path, args, deadline)
        });
        let checker = match applicability(&facts) {
            HealthApplicability::Applies { checker } => checker,
            HealthApplicability::NotApplicable { reason } => {
                tracing::info!(job_id = %self.job_id, repo = %repo.name, reason = reason.as_str(), "health inventory: not applicable");
                return Ok(RepoInventory {
                    outcome: HealthInventoryOutcome::NotApplicable,
                    evidence: None,
                    details: Vec::new(),
                });
            }
            unavailable @ HealthApplicability::Unavailable { .. } => {
                let why = unavailable.reason().unwrap_or_default();
                let head = head.and_then(Result::ok);
                return Ok(error_inventory(repo.id, &base, head.as_ref(), why));
            }
        };
        let Some(Ok(head)) = head else {
            return Err("applicability applied to a repo without a readable head".into());
        };
        let unchanged = previous.as_ref().is_some_and(|p| {
            p.head_tree == head.tree
                && p.checker.as_ref().map(|c| &c.sha256) == Some(&checker.sha256)
        });
        if unchanged {
            return Ok(RepoInventory {
                outcome: HealthInventoryOutcome::Unchanged,
                evidence: None,
                details: Vec::new(),
            });
        }
        let (evidence, details) = run_health_check_with_details(
            repo.id,
            &path,
            &base,
            &head.sha,
            &checker,
            bundled_checker(),
            self.timeout,
        );
        Ok(RepoInventory {
            outcome: HealthInventoryOutcome::Evaluated,
            evidence: Some(evidence),
            details,
        })
    }

    /// 1 repo の行を 1 回だけ追記する（D6.5。起票の結果も同じ行に入れ、後から更新しない）。
    pub(crate) fn record(
        &self,
        store: &SqliteStore,
        repo_id: RepoId,
        inventory: RepoInventory,
        filed: &FiledHealthDebts,
    ) -> Result<(), String> {
        store
            .health_inventory_append(&HealthInventoryRun {
                id: Ulid::new(),
                job_id: self.job_id,
                scheduled_for: self.scheduled_for,
                repo_id,
                outcome: inventory.outcome,
                evidence: inventory.evidence,
                task_ids: filed.task_ids.clone(),
                existing_skipped: filed.existing_skipped,
                limit_remaining: filed.limit_remaining,
                created_at: self.now,
            })
            .map_err(|e| e.to_string())
    }
}

/// 既定の branch（`task_ops::changes::resolve_default_branch`）の HEAD の commit と tree を読む（ref は変えない）。
fn read_head(path: &Path, configured: Option<&str>, deadline: Instant) -> Result<Head, String> {
    let branch = task_ops::changes::resolve_default_branch(path, configured)
        .map_err(|e| format!("head_unreadable: {e}"))?;
    let git = |args: &[&str]| git_stdout(path, args, deadline);
    let commit = format!("refs/heads/{branch}^{{commit}}");
    let sha = git(&["rev-parse", "--verify", &commit])
        .map_err(|e| format!("head_unreadable: {commit}: {e}"))?
        .trim()
        .to_string();
    let tree = format!("{sha}^{{tree}}");
    let tree = git(&["rev-parse", "--verify", &tree])
        .map_err(|e| format!("head_unreadable: {tree}: {e}"))?
        .trim()
        .to_string();
    Ok(Head { sha, tree })
}

/// 評価できなかった repo の行（verdict Error。次の base には使われない）。
fn error_inventory(
    repo_id: RepoId,
    base: &str,
    head: Option<&Head>,
    reason: String,
) -> RepoInventory {
    RepoInventory {
        outcome: HealthInventoryOutcome::Evaluated,
        evidence: Some(HealthEvidence {
            repo_id,
            base_sha: base.to_string(),
            head_sha: head.map(|h| h.sha.clone()).unwrap_or_default(),
            head_tree: head.map(|h| h.tree.clone()).unwrap_or_default(),
            policy_sha256: String::new(),
            checker: None,
            report_sha256: String::new(),
            tool_version: String::new(),
            verdict: HealthVerdict::Error,
            blocking: Vec::new(),
            reason: Some(reason),
            reused_from: None,
            reviewable: false,
        }),
        details: Vec::new(),
    }
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "inventory_filing_tests.rs"]
mod filing_tests;
