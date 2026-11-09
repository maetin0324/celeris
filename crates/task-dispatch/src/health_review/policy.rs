//! ADR 2026-11-07-health-review-integration D2.3・I3: review 方針（profile の `[review]` 節）を base（target）側の
//! 検査器で読む。
//!
//! [`load_review_policy`] の手順: 検査器は経路共通の選び方（`health_review::gate::with_applicable_checker`。ADR
//! 2026-11-08-health-enforcement-loop D1.1・D1.3）で決める。target の commit に検査器（`task_core::required_checks::health`
//! の [`CHECKER_DIR`] と、検査器が同じ checkout から読む [`CHECKER_SIDE_FILES`]）があればそれを `git archive` で一時 dir に
//! 出し、無ければ呼び出し側が渡した Celeris の release の同梱の検査器を使う（J3）。その `repo_health.py` を
//! `--print-review-policy --repo <repo> --base <target_sha>` で起こす。候補（head・作業ツリー）の検査器と profile は
//! 使わない（候補が自分の review の条件を変えられない。I3）。出力 `celeris.repo-health.review-policy/1` は
//! [`ReviewPolicyState`] に写す:
//! - `source = absent`（その repo は `[review]` 節を持たず、設計 review を使わない）、または適用判定が `NotApplicable`
//!   （Git 管理でない作業場所） → `Absent`。
//! - `source = base`・`builtin` → `Present`。
//! - 適用判定が `Unavailable`（base にも同梱にも検査器が無い `checker_missing`、事実を読めない `facts_unreadable:<理由>`）、
//!   target を解決できない・非 0 exit・壊れた JSON・schema 違い・時間切れ・`source = bootstrap`（head の profile。
//!   `--print-review-policy` は head を読まないので出ないはずの値）・remote workspace → `Unavailable(理由)`。当てないの
//!   ではなく合格にしない（risk の判定では `unknown`。G2 D3.3）。
//!
//! どれも黙って省かない。worker と planner の run には、tick が [`Dispatcher::review_policy_target`] で repo と target を
//! 決め、run の起動の非同期部分で 1 回だけ読む（付記 B2。`health_review/worker_profile.rs`）。全体の時間に上限
//! （[`REVIEW_POLICY_TIMEOUT`]）を置き、tick を止めない。LLM は呼ばない。
//!
//! 検査器を出す口は [`with_base_checker`] の 1 つ（D1.3）。site の写し（`CheckerId` の出どころ → [`CheckerSite`]）は
//! `health_review::gate` の中だけにあり、ここと G2 の材料（`health_review::input`）は写しを持たない。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;
use task_core::design_review::{PathClass, ReviewPolicy, ReviewPolicyState};
use task_core::required_checks::health::{
    CHECKER_DIR, CHECKER_SCRIPT, CHECKER_SIDE_FILES, base_has_file,
};
use task_core::{Task, WorkspaceSpec};
use task_ops::changes::{CmdOutput, git, run_with_timeout};

use super::gate::{self, BundledChecker, CheckerChoice};
use crate::dispatcher::Dispatcher;

/// `--print-review-policy` の出力の schema（`scripts/dev/repo-health/schema/review-policy-v1.schema.json`）。
pub const REVIEW_POLICY_SCHEMA: &str = "celeris.repo-health.review-policy/1";
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

/// `repo` の `target`（commit の sha か ref）で適用判定が選んだ検査器（base の版、無ければ `bundled`）で review 方針を
/// 読む。`bundled` は呼び出し側が `gate::bundled_checker()` で渡す。`timeout` は事実の読み取りを含む全体の上限。
pub fn load_review_policy(
    repo: &Path,
    target: &str,
    bundled: Option<&BundledChecker>,
    timeout: Duration,
) -> ReviewPolicyState {
    let deadline = Instant::now() + timeout;
    let chosen = gate::with_applicable_checker(repo, target, true, bundled, deadline, |c, _| {
        c.print_review_policy(repo)
    });
    match chosen {
        CheckerChoice::NotApplicable(_) => ReviewPolicyState::Absent,
        CheckerChoice::Unavailable(reason) => ReviewPolicyState::Unavailable(reason.to_string()),
        CheckerChoice::Ran(Ok(stdout)) => parse_review_policy(&stdout),
        CheckerChoice::Ran(Err(why)) => ReviewPolicyState::Unavailable(why),
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

/// [`with_base_checker`] が起こす検査器の出どころ（D1.3・D1.4）。`CheckerId` からの写しは `health_review::gate` だけが
/// 作る。
#[derive(Debug, Clone, Copy)]
pub(crate) enum CheckerSite<'a> {
    /// target の commit の検査器を一時 dir に出す（base に無ければ `run` を呼ばず `None`）。
    Base,
    /// Celeris の release に同梱した検査器の dir（`<release>/repo-health`。base から出したときと同じ repo 相対の配置）。
    Bundled(&'a Path),
}

/// 起こす検査器（[`with_base_checker`] の間だけある）。target の commit から出した一時 dir か、同梱の dir。
pub(crate) struct BaseChecker {
    /// target を解決した commit の sha。
    pub sha: String,
    tree: PathBuf,
    deadline: Instant,
}

impl BaseChecker {
    /// 出した `repo_health.py` を `args` で起こす（cwd は検査器の dir、残りの時間が上限）。非 0 exit も返す。
    pub(crate) fn run(&self, args: &[&str], what: &str) -> Result<CmdOutput, String> {
        let script = self.tree.join(CHECKER_SCRIPT);
        let script_arg = script.to_string_lossy();
        let mut argv = vec![&*script_arg];
        argv.extend_from_slice(args);
        self.python(&argv, what)
    }

    /// 同じ検査器の dir を cwd にして `python3 -I -B <argv>` を起こす（残りの時間が上限）。非 0 exit も返す。
    pub(crate) fn python(&self, argv: &[&str], what: &str) -> Result<CmdOutput, String> {
        let mut full = vec!["-I", "-B"];
        full.extend_from_slice(argv);
        finished(
            run_with_timeout(&self.tree, "python3", &full, left(self.deadline)?),
            what,
        )
    }

    /// 検査器の package の dir（`<tree>/scripts/dev/repo-health`。`health` package の親）。
    pub(crate) fn checker_dir(&self) -> PathBuf {
        self.tree.join(CHECKER_DIR)
    }

    /// `--print-review-policy --repo <repo> --base <sha>` の stdout。非 0 exit は理由の文。
    pub(crate) fn print_review_policy(&self, repo: &Path) -> Result<String, String> {
        let repo_arg = repo.to_string_lossy();
        let printed = self.run(
            &[
                "--print-review-policy",
                "--repo",
                &repo_arg,
                "--base",
                &self.sha,
            ],
            "repo_health.py --print-review-policy",
        )?;
        if !printed.ok {
            return Err(format!(
                "repo_health.py --print-review-policy が失敗: {}",
                printed.why()
            ));
        }
        Ok(printed.stdout)
    }
}

/// `repo` の `target` を commit に解決し、`site` の検査器を `run` に渡す。site が `Base` なら `target` の版の検査器を
/// 一時 dir に出し（戻ると消す）、base に検査器が無ければ `run` を呼ばず `None`。`Bundled` なら同梱の dir をそのまま
/// 渡す（base は見ない）。失敗は理由の文。site を選ぶのは `health_review::gate` の適用判定だけ。
/// 候補（head・作業ツリー）の検査器は使わない（I3・J3）。`deadline` は ref の解決・archive・展開・`run` の全体の締め切り。
pub(crate) fn with_base_checker<T>(
    repo: &Path,
    target: &str,
    site: CheckerSite<'_>,
    deadline: Instant,
    run: impl FnOnce(&BaseChecker) -> Result<T, String>,
) -> Result<Option<T>, String> {
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
    if let CheckerSite::Bundled(dir) = site {
        return run(&BaseChecker {
            sha,
            tree: dir.to_path_buf(),
            deadline,
        })
        .map(Some);
    }
    let short = &sha[..sha.len().min(12)];
    let on_base = |path: &str| base_has_file(&sha, path, |args| git_stdout(repo, args, deadline));
    if !on_base(CHECKER_SCRIPT)? {
        tracing::debug!(base = %short, "no {CHECKER_SCRIPT} on the base; the repo has no review policy");
        return Ok(None);
    }
    let scratch = Scratch::create("celeris-review-policy")?;
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
    run(&BaseChecker {
        sha,
        tree,
        deadline,
    })
    .map(Some)
}

/// `repo` で git を締め切りまで実行した stdout（非 0 exit・起動の失敗・時間切れは理由の文）。task_core の判定関数
/// （`base_has_file` など）に渡す closure の形。
pub(crate) fn git_stdout(repo: &Path, args: &[&str], deadline: Instant) -> Result<String, String> {
    let out = finished(git(repo, args, left(deadline)?), "git")?;
    if out.ok {
        Ok(out.stdout)
    } else {
        Err(out.why())
    }
}

/// 締め切りまでの残り。過ぎていれば時間切れの理由。
pub(crate) fn left(deadline: Instant) -> Result<Duration, String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err("base の検査器の実行が時間の上限を超えた".to_string());
    }
    Ok(left)
}

/// 起動できなかった・時間切れを理由の文にする（終わったものはそのまま返す）。
pub(crate) fn finished(out: Option<CmdOutput>, what: &str) -> Result<CmdOutput, String> {
    match out {
        None => Err(format!("{what} を起こせない")),
        Some(out) if out.timed_out => Err(format!("{what} が時間内に終わらない")),
        Some(out) => Ok(out),
    }
}

/// 検査器・report を置く一時 dir（drop で消す）。
pub(crate) struct Scratch(pub PathBuf);

impl Scratch {
    pub(crate) fn create(prefix: &str) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", ulid::Ulid::new()));
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
    /// D2.3・付記 B2: worker と planner の run で review 方針を読む repo と target を、tick の中で決める（読み込みは
    /// しない。[`load_review_policy`] は run の起動の非同期部分が呼ぶ。`health_review/worker_profile.rs`）。repo は task の
    /// 先頭の repo（worker の cwd）。WU の run でも target は task の target（WU の base ではない）。
    ///
    /// target は review 前の同期と同じ（`review_target_ref`: 木の子は親の branch、ほかは repo の既定 branch）。
    /// 案件の repo の無い task（作業場所そのものの worktree）は worktree の base。`Ok(None)` は方針を読む repo が無い
    /// （git の repo が無い task。`Absent`）。`Err` は `Unavailable` の理由（remote は D4.4 と同じく読まない）。
    pub(crate) fn review_policy_target(
        &self,
        task: &Task,
    ) -> Result<Option<(PathBuf, String)>, String> {
        if matches!(task.workspace, WorkspaceSpec::Remote { .. }) {
            return Err("remote workspace".into());
        }
        let Some(workspaces) = self.task_workspaces_for(task) else {
            if task.repos.is_empty() {
                return Ok(None);
            }
            return Err(
                "作業場所の repo を解決できない（remote の repo・消えた repo の行）".into(),
            );
        };
        let Some(primary) = workspaces.repos.into_iter().next() else {
            return Ok(None);
        };
        let Some(worktree) = primary.worktree else {
            return Ok(None);
        };
        let target = match task.repos.iter().find(|r| r.name == primary.name) {
            Some(reference) => {
                match self.review_target_ref(task, reference.repo_id, &worktree.repo) {
                    Ok(Some(target)) => target,
                    Ok(None) => {
                        return Err(format!(
                            "repo {} の target branch を解決できない",
                            primary.name
                        ));
                    }
                    Err(e) => {
                        return Err(format!(
                            "repo {} の target branch を読めない: {e}",
                            primary.name
                        ));
                    }
                }
            }
            None => worktree.base.sha.clone(),
        };
        Ok(Some((worktree.repo, target)))
    }
}

#[cfg(test)]
mod tests;
