//! ADR 2026-11-08-health-enforcement-loop D4.3: 同じ health の指摘の往復（`Event::HealthFindingsRepeated`）の
//! payload の型と、fingerprint の集合の鍵。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Event, TaskId};

/// 往復を見つけた位置（源の task の中のどこの評価か）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HealthRepeatSite {
    /// WorkUnit の check（`key` は WU の key）。
    WorkUnit { key: String },
    /// 段の統合（`key` は段の key）。
    Phase { key: String },
    /// task の review。
    Review,
}

impl HealthRepeatSite {
    /// 人が読む 1 語（受信箱の本文）。
    pub fn label(&self) -> String {
        match self {
            HealthRepeatSite::WorkUnit { key } => format!("WU {key} の check"),
            HealthRepeatSite::Phase { key } => format!("段 {key} の統合"),
            HealthRepeatSite::Review => "review".to_string(),
        }
    }
}

/// fingerprint の集合を正規化する（重複を除き辞書順）。
pub fn normalize_fingerprints(fingerprints: &[String]) -> Vec<String> {
    let mut set: Vec<String> = fingerprints.to_vec();
    set.sort();
    set.dedup();
    set
}

/// fingerprint の集合の sha256（hex）。受信箱の項目の鍵 `(root, set_sha256)` の片方。並びと重複に依らない。
pub fn fingerprint_set_sha256(fingerprints: &[String]) -> String {
    let mut hasher = Sha256::new();
    for fp in normalize_fingerprints(fingerprints) {
        hasher.update(fp.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

impl Event {
    /// `Event::HealthFindingsRepeated` を作る（fingerprint を正規化し、`set_sha256` をここで求める）。
    pub fn health_findings_repeated(
        source_task: TaskId,
        site: HealthRepeatSite,
        fingerprints: &[String],
    ) -> Self {
        Event::HealthFindingsRepeated {
            source_task,
            site,
            fingerprints: normalize_fingerprints(fingerprints),
            set_sha256: fingerprint_set_sha256(fingerprints),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 追記専用の event は古い形も新しい形も読めなければならない。既存の serde の試験は新しい variant の
    /// 形（tag `health_findings_repeated` と `site.kind`）を知らないので、往復をここで固定する。
    #[test]
    fn health_limits_inbox_event_serde_roundtrip() {
        let source = TaskId::new();
        for site in [
            HealthRepeatSite::WorkUnit { key: "wu-a".into() },
            HealthRepeatSite::Phase { key: "core".into() },
            HealthRepeatSite::Review,
        ] {
            let event = Event::health_findings_repeated(
                source,
                site.clone(),
                &["fp-b".to_string(), "fp-a".to_string(), "fp-b".to_string()],
            );
            let json = serde_json::to_value(&event).expect("serialize");
            assert_eq!(json["type"], "health_findings_repeated");
            assert_eq!(json["fingerprints"], serde_json::json!(["fp-a", "fp-b"]));
            let back: Event = serde_json::from_value(json).expect("deserialize");
            assert_eq!(back, event);
        }
        let review = serde_json::to_value(HealthRepeatSite::Review).expect("site");
        assert_eq!(review, serde_json::json!({"kind": "review"}));
        // 集合の鍵は並びと重複に依らない。
        assert_eq!(
            fingerprint_set_sha256(&["b".into(), "a".into()]),
            fingerprint_set_sha256(&["a".into(), "b".into(), "a".into()])
        );
        assert_ne!(
            fingerprint_set_sha256(&["a".into()]),
            fingerprint_set_sha256(&["a".into(), "b".into()])
        );
    }
}
