//! 実験セット（A/B のみ・B/C のみ）を、実バイナリ `celeris-campaign` の `--executor celeris` から
//! 所有 loopback の HTTP stub（通常の `POST /api/v1/tasks` と lookup/poll/観測/費用の応答）まで通す統合試験。
//! validate → dry-run → preflight → env-lock → run → watch(再開) → check → grade-out → export を両セットで成功させる。
//! 実 LLM・他 LLM CLI・外部 network は使わない。A は LLM を起動しない専用 fixture 実行器（sh script）。
//! stub は 127.0.0.1:動的 port・試験ごとの一時 dir・子 process の環境（token 環境変数は子だけ）で並列試験と衝突しない。
//! stub の終了は event（停止フラグ + 自己接続）と有界 timeout で待つ。cap probe は fixture case が必要能力を宣言しないため
//! 走らない（userns の実走は不要）。
#![allow(dead_code)]
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use celeris_campaign::{
    manifest::{
        ArmRouting, Budget, CampaignManifest, Cap, Config, ConfigId, ExperimentSet, InputRef,
        Secondary, SetFixed, SnapshotRef, Split, Termination, TokenBasis, manifest_hash,
    },
    snapshot::hash_tree,
};
use routing_eval::manifest::Manifest as Input;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

mod daemon_identity;
mod preflight_setup;

use preflight_setup::{CANARY_GRADER, CANARY_NORMAL, CANARY_TRIAL, DAEMON_PID, MEASURE_RELEASE};

const CASES: [&str; 2] = ["fx-alpha", "fx-beta"];
const TOKEN_ENV: &str = "CELERIS_ARMSET_TEST_TOKEN";
/// 値は試験用の使い捨て。子 process の環境にだけ渡し、出力に出ないことを確かめる。
const TOKEN_VALUE: &str = "armset-owned-stub-token-0001";
const A_VERSION: &str = "fxa 2.1.0";
const ROUTING_MODEL: &str = "claude-sonnet-5-5";
/// 公開テスト（成果物 repo に在るべき file。改変・削除は失格）。固定の内容。
const PUBLIC_TEST: &str = "# public acceptance check (fixture)\n";

fn sha(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}

fn write_executable(path: &Path, contents: &str) {
    // ETXTBSY を避けるため、書き込みは別 process（sh）に任せて終わりを待つ。
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(r#"cat > "$1" && chmod 755 "$1""#)
        .arg("sh")
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn writer");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(contents.as_bytes())
        .expect("write");
    assert!(child.wait().expect("wait").success());
}

// ---- 所有 HTTP stub ----

#[derive(Default)]
struct State {
    /// `POST /api/v1/tasks` の本文（通常の作成 API）。
    posts: Vec<Value>,
    /// 受けた GET の path（query 付き）。
    gets: Vec<String>,
    /// 作成した task の詳細（`GET /tasks/{id}` の本文）。
    tasks: BTreeMap<String, Value>,
    /// Authorization が期待どおりでなかった要求数。
    bad_auth: u32,
    other: Vec<String>,
}

struct Stub {
    url: String,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    addr: std::net::SocketAddr,
    done: mpsc::Receiver<()>,
    handle: Option<thread::JoinHandle<()>>,
}

/// (method, path, headers(小文字名), body)
type Request = (String, String, Vec<(String, String)>, Vec<u8>);

fn read_request(s: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next()?.split(' ');
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((method, path, headers, body))
}

fn respond(s: &mut TcpStream, status: u16, body: &Value) {
    let b = body.to_string();
    let _ = write!(
        s,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    );
}

fn query_param(path: &str, key: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    q.split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .map(String::from)
}

/// 費用・採点の集計を確かめる試験（campaign_id が `-acct` 終わり）だけが使う、試行ごとの結末。
/// それ以外の試験は常に `Pass`。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fate {
    Pass,
    /// 成功。同じ task 内に retry の run がもう 1 つある。
    PassWithRetry,
    /// task が failed（成果物なし）。
    Fail,
    /// task は done だが成果物が空で、独立採点が通らない。
    EmptyArtifact,
}

fn fate_of(campaign: &Value) -> Fate {
    if !campaign["campaign_id"]
        .as_str()
        .is_some_and(|c| c.ends_with("-acct"))
    {
        return Fate::Pass;
    }
    match (
        campaign["arm"].as_str(),
        campaign["case_id"].as_str(),
        campaign["repetition"].as_u64(),
    ) {
        (Some("B"), Some("fx-alpha"), Some(1)) => Fate::PassWithRetry,
        (Some("B"), Some("fx-beta"), Some(2)) => Fate::Fail,
        (Some("C"), Some("fx-alpha"), Some(2)) => Fate::EmptyArtifact,
        (Some("C"), Some("fx-beta"), Some(1)) => Fate::Fail,
        _ => Fate::Pass,
    }
}

/// POST 本文から、B/C の worker が作る成果物（`src/answer.txt`）を試行 workspace に置き、完了した task の詳細を作る。
fn finish_task(id: &str, body: &Value) -> Value {
    let ws = PathBuf::from(body["workspace"].as_str().expect("workspace"));
    let fate = fate_of(&body["campaign"]);
    fs::create_dir_all(ws.join("src")).expect("src");
    match fate {
        // 失敗した task は成果物を残さない。
        Fate::Fail => {}
        // 実行器は done だが、独立採点の hidden test（`test -s answer.txt`）は通らない。
        Fate::EmptyArtifact => fs::write(ws.join("src/answer.txt"), "").expect("artifact"),
        _ => fs::write(ws.join("src/answer.txt"), "b-c-fixture-answer\n").expect("artifact"),
    }
    fs::write(ws.join("src/public_check.sh"), PUBLIC_TEST).expect("public test");
    let ro = &body["routing_override"];
    let labels = body["labels"].clone();
    let selection = json!({
        "status": "selected", "harness_id": "claude_code",
        "source_id": "claude_oauth", "model_id": ROUTING_MODEL,
        "basis": {"route_decision": {"routing_resolved": {
            "mode": ro["mode"], "source": "task",
            "lower_policy": ro["lower_policy"], "evidence_mode": ro["evidence_mode"],
            "evidence_table_id": ro["evidence_table_id"], "online_update": ro["online_update"]
        }}}
    });
    let mut runs = vec![json!({
        "run_id": format!("R{id}"), "role": "worker", "is_retry": false,
        "finished_at": "2026-10-19T00:00:05Z",
        "usage": {"input_tokens": 400, "output_tokens": 60,
                  "cache_read_tokens": 0, "cache_creation_tokens": 0, "cost_usd": 0.004},
        "selection": selection.clone()
    })];
    if fate == Fate::PassWithRetry {
        // 同じ task 内の再実行 run（retry）。費用は捨てず同じ試行に加える。
        runs.push(json!({
            "run_id": format!("R{id}r"), "role": "worker", "is_retry": true,
            "finished_at": "2026-10-19T00:00:09Z",
            "usage": {"input_tokens": 300, "output_tokens": 40,
                      "cache_read_tokens": 0, "cache_creation_tokens": 0, "cost_usd": 0.003},
            "selection": selection
        }));
    }
    // 独立に観測された他 role の run（planner / reviewer / CoS / 子 run）。B0（ab）は全 role Sonnet、
    // B1/C1（bc）は Sonnet/Opus 候補のうち planner だけ Opus を選んだ。reviewer の 2 件目は選択前（model 等は欠測のまま）。
    // 使用量は 0 を観測した値として置く（既存の token・費用集計を動かさない）。
    let other_model = |role: &str| {
        if body["campaign"]["campaign_id"] == "armset-bc" && role == "planner" {
            "claude-opus-5-5"
        } else {
            ROUTING_MODEL
        }
    };
    let zero = json!({"input_tokens": 0, "output_tokens": 0,
        "cache_read_tokens": 0, "cache_creation_tokens": 0, "cost_usd": 0.0});
    for role in ["planner", "reviewer", "cos", "child-worker"] {
        let mut sel = selection.clone();
        sel["model_id"] = json!(other_model(role));
        runs.push(json!({
            "run_id": format!("R{id}-{role}"),
            // 子 run の role は API の RunRole（worker）。子であることは run_id で区別する。
            "role": if role == "child-worker" { "worker" } else { role }, "is_retry": false,
            "finished_at": "2026-10-19T00:00:06Z", "usage": zero.clone(), "selection": sel
        }));
    }
    let status = if fate == Fate::Fail { "failed" } else { "done" };
    json!({
        "task": {"id": id, "status": status, "labels": labels, "created_at": "2026-10-19T00:00:00Z"},
        "children": [],
        "runs": runs,
        "timers": {"now": "2026-10-19T00:00:10Z"}
    })
}

fn handle(st: &Mutex<State>, cfg: &Path, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
    let mut s = st.lock().expect("state");
    let route = path.split('?').next().unwrap_or("");
    match (method, route) {
        ("GET", p) => {
            s.gets.push(path.to_string());
            match p {
                "/api/v1/config" => (
                    200,
                    daemon_identity::daemon_config_body(cfg, MEASURE_RELEASE),
                ),
                "/api/v1/llm/models" => (
                    200,
                    json!({"items": [
                        {"source": "claude_oauth", "model_id": ROUTING_MODEL, "override": {"tier": "standard"}}
                    ]}),
                ),
                "/api/v1/llm/accounts" => (
                    200,
                    json!({"items": [{"source_id": "claude_oauth", "account_id": "lab",
                        "dir": cfg.parent().expect("dir").join("account"), "logged_in": true}]}),
                ),
                "/api/v1/projects/01ABC" => (
                    200,
                    json!({"project": {"id": "01ABC", "auto_integrate": false}, "repos": []}),
                ),
                "/api/v1/projects/01ABC/repos" => (
                    200,
                    json!({"items": [{"name": "scratch", "is_primary": true,
                        "location": {"kind": "local", "path": "/x"}}]}),
                ),
                "/api/v1/tasks" => {
                    let label = query_param(path, "label").unwrap_or_default();
                    let items: Vec<Value> = s
                        .tasks
                        .iter()
                        .filter(|(_, d)| {
                            d["task"]["labels"]
                                .as_array()
                                .is_some_and(|l| l.iter().any(|x| x == label.as_str()))
                        })
                        .map(|(id, _)| json!({"id": id}))
                        .collect();
                    (200, json!({"items": items}))
                }
                t if t.starts_with("/api/v1/tasks/") => {
                    let id = t.trim_start_matches("/api/v1/tasks/");
                    match s.tasks.get(id) {
                        Some(d) => (200, d.clone()),
                        None => (404, json!({})),
                    }
                }
                _ => {
                    s.other.push(format!("GET {path}"));
                    (404, json!({}))
                }
            }
        }
        ("POST", "/api/v1/tasks") => {
            let b: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
            let id = format!("01STUBTASK{:04}", s.posts.len() + 1);
            let detail = finish_task(&id, &b);
            s.tasks.insert(id.clone(), detail);
            s.posts.push(b);
            (201, json!({"id": id}))
        }
        _ => {
            s.other.push(format!("{method} {path}"));
            (404, json!({}))
        }
    }
}

impl Stub {
    /// port を先に確保する（測定 config の `[api] listen` に書くため）。config は `start` で渡す。
    fn bind() -> (TcpListener, u16) {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = l.local_addr().expect("addr").port();
        (l, port)
    }

    fn start(l: TcpListener, config: PathBuf) -> Stub {
        let addr = l.local_addr().expect("addr");
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, done) = mpsc::channel();
        let (st, sp) = (state.clone(), stop.clone());
        let handle = thread::spawn(move || {
            for conn in l.incoming() {
                if sp.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut c) = conn else { continue };
                let _ = c.set_read_timeout(Some(Duration::from_secs(10)));
                let Some((m, p, h, b)) = read_request(&mut c) else {
                    continue;
                };
                let auth_ok = h
                    .iter()
                    .any(|(k, v)| k == "authorization" && v == &format!("Bearer {TOKEN_VALUE}"));
                if !auth_ok {
                    st.lock().expect("state").bad_auth += 1;
                }
                let (code, v) = handle(&st, &config, &m, &p, &b);
                respond(&mut c, code, &v);
            }
            let _ = tx.send(());
        });
        Stub {
            url: format!("http://{addr}"),
            state,
            stop,
            addr,
            done,
            handle: Some(handle),
        }
    }

    /// 停止フラグを立て、自己接続で accept を起こし、終了の event を有界 timeout で待つ。
    fn finish(mut self) -> State {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        self.done
            .recv_timeout(Duration::from_secs(10))
            .expect("stub stopped within the bound");
        if let Some(h) = self.handle.take() {
            h.join().expect("join");
        }
        std::mem::take(&mut *self.state.lock().expect("state"))
    }
}

// ---- fixture（case・grader・入力・manifest）----

struct Fx {
    dir: tempfile::TempDir,
    which: &'static str,
    manifest: PathBuf,
    a_program: PathBuf,
    config: PathBuf,
    stub: Option<Stub>,
}

fn input() -> Input {
    let problems: Vec<Value> = CASES
        .iter()
        .map(|id| json!({"id":id,"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":format!("task.md を読み、課題を実施せよ。公開 case 定義は materials/{id}/case.json。"),"split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut m: Input = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    m.sha256 = manifest_hash(&m).expect("hash");
    m
}

/// case 2 件の定義・grader・外部入力を作る（実 final case には依存しない所有 fixture）。
fn write_cases(d: &Path) {
    let run_sh = "test -s answer.txt && echo 'test fixture_answer ... ok'\n";
    for id in CASES {
        let case_dir = d.join("cases/dev").join(id);
        for sub in ["kb", "materials"] {
            fs::create_dir_all(case_dir.join(sub)).expect("dir");
        }
        fs::write(
            case_dir.join("kb/CANARY.md"),
            format!("trial kb {CANARY_TRIAL}\n"),
        )
        .expect("kb");
        fs::write(case_dir.join("materials/task.md"), format!("task {id}\n")).expect("mat");
        // grade-case（hidden の評価 command は成果物 repo `src` に answer.txt があるかだけを見る）。
        let gdir = d.join("grade-cases/dev").join(id);
        fs::create_dir_all(gdir.join("hidden/.grader")).expect("hidden");
        fs::create_dir_all(gdir.join("pristine")).expect("pristine");
        fs::write(gdir.join("hidden/.grader/run.sh"), run_sh).expect("run.sh");
        fs::write(gdir.join("pristine/public_check.sh"), PUBLIC_TEST).expect("public");
        let gc = json!({
            "schema": "celeris.grade-case/1", "id": id, "split": "dev",
            "implementation": {
                "pristine_dir": "pristine", "repo": "src",
                "tests": [{"path": "public_check.sh", "sha256": sha(PUBLIC_TEST.as_bytes())}],
                "hidden_dir": "hidden",
                "hidden_tests": [{"path": ".grader/run.sh", "sha256": sha(run_sh.as_bytes())}],
                "command": ["sh", ".grader/run.sh"],
                "must_pass": ["fixture_answer"]
            },
            "experiment": null, "recompute": null, "review": null
        });
        let gtext = serde_json::to_string_pretty(&gc).expect("ser");
        fs::write(gdir.join(format!("{id}.json")), &gtext).expect("grade case");
        // 非公開（期待値・欠陥）。
        let pdir = d.join("grader-private/dev").join(id);
        fs::create_dir_all(&pdir).expect("private dir");
        let private = json!({
            "schema": "celeris.campaign-case-private/1", "case_id": id, "split": "dev",
            "hidden_command": "sh .grader/run.sh",
            "known_defects": [{"id": format!("{id}-d1"), "description": "fixture defect"}],
            "false_claim_traps": [{"id": format!("{id}-t1"), "claim": "c", "why_wrong": "w"}],
            "expected": {"answer": "x"}
        });
        let ptext = serde_json::to_string_pretty(&private).expect("ser");
        fs::write(pdir.join("private.json"), &ptext).expect("private");
        // 外部入力（--inputs-src が sha256 を照合して試行 workspace に置く）。
        let iname = format!("{id}-input.tgz");
        let ibytes = format!("fixture external input for {id}");
        fs::create_dir_all(d.join("inputs-src")).expect("inputs");
        fs::write(d.join("inputs-src").join(&iname), &ibytes).expect("input");
        let case = json!({
            "schema": "celeris.campaign-case/1", "id": id, "kind": "real", "split": "dev",
            "title": format!("fixture {id}"),
            "stages": ["implementation"],
            "inputs": [{"name": iname, "sha256": sha(ibytes.as_bytes()), "fetch": ["置くだけ"]}],
            "snapshot": {
                "repo_commit": "none (input-only fixture case)",
                "materials_sha256": hash_tree(&case_dir.join("materials")).expect("mat hash"),
                "kb_snapshot": hash_tree(&case_dir.join("kb")).expect("kb hash"),
            },
            "visible_command": "true",
            "evidence": ["raw_data"],
            "budget": {"max_tokens": 20000, "max_wall_secs": 600, "max_parallel": 1,
                "per_case": {"max_tokens": 20000, "max_wall_secs": 600},
                "per_arm": {"max_tokens": 20000, "max_wall_secs": 600}, "token_basis": "all",
                "max_disk_bytes": 1073741824},
            "private_ref": {"path": format!("dev/{id}/private.json"), "sha256": sha(ptext.as_bytes())},
            "grade_case_ref": {"path": format!("dev/{id}/{id}.json"), "sha256": sha(gtext.as_bytes())},
        });
        fs::write(
            case_dir.join("case.json"),
            serde_json::to_string_pretty(&case).expect("ser"),
        )
        .expect("case");
        // --materials-src は問題ごとの dir を束ねた木。
        let m = d.join("materials-src").join(id);
        fs::create_dir_all(&m).expect("m");
        fs::copy(case_dir.join("materials/task.md"), m.join("task.md")).expect("copy");
    }
    // 試行用 KB（各 case の kb と同じ内容。canary を含む）。
    fs::create_dir_all(d.join("kb-src")).expect("kb-src");
    fs::copy(
        d.join("cases/dev").join(CASES[0]).join("kb/CANARY.md"),
        d.join("kb-src/CANARY.md"),
    )
    .expect("kb copy");
}

fn cfg_of(
    id: ConfigId,
    harness: &str,
    version: &str,
    policy: &str,
    routing: Option<ArmRouting>,
) -> Config {
    Config {
        id,
        harness: harness.into(),
        harness_version: version.into(),
        model: ROUTING_MODEL.into(),
        model_version: ROUTING_MODEL.into(),
        policy_version: policy.into(),
        routing,
    }
}

/// repeats > 1 の集計試験は `-acct` 終わりの別 ID（stub が結末を割り当てる印）。
fn campaign_id_of(which: &str, repeats: u32) -> String {
    if repeats > 1 {
        format!("armset-{which}-acct")
    } else {
        format!("armset-{which}")
    }
}

fn manifest(
    d: &Path,
    which: &str,
    repeats: u32,
    a_version: &str,
    kb_hash: &str,
    mat_hash: &str,
) -> CampaignManifest {
    let i = input();
    // B1/C1: 共通の候補集合・harness・tier 対応表。根拠表は使わない（C は no_evidence_table）。
    let routing = |c: bool| ArmRouting {
        lower_policy: Some("mlsys-min-1".into()),
        online_update: Some(false),
        models: Some(vec![ROUTING_MODEL.into(), "claude-opus-5-5".into()]),
        run_harnesses: Some(vec!["claude_code".into()]),
        no_evidence_table: c.then_some(true),
        ..Default::default()
    };
    let b = cfg_of(
        ConfigId::B,
        "celeris",
        MEASURE_RELEASE,
        "fixed",
        Some(routing(false)),
    );
    let (configs, arms, pair, subset) = if which == "ab" {
        (
            vec![
                cfg_of(ConfigId::A, "claude-code", a_version, "ops-baseline", None),
                b,
            ],
            vec![ConfigId::A, ConfigId::B],
            (ConfigId::A, ConfigId::B),
            Vec::new(),
        )
    } else {
        (
            vec![
                b,
                cfg_of(
                    ConfigId::C,
                    "celeris",
                    MEASURE_RELEASE,
                    "jev-1",
                    Some(routing(true)),
                ),
            ],
            vec![ConfigId::B, ConfigId::C],
            (ConfigId::B, ConfigId::C),
            CASES.map(String::from).to_vec(),
        )
    };
    fs::write(d.join("input.json"), serde_json::to_vec(&i).expect("ser")).expect("input");
    let mut m = CampaignManifest {
        variant: None,
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: campaign_id_of(which, repeats),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: CASES.map(String::from).to_vec(),
        configs,
        fault_scenarios: Default::default(),
        experiment_sets: BTreeMap::from([(
            which.to_string(),
            ExperimentSet {
                arms,
                compare: vec![pair],
                fixed: SetFixed {
                    role_models: if which == "ab" {
                        ["planner", "reviewer", "cos", "worker"]
                            .map(|r| (r.to_string(), ROUTING_MODEL.to_string()))
                            .into()
                    } else {
                        Default::default()
                    },
                    // 固定証拠（B/C は方策・候補・harness、両方とも初期情報）と共有台帳を有効にして通常経路を通す。
                    policy: (which == "bc").then(|| "mlsys-min-1".to_string()),
                    models: (which == "bc")
                        .then(|| vec![ROUTING_MODEL.to_string(), "claude-opus-5-5".to_string()]),
                    harnesses: (which == "bc").then(|| vec!["claude_code".to_string()]),
                    initial_info: Some(mat_hash.to_string()),
                    approved_trials: Some(APPROVED_TRIALS),
                    shared_ledger: Some(ledger_dir(d).to_string_lossy().into_owned()),
                    ..Default::default()
                },
            },
        )]),
        selected_sets: vec![which.to_string()],
        primary: (ConfigId::A, ConfigId::B),
        secondary: Secondary {
            pair: (ConfigId::B, ConfigId::C),
            subset,
        },
        budget: Budget {
            token_basis: TokenBasis::All,
            max_tokens: 3_000_000,
            max_wall_secs: 36_000,
            max_disk_bytes: 1 << 36,
            max_parallel: 1,
            per_case: Cap {
                max_tokens: 300_000,
                max_wall_secs: 1_800,
            },
            per_arm: Cap {
                max_tokens: 1_300_000,
                max_wall_secs: 20_000,
            },
        },
        termination: Termination {
            trial_timeout_secs: 1800,
            max_attempts_per_trial: 1,
            stop_on_budget: true,
        },
        snapshot: SnapshotRef {
            repo_commit: "none (input-only fixture cases)".into(),
            materials_sha256: mat_hash.into(),
            kb_snapshot: kb_hash.into(),
        },
        order: Default::default(),
        seed: 0,
        repeats,
        dependencies: BTreeMap::new(),
        sha256: String::new(),
    };
    m.sha256 = m.compute_sha256().expect("hash");
    m
}

/// 承認単位の試行数上限（fixture。全計画の試行数より大きい）。
const APPROVED_TRIALS: u64 = 100;

/// 共有承認の全体台帳 dir（絶対 path）。
fn ledger_dir(d: &Path) -> PathBuf {
    d.join("global-ledger")
}

fn fixture(which: &'static str, repeats: u32) -> Fx {
    let dir = tempfile::tempdir().expect("dir");
    let d = dir.path();
    write_cases(d);
    // A の専用 fixture 実行器（LLM を起動しない）。版は実行から得る。呼出しの印は試行の state dir に残す。
    let a_program = d.join("bin").join("fxa");
    fs::create_dir_all(d.join("bin")).expect("bin");
    write_executable(
        &a_program,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{A_VERSION}'; exit 0; fi\n\
             mkdir -p src && printf 'a-fixture-answer\\n' > src/answer.txt\n\
             printf '# public acceptance check (fixture)\\n' > src/public_check.sh\n\
             : > \"${{CLAUDE_CONFIG_DIR:-.}}/fxa-called\"\n\
             printf '{{\"type\":\"result\",\"usage\":{{\"input_tokens\":120,\"output_tokens\":30,\"cache_read_input_tokens\":5,\"cache_creation_input_tokens\":0}},\"total_cost_usd\":0.002}}\\n'\n"
        ),
    );
    let kb_hash = hash_tree(&d.join("kb-src")).expect("kb");
    let mat_hash = hash_tree(&d.join("materials-src")).expect("mat");
    let m = manifest(d, which, repeats, A_VERSION, &kb_hash, &mat_hash);
    let manifest_path = d.join("campaign.json");
    fs::write(&manifest_path, serde_json::to_vec_pretty(&m).expect("ser")).expect("manifest");
    // 測定専用 daemon の config。`[api] listen` は所有 stub の動的 port。
    let (listener, port) = Stub::bind();
    let measure = preflight_setup::measure_config(d, "measure")
        .replace("127.0.0.1:7701", &format!("127.0.0.1:{port}"));
    let me = preflight_setup::write_configs(d, &measure, &preflight_setup::normal_config(d));
    fs::write(
        d.join("canaries.json"),
        json!({"forbidden":[CANARY_NORMAL, CANARY_GRADER, "CANARY-OTHER-TRIAL-55aa"],"required":[CANARY_TRIAL]}).to_string(),
    )
    .expect("canaries");
    fs::write(
        d.join("task-template.json"),
        json!({"project_id":"01ABC","acceptance":[{"type":"reviewer","text":"x"}]}).to_string(),
    )
    .expect("template");
    // A のアカウント dir（認証材料の置き場。中身は空の使い捨て）。
    fs::create_dir_all(d.join("account")).expect("account");
    fs::write(d.join("account/.credentials.json"), "{}").expect("cred");
    preflight_setup::write_cap_env(d);
    let stub = Stub::start(listener, me.config.clone());
    Fx {
        which,
        manifest: manifest_path,
        a_program,
        config: me.config,
        stub: Some(stub),
        dir,
    }
}

impl Fx {
    fn out(&self) -> PathBuf {
        self.dir.path().join("out")
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn url(&self) -> String {
        self.stub.as_ref().expect("stub").url.clone()
    }

    /// 全 subcommand が共有する引数（run と同じ値から preflight・期待 lock を作る）。
    fn common(&self) -> Vec<String> {
        self.common_with(&self.manifest)
    }

    fn common_with(&self, manifest: &Path) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        let mut add = |k: &str, p: &Path| {
            v.push(k.into());
            v.push(p.to_string_lossy().into_owned());
        };
        add("--manifest", manifest);
        add("--cases-root", &self.p("cases"));
        add("--grader-root", &self.p("grader-private"));
        add("--grade-case-root", &self.p("grade-cases"));
        add("--materials-src", &self.p("materials-src"));
        add("--kb-src", &self.p("kb-src"));
        add("--inputs-src", &self.p("inputs-src"));
        add("--measure-config", &self.config);
        add("--normal-config", &self.p("normal.toml"));
        add("--canaries", &self.p("canaries.json"));
        add("--task-template", &self.p("task-template.json"));
        add("--cap-env", &self.p("cap-env.json"));
        add("--out", &self.out());
        v.extend([
            "--daemon-pid".to_string(),
            DAEMON_PID.to_string(),
            "--api-url".into(),
            self.url(),
            "--token-env".into(),
            TOKEN_ENV.into(),
        ]);
        if self.which == "ab" {
            // A が有効なときだけ A の program・account を渡す。B/C のみでは渡さない。
            v.extend([
                "--a-program".to_string(),
                self.a_program.to_string_lossy().into_owned(),
                "--a-account".into(),
                "lab".into(),
            ]);
        }
        v
    }

    fn bin(&self, sub: &str, extra: &[&str]) -> Output {
        self.bin_with(&self.manifest, sub, extra)
    }

    fn bin_with(&self, manifest: &Path, sub: &str, extra: &[&str]) -> Output {
        let mut c = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"));
        c.arg(sub)
            .args(self.common_with(manifest))
            .args(extra)
            .arg("--global-ledger")
            .arg(ledger_dir(self.dir.path()));
        // token は子の環境にだけ渡す。Jev のキー・他の認証は渡さない。
        c.env(TOKEN_ENV, TOKEN_VALUE)
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("JEV_API_KEY");
        c.output().expect("run binary")
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn ok_json(o: &Output) -> Value {
    assert!(o.status.success(), "{}", text(o));
    assert!(
        !text(o).contains(TOKEN_VALUE),
        "the token value must never be printed"
    );
    serde_json::from_slice(&o.stdout).unwrap_or(Value::Null)
}

fn files_named(root: &Path, name: &str) -> usize {
    let mut n = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|f| f == name) {
                n += 1;
            }
        }
    }
    n
}

/// `which` = "ab"（A/B のみ）か "bc"（B/C のみ）。両セットで全 CLI 成功経路を通す。
fn flow(which: &'static str) {
    let mut f = fixture(which, 1);
    let (trials_total, a_calls, post_arms): (u64, usize, &[&str]) = if which == "ab" {
        (4, 2, &["B"])
    } else {
        (4, 0, &["B", "C"])
    };
    let absent = if which == "ab" { "C" } else { "A" };

    let v = ok_json(&f.bin("validate", &[]));
    assert!(v.is_null() || v.is_string());
    let dr = ok_json(&f.bin("dry-run", &[]));
    assert_eq!(dr["resources"]["total_trials"], trials_total);
    assert_eq!(dr["llm_calls_made"], 0);
    assert!(!dr.to_string().contains(&format!("\"config\":\"{absent}\"")));

    let pf = ok_json(&f.bin("preflight", &[]));
    assert_eq!(pf["ok"], true, "{pf}");
    assert_eq!(pf["paid_calls_made"], 0);
    // preflight が引いた A の program は、A 有効のときだけ。
    assert_eq!(pf["a_program"].is_null(), which == "bc", "{pf}");

    // env-lock: 実 fixture 実行器の内容 hash・実行から得た版と一致する。
    let el = f.bin("env-lock", &[]);
    assert!(el.status.success(), "{}", text(&el));
    let lock: Value =
        serde_json::from_slice(&fs::read(f.out().join("env-lock.json")).expect("env-lock"))
            .expect("json");
    if which == "ab" {
        assert_eq!(
            lock["a"]["program"]["sha256"],
            sha(&fs::read(&f.a_program).expect("fixture"))
        );
        assert_eq!(lock["a"]["program"]["version"], A_VERSION);
    } else {
        assert!(lock["a"].is_null(), "B/C のみは A の program を固定しない");
    }
    assert_eq!(lock["bc"]["release"], MEASURE_RELEASE);
    assert!(f.bin("env-lock", &["--verify"]).status.success());

    // run（実バイナリ --executor celeris → 通常 POST）。
    // A（fixture 実行器）は別 process で非同期に走るので、台帳が閉じるまで同じ out を再開して進める
    // （出来事 = 台帳の waiting が 0 になること。上限付きで、間に短い待ちだけ置く）。
    let mut run = ok_json(&f.bin("run", &["--executor", "celeris"]));
    for _ in 0..300 {
        if run["waiting"] == 0 && run["parallel_deferred"] == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(100));
        run = ok_json(&f.bin("run", &["--executor", "celeris"]));
    }
    assert_eq!(run["waiting"], 0, "ledger did not close: {run}");
    assert_eq!(run["executor_errors"], 0);
    assert_eq!(run["blocked"], 0);
    assert_eq!(run["budget_cancelled"], 0);
    assert_eq!(run["ledger"]["rows"], trials_total, "{run}");
    assert!(
        run["ledger"]["by_state"]
            .as_object()
            .is_some_and(|m| !m.contains_key("waiting") && !m.contains_key("failed")),
        "{run}"
    );
    // lock は実 fixture の内容 hash と一致する。
    let saved: Value =
        serde_json::from_slice(&fs::read(f.out().join("lock.json")).expect("lock")).expect("json");
    for (i, id) in CASES.iter().enumerate() {
        let gc = fs::read(f.p(&format!("grade-cases/dev/{id}/{id}.json"))).expect("grade case");
        let entry = saved["cases"]
            .as_array()
            .expect("cases")
            .iter()
            .find(|c| c["case_id"] == *id || c["id"] == *id)
            .unwrap_or_else(|| panic!("case {i} {id} in lock: {saved}"));
        assert_eq!(entry["grader"]["grade_case_sha256"], sha(&gc));
    }

    // watch: 同じ out を再開して settled。再投入も A の再呼出しも起きない。
    let w = ok_json(&f.bin(
        "watch",
        &[
            "--executor",
            "celeris",
            "--interval-secs",
            "1",
            "--max-iterations",
            "3",
        ],
    ));
    assert_eq!(w["watch"]["end"], "settled", "{w}");
    assert_eq!(w["executed"], 0, "{w}");
    assert_eq!(w["resumed_skipped"], trials_total, "{w}");

    let c = ok_json(&f.bin("check", &[]));
    assert_eq!(c["ok"], true, "{c}");
    let g = f.bin(
        "grade-out",
        &["--cases", f.p("grade-cases").to_str().expect("p")],
    );
    let g = ok_json(&g);
    assert_eq!(g["ok"], true, "{g}");
    let dest = f.p("export");
    let t = ok_json(&f.bin("export", &["--dest", dest.to_str().expect("p")]));
    assert_eq!(t["lock_verified"], true, "{t}");
    assert_eq!(t["complete"], true, "{t}");
    let cfgs: Vec<String> = t["configs"]
        .as_array()
        .expect("configs")
        .iter()
        .map(|c| c["config"].as_str().expect("config").to_string())
        .collect();
    assert!(!cfgs.contains(&absent.to_string()), "{cfgs:?}");
    assert_eq!(t["comparisons"].as_array().expect("cmp").len(), 1);
    for row in t["configs"].as_array().expect("rows") {
        assert_eq!(row["n"], 2, "{row}");
        assert_eq!(row["final_passed"], 2, "{row}");
        assert_eq!(row["failures"], 0, "{row}");
    }

    // 台帳・試行記録: 未使用 arm は 1 行も無い。
    let trials = fs::read_to_string(f.out().join("trials.jsonl")).expect("trials");
    assert_eq!(trials.lines().count() as u64, trials_total);
    assert!(!trials.contains(&format!("\"config\":\"{absent}\"")));

    // 観測 fixture: 各試行記録の observed は、stub が返した run の role・model・harness そのまま。
    // 宣言（manifest の role_models / 候補）から補完した値ではない。
    for line in trials.lines() {
        let rec: Value = serde_json::from_str(line).expect("trial");
        if rec["config"] == "A" {
            continue;
        }
        let obs = rec["observed"].as_array().expect("observed");
        let roles: Vec<&str> = obs
            .iter()
            .map(|o| o["role"].as_str().expect("role"))
            .collect();
        for r in ["worker", "planner", "reviewer", "cos"] {
            assert!(roles.contains(&r), "role {r} observed: {roles:?}");
        }
        for o in obs {
            let role = o["role"].as_str().expect("role");
            let want = if which == "bc" && role == "planner" {
                "claude-opus-5-5"
            } else {
                ROUTING_MODEL
            };
            assert_eq!(o["model"], want, "{o}");
            assert_eq!(o["harness"], "claude_code", "{o}");
        }
    }

    // A の fixture 実行器の呼出し数（試行の state dir に残る印）。B/C のみは state dir 自体が無い。
    assert_eq!(files_named(&f.out(), "fxa-called"), a_calls);
    if which == "bc" {
        assert!(!f.out().join("trial-state").exists());
        assert!(!f.out().join("harness-state").exists());
    }

    // stub の記録。
    let s = f.stub.take().expect("stub").finish();
    assert_eq!(s.bad_auth, 0, "every request carried the bearer token");
    assert!(s.other.is_empty(), "unexpected requests: {:?}", s.other);
    assert_eq!(
        s.posts.len(),
        post_arms.len() * CASES.len(),
        "{:?}",
        s.posts
    );
    for p in &s.posts {
        let arm = p["campaign"]["arm"].as_str().expect("arm");
        assert!(post_arms.contains(&arm), "POST for an unused arm {arm}");
        assert_eq!(p["campaign"]["campaign_id"], format!("armset-{which}"));
        assert_eq!(p["project_id"], "01ABC");
        assert!(p["campaign_idempotency_key"].is_string());
        // probe 用の task（能力 probe の分野）は無い。
        assert!(!p.to_string().contains("cap-probe"), "{p}");
    }
    for arm in post_arms {
        let n = s
            .posts
            .iter()
            .filter(|p| p["campaign"]["arm"] == *arm)
            .count();
        assert_eq!(n, CASES.len(), "arm {arm}");
    }
    if which == "ab" {
        // Jev(C) は無効・キーなし: C の POST も evidence table の指定も無い。
        assert!(
            s.posts
                .iter()
                .all(|p| p["routing_override"]["evidence_mode"] == "none")
        );
        assert!(!s.gets.iter().any(|g| g.contains("jev")));
    } else {
        // A の credential（アカウント一覧）は一度も要求しない。
        assert!(
            !s.gets.iter().any(|g| g.starts_with("/api/v1/llm/accounts")),
            "{:?}",
            s.gets
        );
        let c_posts: Vec<_> = s
            .posts
            .iter()
            .filter(|p| p["campaign"]["arm"] == "C")
            .collect();
        assert!(
            c_posts
                .iter()
                .all(|p| p["routing_override"]["evidence_mode"] == "none"
                    && p["routing_override"]["mode"] == "decision_assisted")
        );
    }
    // 通常の lookup（label 照会）・poll（task 詳細）・費用（usage）が API 経由で読まれた。
    assert!(s.gets.iter().any(|g| g.starts_with("/api/v1/tasks?label=")));
    assert!(
        s.gets
            .iter()
            .any(|g| g.starts_with("/api/v1/tasks/01STUBTASK"))
    );
    assert!(run["tokens_total"].as_u64().is_some_and(|t| t > 0), "{run}");
}

#[test]
fn ab_only_runs_through_the_real_cli_and_normal_post_stub() {
    flow("ab");
}

#[test]
fn bc_only_runs_through_the_real_cli_and_normal_post_stub() {
    flow("bc");
}

// ---- 複数 case × repetition・retry・採点・費用・再開（同じ planned_configs を入口から export まで）----

const REPEATS: u32 = 2;

/// 台帳が閉じるまで同じ out を再開する（出来事 = 台帳の waiting/parallel_deferred が 0。上限付き）。
fn run_until_closed(f: &Fx) -> Value {
    let mut run = ok_json(&f.bin("run", &["--executor", "celeris"]));
    for _ in 0..300 {
        if run["waiting"] == 0 && run["parallel_deferred"] == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(100));
        run = ok_json(&f.bin("run", &["--executor", "celeris"]));
    }
    assert_eq!(run["waiting"], 0, "ledger did not close: {run}");
    run
}

fn config_row<'a>(t: &'a Value, cfg: &str) -> &'a Value {
    t["configs"]
        .as_array()
        .expect("configs")
        .iter()
        .find(|c| c["config"] == cfg)
        .unwrap_or_else(|| panic!("config {cfg} in export: {t}"))
}

/// 選択集合を別の集合へ替えた manifest（同じ campaign_id・同じ out）。
fn with_other_set(f: &Fx, to: &str) -> PathBuf {
    let mut m: CampaignManifest =
        serde_json::from_slice(&fs::read(&f.manifest).expect("read")).expect("manifest");
    // 有効 arm が変わる、それ以外は妥当な manifest（preflight の別の検査で落ちない）にする。
    let b = m
        .configs
        .iter()
        .find(|c| c.id == ConfigId::B)
        .expect("B")
        .clone();
    let (arms, pair, other_cfg) = if to == "bc" {
        let mut routing = b.routing.clone().expect("B routing");
        routing.no_evidence_table = Some(true);
        (
            vec![ConfigId::B, ConfigId::C],
            (ConfigId::B, ConfigId::C),
            cfg_of(
                ConfigId::C,
                "celeris",
                MEASURE_RELEASE,
                "jev-1",
                Some(routing),
            ),
        )
    } else {
        (
            vec![ConfigId::A, ConfigId::B],
            (ConfigId::A, ConfigId::B),
            cfg_of(ConfigId::A, "claude-code", A_VERSION, "ops-baseline", None),
        )
    };
    m.configs = vec![b, other_cfg];
    m.configs.sort_by_key(|c| c.id);
    // B/C のみは事前登録 subset が要り、A/B のみは空でなければならない。
    m.secondary.subset = if to == "bc" {
        CASES.map(String::from).to_vec()
    } else {
        Vec::new()
    };
    m.experiment_sets.insert(
        to.to_string(),
        ExperimentSet {
            arms,
            compare: vec![pair],
            fixed: SetFixed {
                // B0 は全役割の model を宣言する（A/B のとき）。
                role_models: if to == "ab" {
                    ["planner", "reviewer", "cos", "worker"]
                        .map(|r| (r.to_string(), ROUTING_MODEL.to_string()))
                        .into()
                } else {
                    Default::default()
                },
                // 共有承認の台帳は全セットで同じ宣言にする（違えば lock の gate より前に拒否される）。
                shared_ledger: Some(ledger_dir(f.dir.path()).to_string_lossy().into_owned()),
                ..Default::default()
            },
        },
    );
    m.selected_sets = vec![to.to_string()];
    m.sha256 = m.compute_sha256().expect("hash");
    let path = f.p("campaign-other-set.json");
    fs::write(&path, serde_json::to_vec_pretty(&m).expect("ser")).expect("write");
    path
}

/// 選択集合は同じで、固定条件（role の model）だけを変えた manifest。
fn with_changed_fixed(f: &Fx) -> PathBuf {
    let mut m: CampaignManifest =
        serde_json::from_slice(&fs::read(&f.manifest).expect("read")).expect("manifest");
    for s in m.experiment_sets.values_mut() {
        s.fixed.success = Some("grader-v2".into());
    }
    m.sha256 = m.compute_sha256().expect("hash");
    let path = f.p("campaign-fixed-changed.json");
    fs::write(&path, serde_json::to_vec_pretty(&m).expect("ser")).expect("write");
    path
}

fn trial_keys(out: &Path) -> Vec<(String, String, u64, u64)> {
    let mut v: Vec<_> = fs::read_to_string(out.join("trials.jsonl"))
        .expect("trials")
        .lines()
        .map(|l| {
            let r: Value = serde_json::from_str(l).expect("json");
            (
                r["config"].as_str().expect("config").to_string(),
                r["problem_id"].as_str().expect("problem").to_string(),
                r["rep"].as_u64().expect("rep"),
                r["trial_no"].as_u64().expect("trial_no"),
            )
        })
        .collect();
    v.sort();
    v
}

/// 全 case × 有効 arm × 複数 repetition を通し、retry run・失敗・未採点・採点不合格を含めて
/// 分母（試行 = case × repetition。retry run は数えない）・費用・選択比較を実 CLI 出力から確かめる。
fn accounting(which: &'static str) {
    let mut f = fixture(which, REPEATS);
    let (arms, other): (&[&str], &str) = if which == "ab" {
        (&["A", "B"], "bc")
    } else {
        (&["B", "C"], "ab")
    };
    let absent = if which == "ab" { "C" } else { "A" };
    let posts_per_set = (CASES.len() as u32 * REPEATS) as usize;
    let b_posts = if which == "ab" { 1 } else { 2 };

    let dr = ok_json(&f.bin("dry-run", &[]));
    // 計画 = 有効 arm 数 × case × repetition（retry や未使用 arm は入らない）。
    assert_eq!(dr["resources"]["total_trials"], arms.len() * posts_per_set);
    let pf = ok_json(&f.bin("preflight", &[]));
    assert_eq!(pf["ok"], true, "{pf}");
    assert!(f.bin("env-lock", &[]).status.success());

    let run = run_until_closed(&f);
    assert_eq!(run["executor_errors"], 0, "{run}");
    assert_eq!(run["blocked"], 0, "{run}");
    assert_eq!(run["budget_cancelled"], 0, "{run}");
    let rows = (arms.len() * posts_per_set) as u64;
    assert_eq!(run["ledger"]["rows"], rows, "{run}");

    // trial identity: (config, problem, rep, trial_no=1) が計画どおり 1 回ずつ。
    let mut want: Vec<(String, String, u64, u64)> = Vec::new();
    for a in arms {
        for c in CASES {
            for rep in 1..=u64::from(REPEATS) {
                want.push((a.to_string(), c.to_string(), rep, 1));
            }
        }
    }
    want.sort();
    assert_eq!(trial_keys(&f.out()), want);

    // 採点前の export: 実行器 passed は未採点として分母に残り、成功に数えない。
    let pre = f.p("export-pre");
    let t = ok_json(&f.bin("export", &["--dest", pre.to_str().expect("p")]));
    assert_eq!(t["complete"], false, "{t}");
    let b = config_row(&t, "B");
    assert_eq!((b["n"].as_u64(), b["passed"].as_u64()), (Some(4), Some(3)));
    assert_eq!(b["ungraded"], 3, "{b}");
    assert_eq!(b["final_passed"], 0, "{b}");

    let g = ok_json(&f.bin(
        "grade-out",
        &["--cases", f.p("grade-cases").to_str().expect("p")],
    ));
    assert_eq!(g["ok"], true, "{g}");
    let c = ok_json(&f.bin("check", &[]));
    assert_eq!(c["ok"], true, "{c}");
    let dest = f.p("export");
    let t = ok_json(&f.bin("export", &["--dest", dest.to_str().expect("p")]));
    assert_eq!(t["lock_verified"], true, "{t}");
    assert_eq!(t["complete"], true, "{t}");

    // 選択した arm の行だけ。選択比較は 1 組で、その 2 arm だけ。
    let names: Vec<&str> = t["configs"]
        .as_array()
        .expect("configs")
        .iter()
        .map(|c| c["config"].as_str().expect("config"))
        .collect();
    assert_eq!(names, arms, "{t}");
    let cmps = t["comparisons"].as_array().expect("cmp");
    assert_eq!(cmps.len(), 1, "{t}");
    let cmp = cmps[0].to_string();
    assert!(
        cmp.contains(&format!("\"{}\"", arms[0])) && cmp.contains(&format!("\"{}\"", arms[1])),
        "{cmp}"
    );
    assert!(!cmp.contains(&format!("\"{absent}\"")), "{cmp}");

    // B: alpha rep1 は retry run 付きで成功、beta rep2 は task failed。分母は 4（retry run を数えない）。
    // 費用は retry run も失敗した試行の run も落とさない。
    let b = config_row(&t, "B");
    assert_eq!(b["n"], 4, "{b}");
    assert_eq!(b["passed"], 3, "{b}");
    assert_eq!(b["failed"], 1, "{b}");
    assert_eq!(b["final_passed"], 3, "{b}");
    assert_eq!(b["failures"], 1, "{b}");
    assert_eq!(b["ungraded"], 0, "{b}");
    assert_eq!(b["grader_failed"], 0, "{b}");
    assert_eq!(b["input_tokens_sum"], 400 * 3 + 700, "{b}");
    assert_eq!(b["output_tokens_sum"], 60 * 3 + 100, "{b}");
    assert_eq!(b["tokens_sum"], 460 * 3 + 800, "{b}");
    assert_eq!(b["cost_micro_usd_sum"], 4_000 * 3 + 7_000, "{b}");
    if which == "ab" {
        // A は fixture 実行器 4 試行（case × repetition）。LLM は起動しない。
        let a = config_row(&t, "A");
        assert_eq!(a["n"], 4, "{a}");
        assert_eq!(a["final_passed"], 4, "{a}");
        assert_eq!(a["failures"], 0, "{a}");
        assert_eq!(a["tokens_sum"], 155 * 4, "{a}");
        assert_eq!(a["cost_micro_usd_sum"], 2_000 * 4, "{a}");
        assert_eq!(files_named(&f.out(), "fxa-called"), 4);
    } else {
        // C: alpha rep2 は done だが採点不合格、beta rep1 は task failed。
        let c = config_row(&t, "C");
        assert_eq!(c["n"], 4, "{c}");
        assert_eq!(c["passed"], 3, "{c}");
        assert_eq!(c["failed"], 1, "{c}");
        assert_eq!(c["grader_failed"], 1, "{c}");
        assert_eq!(c["final_passed"], 2, "{c}");
        assert_eq!(c["failures"], 2, "{c}");
        assert_eq!(c["tokens_sum"], 460 * 4, "{c}");
        assert_eq!(c["cost_micro_usd_sum"], 4_000 * 4, "{c}");
        assert_eq!(files_named(&f.out(), "fxa-called"), 0);
    }

    // ---- 同じ out の再開: 同じ trial identity のまま、再投入も A の再起動も起きない ----
    let trials_before = fs::read(f.out().join("trials.jsonl")).expect("trials");
    let posts_before = f
        .stub
        .as_ref()
        .expect("stub")
        .state
        .lock()
        .expect("s")
        .posts
        .len();
    assert_eq!(posts_before, b_posts * posts_per_set);
    let again = ok_json(&f.bin("run", &["--executor", "celeris"]));
    assert_eq!(again["executed"], 0, "{again}");
    assert_eq!(again["resumed_skipped"], rows, "{again}");
    let w = ok_json(&f.bin(
        "watch",
        &[
            "--executor",
            "celeris",
            "--interval-secs",
            "1",
            "--max-iterations",
            "3",
        ],
    ));
    assert_eq!(w["watch"]["end"], "settled", "{w}");
    assert_eq!(w["executed"], 0, "{w}");
    assert_eq!(w["resumed_skipped"], rows, "{w}");
    assert_eq!(
        fs::read(f.out().join("trials.jsonl")).expect("trials"),
        trials_before,
        "resume must not touch the recorded trials"
    );
    assert_eq!(trial_keys(&f.out()), want);
    assert_eq!(
        f.stub
            .as_ref()
            .expect("stub")
            .state
            .lock()
            .expect("s")
            .posts
            .len(),
        posts_before,
        "resume must not POST again"
    );
    assert_eq!(
        files_named(&f.out(), "fxa-called"),
        if which == "ab" { 4 } else { 0 },
        "A must not be restarted"
    );

    // ---- 選択集合・固定条件を変えた再開は、同じ out では拒否（POST も記録の変更も無し）----
    // 未使用 arm（B/C のみの A・A/B のみの Jev）の要求は、正しい入口の間は 0。
    // 拒否される試し（A を使う別 manifest）の要求とは分けて数える。
    let gets_valid = f
        .stub
        .as_ref()
        .expect("stub")
        .state
        .lock()
        .expect("s")
        .gets
        .clone();
    assert!(!gets_valid.iter().any(|g| g.contains("jev")));
    if which == "bc" {
        assert!(
            !gets_valid
                .iter()
                .any(|g| g.starts_with("/api/v1/llm/accounts")),
            "{gets_valid:?}"
        );
    }
    for (what, m) in [
        ("selected set", with_other_set(&f, other)),
        ("fixed condition", with_changed_fixed(&f)),
    ] {
        // B/C のみから A/B へ替える入力には、A を使う引数も付ける（引数不足で落ちた拒否にしない）。
        let a_program = f.a_program.to_string_lossy().into_owned();
        let mut extra = vec!["--executor", "celeris"];
        if which == "bc" {
            extra.extend(["--a-program", &a_program, "--a-account", "lab"]);
        }
        let o = f.bin_with(&m, "run", &extra);
        assert!(
            !o.status.success(),
            "{what} change must be rejected: {}",
            text(&o)
        );
        assert!(!text(&o).contains(TOKEN_VALUE));
        assert!(
            text(&o).contains("saved lock"),
            "{what}: rejected by the saved-lock gate, not by an unrelated error: {}",
            text(&o)
        );
    }
    assert_eq!(
        fs::read(f.out().join("trials.jsonl")).expect("trials"),
        trials_before
    );

    let s = f.stub.take().expect("stub").finish();
    assert_eq!(s.bad_auth, 0);
    assert!(s.other.is_empty(), "{:?}", s.other);
    assert_eq!(
        s.posts.len(),
        posts_before,
        "rejected resumes must not POST"
    );
    let mut keys: Vec<String> = s
        .posts
        .iter()
        .map(|p| {
            p["campaign_idempotency_key"]
                .as_str()
                .expect("key")
                .to_string()
        })
        .collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), s.posts.len(), "one POST per trial identity");
    for p in &s.posts {
        let arm = p["campaign"]["arm"].as_str().expect("arm");
        assert_ne!(arm, absent);
        assert_eq!(p["campaign"]["campaign_id"], campaign_id_of(which, REPEATS));
    }
}

#[test]
fn ab_only_counts_repetitions_retries_grading_and_cost_and_resumes_without_reposting() {
    accounting("ab");
}

#[test]
fn bc_only_counts_repetitions_retries_grading_and_cost_and_resumes_without_reposting() {
    accounting("bc");
}

// ---- 有効 arm の要求が発生する対照ケースで、無効 arm の probe / credential / 準備 / 実行が 0 件 ----
// 所有 fixture（偽 Host）だけで、userns・実 daemon・LLM は要らない。case は必要能力を宣言し、
// A の sandbox probe と B/C の daemon worker probe が有効 arm には実際に発生する（対照）。

mod unused_arm_requests {
    use std::cell::RefCell;

    use celeris_campaign::{
        cap_probe::{ProbeRunner, WorkerProbeRun, WorkerProbeSpec},
        preflight::{Host, ResolvedProgram, check},
    };

    use super::*;
    use crate::preflight_setup::{OkRunner, ok_worker_run, setup};

    /// Host への要求を全部数える。秘密値は持たず、名前・path・harness だけを記録する。
    #[derive(Default)]
    struct CountingHost {
        env_names: RefCell<Vec<String>>,
        resolves: RefCell<Vec<String>>,
        api_gets: RefCell<Vec<String>>,
        sandbox_checks: RefCell<u32>,
        a_probe_runners: RefCell<u32>,
        worker_probe_harnesses: RefCell<Vec<String>>,
    }

    impl Host for CountingHost {
        fn env_nonempty(&self, name: &str) -> bool {
            self.env_names.borrow_mut().push(name.to_string());
            true
        }
        fn resolve_program(&self, p: &Path) -> Result<ResolvedProgram, String> {
            self.resolves.borrow_mut().push(p.display().to_string());
            Ok(ResolvedProgram {
                path: p.into(),
                version: A_VERSION.into(),
            })
        }
        fn api_get(&self, _: &str, _: &str, path: &str) -> Result<Value, String> {
            self.api_gets.borrow_mut().push(path.to_string());
            match path {
                "/api/v1/config" => Ok(crate::daemon_identity::matching_daemon_body()),
                "/api/v1/projects/01ABC" => {
                    Ok(json!({"project":{"id":"01ABC","auto_integrate":false},"repos":[]}))
                }
                "/api/v1/projects/01ABC/repos" => Ok(json!({"items":[
                    {"name":"scratch","is_primary":true,"location":{"kind":"local","path":"/x"}}]})),
                "/api/v1/llm/accounts" => {
                    static D: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
                    let d = D.get_or_init(|| tempfile::tempdir().expect("dir")).path();
                    Ok(json!({"items":[
                        {"source_id":"claude_oauth","account_id":"lab","dir":d,"logged_in":true}]}))
                }
                _ => Err(format!("GET {path}: not served by this fixture")),
            }
        }
        fn sandbox_usable(&self) -> Result<(), String> {
            *self.sandbox_checks.borrow_mut() += 1;
            Ok(())
        }
        fn probe_daemon_worker(
            &self,
            _: &str,
            _: &str,
            spec: &WorkerProbeSpec,
        ) -> Result<WorkerProbeRun, String> {
            self.worker_probe_harnesses
                .borrow_mut()
                .push(spec.harness.clone());
            Ok(ok_worker_run(spec))
        }
        fn probe_runner(&self) -> Box<dyn ProbeRunner> {
            *self.a_probe_runners.borrow_mut() += 1;
            Box::new(OkRunner)
        }
    }

    /// 全 arm の final を、`arms` だけを持つ実験セット 1 つ（`compare` を比較組）に直す。未使用 arm の config は持たない。
    fn select(inp: &mut celeris_campaign::preflight::Inputs, arms: &[&str], compare: (&str, &str)) {
        let mut v: Value =
            serde_json::from_slice(&fs::read(&inp.manifest_path).expect("final")).expect("json");
        v["configs"]
            .as_array_mut()
            .expect("configs")
            .retain(|c| arms.contains(&c["id"].as_str().expect("id")));
        // B1/C1: 共通の候補集合・harness。C は根拠表を使わない。B0（A/B）は全 role が Sonnet。
        for c in v["configs"].as_array_mut().expect("configs") {
            let is_c = c["id"] == "C";
            if let Some(r) = c.get_mut("routing") {
                r["models"] = json!([ROUTING_MODEL, "claude-opus-5-5"]);
                if is_c {
                    r.as_object_mut()
                        .expect("routing")
                        .remove("evidence_table_id");
                    r["no_evidence_table"] = json!(true);
                }
            }
        }
        let role_models = if arms.contains(&"A") {
            json!({"planner": ROUTING_MODEL, "reviewer": ROUTING_MODEL, "cos": ROUTING_MODEL, "worker": ROUTING_MODEL})
        } else {
            json!({})
        };
        if arms.contains(&"A") {
            v["secondary"]["subset"] = json!([]);
        }
        v["experiment_sets"] = json!({"sel": {"arms": arms, "compare": [[compare.0, compare.1]], "fixed": {"role_models": role_models}}});
        v["selected_sets"] = json!(["sel"]);
        fs::write(&inp.manifest_path, v.to_string()).expect("write");
    }

    fn run_check(arms: &[&str], compare: (&str, &str)) -> (CountingHost, Vec<String>) {
        let mut s = setup();
        select(&mut s.inp, arms, compare);
        let host = CountingHost::default();
        let r = check(&s.inp, &host);
        let miss = r
            .missing
            .iter()
            .map(|m| format!("{}: {}", m.check, m.detail))
            .collect();
        (host, miss)
    }

    /// 秘密値でなく名前だけを数える。token の環境変数名以外は、どの arm の credential も問い合わせない。
    fn only_token_env(h: &CountingHost) {
        assert!(
            h.env_names
                .borrow()
                .iter()
                .all(|n| n == "CELERIS_CAMPAIGN_TOKEN"),
            "{:?}",
            h.env_names.borrow()
        );
    }

    /// A/B のみ: A の sandbox probe と B の daemon worker probe は発生する（対照）。C（Jev）は probe も credential も 0。
    #[test]
    fn ab_only_probes_a_and_b_and_never_touches_jev() {
        let (h, miss) = run_check(&["A", "B"], ("A", "B"));
        assert!(miss.is_empty(), "{miss:?}");
        // 対照: 有効 arm の要求は実際に発生した。
        assert_eq!(*h.sandbox_checks.borrow(), 1);
        assert_eq!(*h.a_probe_runners.borrow(), 1, "A sandbox probe ran");
        assert_eq!(h.resolves.borrow().len(), 1, "A program resolved once");
        assert!(
            h.api_gets
                .borrow()
                .iter()
                .any(|g| g == "/api/v1/llm/accounts")
        );
        // B の worker probe だけ（case 2 件 × B 1 arm）。C の分は無い。
        assert_eq!(
            *h.worker_probe_harnesses.borrow(),
            ["claude-code", "claude-code"]
        );
        // C / Jev: 根拠表・Jev の API・Jev の credential の要求は 0。
        let gets = h.api_gets.borrow();
        assert!(
            gets.iter()
                .all(|g| !g.contains("jev") && !g.contains("evidence")),
            "{gets:?}"
        );
        only_token_env(&h);
    }

    /// B/C のみ: B/C の daemon worker probe は発生する（対照）。A の program/account/準備/sandbox probe は 0。
    #[test]
    fn bc_only_probes_b_and_c_and_never_prepares_a() {
        let (h, miss) = run_check(&["B", "C"], ("B", "C"));
        assert!(miss.is_empty(), "{miss:?}");
        // 対照: case 2 件 × B/C 2 arm の worker probe が daemon へ出た。
        assert_eq!(h.worker_probe_harnesses.borrow().len(), 4);
        assert!(h.api_gets.borrow().iter().any(|g| g == "/api/v1/config"));
        // A: resolve / version / account / 準備（sandbox）/ sandbox probe の全てが 0。
        assert!(h.resolves.borrow().is_empty(), "{:?}", h.resolves.borrow());
        assert_eq!(*h.sandbox_checks.borrow(), 0);
        assert_eq!(*h.a_probe_runners.borrow(), 0);
        assert!(
            h.api_gets
                .borrow()
                .iter()
                .all(|g| !g.starts_with("/api/v1/llm/accounts")),
            "{:?}",
            h.api_gets.borrow()
        );
        only_token_env(&h);
    }
}
