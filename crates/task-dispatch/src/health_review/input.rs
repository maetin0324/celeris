//! ADR 2026-11-07-health-review-integration D4・I5: final review の `ReviewRequest.health`（base→candidate の diff
//! 統計・変更 path の owner 地図の行・repo health の new findings の要約・policy の状態・不変条件・採用した計画の
//! 設計 brief）を candidate SHA ごとに 1 回だけ作る。
//!
//! 流れ:
//! - [`Dispatcher::health_job`]（tick の中。git も python も起こさない）: `spawn_review` が review 前の同期
//!   （ADR-0118）で `ReviewSync` を作り、reviewer run の有無を決めた後に呼ぶ。同期した repo が候補になり、
//!   remote workspace の repo と同期の無い repo は「作らない理由」になる。`reviewer_reuse` で reviewer を省いた
//!   candidate（`ReviewerPassReused` がある）は作らない（policy の関門は元の candidate の記録を見る）。
//! - reviewer run の無い review（`Check::Reviewer` の無い task。I8）は `policy_only`: reviewer には何も渡さず、
//!   candidate の policy の状態だけを適用判定が選んだ検査器の report で決めて `HealthReviewInputSkipped` の `policy` に
//!   残す（同じ candidate の既知の状態があれば repo health を走らせない）。health を当てない repo（適用判定が
//!   `NotApplicable`）は何も残さない。
//! - [`HealthInputJob::attach`]（review の非同期 task の中）: [`build_health_input`] を `spawn_blocking` で走らせ、
//!   結果を `ReviewerRun.health`（reviewer の `ReviewRequest.health`）に入れる。reviewer run が無くても走らせる
//!   （`policy_only`。review はその分だけ遅れる）。子 process は全体で [`HEALTH_INPUT_TIMEOUT`] の締め切りを共有する。
//!   tick は止めない。
//! - [`build_health_input`]: repo ごとに経路共通の検査器の選び方（`health_review::gate::with_applicable_checker`。ADR
//!   2026-11-08-health-enforcement-loop D1.1・D1.3: target の版の検査器、無ければ同梱の検査器）で review 方針を読み、
//!   同じ `(repo_id, target_sha, reviewed_sha, definition_sha256)` の `Event::HealthReviewInputBuilt` があればその
//!   `input` を使う（reviewer run の再試行・stale 再同期の後の再 review）。無ければ diff 統計・owner 地図の行・
//!   repo health の report から作って追記する。report は経路共通の評価（`health_review::gate::check_with`。entry の
//!   入口 `run_health_check` と同じ評価）が読んだものを使い、同じ candidate で検査器を 2 回起こさない。
//!   方針が `absent`・読めない repo、health を当てない repo、remote workspace・同期の無い repo は
//!   `Event::HealthReviewInputSkipped` に理由を残す（その repo の分は `ReviewRequest.health` に入らない）。
//! - policy の関門（I8・D6.4。`dispatcher::policy_review`）のため、health を当てる repo は candidate の policy の
//!   状態を必ず event に残す: 材料を作った repo は `HealthReviewInputBuilt` の `input.policy`、作らなかった repo は
//!   `HealthReviewInputSkipped` の `policy`。方針が `absent` の repo も report を走らせて決める（reviewer には渡さない）。
//!   適用判定が `Unavailable`（base にも同梱にも検査器が無い・事実を読めない）・report を作れない・方針を読めない・
//!   diff を取れないときは `source =` [`POLICY_UNKNOWN`]（不明）にする（当てないのではなく合格にしない。J4）。不明の
//!   材料は再利用しない（次の review で作り直す。D4.3）。
//!
//! WU の check・WU の review（`work_units`・`phase_integration` の検査）はここを通らない（D4.4）。LLM は呼ばない。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use task_core::design_review::ReviewPolicyState;
use task_core::{Event, Task, TaskId, TaskStore};
use task_ops::changes::git;
use task_worker::protocol::{
    DiffStat, HealthFindingsSummary, HealthPolicyState, HealthRepoInput, HealthReportFinding,
    HealthReviewInput, OwnerRow, PathStat,
};

use super::gate::{self, BundledChecker, CheckerChoice, RepoHealthReport};
use super::policy::{BaseChecker, finished, left, parse_review_policy};
use crate::dispatcher::Dispatcher;
use crate::dispatcher::review_spawn::ReviewSync;
use crate::review::ReviewerRun;
use task_core::RepoId;
use task_core::required_checks::health::CheckerId;

/// 1 回の材料作り（全 repo の方針の読み込み・diff・owner 地図・repo health）の時間の上限。
pub const HEALTH_INPUT_TIMEOUT: Duration = Duration::from_secs(600);
/// `findings.new` の上限（超えた数は `findings.truncated`）。
pub const MAX_FINDINGS: usize = 50;
/// `diff.paths` の上限（`diff.files` は総数のまま）。
pub const MAX_PATHS: usize = 40;
/// `owner_rows` の上限。
pub const MAX_OWNER_ROWS: usize = 40;

/// 同期の無い repo の理由（同期を省いた・失敗した repo は target と候補の SHA が決まらない）。
pub const NOT_SYNCED: &str = "review 前の同期が無い（target と候補の SHA が決まらない）";
/// health を当てない repo（適用判定が `NotApplicable`: Git 管理でない作業場所）の理由の接頭辞（policy の関門も無い）。
pub const NOT_APPLICABLE: &str = "repo health を当てない";
/// 適用判定が `Unavailable`（base にも同梱にも検査器が無い・事実を読めない）の repo の理由の接頭辞。policy の状態は
/// 不明（[`POLICY_UNKNOWN`]）にして関門に掛ける（合格にしない）。
pub const CHECKER_UNAVAILABLE: &str = "検査器を当てられない";
/// 方針が absent の repo の理由（policy の状態は report で決めて関門に掛ける）。
pub const POLICY_ABSENT: &str = "review 方針が absent（base の profile に [review] が無い）";
/// reviewer run の無い review の理由（policy の状態は report で決めて関門に掛ける）。
pub const NO_REVIEWER: &str = "reviewer run の無い review（policy の状態だけを決める）";
/// policy の状態を決められなかった（report を作れない・方針を読めない・diff を取れない）ことを表す
/// `HealthPolicyState.source`。policy の関門は `policy.changed` と同じに扱う（I8。fail closed）。
pub const POLICY_UNKNOWN: &str = "unavailable";

/// 1 repo の候補（review 前の同期の結果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRepo {
    pub repo_id: String,
    /// 候補を checkout した worktree（git の object は source の repo と共有）。
    pub dir: PathBuf,
    pub target_sha: String,
    pub reviewed_sha: String,
}

/// [`build_health_input`] の入力（tick の中で集めた値だけ）。
pub struct HealthInputWork {
    pub store: Arc<dyn TaskStore>,
    pub task_id: TaskId,
    /// 材料を渡す reviewer run の id（event の `review_run`。`policy_only` なら worker run の id）。
    pub review_run: String,
    /// reviewer run の無い review: 材料は作らず、candidate の policy の状態だけを決めて記録する（I8）。
    pub policy_only: bool,
    pub candidates: Vec<CandidateRepo>,
    /// 作らない repo と理由（remote workspace・同期の無い repo）。
    pub skipped: Vec<(String, String)>,
    /// report の書き先の dir（daemon の task dir の `runs/health/`）。
    pub out_dir: PathBuf,
    pub timeout: Duration,
}

/// `spawn_review` が reviewer run を起こす非同期 task に持ち込む仕事（`None` なら何もしない）。
pub struct HealthInputJob(pub Option<HealthInputWork>);

impl HealthInputJob {
    /// 仕事があれば材料を作り（`policy_only` なら policy の状態だけを記録し）、reviewer run があれば
    /// `run.health` に入れて返す。
    pub(crate) async fn attach(self, run: Option<ReviewerRun>) -> Option<ReviewerRun> {
        let Some(work) = self.0 else {
            return run;
        };
        let task_id = work.task_id;
        let built = tokio::task::spawn_blocking(move || build_health_input(&work)).await;
        let mut run = run?;
        match built {
            Ok(health) => run.health = health,
            Err(e) => {
                tracing::warn!(%task_id, error = %e, "health input builder failed; reviewing without health");
            }
        }
        Some(run)
    }
}

impl Dispatcher {
    /// D4.2・D4.4: `spawn_review` の review 前の同期の後、reviewer run の有無を決めた後に 1 回呼ぶ（`run_id` は
    /// worker run の id）。重い処理はしない（[`HealthInputJob::attach`] が review の非同期 task の中でする）。
    pub(crate) fn health_job(
        &self,
        task: &Task,
        synced: &[ReviewSync],
        run_id: &str,
        reviewer: Option<&ReviewerRun>,
    ) -> HealthInputJob {
        let Some(task_dir) = self.task_dir(task) else {
            return HealthInputJob(None);
        };
        let work = |review_run: String, candidates, skipped, policy_only| HealthInputWork {
            store: self.store(),
            task_id: task.id,
            review_run,
            policy_only,
            candidates,
            skipped,
            out_dir: task_dir.join("runs").join("health"),
            timeout: HEALTH_INPUT_TIMEOUT,
        };
        let Some(run) = reviewer else {
            return self.policy_only_job(task, synced, run_id, work);
        };
        let remote = matches!(task.workspace, task_core::WorkspaceSpec::Remote { .. });
        let mut candidates = Vec::new();
        let mut skipped = Vec::new();
        for reference in &task.repos {
            let repo_id = reference.repo_id.to_string();
            match synced.iter().find(|s| s.repo_id == reference.repo_id) {
                Some(s) => candidates.push(CandidateRepo {
                    repo_id,
                    dir: s.dir.clone(),
                    target_sha: s.target_sha.clone(),
                    reviewed_sha: s.reviewed_sha.clone(),
                }),
                None if remote => skipped.push((repo_id, "remote workspace".to_string())),
                None => skipped.push((repo_id, NOT_SYNCED.to_string())),
            }
        }
        if candidates.is_empty() && skipped.is_empty() {
            return HealthInputJob(None);
        }
        HealthInputJob(Some(work(run.run_id.clone(), candidates, skipped, false)))
    }

    /// reviewer run の無い review（I8）: 同期した candidate のうち、reviewer の合格の再利用（`ReviewerPassReused`）
    /// でないものの policy の状態を決める仕事。event を読めなければ全 candidate を対象にする（fail closed）。
    fn policy_only_job(
        &self,
        task: &Task,
        synced: &[ReviewSync],
        run_id: &str,
        work: impl FnOnce(String, Vec<CandidateRepo>, Vec<(String, String)>, bool) -> HealthInputWork,
    ) -> HealthInputJob {
        let events = self.store().events_for(task.id).unwrap_or_else(|e| {
            tracing::warn!(task_id = %task.id, error = %e, "cannot read events for the policy-only health job");
            Vec::new()
        });
        let candidates: Vec<CandidateRepo> = synced
            .iter()
            .filter(|s| reused_from(&events, s.repo_id, &s.target_sha, &s.reviewed_sha).is_none())
            .map(|s| CandidateRepo {
                repo_id: s.repo_id.to_string(),
                dir: s.dir.clone(),
                target_sha: s.target_sha.clone(),
                reviewed_sha: s.reviewed_sha.clone(),
            })
            .collect();
        if candidates.is_empty() {
            return HealthInputJob(None);
        }
        HealthInputJob(Some(work(run_id.to_string(), candidates, Vec::new(), true)))
    }
}

/// D4: 材料を作る本体（blocking）。作れた repo が 1 つも無ければ `None`（`ReviewRequest.health` を持たない）。
/// `policy_only` なら policy の状態を記録するだけで常に `None`。同梱の検査器は daemon が起動で設定したもの
/// （`gate::bundled_checker()`）を渡す。
pub fn build_health_input(work: &HealthInputWork) -> Option<HealthReviewInput> {
    build_health_input_with(work, gate::bundled_checker())
}

/// [`build_health_input`] の本体。`bundled` は base に検査器の無い repo に当てる同梱の検査器。
pub(crate) fn build_health_input_with(
    work: &HealthInputWork,
    bundled: Option<&BundledChecker>,
) -> Option<HealthReviewInput> {
    let deadline = Instant::now() + work.timeout;
    for (repo_id, reason) in &work.skipped {
        work.skip(repo_id, reason, None);
    }
    let recorded = match work.store.events_for(work.task_id) {
        Ok(events) => events,
        Err(e) => {
            tracing::warn!(task_id = %work.task_id, error = %e, "cannot read events for health input reuse");
            Vec::new()
        }
    };
    if work.policy_only {
        for candidate in &work.candidates {
            work.record_policy_only(candidate, &recorded, bundled, deadline);
        }
        return None;
    }
    let mut repos = Vec::new();
    for candidate in &work.candidates {
        match work.repo_input(candidate, &recorded, bundled, deadline) {
            Ok(input) => repos.push(input),
            Err((reason, policy)) => {
                let policy = policy.map(|p| (candidate, p));
                work.skip(&candidate.repo_id, &reason, policy);
            }
        }
    }
    if repos.is_empty() {
        return None;
    }
    let brief = match work.store.execution_plan_active(work.task_id) {
        Ok(plan) => plan.and_then(|p| p.spec.design_brief).map(Box::new),
        Err(e) => {
            tracing::warn!(task_id = %work.task_id, error = %e, "cannot read the adopted plan for the design brief");
            None
        }
    };
    Some(HealthReviewInput { repos, brief })
}

/// reviewer に材料を渡さない repo の理由と、関門に使う candidate の policy の状態（検査器の無い repo は `None`）。
type Skip = (String, Option<HealthPolicyState>);

impl HealthInputWork {
    fn skip(
        &self,
        repo_id: &str,
        reason: &str,
        policy: Option<(&CandidateRepo, HealthPolicyState)>,
    ) {
        let (target_sha, reviewed_sha, policy) = match policy {
            Some((c, p)) => (
                c.target_sha.clone(),
                c.reviewed_sha.clone(),
                serde_json::to_value(p).ok(),
            ),
            None => Default::default(),
        };
        let event = Event::HealthReviewInputSkipped {
            review_run: self.review_run.clone(),
            repo_id: repo_id.to_string(),
            reason: reason.to_string(),
            target_sha,
            reviewed_sha,
            policy,
        };
        if let Err(e) = self.store.append_event(self.task_id, &event) {
            tracing::warn!(task_id = %self.task_id, error = %e, "cannot record the skipped health input");
        }
    }

    /// 1 repo の材料。方針が `Present` でなければ理由と policy の状態（skip）。適用判定が `Unavailable`・検査器を
    /// 出せない・方針を読めない・diff を取れないときの policy の状態は不明（[`POLICY_UNKNOWN`]）。
    fn repo_input(
        &self,
        candidate: &CandidateRepo,
        recorded: &[(u64, Event)],
        bundled: Option<&BundledChecker>,
        deadline: Instant,
    ) -> Result<HealthRepoInput, Skip> {
        let c = candidate;
        let chosen = gate::with_applicable_checker(
            &c.dir,
            &c.target_sha,
            true,
            bundled,
            deadline,
            |checker, id| {
                let policy = match parse_review_policy(&checker.print_review_policy(&c.dir)?) {
                    ReviewPolicyState::Present(policy) => policy,
                    ReviewPolicyState::Absent => {
                        return Ok(Err(self.absent_policy(checker, id, c, recorded, deadline)));
                    }
                    ReviewPolicyState::Unavailable(why) => {
                        return Err(format!("review 方針を読めない: {why}"));
                    }
                };
                let definition = policy.definition_sha256.clone();
                if let Some(input) = find_recorded(recorded, c, &definition) {
                    return Ok(Ok(input));
                }
                let (diff, changed) = diff_stat(c, deadline)?;
                let owner_rows = policy
                    .owner_map
                    .as_deref()
                    .and_then(|map| show_at(&c.dir, &c.target_sha, map, deadline))
                    .map(|text| owner_rows(&text, &changed))
                    .unwrap_or_default();
                let (findings, policy_state) = match self.run_report(checker, id, c, deadline) {
                    Ok(report) => summarize_report(report),
                    Err(why) => unavailable_report(why),
                };
                let input = HealthRepoInput {
                    repo_id: c.repo_id.clone(),
                    target_sha: c.target_sha.clone(),
                    reviewed_sha: c.reviewed_sha.clone(),
                    diff,
                    owner_rows,
                    findings,
                    policy: policy_state,
                    invariants: policy.invariants,
                };
                // 走らせられなかった report（時間切れ・壊れた report）も policy の関門のために記録する（不明）。
                // 再利用はしない（`find_recorded`）ので次の review でやり直す。
                self.record(c, definition, &input);
                Ok(Ok(input))
            },
        );
        match chosen {
            CheckerChoice::NotApplicable(reason) => {
                Err((format!("{NOT_APPLICABLE}: {}", reason.as_str()), None))
            }
            CheckerChoice::Unavailable(reason) => Err((
                format!("{CHECKER_UNAVAILABLE}: {reason}"),
                Some(unknown_policy()),
            )),
            CheckerChoice::Ran(Ok(result)) => result,
            CheckerChoice::Ran(Err(why)) => Err((why, Some(unknown_policy()))),
        }
    }

    /// reviewer run の無い review の 1 candidate: 既知の policy の状態が記録済みなら何もしない（repo health は
    /// candidate ごとに 1 回）。無ければ適用判定が選んだ検査器の report で決めて記録する（`Unavailable`・作れなければ
    /// 不明）。health を当てない repo は関門が無いので記録しない。方針（`--print-review-policy`）は reviewer の材料に
    /// しか使わないので読まない。
    fn record_policy_only(
        &self,
        c: &CandidateRepo,
        recorded: &[(u64, Event)],
        bundled: Option<&BundledChecker>,
        deadline: Instant,
    ) {
        if find_recorded_policy(recorded, c).is_some() {
            return;
        }
        let chosen = gate::with_applicable_checker(
            &c.dir,
            &c.target_sha,
            true,
            bundled,
            deadline,
            |checker, id| Ok(self.run_report(checker, id, c, deadline)),
        );
        let unknown = |why: &dyn std::fmt::Display| {
            (
                format!("{NO_REVIEWER}。policy の状態を決められない: {why}"),
                unknown_policy(),
            )
        };
        let (reason, policy) = match chosen {
            CheckerChoice::NotApplicable(_) => return,
            CheckerChoice::Unavailable(reason) => {
                unknown(&format!("{CHECKER_UNAVAILABLE}: {reason}"))
            }
            CheckerChoice::Ran(Ok(Ok(report))) => {
                (NO_REVIEWER.to_string(), summarize_report(report).1)
            }
            CheckerChoice::Ran(Ok(Err(why)) | Err(why)) => unknown(&why),
        };
        self.skip(&c.repo_id, &reason, Some((c, policy)));
    }

    /// 方針が absent の repo: reviewer には渡さず、policy の状態だけを report で決める（同じ candidate の既知の
    /// 状態があれば使う）。
    fn absent_policy(
        &self,
        checker: &BaseChecker,
        id: &CheckerId,
        c: &CandidateRepo,
        recorded: &[(u64, Event)],
        deadline: Instant,
    ) -> Skip {
        if let Some(known) = find_recorded_policy(recorded, c) {
            return (POLICY_ABSENT.into(), Some(known));
        }
        match self.run_report(checker, id, c, deadline) {
            Ok(report) => (POLICY_ABSENT.into(), Some(summarize_report(report).1)),
            Err(why) => (
                format!("{POLICY_ABSENT}。policy の状態を決められない: {why}"),
                Some(unknown_policy()),
            ),
        }
    }

    fn record(&self, c: &CandidateRepo, definition_sha256: String, input: &HealthRepoInput) {
        let input = match serde_json::to_value(input) {
            Ok(input) => input,
            Err(e) => {
                tracing::warn!(task_id = %self.task_id, error = %e, "cannot encode the health input");
                return;
            }
        };
        let event = Event::HealthReviewInputBuilt {
            review_run: self.review_run.clone(),
            repo_id: c.repo_id.clone(),
            target_sha: c.target_sha.clone(),
            reviewed_sha: c.reviewed_sha.clone(),
            definition_sha256,
            input,
        };
        if let Err(e) = self.store.append_event(self.task_id, &event) {
            tracing::warn!(task_id = %self.task_id, error = %e, "cannot record the health input");
        }
    }

    /// 適用判定が選んだ検査器（`checker`・`id`）で candidate を 1 回評価し、その report を読む（評価は経路共通の
    /// [`gate::check_with`]。entry の入口と同じ評価で、検査器の起動は 1 回）。report は daemon の task dir の
    /// `runs/health/<repo>-<target>-<reviewed>.json` に書く。
    fn run_report(
        &self,
        checker: &BaseChecker,
        id: &CheckerId,
        c: &CandidateRepo,
        deadline: Instant,
    ) -> Result<RepoHealthReport, String> {
        let repo_id: RepoId = c
            .repo_id
            .parse()
            .map_err(|e| format!("repo の id {:?} を読めない: {e}", c.repo_id))?;
        let name = format!(
            "{}-{}-{}.json",
            c.repo_id,
            short(&c.target_sha),
            short(&c.reviewed_sha)
        );
        let out = self.out_dir.join(name);
        gate::check_with(
            checker,
            id,
            repo_id,
            &c.dir,
            &c.reviewed_sha,
            Some(&out),
            deadline,
        )
        .report
    }
}

/// reviewer の合格を再利用した candidate（`ReviewerPassReused`）なら、その合格の元の candidate の
/// `(target_sha, reviewed_sha)`。policy の関門と `policy_only` の仕事が共有する（再利用の回は元の candidate を見る）。
pub(crate) fn reused_from(
    events: &[(u64, Event)],
    repo: task_core::RepoId,
    target: &str,
    reviewed: &str,
) -> Option<(String, String)> {
    events.iter().rev().find_map(|(_, e)| match e {
        Event::ReviewerPassReused {
            repo_id,
            target_sha,
            reviewed_sha,
            from_target_sha,
            from_reviewed_sha,
            ..
        } if *repo_id == repo && target_sha == target && reviewed_sha == reviewed => {
            Some((from_target_sha.clone(), from_reviewed_sha.clone()))
        }
        _ => None,
    })
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// 同じ key の `HealthReviewInputBuilt` の `input`（最後のもの。policy の状態が不明の材料は使わない）。
fn find_recorded(
    recorded: &[(u64, Event)],
    c: &CandidateRepo,
    definition: &str,
) -> Option<HealthRepoInput> {
    recorded.iter().rev().find_map(|(_, event)| match event {
        Event::HealthReviewInputBuilt {
            repo_id,
            target_sha,
            reviewed_sha,
            definition_sha256,
            input,
            ..
        } if *repo_id == c.repo_id
            && *target_sha == c.target_sha
            && *reviewed_sha == c.reviewed_sha
            && definition_sha256 == definition =>
        {
            serde_json::from_value::<HealthRepoInput>(input.clone())
                .ok()
                .filter(|i| i.policy.source != POLICY_UNKNOWN)
        }
        _ => None,
    })
}

/// 同じ candidate の `HealthReviewInputSkipped` の `policy`・`HealthReviewInputBuilt` の `input.policy` に残した
/// 既知（不明でない）の policy の状態（最後のもの）。
fn find_recorded_policy(recorded: &[(u64, Event)], c: &CandidateRepo) -> Option<HealthPolicyState> {
    let same = |repo: &str, target: &str, reviewed: &str| {
        repo == c.repo_id && target == c.target_sha && reviewed == c.reviewed_sha
    };
    recorded.iter().rev().find_map(|(_, event)| match event {
        Event::HealthReviewInputSkipped {
            repo_id,
            target_sha,
            reviewed_sha,
            policy: Some(policy),
            ..
        } if same(repo_id, target_sha, reviewed_sha) => {
            serde_json::from_value::<HealthPolicyState>(policy.clone())
                .ok()
                .filter(|p| p.source != POLICY_UNKNOWN)
        }
        Event::HealthReviewInputBuilt {
            repo_id,
            target_sha,
            reviewed_sha,
            input,
            ..
        } if same(repo_id, target_sha, reviewed_sha) => {
            serde_json::from_value::<HealthRepoInput>(input.clone())
                .ok()
                .map(|i| i.policy)
                .filter(|p| p.source != POLICY_UNKNOWN)
        }
        _ => None,
    })
}

/// `git diff --numstat -z --no-renames <target>..<reviewed>` の統計と、変更した全 path。
fn diff_stat(c: &CandidateRepo, deadline: Instant) -> Result<(DiffStat, Vec<String>), String> {
    let range = format!("{}..{}", c.target_sha, c.reviewed_sha);
    let out = finished(
        git(
            &c.dir,
            &["diff", "--numstat", "-z", "--no-renames", &range],
            left(deadline)?,
        ),
        "git diff --numstat",
    )?;
    if !out.ok {
        return Err(format!("git diff --numstat に失敗: {}", out.why()));
    }
    Ok(read_numstat(&out.stdout))
}

/// `--numstat -z --no-renames` の出力（`<ins>\t<del>\t<path>\0` の並び。binary は `-`）を読む。`paths` は
/// [`MAX_PATHS`] 件まで、`files`・行数は全件。
fn read_numstat(stdout: &str) -> (DiffStat, Vec<String>) {
    let mut stat = DiffStat::default();
    let mut all = Vec::new();
    for record in stdout.split('\0') {
        let mut parts = record.trim_start_matches('\n').splitn(3, '\t');
        let (Some(ins), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let (insertions, deletions) = (ins.parse().unwrap_or(0), del.parse().unwrap_or(0));
        stat.files += 1;
        stat.insertions += insertions;
        stat.deletions += deletions;
        if stat.paths.len() < MAX_PATHS {
            stat.paths.push(PathStat {
                path: path.to_string(),
                insertions,
                deletions,
            });
        }
        all.push(path.to_string());
    }
    (stat, all)
}

/// `git show <rev>:<path>`（無い・読めなければ `None`）。
fn show_at(dir: &Path, rev: &str, path: &str, deadline: Instant) -> Option<String> {
    let spec = format!("{rev}:{path}");
    let out = finished(git(dir, &["show", &spec], left(deadline).ok()?), "git show").ok()?;
    out.ok.then_some(out.stdout)
}

/// owner 地図（Markdown）の表の行のうち、変更 path を含む行（[`MAX_OWNER_ROWS`] 件まで）。行の `…` の中の path
/// （`{a,b}` は展開する）が変更 path そのもの、その親 dir、`foo.rs` の module dir `foo/` なら含むとみなす。`path` は
/// 変更 path のうち最初に当たったもの。
pub fn owner_rows(map: &str, changed: &[String]) -> Vec<OwnerRow> {
    let mut rows = Vec::new();
    for line in map.lines() {
        let row = line.trim();
        if !row.starts_with('|') || row.chars().all(|ch| matches!(ch, '|' | '-' | ':' | ' ')) {
            continue;
        }
        let tokens = path_tokens(row);
        if let Some(path) = changed.iter().find(|c| tokens.iter().any(|t| covers(t, c))) {
            rows.push(OwnerRow {
                path: path.clone(),
                row: row.to_string(),
            });
            if rows.len() == MAX_OWNER_ROWS {
                break;
            }
        }
    }
    rows
}

/// 行の `…` の中の path（`/` を含み空白を含まないもの。`{a,b}` は展開する）。
fn path_tokens(row: &str) -> Vec<String> {
    row.split('`')
        .skip(1)
        .step_by(2)
        .flat_map(expand_braces)
        .filter(|t| t.contains('/') && !t.contains(char::is_whitespace))
        .collect()
}

fn expand_braces(token: &str) -> Vec<String> {
    let (Some(open), Some(close)) = (token.find('{'), token.find('}')) else {
        return vec![token.to_string()];
    };
    if close < open {
        return vec![token.to_string()];
    }
    let (head, alternatives, tail) = (&token[..open], &token[open + 1..close], &token[close + 1..]);
    alternatives
        .split(',')
        .flat_map(|alt| expand_braces(&format!("{head}{}{tail}", alt.trim())))
        .collect()
}

fn covers(token: &str, changed: &str) -> bool {
    let token = token.trim_end_matches('/');
    let under = |dir: &str| {
        changed
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
    };
    !token.is_empty()
        && (changed == token || under(token) || token.strip_suffix(".rs").is_some_and(under))
}

/// report を要約する: `delta ∈ {new, worsened}` かつ `exception == null` の finding を [`MAX_FINDINGS`] 件まで
/// （超えた数は `truncated`）。`summary.reviewable == false` なら finding は材料にせず理由を残す。policy の
/// `files` は base と head で blob の違う file だけ。
pub fn summarize_report(report: RepoHealthReport) -> (HealthFindingsSummary, HealthPolicyState) {
    let RepoHealthReport {
        summary,
        findings,
        policy,
        ..
    } = report;
    let mut new = Vec::new();
    let mut truncated = 0;
    let mut reason = None;
    if summary.reviewable {
        let wanted = findings.into_iter().filter(|f| {
            matches!(f.delta.as_str(), "new" | "worsened")
                && f.exception.as_ref().is_none_or(serde_json::Value::is_null)
        });
        for finding in wanted {
            if new.len() == MAX_FINDINGS {
                truncated += 1;
                continue;
            }
            new.push(HealthReportFinding {
                path: finding.evidence.into_iter().find_map(|e| e.path),
                rule_id: finding.rule_id,
                fingerprint: finding.fingerprint,
                severity: finding.severity,
                delta: finding.delta,
            });
        }
    } else {
        reason = Some(
            "report の summary.reviewable が false（head が commit でない・dirty・base に届かない）。findings は材料にしない"
                .to_string(),
        );
    }
    let files = policy
        .blobs
        .into_iter()
        .filter(|f| f.base_blob != f.head_blob)
        .map(|f| f.path)
        .collect();
    (
        HealthFindingsSummary {
            verdict: summary.verdict,
            reviewable: summary.reviewable,
            counts: summary.counts,
            new,
            truncated,
            reason,
        },
        HealthPolicyState {
            source: policy.source,
            changed: policy.changed,
            files,
        },
    )
}

/// repo health を走らせられなかった（時間切れ・report が無い・壊れた）ときの要約。
fn unavailable_report(why: String) -> (HealthFindingsSummary, HealthPolicyState) {
    (
        HealthFindingsSummary {
            verdict: "error".into(),
            reviewable: false,
            reason: Some(why),
            ..Default::default()
        },
        unknown_policy(),
    )
}

/// 決められなかった policy の状態（関門は変更と同じに扱う）。
fn unknown_policy() -> HealthPolicyState {
    HealthPolicyState {
        source: POLICY_UNKNOWN.into(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
