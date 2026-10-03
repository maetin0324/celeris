//! ADR-0131 付記（2026-10-04、日次整理の自動適用）: 適用後の 1 commit と KB の remote への push。
//!
//! daemon が再検証に通った計画を適用した後に呼ぶ部品。判断も LLM も無く `git` を起こすだけ。
//! push はネットワークに出るが LLM 呼び出しではない（配送の `git push` と同じ扱い）。

use super::*;

use crate::changes::git_with_env;

/// 日次整理 1 回の件数（commit の題に書く）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CurationCounts {
    /// 重複の統合。
    pub merged: usize,
    /// 新規ページ。
    pub new: usize,
    /// 削除（retire を含む）。
    pub deleted: usize,
    /// 古い記述の修正。
    pub fixed: usize,
}

/// commit の題: `knowledge curation 2026-10-04: 統合 2・新規 1・削除 0・修正 3`。
pub fn curation_commit_subject(date: &str, counts: CurationCounts) -> String {
    format!(
        "knowledge curation {date}: 統合 {}・新規 {}・削除 {}・修正 {}",
        counts.merged, counts.new, counts.deleted, counts.fixed
    )
}

/// 適用で変えた `paths`（KB 相対。`_curation/YYYY-MM-DD.md`・`index.json`・`README.md` を含めてよい）を
/// 1 commit にまとめる。作者は [`commit_paths`] と同じ設定（`kb::AGENT_AUTHOR_*`）。
///
/// `.gitignore` で除かれた path（`index.json` は派生物で追跡しない）と、作業ツリーにも index にも
/// 無い path は commit の対象から外す。変更が無ければ新しい commit を作らず HEAD を返す。
pub fn commit_curation(
    root: &Path,
    date: &str,
    counts: CurationCounts,
    task_id: &str,
    paths: &[&str],
) -> Result<String, String> {
    let mut kept: Vec<&str> = Vec::new();
    for path in paths {
        if path.is_empty() || kept.contains(path) {
            continue;
        }
        if git(root, &["check-ignore", "-q", "--", path], GIT_TIMEOUT).is_some_and(|o| o.ok) {
            continue;
        }
        let tracked = git(
            root,
            &["ls-files", "--error-unmatch", "--", path],
            GIT_TIMEOUT,
        )
        .is_some_and(|o| o.ok);
        if !tracked && !root.join(path).exists() {
            continue;
        }
        kept.push(path);
    }
    if kept.is_empty() {
        return head(root).ok_or_else(|| "HEAD を読めませんでした".to_string());
    }
    let message = format!(
        "{}\n\ntask: {task_id}\n",
        curation_commit_subject(date, counts)
    );
    commit_paths(
        root,
        &message,
        (kb::AGENT_AUTHOR_NAME, kb::AGENT_AUTHOR_EMAIL),
        &kept,
    )
}

/// [`push_remote`] の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    /// remote が無い（push を省いた。記録だけ残す）。
    NoRemote,
    /// push した（既に最新だった場合も含む）。
    Pushed { remote: String, branch: String },
    /// push できなかった。apply は失敗にせず、報告と event に残して次回の push でまとめて送る。
    Failed(String),
}

fn git_line(root: &Path, args: &[&str]) -> Option<String> {
    git(root, args, GIT_TIMEOUT)
        .filter(|o| o.ok)
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 現在の branch を、その upstream の remote（無ければ `origin`）へ `git push` する。
/// force push はしない。非対話（`GIT_TERMINAL_PROMPT=0`・ssh の `BatchMode=yes`）で、時間の上限がある。
pub fn push_remote(root: &Path) -> PushOutcome {
    let Some(branch) = git_line(root, &["symbolic-ref", "--short", "-q", "HEAD"]) else {
        return PushOutcome::Failed("現在の branch を決められません（detached HEAD）".to_string());
    };
    let remotes: Vec<String> = git_line(root, &["remote"])
        .map(|s| s.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default();
    if remotes.is_empty() {
        return PushOutcome::NoRemote;
    }
    let upstream_remote = git_line(
        root,
        &["config", "--get", &format!("branch.{branch}.remote")],
    )
    .filter(|r| remotes.contains(r));
    let (remote, dest) = match upstream_remote {
        Some(remote) => {
            let merge = git_line(
                root,
                &["config", "--get", &format!("branch.{branch}.merge")],
            )
            .unwrap_or_else(|| format!("refs/heads/{branch}"));
            (remote, merge)
        }
        None if remotes.iter().any(|r| r == "origin") => {
            ("origin".to_string(), format!("refs/heads/{branch}"))
        }
        None => return PushOutcome::NoRemote,
    };
    let ssh_command = match std::env::var("GIT_SSH_COMMAND") {
        Ok(existing) if !existing.trim().is_empty() => format!("{existing} -o BatchMode=yes"),
        _ => "ssh -o BatchMode=yes".to_string(),
    };
    let refspec = format!("refs/heads/{branch}:{dest}");
    match git_with_env(
        root,
        &["push", "-q", &remote, &refspec],
        &[("GIT_SSH_COMMAND", ssh_command.as_str())],
        GIT_WRITE_TIMEOUT,
    ) {
        Some(o) if o.ok => PushOutcome::Pushed { remote, branch },
        Some(o) => PushOutcome::Failed(format!("{remote} への push に失敗: {}", o.why())),
        None => PushOutcome::Failed("git を起動できませんでした".to_string()),
    }
}
