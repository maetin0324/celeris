//! B/C 用 CelerisExecutor の試験。task-api は in-process の偽 server（[`Fake`]）で置き換え、network に出ない。
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use celeris_campaign::{
    aggregate::Outcome,
    celeris_exec::{
        CLUSTER_CANCEL_FILE, CelerisExecutor, HttpResponse, Transport, idempotency_label,
        routing_override,
    },
    executor::{ExecError, ExecStatus, Executor, MockAsyncExecutor, TaskRef, TrialRequest},
    ledger::{Ledger, LedgerState, TrialKey},
    manifest::{
        ArmRouting, Budget, CampaignManifest, Config, ConfigId, InputRef, Secondary, SnapshotRef,
        Split, Termination, manifest_hash,
    },
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Map, Value, json};

// ---- 偽 task-api ----

#[derive(Clone)]
struct FakeTask {
    status: String,
    labels: Vec<String>,
    created_at: String,
    children: Vec<String>,
    runs: Vec<Value>,
    cluster_job_wait: Value,
    body: Value,
}

#[derive(Default)]
struct State {
    tasks: BTreeMap<String, FakeTask>,
    next: u32,
    now: String,
    posts: u32,
    cancels: Vec<String>,
    /// 次の POST は task を作るが応答を失う。
    lose_next_post: bool,
    /// 次の label 照会を n 回だけ空にする（照会と投入の間の競合の再現）。
    hide_list: u32,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);

const T0: &str = "2026-10-16T00:00:00Z";

impl Fake {
    fn new() -> Self {
        let f = Fake::default();
        f.st().now = T0.into();
        f
    }
    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn create(&self, body: Value, parent: Option<&str>) -> String {
        let mut s = self.st();
        s.next += 1;
        let id = format!("01T{:04}", s.next);
        let labels = body["labels"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let created_at = s.now.clone();
        s.tasks.insert(
            id.clone(),
            FakeTask {
                status: "running".into(),
                labels,
                created_at,
                children: Vec::new(),
                runs: Vec::new(),
                cluster_job_wait: Value::Null,
                body,
            },
        );
        if let Some(p) = parent {
            s.tasks
                .get_mut(p)
                .expect("parent")
                .children
                .push(id.clone());
        }
        id
    }
    fn set(&self, id: &str, f: impl FnOnce(&mut FakeTask)) {
        f(self.st().tasks.get_mut(id).expect("task"))
    }
    fn finish(&self, id: &str, status: &str, tokens: u64) {
        self.set(id, |t| {
            t.status = status.into();
            t.runs.push(
                json!({"run_id": format!("run-{id}"), "finished_at": "2026-10-16T00:10:00Z",
                "usage": {"input_tokens": tokens, "output_tokens": 0, "cost_usd": 0.5}}),
            );
        });
    }
    fn posts(&self) -> u32 {
        self.st().posts
    }
    fn body(&self, id: &str) -> Value {
        self.st().tasks[id].body.clone()
    }
    fn status(&self, id: &str) -> String {
        self.st().tasks[id].status.clone()
    }
}

fn resp(status: u16, body: Value) -> Result<HttpResponse, String> {
    Ok(HttpResponse { status, body })
}

impl Transport for Fake {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        let (p, q) = path.split_once('?').unwrap_or((path, ""));
        let seg: Vec<&str> = p.trim_start_matches("/api/v1/").split('/').collect();
        match (method, seg.as_slice()) {
            ("GET", ["tasks"]) => {
                let label = q
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("label="))
                    .expect("label query");
                let mut s = self.st();
                if s.hide_list > 0 {
                    s.hide_list -= 1;
                    return resp(200, json!({"items": []}));
                }
                let items: Vec<Value> = s
                    .tasks
                    .iter()
                    .filter(|(_, t)| t.labels.iter().any(|l| l == label))
                    .map(|(id, t)| json!({"id": id, "status": t.status}))
                    .collect();
                resp(200, json!({"items": items}))
            }
            ("POST", ["tasks"]) => {
                let b = body.expect("body").clone();
                // task-api と同じ検査: label は 64 文字以下、routing_override.mode は既知の値。
                if b["labels"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|l| l.as_str().is_none_or(|l| l.len() > 64)))
                    || !matches!(
                        b["routing_override"]["mode"].as_str(),
                        Some("fixed" | "decision_assisted")
                    )
                {
                    return resp(400, json!({"title": "bad request"}));
                }
                self.st().posts += 1;
                let id = self.create(b, None);
                let mut s = self.st();
                if s.lose_next_post {
                    s.lose_next_post = false;
                    return Err("connection reset (response lost)".into());
                }
                resp(201, json!({"id": id}))
            }
            ("GET", ["tasks", id]) => {
                let s = self.st();
                let Some(t) = s.tasks.get(*id) else {
                    return resp(404, json!({}));
                };
                let children: Vec<Value> = t.children.iter().map(|c| json!({"id": c})).collect();
                resp(
                    200,
                    json!({"task": {"id": id, "status": t.status, "created_at": t.created_at,
                        "labels": t.labels, "workspace": {"kind": "local", "path": t.body["workspace"]},
                        "repos": t.body["repos"]},
                        "children": children, "runs": t.runs, "timers": {"now": s.now},
                        "cluster_job_wait": t.cluster_job_wait}),
                )
            }
            ("POST", ["tasks", id, "cancel"]) => {
                let mut s = self.st();
                let t = s.tasks.get_mut(*id).expect("task");
                if matches!(t.status.as_str(), "done" | "failed" | "cancelled") {
                    return resp(409, json!({"title": "conflict"}));
                }
                t.status = "cancelled".into();
                s.cancels.push(id.to_string());
                resp(200, json!({}))
            }
            _ => resp(404, json!({})),
        }
    }
}

// ---- 共通 ----

fn cfg(id: ConfigId, routing: Option<ArmRouting>) -> Config {
    Config {
        id,
        harness: if id == ConfigId::A {
            "claude-code"
        } else {
            "celeris"
        }
        .into(),
        harness_version: "1".into(),
        model: "m".into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        // B は routing を必ず固定する（欠落は投入前に Err。routing_pin.rs で固定）。
        routing: routing.or_else(|| (id == ConfigId::B).then(ArmRouting::default)),
    }
}

fn c_routing() -> ArmRouting {
    ArmRouting {
        lower_policy: Some("mlsys-min-1".into()),
        evidence_table_id: Some("ev-2026-10-15".into()),
        online_update: Some(true),
        ..Default::default()
    }
}

fn template() -> Map<String, Value> {
    json!({"acceptance": [{"type": "reviewer"}], "labels": ["campaign"]})
        .as_object()
        .cloned()
        .expect("object")
}

fn exec(f: &Fake, dir: &Path, timeout: u64) -> CelerisExecutor<Fake> {
    CelerisExecutor::new(
        f.clone(),
        template(),
        timeout,
        dir.join(CLUSTER_CANCEL_FILE),
    )
}

fn req<'a>(c: &'a Config, dir: &'a Path) -> TrialRequest<'a> {
    TrialRequest {
        campaign_id: "c1",
        manifest_sha256: "m1",
        config: c,
        problem_id: "p0",
        prompt: "fix the bug",
        trial_no: 1,
        rep: 1,
        workspace: dir,
        kb: dir,
        timeout_secs: 1800,
        budget: celeris_campaign::manifest::Cap {
            max_tokens: u64::MAX,
            max_wall_secs: u64::MAX,
        },
        variant: None,
    }
}

const KEY: &str = "campaign-0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn done(s: ExecStatus) -> celeris_campaign::executor::ExecResult {
    match s {
        ExecStatus::Done(r) => r,
        ExecStatus::Wait => panic!("expected done"),
    }
}

// ---- 受け入れ 0: 投入・poll・応答喪失後の再照会 ----

#[test]
fn submit_poll_and_requery_after_lost_response() {
    let d = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let b = cfg(ConfigId::B, None);
    let x = exec(&f, d.path(), 1800);
    // 投入は届いたが応答を失う。
    f.st().lose_next_post = true;
    let err = x.submit(&req(&b, d.path()), KEY).expect_err("lost");
    assert!(err.msg.contains("response lost"), "{err}");
    assert_eq!(err.kind, celeris_campaign::executor::ErrKind::Unknown);
    assert_eq!(f.posts(), 1);

    // アダプタ再起動（新しい実行器）: 冪等キーで照会し、既存 task を拾う（再投入しない）。
    let x2 = exec(&f, d.path(), 1800);
    let t = x2.resolve(&req(&b, d.path()), KEY).expect("resolve");
    assert_eq!(t.task_id, "01T0001");
    assert_eq!(f.posts(), 1, "no duplicate submit");
    // submit を直接呼んでも（二重起動）照会で同じ task を返す。
    assert_eq!(
        x2.submit(&req(&b, d.path()), KEY).expect("again").task_id,
        t.task_id
    );
    assert_eq!(f.posts(), 1);

    let body = f.body(&t.task_id);
    assert_eq!(body["labels"], json!(["campaign", idempotency_label(KEY)]));
    assert_eq!(idempotency_label(KEY).len(), 64);
    assert!(body["objective"].as_str().expect("obj").contains(KEY));
    assert_eq!(body["workspace"], json!(d.path().to_string_lossy()));
    assert_eq!(body["acceptance"], json!([{"type": "reviewer"}]));

    // 走行中は Wait。作業ツリーは実行器が触らない（消さない）。
    fs::write(d.path().join("work.txt"), "x").expect("write");
    assert_eq!(x2.poll(&t).expect("poll"), ExecStatus::Wait);
    assert!(d.path().join("work.txt").exists());

    f.finish(&t.task_id, "done", 1234);
    let r = done(x2.poll(&t).expect("poll"));
    assert_eq!(r.outcome, Outcome::Passed);
    let u = r.tokens.expect("usage");
    assert_eq!(u.input + u.output, 1234);
    assert_eq!(r.cost_micro_usd, Some(500_000));
    assert_eq!(r.wall_secs, 600);
    assert_eq!(
        x2.track(&t).expect("track").run_id.as_deref(),
        Some("run-01T0001")
    );
}

#[test]
fn concurrent_double_submit_collapses_to_oldest_task() {
    let d = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let b = cfg(ConfigId::B, None);
    let x = exec(&f, d.path(), 1800);
    let first = x.submit(&req(&b, d.path()), KEY).expect("first");
    // 2 つ目の起動は、照会の時点ではまだ 1 つ目が見えない（競合）。
    f.st().hide_list = 1;
    let second = exec(&f, d.path(), 1800)
        .submit(&req(&b, d.path()), KEY)
        .expect("second");
    assert_eq!(second.task_id, first.task_id, "the oldest task wins");
    assert_eq!(f.status("01T0002"), "cancelled", "duplicate closed");
    assert_eq!(f.status(&first.task_id), "running");
    assert_eq!(
        x.lookup(KEY).expect("lookup").expect("some").task_id,
        first.task_id
    );
}

// ---- 受け入れ 1: 子 task 追跡と cancel 方針 ----

#[test]
fn children_and_cluster_jobs_keep_waiting_until_all_terminal() {
    let d = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let b = cfg(ConfigId::B, None);
    let x = exec(&f, d.path(), 1800);
    let t = x.submit(&req(&b, d.path()), KEY).expect("submit");
    let c1 = f.create(json!({}), Some(&t.task_id));
    let g1 = f.create(json!({}), Some(&c1));
    f.set(&c1, |c| {
        c.cluster_job_wait = json!({"cluster": "pegasus", "scheduler": "slurm",
            "jobs": [{"job_id": "42634", "state": "running"}]});
    });
    let tracked = x.track(&t).expect("track");
    assert_eq!(tracked.child_task_ids, vec![c1.clone(), g1.clone()]);
    assert_eq!(
        tracked.cluster_job_ids,
        vec!["slurm:pegasus:42634".to_string()]
    );

    // 根が done でも、子孫が終わるまでは done にしない。
    f.finish(&t.task_id, "done", 10);
    assert_eq!(x.poll(&tracked).expect("poll"), ExecStatus::Wait);
    f.finish(&g1, "done", 20);
    f.finish(&c1, "done", 30);
    // task は全部終端でも cluster job が走っていれば Wait。
    assert_eq!(x.poll(&tracked).expect("poll"), ExecStatus::Wait);
    f.set(&c1, |c| {
        c.cluster_job_wait["jobs"][0]["state"] = json!("finished")
    });
    let r = done(x.poll(&tracked).expect("poll"));
    assert_eq!(r.outcome, Outcome::Passed);
    let u = r.tokens.expect("usage");
    assert_eq!(u.input + u.output, 60, "usage summed over the whole tree");
    // 子 task の失敗は根の結果に従う（根が done なら passed、採点は独立 checker が行う）。
    assert!(f.st().cancels.is_empty());
}

#[test]
fn cancel_closes_descendants_then_root_and_records_cluster_job_step() {
    let d = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let c = cfg(ConfigId::C, Some(c_routing()));
    let x = exec(&f, d.path(), 1800);
    let t = x.submit(&req(&c, d.path()), KEY).expect("submit");
    let c1 = f.create(json!({}), Some(&t.task_id));
    let c2 = f.create(json!({}), Some(&t.task_id));
    let g1 = f.create(json!({}), Some(&c1));
    f.finish(&c2, "done", 1);
    f.set(&g1, |g| {
        g.cluster_job_wait = json!({"cluster": "sirius", "scheduler": "pbs",
            "jobs": [{"job_id": "777", "state": "queued"}, {"job_id": "778", "state": "finished"}]});
    });
    x.cancel(&t, "budget_stop").expect("cancel");
    // 深い順（子孫 → 根）、終端の c2 は対象外。
    assert_eq!(
        f.st().cancels,
        vec![g1.clone(), c1.clone(), t.task_id.clone()]
    );
    let log = fs::read_to_string(d.path().join(CLUSTER_CANCEL_FILE)).expect("log");
    let rows: Vec<Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    assert_eq!(rows.len(), 1, "finished job is not a cancel target");
    assert_eq!(rows[0]["command"], "qdel 777");
    assert_eq!(rows[0]["cluster"], "sirius");
    assert_eq!(
        rows[0]["executed"], false,
        "cluster job cancel is a manual step"
    );
    assert_eq!(rows[0]["reason"], "budget_stop");
    // 再 cancel は冪等（409 を許し、手順も重ねない）。
    x.cancel(&t, "budget_stop").expect("again");
    assert_eq!(f.st().cancels.len(), 3);
    assert_eq!(
        fs::read_to_string(d.path().join(CLUSTER_CANCEL_FILE))
            .expect("log")
            .lines()
            .count(),
        1
    );
}

#[test]
fn trial_timeout_cancels_tree_and_reports_timeout() {
    let d = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let b = cfg(ConfigId::B, None);
    let x = exec(&f, d.path(), 600);
    let t = x.submit(&req(&b, d.path()), KEY).expect("submit");
    let c1 = f.create(json!({}), Some(&t.task_id));
    f.set(&c1, |c| {
        c.cluster_job_wait = json!({"cluster": "pegasus", "scheduler": "slurm",
            "jobs": [{"job_id": "9", "state": "running"}]});
    });
    f.st().now = "2026-10-16T00:09:59Z".into();
    assert_eq!(x.poll(&t).expect("poll"), ExecStatus::Wait);
    f.st().now = "2026-10-16T00:10:01Z".into();
    // task は cancel で閉じても、cluster job が終わるまでは完了扱いにしない（task cancelled ≠ job 停止）。
    assert_eq!(x.poll(&t).expect("poll"), ExecStatus::Wait);
    assert!(!x.stopped(&t).expect("stopped"));
    assert_eq!(x.poll(&t).expect("again"), ExecStatus::Wait);
    f.set(&c1, |c| {
        c.cluster_job_wait = json!({"cluster": "pegasus", "scheduler": "slurm",
            "jobs": [{"job_id": "9", "state": "finished"}]});
    });
    assert!(x.stopped(&t).expect("stopped"));
    let r = done(x.poll(&t).expect("poll"));
    assert_eq!((r.outcome, r.wall_secs), (Outcome::Timeout, 600));
    assert_eq!(f.status(&c1), "cancelled");
    assert_eq!(f.status(&t.task_id), "cancelled");
    let log = fs::read_to_string(d.path().join(CLUSTER_CANCEL_FILE)).expect("log");
    assert!(
        log.contains("scancel 9") && log.contains("trial_timeout"),
        "{log}"
    );
}

// ---- 受け入れ 2: B/C で routing_override が異なる ----

#[test]
fn b_and_c_pin_different_routing_overrides() {
    let d = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let x = exec(&f, d.path(), 1800);
    let b = cfg(ConfigId::B, None);
    let c = cfg(ConfigId::C, Some(c_routing()));
    let tb = x.submit(&req(&b, d.path()), KEY).expect("b");
    let kc = KEY.replace("0123", "4567");
    let tc = x.submit(&req(&c, d.path()), &kc).expect("c");
    let (ob, oc) = (
        f.body(&tb.task_id)["routing_override"].clone(),
        f.body(&tc.task_id)["routing_override"].clone(),
    );
    assert_eq!(
        ob,
        json!({"mode": "fixed", "lower_policy": "mlsys-min-1", "evidence_mode": "none", "online_update": false})
    );
    assert_eq!(
        oc,
        json!({"mode": "decision_assisted", "lower_policy": "mlsys-min-1",
               "evidence_mode": "pinned", "evidence_table_id": "ev-2026-10-15", "online_update": true})
    );
    assert_ne!(ob, oc);
    // 台帳に投入前に記録する値と、投入する値は同じ。
    assert_eq!(x.routing(&req(&b, d.path())), Ok(ob));
    assert_eq!(x.routing(&req(&c, d.path())), Ok(oc));
    // A は Celeris に投入しない。C の根拠表は固定必須（最新版への暗黙追従を許さない）。
    assert!(routing_override(&cfg(ConfigId::A, None)).is_err());
    assert!(routing_override(&cfg(ConfigId::C, None)).is_err());
    let posts = f.posts();
    assert!(
        x.submit(&req(&cfg(ConfigId::A, None), d.path()), "campaign-a")
            .is_err()
    );
    assert_eq!(f.posts(), posts);
}

// ---- runner 経由: Wait で workspace を残し、再開で同じ task を追う ----

/// A は mock、B/C は CelerisExecutor に振り分ける試験用の束ね（CLI の --executor 配線は別 unit）。
struct ByArm<'a> {
    a: &'a MockAsyncExecutor,
    bc: &'a CelerisExecutor<Fake>,
}

impl ByArm<'_> {
    fn by_task(&self, t: &TaskRef) -> &dyn Executor {
        if t.task_id.starts_with("01T") {
            self.bc
        } else {
            self.a
        }
    }
    fn by_req(&self, r: &TrialRequest) -> &dyn Executor {
        if r.config.id == ConfigId::A {
            self.a
        } else {
            self.bc
        }
    }
}

impl Executor for ByArm<'_> {
    fn name(&self) -> &'static str {
        "split"
    }
    fn execute(&self, _: &TrialRequest) -> Result<ExecStatus, String> {
        Err("async only".into())
    }
    fn is_async(&self) -> bool {
        true
    }
    fn routing(&self, r: &TrialRequest) -> Result<Value, String> {
        self.by_req(r).routing(r)
    }
    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, ExecError> {
        Ok(match self.bc.lookup(key)? {
            Some(t) => Some(t),
            None => self.a.lookup(key)?,
        })
    }
    fn submit(&self, r: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
        self.by_req(r).submit(r, key)
    }
    fn track(&self, t: &TaskRef) -> Result<TaskRef, ExecError> {
        self.by_task(t).track(t)
    }
    fn poll(&self, t: &TaskRef) -> Result<ExecStatus, ExecError> {
        self.by_task(t).poll(t)
    }
    fn cancel(&self, t: &TaskRef, reason: &str) -> Result<(), ExecError> {
        self.by_task(t).cancel(t, reason)
    }
}

fn input() -> Input {
    let p = json!({"id":"p0","task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""});
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":[p],"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn campaign(i: &Input) -> CampaignManifest {
    CampaignManifest {
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: "bc".into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: vec!["p0".into()],
        configs: vec![
            cfg(ConfigId::A, None),
            cfg(ConfigId::B, None),
            cfg(ConfigId::C, Some(c_routing())),
        ],
        fault_scenarios: Default::default(),
        experiment_sets: Default::default(),
        selected_sets: Vec::new(),
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec!["p0".into()],
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens: 10_000_000,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            // in-flight 上限はこの試験の対象ではない（max_parallel は parallel_cap.rs）。
            max_parallel: 64,
            per_case: celeris_campaign::manifest::Cap {
                max_tokens: 100_000,
                max_wall_secs: 1_800,
            },
            per_arm: celeris_campaign::manifest::Cap {
                max_tokens: 10_000_000_000,
                max_wall_secs: 1_000_000,
            },
        },
        termination: Termination {
            trial_timeout_secs: 1800,
            max_attempts_per_trial: 1,
            stop_on_budget: true,
        },
        snapshot: SnapshotRef {
            repo_commit: "abc".into(),
            materials_sha256: "d".into(),
            kb_snapshot: "kb@1".into(),
        },
        order: Default::default(),
        seed: 0,
        repeats: 1,
        dependencies: BTreeMap::new(),
        variant: None,
        sha256: String::new(),
    }
}

#[test]
fn runner_waits_keeps_workspace_and_resumes_same_celeris_tasks() {
    let d = tempfile::tempdir().expect("dir");
    let i = input();
    let m = campaign(&i);
    m.validate().expect("valid manifest");
    let opts = RunOptions {
        global_ledger: None,
        out: d.path().join("out"),
        ..RunOptions::default()
    };
    let f = Fake::new();
    let a = MockAsyncExecutor::new(0);
    let bc = exec(&f, d.path(), 1800);
    let rep = run(&m, &i, &ByArm { a: &a, bc: &bc }, &opts).expect("run");
    assert_eq!((rep.executed, rep.waiting), (1, 2), "A done, B/C wait");
    assert_eq!(f.posts(), 2);

    let hash = m.compute_sha256().expect("hash");
    let key = |arm| TrialKey {
        manifest_sha256: hash.clone(),
        problem_id: "p0".into(),
        arm,
        trial_no: 1,
        rep: 1,
    };
    let lg = Ledger::open(&opts.out).expect("ledger");
    let (rb, rc) = (
        lg.get(&key(ConfigId::B)).expect("b").clone(),
        lg.get(&key(ConfigId::C)).expect("c").clone(),
    );
    for r in [&rb, &rc] {
        assert_eq!(r.state, LedgerState::Waiting);
        assert!(
            opts.out.join(&r.workspace).is_dir(),
            "waiting workspace stays"
        );
        // 投入した task の workspace は台帳の試行 workspace そのもの。
        let tid = r.task_id.clone().expect("task id");
        assert_eq!(
            f.body(&tid)["workspace"],
            json!(opts.out.join(&r.workspace).to_string_lossy())
        );
    }
    assert_eq!(rb.routing["mode"], "fixed");
    assert_eq!(rc.routing["mode"], "decision_assisted");
    assert_eq!(rc.routing["evidence_table_id"], "ev-2026-10-15");

    // 外部待ちの間に再起動しても同じ task を追い、重複投入しない。
    let bc2 = exec(&f, d.path(), 1800);
    let rep = run(&m, &i, &ByArm { a: &a, bc: &bc2 }, &opts).expect("resume");
    assert_eq!(rep.waiting, 2);
    assert_eq!(f.posts(), 2);

    for r in [&rb, &rc] {
        f.finish(r.task_id.as_deref().expect("id"), "done", 100);
    }
    let rep = run(&m, &i, &ByArm { a: &a, bc: &bc2 }, &opts).expect("finish");
    assert_eq!(rep.executed, 2);
    assert_eq!(f.posts(), 2, "one task per trial");
    let lg = Ledger::open(&opts.out).expect("ledger");
    assert_eq!(lg.get(&key(ConfigId::B)).expect("b").task_id, rb.task_id);
    assert!(lg.referenced_workspaces().is_empty());
}

// ---- 識別を通常の作成 API（POST /tasks）に載せる ----

#[test]
fn body_carries_campaign_identity_and_idempotency_key() {
    let dir = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let e = exec(&f, dir.path(), 1800);
    let c = cfg(ConfigId::C, Some(c_routing()));
    let mut r = req(&c, dir.path());
    r.rep = 2;
    let b = e.body(&r, KEY).expect("body");
    assert_eq!(
        b["campaign"],
        json!({"campaign_id": "c1", "case_id": "p0", "arm": "C", "repetition": 2,
               "manifest_hash": "m1", "routing_setting": "decision_assisted"})
    );
    assert_eq!(b["campaign_idempotency_key"], json!(idempotency_label(KEY)));
    assert!(
        b["labels"]
            .as_array()
            .expect("labels")
            .contains(&json!(idempotency_label(KEY)))
    );
}

// ---- job 終了確認と所有判定 ----

#[test]
fn cancelled_task_with_live_job_is_not_complete_until_job_finishes() {
    let d = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let c = cfg(ConfigId::C, Some(c_routing()));
    let x = exec(&f, d.path(), 1800);
    let t = x.submit(&req(&c, d.path()), KEY).expect("submit");
    let job = |state: &str| json!({"cluster": "sirius", "scheduler": "pbs", "jobs": [{"job_id": "5", "state": state}]});
    f.set(&t.task_id, |t| t.cluster_job_wait = job("running"));
    x.cancel(&t, "budget_stop").expect("cancel");
    assert_eq!(f.status(&t.task_id), "cancelled");
    // 状態が読めない（unknown）job も終わったとは扱わない。
    for state in ["running", "exiting", "unknown"] {
        f.set(&t.task_id, |t| t.cluster_job_wait = job(state));
        assert!(!x.stopped(&t).expect("stopped"), "{state}");
        assert_eq!(x.poll(&t).expect("poll"), ExecStatus::Wait, "{state}");
    }
    for state in ["finished", "gone"] {
        f.set(&t.task_id, |t| t.cluster_job_wait = job(state));
        assert!(x.stopped(&t).expect("stopped"), "{state}");
        assert_eq!(
            done(x.poll(&t).expect("poll")).outcome,
            Outcome::Cancelled,
            "{state}"
        );
    }
    // 台帳の row は stopped を確認できるまで cancel_pending のまま（runner 側は budget_reserve.rs で固定）。
}

#[test]
fn cancel_never_touches_tasks_or_jobs_not_owned_by_the_campaign() {
    let d = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let c = cfg(ConfigId::B, None);
    let x = exec(&f, d.path(), 1800);
    let mine = x.submit(&req(&c, d.path()), KEY).expect("submit");
    // 無関係な task（campaign の label が無い）とその cluster job。
    let other = f.create(json!({"labels": ["unrelated"]}), None);
    f.set(&other, |t| {
        t.cluster_job_wait = json!({"cluster": "sirius", "scheduler": "pbs",
            "jobs": [{"job_id": "999", "state": "running"}]});
    });
    // 台帳の記録が誤って無関係な task を指していても、止めない・job の手順も記録しない。
    let wrong = TaskRef {
        task_id: other.clone(),
        ..TaskRef::default()
    };
    let e = x.cancel(&wrong, "budget_stop").expect_err("not owned");
    assert_eq!(e.kind, celeris_campaign::executor::ErrKind::Refused);
    assert_eq!(f.status(&other), "running");
    assert!(f.st().cancels.is_empty());
    assert!(!d.path().join(CLUSTER_CANCEL_FILE).exists());
    // 自分の task は止まり、無関係な task は走り続ける。
    x.cancel(&mine, "budget_stop").expect("cancel mine");
    assert_eq!(f.status(&mine.task_id), "cancelled");
    assert_eq!(f.status(&other), "running");
    assert!(!d.path().join(CLUSTER_CANCEL_FILE).exists());
}

#[test]
fn objective_and_cancel_log_carry_cluster_walltime() {
    let d = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let c = cfg(ConfigId::B, None);
    let x = exec(&f, d.path(), 900);
    let t = x.submit(&req(&c, d.path()), KEY).expect("submit");
    assert!(
        f.body(&t.task_id)["objective"]
            .as_str()
            .expect("obj")
            .contains("cluster-job-walltime-secs: 1800")
    );
    f.set(&t.task_id, |t| {
        t.cluster_job_wait = json!({"cluster": "sirius", "scheduler": "pbs",
            "jobs": [{"job_id": "5", "state": "running"}]});
    });
    x.cancel(&t, "trial_timeout").expect("cancel");
    let log = fs::read_to_string(d.path().join(CLUSTER_CANCEL_FILE)).expect("log");
    assert!(log.contains("\"walltime_secs\":900"), "{log}");
}

/// no_evidence_table=true は POST 本文に evidence_mode=none として載り、table id は載らない。
/// C が id も no_evidence_table も無い（暗黙の latest）構成は投入前に拒否する。
#[test]
fn no_evidence_table_posts_evidence_mode_none_and_implicit_latest_is_rejected() {
    let d = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let x = exec(&f, d.path(), 1800);
    let mut r = c_routing();
    r.evidence_table_id = None;
    r.no_evidence_table = Some(true);
    let c = cfg(ConfigId::C, Some(r.clone()));
    let t = x.submit(&req(&c, d.path()), KEY).expect("c none");
    let o = f.body(&t.task_id)["routing_override"].clone();
    assert_eq!(o["evidence_mode"], "none");
    assert!(o.get("evidence_table_id").is_none());
    // 矛盾（none + id）は起動前に拒否。
    r.evidence_table_id = Some("ev".into());
    let err = routing_override(&cfg(ConfigId::C, Some(r.clone()))).unwrap_err();
    assert!(err.contains("contradicts"), "{err}");
    // 暗黙 latest。
    r.evidence_table_id = None;
    r.no_evidence_table = None;
    let err = routing_override(&cfg(ConfigId::C, Some(r))).unwrap_err();
    assert!(err.contains("implicit latest"), "{err}");
}

// ---- B の成果物は trial workspace に出る（ADR 2026-10-20-mlsys-main2-fixes D2） ----

#[test]
fn celeris_body_pins_trial_workspace_and_empty_repos() {
    let dir = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let e = exec(&f, dir.path(), 1800);
    let c = cfg(ConfigId::B, None);
    let b = e.body(&req(&c, dir.path()), KEY).expect("body");
    assert_eq!(b["workspace"], json!(dir.path().to_string_lossy()));
    assert_eq!(b["repos"], json!([]));
    // 空配列だけでは案件の primary repo が補われる（task-ops resolve_repos）。shared で補完を止める。
    assert_eq!(b["workspace_mode"], json!("shared"));
}

#[test]
fn check_template_rejects_repos() {
    for k in ["repos", "workspace_mode"] {
        let mut t = template();
        t.insert(k.into(), json!(["scratch"]));
        let e = celeris_campaign::celeris_exec::check_template(&t).expect_err(k);
        assert!(e.contains(k), "{e}");
    }
}

#[test]
fn lookup_rejects_task_with_foreign_workspace() {
    let dir = tempfile::tempdir().expect("dir");
    let other = tempfile::tempdir().expect("other");
    let f = Fake::new();
    let e = exec(&f, dir.path(), 1800);
    let c = cfg(ConfigId::B, None);
    // 別の場所を作業場所にした既存 task は submit が採用しない。
    let mut body = e.body(&req(&c, dir.path()), KEY).expect("body");
    body["workspace"] = json!(other.path().to_string_lossy());
    f.create(body.clone(), None);
    let err = e
        .submit(&req(&c, dir.path()), KEY)
        .expect_err("foreign workspace");
    assert!(err.to_string().contains("not the trial workspace"), "{err}");
    // repos を持つ task は照会（lookup）も採用しない。
    let g = Fake::new();
    let e2 = exec(&g, dir.path(), 1800);
    let mut body = e2.body(&req(&c, dir.path()), KEY).expect("body");
    body["repos"] = json!([{"name": "scratch"}]);
    g.create(body, None);
    let err = e2.lookup(KEY).expect_err("repos");
    assert!(err.to_string().contains("repos"), "{err}");
}

/// 試行 workspace に worker の代わりに成果物を書き（試験が cwd = task の workspace を再現する）、B の task を done にする。
fn run_b_and_finish(write_artifact: bool) -> (celeris_campaign::record::TrialRecord, Vec<String>) {
    let d = tempfile::tempdir().expect("dir");
    let i = input();
    let m = campaign(&i);
    let opts = RunOptions {
        global_ledger: None,
        out: d.path().join("out"),
        ..RunOptions::default()
    };
    let f = Fake::new();
    let a = MockAsyncExecutor::new(0);
    let bc = exec(&f, d.path(), 1800);
    let by = ByArm { a: &a, bc: &bc };
    let rep = run(&m, &i, &by, &opts).expect("submit");
    assert_eq!(rep.waiting, 2, "B and C wait");
    let hash = m.compute_sha256().expect("hash");
    let key = TrialKey {
        manifest_sha256: hash,
        problem_id: "p0".into(),
        arm: ConfigId::B,
        trial_no: 1,
        rep: 1,
    };
    let lg = Ledger::open(&opts.out).expect("ledger");
    let tid = lg.get(&key).expect("b").task_id.clone().expect("task id");
    if write_artifact {
        // task の workspace.path（= trial workspace）に出力する。scratch repo など別の場所には出ない。
        let ws = f.body(&tid)["workspace"]
            .as_str()
            .expect("path")
            .to_string();
        fs::create_dir_all(Path::new(&ws).join("docs")).expect("docs");
        fs::write(Path::new(&ws).join("docs/analysis.md"), "# analysis\n").expect("artifact");
    }
    f.finish(&tid, "done", 100);
    // C は何も書かずに done（B とは独立）。
    let ctid = lg
        .get(&TrialKey {
            arm: ConfigId::C,
            ..key.clone()
        })
        .and_then(|r| r.task_id.clone())
        .expect("c task id");
    f.finish(&ctid, "done", 100);
    let rep = run(&m, &i, &by, &opts).expect("finish");
    assert_eq!(rep.executed, 2);
    let r = fs::read_to_string(opts.out.join("trials.jsonl"))
        .expect("trials.jsonl")
        .lines()
        .filter_map(|l| serde_json::from_str::<celeris_campaign::record::TrialRecord>(l).ok())
        .find(|r| r.config == ConfigId::B)
        .expect("B record");
    let v = celeris_campaign::checker::check(&m, &opts.out).violations;
    (r, v)
}

#[test]
fn b_done_artifact_lands_in_trial_workspace_and_is_graded() {
    let (r, v) = run_b_and_finish(true);
    assert_eq!(r.outcome, Outcome::Passed);
    assert_eq!(
        r.reason, None,
        "an artifact in the trial workspace is not no_artifact"
    );
    assert_ne!(r.workspace_sha256_final, r.workspace_sha256_initial);
    assert!(
        v.iter().all(|x| !x.contains("without any artifact")),
        "{v:?}"
    );
}

#[test]
fn b_passed_without_workspace_change_is_no_artifact() {
    let (r, v) = run_b_and_finish(false);
    assert_eq!(r.outcome, Outcome::Failed);
    assert_eq!(r.reason.as_deref(), Some("no_artifact"));
    assert!(v.iter().all(|x| !x.contains("no_artifact must")), "{v:?}");
}

// ---- per_case は task の token 予算として渡る（ADR 2026-10-20-mlsys-main2-fixes D1） ----

#[test]
fn celeris_body_sets_per_case_max_tokens() {
    let dir = tempfile::tempdir().expect("dir");
    let f = Fake::new();
    let e = exec(&f, dir.path(), 1800);
    let c = cfg(ConfigId::B, None);
    let mut r = req(&c, dir.path());
    r.budget.max_tokens = 1_500_000;
    let b = e.body(&r, KEY).expect("body");
    assert_eq!(b["max_tokens"], json!(1_500_000));
}

#[test]
fn check_template_rejects_max_tokens() {
    let mut t = template();
    t.insert("max_tokens".into(), json!(1));
    let e = celeris_campaign::celeris_exec::check_template(&t).expect_err("max_tokens");
    assert!(e.contains("max_tokens"), "{e}");
}

#[test]
fn per_case_overshoot_bounded_and_ledger_matches() {
    let d = tempfile::tempdir().expect("dir");
    let i = input();
    let m = campaign(&i); // per_case 100_000
    let opts = RunOptions {
        global_ledger: None,
        out: d.path().join("out"),
        ..RunOptions::default()
    };
    let f = Fake::new();
    let a = MockAsyncExecutor::new(0);
    let bc = exec(&f, d.path(), 1800);
    let by = ByArm { a: &a, bc: &bc };
    run(&m, &i, &by, &opts).expect("submit");
    let hash = m.compute_sha256().expect("hash");
    let lg = Ledger::open(&opts.out).expect("ledger");
    let key = |arm| TrialKey {
        manifest_sha256: hash.clone(),
        problem_id: "p0".into(),
        arm,
        trial_no: 1,
        rep: 1,
    };
    let tid = lg
        .get(&key(ConfigId::B))
        .expect("b")
        .task_id
        .clone()
        .expect("id");
    // task の予算として per_case が渡っている。
    assert_eq!(f.body(&tid)["max_tokens"], json!(100_000));
    // Celeris は残り予算の内側で再開し、上限に達した応答で止める: 2 本の run の和が上限を 1 応答ぶん（1_000）だけ超える。
    f.set(&tid, |t| {
        t.status = "failed".into();
        for (n, tok) in [(1, 60_000u64), (2, 41_000)] {
            t.runs.push(
                json!({"run_id": format!("r{n}"), "finished_at": "2026-10-16T00:10:00Z",
                "usage": {"input_tokens": tok, "output_tokens": 0, "cost_usd": 0.1}}),
            );
        }
    });
    let ctid = lg
        .get(&key(ConfigId::C))
        .and_then(|r| r.task_id.clone())
        .expect("c");
    f.finish(&ctid, "done", 100);
    let rep = run(&m, &i, &by, &opts).expect("finish");
    assert!(
        rep.max_overshoot_tokens <= 1_000,
        "overshoot {}",
        rep.max_overshoot_tokens
    );
    let b: celeris_campaign::record::TrialRecord =
        fs::read_to_string(opts.out.join("trials.jsonl"))
            .expect("trials")
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .find(|r: &celeris_campaign::record::TrialRecord| r.config == ConfigId::B)
            .expect("B record");
    let seen = b.tokens;
    assert_eq!(seen, Some(101_000), "record tokens = sum of run usages");
    // 台帳の課金は各試行の観測値の和（B は 101_000）と一致する。
    let observed: u64 = fs::read_to_string(opts.out.join("trials.jsonl"))
        .expect("trials")
        .lines()
        .filter_map(|l| serde_json::from_str::<celeris_campaign::record::TrialRecord>(l).ok())
        .filter_map(|r| r.tokens)
        .sum();
    assert_eq!(
        rep.tokens_charged_total, observed,
        "ledger charge matches observed"
    );
}
