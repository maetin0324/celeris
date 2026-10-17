//! 外部入力（case.json の `inputs[]`）の取り込み・照合・各試行の workspace への配送。
//! ADR 2026-10-17-mlsys-pilot2-fixes D1。入力は `materials/` に混ぜない（`materials_sha256` が変わる）。
//! `--inputs-src/<name>` → `<out>/snapshot/inputs/<name>` → 試行の `workspace/inputs/<name>`。
//! 取り込み時と配送時の両方で sha256 を case.json の値と照合する。
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{cases::CaseSet, record::DeliveredInput};

/// 試行の workspace 直下で入力を置く dir 名。
pub const INPUTS_DIR: &str = "inputs";

fn sha256_file(p: &Path) -> Result<String, String> {
    let meta = fs::symlink_metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if !meta.is_file() {
        return Err(format!("{} is not a regular file", p.display()));
    }
    let b = fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(format!("{:x}", Sha256::digest(b)))
}

/// 問題ごとの宣言入力。
pub fn for_problems(set: &CaseSet, problems: &[String]) -> BTreeMap<String, Vec<DeliveredInput>> {
    problems
        .iter()
        .filter_map(|id| {
            let c = set.get(id)?;
            (!c.case.inputs.is_empty()).then(|| {
                (
                    id.clone(),
                    c.case
                        .inputs
                        .iter()
                        .map(|x| DeliveredInput {
                            name: x.name.clone(),
                            sha256: x.sha256.clone(),
                        })
                        .collect(),
                )
            })
        })
        .collect()
}

/// 全問題の入力を（名前で重ねて）1 つの一覧にする。同名で hash が違えば Err。
pub fn unique(map: &BTreeMap<String, Vec<DeliveredInput>>) -> Result<Vec<DeliveredInput>, String> {
    let mut by: BTreeMap<&str, &str> = BTreeMap::new();
    for x in map.values().flatten() {
        if let Some(prev) = by.insert(&x.name, &x.sha256)
            && prev != x.sha256
        {
            return Err(format!(
                "input {:?} is declared with two different sha256 values",
                x.name
            ));
        }
    }
    Ok(by
        .into_iter()
        .map(|(n, s)| DeliveredInput {
            name: n.into(),
            sha256: s.into(),
        })
        .collect())
}

/// `src/<name>` が全て在り、sha256 が宣言と一致するか。不足を 1 件ずつ返す（空 = 揃っている）。
pub fn check_src(src: &Path, map: &BTreeMap<String, Vec<DeliveredInput>>) -> Vec<String> {
    let mut out = Vec::new();
    let all = match unique(map) {
        Ok(a) => a,
        Err(e) => return vec![e],
    };
    for x in all {
        let p = src.join(&x.name);
        match sha256_file(&p) {
            Ok(h) if h == x.sha256 => {}
            Ok(h) => out.push(format!(
                "input {}: sha256 {h} differs from the pinned {}",
                x.name, x.sha256
            )),
            Err(e) => out.push(format!("input {}: {e}", x.name)),
        }
    }
    out
}

/// `src` から `stage` へ取り込む（照合してから複製）。
pub fn stage(
    src: &Path,
    stage: &Path,
    map: &BTreeMap<String, Vec<DeliveredInput>>,
) -> Result<(), String> {
    let errs = check_src(src, map);
    if !errs.is_empty() {
        return Err(errs.join("; "));
    }
    fs::create_dir_all(stage).map_err(|e| format!("{}: {e}", stage.display()))?;
    for x in unique(map)? {
        fs::copy(src.join(&x.name), stage.join(&x.name))
            .map_err(|e| format!("input {}: {e}", x.name))?;
    }
    Ok(())
}

/// 取り込み済みの入力を再照合する（再開時に壊れていないか）。
pub fn verify_stage(
    stage: &Path,
    map: &BTreeMap<String, Vec<DeliveredInput>>,
) -> Result<(), String> {
    let errs = check_src(stage, map);
    if errs.is_empty() {
        Ok(())
    } else {
        Err(format!("saved inputs: {}", errs.join("; ")))
    }
}

/// 試行の workspace に、その問題の入力だけを `inputs/<name>` として置く。置いたあと再照合する。
pub fn deliver(stage: &Path, ws: &Path, specs: &[DeliveredInput]) -> Result<(), String> {
    if specs.is_empty() {
        return Ok(());
    }
    let dir = ws.join(INPUTS_DIR);
    if dir.exists() {
        return Err(format!(
            "workspace already has {INPUTS_DIR}/; it would collide with the delivered inputs"
        ));
    }
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for x in specs {
        let to = dir.join(&x.name);
        fs::copy(stage.join(&x.name), &to).map_err(|e| format!("input {}: {e}", x.name))?;
        if sha256_file(&to)? != x.sha256 {
            return Err(format!("input {} changed while being delivered", x.name));
        }
    }
    Ok(())
}

/// 試行 workspace の作り方（初期木の複製 + 入力の配送）。runner が workspace を作るたびにこれを通る。
pub struct Seeder<'a> {
    pub repo: &'a Path,
    pub stage: PathBuf,
    pub inputs: &'a BTreeMap<String, Vec<DeliveredInput>>,
}

impl Seeder<'_> {
    pub fn specs(&self, problem: &str) -> &[DeliveredInput] {
        self.inputs.get(problem).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn make(&self, problem: &str, ws: &Path) -> Result<(), String> {
        crate::snapshot::copy_tree(self.repo, ws)?;
        deliver(&self.stage, ws, self.specs(problem))
    }
}
