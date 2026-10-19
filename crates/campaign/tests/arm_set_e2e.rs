//! 実験セット（A/B のみ・B/C のみ）を実 CLI（validate→dry-run→env-lock→run→check→export）と
//! 通常 POST を受ける所有 stub で通す統合試験。実 LLM・外部 network は使わない。
//! ADR 2026-10-19-experiment-sets D7。実 pilot・最終 preflight は対象外（統合 01M58VG5C4XGK4E70M6WJ6MEX0）。
mod common;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Arc, Mutex},
};

use celeris_campaign::{
    celeris_exec::{CLUSTER_CANCEL_FILE, CelerisExecutor, HttpResponse, Transport},
    checker::check,
    executor::{ExecStatus, Executor, MockExecutor, MockStep, TaskRef, TrialRequest},
    global_ledger::{GLOBAL_LEDGER_SCHEMA_V1, GlobalLedger, LegacyRow},
    lock::CampaignLock,
    manifest::{
        ArmRouting, Budget, CampaignManifest, Cap, Config, ConfigId, ExperimentSet, InputRef,
        Secondary, SetFixed, Share, SnapshotRef, Split, Termination, manifest_hash,
    },
    record::*,
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Value, json};

fn input(n: usize) -> Input {
    let problems: Vec<Value> = (0..n)
        .map(|i| json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

fn set(arms: &[ConfigId], compare: (ConfigId, ConfigId)) -> ExperimentSet {
    ExperimentSet {
        arms: arms.to_vec(),
        compare: vec![compare],
        fixed: Default::default(),
    }
}

/// `which` = "ab"（A/B のみ）か "bc"（B/C のみ）。測定 ID は which。
fn campaign(i: &Input, which: &str, n: usize, max_tokens: u64) -> CampaignManifest {
    let cfg = |id, h: &str, model: &str| Config {
        id,
        harness: h.into(),
        harness_version: "1".into(),
        model: model.into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        routing: None,
    };
    let (arms, pair, problems): (Vec<ConfigId>, _, Vec<String>) = if which == "ab" {
        (
            vec![ConfigId::A, ConfigId::B],
            (ConfigId::A, ConfigId::B),
            (0..n).map(|k| format!("p{k}")).collect(),
        )
    } else {
        (
            vec![ConfigId::B, ConfigId::C],
            (ConfigId::B, ConfigId::C),
            vec!["p0".to_string(), "p1".to_string()],
        )
    };
    let configs = arms
        .iter()
        .map(|a| match a {
            ConfigId::A => cfg(ConfigId::A, "claude-code", "sonnet"),
            ConfigId::B => cfg(ConfigId::B, "celeris", "sonnet"),
            ConfigId::C => cfg(ConfigId::C, "celeris", "sonnet"),
        })
        .collect();
    CampaignManifest {
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: which.into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: problems.clone(),
        configs,
        experiment_sets: BTreeMap::from([(which.to_string(), set(&arms, pair))]),
        selected_sets: vec![which.to_string()],
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset: if which == "bc" { problems } else { Vec::new() },
        },
        budget: Budget {
            token_basis: celeris_campaign::manifest::TokenBasis::InputOutput,
            max_tokens,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: 4,
            per_case: Cap {
                max_tokens: 100_000,
                max_wall_secs: 1_800,
            },
            per_arm: Cap {
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
        sha256: String::new(),
    }
}

fn opts(out: &Path, global: Option<&Path>) -> RunOptions {
    RunOptions {
        out: out.to_path_buf(),
        global_ledger: global.map(Path::to_path_buf),
        ..RunOptions::default()
    }
}

fn arms_of(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|c| c["config"].as_str().expect("config").to_string())
        .collect()
}

// ---- 実 CLI ----

fn bin(sub: &str, m: &Path, extra: &[&Path], flags: &[&str]) -> Output {
    let mut c = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"));
    c.arg(sub).arg("--manifest").arg(m);
    for (k, p) in flags.iter().zip(extra) {
        c.arg(k).arg(p);
    }
    // 認証・A の program を環境から与えない（不要な要求が出れば失敗する）。
    c.env_remove("ANTHROPIC_API_KEY")
        .env_remove("CELERIS_CAMPAIGN_TOKEN")
        .env("PATH", "/nonexistent");
    c.output().expect("run binary")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn write_manifest(dir: &Path, i: &Input, m: &mut CampaignManifest, name: &str) -> PathBuf {
    m.sha256 = String::new();
    m.sha256 = m.compute_sha256().expect("hash");
    fs::write(dir.join("input.json"), serde_json::to_vec(i).expect("ser")).expect("input");
    let p = dir.join(name);
    fs::write(&p, serde_json::to_vec_pretty(m).expect("ser")).expect("manifest");
    p
}

/// CLI の通し。`n_trials` = 期待試行数、`absent` = 出てはいけない arm。
fn cli_flow(which: &str, n_problems: usize, n_trials: u64, absent: &str) {
    let d = tempfile::tempdir().expect("dir");
    let i = input(n_problems.max(2));
    let mut m = campaign(&i, which, n_problems, 10_000_000);
    let mp = write_manifest(d.path(), &i, &mut m, "campaign.json");
    let (out, ex, g) = (
        d.path().join("out"),
        d.path().join("ex"),
        d.path().join("g"),
    );
    let o = bin("validate", &mp, &[], &[]);
    assert!(o.status.success(), "{}", text(&o));
    let o = bin("dry-run", &mp, &[], &[]);
    assert!(o.status.success(), "{}", text(&o));
    let dr: Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(dr["resources"]["total_trials"], n_trials);
    assert_eq!(dr["llm_calls_made"], 0);
    assert!(!dr.to_string().contains(&format!("\"config\":\"{absent}\"")));
    // env-lock: B/C のみは A の program（claude）を探さない。PATH が空でも A の解決失敗では止まらず、
    // 次の入力（daemon の catalog）の欠落で止まる。A/B は A の program を要求する。
    let o = bin("env-lock", &mp, &[], &[]);
    assert!(!o.status.success());
    let t = text(&o);
    if which == "bc" {
        assert!(t.contains("--api-url") && !t.contains("claude"), "{t}");
    } else {
        assert!(t.contains("claude"), "{t}");
    }
    // --executor は flags ペアに載らないので別に実行する。
    let mut c = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"));
    c.args(["run", "--executor", "mock", "--manifest"])
        .arg(&mp)
        .arg("--out")
        .arg(&out)
        .arg("--global-ledger")
        .arg(&g)
        .env("PATH", "/nonexistent");
    let o2 = c.output().expect("run");
    assert!(o2.status.success(), "{}", text(&o2));
    let rep: Value = serde_json::from_slice(&o2.stdout).expect("json");
    assert_eq!(rep["executed"], n_trials);
    let trials = fs::read_to_string(out.join(TRIALS_FILE)).expect("trials");
    assert_eq!(trials.lines().count() as u64, n_trials);
    assert!(!trials.contains(&format!("\"config\":\"{absent}\"")));
    let o = bin("check", &mp, &[&out], &["--out"]);
    assert!(o.status.success(), "{}", text(&o));
    common::write_grades(&m, &out, |_| true);
    let o = bin("export", &mp, &[&out, &ex], &["--out", "--dest"]);
    assert!(o.status.success(), "{}", text(&o));
    let t: Value = serde_json::from_slice(&o.stdout).expect("table");
    let cfgs = arms_of(&t["configs"]);
    assert!(!cfgs.contains(&absent.to_string()), "{cfgs:?}");
    assert_eq!(t["comparisons"].as_array().expect("cmp").len(), 1);
    // 全体台帳: 試行数がそのまま入る（分母に未使用 arm なし）。
    let row = GlobalLedger::open(&g)
        .expect("open")
        .current()
        .expect("cur")[which]
        .clone();
    assert_eq!(row.trials, n_trials);
    // 有効 arm を変えた resume（同じ測定 ID・同じ out）は CLI で拒否。
    let mut other = campaign(&i, if which == "ab" { "bc" } else { "ab" }, 2, 10_000_000);
    other.campaign_id = which.into();
    let op = write_manifest(d.path(), &i, &mut other, "other.json");
    let mut c = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"));
    c.args(["run", "--executor", "mock", "--manifest"])
        .arg(&op)
        .arg("--out")
        .arg(&out);
    let o = c.output().expect("run");
    assert!(!o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("resume rejected"), "{}", text(&o));
}

#[test]
fn cli_ab_only_runs_end_to_end_without_c() {
    cli_flow("ab", 3, 6, "C");
}

#[test]
fn cli_bc_only_runs_end_to_end_without_a() {
    // B/C は p0,p1 の 2 case × (B,C)。
    cli_flow("bc", 2, 4, "A");
}

// ---- 通常 POST を受ける所有 stub ----

#[derive(Clone, Default)]
struct Stub(Arc<Mutex<Vec<Value>>>);

impl Transport for Stub {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        let ok = |status, body| Ok(HttpResponse { status, body });
        match (method, path.split('?').next().unwrap_or("")) {
            ("GET", "/api/v1/tasks") => ok(200, json!({"items": []})),
            ("POST", "/api/v1/tasks") => {
                let mut v = self.0.lock().expect("lock");
                v.push(body.expect("body").clone());
                ok(201, json!({"id": format!("01S{:04}", v.len())}))
            }
            _ => ok(404, json!({})),
        }
    }
}

fn cfg_of(arm: ConfigId, routing: Option<ArmRouting>) -> Config {
    Config {
        id: arm,
        harness: "celeris".into(),
        harness_version: "1".into(),
        model: "sonnet".into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        routing,
    }
}

fn post_body(arm: ConfigId, routing: Option<ArmRouting>) -> Value {
    let d = tempfile::tempdir().expect("dir");
    let stub = Stub::default();
    let x = CelerisExecutor::new(
        stub.clone(),
        json!({"acceptance": [{"type": "reviewer"}]})
            .as_object()
            .cloned()
            .expect("o"),
        1800,
        d.path().join(CLUSTER_CANCEL_FILE),
    );
    let c = cfg_of(arm, routing);
    let req = TrialRequest {
        campaign_id: "bc",
        manifest_sha256: "m",
        config: &c,
        problem_id: "p0",
        prompt: "fix",
        trial_no: 1,
        rep: 1,
        workspace: d.path(),
        kb: d.path(),
        timeout_secs: 1800,
        budget: Cap {
            max_tokens: u64::MAX,
            max_wall_secs: u64::MAX,
        },
    };
    let key = format!("campaign-{}", "0123456789abcdef".repeat(4));
    let t: TaskRef = x.submit(&req, &key).expect("submit");
    let posts = stub.0.lock().expect("lock");
    assert_eq!(posts.len(), 1, "{}", t.task_id);
    posts[0].clone()
}

#[test]
fn owned_stub_receives_b_and_c_payloads_and_never_an_a_post() {
    let b = post_body(ConfigId::B, Some(ArmRouting::default()));
    assert_eq!(b["campaign"]["arm"], "B");
    assert_eq!(b["routing_override"]["mode"], "fixed");
    let c = post_body(
        ConfigId::C,
        Some(ArmRouting {
            lower_policy: Some("tiers-v1".into()),
            evidence_table_id: Some("ev1".into()),
            online_update: Some(false),
            ..Default::default()
        }),
    );
    assert_eq!(c["campaign"]["arm"], "C");
    assert_eq!(c["routing_override"]["mode"], "decision_assisted");
    assert_eq!(c["campaign"]["campaign_id"], "bc");
    // A は Celeris へ POST しない（stub に何も届かない）。
    let d = tempfile::tempdir().expect("dir");
    let stub = Stub::default();
    let x = CelerisExecutor::new(
        stub.clone(),
        Default::default(),
        1800,
        d.path().join(CLUSTER_CANCEL_FILE),
    );
    let a = cfg_of(ConfigId::A, None);
    let req = TrialRequest {
        campaign_id: "ab",
        manifest_sha256: "m",
        config: &a,
        problem_id: "p0",
        prompt: "x",
        trial_no: 1,
        rep: 1,
        workspace: d.path(),
        kb: d.path(),
        timeout_secs: 1,
        budget: Cap {
            max_tokens: 1,
            max_wall_secs: 1,
        },
    };
    assert!(x.submit(&req, "campaign-a").is_err());
    assert!(stub.0.lock().expect("lock").is_empty());
}

// ---- 呼出し記録 ----

struct Counting {
    inner: MockExecutor,
    calls: Mutex<BTreeMap<ConfigId, u32>>,
}

impl Executor for Counting {
    fn name(&self) -> &'static str {
        "counting"
    }
    fn simulated(&self) -> bool {
        true
    }
    fn execute(&self, req: &TrialRequest) -> Result<ExecStatus, String> {
        *self
            .calls
            .lock()
            .expect("l")
            .entry(req.config.id)
            .or_default() += 1;
        self.inner.execute(req)
    }
}

#[test]
fn unused_arm_is_never_called_and_costs_match_the_effective_plan() {
    for (which, absent, n) in [("ab", ConfigId::C, 2usize), ("bc", ConfigId::A, 2)] {
        let d = tempfile::tempdir().expect("dir");
        let i = input(2);
        let m = campaign(&i, which, n, 10_000_000);
        let spend = |arm| {
            BTreeMap::from([
                ((arm, "p0".to_string()), MockStep::Spend(1_000)),
                ((arm, "p1".to_string()), MockStep::Spend(1_000)),
            ])
        };
        let first = if which == "ab" {
            ConfigId::A
        } else {
            ConfigId::B
        };
        let x = Counting {
            inner: MockExecutor::scripted(spend(first)),
            calls: Mutex::new(BTreeMap::new()),
        };
        let out = d.path().join("out");
        let rep = run(&m, &i, &x, &opts(&out, None)).expect("run");
        assert_eq!(rep.executed, 4, "{which}");
        let calls = x.calls.lock().expect("l");
        assert!(!calls.contains_key(&absent), "{which}: {calls:?}");
        assert_eq!(calls.values().sum::<u32>(), 4);
        assert!(check(&m, &out).ok);
        let sm: Value =
            serde_json::from_slice(&fs::read(out.join(SUMMARY_FILE)).expect("s")).expect("j");
        assert!(sm["cells"].get(format!("{absent:?}")).is_none());
        assert!(sm.to_string().contains("2000"), "{which}: {sm}");
    }
}

// ---- B1/C1 の候補・tier・harness 不一致と B0 異モデル（manifest 検査） ----

fn bc_routed(models_c: &[&str], policy_c: &str, harnesses_c: &[&str]) -> Result<(), String> {
    let r = |models: &[&str], policy: &str, h: &[&str]| ArmRouting {
        lower_policy: Some(policy.into()),
        models: Some(models.iter().map(|s| s.to_string()).collect()),
        run_harnesses: Some(h.iter().map(|s| s.to_string()).collect()),
        no_evidence_table: Some(true),
        ..Default::default()
    };
    let mut b = cfg_of(ConfigId::B, None);
    b.routing = Some(r(&["sonnet", "opus"], "tiers-v1", &["claude_code"]));
    b.routing.as_mut().expect("r").no_evidence_table = None;
    let mut c = cfg_of(ConfigId::C, None);
    c.routing = Some(r(models_c, policy_c, harnesses_c));
    celeris_campaign::manifest::check_b1_c1_common(&b, &c)
}

#[test]
fn b1_c1_candidate_tier_and_harness_mismatches_are_rejected() {
    bc_routed(&["sonnet", "opus"], "tiers-v1", &["claude_code"]).expect("same set is accepted");
    let e = bc_routed(&["sonnet"], "tiers-v1", &["claude_code"]).expect_err("pinned");
    assert!(e.contains("candidate"), "{e}");
    let e = bc_routed(&["sonnet", "opus"], "tiers-v2", &["claude_code"]).expect_err("tier");
    assert!(e.contains("lower_policy"), "{e}");
    let e = bc_routed(&["sonnet", "opus"], "tiers-v1", &["codex"]).expect_err("harness");
    assert!(e.contains("run_harnesses"), "{e}");
}

// ---- 全体台帳: 上限・予約・retry ----

fn ledger_manifest(
    i: &Input,
    id: &str,
    max_tokens: u64,
    slots: u32,
    trials_cap: Option<u64>,
) -> CampaignManifest {
    ledger_manifest_n(i, id, 2, max_tokens, slots, trials_cap)
}

fn ledger_manifest_n(
    i: &Input,
    id: &str,
    n: usize,
    max_tokens: u64,
    slots: u32,
    trials_cap: Option<u64>,
) -> CampaignManifest {
    let mut m = campaign(i, "ab", n, max_tokens);
    m.campaign_id = id.into();
    m.budget.max_parallel = slots;
    let s = m.experiment_sets.get_mut("ab").expect("set");
    s.fixed.approved_trials = trials_cap;
    m.selected_sets = vec!["ab".into()];
    m
}

#[test]
fn global_ledger_caps_cannot_be_raised_by_a_new_measurement_id() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    let i = input(2);
    // 承認単位: tokens 400k / 試行 6 / 枠 2。最初の測定（試行 4）が通る。
    g.admit_start(&ledger_manifest(&i, "m1", 400_000, 2, Some(6)))
        .expect("m1");
    // 別測定 ID が budget を 10 倍にしても、既存の上限（400k・6 試行・枠 2）に縛られる。
    let e = g
        .admit_start(&ledger_manifest(&i, "m2", 4_000_000, 2, Some(100)))
        .expect_err("trials exceed approved cap (4 reserved + 4 > 6)");
    assert!(
        e.contains("global ledger rejects") && e.contains("trials"),
        "{e}"
    );
    let e = g
        .admit_start(&ledger_manifest_n(&i, "m3", 1, 400_000, 8, Some(6)))
        .expect_err("provider slots cannot grow");
    assert!(e.contains("provider_slots"), "{e}");
    // 上限内（試行 2 が残っている分には足りない 4 試行なので、別に 1 case のセットを見る）は
    // 拒否された測定 ID が予約を残さない。
    assert!(!g.current().expect("cur").contains_key("m2"));
}

#[test]
fn concurrent_admits_reserve_atomically_and_only_one_fits() {
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let path = d.path().to_path_buf();
    let results: Vec<bool> = (0..6)
        .map(|k| {
            let p = path.clone();
            let m = ledger_manifest(&i, &format!("m{k}"), 400_000, 2, Some(4));
            std::thread::spawn(move || {
                GlobalLedger::open(&p)
                    .expect("open")
                    .admit_start(&m)
                    .is_ok()
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().expect("join"))
        .collect();
    assert_eq!(results.iter().filter(|r| **r).count(), 1, "{results:?}");
}

#[test]
fn admit_allocation_survives_an_empty_publish_before_the_first_submit() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    let i = input(2);
    let m1 = ledger_manifest(&i, "m1", 400_000, 2, Some(6));
    g.admit_start(&m1).expect("m1");
    // runner が最初の投入前に出す空の再集計。取り分（試行 4）は消えない。
    g.record(&GlobalLedger::published(&m1, "h", Default::default()))
        .expect("empty publish");
    assert_eq!(g.current().expect("c")["m1"].trials, 4);
    let e = g
        .admit_start(&ledger_manifest(&i, "m2", 400_000, 2, Some(6)))
        .expect_err("4 held + 4 > 6");
    assert!(e.contains("trials"), "{e}");
    // 測定の終わりに取り分を戻すと、確定消費（0）だけが残る。
    g.release_allocation("m1").expect("release");
    g.admit_start(&ledger_manifest(&i, "m2", 400_000, 2, Some(6)))
        .expect("m2 fits after m1 released its allocation");
}

#[test]
fn retry_and_resume_do_not_double_count_and_old_rows_stay_readable() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    let i = input(2);
    let m = ledger_manifest(&i, "m1", 400_000, 2, Some(8));
    g.admit_start(&m).expect("admit");
    g.admit_start(&m).expect("admit again (resume)");
    let r1 = g.current().expect("c")["m1"].clone();
    assert_eq!(r1.trials, 4, "予約は上書き。二重に積まない");
    // 旧形式（cap 欄なし）の行は読めて、無制限として扱う。
    let old = json!({"schema": GLOBAL_LEDGER_SCHEMA_V1, "measurement_id": "old", "manifest_sha256": "x",
        "tokens": 10, "wall_secs": 1, "trials": 1, "provider_slots": 1});
    let _: LegacyRow = serde_json::from_value(old.clone()).expect("old row");
    fs::write(
        d.path()
            .join(celeris_campaign::global_ledger::GLOBAL_LEDGER_FILE),
        format!("{old}\n"),
    )
    .expect("w");
    let cur = g.current().expect("c");
    assert_eq!(cur["old"].caps.trials, u64::MAX);
    let _ = Share::default();
    let _ = (
        SetFixed::default(),
        ExperimentSet {
            arms: vec![],
            compare: vec![],
            fixed: Default::default(),
        },
    );
}

// ---- 共有承認の台帳の結び付け（省略・別 path・別測定 ID・結び付き不明の lock を投入前に拒否） ----

/// 共有承認（`approved_trials` 6）を宣言し、台帳を `ledger` に結び付けた manifest。
fn shared_manifest(i: &Input, id: &str, ledger: &Path, trials_cap: u64) -> CampaignManifest {
    let mut m = ledger_manifest(i, id, 400_000, 2, Some(trials_cap));
    m.experiment_sets
        .get_mut("ab")
        .expect("set")
        .fixed
        .shared_ledger = Some(ledger.to_string_lossy().into_owned());
    m
}

fn counting() -> Counting {
    Counting {
        inner: MockExecutor::seeded(),
        calls: Mutex::new(BTreeMap::new()),
    }
}

fn calls(x: &Counting) -> u32 {
    x.calls.lock().expect("l").values().sum()
}

#[test]
fn run_api_refuses_shared_approval_bypass_before_any_submit() {
    let d = tempfile::tempdir().expect("dir");
    let (g, other) = (d.path().join("g"), d.path().join("other"));
    let i = input(2);
    let m = shared_manifest(&i, "m1", &g, 6);
    m.validate().expect("valid shared manifest");
    let x = counting();
    let out = d.path().join("out");
    // 台帳の省略・別 path（空の新しい台帳）・別測定 ID + 別 path は、台帳にも out にも触れずに拒否。
    let e = run(&m, &i, &x, &opts(&out, None)).expect_err("omitted ledger");
    assert!(
        e.contains("shared approval requires --global-ledger"),
        "{e}"
    );
    let e = run(&m, &i, &x, &opts(&out, Some(&other))).expect_err("other ledger");
    assert!(e.contains("not the declared shared ledger"), "{e}");
    let mut m2 = m.clone();
    m2.campaign_id = "m2".into();
    for p in [None, Some(other.as_path())] {
        assert!(run(&m2, &i, &x, &opts(&out, p)).is_err());
    }
    // 結び付きの不明な lock（共有宣言の無い manifest の lock）も投入前に拒否。
    let mut unbound = m.clone();
    unbound.experiment_sets.get_mut("ab").expect("set").fixed = SetFixed::default();
    let lock = CampaignLock::new("1", "m1", &unbound, vec![]).expect("lock");
    let o = RunOptions {
        lock: Some(lock),
        ..opts(&out, Some(&g))
    };
    let e = run(&m, &i, &x, &o).expect_err("unbound lock");
    assert!(
        e.contains("lock is not bound to this shared approval"),
        "{e}"
    );
    assert_eq!(calls(&x), 0, "拒否時の submit は 0 件");
    assert!(!out.exists() && !other.exists() && !g.exists());
    // 宣言した台帳でだけ走る（末尾 / の違いは同じ dir）。
    let rep = run(&m, &i, &x, &opts(&out, Some(&d.path().join("g/")))).expect("run");
    assert_eq!(rep.executed, 4);
    assert_eq!(calls(&x), 4);
    // 別測定 ID が同じ台帳で manifest の上限を増やしても、承認上限（試行 6 のうち 4 消費）を超える投入は拒否。
    let big = shared_manifest(&i, "m3", &g, 100);
    let e = run(&big, &i, &x, &opts(&d.path().join("out3"), Some(&g))).expect_err("over cap");
    assert!(e.contains("global ledger rejects"), "{e}");
    assert_eq!(calls(&x), 4);
}

#[test]
fn run_api_keeps_the_legacy_non_shared_path() {
    // 共有承認の宣言が無い manifest（旧形式・単独測定）は、台帳の省略も任意の台帳の opt-in も従来どおり。
    let d = tempfile::tempdir().expect("dir");
    let i = input(2);
    let m = campaign(&i, "ab", 2, 10_000_000);
    let x = counting();
    run(&m, &i, &x, &opts(&d.path().join("a"), None)).expect("no ledger");
    run(
        &m,
        &i,
        &x,
        &opts(&d.path().join("b"), Some(&d.path().join("g"))),
    )
    .expect("opt-in");
    assert_eq!(calls(&x), 8);
}

fn cli_run(mp: &Path, out: &Path, ledger: Option<&Path>) -> Output {
    let mut c = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"));
    c.args(["run", "--executor", "mock", "--manifest"])
        .arg(mp)
        .arg("--out")
        .arg(out)
        .env("PATH", "/nonexistent");
    if let Some(g) = ledger {
        c.arg("--global-ledger").arg(g);
    }
    c.output().expect("run")
}

#[test]
fn cli_refuses_shared_approval_bypass_before_any_submit() {
    let d = tempfile::tempdir().expect("dir");
    let (g, other, out) = (
        d.path().join("g"),
        d.path().join("other"),
        d.path().join("out"),
    );
    let i = input(2);
    // 共有承認を宣言して台帳を結び付けない manifest は validate で拒否（非共有 fallback を許さない）。
    let mut loose = ledger_manifest(&i, "m1", 400_000, 2, Some(6));
    let lp = write_manifest(d.path(), &i, &mut loose, "loose.json");
    let o = bin("validate", &lp, &[], &[]);
    assert!(
        !o.status.success() && text(&o).contains("shared_ledger"),
        "{}",
        text(&o)
    );
    let mut m = shared_manifest(&i, "m1", &g, 6);
    let mp = write_manifest(d.path(), &i, &mut m, "m1.json");
    let mut m2 = shared_manifest(&i, "m2", &g, 6);
    let mp2 = write_manifest(d.path(), &i, &mut m2, "m2.json");
    for (p, ledger, why) in [
        (&mp, None, "shared approval requires --global-ledger"),
        (&mp, Some(other.as_path()), "not the declared shared ledger"),
        (&mp2, None, "shared approval requires --global-ledger"),
        (
            &mp2,
            Some(other.as_path()),
            "not the declared shared ledger",
        ),
        (&lp, Some(g.as_path()), "shared_ledger"),
    ] {
        let o = cli_run(p, &out, ledger);
        assert!(
            !o.status.success() && text(&o).contains(why),
            "{}",
            text(&o)
        );
    }
    // 拒否時は out（試行記録）も台帳も作らない = submit 0 件。
    assert!(!out.exists() && !other.exists() && !g.exists());
    let o = cli_run(&mp, &out, Some(&g));
    assert!(o.status.success(), "{}", text(&o));
    let rep: Value = serde_json::from_slice(&o.stdout).expect("json");
    assert_eq!(rep["executed"], 4);
    // 別測定 ID・別 out でも同じ台帳に数えられ、承認上限（試行 6）を超える投入は拒否。
    let o = cli_run(&mp2, &d.path().join("out2"), Some(&g));
    assert!(
        !o.status.success() && text(&o).contains("global ledger rejects"),
        "{}",
        text(&o)
    );
    assert!(!d.path().join("out2").join(TRIALS_FILE).exists());
}
