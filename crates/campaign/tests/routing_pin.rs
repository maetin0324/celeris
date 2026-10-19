//! B/C の routing を task（試行）単位で固定し、記録し、checker が manifest と突き合わせる試験。
//! task-api は in-process の偽 transport で置き換え、network に出ない。全体 config は一切切り替えない。
use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, Mutex},
};

use celeris_campaign::{
    aggregate::Outcome,
    celeris_exec::{
        CLUSTER_CANCEL_FILE, CelerisExecutor, HttpResponse, Transport, routing_override,
    },
    checker::check,
    dispatch::ArmDispatch,
    executor::{Executor, TrialRequest},
    ledger::{Ledger, TrialKey},
    manifest::{
        ArmRouting, Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary,
        SnapshotRef, Split, Termination, manifest_hash,
    },
    record::{REASON_CASE_BUDGET, REASON_EXECUTOR_ERROR, TRIALS_FILE, TrialRecord},
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Map, Value, json};

/// 偽 task-api。POST /tasks は即 done の task を作る。tokens は problem ごとに指定できる。
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    /// (id, 投入本文)
    posts: Vec<(String, Value)>,
    /// 実測を宣言から外す（"model" / "harness" / "online_update" / "global"）。
    deviate: Option<&'static str>,
}

impl Fake {
    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn posts(&self) -> Vec<(String, Value)> {
        self.st().posts.clone()
    }
}

fn tokens_of(body: &Value) -> u64 {
    // per_case を超える試行は B の p1 だけ（title に問題名が入る）。
    let t = body["title"].as_str().unwrap_or_default();
    if t.contains("B") && t.contains(" p1 ") {
        9_000
    } else {
        100
    }
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
        let ok = |b: Value| {
            Ok(HttpResponse {
                status: 200,
                body: b,
            })
        };
        let mut s = self.st();
        match (method, seg.as_slice()) {
            ("GET", ["tasks"]) => {
                let label = q
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("label="))
                    .unwrap_or("");
                let items: Vec<Value> = s
                    .posts
                    .iter()
                    .filter(|(_, b)| {
                        b["labels"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|l| l == label))
                    })
                    .map(|(id, _)| json!({"id": id, "status": "done"}))
                    .collect();
                ok(json!({"items": items}))
            }
            ("POST", ["tasks"]) => {
                let b = body.cloned().unwrap_or_default();
                let id = format!("T{:04}", s.posts.len() + 1);
                s.posts.push((id.clone(), b));
                Ok(HttpResponse {
                    status: 201,
                    body: json!({"id": id}),
                })
            }
            ("GET", ["tasks", id]) => {
                let Some((_, b)) = s.posts.iter().find(|(i, _)| i == id) else {
                    return Ok(HttpResponse {
                        status: 404,
                        body: json!({}),
                    });
                };
                let t = tokens_of(b);
                // 実測: run の選択と、判断 record の実効 routing（task 欄から解決された値）。
                let pin = &b["routing_override"];
                let dv = s.deviate;
                let global = dv == Some("global");
                let resolved = json!({
                    "mode": if global { json!("fixed") } else { pin["mode"].clone() },
                    "source": if global { "config" } else { "task" },
                    "lower_policy": pin["lower_policy"],
                    "evidence_mode": if dv == Some("evidence_mode") { json!("latest") } else { pin["evidence_mode"].clone() },
                    "evidence_table_id": pin["evidence_table_id"],
                    "online_update": if dv == Some("online_update") { json!(true) } else { pin["online_update"].clone() },
                });
                let selection = json!({"status": "selected",
                    "harness_id": if dv == Some("harness") { "codex" } else { "claude-code" },
                    "source_id": "src", "account_id": "acc",
                    "model_id": if dv == Some("model") { "m-other" } else { "m-x" },
                    "basis": {"route_decision": {"routing_resolved": resolved}}});
                ok(
                    json!({"task": {"id": id, "status": "done", "created_at": "2026-10-16T00:00:00Z"},
                    "children": [],
                    "runs": [{"run_id": format!("run-{id}"), "role": "worker", "selection": selection,
                        "finished_at": "2026-10-16T00:01:00Z",
                        "usage": {"input_tokens": t, "output_tokens": 0, "cost_usd": 0.1}}],
                    "timers": {"now": "2026-10-16T00:02:00Z"}, "cluster_job_wait": null}),
                )
            }
            ("POST", ["tasks", _, "cancel"]) => ok(json!({})),
            _ => Ok(HttpResponse {
                status: 404,
                body: json!({}),
            }),
        }
    }
}

/// A の実行器: 投入が常に失敗する（A の失敗・枯渇が B/C を止めないことの確認用）。
struct DownA;

impl Executor for DownA {
    fn name(&self) -> &'static str {
        "down-a"
    }
    fn execute(&self, _: &TrialRequest) -> Result<celeris_campaign::executor::ExecStatus, String> {
        Err("a down".into())
    }
    fn is_async(&self) -> bool {
        true
    }
}

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
        routing,
    }
}

fn b_routing() -> ArmRouting {
    ArmRouting {
        lower_policy: Some("lp-b-7".into()),
        evidence_table_id: None,
        online_update: Some(false),
        models: Some(vec!["m-x".into()]),
        run_harnesses: Some(vec!["claude-code".into()]),
        ..Default::default()
    }
}

fn c_routing() -> ArmRouting {
    ArmRouting {
        lower_policy: Some("lp-c-9".into()),
        evidence_table_id: Some("ev-2026-10-15".into()),
        online_update: Some(true),
        models: Some(vec!["m-x".into()]),
        run_harnesses: Some(vec!["claude-code".into()]),
        ..Default::default()
    }
}

fn input(n: usize) -> Input {
    let problems: Vec<Value> = (0..n)
        .map(|i| json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn campaign(i: &Input, n: usize, b: Option<ArmRouting>, c: Option<ArmRouting>) -> CampaignManifest {
    CampaignManifest {
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: "pin".into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: (0..n).map(|k| format!("p{k}")).collect(),
        configs: vec![
            cfg(ConfigId::A, None),
            cfg(ConfigId::B, b),
            cfg(ConfigId::C, c),
        ],
        experiment_sets: Default::default(),
        selected_sets: Vec::new(),
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: vec!["p0".into(), "p1".into()],
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens: 10_000_000,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 1,
            per_case: Cap {
                max_tokens: 5_000,
                max_wall_secs: 1_800,
            },
            per_arm: Cap {
                max_tokens: 10_000_000,
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
        sha256: String::new(),
    }
}

fn template() -> Map<String, Value> {
    json!({"acceptance": [{"type": "reviewer"}], "labels": ["campaign"]})
        .as_object()
        .cloned()
        .expect("object")
}

fn dispatch(f: &Fake, dir: &std::path::Path) -> ArmDispatch<DownA, CelerisExecutor<Fake>> {
    ArmDispatch {
        a: DownA,
        bc: CelerisExecutor::new(f.clone(), template(), 1800, dir.join(CLUSTER_CANCEL_FILE)),
    }
}

fn records(out: &std::path::Path) -> Vec<TrialRecord> {
    fs::read_to_string(out.join(TRIALS_FILE))
        .expect("trials")
        .lines()
        .map(|l| serde_json::from_str(l).expect("record"))
        .collect()
}

/// manifest の arm 設定が、全体 config を触らず、同じ executor から試行（task）ごとに別々に固定される。
#[test]
fn each_task_carries_its_arm_routing_and_the_record_matches_the_manifest() {
    let d = tempfile::tempdir().expect("dir");
    let (i, f) = (input(2), Fake::default());
    let m = campaign(&i, 2, Some(b_routing()), Some(c_routing()));
    m.validate().expect("valid");
    let opts = RunOptions {
        global_ledger: None,
        out: d.path().join("out"),
        ..RunOptions::default()
    };
    run(&m, &i, &dispatch(&f, d.path()), &opts).expect("run");

    let want_b = routing_override(&m.configs[1]).expect("b");
    let want_c = routing_override(&m.configs[2]).expect("c");
    assert_eq!(
        want_b,
        json!({"mode":"fixed","lower_policy":"lp-b-7","evidence_mode":"none","online_update":false})
    );
    assert_eq!(
        want_c,
        json!({"mode":"decision_assisted","lower_policy":"lp-c-9","evidence_mode":"pinned","evidence_table_id":"ev-2026-10-15","online_update":true})
    );
    // 投入した task の本文: B は B の、C は C の routing_override（同じ executor・同じ run の中で）。
    let posts = f.posts();
    assert_eq!(posts.len(), 4, "B x2 + C x2 (A never reaches Celeris)");
    for (_, b) in &posts {
        let title = b["title"].as_str().expect("title");
        let want = if title.contains("B") {
            &want_b
        } else {
            &want_c
        };
        assert_eq!(&b["routing_override"], want, "{title}");
    }
    // 試行記録・台帳にも実効 routing が残る。A は持たない。
    let recs = records(&opts.out);
    let hash = m.compute_sha256().expect("hash");
    let lg = Ledger::open(&opts.out).expect("ledger");
    for r in &recs {
        match r.config {
            ConfigId::A => assert!(r.routing.is_none()),
            c => {
                let want = if c == ConfigId::B { &want_b } else { &want_c };
                assert_eq!(
                    r.routing.as_ref(),
                    Some(want),
                    "{:?}/{}",
                    r.config,
                    r.problem_id
                );
                let key = TrialKey {
                    manifest_sha256: hash.clone(),
                    problem_id: r.problem_id.clone(),
                    arm: c,
                    trial_no: 1,
                    rep: 1,
                };
                assert_eq!(lg.get(&key).expect("row").routing, *want);
            }
        }
    }
    let rep = check(&m, &opts.out);
    assert!(rep.ok, "{:?}", rep.violations);
}

/// B/C の routing 欠落、C の根拠表 id 無しは投入前に Err（task を 1 つも作らない）。
#[test]
fn missing_routing_is_rejected_before_any_submission() {
    assert!(routing_override(&cfg(ConfigId::B, None)).is_err());
    assert!(routing_override(&cfg(ConfigId::C, None)).is_err());
    let no_evidence = ArmRouting {
        evidence_table_id: None,
        ..c_routing()
    };
    assert!(routing_override(&cfg(ConfigId::C, Some(no_evidence.clone()))).is_err());

    let d = tempfile::tempdir().expect("dir");
    let (i, f) = (input(2), Fake::default());
    let m = campaign(&i, 2, None, Some(no_evidence));
    let opts = RunOptions {
        global_ledger: None,
        out: d.path().join("out"),
        ..RunOptions::default()
    };
    let rep = run(&m, &i, &dispatch(&f, d.path()), &opts).expect("run continues");
    assert!(
        f.posts().is_empty(),
        "nothing was submitted to the task API"
    );
    // 拒否された B/C 試行は error として分母に残り、routing は記録しない。checker は欠落を違反にしない。
    let recs = records(&opts.out);
    for r in recs.iter().filter(|r| r.config != ConfigId::A) {
        assert_eq!(r.outcome, Outcome::Error);
        assert_eq!(r.reason.as_deref(), Some(REASON_EXECUTOR_ERROR));
        assert!(r.routing.is_none());
    }
    assert_eq!(rep.executor_errors, 2 + 2 + 2);
    let c = check(&m, &opts.out);
    assert!(c.ok, "{:?}", c.violations);
}

fn tamper(out: &std::path::Path, f: impl Fn(&mut Value)) {
    let s: String = fs::read_to_string(out.join(TRIALS_FILE))
        .expect("read")
        .lines()
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).expect("json");
            f(&mut v);
            format!("{v}\n")
        })
        .collect();
    fs::write(out.join(TRIALS_FILE), s).expect("write");
}

/// checker は記録された routing と manifest の arm 設定の食い違い・欠落・A への付与を検出する。
#[test]
fn checker_flags_routing_that_differs_from_the_manifest() {
    let d = tempfile::tempdir().expect("dir");
    let (i, f) = (input(2), Fake::default());
    let m = campaign(&i, 2, Some(b_routing()), Some(c_routing()));
    let opts = RunOptions {
        global_ledger: None,
        out: d.path().join("out"),
        ..RunOptions::default()
    };
    run(&m, &i, &dispatch(&f, d.path()), &opts).expect("run");
    assert!(check(&m, &opts.out).ok);
    let orig = fs::read_to_string(opts.out.join(TRIALS_FILE)).expect("orig");

    let violations = |f: &dyn Fn(&mut Value)| {
        tamper(&opts.out, f);
        let r = check(&m, &opts.out);
        fs::write(opts.out.join(TRIALS_FILE), &orig).expect("restore");
        r.violations.join("\n")
    };
    let v = violations(&|r| {
        if r["config"] == "C" {
            r["routing"]["evidence_table_id"] = json!("ev-latest");
        }
    });
    assert!(v.contains("differs from the manifest arm setting"), "{v}");
    let v = violations(&|r| {
        if r["config"] == "B" {
            r["routing"]["mode"] = json!("decision_assisted");
        }
    });
    assert!(v.contains("differs from the manifest arm setting"), "{v}");
    let v = violations(&|r| {
        if r["config"] == "B" {
            r.as_object_mut().expect("obj").remove("routing");
        }
    });
    assert!(v.contains("ran without a recorded routing"), "{v}");
    let v = violations(&|r| {
        if r["config"] == "A" {
            r["routing"] = json!({"mode": "fixed"});
        }
    });
    assert!(v.contains("arm A must not carry"), "{v}");
}

/// 通し: 2 反復・巡回順・A の全失敗・B の上限超過・routing 固定が 1 回の run で揃い、checker も通る。
#[test]
fn e2e_budget_isolation_order_repeats_and_routing_hold_together() {
    let d = tempfile::tempdir().expect("dir");
    let (i, f) = (input(3), Fake::default());
    let mut m = campaign(&i, 3, Some(b_routing()), Some(c_routing()));
    m.repeats = 2;
    m.validate().expect("valid");
    let opts = RunOptions {
        global_ledger: None,
        out: d.path().join("out"),
        ..RunOptions::default()
    };
    let rep = run(&m, &i, &dispatch(&f, d.path()), &opts).expect("run");
    let recs = records(&opts.out);
    // 計画数: A/B = 3 problems x 2 reps、C = subset 2 x 2 reps。分母は減らない。
    assert_eq!(recs.len(), 6 + 6 + 4);
    // 隔離: A は全試行が失敗でも B/C は全試行が実行された。
    assert!(
        recs.iter()
            .filter(|r| r.config == ConfigId::A)
            .all(|r| r.outcome == Outcome::Error)
    );
    assert_eq!(f.posts().len(), 6 + 4);
    // 予算: B の p1（2 反復とも）は per_case 超過で cancelled のまま分母に残る。欠測は 0 扱いされない。
    let over: Vec<&TrialRecord> = recs
        .iter()
        .filter(|r| r.reason.as_deref() == Some(REASON_CASE_BUDGET))
        .collect();
    assert_eq!(over.len(), 2);
    assert!(over.iter().all(|r| r.config == ConfigId::B
        && r.problem_id == "p1"
        && r.outcome == Outcome::Cancelled));
    assert_eq!(rep.case_budget_cancelled, 2);
    // 反復: rep は別 ID・別 out、内部再試行は常に 1。
    let ws: std::collections::BTreeSet<&str> = recs
        .iter()
        .filter(|r| r.config == ConfigId::B)
        .map(|r| r.workspace.as_str())
        .collect();
    assert_eq!(ws.len(), 6);
    assert!(
        recs.iter()
            .all(|r| r.trial_no == 1 && (1..=2).contains(&r.rep))
    );
    // 順序: problem ごとの arm 位置が巡回する（p0: A 先頭、p1: B 先頭 …）。
    let rank = |c, p: &str| {
        recs.iter()
            .find(|r| r.config == c && r.problem_id == p && r.rep == 1)
            .map(|r| r.arm_rank)
    };
    assert_eq!(rank(ConfigId::A, "p0"), Some(1));
    assert_eq!(rank(ConfigId::B, "p1"), Some(1));
    // routing: B/C の全試行に manifest 由来の実効 routing が残る。
    for r in recs.iter().filter(|r| r.config != ConfigId::A) {
        assert_eq!(
            r.routing.as_ref(),
            Some(
                &routing_override(m.configs.iter().find(|c| c.id == r.config).expect("cfg"))
                    .expect("ro")
            )
        );
    }
    let c = check(&m, &opts.out);
    assert!(c.ok, "{:?}", c.violations);
    let totals = c.recomputed.expect("recomputed").totals;
    assert_eq!(totals[&ConfigId::A].n, 6);
    assert_eq!(totals[&ConfigId::B].n, 6);
    assert_eq!(totals[&ConfigId::B].cancelled, 2);
    assert_eq!(totals[&ConfigId::C].n, 4);
}

fn run_with(
    deviate: Option<&'static str>,
) -> (CampaignManifest, std::path::PathBuf, tempfile::TempDir) {
    let d = tempfile::tempdir().expect("dir");
    let (i, f) = (input(2), Fake::default());
    f.st().deviate = deviate;
    let mut b = b_routing();
    let mut c = c_routing();
    // online_update は arm の宣言どおりに走らせる試験。B・C とも false を宣言する。
    b.online_update = Some(false);
    c.online_update = Some(false);
    let m = campaign(&i, 2, Some(b), Some(c));
    let opts = RunOptions {
        global_ledger: None,
        out: d.path().join("out"),
        ..RunOptions::default()
    };
    run(&m, &i, &dispatch(&f, d.path()), &opts).expect("run");
    let out = opts.out;
    (m, out, d)
}

/// 実測を記録し、照合が通り、export の表は宣言名でなく実測の model/harness/routing を出す。
#[test]
fn export_shows_observed_model_harness_and_routing_not_declared_names() {
    let (m, out, d) = run_with(None);
    let recs = records(&out);
    for r in recs.iter().filter(|r| r.config != ConfigId::A) {
        assert_eq!(r.observed.len(), 1, "{:?}", r.config);
        assert_eq!(r.observed[0].model.as_deref(), Some("m-x"));
        assert_eq!(r.observed[0].routing_source.as_deref(), Some("task"));
    }
    let rep = check(&m, &out);
    assert!(rep.ok, "{:?}", rep.violations);
    let table = celeris_campaign::export::export(&m, &out, &d.path().join("exp")).expect("export");
    let rows = table["configs"].as_array().expect("rows");
    let row = |c: &str| rows.iter().find(|r| r["config"] == c).expect("row").clone();
    for (c, mode) in [("B", "fixed"), ("C", "decision_assisted")] {
        let r = row(c);
        assert_eq!(r["model"], "m-x", "実測の model（宣言の \"m\" ではない）");
        assert_eq!(r["harness"], "claude-code");
        assert_eq!(r["declared_model"], "m");
        assert_eq!(r["declared_harness"], "celeris");
        assert_eq!(r["routing"], format!("{mode}/task/online_update=false"));
    }
    // A は観測されていない（A の executor がこの試験では動かない）: 宣言名を実測として出さず欠測。
    assert_eq!(row("A")["model"], Value::Null);
}

/// 禁止された構成（別 model・別 harness・online_update=true・全体 config への fallback）は照合で検出する。
#[test]
fn reconciliation_flags_forbidden_deviations_and_export_refuses() {
    for (dev, needle) in [
        ("model", "outside the arm's models set"),
        ("harness", "outside the arm's run_harnesses set"),
        ("online_update", "online_update true differs"),
        ("global", "not the pinned task field"),
        ("evidence_mode", "evidence_mode"),
    ] {
        let (m, out, d) = run_with(Some(dev));
        let rep = check(&m, &out);
        assert!(!rep.ok, "{dev}");
        assert!(
            rep.violations.iter().any(|v| v.contains(needle)),
            "{dev}: {:?}",
            rep.violations
        );
        assert!(
            celeris_campaign::export::export(&m, &out, &d.path().join("exp")).is_err(),
            "{dev}"
        );
    }
}

/// B/C は同じ利用可能モデル集合・下段で比べる。C の根拠表は明示すれば要らない。
#[test]
fn b_and_c_must_share_models_and_c_may_declare_no_evidence_table() {
    let i = input(2);
    let mut m = campaign(&i, 2, Some(b_routing()), Some(c_routing()));
    m.split = Split::Final;
    for c in &mut m.configs {
        c.harness_version = if c.harness == "celeris" {
            "0123456789ab"
        } else {
            "1"
        }
        .into();
        if let Some(r) = &mut c.routing {
            r.online_update = Some(false);
            r.lower_policy = Some("lp".into());
        }
    }
    m.configs[0].model = "m-x".into();
    // C は根拠表を使わないと明示（根拠表 id 無しで通る）。
    if let Some(r) = &mut m.configs[2].routing {
        r.evidence_table_id = None;
        r.no_evidence_table = Some(true);
    }
    m.check_resolved().expect("no-evidence C is valid");
    assert!(
        routing_override(&m.configs[2])
            .expect("override")
            .get("evidence_table_id")
            .is_none()
    );
    assert_eq!(
        routing_override(&m.configs[2]).expect("override")["evidence_mode"],
        "none"
    );
    // C が id も no_evidence_table も無い（暗黙 latest）構成は preflight（check_resolved）で拒否。
    let mut implicit = m.clone();
    if let Some(r) = &mut implicit.configs[2].routing {
        r.no_evidence_table = None;
    }
    assert!(implicit.check_resolved().is_err());
    // C だけ別のモデル集合へ逸れる構成は起動前に拒否する。
    if let Some(r) = &mut m.configs[2].routing {
        r.models = Some(vec!["m-y".into()]);
    }
    assert!(m.check_resolved().unwrap_err().contains("same models set"));
}
