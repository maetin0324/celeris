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

use crate::CAMPAIGN_SCHEMA;

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Budget {
    pub max_tokens: u64,
    pub max_wall_secs: u64,
    pub max_disk_bytes: u64,
    pub max_parallel: u32,
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
    pub snapshot: SnapshotRef,
    /// problem id -> 前提 problem id の配列。前提の全 config の trial が passed になるまで依存先は実行しない。
    /// 空なら直列化しない（依存の無い manifest の hash を変えない）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub sha256: String,
}

fn canonical(v: &Value) -> Value {
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
        let mut v = vec![ConfigId::A, ConfigId::B];
        if self.secondary.subset.iter().any(|s| s == problem) {
            v.push(ConfigId::C);
        }
        v
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
        if self.schema != CAMPAIGN_SCHEMA {
            return Err(format!("schema must be {CAMPAIGN_SCHEMA}"));
        }
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
        if ids != [ConfigId::A, ConfigId::B, ConfigId::C] {
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
        }
        if self.primary != (ConfigId::A, ConfigId::B) {
            return Err("primary comparison must be (A, B)".into());
        }
        if self.secondary.pair != (ConfigId::B, ConfigId::C) {
            return Err("secondary comparison must be (B, C)".into());
        }
        let b = &self.budget;
        if b.max_tokens == 0 || b.max_wall_secs == 0 || b.max_disk_bytes == 0 || b.max_parallel == 0
        {
            return Err("budget fields must all be > 0".into());
        }
        let t = &self.termination;
        if t.trial_timeout_secs == 0 || t.max_attempts_per_trial == 0 {
            return Err("termination fields must be > 0".into());
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
        if !self.sha256.is_empty() && self.sha256 != self.compute_sha256()? {
            return Err("sha256 does not match manifest content".into());
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
                max_tokens: 10_000_000,
                max_wall_secs: 86_400,
                max_disk_bytes: 1 << 40,
                max_parallel: 4,
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
            dependencies: BTreeMap::new(),
            sha256: String::new(),
        }
    }
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
        assert!(
            campaign(&held, Split::Final, 4)
                .check_against_input(&held)
                .is_ok()
        );
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
    fn subset_must_be_declared_and_contained() {
        let i = input(4, "dev");
        let mut m = campaign(&i, Split::Dev, 4);
        m.secondary.subset.clear();
        assert!(m.validate().is_err());
        m.secondary.subset = vec!["nope".into()];
        assert!(m.validate().is_err());
    }
}
