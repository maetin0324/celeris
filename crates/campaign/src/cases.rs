//! 代表 case の定義・読み込み・検証（既存 Benchfs／Pluvio 資産、ADR 2026-10-15-mlsys-min-campaign の続き）。
//!
//! 配置:
//! - `cases/<dev|final>/<id>/case.json` — 試行に見せてよい公開部（初期 commit の固定・資料と KB の snapshot・実験条件）。
//!   `materials/` と `kb/` は同じ directory にあり、内容 hash を `snapshot` に固定する。
//! - `fixtures/grader-private/<dev|final>/<id>/private.json` — 期待値・既知欠陥・誤指摘 trap・隠し受入 command。
//!   `cases/` の外の隔離 directory で、final の中身は dev 側のどの file からも参照されない。
//!
//! manifest 型（`Split`・`InputRef`・`Budget`・`SnapshotRef`）は `manifest.rs` のものを再利用する。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    manifest::{Budget, InputRef, SnapshotRef, Split},
    snapshot::hash_tree,
};

pub const CASE_SCHEMA: &str = "celeris.campaign-case/1";
pub const PRIVATE_SCHEMA: &str = "celeris.campaign-case-private/1";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CaseKind {
    /// 既存 Benchfs／Pluvio 資産に基づく本測定用 case。
    Real,
    /// fixture／mock の制御試験用。本測定にも最終合格にも使わない。
    Control,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    JobId,
    Log,
    RawData,
}

/// 固定した初期 commit。コード本体は写さず、取得手順と hash だけを持つ。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Initial {
    pub repo: String,
    pub commit: String,
    pub tree: String,
    /// `git archive` した tar の sha256（submodule は含まない）。
    pub archive_sha256: String,
    pub fetch: Vec<String>,
    #[serde(default)]
    pub submodules: Vec<String>,
}

/// 外部に置いた原票（巨大なので写さない）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExternalInput {
    pub name: String,
    pub sha256: String,
    pub fetch: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Experiment {
    pub cluster: String,
    pub nodes: u32,
    pub walltime_secs: u64,
    pub params: BTreeMap<String, Value>,
}

/// 試行が実際に使う能力の 1 件（cap-probe が A の sandbox argv で包んで走らせる）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    pub id: String,
    pub kind: CapKind,
    /// sandbox 内で走らせる command（shell を介さない。複合は `sh -c`）。job 投入は書かない。
    pub argv: Vec<String>,
    /// 標準出力に含まれるべき文字列（無ければ exit 0 だけ見る）。
    #[serde(default)]
    pub expect_stdout: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapKind {
    Compiler,
    Library,
    Build,
    Interpreter,
    /// 承認済みクラスタ接続経路の到達確認（接続のみ。job は投入しない）。
    Connection,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub schema: String,
    pub id: String,
    pub kind: CaseKind,
    pub split: Split,
    pub title: String,
    /// 含む工程: implementation / experiment / analysis / review。
    pub stages: Vec<String>,
    /// 採点器が判定する工程（stages の部分集合。省略時は stages 全部）。差が `not_judged`
    /// （今回は採点せず、報告で検証済みと主張しない）。ADR 2026-10-18-k2-judged-scope。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judged_scope: Option<Vec<String>>,
    #[serde(default)]
    pub initial: Option<Initial>,
    #[serde(default)]
    pub inputs: Vec<ExternalInput>,
    pub snapshot: SnapshotRef,
    /// 試行が実行してよい公開の受入 command（隠し試験は private 側）。
    pub visible_command: String,
    #[serde(default)]
    pub experiment: Option<Experiment>,
    pub evidence: Vec<EvidenceKind>,
    pub budget: Budget,
    /// grader root からの相対 path と hash。
    pub private_ref: InputRef,
    /// 実行可能な `celeris.grade-case/1`（`fixtures/grade-cases` からの相対 path と hash）。real case は必須。
    /// `campaign-case-private/1`（期待値・欠陥 list）とは別物で、評価 command・テスト hash・実験/再計算の条件を持つ。
    #[serde(default)]
    pub grade_case_ref: Option<InputRef>,
    /// 本測定の前に LLM なしで確かめる必要能力（compiler・依存 lib・最小 build・接続・再計算 toolchain）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_capabilities: Vec<Capability>,
}

impl Case {
    /// worker に見せてよい公開の case 定義（`materials/<case>/case.json` に置く JSON）。grader だけが読む
    /// `private_ref`・`grade_case_ref` は含めない（ADR 2026-10-20-mlsys-main2-fixes D2-4）。
    pub fn public_json(&self) -> Result<String, String> {
        let mut v = serde_json::to_value(self).map_err(|e| e.to_string())?;
        if let Some(o) = v.as_object_mut() {
            o.remove("private_ref");
            o.remove("grade_case_ref");
        }
        serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
    }

    pub fn judged_scope(&self) -> Vec<String> {
        self.judged_scope
            .clone()
            .unwrap_or_else(|| self.stages.clone())
    }

    pub fn not_judged(&self) -> Vec<String> {
        let j = self.judged_scope();
        self.stages
            .iter()
            .filter(|s| !j.contains(s))
            .cloned()
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KnownDefect {
    pub id: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FalseClaimTrap {
    pub id: String,
    pub claim: String,
    pub why_wrong: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CasePrivate {
    pub schema: String,
    pub case_id: String,
    pub split: Split,
    pub hidden_command: String,
    /// 初期 tree（隠しテストなし）が評価と同じ build 設定で compile できるかを見る command（grade-case の
    /// `baseline_command`）。compile error を初期 tree 由来（env_failure）とモデル由来に分ける。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_command: Option<String>,
    pub known_defects: Vec<KnownDefect>,
    pub false_claim_traps: Vec<FalseClaimTrap>,
    /// 参照値・再計算の仕様。grader だけが読む。
    pub expected: Value,
}

#[derive(Debug)]
pub struct LoadedCase {
    pub case: Case,
    pub private: CasePrivate,
}

#[derive(Debug)]
pub struct CaseSet {
    pub cases: Vec<LoadedCase>,
}

fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn sha256_file(p: &Path) -> Result<String, String> {
    let b = fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(format!("{:x}", Sha256::digest(b)))
}

/// 外部に置いた原票の実体が固定した hash と一致するか。
pub fn verify_file_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let got = sha256_file(path)?;
    if got != expected {
        return Err(format!(
            "sha256 mismatch for {}: expected {expected}, got {got}",
            path.display()
        ));
    }
    Ok(())
}

fn safe_rel(p: &str) -> Result<(), String> {
    let path = Path::new(p);
    if p.is_empty() || path.is_absolute() {
        return Err(format!("path must be relative and non-empty: {p:?}"));
    }
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!("path must not contain `..` or `.`: {p}"));
    }
    Ok(())
}

fn split_dir(s: Split) -> &'static str {
    match s {
        Split::Dev => "dev",
        Split::Final => "final",
    }
}

impl Case {
    /// 必須欄・形式の検査（file には触れない）。
    pub fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        if self.schema != CASE_SCHEMA {
            return Err(format!("{id}: schema must be {CASE_SCHEMA}"));
        }
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(format!("case id must be [a-z0-9-]+: {id:?}"));
        }
        for (name, v) in [
            ("title", &self.title),
            ("visible_command", &self.visible_command),
            ("snapshot.repo_commit", &self.snapshot.repo_commit),
            ("snapshot.kb_snapshot", &self.snapshot.kb_snapshot),
        ] {
            if v.trim().is_empty() {
                return Err(format!("{id}: required field is empty: {name}"));
            }
        }
        if !is_hex(&self.snapshot.materials_sha256, 64) {
            return Err(format!(
                "{id}: snapshot.materials_sha256 must be sha256 hex"
            ));
        }
        if !is_hex(&self.private_ref.sha256, 64) {
            return Err(format!("{id}: private_ref.sha256 must be sha256 hex"));
        }
        safe_rel(&self.private_ref.path).map_err(|e| format!("{id}: private_ref: {e}"))?;
        let expect_prefix = format!("{}/{id}/", split_dir(self.split));
        if !self.private_ref.path.starts_with(&expect_prefix) {
            return Err(format!(
                "{id}: private_ref.path must start with {expect_prefix} (split mixing)"
            ));
        }
        if self.stages.is_empty() {
            return Err(format!("{id}: stages is empty"));
        }
        if let Some(js) = &self.judged_scope
            && (js.is_empty() || js.iter().any(|x| !self.stages.contains(x)))
        {
            return Err(format!(
                "{id}: judged_scope must be a non-empty subset of stages"
            ));
        }
        let mut seen = BTreeSet::new();
        for c in &self.required_capabilities {
            if c.id.is_empty() || c.argv.is_empty() || !seen.insert(c.id.as_str()) {
                return Err(format!(
                    "{id}: required_capabilities needs unique non-empty id and argv ({:?})",
                    c.id
                ));
            }
        }
        if self.evidence.is_empty() {
            return Err(format!("{id}: evidence kinds are required"));
        }
        let b = &self.budget;
        if b.max_tokens == 0 || b.max_wall_secs == 0 || b.max_parallel == 0 {
            return Err(format!("{id}: budget fields must be positive"));
        }
        let (c, a) = (b.per_case, b.per_arm);
        if c.max_tokens == 0
            || c.max_wall_secs == 0
            || a.max_tokens < c.max_tokens
            || a.max_wall_secs < c.max_wall_secs
        {
            return Err(format!(
                "{id}: budget per_case must be positive and per_arm >= per_case"
            ));
        }
        if self.kind == CaseKind::Control {
            if self.split != Split::Dev {
                return Err(format!("{id}: a control case must be split=dev"));
            }
            return Ok(());
        }
        if self.initial.is_none() && self.inputs.is_empty() {
            return Err(format!("{id}: real case needs initial or inputs"));
        }
        let Some(g) = &self.grade_case_ref else {
            return Err(format!(
                "{id}: real case needs grade_case_ref (celeris.grade-case/1)"
            ));
        };
        if !is_hex(&g.sha256, 64) {
            return Err(format!("{id}: grade_case_ref.sha256 must be sha256 hex"));
        }
        safe_rel(&g.path).map_err(|e| format!("{id}: grade_case_ref: {e}"))?;
        let expect_grade = format!("{}/{id}/{id}.json", split_dir(self.split));
        if g.path != expect_grade {
            return Err(format!("{id}: grade_case_ref.path must be {expect_grade}"));
        }
        if let Some(i) = &self.initial {
            if !is_hex(&i.commit, 40) || !is_hex(&i.tree, 40) || !is_hex(&i.archive_sha256, 64) {
                return Err(format!("{id}: initial commit/tree/archive hash malformed"));
            }
            if i.repo.trim().is_empty() || i.fetch.is_empty() {
                return Err(format!("{id}: initial.repo and initial.fetch are required"));
            }
            // submodule は `<path>@<40 桁 commit>` で固定する（短縮 hash や未固定は許さない）。
            for m in &i.submodules {
                let ok = m
                    .split_once('@')
                    .is_some_and(|(path, c)| !path.is_empty() && is_hex(c, 40));
                if !ok {
                    return Err(format!(
                        "{id}: submodule must be <path>@<40-hex commit>: {m}"
                    ));
                }
            }
            if self.snapshot.repo_commit != i.commit {
                return Err(format!(
                    "{id}: snapshot.repo_commit differs from initial.commit"
                ));
            }
        }
        for x in &self.inputs {
            if !is_hex(&x.sha256, 64)
                || x.fetch.is_empty()
                || x.name.is_empty()
                || x.name.starts_with('.')
                || x.name.contains(['/', '\\', '\0'])
            {
                return Err(format!("{id}: external input {:?} malformed", x.name));
            }
        }
        let needs_exp = self.stages.iter().any(|s| s == "experiment");
        match (&self.experiment, needs_exp) {
            (None, true) => {
                return Err(format!(
                    "{id}: experiment stage needs experiment conditions"
                ));
            }
            (Some(e), _) if e.nodes == 0 || e.walltime_secs == 0 || e.cluster.is_empty() => {
                return Err(format!("{id}: experiment nodes/walltime/cluster required"));
            }
            (Some(_), false) => {
                return Err(format!(
                    "{id}: experiment given without an experiment stage"
                ));
            }
            _ => {}
        }
        if needs_exp && !self.evidence.contains(&EvidenceKind::JobId) {
            return Err(format!("{id}: experiment case must expect job_id evidence"));
        }
        if !self.evidence.contains(&EvidenceKind::RawData) {
            return Err(format!("{id}: real case must expect raw_data evidence"));
        }
        Ok(())
    }
}

impl CasePrivate {
    /// 読まなかった private の placeholder（`CaseSet::load_public`）。中身は空で、採点・照合には使えない。
    fn withheld(case: &Case) -> Self {
        CasePrivate {
            schema: PRIVATE_SCHEMA.into(),
            case_id: case.id.clone(),
            split: case.split,
            hidden_command: String::new(),
            baseline_command: None,
            known_defects: Vec::new(),
            false_claim_traps: Vec::new(),
            expected: Value::Null,
        }
    }

    pub fn validate(&self, case: &Case) -> Result<(), String> {
        let id = &case.id;
        if self.schema != PRIVATE_SCHEMA {
            return Err(format!("{id}: private schema must be {PRIVATE_SCHEMA}"));
        }
        if self.case_id != case.id {
            return Err(format!("{id}: private case_id is {:?}", self.case_id));
        }
        if self.split != case.split {
            return Err(format!(
                "{id}: private split differs from case split (split mixing)"
            ));
        }
        if self.hidden_command.trim().is_empty() {
            return Err(format!("{id}: private hidden_command is required"));
        }
        if case.kind == CaseKind::Real
            && (self.known_defects.is_empty() || self.false_claim_traps.is_empty())
        {
            return Err(format!(
                "{id}: real case needs known_defects and false_claim_traps"
            ));
        }
        let mut seen = BTreeSet::new();
        for d in self
            .known_defects
            .iter()
            .map(|d| &d.id)
            .chain(self.false_claim_traps.iter().map(|t| &t.id))
        {
            if !seen.insert(d) {
                return Err(format!("{id}: duplicate defect/trap id {d}"));
            }
        }
        Ok(())
    }
}

fn read_json<T: serde::de::DeserializeOwned>(p: &Path) -> Result<T, String> {
    let s = fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))
}

fn subdirs(p: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut v = Vec::new();
    for e in fs::read_dir(p).map_err(|e| format!("{}: {e}", p.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        if e.path().is_dir() {
            v.push(e.path());
        }
    }
    v.sort();
    Ok(v)
}

impl CaseSet {
    /// `cases_root/{dev,final}/<id>/case.json` と `grader_root/<split>/<id>/private.json` を読んで検証する。
    pub fn load(cases_root: &Path, grader_root: &Path) -> Result<Self, String> {
        Self::load_with(cases_root, Some(grader_root))
    }

    /// `case.json` と materials・kb の hash だけを読む（private は読まない。`private` は空の placeholder）。
    /// ops-r2 の preflight 用: 新しい private は評価器領域（`EVAL_ROOT`）にあり、呼び出し側は読まない。
    /// private の sha256 は manifest の `grader_refs` と case.json の `private_ref` の一致で見る。
    pub fn load_public(cases_root: &Path) -> Result<Self, String> {
        Self::load_with(cases_root, None)
    }

    fn load_with(cases_root: &Path, grader_root: Option<&Path>) -> Result<Self, String> {
        let cr = cases_root
            .canonicalize()
            .map_err(|e| format!("{}: {e}", cases_root.display()))?;
        let gr = match grader_root {
            Some(g) => {
                let gr = g
                    .canonicalize()
                    .map_err(|e| format!("{}: {e}", g.display()))?;
                if gr.starts_with(&cr) || cr.starts_with(&gr) {
                    return Err("grader root must be isolated from the cases root".into());
                }
                Some(gr)
            }
            None => None,
        };
        let mut cases = Vec::new();
        let mut ids = BTreeSet::new();
        for split in [Split::Dev, Split::Final] {
            let sdir = cr.join(split_dir(split));
            if !sdir.is_dir() {
                continue;
            }
            for dir in subdirs(&sdir)? {
                let case: Case = read_json(&dir.join("case.json"))?;
                case.validate()?;
                let dirname = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                if case.split != split || case.id != dirname {
                    return Err(format!(
                        "{}: case id/split disagrees with its directory {}/{dirname}",
                        case.id,
                        split_dir(split)
                    ));
                }
                if !ids.insert(case.id.clone()) {
                    return Err(format!("duplicate case id {}", case.id));
                }
                check_snapshot(&case, &dir)?;
                let private = match &gr {
                    Some(gr) => {
                        let ppath = gr.join(&case.private_ref.path);
                        let got = sha256_file(&ppath)?;
                        if got != case.private_ref.sha256 {
                            return Err(format!("{}: private file hash mismatch", case.id));
                        }
                        let private: CasePrivate = read_json(&ppath)?;
                        private.validate(&case)?;
                        private
                    }
                    None => CasePrivate::withheld(&case),
                };
                cases.push(LoadedCase { case, private });
            }
        }
        if cases.is_empty() {
            return Err("no cases found".into());
        }
        let set = CaseSet { cases };
        if let Some(gr) = &gr {
            check_isolation(&set, &cr, gr)?;
        }
        Ok(set)
    }

    /// 選んだ case の時間・token が manifest の per_case 上限に収まるか（収まらなければ起動前に Err）。
    pub fn check_within_budget(&self, ids: &[String], b: &Budget) -> Result<(), String> {
        for id in ids {
            let c = self.get(id).ok_or_else(|| format!("unknown case {id}"))?;
            let (cb, pc) = (&c.case.budget.per_case, &b.per_case);
            if cb.max_wall_secs > pc.max_wall_secs || cb.max_tokens > pc.max_tokens {
                return Err(format!(
                    "{id}: case budget (wall {}s, tokens {}) exceeds manifest per_case (wall {}s, tokens {})",
                    cb.max_wall_secs, cb.max_tokens, pc.max_wall_secs, pc.max_tokens
                ));
            }
            if let Some(e) = &c.case.experiment
                && e.walltime_secs > pc.max_wall_secs
            {
                return Err(format!(
                    "{id}: experiment walltime {}s exceeds manifest per_case wall {}s",
                    e.walltime_secs, pc.max_wall_secs
                ));
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&LoadedCase> {
        self.cases.iter().find(|c| c.case.id == id)
    }

    /// 本測定に使う case の選択。control・未知 id・dev と final の混在・重複を拒否する。
    pub fn select_for_measurement(&self, ids: &[String]) -> Result<Vec<&LoadedCase>, String> {
        if ids.is_empty() {
            return Err("empty measurement selection".into());
        }
        let mut out: Vec<&LoadedCase> = Vec::new();
        for id in ids {
            let c = self.get(id).ok_or_else(|| format!("unknown case {id}"))?;
            if c.case.kind == CaseKind::Control {
                return Err(format!("control case {id} must not enter a measurement"));
            }
            if out.iter().any(|o| o.case.id == *id) {
                return Err(format!("duplicate case {id} in selection"));
            }
            if let Some(first) = out.first()
                && first.case.split != c.case.split
            {
                return Err(format!(
                    "split mixing: {} is {:?} but {id} is {:?}",
                    first.case.id, first.case.split, c.case.split
                ));
            }
            out.push(c);
        }
        Ok(out)
    }
}

fn check_snapshot(case: &Case, dir: &Path) -> Result<(), String> {
    let id = &case.id;
    let mat = hash_tree(&dir.join("materials")).map_err(|e| format!("{id}: materials: {e}"))?;
    if mat != case.snapshot.materials_sha256 {
        return Err(format!(
            "{id}: materials hash mismatch (pinned {}, got {mat})",
            case.snapshot.materials_sha256
        ));
    }
    let kb = hash_tree(&dir.join("kb")).map_err(|e| format!("{id}: kb: {e}"))?;
    if kb != case.snapshot.kb_snapshot {
        return Err(format!(
            "{id}: kb snapshot hash mismatch (pinned {}, got {kb})",
            case.snapshot.kb_snapshot
        ));
    }
    Ok(())
}

/// dev 側（case.json・materials・kb と fixtures/grader-private/dev）に final の case id・期待値・欠陥 id・private hash が現れない。
fn check_isolation(set: &CaseSet, cases_root: &Path, grader_root: &Path) -> Result<(), String> {
    let mut needles: Vec<String> = Vec::new();
    for c in set.cases.iter().filter(|c| c.case.split == Split::Final) {
        needles.push(c.case.id.clone());
        needles.push(c.case.private_ref.sha256.clone());
        needles.extend(c.private.known_defects.iter().map(|d| d.id.clone()));
        needles.extend(c.private.false_claim_traps.iter().map(|d| d.id.clone()));
        if let Some(i) = &c.case.initial {
            needles.push(i.commit.clone());
        }
    }
    let mut stack: Vec<std::path::PathBuf> = [cases_root.join("dev"), grader_root.join("dev")]
        .into_iter()
        .filter(|p| p.is_dir())
        .collect();
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).map_err(|e| e.to_string())? {
            let p = e.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let text =
                String::from_utf8_lossy(&fs::read(&p).map_err(|e| e.to_string())?).into_owned();
            if let Some(n) = needles.iter().find(|n| text.contains(n.as_str())) {
                return Err(format!(
                    "final-side value {n:?} is referenced from dev file {}",
                    p.display()
                ));
            }
        }
    }
    Ok(())
}
