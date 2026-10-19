//! 版付き campaign lock（ADR 2026-10-17-campaign-lock）。初期 tree・submodule・外部入力・grader・公開/隠しテスト・
//! 実行構成を 1 つの記録に固定し、campaign hash に結び付ける。照合は `verify_lock(expected, actual)`。
//! 実体（repo / archive / submodule）から lock を作る側は別 WorkUnit。ここは型・正規化・hash・検証だけを持つ。
use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    cases::Case,
    grader::case::ImplementationSpec,
    manifest::{ArmOrder, Budget, CampaignManifest, Config, Split, Termination, canonical},
    record::DeliveredInput,
};

pub const LOCK_SCHEMA: &str = "celeris.campaign-lock/1";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockedSubmodule {
    pub path: String,
    pub commit: String,
    /// 展開済み submodule 木の内容 hash（`snapshot::hash_tree`）。
    pub content_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedInitial {
    pub commit: String,
    pub tree: String,
    /// 展開済み初期木（submodule を除く）の内容 hash。git metadata の有無に依らない。
    pub content_sha256: String,
    pub submodules: Vec<LockedSubmodule>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TestVisibility {
    Public,
    Hidden,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockedTest {
    pub path: String,
    pub visibility: TestVisibility,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraderPin {
    /// case の private 定義（期待値・欠陥 list）の hash。
    pub private_sha256: String,
    /// 実行可能な grade-case の hash。
    pub grade_case_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LockedCase {
    pub id: String,
    pub split: Split,
    #[serde(default)]
    pub implementation: bool,
    #[serde(default)]
    pub initial: Option<LockedInitial>,
    pub inputs: Vec<DeliveredInput>,
    pub grader: GraderPin,
    pub tests: Vec<LockedTest>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExecutionPin {
    /// 有効 arm の config だけ（未使用 arm は固定しない）。
    pub configs: Vec<Config>,
    /// 実験セット宣言（選択を含む）の hash。宣言の無い測定は空（直列化しない。既存 lock の hash は変わらない）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sets_sha256: String,
    pub budget: Budget,
    pub termination: Termination,
    pub order: ArmOrder,
    pub seed: u64,
    pub repeats: u32,
    /// manifest の variant 参照の写し。省略時は直列化しない（従来 lock の hash を変えない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<crate::variant::VariantRef>,
}

impl ExecutionPin {
    pub fn from_manifest(m: &CampaignManifest) -> Self {
        let n = m.normalized();
        let active = n.active_arms();
        let sets_sha256 = if n.experiment_sets.is_empty() {
            String::new()
        } else {
            n.sets_sha256().unwrap_or_default()
        };
        ExecutionPin {
            configs: n
                .configs
                .into_iter()
                .filter(|c| active.contains(&c.id))
                .collect(),
            sets_sha256,
            budget: n.budget,
            termination: n.termination,
            order: n.order,
            seed: n.seed,
            repeats: n.repeats,
            variant: n.variant,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CampaignLock {
    pub schema: String,
    pub version: String,
    pub measurement_id: String,
    pub campaign_sha256: String,
    pub materials_sha256: String,
    pub kb_snapshot: String,
    pub cases: Vec<LockedCase>,
    pub execution: ExecutionPin,
    /// 副比較（B1/C1）の系列台帳の固定位置（測定 ID ごとに 1 か所）。主比較は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<crate::series::SeriesPin>,
    /// `lock_hash()`。空なら未封印。
    #[serde(default)]
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockError {
    Unpinned(String),
    Duplicate(String),
    BadPath(String),
    MissingTests(String),
    Schema(String),
    SealMismatch(String),
    VersionMismatch {
        expected: String,
        actual: String,
    },
    MeasurementMismatch {
        expected: String,
        actual: String,
    },
    CampaignMismatch(String),
    MaterialsMismatch(String),
    CaseSetMismatch(String),
    InitialMismatch(String),
    SubmoduleMismatch(String),
    InputMismatch(String),
    GraderMismatch(String),
    TestMismatch(String),
    ExecutionMismatch(String),
    /// 同じ測定 ID・版で内容が違う（検出できる差が上のどれにも当たらない場合）。
    ContentChanged(String),
    NotSuccessor(String),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use LockError::*;
        match self {
            VersionMismatch { expected, actual } => {
                write!(
                    f,
                    "lock version {actual:?} differs from expected {expected:?}"
                )
            }
            MeasurementMismatch { expected, actual } => write!(
                f,
                "measurement id {actual:?} differs from expected {expected:?}"
            ),
            Unpinned(s) => write!(f, "unpinned value: {s}"),
            Duplicate(s) => write!(f, "duplicate: {s}"),
            BadPath(s) => write!(f, "bad path: {s}"),
            MissingTests(s) => write!(f, "tests are not pinned: {s}"),
            Schema(s) => write!(f, "schema: {s}"),
            SealMismatch(s) => write!(f, "lock hash does not match its content: {s}"),
            CampaignMismatch(s) => write!(f, "campaign hash mismatch: {s}"),
            MaterialsMismatch(s) => write!(f, "materials/KB mismatch: {s}"),
            CaseSetMismatch(s) => write!(f, "case set mismatch: {s}"),
            InitialMismatch(s) => write!(f, "initial tree mismatch: {s}"),
            SubmoduleMismatch(s) => write!(f, "submodule mismatch: {s}"),
            InputMismatch(s) => write!(f, "external input mismatch: {s}"),
            GraderMismatch(s) => write!(f, "grader mismatch: {s}"),
            TestMismatch(s) => write!(f, "test mismatch: {s}"),
            ExecutionMismatch(s) => write!(f, "execution config mismatch: {s}"),
            ContentChanged(s) => write!(
                f,
                "content changed under the same measurement id and version ({s}); a new measurement id is required"
            ),
            NotSuccessor(s) => write!(f, "not a valid successor lock: {s}"),
        }
    }
}

impl std::error::Error for LockError {}

fn is_hex(s: &str, lens: &[usize]) -> bool {
    lens.contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn pin_hash(what: &str, v: &str, lens: &[usize]) -> Result<(), LockError> {
    if v.is_empty() {
        return Err(LockError::Unpinned(format!("{what} is empty")));
    }
    if !is_hex(v, lens) {
        return Err(LockError::Unpinned(format!(
            "{what} is not a lowercase hex hash of length {lens:?}: {v:?}"
        )));
    }
    Ok(())
}

fn pin_text(what: &str, v: &str) -> Result<(), LockError> {
    if v.trim().is_empty() {
        return Err(LockError::Unpinned(format!("{what} is empty")));
    }
    Ok(())
}

fn check_rel_path(what: &str, p: &str) -> Result<(), LockError> {
    pin_text(what, p)?;
    if p.starts_with('/') || p.split('/').any(|c| c == ".." || c.is_empty() || c == ".") {
        return Err(LockError::BadPath(format!("{what}: {p:?}")));
    }
    Ok(())
}

fn no_dups<'a>(what: &str, it: impl Iterator<Item = &'a str>) -> Result<(), LockError> {
    let mut seen = BTreeSet::new();
    for x in it {
        if !seen.insert(x) {
            return Err(LockError::Duplicate(format!("{what} {x:?}")));
        }
    }
    Ok(())
}

const GIT_ID: &[usize] = &[40, 64];
const SHA256: &[usize] = &[64];

impl LockedCase {
    /// 宣言（case.json）と、実体から得た初期木の証明・grade-case から固定する。
    /// `grade` の公開/隠しテストを固定する。`initial` は case が `initial` を宣言するときに要る。
    pub fn from_declared(
        case: &Case,
        initial: Option<LockedInitial>,
        grade: Option<&ImplementationSpec>,
    ) -> Result<Self, LockError> {
        let mut tests = Vec::new();
        if let Some(g) = grade {
            for t in &g.tests {
                tests.push(LockedTest {
                    path: t.path.clone(),
                    visibility: TestVisibility::Public,
                    sha256: t.sha256.clone(),
                });
            }
            for t in &g.hidden_tests {
                tests.push(LockedTest {
                    path: t.path.clone(),
                    visibility: TestVisibility::Hidden,
                    sha256: t.sha256.clone(),
                });
            }
        }
        let grade_case = case
            .grade_case_ref
            .as_ref()
            .map(|r| r.sha256.clone())
            .unwrap_or_default();
        Ok(LockedCase {
            id: case.id.clone(),
            split: case.split,
            implementation: case.stages.iter().any(|s| s == "implementation"),
            initial,
            inputs: case
                .inputs
                .iter()
                .map(|x| DeliveredInput {
                    name: x.name.clone(),
                    sha256: x.sha256.clone(),
                })
                .collect(),
            grader: GraderPin {
                private_sha256: case.private_ref.sha256.clone(),
                grade_case_sha256: grade_case,
            },
            tests,
        })
    }

    fn validate(&self) -> Result<(), LockError> {
        let id = &self.id;
        pin_text("case id", id)?;
        if let Some(i) = &self.initial {
            pin_hash(&format!("{id}: initial.commit"), &i.commit, GIT_ID)?;
            pin_hash(&format!("{id}: initial.tree"), &i.tree, GIT_ID)?;
            pin_hash(
                &format!("{id}: initial.content_sha256"),
                &i.content_sha256,
                SHA256,
            )?;
            for s in &i.submodules {
                check_rel_path(&format!("{id}: submodule path"), &s.path)?;
                pin_hash(
                    &format!("{id}: submodule {} commit", s.path),
                    &s.commit,
                    GIT_ID,
                )?;
                pin_hash(
                    &format!("{id}: submodule {} content_sha256", s.path),
                    &s.content_sha256,
                    SHA256,
                )?;
            }
            no_dups(
                &format!("{id}: submodule path"),
                i.submodules.iter().map(|s| s.path.as_str()),
            )?;
        }
        for x in &self.inputs {
            check_rel_path(&format!("{id}: input name"), &x.name)?;
            pin_hash(&format!("{id}: input {}", x.name), &x.sha256, SHA256)?;
        }
        no_dups(
            &format!("{id}: input name"),
            self.inputs.iter().map(|x| x.name.as_str()),
        )?;
        pin_hash(
            &format!("{id}: grader.private_sha256"),
            &self.grader.private_sha256,
            SHA256,
        )?;
        pin_hash(
            &format!("{id}: grader.grade_case_sha256"),
            &self.grader.grade_case_sha256,
            SHA256,
        )?;
        for t in &self.tests {
            check_rel_path(&format!("{id}: test path"), &t.path)?;
            pin_hash(&format!("{id}: test {}", t.path), &t.sha256, SHA256)?;
        }
        // 公開と隠しで同じ path を持つことも許さない（隠しが公開を上書きする経路を残さない）。
        no_dups(
            &format!("{id}: test path"),
            self.tests.iter().map(|t| t.path.as_str()),
        )?;
        if self.implementation {
            // 公開テストは初期木の一部（initial の内容 hash が固定する）。初期木が無い case だけ個別に固定させる。
            let public_in_initial = self.initial.is_some();
            for (vis, name) in [
                (TestVisibility::Public, "public"),
                (TestVisibility::Hidden, "hidden"),
            ] {
                if vis == TestVisibility::Public && public_in_initial {
                    continue;
                }
                if !self.tests.iter().any(|t| t.visibility == vis) {
                    return Err(LockError::MissingTests(format!(
                        "{id}: implementation case has no pinned {name} test"
                    )));
                }
            }
        }
        Ok(())
    }

    /// 公開/隠しテスト全件（path・可視性・内容 hash）の整列済み digest。grade 行に写して bind で照合する。
    pub fn tests_sha256(&self) -> String {
        let mut t = self.tests.clone();
        t.sort();
        let mut s = String::new();
        for x in &t {
            s += &format!("{:?}\t{}\t{}\n", x.visibility, x.path, x.sha256);
        }
        format!("{:x}", Sha256::digest(s.as_bytes()))
    }

    fn normalized(&self) -> Self {
        let mut c = self.clone();
        c.inputs.sort_by(|a, b| a.name.cmp(&b.name));
        c.tests.sort();
        if let Some(i) = &mut c.initial {
            i.submodules.sort();
        }
        c
    }
}

impl CampaignLock {
    /// 全 case の grader pin（case id・private・grade-case hash）の整列済み digest。`SetFixed.success` の実測側。
    pub fn grader_set_sha256(&self) -> String {
        let mut c: Vec<&LockedCase> = self.cases.iter().collect();
        c.sort_by(|a, b| a.id.cmp(&b.id));
        let mut s = String::new();
        for x in c {
            s += &format!(
                "{}\t{}\t{}\n",
                x.id, x.grader.private_sha256, x.grader.grade_case_sha256
            );
        }
        format!("{:x}", Sha256::digest(s.as_bytes()))
    }

    /// manifest と lock の本体（cases）から、封印前の lock を作る。
    pub fn new(
        version: &str,
        measurement_id: &str,
        manifest: &CampaignManifest,
        cases: Vec<LockedCase>,
    ) -> Result<Self, LockError> {
        let campaign_sha256 = manifest
            .compute_sha256()
            .map_err(|e| LockError::Schema(format!("campaign hash: {e}")))?;
        Ok(CampaignLock {
            schema: LOCK_SCHEMA.into(),
            version: version.into(),
            measurement_id: measurement_id.into(),
            campaign_sha256,
            materials_sha256: manifest.snapshot.materials_sha256.clone(),
            kb_snapshot: manifest.snapshot.kb_snapshot.clone(),
            cases,
            execution: ExecutionPin::from_manifest(manifest),
            series: None,
            sha256: String::new(),
        })
    }

    pub fn validate(&self) -> Result<(), LockError> {
        if self.schema != LOCK_SCHEMA {
            return Err(LockError::Schema(format!("schema must be {LOCK_SCHEMA}")));
        }
        pin_text("version", &self.version)?;
        pin_text("measurement_id", &self.measurement_id)?;
        pin_hash("campaign_sha256", &self.campaign_sha256, SHA256)?;
        pin_hash("materials_sha256", &self.materials_sha256, SHA256)?;
        pin_text("kb_snapshot", &self.kb_snapshot)?;
        if self.cases.is_empty() {
            return Err(LockError::Unpinned("no case is pinned".into()));
        }
        no_dups("case id", self.cases.iter().map(|c| c.id.as_str()))?;
        for c in &self.cases {
            c.validate()?;
        }
        if self.execution.configs.is_empty() {
            return Err(LockError::Unpinned("execution.configs is empty".into()));
        }
        if self.execution.repeats == 0 {
            return Err(LockError::Unpinned("execution.repeats is 0".into()));
        }
        Ok(())
    }

    /// cases・submodule・入力・テストを整列した写し。
    pub fn normalized(&self) -> Self {
        let mut l = self.clone();
        l.cases = l.cases.iter().map(LockedCase::normalized).collect();
        l.cases.sort_by(|a, b| a.id.cmp(&b.id));
        l.execution.configs.sort_by_key(|c| c.id);
        l
    }

    /// 正規化 JSON（キー順固定、`sha256` 欄は空として数える）の sha256。
    pub fn lock_hash(&self) -> Result<String, LockError> {
        let mut l = self.normalized();
        l.sha256 = String::new();
        let v = serde_json::to_value(&l).map_err(|e| LockError::Schema(e.to_string()))?;
        let bytes =
            serde_json::to_vec(&canonical(&v)).map_err(|e| LockError::Schema(e.to_string()))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    /// 封印が自身の内容と一致するか（保存 lock の改変検出）。
    pub fn check_seal(&self) -> Result<(), LockError> {
        self.validate()?;
        if self.sha256.is_empty() || self.sha256 != self.lock_hash()? {
            return Err(LockError::Schema(
                "lock seal does not match its content".into(),
            ));
        }
        Ok(())
    }

    pub fn case(&self, id: &str) -> Option<&LockedCase> {
        self.cases.iter().find(|c| c.id == id)
    }

    /// 検査してから `sha256` を埋める。
    pub fn seal(mut self) -> Result<Self, LockError> {
        self.validate()?;
        self.sha256 = self.lock_hash()?;
        Ok(self)
    }

    /// lock が与えられた manifest に結び付いているか（campaign hash・資料・KB・実行構成を再計算して照合）。
    pub fn bind_to_manifest(&self, m: &CampaignManifest) -> Result<(), LockError> {
        let h = m
            .compute_sha256()
            .map_err(|e| LockError::Schema(format!("campaign hash: {e}")))?;
        if h != self.campaign_sha256 {
            return Err(LockError::CampaignMismatch(format!(
                "manifest {h} vs lock {}",
                self.campaign_sha256
            )));
        }
        if m.snapshot.materials_sha256 != self.materials_sha256
            || m.snapshot.kb_snapshot != self.kb_snapshot
        {
            return Err(LockError::MaterialsMismatch(
                "manifest snapshot differs from the lock".into(),
            ));
        }
        if ExecutionPin::from_manifest(m) != self.normalized().execution {
            return Err(LockError::ExecutionMismatch(
                "manifest execution config differs from the lock".into(),
            ));
        }
        let mut ids: Vec<&str> = self.cases.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        let mut want: Vec<&str> = m.problems.iter().map(String::as_str).collect();
        want.sort();
        if ids != want {
            return Err(LockError::CaseSetMismatch(
                "manifest problems differ from the locked cases".into(),
            ));
        }
        Ok(())
    }

    /// 共有承認を宣言した manifest で、この lock が同じ台帳宣言（set 宣言 hash）に結び付いているか。
    /// 宣言の無い旧 lock（`sets_sha256` 空）や別の宣言の lock は、台帳との結び付きが不明なので投入前に拒否する。
    /// 非共有の manifest は従来どおり（ここでは何も見ない）。
    pub fn check_shared_ledger(&self, m: &CampaignManifest) -> Result<(), String> {
        if matches!(m.shared_ledger_decl(), Ok(None)) {
            return Ok(());
        }
        if self.execution.sets_sha256.is_empty() {
            return Err(
                "lock does not bind the shared ledger declaration (no set hash); re-lock the approved manifest"
                    .into(),
            );
        }
        self.bind_to_manifest(m)
            .map_err(|e| format!("lock is not bound to this shared approval: {e}"))
    }

    fn check_sealed(&self, which: &str) -> Result<(), LockError> {
        self.validate()?;
        let h = self.lock_hash()?;
        if self.sha256 != h {
            return Err(LockError::SealMismatch(format!(
                "{which}: recorded {:?}, recomputed {h}",
                self.sha256
            )));
        }
        Ok(())
    }
}

fn diff_case(e: &LockedCase, a: &LockedCase) -> Option<LockError> {
    let id = &e.id;
    if e.split != a.split {
        return Some(LockError::CaseSetMismatch(format!("{id}: split differs")));
    }
    if e.initial != a.initial {
        let (Some(ei), Some(ai)) = (&e.initial, &a.initial) else {
            return Some(LockError::InitialMismatch(format!(
                "{id}: initial tree presence differs"
            )));
        };
        if ei.submodules != ai.submodules {
            let paths = |i: &LockedInitial| -> BTreeSet<String> {
                i.submodules.iter().map(|s| s.path.clone()).collect()
            };
            let (pe, pa) = (paths(ei), paths(ai));
            let why = if let Some(p) = pe.difference(&pa).next() {
                format!("missing submodule {p}")
            } else if let Some(p) = pa.difference(&pe).next() {
                format!("extra submodule {p}")
            } else {
                "submodule commit or content differs".into()
            };
            return Some(LockError::SubmoduleMismatch(format!("{id}: {why}")));
        }
        return Some(LockError::InitialMismatch(format!(
            "{id}: commit/tree/content differs"
        )));
    }
    if e.inputs != a.inputs {
        return Some(LockError::InputMismatch(format!("{id}: inputs differ")));
    }
    if e.grader != a.grader {
        return Some(LockError::GraderMismatch(format!("{id}: grader differs")));
    }
    if e.tests != a.tests || e.implementation != a.implementation {
        return Some(LockError::TestMismatch(format!(
            "{id}: public/hidden tests differ"
        )));
    }
    None
}

/// `expected`（信頼できる側）と `actual` を照合する。どちらも検査・封印の再計算を通ること。
pub fn verify_lock(expected: &CampaignLock, actual: &CampaignLock) -> Result<(), LockError> {
    expected.check_sealed("expected")?;
    actual.check_sealed("actual")?;
    if expected.version != actual.version {
        return Err(LockError::VersionMismatch {
            expected: expected.version.clone(),
            actual: actual.version.clone(),
        });
    }
    if expected.measurement_id != actual.measurement_id {
        return Err(LockError::MeasurementMismatch {
            expected: expected.measurement_id.clone(),
            actual: actual.measurement_id.clone(),
        });
    }
    if expected.sha256 == actual.sha256 {
        return Ok(());
    }
    // 同じ測定 ID・版で内容が違う。原因を絞って返す。
    if expected.campaign_sha256 != actual.campaign_sha256 {
        return Err(LockError::CampaignMismatch(format!(
            "{} vs {}",
            expected.campaign_sha256, actual.campaign_sha256
        )));
    }
    if expected.materials_sha256 != actual.materials_sha256 {
        return Err(LockError::MaterialsMismatch(
            "materials_sha256 differs".into(),
        ));
    }
    if expected.kb_snapshot != actual.kb_snapshot {
        return Err(LockError::MaterialsMismatch("kb_snapshot differs".into()));
    }
    let (en, an) = (expected.normalized(), actual.normalized());
    let ids =
        |l: &CampaignLock| -> BTreeSet<String> { l.cases.iter().map(|c| c.id.clone()).collect() };
    let (ie, ia) = (ids(&en), ids(&an));
    if let Some(x) = ie.difference(&ia).next() {
        return Err(LockError::CaseSetMismatch(format!("missing case {x}")));
    }
    if let Some(x) = ia.difference(&ie).next() {
        return Err(LockError::CaseSetMismatch(format!("extra case {x}")));
    }
    for (e, a) in en.cases.iter().zip(&an.cases) {
        if let Some(err) = diff_case(e, a) {
            return Err(err);
        }
    }
    if en.execution != an.execution {
        return Err(LockError::ExecutionMismatch("execution pin differs".into()));
    }
    Err(LockError::ContentChanged("lock hash differs".into()))
}

/// 内容を変えた新しい lock が旧 lock の後継として許されるか。別の測定 ID であること、旧 lock が検査を通ることだけを求める。
/// 旧 lock・grader 条件を書き換える経路は無い（新 lock は新しい測定 ID で別に封印する）。
pub fn check_successor(old: &CampaignLock, new: &CampaignLock) -> Result<(), LockError> {
    old.check_sealed("old")?;
    new.check_sealed("new")?;
    if old.measurement_id == new.measurement_id {
        return Err(LockError::NotSuccessor(
            "the measurement id is unchanged; changed content needs a new measurement id".into(),
        ));
    }
    if old.sha256 == new.sha256 {
        return Err(LockError::NotSuccessor(
            "content is identical; nothing to supersede".into(),
        ));
    }
    // variant は後継では差し替えられない（再 seal で別 variant・別内容に付け替えて通さない）。
    if old.execution.variant != new.execution.variant {
        return Err(LockError::NotSuccessor(
            "the variant reference differs; a variant cannot be swapped by a successor lock".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::fixtures::{campaign, input};

    type Edit = Box<dyn Fn(&mut CampaignLock)>;

    fn h(c: char) -> String {
        c.to_string().repeat(64)
    }
    fn g(c: char) -> String {
        c.to_string().repeat(40)
    }

    fn case(id: &str) -> LockedCase {
        LockedCase {
            id: id.into(),
            split: Split::Dev,
            implementation: true,
            initial: Some(LockedInitial {
                commit: g('a'),
                tree: g('b'),
                content_sha256: h('1'),
                submodules: vec![
                    LockedSubmodule {
                        path: "ext/b".into(),
                        commit: g('c'),
                        content_sha256: h('2'),
                    },
                    LockedSubmodule {
                        path: "ext/a".into(),
                        commit: g('d'),
                        content_sha256: h('3'),
                    },
                ],
            }),
            inputs: vec![DeliveredInput {
                name: "data.tgz".into(),
                sha256: h('4'),
            }],
            grader: GraderPin {
                private_sha256: h('5'),
                grade_case_sha256: h('6'),
            },
            tests: vec![
                LockedTest {
                    path: "tests/pub.rs".into(),
                    visibility: TestVisibility::Public,
                    sha256: h('7'),
                },
                LockedTest {
                    path: "tests/hid.rs".into(),
                    visibility: TestVisibility::Hidden,
                    sha256: h('8'),
                },
            ],
        }
    }

    fn lock() -> (CampaignManifest, CampaignLock) {
        let i = input(2, "dev");
        let mut m = campaign(&i, Split::Dev, 2);
        m.snapshot.materials_sha256 = h('e');
        let l = CampaignLock::new("1", "m-1", &m, vec![case("p1"), case("p0")])
            .expect("new")
            .seal()
            .expect("seal");
        (m, l)
    }

    fn reseal(mut l: CampaignLock) -> CampaignLock {
        l.sha256 = String::new();
        l.seal().expect("seal")
    }

    #[test]
    fn hash_is_order_insensitive_and_verifies() {
        let (m, l) = lock();
        let mut r = l.clone();
        r.cases.reverse();
        r.cases[0].initial.as_mut().expect("i").submodules.reverse();
        r.cases[1].tests.reverse();
        assert_eq!(r.lock_hash().expect("h"), l.sha256);
        assert!(verify_lock(&l, &reseal(r)).is_ok());
        assert!(l.bind_to_manifest(&m).is_ok());
    }

    #[test]
    fn shared_approval_needs_a_lock_bound_to_the_ledger_declaration() {
        use crate::manifest::{ConfigId, ExperimentSet, SetFixed};
        let (m, legacy) = lock();
        // 旧形式（共有承認の宣言なし）は従来どおり何も見ない。
        assert!(legacy.check_shared_ledger(&m).is_ok());
        let shared = |p: &str| {
            let mut s = m.clone();
            s.experiment_sets.insert(
                "all".into(),
                ExperimentSet {
                    arms: vec![ConfigId::A, ConfigId::B, ConfigId::C],
                    compare: vec![(ConfigId::A, ConfigId::B), (ConfigId::B, ConfigId::C)],
                    fixed: SetFixed {
                        approved_trials: Some(4),
                        shared_ledger: Some(p.into()),
                        ..Default::default()
                    },
                },
            );
            s
        };
        let s = shared("/approved/ledger");
        let bound = |m: &CampaignManifest| {
            CampaignLock::new("1", "m-1", m, vec![case("p1"), case("p0")])
                .expect("new")
                .seal()
                .expect("seal")
        };
        assert!(bound(&s).check_shared_ledger(&s).is_ok());
        // 宣言の無い旧 lock は結び付きが不明なので拒否。
        let e = legacy.check_shared_ledger(&s).expect_err("legacy lock");
        assert!(e.contains("does not bind the shared ledger"), "{e}");
        // 別の台帳 path を宣言した manifest の lock は使えない（承認と lock の台帳が食い違う）。
        let e = bound(&shared("/other/ledger"))
            .check_shared_ledger(&s)
            .expect_err("other ledger");
        assert!(e.contains("not bound to this shared approval"), "{e}");
    }

    #[test]
    fn lock_is_bound_to_the_campaign_hash() {
        let (m, l) = lock();
        let mut m2 = m.clone();
        m2.budget.max_tokens += 1;
        assert!(matches!(
            l.bind_to_manifest(&m2),
            Err(LockError::CampaignMismatch(_))
        ));
        let mut l2 = l.clone();
        l2.campaign_sha256 = h('9');
        assert!(matches!(
            verify_lock(&l, &reseal(l2)),
            Err(LockError::CampaignMismatch(_))
        ));
    }

    #[test]
    fn duplicates_and_bad_paths_are_rejected() {
        let (_, l) = lock();
        let mut d = l.clone();
        d.cases[1].id = d.cases[0].id.clone();
        assert!(matches!(d.validate(), Err(LockError::Duplicate(_))));
        let mut d = l.clone();
        let subs = &mut d.cases[0].initial.as_mut().expect("i").submodules;
        subs[1].path = subs[0].path.clone();
        assert!(matches!(d.validate(), Err(LockError::Duplicate(_))));
        let mut d = l.clone();
        d.cases[0].tests[1].path = d.cases[0].tests[0].path.clone();
        assert!(matches!(d.validate(), Err(LockError::Duplicate(_))));
        for bad in ["/abs", "a/../b", "", "a//b"] {
            let mut d = l.clone();
            d.cases[0].initial.as_mut().expect("i").submodules[0].path = bad.into();
            assert!(d.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn unpinned_values_are_rejected() {
        let (_, l) = lock();
        let edits: Vec<Edit> = vec![
            Box::new(|l| l.measurement_id.clear()),
            Box::new(|l| l.version.clear()),
            Box::new(|l| l.materials_sha256.clear()),
            Box::new(|l| l.kb_snapshot.clear()),
            Box::new(|l| l.cases.clear()),
            Box::new(|l| l.cases[0].initial.as_mut().expect("i").tree = String::new()),
            Box::new(|l| l.cases[0].initial.as_mut().expect("i").commit = "abc".into()),
            Box::new(|l| l.cases[0].initial.as_mut().expect("i").content_sha256 = h('A')),
            Box::new(|l| {
                l.cases[0].initial.as_mut().expect("i").submodules[0]
                    .content_sha256
                    .clear()
            }),
            Box::new(|l| l.cases[0].inputs[0].sha256.clear()),
            Box::new(|l| l.cases[0].grader.grade_case_sha256.clear()),
            Box::new(|l| l.cases[0].tests[0].sha256.clear()),
        ];
        for (n, e) in edits.iter().enumerate() {
            let mut d = l.clone();
            e(&mut d);
            assert!(d.validate().is_err(), "edit {n}");
            assert!(verify_lock(&l, &d).is_err(), "edit {n}");
        }
    }

    #[test]
    fn implementation_case_must_pin_public_and_hidden_tests() {
        let (_, l) = lock();
        // 初期木が無い case は公開テストも個別に固定する。初期木がある case は公開テストを initial の内容 hash が固定する。
        for vis in [TestVisibility::Public, TestVisibility::Hidden] {
            let mut d = l.clone();
            d.cases[0].initial = None;
            d.cases[0].tests.retain(|t| t.visibility != vis);
            assert!(matches!(d.validate(), Err(LockError::MissingTests(_))));
        }
        let mut with_initial = l.clone();
        with_initial.cases[0]
            .tests
            .retain(|t| t.visibility != TestVisibility::Public);
        assert!(with_initial.validate().is_ok());
        with_initial.cases[0].tests.clear();
        assert!(matches!(
            with_initial.validate(),
            Err(LockError::MissingTests(_))
        ));
        let mut d = l.clone();
        d.cases[0].tests.clear();
        assert!(matches!(d.validate(), Err(LockError::MissingTests(_))));
        // 実装でない case は空でよい。
        let mut ok = l.clone();
        ok.cases[0].implementation = false;
        ok.cases[0].tests.clear();
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn version_and_measurement_differences_are_distinguished() {
        let (_, l) = lock();
        let mut v = l.clone();
        v.version = "2".into();
        assert!(matches!(
            verify_lock(&l, &reseal(v)),
            Err(LockError::VersionMismatch { .. })
        ));
        let mut m = l.clone();
        m.measurement_id = "m-2".into();
        assert!(matches!(
            verify_lock(&l, &reseal(m)),
            Err(LockError::MeasurementMismatch { .. })
        ));
    }

    #[test]
    fn same_measurement_with_changed_content_is_rejected_per_kind() {
        let (_, l) = lock();
        let cases: Vec<(&str, Edit)> = vec![
            ("materials", Box::new(|l| l.materials_sha256 = h('9'))),
            ("kb", Box::new(|l| l.kb_snapshot = "other".into())),
            (
                "grader",
                Box::new(|l| l.cases[0].grader.private_sha256 = h('9')),
            ),
            (
                "grade_case",
                Box::new(|l| l.cases[1].grader.grade_case_sha256 = h('9')),
            ),
            (
                "hidden test",
                Box::new(|l| l.cases[0].tests[0].sha256 = h('9')),
            ),
            ("input", Box::new(|l| l.cases[0].inputs[0].sha256 = h('9'))),
            (
                "tree",
                Box::new(|l| l.cases[0].initial.as_mut().expect("i").tree = g('9')),
            ),
            (
                "content",
                Box::new(|l| l.cases[0].initial.as_mut().expect("i").content_sha256 = h('9')),
            ),
            (
                "sub swap",
                Box::new(|l| l.cases[0].initial.as_mut().expect("i").submodules[0].commit = g('9')),
            ),
            (
                "sub content",
                Box::new(|l| {
                    l.cases[0].initial.as_mut().expect("i").submodules[1].content_sha256 = h('9')
                }),
            ),
            (
                "sub removed",
                Box::new(|l| {
                    l.cases[0].initial.as_mut().expect("i").submodules.pop();
                }),
            ),
            (
                "sub added",
                Box::new(|l| {
                    l.cases[0]
                        .initial
                        .as_mut()
                        .expect("i")
                        .submodules
                        .push(LockedSubmodule {
                            path: "x".into(),
                            commit: g('9'),
                            content_sha256: h('9'),
                        })
                }),
            ),
            (
                "case removed",
                Box::new(|l| {
                    l.cases.pop();
                }),
            ),
            (
                "case added",
                Box::new(|l| {
                    let mut c = case("p9");
                    c.id = "p9".into();
                    l.cases.push(c);
                }),
            ),
            ("exec", Box::new(|l| l.execution.seed += 1)),
        ];
        for (name, e) in cases {
            let mut d = l.clone();
            e(&mut d);
            let r = verify_lock(&l, &reseal(d));
            assert!(r.is_err(), "{name} accepted");
            assert!(
                !matches!(r, Err(LockError::ContentChanged(_))),
                "{name}: unclassified"
            );
        }
        let mut d = l.clone();
        d.cases[0].initial.as_mut().expect("i").submodules[0].commit = g('9');
        assert!(matches!(
            verify_lock(&l, &reseal(d)),
            Err(LockError::SubmoduleMismatch(_))
        ));
        let mut d = l.clone();
        d.cases[0].tests[0].sha256 = h('9');
        assert!(matches!(
            verify_lock(&l, &reseal(d)),
            Err(LockError::TestMismatch(_))
        ));
    }

    #[test]
    fn tampered_content_without_resealing_is_rejected() {
        let (_, l) = lock();
        let mut d = l.clone();
        d.cases[0].grader.private_sha256 = h('9');
        assert!(matches!(
            verify_lock(&l, &d),
            Err(LockError::SealMismatch(_))
        ));
        // 期待側の改変も拒否（信頼側も再計算で照合する）。
        assert!(matches!(
            verify_lock(&d, &l),
            Err(LockError::SealMismatch(_))
        ));
    }

    #[test]
    fn changed_content_needs_a_new_measurement_id() {
        let (_, l) = lock();
        let mut n = l.clone();
        n.cases[0].grader.private_sha256 = h('9');
        let same_id = reseal(n.clone());
        assert!(matches!(
            check_successor(&l, &same_id),
            Err(LockError::NotSuccessor(_))
        ));
        n.measurement_id = "m-2".into();
        let new = reseal(n);
        assert!(check_successor(&l, &new).is_ok());
        // 旧 lock は新 lock で上書きされず、旧 lock の照合は旧内容を基準にし続ける。
        assert!(verify_lock(&l, &new).is_err());
        assert!(verify_lock(&l, &l.clone()).is_ok());
        // 内容が同じなら後継ではない。
        let mut same = l.clone();
        same.measurement_id = "m-2".into();
        assert!(check_successor(&l, &reseal(same.clone())).is_ok());
        assert!(check_successor(&l, &l.clone()).is_err());
    }

    #[test]
    fn adding_a_nested_submodule_is_new_content_needing_a_new_measurement_id() {
        // 入れ子 submodule の snapshot を足すのは asset の変更。旧測定の lock には混ぜず、新しい測定 ID にだけ載る。
        let (_, l) = lock();
        let mut n = l.clone();
        let parent = n.cases[0].initial.as_ref().expect("i").submodules[0]
            .path
            .clone();
        n.cases[0]
            .initial
            .as_mut()
            .expect("i")
            .submodules
            .push(LockedSubmodule {
                path: format!("{parent}/inner/nested"),
                commit: g('7'),
                content_sha256: h('7'),
            });
        let same_id = reseal(n.clone());
        assert!(matches!(
            check_successor(&l, &same_id),
            Err(LockError::NotSuccessor(_))
        ));
        assert!(verify_lock(&l, &same_id).is_err());
        n.measurement_id = "m-2".into();
        let new = reseal(n);
        assert!(check_successor(&l, &new).is_ok());
        assert!(verify_lock(&l, &new).is_err());
        assert!(verify_lock(&l, &l.clone()).is_ok());
    }
}
