//! 実行器 trait。runner は通常の Celeris 実行経路をこの trait の背後に置く薄い層で、
//! この crate には実 LLM・network を呼ぶ実装を置かない（mock のみ）。
use std::{collections::BTreeMap, fs, path::Path};

use sha2::{Digest, Sha256};

pub use crate::ledger::TaskRef;
pub use crate::record::ObservedRun;
pub use crate::versions::ObservedVersions;
use crate::{
    aggregate::Outcome,
    manifest::{Cap, Config, ConfigId},
};
use serde_json::Value;
use std::sync::Mutex;

pub struct TrialRequest<'a> {
    /// manifest の campaign_id と hash。task 作成 API の識別（campaign / manifest_hash）に載せる。
    pub campaign_id: &'a str,
    pub manifest_sha256: &'a str,
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
    /// manifest の専門分担 variant 参照（省略なら従来どおり）。POST・GET 照合が同じ値を使う。
    pub variant: Option<&'a crate::variant::VariantRef>,
}

pub use crate::manifest::TokenUsage;

/// 実行中の試行（子 task・job を含む）の累積使用量。tokens が None なら欠測（0 ではない）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub tokens: Option<TokenUsage>,
    pub wall_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecResult {
    pub outcome: Outcome,
    /// 内訳（input/output/cache）。欠測は None。予算に数える量は runner が manifest の token_basis で決める。
    pub tokens: Option<TokenUsage>,
    pub cost_micro_usd: Option<u64>,
    pub wall_secs: u64,
}

/// 実行器の返事。`Wait` は外部待ち（まだ結果が出ない）で、記録は残さず後の再実行（resume）に回る。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecStatus {
    Done(ExecResult),
    Wait,
}

/// 非同期 executor の失敗の分類。runner はこの分類で台帳の扱いを変える（一律 error にしない）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrKind {
    /// 確実に未投入で、再試行しても害が無い（接続前の失敗など）。行は planned のまま次回また試す。
    NotSent,
    /// 確実に未投入で、再試行しても通らない（400 の拒否・設定不備）。その試行だけ error にする。
    Refused,
    /// 投入の結果が不明（POST の応答喪失など）。再 submit せず、冪等キーの照会で追跡を続ける。
    Unknown,
    /// 投入済みの試行との通信が切れた（track/poll/usage/cancel）。台帳の task を保ち、次回また追う。
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecError {
    pub kind: ErrKind,
    pub msg: String,
}

impl ExecError {
    pub fn new(kind: ErrKind, msg: impl Into<String>) -> Self {
        Self {
            kind,
            msg: msg.into(),
        }
    }
    pub fn not_sent(msg: impl Into<String>) -> Self {
        Self::new(ErrKind::NotSent, msg)
    }
    pub fn refused(msg: impl Into<String>) -> Self {
        Self::new(ErrKind::Refused, msg)
    }
    pub fn unknown(msg: impl Into<String>) -> Self {
        Self::new(ErrKind::Unknown, msg)
    }
    pub fn interrupted(msg: impl Into<String>) -> Self {
        Self::new(ErrKind::Interrupted, msg)
    }
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.msg)
    }
}

/// 分類が付いていない失敗は保守的に `Unknown`（結果不明）として扱う。
impl From<String> for ExecError {
    fn from(msg: String) -> Self {
        Self::unknown(msg)
    }
}

impl From<&str> for ExecError {
    fn from(msg: &str) -> Self {
        Self::unknown(msg)
    }
}

pub trait Executor {
    fn name(&self) -> &'static str;
    /// true なら結果は模擬（mock）で、実性能・実費用として扱えない。出力（run-meta・summary・export）に印として残る。
    fn simulated(&self) -> bool {
        false
    }
    /// 同期実行（mock など）。台帳を使わない実行器はこれだけを実装する。
    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String>;

    /// true なら runner は `execute` でなく台帳を引いて `submit`/`poll` で追跡する。
    fn is_async(&self) -> bool {
        false
    }

    /// 試行ごとに task 単位で固定する routing 設定（台帳に投入前に記録する）。
    /// Err は投入前の拒否（B/C の routing 欠落・C の根拠表 id 無し）で、その試行は executor_error になる。
    fn routing(&self, _req: &TrialRequest) -> Result<Value, String> {
        Ok(Value::Null)
    }

    /// 試行の開始時（投入前）に、実際に動く版を観測する（ADR 2026-10-27-trial-observed-versions）。
    /// manifest が契約 revision を宣言したときだけ呼ばれる。観測できない欄は `None` にし、宣言の値を返してはならない。
    fn observe_versions(&self, _req: &TrialRequest) -> Result<ObservedVersions, String> {
        Ok(ObservedVersions::default())
    }

    /// 冪等キーで既に投入済みの task を照会する。応答喪失・再起動後の再開手順の第 1 段。
    fn lookup(&self, _key: &str) -> Result<Option<TaskRef>, ExecError> {
        Ok(None)
    }

    /// 冪等キー付きで task を投入する。同じ key の再投入は新しい task を作ってはならない。
    fn submit(&self, _req: &TrialRequest, _key: &str) -> Result<TaskRef, ExecError> {
        Err(ExecError::refused(format!(
            "executor {} does not support submit",
            self.name()
        )))
    }

    /// 再開手順: 同一冪等キーで照会し、無ければ投入する（planned のまま task_id 無しのとき）。
    fn resolve(&self, req: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
        match self.lookup(key)? {
            Some(t) => Ok(t),
            None => self.submit(req, key),
        }
    }

    /// 子 task・クラスタ job を含む最新の参照を返す（台帳へ追記される）。
    fn track(&self, t: &TaskRef) -> Result<TaskRef, ExecError> {
        Ok(t.clone())
    }

    /// 実行中の累積使用量。観測できない executor は None（runner は上限の実行中判定をせず、完了後に判定する）。
    fn usage(&self, _t: &TaskRef) -> Result<Option<Usage>, ExecError> {
        Ok(None)
    }

    fn poll(&self, _t: &TaskRef) -> Result<ExecStatus, ExecError> {
        Err(ExecError::interrupted(format!(
            "executor {} does not support poll",
            self.name()
        )))
    }

    /// 試行で実際に走った全 run（子・孫・再試行を含む）の実測。観測できない executor は空。
    /// 宣言（manifest）の名前を返してはならない。
    fn observed(&self, _t: &TaskRef) -> Result<Vec<ObservedRun>, String> {
        Ok(Vec::new())
    }

    /// 打ち切り時のキャンセル。task 本体・子 task・クラスタ job のすべてを対象にする。
    fn cancel(&self, _t: &TaskRef, _reason: &str) -> Result<(), ExecError> {
        Ok(())
    }

    /// 外部の task（子 task・クラスタ job を含む）がすべて終端か。cancel 要求が通っただけでは停止とみなさない。
    /// false なら台帳は cancel_pending のまま残り、予約も解除されない。
    fn stopped(&self, _t: &TaskRef) -> Result<bool, ExecError> {
        Ok(true)
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
    /// `observe_versions` が返す観測（試験用。無ければ何も観測できない）。
    versions: Option<ObservedVersions>,
}

impl MockExecutor {
    pub fn scripted(script: BTreeMap<(ConfigId, String), MockStep>) -> Self {
        Self {
            script,
            seeded: false,
            versions: None,
        }
    }

    /// CLI 用。結果は (config, problem) だけで決まる決定的な混合（pass/fail/timeout/error）。
    pub fn seeded() -> Self {
        Self {
            script: BTreeMap::new(),
            seeded: true,
            versions: None,
        }
    }

    /// 試行の開始時の観測を固定する（契約 revision の照合の試験用）。
    pub fn with_versions(mut self, v: ObservedVersions) -> Self {
        self.versions = Some(v);
        self
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

    fn simulated(&self) -> bool {
        true
    }

    fn observe_versions(&self, _req: &TrialRequest) -> Result<ObservedVersions, String> {
        Ok(self.versions.clone().unwrap_or_default())
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
            tokens: Some(TokenUsage::full(tokens, 0, 0, 0)),
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
    /// `observe_versions` が返す観測（試験用。無ければ何も観測できない）。
    versions: Option<ObservedVersions>,
}

#[derive(Default)]
struct MockRemote {
    tasks: BTreeMap<String, (TaskRef, u32)>,
    submit_calls: u32,
    /// 投入は成功させるが応答だけ失う回数（残り）。
    lose_responses: u32,
    cancelled: Vec<(TaskRef, String)>,
    /// 次の poll / usage / cancel を通信断（Interrupted）にする回数（残り）。
    fail_polls: u32,
    fail_usages: u32,
    fail_cancels: u32,
    /// cancel 要求は通るが、外部がまだ終端でない（stopped が false）回数（残り）。
    unconfirmed_stops: u32,
    /// 実行中に報告する累積使用量（None なら観測不能）。
    usage: Option<Usage>,
    poll_calls: u32,
}

impl MockAsyncExecutor {
    pub fn new(wait_polls: u32) -> Self {
        Self {
            remote: Mutex::default(),
            wait_polls,
            versions: None,
        }
    }

    /// 試行の開始時の観測を固定する（契約 revision の照合の試験用）。
    pub fn with_versions(mut self, v: ObservedVersions) -> Self {
        self.versions = Some(v);
        self
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

    pub fn poll_calls(&self) -> u32 {
        self.remote().poll_calls
    }

    /// 次の `n` 回の poll を通信断にする（投入済みの試行との接続断）。
    pub fn fail_next_polls(&self, n: u32) {
        self.remote().fail_polls = n;
    }

    pub fn fail_next_usages(&self, n: u32) {
        self.remote().fail_usages = n;
    }

    /// 次の `n` 回の cancel を通信断にする（外部は止まらない）。
    pub fn fail_next_cancels(&self, n: u32) {
        self.remote().fail_cancels = n;
    }

    /// 次の `n` 回の停止確認で「まだ終端でない」を返す。
    pub fn unconfirmed_stops(&self, n: u32) {
        self.remote().unconfirmed_stops = n;
    }

    /// 実行中の累積使用量として報告する値。
    pub fn set_usage(&self, tokens: Option<TokenUsage>, wall_secs: u64) {
        self.remote().usage = Some(Usage { tokens, wall_secs });
    }

    pub fn cancelled(&self) -> Vec<(TaskRef, String)> {
        self.remote().cancelled.clone()
    }
}

impl Executor for MockAsyncExecutor {
    fn name(&self) -> &'static str {
        "mock-async"
    }

    fn simulated(&self) -> bool {
        true
    }

    fn execute(&self, _req: &TrialRequest) -> Result<ExecStatus, String> {
        Err("mock-async is driven via submit/poll".into())
    }

    fn observe_versions(&self, _req: &TrialRequest) -> Result<ObservedVersions, String> {
        Ok(self.versions.clone().unwrap_or_default())
    }

    fn is_async(&self) -> bool {
        true
    }

    fn routing(&self, req: &TrialRequest) -> Result<Value, String> {
        Ok(serde_json::json!({"mode": "mock", "arm": format!("{:?}", req.config.id)}))
    }

    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, ExecError> {
        Ok(self.remote().tasks.get(key).map(|(t, _)| t.clone()))
    }

    fn submit(&self, _req: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
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
            return Err(ExecError::unknown("response lost"));
        }
        Ok(t)
    }

    fn usage(&self, _t: &TaskRef) -> Result<Option<Usage>, ExecError> {
        let mut r = self.remote();
        if r.fail_usages > 0 {
            r.fail_usages -= 1;
            return Err(ExecError::interrupted("usage: connection lost"));
        }
        Ok(r.usage)
    }

    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, ExecError> {
        let mut r = self.remote();
        r.poll_calls += 1;
        if r.fail_polls > 0 {
            r.fail_polls -= 1;
            return Err(ExecError::interrupted("poll: connection lost"));
        }
        let polls = r
            .tasks
            .values_mut()
            .find(|(x, _)| x.task_id == t.task_id)
            .map(|(_, n)| {
                *n += 1;
                *n
            })
            .ok_or_else(|| ExecError::interrupted(format!("unknown task {}", t.task_id)))?;
        if polls <= self.wait_polls {
            return Ok(ExecStatus::Wait);
        }
        Ok(ExecStatus::Done(ExecResult {
            outcome: Outcome::Passed,
            tokens: Some(TokenUsage::full(10_000, 0, 0, 0)),
            cost_micro_usd: Some(100_000),
            wall_secs: 60,
        }))
    }

    fn cancel(&self, t: &TaskRef, reason: &str) -> Result<(), ExecError> {
        let mut r = self.remote();
        if r.fail_cancels > 0 {
            r.fail_cancels -= 1;
            return Err(ExecError::interrupted("cancel: connection lost"));
        }
        r.cancelled.push((t.clone(), reason.into()));
        Ok(())
    }

    fn stopped(&self, t: &TaskRef) -> Result<bool, ExecError> {
        let mut r = self.remote();
        if r.unconfirmed_stops > 0 {
            r.unconfirmed_stops -= 1;
            return Ok(false);
        }
        Ok(r.cancelled.iter().any(|(c, _)| c.task_id == t.task_id))
    }
}
