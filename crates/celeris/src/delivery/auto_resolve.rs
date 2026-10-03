//! ADR-0137 D1d・D2・D5: `merge_base` 系の配送失敗で局所修復（LLM の repair unit）を作る前に、
//! scratch の worktree で既定ブランチの先端に task branch を merge し、定型の衝突を
//! `task_dispatch::auto_resolve::resolve` で決定的に解く。本番の checkout（`[selfdeploy] repo` の
//! 作業ツリー）は触らず、触るのは task branch の ref だけ。
//!
//! - 解けて、actions が全て追記だけの記録（D5 で再 review 不要）なら、merge commit で task branch を
//!   進め、新しい base/head を固定して `MergeQueued` に戻す（gate は新しい候補 SHA で `start_prepare`
//!   からやり直す。旧 release の gate 結果は使わない）。
//! - それ以外（無効・上限・衝突なしの main 追従のみ・再 review が要る種類・`NeedsHuman`・git の失敗）は
//!   [`Fallback`] を返し、[`fall_back`] が従来経路へ落とす分岐点になる。
use super::{MERGE_TIMEOUT, git_text, git_text_within, sha};
use crate::config::Config;
use std::path::{Path, PathBuf};
use task_core::{Delivery, StoreError, TaskStore};
use task_dispatch::auto_resolve::{
    ConflictKind, IntegrationRequest, Resolution, ResolutionAction, ResolveContext,
};
use time::OffsetDateTime;

/// 解消して task branch を進めたときの comment。試行回数の数え上げにも使う。
pub(super) const RESOLVED_PREFIX: &str = "[delivery-auto-resolved]";
/// 試したが従来経路へ落としたときの comment。試行回数に数える。
pub(super) const FALLBACK_PREFIX: &str = "[delivery-auto-resolve-fallback]";
/// 自動で作る merge commit の作者（人の identity を借りない）。
const AUTHOR: [&str; 4] = [
    "-c",
    "user.name=Celeris",
    "-c",
    "user.email=celeris@localhost",
];

#[derive(Debug)]
pub(super) enum Outcome {
    /// task branch を `head` へ進めた。`base` は merge した既定ブランチの SHA。
    Resolved {
        base: String,
        head: String,
        actions: Vec<ResolutionAction>,
    },
    Fallback(Fallback),
}

/// 従来経路（局所修復・上限なら `[needs-human]`）へ落とす理由。
#[derive(Debug)]
pub(super) enum Fallback {
    Disabled,
    LimitReached {
        attempts: u32,
    },
    /// 衝突なしの main 追従。ADR-0137 D5 で再 review が要るので自動では配送しない。
    NoConflict,
    /// 解けたが、番号の振り直し・生成物の再生成を含む（D5 で再 review が要る）。
    NeedsReview {
        actions: Vec<ResolutionAction>,
    },
    NeedsHuman {
        request: Box<IntegrationRequest>,
    },
    Failed(String),
}

impl Fallback {
    /// 試行として数えるか（実際に merge を試したか）。
    fn attempted(&self) -> bool {
        !matches!(self, Fallback::Disabled | Fallback::LimitReached { .. })
    }

    fn summary(&self) -> String {
        match self {
            Fallback::Disabled => "無効".into(),
            Fallback::LimitReached { attempts } => format!("試行上限 {attempts} 回に到達"),
            Fallback::NoConflict => "衝突なしの main 追従（再 review が必要）".into(),
            Fallback::NeedsReview { actions } => {
                format!("再 review が必要な解消: {}", summarize(actions))
            }
            Fallback::NeedsHuman { request } => format!("人の判断が必要: {}", request.reason),
            Fallback::Failed(e) => format!("失敗: {e}"),
        }
    }
}

/// actions の短い要約（comment 用）。path の順は resolver の出力順。
pub(super) fn summarize(actions: &[ResolutionAction]) -> String {
    if actions.is_empty() {
        return "なし".into();
    }
    actions
        .iter()
        .map(|a| format!("{} ({:?}): {}", a.path, a.kind, a.detail))
        .collect::<Vec<_>>()
        .join("; ")
}

/// この配送で既に試した回数（comment の目印で数える）。
pub(super) fn attempts(store: &dyn TaskStore, d: &Delivery) -> Result<u32, StoreError> {
    Ok(store
        .comments_for(d.task_id)?
        .iter()
        .filter(|c| {
            c.author_kind == task_core::CommentAuthorKind::System
                && (c.body.starts_with(RESOLVED_PREFIX) || c.body.starts_with(FALLBACK_PREFIX))
        })
        .count() as u32)
}

fn comment(
    store: &dyn TaskStore,
    d: &Delivery,
    body: String,
    now: OffsetDateTime,
) -> Result<(), StoreError> {
    store.comment_add(
        &task_core::TaskComment {
            id: task_core::CommentId::new(),
            task_id: d.task_id,
            author_kind: task_core::CommentAuthorKind::System,
            author: None,
            run_id: None,
            created_at: now,
            body,
        },
        None,
    )?;
    Ok(())
}

/// 上限と有効化を確かめてから 1 回試す。git と resolver だけを使い、LLM は呼ばない。
pub(super) fn attempt(
    store: &dyn TaskStore,
    config: &Config,
    d: &Delivery,
) -> Result<Outcome, StoreError> {
    let settings = &config.delivery.auto_resolve;
    if !settings.enabled {
        return Ok(Outcome::Fallback(Fallback::Disabled));
    }
    let tried = attempts(store, d)?;
    if tried >= settings.max_attempts {
        return Ok(Outcome::Fallback(Fallback::LimitReached {
            attempts: tried,
        }));
    }
    Ok(
        match merge_in_scratch(
            &config.selfdeploy.repo,
            d,
            settings.generated.command(),
            &scratch_root(config),
        ) {
            Ok(outcome) => outcome,
            Err(e) => Outcome::Fallback(Fallback::Failed(e)),
        },
    )
}

/// 解消後の配送の記録。新しい SHA を固定し、gate（`start_prepare`）を `MergeQueued` からやり直す。
pub(super) fn requeue(
    store: &dyn TaskStore,
    old: &Delivery,
    base: String,
    head: String,
    actions: &[ResolutionAction],
    now: OffsetDateTime,
) -> Result<(), StoreError> {
    let mut d = old.clone();
    d.base = base;
    d.head = head;
    d.state = task_core::DeliveryState::MergeQueued;
    d.release = None;
    d.prepare_pid = None;
    d.notification = None;
    d.pushed_at = None;
    d.push_error = None;
    d.detail = "既定ブランチとの定型の衝突を自動で解消しました。新しい SHA で取り込みと gate をやり直します".into();
    if store.delivery_save(Some(old), &d)? {
        comment(
            store,
            old,
            format!(
                "{RESOLVED_PREFIX} {} base={} head={}",
                summarize(actions),
                d.base,
                d.head
            ),
            now,
        )?;
    }
    Ok(())
}

/// 従来経路へ落とす分岐点。試行したものは comment に残して回数に数える。`true` を返したら呼び出し側は
/// それ以上進めない（`NeedsHuman` と上限到達の統合の依頼は後続の葉がここで扱う）。今は常に `false`
/// で、呼び出し側は従来どおり局所修復（上限なら `[needs-human]`）へ進む。
pub(super) fn fall_back(
    store: &dyn TaskStore,
    d: &Delivery,
    fallback: &Fallback,
    now: OffsetDateTime,
) -> Result<bool, StoreError> {
    if fallback.attempted() {
        comment(
            store,
            d,
            format!("{FALLBACK_PREFIX} {}", fallback.summary()),
            now,
        )?;
    }
    Ok(false)
}

fn scratch_root(config: &Config) -> PathBuf {
    config
        .selfdeploy
        .releases_dir
        .parent()
        .map(|p| p.join("auto-resolve"))
        .unwrap_or_else(|| std::env::temp_dir().join("celeris-auto-resolve"))
}

/// 一時 worktree。drop で worktree の登録と dir を消す（本番 checkout には触れない）。
struct ScratchWorktree {
    repo: PathBuf,
    dir: PathBuf,
}

impl ScratchWorktree {
    fn add(repo: &Path, root: &Path, d: &Delivery, at: &str) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let dir = root.join(format!("{}-{}", d.task_id, ulid::Ulid::new()));
        let dir_text = dir.to_string_lossy().to_string();
        git_text_within(
            repo,
            &["worktree", "add", "--detach", "--quiet", &dir_text, at],
            MERGE_TIMEOUT,
        )?;
        Ok(Self {
            repo: repo.to_path_buf(),
            dir,
        })
    }
}

impl Drop for ScratchWorktree {
    fn drop(&mut self) {
        let dir = self.dir.to_string_lossy().to_string();
        let _ = git_text(&self.repo, &["worktree", "remove", "--force", &dir]);
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = git_text(&self.repo, &["worktree", "prune"]);
    }
}

fn unmerged(dir: &Path) -> Result<Vec<String>, String> {
    Ok(git_text(dir, &["diff", "--name-only", "--diff-filter=U"])?
        .lines()
        .map(str::to_string)
        .collect())
}

/// ADR-0137 D5 で再 review 不要なのは追記だけの記録の結合だけ。
fn review_free(actions: &[ResolutionAction]) -> bool {
    !actions.is_empty() && actions.iter().all(|a| a.kind == ConflictKind::Record)
}

/// 既定ブランチの先端（target, ours）に task branch（source, theirs）を merge する。D1a の
/// 「base → target の追記 → source の追記」と D1c の「target 側採用」がこの向きで成り立つ。
/// できた merge commit は task branch の子孫なので、task branch は早送りで進む。
fn merge_in_scratch(
    repo: &Path,
    d: &Delivery,
    generated_command: Option<Vec<String>>,
    root: &Path,
) -> Result<Outcome, String> {
    let target_sha = sha(repo, &format!("refs/heads/{}", d.default_branch))?;
    let source_sha = sha(repo, &format!("refs/heads/{}", d.branch))?;
    let merge_base = git_text(repo, &["merge-base", &target_sha, &source_sha]).ok();
    let scratch = ScratchWorktree::add(repo, root, d, &target_sha)?;
    let dir = scratch.dir.as_path();
    let message = format!(
        "Merge {} into {} (delivery auto-resolve)",
        d.default_branch, d.branch
    );
    let mut merge = AUTHOR.to_vec();
    merge.extend([
        "-c",
        "core.hooksPath=/dev/null",
        "merge",
        "--no-ff",
        "--no-progress",
        "-m",
        &message,
        &source_sha,
    ]);
    if git_text_within(dir, &merge, MERGE_TIMEOUT).is_ok() {
        return Ok(Outcome::Fallback(Fallback::NoConflict));
    }
    if unmerged(dir)?.is_empty() {
        return Err("merge が衝突以外の理由で失敗しました".into());
    }
    let ctx = ResolveContext {
        target_branch: d.default_branch.clone(),
        target_sha: target_sha.clone(),
        source_branch: d.branch.clone(),
        source_sha: source_sha.clone(),
        merge_base,
        generated_command,
    };
    let resolution = task_dispatch::auto_resolve::resolve(dir, &ctx);
    let actions = match resolution {
        Ok(Resolution::Resolved { actions }) => actions,
        Ok(Resolution::NeedsHuman { request }) => {
            let _ = git_text(dir, &["merge", "--abort"]);
            return Ok(Outcome::Fallback(Fallback::NeedsHuman { request }));
        }
        Err(e) => {
            let _ = git_text(dir, &["merge", "--abort"]);
            return Err(e);
        }
    };
    if !review_free(&actions) {
        let _ = git_text(dir, &["merge", "--abort"]);
        return Ok(Outcome::Fallback(Fallback::NeedsReview { actions }));
    }
    if !unmerged(dir)?.is_empty() {
        let _ = git_text(dir, &["merge", "--abort"]);
        return Err("resolver の後に未解消の path が残りました".into());
    }
    let mut commit = AUTHOR.to_vec();
    commit.extend(["-c", "core.hooksPath=/dev/null", "commit", "--no-edit"]);
    git_text_within(dir, &commit, MERGE_TIMEOUT)?;
    let head = sha(dir, "HEAD")?;
    advance_branch(repo, &d.branch, &source_sha, &head)?;
    Ok(Outcome::Resolved {
        base: target_sha,
        head,
        actions,
    })
}

/// task branch を `old` から `new` へ進める。branch がどこかの worktree で checkout されていれば
/// そこで早送りし（作業ツリーと index を揃える）、そうでなければ ref を CAS で更新する。
fn advance_branch(repo: &Path, branch: &str, old: &str, new: &str) -> Result<(), String> {
    let full = format!("refs/heads/{branch}");
    let listing = git_text(repo, &["worktree", "list", "--porcelain"])?;
    let mut current: Option<&str> = None;
    for line in listing.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current = Some(path);
        } else if line.strip_prefix("branch ") == Some(full.as_str())
            && let Some(path) = current
        {
            let checkout = Path::new(path);
            if sha(checkout, "HEAD")? != old {
                return Err(format!("{branch} が自動解消の途中で動きました"));
            }
            git_text_within(
                checkout,
                &[
                    "-c",
                    "core.hooksPath=/dev/null",
                    "merge",
                    "--ff-only",
                    "--no-autostash",
                    "--no-progress",
                    new,
                ],
                MERGE_TIMEOUT,
            )?;
            return Ok(());
        }
    }
    git_text(repo, &["update-ref", &full, new, old]).map(|_| ())
}
