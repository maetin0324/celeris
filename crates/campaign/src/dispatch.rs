//! `--executor harness|celeris` の実体: A 腕は HarnessExecutor、B/C 腕は CelerisExecutor に振り分ける。
//! 台帳の `TaskRef` は task_id の形で見分ける（A = 冪等キー `campaign-…`、B/C = Celeris の task id）。
use serde_json::Value;

use crate::{
    executor::{ExecStatus, Executor, TaskRef, TrialRequest},
    manifest::ConfigId,
};

/// A の task_id は冪等キーそのもの（`ledger::TrialKey::idempotency_key` が `campaign-` で始まる）。
const A_PREFIX: &str = "campaign-";

pub struct ArmDispatch<A: Executor, BC: Executor> {
    pub a: A,
    pub bc: BC,
}

impl<A: Executor, BC: Executor> ArmDispatch<A, BC> {
    fn by_task(&self, t: &TaskRef) -> &dyn Executor {
        if t.task_id.starts_with(A_PREFIX) {
            &self.a
        } else {
            &self.bc
        }
    }
    fn by_req(&self, r: &TrialRequest) -> &dyn Executor {
        if r.config.id == ConfigId::A {
            &self.a
        } else {
            &self.bc
        }
    }
}

impl<A: Executor, BC: Executor> Executor for ArmDispatch<A, BC> {
    fn name(&self) -> &'static str {
        "arm-dispatch"
    }
    fn simulated(&self) -> bool {
        self.a.simulated() || self.bc.simulated()
    }
    fn execute(&self, _: &TrialRequest) -> Result<ExecStatus, String> {
        Err("async only (use submit/poll via the ledger)".into())
    }
    fn is_async(&self) -> bool {
        true
    }
    fn routing(&self, r: &TrialRequest) -> Result<Value, String> {
        self.by_req(r).routing(r)
    }
    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, String> {
        // 腕は key だけでは分からないので両方を引く（A は局所 file、B/C は task-api）。
        match self.a.lookup(key)? {
            Some(t) => Ok(Some(t)),
            None => self.bc.lookup(key),
        }
    }
    fn submit(&self, r: &TrialRequest, key: &str) -> Result<TaskRef, String> {
        self.by_req(r).submit(r, key)
    }
    fn resolve(&self, r: &TrialRequest, key: &str) -> Result<TaskRef, String> {
        self.by_req(r).resolve(r, key)
    }
    fn track(&self, t: &TaskRef) -> Result<TaskRef, String> {
        self.by_task(t).track(t)
    }
    fn observed(&self, t: &TaskRef) -> Result<Vec<crate::record::ObservedRun>, String> {
        self.by_task(t).observed(t)
    }
    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, String> {
        self.by_task(t).poll(t)
    }
    fn cancel(&self, t: &TaskRef, reason: &str) -> Result<(), String> {
        self.by_task(t).cancel(t, reason)
    }
}
