use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

use routing_eval::{aggregate, dryrun, manifest, run};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use task_core::store::SqliteStore;
use tempfile::tempdir;

#[test]
fn stub_pipeline_freezes_estimates_runs_aggregates_and_exports_features() {
    let dir = tempdir().expect("tempdir");
    let db = dir.path().join("fixture.sqlite3");
    let store = SqliteStore::open(&db).expect("migrate fixture db");
    drop(store);
    let conn = Connection::open(&db).expect("db connection");
    for i in 0..30 {
        let task = format!("task-{i}");
        let wu = format!("wu-{i}");
        let created = if i < 15 {
            format!("2026-09-{:02}T00:00:00Z", i + 1)
        } else {
            format!("2026-10-{:02}T00:00:00Z", i - 10)
        };
        let task_json = json!({"title":task,"objective":"fixture","genre":"coding"}).to_string();
        conn.execute("INSERT INTO tasks(id,status,kind,parent_id,priority,created_at,json) VALUES(?1,'done','task',NULL,0,?2,?3)", params![task,created,task_json]).expect("task");
        let wu_json = json!({"objective":"fixture","kind":if i%2==0{"implement"}else{"test"},"checks":[{"cmd":"true","expect_exit":0}]}).to_string();
        conn.execute("INSERT INTO work_units(id,task_id,plan_id,key,seq,kind,status,depends_on_json,json,created_at,updated_at,head_commit,base_commit) VALUES(?1,?2,'plan',?1,0,'work','done','[]',?3,?4,?4,'head','base')", params![wu,task,wu_json,created]).expect("work unit");
    }
    drop(conn);

    let manifest =
        manifest::create(&db, dir.path(), "2026-10-05", 7, "v1", None).expect("manifest");
    assert_eq!(manifest.problems.len(), 30);
    assert_eq!(manifest.problems.iter().filter(|p| p.subset_12).count(), 12);
    let estimates = dryrun::estimate(&manifest, &db, None).expect("dry-run");
    assert_eq!(
        estimates.accounts.iter().map(|a| a.calls).sum::<usize>(),
        324
    );

    let listener = TcpListener::bind("127.0.0.1:0").expect("stub listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request");
        let mut request = [0u8; 65_536];
        let size = stream.read(&mut request).expect("body");
        let text = String::from_utf8_lossy(&request[..size]);
        let body = text.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("");
        let request: Value = serde_json::from_str(body).expect("trial request json");
        let trials: Vec<Value> = request["trials"].as_array().expect("trials").iter().enumerate().map(|(i,t)| json!({"trial_id":format!("trial-{i}"),"problem_id":t["problem_id"],"trial_no":t["trial_no"],"status":"queued"})).collect();
        let response = json!({"batch_id":"batch-1","trials":trials}).to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).expect("response");
    });
    let manifest_path = dir.path().join("manifest.json");
    let manifest_json = serde_json::to_vec(&manifest).expect("serialize");
    std::fs::write(&manifest_path, manifest_json).expect("manifest file");
    run::run_args(vec![
        "--api".into(),
        format!("http://{address}"),
        "--manifest".into(),
        manifest_path.display().to_string(),
        "--candidates".into(),
        "claude-opus-5-5".into(),
        "--repo".into(),
        ".".into(),
        "--out".into(),
        dir.path().join("run.json").display().to_string(),
    ])
    .expect("stub run");
    server.join().expect("stub server");
    let summary: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("run.json")).expect("summary"))
            .expect("summary json");
    assert_eq!(summary["trial_count"], 60);

    let problem = &manifest.problems[0];
    let conn = Connection::open(&db).expect("db");
    conn.execute("INSERT INTO runs(run_id,task_id,role,seq,status,started_at,finished_at,model,usage_json,metrics_json) VALUES('r1',?1,'eval',0,'passed','2026-10-12T00:00:00Z','2026-10-12T00:00:03Z','claude-opus-5-5','{\"input_tokens\":100,\"output_tokens\":20,\"usd_list\":0.01}','{\"wall_secs\":3}')", [&problem.task_id]).expect("run row");
    conn.execute("INSERT INTO route_features(hash,schema,template,subject_kind,subject_id,brief,aux_json,created_at) VALUES('f','schema','template','eval_problem',?1,'{}','{}','now')", [&problem.id]).expect("feature row");
    conn.execute("INSERT INTO route_options(hash,schema,builder,question,options_json,created_at) VALUES('o','schema','builder','question','[]','now')", []).expect("options row");
    conn.execute("INSERT INTO route_decisions(decision_id,subject_kind,subject_id,run_id,features_hash,options_hash,input_json,output_json,judge_json,stage,created_at) VALUES('d','eval_problem',?1,'r1','f','o',?2,?3,'{}','eval','now')", params![problem.id,json!({"trial_no":1}).to_string(),json!({"selected":{"source_id":"claude_oauth","model_id":"claude-opus-5-5","harness_id":"claude_code","harness_version":"2.1.0"}}).to_string()]).expect("decision row");
    conn.execute("INSERT INTO route_outcomes(decision_id,schema,labels_json,recorded_at) VALUES('d','schema',?1,'now')", [json!({"passed":true,"wall_secs":3.0,"usd_list":0.01}).to_string()]).expect("outcome row");
    drop(conn);

    let aggregate_out = dir.path().join("aggregate");
    aggregate::run_args(vec![
        "--db".into(),
        db.display().to_string(),
        "--manifest".into(),
        manifest_path.display().to_string(),
        "--out".into(),
        aggregate_out.display().to_string(),
    ])
    .expect("aggregate");
    // 評価 trial の集計はレポートだけで、根拠表（本番の完了単位から作る。ADR 2026-10-22 D5）は書かない。
    assert!(aggregate_out.join("evidence.md").is_file());
    let (store, _) = SqliteStore::open_client(&db).expect("store");
    assert!(
        task_core::TaskStore::latest_routing_evidence(&store, "coding", "claude_code")
            .expect("read evidence")
            .is_empty()
    );
    let out = dir.path().join("results.jsonl");
    routing_eval::export::run_args(vec![
        "--db".into(),
        db.display().to_string(),
        "--manifest".into(),
        manifest_path.display().to_string(),
        "--out".into(),
        out.display().to_string(),
    ])
    .expect("export");
    let exported: Value =
        serde_json::from_str(std::fs::read_to_string(out).expect("export file").trim())
            .expect("jsonl row");
    assert_eq!(exported["schema"], "celeris.routing-eval-result/1");
    assert_eq!(
        exported["problem"]["features"]["schema"],
        "celeris.task-features/1"
    );
}
