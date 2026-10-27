//! ADR eval-dispatch 契約 7: API 投入から hidden check までの試験 daemon 経路。

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use celeris::{Config, RunOptions};
use serde_json::json;
use task_core::{DaemonMode, SqliteStore, TaskStore};

const TOKEN: &str = "routing-eval-test-token";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Eval Test",
            "-c",
            "user.email=eval@example.invalid",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn daemon_runs_two_stub_trials_with_hidden_checks_and_eval_records() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init", "-q"]);
    git(&repo, &["commit", "--allow-empty", "-qm", "base"]);
    let base_commit = git(&repo, &["rev-parse", "HEAD"]);
    // The worker run cannot create entries directly in the DB directory (ADR-0095 db_guard), so
    // the stub's logs live in their own directory.
    let logs = root.join("stub-logs");
    std::fs::create_dir_all(&logs).expect("stub logs");
    let prompt_log = logs.join("prompts.log");
    let overlap_log = logs.join("overlap.log");
    let active_lock = logs.join("active.lock");
    let stub = root.join("stub-harness.sh");
    std::fs::write(&stub, format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'eval-stub 1.2.3'; exit 0; fi\nif ! mkdir '{}'; then echo overlap >> '{}'; else trap 'rmdir \"{}\"' EXIT; fi\ncat >> '{}'\nsleep 0.1\nprintf '{{\"type\":\"done\",\"summary\":\"stub done\",\"evidence\":[]}}\\n'\n",
        active_lock.display(), overlap_log.display(), active_lock.display(), prompt_log.display()
    )).expect("stub");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    std::os::unix::fs::symlink(&stub, bin.join("fake")).expect("fake on PATH");
    let path = std::env::var_os("PATH").unwrap_or_default();
    // This integration-test binary contains only this test, so the daemon's --version probe can
    // resolve the fixture executable by the same harness id used for adapter selection.
    let path = std::env::split_paths(&path);
    let path = std::env::join_paths(std::iter::once(bin).chain(path)).expect("PATH");
    unsafe {
        std::env::set_var("PATH", path);
    }
    std::fs::write(root.join("api.token"), format!("{TOKEN}\n")).expect("token");
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("port");
    let addr = probe.local_addr().expect("addr");
    drop(probe);
    let config_path = root.join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
db = "eval.sqlite3"
workspace_root = "ws"
tick_ms = 20
max_concurrency = 2
lease_grace_secs = 60
kill_grace_secs = 1

[api]
listen = "{addr}"

[[api.tokens]]
principal = "operator:test"
token_file = "api.token"

[routing_eval]
reserved_runs = 1

# Evaluation trials need a per-trial scratch target (ADR 2026-10-14-eval-trial-scratch-target D1).
[scratch]
dir = "scratch"

[adapters.fake]
command = ["{}"]

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["model-1"]

[harnesses.fake]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.fake]
candidates = [{{ source = "local", model = "model-1" }}]
"#,
            stub.display()
        ),
    )
    .expect("config");
    let config = Config::load(&config_path).expect("config load");
    let run = tokio::spawn(celeris::run(
        config,
        RunOptions {
            until_idle: false,
            max_ticks: 0,
            mode: DaemonMode::Normal,
            release: None,
        },
    ));
    let base_url = format!("http://{addr}/api/v1");
    let client = reqwest::Client::new();
    let api_ready = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if client
                .get(format!("{base_url}/routing-eval/trials"))
                .bearer_auth(TOKEN)
                .send()
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(api_ready.is_ok(), "daemon API became ready");

    let body = json!({"manifest_sha":"sha256:fixture", "trials": (0..2).map(|trial_no| json!({
        "problem_id":"problem-1", "trial_no":trial_no, "source_id":"local", "model_id":"model-1",
        "harness_id":"fake", "repo":repo, "base_commit":base_commit,
        "objective":"solve the fixture", "check":{"cmd":"printf hidden-check-marker", "timeout_secs":10}
    })).collect::<Vec<_>>()});
    let response = client
        .post(format!("{base_url}/routing-eval/trials"))
        .bearer_auth(TOKEN)
        .json(&body)
        .send()
        .await
        .expect("POST");
    assert_eq!(
        response.status().as_u16(),
        201,
        "{}",
        response.text().await.unwrap_or_default()
    );

    let db = root.join("eval.sqlite3");
    let terminal = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(store) = SqliteStore::open(&db) {
                let rows = store.eval_trials_list(None, None).expect("trials");
                if rows.len() == 2 && rows.iter().all(|t| t.status.is_terminal()) {
                    break rows;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "trials did not complete: {:?}",
            SqliteStore::open(&db).and_then(|s| s.eval_trials_list(None, None))
        )
    });
    assert!(
        terminal
            .iter()
            .all(|t| t.status == task_core::routing_eval_trial::EvalTrialStatus::Passed),
        "{terminal:?}"
    );
    assert!(terminal.iter().all(|t| {
        t.harness_version
            .as_deref()
            .is_some_and(|v| v.contains("1.2.3"))
    }));
    assert_ne!(terminal[0].worktree_path, terminal[1].worktree_path);
    // Each trial had its own scratch target, gone after scoring (the released lease stays).
    // The daemon writes the terminal status before it removes the files, so wait for the
    // cleanup instead of reading it right after the status turns terminal.
    for t in &terminal {
        let owner = root
            .join("scratch/targets")
            .join(format!("agent-eval-{}", t.trial_id));
        let cleaned = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let released = std::fs::read_to_string(owner.join("lease.json"))
                    .is_ok_and(|lease| lease.contains("released_at"));
                if released && !owner.join("target").exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await;
        assert!(
            cleaned.is_ok(),
            "trial target not cleaned up: {} (lease: {:?})",
            owner.display(),
            std::fs::read_to_string(owner.join("lease.json"))
        );
    }
    let prompts = std::fs::read_to_string(&prompt_log).expect("prompt log");
    assert!(
        !prompts.contains("hidden-check-marker"),
        "worker received acceptance command"
    );
    assert!(
        !overlap_log.exists(),
        "reserved_runs=1 started overlapping trials"
    );

    let store = SqliteStore::open(&db).expect("store");
    let decisions = store
        .route_decisions_export(None, None, Some("eval"))
        .expect("decisions");
    assert_eq!(decisions.len(), 2);
    assert!(decisions.iter().all(|d| {
        d.decision.output["selected"]["harness_version"]
            .as_str()
            .is_some()
    }));
    assert!(
        decisions.iter().all(|d| d.outcome.is_some()),
        "each eval decision has a route outcome"
    );
    let runs: Vec<_> = decisions
        .iter()
        .map(|d| {
            store
                .run_index_get(d.decision.run_id.as_deref().expect("run id"))
                .expect("run query")
                .expect("run row")
        })
        .collect();
    assert!(runs.iter().all(|r| r.role == task_core::RunIndexRole::Eval));
    assert_ne!(runs[0].session_id, runs[1].session_id);

    run.abort();
    let _ = run.await;
}
