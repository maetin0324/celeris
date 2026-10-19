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

/// POST 本文から、B/C の worker が作る成果物（`src/answer.txt`）を試行 workspace に置き、完了した task の詳細を作る。
fn finish_task(id: &str, body: &Value) -> Value {
    let ws = PathBuf::from(body["workspace"].as_str().expect("workspace"));
    fs::create_dir_all(ws.join("src")).expect("src");
    fs::write(ws.join("src/answer.txt"), "b-c-fixture-answer\n").expect("artifact");
    fs::write(ws.join("src/public_check.sh"), PUBLIC_TEST).expect("public test");
    let ro = &body["routing_override"];
    let labels = body["labels"].clone();
    json!({
        "task": {"id": id, "status": "done", "labels": labels, "created_at": "2026-10-19T00:00:00Z"},
        "children": [],
        "runs": [{
            "run_id": format!("R{id}"),
            "role": "worker",
            "finished_at": "2026-10-19T00:00:05Z",
            "usage": {"input_tokens": 400, "output_tokens": 60,
                      "cache_read_tokens": 0, "cache_creation_tokens": 0, "cost_usd": 0.004},
            "selection": {
                "status": "selected", "harness_id": "claude_code",
                "source_id": "claude_oauth", "model_id": ROUTING_MODEL,
                "basis": {"route_decision": {"routing_resolved": {
                    "mode": ro["mode"], "source": "task",
                    "lower_policy": ro["lower_policy"], "evidence_mode": ro["evidence_mode"],
                    "evidence_table_id": ro["evidence_table_id"], "online_update": ro["online_update"]
                }}}
            }
        }],
        "timers": {"now": "2026-10-19T00:00:05Z"}
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
        .map(|id| json!({"id":id,"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
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

fn manifest(
    d: &Path,
    which: &str,
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
        schema: celeris_campaign::CAMPAIGN_SCHEMA.into(),
        campaign_id: format!("armset-{which}"),
        version: "v1".into(),
        split: Split::Dev,
        input: InputRef {
            path: "input.json".into(),
            sha256: i.sha256.clone(),
        },
        problems: CASES.map(String::from).to_vec(),
        configs,
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
        repeats: 1,
        dependencies: BTreeMap::new(),
        sha256: String::new(),
    };
    m.sha256 = m.compute_sha256().expect("hash");
    m
}

fn fixture(which: &'static str) -> Fx {
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
    let m = manifest(d, which, A_VERSION, &kb_hash, &mat_hash);
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
        let mut v: Vec<String> = Vec::new();
        let mut add = |k: &str, p: &Path| {
            v.push(k.into());
            v.push(p.to_string_lossy().into_owned());
        };
        add("--manifest", &self.manifest);
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
        let mut c = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"));
        c.arg(sub).args(self.common()).args(extra);
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
    let mut f = fixture(which);
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
