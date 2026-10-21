//! A の試行 state 分離と CLI 停止・再起動の e2e（実 CLI 入口 → preflight → executor）。無 LLM・無 network。
//! - 実 `celeris-campaign watch --executor harness` を子 process として起動する。task-api は実 router（正式 DTO）を
//!   loopback の TCP で serve し、案件・アカウントは実 router の `POST /projects` と `[accounts] claude_dir` で作る。
//! - B/C の能力検査 task は、daemon の dispatcher の代役が daemon の環境（CLI の環境ではない）で実 `cap-worker` を起こして走らせる。
//! - A の実行ファイルは履歴（transcript・memory）に試行固有の文字列を書く stub。bwrap の中で動く実 process。
//! - CLI は SIGSTOP → SIGKILL で止め、同じ引数で再起動する。時間は gate file（state dir 内）と出来事待ちで進める。
mod daemon_identity;
mod preflight_setup;

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use celeris_campaign::{
    ledger::{Ledger, LedgerRow, LedgerState},
    manifest::CampaignManifest,
};
use daemon_identity::{
    api_common::{EnvOptions, TOKEN, TestEnv, post_admin, send, settings},
    daemon_config_view,
};
use preflight_setup::*;
use serde_json::{Value, json};

const CRED: &str = "FAKE-CREDENTIAL-VALUE-MUST-NOT-LEAK";
const WAIT: Duration = Duration::from_secs(120);

/// 実 router を loopback で serve する環境（案件・A のアカウントは実 API／実 scan で作る）。
struct Env {
    s: Setup,
    _api: TestEnv,
    _acct: tempfile::TempDir,
    url: String,
    project_id: String,
    stub: PathBuf,
    campaign_id: String,
    /// 測定 daemon の代役の実 process（identity が答える pid）。CLI の PATH とは別の環境を持つ。
    daemon: Child,
    /// daemon の dispatcher の代役（検査 task を、daemon の環境で実 `cap-worker` process に走らせる）。
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    standin: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(h) = self.standin.take() {
            let _ = h.join();
        }
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// 測定 daemon の dispatcher 相当（この試験の関心は A の試行 state と CLI の停止・再開。daemon の worker 起動経路そのものは
/// `crates/celeris/tests/cap_probe_real_daemon.rs` が実 `celeris` で確かめる）。cap-probe の検査 task を、daemon の環境
/// （`env_clear` + `worker_path`。CLI の環境ではない）で実 `celeris-campaign cap-worker` process に走らせ、worker の
/// outcome を通常の event（`worker_finished`）で残して完了させる。
fn spawn_standin_dispatcher(
    db: PathBuf,
    ws: PathBuf,
    worker_path: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    use std::io::Write;
    use std::sync::atomic::Ordering;
    use task_core::{Event, RunEnd, SqliteStore, Status, TaskStore, Trigger};
    std::thread::spawn(move || {
        let store = SqliteStore::open(&db).unwrap();
        while !stop.load(Ordering::SeqCst) {
            for t in store.list(Some(Status::Ready)).unwrap() {
                if !t
                    .genre
                    .as_deref()
                    .is_some_and(|g| g.starts_with(celeris_campaign::cap_probe::PROBE_GENRE_PREFIX))
                {
                    continue;
                }
                store
                    .apply_transition(t.id, Trigger::Dispatch, None)
                    .unwrap();
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
                let mut c = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").unwrap())
                    .arg("cap-worker")
                    .env_clear()
                    .env("PATH", &worker_path)
                    .current_dir(&work)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap();
                let mut stdin = c.stdin.take().unwrap();
                stdin
                    .write_all(format!("{}\n", serde_json::to_string(&req).unwrap()).as_bytes())
                    .unwrap();
                drop(stdin);
                let out = c.wait_with_output().unwrap();
                let done: Value =
                    serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
                let run = format!("standin-{}", t.id);
                store
                    .apply_transition_with_events(
                        t.id,
                        Trigger::WorkerDone,
                        vec![
                            Event::WorkerStarted {
                                run_id: run.clone(),
                                adapter: "fake".into(),
                                model: "fake".into(),
                                provider: Some("cap-probe".into()),
                                account: None,
                                role: None,
                                task_role: None,
                            },
                            Event::WorkerFinished {
                                run_id: run,
                                outcome: format!("done: {}", done["summary"].as_str().unwrap()),
                                usage: None,
                                role: None,
                                metrics: None,
                                end: Some(RunEnd::Completed),
                            },
                        ],
                    )
                    .unwrap();
                store
                    .apply_transition(t.id, Trigger::ReviewPass, None)
                    .unwrap();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })
}

fn write_executable(path: &Path, contents: &str) {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).unwrap();
    let ok = Command::new("sh")
        .arg("-c")
        .arg(r#"chmod 755 "$1" && mv "$1" "$2""#)
        .arg("sh")
        .arg(&tmp)
        .arg(path)
        .status()
        .unwrap();
    assert!(ok.success());
}

/// 履歴に試行固有の文字列（`HIST-<arm>-<case>-<rep>`）を書き、他試行の履歴が見えるか探して記録し、
/// gate が開くまで待ってから終わる。起動のたびに `launches` へ 1 行足す（二重投入の検出）。
const STUB: &str = r#"#!/bin/sh
if [ "$1" = --version ]; then echo 'stub 1.0'; exit 0; fi
d="$CLAUDE_CONFIG_DIR"
tag="HIST-$(echo "$d" | awk -F/ '{print $(NF-2)"-"$(NF-1)"-"$NF}')"
mkdir -p "$d/projects/p" "$d/memory"
echo "$tag" >> "$d/projects/p/transcript.jsonl"
echo "$tag" >> "$d/memory/MEMORY.md"
root="$(dirname "$(dirname "$(dirname "$(dirname "$d")")")")"
grep -rl 'HIST-' "$root" 2>/dev/null | grep -v "^$d/" > "$d/foreign.txt"
ls "$root" > "$d/root-listing.txt" 2>&1
echo x >> "$d/launches"
echo '{"type":"assistant","message":{"id":"m0","usage":{"input_tokens":100,"output_tokens":5}}}'
echo ready > "$d/probe-done"
while [ ! -e "$d/GATE" ]; do sleep 0.05; done
echo '{"type":"assistant","message":{"id":"m1","usage":{"input_tokens":10,"output_tokens":5}}}'
"#;

/// 必要能力 probe 用の道具の stub（どの expect_stdout も満たす）。
const CAP_STUB: &str = "#!/bin/sh\necho 'cmake version git version Python 3 rustc-min ok'\n";

/// worker の PATH に足す `sh`。能力 probe の複合 command（`sh -c`）だけ成功で返し、他は実 sh に渡す
/// （PATH の先頭に置かれるので、他の script が呼ぶ `sh` を壊さない）。
const CAP_SH_STUB: &str = r#"#!/bin/sh
case "$2" in
  *min-build*|*printf*|*autoreconf*|*libclang*|*verbs.h*) echo 'cmake version git version Python 3 rustc-min ok'; exit 0 ;;
esac
exec /bin/sh "$@"
"#;

async fn env() -> Env {
    let s = setup();
    let worker_path = format!(
        "{}:/usr/local/bin:/usr/bin:/bin",
        s.dir.path().join("cap-tools").display()
    );
    let daemon = Command::new("/bin/sleep")
        .arg("600")
        .env_clear()
        .env("PATH", &worker_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let acct = tempfile::tempdir().unwrap();
    let lab = acct.path().join("lab");
    fs::create_dir_all(&lab).unwrap();
    fs::write(lab.join(".credentials.json"), CRED).unwrap();
    // 他の試行から見えてはならない、アカウント dir 側の履歴。
    fs::create_dir_all(lab.join("projects/old")).unwrap();
    fs::write(lab.join("projects/old/t.jsonl"), "HIST-ACCOUNT-DIR-OLD\n").unwrap();
    let options = || EnvOptions {
        token: Some(TOKEN.into()),
        accounts_root: Some(acct.path().to_path_buf()),
        max_runs_per_account: 2,
        ..Default::default()
    };
    let mut api = TestEnv::with(options());
    // 測定 config で起動した daemon として `GET /config` に答える（env-check の接続先照合）。
    let mut st = settings(
        &api.db_path,
        &api.workspace_root,
        &api.docs_repo_root,
        &api.knowledge_root,
        options(),
    );
    // 測定 config の `[api] listen` を実際に serve する loopback の port に合わせる。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let measure = current_config();
    let text = fs::read_to_string(&measure).unwrap();
    fs::write(&measure, text.replace("127.0.0.1:7701", &addr)).unwrap();
    st.config_view = daemon_config_view(&measure, MEASURE_RELEASE);
    st.config_view.identity.pid = daemon.id();
    api.state = task_api::ApiState::new(st, api.daemon_tx.subscribe()).unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let standin = spawn_standin_dispatcher(
        api.db_path.clone(),
        api.workspace_root.clone(),
        worker_path.clone(),
        stop.clone(),
    );
    let created = send(
        &api.router(),
        post_admin(
            "/api/v1/projects",
            &json!({"title": "campaign", "request": "trial"}),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let project_id = created.json()["id"].as_str().unwrap().to_string();
    let repo = send(
        &api.router(),
        post_admin(
            &format!("/api/v1/projects/{project_id}/repos"),
            &json!({"name": "scratch", "location": {"kind": "local", "path": "/x"}}),
        ),
    )
    .await;
    assert_eq!(repo.status.as_u16(), 201, "{}", repo.text());
    let url = format!("http://{addr}");
    let state = api.state.clone();
    tokio::spawn(async move {
        let _ = task_api::serve_with_listener(listener, state, std::future::pending()).await;
    });
    fs::write(
        s.dir.path().join("task-template.json"),
        json!({"project_id": project_id, "max_turns": 100, "acceptance": [{"type": "reviewer", "text": "x"}]})
            .to_string(),
    )
    .unwrap();
    let stub = s.dir.path().join("a-stub");
    write_executable(&stub, STUB);
    // 必要能力 probe は実 bwrap で走る。この host に無い道具（UCX 等）は stub を限定 mount で足す。
    let tools = s.dir.path().join("cap-tools");
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
        write_executable(
            &tools.join(t),
            if t == "sh" { CAP_SH_STUB } else { CAP_STUB },
        );
    }
    let tc = json!({
        "ro_mounts": [tools],
        "env": [["PATH", format!("{}:/usr/local/bin:/usr/bin:/bin", tools.display())]],
    });
    fs::write(
        s.inp.cap_env.as_ref().unwrap(),
        json!({"schema": "celeris.campaign-cap-env/1", "a": tc, "worker": tc}).to_string(),
    )
    .unwrap();
    let campaign_id = CampaignManifest::load(&s.inp.manifest_path)
        .unwrap()
        .campaign_id;
    Env {
        s,
        _api: api,
        _acct: acct,
        url,
        project_id,
        stub,
        campaign_id,
        daemon,
        stop,
        standin: Some(standin),
    }
}

impl Env {
    fn out(&self) -> PathBuf {
        self.s.inp.measure.as_ref().unwrap().out.clone().unwrap()
    }

    fn state_root(&self) -> PathBuf {
        self.out().join("trial-state")
    }

    fn cmd(&self) -> Command {
        let d = self.s.dir.path();
        let i = &self.s.inp;
        let mut c = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").unwrap());
        c.args(["watch", "--executor", "harness", "--interval-secs", "1"])
            .args(["--max-iterations", "600", "--manifest"])
            .arg(&i.manifest_path)
            .arg("--out")
            .arg(self.out())
            .arg("--cases-root")
            .arg(&i.cases_root)
            .arg("--grader-root")
            .arg(&i.grader_root)
            .arg("--grade-case-root")
            .arg(&i.grade_cases_root)
            .arg("--a-program")
            .arg(&self.stub)
            .arg("--materials-src")
            .arg(i.materials_src.as_ref().unwrap())
            .arg("--kb-src")
            .arg(i.kb_src.as_ref().unwrap())
            .arg("--inputs-src")
            .arg(i.inputs_src.as_ref().unwrap())
            .arg("--repo-src")
            .arg(i.repo_src.as_ref().unwrap())
            .arg("--cap-env")
            .arg(i.cap_env.as_ref().unwrap())
            .args(["--a-account", "lab", "--api-url", &self.url])
            .args(["--token-env", "CELERIS_E2E_TOKEN"])
            .arg("--task-template")
            .arg(d.join("task-template.json"))
            .args(["--daemon-pid", &self.daemon.id().to_string()])
            .arg("--measure-config")
            .arg(d.join("measure.toml"))
            .arg("--normal-config")
            .arg(d.join("normal.toml"))
            .arg("--canaries")
            .arg(d.join("canaries.json"))
            // CLI の PATH は worker の PATH と別（B/C の probe は daemon process の環境を見る）。
            .env("CELERIS_E2E_TOKEN", TOKEN);
        c
    }

    /// 背景で走らせ、stdout/stderr は file へ（pipe を詰まらせない）。
    fn spawn(&self, tag: &str) -> (Child, PathBuf, PathBuf) {
        let (o, e) = (
            self.s.dir.path().join(format!("{tag}.stdout")),
            self.s.dir.path().join(format!("{tag}.stderr")),
        );
        let child = self
            .cmd()
            .stdin(Stdio::null())
            .stdout(fs::File::create(&o).unwrap())
            .stderr(fs::File::create(&e).unwrap())
            .spawn()
            .unwrap();
        (child, o, e)
    }

    /// `trial-state/<campaign>/<arm>/<case>/rep<N>` のうち、identity marker がある dir。
    fn state_dirs(&self) -> Vec<PathBuf> {
        let mut v = Vec::new();
        let base = self.state_root().join(&self.campaign_id);
        for arm in ls(&base) {
            for case in ls(&arm) {
                for rep in ls(&case) {
                    if rep.join("trial-identity.json").is_file() {
                        v.push(rep);
                    }
                }
            }
        }
        v
    }

    fn a_state(&self, case: &str) -> PathBuf {
        self.state_root()
            .join(&self.campaign_id)
            .join("A")
            .join(case)
            .join("rep1")
    }
}

fn ls(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(p)
        .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let t = Instant::now();
    while !ok() {
        assert!(t.elapsed() < WAIT, "timed out: {what}");
        sleep(Duration::from_millis(25));
    }
}

fn kill(sig: &str, pid: u32) {
    assert!(
        Command::new("kill")
            .arg(format!("-{sig}"))
            .arg(pid.to_string())
            .status()
            .unwrap()
            .success()
    );
}

fn proc_state(pid: u32) -> char {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    s[s.rfind(')').map_or(0, |i| i + 1)..]
        .trim_start()
        .chars()
        .next()
        .unwrap_or('?')
}

fn rows(out: &Path) -> Vec<LedgerRow> {
    Ledger::open(out).unwrap().rows().cloned().collect()
}

fn lines(p: &Path) -> usize {
    fs::read_to_string(p).map_or(0, |s| s.lines().count())
}

fn text(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_default()
}

fn no_secret(files: &[&Path]) {
    for f in files {
        assert!(!text(f).contains(CRED), "credential leaked into {f:?}");
        assert!(!text(f).contains(TOKEN), "api token leaked into {f:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_cli_restarts_and_reattaches_to_the_same_trial_without_resubmit() {
    let e = env().await;
    let out = e.out();
    let first_case = "k2-bf-xattr-restart-campaign";

    // 1 回目の CLI: A（k2）の stub が履歴を書いて gate 待ちになるまで進める。
    let (mut c1, o1, e1) = e.spawn("cli1");
    let a_dir = e.a_state(first_case);
    wait_for("first stub wrote history", || {
        a_dir.join("probe-done").exists()
    });
    assert_eq!(lines(&a_dir.join("launches")), 1);
    let before: Vec<LedgerRow> = rows(&out);
    let a_row = before
        .iter()
        .find(|r| format!("{:?}", r.key.arm) == "A" && r.key.problem_id == first_case)
        .expect("A row")
        .clone();
    assert!(!a_row.state.is_terminal());
    assert!(a_row.reserved.is_some(), "tokens are reserved at submit");

    // CLI を SIGSTOP（固まった CLI）→ 動かないことを確かめ、その間に stub は先へ進む → SIGKILL（落ちた CLI）。
    kill("STOP", c1.id());
    wait_for("cli stopped", || proc_state(c1.id()) == 'T');
    fs::write(a_dir.join("GATE"), b"1").unwrap();
    wait_for("stub consumed more while the CLI was stopped", || {
        stub_exited(&out)
    });
    kill("KILL", c1.id());
    let _ = c1.wait();
    let ledger_after_kill = rows(&out);
    assert_eq!(
        ledger_after_kill.len(),
        before.len(),
        "a dead CLI plans nothing"
    );
    assert_eq!(
        ledger_after_kill
            .iter()
            .find(|r| r.key == a_row.key)
            .unwrap()
            .reserved,
        a_row.reserved,
        "reservation is untouched by the crash"
    );

    // 再開検査: marker が別試行の識別に書き換わっていれば、起動前に拒否される（投入 0・launch 増なし）。
    let marker = a_dir.join("trial-identity.json");
    let good = fs::read(&marker).unwrap();
    let mut bad: Value = serde_json::from_slice(&good).unwrap();
    bad["rep"] = json!(9);
    fs::write(&marker, bad.to_string()).unwrap();
    let rej = e.cmd().output().unwrap();
    assert_eq!(rej.status.code(), Some(1), "{}", lossy(&rej.stderr));
    assert!(
        lossy(&rej.stderr).contains("trial-state"),
        "{}",
        lossy(&rej.stderr)
    );
    assert_eq!(
        lines(&a_dir.join("launches")),
        1,
        "no relaunch on rejection"
    );
    fs::write(&marker, &good).unwrap();

    // 再起動: 同じ引数。台帳から再開し、gate は出来事（新しい state dir の出現）に合わせて開ける。
    let (mut c2, o2, e2) = e.spawn("cli2");
    let mut opened: BTreeSet<PathBuf> = BTreeSet::from([a_dir.clone()]);
    let t = Instant::now();
    let status = loop {
        for d in e.state_dirs() {
            if d.join("probe-done").exists() && opened.insert(d.clone()) {
                fs::write(d.join("GATE"), b"1").unwrap();
            }
        }
        if let Some(s) = c2.try_wait().unwrap() {
            break s;
        }
        assert!(t.elapsed() < WAIT, "restarted CLI did not settle");
        sleep(Duration::from_millis(25));
    };
    assert!(status.success(), "{}", text(&e2));
    let report: Value = serde_json::from_str(&text(&o2)).unwrap();
    assert_eq!(report["watch"]["end"], "settled", "{report}");

    // 同じ実行へ再接続: 全試行で process は 1 回しか起動していない。台帳の行は増えず、全て終端。
    let dirs = e.state_dirs();
    assert_eq!(dirs.len(), 6, "A/B/C × 2 cases");
    for d in &dirs {
        assert_eq!(lines(&d.join("launches")), 1, "double launch in {d:?}");
    }
    let after = rows(&out);
    // max_parallel=1 なので残りの試行は再起動後に順に plan される。同じ試行の行は 1 つだけ（再 plan・再投入なし）。
    assert_eq!(after.len(), 6);
    assert_eq!(
        after
            .iter()
            .map(|r| r.key.clone())
            .collect::<BTreeSet<_>>()
            .len(),
        6
    );
    assert!(after.iter().all(|r| r.state == LedgerState::Done));
    let kept = after.iter().find(|r| r.key == a_row.key).unwrap();
    assert_eq!(kept.idempotency_key, a_row.idempotency_key);
    assert_eq!(kept.reserved, a_row.reserved, "reservation kept to the end");
    // 消費は 0 に戻らず、再起動をまたいで合算される（m0: 105 + m1: 15）。
    let recs: Vec<Value> = text(&out.join("trials.jsonl"))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(recs.len(), 6);
    assert!(recs.iter().all(|r| r["tokens"] == 120), "{recs:?}");

    // 試行間の分離: 各 stub は自分の識別文字列だけを持ち、他試行の履歴（と account dir の履歴）を読めない。
    let mut tags = BTreeSet::new();
    for d in &dirs {
        let tag = text(&d.join("projects/p/transcript.jsonl"));
        assert_eq!(tag, text(&d.join("memory/MEMORY.md")));
        assert_eq!(tag.lines().count(), 1, "history of exactly one launch");
        assert!(tag.starts_with("HIST-"), "{tag}");
        tags.insert(tag.trim().to_string());
        assert_eq!(
            text(&d.join("foreign.txt")),
            "",
            "foreign history visible from {d:?}"
        );
        assert!(
            !text(&d.join("root-listing.txt")).contains("rep"),
            "{}",
            text(&d.join("root-listing.txt"))
        );
        assert!(
            !d.join("projects/old").exists(),
            "account dir history reached the trial"
        );
        // 認証材料は read-only mount の mount 先（空 file）だけ。state dir に複製されない。
        assert_eq!(fs::metadata(d.join(".credentials.json")).unwrap().len(), 0);
    }
    assert_eq!(tags.len(), 6, "{tags:?}");
    no_secret(&[&o1, &e1, &o2, &e2]);
    assert!(!text(&e.state_root()).contains(CRED));
    assert!(e.project_id.len() > 4);
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// gate を開けた A の stub が終了した（`exit.code` が出た）か。
fn stub_exited(out: &Path) -> bool {
    ls(&out.join("harness-state"))
        .iter()
        .any(|d| d.join("exit.code").exists())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contaminated_new_trial_state_is_refused_before_any_launch() {
    let e = env().await;
    let out = e.out();
    let leak = e.a_state("k2-bf-xattr-restart-campaign");
    fs::create_dir_all(leak.join("projects/other")).unwrap();
    fs::write(leak.join("projects/other/t.jsonl"), "HIST-OTHER-TRIAL\n").unwrap();
    let o = e.cmd().output().unwrap();
    assert_eq!(o.status.code(), Some(1), "{}", lossy(&o.stderr));
    let err = lossy(&o.stderr);
    assert!(
        err.contains("trial-state") && err.contains("not empty"),
        "{err}"
    );
    assert!(!err.contains(CRED) && !lossy(&o.stdout).contains(CRED));
    // 起動前: 台帳も harness-state も作られず、混入は消されずそのまま残る（削除して回避しない）。
    assert!(!out.join("ledger.jsonl").exists());
    assert!(!out.join("harness-state").exists());
    assert!(leak.join("projects/other/t.jsonl").is_file());

    // 別の試行の identity marker を持ち込んだ state も同じく拒否される。
    fs::remove_dir_all(leak.join("projects")).unwrap();
    fs::write(
        leak.join("trial-identity.json"),
        json!({"campaign_id":"other","manifest_sha256":"x","arm":"A","case":"c","rep":1,"account_id":"lab"})
            .to_string(),
    )
    .unwrap();
    let o = e.cmd().output().unwrap();
    assert_eq!(o.status.code(), Some(1), "{}", lossy(&o.stderr));
    assert!(lossy(&o.stderr).contains("trial-state"));
    assert!(!out.join("harness-state").exists());
}

/// socket / toolchain が欠けると env_failure で最初の投入前に止まる（台帳も作らない）。A が落ちている間は B/C の検査 task を投入しない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_worker_socket_or_toolchain_stops_b_and_c_as_env_failure_before_launch() {
    let e = env().await;
    let tools = e.s.dir.path().join("cap-tools");
    let cap_env = e.s.inp.cap_env.as_ref().unwrap();
    let base: Value = serde_json::from_str(&fs::read_to_string(cap_env).unwrap()).unwrap();
    let absent_sock = e.s.dir.path().join("absent-link.sock");
    let absent_mount = e.s.dir.path().join("absent-prefix");
    for (label, extra) in [
        ("socket", json!({"sockets": [absent_sock]})),
        ("toolchain", json!({"ro_mounts": [tools, absent_mount]})),
    ] {
        let mut v = base.clone();
        for side in ["a", "worker"] {
            for (k, x) in extra.as_object().unwrap() {
                v[side][k] = x.clone();
            }
        }
        fs::write(cap_env, v.to_string()).unwrap();
        let o = e.cmd().output().unwrap();
        let err = lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(1), "{label}: {err}");
        assert!(err.contains("env_failure"), "{label}: {err}");
        // cap env は A と worker が同じ toolchain を要る。A の検査が落ちている preflight は B/C の検査 task を投入しない
        // （B/C 側の欠落が env_failure になる経路は crates/celeris/tests/cap_probe_real_daemon.rs が実 daemon で確かめる）。
        assert!(err.contains("arm A"), "{label}: {err}");
        for arm in ["arm B", "arm C"] {
            assert!(
                !err.contains(arm),
                "{label} {arm} must not be probed: {err}"
            );
        }
        assert!(!e.out().join("ledger.jsonl").exists(), "{label}");
    }
}
