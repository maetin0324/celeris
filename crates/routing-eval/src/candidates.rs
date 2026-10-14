use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidate {
    pub source_id: &'static str,
    pub account: &'static str,
    pub harness_id: &'static str,
    pub model_id: &'static str,
    pub problem_count: usize,
    pub k: usize,
    pub quota_budget_percent: u8,
}

pub const CANDIDATES: [Candidate; 7] = [
    Candidate {
        source_id: "claude_oauth",
        account: "claude",
        harness_id: "claude_code",
        model_id: "claude-opus-5-5",
        problem_count: 30,
        k: 2,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "claude_oauth",
        account: "claude",
        harness_id: "claude_code",
        model_id: "claude-sonnet-5-5",
        problem_count: 30,
        k: 2,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "claude_oauth",
        account: "claude",
        harness_id: "claude_code",
        model_id: "claude-haiku-5-5",
        problem_count: 30,
        k: 2,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "claude_oauth",
        account: "claude",
        harness_id: "claude_code",
        model_id: "claude-fable-5-1",
        problem_count: 12,
        k: 1,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "codex_oauth",
        account: "codex",
        harness_id: "codex",
        model_id: "gpt-6.1-sol",
        problem_count: 30,
        k: 2,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "codex_oauth",
        account: "codex",
        harness_id: "codex",
        model_id: "gpt-6-luna",
        problem_count: 30,
        k: 2,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "codex_oauth",
        account: "codex",
        harness_id: "codex",
        model_id: "gpt-6-astra",
        problem_count: 12,
        k: 1,
        quota_budget_percent: 15,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// 候補表の source・harness の id は config の例（`config/*.example.toml`）の宣言（`[llm_sources.<id>]`・`[harnesses.<id>]`）の
    /// 綴りそのもの。選択器は宣言の綴りで根拠表を引くので、綴りが違うと評価結果が選択に効かない。
    #[test]
    fn candidate_ids_match_declared_spelling() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mut sources = toml::Table::new();
        let mut harnesses = toml::Table::new();
        let config_dir = format!("{manifest_dir}/../../config");
        let mut paths: Vec<_> = std::fs::read_dir(&config_dir)
            .expect("config dir")
            .map(|entry| entry.expect("config entry").path())
            .filter(|path| path.to_string_lossy().ends_with(".example.toml"))
            .collect();
        paths.sort();
        for path in paths {
            let text = std::fs::read_to_string(&path).expect("example config");
            let config: toml::Table = text.parse().expect("example config parses");
            for (table, into) in [("llm_sources", &mut sources), ("harnesses", &mut harnesses)] {
                if let Some(toml::Value::Table(declared)) = config.get(table) {
                    into.extend(declared.clone());
                }
            }
        }
        for c in CANDIDATES {
            let source = sources
                .get(c.source_id)
                .unwrap_or_else(|| panic!("source {} is not declared", c.source_id));
            assert_eq!(
                source.get("kind").and_then(|k| k.as_str()),
                Some(c.source_id)
            );
            let parsed: task_core::LlmSourceRef =
                serde_json::from_value(serde_json::json!(c.source_id)).expect("source kind");
            assert_eq!(parsed.as_str(), c.source_id);
            let harness = harnesses
                .get(c.harness_id)
                .unwrap_or_else(|| panic!("harness {} is not declared", c.harness_id));
            assert_eq!(
                harness.get("kind").and_then(|k| k.as_str()),
                Some(c.harness_id)
            );
            let source_kinds = harness
                .get("source_kinds")
                .and_then(|k| k.as_array())
                .expect("source_kinds");
            assert!(
                source_kinds.iter().any(|k| k.as_str() == Some(c.source_id)),
                "harness {} cannot use source {}",
                c.harness_id,
                c.source_id
            );
        }
    }
}
