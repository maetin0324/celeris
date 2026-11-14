//! ADR 2026-11-13-health-base-merged-target: WU の check と段の統合の health の base を決める 1 つの関数
//! [`health_base`]（D1・D2・D3.5・D4・D5）。
//!
//! head が review target の commit T を start より多く含むとき、base は start と T の決定的な merge にする。
//! 取り込みが無ければ start のまま（K3）。base の tree は start・T・git の merge だけで決まり、worker の commit は入らない
//! （K1・I3）。target・git・merge を確かめられなければ `Err`（呼び出し側が Unavailable の Error にし、合格にしない。K4）。
//! 衝突した merge は [`HealthBase::against_target`] に T を返し、呼び出し側が base = T の 2 回目の検査を足す（D3）。
//! git を起こすだけで、ref・index・作業ツリーは変えない。LLM は呼ばない。

use std::path::Path;
use std::process::Command;

use task_core::required_checks::health::{CHECKER_DIR, CHECKER_SIDE_FILES};

/// [`health_base`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HealthBase {
    /// health の base（commit の sha）。start・T・start と T の merge の commit のどれか。
    pub(crate) sha: String,
    /// merge が衝突したときの T（2 回目の検査の base。D3）。衝突が無ければ `None`。
    pub(crate) against_target: Option<String>,
}

impl HealthBase {
    fn plain(sha: impl Into<String>) -> Self {
        Self {
            sha: sha.into(),
            against_target: None,
        }
    }
}

/// `dir` で git を実行する。`ok` に含まれる exit code なら `(code, stdout の trim)`、それ以外・起動失敗は `Err`。
fn git(
    dir: &Path,
    args: &[&str],
    envs: &[(&str, String)],
    ok: &[i32],
) -> Result<(i32, String), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(envs.iter().map(|(k, v)| (*k, v.as_str())))
        .output()
        .map_err(|e| format!("git {} を起動できない: {e}", args.join(" ")))?;
    match out.status.code() {
        Some(code) if ok.contains(&code) => Ok((
            code,
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )),
        code => Err(format!(
            "git {} が失敗した（exit {code:?}）: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

fn is_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn commit_of(dir: &Path, rev: &str) -> Result<String, String> {
    let spec = format!("{rev}^{{commit}}");
    let (_, sha) = git(dir, &["rev-parse", "--verify", &spec], &[], &[0])?;
    if is_sha(&sha) {
        Ok(sha)
    } else {
        Err(format!("{rev} を commit に解決できない"))
    }
}

fn is_ancestor(dir: &Path, ancestor: &str, of: &str) -> Result<bool, String> {
    let (code, _) = git(
        dir,
        &["merge-base", "--is-ancestor", ancestor, of],
        &[],
        &[0, 1],
    )?;
    Ok(code == 0)
}

fn checker_conflicts(conflicted: &[String]) -> Vec<&str> {
    conflicted
        .iter()
        .map(String::as_str)
        .filter(|p| {
            p.strip_prefix(CHECKER_DIR)
                .is_some_and(|rest| rest.starts_with('/'))
                || CHECKER_SIDE_FILES.contains(p)
        })
        .collect()
}

/// D1・D2・D4: `dir` の git の repo で、評価する `head`・取り込みが無いときの base `start`・review target `target`
/// （`Ok(None)` は案件の repo の参照が無い作業場所で、取り込みは無い）から health の base を決める。
/// 同じ (start, head の祖先の T, target) からは同じ sha ができる（K5）。
pub(crate) fn health_base(
    dir: &Path,
    head: &str,
    start: &str,
    target: &Result<Option<String>, String>,
) -> Result<HealthBase, String> {
    let fail = |why: String| format!("health_base: {why}");
    let target = match target {
        Err(why) => return Err(fail(why.clone())),
        Ok(None) => return Ok(HealthBase::plain(start)),
        Ok(Some(target)) => target,
    };
    resolve(dir, head, start, target).map_err(fail)
}

fn resolve(dir: &Path, head: &str, start: &str, target: &str) -> Result<HealthBase, String> {
    let head = commit_of(dir, head)?;
    let start = commit_of(dir, start)?;
    let (code, bases) = git(dir, &["merge-base", "--all", &head, target], &[], &[0, 1])?;
    if code == 1 {
        return Ok(HealthBase::plain(start));
    }
    let mut taken = Vec::new();
    for t in bases.lines().filter(|l| !l.is_empty()) {
        if !is_ancestor(dir, t, &start)? {
            taken.push(t.to_string());
        }
    }
    let t = match taken.as_slice() {
        [] => return Ok(HealthBase::plain(start)),
        [t] => t.clone(),
        _ => return Err("merge-base が複数ある（T を 1 つに決められない）".to_string()),
    };
    if is_ancestor(dir, &start, &t)? {
        return Ok(HealthBase::plain(t));
    }
    let (code, out) = git(
        dir,
        &[
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            &start,
            &t,
        ],
        &[],
        &[0, 1],
    )?;
    let mut lines = out.lines().filter(|l| !l.is_empty());
    let tree = lines
        .next()
        .filter(|l| is_sha(l))
        .ok_or("merge-tree の出力に tree が無い")?;
    let mut conflicted: Vec<String> = lines.map(str::to_string).collect();
    conflicted.sort();
    conflicted.dedup();
    if code == 1 && conflicted.is_empty() {
        return Err("merge-tree が衝突を報告したが path が読めない".to_string());
    }
    let on_checker = checker_conflicts(&conflicted);
    if !on_checker.is_empty() {
        return Err(format!(
            "検査器の file が衝突した: {}",
            on_checker.join(",")
        ));
    }
    let sha = commit_merge(dir, tree, &start, &t)?;
    Ok(HealthBase {
        sha,
        against_target: (code == 1).then_some(t),
    })
}

/// D4: 親が start・T、作者と日付と本文が固定の commit（host の git の設定に依らない）。
fn commit_merge(dir: &Path, tree: &str, start: &str, t: &str) -> Result<String, String> {
    let mut date = 0u64;
    for sha in [start, t] {
        let (_, ct) = git(dir, &["show", "-s", "--format=%ct", sha], &[], &[0])?;
        date = date.max(
            ct.parse::<u64>()
                .map_err(|e| format!("committer date を読めない: {e}"))?,
        );
    }
    let when = format!("{date} +0000");
    let envs = [
        ("GIT_AUTHOR_NAME", "celeris-health-base".to_string()),
        (
            "GIT_AUTHOR_EMAIL",
            "health-base@celeris.invalid".to_string(),
        ),
        ("GIT_AUTHOR_DATE", when.clone()),
        ("GIT_COMMITTER_NAME", "celeris-health-base".to_string()),
        (
            "GIT_COMMITTER_EMAIL",
            "health-base@celeris.invalid".to_string(),
        ),
        ("GIT_COMMITTER_DATE", when),
    ];
    let message = format!("celeris health base\nstart {start}\ntarget {t}");
    let (_, sha) = git(
        dir,
        &[
            "commit-tree",
            tree,
            "-p",
            start,
            "-p",
            t,
            "--no-gpg-sign",
            "-m",
            &message,
        ],
        &envs,
        &[0],
    )?;
    if is_sha(&sha) {
        Ok(sha)
    } else {
        Err("commit-tree の出力が commit の sha でない".to_string())
    }
}
