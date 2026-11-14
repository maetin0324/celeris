//! preflight の案件契約: 実 task-api の router（正式 `ProjectDetail` / `RepoList` DTO）の応答 body をそのまま
//! `check_project_rules` と preflight（`gate` / `run_gated`）に通す。手書きの JSON は使わない（案件は
//! `POST /projects`・`POST /projects/{id}/repos`・`PATCH /projects/{id}` で作る）。外部ネットワーク・LLM は使わない。
mod daemon_identity;
mod preflight_setup;
use daemon_identity::api_common;

use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

use api_common::{TestEnv, admin_env, get_admin, post_admin, send};
use celeris_campaign::{
    celeris_exec::{CelerisExecutor, HttpResponse, Transport},
    manifest::CampaignManifest,
    preflight::{Host, Inputs, ResolvedProgram, check, check_project_rules, run_gated},
    runner::RunOptions,
};
use preflight_setup::*;
use serde_json::{Map, Value, json};
use task_core::TaskStore;

/// 実 router で案件を作り（repo の既定は auto_integrate=false）、repo を足す。`auto_integrate` が真なら
/// store で repo の欄を有効にする（API の切り替えは task-api の試験の範囲）。`(project id, detail body, repos body)`。
async fn real_project(env: &TestEnv, auto_integrate: bool) -> (String, Value, Value) {
    let app = env.router();
    let created = send(
        &app,
        post_admin(
            "/api/v1/projects",
            &json!({"title": "campaign", "request": "trial"}),
        ),
    )
    .await;
    assert_eq!(created.status.as_u16(), 201, "{}", created.text());
    let id = created.json()["id"].as_str().expect("id").to_string();
    let repo = send(
        &app,
        post_admin(
            &format!("/api/v1/projects/{id}/repos"),
            &json!({"name": "scratch", "kind": "git", "location": {"kind": "local", "path": "/x"}}),
        ),
    )
    .await;
    assert_eq!(repo.status.as_u16(), 201, "{}", repo.text());
    if auto_integrate {
        let pid = id.parse().expect("project id");
        let mut rows = env.store.repo_list(pid).expect("repo_list");
        let row = rows.first_mut().expect("repo row");
        row.auto_integrate = true;
        assert!(env.store.repo_update(row).expect("repo_update"));
    }
    let detail = send(&app, get_admin(&format!("/api/v1/projects/{id}"))).await;
    assert_eq!(detail.status.as_u16(), 200, "{}", detail.text());
    let repos = send(&app, get_admin(&format!("/api/v1/projects/{id}/repos"))).await;
    assert_eq!(repos.status.as_u16(), 200, "{}", repos.text());
    (id, detail.json(), repos.json())
}

fn auto_integrate_rejected(v: &[String]) -> bool {
    v.iter().any(|m| m.contains("auto_integrate"))
}

#[tokio::test]
async fn real_project_detail_with_repos_auto_integrate_false_passes() {
    let env = admin_env();
    let (_, detail, repos) = real_project(&env, false).await;
    // 実形式の確認: auto_integrate は repos[] の各行にあり、project（案件）には無い。
    assert_eq!(
        detail["repos"][0]["auto_integrate"],
        json!(false),
        "{detail}"
    );
    assert!(
        detail["project"].get("auto_integrate").is_none(),
        "{detail}"
    );
    assert!(repos["items"].is_array(), "{repos}");
    let v = check_project_rules(&detail);
    assert!(v.is_empty(), "{v:?}");
}

#[tokio::test]
async fn real_project_detail_with_an_enabled_repo_is_rejected() {
    let env = admin_env();
    let (_, detail, _) = real_project(&env, true).await;
    assert_eq!(
        detail["repos"][0]["auto_integrate"],
        json!(true),
        "{detail}"
    );
    let v = check_project_rules(&detail);
    assert!(auto_integrate_rejected(&v), "{v:?}");
}

#[tokio::test]
async fn missing_or_malformed_repo_auto_integrate_is_rejected() {
    let env = admin_env();
    let (_, detail, _) = real_project(&env, false).await;
    let mut cases: Vec<(&str, Value)> = Vec::new();
    let mut d = detail.clone();
    d["repos"][0]
        .as_object_mut()
        .expect("repo")
        .remove("auto_integrate");
    cases.push(("auto_integrate missing", d));
    for bad in [json!("false"), json!(0), json!(null), json!({"v": false})] {
        let mut d = detail.clone();
        d["repos"][0]["auto_integrate"] = bad.clone();
        cases.push(("auto_integrate not a bool", d));
    }
    // 1 件でも有効なら、他が無効でも止める。
    let mut d = detail.clone();
    d["repos"] = json!([{"auto_integrate": false}, {"auto_integrate": true}]);
    cases.push(("one of two enabled", d));
    let mut d = detail.clone();
    d.as_object_mut().expect("detail").remove("repos");
    cases.push(("repos missing", d));
    let mut d = detail.clone();
    d["repos"] = json!("nope");
    cases.push(("repos not an array", d));
    // 旧い平坦な形（repos の外に auto_integrate=false）は通さない。
    cases.push(("flat legacy shape", json!({"auto_integrate": false})));
    cases.push(("null body", Value::Null));
    for (what, d) in cases {
        let v = check_project_rules(&d);
        assert!(auto_integrate_rejected(&v), "{what}: {v:?}");
    }
}

/// 実 router の本文を返す Host（GET だけ。他の経路は 404 相当）。
struct RouterHost {
    api: Vec<(String, Value)>,
    acct_dir: std::path::PathBuf,
}

impl Host for RouterHost {
    fn runtime_probe(
        &self,
        r: &celeris_campaign::preflight_isolation::ProbeRequest,
    ) -> Result<celeris_campaign::runtime_probe::RuntimeProbe, String> {
        Ok(r.all_denied())
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

    fn env_nonempty(&self, name: &str) -> bool {
        name == "CELERIS_CAMPAIGN_TOKEN"
    }
    fn resolve_program(&self, p: &Path) -> Result<ResolvedProgram, String> {
        if p == Path::new("claude") {
            Ok(ResolvedProgram {
                path: p.to_path_buf(),
                version: "fake 1.0".into(),
            })
        } else {
            Err("not available".into())
        }
    }
    fn api_get(&self, _: &str, _: &str, path: &str) -> Result<Value, String> {
        if path == "/api/v1/config" {
            return Ok(daemon_identity::matching_daemon_body());
        }
        if path == "/api/v1/llm/accounts" {
            return Ok(
                json!({"items":[{"source_id":"claude_oauth","account_id":"lab","dir":self.acct_dir,"logged_in":true}]}),
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
}

struct CountingTransport(AtomicUsize);

impl Transport for &CountingTransport {
    fn request(&self, _: &str, _: &str, _: Option<&Value>) -> Result<HttpResponse, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("must not be called".into())
    }
}

fn router_host(id: &str, detail: Value, repos: Value) -> RouterHost {
    RouterHost {
        api: vec![
            (format!("/api/v1/projects/{id}"), detail),
            (format!("/api/v1/projects/{id}/repos"), repos),
        ],
        acct_dir: "/tmp".into(),
    }
}

fn with_project_id(s: &Setup, id: &str) -> Inputs {
    fs::write(
        s.dir.path().join("task-template.json"),
        json!({"project_id": id, "max_turns": 100, "acceptance": [{"type": "reviewer", "text": "x"}]}).to_string(),
    )
    .expect("template");
    s.inp.clone()
}

/// preflight 全体: false の実案件なら通り run が始まる。true・欠落・不正形式なら拒否され、A を含む
/// transport 呼び出し（＝外部実行・task 作成）が 0 件で ledger も作られない。
#[tokio::test]
async fn preflight_over_real_router_bodies_gates_external_execution() {
    let env = admin_env();
    let (ok_id, ok_detail, ok_repos) = real_project(&env, false).await;
    let (on_id, on_detail, on_repos) = real_project(&env, true).await;
    let mut missing = ok_detail.clone();
    missing["repos"][0]
        .as_object_mut()
        .expect("repo")
        .remove("auto_integrate");
    let mut malformed = ok_detail.clone();
    malformed["repos"][0]["auto_integrate"] = json!("false");

    let s = setup();
    let m = CampaignManifest::load(&s.inp.manifest_path).expect("manifest");
    let input = m.load_input(&s.inp.manifest_path, None).expect("input");

    // 通る（repo の auto_integrate=false）。
    let inp = with_project_id(&s, &ok_id);
    let r = check(
        &inp,
        &router_host(&ok_id, ok_detail.clone(), ok_repos.clone()),
    );
    assert!(r.ok, "{:?}", r.missing);

    for (what, id, detail, repos) in [
        ("repo auto_integrate=true", &on_id, on_detail, on_repos),
        ("missing", &ok_id, missing, ok_repos.clone()),
        ("malformed", &ok_id, malformed, ok_repos.clone()),
    ] {
        let inp = with_project_id(&s, id);
        let host = router_host(id, detail, repos);
        let r = check(&inp, &host);
        assert!(
            !r.ok
                && r.missing
                    .iter()
                    .any(|x| x.check == "task-template" && x.detail.contains("auto_integrate")),
            "{what}: {:?}",
            r.missing
        );
        let out = s.dir.path().join(format!("out-{}", what.replace('=', "-")));
        let t = CountingTransport(AtomicUsize::new(0));
        let exec = CelerisExecutor::new(&t, Map::new(), 1800, out.join("cancel.jsonl"));
        let e = run_gated(
            &inp,
            &host,
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
        assert!(e.contains("preflight failed"), "{what}: {e}");
        assert_eq!(
            t.0.load(Ordering::SeqCst),
            0,
            "{what}: 外部実行（A を含む）0 件"
        );
        assert!(!out.exists(), "{what}: ledger・out を作らない");
    }
}
