//! A 無効（B/C のみ）の ArmDispatch は A 実行器を持たず、B/C 側だけを呼ぶ。A 側は 1 回でも呼ばれたら失敗する。
use std::{
    path::Path,
    sync::atomic::{AtomicU32, Ordering},
};

use celeris_campaign::{
    dispatch::ArmDispatch,
    executor::{ExecError, ExecStatus, Executor, TaskRef, TrialRequest, Usage},
    manifest::{Cap, Config, ConfigId},
};

#[derive(Default)]
struct Counting(AtomicU32);
impl Counting {
    fn hit(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
impl Executor for Counting {
    fn name(&self) -> &'static str {
        "counting"
    }
    fn execute(&self, _: &TrialRequest) -> Result<ExecStatus, String> {
        self.hit();
        Err("n/a".into())
    }
    fn routing(&self, _: &TrialRequest) -> Result<serde_json::Value, String> {
        self.hit();
        Ok(serde_json::Value::Null)
    }
    fn lookup(&self, _: &str) -> Result<Option<TaskRef>, ExecError> {
        self.hit();
        Ok(None)
    }
    fn submit(&self, _: &TrialRequest, _: &str) -> Result<TaskRef, ExecError> {
        self.hit();
        Ok(TaskRef {
            task_id: "t1".into(),
            ..TaskRef::default()
        })
    }
    fn track(&self, t: &TaskRef) -> Result<TaskRef, ExecError> {
        self.hit();
        Ok(t.clone())
    }
    fn observed(
        &self,
        _: &TaskRef,
    ) -> Result<Vec<celeris_campaign::executor::ObservedRun>, String> {
        self.hit();
        Ok(vec![])
    }
    fn poll(&self, _: &TaskRef) -> Result<ExecStatus, ExecError> {
        self.hit();
        Ok(ExecStatus::Wait)
    }
    fn cancel(&self, _: &TaskRef, _: &str) -> Result<(), ExecError> {
        self.hit();
        Ok(())
    }
    fn usage(&self, _: &TaskRef) -> Result<Option<Usage>, ExecError> {
        self.hit();
        Ok(None)
    }
    fn stopped(&self, _: &TaskRef) -> Result<bool, ExecError> {
        self.hit();
        Ok(true)
    }
}

fn cfg(id: ConfigId) -> Config {
    Config {
        id,
        harness: "h".into(),
        harness_version: "1".into(),
        model: "m".into(),
        model_version: "1".into(),
        policy_version: "p".into(),
        routing: None,
    }
}

#[test]
fn without_a_only_bc_is_called_and_a_tasks_are_refused() {
    let d = ArmDispatch::<Counting, Counting>::new(None, Counting::default());
    let c = cfg(ConfigId::B);
    let p = Path::new("/nonexistent");
    let req = TrialRequest {
        campaign_id: "c",
        manifest_sha256: "s",
        config: &c,
        problem_id: "p",
        prompt: "",
        trial_no: 1,
        rep: 1,
        workspace: p,
        kb: p,
        timeout_secs: 1,
        budget: Cap {
            max_tokens: 1,
            max_wall_secs: 1,
        },
    };
    let t = TaskRef {
        task_id: "01ABC".into(),
        ..TaskRef::default()
    };
    d.routing(&req).expect("routing");
    d.lookup("campaign-x").expect("lookup");
    d.resolve(&req, "campaign-x").expect("resolve");
    d.track(&t).expect("track");
    d.observed(&t).expect("observed");
    d.poll(&t).expect("poll");
    d.usage(&t).expect("usage");
    d.stopped(&t).expect("stopped");
    d.cancel(&t, "r").expect("cancel");
    // resolve = lookup + submit なので BC 側は 10 回。
    assert_eq!(d.bc.0.load(Ordering::SeqCst), 10);
    // A の task / 試行は A 実行器が無いので拒否され、BC には流れない。
    let a_req = TrialRequest {
        config: &cfg(ConfigId::A),
        ..req
    };
    let a_task = TaskRef {
        task_id: "campaign-abc".into(),
        ..TaskRef::default()
    };
    assert!(d.routing(&a_req).is_err());
    assert!(d.submit(&a_req, "k").is_err());
    assert!(d.resolve(&a_req, "k").is_err());
    assert!(d.poll(&a_task).is_err());
    assert!(d.cancel(&a_task, "r").is_err());
    assert!(d.usage(&a_task).is_err());
    assert!(d.stopped(&a_task).is_err());
    assert!(d.track(&a_task).is_err());
    assert!(d.observed(&a_task).is_err());
    assert_eq!(d.bc.0.load(Ordering::SeqCst), 10);
}
