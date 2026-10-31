//! ops-r2 の fixture probe（ADR 2026-10-30-ops-r2-isolation-principal、probe unit）。
//! 実験 LLM・実 HPC 負荷・本物の daemon は使わない。確かめること:
//! (a) B の CoS が自分で済ませる経路（CoS chat run）と task 経由の経路（worker run）の両方から、private grader・
//!     旧解答・他試行の出力が読めない。canary を許可領域に置く漏洩と、許可表の残り（deny_readable）で fail-closed する。
//!     実 bwrap（ops-r2-sandbox.sh）を使うので CELERIS_USERNS_TESTS=1 のときだけ走り、それ以外は skip と出す。
//! (b) 同じ relay の操作が human-ops・operator-usage・CoS 消費の 3 つで二重に数えられない。
//! (c) operator principal の relay が出典付きで記録され、operator の変更操作と human-ops の op 行が 1 対 1 で対応する。
//!     (b)(c) は実 task-api router（in-process、一時 DB）と stub の transcript で、userns は要らない。
#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use api_common::{EnvOptions, project_cos_thread, send};
use celeris_campaign::{
    celeris_exec::{HttpResponse, Transport},
    human_ops::{ActorKind, read_records},
    human_ops_report::counts_as_human_op,
    operator_usage::{Category, UsageRow, import},
    ops_r1_intake::{IntakeInput, IntakeRecord, Mode, SourceInput, SourceKind, intake, sha256_hex},
    preflight_isolation::{ProbeRequest, ScriptSandbox, Wrap, allow_sources, findings, run},
    runtime_probe::RuntimeProbe,
};
use serde_json::{Value, json};
use task_api::{ApiToken, Principal};

fn bin() -> String {
    std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path")
}

fn scripts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/campaign")
}

fn s(p: &Path) -> &str {
    p.to_str().expect("utf-8 path")
}

fn ok(o: &Output, what: &str) {
    assert!(
        o.status.success(),
        "{what}: {}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

// ---- (a) 両経路の隔離と fail-closed（userns が要る） ----

/// bwrap の user namespace が使えるときだけ true。使えなければ理由を出して skip する（ADR-0126）。
fn userns_enabled(what: &str) -> bool {
    let setting = std::env::var("CELERIS_USERNS_TESTS").unwrap_or_default();
    if setting != "1" && setting != "require" {
        eprintln!("skip: {what} requires CELERIS_USERNS_TESTS=1 (or require)");
        return false;
    }
    let usable = Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--proc", "/proc", "--unshare-pid"])
        .args(["--", "true"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !usable {
        assert_ne!(setting, "require", "bwrap user namespace is required");
        eprintln!("skip: {what}: bwrap cannot create a namespace here");
    }
    usable
}

const CANARY: &str = "CANARY-ops-r2-probe-private-7f3e9a";

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    sb: ScriptSandbox,
    /// private grader・旧解答・他試行の出力（どの主体からも読めないはず）。
    deny: Vec<PathBuf>,
    private: PathBuf,
    public: PathBuf,
}

fn fixture() -> Fixture {
    // ops-r2-sandbox.sh は workspaces・state の下の path を拒むので、cargo の target の一時 dir を使う。
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("tempdir");
    let root = dir.path().to_path_buf();
    let write = |p: &Path, body: &str| {
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, body).expect("write");
    };
    let private = root.join("eval/private/k2/private.json");
    write(
        &private,
        &json!({"schema": "grader-private/1", "case_id": "k2", "expect": CANARY}).to_string(),
    );
    let old = root.join("old-campaign/out/trial-000/artifacts/answer.md");
    write(&old, "旧解答: 前の試行が出した答え\n");
    let other = root.join("state-t002/ws/other-task/repos/work/out.txt");
    write(&other, "他試行の出力\n");
    let state = root.join("state-t001");
    let public = state.join("inputs/public-case.txt");
    write(&public, "公開入力: K2 の case の説明\n");
    let release = root.join("release");
    fs::create_dir_all(&release).expect("release");
    fs::copy(bin(), release.join("celeris-campaign")).expect("stage probe");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ops_r2_probe/daemon.sh"),
        release.join("daemon.sh"),
    )
    .expect("stage daemon fixture");
    let account = root.join("account");
    write(&account.join(".credentials.json"), "{}");
    Fixture {
        sb: ScriptSandbox {
            state: Some(state),
            release: Some(release),
            account: Some(account),
            ro: Vec::new(),
        },
        deny: vec![private.clone(), old, other],
        root,
        _dir: dir,
        private,
        public,
    }
}

fn sandbox_base(sb: &ScriptSandbox) -> Vec<String> {
    let mut a = vec![s(&scripts().join("ops-r2-sandbox.sh")).to_string()];
    for (flag, p) in [
        ("--state", &sb.state),
        ("--release", &sb.release),
        ("--account", &sb.account),
    ] {
        a.extend([flag.into(), s(p.as_ref().expect("path")).into()]);
    }
    for r in &sb.ro {
        a.extend(["--ro".into(), s(r).into()]);
    }
    a
}

/// 測定 daemon の fixture を ops-r2-sandbox.sh の中で起動し、CoS run と worker run の probe 結果を返す。
/// mount の照合に使う許可表は preflight と同じく `--print-args` の引数から作る。
fn run_daemon_fixture(fx: &Fixture, deny: &[PathBuf]) -> [(RuntimeProbe, i32, String); 2] {
    let state = fx.sb.state.as_ref().expect("state");
    let release = fx.sb.release.as_ref().expect("release");
    let mut print = sandbox_base(&fx.sb);
    print.extend(["--print-args".into(), "--".into(), "true".into()]);
    let o = Command::new("sh").args(&print).output().expect("sh");
    ok(&o, "ops-r2-sandbox.sh --print-args");
    let printed: Vec<String> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    let host = fs::read_to_string("/proc/self/mountinfo").expect("mountinfo");
    let mut a = sandbox_base(&fx.sb);
    a.extend([
        "--".into(),
        "sh".into(),
        s(&release.join("daemon.sh")).into(),
        s(state).into(),
        s(&release.join("celeris-campaign")).into(),
    ]);
    let deny_file = state.join("deny.list");
    let text: String = deny.iter().map(|d| format!("{}\n", d.display())).collect();
    fs::write(&deny_file, text).expect("deny file");
    a.extend(["--deny-readable-file".into(), s(&deny_file).into()]);
    for src in allow_sources(&printed, &host) {
        a.extend(["--allow".into(), s(&src).into()]);
    }
    let o = Command::new("sh").args(&a).output().expect("sh");
    ok(&o, "daemon fixture in ops-r2-sandbox.sh");
    let out = state.join("probe");
    ["cos", "worker"].map(|k| {
        let body = fs::read(out.join(format!("{k}.json"))).expect("probe json");
        let p: RuntimeProbe = serde_json::from_slice(&body)
            .unwrap_or_else(|e| panic!("{k}: {e}: {}", String::from_utf8_lossy(&body)));
        let exit = fs::read_to_string(out.join(format!("{k}.exit")))
            .expect("exit")
            .trim()
            .parse()
            .expect("exit code");
        let ns = fs::read_to_string(out.join(format!("{k}.ns")))
            .expect("ns")
            .trim()
            .to_string();
        (p, exit, ns)
    })
}

fn leak_check(fx: &Fixture, probes: &[PathBuf], out: &Path) -> (i32, Value) {
    let mut cmd = Command::new("python3");
    cmd.arg("-I")
        .arg(scripts().join("check-ops-r1-leak.py"))
        .args(["--private", s(&fx.private), "--canary", CANARY])
        .args(["--scan", s(&fx.public.parent().expect("inputs"))]);
    for d in &fx.deny {
        cmd.args(["--deny-readable", s(d)]);
    }
    for p in probes {
        cmd.args(["--runtime-probe", s(p)]);
    }
    let o = cmd.args(["--out", s(out)]).output().expect("python3");
    let v = serde_json::from_slice(&fs::read(out).unwrap_or_default()).unwrap_or(Value::Null);
    (o.status.code().unwrap_or(-1), v)
}

/// CoS chat run と worker run は同じ daemon の mount namespace を継ぎ、どちらからも deny の 3 種が読めない。
/// 公開入力（許可領域）は読める（probe が拒否を偽っていない）。漏洩検査は両経路の probe を採って通る。
#[test]
fn cos_direct_and_worker_runs_cannot_read_private_old_or_other_trial() {
    if !userns_enabled("cos/worker isolation probe") {
        return;
    }
    let fx = fixture();
    let mut deny = fx.deny.clone();
    deny.push(fx.public.clone());
    let [cos, worker] = run_daemon_fixture(&fx, &deny);
    let host_ns = fs::read_link("/proc/self/ns/pid").expect("host ns");
    assert_eq!(
        cos.2, worker.2,
        "CoS run と worker run は同じ pid namespace"
    );
    assert_ne!(Path::new(&cos.2), host_ns, "daemon fixture は host の外");
    for (who, (p, exit, _)) in [("B-cos", &cos), ("B-worker", &worker)] {
        assert_eq!(p.principal, who);
        assert!(p.mounts_outside_allow.is_empty(), "{who}: {p:?}");
        for x in &p.paths[..3] {
            assert!(!x.readable && !x.listable, "{who}: {x:?}");
        }
        // 公開入力だけが読める = deny_readable の 1 件（control）。exit 1 は「拒否されていない path がある」。
        assert!(p.paths[3].readable, "{who}: {p:?}");
        assert_eq!(
            findings(p),
            [format!("deny_readable:{who}:{}", fx.public.display())]
        );
        assert_eq!(*exit, 1, "{who}");
    }
    // control を外した deny list では両経路とも findings 0 件・exit 0。
    let [cos, worker] = run_daemon_fixture(&fx, &fx.deny);
    let state_probe = fx.sb.state.as_ref().expect("state").join("probe");
    let probe_dir = fx.root.join("probe-out");
    fs::create_dir_all(&probe_dir).expect("probe-out");
    let mut files = Vec::new();
    for (k, (p, exit, _)) in [("cos", &cos), ("worker", &worker)] {
        assert!(p.ok && findings(p).is_empty(), "{k}: {p:?}");
        assert_eq!(*exit, 0, "{k}");
        // probe-read が書いた JSON をそのまま漏洩検査に渡す。
        let f = probe_dir.join(format!("{k}.json"));
        fs::copy(state_probe.join(format!("{k}.json")), &f).expect("copy probe");
        files.push(f);
    }
    let (code, v) = leak_check(&fx, &files, &fx.root.join("leak.json"));
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["pass"], true, "{v}");
}

/// 漏洩の fixture: (1) private の写し（canary 入り）を許可領域に置くと漏洩検査が exit 1。
/// (2) 許可表に旧解答の dir が残る（`--ro`）と、preflight と同じ `preflight_isolation::run` の B の probe が
/// deny_readable を返す（preflight の missing に入り、run_gated が有料実行・task 作成を 0 件で止める。
/// その接続は tests/preflight.rs の one_readable_path_in_any_principal_stops_the_run が固定している）。
/// (3) 同じ残りは daemon fixture の CoS run・worker run の両方からも読める。
#[test]
fn leaks_and_residual_deny_readable_fail_closed_on_both_paths() {
    if !userns_enabled("leak fail-closed probe") {
        return;
    }
    let fx = fixture();
    // (1) canary を許可領域に置いた漏洩。
    let leaked = fx.public.with_file_name("leaked-private.json");
    fs::copy(&fx.private, &leaked).expect("leak");
    let (code, v) = leak_check(&fx, &[], &fx.root.join("leak.json"));
    assert_eq!(code, 1, "{v}");
    assert_eq!(v["pass"], false, "{v}");
    assert!(
        !v.to_string().contains(CANARY),
        "結果 file は canary を書かない"
    );
    fs::remove_file(&leaked).expect("unleak");

    // (2) 許可表の残り: 旧解答の dir を ro で足した B の sandbox。
    let old_dir = fx.deny[1].parent().expect("old dir").to_path_buf();
    let mut residual = fx.sb.clone();
    residual.ro.push(old_dir);
    let req = |sb: &ScriptSandbox| ProbeRequest {
        principal: "B",
        program: bin().into(),
        wrap: Wrap::Script {
            script: scripts().join("ops-r2-sandbox.sh"),
            sandbox: sb.clone(),
        },
        deny: fx.deny.clone(),
    };
    let clean = run(&req(&fx.sb)).expect("clean probe");
    assert!(findings(&clean).is_empty(), "{clean:?}");
    let bad = run(&req(&residual)).expect("residual probe");
    assert_eq!(
        findings(&bad),
        [format!("deny_readable:B:{}", fx.deny[1].display())]
    );

    // (3) 同じ残りで daemon fixture の両経路とも読めてしまう（どちらの経路でも塞ぎ方は同じ許可表）。
    let fx2 = Fixture { sb: residual, ..fx };
    let [cos, worker] = run_daemon_fixture(&fx2, &fx2.deny);
    for (who, (p, exit, _)) in [("B-cos", &cos), ("B-worker", &worker)] {
        assert_eq!(
            findings(p),
            [format!("deny_readable:{who}:{}", fx2.deny[1].display())]
        );
        assert_eq!(*exit, 1, "{who}");
    }
}

// ---- (b)(c) relay の出典・1 対 1 照合・二重計上なし（userns は要らない） ----

const REQUEST: &str = "固定依頼: K2 の case を解いて報告する。\n";
const OPERATOR_TOKEN: &str = "ops-r2-probe-operator-token";
const SESSION: &str = "22222222-3333-4444-8555-666666666666";
const COS_SESSION: &str = "cccccccc-0000-4000-8000-000000000001";
const H: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct RouterTransport {
    app: axum::Router,
    rt: tokio::runtime::Handle,
    token: &'static str,
}

impl Transport for RouterTransport {
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse, String> {
        let auth = format!("Bearer {}", self.token);
        let headers = [("authorization", auth.as_str())];
        let req = match (method, body) {
            ("GET", None) => api_common::get_with(path, &headers),
            ("POST", Some(b)) => api_common::post_json_with(path, b, &headers),
            _ => return Err(format!("unexpected {method} {path}")),
        };
        let r = self.rt.block_on(send(&self.app, req));
        let body = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
        Ok(HttpResponse {
            status: r.status.as_u16(),
            body,
        })
    }
}

/// 試行 2 件: CoS が自分で済ませる条件と task 経由の条件。relay は試行の前、CoS の選択より前に入る。
struct Trial {
    key: &'static str,
    condition: &'static str,
    rep: &'static str,
    decision: &'static str,
    said_at: &'static str,
    observe: (&'static str, &'static str),
    relay_op: (&'static str, &'static str),
}

const TRIALS: [Trial; 2] = [
    Trial {
        key: "ops-r2-B-k2-direct",
        condition: "B-cos-direct",
        rep: "1",
        decision: "01APPROVALDIRECT",
        said_at: "2026-10-30T01:00:00Z",
        observe: ("2026-10-30T01:05:00Z", "2026-10-30T01:30:00Z"),
        relay_op: ("2026-10-30T01:10:00Z", "2026-10-30T01:11:00Z"),
    },
    Trial {
        key: "ops-r2-B-k2-task",
        condition: "B-task-worker",
        rep: "2",
        decision: "01APPROVALTASK",
        said_at: "2026-10-30T02:00:00Z",
        observe: ("2026-10-30T02:05:00Z", "2026-10-30T02:30:00Z"),
        relay_op: ("2026-10-30T02:10:00Z", "2026-10-30T02:11:00Z"),
    },
];

fn human_op(args: &[&str]) {
    let o = Command::new(bin())
        .arg("human-op")
        .args(args)
        .env("CELERIS_HUMAN_OPS_NOW", "2026-10-30T03:00:00Z")
        .env("CELERIS_HUMAN_OPS_HOST", "eval-host")
        .output()
        .expect("spawn");
    ok(&o, "human-op");
}

fn record_relay_op(file: &Path, t: &Trial, r: &IntakeRecord) {
    let h = &r.human_op;
    human_op(&[
        "record",
        "--file",
        s(file),
        "--campaign",
        "c-probe",
        "--manifest-sha256",
        H,
        "--case",
        "k2",
        "--arm",
        "B0",
        "--rep",
        t.rep,
        "--actor-kind",
        h.actor_kind.as_str(),
        "--actor",
        h.actor.as_str(),
        "--decided-by",
        h.decided_by.as_str(),
        "--executed-by",
        h.executed_by.as_str(),
        "--approved-by",
        h.approved_by.as_str(),
        "--credential",
        h.credential.as_str(),
        "--request-ref",
        h.request_ref.as_str(),
        "--request-revision",
        h.request_revision.as_str(),
        "--policy-ref",
        h.policy_ref.as_str(),
        "--celeris-event-ref",
        h.celeris_event_ref.as_str(),
        "--mode",
        "manual",
        "--intent",
        "protocol",
        "--op-kind",
        "protocol_step",
        "--logical-op",
        "relay-request",
        "--started-at",
        t.relay_op.0,
        "--ended-at",
        t.relay_op.1,
        "--outcome",
        "ok",
    ]);
}

/// D5 の照合: thread に現れた operator principal の message（変更操作）と、human-ops の external_operator の
/// op 行の celeris_event_ref を突き合わせ、対応しないものを返す（(op 行の無い操作, 操作の無い op 行)）。
fn reconcile(messages: &[Value], human_ops: &Path) -> (Vec<String>, Vec<String>) {
    let ops: BTreeSet<String> = messages
        .iter()
        .filter(|m| {
            m["author"]
                .as_str()
                .is_some_and(|a| a.starts_with("operator:"))
        })
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    let rows: BTreeSet<String> = read_records(human_ops)
        .expect("records")
        .iter()
        .filter(|r| {
            r.actor
                .as_ref()
                .is_some_and(|a| a.kind == ActorKind::ExternalOperator)
        })
        .filter_map(|r| r.celeris_event_ref.clone())
        .collect();
    (
        ops.difference(&rows).cloned().collect(),
        rows.difference(&ops).cloned().collect(),
    )
}

fn assistant(session: &str, id: &str, at: &str, agent: Option<&str>, tokens: u64) -> Value {
    let mut v = json!({"type": "assistant", "sessionId": session, "timestamp": at,
        "isSidechain": agent.is_some(), "message": {"id": id, "model": "claude-test",
        "content": [{"type": "text", "text": "secret"}],
        "usage": {"input_tokens": tokens, "output_tokens": tokens / 10,
            "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}}});
    if let Some(a) = agent {
        v["agentId"] = json!(a);
    }
    v
}

fn session_map(args: &[&str]) {
    let o = Command::new("python3")
        .arg("-I")
        .arg(scripts().join("ops-r2-session-map.py"))
        .args(args)
        .output()
        .expect("python3");
    ok(&o, "ops-r2-session-map.py");
}

#[tokio::test(flavor = "multi_thread")]
async fn relay_is_sourced_reconciled_and_counted_once_for_both_paths() {
    let env = api_common::TestEnv::with(EnvOptions {
        extra_tokens: vec![ApiToken::new(
            Principal::Operator {
                name: "ops-r1".into(),
            },
            OPERATOR_TOKEN,
        )],
        ..EnvOptions::default()
    });
    let thread = project_cos_thread(&env.store, "ops-r2-probe");
    let app = env.router();
    let transport = RouterTransport {
        app: app.clone(),
        rt: tokio::runtime::Handle::current(),
        token: OPERATOR_TOKEN,
    };
    let tid = thread.id.clone();
    // (c) relay: 試行ごとに承認済み依頼文を operator principal で relay する。出典は承認の id と依頼文 sha256。
    let records: Vec<IntakeRecord> = tokio::task::spawn_blocking(move || {
        TRIALS
            .iter()
            .map(|t| {
                let input = IntakeInput {
                    thread_id: tid.clone(),
                    trial_id: t.key.into(),
                    condition_id: t.condition.into(),
                    request_text: REQUEST.into(),
                    mode: Mode::Relay,
                    source: SourceInput {
                        kind: SourceKind::ApprovedRequest,
                        daemon: "production".into(),
                        thread_id: None,
                        message_id: None,
                        decision_id: Some(t.decision.into()),
                        said_at: t.said_at.into(),
                        request_sha256: sha256_hex(REQUEST.as_bytes()),
                        decided_by: "human:rmaeda".into(),
                        approved_by: "human:rmaeda".into(),
                    },
                    operator_session: Some(SESSION.into()),
                };
                intake(&transport, &input).expect("relay")
            })
            .collect()
    })
    .await
    .expect("join");
    for (t, r) in TRIALS.iter().zip(&records) {
        assert_eq!(r.intake_mode, "relay");
        assert_eq!(r.executed_by.id, "operator:ops-r1");
        assert_eq!(r.relayed_by, "operator:ops-r1");
        assert_eq!(r.decided_by.kind, ActorKind::Human);
        assert_eq!(r.approved_by.kind, ActorKind::Human);
        assert_eq!(r.source.decision_id.as_deref(), Some(t.decision));
        assert_eq!(r.source.request_sha256, sha256_hex(REQUEST.as_bytes()));
        assert_eq!(r.human_op.policy_ref, format!("decision:{}", t.decision));
    }

    // human-ops: relay の op（external_operator）を試行ごとに 1 件、人の区間は direct の試行だけに 1 件。
    let dir = tempfile::tempdir().expect("dir");
    let ev = dir.path();
    let hops = ev.join("human-ops.jsonl");
    for (t, r) in TRIALS.iter().zip(&records) {
        record_relay_op(&hops, t, r);
    }
    human_op(&[
        "record",
        "--file",
        s(&hops),
        "--campaign",
        "c-probe",
        "--manifest-sha256",
        H,
        "--case",
        "k2",
        "--arm",
        "B0",
        "--rep",
        "1",
        "--actor-kind",
        "human",
        "--actor",
        "rmaeda",
        "--decided-by",
        "human:rmaeda",
        "--executed-by",
        "human:rmaeda",
        "--mode",
        "manual",
        "--intent",
        "safety",
        "--op-kind",
        "safety_approval",
        "--logical-op",
        "approve",
        "--started-at",
        "2026-10-30T00:58:00Z",
        "--ended-at",
        "2026-10-30T01:00:00Z",
        "--outcome",
        "ok",
    ]);

    // (c) D5 の照合: operator の変更操作（relay の message）と op 行が 1 対 1。
    let list_messages = || async {
        let r = send(
            &app,
            api_common::get_with(
                &format!("/api/v1/chat/threads/{}/messages", thread.id),
                &[("authorization", &format!("Bearer {OPERATOR_TOKEN}"))],
            ),
        )
        .await;
        r.json()["items"].as_array().expect("items").clone()
    };
    let msgs = list_messages().await;
    assert_eq!(msgs.iter().filter(|m| m["relay"].is_object()).count(), 2);
    assert_eq!(reconcile(&msgs, &hops), (vec![], vec![]));
    // op 行の無い operator の操作は照合で残る（その試行は正式にしない）。
    let stray = send(
        &app,
        api_common::post_json_with(
            &format!("/api/v1/chat/threads/{}/messages", thread.id),
            &json!({"client_message_id": "stray-1", "text": "記録の無い操作", "attachment_ids": [],
                "reply_to_id": null, "mode": "queue", "resume_queue": false}),
            &[("authorization", &format!("Bearer {OPERATOR_TOKEN}"))],
        ),
    )
    .await;
    assert_eq!(stray.status.as_u16(), 202, "{}", stray.text());
    let stray_id = stray.json()["message"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    assert_eq!(
        reconcile(&list_messages().await, &hops),
        (vec![stray_id], vec![])
    );

    // (b-1) human-ops: relay の op は人の能動時間に数えない。人の時間は人が記録した区間だけ。
    let recs = read_records(&hops).expect("records");
    // trial_key は human-op が identity（campaign・case・arm・rep）から決める。
    let keys: Vec<String> = TRIALS
        .iter()
        .map(|t| {
            recs.iter()
                .find(|r| r.repetition.map(|n| n.to_string()).as_deref() == Some(t.rep))
                .and_then(|r| r.trial_key.clone())
                .expect("trial_key")
        })
        .collect();
    // op_start の start と op_end の end を (trial, logical op) で対にする。
    let mut spans: BTreeMap<(String, String), (Option<String>, Option<String>)> = BTreeMap::new();
    let mut operator_ops: BTreeSet<(String, String)> = BTreeSet::new();
    for r in &recs {
        let key = (
            r.trial_key.clone().expect("trial_key"),
            r.logical_op_id.clone().expect("op"),
        );
        if counts_as_human_op(r) {
            let e = spans.entry(key).or_default();
            if r.start.is_some() {
                e.0.clone_from(&r.start);
            }
            if r.end.is_some() {
                e.1.clone_from(&r.end);
            }
        } else if r
            .actor
            .as_ref()
            .is_some_and(|a| a.kind == ActorKind::ExternalOperator)
        {
            operator_ops.insert(key);
        }
    }
    let p = |x: &str| celeris_campaign::ops_r1_trace::parse_ts(x).expect("ts");
    let mut human_secs: BTreeMap<String, i64> = BTreeMap::new();
    for ((trial, _), (st, en)) in &spans {
        let (Some(st), Some(en)) = (st, en) else {
            continue;
        };
        *human_secs.entry(trial.clone()).or_default() += (p(en) - p(st)).whole_seconds();
    }
    assert_eq!(human_secs, BTreeMap::from([(keys[0].clone(), 120)]));
    assert_eq!(
        operator_ops.len(),
        2,
        "relay は試行ごとに 1 op（start と end の 2 行で 1 件）"
    );

    // (b-2) operator-usage: session-map（base → human-op list の介入で final）と親・subagent の transcript。
    let trials = ev.join("trials.json");
    fs::write(
        &trials,
        json!({"trials": TRIALS.iter().zip(&keys).map(|(t, k)| json!({"trial_key": k, "condition_id": t.condition,
            "request_ref": format!("decision:{}", t.decision),
            "observe": {"start": t.observe.0, "end": t.observe.1}})).collect::<Vec<_>>()})
        .to_string(),
    )
    .expect("trials");
    let (base, st) = (
        ev.join("session-map.base.json"),
        ev.join("session-trials.json"),
    );
    session_map(&[
        "base",
        "--session-id",
        SESSION,
        "--trials",
        s(&trials),
        "--out-map",
        s(&base),
        "--out-trials",
        s(&st),
    ]);
    let listed = Command::new(bin())
        .args(["human-op", "list", "--file", s(&hops)])
        .output()
        .expect("list");
    ok(&listed, "human-op list");
    let list = ev.join("human-ops-list.json");
    fs::write(&list, &listed.stdout).expect("list file");
    let map = ev.join("session-map.json");
    session_map(&[
        "final",
        "--base",
        s(&base),
        "--trials",
        s(&st),
        "--human-ops-list",
        s(&list),
        "--out",
        s(&map),
    ]);
    // 親の transcript と subagent の transcript。subagent の行は親にも同じ message id で写る（累積の再出現）。
    // CoS run の session の行が snapshot に紛れても、どの試行の operator 消費にも入らない。
    let snap = ev.join("operator").join(SESSION).join("end");
    fs::create_dir_all(snap.join("subagents")).expect("snapshot");
    let line = |v: Value| format!("{v}\n");
    let sub = assistant(SESSION, "s-relay", "2026-10-30T02:10:30Z", Some("a1"), 1000);
    let main: String = [
        assistant(SESSION, "m-obs-d", "2026-10-30T01:06:00Z", None, 100),
        assistant(SESSION, "m-relay-d", "2026-10-30T01:10:30Z", None, 200),
        assistant(SESSION, "m-obs-t", "2026-10-30T02:20:00Z", None, 300),
        sub.clone(),
        assistant(COS_SESSION, "cos-run-1", "2026-10-30T01:12:00Z", None, 5000),
    ]
    .into_iter()
    .map(line)
    .collect();
    fs::write(snap.join("main.jsonl"), &main).expect("main");
    fs::write(snap.join("subagents/agent-a1.jsonl"), line(sub)).expect("sub");
    let usage = ev.join("operator-usage.jsonl");
    for _ in 0..2 {
        for f in [
            snap.join("main.jsonl"),
            snap.join("subagents/agent-a1.jsonl"),
        ] {
            import(&f, &map, &usage).expect("import");
        }
    }
    let rows: Vec<UsageRow> = fs::read_to_string(&usage)
        .expect("usage")
        .lines()
        .map(|l| serde_json::from_str(l).expect("row"))
        .collect();
    let got: BTreeMap<&str, (Category, Option<&str>)> = rows
        .iter()
        .map(|r| (r.message_id.as_str(), (r.category, r.trial_key.as_deref())))
        .collect();
    assert_eq!(
        rows.len(),
        got.len(),
        "message ごとに 1 行（再取り込み・親子の重複で増えない）"
    );
    assert_eq!(
        got,
        BTreeMap::from([
            ("m-obs-d", (Category::Observe, Some(keys[0].as_str()))),
            ("m-relay-d", (Category::Intervene, Some(keys[0].as_str()))),
            ("m-obs-t", (Category::Observe, Some(keys[1].as_str()))),
            ("s-relay", (Category::Intervene, Some(keys[1].as_str()))),
            ("cos-run-1", (Category::SharedUnattributable, None)),
        ])
    );
    let summary = ev.join("summary.json");
    session_map(&[
        "summary",
        "--trials",
        s(&st),
        "--usage",
        s(&usage),
        "--snapshot",
        s(&snap),
        "--out",
        s(&summary),
    ]);
    let sv: Value = serde_json::from_slice(&fs::read(&summary).expect("summary")).expect("json");
    let input = |cat: &str, key: &str| {
        sv["groups"]
            .as_array()
            .expect("groups")
            .iter()
            .find(|g| g["category"] == cat && g["trial_key"] == key)
            .map(|g| g["input_tokens"].clone())
    };
    assert_eq!(
        input("intervene", keys[0].as_str()),
        Some(json!(200)),
        "{sv}"
    );
    assert_eq!(
        input("intervene", keys[1].as_str()),
        Some(json!(1000)),
        "{sv}"
    );
    assert_eq!(input("observe", keys[0].as_str()), Some(json!(100)), "{sv}");
    assert_eq!(input("observe", keys[1].as_str()), Some(json!(300)), "{sv}");
    // summary はこの session の行だけを数える（CoS run の 5000 は operator 消費に入らない）。
    let total: u64 = sv["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .filter_map(|g| g["input_tokens"].as_u64())
        .sum();
    assert_eq!(total, 1600, "{sv}");

    // (b-3) 3 つの台帳の鍵が交わらない: human-ops は op、operator-usage は operator session の message、
    // CoS 消費は daemon の run（CoS run・worker run。ここでは固定の行）。relay の操作はそれぞれに 1 回だけ現れる。
    let cos_runs = [
        json!({"run_id": "run-cos-direct", "trial_key": keys[0].as_str(), "kind": "cos_chat",
            "session_id": COS_SESSION, "input_tokens": 5000}),
        json!({"run_id": "run-worker-task", "trial_key": keys[1].as_str(), "kind": "worker",
            "session_id": "wwwwwwww-0000-4000-8000-000000000002", "input_tokens": 7000}),
    ];
    let attributed_operator: BTreeSet<&str> = rows
        .iter()
        .filter(|r| r.trial_key.is_some())
        .map(|r| r.session_id.as_str())
        .collect();
    assert_eq!(attributed_operator, BTreeSet::from([SESSION]));
    for c in &cos_runs {
        assert!(!attributed_operator.contains(c["session_id"].as_str().expect("sid")));
    }
    // human-ops の op 行は人か external_operator だけで、CoS・worker の run を op にしない。
    assert!(recs.iter().all(|r| {
        r.actor
            .as_ref()
            .is_some_and(|a| matches!(a.kind, ActorKind::Human | ActorKind::ExternalOperator))
    }));
}
