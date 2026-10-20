//! 実行・再開・監視を campaign lock に接続する（ADR 2026-10-17-campaign-lock D8）。
//! 期待 lock は case.json・`initial.lock.json`・grade-case の宣言から作る（preflight・CLI・ライブラリ経路で同じ関数）。
//! 実体（repo / 資料 / KB / 外部入力 / 公開・隠しテスト）は executor 投入・task 作成・台帳更新より前に照合し、
//! 取り込んだ後の snapshot と workspace-seed も同じ期待値で照合する。保存 lock は `<out>/lock.json`。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    cases::{CaseKind, CaseSet},
    grader::{GradeCase, sha256_file},
    inputs,
    lock::{CampaignLock, LockedCase, LockedInitial, verify_lock},
    lock_tree,
    manifest::CampaignManifest,
    record::DeliveredInput,
    snapshot::hash_tree,
};

/// 保存 lock の位置（out 直下）。
pub const LOCK_FILE: &str = "lock.json";
pub const LOCK_VERSION: &str = "1";
/// 初期木の内容証明（case ごとの directory に置く）。
/// 初期木を持たない case 用の seed（`materials/` と `kb/` だけ）。
pub const EMPTY_SEED_DIR: &str = "workspace-seed-empty";
pub const INITIAL_LOCK_FILE: &str = "initial.lock.json";

/// 期待 lock を作るための入力（preflight の `Inputs` と同じ root）。
pub struct LockSpec<'a> {
    pub cases_root: &'a Path,
    pub grader_root: &'a Path,
    pub grade_cases_root: &'a Path,
    /// 省略時は `<campaign_id>-<campaign hash の先頭 12 桁>`。内容を変えた新しい測定は新しい ID で別の out に作る。
    pub measurement_id: Option<&'a str>,
}

/// 現在指定された取り込み元。
#[derive(Clone, Copy, Default)]
pub struct Sources<'a> {
    pub repo: Option<&'a Path>,
    pub materials: Option<&'a Path>,
    pub kb: Option<&'a Path>,
    pub inputs: Option<&'a Path>,
}

fn rel_join(root: &Path, rel: &str) -> Result<PathBuf, String> {
    use std::path::Component;
    if rel.is_empty()
        || !Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(format!("unsafe path {rel:?}"));
    }
    Ok(root.join(rel))
}

/// 初期木を宣言しない case（初期木のある campaign の中で、repo を渡してはならない case）。
pub fn repo_free_cases(l: &CampaignLock) -> BTreeSet<String> {
    if l.cases.iter().any(|c| c.initial.is_some()) {
        l.cases
            .iter()
            .filter(|c| c.initial.is_none())
            .map(|c| c.id.clone())
            .collect()
    } else {
        BTreeSet::new()
    }
}

/// 初期木を持つ case が共有する初期木。異なる初期木の case を 1 つの `--repo-src` に載せない。
/// 初期木を持たない case には repo を配らない（`repo_free_cases`、別の空 seed を使う）。
pub fn shared_initial(l: &CampaignLock) -> Result<Option<&LockedInitial>, String> {
    let mut first: Option<&LockedInitial> = None;
    let mut with = 0usize;
    for c in &l.cases {
        if let Some(i) = &c.initial {
            with += 1;
            match first {
                None => first = Some(i),
                Some(f) if f == i => {}
                Some(_) => {
                    return Err(format!(
                        "case {} pins a different initial tree; one --repo-src cannot serve cases with different initial trees (split them into separate campaigns)",
                        c.id
                    ));
                }
            }
        }
    }
    let _ = with;
    Ok(first)
}

/// lock に固定された、問題ごとの外部入力。
pub fn locked_inputs(l: &CampaignLock) -> BTreeMap<String, Vec<DeliveredInput>> {
    l.cases
        .iter()
        .filter(|c| !c.inputs.is_empty())
        .map(|c| (c.id.clone(), c.inputs.clone()))
        .collect()
}

/// 宣言から期待 lock を作る。grade-case・公開/隠しテストの実体が宣言 hash と違えばここで Err。
pub fn expected_lock(spec: &LockSpec, m: &CampaignManifest) -> Result<CampaignLock, String> {
    let set = CaseSet::load(spec.cases_root, spec.grader_root)?;
    let mut cases = Vec::new();
    for id in &m.problems {
        let c = set
            .get(id)
            .ok_or_else(|| format!("{id}: no case.json under cases root"))?;
        let case = &c.case;
        if case.kind == CaseKind::Control {
            return Err(format!("{id}: control case in a measurement"));
        }
        if case.split != m.split {
            return Err(format!(
                "{id}: case split {:?} != campaign {:?}",
                case.split, m.split
            ));
        }
        let r = case
            .grade_case_ref
            .as_ref()
            .ok_or_else(|| format!("{id}: no grade_case_ref"))?;
        let gpath = rel_join(spec.grade_cases_root, &r.path)?;
        let got = sha256_file(&gpath)?;
        if got != r.sha256 {
            return Err(format!(
                "{id}: grade-case hash {got} differs from the pinned {}",
                r.sha256
            ));
        }
        let g = GradeCase::load(&gpath)?;
        if g.id != *id {
            return Err(format!("{id}: grade-case id is {}", g.id));
        }
        if let Some(i) = &g.implementation {
            let dir = gpath.parent().map(Path::to_path_buf).unwrap_or_default();
            for (sub, tests) in [
                (&i.pristine_dir, &i.tests),
                (
                    i.hidden_dir.as_ref().unwrap_or(&i.pristine_dir),
                    &i.hidden_tests,
                ),
            ] {
                for t in tests {
                    let f = rel_join(&rel_join(&dir, sub)?, &t.path)?;
                    let h = sha256_file(&f)?;
                    if h != t.sha256 {
                        return Err(format!(
                            "{id}: grader copy of {} differs from its hash",
                            t.path
                        ));
                    }
                }
            }
        }
        let initial = match &case.initial {
            None => None,
            Some(decl) => {
                let path = spec
                    .cases_root
                    .join(match case.split {
                        crate::manifest::Split::Dev => "dev",
                        crate::manifest::Split::Final => "final",
                    })
                    .join(id)
                    .join(INITIAL_LOCK_FILE);
                let text = fs::read_to_string(&path).map_err(|e| {
                    format!(
                        "{id}: the case declares an initial tree but {} is missing ({e}); its content cannot be verified",
                        path.display()
                    )
                })?;
                let li: LockedInitial =
                    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                if li.commit != decl.commit || li.tree != decl.tree {
                    return Err(format!(
                        "{id}: {INITIAL_LOCK_FILE} commit/tree differ from case.json initial"
                    ));
                }
                let mut want: Vec<String> = decl.submodules.clone();
                let mut have: Vec<String> = li
                    .submodules
                    .iter()
                    .map(|s| format!("{}@{}", s.path, s.commit))
                    .collect();
                want.sort();
                have.sort();
                if want != have {
                    return Err(format!(
                        "{id}: {INITIAL_LOCK_FILE} submodules differ from case.json initial"
                    ));
                }
                Some(li)
            }
        };
        cases.push(
            LockedCase::from_declared(case, initial, g.implementation.as_ref())
                .map_err(|e| e.to_string())?,
        );
    }
    let hash = m.compute_sha256()?;
    let mid = match spec.measurement_id {
        Some(x) => x.to_string(),
        None => format!("{}-{}", m.campaign_id, &hash[..12.min(hash.len())]),
    };
    let lock = CampaignLock::new(LOCK_VERSION, &mid, m, cases)
        .and_then(CampaignLock::seal)
        .map_err(|e| e.to_string())?;
    lock.bind_to_manifest(m).map_err(|e| e.to_string())?;
    shared_initial(&lock)?;
    Ok(lock)
}

/// 現在指定された取り込み元の実体を期待 lock と照合する（副作用なし）。
/// `require_all` は初回（全部要る）。再開では指定された分だけを照合する。
pub fn verify_sources(l: &CampaignLock, s: Sources, require_all: bool) -> Result<(), String> {
    let initial = shared_initial(l)?;
    match (initial, s.repo) {
        (Some(i), Some(r)) => {
            lock_tree::verify_archive(i, r).map_err(|e| format!("--repo-src: {e}"))?;
        }
        (Some(_), None) if require_all => {
            return Err("--repo-src is required: the cases pin an initial tree".into());
        }
        (None, Some(_)) => {
            return Err("--repo-src was given but no case pins an initial tree".into());
        }
        _ => {}
    }
    for (name, p, want) in [
        ("--materials-src", s.materials, &l.materials_sha256),
        ("--kb-src", s.kb, &l.kb_snapshot),
    ] {
        match p {
            Some(p) => {
                let got = hash_tree(p).map_err(|e| format!("{name}: {e}"))?;
                if &got != want {
                    return Err(format!(
                        "{name}: content hash {got} differs from the locked {want}"
                    ));
                }
            }
            None if require_all => return Err(format!("{name} is required")),
            None => {}
        }
    }
    let map = locked_inputs(l);
    if !map.is_empty() {
        match s.inputs {
            Some(src) => {
                let errs = inputs::check_src(src, &map);
                if !errs.is_empty() {
                    return Err(format!("--inputs-src: {}", errs.join("; ")));
                }
            }
            None if require_all => {
                return Err("--inputs-src is required: the cases declare external inputs".into());
            }
            None => {}
        }
    }
    Ok(())
}

/// workspace-seed（repo + `materials/` + `kb/`）の repo 部分を期待値と照合する。
fn verify_seed(i: Option<&LockedInitial>, seed: &Path) -> Result<(), String> {
    let mut ex: Vec<String> = vec!["materials".into(), "kb".into()];
    match i {
        Some(i) => {
            for s in &i.submodules {
                let dir = seed.join(&s.path);
                let got = lock_tree::archive_content_sha256(
                    &dir,
                    &lock_tree::nested_paths(&i.submodules, &s.path),
                )
                .map_err(|e| format!("seed {}: {e}", s.path))?;
                if got != s.content_sha256 {
                    return Err(format!("seed submodule {} differs from the lock", s.path));
                }
            }
            for s in lock_tree::top_level_submodules(&i.submodules) {
                ex.push(s.path.clone());
            }
            let got =
                lock_tree::archive_content_sha256(seed, &ex).map_err(|e| format!("seed: {e}"))?;
            if got != i.content_sha256 {
                return Err("workspace-seed repo content differs from the lock".into());
            }
        }
        None => {
            let extra = fs::read_dir(seed)
                .map_err(|e| format!("seed: {e}"))?
                .filter_map(Result::ok)
                .any(|e| !ex.iter().any(|x| e.file_name() == x.as_str()));
            if extra {
                return Err(
                    "workspace-seed has repo content but no case pins an initial tree".into(),
                );
            }
        }
    }
    Ok(())
}

/// 保存済み（または copy 直後の）snapshot を期待 lock と照合する。取り込み後の差替え・改変を見逃さない。
pub fn verify_snapshot(l: &CampaignLock, snap: &Path) -> Result<(), String> {
    let initial = shared_initial(l)?;
    if let Some(i) = initial {
        lock_tree::verify_archive(i, &snap.join("repo"))
            .map_err(|e| format!("snapshot/repo: {e}"))?;
    }
    verify_seed(initial, &snap.join("workspace-seed"))?;
    if !repo_free_cases(l).is_empty() {
        verify_seed(None, &snap.join(EMPTY_SEED_DIR))?;
    }
    let mat = hash_tree(&snap.join("materials")).map_err(|e| format!("snapshot/materials: {e}"))?;
    if mat != l.materials_sha256 {
        return Err("snapshot/materials differs from the lock".into());
    }
    let kb = hash_tree(&snap.join("kb")).map_err(|e| format!("snapshot/kb: {e}"))?;
    if kb != l.kb_snapshot {
        return Err("snapshot/kb differs from the lock".into());
    }
    let map = locked_inputs(l);
    if !map.is_empty() {
        inputs::verify_stage(&snap.join(inputs::INPUTS_DIR), &map)?;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// 保存 lock が無い空の out。executor 投入前に lock を書いて始める。
    Fresh,
    /// 保存 lock が期待と一致した再開。
    Resume,
}

/// 試行の data があるか。測定開始時の固定記録（`env-lock.json`。launcher が初回実行の直前に作る）は data に数えない。
/// runner の data があるか。env-lock と、評価者の raw 人操作記録（`human-ops.jsonl`。pilot 開始時＝初回 watch の前から
/// 同じ out に追記する。runbook §2）は runner の data ではないので数えない。
fn out_has_data(out: &Path) -> Result<bool, String> {
    match fs::read_dir(out) {
        Ok(d) => Ok(d.filter_map(Result::ok).any(|e| {
            let n = e.file_name();
            n != crate::env_lock::ENV_LOCK_FILE && n != crate::human_ops::HUMAN_OPS_FILE
        })),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", out.display())),
    }
}

/// 配る入力（`RunOptions.case_inputs`）が lock の固定と同じか。足りない入力のまま始めない。
pub fn check_delivery(
    l: &CampaignLock,
    given: &BTreeMap<String, Vec<DeliveredInput>>,
) -> Result<(), String> {
    let norm = |m: &BTreeMap<String, Vec<DeliveredInput>>| {
        m.iter()
            .map(|(k, v)| {
                let mut v = v.clone();
                v.sort_by(|a, b| a.name.cmp(&b.name));
                (k.clone(), v)
            })
            .collect::<BTreeMap<_, _>>()
    };
    if norm(&locked_inputs(l)) != norm(given) {
        return Err("external inputs to deliver differ from the inputs pinned in the lock".into());
    }
    Ok(())
}

/// 副作用の前の判定。lock 欠落・変更、manifest/入力/構成の差替えはここで Err（out は一切変えない）。
pub fn begin(
    m: &CampaignManifest,
    expected: &CampaignLock,
    out: &Path,
    s: Sources,
) -> Result<Start, String> {
    expected.bind_to_manifest(m).map_err(|e| e.to_string())?;
    let path = out.join(LOCK_FILE);
    if path.exists() {
        let stored: CampaignLock = serde_json::from_slice(
            &fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?,
        )
        .map_err(|e| format!("{}: {e}", path.display()))?;
        stored
            .bind_to_manifest(m)
            .map_err(|e| format!("saved lock: {e}"))?;
        verify_lock(expected, &stored).map_err(|e| {
            format!("saved lock differs from the expected lock; refusing to resume: {e}")
        })?;
        verify_sources(expected, s, false)?;
        return Ok(Start::Resume);
    }
    if out_has_data(out)? {
        return Err(format!(
            "{} has data but no {LOCK_FILE}; existing data is not adopted into a new lock (use an empty dir for a new measurement)",
            out.display()
        ));
    }
    verify_sources(expected, s, true)?;
    Ok(Start::Fresh)
}

/// 初回だけ。lock を書く（既にあれば書き換えない）。
pub fn save(out: &Path, expected: &CampaignLock) -> Result<(), String> {
    use std::io::Write;
    let path = out.join(LOCK_FILE);
    let mut text = serde_json::to_string_pretty(expected).map_err(|e| e.to_string())?;
    text.push('\n');
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}

/// 保存 lock を読む。無ければ `None`（legacy: lock 導入前、または無課金の mock 実行）。
/// あるが読めない・封印が合わない・manifest に結び付かない・`expected`（cases root から作った期待 lock）と違う場合は Err。
pub fn load_saved(
    m: &CampaignManifest,
    out: &Path,
    expected: Option<&CampaignLock>,
) -> Result<Option<CampaignLock>, String> {
    let path = out.join(LOCK_FILE);
    let locked_run = fs::read(out.join(crate::record::META_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .is_some_and(|v| v["locked"] == true);
    if !path.exists() {
        if locked_run || expected.is_some() {
            return Err(format!(
                "{} is missing although this measurement is locked; grades and tables cannot be trusted",
                path.display()
            ));
        }
        return Ok(None);
    }
    let l: CampaignLock =
        serde_json::from_slice(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    l.check_seal().map_err(|e| format!("saved lock: {e}"))?;
    l.bind_to_manifest(m)
        .map_err(|e| format!("saved lock: {e}"))?;
    if let Some(x) = expected {
        verify_lock(x, &l)
            .map_err(|e| format!("saved lock differs from the expected lock: {e}"))?;
    }
    Ok(Some(l))
}

/// grade-case 実体（grade-case file と公開/隠しテスト全件）を lock の固定値と照合する。grader command を走らせる前に呼ぶ。
/// 戻り値は読み込んだ grade-case。case id/split の一致だけでは信用しない。
pub fn verify_grade_case(c: &LockedCase, path: &Path) -> Result<GradeCase, String> {
    let got = sha256_file(path)?;
    if got != c.grader.grade_case_sha256 {
        return Err(format!(
            "{}: grade-case hash {got} differs from the locked {}",
            c.id, c.grader.grade_case_sha256
        ));
    }
    let g = GradeCase::load(path)?;
    if g.id != c.id {
        return Err(format!("{}: grade-case id is {}", c.id, g.id));
    }
    let mut actual: Vec<crate::lock::LockedTest> = Vec::new();
    if let Some(i) = &g.implementation {
        use crate::lock::{LockedTest, TestVisibility};
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        for (vis, sub, tests) in [
            (TestVisibility::Public, &i.pristine_dir, &i.tests),
            (
                TestVisibility::Hidden,
                i.hidden_dir.as_ref().unwrap_or(&i.pristine_dir),
                &i.hidden_tests,
            ),
        ] {
            for t in tests {
                let f = rel_join(&rel_join(&dir, sub)?, &t.path)?;
                // 宣言 hash ではなく、ディスク上の実体の hash。
                actual.push(LockedTest {
                    path: t.path.clone(),
                    visibility: vis,
                    sha256: sha256_file(&f)?,
                });
            }
        }
    }
    let mut want = c.tests.clone();
    want.sort();
    actual.sort();
    if want != actual {
        return Err(format!(
            "{}: public/hidden tests differ from the locked set (path, visibility or content)",
            c.id
        ));
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 評価者の raw 人操作記録を初回 watch の前から out に置いても、新しい測定として始められる（runner の data だけ数える）。
    #[test]
    fn human_ops_record_written_before_the_first_watch_is_not_runner_data() {
        let d = tempfile::tempdir().unwrap();
        assert!(!out_has_data(d.path()).unwrap());
        fs::write(d.path().join(crate::human_ops::HUMAN_OPS_FILE), "{}\n").unwrap();
        fs::write(d.path().join(crate::env_lock::ENV_LOCK_FILE), "{}").unwrap();
        assert!(!out_has_data(d.path()).unwrap());
        fs::write(d.path().join("ledger.jsonl"), "{}\n").unwrap();
        assert!(out_has_data(d.path()).unwrap());
    }
}
