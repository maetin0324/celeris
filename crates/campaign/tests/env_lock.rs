//! 固定記録 env-lock（ADR 2026-10-18-measure-env-launcher D3）。実 binary `celeris-campaign env-lock` を stub 実行ファイル
//! （実 LLM なし）で叩く: 作成・再開一致、同一 path の binary 差替え、版不一致、manifest hash 不一致、必須入力欠落の拒否、
//! 秘密値が env-lock.json に入らないこと。
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};
use task_core::{
    ModelCatalogStore, Tier,
    model_catalog::{CatalogOverride, CatalogSource, DiscoveredModel},
};

#[path = "../../task-api/tests/common/mod.rs"]
mod api_common;
use api_common::{EnvOptions, TOKEN, TestEnv};

/// 実 task-api router（正式 `GET /api/v1/llm/models`）を loopback で serve する測定 daemon 役。
/// catalog は実 store（別接続）へ実 API と同じ操作（apply・set_override）で入れる。
struct Daemon {
    env: TestEnv,
    url: String,
}

impl Daemon {
    fn start() -> Self {
        let env = TestEnv::with(EnvOptions {
            token: Some(TOKEN.to_string()),
            ..EnvOptions::default()
        });
        let state = env.state.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("rt");
            rt.block_on(async {
                let l = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind");
                tx.send(l.local_addr().expect("addr")).expect("send");
                let _ = task_api::serve_with_listener(l, state, std::future::pending()).await;
            });
        });
        let addr = rx.recv().expect("addr");
        let d = Daemon {
            env,
            url: format!("http://{addr}"),
        };
        d.add("opencode-go", "glm-5");
        d.add("opencode-go", "kimi");
        d.add("claude-oauth", "claude-haiku");
        d.tier("opencode-go", "glm-5", Some(Tier::Cheap), false);
        d.tier("opencode-go", "kimi", Some(Tier::Standard), false);
        d.tier("claude-oauth", "claude-haiku", Some(Tier::Frontier), false);
        d
    }
    fn add(&self, source: &str, model: &str) {
        self.env
            .store
            .model_catalog_apply(
                &CatalogSource::new(source),
                &[DiscoveredModel::new(model)],
                1_700_000_000,
            )
            .expect("apply");
    }
    fn tier(&self, source: &str, model: &str, tier: Option<Tier>, disabled: bool) {
        self.env
            .store
            .model_catalog_set_override(
                &CatalogSource::new(source),
                model,
                &CatalogOverride {
                    disabled,
                    tier,
                    alias: None,
                    note: None,
                },
                1_700_000_001,
            )
            .expect("override");
    }
}

fn write_executable(path: &Path, contents: &str) {
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

const SECRET: &str = "sk-ant-TOP-SECRET-VALUE-0123";
const REL: &str = "0123456789ab";

struct Env {
    dir: tempfile::TempDir,
    daemon: Daemon,
}

impl Env {
    fn new() -> Self {
        let e = Env {
            dir: tempfile::tempdir().expect("tmp"),
            daemon: Daemon::start(),
        };
        write_executable(&e.a(), "#!/bin/sh\necho 'claude-stub 2.0.1'\n");
        write_executable(&e.worker(), "#!/bin/sh\necho 'codex-stub 0.9'\n");
        e.manifest(REL, "2.0.1");
        e
    }
    fn p(&self, n: &str) -> PathBuf {
        self.dir.path().join(n)
    }
    fn a(&self) -> PathBuf {
        self.p("claude")
    }
    fn worker(&self) -> PathBuf {
        self.p("codex")
    }
    fn out(&self) -> PathBuf {
        self.p("out")
    }
    fn manifest(&self, rel: &str, a_version: &str) {
        let cfg = |id: &str, h: &str, v: &str, routing: Value| {
            let mut c = json!({"id": id, "harness": h, "harness_version": v, "model": "m",
                "model_version": "1", "policy_version": "p"});
            if !routing.is_null() {
                c["routing"] = routing;
            }
            c
        };
        let routing = json!({"lower_policy": "tier-v1", "evidence_table_id": "ev-1", "online_update": false,
            "models": ["m-small", "m-big"]});
        let m = json!({
            "schema": "celeris.campaign-manifest/1", "campaign_id": "c1", "version": "v1", "split": "final",
            "input": {"path": "input.json", "sha256": "00"}, "problems": ["p0"],
            "configs": [cfg("A", "claude-code", a_version, Value::Null),
                        cfg("B", "celeris", rel, routing.clone()), cfg("C", "celeris", rel, routing)],
            "primary": ["A", "B"], "secondary": {"pair": ["B", "C"], "subset": ["p0"]},
            "budget": {"token_basis": "input_output", "max_tokens": 1000, "max_wall_secs": 10, "max_disk_bytes": 1,
                "max_parallel": 1, "per_case": {"max_tokens": 1, "max_wall_secs": 1},
                "per_arm": {"max_tokens": 1, "max_wall_secs": 1}},
            "termination": {"trial_timeout_secs": 1, "max_attempts_per_trial": 1, "stop_on_budget": true},
            "snapshot": {"repo_commit": "a", "materials_sha256": "b", "kb_snapshot": "k"},
        });
        fs::write(
            self.p("manifest.json"),
            serde_json::to_vec(&m).expect("json"),
        )
        .expect("manifest");
    }
    fn run(&self, extra: &[&str]) -> Output {
        self.run_at(&self.daemon.url, extra)
    }
    fn run_at(&self, url: &str, extra: &[&str]) -> Output {
        Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("bin"))
            .arg("env-lock")
            .arg("--manifest")
            .arg(self.p("manifest.json"))
            .arg("--out")
            .arg(self.out())
            .arg("--a-program")
            .arg(self.a())
            .arg("--worker-program")
            .arg(format!("codex={}", self.worker().display()))
            .args(["--api-url", url])
            .args(["--token-env", "CELERIS_ENVLOCK_TOKEN"])
            .args(extra)
            .env("CELERIS_ENVLOCK_TOKEN", TOKEN)
            // 秘密に見える環境変数があっても固定記録には入らない。
            .env("ANTHROPIC_API_KEY", SECRET)
            .output()
            .expect("run")
    }
    fn lock(&self) -> String {
        fs::read_to_string(self.out().join("env-lock.json")).expect("env-lock.json")
    }
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn creates_then_verifies_on_resume_with_no_secret() {
    let e = Env::new();
    assert!(
        e.run(&["--verify"]).status.code() != Some(0),
        "verify without lock must fail"
    );
    assert!(!e.out().join("env-lock.json").exists());
    let o = e.run(&[]);
    assert!(o.status.success(), "{}", err(&o));
    let body = e.lock();
    let v: Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["schema"], "celeris-env-lock/2");
    assert_eq!(v["bc"]["release"], REL);
    assert_eq!(v["a"]["program"]["version"], "claude-stub 2.0.1");
    assert_eq!(v["a"]["program"]["sha256"].as_str().map(str::len), Some(64));
    assert_eq!(v["campaign_sha256"].as_str().map(str::len), Some(64));
    assert_eq!(v["bc"]["arms"][0]["lower_policy"], "tier-v1");
    assert_eq!(v["bc"]["arms"][0]["evidence_table_id"], "ev-1");
    assert_eq!(v["bc"]["worker_programs"][0]["name"], "codex");
    assert!(
        !body.contains(SECRET)
            && !body.to_lowercase().contains("token")
            && !body.contains("API_KEY")
    );
    // 再開・--verify は一致。
    assert!(e.run(&[]).status.success());
    assert!(e.run(&["--verify"]).status.success());
}

#[test]
fn binary_swap_at_same_path_is_refused_even_with_same_version() {
    let e = Env::new();
    assert!(e.run(&[]).status.success());
    let before = e.lock();
    // 同じ path・同じ --version 出力で、実体だけ違う。
    write_executable(&e.a(), "#!/bin/sh\n# tampered\necho 'claude-stub 2.0.1'\n");
    let o = e.run(&[]);
    assert!(!o.status.success());
    assert!(err(&o).contains("a.program.sha256"), "{}", err(&o));
    assert_eq!(before, e.lock(), "lock must not be rewritten");
}

#[test]
fn version_mismatch_with_manifest_is_refused_and_writes_nothing() {
    let e = Env::new();
    e.manifest(REL, "9.9.9");
    let o = e.run(&[]);
    assert!(!o.status.success());
    assert!(err(&o).contains("a.program.version"), "{}", err(&o));
    assert!(!e.out().join("env-lock.json").exists());
    // B/C の release が sha12 でなければ拒否。
    e.manifest("latest", "2.0.1");
    let o = e.run(&[]);
    assert!(!o.status.success());
    assert!(err(&o).contains("bc.release"), "{}", err(&o));
}

#[test]
fn changed_manifest_or_release_or_worker_is_refused_on_resume() {
    let e = Env::new();
    assert!(e.run(&[]).status.success());
    // worker harness の実体が変わる。
    write_executable(&e.worker(), "#!/bin/sh\necho 'codex-stub 1.0'\n");
    let o = e.run(&[]);
    assert!(!o.status.success());
    assert!(err(&o).contains("bc.worker_programs"), "{}", err(&o));
    write_executable(&e.worker(), "#!/bin/sh\necho 'codex-stub 0.9'\n");
    assert!(e.run(&[]).status.success());
    // release を変えた manifest（hash が変わる）。
    e.manifest("ba9876543210", "2.0.1");
    let o = e.run(&[]);
    assert!(!o.status.success());
    let s = err(&o);
    assert!(
        s.contains("campaign_sha256") && s.contains("bc.release"),
        "{s}"
    );
    // 保存 lock の manifest hash を書き換えても、現物から作り直した期待値と食い違う。
    e.manifest(REL, "2.0.1");
    let mut v: Value = serde_json::from_str(&e.lock()).expect("json");
    v["campaign_sha256"] = json!("0".repeat(64));
    fs::write(
        e.out().join("env-lock.json"),
        serde_json::to_vec(&v).expect("json"),
    )
    .expect("write");
    let o = e.run(&["--verify"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("campaign_sha256"), "{}", err(&o));
}

#[test]
fn manifest_sha256_field_mismatch_is_refused() {
    let e = Env::new();
    let mut m: Value =
        serde_json::from_slice(&fs::read(e.p("manifest.json")).expect("read")).expect("json");
    m["sha256"] = json!("1".repeat(64));
    fs::write(e.p("manifest.json"), serde_json::to_vec(&m).expect("json")).expect("write");
    let o = e.run(&[]);
    assert!(!o.status.success());
    assert!(err(&o).contains("campaign_sha256"), "{}", err(&o));
    assert!(!e.out().join("env-lock.json").exists());
}

#[test]
fn missing_required_input_is_refused() {
    let e = Env::new();
    fs::remove_file(e.a()).expect("rm");
    let o = e.run(&[]);
    assert!(!o.status.success());
    assert!(err(&o).contains("not an executable"), "{}", err(&o));
    assert!(!e.out().join("env-lock.json").exists());
    // worker の版が取れない（--version が失敗）も拒否。
    let e = Env::new();
    write_executable(&e.worker(), "#!/bin/sh\nexit 3\n");
    let o = e.run(&[]);
    assert!(!o.status.success());
    assert!(!e.out().join("env-lock.json").exists());
}

fn catalog_of(e: &Env, arm: usize) -> Value {
    serde_json::from_str::<Value>(&e.lock()).expect("json")["bc"]["arms"][arm]["catalog"].clone()
}

#[test]
fn catalog_is_fixed_from_the_daemon_without_secrets() {
    let e = Env::new();
    let o = e.run(&[]);
    assert!(o.status.success(), "{}", err(&o));
    for arm in [0, 1] {
        assert_eq!(
            catalog_of(&e, arm),
            json!([
                {"model_id": "claude-haiku", "tier": "frontier", "source_id": "claude-oauth"},
                {"model_id": "glm-5", "tier": "cheap", "source_id": "opencode-go"},
                {"model_id": "kimi", "tier": "standard", "source_id": "opencode-go"},
            ])
        );
    }
    let body = e.lock();
    assert!(!body.contains(TOKEN) && !body.contains(SECRET), "{body}");
    assert!(!body.contains("capabilities"), "{body}");
    assert!(e.run(&["--verify"]).status.success());
}

#[test]
fn empty_catalog_is_refused_and_nothing_is_written() {
    let e = Env::new();
    for m in ["glm-5", "kimi"] {
        e.daemon.tier("opencode-go", m, None, true);
    }
    e.daemon.tier("claude-oauth", "claude-haiku", None, true);
    let o = e.run(&[]);
    assert!(!o.status.success());
    assert!(err(&o).contains("empty catalog"), "{}", err(&o));
    assert!(!e.out().join("env-lock.json").exists());
}

#[test]
fn unreachable_daemon_is_refused_and_nothing_is_written() {
    let e = Env::new();
    let o = e.run_at("http://127.0.0.1:9", &[]);
    assert!(!o.status.success());
    assert!(!e.out().join("env-lock.json").exists());
}

#[test]
fn candidate_added_removed_or_moved_tier_is_refused_on_resume() {
    let e = Env::new();
    assert!(e.run(&[]).status.success());
    let before = e.lock();
    // 追加。
    e.daemon.add("opencode-go", "newcomer");
    let o = e.run(&[]);
    assert!(!o.status.success());
    assert!(err(&o).contains("opencode-go/newcomer"), "{}", err(&o));
    assert!(err(&o).contains("bc.arms.B.catalog"), "{}", err(&o));
    // 除外（削除相当）。
    e.daemon.tier("opencode-go", "newcomer", None, true);
    assert!(
        e.run(&[]).status.success(),
        "disabled candidate is not in the set"
    );
    e.daemon
        .tier("opencode-go", "kimi", Some(Tier::Standard), true);
    let o = e.run(&["--verify"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("opencode-go/kimi"), "{}", err(&o));
    // 階層移動。
    e.daemon
        .tier("opencode-go", "kimi", Some(Tier::Cheap), false);
    let o = e.run(&["--verify"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("opencode-go/kimi"), "{}", err(&o));
    e.daemon
        .tier("opencode-go", "kimi", Some(Tier::Standard), false);
    assert!(e.run(&["--verify"]).status.success());
    assert_eq!(before, e.lock(), "lock must not be rewritten");
}

fn write_cap(e: &Env, name: &str, mounts: &[&std::path::Path], env: Value) -> PathBuf {
    let t = json!({"ro_mounts": mounts, "env": env});
    let p = e.p(name);
    fs::write(
        &p,
        json!({"schema":"celeris.campaign-cap-env/1","a":t,"worker":t}).to_string(),
    )
    .expect("cap env");
    p
}

#[test]
fn cap_env_is_pinned_order_is_normalized_and_changes_are_refused() {
    let e = Env::new();
    let (m1, m2) = (e.p("m1"), e.p("m2"));
    fs::create_dir_all(&m1).expect("m1");
    fs::create_dir_all(&m2).expect("m2");
    fs::write(m1.join("bin"), "x").expect("f");
    let base = write_cap(
        &e,
        "cap.json",
        &[&m1, &m2],
        json!([["PATH", "/opt/a"], ["LANG", "C"]]),
    );
    let s = base.to_string_lossy().into_owned();
    let o = e.run(&["--cap-env", &s]);
    assert!(o.status.success(), "{}", err(&o));
    let v: Value = serde_json::from_str(&e.lock()).expect("json");
    assert_eq!(v["cap_env"]["mounts"].as_array().map(Vec::len), Some(2));
    // 無変更・配列順のみの変更は通る（正規化）。
    assert!(e.run(&["--cap-env", &s, "--verify"]).status.success());
    let reordered = write_cap(
        &e,
        "cap2.json",
        &[&m2, &m1],
        json!([["LANG", "C"], ["PATH", "/opt/a"]]),
    );
    let r = reordered.to_string_lossy().into_owned();
    assert!(e.run(&["--cap-env", &r, "--verify"]).status.success());
    // PATH の意味のある変更・mount の追加・mount 内の実体変更・CAP_ENV の脱落は拒否（key 名つき）。
    let path_changed = write_cap(
        &e,
        "cap3.json",
        &[&m1, &m2],
        json!([["PATH", "/opt/b"], ["LANG", "C"]]),
    );
    let o = e.run(&["--cap-env", &path_changed.to_string_lossy(), "--verify"]);
    assert!(
        !o.status.success() && err(&o).contains("cap_env.toolchain.env"),
        "{}",
        err(&o)
    );
    let m3 = e.p("m3");
    fs::create_dir_all(&m3).expect("m3");
    let more = write_cap(
        &e,
        "cap4.json",
        &[&m1, &m2, &m3],
        json!([["PATH", "/opt/a"], ["LANG", "C"]]),
    );
    assert!(
        !e.run(&["--cap-env", &more.to_string_lossy(), "--verify"])
            .status
            .success()
    );
    fs::write(m1.join("lib"), "y").expect("f");
    let o = e.run(&["--cap-env", &s, "--verify"]);
    assert!(
        !o.status.success() && err(&o).contains("cap_env.mounts"),
        "{}",
        err(&o)
    );
    fs::remove_file(m1.join("lib")).expect("rm");
    assert!(!e.run(&["--verify"]).status.success());
    // 秘密らしい env は CAP_ENV に入れられない。
    let sec = write_cap(&e, "cap5.json", &[&m1], json!([["API_TOKEN", "zzz"]]));
    assert!(
        !e.run(&["--cap-env", &sec.to_string_lossy(), "--verify"])
            .status
            .success()
    );
    assert!(!e.lock().contains("zzz"));
}

#[test]
fn old_env_lock_without_cap_env_pin_is_refused() {
    let e = Env::new();
    assert!(e.run(&[]).status.success());
    let mut v: Value = serde_json::from_str(&e.lock()).expect("json");
    v["schema"] = json!("celeris-env-lock/1");
    v.as_object_mut().expect("obj").remove("cap_env");
    fs::write(
        e.out().join("env-lock.json"),
        serde_json::to_vec(&v).expect("json"),
    )
    .expect("w");
    let o = e.run(&[]);
    assert!(
        !o.status.success() && err(&o).contains("refusing to resume"),
        "{}",
        err(&o)
    );
}
