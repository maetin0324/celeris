//! 最終報告の消費集計（final_accounting）: 外側 trial（campaign 表）と decision-log export の run・Jev・CoS を
//! campaign/case/arm/repetition/hash で結合し、重複排除・欠測・共有・retry を既知量から再計算して固定する。
use std::{path::Path, sync::Arc};

use celeris_campaign::{
    aggregate::Outcome,
    executor::{ExecResult, ExecStatus, Executor, TrialRequest},
    final_accounting::{FinalInputs, compute, recheck},
    manifest::{
        ArmRouting, Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary,
        SnapshotRef, Split, Termination, TokenBasis, TokenUsage, manifest_hash,
    },
    report::check_consumption_claims,
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Value, json};
use task_core::decision_log::{
    ClassifierRecord, DecisionLogRun, DecisionLogUsage, UsageKey, UsageRole,
};
use task_core::decision_log_export::{ExportParams, build_campaign_export};

const COS_BASIS: &str = "CoS session log for the campaign window is empty";

fn input() -> Input {
    let problems: Vec<Value> = vec![
        json!({"id":"p0","task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}),
    ];
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
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

fn campaign(i: &Input) -> CampaignManifest {
    let b = ArmRouting {
        lower_policy: Some("lp".into()),
        online_update: Some(false),
        ..Default::default()
    };
    let c = ArmRouting {
        evidence_table_id: Some("ev".into()),
        ..b.clone()
    };
    CampaignManifest {
        contract: None,
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: "c1".into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: vec!["p0".into()],
        configs: vec![
            cfg(ConfigId::A, None),
            cfg(ConfigId::B, Some(b)),
            cfg(ConfigId::C, Some(c)),
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
            // cache_read / cache_creation を含む定義（既定の論文用）。
            token_basis: TokenBasis::All,
            max_tokens: 10_000_000,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 64,
            per_case: Cap {
                max_tokens: 1_000,
                max_wall_secs: 600,
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
        repeats: 2,
        dependencies: Default::default(),
        variant: None,
        sha256: String::new(),
    }
}

/// 外側の trial 1 件の結果: (outcome, tokens)。
type Outer = Arc<dyn Fn(ConfigId, u32) -> (Outcome, Option<TokenUsage>) + Send + Sync>;

struct Table(Outer);

impl Executor for Table {
    fn name(&self) -> &'static str {
        "table"
    }
    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String> {
        std::fs::write(req.workspace.join("out.txt"), "x").map_err(|e| e.to_string())?;
        let (outcome, tokens) = (self.0)(req.config.id, req.rep);
        let wall_secs = if outcome == Outcome::Timeout {
            req.timeout_secs
        } else {
            1
        };
        Ok(ExecStatus::Done(ExecResult {
            outcome,
            tokens,
            cost_micro_usd: Some(7),
            wall_secs,
        }))
    }
}

fn tu(i: u64, o: u64, cr: u64, cc: u64) -> Option<TokenUsage> {
    Some(TokenUsage::full(i, o, cr, cc))
}

// 外側の既知量（basis=All: input+output+cache_read+cache_creation）。
// A1=370 A2=374 B1=89 B2=90 C1=100 C2=110。A は Celeris の外で走るので run 内訳は無い。
fn standard_outer() -> Outer {
    Arc::new(|c, rep| {
        let t = match (c, rep) {
            (ConfigId::A, 1) => tu(10, 20, 300, 40),
            (ConfigId::A, _) => tu(11, 21, 301, 41),
            (ConfigId::B, 1) => tu(5, 6, 70, 8),
            (ConfigId::B, _) => tu(6, 6, 70, 8),
            (ConfigId::C, 1) => tu(10, 10, 70, 10),
            (ConfigId::C, _) => tu(10, 10, 80, 10),
        };
        // 失敗・timeout の試行も消費は数える。
        let o = match (c, rep) {
            (ConfigId::A, 2) => Outcome::Timeout,
            _ => Outcome::Failed,
        };
        (o, t)
    })
}

struct Fixture {
    _tmp: tempfile::TempDir,
    m: CampaignManifest,
    out: std::path::PathBuf,
    hash: String,
}

fn fixture(outer: Outer) -> Fixture {
    let tmp = tempfile::tempdir().expect("tmp");
    let i = input();
    let m = campaign(&i);
    m.validate().expect("valid");
    let out = tmp.path().join("out");
    run(
        &m,
        &i,
        &Table(outer),
        &RunOptions {
            global_ledger: None,
            out: out.clone(),
            ..RunOptions::default()
        },
    )
    .expect("run");
    let hash = m.compute_sha256().expect("hash");
    Fixture {
        _tmp: tmp,
        m,
        out,
        hash,
    }
}

fn proposal(
    f: &Fixture,
    id: &str,
    arm: &str,
    rep: u32,
    task: &str,
    classifier: Option<&str>,
) -> DecisionLogRun {
    let mut r = DecisionLogRun::default();
    let p = &mut r.proposal;
    p.run_id = id.into();
    p.campaign_id = Some("c1".into());
    p.arm = Some(arm.into());
    p.case_id = Some("p0".into());
    p.repetition = Some(rep);
    p.manifest_hash = Some(f.hash.clone());
    p.task_id = Some(task.into());
    p.decision_id = Some(format!("d-{task}"));
    p.classifier = classifier.map(|k| ClassifierRecord {
        kind: k.into(),
        model: None,
        snapshot: None,
        confidence: None,
        probs: None,
        fallback: None,
    });
    r
}

fn run_usage(id: &str, role: UsageRole, retry: bool, t: (u64, u64, u64, u64)) -> DecisionLogUsage {
    let mut u = DecisionLogUsage::new(
        UsageKey::Run {
            run_id: id.into(),
            usage_source: "worker_finished".into(),
        },
        role,
    );
    u.run_id = Some(id.into());
    u.campaign_id = Some("c1".into());
    u.is_retry = retry;
    u.input_tokens = Some(t.0);
    u.output_tokens = Some(t.1);
    u.cache_read_tokens = Some(t.2);
    u.cache_creation_tokens = Some(t.3);
    u
}

fn jev(task: &str, attempt: u32, t: Option<(u64, u64)>) -> DecisionLogUsage {
    let mut u = DecisionLogUsage::jev_call(task, attempt, task, Some("c1"), t.map(|_| 0.01));
    if let Some((i, o)) = t {
        u.input_tokens = Some(i);
        u.output_tokens = Some(o);
        u.cache_read_tokens = Some(0);
        u.cache_creation_tokens = Some(0);
    }
    u
}

struct Logs {
    runs: Vec<DecisionLogRun>,
    usage: Vec<DecisionLogUsage>,
}

/// B1: worker 49 + retry worker 13 + planner 13 + reviewer 14 = 89（外側と一致）。B2 は内訳の無い旧記録。
/// C1: worker 60 + reviewer 40 = 100、C2: worker 70 + reviewer 40 = 110（C は Jev 分類。Jev 各 5 tokens）。
fn standard_logs(f: &Fixture) -> Logs {
    let runs = vec![
        proposal(f, "b1-w", "B", 1, "tb1", None),
        {
            let mut r = proposal(f, "b1-wr", "B", 1, "tb1", None);
            r.proposal.parent_run_id = Some("b1-w".into());
            r
        },
        proposal(f, "b1-p", "B", 1, "tb1", None),
        proposal(f, "b1-r", "B", 1, "tb1", None),
        proposal(f, "c1-w", "C", 1, "tc1", Some("jev")),
        proposal(f, "c1-r", "C", 1, "tc1", Some("jev")),
        proposal(f, "c2-w", "C", 2, "tc2", Some("jev")),
        proposal(f, "c2-r", "C", 2, "tc2", Some("jev")),
    ];
    let usage = vec![
        run_usage("b1-w", UsageRole::Worker, false, (2, 3, 40, 4)),
        run_usage("b1-wr", UsageRole::Worker, true, (1, 1, 10, 1)),
        run_usage("b1-p", UsageRole::Planner, false, (1, 1, 10, 1)),
        run_usage("b1-r", UsageRole::Reviewer, false, (1, 1, 10, 2)),
        run_usage("c1-w", UsageRole::Worker, false, (6, 4, 40, 10)),
        run_usage("c1-r", UsageRole::Reviewer, false, (4, 6, 30, 0)),
        run_usage("c2-w", UsageRole::Worker, false, (6, 4, 50, 10)),
        run_usage("c2-r", UsageRole::Reviewer, false, (4, 6, 30, 0)),
        jev("tc1", 1, Some((2, 3))),
        jev("tc2", 1, Some((2, 3))),
    ];
    Logs { runs, usage }
}

fn records(f: &Fixture, l: &Logs) -> String {
    let p = ExportParams {
        campaign_id: "c1".into(),
        manifest_hash: Some(f.hash.clone()),
        export_cursor: None,
    };
    build_campaign_export(&p, &l.runs, &l.usage, &[]).records_jsonl()
}

fn acct(f: &Fixture, jsonl: &str, cos: Option<&str>) -> Value {
    compute(
        &f.m,
        &f.out,
        &FinalInputs {
            records_jsonl: jsonl,
            cos_not_called_basis: cos,
            quota: None,
        },
    )
    .expect("compute")
}

fn gap_codes(a: &Value) -> Vec<String> {
    a["gaps"]
        .as_array()
        .expect("gaps")
        .iter()
        .map(|g| g["code"].as_str().unwrap_or("").to_string())
        .collect()
}

fn cell<'a>(a: &'a Value, arm: &str, rep: u64) -> &'a Value {
    a["trials"]["cells"]
        .as_array()
        .expect("cells")
        .iter()
        .find(|c| c["arm"] == arm && c["repetition"] == rep)
        .expect("cell")
}

const OUTER_SUM: u64 = 370 + 374 + 89 + 90 + 100 + 110;

#[test]
fn known_quantities_are_recomputed_from_the_final_output() {
    let f = fixture(standard_outer());
    let a = acct(&f, &records(&f, &standard_logs(&f)), Some(COS_BASIS));
    assert_eq!(recheck(&a), Vec::<String>::new());
    // 外側の合計と decision-log の run を加算せず、Jev（call ごと 5）だけを足す。cache 込みの定義。
    assert_eq!(a["token_basis"], "all");
    assert_eq!(a["trials"]["outer"]["basis_tokens"]["sum"], OUTER_SUM);
    assert_eq!(a["observed_sum"]["tokens"], OUTER_SUM + 10);
    assert_eq!(a["total"]["tokens"], OUTER_SUM + 10);
    assert_eq!(a["total"]["complete"]["tokens"], true);
    assert_eq!(a["reduction_confirmable"]["tokens"], true);
    // cache_read / cache_creation は内訳として別に見える。
    assert_eq!(
        a["trials"]["outer"]["cache_read_tokens"]["sum"],
        300 + 301 + 70 + 70 + 70 + 80
    );
    assert_eq!(
        a["trials"]["outer"]["cache_creation_tokens"]["sum"],
        40 + 41 + 8 + 8 + 10 + 10
    );
    // 役割別は各 run 1 回: worker = 49+13+60+70、planner 13、reviewer = 14+40+40。
    assert_eq!(
        a["by_role"]["worker"]["basis_tokens"]["sum"],
        49 + 13 + 60 + 70
    );
    assert_eq!(a["by_role"]["planner"]["basis_tokens"]["sum"], 13);
    assert_eq!(
        a["by_role"]["reviewer"]["basis_tokens"]["sum"],
        14 + 40 + 40
    );
    // retry は加算しない別の切り口（同じ 13 を by_role に 1 回だけ持つ）。
    assert_eq!(a["by_retry"]["additive"], false);
    assert_eq!(a["by_retry"]["retry"]["basis_tokens"]["sum"], 13);
    assert_eq!(
        a["by_retry"]["initial"]["basis_tokens"]["sum"],
        49 + 13 + 14 + 60 + 70 + 40 + 40
    );
    // run 内訳の無い旧記録（A 両反復と B2）は外側の値のまま、推測で分解しない。
    assert_eq!(a["undecomposed"]["cells"], 3);
    assert_eq!(a["undecomposed"]["tokens"]["sum"], 370 + 374 + 90);
    for (arm, rep, state) in [
        ("A", 1, "no_breakdown"),
        ("B", 1, "match"),
        ("B", 2, "no_breakdown"),
        ("C", 2, "match"),
    ] {
        assert_eq!(cell(&a, arm, rep)["reconcile"], state, "{arm}{rep}");
    }
    // 内訳が無いので定価換算・実支出は完全と言えない（欠測。0 にしない）。
    assert_eq!(a["total"]["complete"]["list_cost_usd"], false);
    assert_eq!(a["total"]["list_cost_micro_usd"], Value::Null);
    assert_eq!(a["total"]["complete"]["cash_cost_usd"], false);
    // 外側が報告した費用は別項目（定価換算・実支出に混ぜない）。
    assert_eq!(
        a["trials"]["executor_reported_cost_micro_usd"]["sum"],
        6 * 7
    );
    // 呼ばれなかった 0 には根拠が付く。
    assert_eq!(
        cell(&a, "A", 1)["jev"]["basis"],
        "arm_without_celeris_routing"
    );
    assert_eq!(cell(&a, "B", 1)["jev"]["basis"], "fixed_routing");
    assert_eq!(cell(&a, "C", 1)["jev"]["state"], "observed");
    assert_eq!(a["shared_unattributable"]["cos"]["state"], "not_called");
    assert_eq!(a["shared_unattributable"]["cos"]["basis"], COS_BASIS);
    // quota は別項目で、消費に混ざらない。
    assert_eq!(a["quota"]["state"], "not_observed");
}

#[test]
fn reruns_reorders_and_reexports_give_the_same_totals_without_double_counting() {
    let f = fixture(standard_outer());
    let base = records(&f, &standard_logs(&f));
    let want = acct(&f, &base, Some(COS_BASIS));
    let mut lines: Vec<&str> = base.lines().collect();
    lines.reverse();
    let reversed = lines.join("\n");
    let doubled = format!("{base}{base}");
    // 外側の再 export（同じ run を別の dedup_key の鍵で 2 回含む形ではなく、同じ行の再投入）。
    let shuffled: String = lines
        .iter()
        .enumerate()
        .flat_map(|(i, l)| [*l, if i % 2 == 0 { *l } else { "" }])
        .collect::<Vec<_>>()
        .join("\n");
    let strip = |mut v: Value| {
        v["integrity"]["duplicates_identical"] = json!(0);
        v
    };
    for (name, text) in [
        ("reversed", &reversed),
        ("doubled", &doubled),
        ("shuffled", &shuffled),
    ] {
        let got = acct(&f, text, Some(COS_BASIS));
        assert_eq!(strip(got.clone()), strip(want.clone()), "{name}");
        assert_eq!(recheck(&got), Vec::<String>::new(), "{name}");
    }
    assert!(
        acct(&f, &doubled, Some(COS_BASIS))["integrity"]["duplicates_identical"].as_u64() > Some(0)
    );
    // 二重に読んでも Jev・run は増えない。
    assert_eq!(
        acct(&f, &doubled, Some(COS_BASIS))["observed_sum"]["tokens"],
        OUTER_SUM + 10
    );
}

#[test]
fn missing_jev_is_a_gap_and_blocks_reduction_but_a_real_zero_is_distinct() {
    let f = fixture(standard_outer());
    // C2 の Jev 呼び出しが取れていない（行が無い）: 0 ではなく欠測。
    let mut l = standard_logs(&f);
    l.usage.pop();
    let a = acct(&f, &records(&f, &l), Some(COS_BASIS));
    assert_eq!(recheck(&a), Vec::<String>::new());
    assert_eq!(cell(&a, "C", 2)["jev"]["state"], "unretrieved");
    assert!(gap_codes(&a).contains(&"jev_unretrieved".to_string()));
    assert_eq!(a["total"]["complete"]["tokens"], false);
    assert_eq!(a["total"]["tokens"], Value::Null);
    assert_eq!(a["reduction_confirmable"]["tokens"], false);
    // 観測和は出るが総費用とは呼ばない。
    assert_eq!(a["observed_sum"]["tokens"], OUTER_SUM + 5);
    // 行はあるが全欄が欠測の Jev も欠測。
    let mut l = standard_logs(&f);
    l.usage.pop();
    l.usage.push(jev("tc2", 1, None));
    let a = acct(&f, &records(&f, &l), Some(COS_BASIS));
    assert_eq!(cell(&a, "C", 2)["jev"]["state"], "missing");
    assert_eq!(a["reduction_confirmable"]["tokens"], false);
    // 分類器の情報が全く無い C の cell は未呼出しとは言えない（根拠なし = 欠測）。
    let mut l = standard_logs(&f);
    l.usage.truncate(8);
    l.runs.truncate(6);
    l.usage
        .retain(|u| u.run_id.as_deref() != Some("c2-w") && u.run_id.as_deref() != Some("c2-r"));
    let a = acct(&f, &records(&f, &l), Some(COS_BASIS));
    assert_eq!(cell(&a, "C", 2)["jev"]["state"], "unknown");
    assert_eq!(cell(&a, "C", 2)["reconcile"], "no_breakdown");
    // 分類器が rule（Jev を呼ばない）なら根拠付きの 0。
    let mut l = standard_logs(&f);
    for r in &mut l.runs {
        if r.proposal.arm.as_deref() == Some("C") {
            r.proposal.classifier.as_mut().expect("classifier").kind = "rule".into();
        }
    }
    l.usage.retain(|u| u.role != UsageRole::Jev);
    let a = acct(&f, &records(&f, &l), Some(COS_BASIS));
    assert_eq!(cell(&a, "C", 1)["jev"]["state"], "not_called");
    assert_eq!(cell(&a, "C", 1)["jev"]["basis"], "classifier_rule");
    assert_eq!(a["reduction_confirmable"]["tokens"], true);
}

#[test]
fn cos_is_shared_never_added_and_unretrieved_cos_blocks_reduction() {
    let f = fixture(standard_outer());
    let base = standard_logs(&f);
    let a = acct(&f, &records(&f, &base), None);
    assert!(gap_codes(&a).contains(&"cos_not_retrieved".to_string()));
    assert_eq!(a["reduction_confirmable"]["tokens"], false);
    // CoS の call は共有として別枠。総和には足さず、call 1 回だけ数える。
    let mut l = standard_logs(&f);
    let mut cos = DecisionLogUsage::cos_turn("s1", 1, Some("c1"));
    cos.input_tokens = Some(900);
    cos.output_tokens = Some(90);
    cos.cache_read_tokens = Some(9);
    cos.cache_creation_tokens = Some(1);
    l.usage.push(cos.clone());
    l.usage.push(cos);
    let a = acct(&f, &records(&f, &l), None);
    assert_eq!(a["shared_unattributable"]["added_to_total"], false);
    assert_eq!(
        a["shared_unattributable"]["rows"]["basis_tokens"]["sum"],
        1000
    );
    assert_eq!(a["shared_unattributable"]["rows"]["n_rows"], 1);
    assert_eq!(a["shared_unattributable"]["cos"]["state"], "observed");
    assert_eq!(a["observed_sum"]["tokens"], OUTER_SUM + 10);
    assert_eq!(a["total"]["tokens"], OUTER_SUM + 10);
    assert_eq!(recheck(&a), Vec::<String>::new());
    // CoS の行はあるが全欄が欠測。
    let mut l = standard_logs(&f);
    l.usage
        .push(DecisionLogUsage::cos_turn("s1", 1, Some("c1")));
    let a = acct(&f, &records(&f, &l), Some(COS_BASIS));
    assert_eq!(a["shared_unattributable"]["cos"]["state"], "missing");
    assert_eq!(a["reduction_confirmable"]["tokens"], false);
}

#[test]
fn other_campaign_hash_repetition_and_plan_are_not_mixed() {
    let f = fixture(standard_outer());
    let base = records(&f, &standard_logs(&f));
    let want = acct(&f, &base, Some(COS_BASIS));
    // 別 campaign の行、別 hash の行は採らない（件数だけ残す）。
    let foreign = |campaign: &str, hash: &str, key: &str| {
        json!({"record_type":"usage","schema":"celeris.decision-log/1","dedup_key":key,
            "campaign_id":campaign,"manifest_hash":hash,"role":"worker","run_id":"b1-w",
            "input_tokens":999_999,"output_tokens":1,"cache_read_tokens":1,"cache_creation_tokens":1,
            "arm":"B","case_id":"p0","repetition":1}).to_string()
    };
    let mixed = format!(
        "{base}{}\n{}\n",
        foreign("c2", &f.hash, "run:b1-w:other"),
        foreign("c1", "deadbeef", "run:b1-w:stale")
    );
    let got = acct(&f, &mixed, Some(COS_BASIS));
    assert_eq!(got["integrity"]["excluded_other_campaign"], 1);
    assert_eq!(got["integrity"]["excluded_manifest_mismatch"], 1);
    assert_eq!(got["observed_sum"], want["observed_sum"]);
    assert_eq!(got["by_role"], want["by_role"]);
    assert_eq!(got["reduction_confirmable"], want["reduction_confirmable"]);
    // 別反復の run は自分の cell にだけ入る（rep1 の消費は rep2 に混ざらない）。
    assert_eq!(cell(&got, "B", 1)["decision_log_tokens"], 89);
    assert_eq!(cell(&got, "C", 1)["decision_log_tokens"], 100);
    assert_eq!(cell(&got, "C", 2)["decision_log_tokens"], 110);
    // 計画外の反復（rep 3）や識別が食い違う消費行は結合せず、gap にする。
    let mut l = standard_logs(&f);
    l.runs.push(proposal(&f, "b3-w", "B", 3, "tb3", None));
    l.usage
        .push(run_usage("b3-w", UsageRole::Worker, false, (500, 0, 0, 0)));
    let a = acct(&f, &records(&f, &l), Some(COS_BASIS));
    assert!(gap_codes(&a).contains(&"runs_outside_plan".to_string()));
    assert_eq!(a["observed_sum"]["tokens"], OUTER_SUM + 10);
    assert_eq!(a["reduction_confirmable"]["tokens"], false);
    // 識別（arm）が自分の run と食い違う消費行は数えず、gap にする。
    let wrong_arm = json!({"record_type":"usage","schema":"celeris.decision-log/1","dedup_key":"run:b1-w:wrong",
        "campaign_id":"c1","manifest_hash":f.hash,"role":"worker","run_id":"b1-w",
        "input_tokens":1,"output_tokens":1,"cache_read_tokens":1,"cache_creation_tokens":1,
        "arm":"C","case_id":"p0","repetition":1}).to_string();
    let a = acct(&f, &format!("{base}{wrong_arm}\n"), Some(COS_BASIS));
    assert_eq!(
        a["integrity"]["identity_conflicts"],
        json!(["run:b1-w:wrong"])
    );
    assert_eq!(a["observed_sum"], want["observed_sum"]);
    assert_eq!(a["reduction_confirmable"]["tokens"], false);
}

#[test]
fn conflicting_duplicates_are_excluded_in_any_order_and_block_the_total() {
    let f = fixture(standard_outer());
    let l = standard_logs(&f);
    let base = records(&f, &l);
    let row = |tokens: u64| {
        let mut u = run_usage("c1-w", UsageRole::Worker, false, (tokens, 4, 40, 10));
        u.key = UsageKey::Run {
            run_id: "c1-w".into(),
            usage_source: "worker_finished".into(),
        };
        u
    };
    // 同じ dedup_key で内容が違う 2 行（writer の主キーでは起きないが、外部入力の防御）。
    let make = |a: u64, b: u64| {
        let mut x = standard_logs(&f);
        x.usage.retain(|u| u.run_id.as_deref() != Some("c1-w"));
        let p = ExportParams {
            campaign_id: "c1".into(),
            manifest_hash: Some(f.hash.clone()),
            export_cursor: None,
        };
        let e = build_campaign_export(&p, &x.runs, &x.usage, &[]).records_jsonl();
        let one = |t| {
            let mut v = build_campaign_export(&p, &x.runs, &[row(t)], &[]);
            v.records.retain(|r| r["record_type"] == "usage");
            v.records
                .iter()
                .map(|r| format!("{r}\n"))
                .collect::<String>()
        };
        format!("{e}{}{}", one(a), one(b))
    };
    let ab = acct(&f, &make(6, 7), Some(COS_BASIS));
    let ba = acct(&f, &make(7, 6), Some(COS_BASIS));
    assert_eq!(ab, ba, "the order of conflicting rows must not matter");
    assert!(gap_codes(&ab).contains(&"conflicting_duplicates".to_string()));
    assert_eq!(
        ab["integrity"]["conflicts"],
        json!(["run:c1-w:worker_finished"])
    );
    assert_eq!(ab["total"]["tokens"], Value::Null);
    assert_eq!(ab["reduction_confirmable"]["tokens"], false);
    // 衝突した鍵は数えない（先着で選ばない）ので、C1 の内訳は欠測のまま。
    assert_eq!(cell(&ab, "C", 1)["decision_log_tokens"], Value::Null);
    assert_ne!(base, make(6, 7));
}

#[test]
fn a_disagreement_between_trial_and_runs_is_shown_not_added() {
    let f = fixture(standard_outer());
    let mut l = standard_logs(&f);
    for u in &mut l.usage {
        if u.run_id.as_deref() == Some("b1-w") {
            u.input_tokens = Some(2 + 1000);
        }
    }
    let a = acct(&f, &records(&f, &l), Some(COS_BASIS));
    let c = cell(&a, "B", 1);
    assert_eq!(c["reconcile"], "mismatch");
    assert_eq!(c["trial_tokens"], 89);
    assert_eq!(c["decision_log_tokens"], 89 + 1000);
    // 総和は外側の値のまま（二重にも差し替えにもしない）。確定はしない。
    assert_eq!(a["observed_sum"]["tokens"], OUTER_SUM + 10);
    assert_eq!(a["reduction_confirmable"]["tokens"], false);
    assert!(gap_codes(&a).contains(&"reconcile_mismatch".to_string()));
    // run があるのに消費行が無いときも 0 にしない。
    let mut l = standard_logs(&f);
    l.usage.retain(|u| u.run_id.as_deref() != Some("b1-p"));
    let a = acct(&f, &records(&f, &l), Some(COS_BASIS));
    assert_eq!(cell(&a, "B", 1)["reconcile"], "incomplete");
    assert!(gap_codes(&a).contains(&"run_without_usage".to_string()));
}

#[test]
fn unobserved_tokens_stay_missing_and_budget_view_shows_the_substitution() {
    // C の rep2 の tokens が取れていない（欠測）。実行中の予算は per_case 上限で見積もるが、最終は欠測のまま。
    let outer: Outer = Arc::new(|c, rep| {
        let (o, t) = standard_outer()(c, rep);
        if (c, rep) == (ConfigId::C, 2) {
            (o, None)
        } else {
            (o, t)
        }
    });
    let f = fixture(outer);
    let mut l = standard_logs(&f);
    l.runs
        .retain(|r| r.proposal.repetition != Some(2) || r.proposal.arm.as_deref() != Some("C"));
    l.usage
        .retain(|u| !matches!(u.run_id.as_deref(), Some("c2-w" | "c2-r")));
    l.usage
        .retain(|u| !(u.role == UsageRole::Jev && u.task_id.as_deref() == Some("tc2")));
    let a = acct(&f, &records(&f, &l), Some(COS_BASIS));
    assert_eq!(a["trials"]["outer"]["basis_tokens"]["n_missing"], 1);
    assert_eq!(a["trials"]["outer"]["basis_tokens"]["sum"], OUTER_SUM - 110);
    assert!(gap_codes(&a).contains(&"trial_tokens_missing".to_string()));
    assert_eq!(a["total"]["tokens"], Value::Null);
    assert_eq!(a["reduction_confirmable"]["tokens"], false);
    let b = &a["budget"];
    assert_eq!(b["runtime_view"]["missing_substituted_tokens"], 1_000);
    assert_eq!(b["runtime_view"]["charged_tokens"], OUTER_SUM - 110 + 1_000);
    assert_eq!(b["final_cutoff"]["observed_trial_tokens"], OUTER_SUM - 110);
    assert_eq!(b["final_cutoff"]["missing_trials"], 1);
    assert_eq!(
        b["difference"]["runtime_charged_minus_observed_tokens"],
        1_000
    );
    // 失敗・timeout の試行の消費も外側に数える（A2 は timeout）。
    assert_eq!(cell(&a, "A", 2)["trial_tokens"], 374);
}

#[test]
fn report_cannot_call_a_partial_sum_complete_or_confirm_a_reduction() {
    let f = fixture(standard_outer());
    let mut l = standard_logs(&f);
    l.usage.pop();
    let partial = acct(&f, &records(&f, &l), Some(COS_BASIS));
    let sum = partial["observed_sum"]["tokens"].as_u64().expect("sum");
    let honest = format!(
        "consumption metric=tokens total={sum} scope=observed_partial\nreduction metric=tokens confirmed=false\n"
    );
    assert_eq!(
        check_consumption_claims(&partial, &honest),
        Vec::<String>::new()
    );
    let v = check_consumption_claims(
        &partial,
        &format!("consumption metric=tokens total={sum} scope=complete\n"),
    );
    assert!(v.iter().any(|x| x.contains("called complete")), "{v:?}");
    let v = check_consumption_claims(&partial, "reduction metric=tokens confirmed=true\n");
    assert!(v.iter().any(|x| x.contains("cannot be confirmed")), "{v:?}");
    let v = check_consumption_claims(
        &partial,
        "consumption metric=tokens total=1 scope=observed_partial\n",
    );
    assert!(v.iter().any(|x| x.contains("observed sum")), "{v:?}");
    // 完全なら完全と書ける。
    let full = acct(&f, &records(&f, &standard_logs(&f)), Some(COS_BASIS));
    let t = full["total"]["tokens"].as_u64().expect("total");
    assert_eq!(
        check_consumption_claims(
            &full,
            &format!(
                "consumption metric=tokens total={t} scope=complete\nreduction metric=tokens confirmed=true\n"
            )
        ),
        Vec::<String>::new()
    );
    // 定価換算は内訳が無いので完全とは書けない。
    let v = check_consumption_claims(
        &full,
        "consumption metric=list_cost_usd total=0 scope=complete\n",
    );
    assert!(!v.is_empty());
}

#[test]
fn recheck_catches_a_tampered_final_output() {
    let f = fixture(standard_outer());
    let mut l = standard_logs(&f);
    l.usage.pop();
    let mut a = acct(&f, &records(&f, &l), Some(COS_BASIS));
    assert_eq!(recheck(&a), Vec::<String>::new());
    // 欠測があるのに完全・削減確定と書き換えた出力を検出する。
    a["total"]["complete"]["tokens"] = json!(true);
    a["total"]["tokens"] = a["observed_sum"]["tokens"].clone();
    a["reduction_confirmable"]["tokens"] = json!(true);
    let v = recheck(&a);
    assert!(
        v.iter().any(|x| x.contains("disagrees with the gaps")),
        "{v:?}"
    );
    // retry を加算して数え直した出力を検出する。
    let mut b = acct(&f, &records(&f, &standard_logs(&f)), Some(COS_BASIS));
    b["by_retry"]["retry"]["basis_tokens"]["sum"] = json!(26);
    assert!(recheck(&b).iter().any(|x| x.contains("by_retry")));
}

#[test]
fn cli_export_writes_the_final_accounting_and_check_report_rejects_overclaims() {
    let f = fixture(standard_outer());
    let mut l = standard_logs(&f);
    l.usage.pop();
    let dir = f.out.parent().expect("dir");
    let recs = dir.join("records.jsonl");
    std::fs::write(&recs, records(&f, &l)).expect("write");
    let bin = std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin");
    let base = |sub: &str| {
        let mut c = std::process::Command::new(&bin);
        c.args([sub, "--manifest"])
            .arg(f.out.join("campaign.json"))
            .arg("--out")
            .arg(&f.out);
        c.arg("--records")
            .arg(&recs)
            .args(["--cos-not-called", COS_BASIS]);
        c
    };
    let o = base("final-accounting").output().expect("run");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let a: Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(a["reduction_confirmable"]["tokens"], false);
    let dest = dir.join("dest");
    let o = base("export")
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("run");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let t: Value = serde_json::from_slice(&o.stdout).expect("table");
    assert_eq!(t["consumption"]["scope"]["tokens"], "observed_partial");
    assert!(Path::new(&dest.join("final-accounting.json")).is_file());
    // 欠測のある指標を完全と書いた報告は check-report が落とす。
    let rep = dir.join("report.md");
    let body = |scope: &str| {
        let mut s = String::new();
        for c in ["A", "B", "C"] {
            s += &format!("result {c} passed=0 n=2 split=dev\n");
        }
        s + &format!(
            "consumption metric=tokens total={} scope={scope}\n",
            a["observed_sum"]["tokens"]
        )
    };
    std::fs::write(&rep, body("complete")).expect("write");
    let o = base("check-report")
        .arg("--report")
        .arg(&rep)
        .output()
        .expect("run");
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("called complete"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}
