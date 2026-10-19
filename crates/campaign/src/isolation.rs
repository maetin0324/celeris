//! 試行ごとの隔離 workspace 生成と漏れ検査。
//! A/B/C に同じ初期 snapshot・資料・KB・資源上限を与え、試行間で KB・解答を見せない。
//! final の期待値・欠陥 list・grader の pristine テストは `withheld_names` で workspace から除く。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    manifest::{Budget, ConfigId},
    snapshot::{copy_inner_symlink, hash_tree},
};

/// 初期情報の取り込み元と、workspace へ入れてはならない名前。
#[derive(Clone, Debug)]
pub struct InitialSources {
    pub repo: PathBuf,
    pub materials: PathBuf,
    pub kb: PathBuf,
    pub budget: Budget,
    /// この名前の file / dir は（どの深さでも）複製しない。
    pub withheld_names: Vec<String>,
}

/// config 間で一致すべき初期入力の hash 記録。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputRecord {
    pub config: ConfigId,
    pub problem: String,
    pub attempt: u32,
    pub repo_sha256: String,
    pub materials_sha256: String,
    pub kb_sha256: String,
    pub budget_sha256: String,
}

impl InputRecord {
    fn inputs(&self) -> (&str, &str, &str, &str) {
        (
            &self.repo_sha256,
            &self.materials_sha256,
            &self.kb_sha256,
            &self.budget_sha256,
        )
    }
}

#[derive(Clone, Debug)]
pub struct TrialDirs {
    pub id: String,
    pub root: PathBuf,
    pub workspace: PathBuf,
    pub materials: PathBuf,
    pub kb: PathBuf,
    pub record: InputRecord,
}

/// 漏れ検査の対象。`owned_hashes`（試行固有の final hash 等。初期 hash は全 config 共通なので不可）/ `final_values` はこの試行が持つ秘密（他試行に出てはならない）。
#[derive(Clone, Debug, Default)]
pub struct TrialView {
    pub id: String,
    pub root: PathBuf,
    pub owned_hashes: Vec<String>,
    pub final_values: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leak {
    pub in_trial: String,
    pub file: String,
    pub what: String,
}

fn budget_hash(b: &Budget) -> Result<String, String> {
    let v = serde_json::to_vec(b).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(v)))
}

fn copy_filtered(root: &Path, src: &Path, dst: &Path, withheld: &[String]) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let mut es: Vec<PathBuf> = fs::read_dir(src)
        .map_err(|e| format!("{}: {e}", src.display()))?
        .map(|e| e.map(|e| e.path()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    es.sort();
    for p in es {
        let name = p
            .file_name()
            .ok_or("path without file name")?
            .to_string_lossy()
            .into_owned();
        if withheld.contains(&name) {
            continue;
        }
        let meta = fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        let to = dst.join(&name);
        if meta.file_type().is_symlink() {
            copy_inner_symlink(root, &p, &to)?;
        } else if meta.is_dir() {
            copy_filtered(root, &p, &to, withheld)?;
        } else {
            fs::copy(&p, &to).map_err(|e| format!("{}: {e}", p.display()))?;
        }
    }
    Ok(())
}

/// 試行 1 つ分の隔離 dir を `<out>/<config>/<problem>/<attempt>/` に作る。
/// 既存の残骸は捨て、必ず初期 snapshot から作り直す。
pub fn prepare_trial(
    src: &InitialSources,
    out: &Path,
    config: ConfigId,
    problem: &str,
    attempt: u32,
) -> Result<TrialDirs, String> {
    if problem.is_empty() || problem.contains(['/', '\\']) || problem.starts_with('.') {
        return Err(format!("invalid problem id: {problem:?}"));
    }
    let id = format!("{config:?}/{problem}/{attempt}");
    let root = out
        .join(format!("{config:?}"))
        .join(problem)
        .join(attempt.to_string());
    let _ = fs::remove_dir_all(&root);
    let (workspace, materials, kb) = (
        root.join("workspace"),
        root.join("materials"),
        root.join("kb"),
    );
    for (s, d) in [
        (&src.repo, &workspace),
        (&src.materials, &materials),
        (&src.kb, &kb),
    ] {
        copy_filtered(s, s, d, &src.withheld_names)?;
    }
    let record = InputRecord {
        config,
        problem: problem.into(),
        attempt,
        repo_sha256: hash_tree(&workspace)?,
        materials_sha256: hash_tree(&materials)?,
        kb_sha256: hash_tree(&kb)?,
        budget_sha256: budget_hash(&src.budget)?,
    };
    let json = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
    fs::write(root.join("inputs.json"), json).map_err(|e| e.to_string())?;
    Ok(TrialDirs {
        id,
        root,
        workspace,
        materials,
        kb,
        record,
    })
}

/// 同じ problem・attempt の config 間で初期入力 hash が一致することを確かめる。
pub fn check_same_inputs(records: &[InputRecord]) -> Result<(), String> {
    let Some(first) = records.first() else {
        return Err("no input records".into());
    };
    for r in records {
        if r.problem != first.problem || r.attempt != first.attempt {
            return Err(format!(
                "records are for different trials: {}#{} vs {}#{}",
                first.problem, first.attempt, r.problem, r.attempt
            ));
        }
        if r.inputs() != first.inputs() {
            return Err(format!(
                "initial inputs differ between {:?} and {:?}",
                first.config, r.config
            ));
        }
    }
    Ok(())
}

/// 試行の終了: 書き込み可能な KB の最終 hash を返して KB を破棄する。
pub fn finish_trial(t: &TrialDirs) -> Result<String, String> {
    let h = hash_tree(&t.kb)?;
    fs::remove_dir_all(&t.kb).map_err(|e| format!("{}: {e}", t.kb.display()))?;
    Ok(h)
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for e in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        let meta = fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            files(&p, out)?;
        } else if meta.is_file() {
            out.push(p);
        }
    }
    Ok(())
}

/// 各試行の dir を走査し、他試行の path・hash、および全試行の final 値を含む file を報告する。
/// 試行自身の `inputs.json`（自分の記録）は対象外。
pub fn verify_isolation(trials: &[TrialView]) -> Result<Vec<Leak>, String> {
    let mut leaks = Vec::new();
    for t in trials {
        let mut fs_ = Vec::new();
        if t.root.exists() {
            files(&t.root, &mut fs_)?;
        }
        for f in fs_ {
            let text =
                String::from_utf8_lossy(&fs::read(&f).map_err(|e| e.to_string())?).into_owned();
            let rel = f
                .strip_prefix(&t.root)
                .unwrap_or(&f)
                .to_string_lossy()
                .into_owned();
            let mut hit = |what: String| {
                leaks.push(Leak {
                    in_trial: t.id.clone(),
                    file: rel.clone(),
                    what,
                });
            };
            for o in trials {
                let other = o.id != t.id;
                if other {
                    let p = o.root.to_string_lossy();
                    if !p.is_empty() && text.contains(p.as_ref()) {
                        hit(format!("path of trial {}", o.id));
                    }
                    for h in o.owned_hashes.iter().filter(|h| !h.is_empty()) {
                        if text.contains(h.as_str()) {
                            hit(format!("hash of trial {}", o.id));
                        }
                    }
                }
                for v in o.final_values.iter().filter(|v| !v.is_empty()) {
                    if text.contains(v.as_str()) {
                        hit(format!("final value of trial {}", o.id));
                    }
                }
            }
        }
    }
    Ok(leaks)
}

/// canary 検査の結果。`present_forbidden`: 試行の木に在った禁止 canary（file 付き）、`missing_required`: 試行用 KB に無かった必須 canary。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CanaryReport {
    pub present_forbidden: Vec<(String, String)>,
    pub missing_required: Vec<String>,
}

/// 試行の木（`kb/`・`materials/`・`output` を含む root）を走査する。禁止 canary（通常 KB・他試行・grader 私有領域に置いた
/// 目印）が 1 つでも在れば汚染、`kb_required` は `<root>/kb` の中に全て在ること（試行用 KB が実際に配られている証拠）。
pub fn scan_canaries(
    root: &Path,
    forbidden: &[String],
    required: &[String],
) -> Result<CanaryReport, String> {
    let mut all = Vec::new();
    files(root, &mut all)?;
    let mut rep = CanaryReport::default();
    let mut seen_required = vec![false; required.len()];
    for f in all {
        let text = String::from_utf8_lossy(&fs::read(&f).map_err(|e| e.to_string())?).into_owned();
        let rel = f.strip_prefix(root).unwrap_or(&f);
        for c in forbidden.iter().filter(|c| text.contains(c.as_str())) {
            rep.present_forbidden
                .push((rel.to_string_lossy().into_owned(), c.clone()));
        }
        if rel.starts_with("kb") {
            for (i, c) in required.iter().enumerate() {
                seen_required[i] |= text.contains(c.as_str());
            }
        }
    }
    rep.missing_required = required
        .iter()
        .zip(seen_required)
        .filter(|(_, s)| !s)
        .map(|(c, _)| c.clone())
        .collect();
    Ok(rep)
}

/// A/B/C の試行木を初期 snapshot から作り（`scratch` の下。呼び出し側が消す）、(2) kb/ が同一 hash、
/// (3) canary が禁止側に無く必須側に在ることを確かめる。不足の一覧を返す。
pub fn check_trial_context(
    src: &InitialSources,
    scratch: &Path,
    forbidden: &[String],
    required: &[String],
) -> Result<Vec<String>, String> {
    let mut miss = Vec::new();
    let mut records = Vec::new();
    for config in [ConfigId::A, ConfigId::B, ConfigId::C] {
        let t = prepare_trial(src, scratch, config, "preflight", 1)?;
        let rep = scan_canaries(&t.root, forbidden, required)?;
        for (file, c) in rep.present_forbidden {
            miss.push(format!(
                "{config:?}: forbidden canary {c:?} is in the trial tree ({file})"
            ));
        }
        for c in rep.missing_required {
            miss.push(format!(
                "{config:?}: required canary {c:?} is not in the trial KB (kb/)"
            ));
        }
        records.push(t.record);
    }
    if let Err(e) = check_same_inputs(&records) {
        miss.push(format!("trial kb/ is not identical across A/B/C: {e}"));
    }
    Ok(miss)
}
