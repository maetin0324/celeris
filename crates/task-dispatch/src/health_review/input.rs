//! ADR 2026-11-07-health-review-integration D4・I5: final review の `ReviewRequest.health`（base→candidate の diff
//! 統計・変更 path の owner 地図の行・repo health の new findings の要約・policy の状態・不変条件・採用した計画の
//! 設計 brief）を candidate SHA ごとに 1 回だけ作る。
//!
//! 流れ:
//! - [`Dispatcher::health_job`]（tick の中。git も python も起こさない）: `spawn_review` が review 前の同期
//!   （ADR-0118）で `ReviewSync` を作り、reviewer run を起こすと決めた後に呼ぶ。reviewer run の無い review
//!   （reviewer 条件が無い・`reviewer_reuse` で reviewer を省いた）では何もしない。同期した repo が候補になり、
//!   remote workspace の repo と同期の無い repo は「作らない理由」になる。
//! - [`HealthInputJob::attach`]（reviewer run を起こす非同期 task の中）: [`build_health_input`] を `spawn_blocking`
//!   で走らせ、結果を `ReviewerRun.health`（reviewer の `ReviewRequest.health`）に入れる。子 process は全体で
//!   [`HEALTH_INPUT_TIMEOUT`] の締め切りを共有する。tick は止めない。
//! - [`build_health_input`]: repo ごとに base（target）側の検査器（[`with_base_checker`]）で review 方針を読み、
//!   同じ `(repo_id, target_sha, reviewed_sha, definition_sha256)` の `Event::HealthReviewInputBuilt` があればその
//!   `input` を使う（reviewer run の再試行・stale 再同期の後の再 review）。無ければ diff 統計・owner 地図の行・
//!   repo health の report（`repo_health.py --repo --base --head --policy-version 1 --out`）から作って追記する。
//!   方針が `absent`・読めない repo、remote workspace・同期の無い repo は `Event::HealthReviewInputSkipped` に
//!   理由を残す（その repo の分は `ReviewRequest.health` に入らない）。
//!
//! WU の check・WU の review（`work_units`・`phase_integration` の検査）はここを通らない（D4.4）。LLM は呼ばない。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use task_core::design_review::ReviewPolicyState;
use task_core::{Event, Task, TaskId, TaskStore};
use task_ops::changes::git;
use task_worker::protocol::{
    DiffStat, HealthFindingCounts, HealthFindingsSummary, HealthPolicyState, HealthRepoInput,
    HealthReportFinding, HealthReviewInput, OwnerRow, PathStat,
};

use super::policy::{BaseChecker, finished, left, parse_review_policy, with_base_checker};
use crate::dispatcher::Dispatcher;
use crate::dispatcher::review_spawn::ReviewSync;
use crate::review::ReviewerRun;

/// 1 回の材料作り（全 repo の方針の読み込み・diff・owner 地図・repo health）の時間の上限。
pub const HEALTH_INPUT_TIMEOUT: Duration = Duration::from_secs(600);
/// `repo_health.py --policy-version` に渡す版。
pub const POLICY_VERSION: &str = "1";
/// 読む report の schema。
pub const REPORT_SCHEMA: &str = "celeris.repo-health.report/1";
/// `findings.new` の上限（超えた数は `findings.truncated`）。
pub const MAX_FINDINGS: usize = 50;
/// `diff.paths` の上限（`diff.files` は総数のまま）。
pub const MAX_PATHS: usize = 40;
/// `owner_rows` の上限。
pub const MAX_OWNER_ROWS: usize = 40;

/// 同期の無い repo の理由（同期を省いた・失敗した repo は target と候補の SHA が決まらない）。
pub const NOT_SYNCED: &str = "review 前の同期が無い（target と候補の SHA が決まらない）";

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
    /// 材料を渡す reviewer run の id（event の `review_run`）。
    pub review_run: String,
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
    /// reviewer run があれば材料を作り `run.health` に入れて返す（reviewer run が無ければ何もしない）。
    pub(crate) async fn attach(self, run: Option<ReviewerRun>) -> Option<ReviewerRun> {
        let mut run = run?;
        if let Some(work) = self.0 {
            let task_id = work.task_id;
            match tokio::task::spawn_blocking(move || build_health_input(&work)).await {
                Ok(health) => run.health = health,
                Err(e) => {
                    tracing::warn!(%task_id, error = %e, "health input builder failed; reviewing without health");
                }
            }
        }
        Some(run)
    }
}

impl Dispatcher {
    /// D4.2・D4.4: `spawn_review` の review 前の同期の後、reviewer run を起こすと決めた後に 1 回呼ぶ。重い処理は
    /// しない（[`HealthInputJob::attach`] が reviewer run の非同期 task の中でする）。
    pub(crate) fn health_job(
        &self,
        task: &Task,
        synced: &[ReviewSync],
        remote: bool,
        reviewer: Option<&ReviewerRun>,
    ) -> HealthInputJob {
        let (Some(run), Some(task_dir)) = (reviewer, self.task_dir(task)) else {
            return HealthInputJob(None);
        };
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
        HealthInputJob(Some(HealthInputWork {
            store: self.store(),
            task_id: task.id,
            review_run: run.run_id.clone(),
            candidates,
            skipped,
            out_dir: task_dir.join("runs").join("health"),
            timeout: HEALTH_INPUT_TIMEOUT,
        }))
    }
}

/// D4: 材料を作る本体（blocking）。作れた repo が 1 つも無ければ `None`（`ReviewRequest.health` を持たない）。
pub fn build_health_input(work: &HealthInputWork) -> Option<HealthReviewInput> {
    let deadline = Instant::now() + work.timeout;
    for (repo_id, reason) in &work.skipped {
        work.skip(repo_id, reason);
    }
    let recorded = match work.store.events_for(work.task_id) {
        Ok(events) => events,
        Err(e) => {
            tracing::warn!(task_id = %work.task_id, error = %e, "cannot read events for health input reuse");
            Vec::new()
        }
    };
    let mut repos = Vec::new();
    for candidate in &work.candidates {
        match work.repo_input(candidate, &recorded, deadline) {
            Ok(input) => repos.push(input),
            Err(reason) => work.skip(&candidate.repo_id, &reason),
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

impl HealthInputWork {
    fn skip(&self, repo_id: &str, reason: &str) {
        let event = Event::HealthReviewInputSkipped {
            review_run: self.review_run.clone(),
            repo_id: repo_id.to_string(),
            reason: reason.to_string(),
        };
        if let Err(e) = self.store.append_event(self.task_id, &event) {
            tracing::warn!(task_id = %self.task_id, error = %e, "cannot record the skipped health input");
        }
    }

    /// 1 repo の材料。方針が `Present` でなければ理由（skip）。
    fn repo_input(
        &self,
        candidate: &CandidateRepo,
        recorded: &[(u64, Event)],
        deadline: Instant,
    ) -> Result<HealthRepoInput, String> {
        let c = candidate;
        with_base_checker(&c.dir, &c.target_sha, deadline, |checker| {
            let policy = match parse_review_policy(&checker.print_review_policy(&c.dir)?) {
                ReviewPolicyState::Present(policy) => policy,
                ReviewPolicyState::Absent => {
                    return Err("review 方針が absent（base の profile に [review] が無い）".into());
                }
                ReviewPolicyState::Unavailable(why) => {
                    return Err(format!("review 方針を読めない: {why}"));
                }
            };
            let definition = policy.definition_sha256.clone();
            if let Some(input) = find_recorded(recorded, c, &definition) {
                return Ok(input);
            }
            let (diff, changed) = diff_stat(c, deadline)?;
            let owner_rows = policy
                .owner_map
                .as_deref()
                .and_then(|map| show_at(&c.dir, &c.target_sha, map, deadline))
                .map(|text| owner_rows(&text, &changed))
                .unwrap_or_default();
            let report = self.run_report(checker, c);
            let complete = report.is_ok();
            let (findings, policy_state) = match report {
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
            // 走らせられなかった report（時間切れ・壊れた report）は記録しない。次の再試行でやり直す。
            if complete {
                self.record(c, definition, &input);
            }
            Ok(input)
        })
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

    /// base の検査器で repo health を 1 回走らせ、report を読む。exit 1（fail）・2（error）でも report があれば読む。
    fn run_report(
        &self,
        checker: &BaseChecker,
        c: &CandidateRepo,
    ) -> Result<RepoHealthReport, String> {
        std::fs::create_dir_all(&self.out_dir)
            .map_err(|e| format!("report の dir を作れない: {e}"))?;
        let name = format!(
            "{}-{}-{}.json",
            c.repo_id,
            short(&c.target_sha),
            short(&c.reviewed_sha)
        );
        let out = self.out_dir.join(name);
        let _ = std::fs::remove_file(&out);
        let (repo_arg, out_arg) = (c.dir.to_string_lossy(), out.to_string_lossy());
        let ran = checker.run(
            &[
                "--repo",
                &repo_arg,
                "--base",
                &c.target_sha,
                "--head",
                &c.reviewed_sha,
                "--policy-version",
                POLICY_VERSION,
                "--out",
                &out_arg,
            ],
            "repo_health.py",
        )?;
        let text = std::fs::read_to_string(&out).map_err(|e| {
            format!(
                "repo_health.py が report を書かなかった（exit {:?}: {}）: {e}",
                ran.code,
                ran.why()
            )
        })?;
        let report: RepoHealthReport =
            serde_json::from_str(&text).map_err(|e| format!("report を読めない: {e}"))?;
        if report.schema != REPORT_SCHEMA {
            return Err(format!(
                "report の schema が {REPORT_SCHEMA} でない: {:?}",
                report.schema
            ));
        }
        Ok(report)
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// 同じ key の `HealthReviewInputBuilt` の `input`（最後のもの）。
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
            serde_json::from_value(input.clone()).ok()
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

/// report（`celeris.repo-health.report/1`）のうち読む欄。
#[derive(Debug, Deserialize)]
pub struct RepoHealthReport {
    pub schema: String,
    pub summary: RepoHealthSummary,
    #[serde(default)]
    pub findings: Vec<RepoHealthFinding>,
    pub policy: RepoHealthPolicy,
}

#[derive(Debug, Deserialize)]
pub struct RepoHealthSummary {
    pub verdict: String,
    pub reviewable: bool,
    pub counts: HealthFindingCounts,
}

#[derive(Debug, Deserialize)]
pub struct RepoHealthFinding {
    pub rule_id: String,
    pub fingerprint: String,
    pub severity: String,
    pub delta: String,
    #[serde(default)]
    pub exception: Option<serde_json::Value>,
    #[serde(default)]
    pub evidence: Vec<RepoHealthEvidence>,
}

#[derive(Debug, Deserialize)]
pub struct RepoHealthEvidence {
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RepoHealthPolicy {
    pub source: String,
    pub changed: bool,
    /// report の `policy.files`（file ごとの base と head の blob）。
    #[serde(default, rename = "files")]
    pub blobs: Vec<PolicyBlobs>,
}

#[derive(Debug, Deserialize)]
pub struct PolicyBlobs {
    pub path: String,
    #[serde(default)]
    pub base_blob: Option<String>,
    #[serde(default)]
    pub head_blob: Option<String>,
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
        HealthPolicyState {
            source: "unavailable".into(),
            ..Default::default()
        },
    )
}

#[cfg(test)]
mod tests;
