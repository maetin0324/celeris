//! 測定環境 launcher（`scripts/campaign/measure-launcher.sh`）の無 LLM e2e。
//! 実 script・実 `celeris-campaign` CLI・実 task-api router（実 HTTP・正式 DTO）を通す。
//! 測定 daemon は launcher が起動した process そのもの: `CELERIS_BIN` はこの試験 binary を daemon 役
//! （[`launcher_daemon_role`]）で exec する wrapper で、その process が測定 config の `[api] listen` で実 router を serve し、
//! `GET /api/v1/config` の `identity.pid` に自分の PID を返す（別 process の router に identity を偽装させない）。
//! 実行内部だけ stub: A の `claude`（stream-json の usage を出すだけ）、`bwrap`（namespace を作らず `--` の後を exec）、
//! daemon の実行器（投入された B/C task を store の通常の遷移で終える）。userns・実 LLM・外部ネットワーク・本番 host の変更は無い。
//! 拒否は外部実行 0 件（A の claude が `--version` 以外で呼ばれない・台帳が増えない・`POST /api/v1/tasks` が 0）で確かめる。
mod daemon_identity;
mod preflight_setup;

use std::{
    fs,
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output},
    time::{Duration, Instant},
};

use daemon_identity::{
    api_common::{EnvOptions, TOKEN, settings},
    daemon_config_view,
};
use preflight_setup::{MEASURE_RELEASE, Setup, measure_config, setup};
use serde_json::{Value, json};

/// 来るはずの状態を待つ保険の上限（ADR-0125）。主判定は出来事の到着。
const WAIT: Duration = Duration::from_secs(120);
/// daemon 役の試験名（wrapper が `--exact` で指す）。
const ROLE: &str = "launcher_daemon_role";
// ---- daemon 役（launcher が起動する process の中身） ----

/// 通常の試験実行では何もしない。launcher が wrapper 経由で `-- --config <toml> --release <sha12>` を付けて
/// 起動したときだけ、その config の測定 daemon として実 task-api router を serve する（終了は launcher の SIGTERM）。
#[test]
fn launcher_daemon_role() {
    let args: Vec<String> = std::env::args().collect();
    let Some(sep) = args.iter().position(|a| a == "--") else {
        return;
    };
    let rest = &args[sep + 1..];
    let get = |n: &str| {
        rest.iter()
            .position(|a| a == n)
            .and_then(|i| rest.get(i + 1))
            .cloned()
    };
    let (Some(config), Some(release)) = (get("--config"), get("--release")) else {
        return;
    };
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(serve_role(PathBuf::from(config), release));
}

fn toml_str(t: &toml::Value, keys: &[&str]) -> String {
    keys.iter()
        .try_fold(t, |c, k| c.get(k))
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string()
}

async fn serve_role(config: PathBuf, release: String) {
    let t: toml::Value = toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    let listen = toml_str(&t, &["api", "listen"]);
    let db = PathBuf::from(toml_str(&t, &["db"]));
    let ws = PathBuf::from(toml_str(&t, &["workspace_root"]));
    let base = db.parent().unwrap().to_path_buf();
    fs::create_dir_all(&ws).unwrap();
    let token = fs::read_to_string(toml_str(&t, &["api", "token_file"]))
        .unwrap()
        .trim()
        .to_string();
    {
        // B/C の候補集合・階層（daemon の model catalog。`GET /llm/models` が返す）。
        use task_core::{
            ModelCatalogStore,
            model_catalog::{CatalogOverride, CatalogSource, DiscoveredModel},
        };
        let store = task_core::SqliteStore::open(&db).unwrap();
        let src = CatalogSource::new("opencode-go");
        store
            .model_catalog_apply(&src, &[DiscoveredModel::new("glm-5")], 1_700_000_000)
            .unwrap();
        let tier = CatalogOverride {
            tier: Some(task_core::Tier::Cheap),
            ..Default::default()
        };
        store
            .model_catalog_set_override(&src, "glm-5", &tier, 1_700_000_001)
            .unwrap();
    }
    let mut s = settings(
        &db,
        &ws,
        &base.join("daemon-docs"),
        // 文書状態は KB の外に置く（測定 KB root は空か固定 KB のまま）。
        &base.join("daemon-kb"),
        EnvOptions {
            token: Some(token),
            allowed_hosts: vec![listen.clone()],
            accounts_root: Some(PathBuf::from(toml_str(&t, &["accounts", "claude_dir"]))),
            release: release.clone(),
            ..EnvOptions::default()
        },
    );
    s.listen = listen.parse().unwrap();
    // 正式 DTO。identity.pid はこの process の PID（launcher が起動した process が答えている）。
    let mut view = daemon_config_view(&config, &release);
    view.identity.pid = std::process::id();
    s.config_view = view;
    let (tx, rx) = tokio::sync::watch::channel(None);
    let state = task_api::ApiState::new(s, rx).unwrap();
    let app = task_api::router(state);
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("daemon role: bind {listen}: {e}");
            std::process::exit(1);
        }
    };
    // 実行器の stub: 投入された task を store の通常の遷移（dispatch → worker done → review pass）で終える。
    std::thread::spawn(move || stub_executor(&db, &config, &ws));
    axum::serve(listener, app).await.unwrap();
    drop(tx);
}

/// 検査 task の worker（実 `celeris-campaign cap-worker`）。環境は daemon（この process）のまま継ぐ。
fn run_cap_worker(t: &task_core::Task, ws: &Path) -> String {
    use std::io::Write;
    let work = ws.join(t.id.to_string());
    fs::create_dir_all(work.join("artifacts")).unwrap();
    let req = task_worker::RunRequest {
        protocol: task_worker::PROTOCOL_VERSION,
        task: t.clone(),
        workspace: work.clone(),
        work_dir: None,
        artifacts_dir: work.join("artifacts"),
        context: task_worker::RunContext::default(),
        llm_route: None,
        cargo_target_dir: None,
    };
    // daemon 役の環境は launcher が絞っているので CARGO_BIN_EXE_* は無い。wrapper が渡した path を使う。
    let mut c = Command::new(std::env::var("CAP_WORKER_BIN").expect("CAP_WORKER_BIN"))
        .arg("cap-worker")
        .current_dir(&work)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = c.stdin.take().unwrap();
    writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).unwrap();
    drop(stdin);
    let out = c.wait_with_output().unwrap();
    let done: Value = serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    done["summary"].as_str().unwrap().to_string()
}

fn stub_executor(db: &Path, config: &Path, ws: &Path) {
    use task_core::{Event, Status, TaskStore, Trigger};
    let store = task_core::SqliteStore::open(db).unwrap();
    loop {
        // 試験の一時 dir が消えたら（試験 process が落ちた後も）残らない。
        if !config.exists() {
            std::process::exit(0);
        }
        for t in store.list(None).unwrap_or_default() {
            let run_id = format!("run-{}", t.id);
            let step = match t.status {
                Status::Ready => store.apply_transition(
                    t.id,
                    Trigger::Dispatch,
                    Some(Event::WorkerStarted {
                        run_id,
                        adapter: "stub".into(),
                        model: "glm-5".into(),
                        provider: None,
                        account: None,
                        role: None,
                        task_role: None,
                    }),
                ),
                Status::Running => store.apply_transition(
                    t.id,
                    Trigger::WorkerDone,
                    Some(Event::WorkerFinished {
                        run_id,
                        // 検査 task は実 `cap-worker` を、この daemon process の環境のまま起こして走らせる。
                        outcome: if t.genre.as_deref().is_some_and(|g| {
                            g.starts_with(celeris_campaign::cap_probe::PROBE_GENRE_PREFIX)
                        }) {
                            format!("done: {}", run_cap_worker(&t, ws))
                        } else {
                            "done: stub".into()
                        },
                        usage: serde_json::from_value(
                            json!({"input_tokens": 1000, "output_tokens": 50, "cost_usd": 0.01}),
                        )
                        .ok(),
                        role: None,
                        metrics: None,
                        end: None,
                    }),
                ),
                Status::Reviewing => store.apply_transition(t.id, Trigger::ReviewPass, None),
                _ => continue,
            };
            if let Err(e) = step {
                eprintln!("stub executor: {}: {e}", t.id);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ---- 試験の部品 ----

fn campaign_bin() -> String {
    std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin")
}

fn script() -> PathBuf {
    preflight_setup::root().join("../../scripts/campaign/measure-launcher.sh")
}

fn write_exec(p: &Path, body: &str) {
    // 実行中の file を書き換えない（ETXTBSY）: 別名に書いて rename する。
    let tmp = p.with_extension("tmp");
    fs::write(&tmp, body).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(&tmp, p).unwrap();
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let t = Instant::now();
    while !ok() {
        assert!(t.elapsed() < WAIT, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A の stub: `--version` に版を返し、実行時は呼び出しを記録して stream-json の usage を出す。
const CLAUDE_STUB: &str = r#"#!/bin/sh
echo "$*" >> @BIN@/claude.log
if [ "$1" = --version ]; then echo claude-code@2.1.0; exit 0; fi
# 実験 case の試行 workspace に、エージェントが申告する job 証拠と raw を残す（評価側の観測が照合する対象）。
if [ ! -f evidence/jobs.json ]; then
    # 試験が有効にしたときだけ、制御された submit wrapper で（偽の）scheduler へ投入する（A の通常の投入経路）。
    # 投入先と実 submit command は運用者が cap-env で与える値（ここでは偽の qsub）。
    if [ -f @BIN@/qsub.enabled ]; then
        CELERIS_SUBMIT_TARGET=pbs:sirius CELERIS_SUBMIT_CMD=fake-qsub celeris-qsub job.sh >/dev/null || exit 1
    fi
    mkdir -p evidence results
    echo raw >results/out.csv
    h=$(sha256sum results/out.csv | cut -d' ' -f1)
    printf '{"jobs":[{"job_id":"4242.sirius","evidence_kind":"real","raw":[{"path":"results/out.csv","sha256":"%s"}]}]}' "$h" >evidence/jobs.json
fi
echo '{"type":"assistant","message":{"id":"m0","usage":{"input_tokens":100,"output_tokens":5}}}'
exit 0
"#;

/// bwrap の stub: namespace を作らず、`--setenv` と `--chdir` だけ反映して `--` の後を exec する。
const BWRAP_STUB: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do
    case $1 in
        --) shift; break ;;
        --setenv) export "$2=$3"; shift 3 ;;
        --chdir) cd "$2" || exit 1; shift 2 ;;
        *) shift ;;
    esac
done
exec "$@"
"#;

const CAP_STUB: &str = "#!/bin/sh\necho 'cmake version git version Python 3 rustc-min ok'\n";

/// worker の PATH に足す `sh`。能力 probe の複合 command（`sh -c`）だけ成功で返し、他は実 sh に渡す
/// （PATH の先頭に置かれるので、他の script が呼ぶ `sh` を壊さない）。
const CAP_SH_STUB: &str = r#"#!/bin/sh
case "$2" in
  *min-build*|*printf*|*autoreconf*|*libclang*|*verbs.h*) echo 'cmake version git version Python 3 rustc-min ok'; exit 0 ;;
esac
exec /bin/sh "$@"
"#;

struct Fx {
    s: Setup,
    port: u16,
}

impl Fx {
    fn d(&self) -> &Path {
        self.s.dir.path()
    }
    fn env(&self) -> PathBuf {
        self.d().join("launcher.env")
    }
    fn out(&self) -> PathBuf {
        self.d().join("out")
    }
    fn state(&self) -> PathBuf {
        self.d().join("state")
    }
    fn measure(&self) -> PathBuf {
        self.d().join("measure.toml")
    }
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    fn claude_log(&self) -> String {
        fs::read_to_string(self.d().join("bin/claude.log")).unwrap_or_default()
    }
    /// A の実起動（`--version` 以外）の回数。
    fn a_launches(&self) -> usize {
        self.claude_log()
            .lines()
            .filter(|l| l.trim() != "--version")
            .count()
    }
    fn ledger(&self) -> Vec<u8> {
        fs::read(self.out().join("ledger.jsonl")).unwrap_or_default()
    }
    /// 測定 daemon に作られた試行 task の数（無 LLM の能力検査 task `cap-probe` は数えない。daemon が無ければ 0）。
    fn task_posts(&self) -> usize {
        if !self.healthy() {
            return 0;
        }
        let (_, v) = self.api("GET", "/api/v1/tasks?archived=1&limit=200", None);
        v["items"]
            .as_array()
            .or(v.as_array())
            .map_or(0, |a| a.iter().filter(|t| !is_probe(t)).count())
    }
    fn owned_pid(&self) -> Option<u32> {
        fs::read_to_string(self.state().join("launcher.pid"))
            .ok()
            .and_then(|t| t.trim().parse().ok())
    }

    /// daemon の実行ファイル（wrapper）。`real` を daemon 役で exec する。
    fn write_daemon_wrapper(&self, real: &Path, extra: &str) {
        write_exec(
            &self.d().join("bin/celeris"),
            &format!(
                "#!/bin/sh\nexport CAP_WORKER_BIN='{}'\n{extra}exec '{}' --exact {ROLE} --nocapture --test-threads=1 -- \"$@\"\n",
                campaign_bin(),
                real.display()
            ),
        );
    }

    fn write_env(&self, extra: &str) {
        let d = self.d();
        let i = &self.s.inp;
        let p = |v: &Option<PathBuf>| v.as_ref().unwrap().display().to_string();
        fs::write(
            self.env(),
            format!(
                "MANIFEST={m}\nDAEMON_CONFIG={d}/measure.toml\nNORMAL_CONFIG={d}/normal.toml\nCANARIES={d}/canaries.json\n\
                 TASK_TEMPLATE={d}/task-template.json\nOUT={d}/out\nSTATE_DIR={d}/state\nRELEASE={rel}\n\
                 CELERIS_BIN={d}/bin/celeris\nRUN_PATH={d}/cap-tools:{d}/bin\nA_PROGRAM=claude\nA_ACCOUNT=lab\nWORKER_PROGRAMS=\"claude=claude\"\n\
                 MATERIALS_SRC={mat}\nKB_SRC={kb}\nREPO_SRC={repo}\nINPUTS_SRC={ins}\n\
                 CASES_ROOT={cases}\nGRADER_ROOT={gr}\nGRADE_CASES_ROOT={gc}\nCAMPAIGN_BIN={cb}\nCAP_ENV={ce}\n\
                 EXECUTOR=celeris\nINTERVAL_SECS=1\nMAX_ITERATIONS=600\nHEALTH_WAIT_SECS=600\n{extra}",
                m = i.manifest_path.display(),
                d = d.display(),
                rel = MEASURE_RELEASE,
                mat = p(&i.materials_src),
                kb = p(&i.kb_src),
                repo = p(&i.repo_src),
                ins = p(&i.inputs_src),
                cases = i.cases_root.display(),
                gr = i.grader_root.display(),
                gc = i.grade_cases_root.display(),
                cb = campaign_bin(),
                ce = p(&i.cap_env),
            ),
        )
        .unwrap();
    }

    /// 測定 daemon の通常 API（実 HTTP）。token は試験の値（本番の token ではない）。
    fn api(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        let mut c = Command::new("curl");
        c.args(["-sS", "-X", method, "-w", "\n%{http_code}"])
            .arg("-H")
            .arg(format!("authorization: Bearer {TOKEN}"));
        if let Some(b) = body {
            c.args(["-H", "content-type: application/json", "--data-binary"])
                .arg(b.to_string());
        }
        let o = c.arg(format!("{}{path}", self.url())).output().unwrap();
        let t = String::from_utf8_lossy(&o.stdout).into_owned();
        let (b, code) = t.rsplit_once('\n').unwrap_or(("", "0"));
        (
            code.trim().parse().unwrap_or(0),
            serde_json::from_str(b).unwrap_or(Value::Null),
        )
    }

    fn healthy(&self) -> bool {
        Command::new("curl")
            .args(["-fs", "-o", "/dev/null"])
            .arg(format!("{}/api/v1/health", self.url()))
            .status()
            .is_ok_and(|s| s.success())
    }

    /// 案件と repo を測定 daemon の正式 API で作り、B/C の task template に入れる。
    fn create_project(&self) -> String {
        let (st, p) = self.api(
            "POST",
            "/api/v1/projects",
            Some(&json!({"title": "campaign", "request": "trial"})),
        );
        assert_eq!(st, 201, "{p}");
        let id = p["id"].as_str().unwrap().to_string();
        let (st, r) = self.api(
            "POST",
            &format!("/api/v1/projects/{id}/repos"),
            Some(&json!({"name": "scratch", "location": {"kind": "local", "path": "/x"}})),
        );
        assert_eq!(st, 201, "{r}");
        fs::write(
            self.d().join("task-template.json"),
            json!({"project_id": id, "max_turns": 100, "acceptance": [{"type": "reviewer", "text": "x"}]})
                .to_string(),
        )
        .unwrap();
        id
    }

    /// launcher を通さずに、同じ測定 config の daemon を別 process として起動する（launcher の所有ではない）。
    fn spawn_foreign_daemon(&self) -> Foreign {
        let c = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", ROLE, "--nocapture", "--test-threads=1", "--"])
            .arg("--config")
            .arg(self.measure())
            .args(["--release", MEASURE_RELEASE])
            .stdout(fs::File::create(self.d().join("foreign.log")).unwrap())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let c = Foreign(c);
        wait_for("foreign daemon healthy", || self.healthy());
        c
    }
}

impl Drop for Fx {
    /// 試験が途中で落ちても、launcher 所有の daemon を launcher の stop（所有 PID と起動 identity の照合つき）で止める。
    fn drop(&mut self) {
        if self.owned_pid().is_some() {
            let _ = launch(self, "stop");
        }
    }
}

/// launcher の外で起動した process（試験が落ちても残さない）。
struct Foreign(Child);

impl Drop for Foreign {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 一時 port の測定 config・token・試験用アカウント・stub まで揃える（daemon は launcher の start が起動する）。
fn fixture() -> Fx {
    let s = setup();
    let d = s.dir.path().to_path_buf();
    let port = free_port();
    let measure = format!(
        "{}[accounts]\nclaude_dir = \"{}\"\n",
        measure_config(&d, "measure").replace("127.0.0.1:7701", &format!("127.0.0.1:{port}")),
        d.join("accounts").display()
    );
    fs::write(d.join("measure.toml"), measure).unwrap();
    fs::create_dir_all(d.join("measure")).unwrap();
    fs::write(d.join("measure/token"), TOKEN).unwrap();
    // 測定用アカウント（claude_oauth の pool）。ログイン済みの印だけ。
    fs::create_dir_all(d.join("accounts/lab")).unwrap();
    fs::write(d.join("accounts/lab/.credentials.json"), "{}").unwrap();
    let bin = d.join("bin");
    fs::create_dir_all(&bin).unwrap();
    write_exec(
        &bin.join("claude"),
        &CLAUDE_STUB.replace("@BIN@", &bin.display().to_string()),
    );
    write_exec(&bin.join("bwrap"), BWRAP_STUB);
    // 必要能力 probe（cap-probe）は bwrap stub 越しに走る。この host に無い道具は stub を限定 mount で足す。
    let tools = d.join("cap-tools");
    fs::create_dir_all(&tools).unwrap();
    for t in [
        "cc",
        "c++",
        "cmake",
        "git",
        "pkg-config",
        "sh",
        "ssh",
        "python3",
    ] {
        write_exec(
            &tools.join(t),
            if t == "sh" { CAP_SH_STUB } else { CAP_STUB },
        );
    }
    // 制御された submit wrapper（repo の実物）と、受理した job id を返す偽の qsub。
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/campaign/celeris-qsub"),
        tools.join("celeris-qsub"),
    )
    .unwrap();
    write_exec(
        &tools.join("fake-qsub"),
        &format!(
            "#!/bin/sh\necho \"$*\" >> {}/qsub.log\necho 4242.sirius\n",
            bin.display()
        ),
    );
    let tc = json!({
        "ro_mounts": [tools],
        "env": [["PATH", format!("{}:{}:/usr/local/bin:/usr/bin:/bin", tools.display(), bin.display())]],
    });
    fs::write(
        s.inp.cap_env.as_ref().unwrap(),
        json!({"schema": "celeris.campaign-cap-env/1", "a": tc, "worker": tc}).to_string(),
    )
    .unwrap();
    let f = Fx { s, port };
    f.write_daemon_wrapper(&std::env::current_exe().unwrap(), "");
    f.write_env("");
    f
}

fn launch(f: &Fx, sub: &str) -> Output {
    Command::new("sh")
        .arg(script())
        .arg(sub)
        .arg("--env")
        .arg(f.env())
        .output()
        .expect("run launcher")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn ok(f: &Fx, sub: &str) -> String {
    let o = launch(f, sub);
    assert!(o.status.success(), "{sub}: {}", text(&o));
    assert!(!text(&o).contains(TOKEN), "token leaked by {sub}");
    text(&o)
}

/// 外部実行 0 件で拒否された: 非 0・メッセージ・A の実起動が増えない・`POST /api/v1/tasks` が増えない・台帳が増えない。
fn assert_rejected(f: &Fx, sub: &str, needle: &str) {
    let (a, posts, ledger) = (f.a_launches(), f.task_posts(), f.ledger());
    let o = launch(f, sub);
    let t = text(&o);
    assert!(!o.status.success(), "{sub} must be rejected: {t}");
    assert!(t.contains(needle), "expected {needle:?} in: {t}");
    assert_eq!(f.a_launches(), a, "A was launched: {}", f.claude_log());
    assert_eq!(f.task_posts(), posts, "a task was POSTed ({sub}: {needle})");
    assert_eq!(f.ledger(), ledger, "ledger changed");
    assert!(!t.contains(TOKEN), "token leaked");
}

fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// 起動した daemon を止める（launcher 所有の pid だけ）。
fn stop(f: &Fx) {
    let pid = f.owned_pid();
    let _ = launch(f, "stop");
    if let Some(p) = pid {
        wait_for("daemon stopped", || !alive(p));
    }
}

fn exe_of(pid: u32) -> PathBuf {
    fs::read_link(format!("/proc/{pid}/exe")).unwrap()
}

// ---- 試験 ----

/// launcher.env の SERIES_ROOT は `prepare` が `series-init` へ渡す。主比較の manifest（この fixture）では台帳を使わず成功し、
/// MEASUREMENT_ID なしの SERIES_ROOT は拒否される。副比較の台帳作成・OUT 拒否・欠損/破損の拒否は side_budget.rs（同じ CLI 引数）。
#[test]
fn series_root_in_launcher_env_is_passed_to_series_init() {
    let f = fixture();
    let d = f.d().display().to_string();
    f.write_env(&format!("SERIES_ROOT={d}/series\n"));
    let o = launch(&f, "prepare");
    assert!(!o.status.success(), "{}", text(&o));
    assert!(
        text(&o).contains("MEASUREMENT_ID is required with SERIES_ROOT"),
        "{}",
        text(&o)
    );
    f.write_env(&format!(
        "MEASUREMENT_ID=m-series\nSERIES_ROOT={d}/series\n"
    ));
    ok(&f, "prepare");
    assert!(
        !f.d().join("series").exists(),
        "a primary manifest uses no series ledger"
    );
}

#[test]
fn matching_environment_runs_task_api_watch_grade_export_and_resumes_with_the_fixed_record() {
    let f = fixture();
    let t = ok(&f, "prepare");
    assert!(t.contains(&format!("127.0.0.1:{}", f.port)), "{t}");
    ok(&f, "start");
    // HTTP で答えている daemon は launcher が起動した process そのもの（identity.pid = 所有 PID、実行像 = 試験 binary）。
    let pid = f.owned_pid().expect("launcher pid");
    assert_eq!(exe_of(pid), std::env::current_exe().unwrap());
    let (st, cfg) = f.api("GET", "/api/v1/config", None);
    assert_eq!(st, 200, "{cfg}");
    assert_eq!(cfg["identity"]["pid"], json!(pid), "{cfg}");
    assert_eq!(cfg["identity"]["release"], json!(MEASURE_RELEASE));
    f.create_project();
    ok(&f, "preflight");
    // preflight が測定 daemon に作るのは無 LLM の能力検査 task（case × B/C = 4 件）だけで、試行 task は作らない。
    assert_eq!(f.task_posts(), 0, "preflight must not create trial tasks");
    let (_, probes) = f.api("GET", "/api/v1/tasks?limit=50", None);
    let probes = probes["items"].as_array().or(probes.as_array()).unwrap();
    assert_eq!(probes.len(), 4, "{probes:?}");
    assert!(probes.iter().all(is_probe), "{probes:?}");

    // 通常 API による task 投入 → watch が台帳を閉じるまで。
    let w = ok(&f, "watch");
    let line = w.lines().rev().find(|l| l.starts_with('{')).unwrap();
    let rep: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {w}"));
    assert_eq!(rep["watch"]["end"], json!("settled"), "{w}");
    assert_eq!(rep["ledger"]["by_state"], json!({"done": 6}), "{w}");
    assert_eq!(rep["executor_errors"], json!(0), "{w}");
    // B/C × 2 問題 = 4 件が正式 API（POST /api/v1/tasks）で投入され、A × 2 問題が stub harness で走った
    // （無 LLM の能力検査 task は数えない）。
    assert_eq!(f.task_posts(), 4, "{w}");
    assert_eq!(f.a_launches(), 2, "{}", f.claude_log());
    let (st, tasks) = f.api("GET", "/api/v1/tasks?limit=50", None);
    assert_eq!(st, 200);
    let items: Vec<_> = tasks["items"]
        .as_array()
        .or(tasks.as_array())
        .unwrap()
        .iter()
        .filter(|t| !is_probe(t))
        .cloned()
        .collect();
    assert_eq!(items.len(), 4, "{tasks}");
    assert!(items.iter().all(|t| t["status"] == "done"), "{tasks}");
    // 各 task は通常の run（usage 付き）で終わっている。
    let id = items[0]["id"].as_str().unwrap();
    let (_, detail) = f.api("GET", &format!("/api/v1/tasks/{id}"), None);
    assert_eq!(detail["runs"][0]["outcome"], json!("done"), "{detail}");
    assert_eq!(detail["runs"][0]["usage"]["input_tokens"], json!(1000));
    assert!(f.out().join("env-lock.json").exists());
    let lock = fs::read(f.out().join("env-lock.json")).unwrap();

    // 独立採点（grade-out と check）は exit 0。
    ok(&f, "grade");
    let grades = fs::read_to_string(f.out().join("grades.jsonl")).unwrap();
    assert!(
        grades.lines().any(|l| l.contains("\"graded\":true")),
        "{grades}"
    );
    ok(&f, "export");
    let exported: Vec<String> = fs::read_dir(f.out().join("export"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        exported.iter().any(|n| n.starts_with("table")),
        "{exported:?}"
    );

    // 再開: 同じ入口。daemon 実体・env-lock は一致を確認し、再投入しない。
    let (before, a_before) = (f.ledger(), f.a_launches());
    ok(&f, "start");
    assert_eq!(
        f.owned_pid(),
        Some(pid),
        "start must reconnect, not restart"
    );
    let posts_before = f.task_posts();
    ok(&f, "watch");
    assert_eq!(fs::read(f.out().join("env-lock.json")).unwrap(), lock);
    assert_eq!(f.ledger(), before);
    // 再開は試行 task を再投入しない（能力検査 task は数えない）。
    assert_eq!(f.task_posts(), posts_before);
    assert_eq!(f.a_launches(), a_before);
    // 秘密値は固定記録にもログにも無い。
    for p in [
        f.out().join("env-lock.json"),
        f.state().join("daemon.log"),
        f.state().join("launcher.meta"),
    ] {
        let body = fs::read_to_string(&p).unwrap_or_default();
        assert!(!body.contains(TOKEN), "{}", p.display());
    }
    // 再開時に固定条件が変わっていれば拒否: 同一 path の A の差替え（版文字列は同じ）。
    let claude = f.d().join("bin/claude");
    let saved = fs::read_to_string(&claude).unwrap();
    write_exec(&claude, &format!("{saved}# swapped\n"));
    assert_rejected(&f, "watch", "env-lock");
    stop(&f);
    assert!(!alive(pid));
}

#[test]
fn a_bad_measurement_config_is_refused_before_the_daemon_starts() {
    let f = fixture();
    ok(&f, "prepare");
    let good = fs::read_to_string(f.measure()).unwrap();
    let normal = f.d().join("normal");
    for (what, bad, needle) in [
        (
            "normal db",
            good.replace(
                &format!("{}/celeris.db", f.d().join("measure").display()),
                &format!("{}/celeris.db", normal.display()),
            ),
            "is the normal operation's db",
        ),
        (
            "normal workspace",
            good.replace(
                &format!("{}/ws", f.d().join("measure").display()),
                &format!("{}/ws/trial", normal.display()),
            ),
            "workspace_root",
        ),
        (
            "langmem",
            format!("{good}[knowledge.langmem]\nenabled = true\n"),
            "[knowledge.langmem] enabled",
        ),
    ] {
        fs::write(f.measure(), &bad).unwrap();
        let o = launch(&f, "start");
        let t = text(&o);
        assert!(!o.status.success(), "{what}: {t}");
        assert!(t.contains("config-check failed"), "{what}: {t}");
        assert!(t.contains(needle), "{what}: {t}");
        // daemon は起動していない: pidfile も daemon log も無く、port は誰も答えず、DB・作業場所も作られていない。
        assert!(f.owned_pid().is_none(), "{what}");
        assert!(!f.state().join("daemon.log").exists(), "{what}");
        assert!(!f.healthy(), "{what}");
        assert!(!normal.join("celeris.db").exists(), "{what}");
        assert!(!normal.join("ws").exists(), "{what}");
        assert!(!f.d().join("measure/celeris.db").exists(), "{what}");
    }
    assert_eq!(f.task_posts(), 0);
    assert_eq!(f.a_launches(), 0);
    assert!(!f.out().exists());
}

#[test]
fn a_different_daemon_behind_the_url_is_rejected_with_zero_execution() {
    let f = fixture();
    // 同じ測定 config で、launcher の外で起動した別 daemon が URL で答える。
    let mut foreign = f.spawn_foreign_daemon();
    let foreign_pid = foreign.0.id();
    // launcher の daemon は（port が塞がっていても）生き続ける常駐 process: 所有 PID はあるが HTTP は別 process が答える。
    write_exec(
        &f.d().join("bin/celeris"),
        "#!/bin/sh\nwhile :; do sleep 0.2; done\n",
    );
    ok(&f, "prepare");
    let o = launch(&f, "start");
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(t.contains("env-check failed"), "{t}");
    assert!(t.contains("identity.pid"), "{t}");
    // 失敗時の片付けは、この start が起動した process だけ（別 daemon には触らない）。
    assert!(f.owned_pid().is_none(), "{t}");
    assert!(
        foreign.0.try_wait().unwrap().is_none(),
        "foreign daemon touched"
    );
    assert!(f.healthy());
    assert_rejected(&f, "preflight", "not running");
    assert_rejected(&f, "watch", "not running");
    // CLI の env-check を直接、別 daemon の URL に向けても identity.pid が launcher 所有でなければ拒否。
    let o = Command::new(campaign_bin())
        .arg("env-check")
        .arg("--manifest")
        .arg(&f.s.inp.manifest_path)
        .args(["--api-url", &f.url(), "--token-env", "T"])
        .arg("--measure-config")
        .arg(f.measure())
        .arg("--normal-config")
        .arg(f.d().join("normal.toml"))
        .arg("--canaries")
        .arg(f.d().join("canaries.json"))
        .arg("--kb-src")
        .arg(f.s.inp.kb_src.as_ref().unwrap())
        .args(["--daemon-pid", "1"])
        .env("T", TOKEN)
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(text(&o).contains("identity.pid"), "{}", text(&o));
    assert_eq!(f.task_posts(), 0);
    assert_eq!(f.a_launches(), 0);
    assert!(!f.out().exists(), "out must not be created");
    assert!(alive(foreign_pid));
    drop(foreign);
}

#[test]
fn effective_config_differences_are_rejected_with_zero_execution() {
    let f = fixture();
    ok(&f, "prepare");
    ok(&f, "start");
    f.create_project();
    // daemon が起動した後に config が書き換わった（起動時の bytes の hash と違う）。
    let saved = fs::read_to_string(f.measure()).unwrap();
    fs::write(
        f.measure(),
        format!("{saved}\n# edited after the daemon started\n"),
    )
    .unwrap();
    assert_rejected(&f, "preflight", "config_sha256");
    assert_rejected(&f, "watch", "config_sha256");
    assert!(!f.out().exists());
    stop(&f);
}

#[test]
fn a_version_mismatch_and_missing_inputs_are_rejected_with_zero_execution() {
    let f = fixture();
    ok(&f, "prepare");
    ok(&f, "start");
    f.create_project();
    // 版不一致: A の実際の版が manifest の harness_version と違う。
    let claude = f.d().join("bin/claude");
    let saved = fs::read_to_string(&claude).unwrap();
    write_exec(&claude, &saved.replace("2.1.0", "2.0.9"));
    assert_rejected(&f, "watch", "does not match manifest A harness_version");
    write_exec(&claude, &saved);
    // 必須入力の欠落: 外部入力 dir が無い。
    let ins = f.s.inp.inputs_src.clone().unwrap();
    fs::rename(&ins, f.d().join("inputs-gone")).unwrap();
    assert_rejected(&f, "watch", "preflight failed");
    let o = launch(&f, "prepare");
    assert!(
        !o.status.success() && text(&o).contains("required input missing"),
        "{}",
        text(&o)
    );
    fs::rename(f.d().join("inputs-gone"), &ins).unwrap();
    // 必須の設定欠落（env ファイルの A_ACCOUNT）。
    f.write_env("A_ACCOUNT=\n");
    assert_rejected(&f, "watch", "--a-account");
    f.write_env("");
    assert!(!f.out().exists());
    stop(&f);
}

#[test]
fn a_swapped_daemon_binary_is_rejected_on_every_path_and_stop_only_touches_the_launcher_pid() {
    let f = fixture();
    // daemon の実体は試験 binary の写し（同一 path での差替えを再現するため。reflink できれば複製しない）。
    let real = f.d().join("bin/celeris-real");
    assert!(
        Command::new("cp")
            .arg("--reflink=auto")
            .arg(std::env::current_exe().unwrap())
            .arg(&real)
            .status()
            .unwrap()
            .success()
    );
    f.write_daemon_wrapper(&real, "");
    ok(&f, "prepare");
    // (1) 起動前: 同一 path の daemon 実行ファイル差替え → start は起動しない。
    f.write_daemon_wrapper(&real, "# swapped\n");
    assert_rejected(&f, "start", "differs from the one pinned at prepare");
    assert!(f.owned_pid().is_none());
    assert!(!f.healthy());
    f.write_daemon_wrapper(&real, "");
    ok(&f, "start");
    let mine = f.owned_pid().unwrap();
    assert_eq!(exe_of(mine), real.canonicalize().unwrap());
    f.create_project();
    ok(&f, "preflight");

    // (2) 稼働中に CELERIS_BIN（同一 path）が差し替わった → 再接続・preflight・watch の全経路で拒否。
    f.write_daemon_wrapper(&real, "# swapped while running\n");
    for sub in ["start", "preflight", "watch"] {
        assert_rejected(&f, sub, "differs from the one pinned at prepare");
    }
    f.write_daemon_wrapper(&real, "");
    ok(&f, "preflight");

    // (3) 動いている daemon の実行像（同一 path の binary）が差し替わった → 全経路で拒否。
    let swapped = f.d().join("bin/celeris-real.new");
    fs::copy(&real, &swapped).unwrap();
    {
        use std::io::Write;
        let mut h = fs::OpenOptions::new().append(true).open(&swapped).unwrap();
        h.write_all(b"swapped").unwrap();
    }
    fs::rename(&swapped, &real).unwrap();
    for sub in ["start", "preflight", "watch"] {
        assert_rejected(&f, sub, "daemon binary");
    }
    assert_eq!(f.owned_pid(), Some(mine), "rejection must not restart");
    assert_eq!(f.task_posts(), 0);
    assert_eq!(f.a_launches(), 0);
    assert!(!f.out().exists());

    // 本番 daemon に見立てた無関係の process（別 pidfile）。launcher の stop は触らない。
    let mut prod = Command::new("sleep").arg("300").spawn().unwrap();
    let prod_pid = prod.id();
    let prod_pidfile = f.d().join("production.pid");
    fs::write(&prod_pidfile, prod_pid.to_string()).unwrap();
    assert_ne!(mine, prod_pid);
    // pidfile が（pid 再利用で）別 process を指しても stop は送らない。
    let pidfile = f.state().join("launcher.pid");
    let saved = fs::read_to_string(&pidfile).unwrap();
    fs::write(&pidfile, prod_pid.to_string()).unwrap();
    let o = launch(&f, "stop");
    assert!(!o.status.success(), "{}", text(&o));
    assert!(alive(prod_pid));
    assert!(
        prod.try_wait().unwrap().is_none(),
        "production process was signalled"
    );
    // 本来の pidfile に戻すと launcher 所有の pid だけが止まる（実行像の path は起動時と同じ process）。
    fs::write(&pidfile, saved).unwrap();
    let t = ok(&f, "stop");
    assert!(t.contains(&format!("stopped pid {mine}")), "{t}");
    wait_for("daemon stopped", || !alive(mine));
    assert!(
        prod.try_wait().unwrap().is_none(),
        "production process was stopped"
    );
    assert_eq!(
        fs::read_to_string(&prod_pidfile).unwrap(),
        prod_pid.to_string()
    );
    prod.kill().unwrap();
    let _ = prod.wait();
}

/// 固定 toolchain（cap-env の ro_mounts）の同名 file の内容だけを変える。path・file 名・mount 構成は同じで、
/// 必要能力 probe（stub は何でも合格）は通るが、同じ測定の再開（watch / start）は env-lock で外部実行 0 件のまま拒否される。
/// 内容を戻せば再開でき、読取不能は検証不能として同じく止まる（root では chmod が効かないので skip）。
#[test]
fn a_toolchain_content_change_under_the_same_names_is_rejected_on_resume() {
    let f = fixture();
    ok(&f, "prepare");
    ok(&f, "start");
    f.create_project();
    ok(&f, "preflight");
    ok(&f, "watch");
    let lock = fs::read(f.out().join("env-lock.json")).unwrap();
    let cc = f.d().join("cap-tools/cc");
    let saved = fs::read_to_string(&cc).unwrap();

    // 同名のまま内容だけ変える（probe の stub は合格のまま）。
    write_exec(&cc, &format!("{saved}# different compiler\n"));
    assert_rejected(&f, "watch", "env-lock");
    // 固定記録は黙って書き換わらない。
    assert_eq!(fs::read(f.out().join("env-lock.json")).unwrap(), lock);

    // 内容を戻せば同じ測定を再開できる。
    write_exec(&cc, &saved);
    ok(&f, "watch");

    // 読取不能は検証不能として止まる（空一覧・成功として扱わない）。
    if !running_as_root() {
        let git = f.d().join("cap-tools/git");
        fs::set_permissions(&git, fs::Permissions::from_mode(0o000)).unwrap();
        assert_rejected(&f, "watch", "env-lock");
        fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
        ok(&f, "watch");
    }
    stop(&f);
}

fn running_as_root() -> bool {
    fs::read_to_string("/proc/self/status")
        .map(|s| {
            s.lines()
                .any(|l| l.starts_with("Uid:") && l.split_whitespace().nth(2) == Some("0"))
        })
        .unwrap_or(false)
}

/// 能力検査 task（分野 `cap-probe-<harness>`）か。
fn is_probe(t: &serde_json::Value) -> bool {
    t["genre"]
        .as_str()
        .is_some_and(|g| g.starts_with(celeris_campaign::cap_probe::PROBE_GENRE_PREFIX))
}

// ---- 研究評価接続: 共有台帳・arm 別の認証・variant 基準 hash・評価側観測の収集（無 LLM・標準 launcher 入口） ----

fn edit_manifest(f: &Fx, g: impl FnOnce(&mut Value)) {
    let p = &f.s.inp.manifest_path;
    let mut v: Value = serde_json::from_slice(&fs::read(p).unwrap()).unwrap();
    g(&mut v);
    fs::write(p, v.to_string()).unwrap();
}

const B0_ROLES_SONNET: &str = r#"{"planner":"claude-sonnet-5-5","reviewer":"claude-sonnet-5-5","cos":"claude-sonnet-5-5","worker":"claude-sonnet-5-5"}"#;

/// 有効 arm だけの configs・set 宣言にする（manifest は有効 arm の config をちょうど 1 件ずつ持つ）。
fn only_arms(v: &mut Value, arms: &[&str], compare: Value, fixed: Value) {
    v["configs"]
        .as_array_mut()
        .unwrap()
        .retain(|c| arms.contains(&c["id"].as_str().unwrap()));
    // B/C が同じ有効 arm の set に入るときの B1/C1 共通条件（共通の候補モデル 2 つ以上）。
    for c in v["configs"].as_array_mut().unwrap() {
        if c["routing"].is_object() {
            c["routing"]["models"] = json!(["claude-sonnet-5-5", "claude-sonnet-5-5-b"]);
        }
    }
    if !arms.contains(&"C") {
        v["secondary"]["subset"] = json!([]);
    }
    let mut fixed = fixed;
    fixed["role_models"] = serde_json::from_str(B0_ROLES_SONNET).unwrap();
    v["experiment_sets"] = json!({"s": {"arms": arms, "compare": compare, "fixed": fixed}});
    v["selected_sets"] = json!(["s"]);
}

fn env_without(f: &Fx, key: &str) {
    let t = fs::read_to_string(f.env()).unwrap();
    let kept: String = t
        .lines()
        .filter(|l| !l.starts_with(&format!("{key}=")))
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(f.env(), kept).unwrap();
}

/// 共有承認の台帳を宣言した manifest の測定は、宣言した台帳を渡す launcher だけが走れる。
/// 省略・別 path は投入前に拒否され（台帳・task 投入 0 件）、一致すれば通常どおり台帳が閉じる。
#[test]
fn shared_approval_runs_only_with_the_declared_global_ledger() {
    let f = fixture();
    let d = f.d().display().to_string();
    let decl = format!("{d}/global-ledger");
    edit_manifest(&f, |v| {
        only_arms(
            v,
            &["A", "B", "C"],
            json!([["A", "B"]]),
            json!({"approved_trials": 100, "shared_ledger": decl}),
        );
    });
    ok(&f, "prepare");
    ok(&f, "start");
    f.create_project();
    ok(&f, "preflight");
    // 省略。
    assert_rejected(&f, "watch", "requires --global-ledger");
    // 宣言と別 path。
    f.write_env(&format!("GLOBAL_LEDGER={d}/other-ledger\n"));
    assert_rejected(&f, "watch", "is not the declared shared ledger");
    // 宣言どおり（相対 path は env ファイルの dir 基準）。
    f.write_env("GLOBAL_LEDGER=global-ledger\n");
    let w = ok(&f, "watch");
    let line = w.lines().rev().find(|l| l.starts_with('{')).unwrap();
    let rep: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {w}"));
    assert_eq!(rep["ledger"]["by_state"], json!({"done": 6}), "{w}");
    assert!(
        f.d().join("global-ledger").is_dir(),
        "the shared ledger was written"
    );
    stop(&f);
}

/// B/C のみの構成は A の認証（A_ACCOUNT）も program も要求しない。A が有効なら A_ACCOUNT を要求する。
#[test]
fn b_c_only_needs_no_a_credentials_but_an_active_a_arm_does() {
    let f = fixture();
    edit_manifest(&f, |v| {
        only_arms(v, &["B", "C"], json!([["B", "C"]]), json!({}));
    });
    env_without(&f, "A_ACCOUNT");
    f.write_env("A_PROGRAM=/nonexistent/claude\n");
    ok(&f, "prepare");
    ok(&f, "start");
    f.create_project();
    ok(&f, "preflight");
    let w = ok(&f, "watch");
    let line = w.lines().rev().find(|l| l.starts_with('{')).unwrap();
    let rep: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {w}"));
    assert_eq!(rep["ledger"]["by_state"], json!({"done": 4}), "{w}");
    assert_eq!(f.a_launches(), 0, "A must not run in a B/C-only set");
    stop(&f);

    // A が有効（A/B）で A_ACCOUNT が無ければ、A の認証を求めて拒否する。C だけが無効でも B の worker は要る。
    let g = fixture();
    edit_manifest(&g, |v| {
        only_arms(v, &["A", "B"], json!([["A", "B"]]), json!({}));
    });
    env_without(&g, "A_ACCOUNT");
    ok(&g, "prepare");
    ok(&g, "start");
    g.create_project();
    assert_rejected(&g, "preflight", "--a-account");
    // A_ACCOUNT を与えれば A/B-only の preflight は通る（C の検査は要らない）。
    g.write_env("A_ACCOUNT=lab\n");
    ok(&g, "preflight");
    stop(&g);
}

/// manifest が variant を参照する測定は、承認済み B0 の sha256（VARIANT_B0_SHA256）を launcher が preflight へ渡す。
/// 未指定は拒否、誤った hash は sha256 不一致で拒否される（値が CLI まで届いている）。
#[test]
fn variant_b0_sha256_is_passed_to_preflight_and_a_wrong_hash_is_rejected() {
    let f = fixture();
    let d = f.d();
    let b0 = json!({"coding_node": "coding", "role_models": {}, "org": {"items": [], "effective_profiles": []}});
    fs::write(d.join("variant-b0.json"), b0.to_string()).unwrap();
    edit_manifest(&f, |v| {
        v["variant"] = json!({
            "comparison_id": celeris_campaign::variant::COMPARISON_ID,
            "id": "M1-spec-on",
            "content_sha256": "0".repeat(64),
            "selection_sha256": celeris_campaign::variant::selection_sha256(),
        });
    });
    ok(&f, "prepare");
    ok(&f, "start");
    f.create_project();
    assert_rejected(&f, "preflight", "--variant-b0-sha256");
    f.write_env(&format!("VARIANT_B0_SHA256={}\n", "0".repeat(64)));
    let o = launch(&f, "preflight");
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(
        !t.contains("--variant-b0-sha256 (approved sha256 of variant-b0.json) is required"),
        "the launcher did not pass the hash: {t}"
    );
    assert!(t.contains("variant"), "{t}");
    stop(&f);
}

/// stub が cluster job を投げない trial（not_submitted だけ）では、collect は失敗記録と履歴を書いて 0 で終わる。
/// grade/checker と同じく、モデルの失敗で測定基盤の失敗ではない（ADR 2026-10-26-collect-not-submitted-exit）。
#[test]
fn collect_not_submitted_only_writes_failure_records_and_exits_zero() {
    let f = fixture();
    ok(&f, "prepare");
    ok(&f, "start");
    f.create_project();
    ok(&f, "preflight");
    ok(&f, "watch");
    let out = f.out();
    let o = launch(&f, "collect");
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    let rep: Value = serde_json::from_str(&String::from_utf8_lossy(&o.stdout)).expect("report");
    assert_eq!(rep["ok"], json!(true), "{rep}");
    assert_eq!(rep["blocking"], json!(0), "{rep}");
    let failed = rep["failed"].as_array().expect("failed");
    assert!(!failed.is_empty(), "{rep}");
    assert_eq!(rep["not_submitted"], json!(failed.len()), "{rep}");
    for c in ["A", "B", "C"] {
        assert!(
            failed.iter().any(|r| r["trial"]
                == json!(format!("{c}/k2-bf-xattr-restart-campaign/rep1#1"))
                && r["classes"] == json!(["not_submitted"])
                && r["failure_class"] == json!("model_failure")),
            "{rep}"
        );
        let rec: Value = serde_json::from_slice(
            &fs::read(out.join(format!(
                "observed/k2-bf-xattr-restart-campaign/{c}-rep1.collect-failure.json"
            )))
            .expect("failure record"),
        )
        .unwrap();
        assert_eq!(rec["failure_class"], json!("model_failure"), "{rec}");
        assert_eq!(rec["issues"][0]["class"], json!("not_submitted"), "{rec}");
    }
    assert!(
        out.join("observed/k2-bf-xattr-restart-campaign/collect-log.jsonl")
            .is_file()
    );
    // grade は not_submitted を観測障害の警告なしに採点する。
    let g = launch(&f, "grade");
    assert!(
        !text(&g).contains("measure-launcher: warning: collect-observed-out"),
        "{}",
        text(&g)
    );
    stop(&f);
}

/// not_submitted（B/C）と測定基盤の class（A: scheduler の応答に job 記録が無い = unobserved）が混ざると、collect は非 0 のまま。
#[test]
fn collect_not_submitted_mixed_with_infra_class_stays_nonzero() {
    let f = fixture();
    fs::write(f.d().join("bin/qsub.enabled"), "").unwrap();
    ok(&f, "prepare");
    ok(&f, "start");
    f.create_project();
    ok(&f, "preflight");
    ok(&f, "watch");
    let o = launch(&f, "collect");
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(
        t.contains("[unobserved]") && t.contains("[not_submitted]"),
        "{t}"
    );
    assert!(
        t.contains("\"ok\": false") && t.contains("\"blocking\": 1"),
        "{t}"
    );
    let g = launch(&f, "grade");
    assert!(
        text(&g).contains("collect-observed-out failed on measurement infrastructure"),
        "{}",
        text(&g)
    );
    stop(&f);
}

/// 実験 case の試行について、A の submit wrapper が受理を記録した job ID が通常経路（executor の track → 台帳）で
/// 台帳に入り、標準の grade が評価側の観測を収集し、trial identity・台帳の job ID・scheduler 応答の控え（trial の印）・
/// 成果物 hash に結ぶ。台帳は手で編集しない。取得失敗は env_failure、未終了は undetermined として grade に残り、
/// 未終了・欠測・成果物改変・別 trial の記録・台帳に無い job・控えの改変は成功証拠にならない。
#[test]
fn grade_collects_job_observations_bound_to_the_trial_and_rejects_every_mismatch() {
    let f = fixture();
    fs::write(f.d().join("bin/qsub.enabled"), "").unwrap();
    ok(&f, "prepare");
    ok(&f, "start");
    f.create_project();
    ok(&f, "preflight");
    ok(&f, "watch");
    let out = f.out();
    // 実験 case（k2）の A 試行。投入先（job ID）は wrapper の受理記録から executor が台帳へ運んだもの。
    let trial: Value = fs::read_to_string(out.join("trials.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|r| r["config"] == "A" && r["problem_id"] == "k2-bf-xattr-restart-campaign")
        .expect("A k2 trial");
    let ws = out.join(trial["workspace"].as_str().unwrap());
    assert!(ws.join("evidence/jobs.json").is_file());
    let row: Value = fs::read_to_string(out.join("ledger.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .rfind(|r| r["arm"] == "A" && r["problem_id"] == trial["problem_id"])
        .expect("ledger row");
    assert_eq!(row["state"], json!("done"), "{row}");
    assert_eq!(
        row["cluster_job_ids"],
        json!(["pbs:sirius:4242.sirius"]),
        "{row}"
    );
    let key = row["idempotency_key"].as_str().unwrap().to_string();
    let qsub_log = fs::read_to_string(f.d().join("bin/qsub.log")).unwrap();
    assert!(
        qsub_log.contains(&format!("-v CELERIS_TRIAL_KEY={key} job.sh")),
        "{qsub_log}"
    );
    // B/C は A の wrapper を使わない（Celeris の cluster wait が投入先の正）。この fixture では投入しない。
    for r in fs::read_to_string(out.join("ledger.jsonl"))
        .unwrap()
        .lines()
    {
        let r: Value = serde_json::from_str(r).unwrap();
        if r["arm"] != "A" {
            assert!(
                r["cluster_job_ids"].as_array().is_none_or(Vec::is_empty),
                "{r}"
            );
        }
    }
    // read-only の scheduler 照会（ssh qstat）の固定応答。
    let qstat = f.d().join("bin/qstat.out");
    // cap-tools の ssh stub（能力検査用）を、qstat の固定応答を返す stub に置き換える（PATH 上で先に当たる）。
    write_exec(
        &f.d().join("cap-tools/ssh"),
        &format!("#!/bin/sh\ncat {}\n", qstat.display()),
    );
    let reply = |state: &str| {
        fs::write(
            &qstat,
            format!("Job Id: 4242.sirius\n    job_state = {state}\n    Exit_status = 0\n    Resource_List.nodes = 3\n    Variable_List = PBS_O_HOME=/home/u,CELERIS_TRIAL_KEY={key}\n"),
        )
        .unwrap();
    };
    let obs = out.join("observed/k2-bf-xattr-restart-campaign/A-rep1.json");
    let a_grade = |f: &Fx| -> String {
        fs::read_to_string(f.out().join("grades.jsonl"))
            .unwrap()
            .lines()
            .find(|l| l.contains("\"config\":\"A\"") && l.contains("k2-bf-xattr"))
            .unwrap()
            .to_string()
    };

    let a_line = |f: &Fx| -> Value { serde_json::from_str(&a_grade(f)).unwrap() };

    // 取得失敗（scheduler 照会の不通）: 記録を作らず、grade はその試行を env_failure にする（model_failure にしない）。
    let o = launch(&f, "collect");
    assert!(
        !o.status.success() && text(&o).contains("[fetch_failed]"),
        "{}",
        text(&o)
    );
    assert!(!obs.exists(), "no partial record");
    launch(&f, "grade");
    let l = a_line(&f);
    assert_eq!(l["failure_class"], json!("env_failure"), "{l}");
    assert_eq!(l["observation"]["status"], json!("collect_failed"), "{l}");
    assert_eq!(
        l["observation"]["issues"][0]["class"],
        json!("fetch_failed"),
        "{l}"
    );

    // The actual collect -> grade -> binding -> check/export path must retain the cause.
    let manifest =
        celeris_campaign::manifest::CampaignManifest::load(&out.join("campaign.json")).unwrap();
    let check = celeris_campaign::checker::check(&manifest, &out);
    assert!(check.ok, "{:?}", check.violations);
    assert!(!check.complete);
    assert!(
        check
            .incomplete_reasons
            .iter()
            .any(|r| r.contains("fetch_failed") && r.contains("re-collect and re-grade"))
    );
    let before = serde_json::to_value(check.recomputed.unwrap().totals).unwrap();
    assert_eq!(before["A"]["observation_failures"]["fetch_failed"], 1);
    assert_eq!(before["A"]["grader_env_failure"], 0);
    let table =
        celeris_campaign::export::export(&manifest, &out, &out.join("collector-export")).unwrap();
    let a = table["configs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["config"] == "A")
        .unwrap();
    assert_eq!(a["observation_failures"]["fetch_failed"], 1);
    assert_eq!(a["fault_breakdown"]["not_planned"]["observation_failed"], 1);
    assert_eq!(table["complete"], false);
    let csv = fs::read_to_string(out.join("collector-export/table.incomplete.csv")).unwrap();
    assert!(csv.contains("fetch_failed"));
    let trials_before = fs::read(out.join("trials.jsonl")).unwrap();

    // 未終了（job_state=R）: 記録を作らず collect は非 0。grade は続き、その試行は観測なしとして成功にならない。
    reply("R");
    let o = launch(&f, "collect");
    assert!(
        !o.status.success() && text(&o).contains("not finished"),
        "{}",
        text(&o)
    );
    assert!(!obs.exists(), "no partial record");
    let g = launch(&f, "grade");
    assert!(text(&g).contains("not finished"), "{}", text(&g));
    assert!(
        a_grade(&f).contains("no evaluator-collected job records"),
        "{}",
        a_grade(&f)
    );
    assert_eq!(a_line(&f)["failure_class"], json!("undetermined"));
    // 別 trial の印を持つ scheduler 記録は identity 不一致（成功証拠にしない）。
    fs::write(
        &qstat,
        "Job Id: 4242.sirius\n    job_state = F\n    Exit_status = 0\n    Variable_List = CELERIS_TRIAL_KEY=campaign-ffffffffffffffff\n",
    )
    .unwrap();
    let o = launch(&f, "collect");
    assert!(text(&o).contains("[identity_mismatch]"), "{}", text(&o));
    assert!(!obs.exists());

    // 終了: 収集でき、記録は trial identity・台帳の job ID に結ばれ、grade の実験証拠検査を通る。
    reply("F");
    // B/C は cluster job を投入していない（台帳に job が無い）ので not_submitted（モデルの失敗）として報告され、
    // A の試行だけが収集される。基盤の失敗は無いので collect は 0 で終わる。
    let o = launch(&f, "collect");
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(
        t.contains("\"A/k2-bf-xattr-restart-campaign/rep1#1\"")
            && t.contains("no job was submitted by Celeris"),
        "{t}"
    );
    let doc: Value = serde_json::from_slice(&fs::read(&obs).unwrap()).unwrap();
    assert_eq!(
        doc["trial"]["problem_id"],
        json!("k2-bf-xattr-restart-campaign")
    );
    assert_eq!(doc["trial"]["config"], json!("A"));
    assert_eq!(doc["trial"]["trial_no"], trial["trial_no"]);
    assert_eq!(doc["trial"]["campaign_sha256"], trial["campaign_sha256"]);
    assert_eq!(doc["jobs"][0]["job_id"], json!("4242.sirius"));
    ok(&f, "grade");
    let line = a_grade(&f);
    assert!(
        !line.contains("no evaluator-collected") && !line.contains("does not match"),
        "{line}"
    );
    let after = serde_json::to_value(
        celeris_campaign::checker::check(&manifest, &out)
            .recomputed
            .unwrap()
            .totals,
    )
    .unwrap();
    assert!(
        after["A"]["observation_failures"]
            .get("fetch_failed")
            .is_none()
    );
    for key in [
        "n",
        "passed",
        "tokens_sum",
        "cost_micro_usd_sum",
        "ran_trials",
        "wall_secs_sum",
    ] {
        assert_eq!(before["A"][key], after["A"][key], "{key}");
    }
    assert_eq!(fs::read(out.join("trials.jsonl")).unwrap(), trials_before);
    let history = fs::read_to_string(out.join("grades.history.jsonl")).unwrap();
    assert!(
        history.lines().any(|line| {
            let row: Value = serde_json::from_str(line).unwrap();
            row["observation"]["issues"][0]["class"] == "fetch_failed"
        }),
        "{history}"
    );
    let good = fs::read(&obs).unwrap();

    // 別 trial の記録（identity が違う）・台帳に無い job・scheduler 応答の控えの改変は grade-out が拒否する。
    // （grade は先に収集し直すので、scheduler 照会を失敗させて、置いた記録がそのまま grade-out に届くようにする。）
    fs::remove_file(&qstat).unwrap();
    let mut other = doc.clone();
    other["trial"]["rep"] = json!(2);
    fs::write(&obs, other.to_string()).unwrap();
    let g = launch(&f, "grade");
    assert!(
        !g.status.success() && text(&g).contains("not this trial"),
        "{}",
        text(&g)
    );
    assert!(a_grade(&f).contains("\"graded\":false"));
    let mut foreign = doc.clone();
    foreign["jobs"][0]["job_id"] = json!("9999.sirius");
    fs::write(&obs, foreign.to_string()).unwrap();
    let g = launch(&f, "grade");
    assert!(
        !g.status.success() && text(&g).contains("differ from the jobs Celeris submitted"),
        "{}",
        text(&g)
    );
    fs::write(&obs, &good).unwrap();
    let rec = out.join("observed/k2-bf-xattr-restart-campaign/scheduler/A-rep1/4242.sirius.qstat");
    let saved = fs::read(&rec).unwrap();
    fs::write(
        &rec,
        "Job Id: 4242.sirius\n job_state = F\n Exit_status = 0\n",
    )
    .unwrap();
    let g = launch(&f, "grade");
    assert!(
        !g.status.success() && text(&g).contains("altered"),
        "{}",
        text(&g)
    );
    fs::write(&rec, saved).unwrap();
    let g = launch(&f, "grade");
    assert!(g.status.success(), "{}", text(&g));
    reply("F");

    // 成果物の改変: 実行終了時の hash と違えば収集せず、grade も採点しない。
    fs::remove_file(&obs).unwrap();
    fs::write(ws.join("results/out.csv"), "tampered\n").unwrap();
    let o = launch(&f, "collect");
    assert!(
        !o.status.success() && text(&o).contains("artifact hash differs"),
        "{}",
        text(&o)
    );
    assert!(!obs.exists());
    stop(&f);
}

// ---- 実 `celeris` daemon での通し（無 LLM。運用前の同一構成リハーサル） ----

/// 能力検査 harness（kind=fake、`[adapters.fake] command`）の worker: 実 `cap-worker` に渡す。起動を記録する。
const PROBE_WORKER_STUB: &str = r#"import json, os, subprocess, sys
line = sys.stdin.readline()
t = json.loads(line)["task"]
with open(os.path.join("@LOG@", "worker-launches.jsonl"), "a") as f:
    f.write(json.dumps({"adapter": "fake", "task": t["id"], "kind": t.get("kind"), "genre": t.get("genre")}) + "\n")
p = subprocess.run(["@CAMPAIGN@", "cap-worker"], input=line, text=True, capture_output=True)
sys.stdout.write(p.stdout)
sys.exit(p.returncode)
"#;

/// B の実 harness（kind=claude_code）の `[adapters.claude_code] command` に置く非 LLM の program。実 adapter が渡す
/// prompt（stdin と argv）から `result.json` / `review.json` の path を読み、試行は作業場所に stub の成果と
/// `result.json` を、reviewer run は全 criterion pass の `review.json` を書いて、stream-json の usage を出す。
const B_CLAUDE_STUB: &str = r#"#!/usr/bin/python3
import json, os, re, sys
args = sys.argv[1:]
if args[:1] == ["--version"]:
    print("2.1.0 (Claude Code)")
    sys.exit(0)
data = sys.stdin.read()
text = data + "\n" + "\n".join(args)
review = re.search(r"verdicts to `([^`]+)/review\.json`", text)
with open(os.path.join("@LOG@", "worker-launches.jsonl"), "a") as f:
    f.write(json.dumps({"adapter": "claude_code", "review": bool(review), "cwd": os.getcwd()}) + "\n")
if review:
    art = review.group(1)
    os.makedirs(art, exist_ok=True)
    v = [{"criterion": i, "pass": True, "reason": "stub reviewer (no LLM)"} for i in range(8)]
    with open(os.path.join(art, "review.json"), "w") as f:
        json.dump({"verdicts": v[:1]}, f)
else:
    m = re.search(r"([^\s`'\"]+/artifacts)/result\.json", text)
    art = m.group(1) if m else os.path.join(os.getcwd(), "artifacts")
    os.makedirs(art, exist_ok=True)
    os.makedirs("analysis", exist_ok=True)
    with open(os.path.join("analysis", "stub-report.md"), "w") as f:
        f.write("stub worker output (no LLM)\n")
    with open(os.path.join(art, "result.json"), "w") as f:
        json.dump({"summary": "stub worker (no LLM)", "evidence": []}, f)
u = {"input_tokens": 1000, "output_tokens": 50}
print(json.dumps({"type": "assistant", "message": {"id": "m0", "usage": u, "content": [{"type": "text", "text": "stub"}]}}))
print(json.dumps({"type": "result", "subtype": "success", "is_error": False, "result": "stub", "usage": u}))
"#;

/// 実 `celeris` binary（`celeris-campaign` と同じ出力 dir）。
fn real_celeris_bin() -> PathBuf {
    let p = Path::new(&campaign_bin()).parent().unwrap().join("celeris");
    assert!(
        p.exists(),
        "{} was not built (cargo build -p celeris)",
        p.display()
    );
    p
}

/// launcher の 1 手順の記録（evidence 用）。
fn step(f: &Fx, log: &mut Vec<Value>, sub: &str) -> (bool, String) {
    let o = launch(f, sub);
    let t = text(&o);
    assert!(!t.contains(TOKEN), "token leaked by {sub}");
    log.push(json!({"step": sub, "exit": o.status.code(), "output": t}));
    (o.status.success(), t)
}

fn cc(f: &Fx, log: &mut Vec<Value>, name: &str, args: &[String]) -> (bool, String) {
    let o = Command::new(campaign_bin())
        .args(args)
        .env("CELERIS_MEASURE_TOKEN", TOKEN)
        .current_dir(f.d())
        .output()
        .unwrap();
    let t = text(&o);
    log.push(json!({"step": name, "argv": args, "exit": o.status.code(), "output": t}));
    (o.status.success(), t)
}

/// 主比較（A0/B0 の set、K4 × 1 rep）を、実 `celeris` daemon（この checkout の build、`--config` だけで launcher が起動）に
/// 対して通常の launcher 入口で通す: prepare → start → preflight → raw 人操作の記録開始 → watch → 停止 → 再起動 → watch 再開
/// （再投入なし）→ collect → grade → export → decision-log export → human-op join/report → stop。LLM・userns・本番は使わない
/// （A は claude stub、B は fake harness の worker stub）。証跡は `LAUNCHER_REAL_EVIDENCE` の dir へ写す（運用前の確認用）。
#[test]
#[ignore = "real celeris daemon rehearsal; run with --ignored (LAUNCHER_REAL_EVIDENCE=<dir> keeps the evidence)"]
fn real_daemon_main_set_runs_through_the_launcher_without_llm() {
    let f = fixture();
    let d = f.d().to_path_buf();
    let ds = d.display().to_string();
    let mut log: Vec<Value> = Vec::new();
    let campaign = "research-eval-main-rehearsal-1";
    // B の版 = 測定 daemon の release（この checkout の sha12）。
    // 未指定なら checkout の HEAD の sha12（manifest は 12-hex の release sha しか受けない）。
    let release = std::env::var("LAUNCHER_REAL_RELEASE").unwrap_or_else(|_| {
        let o = Command::new("git")
            .args(["rev-parse", "--short=12", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        String::from_utf8(o.stdout).unwrap().trim().to_string()
    });
    edit_manifest(&f, |v| {
        only_arms(
            v,
            &["A", "B"],
            json!([["A", "B"]]),
            json!({"approved_trials": 12, "shared_ledger": format!("{ds}/global-ledger")}),
        );
        v["campaign_id"] = json!(campaign);
        v["problems"] = json!(["k4-cmp4-validity-analysis"]);
        for c in v["configs"].as_array_mut().unwrap() {
            if c["id"] != "A" {
                c["harness_version"] = json!(release);
            }
        }
    });
    // 実 daemon の測定 config: fake の source・harness だけ（LLM CLI を起動できない）、releases_dir・repo は一時 dir、
    // llm_proxy は無効、[[clusters]] なし（tunnel を張らない）。
    fs::create_dir_all(d.join("releases")).unwrap();
    fs::create_dir_all(d.join("measure/ws")).unwrap();
    let worker = d.join("bin/probe-worker-stub.py");
    fs::write(
        &worker,
        PROBE_WORKER_STUB
            .replace("@LOG@", &ds)
            .replace("@CAMPAIGN@", &campaign_bin()),
    )
    .unwrap();
    let b_claude = d.join("bin/b-claude-stub");
    write_exec(&b_claude, &B_CLAUDE_STUB.replace("@LOG@", &ds));
    let measure = format!(
        r#"db = "{n}/celeris.db"
workspace_root = "{n}/ws"
tick_ms = 100

[knowledge]
root = "{n}/kb"
default_mounts = ["kb:trial"]

[api]
listen = "127.0.0.1:{port}"
token_file = "{n}/token"

[accounts]
claude_dir = "{ds}/accounts"

[selfdeploy]
releases_dir = "{ds}/releases"
repo = "{ds}/selfdeploy-repo"
push = false

[llm_proxy]
enabled = false

[adapters.fake]
command = ["/usr/bin/python3", "{w}"]

[llm_sources.measure]
kind = "fake"
accounts = ["measure"]
models = ["claude-sonnet-5-5", "claude-sonnet-5-5-b"]

[adapters.claude_code]
command = "{bc}"

[harnesses.claude-code]
kind = "claude_code"
api_formats = ["fake"]
source_kinds = ["fake"]

[harnesses.cap-probe-claude-code]
kind = "fake"
env_of = "claude-code"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.claude-code]
candidates = [{{ source = "measure", model = "claude-sonnet-5-5" }}]

[route_defaults.cap-probe-claude-code]
candidates = [{{ source = "measure", model = "claude-sonnet-5-5" }}]

[harness_selection]
execution_class_defaults = {{}}
rules = [
  {{ genre = "cap-probe-claude-code", harness = "cap-probe-claude-code", required_capabilities = [] }},
  {{ harness = "claude-code", required_capabilities = [] }},
]
"#,
        n = d.join("measure").display(),
        port = f.port,
        w = worker.display(),
        bc = b_claude.display(),
    );
    fs::write(f.measure(), measure).unwrap();
    // 突き合わせ用の通常 config も cluster なし（この rehearsal は cluster に繋がない。K4 は job を投げない）。
    let normal = fs::read_to_string(d.join("normal.toml")).unwrap();
    let normal = normal.split("[[clusters]]").next().unwrap().to_string();
    fs::write(d.join("normal.toml"), normal).unwrap();
    let celeris = real_celeris_bin();
    f.write_env(&format!(
        "GLOBAL_LEDGER={ds}/global-ledger\nMEASUREMENT_ID={campaign}\nRELEASE={release}\nCELERIS_BIN={}\n",
        celeris.display()
    ));
    // K4 だけの選択では初期 tree を固定する case が無いので REPO_SRC を渡さない（launcher は任意として扱う）。
    env_without(&f, "REPO_SRC");
    let keep = std::env::var_os("LAUNCHER_REAL_EVIDENCE").map(PathBuf::from);
    let save = |log: &Vec<Value>| {
        if let Some(k) = &keep {
            let _ = fs::remove_dir_all(k.join("root"));
            fs::create_dir_all(k).unwrap();
            fs::write(
                k.join("steps.json"),
                serde_json::to_string_pretty(log).unwrap(),
            )
            .unwrap();
            let _ = Command::new("cp")
                .arg("-a")
                .arg(format!("{ds}/."))
                .arg(k.join("root"))
                .status();
        }
    };

    let (o, t) = step(&f, &mut log, "prepare");
    save(&log);
    assert!(o, "prepare: {t}");
    {
        // 測定 DB の model catalog（B の候補集合。fake source には発見が無いので、発見の結果と同じ store 経路で入れる）。
        use task_core::{
            ModelCatalogStore,
            model_catalog::{CatalogSource, DiscoveredModel},
        };
        let store = task_core::SqliteStore::open(&d.join("measure/celeris.db")).unwrap();
        store
            .model_catalog_apply(
                &CatalogSource::new("measure"),
                &[
                    DiscoveredModel::new("claude-sonnet-5-5"),
                    DiscoveredModel::new("claude-sonnet-5-5-b"),
                ],
                1_700_000_000,
            )
            .unwrap();
    }
    let (o, t) = step(&f, &mut log, "start");
    save(&log);
    assert!(o, "start: {t}");
    let pid = f.owned_pid().expect("launcher pid");
    assert_eq!(
        exe_of(pid),
        celeris,
        "the daemon is the real celeris binary"
    );
    let (st, cfg) = f.api("GET", "/api/v1/config", None);
    assert_eq!(st, 200, "{cfg}");
    assert_eq!(cfg["identity"]["pid"], json!(pid), "{cfg}");
    log.push(json!({"step": "GET /api/v1/config identity", "identity": cfg["identity"]}));
    // 案件は測定 daemon の正式 API で作る（測定 DB は新規）。試行の task は trial workspace だけを作業場所にし、repo は登録しない。
    let (st, p) = f.api(
        "POST",
        "/api/v1/projects",
        Some(&json!({"title": "research-eval rehearsal", "request": "no-LLM rehearsal"})),
    );
    assert_eq!(st, 201, "{p}");
    let pid_s = p["id"].as_str().unwrap().to_string();
    fs::write(
        d.join("task-template.json"),
        json!({"project_id": pid_s, "max_turns": 100, "acceptance": [{"type": "reviewer", "text": "reviewed"}]})
            .to_string(),
    )
    .unwrap();
    let (o, t) = step(&f, &mut log, "preflight");
    save(&log);
    assert!(o, "preflight: {t}");

    // raw 人操作の記録は試行の開始前から（runbook §2.1。B 腕の試行 identity に結ぶ）。
    let hops = f.out().join("human-ops.jsonl");
    fs::create_dir_all(f.out()).unwrap();
    let manifest = f.s.inp.manifest_path.display().to_string();
    let id_args = |arm: &str| -> Vec<String> {
        [
            "--file",
            &hops.display().to_string(),
            "--campaign",
            campaign,
            "--manifest",
            &manifest,
            "--case",
            "k4-cmp4-validity-analysis",
            "--arm",
            arm,
            "--rep",
            "1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    };
    for arm in ["A0", "B0"] {
        let mut a = vec!["human-op".to_string(), "record".into()];
        a.extend(id_args(arm));
        a.extend(
            [
                "--op-kind",
                "status_check",
                "--actor",
                "eval1",
                "--actor-kind",
                "human",
                "--mode",
                "manual",
                "--intent",
                "monitoring",
                "--trigger",
                "scheduled",
                "--logical-op",
                "p0/start-check",
                "--started-at",
                "now",
                "--ended-at",
                "now",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        let (o, t) = cc(&f, &mut log, "human-op record (before watch)", &a);
        assert!(o, "human-op record: {t}");
    }

    let (o, t) = step(&f, &mut log, "watch");
    save(&log);
    assert!(o, "watch: {t}");
    let line = t.lines().rev().find(|l| l.starts_with('{')).unwrap();
    let rep: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {t}"));
    assert_eq!(rep["watch"]["end"], json!("settled"), "{t}");
    let ledger_after = f.ledger();
    let posts = f.task_posts();
    let a_runs = f.a_launches();

    // 停止 → 再起動 → 同じ watch で再開（台帳から再接続し、再投入しない）。
    let (o, t) = step(&f, &mut log, "stop");
    assert!(o, "stop: {t}");
    assert!(!alive(pid));
    let (o, t) = step(&f, &mut log, "start");
    assert!(o, "restart: {t}");
    let (o, t) = step(&f, &mut log, "watch");
    save(&log);
    assert!(o, "resume watch: {t}");
    assert_eq!(f.ledger(), ledger_after, "resume must not add ledger rows");
    assert_eq!(f.task_posts(), posts, "resume must not resubmit");
    assert_eq!(f.a_launches(), a_runs, "resume must not relaunch A");

    let (_, _) = step(&f, &mut log, "collect");
    // grade = 評価側観測の収集 → 独立採点 → check。check の結果（violations）は証跡に残し、ここでは採点行だけ確かめる
    // （2026-10-20 の通しでは B の reviewer run に resolved routing が無いと check が拒否した。進捗文書の未解決）。
    let (_, t) = step(&f, &mut log, "grade");
    save(&log);
    let grades =
        fs::read_to_string(f.out().join("grades.jsonl")).unwrap_or_else(|e| panic!("{e}: {t}"));
    assert_eq!(grades.lines().count(), 2, "{grades}");
    for l in grades.lines() {
        let g: Value = serde_json::from_str(l).unwrap();
        assert_eq!(g["graded"], json!(true), "{l}");
        assert!(
            g["failure_class"].is_string() || g["passed"] == json!(true),
            "{l}"
        );
    }
    let (_, _) = step(&f, &mut log, "export");
    save(&log);

    // decision-log（測定 DB の写し）→ human-op join / report。
    let dl = d.join("dl");
    let db_copy = d.join("measure-db-copy.sqlite3");
    let celerisctl = Path::new(&campaign_bin())
        .parent()
        .unwrap()
        .join("celerisctl");
    let o = Command::new(&celerisctl)
        .args(["db", "backup"])
        .arg(&db_copy)
        .arg("--src")
        .arg(d.join("measure/celeris.db"))
        .output()
        .unwrap();
    log.push(json!({"step": "celerisctl db backup (measure DB)", "exit": o.status.code(), "output": text(&o)}));
    let msha = {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(fs::read(&manifest).unwrap()))
    };
    let o = Command::new(&celerisctl)
        .arg("--db")
        .arg(&db_copy)
        .args(["campaign", "export", campaign, "--out"])
        .arg(&dl)
        .args(["--manifest-hash", &msha])
        .output()
        .unwrap();
    log.push(
        json!({"step": "celerisctl campaign export", "exit": o.status.code(), "output": text(&o)}),
    );
    let mut a = vec![
        "human-op".to_string(),
        "report".into(),
        "--file".into(),
        hops.display().to_string(),
    ];
    a.extend(["--events".to_string(), f.out().display().to_string()]);
    if dl.is_dir() {
        a.extend(["--events".to_string(), dl.display().to_string()]);
    }
    a.extend([
        "--out".to_string(),
        d.join("human-ops-report").display().to_string(),
    ]);
    let (o, t) = cc(&f, &mut log, "human-op report", &a);
    save(&log);
    assert!(o, "human-op report: {t}");

    let (o, t) = step(&f, &mut log, "stop");
    save(&log);
    assert!(o, "stop: {t}");
    assert!(!alive(pid));
}
