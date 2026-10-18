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
}

impl Env {
    fn new() -> Self {
        let e = Env {
            dir: tempfile::tempdir().expect("tmp"),
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
            .args(extra)
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
    assert_eq!(v["schema"], "celeris-env-lock/1");
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
