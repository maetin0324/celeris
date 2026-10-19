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
                 EXECUTOR=celeris\nINTERVAL_SECS=1\nMAX_ITERATIONS=600\nHEALTH_WAIT_SECS=60\n{extra}",
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
            json!({"project_id": id, "acceptance": [{"type": "reviewer", "text": "x"}]})
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
    assert_rejected(&f, "watch", "A_ACCOUNT is required");
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
