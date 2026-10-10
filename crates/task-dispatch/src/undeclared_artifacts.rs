//! ADR-0067 D3: 未申告の成果物を拾う。
//!
//! `claude-code` / `codex` / `acp` アダプタは `result.json` の `summary`/`question`/`evidence` しか読まず、
//! `Event::ArtifactProduced` を出さない（`sink.artifact()` を呼ぶのは `paperqa` / `local-deep-research` /
//! ストリーミングプロトコルのハーネスだけ）。そのため人が読む成果物を `artifacts/` の外に書いても、
//! `GET /tasks/{id}/artifacts` には現れない（本番事故、BenchFS 案件のタスク 01M35X86XTK84F97QW0CN5PGMR）。
//!
//! ここでは run の終端が `Done` / `Question` / `Waiting` のとき（ADR-0067 付記 2026-10-07 D3-a）、
//! - git worktree ではない `local` の作業場所を所有する task は
//!   `artifacts_dir` の外にある `*.md` を走査し（D3）、
//! - すべての task は run の `artifacts_dir` の中を走査して
//!   （ADR-0074 D6.3 / 付記 D3-c）、
//! 共有 workspace（親なしの retry を含む）では元 task や兄弟の成果物を混ぜないため全体走査をしない。
//! まだ登録されていないもの（`(path, sha256)` の組で見る。付記 D3-b）を「未申告の成果物」（`declared: false`）
//! として返す。呼び出し側（`run_worker`、`celerisctl workspace backfill-artifacts`）が
//! `Event::ArtifactProduced` として記録する。LLM 呼び出しは無い（DESIGN 原則 1）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use task_core::{ArtifactRef, Event};
use task_worker::Terminal;

pub mod backfill;

/// 件数の上限（ADR-0067 D3。作業場所全体の `*.md` 走査）。
pub const MAX_FILES: usize = 20;
/// 1 ファイルの上限（ADR-0067 D3。両方の走査で共有）。
pub const MAX_BYTES: u64 = 1024 * 1024;
/// ADR-0067 付記 2026-10-07 D3-c: `artifacts_dir` 走査の 1 回あたりの登録件数の上限。
pub const MAX_FILES_IN_DIR: usize = 64;
/// ADR-0067 付記 2026-10-07 D3-c: `artifacts_dir` 走査で見る深さ（`artifacts_dir` からの相対 path の要素数）。
/// `cmp4/final/report.md` = 3 は入り、job の raw（`cmp4/run2/runs/<job>/cells/<fs>/rep1/w1a/cell.json` = 8）は入らない。
pub const MAX_DEPTH_IN_DIR: usize = 4;
/// 配下を見ないディレクトリ名（ADR-0067 D3）。`.taskd` は celeris の管理用（ADR-0018 D1）で、
/// 共有 workspace では兄弟タスクの `artifacts/`（`.taskd/artifacts/<task_id>/`）もここに入る。
/// 自分の `artifacts_dir` だけでなく `.taskd` 全体を除外しないと、兄弟の成果物まで「未申告」として
/// 拾ってしまう（ADR-0036 の「兄弟の成果物を混ぜない」契約に反する）。
const EXCLUDED_DIRS: &[&str] = &["node_modules", ".venv", "target", ".git", ".taskd"];

/// 登録済みの成果物の鍵（ADR-0067 付記 2026-10-07 D3-b）: `(path, sha256)`。同じ path でも中身が変われば
/// 別の鍵になり、新しい版として登録される。
pub type ArtifactKey = (String, String);

/// 同じ task の `Event::ArtifactProduced` 全履歴（申告済み・未申告どちらも）から登録済みの鍵を作る。
pub fn registered_keys<'a>(events: impl IntoIterator<Item = &'a Event>) -> HashSet<ArtifactKey> {
    events
        .into_iter()
        .filter_map(|ev| match ev {
            Event::ArtifactProduced { artifact, .. } => {
                Some((artifact.path.clone(), artifact.sha256.to_ascii_lowercase()))
            }
            _ => None,
        })
        .collect()
}

fn is_registered(existing: &HashSet<ArtifactKey>, path: &str, sha256: &str) -> bool {
    existing.contains(&(path.to_string(), sha256.to_ascii_lowercase()))
}

/// ADR-0067 付記 2026-10-07 D3-a: run の終端が未申告成果物の走査を要るか。`Done`（完了）・`Question`
/// （判断待ち）・`Waiting`（cluster job 待ち → blocked）は人がその時点の成果物を使うので走る。
/// `Yielded` / `BudgetExhausted` / `Error` は走らない（中間物を一覧に混ぜない。終端で拾う）。
pub fn terminal_wants_scan(terminal: &Terminal) -> bool {
    matches!(
        terminal,
        Terminal::Done { .. } | Terminal::Question { .. } | Terminal::Waiting { .. }
    )
}

/// `workspace_dir` の下にある `*.md` のうち、`artifacts_dir` の外にあり `existing`
/// （`(workspace 相対パス, sha256)`。既に `Event::ArtifactProduced` で記録済みのもの）に無いものを、
/// `declared: false` の `ArtifactRef` として返す（`path` の昇順。上限 [`MAX_FILES`] 件・
/// 1 ファイル [`MAX_BYTES`] 以下・空ファイルは対象外）。
pub fn scan_undeclared_markdown_artifacts(
    workspace_dir: &Path,
    artifacts_dir: &Path,
    existing: &HashSet<ArtifactKey>,
) -> Vec<ArtifactRef> {
    let artifacts_rel = artifacts_dir
        .strip_prefix(workspace_dir)
        .ok()
        .map(Path::to_path_buf);
    let mut found = Vec::new();
    // 相対パス（`workspace_dir` からの）を積むスタック。`PathBuf::new()` はルート自身。
    let mut stack: Vec<PathBuf> = vec![PathBuf::new()];
    while let Some(rel_dir) = stack.pop() {
        if found.len() >= MAX_FILES {
            break;
        }
        if let Some(ar) = &artifacts_rel
            && rel_dir == *ar
        {
            continue; // artifacts_dir 自身の配下は見ない（既に「成果物置き場」）。
        }
        let abs_dir = workspace_dir.join(&rel_dir);
        let Ok(entries) = std::fs::read_dir(&abs_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if found.len() >= MAX_FILES {
                break;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let rel_path = rel_dir.join(&name);
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if EXCLUDED_DIRS.contains(&name_str.as_ref()) {
                    continue;
                }
                stack.push(rel_path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if rel_path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() == 0 || metadata.len() > MAX_BYTES {
                continue;
            }
            let path_str = rel_path.to_string_lossy().replace('\\', "/");
            let Ok(sha256) = task_worker::artifact::sha256_file(&workspace_dir.join(&rel_path))
            else {
                continue;
            };
            if is_registered(existing, &path_str, &sha256) {
                continue;
            }
            found.push(ArtifactRef {
                name: name_str.into_owned(),
                path: path_str,
                sha256,
                kind: "md".to_string(),
                declared: false,
            });
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.truncate(MAX_FILES);
    found
}

/// ADR-0074 D6.3（Phase F1 (j)）/ ADR-0067 付記 2026-10-07 D3-c: 人が読む成果物の拡張子
/// （`artifacts_dir` 走査の対象）。並びがそのまま優先順（上限に当たるとき文書が先に残る）。
pub const HUMAN_ARTIFACT_EXTENSIONS: &[&str] = &["md", "html", "pdf", "csv", "png", "svg", "json"];

/// ADR-0074 D6.3 / ADR-0067 付記 D3-c: 機械的なファイル（celeris 自身が書く、成果物ではないもの）。走査対象から
/// 外す（`execution-plan*.json`/`project-plan*.json` は前方一致、`phase-reports/` はディレクトリごと除く）。
const EXCLUDED_ARTIFACT_FILENAMES: &[&str] = &[
    "checkpoint.json",
    "result.json",
    "request.json",
    "prompt.txt",
    task_worker::delegate_file::DELEGATE_FILE_NAME,
    task_ops::followup::FOLLOWUPS_FILE_NAME,
    crate::review::PLAN_FILE_NAME,
    crate::review::REVIEW_FILE_NAME,
    "knowledge-candidates.json",
];

fn is_excluded_artifact_filename(name: &str) -> bool {
    EXCLUDED_ARTIFACT_FILENAMES.contains(&name)
        || (name.starts_with("execution-plan") && name.ends_with(".json"))
        || (name.starts_with("project-plan") && name.ends_with(".json"))
}

/// 走査の候補（sha256 はまだ計算していない）。並べ替えの鍵は（拡張子のクラス → 深さ → path）。
struct Candidate {
    class: usize,
    depth: usize,
    rel_path: PathBuf,
    name: String,
    kind: String,
}

/// ADR-0074 D6.3（Phase F1 (j)）/ ADR-0067 付記 2026-10-07 D3-c: git worktree の Task と remote の Task の
/// 未申告成果物の走査。ADR-0067 D3 の走査（[`scan_undeclared_markdown_artifacts`]）はリポジトリ全体を見るので
/// worktree / remote の写しでは使えない（無関係なファイルまで拾ってしまう）。ここでは run の**成果物
/// ディレクトリの中だけ**を見て、`HUMAN_ARTIFACT_EXTENSIONS` のうち深さ [`MAX_DEPTH_IN_DIR`] までにあり
/// `existing`（`(path, sha256)`）に無いものを（拡張子のクラス → 深さ → path の順で、最大
/// [`MAX_FILES_IN_DIR`] 件）返す。`path` は `workspace_dir` からの相対。
pub fn scan_undeclared_artifacts_in_dir(
    workspace_dir: &Path,
    artifacts_dir: &Path,
    existing: &HashSet<ArtifactKey>,
) -> Vec<ArtifactRef> {
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![PathBuf::new()];
    while let Some(rel_dir) = stack.pop() {
        // D2.3（将来の phase-reports/。今はまだ発生しないが、あらかじめ除く）。
        if rel_dir.file_name().and_then(|n| n.to_str()) == Some("phase-reports") {
            continue;
        }
        let dir_depth = rel_dir.components().count();
        let abs_dir = artifacts_dir.join(&rel_dir);
        let Ok(entries) = std::fs::read_dir(&abs_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let rel_path = rel_dir.join(&name);
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if EXCLUDED_DIRS.contains(&name_str.as_ref()) || name_str == "phase-reports" {
                    continue;
                }
                // この dir の直下の file の深さは `dir_depth + 2`。上限を超えるなら潜らない。
                if dir_depth + 2 > MAX_DEPTH_IN_DIR {
                    continue;
                }
                stack.push(rel_path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if is_excluded_artifact_filename(&name_str) {
                continue;
            }
            let Some(class) = rel_path
                .extension()
                .and_then(|e| e.to_str())
                .and_then(|e| HUMAN_ARTIFACT_EXTENSIONS.iter().position(|x| *x == e))
            else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.len() == 0 || metadata.len() > MAX_BYTES {
                continue;
            }
            candidates.push(Candidate {
                class,
                depth: dir_depth + 1,
                kind: HUMAN_ARTIFACT_EXTENSIONS[class].to_string(),
                name: name_str.into_owned(),
                rel_path,
            });
        }
    }
    candidates
        .sort_by(|a, b| (a.class, a.depth, &a.rel_path).cmp(&(b.class, b.depth, &b.rel_path)));
    let mut found = Vec::new();
    for candidate in candidates {
        if found.len() >= MAX_FILES_IN_DIR {
            break;
        }
        let abs_path = artifacts_dir.join(&candidate.rel_path);
        let path_str = abs_path
            .strip_prefix(workspace_dir)
            .unwrap_or(&abs_path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(sha256) = task_worker::artifact::sha256_file(&abs_path) else {
            continue;
        };
        if is_registered(existing, &path_str, &sha256) {
            continue;
        }
        found.push(ArtifactRef {
            name: candidate.name,
            path: path_str,
            sha256,
            kind: candidate.kind,
            declared: false,
        });
    }
    found
}

#[cfg(test)]
mod tests;
