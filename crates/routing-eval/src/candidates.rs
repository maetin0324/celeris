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
        // 例の config から `[llm_sources.<id>]`・`[harnesses.<id>]` の節ごとに `kind` と `source_kinds` の行を拾う。
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let config_dir = format!("{manifest_dir}/../../config");
        let mut paths: Vec<_> = std::fs::read_dir(&config_dir)
            .expect("config dir")
            .map(|entry| entry.expect("config entry").path())
            .filter(|path| path.to_string_lossy().ends_with(".example.toml"))
            .collect();
        paths.sort();
        let mut declared: std::collections::BTreeMap<String, Vec<(String, String)>> =
            std::collections::BTreeMap::new();
        for path in paths {
            let text = std::fs::read_to_string(&path).expect("example config");
            let mut section = String::new();
            for line in text.lines().map(str::trim) {
                if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                    section = header.to_string();
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    declared
                        .entry(section.clone())
                        .or_default()
                        .push((key.trim().to_string(), value.trim().to_string()));
                }
            }
        }
        let field = |section: &str, key: &str| -> Option<String> {
            declared
                .get(section)?
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };
        for c in CANDIDATES {
            let source_section = format!("llm_sources.{}", c.source_id);
            assert_eq!(
                field(&source_section, "kind"),
                Some(format!("\"{}\"", c.source_id)),
                "source {} is not declared with kind {}",
                c.source_id,
                c.source_id
            );
            let parsed: task_core::LlmSourceRef =
                serde_json::from_value(serde_json::json!(c.source_id)).expect("source kind");
            assert_eq!(parsed.as_str(), c.source_id);
            let harness_section = format!("harnesses.{}", c.harness_id);
            assert_eq!(
                field(&harness_section, "kind"),
                Some(format!("\"{}\"", c.harness_id)),
                "harness {} is not declared with kind {}",
                c.harness_id,
                c.harness_id
            );
            let source_kinds = field(&harness_section, "source_kinds").expect("source_kinds");
            assert!(
                source_kinds.contains(&format!("\"{}\"", c.source_id)),
                "harness {} cannot use source {}",
                c.harness_id,
                c.source_id
            );
        }
    }
}
