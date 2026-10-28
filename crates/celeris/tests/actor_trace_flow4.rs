//! Flow 4: real selfdeploy receipts join an external executor to a human decision.
#[path = "actor_trace/support.rs"]
mod support;
use celeris_campaign::{human_ops::*, manifest::ConfigId};
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
    fs::write(clock.join("sitecustomize.py"), "import datetime, sys\nclass Fixed(datetime.datetime):\n @classmethod\n def now(cls, tz=None):\n  return cls(2026,10,28,10,0,12 if sys.argv[1:2] == ['finish'] else 10,tzinfo=datetime.timezone.utc)\ndatetime.datetime=Fixed\n").unwrap();
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
    let (_, revision) = h.raise_and_claim();
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
    let observed = h.dir.path().join("deployment-human-ops.jsonl");
    let observations = HumanOpLog::new(&observed);
    let person = Actor {
        kind: ActorKind::Human,
        id: request["resolved"]["actor_id"].as_str().unwrap().into(),
    };
    // Observe the actual answer transaction above: active work 4..5, not request wait 1..5.
    let mut answer = human_record_base(ExperimentSet::MainAb, ConfigId::B, RecordType::OpStart);
    answer.task_id = Some(h.task.id.to_string());
    answer.logical_op_id = Some(format!("human-answer:{DECISION}"));
    answer.attempt = Some(1);
    answer.start = Some(ts(4));
    answer.op_kind = Some(OpKind::SafetyApproval);
    answer.mode = Some(Mode::Manual);
    answer.intent = Some(Intent::Safety);
    answer.time_basis = Some(TimeBasis::HarnessTranscript);
    answer.time_precision = Some(TimePrecision::Second);
    answer.credential = Some("web".into());
    answer.actor = Some(person.clone());
    answer.decided_by = Some(person.clone());
    answer.executed_by = Some(person.clone());
    answer.request_ref = Some(request["request_ref"].as_str().unwrap().into());
    answer.request_revision = Some(revision);
    answer.policy_ref = Some(format!("decision:{DECISION}"));
    answer.reason = Some(WHY.into());
    answer.trigger = Some(Trigger::SystemRequest);
    observations
        .append(&answer.clone().with_record_id())
        .unwrap();
    answer.record_type = RecordType::OpEnd;
    answer.start = None;
    answer.end = Some(ivs[0]["at"].as_str().unwrap().into());
    answer.outcome = Some(Outcome::Ok);
    observations
        .append(&answer.clone().with_record_id())
        .unwrap();
    let mut receipts = Vec::new();
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
        assert_eq!(audit["finished_at"], "2026-10-28T10:00:12+00:00");
        // Preserve the receipt's actual timestamps and references; do not invent a DB operation.
        let mut record = answer.clone();
        record.logical_op_id = Some(format!("deployment:{case}:{target}"));
        record.actor = Some(Actor {
            kind: ActorKind::ExternalOperator,
            id: audit["actor_id"].as_str().unwrap().into(),
        });
        record.executed_by = record.actor.clone();
        record.credential = Some("unknown".into());
        record.op_kind = Some(if operation == "rollback" {
            OpKind::SystemRecovery
        } else {
            OpKind::SystemAdjustment
        });
        record.policy_ref = Some(audit["policy_ref"].as_str().unwrap().into());
        record.reason = Some(audit["reason"].as_str().unwrap().into());
        record.targets = vec![
            Target {
                kind: TargetKind::Artifact,
                target_ref: format!("release:{source}"),
                sha256: None,
            },
            Target {
                kind: TargetKind::Artifact,
                target_ref: format!("release:{target}"),
                sha256: None,
            },
            Target {
                kind: TargetKind::Config,
                target_ref: format!("decision:{}", audit["decision_refs"][0].as_str().unwrap()),
                sha256: None,
            },
            Target {
                kind: TargetKind::Task,
                target_ref: audit["task_refs"][0].as_str().unwrap().into(),
                sha256: None,
            },
            Target {
                kind: TargetKind::Artifact,
                target_ref: format!("{}#deployment_audit", root.join(target).display()),
                sha256: None,
            },
        ];
        record.record_type = RecordType::OpStart;
        record.start = Some(audit["started_at"].as_str().unwrap().replace("+00:00", "Z"));
        record.end = None;
        record.outcome = None;
        observations
            .append(&record.clone().with_record_id())
            .unwrap();
        record.record_type = RecordType::OpEnd;
        record.start = None;
        record.end = Some(
            audit["finished_at"]
                .as_str()
                .unwrap()
                .replace("+00:00", "Z"),
        );
        record.outcome = Some(if success {
            Outcome::Ok
        } else {
            Outcome::Failed
        });
        observations
            .append(&record.clone().with_record_id())
            .unwrap();
        receipts.push((record, audit.clone()));
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
    let out = h.outputs(&observed);
    assert_eq!(out.export.records_jsonl(), export.records_jsonl());
    assert_eq!(out.requests, requests);
    let summary = serde_json::to_value(&out.export.summary).unwrap();
    assert_eq!(summary["interventions_by_actor_kind"]["human"], 1);
    assert_eq!(
        summary["interventions_by_actor_kind"]["external_operator"],
        0
    );
    assert!(
        summary["incomplete"]
            .as_array()
            .unwrap()
            .contains(&json!("usage"))
    );
    assert!(summary["quota"].is_null());
    let trials = out.report["trials"].as_array().unwrap();
    assert_eq!(trials.len(), 1);
    let trial = &trials[0];
    assert_eq!(trial["interventions"]["value"]["safety_approval"], 1);
    // Actor counts are logical requests, not the number of receipt attempts.
    assert_eq!(
        trial["interventions_by_actor_kind"]["value"],
        json!({"human":1,"external_operator":1})
    );
    assert_eq!(trial["excluded_ops"]["non_human"]["external_operator"], 3);
    assert_eq!(trial["human_time"]["value"]["union_secs"], 1);
    assert_eq!(trial["human_time"]["value"]["sum_secs"], 1);
    assert_eq!(trial["human_time"]["value"]["observed"], 1);
    assert_eq!(trial["human_time"]["value"]["lower_bound"], false);
    assert!(trial["human_wait"]["value"].is_null());
    for field in ["tokens", "cost_micro_usd", "wall_secs"] {
        assert!(trial[field]["value"].is_null(), "{field}: {trial}");
    }
    let items = trial["intervention_items"]["value"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item["request_ref"], request["request_ref"]);
    assert_eq!(item["human"], true);
    let history = item["history"].as_array().unwrap();
    assert_eq!(history.len(), 4);
    assert_eq!(
        history[0]["executed_by"],
        serde_json::to_value(&person).unwrap()
    );
    assert_eq!(history[0]["duration_secs"], 1);
    assert_eq!(history[0]["counts_as_human"], true);
    let raw = read_records(&observed).unwrap();
    assert_eq!(raw.len(), 8);
    for (record, audit) in receipts {
        let entry = history
            .iter()
            .find(|entry| entry["logical_op_id"] == record.logical_op_id.as_deref().unwrap())
            .unwrap();
        assert_eq!(entry["decided_by"], history[0]["executed_by"]);
        assert_eq!(entry["executed_by"]["kind"], audit["actor_kind"]);
        assert_eq!(entry["executed_by"]["id"], audit["actor_id"]);
        assert_eq!(entry["counts_as_human"], false);
        assert_eq!(entry["mode"], "manual");
        assert_eq!(entry["policy_ref"], audit["policy_ref"]);
        assert_eq!(entry["duration_secs"], 2); // Fixed receipt interval is excluded from human time.
        let end = raw
            .iter()
            .find(|r| r.logical_op_id == record.logical_op_id && r.record_type == RecordType::OpEnd)
            .unwrap();
        assert_eq!(end.request_ref.as_deref(), item["request_ref"].as_str());
        assert_eq!(end.task_id.as_deref(), ivs[0]["task_id"].as_str());
        assert_eq!(
            end.targets[0].target_ref,
            format!("release:{}", audit["from"].as_str().unwrap())
        );
        assert_eq!(
            end.targets[1].target_ref,
            format!("release:{}", audit["to"].as_str().unwrap())
        );
        assert_eq!(
            end.targets[2].target_ref,
            format!("decision:{}", audit["decision_refs"][0].as_str().unwrap())
        );
        assert_eq!(end.targets[3].target_ref, end.task_id.as_deref().unwrap());
        assert!(end.targets[4].target_ref.ends_with("#deployment_audit"));
        assert_eq!(
            end.outcome,
            Some(if audit["status"] == "succeeded" {
                Outcome::Ok
            } else {
                Outcome::Failed
            })
        );
        assert!(end.run_id.is_none()); // Receipt has no run attribution; never infer one.
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
