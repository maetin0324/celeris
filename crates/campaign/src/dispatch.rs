//! `--executor harness|celeris` の実体: A 腕は HarnessExecutor、B/C 腕は CelerisExecutor に振り分ける。
//! 台帳の `TaskRef` は task_id の形で見分ける（A = 冪等キー `campaign-…`、B/C = Celeris の task id）。
use serde_json::Value;

use crate::{
    executor::{ExecError, ExecStatus, Executor, ObservedVersions, TaskRef, TrialRequest, Usage},
    manifest::ConfigId,
};

/// A の task_id は冪等キーそのもの（`ledger::TrialKey::idempotency_key` が `campaign-` で始まる）。
const A_PREFIX: &str = "campaign-";

/// `a` は A が有効な manifest のときだけ持つ。無効なら `None` で、A 実行器は構築も呼出しもされない。
pub struct ArmDispatch<A: Executor, BC: Executor> {
    pub a: Option<A>,
    pub bc: BC,
}

fn a_disabled() -> String {
    "arm A is not selected in this experiment set; its executor is not available".into()
}

impl<A: Executor, BC: Executor> ArmDispatch<A, BC> {
    pub fn new(a: Option<A>, bc: BC) -> Self {
        Self { a, bc }
    }
    /// `None` は「A の task だが A 実行器が無い」。
    fn by_task(&self, t: &TaskRef) -> Option<&dyn Executor> {
        if t.task_id.starts_with(A_PREFIX) {
            self.a.as_ref().map(|a| a as &dyn Executor)
        } else {
            Some(&self.bc)
        }
    }
    fn by_req(&self, r: &TrialRequest) -> Option<&dyn Executor> {
        if r.config.id == ConfigId::A {
            self.a.as_ref().map(|a| a as &dyn Executor)
        } else {
            Some(&self.bc)
        }
    }
}

impl<A: Executor, BC: Executor> Executor for ArmDispatch<A, BC> {
    fn name(&self) -> &'static str {
        "arm-dispatch"
    }
    fn simulated(&self) -> bool {
        self.a.as_ref().is_some_and(|a| a.simulated()) || self.bc.simulated()
    }
    fn execute(&self, _: &TrialRequest) -> Result<ExecStatus, String> {
        Err("async only (use submit/poll via the ledger)".into())
    }
    fn is_async(&self) -> bool {
        true
    }
    fn routing(&self, r: &TrialRequest) -> Result<Value, String> {
        self.by_req(r).ok_or_else(a_disabled)?.routing(r)
    }
    fn observe_versions(&self, r: &TrialRequest) -> Result<ObservedVersions, String> {
        self.by_req(r).ok_or_else(a_disabled)?.observe_versions(r)
    }
    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, ExecError> {
        // 腕は key だけでは分からないので、有効な側だけを引く（A 無効なら B/C だけ）。
        if let Some(a) = &self.a
            && let Some(t) = a.lookup(key)?
        {
            return Ok(Some(t));
        }
        self.bc.lookup(key)
    }
    fn submit(&self, r: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
        self.by_req(r)
            .ok_or_else(|| ExecError::refused(a_disabled()))?
            .submit(r, key)
    }
    fn resolve(&self, r: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
        self.by_req(r)
            .ok_or_else(|| ExecError::refused(a_disabled()))?
            .resolve(r, key)
    }
    fn track(&self, t: &TaskRef) -> Result<TaskRef, ExecError> {
        self.by_task(t)
            .ok_or_else(|| ExecError::interrupted(a_disabled()))?
            .track(t)
    }
    fn observed(&self, t: &TaskRef) -> Result<Vec<crate::record::ObservedRun>, String> {
        self.by_task(t).ok_or_else(a_disabled)?.observed(t)
    }
    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, ExecError> {
        self.by_task(t)
            .ok_or_else(|| ExecError::interrupted(a_disabled()))?
            .poll(t)
    }
    fn cancel(&self, t: &TaskRef, reason: &str) -> Result<(), ExecError> {
        self.by_task(t)
            .ok_or_else(|| ExecError::interrupted(a_disabled()))?
            .cancel(t, reason)
    }
    fn usage(&self, t: &TaskRef) -> Result<Option<Usage>, ExecError> {
        self.by_task(t)
            .ok_or_else(|| ExecError::interrupted(a_disabled()))?
            .usage(t)
    }
    fn stopped(&self, t: &TaskRef) -> Result<bool, ExecError> {
        self.by_task(t)
            .ok_or_else(|| ExecError::interrupted(a_disabled()))?
            .stopped(t)
    }
}
