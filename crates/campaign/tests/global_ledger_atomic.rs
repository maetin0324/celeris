//! 全体台帳の原子的な予約・精算と承認上限（ADR 2026-10-19-experiment-sets D5/D7。契約は global_ledger.rs の module doc）。
//!
//! 構成: 先頭に共通の道具（台帳・要求・子 process の起動）、その後に節ごとの試験。
//! process を跨ぐ試験は `child` で同じ試験 binary を子 process として起動し、子は環境変数
//! `GLA_CHILD` を見て役割（`child_main`）だけを実行して終わる。同期は台帳ファイルと flock などの
//! 出来事で取り、sleep や負荷で交差を待たない。
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
};

use celeris_campaign::{
    executor::{ExecError, ExecResult, ExecStatus, Executor, TokenUsage, TrialRequest},
    global_ledger::{
        Admission, AttemptId, AttemptState, Caps, Consumed, GLOBAL_LEDGER_FILE,
        GLOBAL_LEDGER_SCHEMA, GLOBAL_LEDGER_SCHEMA_V1, GlobalLedger, Published, ReserveRequest,
        Reserved, Terminal, TrialAdmission, Usage,
    },
    ledger::{TaskRef, TrialKey},
    manifest::{
        Budget, CampaignManifest, Cap, Config, ConfigId, InputRef, Secondary, SnapshotRef, Split,
        Termination, TokenBasis, manifest_hash,
    },
    record::{
        REASON_ARM_BUDGET, REASON_CASE_BUDGET, REASON_EXECUTOR_ERROR, TRIALS_FILE, TrialRecord,
    },
    runner::{RunOptions, run},
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Value, json};

// ---- 共通の道具 ----

const CAPS: Caps = Caps {
    tokens: 1_000,
    trials: 10,
    provider_slots: 2,
};

fn id(m: &str, trial: &str, attempt: u32) -> AttemptId {
    AttemptId {
        measurement_id: m.into(),
        trial: trial.into(),
        attempt,
    }
}

fn req(m: &str, trial: &str, attempt: u32, tokens: u64, caps: Caps) -> ReserveRequest {
    ReserveRequest {
        id: id(m, trial, attempt),
        manifest_sha256: format!("sha-{m}"),
        caps,
        tokens,
        provider_slots: 1,
    }
}

fn publish(g: &GlobalLedger, m: &str, tokens: u64, trials: u64, caps: Caps) {
    g.record(&Published {
        measurement_id: m.into(),
        manifest_sha256: format!("sha-{m}"),
        consumed: Consumed {
            tokens,
            wall_secs: 0,
            trials,
        },
        caps,
    })
    .expect("publish");
}

fn lines(dir: &Path) -> usize {
    fs::read_to_string(dir.join(GLOBAL_LEDGER_FILE))
        .map(|t| t.lines().count())
        .unwrap_or(0)
}

fn finished(tokens: u64) -> Terminal {
    Terminal::Finished(Usage {
        tokens,
        wall_secs: 1,
        ran: true,
    })
}

/// 同じ試験 binary を子 process として `role` で起動し、終了を待つ。
fn child(role: &str, dir: &Path) -> std::process::Output {
    child_cmd(role, dir).output().expect("spawn child")
}

fn child_cmd(role: &str, dir: &Path) -> Command {
    let mut c = Command::new(std::env::current_exe().expect("exe"));
    c.args(["--exact", "child_entry", "--nocapture", "--test-threads=1"])
        .env("GLA_CHILD", role)
        .env("GLA_DIR", dir);
    c
}

/// 子 process の入口。`GLA_CHILD` が無ければ何もしない（通常の試験としては空振り）。
#[test]
fn child_entry() {
    let Ok(role) = std::env::var("GLA_CHILD") else {
        return;
    };
    let dir = std::path::PathBuf::from(std::env::var("GLA_DIR").expect("GLA_DIR"));
    child_main(&role, &dir);
}

fn child_main(role: &str, dir: &Path) {
    if let Some(r) = process_roles(role, dir) {
        return r;
    }
    let g = GlobalLedger::open(dir).expect("open");
    match role {
        // 予約を fsync した直後に（投入も精算もせず）落ちる。
        "reserve-then-die" => {
            g.reserve(&req("child", "p0", 1, 300, CAPS))
                .expect("reserve");
            std::process::exit(3);
        }
        other => panic!("unknown child role {other}"),
    }
}

// ---- 確定消費と未精算予約の区別・空 publish ----

#[test]
fn empty_publish_keeps_reservations_and_settled_consumption() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    g.reserve(&req("m1", "p0", 1, 200, CAPS)).expect("r0");
    g.mark_submitted(&id("m1", "p0", 1)).expect("sub");
    g.settle(&id("m1", "p0", 1), finished(120)).expect("settle");
    g.reserve(&req("m1", "p1", 1, 300, CAPS)).expect("r1");
    publish(&g, "m1", 500, 3, CAPS);
    // 最初の投入前と同じ空の再集計。
    publish(&g, "m1", 0, 0, CAPS);
    let v = &g.current().expect("cur")["m1"];
    assert_eq!(v.consumed.tokens, 500, "確定消費は下がらない");
    assert_eq!(v.consumed.trials, 3);
    assert_eq!(v.tokens, 800, "確定 500 + 未精算予約 300");
    assert_eq!(v.provider_slots, 1, "未精算の p1 だけが枠を持つ");
    assert_eq!(v.outstanding, 1);
    let s = g.state().expect("state");
    assert_eq!(s.attempts[&id("m1", "p1", 1)].state, AttemptState::Reserved);
}

// ---- 上限: 古い低 cap・増やせない・拒否で何も残さない ----

#[test]
fn old_low_cap_binds_later_larger_manifest_and_is_persisted_with_the_reservation() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    let low = Caps {
        tokens: 400,
        ..CAPS
    };
    publish(&g, "old", 100, 1, low);
    let big = Caps {
        tokens: 1_000_000,
        trials: 1_000,
        provider_slots: 64,
    };
    let before = lines(d.path());
    let e = g
        .reserve(&req("new", "p0", 1, 301, big))
        .expect_err("100 + 301 > 400");
    assert!(e.contains("tokens") && e.contains("400"), "{e}");
    assert_eq!(lines(d.path()), before, "拒否は何も書かない");
    assert_eq!(
        g.reserve(&req("new", "p0", 1, 300, big)).expect("fits"),
        Reserved::New
    );
    let s = g.state().expect("state");
    assert_eq!(s.caps.tokens, 400);
    assert_eq!(s.attempts[&id("new", "p0", 1)].caps.tokens, 400);
    // 大きい上限で publish しても最小は変わらない。
    publish(&g, "new", 0, 0, big);
    assert_eq!(g.state().expect("state").caps, low.min(CAPS).min(big));
}

#[test]
fn trial_and_provider_slot_caps_are_checked_inside_the_reservation() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    let caps = Caps {
        tokens: 10_000,
        trials: 3,
        provider_slots: 2,
    };
    g.reserve(&req("a", "p0", 1, 10, caps)).expect("a p0");
    g.reserve(&req("b", "p0", 1, 10, caps)).expect("b p0");
    let e = g
        .reserve(&req("b", "p1", 1, 10, caps))
        .expect_err("slot sum 3 > 2");
    assert!(e.contains("provider_slots"), "{e}");
    // 終了を確認した枠だけ解放する。
    g.mark_submitted(&id("a", "p0", 1)).expect("sub");
    g.settle(&id("a", "p0", 1), finished(5)).expect("settle");
    g.reserve(&req("b", "p1", 1, 10, caps)).expect("slot freed");
    g.settle(&id("b", "p1", 1), Terminal::NotSubmitted)
        .expect("never submitted");
    g.settle(&id("b", "p0", 1), finished(5)).expect("b p0 done");
    // 試行数: a/p0・b/p0（動いた）+ 新しい試行 1 = 3。もう 1 つは上限を超える。
    g.reserve(&req("a", "p1", 1, 10, caps))
        .expect("third trial");
    let e = g
        .reserve(&req("b", "p2", 1, 10, caps))
        .expect_err("4 trials > 3");
    assert!(e.contains("trials"), "{e}");
}

// ---- 同じ予約の再実行・冪等精算・retry ----

#[test]
fn same_reservation_rerun_is_idempotent_and_settled_identity_cannot_be_reserved() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    let r = req("m", "p0", 1, 400, CAPS);
    assert_eq!(g.reserve(&r).expect("r"), Reserved::New);
    let n = lines(d.path());
    assert_eq!(
        g.reserve(&r).expect("again"),
        Reserved::Existing(AttemptState::Reserved)
    );
    assert_eq!(lines(d.path()), n, "同じ予約は書かない");
    assert_eq!(g.current().expect("c")["m"].tokens, 400, "二重に積まない");
    g.mark_submitted(&r.id).expect("sub");
    g.mark_submitted(&r.id).expect("sub again");
    assert_eq!(
        g.reserve(&r).expect("resume"),
        Reserved::Existing(AttemptState::Submitted)
    );
    g.settle(&r.id, finished(250)).expect("settle");
    let e = g.reserve(&r).expect_err("settled");
    assert!(e.contains("already settled"), "{e}");
}

#[test]
fn resettlement_replaces_and_other_attempts_accumulate() {
    let d = tempfile::tempdir().expect("dir");
    let g = GlobalLedger::open(d.path()).expect("open");
    let a1 = id("m", "p0", 1);
    g.reserve(&req("m", "p0", 1, 300, CAPS)).expect("r1");
    g.mark_submitted(&a1).expect("sub");
    g.settle(&a1, finished(100)).expect("settle");
    let n = lines(d.path());
    g.settle(&a1, finished(100)).expect("same settle");
    assert_eq!(lines(d.path()), n, "冪等精算は書かない");
    g.settle(&a1, finished(0)).expect("empty resettle");
    assert_eq!(
        g.current().expect("c")["m"].consumed.tokens,
        100,
        "下げない"
    );
    g.settle(&a1, finished(140)).expect("larger resettle");
    assert_eq!(g.current().expect("c")["m"].consumed.tokens, 140, "置換");
    // retry（別 attempt）の消費は合算し、試行数は同じ試行で 1。
    g.reserve(&req("m", "p0", 2, 300, CAPS)).expect("retry");
    g.settle(&id("m", "p0", 2), finished(60))
        .expect("retry done");
    let v = &g.current().expect("c")["m"];
    assert_eq!(v.consumed.tokens, 200);
    assert_eq!(v.consumed.trials, 1);
    // 投入済みの attempt は「投入していない」で解放できない。
    g.reserve(&req("m", "p1", 1, 10, CAPS)).expect("r");
    g.mark_submitted(&id("m", "p1", 1)).expect("sub");
    let e = g
        .settle(&id("m", "p1", 1), Terminal::NotSubmitted)
        .expect_err("submitted");
    assert!(e.contains("not submitted"), "{e}");
    let e = g
        .settle(&id("m", "ghost", 1), finished(1))
        .expect_err("unknown identity");
    assert!(e.contains("unknown attempt"), "{e}");
}

#[test]
fn crash_after_reserve_keeps_the_reservation_held() {
    let d = tempfile::tempdir().expect("dir");
    let o = child("reserve-then-die", d.path());
    assert_eq!(o.status.code(), Some(3), "{o:?}");
    let g = GlobalLedger::open(d.path()).expect("open");
    let v = &g.current().expect("c")["child"];
    assert_eq!((v.tokens, v.provider_slots, v.outstanding), (300, 1, 1));
    // 再開側が投入の無いことを確かめたときだけ解放できる。
    g.settle(&id("child", "p0", 1), Terminal::NotSubmitted)
        .expect("release");
    assert_eq!(g.current().expect("c")["child"].provider_slots, 0);
}

// ---- 旧 /1 行と破損 ----

#[test]
fn legacy_rows_are_read_conservatively() {
    let d = tempfile::tempdir().expect("dir");
    let old = json!({"schema": GLOBAL_LEDGER_SCHEMA_V1, "measurement_id": "old", "manifest_sha256": "x",
        "tokens": 700, "wall_secs": 5, "trials": 4, "provider_slots": 2});
    fs::write(d.path().join(GLOBAL_LEDGER_FILE), format!("{old}\n")).expect("w");
    let g = GlobalLedger::open(d.path()).expect("open");
    let v = &g.current().expect("c")["old"];
    assert_eq!((v.tokens, v.trials, v.provider_slots), (700, 4, 2));
    assert_eq!(
        v.caps,
        Caps::UNLIMITED,
        "cap 欠落は上限を与えない（増やしもしない）"
    );
    // 旧行の枠は解放の記録が無いので占有のまま。
    let e = g
        .reserve(&req("new", "p0", 1, 100, CAPS))
        .expect_err("legacy slots hold the provider");
    assert!(e.contains("provider_slots"), "{e}");
    // 持ち主の小さい再集計で消費は下がらない。終了確認後に枠だけ解放する。
    publish(&g, "old", 10, 1, Caps::UNLIMITED);
    g.confirm_legacy_idle("old").expect("idle");
    let v = &g.current().expect("c")["old"];
    assert_eq!((v.tokens, v.trials, v.provider_slots), (700, 4, 0));
    let e = g
        .reserve(&req("new", "p0", 1, 301, CAPS))
        .expect_err("700 + 301 > 1000");
    assert!(e.contains("tokens"), "{e}");
    g.reserve(&req("new", "p0", 1, 300, CAPS)).expect("fits");
}

#[test]
fn corrupt_unknown_and_torn_ledgers_are_rejected_not_zeroed() {
    let d = tempfile::tempdir().expect("dir");
    let p = d.path().join(GLOBAL_LEDGER_FILE);
    let g = GlobalLedger::open(d.path()).expect("open");
    fs::write(&p, "{not json\n").expect("w");
    assert!(g.state().expect_err("corrupt").contains("corrupt"));
    assert!(g.reserve(&req("m", "p0", 1, 1, CAPS)).is_err());
    fs::write(&p, "{\"schema\":\"celeris.campaign-global-ledger/9\"}\n").expect("w");
    assert!(g.state().expect_err("unknown").contains("unknown"));
    let bad_kind = json!({"schema": GLOBAL_LEDGER_SCHEMA, "kind": "mystery"});
    fs::write(&p, format!("{bad_kind}\n")).expect("w");
    assert!(g.state().expect_err("kind").contains("unknown"));
    // 完結した JSON だが改行で終わっていない（追記の途中で停止）。読めても追記しない。
    let old = json!({"schema": GLOBAL_LEDGER_SCHEMA_V1, "measurement_id": "o", "manifest_sha256": "x",
        "tokens": 1, "wall_secs": 1, "trials": 1, "provider_slots": 0});
    fs::write(&p, old.to_string()).expect("w");
    assert_eq!(g.current().expect("readable")["o"].tokens, 1);
    let e = g
        .reserve(&req("m", "p0", 1, 1, CAPS))
        .expect_err("torn tail");
    assert!(e.contains("torn"), "{e}");
}

// ---- 同時予約（同じ process の別 fd。flock は fd ごと） ----

#[test]
fn concurrent_reservations_never_exceed_the_provider_slots() {
    let d = tempfile::tempdir().expect("dir");
    let path = d.path().to_path_buf();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
    let ok = (0..6)
        .map(|k| {
            let (p, b) = (path.clone(), barrier.clone());
            std::thread::spawn(move || {
                let g = GlobalLedger::open(&p).expect("open");
                b.wait();
                g.reserve(&req(&format!("m{k}"), "p0", 1, 10, CAPS)).is_ok()
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().expect("join"))
        .filter(|ok| *ok)
        .count();
    assert_eq!(ok, CAPS.provider_slots as usize);
    let g = GlobalLedger::open(d.path()).expect("open");
    let s = g.state().expect("state");
    assert_eq!(
        s.attempts
            .values()
            .filter(|a| a.state.outstanding())
            .count(),
        2
    );
}

// ======== 実 process 試験（runner::run + ファイル台帳 + 外部が永続する stub executor） ========
//
// 子 process は `run` の役割で `runner::run` を回す。外部（executor 側）の task はファイルに残るので、
// 子が止まっても再開側の lookup で見える。停止点は stub の hook（環境変数 GLA_HOOK、最初の 1 回だけ発火）:
// process の終了（exit 9）か、全体台帳の末尾の改行を落として次の書き込みを拒否させる（exit 4 = run の Err。
// 親が改行を戻して再開する）。同期は stdin/stdout の明示 barrier（READY → 親の GO）で、sleep は使わない。

fn say(s: &str) {
    println!("{s}");
    std::io::stdout().flush().expect("flush");
}

fn wait_go() {
    let mut l = String::new();
    std::io::stdin().read_line(&mut l).expect("go");
}

const SLOT1: Caps = Caps {
    tokens: 1_000,
    trials: 10,
    provider_slots: 1,
};

#[derive(Clone, Copy)]
struct Cfg {
    par: u32,
    max_tokens: u64,
    per_case: u64,
    per_arm: u64,
    repeats: u32,
    stop_on_budget: bool,
}

impl Default for Cfg {
    fn default() -> Self {
        Self {
            par: 64,
            max_tokens: 10_000_000,
            per_case: 100_000,
            per_arm: 10_000_000_000,
            repeats: 1,
            stop_on_budget: true,
        }
    }
}

impl Cfg {
    fn env(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.par,
            self.max_tokens,
            self.per_case,
            self.per_arm,
            self.repeats,
            self.stop_on_budget
        )
    }
    fn parse(s: &str) -> Self {
        let v: Vec<&str> = s.split(' ').collect();
        Self {
            par: v[0].parse().expect("par"),
            max_tokens: v[1].parse().expect("max"),
            per_case: v[2].parse().expect("case"),
            per_arm: v[3].parse().expect("arm"),
            repeats: v[4].parse().expect("repeats"),
            stop_on_budget: v[5].parse().expect("stop"),
        }
    }
}

fn input() -> Input {
    let p = json!({"id":"p0","task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""});
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":[p],"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

/// p0 の A/B/C。
fn campaign(i: &Input, mid: &str, c: &Cfg) -> CampaignManifest {
    let cfg = |id, h: &str| Config {
        id,
        harness: h.into(),
        harness_version: "1".into(),
        model: "m".into(),
        model_version: "1".into(),
        policy_version: "p1".into(),
        routing: None,
    };
    CampaignManifest {
        variant: None,
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: mid.into(),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: vec!["p0".into()],
        configs: vec![
            cfg(ConfigId::A, "claude-code"),
            cfg(ConfigId::B, "celeris"),
            cfg(ConfigId::C, "celeris"),
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
            token_basis: TokenBasis::InputOutput,
            max_tokens: c.max_tokens,
            max_wall_secs: 1_000_000,
            max_disk_bytes: 1 << 40,
            max_parallel: c.par,
            per_case: Cap {
                max_tokens: c.per_case,
                max_wall_secs: 1_800,
            },
            per_arm: Cap {
                max_tokens: c.per_arm,
                max_wall_secs: 1_000_000,
            },
        },
        termination: Termination {
            trial_timeout_secs: 1800,
            max_attempts_per_trial: 1,
            stop_on_budget: c.stop_on_budget,
        },
        snapshot: SnapshotRef {
            repo_commit: "abc".into(),
            materials_sha256: "d".into(),
            kb_snapshot: "kb@1".into(),
        },
        order: Default::default(),
        seed: 0,
        repeats: c.repeats,
        dependencies: BTreeMap::new(),
        sha256: String::new(),
    }
}

/// 外部が永続する stub executor（非同期。`root/remote/<key>` が外部の task）。
struct Stub {
    root: PathBuf,
    hook: String,
    fired: AtomicBool,
}

impl Stub {
    fn once(&self, name: &str) -> bool {
        self.hook == name && !self.fired.swap(true, Ordering::SeqCst)
    }
    fn on(&self, name: &str) -> bool {
        self.hook == name
    }
    fn task_path(&self, key: &str) -> PathBuf {
        let k: String = key
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        self.root.join("remote").join(k)
    }
    fn tear_global(&self) {
        let p = self.root.join("global").join(GLOBAL_LEDGER_FILE);
        let f = fs::OpenOptions::new().write(true).open(&p).expect("open");
        let n = f.metadata().expect("meta").len();
        f.set_len(n - 1).expect("truncate");
    }
    fn die(&self) -> ! {
        std::process::exit(9)
    }
}

impl Executor for Stub {
    fn name(&self) -> &'static str {
        "file-stub"
    }
    fn simulated(&self) -> bool {
        true
    }
    fn execute(&self, _req: &TrialRequest) -> Result<ExecStatus, String> {
        Err("async only".into())
    }
    fn is_async(&self) -> bool {
        true
    }
    fn routing(&self, _req: &TrialRequest) -> Result<Value, String> {
        if self.once("after-reserve") {
            self.die();
        }
        if self.once("barrier") {
            say("READY");
            wait_go();
        }
        Ok(json!({"mode": "stub"}))
    }
    fn lookup(&self, key: &str) -> Result<Option<TaskRef>, ExecError> {
        if self.once("lookup-lost") {
            return Err(ExecError::interrupted("lookup: connection lost"));
        }
        Ok(fs::read_to_string(self.task_path(key))
            .ok()
            .map(|t| serde_json::from_str(&t).expect("task")))
    }
    fn submit(&self, req: &TrialRequest, key: &str) -> Result<TaskRef, ExecError> {
        let mut log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("submits.log"))
            .expect("log");
        writeln!(log, "{} {}", key, req.budget.max_tokens).expect("log");
        if self.once("submit-refused") {
            return Err(ExecError::refused("refused"));
        }
        if self.once("submit-notsent") {
            return Err(ExecError::not_sent("not sent"));
        }
        let p = self.task_path(key);
        fs::create_dir_all(p.parent().expect("parent")).expect("dir");
        let t = match fs::read_to_string(&p) {
            Ok(t) => serde_json::from_str(&t).expect("task"),
            Err(_) => {
                let n = fs::read_dir(p.parent().expect("parent"))
                    .expect("ls")
                    .count()
                    + 1;
                let t = TaskRef {
                    task_id: format!("task-{n}"),
                    ..TaskRef::default()
                };
                fs::write(&p, serde_json::to_string(&t).expect("json")).expect("write");
                t
            }
        };
        if self.once("submit-unknown") {
            return Err(ExecError::unknown("response lost"));
        }
        if self.once("after-submit") {
            self.die();
        }
        if self.once("torn-after-submit") {
            self.tear_global();
        }
        Ok(t)
    }
    fn usage(&self, _t: &TaskRef) -> Result<Option<celeris_campaign::executor::Usage>, ExecError> {
        Ok(self
            .on("cancel-unconfirmed")
            .then(|| celeris_campaign::executor::Usage {
                tokens: Some(TokenUsage::full(200_000, 0, 0, 0)),
                wall_secs: 10,
            }))
    }
    fn poll(&self, _t: &TaskRef) -> Result<ExecStatus, ExecError> {
        if self.once("poll-lost") {
            return Err(ExecError::interrupted("poll: connection lost"));
        }
        if self.on("cancel-unconfirmed") {
            return Ok(ExecStatus::Wait);
        }
        Ok(ExecStatus::Done(ExecResult {
            outcome: celeris_campaign::aggregate::Outcome::Passed,
            tokens: Some(TokenUsage::full(10_000, 0, 0, 0)),
            cost_micro_usd: Some(100_000),
            wall_secs: 60,
        }))
    }
    fn observed(&self, _t: &TaskRef) -> Result<Vec<celeris_campaign::record::ObservedRun>, String> {
        if self.once("before-record") {
            self.die();
        }
        if self.once("torn-before-settle") {
            self.tear_global();
        }
        Ok(Vec::new())
    }
    fn cancel(&self, t: &TaskRef, _reason: &str) -> Result<(), ExecError> {
        let d = self.root.join("cancels");
        fs::create_dir_all(&d).expect("dir");
        fs::write(d.join(&t.task_id), "").expect("cancel");
        Ok(())
    }
    fn stopped(&self, t: &TaskRef) -> Result<bool, ExecError> {
        if self.on("cancel-unconfirmed") {
            return Ok(false);
        }
        Ok(self.root.join("cancels").join(&t.task_id).exists())
    }
}

/// `GLA_CHILD` が process 試験の役割ならそれを実行して Some。
fn process_roles(role: &str, dir: &Path) -> Option<()> {
    let mid = std::env::var("GLA_MID").unwrap_or_default();
    match role {
        // flock を取らずに予約だけ競う側。READY → 親の GO → reserve を同時に出す。
        "reserve-barrier" => {
            let g = GlobalLedger::open(dir).expect("open");
            say("READY");
            wait_go();
            say(match g.reserve(&req(&mid, "p0", 1, 10, SLOT1)) {
                Ok(_) => "RESULT ok",
                Err(_) => "RESULT err",
            });
        }
        // 予約を fsync し、台帳の flock を握ったまま親に kill される。
        "reserve-hold-lock" => {
            let g = GlobalLedger::open(dir).expect("open");
            g.reserve(&req(&mid, "p0", 1, 300, CAPS)).expect("reserve");
            let lock = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(dir.join(GLOBAL_LEDGER_FILE).with_extension("lock"))
                .expect("lock file");
            lock.lock().expect("flock");
            say("READY");
            wait_go();
        }
        "run" => {
            let hook = std::env::var("GLA_HOOK").unwrap_or_default();
            let cfg = Cfg::parse(&std::env::var("GLA_CFG").expect("cfg"));
            let i = input();
            let m = campaign(&i, &mid, &cfg);
            let stub = Stub {
                root: dir.to_path_buf(),
                hook,
                fired: AtomicBool::new(false),
            };
            let opts = RunOptions {
                out: dir.join(format!("out-{mid}")),
                global_ledger: Some(dir.join("global")),
                ..RunOptions::default()
            };
            match run(&m, &i, &stub, &opts) {
                Ok(rep) => say(&format!("REPORT {rep:?}")),
                Err(e) => {
                    eprintln!("ERR {e}");
                    std::process::exit(4);
                }
            }
        }
        _ => return None,
    }
    Some(())
}

// ---- 親側の道具 ----

struct Proc {
    c: Child,
    out: BufReader<std::process::ChildStdout>,
}

impl Proc {
    fn spawn(mut cmd: Command) -> Self {
        let mut c = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn");
        let out = BufReader::new(c.stdout.take().expect("stdout"));
        Self { c, out }
    }
    fn wait_line(&mut self, want: &str) {
        loop {
            let mut l = String::new();
            assert!(
                self.out.read_line(&mut l).expect("read") > 0,
                "EOF before {want}"
            );
            // libtest が同じ行に「test child_entry ... 」を先に出すので行末で見る。
            if l.trim().ends_with(want) {
                return;
            }
        }
    }
    fn go(&mut self) {
        let i = self.c.stdin.as_mut().expect("stdin");
        i.write_all(b"GO\n").expect("go");
        i.flush().expect("flush");
    }
    /// 残りの出力を読み切って終了を待つ。
    fn finish(mut self) -> (Option<i32>, String) {
        drop(self.c.stdin.take());
        let mut rest = String::new();
        std::io::Read::read_to_string(&mut self.out, &mut rest).expect("rest");
        let st = self.c.wait().expect("wait");
        (st.code(), rest)
    }
}

struct Out {
    code: Option<i32>,
    stdout: String,
}

#[derive(Debug, PartialEq, Eq, Default)]
struct Snap {
    consumed: u64,
    trials: u64,
    outstanding: usize,
    slots: u32,
    held: u64,
}

struct Sc {
    d: tempfile::TempDir,
}

impl Sc {
    fn new() -> Self {
        Self {
            d: tempfile::tempdir().expect("dir"),
        }
    }
    fn root(&self) -> &Path {
        self.d.path()
    }
    fn gdir(&self) -> PathBuf {
        self.root().join("global")
    }
    fn g(&self) -> GlobalLedger {
        GlobalLedger::open(&self.gdir()).expect("global")
    }
    fn cmd(&self, mid: &str, hook: &str, cfg: &Cfg) -> Command {
        let mut c = child_cmd("run", self.root());
        c.env("GLA_MID", mid)
            .env("GLA_HOOK", hook)
            .env("GLA_CFG", cfg.env());
        c
    }
    fn run(&self, mid: &str, hook: &str, cfg: &Cfg) -> Out {
        let o = self.cmd(mid, hook, cfg).output().expect("run child");
        Out {
            code: o.status.code(),
            stdout: format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        }
    }
    fn run_ok(&self, mid: &str, hook: &str, cfg: &Cfg) -> String {
        let o = self.run(mid, hook, cfg);
        assert_eq!(o.code, Some(0), "{}", o.stdout);
        o.stdout
    }
    fn tasks(&self) -> usize {
        fs::read_dir(self.root().join("remote"))
            .map(|d| d.count())
            .unwrap_or(0)
    }
    /// submit の呼び出し（失敗を含む）の (key, 実 request の上限)。
    fn calls(&self) -> Vec<(String, u64)> {
        fs::read_to_string(self.root().join("submits.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let (k, b) = l.rsplit_once(' ').expect("line");
                (k.to_string(), b.parse().expect("budget"))
            })
            .collect()
    }
    fn records(&self, mid: &str) -> Vec<TrialRecord> {
        fs::read_to_string(self.root().join(format!("out-{mid}")).join(TRIALS_FILE))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).expect("record"))
            .collect()
    }
    fn snap(&self, mid: &str) -> Snap {
        match self.g().current().expect("current").get(mid) {
            Some(v) => Snap {
                consumed: v.consumed.tokens,
                trials: v.consumed.trials,
                outstanding: v.outstanding,
                slots: v.provider_slots,
                held: v.tokens,
            },
            None => Snap::default(),
        }
    }
    fn attempts(&self, mid: &str) -> Vec<(u32, AttemptState)> {
        self.g()
            .state()
            .expect("state")
            .attempts
            .values()
            .filter(|a| a.id.measurement_id == mid)
            .map(|a| (a.id.attempt, a.state))
            .collect()
    }
    /// 落ちた書き込みで途中の行（末尾の改行なし）を、人が確かめて直す。
    fn repair_tail(&self) {
        let p = self.gdir().join(GLOBAL_LEDGER_FILE);
        let mut f = fs::OpenOptions::new().append(true).open(p).expect("open");
        f.write_all(b"\n").expect("nl");
    }
    fn key(&self, mid: &str, arm: ConfigId) -> String {
        TrialKey {
            manifest_sha256: self.sha(mid),
            problem_id: "p0".into(),
            arm,
            trial_no: 1,
            rep: 1,
        }
        .idempotency_key()
    }
    fn sha(&self, mid: &str) -> String {
        campaign(&input(), mid, &Cfg::default())
            .compute_sha256()
            .expect("sha")
    }
}

const MID: &str = "a";

// ---- (1) 2 process の同時予約 ----

#[test]
fn two_processes_reserve_at_the_same_time_and_only_one_passes() {
    let d = tempfile::tempdir().expect("dir");
    let mk = |mid: &str| {
        let mut c = child_cmd("reserve-barrier", d.path());
        c.env("GLA_MID", mid);
        Proc::spawn(c)
    };
    let (mut a, mut b) = (mk("x"), mk("y"));
    // 2 つとも予約の直前で待っていることを確かめてから、同時に出す。
    a.wait_line("READY");
    b.wait_line("READY");
    a.go();
    b.go();
    let (ca, oa) = a.finish();
    let (cb, ob) = b.finish();
    assert_eq!((ca, cb), (Some(0), Some(0)));
    let all = format!("{oa}{ob}");
    assert_eq!(all.matches("RESULT ok").count(), 1, "{all}");
    assert_eq!(all.matches("RESULT err").count(), 1, "{all}");
    let g = GlobalLedger::open(d.path()).expect("open");
    let s = g.state().expect("state");
    assert_eq!(s.attempts.len(), 1, "負けた側は何も書かない");
    assert_eq!(lines(d.path()), 1);
    assert_eq!(s.caps.provider_slots, 1);
}

// ---- kill で flock が解けても予約は残り、再精算は一度だけ ----

#[test]
fn killed_while_holding_the_flock_keeps_the_reservation_and_settles_once() {
    let d = tempfile::tempdir().expect("dir");
    let mut c = child_cmd("reserve-hold-lock", d.path());
    c.env("GLA_MID", "dead");
    let mut p = Proc::spawn(c);
    p.wait_line("READY");
    let lock_path = d.path().join(GLOBAL_LEDGER_FILE).with_extension("lock");
    let probe = fs::OpenOptions::new()
        .write(true)
        .open(&lock_path)
        .expect("probe");
    assert!(
        matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        "子が flock を握っている"
    );
    p.c.kill().expect("SIGKILL");
    p.c.wait().expect("reap");
    probe.try_lock().expect("kill で flock は解ける");
    probe.unlock().expect("unlock");
    // 予約は黙って解放されない。他の ID の予約は枠 2 のうち残り 1 を使える。
    let g = GlobalLedger::open(d.path()).expect("open");
    let v = &g.current().expect("c")["dead"];
    assert_eq!((v.tokens, v.provider_slots, v.outstanding), (300, 1, 1));
    g.reserve(&req("live", "p0", 1, 10, CAPS))
        .expect("one slot left");
    assert!(
        g.reserve(&req("live2", "p0", 1, 10, CAPS)).is_err(),
        "dead の枠はまだ占有"
    );
    // 再開側が終了を確かめて精算する。同じ精算の再実行は二重に数えない。
    let dead = id("dead", "p0", 1);
    g.mark_submitted(&dead).expect("sub");
    g.settle(&dead, finished(120)).expect("settle");
    g.settle(&dead, finished(120)).expect("again");
    let v = &g.current().expect("c")["dead"];
    assert_eq!(
        (v.consumed.tokens, v.consumed.trials, v.provider_slots),
        (120, 1, 0)
    );
    g.reserve(&req("live2", "p0", 1, 10, CAPS))
        .expect("slot freed");
}

// ---- (2)(5) 最初の submit 前の空 publish と、枠不足・確実な終了後の再利用（runner を通す） ----

#[test]
fn empty_publish_before_the_first_submit_does_not_release_the_runner_reservation() {
    let sc = Sc::new();
    let cfg = Cfg::default();
    let mut p = Proc::spawn(sc.cmd(MID, "barrier", &cfg));
    p.wait_line("READY"); // 共有予約は fsync 済み・submit 前。
    assert_eq!(sc.tasks(), 0);
    let before = sc.snap(MID);
    assert_eq!(
        (before.outstanding, before.slots, before.held),
        (1, 1, CASE)
    );
    let caps = sc.g().state().expect("s").caps;
    // 他 ID・自分の ID の空 publish（runner の最初の再集計と同じ）。
    publish(&sc.g(), MID, 0, 0, Caps::UNLIMITED);
    publish(&sc.g(), "other", 0, 0, Caps::UNLIMITED);
    assert_eq!(sc.snap(MID), before, "予約は消えない");
    assert_eq!(sc.g().state().expect("s").caps, caps, "上限は増えない");
    p.go();
    let (code, out) = p.finish();
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(sc.tasks(), 3);
    let s = sc.snap(MID);
    assert_eq!(
        (s.consumed, s.trials, s.outstanding, s.slots),
        (30_000, 3, 0, 0)
    );
}

const CASE: u64 = 100_000;

#[test]
fn two_runner_processes_share_provider_slots_and_reuse_them_after_a_confirmed_end() {
    let sc = Sc::new();
    let cfg = Cfg {
        par: 1,
        ..Cfg::default()
    };
    let mut a = Proc::spawn(sc.cmd("a", "barrier", &cfg));
    a.wait_line("READY"); // a が唯一の枠を予約して待つ。
    // b は a の予約が解けるまで 1 件も投入しない（台帳なしの別 ID でも枠は迂回できない）。
    let o = sc.run_ok("b", "", &cfg);
    assert!(o.contains("reserve_deferred: 3"), "{o}");
    assert_eq!(sc.tasks(), 0);
    assert!(sc.calls().is_empty());
    assert_eq!(sc.snap("b"), Snap::default().with_nothing());
    assert_eq!(sc.snap("a").slots, 1);
    a.go();
    let (code, out) = a.finish();
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(sc.tasks(), 3);
    assert_eq!(sc.snap("a").slots, 0, "終了確認後に解放");
    // 解放された枠を b が再利用する。
    sc.run_ok("b", "", &cfg);
    assert_eq!(sc.tasks(), 6);
    let (sa, sb) = (sc.snap("a"), sc.snap("b"));
    assert_eq!((sa.consumed, sb.consumed), (30_000, 30_000));
    assert_eq!((sa.outstanding, sb.outstanding), (0, 0));
    assert_eq!(sc.g().state().expect("s").caps.provider_slots, 1);
}

impl Snap {
    /// 台帳に行が無い（予約も消費も無い）状態。
    fn with_nothing(self) -> Self {
        self
    }
}

// ---- (3) 実行途中の他 ID 使用増加 ----

#[test]
fn another_ids_usage_growth_mid_run_stops_the_next_submit() {
    let sc = Sc::new();
    let cfg = Cfg {
        max_tokens: 250_000,
        stop_on_budget: false,
        ..Cfg::default()
    };
    let mut p = Proc::spawn(sc.cmd(MID, "barrier", &cfg));
    p.wait_line("READY"); // 1 試行目は予約済み（100k）。ここで他 ID が 150k を消費する。
    publish(&sc.g(), "other", 150_000, 1, Caps::UNLIMITED);
    p.go();
    let (code, out) = p.finish();
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("budget_stopped: true"), "{out}");
    assert_eq!(sc.tasks(), 1, "共有上限を超える 2 件目は投入しない");
    let s = sc.snap(MID);
    assert_eq!((s.consumed, s.outstanding, s.slots), (10_000, 0, 0));
    let stopped = sc
        .records(MID)
        .iter()
        .filter(|r| r.reason.as_deref() == Some(celeris_campaign::record::REASON_BUDGET_STOP))
        .count();
    assert_eq!(stopped, 2);
}

// ---- (4) 既存の低 cap と実 request cap・case/arm 上限の回帰 ----

#[test]
fn persisted_low_cap_binds_the_real_request_and_case_arm_caps_still_apply() {
    // 50k の古い承認: 大きい manifest（10M）でも 1 件も投入しない。
    let sc = Sc::new();
    publish(
        &sc.g(),
        "old",
        0,
        0,
        Caps {
            tokens: 50_000,
            trials: 100,
            provider_slots: 64,
        },
    );
    let o = sc.run_ok(MID, "", &Cfg::default());
    assert!(o.contains("budget_stopped: true"), "{o}");
    assert_eq!((sc.tasks(), sc.calls().len()), (0, 0));
    assert_eq!(sc.snap(MID).outstanding, 0);

    // 150k: 実 request の上限は per_case（100k）以下、永続の最小 cap は 150k のまま（manifest で増えない）。
    let sc = Sc::new();
    publish(
        &sc.g(),
        "old",
        0,
        0,
        Caps {
            tokens: 150_000,
            trials: 100,
            provider_slots: 64,
        },
    );
    sc.run_ok(MID, "", &Cfg::default());
    let b: Vec<u64> = sc.calls().into_iter().map(|c| c.1).collect();
    assert_eq!(b, vec![CASE; 3]);
    assert_eq!(sc.g().state().expect("s").caps.tokens, 150_000);
    assert_eq!(sc.snap(MID).consumed, 30_000);

    // 既存の per_case: 実 request は 5k に絞られ、10k を使った試行は case_budget で止まる。
    let sc = Sc::new();
    let cfg = Cfg {
        per_case: 5_000,
        ..Cfg::default()
    };
    sc.run_ok(MID, "", &cfg);
    let b: Vec<u64> = sc.calls().into_iter().map(|c| c.1).collect();
    assert_eq!(b, vec![5_000; 3]);
    let n = sc
        .records(MID)
        .iter()
        .filter(|r| r.reason.as_deref() == Some(REASON_CASE_BUDGET))
        .count();
    assert_eq!(n, 3);
    assert_eq!(sc.snap(MID).outstanding, 0);

    // 既存の per_arm: 1 試行（10k）で arm の上限に達し、同じ arm の 2 回目は投入しない。
    let sc = Sc::new();
    let cfg = Cfg {
        per_case: 10_000,
        per_arm: 10_000,
        repeats: 2,
        ..Cfg::default()
    };
    sc.run_ok(MID, "", &cfg);
    assert_eq!(sc.tasks(), 3, "arm ごとに 1 件だけ");
    let n = sc
        .records(MID)
        .iter()
        .filter(|r| r.reason.as_deref() == Some(REASON_ARM_BUDGET))
        .count();
    assert_eq!(n, 3);
    assert_eq!(sc.snap(MID).outstanding, 0);
}

// ---- (8) 永続境界での停止と再開 ----

/// 1 回目（hook で停止）→ 停止直後の検証 → 必要なら親が直す → 再開。再開後は常に、全 3 試行が
/// ちょうど 1 回ずつ投入・計上され、未精算の予約と枠が残らず、承認の最小 cap が増えない。
fn crash_and_resume(
    hook: &str,
    code: i32,
    at_stop: impl FnOnce(&Sc),
    between: impl FnOnce(&Sc),
) -> Sc {
    let sc = Sc::new();
    let cfg = Cfg::default();
    let o = sc.run(MID, hook, &cfg);
    assert_eq!(o.code, Some(code), "{hook}: {}", o.stdout);
    let caps = sc.g().state().expect("s").caps;
    at_stop(&sc);
    between(&sc);
    sc.run_ok(MID, "", &cfg);
    assert_eq!(sc.tasks(), 3, "{hook}: 重複投入なし");
    assert_eq!(
        sc.g().state().expect("s").caps,
        caps,
        "{hook}: cap は増えない"
    );
    let s = sc.snap(MID);
    assert_eq!(
        (s.consumed, s.trials, s.outstanding, s.slots),
        (30_000, 3, 0, 0),
        "{hook}: 一度だけ計上"
    );
    assert_eq!(sc.records(MID).len(), 3, "{hook}");
    sc
}

#[test]
fn stop_after_the_reservation_fsync_keeps_it_until_lookup_proves_no_submit() {
    let sc = crash_and_resume(
        "after-reserve",
        9,
        |sc| {
            let s = sc.snap(MID);
            assert_eq!(
                (s.outstanding, s.slots, s.held, s.consumed),
                (1, 1, CASE, 0)
            );
            assert_eq!(sc.tasks(), 0);
            assert_eq!(sc.attempts(MID), vec![(1, AttemptState::Reserved)]);
        },
        |_| {},
    );
    // 投入が無いと照会で確かめて解放（消費 0）。次の attempt が実際の投入になる。
    let a = sc.attempts(MID);
    assert_eq!(a.len(), 4);
    assert!(a.iter().all(|x| x.1 == AttemptState::Settled));
    assert_eq!(sc.calls().len(), 3);
}

#[test]
fn stop_right_after_a_successful_submit_before_the_task_ref_is_saved() {
    let sc = crash_and_resume(
        "after-submit",
        9,
        |sc| {
            assert_eq!(sc.tasks(), 1, "外部には task がある");
            let s = sc.snap(MID);
            assert_eq!((s.outstanding, s.slots), (1, 1));
            assert_eq!(sc.attempts(MID), vec![(1, AttemptState::Reserved)]);
            assert!(sc.records(MID).is_empty());
        },
        |_| {},
    );
    // 再開は lookup で同じ task を拾い、submit し直さない。attempt も同じ。
    assert_eq!(sc.calls().len(), 3);
    assert!(sc.attempts(MID).iter().all(|a| a.0 == 1));
}

#[test]
fn global_and_local_update_orders_are_covered_at_every_boundary() {
    // 順 1（ローカル → 全体）: ローカルが投入済みになった直後、全体の Submitted 化の前に停止。
    let sc = crash_and_resume(
        "torn-after-submit",
        4,
        |sc| {
            assert_eq!(sc.tasks(), 1);
            let g = sc.g();
            assert!(g.state().is_ok(), "読めるが追記は拒否される");
            assert!(g.outstanding(MID).expect("o").len() == 1);
        },
        |sc| sc.repair_tail(),
    );
    assert!(sc.attempts(MID).iter().all(|a| a.0 == 1));

    // 順 1: ローカルが終端（記録・台帳 done）になった後、全体の精算前に停止。
    let sc = crash_and_resume(
        "torn-before-settle",
        4,
        |sc| {
            assert_eq!(sc.records(MID).len(), 1, "ローカルは終端");
            let s = sc.snap(MID);
            assert_eq!(
                (s.outstanding, s.slots, s.consumed),
                (1, 1, 0),
                "全体は未精算のまま保持"
            );
            assert_eq!(sc.attempts(MID), vec![(1, AttemptState::Submitted)]);
        },
        |sc| sc.repair_tail(),
    );
    assert!(sc.attempts(MID).iter().all(|a| a.0 == 1));

    // 全体の精算後・ローカル（再集計の公開・残りの試行）の反映前に停止（精算を親が済ませて再現）。
    let sc = crash_and_resume(
        "torn-before-settle",
        4,
        |_| {},
        |sc| {
            sc.repair_tail();
            let a = id(MID, &sc.key(MID, ConfigId::A), 1);
            sc.g()
                .settle(&a, finished(10_000))
                .expect("global settled first");
            let s = sc.snap(MID);
            assert_eq!((s.outstanding, s.consumed, s.trials), (0, 10_000, 1));
        },
    );
    // 再開側が再精算しても（置換）二重に数えない: crash_and_resume が 30_000 / 3 を assert 済み。
    assert!(sc.attempts(MID).iter().all(|a| a.0 == 1));

    // 順 2（全体 → ローカル）: 全体が精算済みなのにローカルが未反映（task は終了済み・記録なし）。
    let sc = crash_and_resume(
        "before-record",
        9,
        |sc| {
            assert!(sc.records(MID).is_empty());
            assert_eq!(sc.snap(MID).outstanding, 1);
        },
        |sc| {
            let a = id(MID, &sc.key(MID, ConfigId::A), 1);
            sc.g().mark_submitted(&a).expect("sub");
            sc.g().settle(&a, finished(10_000)).expect("global first");
        },
    );
    assert!(sc.attempts(MID).iter().all(|a| a.0 == 1));

    // 実行器の終了を確かめた後・記録の前（全体は Submitted のまま）に停止。
    let sc = crash_and_resume(
        "before-record",
        9,
        |sc| {
            assert_eq!(sc.attempts(MID), vec![(1, AttemptState::Submitted)]);
            assert_eq!(sc.snap(MID).consumed, 0);
        },
        |_| {},
    );
    assert_eq!(sc.calls().len(), 3);
}

// ---- (7) Refused / NotSent / Unknown / 通信断 / cancel 未確認 ----

#[test]
fn refused_submit_releases_only_that_reservation_and_charges_nothing() {
    let sc = Sc::new();
    let cfg = Cfg::default();
    sc.run_ok(MID, "submit-refused", &cfg);
    assert_eq!(sc.tasks(), 2);
    let recs = sc.records(MID);
    assert_eq!(recs.len(), 3);
    assert_eq!(
        recs.iter()
            .filter(|r| r.reason.as_deref() == Some(REASON_EXECUTOR_ERROR))
            .count(),
        1
    );
    let s = sc.snap(MID);
    assert_eq!(
        (s.consumed, s.trials, s.outstanding, s.slots),
        (20_000, 2, 0, 0)
    );
    // 再開しても何も増えない。
    sc.run_ok(MID, "", &cfg);
    assert_eq!(sc.tasks(), 2);
    assert_eq!(sc.snap(MID).consumed, 20_000);
}

/// 1 回目で 1 試行だけ未確定のまま残り（予約と枠を保持）、再開で 3 試行が 1 回ずつ計上される。
fn held_then_resumed(hook: &str, state: AttemptState, tasks_at_stop: usize) {
    let sc = Sc::new();
    let cfg = Cfg::default();
    sc.run_ok(MID, hook, &cfg);
    assert_eq!(sc.tasks(), tasks_at_stop, "{hook}");
    let s = sc.snap(MID);
    assert_eq!(
        (s.consumed, s.outstanding, s.slots),
        (20_000, 1, 1),
        "{hook}"
    );
    assert!(
        sc.attempts(MID).iter().any(|a| a.1 == state),
        "{hook}: {:?}",
        sc.attempts(MID)
    );
    sc.run_ok(MID, "", &cfg);
    assert_eq!(sc.tasks(), 3, "{hook}");
    let s = sc.snap(MID);
    assert_eq!(
        (s.consumed, s.trials, s.outstanding, s.slots),
        (30_000, 3, 0, 0),
        "{hook}"
    );
    assert!(
        sc.attempts(MID).iter().all(|a| a.0 == 1),
        "{hook}: same attempt"
    );
}

#[test]
fn not_sent_unknown_and_lost_connections_hold_the_reservation_until_resumed() {
    // 送っていない（外部に task なし）。
    held_then_resumed("submit-notsent", AttemptState::Reserved, 2);
    // 外部には作られたが応答を失った（結果不明）。再開は lookup で拾い、再 submit しない。
    held_then_resumed("submit-unknown", AttemptState::Reserved, 3);
    // lookup の通信断（投入したか分からない）。
    held_then_resumed("lookup-lost", AttemptState::Reserved, 2);
    // 投入後の poll の通信断。
    held_then_resumed("poll-lost", AttemptState::Submitted, 3);
}

#[test]
fn unconfirmed_cancel_keeps_every_reservation_until_the_stop_is_confirmed() {
    let sc = Sc::new();
    let cfg = Cfg::default();
    sc.run_ok(MID, "cancel-unconfirmed", &cfg);
    assert_eq!(sc.records(MID).len(), 3);
    let s = sc.snap(MID);
    assert_eq!(
        (s.outstanding, s.slots),
        (3, 3),
        "停止未確認: 全部持ち続ける"
    );
    sc.run_ok(MID, "", &cfg);
    assert_eq!(sc.tasks(), 3);
    let s = sc.snap(MID);
    assert_eq!((s.outstanding, s.slots), (0, 0));
    assert_eq!(s.consumed, 3 * 200_000, "観測した使用量を一度だけ");
    assert!(sc.attempts(MID).iter().all(|a| a.0 == 1));
}

// ---- (6) retry の累積既消費と旧行 ----

#[test]
fn retry_attempts_accumulate_prior_consumption_without_double_counting() {
    let sc = Sc::new();
    // 前の attempt（7k 消費で精算済み）がある試行を、runner が次の attempt で再実行する。
    let g = sc.g();
    let a = TrialAdmission {
        measurement_id: MID.into(),
        trial: sc.key(MID, ConfigId::B),
        manifest_sha256: sc.sha(MID),
        caps: Caps::UNLIMITED,
        tokens: CASE,
        provider_slots: 1,
        adopt: false,
    };
    let Admission::Granted { id: first, .. } = g.admit_trial(&a).expect("admit") else {
        panic!("granted");
    };
    g.mark_submitted(&first).expect("sub");
    g.settle(&first, finished(7_000)).expect("settle");
    sc.run_ok(MID, "", &Cfg::default());
    assert_eq!(sc.tasks(), 3);
    let s = sc.snap(MID);
    assert_eq!((s.consumed, s.trials, s.outstanding), (37_000, 3, 0));
    assert!(sc.attempts(MID).contains(&(2, AttemptState::Settled)));
    // もう一度走らせても増えない。
    sc.run_ok(MID, "", &Cfg::default());
    assert_eq!(sc.snap(MID).consumed, 37_000);
}

#[test]
fn legacy_rows_count_against_runner_admission_and_never_raise_caps() {
    // 旧 /1 行: 9_999_000 tokens を消費済み・枠 64 を占有（manifest は 10M・64 枠）。
    let sc = Sc::new();
    let old = json!({"schema": GLOBAL_LEDGER_SCHEMA_V1, "measurement_id": "old", "manifest_sha256": "x",
        "tokens": 9_999_000, "wall_secs": 5, "trials": 4, "provider_slots": 64});
    fs::create_dir_all(sc.gdir()).expect("dir");
    fs::write(sc.gdir().join(GLOBAL_LEDGER_FILE), format!("{old}\n")).expect("w");
    // 枠が旧行に占有されている間は 1 件も投入しない。
    let o = sc.run_ok(MID, "", &Cfg::default());
    assert!(o.contains("reserve_deferred: 3"), "{o}");
    assert_eq!(sc.tasks(), 0);
    // 持ち主が終了を確かめて枠だけ解放しても、既消費は残る: 9_999_000 + 100k > 10M で投入しない。
    sc.g().confirm_legacy_idle("old").expect("idle");
    let o = sc.run_ok(MID, "", &Cfg::default());
    assert!(o.contains("budget_stopped: true"), "{o}");
    assert_eq!(sc.tasks(), 0);
    let v = &sc.g().current().expect("c")["old"];
    assert_eq!((v.tokens, v.provider_slots), (9_999_000, 0));
}
