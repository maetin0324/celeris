//! 実行器 trait。runner は通常の Celeris 実行経路をこの trait の背後に置く薄い層で、
//! この crate には実 LLM・network を呼ぶ実装を置かない（mock のみ）。
use std::{collections::BTreeMap, fs, path::Path};

use sha2::{Digest, Sha256};

pub use crate::ledger::TaskRef;
use crate::{
    aggregate::Outcome,
    manifest::{Cap, Config, ConfigId},
};
use serde_json::Value;
use std::sync::Mutex;

pub struct TrialRequest<'a> {
    pub config: &'a Config,
    pub problem_id: &'a str,
    pub prompt: &'a str,
    pub trial_no: u32,
    pub rep: u32,
    /// この試行専用の作業ツリー（初期 repo の複製）。
    pub workspace: &'a Path,
    /// この試行専用の KB（初期 KB snapshot の複製）。
    pub kb: &'a Path,
    pub timeout_secs: u64,
    /// この試行の実効上限（`min(per_case, per_arm の残り)`）。同期 executor は自分で止めてよい。
    /// 止めずに超えた分は runner が結果の受け取り時に case_budget として cancelled に直す。
    pub budget: Cap,
}

/// 実行中の試行（子 task・job を含む）の累積使用量。tokens が None なら欠測（0 ではない）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub tokens: Option<u64>,
    pub wall_secs: u64,
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
    /// 同期実行（mock など）。台帳を使わない実行器はこれだけを実装する。
    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String>;

    /// true なら runner は `execute` でなく台帳を引いて `submit`/`poll` で追跡する。
    fn is_async(&self) -> bool {
        false
    }

    /// 試行ごとに task 単位で固定する routing 設定（台帳に投入前に記録する）。
    fn routing(&self, _req: &TrialRequest) -> Value {
        Value::Null
    }

    /// 冪等キーで既に投入済みの task を照会する。応答喪失・再起動後の再開手順の第 1 段。
    fn lookup(&self, _key: &str) -> Result<Option<TaskRef>, String> {
        Ok(None)
    }

    /// 冪等キー付きで task を投入する。同じ key の再投入は新しい task を作ってはならない。
    fn submit(&self, _req: &TrialRequest, _key: &str) -> Result<TaskRef, String> {
        Err(format!("executor {} does not support submit", self.name()))
    }

    /// 再開手順: 同一冪等キーで照会し、無ければ投入する（planned のまま task_id 無しのとき）。
    fn resolve(&self, req: &TrialRequest, key: &str) -> Result<TaskRef, String> {
        match self.lookup(key)? {
            Some(t) => Ok(t),
            None => self.submit(req, key),
        }
    }

    /// 子 task・クラスタ job を含む最新の参照を返す（台帳へ追記される）。
    fn track(&self, t: &TaskRef) -> Result<TaskRef, String> {
        Ok(t.clone())
    }

    /// 実行中の累積使用量。観測できない executor は None（runner は上限の実行中判定をせず、完了後に判定する）。
    fn usage(&self, _t: &TaskRef) -> Result<Option<Usage>, String> {
        Ok(None)
    }

    fn poll(&self, _t: &TaskRef) -> Result<ExecStatus, String> {
        Err(format!("executor {} does not support poll", self.name()))
    }

    /// 打ち切り時のキャンセル。task 本体・子 task・クラスタ job のすべてを対象にする。
    fn cancel(&self, _t: &TaskRef, _reason: &str) -> Result<(), String> {
        Ok(())
    }
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
    /// executor 自体が Err を返す（接続失敗など）。その試行だけ error になり run は続く。
    Err,
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
            format!(
                "{:?}/{}/rep{}/{}",
                req.config.id, req.problem_id, req.rep, req.trial_no
            ),
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
            MockStep::Err => return Err("mock executor failure".into()),
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

/// 非同期実行の mock。「外部サービス」の状態（投入済み task・cancel 履歴）をこの object が持つので、
/// 同じ object を複数回の `run` に渡すと「アダプタ再起動後も外部は残る」を再現できる。
#[derive(Default)]
pub struct MockAsyncExecutor {
    remote: Mutex<MockRemote>,
    /// task を Done にするまでに Wait を返す poll 回数。
    pub wait_polls: u32,
}

#[derive(Default)]
struct MockRemote {
    tasks: BTreeMap<String, (TaskRef, u32)>,
    submit_calls: u32,
    /// 投入は成功させるが応答だけ失う回数（残り）。
    lose_responses: u32,
    cancelled: Vec<(TaskRef, String)>,
}

impl MockAsyncExecutor {
    pub fn new(wait_polls: u32) -> Self {
        Self {
            remote: Mutex::default(),
            wait_polls,
        }
    }

    fn remote(&self) -> std::sync::MutexGuard<'_, MockRemote> {
        self.remote.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 次の `lose` 回の submit は task を作るが応答を返さない。
    pub fn lose_next_submit_responses(&self, lose: u32) {
        self.remote().lose_responses = lose;
    }

    /// 外部に作られた task 数（重複投入の検査用）。
    pub fn remote_task_count(&self) -> usize {
        self.remote().tasks.len()
    }

    pub fn submit_calls(&self) -> u32 {
        self.remote().submit_calls
    }

    pub fn cancelled(&self) -> Vec<(TaskRef, String)> {
        self.remote().cancelled.clone()
    }
}

impl Executor for MockAsyncExecutor {
    fn name(&self) -> &'static str {
        "mock-async"
    }

    fn execute(&self, _req: &TrialRequest) -> Result<ExecStatus, String> {
        Err("mock-async is driven via submit/poll".into())
    }

    fn is_async(&self) -> bool {
        true
    }

    fn routing(&self, req: &TrialRequest) -> Value {
        serde_json::json!({"mode": "mock", "arm": format!("{:?}", req.config.id)})
    }

    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, String> {
        Ok(self.remote().tasks.get(key).map(|(t, _)| t.clone()))
    }

    fn submit(&self, _req: &TrialRequest, key: &str) -> Result<TaskRef, String> {
        let mut r = self.remote();
        r.submit_calls += 1;
        // 外部側の冪等性: 同じ key は新しい task を作らない。
        let n = r.tasks.len() + 1;
        let t = r
            .tasks
            .entry(key.to_string())
            .or_insert_with(|| {
                (
                    TaskRef {
                        task_id: format!("task-{n}"),
                        run_id: Some(format!("run-{n}")),
                        child_task_ids: vec![format!("task-{n}-c1")],
                        cluster_job_ids: vec![format!("job-{n}")],
                    },
                    0,
                )
            })
            .0
            .clone();
        if r.lose_responses > 0 {
            r.lose_responses -= 1;
            return Err("response lost".into());
        }
        Ok(t)
    }

    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, String> {
        let mut r = self.remote();
        let polls = r
            .tasks
            .values_mut()
            .find(|(x, _)| x.task_id == t.task_id)
            .map(|(_, n)| {
                *n += 1;
                *n
            })
            .ok_or_else(|| format!("unknown task {}", t.task_id))?;
        if polls <= self.wait_polls {
            return Ok(ExecStatus::Wait);
        }
        Ok(ExecStatus::Done(ExecResult {
            outcome: Outcome::Passed,
            tokens: Some(10_000),
            cost_micro_usd: Some(100_000),
            wall_secs: 60,
        }))
    }

    fn cancel(&self, t: &TaskRef, reason: &str) -> Result<(), String> {
        self.remote().cancelled.push((t.clone(), reason.into()));
        Ok(())
    }
}
