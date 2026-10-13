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
        source_id: "claude-oauth",
        account: "claude",
        harness_id: "claude-code",
        model_id: "claude-opus-5-5",
        problem_count: 30,
        k: 2,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "claude-oauth",
        account: "claude",
        harness_id: "claude-code",
        model_id: "claude-sonnet-5-5",
        problem_count: 30,
        k: 2,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "claude-oauth",
        account: "claude",
        harness_id: "claude-code",
        model_id: "claude-haiku-5-5",
        problem_count: 30,
        k: 2,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "claude-oauth",
        account: "claude",
        harness_id: "claude-code",
        model_id: "claude-fable-5-1",
        problem_count: 12,
        k: 1,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "codex-oauth",
        account: "codex",
        harness_id: "codex",
        model_id: "gpt-6.1-sol",
        problem_count: 30,
        k: 2,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "codex-oauth",
        account: "codex",
        harness_id: "codex",
        model_id: "gpt-6-luna",
        problem_count: 30,
        k: 2,
        quota_budget_percent: 15,
    },
    Candidate {
        source_id: "codex-oauth",
        account: "codex",
        harness_id: "codex",
        model_id: "gpt-6-astra",
        problem_count: 12,
        k: 1,
        quota_budget_percent: 15,
    },
];
