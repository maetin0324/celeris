//! ADR-0040 D6（Phase 48）: `[selfdeploy] releases_dir` を読む／昇格を起こす。
//!
//! ここにあるのは**ディレクトリに対する純粋な関数**だけで、判断（どれを昇格するか）は人が GUI か
//! shell で行う（ADR-0040 D5）。LLM もワーカーも関与しない。
//!
//! 読むもの（`release.sh` / `verify.sh` / `promote.sh` が書いたもの。`docs/selfdeploy.md`）:
//!
//! ```text
//! <releases_dir>/<sha12>/manifest.json   {sha, sha12, ref, built_at, schema_version, ...}
//! <releases_dir>/<sha12>/gate.json       {ok, failed_step, steps: [...]}
//! <releases_dir>/<sha12>/verify.json     {ok, live_ok, at, checks: [...]}   ← 無ければ未検証
//! <releases_dir>/<sha12>/changes.json    {base, commits: [...], files: [...], sensitive: [...]}（ADR-0041 D4）
//! <releases_dir>/<sha12>/promoted.json   {promoted_at, mode, from}（ADR-0041 D3。昇格に成功したときだけ）
//! <releases_dir>/<sha12>/scripts/*.sh    release.sh が同梱した selfdeploy 一式
//! <releases_dir>/<sha12>/promote.lock    昇格中の pid（この模組が書く）
//! <releases_dir>/<sha12>/promote.log     promote.sh の出力
//! <releases_dir>/../current -> releases/<sha12>
//! <releases_dir>/../previous -> releases/<sha12>
//! ```
//!
//! 壊れた JSON・途中で消えたディレクトリでは**落ちない**（その 1 件が `gate_ok = false` と
//! `problem` を持つだけ）。`.build` / `.cargo-target` のような `.` で始まる名前と `*.partial` は飛ばす。
//!
//! ADR-0041 D3 で `[selfdeploy] repo`（人の作業チェックアウト）を**読むだけ**使うようになった:
//! `git -C <repo> merge-base --is-ancestor <sha> main` で `on_main` を出す。git が無い・遅い・
//! リポジトリが無い・その sha を知らない、のどれでも `null` を出すだけで、一覧は落とさない
//! （このモジュールは読むだけ。ADR-0051のdeliveryは別途レビュー済みSHAを取り込む）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use task_api::types::{
    ReleaseChanges, ReleaseCommit, ReleaseGate, ReleaseGateStep, ReleaseItem,
    ReleasePromoteAccepted, ReleasePromoteFailure, ReleaseVerify, ReleaseVerifyCheck,
};
use task_api::{ReleasePromoteError, ReleaseSource, ReleasesFs};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::instance::pid_alive;
use task_worker::detach::DetachLauncher;

/// `git` を待つ上限。一覧の要求の中で走るので、詰まったら諦めて `null` を出す（ADR-0041 D3）。
const GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// `<releases_dir>` を読む `ReleaseSource`（celeris が `ApiSettings` に渡す）。
#[derive(Debug, Clone)]
pub struct FsReleases {
    root: PathBuf,
    /// `[selfdeploy] repo`（作業チェックアウト）。`on_main` を出すためだけに読む。
    repo: PathBuf,
    /// `[selfdeploy] detach`（ADR-0095 付記 D-c）: `promote.sh` の起こし方。`"auto"` / `"systemd-run"` /
    /// `"inline"`。`start_promote` にそのまま渡す。
    detach: String,
}

impl FsReleases {
    pub fn new(root: PathBuf, repo: PathBuf, detach: String) -> Self {
        Self { root, repo, detach }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn repo(&self) -> &Path {
        &self.repo
    }
}

impl ReleaseSource for FsReleases {
    fn list(&self) -> ReleasesFs {
        scan(&self.root, Some(&self.repo))
    }

    /// ADR-0079 R6-4: タイムラインは `on_main` を使わないので git を起こさずに読む（`scan(root, None)`）。
    fn list_for_timeline(&self) -> ReleasesFs {
        scan(&self.root, None)
    }

    fn promote(&self, sha12: &str) -> Result<ReleasePromoteAccepted, ReleasePromoteError> {
        start_promote(&self.root, sha12, &self.detach)
    }

    /// ADR-0044 D5（Phase 53）: タスクのブランチにだけ載っているコミットの sha。
    /// `rev-list --max-count=<N> [<base>..]<branch>` を上限つきで走らせるだけ（読むだけ。
    /// 壊れていても空を返す）。
    fn branch_commits(&self, repo: &Path, branch: &str, base: Option<&str>) -> Vec<String> {
        branch_commits(repo, branch, base)
    }
}

/// ADR-0044 D5: `branch`（`base` があれば `base..branch`）のコミットの sha を新しい順に返す。
/// git が無い・リポジトリが無い・ブランチが無い・時間切れなら空。
fn branch_commits(repo: &Path, branch: &str, base: Option<&str>) -> Vec<String> {
    // ブランチ名・sha に変な文字が混じっていたら走らせない。
    let safe = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "/_-.".contains(c))
    };
    if !repo.is_dir() || !safe(branch) {
        return Vec::new();
    }
    let range = match base.filter(|b| safe(b)) {
        Some(base) => format!("{base}..{branch}"),
        None => branch.to_string(),
    };
    let limit = format!("--max-count={}", task_api::BRANCH_COMMITS_LIMIT);
    let Some(out) = git_output(repo, &["rev-list", &limit, &range]) else {
        return Vec::new();
    };
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// `git_status` と同じ流儀で stdout を取る（失敗・時間切れ・非 0 終了は `None`）。
fn git_output(repo: &Path, args: &[&str]) -> Option<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + GIT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut buf = String::new();
    use std::io::Read;
    child.stdout.as_mut()?.read_to_string(&mut buf).ok()?;
    Some(buf)
}

/// ディレクトリ名として安全で、`git rev-parse --short=12` が出す形か（パストラバーサル防止）。
/// 長さは 7〜40（`--short` の幅を変えても通るように）で、16 進数字だけ。
pub fn valid_sha12(s: &str) -> bool {
    (7..=40).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// `link` が指す先の basename（symlink でなければ `None`）。`current` / `previous` 用。
fn link_target_name(link: &Path) -> Option<String> {
    let dest = std::fs::read_link(link).ok()?;
    let name = dest.file_name()?.to_str()?.to_string();
    (!name.is_empty()).then_some(name)
}

/// `<releases_dir>` を読んで一覧を作る（副作用は読み取りだけ。`repo` は `on_main` のためだけに使う）。
pub fn scan(root: &Path, repo: Option<&Path>) -> ReleasesFs {
    // `current` / `previous` は `releases_dir` の**親**にある（`~/.local/celeris/current -> releases/<sha12>`）。
    let home = root.parent();
    let current = home.and_then(|h| link_target_name(&h.join("current")));
    let previous = home.and_then(|h| link_target_name(&h.join("previous")));

    // ADR-0041 D3: `main` が引けるリポジトリのときだけ `on_main` を出す。1 回で見切りをつけて、
    // リリースごとに `git` を起こす無駄（と、リポジトリが無いときの毎回の失敗）を避ける。
    let git_repo = repo.filter(|r| git_has_main(r));

    let mut items = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        // `releases_dir` がまだ無い（初回）。空の一覧を返す — エラーにはしない。
        return ReleasesFs {
            current,
            previous,
            items,
        };
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        // `.build` / `.cargo-target` と、`release.sh` が組み立て中に使う `<sha12>.partial` は飛ばす。
        if name.starts_with('.') || name.ends_with(".partial") {
            continue;
        }
        if !entry.path().is_dir() {
            continue;
        }
        items.push(read_release(
            &entry.path(),
            &name,
            current.as_deref(),
            previous.as_deref(),
            git_repo,
        ));
    }
    // 新しい順。`built_at` が読めなかったものは最後（同値は sha12 昇順で安定させる）。
    items.sort_by(|a, b| {
        b.built_at
            .cmp(&a.built_at)
            .then_with(|| a.sha12.cmp(&b.sha12))
    });
    ReleasesFs {
        current,
        previous,
        items,
    }
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// `git -C <repo> <args...>` を**上限つき**で走らせる。終了コードを返す（起こせない・時間切れ・
/// シグナルで死んだ、のどれでも `None`）。`GET /releases` の中で走るので、詰まったら諦める。
fn git_status(repo: &Path, args: &[&str]) -> Option<i32> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + GIT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// そのディレクトリが git リポジトリで、`main` という commit を持っているか。
fn git_has_main(repo: &Path) -> bool {
    repo.is_dir()
        && git_status(repo, &["rev-parse", "--verify", "--quiet", "main^{commit}"]) == Some(0)
}

/// ADR-0041 D3: `<sha>` が `main` の祖先か。分からなければ `None`（一覧は落とさない）。
fn on_main(repo: &Path, sha: &str) -> Option<bool> {
    match git_status(repo, &["merge-base", "--is-ancestor", sha, "main"]) {
        Some(0) => Some(true),
        Some(1) => Some(false),
        // 128 = その sha をこのリポジトリが知らない（別のチェックアウトでビルドした等）。
        _ => None,
    }
}

/// ADR-0041 D4: `changes.json` を `ReleaseChanges` に写す。`stale` はここで決める
/// （`base` が**いまの** `current` と違えば、この一覧は「いま昇格したら何が変わるか」ではない）。
fn read_changes(dir: &Path, current: Option<&str>) -> Option<ReleaseChanges> {
    let raw = read_json(&dir.join("changes.json"))?;
    let base = raw
        .get("base")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let files = raw.get("files").and_then(serde_json::Value::as_array);
    let sensitive = raw
        .get("sensitive")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let commits = raw
        .get("commits")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(ReleaseCommit {
                        sha: c.get("sha")?.as_str()?.to_string(),
                        subject: c
                            .get("subject")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(ReleaseChanges {
        stale: base.as_deref() != current,
        base,
        commit_count: commits.len(),
        file_count: files.map(Vec::len).unwrap_or(0),
        sensitive,
        commits,
    })
}

/// ADR-0058: `verify.json` の `checks[]` を `ReleaseVerifyCheck` に写す。無い・壊れているときは
/// 空配列（Phase 94 以前に作られたリリースは `checks` を持たない）。
fn read_verify_checks(verify: &serde_json::Value) -> Vec<ReleaseVerifyCheck> {
    verify
        .get("checks")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(ReleaseVerifyCheck {
                        id: c.get("id")?.as_str()?.to_string(),
                        name: c
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        ok: c
                            .get("ok")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        detail: c
                            .get("detail")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        elapsed_s: c.get("elapsed_s").and_then(serde_json::Value::as_f64),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// ADR-0058: `gate.json` を `ReleaseGate` に写す。`ok` が読めない（壊れている）ときは `None`
/// （`gate_ok` は従来どおり `false` のまま出る。一覧は落ちない）。
fn read_gate(gate: &Option<serde_json::Value>) -> Option<ReleaseGate> {
    let gate = gate.as_ref()?;
    let ok = gate.get("ok")?.as_bool()?;
    // P-94-1: `release.sh` はゲート成功時に `failed_step: ""` を書く。トリムして空なら `None`
    // に正規化する（キー欠落も同じく `None`）。
    let failed_step = gate
        .get("failed_step")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let steps = gate
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    Some(ReleaseGateStep {
                        step: s.get("step")?.as_str()?.to_string(),
                        exit: s
                            .get("exit")
                            .and_then(serde_json::Value::as_i64)
                            .and_then(|v| i32::try_from(v).ok())?,
                        secs: s.get("secs").and_then(serde_json::Value::as_f64)?,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(ReleaseGate {
        ok,
        failed_step,
        steps,
    })
}

fn read_release(
    dir: &Path,
    sha12: &str,
    current: Option<&str>,
    previous: Option<&str>,
    repo: Option<&Path>,
) -> ReleaseItem {
    let manifest = read_json(&dir.join("manifest.json"));
    let gate = read_json(&dir.join("gate.json"));
    let verify = read_json(&dir.join("verify.json"));
    let promoted = read_json(&dir.join("promoted.json"));
    let promote_failed = read_json(&dir.join("promote_failed.json"));

    let mut problems: Vec<&str> = Vec::new();
    if manifest.is_none() {
        problems.push("manifest.json is missing or invalid");
    }
    if gate.is_none() {
        problems.push("gate.json is missing or invalid");
    }

    let field = |v: &Option<serde_json::Value>, key: &str| -> Option<serde_json::Value> {
        v.as_ref().and_then(|m| m.get(key)).cloned()
    };
    let as_string = |v: Option<serde_json::Value>| -> Option<String> {
        v.and_then(|v| v.as_str().map(str::to_string))
    };

    // git に渡すのは完全な sha（`manifest.json`）を優先する。読めなければディレクトリ名（sha12）。
    let full_sha = as_string(field(&manifest, "sha")).unwrap_or_else(|| sha12.to_string());

    ReleaseItem {
        sha12: sha12.to_string(),
        r#ref: as_string(field(&manifest, "ref")),
        built_at: as_string(field(&manifest, "built_at")),
        schema_version: field(&manifest, "schema_version")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok()),
        gate_ok: field(&gate, "ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        gate: read_gate(&gate),
        verify: verify.as_ref().map(|v| ReleaseVerify {
            ok: v
                .get("ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            live_ok: v
                .get("live_ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            at: v
                .get("at")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            checks: read_verify_checks(v),
        }),
        promoted_at: as_string(field(&promoted, "promoted_at")),
        on_main: repo.and_then(|r| on_main(r, &full_sha)),
        changes: read_changes(dir, current),
        is_current: current == Some(sha12),
        is_previous: previous == Some(sha12),
        promoting: promoting_pid(dir).is_some(),
        promote_failed: promote_failed.as_ref().map(|v| ReleasePromoteFailure {
            failed_at: as_string(v.get("failed_at").cloned()).unwrap_or_default(),
            error: as_string(v.get("error").cloned()).unwrap_or_default(),
        }),
        // Phase 105（本番の観測。2026-09-22 21:55 UTC）: `promote.sh` が `setsid` の子として
        // celeris の cgroup に残ったまま、旧デーモンの drain が cgroup ごと巻き添えにして殺した。
        // `promote.lock` の pid が死んでいるのに `promoted.json` が無く、`promote.log` も
        // 「promoted」まで進んでいなければ、途中で殺された（＝止まったまま）と見なせる。
        promote_stale: promote_stale(dir, promoted.is_some()),
        promote_last_line: promote_last_line(dir),
        problem: (!problems.is_empty()).then(|| problems.join("; ")),
    }
}

/// `promote.lock` に書かれた pid（生死は問わない）。読めない・パースできなければ `None`。
fn lock_pid(dir: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(dir.join("promote.lock")).ok()?;
    text.trim().parse().ok()
}

/// `promote.lock` に書かれた pid が**まだ生きていれば** `Some(pid)`。消えていれば `None`
/// （残骸のロックは昇格を塞がない。ADR-0040 D6）。
fn promoting_pid(dir: &Path) -> Option<u32> {
    lock_pid(dir).filter(|&pid| pid_alive(pid))
}

/// `promote.log` の最後の（空でない）行。無ければ `None`。
fn promote_last_line(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("promote.log")).ok()?;
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(str::to_string)
}

/// Phase 105: 昇格が「途中で止まったまま」に見えるか。
///
/// `promote.lock` の pid がまだ生きている（走行中）、`promoted.json` がある（成功した）、
/// そもそも一度も昇格を試みていない（lock が無い）、のどれでもなく、かつ `promote.log` の
/// 最後の行が `promote.sh` の成功時の一行（`sd_log "promoted $SHA12 (mode=$MODE)..."`、
/// `scripts/selfdeploy/promote.sh` 末尾）まで進んでいなければ、`setsid`/scope の子が
/// 途中で殺された（本番 2026-09-22 21:55 UTC の観測）とみなす。
fn promote_stale(dir: &Path, has_promoted_json: bool) -> bool {
    let Some(pid) = lock_pid(dir) else {
        return false;
    };
    if pid_alive(pid) || has_promoted_json {
        return false;
    }
    !promote_last_line(dir).is_some_and(|l| l.contains("promoted"))
}

/// `sh -c` に渡す 1 語を単引用符で囲む。
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Phase 105（ADR-0060 D1 追記）: `promote.sh` を起こす `(exec_program, exec_args)` を組み立てる。純関数。
///
/// - `Inline`: 従来どおり `setsid <script> <sha12>`。
/// - `SystemdRun`: `<runner> --user --scope --quiet --unit celeris-promote-<sha12>-<短い乱数>
///   --description "celeris promote <sha12>" -- sh -c '<script> <sha12>'`。celeris（`celeris@<sha12>`
///   unit）の cgroup の外の scope で `sh -c '<script> <sha12>'` を exec する。`promote.lock` に
///   書く pid は `--scope` が exec するのでそのまま `sh` の pid になる（`ClusterMaster` の master と
///   同じ理屈。ADR-0060 D1 の `crate::detach`）。
fn promote_exec_command(
    launcher: &DetachLauncher,
    script: &Path,
    sha12: &str,
) -> (String, Vec<String>) {
    use task_worker::detach::{scope_unit_name, wrap_command};
    match launcher {
        DetachLauncher::Inline => (
            "setsid".to_string(),
            vec![script.to_string_lossy().into_owned(), sha12.to_string()],
        ),
        DetachLauncher::SystemdRun { .. } => {
            // 末端の `sh -c` に渡す 1 語（この文字列自体、後段でもう一段 `sh_quote` されて
            // 外側の起動用シェルの 1 引数になる。二重引用は正しい: 内側は `sh -c` が、
            // 外側は `promote.sh` を起こす `sh -c` が、それぞれ解釈する）。
            let inner = format!(
                "{} {}",
                sh_quote(&script.to_string_lossy()),
                sh_quote(sha12)
            );
            let unit = scope_unit_name("celeris-promote", sha12);
            let description = format!("celeris promote {sha12}");
            wrap_command(
                launcher,
                "sh",
                &["-c".to_string(), inner],
                &unit,
                &description,
            )
        }
    }
}

/// `promote.sh <sha12>` を detached（既定は `systemd-run --user --scope`。無ければ `setsid` に
/// フォールバック。ADR-0060 D1、Phase 105）、stdin は `/dev/null`、stdout/err は
/// `<release>/promote.log` で起こし、`promote.lock` に pid を書く。どの `promote.sh` かは
/// ADR-0041 D4: **`<current>/scripts/promote.sh`**（無ければ昇格先のもの）。
///
/// **この関数を自動で呼ぶ経路は作らない**（ADR-0040 D5: 昇格は人が押す）。呼ぶのは
/// `POST /releases/{sha12}/promote` だけで、そこは管理系（トークン必須）。
///
/// 昇格は**この celeris 自身を drain させうる**（ADR-0040 D4）。`promote.sh` は celeris の子として
/// 待たない。以前は `setsid` で新しいセッションに切り離すだけだったが、それでも celeris の unit の
/// cgroup には残るため、旧デーモンが drain で cgroup ごと止まると `promote.sh` も巻き添えで死んでいた
/// （本番 2026-09-22 21:55 UTC の観測）。`systemd-run --user --scope` が使える環境では celeris の
/// cgroup の外の scope に起こす（ADR-0060 D1 と同じ判断規則）。
pub fn start_promote(
    root: &Path,
    sha12: &str,
    detach: &str,
) -> Result<ReleasePromoteAccepted, ReleasePromoteError> {
    let launcher = task_worker::detach::resolve_detach_launcher(
        detach,
        task_worker::detach::systemd_run_on_path(),
        task_worker::detach::xdg_runtime_dir_is_set(),
    );
    start_promote_with_launcher(root, sha12, &launcher)
}

/// [`start_promote`] の本体。`launcher` を注入できるのはテストのため
/// （実行環境の `systemd-run`/`XDG_RUNTIME_DIR` の有無に左右されず、常に偽物で確かめる）。
fn start_promote_with_launcher(
    root: &Path,
    sha12: &str,
    launcher: &DetachLauncher,
) -> Result<ReleasePromoteAccepted, ReleasePromoteError> {
    if !valid_sha12(sha12) {
        return Err(ReleasePromoteError::NotFound);
    }
    let dir = root.join(sha12);
    if !dir.is_dir() {
        return Err(ReleasePromoteError::NotFound);
    }

    // 1. 検証済みか（`promote.sh` も同じ判定をするが、409 を早く・はっきり返すためここでも見る）。
    let verify = read_json(&dir.join("verify.json"));
    let verified = verify
        .as_ref()
        .and_then(|v| v.get("ok"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !verified {
        return Err(ReleasePromoteError::NotVerified(match verify {
            None => format!("{sha12} has no verify.json — run scripts/selfdeploy/verify.sh first"),
            Some(_) => {
                format!("verify.json of {sha12} is not ok — there is no --force (ADR-0040 D2)")
            }
        }));
    }

    // 2. 既に current か。
    let current = root
        .parent()
        .and_then(|h| link_target_name(&h.join("current")));
    if current.as_deref() == Some(sha12) {
        return Err(ReleasePromoteError::AlreadyCurrent);
    }

    // 3. 既に昇格中か。
    if promoting_pid(&dir).is_some() {
        return Err(ReleasePromoteError::AlreadyPromoting);
    }

    // 4. どちらの `promote.sh` で昇格するか（ADR-0041 D4）。
    //
    //    **いま動いている版（`current`）のスクリプト**を使う。昇格は「動いている本番を止めて／
    //    引き継いで新しい版に替える」作業で、その手順を知っているべきなのは**いまの本番**の方だから。
    //    実装者が `scripts/selfdeploy/` を壊したリリースを作っても、その壊れた昇格スクリプトが
    //    走ることは無い（新しい昇格スクリプトは、それ自身が一度昇格されてから次の昇格で使われる）。
    //    `current` に `scripts/` が無い（Phase 48 以前のリリース、または初回）ときだけ、
    //    昇格先に同梱された方を使う。どちらを使ったかは応答の `script_from` に出す。
    let target_script = dir.join("scripts").join("promote.sh");
    let current_script = current
        .as_deref()
        .map(|c| root.join(c).join("scripts").join("promote.sh"))
        .filter(|p| p.is_file());
    let (script, script_from) = match current_script {
        Some(script) => (script, "current"),
        None if target_script.is_file() => (target_script, "target"),
        None => {
            return Err(ReleasePromoteError::Unavailable(format!(
                "{} is missing — this release was built before the selfdeploy scripts were bundled, \
                 and the current release does not carry them either",
                target_script.display()
            )));
        }
    };

    // 新しい試みを始めるので、前回の失敗の印は消す（残っていると GUI がいつまでも赤いバナーを出す）。
    // 消せなくても（無い／権限が無い）このまま続ける — 古い失敗が誤って表示され続けるだけで、
    // 昇格そのものを止める理由にはならない。
    let _ = std::fs::remove_file(dir.join("promote_failed.json"));

    let log = dir.join("promote.log");
    let lock = dir.join("promote.lock");
    // `exec_line` を新しいセッション（`setsid`）か celeris の cgroup の外の scope
    // （`systemd-run --user --scope`）に切り離して背景で起こし、pid を lock に書いてから
    // 外側の `sh` は抜ける。`$!` は `exec_line` の先頭プロセスの pid で、
    // `setsid`/`systemd-run --scope` はどちらも（自分がプロセスグループの長でない／`--scope` が
    // exec するので）その場で exec する＝そのまま `promote.sh`（を起こす `sh -c`）の pid になる。
    let (exec_program, exec_args) = promote_exec_command(launcher, &script, sha12);
    let exec_line = std::iter::once(exec_program)
        .chain(exec_args)
        .map(|s| sh_quote(&s))
        .collect::<Vec<_>>()
        .join(" ");
    let command = format!(
        "{exec_line} </dev/null >>{log} 2>&1 & printf '%s\\n' \"$!\" >{lock}",
        log = sh_quote(&log.to_string_lossy()),
        lock = sh_quote(&lock.to_string_lossy()),
    );
    let status = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| ReleasePromoteError::Unavailable(format!("cannot start promote.sh: {e}")))?;
    if !status.success() {
        return Err(ReleasePromoteError::Unavailable(format!(
            "promote.sh could not be started (sh exited with {status})"
        )));
    }

    Ok(ReleasePromoteAccepted {
        sha12: sha12.to_string(),
        log: log.to_string_lossy().into_owned(),
        started_at: OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_else(|_| String::new()),
        script_from: script_from.to_string(),
    })
}

#[cfg(test)]
#[path = "releases/tests.rs"]
mod tests;
