//! env-check: 接続先 daemon が測定 config で起動した daemon であることの照合。daemon の応答は実 task-api router
//! （正式 `ConfigView` DTO・`GET /api/v1/config`）の本文をそのまま使う。別 daemon・実効 config の相違・版の不一致・
//! api-url の食い違いは、有料実行（A を含む transport 呼び出し）0 件・out dir 作成 0 件で拒否される。
//! 実 LLM・外部ネットワークは使わない。
mod daemon_identity;
mod preflight_setup;

use std::{
    fs,
    path::Path,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use celeris_campaign::{
    celeris_exec::{CelerisExecutor, HttpResponse, Transport},
    env_check,
    manifest::CampaignManifest,
    preflight::{Host, Inputs, ResolvedProgram, check, run_gated},
    runner::RunOptions,
};
use daemon_identity::{api_common::*, daemon_config_view};
use preflight_setup::{MEASURE_API_URL, MEASURE_RELEASE, Setup, setup};
use serde_json::{Map, Value, json};

/// 実 router の `GET /api/v1/config` の本文（`view` を config_view にした daemon が返すもの）。
async fn served(view: task_api::ConfigView) -> Value {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("celeris.db");
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).expect("ws");
    task_core::SqliteStore::open(&db).expect("store");
    let mut settings = settings(
        &db,
        &ws,
        &dir.path().join("docs"),
        &dir.path().join("kb"),
        EnvOptions {
            token: Some(TOKEN.to_string()),
            ..EnvOptions::default()
        },
    );
    settings.config_view = view;
    let (_tx, rx) = tokio::sync::watch::channel(None);
    let state = task_api::ApiState::new(settings, rx).expect("api state");
    let r = send(&task_api::router(state), get_admin("/api/v1/config")).await;
    assert_eq!(r.status.as_u16(), 200, "{}", r.text());
    r.json()
}

/// 実 router で作った案件（auto_integrate=false）。`(project id, detail, repos)`。
async fn real_project() -> (String, Value, Value) {
    let env = admin_env();
    let app = env.router();
    let created = send(
        &app,
        post_admin(
            "/api/v1/projects",
            &json!({"title": "campaign", "request": "trial"}),
        ),
    )
    .await;
    let id = created.json()["id"].as_str().expect("id").to_string();
    let repo = send(
        &app,
        post_admin(
            &format!("/api/v1/projects/{id}/repos"),
            &json!({"name": "scratch", "location": {"kind": "local", "path": "/x"}}),
        ),
    )
    .await;
    assert_eq!(repo.status.as_u16(), 201, "{}", repo.text());
    let d = send(&app, get_admin(&format!("/api/v1/projects/{id}"))).await;
    let r = send(&app, get_admin(&format!("/api/v1/projects/{id}/repos"))).await;
    (id, d.json(), r.json())
}

struct DaemonHost {
    api: Vec<(String, Value)>,
    config_gets: AtomicUsize,
}

impl Host for DaemonHost {
    fn env_nonempty(&self, name: &str) -> bool {
        name == "CELERIS_CAMPAIGN_TOKEN"
    }
    fn resolve_program(&self, p: &Path) -> Result<ResolvedProgram, String> {
        Ok(ResolvedProgram {
            path: p.to_path_buf(),
            version: "fake 1.0".into(),
        })
    }
    fn api_get(&self, _: &str, _: &str, path: &str) -> Result<Value, String> {
        if path == "/api/v1/config" {
            self.config_gets.fetch_add(1, Ordering::SeqCst);
        }
        if path == "/api/v1/llm/accounts" {
            return Ok(
                json!({"items":[{"source_id":"claude_oauth","account_id":"lab","dir":"/tmp","logged_in":true}]}),
            );
        }
        self.api
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| format!("GET {path}: HTTP 404"))
    }
    fn sandbox_usable(&self) -> Result<(), String> {
        Ok(())
    }
    fn probe_runner(&self) -> Box<dyn celeris_campaign::cap_probe::ProbeRunner> {
        Box::new(preflight_setup::OkRunner)
    }
    fn probe_daemon_worker(
        &self,
        _: &str,
        _: &str,
        spec: &celeris_campaign::cap_probe::WorkerProbeSpec,
    ) -> Result<celeris_campaign::cap_probe::WorkerProbeRun, String> {
        Ok(preflight_setup::ok_worker_run(spec))
    }
}

struct Fixture {
    s: Setup,
    inp: Inputs,
    project: (String, Value, Value),
}

async fn fixture() -> Fixture {
    let project = real_project().await;
    let s = setup();
    fs::write(
        s.dir.path().join("task-template.json"),
        json!({"project_id": project.0, "acceptance": [{"type": "reviewer", "text": "x"}]})
            .to_string(),
    )
    .expect("template");
    let inp = s.inp.clone();
    Fixture { s, inp, project }
}

impl Fixture {
    fn config(&self) -> std::path::PathBuf {
        self.inp.measure.as_ref().expect("measure").config.clone()
    }
    fn host(&self, config_body: Value) -> DaemonHost {
        DaemonHost {
            api: vec![
                (
                    format!("/api/v1/projects/{}", self.project.0),
                    self.project.1.clone(),
                ),
                (
                    format!("/api/v1/projects/{}/repos", self.project.0),
                    self.project.2.clone(),
                ),
                ("/api/v1/config".into(), config_body),
            ],
            config_gets: AtomicUsize::new(0),
        }
    }
    /// 測定 config で起動した daemon の DTO。
    fn view(&self) -> task_api::ConfigView {
        daemon_config_view(&self.config(), MEASURE_RELEASE)
    }
    fn missing(&self, inp: &Inputs, h: &DaemonHost) -> Vec<String> {
        check(inp, h)
            .missing
            .into_iter()
            .filter(|m| m.check == "env-check")
            .map(|m| m.detail)
            .collect()
    }
}

struct Counting(AtomicUsize);

impl Transport for &Counting {
    fn request(&self, _: &str, _: &str, _: Option<&Value>) -> Result<HttpResponse, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("must not be called".into())
    }
}

/// 拒否: preflight が落ち、`run_gated` は A を含む外部実行 0 件・out 作成なし。
fn assert_gated(f: &Fixture, host: &DaemonHost, expect: &str) {
    let r = check(&f.inp, host);
    assert!(
        !r.ok
            && r.missing
                .iter()
                .any(|m| m.check == "env-check" && m.detail.contains(expect)),
        "{expect}: {:?}",
        r.missing
    );
    let m = CampaignManifest::load(&f.inp.manifest_path).expect("manifest");
    let input = m.load_input(&f.inp.manifest_path, None).expect("input");
    let out = f.s.dir.path().join("out-gated");
    let t = Counting(AtomicUsize::new(0));
    let exec = CelerisExecutor::new(&t, Map::new(), 1800, out.join("cancel.jsonl"));
    let e = run_gated(
        &f.inp,
        host,
        &m,
        &input,
        &exec,
        &RunOptions {
            global_ledger: None,
            out: out.clone(),
            ..Default::default()
        },
    )
    .expect_err("gate");
    assert!(e.contains("preflight failed"), "{expect}: {e}");
    assert_eq!(t.0.load(Ordering::SeqCst), 0, "{expect}: 外部実行 0 件");
    assert!(!out.exists(), "{expect}: out を作らない");
}

#[tokio::test]
async fn matching_daemon_passes_preflight() {
    let f = fixture().await;
    let body = served(f.view()).await;
    // 実形式: identity は ConfigView の中。
    assert_eq!(
        body["identity"]["release"],
        json!(MEASURE_RELEASE),
        "{body}"
    );
    let h = f.host(body);
    let r = check(&f.inp, &h);
    assert!(r.ok, "{:?}", r.missing);
    assert!(r.checked.contains(&"env-check"));
    assert_eq!(
        h.config_gets.load(Ordering::SeqCst),
        1,
        "GET /config は 1 回"
    );
}

#[tokio::test]
async fn different_daemon_is_rejected_without_paid_calls() {
    let f = fixture().await;
    // 別 daemon（例: 通常運用）が同じ URL で答える: 別 config・別 DB・別 instance・別 release。
    let mut v = f.view();
    v.config_path = "/home/x/.config/celeris/config.toml".into();
    v.identity.instance_id = "01NORMALDAEMON0000000000000".into();
    v.identity.release = "ffffffffffff".into();
    v.identity.config_sha256 = Some("0".repeat(64));
    v.identity.effective.db = "/home/x/.local/celeris/celeris.db".into();
    v.identity.effective.api_listen = "127.0.0.1:7700".into();
    let h = f.host(served(v).await);
    for expect in [
        "runs config",
        "config_sha256",
        "identity.release",
        "effective.db",
        "effective.api_listen",
    ] {
        assert_gated(&f, &h, expect);
    }
}

#[tokio::test]
async fn effective_config_differences_are_rejected_without_paid_calls() {
    let f = fixture().await;
    type Edit = Box<dyn Fn(&mut task_api::ConfigView)>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "effective.knowledge_root",
            Box::new(|v| v.identity.effective.knowledge_root = "/home/x/kb".into()),
        ),
        (
            "effective.memory_dir",
            Box::new(|v| v.identity.effective.memory_dir = Some("/home/x/memory".into())),
        ),
        (
            "effective.workspace_root",
            Box::new(|v| v.identity.effective.workspace_root = "/home/x/ws".into()),
        ),
        (
            "effective.knowledge_default_mounts",
            Box::new(|v| v.identity.effective.knowledge_default_mounts = vec!["kb:user".into()]),
        ),
        (
            "effective.langmem_enabled",
            Box::new(|v| v.identity.effective.langmem_enabled = true),
        ),
        (
            "effective.cron_seed_count",
            Box::new(|v| v.identity.effective.cron_seed_count = 2),
        ),
        (
            "effective.selfdeploy_push",
            Box::new(|v| v.identity.effective.selfdeploy_push = false),
        ),
        (
            "CLI overrides",
            Box::new(|v| v.identity.overrides = vec!["db".into()]),
        ),
        ("`dev`", Box::new(|v| v.identity.release = "dev".into())),
        (
            "config_sha256",
            Box::new(|v| v.identity.config_sha256 = None),
        ),
    ];
    for (expect, edit) in cases {
        let mut v = f.view();
        edit(&mut v);
        assert_gated(&f, &f.host(served(v).await), expect);
    }
}

/// 起動後に config ファイルが書き換わった（daemon は古い bytes で動いている）。
#[tokio::test]
async fn config_edited_after_daemon_start_is_rejected() {
    let f = fixture().await;
    let body = served(f.view()).await;
    let cfg = f.config();
    fs::write(
        &cfg,
        format!(
            "{}\n# edited after start\n",
            fs::read_to_string(&cfg).unwrap()
        ),
    )
    .unwrap();
    assert_gated(&f, &f.host(body), "config_sha256");
}

/// api-url が測定 config の `[api] listen` から導いた値と違えば、接続も token 送出もせず拒否する。
#[tokio::test]
async fn api_url_must_match_the_derived_origin_and_is_not_contacted() {
    let f = fixture().await;
    let body = served(f.view()).await;
    assert_eq!(
        env_check::derive_api_url(&f.config()).as_deref(),
        Ok(MEASURE_API_URL)
    );
    for url in [
        "http://127.0.0.1:7700",
        "http://localhost:7701",
        "http://10.0.0.1:7701",
    ] {
        let mut inp = f.inp.clone();
        inp.api_url = Some(url.into());
        let h = f.host(body.clone());
        let miss = f.missing(&inp, &h);
        assert!(
            miss.iter().any(|d| d.contains("api origin")),
            "{url}: {miss:?}"
        );
        assert_eq!(h.config_gets.load(Ordering::SeqCst), 0, "{url}: 接続しない");
    }
    // 末尾の `/` は同じ origin。
    let mut inp = f.inp.clone();
    inp.api_url = Some(format!("{MEASURE_API_URL}/"));
    assert!(f.missing(&inp, &f.host(body)).is_empty());
}

#[test]
fn listen_wildcards_derive_loopback() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("c.toml");
    for (listen, want) in [
        ("0.0.0.0:7801", "http://127.0.0.1:7801"),
        ("[::]:7801", "http://[::1]:7801"),
        ("127.0.0.1:7801", "http://127.0.0.1:7801"),
    ] {
        fs::write(&p, format!("[api]\nlisten = \"{listen}\"\n")).unwrap();
        assert_eq!(env_check::derive_api_url(&p).as_deref(), Ok(want));
    }
    fs::write(&p, "[api]\n").unwrap();
    assert!(env_check::derive_api_url(&p).is_err());
}

/// daemon が identity を返さない（旧い daemon）なら拒否する。
#[tokio::test]
async fn daemon_without_identity_is_rejected() {
    let f = fixture().await;
    let mut body = served(f.view()).await;
    body.as_object_mut().unwrap().remove("identity");
    assert_gated(&f, &f.host(body), "no daemon identity");
}

/// CLI: `celeris-campaign env-check` は api-url の食い違いを接続前に拒否し、不足を JSON で返す。
#[tokio::test]
async fn cli_env_check_rejects_wrong_api_url_before_connecting() {
    let f = fixture().await;
    let me = f.inp.measure.as_ref().unwrap();
    let out = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"))
        .arg("env-check")
        .arg("--manifest")
        .arg(&f.inp.manifest_path)
        .arg("--measure-config")
        .arg(&me.config)
        .arg("--normal-config")
        .arg(&me.normal_config)
        .arg("--canaries")
        .arg(&me.canaries)
        .args(["--api-url", "http://127.0.0.1:7700"])
        .args(["--token-env", "CELERIS_CAMPAIGN_TOKEN"])
        .env("CELERIS_CAMPAIGN_TOKEN", "s3cret-token-value-xyz")
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("api origin"), "{err}");
    assert!(err.contains("\"paid_calls_made\": 0"), "{err}");
    assert!(!err.contains("s3cret-token-value-xyz"));
}

#[tokio::test]
async fn pid_that_is_not_the_launcher_owned_daemon_is_rejected_without_paid_calls() {
    let f = fixture().await;
    // 同じ URL・同じ config・同じ release で別 process（別 pid）が答える。
    let mut v = f.view();
    v.identity.pid = preflight_setup::DAEMON_PID + 1;
    let h = f.host(served(v).await);
    assert_gated(&f, &h, "identity.pid");
    // 一致すれば通る。
    let h = f.host(served(f.view()).await);
    assert!(check(&f.inp, &h).ok);
}

#[tokio::test]
async fn missing_daemon_pid_argument_is_rejected() {
    let mut f = fixture().await;
    f.inp.measure.as_mut().expect("measure").daemon_pid = None;
    let h = f.host(served(f.view()).await);
    assert_gated(&f, &h, "--daemon-pid");
}
