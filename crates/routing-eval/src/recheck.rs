//! 採点の正しさを再実行で検める（ADR 2026-10-11-llm-source-model-routing-evaluation D1「評価集合」）。
//!
//! 凍結 manifest の各問題について、出所 WorkUnit の `base_commit` で決定的 check を再実行して
//! **失敗すること**（= 採点が noop でないこと）、記録された `head_commit` で **成功すること**を検める。
//! 通らない問題は理由つきで除外し、結果を manifest の `recheck` 欄に書く（問題集合そのものと
//! `sha256` は変えない。`recheck` は凍結後に足す検証記録で、hash の外）。
//!
//! 実行場所は `$TMPDIR` の下に作る `git worktree`（元のリポジトリの作業ツリーには触らない）。
//! 問題ごとに base・head の 2 つを切り、終わったら消す。base の実行が残した未追跡 file が
//! head の実行を成功させてしまわないように、同じ worktree を使い回さない。
//!
//! LLM は呼ばない。check command を `sh -c` で起こすだけ。

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::manifest::Manifest;

/// `recheck` 欄の schema。
pub const RECHECK_SCHEMA: &str = "celeris.routing-eval-recheck/1";

/// check command が manifest に記録されていない（採点できない）。
pub const REASON_NO_COMMAND: &str = "no_command_check";
/// 出所 WorkUnit に base commit が記録されていない。
pub const REASON_NO_BASE_COMMIT: &str = "no_base_commit";
/// 出所 WorkUnit に head commit が記録されていない。
pub const REASON_NO_HEAD_COMMIT: &str = "no_head_commit";
/// 記録された commit が `--repo` のリポジトリに無い（force push・shallow clone）。
pub const REASON_COMMIT_NOT_IN_REPO: &str = "commit_not_in_repo";
/// base で check が成功した = 差分を見ない noop な採点。
pub const REASON_NOOP_BASE_PASSES: &str = "noop_check_passes_at_base";
/// head で check が失敗した = 記録された解決済み状態で通らない。
pub const REASON_HEAD_FAILS: &str = "check_fails_at_head_commit";
/// check が時間内に終わらなかった。
pub const REASON_TIMED_OUT: &str = "check_timed_out";
/// check を起こせなかった（`sh` が無い等）。
pub const REASON_CHECK_ERROR: &str = "check_could_not_run";
/// git worktree を切れない・消せない。
pub const REASON_WORKTREE: &str = "worktree_failed";

/// 再実行の記録（manifest の `recheck` 欄に入る）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recheck {
    pub schema: String,
    /// 対象 manifest の `sha256`（下游が記録と集合を結び付ける鍵）。
    pub manifest_sha256: String,
    pub repo: String,
    pub checked_at: String,
    pub timeout_s: u64,
    pub problems: Vec<ProblemRecheck>,
    pub adopted: usize,
    pub excluded: usize,
    pub excluded_by_reason: BTreeMap<String, usize>,
}

/// 1 問題の再実行結果。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProblemRecheck {
    pub id: String,
    pub task_id: String,
    pub split: String,
    pub subset_12: bool,
    pub command: String,
    pub base_commit: Option<String>,
    pub head_commit: Option<String>,
    /// base で失敗し head で成功した（評価集合に採用できる）。
    pub adopted: bool,
    /// 除外理由（`REASON_*`）。採用時は `None`。
    pub reason: Option<String>,
    pub base_exit: Option<i32>,
    pub head_exit: Option<i32>,
    /// 人が理由を追えるようにする短い補足（stderr の末尾・git の失敗）。
    pub detail: Option<String>,
}

/// `recheck` の実行指定。
#[derive(Clone, Debug)]
pub struct Options {
    /// 凍結 manifest（`celeris-routing-eval manifest` の出力）。
    pub manifest: PathBuf,
    /// 出所 commit を持つ git リポジトリ。
    pub repo: PathBuf,
    /// 記録の写しを出す先（省略時は manifest にだけ書く）。
    pub out: Option<PathBuf>,
    /// check 1 本の上限時間。
    pub timeout: Duration,
    /// worktree の置き場所（省略時は `$TMPDIR` の下）。
    pub worktree_root: Option<PathBuf>,
    /// 記録する時刻（省略時は現在時刻。試験は固定値を渡す）。
    pub checked_at: Option<String>,
    /// 先頭 n 問題だけ検める（部分実行・試し）。
    pub limit: Option<usize>,
}

/// 再実行して記録を返し、manifest の `recheck` 欄に書く。
pub fn recheck(opts: &Options) -> Result<Recheck, String> {
    let raw = fs::read(&opts.manifest).map_err(|e| format!("{}: {e}", opts.manifest.display()))?;
    let manifest: Manifest =
        serde_json::from_slice(&raw).map_err(|e| format!("manifest is unreadable: {e}"))?;
    if manifest.schema != crate::MANIFEST_SCHEMA {
        return Err(format!(
            "manifest schema {} is not {}",
            manifest.schema,
            crate::MANIFEST_SCHEMA
        ));
    }
    let is_repo = git(&opts.repo, &["rev-parse", "--git-dir"]).is_some_and(|o| o.ok);
    if !is_repo {
        return Err(format!("{} is not a git repository", opts.repo.display()));
    }
    let mut worktrees = Worktrees::new(&opts.repo, opts.worktree_root.as_deref())?;
    let selected: Vec<_> = match opts.limit {
        Some(n) => manifest.problems.iter().take(n).collect(),
        None => manifest.problems.iter().collect(),
    };
    let mut rows = Vec::with_capacity(selected.len());
    for problem in selected {
        rows.push(check_problem(&mut worktrees, problem, opts.timeout));
    }
    drop(worktrees);
    let mut excluded_by_reason = BTreeMap::new();
    for row in rows.iter().filter(|r| !r.adopted) {
        *excluded_by_reason
            .entry(row.reason.clone().unwrap_or_default())
            .or_insert(0usize) += 1;
    }
    let adopted = rows.iter().filter(|r| r.adopted).count();
    let record = Recheck {
        schema: RECHECK_SCHEMA.into(),
        manifest_sha256: manifest.sha256.clone(),
        repo: opts.repo.display().to_string(),
        checked_at: match &opts.checked_at {
            Some(at) => at.clone(),
            None => OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .map_err(|e| e.to_string())?,
        },
        timeout_s: opts.timeout.as_secs(),
        problems: rows,
        excluded: excluded_by_reason.values().sum(),
        adopted,
        excluded_by_reason,
    };
    write_into_manifest(&opts.manifest, &record)?;
    if let Some(out) = &opts.out {
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(
            out,
            serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(record)
}

/// 候補を 1 問ずつ検める（`manifest --repo` の refill が引くたびに使う）。
pub struct Checker {
    worktrees: Worktrees,
    timeout: Duration,
}

impl Checker {
    pub fn new(
        repo: &Path,
        worktree_root: Option<&Path>,
        timeout: Duration,
    ) -> Result<Self, String> {
        let is_repo = git(repo, &["rev-parse", "--git-dir"]).is_some_and(|o| o.ok);
        if !is_repo {
            return Err(format!("{} is not a git repository", repo.display()));
        }
        Ok(Self {
            worktrees: Worktrees::new(repo, worktree_root)?,
            timeout,
        })
    }

    pub fn repo(&self) -> &Path {
        self.worktrees.repo()
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn check(&mut self, problem: &crate::manifest::Problem) -> ProblemRecheck {
        check_problem(&mut self.worktrees, problem, self.timeout)
    }

    /// `commit` の第 1 親（repair WU の base を補う）。無ければ `None`。
    pub fn parent_of(&self, commit: &str) -> Option<String> {
        let spec = format!("{commit}^1^{{commit}}");
        git(
            self.worktrees.repo(),
            &["rev-parse", "--verify", "--quiet", &spec],
        )
        .filter(|out| out.ok)
        .map(|out| out.stdout.trim().to_owned())
        .filter(|s| !s.is_empty())
    }
}

/// manifest の JSON に `recheck` 鍵を足して書き戻す（他の鍵はそのまま）。
pub fn write_into_manifest(path: &Path, record: &Recheck) -> Result<(), String> {
    let mut raw: Value = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let obj = raw
        .as_object_mut()
        .ok_or_else(|| "manifest is not a JSON object".to_owned())?;
    obj.insert(
        "recheck".into(),
        serde_json::to_value(record).map_err(|e| e.to_string())?,
    );
    fs::write(
        path,
        serde_json::to_vec_pretty(&raw).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// 1 問題: base で失敗・head で成功を検める。
fn check_problem(
    worktrees: &mut Worktrees,
    problem: &crate::manifest::Problem,
    timeout: Duration,
) -> ProblemRecheck {
    let mut row = ProblemRecheck {
        id: problem.id.clone(),
        task_id: problem.task_id.clone(),
        split: problem.split.clone(),
        subset_12: problem.subset_12,
        command: problem.command.clone(),
        base_commit: non_empty(problem.base_commit.clone()),
        head_commit: non_empty(problem.head_commit.clone()),
        adopted: false,
        reason: None,
        base_exit: None,
        head_exit: None,
        detail: None,
    };
    let excluded = |row: &mut ProblemRecheck, reason: &str, detail: Option<String>| {
        row.reason = Some(reason.into());
        row.detail = detail;
    };
    if row.command.trim().is_empty() {
        excluded(&mut row, REASON_NO_COMMAND, None);
        return row;
    }
    let (Some(base), Some(head)) = (row.base_commit.clone(), row.head_commit.clone()) else {
        let reason = if row.base_commit.is_none() {
            REASON_NO_BASE_COMMIT
        } else {
            REASON_NO_HEAD_COMMIT
        };
        excluded(&mut row, reason, None);
        return row;
    };
    for (stage, commit) in [("base", &base), ("head", &head)] {
        if !commit_exists(worktrees.repo(), commit) {
            excluded(
                &mut row,
                REASON_COMMIT_NOT_IN_REPO,
                Some(format!("{stage} commit {commit} is not in the repository")),
            );
            return row;
        }
    }
    // base: 失敗しなければならない（成功 = 差分を見ない noop な採点）。
    let base_outcome = match run_at(worktrees, &base, "base", &row.command, timeout) {
        Ok(o) => o,
        Err(e) => {
            excluded(&mut row, e.reason, Some(e.detail));
            return row;
        }
    };
    row.base_exit = base_outcome.exit;
    if base_outcome.timed_out {
        excluded(
            &mut row,
            REASON_TIMED_OUT,
            Some(format!(
                "base check did not finish in {}s",
                timeout.as_secs()
            )),
        );
        return row;
    }
    if base_outcome.exit == Some(0) {
        excluded(
            &mut row,
            REASON_NOOP_BASE_PASSES,
            Some("check already passes at the base commit".into()),
        );
        return row;
    }
    // head: 成功しなければならない。
    let head_outcome = match run_at(worktrees, &head, "head", &row.command, timeout) {
        Ok(o) => o,
        Err(e) => {
            excluded(&mut row, e.reason, Some(e.detail));
            return row;
        }
    };
    row.head_exit = head_outcome.exit;
    if head_outcome.timed_out {
        excluded(
            &mut row,
            REASON_TIMED_OUT,
            Some(format!(
                "head check did not finish in {}s",
                timeout.as_secs()
            )),
        );
        return row;
    }
    if head_outcome.exit != Some(0) {
        let stderr = tail(&head_outcome.stderr, 400);
        let detail = if stderr.is_empty() {
            format!("check exited {:?} at the head commit", head_outcome.exit)
        } else {
            stderr
        };
        excluded(&mut row, REASON_HEAD_FAILS, Some(detail));
        return row;
    }
    row.adopted = true;
    row
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// worktree を切って check を 1 本実行し、worktree を消す。
fn run_at(
    worktrees: &mut Worktrees,
    commit: &str,
    stage: &str,
    command: &str,
    timeout: Duration,
) -> Result<CheckOutcome, StageFailure> {
    let tree = worktrees
        .add(commit, stage)
        .map_err(StageFailure::worktree)?;
    let outcome = worktrees
        .run_check(&tree.dir, stage, command, timeout)
        .map_err(StageFailure::check)?;
    tree.remove();
    Ok(outcome)
}

struct StageFailure {
    reason: &'static str,
    detail: String,
}

impl StageFailure {
    fn worktree(detail: String) -> Self {
        Self {
            reason: REASON_WORKTREE,
            detail,
        }
    }

    fn check(detail: String) -> Self {
        Self {
            reason: REASON_CHECK_ERROR,
            detail,
        }
    }
}

/// `$TMPDIR` の下に worktree と log の置き場所を作る。`Drop` で消す。
struct Worktrees {
    repo: PathBuf,
    root: PathBuf,
    seq: u32,
}

impl Worktrees {
    fn new(repo: &Path, root: Option<&Path>) -> Result<Self, String> {
        let root = match root {
            Some(r) => r.to_path_buf(),
            None => std::env::temp_dir().join(format!(
                "celeris-routing-eval-recheck-{}",
                std::process::id()
            )),
        };
        fs::create_dir_all(root.join("wt")).map_err(|e| format!("{}: {e}", root.display()))?;
        fs::create_dir_all(root.join("log")).map_err(|e| e.to_string())?;
        let _ = git(repo, &["worktree", "prune"]);
        Ok(Self {
            repo: repo.to_path_buf(),
            root,
            seq: 0,
        })
    }

    fn repo(&self) -> &Path {
        &self.repo
    }

    fn add(&mut self, commit: &str, stage: &str) -> Result<Worktree, String> {
        self.seq += 1;
        let dir = self.root.join("wt").join(format!("{}-{stage}", self.seq));
        let path = dir.to_string_lossy().into_owned();
        let out = git(&self.repo, &["worktree", "add", "--detach", &path, commit])
            .ok_or_else(|| "git is not available".to_owned())?;
        if !out.ok {
            return Err(format!(
                "git worktree add {path} {commit} (exit {:?}): {}",
                out.code,
                out.stderr.trim()
            ));
        }
        Ok(Worktree {
            repo: self.repo.clone(),
            dir,
        })
    }

    /// worktree の中で `sh -c <command>` を実行する（出力は log file へ。pipe を塞がない）。
    fn run_check(
        &self,
        dir: &Path,
        stage: &str,
        command: &str,
        timeout: Duration,
    ) -> Result<CheckOutcome, String> {
        let name = dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let out_path = self.root.join("log").join(format!("{name}-{stage}.out"));
        let err_path = self.root.join("log").join(format!("{name}-{stage}.err"));
        let stdout = fs::File::create(&out_path).map_err(|e| e.to_string())?;
        let stderr = fs::File::create(&err_path).map_err(|e| e.to_string())?;
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir)
            .env("CELERIS_EVAL_RECHECK", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|e| format!("cannot run `sh -c {command}`: {e}"))?;
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Ok(CheckOutcome {
                            exit: None,
                            timed_out: true,
                            stderr: read_tail(&err_path),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(format!("cannot wait for the check: {e}")),
            }
        };
        Ok(CheckOutcome {
            exit: status.code(),
            timed_out: false,
            stderr: read_tail(&err_path),
        })
    }
}

impl Drop for Worktrees {
    fn drop(&mut self) {
        let _ = git(&self.repo, &["worktree", "prune"]);
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Worktree {
    repo: PathBuf,
    dir: PathBuf,
}

impl Worktree {
    fn remove(&self) {
        let path = self.dir.to_string_lossy().into_owned();
        let removed =
            git(&self.repo, &["worktree", "remove", "--force", &path]).is_some_and(|out| out.ok);
        if !removed {
            let _ = git(&self.repo, &["worktree", "prune"]);
            let _ = fs::remove_dir_all(&self.dir);
            let _ = git(&self.repo, &["worktree", "prune"]);
        }
    }
}

struct CheckOutcome {
    exit: Option<i32>,
    timed_out: bool,
    stderr: String,
}

fn read_tail(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn tail(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_owned()
    } else {
        trimmed
            .chars()
            .skip(trimmed.chars().count() - max)
            .collect::<String>()
    }
}

fn commit_exists(repo: &Path, commit: &str) -> bool {
    let spec = format!("{commit}^{{commit}}");
    git(repo, &["rev-parse", "--verify", "--quiet", &spec])
        .is_some_and(|out| out.ok && !out.stdout.trim().is_empty())
}

struct GitOutput {
    ok: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// `git -C <dir> <args...>`。git が無い・起動できないときは `None`。
fn git(dir: &Path, args: &[&str]) -> Option<GitOutput> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        // 人の設定（対話的な認証・lock）に引きずられないようにする。
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .ok()?;
    Some(GitOutput {
        ok: out.status.success(),
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// `recheck` の使い方（`celeris-routing-eval recheck --help`）。
pub const USAGE: &str = "celeris-routing-eval recheck — verify that each frozen problem discriminates\n\nUSAGE:\n  celeris-routing-eval recheck --manifest <file> --repo <path> [OPTIONS]\n\nEach problem's check command is re-run in a temporary git worktree under $TMPDIR:\n  at base_commit it must FAIL (a check that passes there scores nothing — noop),\n  at head_commit it must PASS.\nProblems that do not satisfy this are excluded with a reason, and the record is\nwritten into the manifest's `recheck` key (the frozen problem set and its sha256\nare left untouched).\n\nOPTIONS:\n  --manifest <file>     Frozen manifest.json\n  --repo <path>         Git repository holding the recorded base/head commits\n  --out <file>          Also write the record here (default: only into the manifest)\n  --timeout <secs>      Per-check time limit (default 900)\n  --limit <n>           Re-check only the first n problems\n  --worktree-root <dir> Where to put the temporary worktrees (default: $TMPDIR)\n  --checked-at <ts>     Timestamp to record (default: now, RFC 3339)\n\nEXCLUSION REASONS:\n  no_command_check, no_base_commit, no_head_commit, commit_not_in_repo,\n  noop_check_passes_at_base, check_fails_at_head_commit, check_timed_out,\n  check_could_not_run, worktree_failed";

/// CLI: `celeris-routing-eval recheck --manifest <file> --repo <path> [--out <file>] [--timeout <s>] [--limit <n>] [--worktree-root <dir>]`
pub fn run(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let opts = parse(args)?;
    let record = recheck(&opts)?;
    let mut summary = format!(
        "recheck: manifest={} repo={} adopted={} excluded={}",
        opts.manifest.display(),
        opts.repo.display(),
        record.adopted,
        record.excluded
    );
    for (reason, count) in &record.excluded_by_reason {
        summary.push_str(&format!(" {reason}={count}"));
    }
    println!("{summary}");
    for row in record.problems.iter().filter(|r| !r.adopted) {
        println!(
            "  excluded {} reason={}{}",
            row.id,
            row.reason.clone().unwrap_or_default(),
            match &row.detail {
                Some(d) => format!(" ({})", tail(d, 160)),
                None => String::new(),
            }
        );
    }
    Ok(())
}

fn parse(args: &[String]) -> Result<Options, String> {
    let value = |flag: &str| -> Result<Option<String>, String> {
        Ok(args
            .iter()
            .position(|x| x == flag)
            .and_then(|i| args.get(i + 1))
            .cloned())
    };
    let required = |flag: &str| -> Result<String, String> {
        value(flag)?.ok_or_else(|| format!("missing required option {flag}"))
    };
    let timeout_s = match value("--timeout")? {
        Some(v) => v
            .parse::<u64>()
            .map_err(|e| format!("--timeout is not a number: {e}"))?,
        None => 900,
    };
    Ok(Options {
        manifest: PathBuf::from(required("--manifest")?),
        repo: PathBuf::from(required("--repo")?),
        out: value("--out")?.map(PathBuf::from),
        timeout: Duration::from_secs(timeout_s),
        worktree_root: value("--worktree-root")?.map(PathBuf::from),
        checked_at: value("--checked-at")?,
        limit: match value("--limit")? {
            Some(v) => Some(
                v.parse::<usize>()
                    .map_err(|e| format!("--limit is not a number: {e}"))?,
            ),
            None => None,
        },
    })
}

#[cfg(test)]
#[path = "recheck/tests.rs"]
mod tests;
