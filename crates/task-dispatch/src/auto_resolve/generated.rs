//! ADR 2026-10-02-parallel-integration-auto-resolve D1c: 生成物の衝突は target 側を採用してから設定のコマンドで再生成する。
//! コマンドは shell を介さず argv で実行し、対象外の変更・失敗・timeout は人に回す。

use super::{
    ClassifiedPath, ConflictKind, IntegrationRequest, Resolution, ResolutionAction, ResolveAttempt,
    ResolveContext, git, recommendation,
};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// `classify::kind` が `Generated` とする範囲。
pub const DEFAULT_GLOBS: &[&str] = &["docs/protocol/*.schema.json", "docs/api/v1/*.schema.json"];

/// 内蔵の `Cargo.lock` 規則が対象にする範囲（repo 内の任意の階層）。
pub const CARGO_LOCK_GLOBS: &[&str] = &["Cargo.lock", "**/Cargo.lock"];

/// 再生成コマンドの既定の上限。
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// 設定から渡る再生成の規則。`globs` は repo 相対で、`*` は `/` を越えず `**` は越える。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedRule {
    pub globs: Vec<String>,
    pub cmd: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// 真なら衝突 path のあるディレクトリで実行し、そこに `Cargo.toml` が無ければ扱わない
    /// （ADR 2026-10-21-cargo-lock-regenerate-on-integrate D2）。
    #[serde(default)]
    pub cargo_workspace: bool,
}

/// `Cargo.lock` を再生成する内蔵規則（ADR 2026-10-21-cargo-lock-regenerate-on-integrate D2）。
pub fn cargo_lock_rule() -> GeneratedRule {
    let mut env = BTreeMap::new();
    // cargo metadata は build しないが、target-dir を作業場所の外に固定する。
    if std::env::var_os("CARGO_TARGET_DIR").is_none() {
        env.insert(
            "CARGO_TARGET_DIR".into(),
            std::env::temp_dir()
                .join("celeris-auto-resolve-target")
                .to_string_lossy()
                .into_owned(),
        );
    }
    GeneratedRule {
        globs: CARGO_LOCK_GLOBS.iter().map(|g| g.to_string()).collect(),
        cmd: ["cargo", "metadata", "--format-version", "1"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        env,
        timeout_secs: None,
        cargo_workspace: true,
    }
}

/// 統合の 3 経路（WU の段・子 task の親・delivery）が渡す規則の集合（D4）。`schema_cmd` は
/// delivery の設定 `[delivery.auto_resolve.generated]` の command（D3）。
pub fn rules(schema_cmd: Option<Vec<String>>) -> Vec<GeneratedRule> {
    let mut rules = vec![cargo_lock_rule()];
    if let Some(cmd) = schema_cmd {
        rules.push(GeneratedRule {
            globs: DEFAULT_GLOBS.iter().map(|g| g.to_string()).collect(),
            cmd,
            env: BTreeMap::new(),
            timeout_secs: None,
            cargo_workspace: false,
        });
    }
    rules
}

impl GeneratedRule {
    pub fn matches(&self, path: &str) -> bool {
        self.globs.iter().any(|glob| glob_match(glob, path))
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS))
    }

    fn display_cmd(&self) -> String {
        self.cmd.join(" ")
    }
}

/// `ResolveContext::generated` のうち衝突 path に一致する最初の規則で再生成する。
pub fn resolve(
    repo: &Path,
    ctx: &ResolveContext,
    path: &ClassifiedPath,
) -> Result<ResolveAttempt, String> {
    let Some(rule) = ctx.generated.iter().find(|rule| rule.matches(&path.path)) else {
        return Ok(ResolveAttempt::NotHandled {
            reason: "生成物の再生成の規則が無い".into(),
        });
    };
    resolve_with_rule(repo, rule, path)
}

/// merge 中の衝突 path を target（HEAD）側に戻し、再生成して対象の差分を index に載せる。
/// commit は呼び出し側の merge commit に含める。
pub fn resolve_with_rule(
    repo: &Path,
    rule: &GeneratedRule,
    path: &ClassifiedPath,
) -> Result<ResolveAttempt, String> {
    if !rule.matches(&path.path) {
        return Ok(ResolveAttempt::NotHandled {
            reason: "生成物の対象範囲外".into(),
        });
    }
    let workdir = match rule_workdir(repo, rule, &path.path) {
        Ok(dir) => dir,
        Err(reason) => return Ok(ResolveAttempt::NotHandled { reason }),
    };
    if let Err(e) = git(repo, &["checkout", "--ours", "--", &path.path]) {
        return Ok(ResolveAttempt::NotHandled {
            reason: format!("target 側を採用できない: {e}"),
        });
    }
    let changed = match regenerate(repo, &workdir, rule) {
        Ok(changed) => changed,
        Err(reason) => return Ok(ResolveAttempt::NotHandled { reason }),
    };
    let mut staged = changed;
    staged.insert(path.path.clone());
    let mut actions = Vec::new();
    for staged_path in &staged {
        git(repo, &["add", "-A", "--", staged_path])?;
        let detail = if *staged_path == path.path {
            format!("target 側を採用し `{}` で再生成", rule.display_cmd())
        } else {
            format!("`{}` の再生成で更新", rule.display_cmd())
        };
        actions.push(ResolutionAction {
            path: staged_path.clone(),
            kind: ConflictKind::Generated,
            detail,
        });
    }
    Ok(ResolveAttempt::Handled { actions })
}

/// 衝突が無くても再生成で差分が出れば対象の path だけを commit する。差分が無ければ
/// 空の actions で `Resolved`。コマンド失敗・timeout・対象外の変更は `NeedsHuman`。
pub fn regenerate_if_drifted(
    repo: &Path,
    ctx: &ResolveContext,
    rule: &GeneratedRule,
) -> Result<Resolution, String> {
    let changed = match regenerate(repo, repo, rule) {
        Ok(changed) => changed,
        Err(reason) => return Ok(needs_human(ctx, reason, Vec::new())),
    };
    if changed.is_empty() {
        return Ok(Resolution::Resolved {
            actions: Vec::new(),
        });
    }
    let paths: Vec<&str> = changed.iter().map(String::as_str).collect();
    let mut add = vec!["add", "-A", "--"];
    add.extend(&paths);
    git(repo, &add)?;
    let message = format!("生成物を再生成: {}", rule.display_cmd());
    let mut commit = vec!["commit", "-q", "-m", &message, "--"];
    commit.extend(&paths);
    git(repo, &commit)?;
    let sha = git(repo, &["rev-parse", "HEAD"])?.trim().to_string();
    let actions = changed
        .into_iter()
        .map(|path| ResolutionAction {
            path,
            kind: ConflictKind::Generated,
            detail: format!("drift を `{}` で再生成し commit {sha}", rule.display_cmd()),
        })
        .collect();
    Ok(Resolution::Resolved { actions })
}

fn needs_human(ctx: &ResolveContext, reason: String, conflict_files: Vec<String>) -> Resolution {
    Resolution::NeedsHuman {
        request: Box::new(IntegrationRequest {
            target_branch: ctx.target_branch.clone(),
            target_sha: ctx.target_sha.clone(),
            source_branch: ctx.source_branch.clone(),
            source_sha: ctx.source_sha.clone(),
            merge_base: ctx.merge_base.clone(),
            conflict_files,
            intent: Vec::new(),
            reason,
            recommendation: recommendation(ConflictKind::Generated).into(),
            actions: Vec::new(),
            candidate_sha: None,
        }),
    }
}

/// 規則を走らせるディレクトリ。`cargo_workspace` の規則は衝突 path の親で、`Cargo.toml` が要る。
fn rule_workdir(repo: &Path, rule: &GeneratedRule, path: &str) -> Result<PathBuf, String> {
    if !rule.cargo_workspace {
        return Ok(repo.to_path_buf());
    }
    let dir = match Path::new(path).parent() {
        Some(parent) => repo.join(parent),
        None => repo.to_path_buf(),
    };
    if !dir.join("Cargo.toml").is_file() {
        return Err(format!("`{path}` と同じディレクトリに Cargo.toml が無い"));
    }
    Ok(dir)
}

/// コマンドを走らせ、変化した path（すべて `globs` の内側）を返す。失敗の理由は `Err`。
fn regenerate(
    repo: &Path,
    workdir: &Path,
    rule: &GeneratedRule,
) -> Result<BTreeSet<String>, String> {
    let before = snapshot(repo)?;
    run(repo, workdir, rule)?;
    let after = snapshot(repo)?;
    let changed: BTreeSet<String> = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect();
    let outside: Vec<&str> = changed
        .iter()
        .filter(|path| !rule.matches(path))
        .map(String::as_str)
        .collect();
    if !outside.is_empty() {
        return Err(format!(
            "再生成コマンドが対象外のファイルを変更した: {}",
            outside.join(", ")
        ));
    }
    Ok(changed)
}

/// index と異なる tracked path と untracked path の内容の hash。消えた path は `None`。
fn snapshot(repo: &Path) -> Result<BTreeMap<String, Option<u64>>, String> {
    let mut out = BTreeMap::new();
    let tracked = git(repo, &["diff", "--name-only", "-z"])?;
    let untracked = git(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in tracked
        .split('\0')
        .chain(untracked.split('\0'))
        .filter(|s| !s.is_empty())
    {
        let hash = std::fs::read(repo.join(path)).ok().map(|bytes| {
            let mut hasher = DefaultHasher::new();
            bytes.hash(&mut hasher);
            hasher.finish()
        });
        out.insert(path.to_string(), hash);
    }
    Ok(out)
}

fn run(repo: &Path, workdir: &Path, rule: &GeneratedRule) -> Result<(), String> {
    let Some((program, args)) = rule.cmd.split_first() else {
        return Err("再生成コマンドが空".into());
    };
    let program_path = PathBuf::from(program);
    let program_path = if program_path.is_relative() && program.contains('/') {
        repo.join(program_path)
    } else {
        program_path
    };
    let mut child = Command::new(&program_path)
        .args(args)
        .envs(&rule.env)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "再生成コマンド `{}` を起動できない: {e}",
                rule.display_cmd()
            )
        })?;
    // stderr の pipe が詰まって子が止まらないよう別 thread で読み切る。
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });
    let deadline = Instant::now() + rule.timeout();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("再生成コマンドの待機に失敗: {e}")),
        }
    };
    let stderr = stderr
        .and_then(|handle| handle.join().ok())
        .map(|buf| tail(&String::from_utf8_lossy(&buf)))
        .unwrap_or_default();
    match status {
        None => Err(format!(
            "再生成コマンド `{}` が {} 秒で終わらない",
            rule.display_cmd(),
            rule.timeout().as_secs()
        )),
        Some(status) if !status.success() => Err(format!(
            "再生成コマンド `{}` が失敗 ({status}): {stderr}",
            rule.display_cmd()
        )),
        Some(_) => Ok(()),
    }
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(5)..].join(" / ")
}

/// `*` は `/` 以外の 0 文字以上、`**` は `/` を含む 0 文字以上、`?` は `/` 以外の 1 文字。
fn glob_match(glob: &str, path: &str) -> bool {
    fn go(g: &[u8], p: &[u8]) -> bool {
        match g.split_first() {
            None => p.is_empty(),
            Some((b'*', rest)) if rest.first() == Some(&b'*') => {
                let rest = &rest[1..];
                (0..=p.len()).any(|i| go(rest, &p[i..]))
            }
            Some((b'*', rest)) => {
                let limit = p.iter().position(|&c| c == b'/').unwrap_or(p.len());
                (0..=limit).any(|i| go(rest, &p[i..]))
            }
            Some((b'?', rest)) => p.first().is_some_and(|&c| c != b'/') && go(rest, &p[1..]),
            Some((c, rest)) => p.first() == Some(c) && go(rest, &p[1..]),
        }
    }
    go(glob.as_bytes(), path.as_bytes())
}

#[cfg(test)]
mod tests;
