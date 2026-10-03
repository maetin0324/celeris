//! 変更の取り込み（intake）の足回り（ADR-0043 D5。Phase 54）。
//!
//! タスクが `celeris/<task_id>` ブランチに積んだ変更を、**人が**見て取り込むための道具:
//!
//! - [`changes`] — リポジトリ 1 つ分の差分の要約（base / head / ahead / ファイル一覧 / 汚れ）
//! - [`file_diff`] — 1 ファイルの unified diff（200 KiB で切る）
//! - [`merge_into_default_branch`] — 一時 worktree で rebase → default_branch を fast-forward
//! - [`sync_onto_target`] — review 前に task の worktree を最新の target へ rebase する（ADR-0118 D2）
//! - [`discard`] / [`remove_worktree_and_branch`] — worktree とブランチを消す
//! - [`push_branch`] / [`gh_*`] — `origin` へ push して `gh` で PR を作る・見る・merge する
//!
//! ここは **`git` と `gh` を起こすだけ**で、判断も LLM も無い（DESIGN 原則 1）。全ての子プロセスに
//! 待ち時間の上限があり、超えたら殺して「失敗」を返す（API のハンドラが握りっぱなしにならないように）。
//!
//! **押すのは人だけ**（SPEC §3.6 / ADR-0043 D5）。ワーカーのプロトコルにはこの経路を出さない。

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 読み取り（`status` / `diff` / `rev-list`）の待ち時間。
pub const GIT_TIMEOUT: Duration = Duration::from_secs(30);
/// 書き込み（`worktree add` / `rebase` / `push`）の待ち時間。
pub const GIT_WRITE_TIMEOUT: Duration = Duration::from_secs(300);
/// ADR-0043 D5: PR の状態を見るのは 10 秒で諦める（画面を開くたびに走るため）。
pub const GH_VIEW_TIMEOUT: Duration = Duration::from_secs(10);
/// PR を作る・merge するのは人が押したときだけなので少し長い。
pub const GH_WRITE_TIMEOUT: Duration = Duration::from_secs(120);
/// ADR-0043 D5: 1 ファイルの diff は 200 KiB で切る。
pub const MAX_DIFF_BYTES: usize = 200 * 1024;
/// `gh auth status` の結果をプロセス内で使い回す時間（ADR-0043 D5）。
pub const GH_AUTH_CACHE: Duration = Duration::from_secs(60);
/// 追跡外のファイルの行数を数えるときに読む上限（これより大きければ `additions = 0`）。
const MAX_UNTRACKED_BYTES: u64 = 1024 * 1024;

// ---------------------------------------------------------------------------
// 子プロセス（決定的。全部に待ち時間の上限がある）
// ---------------------------------------------------------------------------

/// 子プロセスの結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl CmdOutput {
    /// 失敗の 1 行説明（人に見せる。`stderr` の最初の非空行）。
    pub fn why(&self) -> String {
        if self.timed_out {
            return "コマンドが時間内に終わりませんでした".to_string();
        }
        let line = self
            .stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .or_else(|| self.stdout.lines().map(str::trim).find(|l| !l.is_empty()))
            .unwrap_or("");
        if line.is_empty() {
            format!("exit {:?}", self.code)
        } else {
            format!("{line}（exit {:?}）", self.code)
        }
    }
}

/// 起動して、`timeout` を過ぎたら殺す。パイプは別スレッドで読み切る（詰まらせない）。
/// 起動そのものに失敗したら `None`（`git` / `gh` が無い）。
///
/// 子は自分のプロセスグループに入れ、時間切れでは**グループごと**殺す。子だけを殺すと、
/// 孫（`git` の `ssh`、`sh -c` の中のコマンド）がパイプを握ったまま残り、読み切りの join が
/// 孫の終わりまで待って上限が効かない。
fn run(mut cmd: Command, timeout: Duration) -> Option<CmdOutput> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = cmd.spawn().ok()?;
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    let out_thread = std::thread::spawn(move || read_all(out_pipe));
    let err_thread = std::thread::spawn(move || read_all(err_pipe));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Err(_) => break None,
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    kill_group(child.id());
                    let _ = child.kill();
                    break child.wait().ok();
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    let code = status.and_then(|s| s.code());
    Some(CmdOutput {
        ok: !timed_out && code == Some(0),
        code,
        stdout,
        stderr,
        timed_out,
    })
}

/// `process_group(0)` で起こした子のグループ（pgid = 子の pid）へ SIGKILL を送る。
/// task-ops は signal の crate を持たないので `kill(1)` に頼る。失敗しても呼び出し側が
/// `child.kill()` で子だけは殺す。
fn kill_group(pid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn read_all(pipe: Option<impl std::io::Read>) -> String {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// `git -C <dir> <args...>`（人の設定と対話的な認証に引きずられない）。
pub fn git(dir: &Path, args: &[&str], timeout: Duration) -> Option<CmdOutput> {
    git_with_env(dir, args, &[], timeout)
}

/// [`git`] に加えて追加の環境変数を渡す版。ADR-0051 Phase 106: `push` の `GIT_SSH_COMMAND`
/// （`-o BatchMode=yes` を足して対話的な鍵入力を止める）など、呼び出しごとに違う環境が要るとき用。
pub fn git_with_env(
    dir: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
    timeout: Duration,
) -> Option<CmdOutput> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        // 引用されたパス（`"src/\346\227\245"`）を避け、人の hooks と対話的な認証を止める。
        .args(["-c", "core.quotePath=false"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    run(cmd, timeout)
}

/// 任意の子プロセスを 1 つ起こす（上限つき）。`git` が無い環境での `grep` の代わりなど、
/// **`git` 以外の決定的な道具**を呼ぶときだけ使う（ADR-0047 D3 の全文検索のフォールバック）。
pub fn run_with_timeout(
    dir: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Option<CmdOutput> {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir).args(args);
    run(cmd, timeout)
}

fn git_ok(dir: &Path, args: &[&str]) -> bool {
    git(dir, args, GIT_TIMEOUT).is_some_and(|o| o.ok)
}

fn git_line(dir: &Path, args: &[&str]) -> Option<String> {
    let out = git(dir, args, GIT_TIMEOUT)?;
    if !out.ok {
        return None;
    }
    let line = out.stdout.trim().to_string();
    if line.is_empty() { None } else { Some(line) }
}

// ---------------------------------------------------------------------------
// ADR-0130 D2: run / WU の actual write-set（確定差分）の採取

/// `base..HEAD` の確定差分（コミット済みの path だけ）。`dirty` は未コミットの編集・追跡外の
/// ファイルがあったか（あれば記録は `incomplete`。未コミットの path は `paths` に入れない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedWriteSet {
    pub base_sha: String,
    pub head_sha: String,
    pub paths: Vec<String>,
    pub dirty: bool,
}

/// run 開始時の HEAD（ADR-0130 D2: 開始前に固定する）。worktree があればその HEAD、無ければ
/// （これから `worktree add` する）ブランチの先端、ブランチも無ければ切り出す base。
pub fn run_start_head(
    dir: &Path,
    repo: &Path,
    branch: &str,
    fallback_base: &str,
) -> Option<String> {
    if dir.join(".git").exists() {
        return git_line(dir, &["rev-parse", "--verify", "HEAD^{commit}"]);
    }
    git_line(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}^{{commit}}"),
        ],
    )
    .or_else(|| {
        git_line(
            repo,
            &[
                "rev-parse",
                "--verify",
                &format!("{fallback_base}^{{commit}}"),
            ],
        )
    })
}

/// `git diff --name-only -z --no-renames <base>..HEAD` を `dir` で取る（ADR-0130 D2）。
/// rename は旧名・新名を両方数え、ソート・重複排除する。git が起きない・SHA が読めない・
/// path が UTF-8 でないときは `Err`（呼び出し側は `unavailable` として残し、run は落とさない）。
pub fn committed_write_set(dir: &Path, base: &str) -> Result<CommittedWriteSet, String> {
    let resolve = |rev: &str| {
        let out = git(
            dir,
            &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
            GIT_TIMEOUT,
        )
        .ok_or_else(|| "git did not start".to_string())?;
        if !out.ok {
            return Err(format!("cannot resolve {rev}: {}", out.why()));
        }
        Ok(out.stdout.trim().to_string())
    };
    let base_sha = resolve(base)?;
    let head_sha = resolve("HEAD")?;
    let range = format!("{base_sha}..{head_sha}");
    let out = git(
        dir,
        &["diff", "--name-only", "-z", "--no-renames", &range],
        GIT_TIMEOUT,
    )
    .ok_or_else(|| "git did not start".to_string())?;
    if !out.ok {
        return Err(format!("git diff {range}: {}", out.why()));
    }
    let mut paths = parse_name_only_z(&out.stdout)?;
    paths.sort();
    paths.dedup();
    let dirty = is_dirty(dir).ok_or_else(|| "git status failed".to_string())?;
    Ok(CommittedWriteSet {
        base_sha,
        head_sha,
        paths,
        dirty,
    })
}

/// `--name-only -z` の出力（NUL 区切り）を path の並びにする。UTF-8 でない path（読み込みで
/// U+FFFD に置き換わったもの）があれば `Err`（実績を偽らない）。
pub fn parse_name_only_z(text: &str) -> Result<Vec<String>, String> {
    text.split('\0')
        .filter(|p| !p.is_empty())
        .map(|p| {
            if p.contains('\u{FFFD}') {
                Err(format!("non UTF-8 path: {p}"))
            } else {
                Ok(p.to_string())
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 読み取り（`GET /tasks/{id}/changes`）
// ---------------------------------------------------------------------------

/// 変わったファイル 1 件。
#[derive(
    Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct ChangedFile {
    /// リポジトリの根からの相対パス。
    pub path: String,
    /// `A`（追加）/ `M`（変更）/ `D`（削除）/ `?`（git の管理外）/ `T`（種類が変わった）。
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    /// バイナリ（git が行数を出さなかった）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub binary: bool,
}

/// ファイル数と ± の合計。
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
pub struct DiffStat {
    pub files: u64,
    pub additions: u64,
    pub deletions: u64,
}

/// リポジトリ 1 つ分の「このタスクが変えたもの」（ADR-0043 D5）。
#[derive(
    Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub struct RepoChanges {
    /// 分岐した地点の sha（`merge-base(default_branch, head)`。取れなければ目印の base）。
    pub base: String,
    /// いまのブランチの先端の sha。
    pub head: String,
    /// `base..head` のコミットの数（**コミットが無ければ 0**。ADR-0043 D5）。
    pub ahead: u64,
    pub files: Vec<ChangedFile>,
    pub stat: DiffStat,
    /// 作業ツリーに未コミットの変更がある。
    pub dirty: bool,
    /// worktree もブランチも無い（取り込み済み・中止済み・そもそも切っていない）。
    pub missing: bool,
}

/// どこを見て差分を出したか（`file_diff` に同じものを渡す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesSource {
    /// worktree がある（未コミットの変更も見える）。
    Worktree(PathBuf),
    /// worktree は無いがブランチはある（元のリポジトリで `base..branch` を見る）。
    Branch { repo: PathBuf, branch: String },
    /// どちらも無い。
    Missing,
}

/// 見るべき場所を決める（worktree → ブランチ → 無い）。
pub fn source_for(repo: &Path, worktree: Option<&Path>, branch: &str) -> ChangesSource {
    if let Some(dir) = worktree
        && dir.join(".git").exists()
        && git_ok(dir, &["rev-parse", "--git-dir"])
    {
        return ChangesSource::Worktree(dir.to_path_buf());
    }
    if !branch.is_empty()
        && git_ok(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
    {
        return ChangesSource::Branch {
            repo: repo.to_path_buf(),
            branch: branch.to_string(),
        };
    }
    ChangesSource::Missing
}

/// ADR-0043 D1: git の既定のブランチ。設定（`project_repos.default_branch`）が無ければ
/// `origin/HEAD` → `main` → `master` の順で検出する。何も無ければ `"main"`。
pub fn default_branch(repo: &Path, configured: Option<&str>) -> String {
    if let Some(name) = configured.map(str::trim).filter(|n| !n.is_empty()) {
        return name.to_string();
    }
    if let Some(head) = git_line(
        repo,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) && let Some(name) = head.strip_prefix("origin/")
        && !name.is_empty()
    {
        return name.to_string();
    }
    for name in ["main", "master"] {
        if git_ok(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{name}"),
            ],
        ) {
            return name.to_string();
        }
    }
    "main".to_string()
}

/// リポジトリ 1 つ分の差分の要約（ADR-0043 D5）。
///
/// - worktree があれば**未コミットの変更も含めて** base からの差分を出す（人が見たいのはそれ）。
///   `ahead` は `base..HEAD` のコミット数なので、まだコミットしていなければ 0 になる。
/// - worktree が無くブランチだけあれば、元のリポジトリで `base..branch` を見る（`dirty = false`）。
/// - どちらも無ければ `missing = true`、`ahead = 0`。
pub fn changes(
    repo: &Path,
    worktree: Option<&Path>,
    branch: &str,
    default_branch: &str,
    fallback_base: Option<&str>,
) -> RepoChanges {
    match source_for(repo, worktree, branch) {
        ChangesSource::Missing => RepoChanges {
            missing: true,
            ..Default::default()
        },
        ChangesSource::Worktree(dir) => {
            let head = git_line(&dir, &["rev-parse", "HEAD"]).unwrap_or_default();
            let base = base_of(&dir, default_branch, &head, fallback_base);
            let dirty = git(&dir, &["status", "--porcelain"], GIT_TIMEOUT)
                .is_some_and(|o| o.ok && !o.stdout.trim().is_empty());
            let mut files = diff_files(&dir, &[&base]);
            files.extend(untracked_files(&dir));
            files.sort_by(|a, b| a.path.cmp(&b.path));
            RepoChanges {
                ahead: commit_count(&dir, &base, "HEAD"),
                stat: stat_of(&files),
                base,
                head,
                files,
                dirty,
                missing: false,
            }
        }
        ChangesSource::Branch { repo, branch } => {
            let reference = format!("refs/heads/{branch}");
            let head = git_line(&repo, &["rev-parse", &reference]).unwrap_or_default();
            let base = base_of(&repo, default_branch, &head, fallback_base);
            let range = format!("{base}..{reference}");
            let files = diff_files(&repo, &[&range]);
            RepoChanges {
                ahead: commit_count(&repo, &base, &reference),
                stat: stat_of(&files),
                base,
                head,
                files,
                dirty: false,
                missing: false,
            }
        }
    }
}

fn stat_of(files: &[ChangedFile]) -> DiffStat {
    DiffStat {
        files: files.len() as u64,
        additions: files.iter().map(|f| f.additions).sum(),
        deletions: files.iter().map(|f| f.deletions).sum(),
    }
}

/// 分岐点。`merge-base(default_branch, head)` が取れなければ目印の base、それも無ければ head。
fn base_of(dir: &Path, default_branch: &str, head: &str, fallback: Option<&str>) -> String {
    if !head.is_empty()
        && let Some(sha) = git_line(dir, &["merge-base", default_branch, head])
    {
        return sha;
    }
    if let Some(fallback) = fallback.map(str::trim).filter(|b| !b.is_empty())
        && let Some(sha) = git_line(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{fallback}^{{commit}}"),
            ],
        )
    {
        return sha;
    }
    head.to_string()
}

fn commit_count(dir: &Path, base: &str, head: &str) -> u64 {
    if base.is_empty() || head.is_empty() {
        return 0;
    }
    git_line(dir, &["rev-list", "--count", &format!("{base}..{head}")])
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

/// `git diff --numstat` + `--name-status` を突き合わせる（`--no-renames` で `a => b` を避ける）。
fn diff_files(dir: &Path, rev: &[&str]) -> Vec<ChangedFile> {
    let mut args: Vec<&str> = vec!["diff", "--numstat", "--no-renames"];
    args.extend_from_slice(rev);
    let Some(numstat) = git(dir, &args, GIT_TIMEOUT).filter(|o| o.ok) else {
        return Vec::new();
    };
    let mut args: Vec<&str> = vec!["diff", "--name-status", "--no-renames"];
    args.extend_from_slice(rev);
    let statuses = git(dir, &args, GIT_TIMEOUT)
        .filter(|o| o.ok)
        .map(|o| parse_name_status(&o.stdout))
        .unwrap_or_default();
    parse_numstat(&numstat.stdout)
        .into_iter()
        .map(|(path, additions, deletions, binary)| {
            let status = statuses
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, s)| s.clone())
                .unwrap_or_else(|| "M".to_string());
            ChangedFile {
                path,
                status,
                additions,
                deletions,
                binary,
            }
        })
        .collect()
}

/// `--numstat` の 1 行は `<+>\t<->\t<path>`（バイナリは `-\t-\t<path>`）。
pub fn parse_numstat(text: &str) -> Vec<(String, u64, u64, bool)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(add), Some(del), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        let binary = add == "-" || del == "-";
        out.push((
            path.to_string(),
            add.parse::<u64>().unwrap_or(0),
            del.parse::<u64>().unwrap_or(0),
            binary,
        ));
    }
    out
}

/// `--name-status` の 1 行は `<status>\t<path>`。
pub fn parse_name_status(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.splitn(2, '\t');
        let (Some(status), Some(path)) = (parts.next(), parts.next()) else {
            continue;
        };
        let status = status.trim();
        let path = path.trim();
        if status.is_empty() || path.is_empty() {
            continue;
        }
        // `M100` のような形（`--find-copies` 等）は先頭の 1 文字だけ使う。
        out.push((path.to_string(), status.chars().take(1).collect::<String>()));
    }
    out
}

/// git の管理外のファイル（`.gitignore` は尊重する）。人が見たいのは「このタスクが置いたもの」なので
/// 追加行だけ数える（大きすぎる・バイナリは 0）。
fn untracked_files(dir: &Path) -> Vec<ChangedFile> {
    let Some(out) = git(
        dir,
        &["ls-files", "--others", "--exclude-standard"],
        GIT_TIMEOUT,
    )
    .filter(|o| o.ok) else {
        return Vec::new();
    };
    out.stdout
        .lines()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|path| {
            let (additions, binary) = count_added_lines(&dir.join(path));
            ChangedFile {
                path: path.to_string(),
                status: "?".to_string(),
                additions,
                deletions: 0,
                binary,
            }
        })
        .collect()
}

fn count_added_lines(path: &Path) -> (u64, bool) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (0, false);
    };
    if !meta.is_file() || meta.len() > MAX_UNTRACKED_BYTES {
        return (0, false);
    }
    let Ok(bytes) = std::fs::read(path) else {
        return (0, false);
    };
    if bytes.contains(&0) {
        return (0, true);
    }
    let lines = bytes.iter().filter(|b| **b == b'\n').count() as u64;
    // 末尾に改行が無いファイルの最後の行も 1 行と数える。
    let lines = if bytes.is_empty() || bytes.ends_with(b"\n") {
        lines
    } else {
        lines + 1
    };
    (lines, false)
}

/// 1 ファイルの unified diff（ADR-0043 D5。200 KiB で切る）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub diff: String,
    /// 200 KiB を超えたので途中で切った。
    pub truncated: bool,
}

/// `path` 1 件の unified diff。git が起こせない・そのファイルに差分が無いときは空の diff を返す。
pub fn file_diff(
    repo: &Path,
    worktree: Option<&Path>,
    branch: &str,
    default_branch: &str,
    fallback_base: Option<&str>,
    path: &str,
) -> Option<FileDiff> {
    let source = source_for(repo, worktree, branch);
    let (dir, rev) = match &source {
        ChangesSource::Missing => return None,
        ChangesSource::Worktree(dir) => {
            let head = git_line(dir, &["rev-parse", "HEAD"]).unwrap_or_default();
            (
                dir.clone(),
                base_of(dir, default_branch, &head, fallback_base),
            )
        }
        ChangesSource::Branch { repo, branch } => {
            let reference = format!("refs/heads/{branch}");
            let head = git_line(repo, &["rev-parse", &reference]).unwrap_or_default();
            let base = base_of(repo, default_branch, &head, fallback_base);
            (repo.clone(), format!("{base}..{reference}"))
        }
    };
    let out = git(
        &dir,
        &["diff", "--no-renames", &rev, "--", path],
        GIT_TIMEOUT,
    )?;
    let mut text = if out.ok { out.stdout } else { String::new() };
    // 追跡外のファイル（`git diff` には出ない）は `--no-index` で「空 → いまの中身」を出す。
    if text.trim().is_empty()
        && matches!(source, ChangesSource::Worktree(_))
        && dir.join(path).is_file()
        && let Some(untracked) = git(
            &dir,
            &["diff", "--no-index", "--", "/dev/null", path],
            GIT_TIMEOUT,
        )
    {
        // `--no-index` は差分があると exit 1 なので `ok` は見ない。
        text = untracked.stdout;
    }
    Some(truncate_diff(&text, path))
}

/// 200 KiB で切る（文字の境界で切る。切ったら印を立てる）。
pub fn truncate_diff(text: &str, path: &str) -> FileDiff {
    if text.len() <= MAX_DIFF_BYTES {
        return FileDiff {
            path: path.to_string(),
            diff: text.to_string(),
            truncated: false,
        };
    }
    let mut end = MAX_DIFF_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    FileDiff {
        path: path.to_string(),
        diff: text[..end].to_string(),
        truncated: true,
    }
}

// ---------------------------------------------------------------------------
// 取り込み（`POST /tasks/{id}/changes/{repo}/integrate`）
// ---------------------------------------------------------------------------

/// `merge` の結果（ADR-0043 D5）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// default_branch を `sha` まで進めた。
    Merged { sha: String, fast_forwarded: bool },
    /// rebase が衝突した（worktree はそのまま。衝突したファイルの一覧を返す）。
    Conflict { files: Vec<String> },
    /// 人のチェックアウトが default_branch を編集中（409）。
    Busy { detail: String },
    /// git が失敗した（`detail` に理由）。
    Failed { detail: String },
}

/// 人のチェックアウト（`local.path`）がいま出しているブランチ。detached なら `None`。
pub fn current_branch(repo: &Path) -> Option<String> {
    let name = git_line(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if name == "HEAD" { None } else { Some(name) }
}

/// 作業ツリーに未コミットの変更があるか（git が動かせなければ `None`）。
pub fn is_dirty(dir: &Path) -> Option<bool> {
    let out = git(dir, &["status", "--porcelain"], GIT_TIMEOUT)?;
    if !out.ok {
        return None;
    }
    Some(!out.stdout.trim().is_empty())
}

/// ADR-0043 D5 の `merge`: 一時 worktree でブランチを default_branch に rebase し、成功したら
/// **default_branch を fast-forward** する。`origin` には push しない。
///
/// - 人のチェックアウトが default_branch を出していて未コミットの変更があれば `Busy`（409）
/// - 出していて綺麗なら `git -C <repo> merge --ff-only <sha>`（作業ツリーもそのまま進む）
/// - 別のブランチを出していれば `git -C <repo> update-ref`（作業ツリーには触らない）
/// - rebase が衝突したら `rebase --abort` して `Conflict`（worktree もブランチも残す）
pub fn merge_into_default_branch(
    repo: &Path,
    branch: &str,
    default_branch: &str,
    temp_dir: &Path,
) -> MergeOutcome {
    let branch_ref = format!("refs/heads/{branch}");
    if !git_ok(repo, &["rev-parse", "--verify", "--quiet", &branch_ref]) {
        return MergeOutcome::Failed {
            detail: format!("ブランチ {branch} がありません"),
        };
    }
    let default_ref = format!("refs/heads/{default_branch}");
    let Some(default_sha) = git_line(repo, &["rev-parse", "--verify", "--quiet", &default_ref])
    else {
        return MergeOutcome::Failed {
            detail: format!("既定のブランチ {default_branch} がありません"),
        };
    };
    // 人のチェックアウトが default_branch を編集中なら、何も触らずに 409（ADR-0043 D5）。
    let checked_out = current_branch(repo).is_some_and(|b| b == default_branch);
    if checked_out && is_dirty(repo) != Some(false) {
        return MergeOutcome::Busy {
            detail: format!("{default_branch} が編集中"),
        };
    }

    if let Some(parent) = temp_dir.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return MergeOutcome::Failed {
            detail: "一時 worktree を作れませんでした".to_string(),
        };
    }
    let temp = temp_dir.to_string_lossy().into_owned();
    let add = git(
        repo,
        &["worktree", "add", "--detach", &temp, &branch_ref],
        GIT_WRITE_TIMEOUT,
    );
    match add {
        Some(o) if o.ok => {}
        Some(o) => {
            return MergeOutcome::Failed {
                detail: format!("一時 worktree を作れませんでした: {}", o.why()),
            };
        }
        None => {
            return MergeOutcome::Failed {
                detail: "git を起動できませんでした".to_string(),
            };
        }
    }

    let outcome = rebase_and_advance(repo, temp_dir, default_branch, &default_sha, checked_out);
    let _ = git(
        repo,
        &["worktree", "remove", "--force", &temp],
        GIT_WRITE_TIMEOUT,
    );
    let _ = git(repo, &["worktree", "prune"], GIT_TIMEOUT);
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(temp_dir);
    }
    outcome
}

fn rebase_and_advance(
    repo: &Path,
    temp_dir: &Path,
    default_branch: &str,
    default_sha: &str,
    checked_out: bool,
) -> MergeOutcome {
    let new_sha = match rebase_onto(temp_dir, default_sha) {
        RebaseStep::Done { head_sha } => head_sha,
        RebaseStep::Conflict { files, .. } => return MergeOutcome::Conflict { files },
        RebaseStep::Failed { detail, .. } => return MergeOutcome::Failed { detail },
    };
    if checked_out {
        // 人の作業ツリーが default_branch を出していて綺麗なので、そのまま早送りする。
        match git(repo, &["merge", "--ff-only", &new_sha], GIT_WRITE_TIMEOUT) {
            Some(o) if o.ok => MergeOutcome::Merged {
                sha: new_sha,
                fast_forwarded: true,
            },
            Some(o) => MergeOutcome::Failed {
                detail: format!("{default_branch} を早送りできませんでした: {}", o.why()),
            },
            None => MergeOutcome::Failed {
                detail: "git を起動できませんでした".to_string(),
            },
        }
    } else {
        // 別のブランチ（または detached）を出しているので、ref だけ動かす。作業ツリーには触らない。
        let reference = format!("refs/heads/{default_branch}");
        match git(
            repo,
            &["update-ref", &reference, &new_sha, default_sha],
            GIT_WRITE_TIMEOUT,
        ) {
            Some(o) if o.ok => MergeOutcome::Merged {
                sha: new_sha,
                fast_forwarded: false,
            },
            Some(o) => MergeOutcome::Failed {
                detail: format!("{default_branch} を動かせませんでした: {}", o.why()),
            },
            None => MergeOutcome::Failed {
                detail: "git を起動できませんでした".to_string(),
            },
        }
    }
}

/// [`rebase_onto`] の結果。取り込み先 ref の早送り・worktree の削除・push は含まない。
enum RebaseStep {
    /// rebase が済んだ。`head_sha` は rebase 後の `HEAD`。
    Done { head_sha: String },
    /// 衝突した。`rebase --abort` を試み、戻せたかを `aborted` に持つ。
    Conflict { files: Vec<String>, aborted: bool },
    /// 衝突以外で失敗した（`aborted` は `rebase --abort` が通ったか、rebase が始まらなかったか）。
    Failed { detail: String, aborted: bool },
}

/// `dir`（clean な worktree）で `git rebase <onto>` を行い、衝突なら衝突ファイルを集めて
/// `rebase --abort` する（ADR-0043 D5 の `merge` と ADR-0118 D2 の同期で共有する）。
fn rebase_onto(dir: &Path, onto: &str) -> RebaseStep {
    match git(dir, &["rebase", onto], GIT_WRITE_TIMEOUT) {
        Some(o) if o.ok => {}
        Some(o) => {
            let files = git(
                dir,
                &["diff", "--name-only", "--diff-filter=U"],
                GIT_TIMEOUT,
            )
            .filter(|o| o.ok)
            .map(|o| {
                o.stdout
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
            let aborted = git(dir, &["rebase", "--abort"], GIT_WRITE_TIMEOUT).is_some_and(|a| a.ok);
            if files.is_empty() && !o.stdout.contains("CONFLICT") && !o.stderr.contains("CONFLICT")
            {
                return RebaseStep::Failed {
                    detail: format!("rebase に失敗しました: {}", o.why()),
                    aborted,
                };
            }
            return RebaseStep::Conflict { files, aborted };
        }
        None => {
            return RebaseStep::Failed {
                detail: "git を起動できませんでした".to_string(),
                aborted: true,
            };
        }
    }
    match git_line(dir, &["rev-parse", "HEAD"]) {
        Some(head_sha) => RebaseStep::Done { head_sha },
        None => RebaseStep::Failed {
            detail: "rebase の結果を読めませんでした".to_string(),
            aborted: true,
        },
    }
}

// ---------------------------------------------------------------------------
// review 前の target 同期（ADR-0118 D2）
// ---------------------------------------------------------------------------

/// [`sync_onto_target`] の結果（ADR-0118 D2）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    /// `target_sha` は既に `HEAD` の祖先（または同一）。rebase していない。
    UpToDate {
        target_sha: String,
        head_sha: String,
    },
    /// `before_sha` を `target_sha` の上へ rebase し、`head_sha` になった。
    Rebased {
        target_sha: String,
        before_sha: String,
        head_sha: String,
    },
    /// rebase が衝突した。`rebase --abort` 済みで、ブランチと worktree は元の `HEAD` のまま。
    Conflict {
        target_sha: String,
        files: Vec<String>,
    },
    /// 未コミットの変更がある。何も触っていない（stash・reset もしない）。
    Dirty,
    /// git が失敗した・前提が満たされない（`detail` に理由）。
    Failed { detail: String },
}

/// ADR-0118 D2: task の worktree（ブランチを出している clean な worktree）を `target_ref` の
/// 現在の commit へ rebase する。取り込み先 ref の早送り・worktree の削除・push はしない。
///
/// - 未コミットの変更があれば `Dirty`（触らない）
/// - rebase が進行中・detached HEAD・`target_ref` が読めなければ `Failed`（触らない）
/// - `target_ref` が既に `HEAD` の祖先なら `UpToDate`
/// - 衝突したら `rebase --abort` して `Conflict`（元の `HEAD` を残す。戻せなければ `Failed`）
/// - 済んだら `HEAD` がブランチを指し、clean であることを確かめて `Rebased`
pub fn sync_onto_target(worktree: &Path, target_ref: &str) -> SyncOutcome {
    match is_dirty(worktree) {
        Some(false) => {}
        Some(true) => return SyncOutcome::Dirty,
        None => {
            return SyncOutcome::Failed {
                detail: "作業ツリーの状態を読めませんでした".to_string(),
            };
        }
    }
    for marker in ["rebase-merge", "rebase-apply"] {
        let in_progress = git_line(worktree, &["rev-parse", "--git-path", marker])
            .map(|p| {
                let p = PathBuf::from(p);
                if p.is_absolute() { p } else { worktree.join(p) }
            })
            .is_some_and(|p| p.exists());
        if in_progress {
            return SyncOutcome::Failed {
                detail: "rebase が進行中です".to_string(),
            };
        }
    }
    let Some(branch_ref) = git_line(worktree, &["symbolic-ref", "-q", "HEAD"]) else {
        return SyncOutcome::Failed {
            detail: "ブランチを出していません（detached HEAD）".to_string(),
        };
    };
    let target_rev = format!("{target_ref}^{{commit}}");
    let Some(target_sha) = git_line(worktree, &["rev-parse", "--verify", "--quiet", &target_rev])
    else {
        return SyncOutcome::Failed {
            detail: format!("{target_ref} を読めませんでした"),
        };
    };
    let Some(before_sha) = git_line(worktree, &["rev-parse", "HEAD"]) else {
        return SyncOutcome::Failed {
            detail: "HEAD を読めませんでした".to_string(),
        };
    };
    if git_ok(
        worktree,
        &["merge-base", "--is-ancestor", &target_sha, &before_sha],
    ) {
        return SyncOutcome::UpToDate {
            target_sha,
            head_sha: before_sha,
        };
    }

    let head_sha = match rebase_onto(worktree, &target_sha) {
        RebaseStep::Done { head_sha } => head_sha,
        RebaseStep::Conflict { files, aborted } => {
            let restored = aborted
                && git_line(worktree, &["rev-parse", "HEAD"]).as_deref() == Some(&before_sha);
            if !restored {
                return SyncOutcome::Failed {
                    detail: format!(
                        "rebase が衝突し、元の HEAD {before_sha} に戻せませんでした（手で確かめてください）"
                    ),
                };
            }
            return SyncOutcome::Conflict { target_sha, files };
        }
        RebaseStep::Failed { detail, aborted } => {
            if aborted {
                return SyncOutcome::Failed { detail };
            }
            return SyncOutcome::Failed {
                detail: format!("{detail}（rebase --abort も失敗しました。手で確かめてください）"),
            };
        }
    };
    let branch_sha = git_line(worktree, &["rev-parse", "--verify", "--quiet", &branch_ref]);
    if branch_sha.as_deref() != Some(head_sha.as_str()) {
        return SyncOutcome::Failed {
            detail: format!("rebase 後の HEAD {head_sha} と {branch_ref} が一致しません"),
        };
    }
    if is_dirty(worktree) != Some(false) {
        return SyncOutcome::Failed {
            detail: "rebase 後の作業ツリーが clean ではありません".to_string(),
        };
    }
    SyncOutcome::Rebased {
        target_sha,
        before_sha,
        head_sha,
    }
}

/// worktree を消してブランチも消す（ADR-0043 D5 の `merge` 成功後と `discard`）。
/// 人が手で消していても落ちない（`prune` してから `branch -D`）。
pub fn remove_worktree_and_branch(
    repo: &Path,
    worktree: Option<&Path>,
    branch: &str,
) -> Result<(), String> {
    if let Some(dir) = worktree
        && dir.exists()
    {
        let path = dir.to_string_lossy().into_owned();
        let removed = git(
            repo,
            &["worktree", "remove", "--force", &path],
            GIT_WRITE_TIMEOUT,
        )
        .is_some_and(|o| o.ok);
        if !removed {
            let _ = git(repo, &["worktree", "prune"], GIT_TIMEOUT);
            if dir.exists() && std::fs::remove_dir_all(dir).is_err() {
                return Err(format!("作業ツリー {path} を消せませんでした"));
            }
        }
        let _ = git(repo, &["worktree", "prune"], GIT_TIMEOUT);
    }
    if branch.is_empty() {
        return Ok(());
    }
    let reference = format!("refs/heads/{branch}");
    if !git_ok(repo, &["rev-parse", "--verify", "--quiet", &reference]) {
        return Ok(());
    }
    match git(repo, &["branch", "-D", branch], GIT_WRITE_TIMEOUT) {
        Some(o) if o.ok => Ok(()),
        Some(o) => Err(format!("ブランチ {branch} を消せませんでした: {}", o.why())),
        None => Err("git を起動できませんでした".to_string()),
    }
}

/// ADR-0043 D5 の `discard`: worktree とブランチを消す（確認は呼び出し側で取る）。
pub fn discard(repo: &Path, worktree: Option<&Path>, branch: &str) -> Result<(), String> {
    remove_worktree_and_branch(repo, worktree, branch)
}

// ---------------------------------------------------------------------------
// GitHub（`gh`）
// ---------------------------------------------------------------------------

/// `origin` リモートがあるか（ADR-0043 D5: `pr` の前提）。
pub fn has_origin(repo: &Path) -> bool {
    git_line(repo, &["remote", "get-url", "origin"]).is_some()
}

/// `git push -u origin <branch>`。
pub fn push_branch(repo: &Path, branch: &str) -> Result<(), String> {
    match git(repo, &["push", "-u", "origin", branch], GIT_WRITE_TIMEOUT) {
        Some(o) if o.ok => Ok(()),
        Some(o) => Err(format!("push に失敗しました: {}", o.why())),
        None => Err("git を起動できませんでした".to_string()),
    }
}

fn gh(gh_path: &str, repo: &Path, args: &[&str], timeout: Duration) -> Option<CmdOutput> {
    let mut cmd = Command::new(gh_path);
    // `auth status` はリポジトリが要らないので、消えた作業場所でも「gh が無い」と誤判定しないように、
    // 実在するディレクトリのときだけ cwd を移す（`pr create` / `pr view` は必ず実在する）。
    if repo.is_dir() {
        cmd.current_dir(repo);
    }
    cmd.env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("NO_COLOR", "1")
        .args(args);
    run(cmd, timeout)
}

static GH_AUTH: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

/// `gh` が PATH にあって認証済みか（ADR-0043 D5: **プロセス内で 60 秒だけ**使い回す）。
pub fn gh_authenticated(gh_path: &str, repo: &Path) -> bool {
    if let Ok(cache) = GH_AUTH.lock()
        && let Some((at, ok)) = *cache
        && at.elapsed() < GH_AUTH_CACHE
    {
        return ok;
    }
    let ok = gh(gh_path, repo, &["auth", "status"], GH_VIEW_TIMEOUT).is_some_and(|o| o.ok);
    if let Ok(mut cache) = GH_AUTH.lock() {
        *cache = Some((Instant::now(), ok));
    }
    ok
}

/// テスト専用: `gh auth status` の記憶を捨てる（偽の `gh` を差し替えるたびに呼ぶ）。
pub fn forget_gh_auth() {
    if let Ok(mut cache) = GH_AUTH.lock() {
        *cache = None;
    }
}

/// 作った PR（URL と番号）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRef {
    pub number: i64,
    pub url: String,
}

/// `gh pr create` の出力から URL と番号を拾う（出力の最後の `https://…/pull/<n>`）。
pub fn parse_pr_url(text: &str) -> Option<PrRef> {
    let mut found: Option<PrRef> = None;
    for token in text.split_whitespace() {
        let token = token.trim_end_matches(['.', ',', ')']);
        if !token.starts_with("https://") {
            continue;
        }
        let Some((_, tail)) = token.split_once("/pull/") else {
            continue;
        };
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        let Ok(number) = digits.parse::<i64>() else {
            continue;
        };
        found = Some(PrRef {
            number,
            url: token.to_string(),
        });
    }
    found
}

/// `gh pr create --base <base> --head <branch> --title … --body …`。
pub fn gh_pr_create(
    gh_path: &str,
    repo: &Path,
    base: &str,
    branch: &str,
    title: &str,
    body: &str,
) -> Result<PrRef, String> {
    let out = gh(
        gh_path,
        repo,
        &[
            "pr", "create", "--base", base, "--head", branch, "--title", title, "--body", body,
        ],
        GH_WRITE_TIMEOUT,
    )
    .ok_or_else(|| format!("{gh_path} を起動できませんでした"))?;
    if !out.ok {
        return Err(format!("gh pr create に失敗しました: {}", out.why()));
    }
    parse_pr_url(&format!("{}\n{}", out.stdout, out.stderr))
        .ok_or_else(|| "gh pr create の出力から PR の URL を読めませんでした".to_string())
}

/// `gh pr view --json state,mergedAt,mergeable,reviewDecision,url` の結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrView {
    pub state: String,
    pub merged_at: Option<String>,
    pub mergeable: Option<String>,
    pub review_decision: Option<String>,
    pub url: Option<String>,
}

/// PR の状態を見る（ADR-0043 D5: 画面を開いたときだけ。10 秒で諦める）。
pub fn gh_pr_view(gh_path: &str, repo: &Path, number: i64) -> Result<PrView, String> {
    let number = number.to_string();
    let out = gh(
        gh_path,
        repo,
        &[
            "pr",
            "view",
            &number,
            "--json",
            "state,mergedAt,mergeable,reviewDecision,url",
        ],
        GH_VIEW_TIMEOUT,
    )
    .ok_or_else(|| format!("{gh_path} を起動できませんでした"))?;
    if !out.ok {
        return Err(format!("gh pr view に失敗しました: {}", out.why()));
    }
    parse_pr_view(&out.stdout)
}

/// `gh pr view --json …` の JSON を読む（知らないキーは無視する）。
pub fn parse_pr_view(text: &str) -> Result<PrView, String> {
    let value: serde_json::Value = serde_json::from_str(text.trim())
        .map_err(|e| format!("gh pr view の JSON を読めませんでした: {e}"))?;
    let string = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Ok(PrView {
        state: string("state").unwrap_or_default(),
        merged_at: string("mergedAt"),
        mergeable: string("mergeable"),
        review_decision: string("reviewDecision"),
        url: string("url"),
    })
}

/// `gh pr merge <n> --<method> --delete-branch`（ADR-0043 D5。`method` は `[github] merge_method`）。
pub fn gh_pr_merge(gh_path: &str, repo: &Path, number: i64, method: &str) -> Result<(), String> {
    let number = number.to_string();
    let flag = format!("--{method}");
    let out = gh(
        gh_path,
        repo,
        &["pr", "merge", &number, &flag, "--delete-branch"],
        GH_WRITE_TIMEOUT,
    )
    .ok_or_else(|| format!("{gh_path} を起動できませんでした"))?;
    if !out.ok {
        return Err(format!("gh pr merge に失敗しました: {}", out.why()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 衝突の解消タスク（ADR-0043 D5）
// ---------------------------------------------------------------------------

/// ADR-0043 D5: rebase が衝突したときに自動で作る「衝突の解消: <題名>」タスク（純粋な組み立て。
/// 保存は呼び出し側）。親と同じ担当（`assignee` / `role` / `genre` / `tier` / 予算）で、
/// **親のブランチの上で**作業する。
///
/// **ADR-0043 D5 からの逸脱（Phase 54 の実装判断。PROGRESS の P54-2）**: 子に `repos` を持たせて
/// `task_repos` に親の worktree を使い回させる（`worktree_of` の目印）のではなく、
/// 子の `workspace` を **親の worktree そのもの + `mode = "shared"`**（ADR-0041 D1 の逃げ道）にする。
/// こうすると既存のディスパッチャがそのまま「そのディレクトリで走る」ので、ワーカーのプロトコルにも
/// `worktree.json` にも新しい概念を足さずに済む。子の cwd は親の `repos/<name>/`、ブランチは
/// 親の `celeris/<parent_id>` のまま。`repos` は**空**にする（空でないと子が自分の worktree を切ってしまう）。
///
/// 受け入れ条件は既存の `Check::Command` だけで書く:
/// 1. 作業ツリーが clean（`status --porcelain` が空）
/// 2. rebase が進行中でない（`rebase-merge` / `rebase-apply` が無い）
/// 3. rebase が済んでいる（`<default_branch>` が `HEAD` の祖先）
pub fn conflict_child_task(
    parent: &task_core::Task,
    repo: &str,
    worktree: &Path,
    default_branch: &str,
    conflicts: &[String],
    now: time::OffsetDateTime,
) -> task_core::Task {
    use task_core::{Check, Criterion, Status, TaskId, WorkspaceMode, WorkspaceSpec};

    let files = if conflicts.is_empty() {
        "（一覧が取れませんでした。作業ツリーの状態を見てください）".to_string()
    } else {
        conflicts
            .iter()
            .map(|f| format!("- `{f}`"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let objective = format!(
        "リポジトリ `{repo}` のブランチを `{default_branch}` に rebase しようとしたところ衝突しました。\n\
         この作業ツリー（`{dir}`。ブランチはこのまま）で rebase をやり直し、衝突を解消して\n\
         完了させてください。コードの意図は元のタスク「{title}」のとおりです。勝手に内容を変えないこと。\n\n\
         衝突したファイル:\n{files}\n\n\
         終わったら人が改めて「{default_branch} に取り込む」を押します。",
        dir = worktree.display(),
        title = parent.title,
    );
    let acceptance = vec![
        Criterion {
            text: "作業ツリーに未コミットの変更が無い".to_string(),
            check: Check::Command {
                cmd: "test -z \"$(git status --porcelain)\"".to_string(),
                expect_exit: 0,
            },
        },
        Criterion {
            text: "rebase が進行中でない".to_string(),
            check: Check::Command {
                cmd: "test ! -e \"$(git rev-parse --git-path rebase-merge)\" && \
                      test ! -e \"$(git rev-parse --git-path rebase-apply)\""
                    .to_string(),
                expect_exit: 0,
            },
        },
        Criterion {
            text: format!("{default_branch} の上に乗っている（rebase が完了している）"),
            check: Check::Command {
                cmd: format!("git merge-base --is-ancestor {default_branch} HEAD"),
                expect_exit: 0,
            },
        },
    ];

    // 親を写してから、子として要るところだけ書き換える（親に列が増えても写し漏れない）。
    let mut child = parent.clone();
    child.id = TaskId::new();
    child.parent_id = Some(parent.id);
    child.title = format!("衝突の解消: {}", parent.title);
    child.objective = objective;
    child.acceptance = acceptance;
    child.inputs = Vec::new();
    child.depends_on = Vec::new();
    child.status = Status::Ready;
    child.aggregate = false;
    child.attempts = 0;
    child.lease = None;
    child.conversation = None;
    child.created_at = now;
    child.updated_at = now;
    // 親の worktree の上で働く（自分の worktree を切らせない）。
    child.repos = Vec::new();
    child.workspace = WorkspaceSpec::Local {
        path: worktree.to_path_buf(),
        mode: Some(WorkspaceMode::Shared),
    };
    child
}

#[cfg(test)]
mod tests;
