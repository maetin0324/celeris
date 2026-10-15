//! 実行器 trait。runner は通常の Celeris 実行経路をこの trait の背後に置く薄い層で、
//! この crate には実 LLM・network を呼ぶ実装を置かない（mock のみ）。
use std::{collections::BTreeMap, fs, path::Path};

use sha2::{Digest, Sha256};

use crate::{
    aggregate::Outcome,
    manifest::{Config, ConfigId},
};

pub struct TrialRequest<'a> {
    pub config: &'a Config,
    pub problem_id: &'a str,
    pub prompt: &'a str,
    pub trial_no: u32,
    /// この試行専用の作業ツリー（初期 repo の複製）。
    pub workspace: &'a Path,
    /// この試行専用の KB（初期 KB snapshot の複製）。
    pub kb: &'a Path,
    pub timeout_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecResult {
    pub outcome: Outcome,
    /// 欠測は None。
    pub tokens: Option<u64>,
    pub cost_micro_usd: Option<u64>,
    pub wall_secs: u64,
}

/// 実行器の返事。`Wait` は外部待ち（まだ結果が出ない）で、記録は残さず後の再実行（resume）に回る。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecStatus {
    Done(ExecResult),
    Wait,
}

pub trait Executor {
    fn name(&self) -> &'static str;
    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String>;
}

/// mock の 1 試行の振る舞い。
#[derive(Clone, Debug)]
pub enum MockStep {
    Pass,
    Fail,
    Error,
    Timeout,
    /// 成功するが指定 token を使う（予算超過の再現用）。
    Spend(u64),
    /// token/cost を報告しない（欠測）。
    PassUnmetered,
    /// 外部待ち（結果がまだ出ない）。
    Wait,
}

#[derive(Clone, Debug, Default)]
pub struct MockExecutor {
    script: BTreeMap<(ConfigId, String), MockStep>,
    /// script に無い試行は (config, problem) の hash から決める。false なら常に Pass。
    seeded: bool,
}

impl MockExecutor {
    pub fn scripted(script: BTreeMap<(ConfigId, String), MockStep>) -> Self {
        Self {
            script,
            seeded: false,
        }
    }

    /// CLI 用。結果は (config, problem) だけで決まる決定的な混合（pass/fail/timeout/error）。
    pub fn seeded() -> Self {
        Self {
            script: BTreeMap::new(),
            seeded: true,
        }
    }

    fn step(&self, req: &TrialRequest) -> MockStep {
        if let Some(s) = self
            .script
            .get(&(req.config.id, req.problem_id.to_string()))
        {
            return s.clone();
        }
        if !self.seeded {
            return MockStep::Pass;
        }
        let d = Sha256::digest(format!("{:?}/{}", req.config.id, req.problem_id));
        match d[0] % 10 {
            0..=5 => MockStep::Pass,
            6 | 7 => MockStep::Fail,
            8 => MockStep::Timeout,
            _ if req.config.id == ConfigId::A => MockStep::PassUnmetered,
            _ => MockStep::Error,
        }
    }
}

impl Executor for MockExecutor {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String> {
        // 試行専用の作業ツリーと KB を実際に書き換える（隔離の検証用）。
        fs::write(
            req.workspace.join("mock-result.txt"),
            format!("{:?}/{}/{}", req.config.id, req.problem_id, req.trial_no),
        )
        .map_err(|e| e.to_string())?;
        fs::write(req.kb.join("mock-note.md"), req.problem_id).map_err(|e| e.to_string())?;
        let metered = |outcome, tokens: u64, wall| ExecResult {
            outcome,
            tokens: Some(tokens),
            cost_micro_usd: Some(tokens * 10),
            wall_secs: wall,
        };
        let done = match self.step(req) {
            MockStep::Wait => return Ok(ExecStatus::Wait),
            MockStep::Pass => metered(Outcome::Passed, 10_000, 60),
            MockStep::Fail => metered(Outcome::Failed, 12_000, 90),
            MockStep::Error => metered(Outcome::Error, 1_000, 5),
            MockStep::Timeout => metered(Outcome::Timeout, 15_000, req.timeout_secs),
            MockStep::Spend(t) => metered(Outcome::Passed, t, 60),
            MockStep::PassUnmetered => ExecResult {
                outcome: Outcome::Passed,
                tokens: None,
                cost_micro_usd: None,
                wall_secs: 60,
            },
        };
        Ok(ExecStatus::Done(done))
    }
}
