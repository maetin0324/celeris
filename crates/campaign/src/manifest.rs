//! campaign manifest: 検証・正規化・sha256。dev pilot と final の混在を型と検査で拒否する。
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use routing_eval::manifest::Manifest as InputManifest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{CAMPAIGN_SCHEMA, SIDE_CAMPAIGN_SCHEMA};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    Dev,
    Final,
}

impl Split {
    /// routing-eval の問題の split 名（`dev` / `held-out`）への対応。
    pub fn input_name(self) -> &'static str {
        match self {
            Split::Dev => "dev",
            Split::Final => "held-out",
        }
    }
}

/// A=既存の強い運用 baseline、B=Celeris+fixed、C=Celeris+Jev。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfigId {
    A,
    B,
    C,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub id: ConfigId,
    pub harness: String,
    pub harness_version: String,
    pub model: String,
    pub model_version: String,
    /// 方策の版（A は baseline の運用版、B は fixed、C は Jev prompt/model 版を含む）。
    pub policy_version: String,
    /// B/C の task 単位 routing 固定（下段・根拠表・オンライン更新）。mode は arm で決まる（B=fixed、
    /// C=decision_assisted）。A には書かない。省略時は直列化しない（既存 manifest の hash を変えない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<ArmRouting>,
}

impl Config {
    /// この arm の worker が実際に使う harness の集合（`routing.run_harnesses`。無ければ `harness`）。B/C の能力 probe は
    /// この各 harness の adapter 環境で検査する。
    pub fn worker_harnesses(&self) -> Vec<String> {
        match self.routing.as_ref().and_then(|r| r.run_harnesses.clone()) {
            Some(h) if !h.is_empty() => h,
            _ => vec![self.harness.clone()],
        }
    }
}

/// 試行ごとに task へ固定する routing 設定（`POST /tasks` の `routing_override` の mode 以外）。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArmRouting {
    /// 下段（fixed 方策・lane→model 対応表）の版。省略時は Celeris の既定版を明示して固定する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lower_policy: Option<String>,
    /// 根拠表 `routing_evidence` の版。C（decision_assisted）では必須（最新版への暗黙の追従を許さない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_table_id: Option<String>,
    /// run の結果をオンライン更新の根拠へ追記するか。省略時は false（試行が根拠表を動かさない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online_update: Option<bool>,
    /// この arm で使ってよい実行モデル（`model_id`）の集合。宣言すると全 run の実測がこの内側かを照合する。
    /// B と C は同じ集合・同じ tier 対応表（`lower_policy`）で方策だけを変える（両方が宣言したら一致が必要）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    /// この arm の run が使ってよい harness（`harness_id`）の集合。`models` と同じく実測と照合する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_harnesses: Option<Vec<String>>,
    /// C が実測の根拠表を使わないことの明示。true のとき `evidence_table_id` は要らず（書いてもいけない）、
    /// decision_assisted のためだけに根拠表作成や全モデル能力評価を要求しない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_evidence_table: Option<bool>,
}

impl ArmRouting {
    /// この arm の実効 evidence_mode（`none` / `pinned` / `latest`）。`POST /tasks` の `routing_override` へ載せ、
    /// checker が観測値と照合する。`no_evidence_table=true`→none、`evidence_table_id`→pinned、
    /// どちらも無い B は根拠表を読まない方策なので none。C で両方無い（= latest の暗黙選択）は Err。
    pub fn effective_evidence_mode(&self, id: ConfigId) -> Result<&'static str, String> {
        let ev = self.evidence_table_id.as_deref().unwrap_or("").trim();
        if self.no_evidence_table == Some(true) {
            if !ev.is_empty() {
                return Err(format!(
                    "config {id:?}: routing.no_evidence_table contradicts evidence_table_id"
                ));
            }
            return Ok("none");
        }
        if !ev.is_empty() {
            return Ok("pinned");
        }
        if id == ConfigId::C {
            return Err("config C: evidence_mode would be an implicit latest; pin routing.evidence_table_id or set no_evidence_table=true".into());
        }
        Ok("none")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InputRef {
    /// campaign manifest からの相対 path（dry-run の `--input-manifest` で上書き可）。
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SnapshotRef {
    pub repo_commit: String,
    pub materials_sha256: String,
    pub kb_snapshot: String,
}

/// 予算判定に数える tokens の基準（ADR 2026-10-16-mlsys-pilot-wiring-fixes D7）。manifest で必須（既定なし）。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TokenBasis {
    /// input + output（cache を数えない）。
    InputOutput,
    /// input + output + cache_read。
    InputOutputCacheRead,
    /// input + output + cache_read + cache_creation。
    All,
}

/// 1 試行（または実行中の累積）の tokens の内訳。cache が取れない harness は None（欠測。0 にしない）。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: Option<u64>,
    pub cache_creation: Option<u64>,
}

impl TokenUsage {
    /// cache の内訳まで分かっている使用量。
    pub fn full(input: u64, output: u64, cache_read: u64, cache_creation: u64) -> Self {
        Self {
            input,
            output,
            cache_read: Some(cache_read),
            cache_creation: Some(cache_creation),
        }
    }

    /// 基準で数えた tokens。基準に要る量が欠けていれば None（欠測）。
    pub fn count(&self, basis: TokenBasis) -> Option<u64> {
        let io = self.input + self.output;
        match basis {
            TokenBasis::InputOutput => Some(io),
            TokenBasis::InputOutputCacheRead => Some(io + self.cache_read?),
            TokenBasis::All => Some(io + self.cache_read? + self.cache_creation?),
        }
    }

    /// 内訳ごとの和（どちらかが欠測なら欠測）。
    pub fn add(&self, o: &TokenUsage) -> TokenUsage {
        TokenUsage {
            input: self.input + o.input,
            output: self.output + o.output,
            cache_read: self.cache_read.zip(o.cache_read).map(|(a, b)| a + b),
            cache_creation: self
                .cache_creation
                .zip(o.cache_creation)
                .map(|(a, b)| a + b),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Budget {
    /// 予算判定に数える tokens の基準。必須（暗黙の既定を置かない）。
    pub token_basis: TokenBasis,
    /// campaign 全体（全 arm の総量）の上限。
    pub max_tokens: u64,
    pub max_wall_secs: u64,
    pub max_disk_bytes: u64,
    pub max_parallel: u32,
    /// 1 試行の上限。実行中に適用する（ADR 2026-10-16-campaign-budget-cap）。
    pub per_case: Cap,
    /// 1 arm の累積上限。超えた arm の残りの試行だけを止める（他 arm は止めない）。
    pub per_arm: Cap,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cap {
    pub max_tokens: u64,
    pub max_wall_secs: u64,
}

impl Budget {
    /// 欠測 tokens を保守的に「1 試行の上限を使い切った」とみなした課金値。
    pub fn charged_tokens(&self, tokens: Option<u64>) -> u64 {
        tokens.unwrap_or(self.per_case.max_tokens)
    }
}

/// problem 内の arm 実行順（順序交絡の対策）。ADR 2026-10-16-campaign-arm-order-isolation D1。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArmOrder {
    /// problem の実行位置で計画 arm 列を巡回させる（A,B,C → B,C,A → C,A,B …）。
    #[default]
    AbcRotation,
    /// problem id と seed から決定的に並べ替える。
    SeededShuffle,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Termination {
    pub trial_timeout_secs: u64,
    pub max_attempts_per_trial: u32,
    pub stop_on_budget: bool,
}

/// 事前登録の副比較（B/C）。subset は manifest に宣言する。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Secondary {
    pub pair: (ConfigId, ConfigId),
    pub subset: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CampaignManifest {
    pub schema: String,
    pub campaign_id: String,
    pub version: String,
    pub split: Split,
    pub input: InputRef,
    pub problems: Vec<String>,
    pub configs: Vec<Config>,
    /// 主比較。常に (A, B)。
    pub primary: (ConfigId, ConfigId),
    pub secondary: Secondary,
    pub budget: Budget,
    pub termination: Termination,
    /// problem 内の arm 順。依存順 `problem_order` は常に守る。
    /// 既定（abc_rotation）は直列化しない（既存 manifest の hash を変えない）。
    #[serde(default, skip_serializing_if = "is_default_order")]
    pub order: ArmOrder,
    /// `seeded_shuffle` の種。`abc_rotation` では使わないが記録する。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub seed: u64,
    /// 独立反復の回数（別 ID・別 out の `rep` 1..=repeats）。内部再試行（attempt）とは別。
    /// 1 は直列化しない（既存 manifest の hash を変えない）。ADR 2026-10-16-campaign-repeats-denominator。
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub repeats: u32,
    pub snapshot: SnapshotRef,
    /// problem id -> 前提 problem id の配列。前提の全 config の trial が passed になるまで依存先は実行しない。
    /// 空なら直列化しない（依存の無い manifest の hash を変えない）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub sha256: String,
}

fn is_default_order(o: &ArmOrder) -> bool {
    *o == ArmOrder::default()
}

fn one() -> u32 {
    1
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

pub(crate) fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| (k.clone(), canonical(x)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        o => o.clone(),
    }
}

impl CampaignManifest {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("campaign manifest: {e}"))
    }

    /// 並びを正規化する（problems・subset・configs を整列）。
    pub fn normalized(&self) -> Self {
        let mut m = self.clone();
        m.problems.sort();
        m.secondary.subset.sort();
        m.configs.sort_by_key(|c| c.id);
        for v in m.dependencies.values_mut() {
            v.sort();
            v.dedup();
        }
        m.dependencies.retain(|_, v| !v.is_empty());
        m
    }

    /// problem 内の arm 順。`pos` は problem_order 後の添字。決定的（seed と id のみに依存）。
    pub fn arm_sequence(&self, problem: &str, pos: usize, arms: Vec<ConfigId>) -> Vec<ConfigId> {
        let mut v = arms;
        let n = v.len();
        if n < 2 {
            return v;
        }
        match self.order {
            ArmOrder::AbcRotation => v.rotate_left(pos % n),
            ArmOrder::SeededShuffle => {
                let mut h = Sha256::new();
                h.update(self.seed.to_le_bytes());
                h.update(problem.as_bytes());
                let d = h.finalize();
                let mut state = u64::from_le_bytes(d[..8].try_into().unwrap_or([0; 8]));
                for i in (1..n).rev() {
                    // splitmix64
                    state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
                    let mut z = state;
                    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                    z ^= z >> 31;
                    v.swap(i, (z % (i as u64 + 1)) as usize);
                }
            }
        }
        v
    }

    /// 問題の実行順（前提が先。依存が無ければ id 昇順）。未知 id・自己依存・循環は Err。
    pub fn problem_order(&self) -> Result<Vec<String>, String> {
        let known: BTreeSet<&str> = self.problems.iter().map(String::as_str).collect();
        let mut pending: BTreeMap<&str, BTreeSet<&str>> =
            known.iter().map(|p| (*p, BTreeSet::new())).collect();
        for (p, qs) in &self.dependencies {
            if !known.contains(p.as_str()) {
                return Err(format!("dependencies: unknown problem id {p}"));
            }
            for q in qs {
                if !known.contains(q.as_str()) {
                    return Err(format!(
                        "dependencies: {p} depends on unknown problem id {q}"
                    ));
                }
                if q == p {
                    return Err(format!("dependencies: {p} depends on itself"));
                }
                if let Some(s) = pending.get_mut(p.as_str()) {
                    s.insert(q.as_str());
                }
            }
        }
        let mut order = Vec::new();
        while !pending.is_empty() {
            let Some(next) = pending.iter().find(|(_, d)| d.is_empty()).map(|(p, _)| *p) else {
                let rest: Vec<&str> = pending.keys().copied().collect();
                return Err(format!("dependencies contain a cycle among {rest:?}"));
            };
            pending.remove(next);
            for d in pending.values_mut() {
                d.remove(next);
            }
            order.push(next.to_string());
        }
        Ok(order)
    }

    /// problem を実行する config（A/B は全問題、C は事前登録 subset のみ）。
    pub fn planned_configs(&self, problem: &str) -> Vec<ConfigId> {
        if self.is_side() {
            return vec![ConfigId::B, ConfigId::C];
        }
        let mut v = vec![ConfigId::A, ConfigId::B];
        if self.secondary.subset.iter().any(|s| s == problem) {
            v.push(ConfigId::C);
        }
        v
    }

    /// Jev 副比較（B1/C1、A なし）の別測定 manifest か。
    pub fn is_side(&self) -> bool {
        self.schema == SIDE_CAMPAIGN_SCHEMA
    }

    /// 正規化後の canonical JSON（キー順固定）の sha256。`sha256` 欄は空として数える。
    pub fn compute_sha256(&self) -> Result<String, String> {
        let mut m = self.normalized();
        m.sha256 = String::new();
        let v = canonical(&serde_json::to_value(&m).map_err(|e| e.to_string())?);
        let bytes = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    /// 構造検査（入力 manifest なし）。
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != CAMPAIGN_SCHEMA && self.schema != SIDE_CAMPAIGN_SCHEMA {
            return Err(format!(
                "schema must be {CAMPAIGN_SCHEMA} or {SIDE_CAMPAIGN_SCHEMA}"
            ));
        }
        let side = self.is_side();
        for (name, v) in [
            ("campaign_id", &self.campaign_id),
            ("version", &self.version),
            ("input.sha256", &self.input.sha256),
            ("snapshot.repo_commit", &self.snapshot.repo_commit),
            ("snapshot.materials_sha256", &self.snapshot.materials_sha256),
            ("snapshot.kb_snapshot", &self.snapshot.kb_snapshot),
        ] {
            if v.trim().is_empty() {
                return Err(format!("{name} is empty"));
            }
        }
        let ids: Vec<ConfigId> = self.normalized().configs.iter().map(|c| c.id).collect();
        if side {
            if ids != [ConfigId::B, ConfigId::C] {
                return Err("side manifest configs must contain exactly B and C once each".into());
            }
        } else if ids != [ConfigId::A, ConfigId::B, ConfigId::C] {
            return Err("configs must contain exactly A, B and C once each".into());
        }
        for c in &self.configs {
            for (n, v) in [
                ("harness", &c.harness),
                ("harness_version", &c.harness_version),
                ("model", &c.model),
                ("model_version", &c.model_version),
                ("policy_version", &c.policy_version),
            ] {
                if v.trim().is_empty() {
                    return Err(format!("config {:?}: {n} is empty", c.id));
                }
            }
            if c.id == ConfigId::A && c.routing.is_some() {
                return Err("config A: routing is for Celeris arms (B/C) only".into());
            }
        }
        let primary = if side {
            (ConfigId::B, ConfigId::C)
        } else {
            (ConfigId::A, ConfigId::B)
        };
        if self.primary != primary {
            return Err(format!("primary comparison must be {primary:?}"));
        }
        if self.secondary.pair != (ConfigId::B, ConfigId::C) {
            return Err("secondary comparison must be (B, C)".into());
        }
        let b = &self.budget;
        if b.max_tokens == 0 || b.max_wall_secs == 0 || b.max_disk_bytes == 0 || b.max_parallel == 0
        {
            return Err("budget fields must all be > 0".into());
        }
        let (c, a) = (b.per_case, b.per_arm);
        if c.max_tokens == 0 || c.max_wall_secs == 0 || a.max_tokens == 0 || a.max_wall_secs == 0 {
            return Err("budget per_case/per_arm fields must all be > 0".into());
        }
        if a.max_tokens < c.max_tokens || a.max_wall_secs < c.max_wall_secs {
            return Err("budget per_arm must be >= per_case".into());
        }
        let t = &self.termination;
        if t.trial_timeout_secs == 0 || t.max_attempts_per_trial == 0 {
            return Err("termination fields must be > 0".into());
        }
        if t.max_attempts_per_trial != 1 {
            return Err(
                "termination.max_attempts_per_trial must be 1 (use repeats for independent repetitions)"
                    .into(),
            );
        }
        if self.repeats == 0 {
            return Err("repeats must be >= 1".into());
        }
        if self.problems.is_empty() {
            return Err("problems is empty".into());
        }
        let set: BTreeSet<&str> = self.problems.iter().map(String::as_str).collect();
        if set.len() != self.problems.len() {
            return Err("problems contain duplicates".into());
        }
        if self.secondary.subset.is_empty() {
            return Err("secondary.subset must be declared (pre-registered)".into());
        }
        let sub: BTreeSet<&str> = self.secondary.subset.iter().map(String::as_str).collect();
        if sub.len() != self.secondary.subset.len() || !sub.is_subset(&set) {
            return Err("secondary.subset must be unique and contained in problems".into());
        }
        self.problem_order()?;
        if self.split == Split::Final {
            self.check_resolved()?;
        }
        if side {
            self.check_side_variant()?;
        }
        if !self.sha256.is_empty() && self.sha256 != self.compute_sha256()? {
            return Err("sha256 does not match manifest content".into());
        }
        Ok(())
    }

    /// 本測定（final）の版・routing の解決検査。未解決の placeholder（`<…>`）・installed alias・
    /// B/C の routing 欠落（C は根拠表の版も）は起動前に落とす（preflight が同じ検査を使う）。
    pub fn check_resolved(&self) -> Result<(), String> {
        for c in &self.configs {
            for (n, v) in [
                ("harness", &c.harness),
                ("harness_version", &c.harness_version),
                ("model", &c.model),
                ("model_version", &c.model_version),
                ("policy_version", &c.policy_version),
            ] {
                if v.contains('<') || v.contains('>') {
                    return Err(format!(
                        "config {:?}: {n} is an unresolved placeholder: {v}",
                        c.id
                    ));
                }
            }
            let hv = c.harness_version.to_ascii_lowercase();
            if ["installed", "latest", "current", "head"]
                .iter()
                .any(|a| hv.contains(a))
            {
                return Err(format!(
                    "config {:?}: harness_version {:?} is an alias; pin the actual version",
                    c.id, c.harness_version
                ));
            }
            if c.harness == "celeris"
                && !(c.harness_version.len() == 12
                    && c.harness_version.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(format!(
                    "config {:?}: celeris harness_version must be a 12-hex release sha",
                    c.id
                ));
            }
            if c.id == ConfigId::A {
                continue;
            }
            let Some(r) = &c.routing else {
                return Err(format!("config {:?}: routing must be pinned", c.id));
            };
            if r.lower_policy
                .as_deref()
                .is_none_or(|v| v.trim().is_empty())
            {
                return Err(format!(
                    "config {:?}: routing.lower_policy must be pinned",
                    c.id
                ));
            }
            if r.online_update != Some(false) {
                return Err(format!(
                    "config {:?}: routing.online_update must be false",
                    c.id
                ));
            }
            let ev = r.evidence_table_id.as_deref().unwrap_or("").trim();
            if r.no_evidence_table == Some(true) && (c.id != ConfigId::C || !ev.is_empty()) {
                return Err(format!(
                    "config {:?}: routing.no_evidence_table is for C only and excludes evidence_table_id",
                    c.id
                ));
            }
            r.effective_evidence_mode(c.id)?;
            if c.id == ConfigId::C && r.no_evidence_table != Some(true) {
                if ev.is_empty() {
                    return Err("config C: routing.evidence_table_id must be pinned".into());
                }
                if ev.contains(['<', '>']) {
                    return Err(format!(
                        "config C: evidence_table_id is an unresolved placeholder: {ev}"
                    ));
                }
            }
        }
        self.check_comparison_conditions()
    }

    /// 比較条件の整合: B/C は同じ利用可能モデル集合・harness 集合・tier 対応表で方策だけを変える。
    /// A は B と同じ実行モデル/ハーネス構成の内側にいる（宣言があるとき）。
    fn check_comparison_conditions(&self) -> Result<(), String> {
        let routing = |id: ConfigId| {
            self.configs
                .iter()
                .find(|c| c.id == id)
                .and_then(|c| c.routing.as_ref())
        };
        let set = |v: &Option<Vec<String>>| {
            v.as_ref()
                .map(|v| v.iter().cloned().collect::<BTreeSet<String>>())
        };
        let (Some(b), Some(c)) = (routing(ConfigId::B), routing(ConfigId::C)) else {
            return Ok(());
        };
        if b.lower_policy != c.lower_policy {
            return Err("B and C must share the same lower_policy (tier mapping)".into());
        }
        if set(&b.models) != set(&c.models) {
            return Err("B and C must share the same models set".into());
        }
        if set(&b.run_harnesses) != set(&c.run_harnesses) {
            return Err("B and C must share the same run_harnesses set".into());
        }
        if let Some(a) = self.configs.iter().find(|c| c.id == ConfigId::A) {
            if let Some(ms) = set(&b.models)
                && !ms.contains(&a.model)
            {
                return Err(format!(
                    "A model {:?} is outside the B/C models set (A/B must share the execution model)",
                    a.model
                ));
            }
            if let Some(hs) = set(&b.run_harnesses)
                && !hs.contains(&a.harness)
            {
                return Err(format!(
                    "A harness {:?} is outside the B/C run_harnesses set",
                    a.harness
                ));
            }
        }
        Ok(())
    }

    /// B1/C1 の共通部分: arm 固有（id・policy_version・根拠表の扱い）を除いた設定。
    fn side_common(c: &Config) -> Value {
        let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
        if let Some(o) = v.as_object_mut() {
            o.remove("id");
            o.remove("policy_version");
            if let Some(r) = o.get_mut("routing").and_then(Value::as_object_mut) {
                r.remove("no_evidence_table");
                r.remove("evidence_table_id");
            }
        }
        v
    }

    /// B1/C1 variant の hash。共通条件（候補集合・階層・harness・model・予算・input・snapshot・problem）と
    /// 方策差（policy_version・根拠表の扱い）を固定する。主比較の manifest hash とは別（A を含めない）。
    pub fn side_variant_hash(&self) -> Result<String, String> {
        let arm = |id: ConfigId| {
            self.configs
                .iter()
                .find(|c| c.id == id)
                .ok_or_else(|| format!("config {id:?} is missing"))
        };
        let (b, c) = (arm(ConfigId::B)?, arm(ConfigId::C)?);
        let policy = |c: &Config| {
            let r = c.routing.as_ref();
            serde_json::json!({
                "id": c.id, "policy_version": c.policy_version,
                "no_evidence_table": r.and_then(|r| r.no_evidence_table),
                "evidence_table_id": r.and_then(|r| r.evidence_table_id.clone()),
            })
        };
        let n = self.normalized();
        let v = serde_json::json!({
            "common_b": Self::side_common(b), "common_c": Self::side_common(c),
            "policy_b": policy(b), "policy_c": policy(c),
            "budget": n.budget, "termination": n.termination, "input": n.input,
            "snapshot": n.snapshot, "problems": n.problems, "subset": n.secondary.subset,
        });
        let bytes = serde_json::to_vec(&canonical(&v)).map_err(|e| e.to_string())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    /// B1/C1 の条件検査: mode（policy）以外の差・根拠表・単一モデル pin を拒否する。
    /// 候補集合は Sonnet と Opus の両方を含む（宣言必須）。階層・harness・model・予算・input は B/C 同一。
    pub fn check_side_variant(&self) -> Result<(), String> {
        self.side_variant_hash()?;
        let find = |id: ConfigId| self.configs.iter().find(|c| c.id == id);
        let (Some(b), Some(c)) = (find(ConfigId::B), find(ConfigId::C)) else {
            return Err("side manifest needs B and C".into());
        };
        if Self::side_common(b) != Self::side_common(c) {
            return Err("B1 and C1 differ in more than the policy".into());
        }
        for x in [b, c] {
            let r = x
                .routing
                .as_ref()
                .ok_or_else(|| format!("config {:?}: routing must be pinned", x.id))?;
            if r.evidence_table_id.is_some() || r.effective_evidence_mode(x.id)? != "none" {
                return Err(format!(
                    "config {:?}: B1/C1 must not use an evidence table",
                    x.id
                ));
            }
        }
        let models = c
            .routing
            .as_ref()
            .and_then(|r| r.models.clone())
            .unwrap_or_default();
        if models.len() < 2
            || !models.iter().any(|m| m.contains("sonnet"))
            || !models.iter().any(|m| m.contains("opus"))
        {
            return Err(format!(
                "C1 is pinned to a single model/lane: candidate set must include both Sonnet and Opus, got {models:?}"
            ));
        }
        Ok(())
    }

    /// 入力 manifest との照合: hash 一致・全 problem が存在・split が campaign の split と一致。
    pub fn check_against_input(&self, input: &InputManifest) -> Result<(), String> {
        self.validate()?;
        let actual = manifest_hash(input)?;
        if actual != input.sha256 {
            return Err("input manifest content does not match its own sha256".into());
        }
        if input.sha256 != self.input.sha256 {
            return Err(format!(
                "input manifest hash mismatch: campaign pins {}, file is {}",
                self.input.sha256, input.sha256
            ));
        }
        let by_id: BTreeMap<&str, &str> = input
            .problems
            .iter()
            .map(|p| (p.id.as_str(), p.split.as_str()))
            .collect();
        for id in &self.problems {
            match by_id.get(id.as_str()) {
                None => return Err(format!("problem {id} not in input manifest")),
                Some(s) if *s != self.split.input_name() => {
                    return Err(format!(
                        "problem {id} has split {s}, campaign split is {:?} (dev and final must not be mixed)",
                        self.split
                    ));
                }
                _ => {}
            }
        }
        self.check_self_contained(input)
    }

    /// case 間依存の拒否（ADR 2026-10-16-mlsys-pilot-wiring-fixes D6）。試行の workspace は分離されているので、
    /// prompt が**他の** problem の `command` が作る `out/…` を読む case は満たせない。
    pub fn check_self_contained(&self, input: &InputManifest) -> Result<(), String> {
        for id in &self.problems {
            let Some(p) = input.problems.iter().find(|p| &p.id == id) else {
                continue;
            };
            for path in out_paths(&p.prompt) {
                if out_paths(&p.command).contains(&path) {
                    continue;
                }
                if let Some(q) = input
                    .problems
                    .iter()
                    .find(|q| q.id != p.id && out_paths(&q.command).contains(&path))
                {
                    return Err(format!(
                        "problem {id} reads {path}, which only problem {} produces; trial workspaces are isolated, \
                         so make the case self-contained (ship the input under materials/)",
                        q.id
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn load_input(
        &self,
        manifest_path: &Path,
        override_path: Option<&Path>,
    ) -> Result<InputManifest, String> {
        let p = match override_path {
            Some(p) => p.to_path_buf(),
            None => manifest_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&self.input.path),
        };
        let text = fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("input manifest: {e}"))
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub fn input(n: usize, split: &str) -> InputManifest {
        let problems: Vec<Value> = (0..n)
            .map(|i| serde_json::json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"x".repeat(400),"split":split,"stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
            .collect();
        let mut m: InputManifest = serde_json::from_value(serde_json::json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
        m.sha256 = manifest_hash(&m).expect("hash");
        m
    }

    pub fn campaign(input: &InputManifest, split: Split, n: usize) -> CampaignManifest {
        let cfg = |id, h: &str| Config {
            id,
            harness: h.into(),
            harness_version: "1".into(),
            model: "m".into(),
            model_version: "1".into(),
            policy_version: "p1".into(),
            routing: None,
        };
        CampaignManifest {
            schema: CAMPAIGN_SCHEMA.into(),
            campaign_id: "c1".into(),
            version: "v1".into(),
            split,
            input: InputRef {
                path: "input.json".into(),
                sha256: input.sha256.clone(),
            },
            problems: (0..n).map(|i| format!("p{i}")).collect(),
            configs: vec![
                cfg(ConfigId::C, "celeris"),
                cfg(ConfigId::A, "claude-code"),
                cfg(ConfigId::B, "celeris"),
            ],
            primary: (ConfigId::A, ConfigId::B),
            secondary: Secondary {
                pair: (ConfigId::B, ConfigId::C),
                subset: vec!["p1".into(), "p0".into()],
            },
            budget: Budget {
                token_basis: TokenBasis::InputOutput,
                max_tokens: 10_000_000,
                max_wall_secs: 86_400,
                max_disk_bytes: 1 << 40,
                max_parallel: 4,
                per_case: Cap {
                    max_tokens: 1_000_000,
                    max_wall_secs: 3_600,
                },
                per_arm: Cap {
                    max_tokens: 5_000_000,
                    max_wall_secs: 43_200,
                },
            },
            termination: Termination {
                trial_timeout_secs: 1800,
                max_attempts_per_trial: 1,
                stop_on_budget: true,
            },
            snapshot: SnapshotRef {
                repo_commit: "abc".into(),
                materials_sha256: "def".into(),
                kb_snapshot: "kb@1".into(),
            },
            order: Default::default(),
            seed: 0,
            repeats: 1,
            dependencies: BTreeMap::new(),
            sha256: String::new(),
        }
    }
}

/// 文中の `out/<path>` 参照（`out/` に続く path 文字の並び）。
fn out_paths(text: &str) -> BTreeSet<String> {
    let mut v = BTreeSet::new();
    let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/');
    let mut rest = text;
    while let Some(i) = rest.find("out/") {
        let before_ok = rest[..i].chars().next_back().is_none_or(|c| !ok(c));
        let tail = &rest[i + 4..];
        let end = tail.find(|c: char| !ok(c)).unwrap_or(tail.len());
        let path = tail[..end].trim_end_matches('.');
        if before_ok && !path.is_empty() {
            v.insert(format!("out/{path}"));
        }
        rest = &rest[i + 4..];
    }
    v
}

/// routing-eval manifest の content hash（sha256 欄を空にして正規化 JSON を hash。routing-eval と同じ方式）。
pub fn manifest_hash(m: &InputManifest) -> Result<String, String> {
    let mut v = serde_json::to_value(m).map_err(|e| e.to_string())?;
    v.as_object_mut()
        .ok_or("manifest serialization is not an object")?
        .insert("sha256".into(), serde_json::Value::String(String::new()));
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&v).map_err(|e| e.to_string())?)
    ))
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};

    #[test]
    fn hash_is_deterministic_and_order_insensitive() {
        let i = input(4, "dev");
        let a = campaign(&i, Split::Dev, 4);
        let mut b = a.clone();
        b.problems.reverse();
        b.configs.reverse();
        assert_eq!(a.compute_sha256().unwrap(), b.compute_sha256().unwrap());
        assert_eq!(a.compute_sha256().unwrap(), a.compute_sha256().unwrap());
        let mut c = a.clone();
        c.version = "v2".into();
        assert_ne!(a.compute_sha256().unwrap(), c.compute_sha256().unwrap());
    }

    #[test]
    fn sealed_hash_must_match_content() {
        let i = input(4, "dev");
        let mut m = campaign(&i, Split::Dev, 4);
        m.sha256 = m.compute_sha256().unwrap();
        assert!(m.validate().is_ok());
        m.budget.max_tokens += 1;
        assert!(m.validate().unwrap_err().contains("sha256"));
    }

    #[test]
    fn missing_budget_field_is_rejected_at_parse_and_zero_at_validate() {
        let i = input(4, "dev");
        let m = campaign(&i, Split::Dev, 4);
        let mut v = serde_json::to_value(&m).unwrap();
        v["budget"].as_object_mut().unwrap().remove("max_tokens");
        assert!(serde_json::from_value::<CampaignManifest>(v).is_err());
        // token_basis も必須（暗黙の既定なし）。
        let mut v = serde_json::to_value(&m).unwrap();
        v["budget"].as_object_mut().unwrap().remove("token_basis");
        let e = serde_json::from_value::<CampaignManifest>(v).unwrap_err();
        assert!(e.to_string().contains("token_basis"), "{e}");
        let mut z = m.clone();
        z.budget.max_wall_secs = 0;
        assert!(z.validate().unwrap_err().contains("budget"));
    }

    #[test]
    fn split_mixing_is_rejected() {
        let mut i = input(4, "dev");
        i.problems[2].split = "held-out".into();
        i.sha256 = manifest_hash(&i).unwrap();
        let m = campaign(&i, Split::Dev, 4);
        assert!(
            m.check_against_input(&i)
                .unwrap_err()
                .contains("must not be mixed")
        );
        let ok = input(4, "dev");
        assert!(
            campaign(&ok, Split::Dev, 4)
                .check_against_input(&ok)
                .is_ok()
        );
        // final campaign over dev problems is also a mix.
        assert!(
            campaign(&ok, Split::Final, 4)
                .check_against_input(&ok)
                .is_err()
        );
        let held = input(4, "held-out");
        let mut fin = campaign(&held, Split::Final, 4);
        for c in &mut fin.configs {
            if c.id != ConfigId::A {
                c.harness = "celeris".into();
                c.harness_version = "0123456789ab".into();
                c.routing = Some(ArmRouting {
                    lower_policy: Some("p".into()),
                    evidence_table_id: Some("e".into()),
                    online_update: Some(false),
                    ..Default::default()
                });
            }
        }
        assert!(fin.check_against_input(&held).is_ok());
    }

    #[test]
    fn input_hash_mismatch_is_rejected() {
        let i = input(4, "dev");
        let mut m = campaign(&i, Split::Dev, 4);
        m.input.sha256 = "0".repeat(64);
        assert!(
            m.check_against_input(&i)
                .unwrap_err()
                .contains("hash mismatch")
        );
        let mut tampered = i.clone();
        tampered.problems[0].prompt = "changed".into();
        assert!(
            campaign(&i, Split::Dev, 4)
                .check_against_input(&tampered)
                .is_err()
        );
    }

    #[test]
    fn case_reading_another_cases_output_is_rejected() {
        // pilot 2026-10-16: review case が含まれない case の出力 out/report.md を読んでいた。
        let mut i = input(4, "dev");
        i.problems[0].command = "test -s out/report.md".into();
        i.problems[1].prompt =
            "out/report.md と materials/raw.json を突き合わせ out/review.md に書け".into();
        i.problems[1].command = "test -s out/review.md".into();
        i.sha256 = manifest_hash(&i).unwrap();
        let m = campaign(&i, Split::Dev, 4);
        let e = m.check_against_input(&i).unwrap_err();
        assert!(
            e.contains("p1 reads out/report.md") && e.contains("p0"),
            "{e}"
        );
        // 自分の出力だけを読む／書く case は通る。
        i.problems[1].prompt = "materials/report-draft.md を読み out/review.md に書け".into();
        i.sha256 = manifest_hash(&i).unwrap();
        assert!(campaign(&i, Split::Dev, 4).check_against_input(&i).is_ok());
        assert_eq!(
            out_paths("see out/a.md, and out/b/c.csv。 not_out/x"),
            ["out/a.md", "out/b/c.csv"].map(String::from).into()
        );
    }

    #[test]
    fn subset_must_be_declared_and_contained() {
        let i = input(4, "dev");
        let mut m = campaign(&i, Split::Dev, 4);
        m.secondary.subset.clear();
        assert!(m.validate().is_err());
        m.secondary.subset = vec!["nope".into()];
        assert!(m.validate().is_err());
    }
}
