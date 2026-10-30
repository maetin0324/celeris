#[path = "support_cases/mod.rs"]
#[allow(dead_code)]
mod support_cases;
use std::process::Command;

use serde_json::{Value, json};

fn write_fixture(dir: &std::path::Path, campaign_split: &str) -> std::path::PathBuf {
    let problems: Vec<Value> = (0..3)
        .map(|i| json!({"id":format!("p{i}"),"task_id":"t","work_unit_id":"w","created_at":"","genre":"coding","kind":"leaf","command":"true","prompt":"fix the bug","split":"dev","stratum":"s","subset_12":false,"head_commit":null,"base_commit":null,"features_sha256":"","features":{},"sha256":""}))
        .collect();
    let mut input: routing_eval::manifest::Manifest = serde_json::from_value(json!({"schema":routing_eval::MANIFEST_SCHEMA,"version":"v","cutoff":"c","seed":1,"problems":problems,"sha256":""})).expect("input");
    input.sha256 = celeris_campaign::manifest::manifest_hash(&input).expect("hash");
    std::fs::write(
        dir.join("input.json"),
        serde_json::to_vec(&input).expect("ser"),
    )
    .expect("write");
    let cfg = |id: &str| json!({"id":id,"harness":"h","harness_version":"1","model":"m","model_version":"1","policy_version":"p"});
    // final では B/C の routing 固定（C は根拠表の版も）が要る。
    let routed = |mut c: Value| {
        c["routing"] = json!({"lower_policy":"lp","evidence_table_id":"ev","online_update":false});
        c
    };
    let m = json!({"schema":"celeris.campaign-manifest/1","campaign_id":"c","version":"v1","split":campaign_split,
        "input":{"path":"input.json","sha256":input.sha256},"problems":["p0","p1","p2"],
        "configs":[cfg("A"),routed(cfg("B")),routed(cfg("C"))],"primary":["A","B"],
        "secondary":{"pair":["B","C"],"subset":["p0"]},
        "budget":{"token_basis":"input_output","max_tokens":100000000u64,"max_wall_secs":86400,"max_disk_bytes":1u64<<40,"max_parallel":2,"per_case":{"max_tokens":100000000u64,"max_wall_secs":86400},"per_arm":{"max_tokens":100000000u64,"max_wall_secs":86400}},
        "termination":{"trial_timeout_secs":600,"max_attempts_per_trial":1,"stop_on_budget":true},
        "snapshot":{"repo_commit":"abc","materials_sha256":"d","kb_snapshot":"kb@1"}});
    let p = dir.join("campaign.json");
    std::fs::write(&p, m.to_string()).expect("write");
    p
}

#[test]
fn dry_run_prints_json_estimate_without_llm() {
    let dir = tempfile::tempdir().expect("dir");
    let p = write_fixture(dir.path(), "dev");
    let out = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
        .args(["dry-run", "--manifest"])
        .arg(&p)
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["llm_calls_made"], 0);
    assert_eq!(v["resources"]["total_trials"], 7);
    assert_eq!(v["tokens"]["label"], "estimated");
}

#[test]
fn dry_run_rejects_split_mix() {
    let dir = tempfile::tempdir().expect("dir");
    let p = write_fixture(dir.path(), "final");
    let out = Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
        .args(["dry-run", "--manifest"])
        .arg(&p)
        .output()
        .expect("run");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("must not be mixed"));
}

/// 単独の `cap-probe --arm b|c` も、投入の前に測定 config の検査宣言を確かめる。宣言漏れでは env_failure で、
/// 接続先 daemon に一切触れない（api-url は届かない port。届けば別の error になる）。
#[test]
fn cap_probe_cli_refuses_undeclared_probe_harness_before_any_submission() {
    let dir = tempfile::tempdir().expect("dir");
    let p = write_fixture(dir.path(), "dev");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::write(
        dir.path().join("cap.json"),
        json!({"schema":"celeris.campaign-cap-env/1","a":{},"worker":{}}).to_string(),
    )
    .expect("cap");
    // 宣言の無い測定 config（genre なしの規則が実 harness に当たり得る）。
    std::fs::write(
        dir.path().join("measure.toml"),
        "[harness_selection]\nrules = [{ harness = \"h\", required_capabilities = [] }]\n[harnesses.h]\nkind = \"claude_code\"\n",
    )
    .expect("cfg");
    for arm in ["b", "c"] {
        let out =
            Command::new(std::env::var("CARGO_BIN_EXE_celeris-campaign").expect("binary path"))
                .args(["cap-probe", "--case", "k0-fixture-control", "--arm", arm])
                .arg("--cap-env")
                .arg(dir.path().join("cap.json"))
                .arg("--cases-root")
                .arg(support_cases::cases_root())
                .arg("--grader-root")
                .arg(root.join("fixtures/grader-private"))
                .arg("--manifest")
                .arg(&p)
                .arg("--measure-config")
                .arg(dir.path().join("measure.toml"))
                .args(["--daemon-pid", "1", "--api-url", "http://127.0.0.1:1"])
                .args(["--token-env", "NO_SUCH_TOKEN_ENV"])
                .output()
                .expect("run");
        assert!(!out.status.success(), "{arm}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("env_failure") && err.contains("B/C probe not submitted"),
            "{arm}: {err}"
        );
    }
}
