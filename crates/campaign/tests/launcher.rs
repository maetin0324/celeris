//! 測定環境 launcher（`scripts/campaign/measure-launcher.sh`）の無 LLM e2e。
//! 実 script・実 `celeris-campaign` CLI・実 task-api router（実 HTTP。案件は正式 API で作る）を通す。
//! 実行内部だけ stub: A の `claude`（`--version` と呼び出し記録）、`bwrap`、測定 daemon の実行ファイル（常駐するだけの script）。
//! 拒否は外部実行 0 件（A の claude が `--version` 以外で呼ばれない・台帳が増えない・task を作る POST が 0）で確かめる。
//! 実 LLM・外部ネットワーク・本番 host の変更は無い。
mod daemon_identity;
mod preflight_setup;

use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use daemon_identity::{api_common::*, daemon_config_view};
use preflight_setup::{MEASURE_RELEASE, Setup, measure_config, setup};
use serde_json::{Value, json};

fn script() -> PathBuf {
    preflight_setup::root().join("../../scripts/campaign/measure-launcher.sh")
}

fn write_exec(p: &Path, body: &str) {
    fs::write(p, body).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
}

struct Server {
    posts: Arc<AtomicUsize>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    handle: tokio::task::JoinHandle<()>,
}

impl Server {
    /// `served_config` で起動した daemon として答える実 router。案件は正式 API で作り id を返す。
    async fn start(
        listener: tokio::net::TcpListener,
        served_config: &Path,
        accounts: &Path,
        pidfile: PathBuf,
    ) -> (Self, String) {
        let port = listener.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap().keep();
        let db = dir.join("celeris.db");
        let ws = dir.join("ws");
        fs::create_dir_all(&ws).unwrap();
        task_core::SqliteStore::open(&db).unwrap();
        let mut s = settings(
            &db,
            &ws,
            &dir.join("docs"),
            &dir.join("kb"),
            EnvOptions {
                token: Some(TOKEN.to_string()),
                allowed_hosts: vec![format!("127.0.0.1:{port}")],
                accounts_root: Some(accounts.to_path_buf()),
                ..EnvOptions::default()
            },
        );
        s.config_view = daemon_config_view(served_config, MEASURE_RELEASE);
        // B/C の候補集合・階層（実 store に正式 API と同じ操作で入れ、`GET /llm/models` が返す）。
        {
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
        let (tx, rx) = tokio::sync::watch::channel(None);
        std::mem::forget(tx);
        let state = task_api::ApiState::new(s, rx).unwrap();
        let app = task_api::router(state.clone());
        let created = send(
            &app,
            post_admin(
                "/api/v1/projects",
                &json!({"title": "campaign", "request": "trial"}),
            ),
        )
        .await;
        let id = created.json()["id"].as_str().unwrap().to_string();
        let repo = send(
            &app,
            post_admin(
                &format!("/api/v1/projects/{id}/repos"),
                &json!({"name": "scratch", "location": {"kind": "local", "path": "/x"}}),
            ),
        )
        .await;
        assert_eq!(repo.status.as_u16(), 201, "{}", repo.text());
        let posts = Arc::new(AtomicUsize::new(0));
        let counter = posts.clone();
        let counted = task_api::router(state.clone()).layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| {
                let counter = counter.clone();
                let pidfile = pidfile.clone();
                async move {
                    if req.method() == axum::http::Method::POST {
                        counter.fetch_add(1, Ordering::SeqCst);
                    }
                    let is_config = req.uri().path() == "/api/v1/config";
                    let resp = next.run(req).await;
                    if !is_config {
                        return resp;
                    }
                    // 常駐 stub の process が daemon 役: 実 router の本文のうち、自分の pid だけを launcher 所有 pid にする。
                    let (parts, body) = resp.into_parts();
                    let bytes = axum::body::to_bytes(body, 1 << 20)
                        .await
                        .unwrap_or_default();
                    let mut v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                    if let Some(pid) = fs::read_to_string(&pidfile)
                        .ok()
                        .and_then(|t| t.trim().parse::<u32>().ok())
                    {
                        v["identity"]["pid"] = json!(pid);
                    }
                    let mut r = axum::response::Response::from_parts(
                        parts,
                        axum::body::Body::from(v.to_string()),
                    );
                    r.headers_mut().remove(axum::http::header::CONTENT_LENGTH);
                    r
                }
            },
        ));
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            axum::serve(listener, counted)
                .with_graceful_shutdown(async move {
                    let _ = stopped.await;
                })
                .await
                .expect("serve");
        });
        (
            Server {
                posts,
                stop: Some(stop),
                handle,
            },
            id,
        )
    }

    async fn shutdown(mut self) {
        let _ = self.stop.take().unwrap().send(());
        let _ = tokio::time::timeout(Duration::from_secs(30), self.handle).await;
    }
}

struct Fx {
    s: Setup,
    port: u16,
    listener: Option<tokio::net::TcpListener>,
    project: Option<String>,
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
    fn claude_log(&self) -> String {
        fs::read_to_string(self.d().join("bin/claude.log")).unwrap_or_default()
    }
    fn ledger(&self) -> Vec<u8> {
        fs::read(self.out().join("ledger.jsonl")).unwrap_or_default()
    }
}

/// 一時 port・stub・env ファイルまで揃える。daemon 側（server）は `serve` で起動する。
async fn fixture() -> Fx {
    let s = setup();
    let d = s.dir.path().to_path_buf();
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let measure =
        measure_config(&d, "measure").replace("127.0.0.1:7701", &format!("127.0.0.1:{port}"));
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
        &format!(
            "#!/bin/sh\necho \"$*\" >> {}/claude.log\n[ \"$1\" = --version ] && echo claude-code@2.1.0\nexit 0\n",
            bin.display()
        ),
    );
    write_exec(&bin.join("bwrap"), "#!/bin/sh\nexit 0\n");
    // 測定 daemon の stub: 常駐するだけ（HTTP は test の実 router が答える）。
    write_exec(
        &bin.join("celeris"),
        &format!(
            "#!/bin/sh\necho \"$*\" > {}/celeris.args\nwhile :; do sleep 0.2; done\n",
            bin.display()
        ),
    );
    Fx {
        s,
        port,
        listener: Some(listener),
        project: None,
    }
}

impl Fx {
    fn write_env(&self, extra: &str) {
        let d = self.d();
        let i = &self.s.inp;
        let p = |v: &Option<PathBuf>| v.as_ref().unwrap().display().to_string();
        fs::write(
            self.env(),
            format!(
                "MANIFEST={m}\nDAEMON_CONFIG={d}/measure.toml\nNORMAL_CONFIG={d}/normal.toml\nCANARIES={d}/canaries.json\n\
                 TASK_TEMPLATE={d}/task-template.json\nOUT={d}/out\nSTATE_DIR={d}/state\nRELEASE={rel}\n\
                 CELERIS_BIN={d}/bin/celeris\nRUN_PATH={d}/bin\nA_PROGRAM=claude\nA_ACCOUNT=lab\nWORKER_PROGRAMS=\"claude=claude\"\n\
                 MATERIALS_SRC={mat}\nKB_SRC={kb}\nREPO_SRC={repo}\nINPUTS_SRC={ins}\n\
                 CASES_ROOT={cases}\nGRADER_ROOT={gr}\nGRADE_CASES_ROOT={gc}\nCAMPAIGN_BIN={cb}\n\
                 EXECUTOR=mock\nINTERVAL_SECS=1\nHEALTH_WAIT_SECS=20\n{extra}",
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
                cb = std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"),
            ),
        )
        .unwrap();
    }

    async fn serve(&mut self, served_config: &Path) -> Server {
        let l = self.listener.take().unwrap();
        let (srv, project) = Server::start(
            l,
            served_config,
            &self.d().join("accounts"),
            self.d().join("state/launcher.pid"),
        )
        .await;
        self.project = Some(project.clone());
        fs::write(
            self.d().join("task-template.json"),
            json!({"project_id": project, "acceptance":[{"type":"reviewer","text":"x"}]})
                .to_string(),
        )
        .unwrap();
        srv
    }
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
    text(&o)
}

/// 外部実行 0 件で拒否された: 非 0・メッセージ・A の起動（`--version` 以外）0・task を作る POST 0・台帳が増えない。
fn assert_rejected(f: &Fx, srv: &Server, sub: &str, ledger_before: &[u8], needle: &str) {
    let o = launch(f, sub);
    let t = text(&o);
    assert!(!o.status.success(), "{sub} must be rejected: {t}");
    assert!(t.contains(needle), "expected {needle:?} in: {t}");
    assert!(
        f.claude_log().lines().all(|l| l.trim() == "--version"),
        "A was launched: {}",
        f.claude_log()
    );
    assert_eq!(srv.posts.load(Ordering::SeqCst), 0, "a POST was issued");
    assert_eq!(f.ledger(), ledger_before, "ledger changed");
    assert!(!t.contains(TOKEN), "token leaked");
}

fn kill_daemon(f: &Fx) {
    let _ = launch(f, "stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn matching_environment_runs_through_grade_and_export_then_resumes_with_a_fixed_condition() {
    let mut f = fixture().await;
    f.write_env("");
    let measure = f.d().join("measure.toml");
    let srv = f.serve(&measure).await;
    let t = ok(&f, "prepare");
    assert!(t.contains(&format!("127.0.0.1:{}", f.port)), "{t}");
    ok(&f, "start");
    assert!(
        fs::read_to_string(f.d().join("bin/celeris.args"))
            .unwrap()
            .contains(&format!("--config {}", measure.display()))
    );
    ok(&f, "preflight");
    let w = ok(&f, "watch");
    assert!(w.contains("\"executed\""), "{w}");
    assert!(f.out().join("env-lock.json").exists());
    let lock = fs::read(f.out().join("env-lock.json")).unwrap();
    let g = launch(&f, "grade");
    // 採点は独立採点の判定そのもの（mock の成果物は不合格でもよい）。launcher が採点を呼べていることを見る。
    assert!(
        f.out().join("grades.jsonl").exists(),
        "grade-out did not run: {}",
        text(&g)
    );
    let e = launch(&f, "export");
    assert!(
        e.status.success(),
        "export: {}\ngrade: {}",
        text(&e),
        text(&g)
    );
    let exported: Vec<String> = fs::read_dir(f.out().join("export"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        exported.iter().any(|n| n.starts_with("table")),
        "{exported:?}"
    );
    // 再開: 同じ入口。env-lock は作り直さず一致を確認する。
    let before = f.ledger();
    ok(&f, "watch");
    assert_eq!(fs::read(f.out().join("env-lock.json")).unwrap(), lock);
    assert_eq!(f.ledger(), before);
    // 秘密値は固定記録にもログにも無い。
    let state = f.d().join("state");
    for p in [
        f.out().join("env-lock.json"),
        state.join("daemon.log"),
        state.join("launcher.meta"),
    ] {
        let body = fs::read_to_string(&p).unwrap_or_default();
        assert!(!body.contains(TOKEN), "{}", p.display());
    }
    // 再開時に固定条件が変わっていれば拒否: 同一 path の A の差替え（版文字列は同じ）。
    write_exec(
        &f.d().join("bin/claude"),
        &format!(
            "#!/bin/sh\necho \"$*\" >> {}/bin/claude.log\n# swapped\n[ \"$1\" = --version ] && echo claude-code@2.1.0\nexit 0\n",
            f.d().display()
        ),
    );
    assert_rejected(&f, &srv, "watch", &before, "env-lock");
    kill_daemon(&f);
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_different_daemon_behind_the_url_is_rejected_with_zero_execution() {
    let mut f = fixture().await;
    f.write_env("");
    // 別 daemon（通常運用の config で起動したもの）が測定 config の listen で答える。
    let other = f.d().join("normal.toml");
    let srv = f.serve(&other).await;
    ok(&f, "prepare");
    let o = launch(&f, "start");
    assert!(!o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("env-check"), "{}", text(&o));
    kill_daemon(&f);
    assert_rejected(&f, &srv, "preflight", b"", "effective.api_listen");
    assert_rejected(&f, &srv, "watch", b"", "preflight failed");
    assert!(!f.out().exists(), "out must not be created");
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn effective_config_differences_are_rejected_with_zero_execution() {
    let mut f = fixture().await;
    f.write_env("");
    // daemon が起動した後に config が書き換わった（起動時の bytes の hash と違う）。
    let measure = f.d().join("measure.toml");
    let srv = f.serve(&measure).await;
    ok(&f, "prepare");
    let mut body = fs::read_to_string(&measure).unwrap();
    body.push_str("\n# edited after the daemon started\n");
    fs::write(&measure, body).unwrap();
    assert_rejected(&f, &srv, "watch", b"", "config_sha256");
    assert!(!f.out().exists());
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_version_mismatch_and_missing_inputs_are_rejected_with_zero_execution() {
    let mut f = fixture().await;
    f.write_env("");
    let measure = f.d().join("measure.toml");
    let srv = f.serve(&measure).await;
    ok(&f, "prepare");
    ok(&f, "start");
    // 版不一致: A の実際の版が manifest の harness_version と違う。
    let claude = f.d().join("bin/claude");
    let saved = fs::read_to_string(&claude).unwrap();
    fs::write(&claude, saved.replace("2.1.0", "2.0.9")).unwrap();
    assert_rejected(
        &f,
        &srv,
        "watch",
        b"",
        "does not match manifest A harness_version",
    );
    fs::write(&claude, saved).unwrap();
    // 必須入力の欠落: 外部入力 dir が無い。
    let ins = f.s.inp.inputs_src.clone().unwrap();
    fs::rename(&ins, f.d().join("inputs-gone")).unwrap();
    assert_rejected(&f, &srv, "watch", b"", "preflight failed");
    let o = launch(&f, "prepare");
    assert!(
        !o.status.success() && text(&o).contains("required input missing"),
        "{}",
        text(&o)
    );
    fs::rename(f.d().join("inputs-gone"), &ins).unwrap();
    // 必須の設定欠落（env ファイルの A_ACCOUNT）。
    f.write_env("A_ACCOUNT=\n");
    assert_rejected(&f, &srv, "watch", b"", "A_ACCOUNT is required");
    assert!(!f.out().exists());
    kill_daemon(&f);
    srv.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swapped_daemon_binary_is_rejected_and_stop_only_touches_the_launcher_pid() {
    let mut f = fixture().await;
    f.write_env("");
    let measure = f.d().join("measure.toml");
    let srv = f.serve(&measure).await;
    ok(&f, "prepare");
    // 同一 path の daemon 実行ファイル差替え → start は起動しない。
    let daemon = f.d().join("bin/celeris");
    let saved = fs::read_to_string(&daemon).unwrap();
    fs::write(&daemon, format!("{saved}# swapped\n")).unwrap();
    let o = launch(&f, "start");
    assert!(!o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("differs"), "{}", text(&o));
    assert!(!f.d().join("state/launcher.pid").exists());
    fs::write(&daemon, saved).unwrap();

    // 本番 daemon に見立てた無関係の process（別 pidfile）。launcher の stop は触らない。
    let mut prod = Command::new("sleep").arg("300").spawn().unwrap();
    let prod_pid = prod.id();
    let prod_pidfile = f.d().join("production.pid");
    fs::write(&prod_pidfile, prod_pid.to_string()).unwrap();

    ok(&f, "start");
    let mine: u32 = fs::read_to_string(f.d().join("state/launcher.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_ne!(mine, prod_pid);
    // pidfile が（pid 再利用で）別 process を指しても stop は送らない。
    let real = fs::read_to_string(f.d().join("state/launcher.pid")).unwrap();
    fs::write(f.d().join("state/launcher.pid"), prod_pid.to_string()).unwrap();
    let o = launch(&f, "stop");
    assert!(!o.status.success(), "{}", text(&o));
    assert!(Path::new(&format!("/proc/{prod_pid}")).exists());
    assert!(
        prod.try_wait().unwrap().is_none(),
        "production process was signalled"
    );
    // 本来の pidfile に戻すと launcher 所有の pid だけが止まる。
    fs::write(f.d().join("state/launcher.pid"), real).unwrap();
    let t = ok(&f, "stop");
    assert!(t.contains(&format!("stopped pid {mine}")), "{t}");
    assert!(!Path::new(&format!("/proc/{mine}")).exists());
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
    srv.shutdown().await;
}
