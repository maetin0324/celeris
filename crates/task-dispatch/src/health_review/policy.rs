//! ADR 2026-11-07-health-review-integration D2.3・I3: review 方針（profile の `[review]` 節）を base（target）側の
//! 検査器で読む。
//!
//! [`load_review_policy`] の手順: target の commit から検査器（[`CHECKER_DIR`] と、検査器が同じ checkout から読む
//! [`CHECKER_SIDE_FILES`]）を `git archive` で一時 dir に出し、その `repo_health.py` を
//! `--print-review-policy --repo <repo> --base <target_sha>` で起こす。候補（head・作業ツリー）の検査器と profile は
//! 使わない（候補が自分の review の条件を変えられない。I3）。出力 `celeris.repo-health.review-policy/1` は
//! [`ReviewPolicyState`] に写す:
//! - `source = absent`、または base に検査器（`repo_health.py`）が無い → `Absent`（その repo は `[review]` 節を持たず、
//!   設計 review を使わない。候補が検査器を足しても base には無いので変わらない。I3）。
//! - `source = base`・`builtin` → `Present`。
//! - target を解決できない・非 0 exit・壊れた JSON・schema 違い・時間切れ・`source = bootstrap`（head の profile。
//!   `--print-review-policy` は head を読まないので出ないはずの値）・remote workspace → `Unavailable(理由)`。
//!
//! どれも黙って省かない（risk の判定では `Unavailable` は `unknown` になる。D3.3）。dispatcher からは planner run の
//! 起動時に 1 回だけ呼ぶ（[`Dispatcher::planner_review_policy`]）。全体の時間に上限（[`REVIEW_POLICY_TIMEOUT`]）を置き、
//! tick を長く止めない。LLM は呼ばない。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;
use task_core::design_review::{PathClass, ReviewPolicy, ReviewPolicyState};
use task_core::{Task, WorkspaceSpec};
use task_ops::changes::{CmdOutput, git, run_with_timeout};

use crate::dispatcher::Dispatcher;

/// `--print-review-policy` の出力の schema（`scripts/dev/repo-health/schema/review-policy-v1.schema.json`）。
pub const REVIEW_POLICY_SCHEMA: &str = "celeris.repo-health.review-policy/1";
/// base から出す検査器の dir（repo 相対）。
pub const CHECKER_DIR: &str = "scripts/dev/repo-health";
/// 起こす script（repo 相対）。base に無ければ `Unavailable`。
pub const CHECKER_SCRIPT: &str = "scripts/dev/repo-health/repo_health.py";
/// 検査器が同じ checkout から読む file（repo 相対）。base にあれば検査器と一緒に出す（無い base もある）。
pub const CHECKER_SIDE_FILES: &[&str] = &["scripts/dev/source-size-report.py"];
/// 1 回の読み込み全体（ref の解決・`git archive`・展開・script）の時間の上限。
pub const REVIEW_POLICY_TIMEOUT: Duration = Duration::from_secs(20);

/// `celeris.repo-health.review-policy/1` の `schema` 以外の欄（schema の `additionalProperties: false` と同じく未知の欄は
/// 拒否する）。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewPolicyOut {
    source: String,
    owner_map: Option<String>,
    high_risk: Vec<PathClass>,
    invariants: BTreeMap<String, String>,
    definition_sha256: String,
}

/// `repo` の `target`（commit の sha か ref）の版の検査器で review 方針を読む。`timeout` は全体の上限。
pub fn load_review_policy(repo: &Path, target: &str, timeout: Duration) -> ReviewPolicyState {
    let deadline = Instant::now() + timeout;
    match run_base_checker(repo, target, deadline) {
        Ok(Some(stdout)) => parse_review_policy(&stdout),
        Ok(None) => ReviewPolicyState::Absent,
        Err(why) => ReviewPolicyState::Unavailable(why),
    }
}

/// `--print-review-policy` の stdout を [`ReviewPolicyState`] に写す。
pub fn parse_review_policy(stdout: &str) -> ReviewPolicyState {
    let unavailable = |why: String| ReviewPolicyState::Unavailable(why);
    let mut value: serde_json::Value = match serde_json::from_str(stdout) {
        Ok(value) => value,
        Err(e) => return unavailable(format!("--print-review-policy の出力が JSON でない: {e}")),
    };
    let schema = value.get("schema").and_then(|s| s.as_str()).unwrap_or("");
    if schema != REVIEW_POLICY_SCHEMA {
        return unavailable(format!(
            "--print-review-policy の schema が {REVIEW_POLICY_SCHEMA} でない: {schema:?}"
        ));
    }
    if let Some(object) = value.as_object_mut() {
        object.remove("schema");
    }
    let out: ReviewPolicyOut = match serde_json::from_value(value) {
        Ok(out) => out,
        Err(e) => {
            return unavailable(format!(
                "--print-review-policy の出力が {REVIEW_POLICY_SCHEMA} の形でない: {e}"
            ));
        }
    };
    match out.source.as_str() {
        "absent" => ReviewPolicyState::Absent,
        "base" | "builtin" => {
            let digest = &out.definition_sha256;
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            {
                return unavailable(format!(
                    "definition_sha256 が 64 桁の 16 進でない: {digest:?}"
                ));
            }
            ReviewPolicyState::Present(ReviewPolicy {
                owner_map: out.owner_map,
                high_risk: out.high_risk,
                invariants: out.invariants,
                definition_sha256: out.definition_sha256,
            })
        }
        "bootstrap" => unavailable("source = bootstrap（head の profile）は使わない（I3）".into()),
        other => unavailable(format!("--print-review-policy の source が未知: {other:?}")),
    }
}

/// base の検査器を一時 dir に出して起こし、stdout を返す。base に検査器が無ければ `None`。失敗は理由の文。
fn run_base_checker(
    repo: &Path,
    target: &str,
    deadline: Instant,
) -> Result<Option<String>, String> {
    let rev = format!("{target}^{{commit}}");
    let resolved = finished(
        git(
            repo,
            &["rev-parse", "--verify", "--quiet", &rev],
            left(deadline)?,
        ),
        "git rev-parse",
    )?;
    if !resolved.ok {
        return Err(format!("target {target} を commit に解決できない"));
    }
    let sha = resolved.stdout.trim().to_string();
    let short = &sha[..sha.len().min(12)];
    let on_base = |path: &str| -> Result<bool, String> {
        let spec = format!("{sha}:{path}");
        let out = finished(
            git(repo, &["cat-file", "-e", &spec], left(deadline)?),
            "git cat-file",
        )?;
        Ok(out.ok)
    };
    if !on_base(CHECKER_SCRIPT)? {
        tracing::debug!(base = %short, "no {CHECKER_SCRIPT} on the base; the repo has no review policy");
        return Ok(None);
    }
    let scratch = Scratch::create()?;
    let archive = scratch.0.join("checker.tar");
    let output = format!("--output={}", archive.display());
    let mut args = vec![
        "archive",
        "--format=tar",
        output.as_str(),
        sha.as_str(),
        "--",
    ];
    args.push(CHECKER_DIR);
    for side in CHECKER_SIDE_FILES {
        if on_base(side)? {
            args.push(*side);
        }
    }
    let archived = finished(git(repo, &args, left(deadline)?), "git archive")?;
    if !archived.ok {
        return Err(format!("git archive に失敗: {}", archived.why()));
    }
    let tree = scratch.0.join("tree");
    std::fs::create_dir_all(&tree).map_err(|e| format!("一時 dir を作れない: {e}"))?;
    let (archive_arg, tree_arg) = (archive.to_string_lossy(), tree.to_string_lossy());
    let extracted = finished(
        run_with_timeout(
            &scratch.0,
            "tar",
            &["-xf", &archive_arg, "-C", &tree_arg],
            left(deadline)?,
        ),
        "tar",
    )?;
    if !extracted.ok {
        return Err(format!("検査器の展開に失敗: {}", extracted.why()));
    }
    let script = tree.join(CHECKER_SCRIPT);
    let (script_arg, repo_arg) = (script.to_string_lossy(), repo.to_string_lossy());
    let printed = finished(
        run_with_timeout(
            &tree,
            "python3",
            &[
                "-I",
                "-B",
                &script_arg,
                "--print-review-policy",
                "--repo",
                &repo_arg,
                "--base",
                &sha,
            ],
            left(deadline)?,
        ),
        "repo_health.py --print-review-policy",
    )?;
    if !printed.ok {
        return Err(format!(
            "repo_health.py --print-review-policy が失敗: {}",
            printed.why()
        ));
    }
    Ok(Some(printed.stdout))
}

/// 締め切りまでの残り。過ぎていれば時間切れの理由。
fn left(deadline: Instant) -> Result<Duration, String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(format!(
            "review 方針の読み込みが {} 秒の上限を超えた",
            REVIEW_POLICY_TIMEOUT.as_secs()
        ));
    }
    Ok(left)
}

/// 起動できなかった・時間切れを理由の文にする（終わったものはそのまま返す）。
fn finished(out: Option<CmdOutput>, what: &str) -> Result<CmdOutput, String> {
    match out {
        None => Err(format!("{what} を起こせない")),
        Some(out) if out.timed_out => Err(format!("{what} が時間内に終わらない")),
        Some(out) => Ok(out),
    }
}

/// 検査器を出す一時 dir（drop で消す）。
struct Scratch(PathBuf);

impl Scratch {
    fn create() -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("celeris-review-policy-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("一時 dir を作れない: {e}"))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Dispatcher {
    /// D2.3: planner run の起動時に 1 回（`execution_planner_context`）、task の先頭の repo（worker の cwd）の
    /// target 側で review 方針を読む。`Unavailable` は理由を log に残す（planner の `RunContext` には `Present` だけ
    /// を入れる。risk の判定は後続の design-gate）。
    pub(crate) fn planner_review_policy(&self, task: &Task) -> ReviewPolicyState {
        let state = self.task_review_policy(task);
        if let ReviewPolicyState::Unavailable(why) = &state {
            tracing::warn!(task_id = %task.id, reason = %why, "review policy unavailable");
        }
        state
    }

    /// target は review 前の同期と同じ（`review_target_ref`: 木の子は親の branch、ほかは repo の既定 branch）。
    /// 案件の repo の無い task（作業場所そのものの worktree）は worktree の base。git の repo が無い task は
    /// 方針を読む repo が無いので `Absent`。remote は `Unavailable`（D4.4 と同じく remote では読まない）。
    fn task_review_policy(&self, task: &Task) -> ReviewPolicyState {
        if matches!(task.workspace, WorkspaceSpec::Remote { .. }) {
            return ReviewPolicyState::Unavailable("remote workspace".into());
        }
        let Some(workspaces) = self.task_workspaces_for(task) else {
            if task.repos.is_empty() {
                return ReviewPolicyState::Absent;
            }
            return ReviewPolicyState::Unavailable(
                "作業場所の repo を解決できない（remote の repo・消えた repo の行）".into(),
            );
        };
        let Some(primary) = workspaces.repos.into_iter().next() else {
            return ReviewPolicyState::Absent;
        };
        let Some(worktree) = primary.worktree else {
            return ReviewPolicyState::Absent;
        };
        let target = match task.repos.iter().find(|r| r.name == primary.name) {
            Some(reference) => {
                match self.review_target_ref(task, reference.repo_id, &worktree.repo) {
                    Ok(Some(target)) => target,
                    Ok(None) => {
                        return ReviewPolicyState::Unavailable(format!(
                            "repo {} の target branch を解決できない",
                            primary.name
                        ));
                    }
                    Err(e) => {
                        return ReviewPolicyState::Unavailable(format!(
                            "repo {} の target branch を読めない: {e}",
                            primary.name
                        ));
                    }
                }
            }
            None => worktree.base.sha.clone(),
        };
        load_review_policy(&worktree.repo, &target, REVIEW_POLICY_TIMEOUT)
    }
}

#[cfg(test)]
mod tests;
