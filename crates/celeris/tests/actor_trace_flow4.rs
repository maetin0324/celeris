//! Flow 4: real selfdeploy receipts join an external executor to a human decision.
#[path = "actor_trace/support.rs"]
mod support;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};
use support::*;

const OLD: &str = "aaaaaaaaaaaa";
const NEW: &str = "bbbbbbbbbbbb";
const ACTOR: &str = "external_operator:flow4";
const WHY: &str = "人の判断に従って配備し、不具合時は戻す";

fn scripts(h: &Harness) -> std::path::PathBuf {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Reuse the existing isolated fake systemctl/curl/sqlite3/ss, without running its cases.
    let fixture =
        fs::read_to_string(repo.join("scripts/selfdeploy/tests/promote_authorization_marker.sh"))
            .unwrap();
    let prefix = fixture.split("# Missing provenance").next().unwrap();
    let prefix = prefix
        .replace(
            "HERE=\"$(cd \"$(dirname \"$0\")\" && pwd)\"",
            "HERE=\"$SOURCE_FIXTURE\"",
        )
        .replace("WORK=\"$(mktemp -d)\"", "WORK=\"$FLOW_WORK\"")
        .replace("trap 'rm -rf \"$WORK\"' EXIT", "");
    let work = h.dir.path().join("deployment");
    fs::create_dir_all(&work).unwrap();
    let clock = work.join("clock");
    fs::create_dir_all(&clock).unwrap();
    fs::write(clock.join("sitecustomize.py"), "import datetime\nclass Fixed(datetime.datetime):\n @classmethod\n def now(cls, tz=None):\n  return cls(2026,10,28,10,0,10,tzinfo=datetime.timezone.utc)\ndatetime.datetime=Fixed\n").unwrap();
    let script = work.join("flow.sh");
    fs::write(
        &script,
        format!(
            r#"{prefix}
export SD_CREDENTIALD_SETTLE=0
export SD_ACTOR='{ACTOR}' SD_REASON='{WHY}' SD_POLICY_REF='decision:{DECISION}'
export SD_TASK_REFS="$FLOW_TASK_REFS" SD_DECISION_REFS="$FLOW_DECISION_REFS"
# No polling or wall-clock waiting may be reached in these cases.
printf '#!/bin/sh\necho unexpected-sleep >&2\nexit 97\n' >"$FAKE_BIN/sleep"
printf '#!/bin/sh\nexec /usr/bin/date -u -d 2026-10-28T10:00:10Z "$@"\n' >"$FAKE_BIN/date"
chmod +x "$FAKE_BIN/sleep" "$FAKE_BIN/date"
setup promote false
run_script promote.sh "$NEW"
printf '%s' "$RC" >"$CASE/exit"
# Roll back the release just promoted, using the previous link written by promote.sh.
cp "$CASE/out.log" "$CASE/promote.log"
mkdir -p "$WORK/rollback"
run_script rollback.sh
printf '%s' "$RC" >"$WORK/rollback/exit"
cp "$CASE/out.log" "$WORK/rollback/out.log"
mv "$CASE/promote.log" "$CASE/out.log"
setup failed true
touch "$FAKE_STATE/started-celeris@$OLD" "$FAKE_STATE/fail-start-celeris@$NEW"
run_script promote.sh "$NEW"
printf '%s' "$RC" >"$CASE/exit"
"#
        ),
    )
    .unwrap();
    let output = Command::new("sh")
        .arg(script)
        .env("SOURCE_FIXTURE", repo.join("scripts/selfdeploy/tests"))
        .env("FLOW_WORK", &work)
        .env("FLOW_TASK_REFS", json!([h.task.id.to_string()]).to_string())
        .env("FLOW_DECISION_REFS", json!([DECISION]).to_string())
        .env("PYTHONPATH", clock)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    work
}

async fn get_releases(h: &Harness, root: &Path) -> Value {
    let config_path = h.dir.path().join("api.toml");
    fs::write(
        &config_path,
        format!(
            r#"
db = {:?}
workspace_root = {:?}
[api]
listen = "127.0.0.1:0"
[selfdeploy]
releases_dir = {:?}
repo = {:?}
detach = "inline"
"#,
            h.db,
            h.dir.path(),
            root,
            h.dir.path().join("no-repo")
        ),
    )
    .unwrap();
    let config = celeris::Config::load(&config_path).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let settings = celeris::api_settings(
        &config,
        addr,
        vec![task_api::ApiToken::new(
            task_api::Principal::Human {
                name: "test".into(),
            },
            "flow4-token",
        )],
        "01J00000000000000000000000".into(),
        ts(0),
        None,
        "dev".into(),
        task_core::DaemonMode::Normal,
        task_core::SharedRole::new(task_core::InstanceRole::Active),
        None,
    );
    let (_tx, rx) = tokio::sync::watch::channel(None);
    let state = task_api::ApiState::new(settings, rx).unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(task_api::serve_with_listener(listener, state, async move {
        let _ = stopped.await;
    }));
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{addr}/api/v1/releases"))
        .bearer_auth("flow4-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.json().await.unwrap();
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    body
}

#[tokio::test]
async fn operator_deployment_and_rollback_join_release_decision_and_request() {
    let h = Harness::new();
    h.raise_and_claim();
    let plan = task_ops::decision::plan_answer(
        &h.store,
        DECISION,
        Some("local"),
        Some(WHY),
        "human:web",
        at(5),
    )
    .unwrap();
    let mut conn = rusqlite::Connection::open(&h.db).unwrap();
    let tx = conn.transaction().unwrap();
    assert!(
        task_core::SqliteStore::decision_resolve_apply_tx_at(
            &tx,
            plan.node_id,
            &plan.decision_id,
            plan.expect,
            plan.rows(),
            plan.events(),
            at(5)
        )
        .unwrap()
    );
    tx.commit().unwrap();
    task_ops::decision::finish_answer(&h.store, plan, at(5)).unwrap();
    h.store
        .decision_log_project_interventions(&h.task.id.to_string())
        .unwrap();
    let export = h
        .store
        .decision_log_export_campaign(&task_core::decision_log_export::ExportParams {
            campaign_id: CAMPAIGN.into(),
            manifest_hash: Some(HASH.into()),
            export_cursor: None,
        })
        .unwrap();
    let requests = serde_json::to_value(h.store.request_ledger(at(0), at(60)).unwrap()).unwrap();
    let request = requests["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["request_ref"] == format!("decision:decision-{DECISION}"))
        .unwrap();
    assert_eq!(request["resolved"]["actor_kind"], "human");
    assert_eq!(request["resolved"]["actor_id"], "web");
    assert!(
        request["tasks"]
            .as_array()
            .unwrap()
            .contains(&json!(h.task.id.to_string()))
    );
    let ivs: Vec<_> = export
        .records
        .iter()
        .filter(|r| r["record_type"] == "intervention")
        .collect();
    assert_eq!(ivs.len(), 1);
    assert_eq!(ivs[0]["actor_kind"], "human");
    assert_eq!(ivs[0]["actor_id"], "web");
    assert_eq!(ivs[0]["task_id"], h.task.id.to_string());
    let work = scripts(&h);
    for (case, target, source, operation, success) in [
        ("promote", NEW, OLD, "promote", true),
        ("rollback", OLD, NEW, "rollback", true),
        ("failed", NEW, OLD, "promote", false),
    ] {
        let case_root = work.join(case);
        let code: i64 = fs::read_to_string(case_root.join("exit"))
            .unwrap()
            .parse()
            .unwrap();
        let log = fs::read_to_string(case_root.join("out.log")).unwrap();
        assert!(!log.contains("unexpected-sleep"), "{log}");
        assert_eq!(code == 0, success, "{case}: {log}");
        let state_case = if case == "rollback" {
            work.join("promote")
        } else {
            case_root.clone()
        };
        let root = state_case.join("home/.local/celeris/releases");
        let body = get_releases(&h, &root).await;
        assert_eq!(body["current"], OLD);
        if success {
            assert_eq!(body["previous"], NEW);
        }
        let item = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["sha12"] == target)
            .unwrap();
        let audits = item["deployment_audit"].as_array().unwrap();
        assert_eq!(audits.len(), 1, "{body}");
        let audit = &audits[0];
        assert_eq!(audit["actor"], ACTOR);
        assert_eq!(audit["actor_kind"], "external_operator");
        assert_eq!(audit["actor_id"], "flow4");
        assert_eq!(audit["reason"], WHY);
        assert_eq!(audit["policy_ref"], format!("decision:{DECISION}"));
        assert_eq!(audit["decision_refs"], json!([DECISION]));
        assert_eq!(audit["task_refs"], json!([ivs[0]["task_id"]]));
        assert_eq!(audit["from"], source);
        assert_eq!(audit["to"], target);
        assert_eq!(audit["operation"], operation);
        assert_eq!(
            audit["status"],
            if success { "succeeded" } else { "failed" }
        );
        assert_eq!(audit["exit_code"], code);
        assert_eq!(audit["verify"]["ok"], true);
        assert_eq!(audit["started_at"], "2026-10-28T10:00:10+00:00");
        assert_eq!(audit["finished_at"], audit["started_at"]);
        let release = root.join(target);
        let summary = release.join(if operation == "rollback" {
            "rolled_back.json"
        } else {
            "promoted.json"
        });
        if success {
            let summary: Value = serde_json::from_slice(&fs::read(summary).unwrap()).unwrap();
            for key in [
                "actor",
                "task_refs",
                "decision_refs",
                "status",
                "exit_code",
                "operation",
            ] {
                assert_eq!(summary[key], audit[key]);
            }
        } else {
            assert!(!summary.exists());
            assert!(release.join("promote_failed.json").exists());
        }
    }
    // Release receipts do not invent DB interventions or LLM consumption for the operator.
    let again = h
        .store
        .decision_log_export_campaign(&task_core::decision_log_export::ExportParams {
            campaign_id: CAMPAIGN.into(),
            manifest_hash: Some(HASH.into()),
            export_cursor: None,
        })
        .unwrap();
    assert_eq!(export.records_jsonl(), again.records_jsonl());
    assert!(!again.records.iter().any(|r| r["record_type"] == "usage"));
}
