//! ADR 2026-11-04-project-controller 付記 A5: 案件の Roadmap の API と CoS op（接頭辞 `project_roadmap_`）。
//! LLM を呼ばない（CoS は fake の run の credential）。一時 DB だけを使う。
//! `PROJECT_ROADMAP_RAW_DIR` があれば版の遷移と主体を `celeris.long-horizon-record/1` の JSONL
//! （`roadmap.jsonl`）に書く。
mod common;

use common::cos_ops::{OPS, cos_run_bearer, op_body};
use common::*;
use serde_json::{Value, json};
use task_api::{ApiToken, Principal};
use task_core::{ProjectId, TaskId, TaskStore, Trigger};
use time::OffsetDateTime;

const OP_TOKEN: &str = "operator-token-a5";

struct Fx {
    env: TestEnv,
    app: axum::Router,
    thread: String,
    project: ProjectId,
}

/// 案件 thread つきの案件。`request` を書き換えたいときは store で直す。
fn fx() -> Fx {
    let env = TestEnv::with(EnvOptions {
        token: Some(TOKEN.to_string()),
        extra_tokens: vec![ApiToken::new(
            Principal::Operator {
                name: "claude-code".into(),
            },
            OP_TOKEN,
        )],
        ..EnvOptions::default()
    });
    let thread = project_cos_thread(&env.store, "pilot:roadmap");
    let project: ProjectId = thread
        .project_id
        .as_deref()
        .expect("project thread")
        .parse()
        .expect("id");
    let app = env.router();
    Fx {
        env,
        app,
        thread: thread.id,
        project,
    }
}

fn outcome(id: &str, deps: &[&str]) -> Value {
    json!({"id": id, "title": format!("{id} の状態"), "acceptance": [format!("{id} を確かめる")],
           "priority": 1, "depends_on": deps})
}

fn proposal(base: Option<i64>, outcomes: Vec<Value>) -> Value {
    json!({"base_version": base, "change_reason": "人の方針の確認を求める", "outcomes": outcomes})
}

fn raw(test: &str, row: Value) {
    let Ok(dir) = std::env::var("PROJECT_ROADMAP_RAW_DIR") else {
        return;
    };
    let mut row = row;
    row["schema"] = json!("celeris.long-horizon-record/1");
    row["test"] = json!(test);
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::path::Path::new(&dir).join("roadmap.jsonl"))
        .and_then(|mut f| f.write_all(format!("{row}\n").as_bytes()))
        .expect("raw");
}

impl Fx {
    fn path(&self, rest: &str) -> String {
        format!("/api/v1/projects/{}/roadmap{rest}", self.project)
    }

    async fn propose(&self, body: &Value) -> Resp {
        send(&self.app, post_admin(&self.path("/proposals"), body)).await
    }

    async fn decide(&self, version: i64, decision: &str, headers: &[(&str, &str)]) -> Resp {
        send(
            &self.app,
            post_json_with(
                &self.path(&format!("/proposals/{version}/decide")),
                &json!({"decision": decision, "note": "確認した"}),
                headers,
            ),
        )
        .await
    }

    async fn current(&self) -> Resp {
        send(&self.app, get_admin(&self.path(""))).await
    }

    async fn history(&self) -> Value {
        let r = send(&self.app, get_admin(&self.path("/history"))).await;
        assert_eq!(r.status.as_u16(), 200, "{}", r.text());
        r.json()
    }

    fn snapshot(&self) -> task_core::project_proposal::ProjectSnapshot {
        use task_core::project_proposal as pp;
        task_ops::project_snapshot::build(
            &self.env.store,
            self.project,
            task_ops::project_snapshot::SnapshotContext {
                controller: pp::SnapshotController {
                    mode: "off".into(),
                    phase: "active".into(),
                    revision: 0,
                    human_epoch: 0,
                    retry_count: 0,
                    progress_epoch: 0,
                },
                decisions: vec![],
                wakes: vec![],
                limits: pp::SnapshotLimits::default(),
                now: OffsetDateTime::now_utc(),
            },
        )
        .expect("snapshot")
    }

    /// 案件の root を 1 つ作る（`acceptance` の文つき）。
    fn root(&self, title: &str, acceptance: &str) -> TaskId {
        let spec: task_ops::add::NewTaskSpec = serde_json::from_value(json!({
            "title": title, "objective": "o", "project_id": self.project,
            "acceptance": [{"type": "reviewer", "text": acceptance}],
        }))
        .expect("spec");
        task_ops::add::create_task(&self.env.store, spec, OffsetDateTime::now_utc())
            .expect("create")
            .id
    }

    /// root を done・合格にする（repo なしなので取り込み済み扱い）。
    fn complete(&self, id: TaskId) {
        let s = &self.env.store;
        s.apply_transition(id, Trigger::Accept, None).ok();
        s.apply_transition(id, Trigger::Dispatch, None)
            .expect("dispatch");
        s.apply_transition(id, Trigger::WorkerDone, None)
            .expect("done");
        // 第 3 フェーズ I4: repo の無い root は成果物（`ArtifactProduced`）があって初めて取り込み済み。
        s.append_event(
            id,
            &task_core::Event::ArtifactProduced {
                run_id: "run".into(),
                artifact: task_core::ArtifactRef {
                    name: "report.md".into(),
                    path: "artifacts/report.md".into(),
                    sha256: "0".repeat(64),
                    kind: "file".into(),
                    declared: true,
                },
            },
        )
        .expect("artifact");
        s.apply_transition(id, Trigger::ReviewPass, None)
            .expect("pass");
    }

    async fn approve_first(&self, outcomes: Vec<Value>) {
        let p = self.propose(&proposal(None, outcomes)).await;
        assert_eq!(p.status.as_u16(), 201, "{}", p.text());
        let d = self.decide(1, "approve", &admin_headers()).await;
        assert_eq!(d.status.as_u16(), 200, "{}", d.text());
    }
}

#[tokio::test]
async fn project_roadmap_proposal_does_not_advance_the_version() {
    let f = fx();
    let none = f.current().await;
    assert_problem(&none, 404, "roadmap_not_found");
    let p = f.propose(&proposal(None, vec![outcome("o1", &[])])).await;
    assert_eq!(p.status.as_u16(), 201, "{}", p.text());
    let v = p.json();
    assert_eq!(v["version"], 1);
    assert_eq!(v["status"], "proposed");
    assert_eq!(v["proposed_by"], TEST_ACTOR);
    assert_eq!(
        v["request_sha256"],
        task_core::project_proposal::sha256_hex("pilot:roadmap")
    );
    raw(
        "project_roadmap_proposal_does_not_advance_the_version",
        json!({"version": 1, "from": null, "to": "proposed", "actor": v["proposed_by"]}),
    );
    // 提案だけでは今の版は無いまま。snapshot も Roadmap を読まない。
    assert_problem(&f.current().await, 404, "roadmap_not_found");
    assert!(f.snapshot().roadmap.is_none());
    // 未知の欄は拒む。
    let bad = f
        .propose(&json!({"base_version": null, "change_reason": "x", "outcomes": [outcome("o1", &[])], "go": true}))
        .await;
    assert!(bad.status.is_client_error(), "{}", bad.text());
}

#[tokio::test]
async fn project_roadmap_human_approval_advances_and_supersedes() {
    let f = fx();
    f.approve_first(vec![outcome("o1", &[])]).await;
    let cur = f.current().await;
    assert_eq!(cur.status.as_u16(), 200, "{}", cur.text());
    assert_eq!(cur.json()["version"], 1);
    assert_eq!(cur.json()["decided_by"], TEST_ACTOR);
    // 土台の違う提案は 409、正しい土台の提案は proposed のまま。
    let stale = f
        .propose(&proposal(
            None,
            vec![outcome("o1", &[]), outcome("o2", &["o1"])],
        ))
        .await;
    assert_problem(&stale, 409, "roadmap_version_conflict");
    let p2 = f
        .propose(&proposal(
            Some(1),
            vec![outcome("o1", &[]), outcome("o2", &["o1"])],
        ))
        .await;
    assert_eq!(p2.json()["version"], 2);
    assert_eq!(
        f.current().await.json()["version"],
        1,
        "a proposal keeps the version"
    );
    let d2 = f.decide(2, "approve", &admin_headers()).await;
    assert_eq!(d2.status.as_u16(), 200, "{}", d2.text());
    let h = f.history().await;
    let statuses: Vec<&str> = h["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, vec!["superseded", "approved"]);
    for v in h["versions"].as_array().unwrap() {
        raw(
            "project_roadmap_human_approval_advances_and_supersedes",
            json!({"version": v["version"], "to": v["status"], "proposed_by": v["proposed_by"], "decided_by": v["decided_by"]}),
        );
    }
    assert_eq!(f.current().await.json()["version"], 2);
    let again = f.decide(2, "approve", &admin_headers()).await;
    assert_problem(&again, 409, "roadmap_already_decided");
    let p3 = f
        .propose(&proposal(
            Some(2),
            vec![outcome("o1", &[]), outcome("o3", &[])],
        ))
        .await;
    assert_problem(&p3, 422, "roadmap_outcome_id_removed");
}

#[tokio::test]
async fn project_roadmap_cos_and_operator_cannot_decide() {
    let f = fx();
    let bearer = cos_run_bearer(&f.env, &f.thread, "rm");
    let headers = [("authorization", bearer.as_str())];
    // CoS は /cos/operations で提案できる（版は進まない）。
    let prop = send(
        &f.app,
        post_json_with(
            OPS,
            &op_body(
                "rm-propose",
                "POST",
                &f.path("/proposals"),
                proposal(None, vec![outcome("o1", &[])]),
            ),
            &headers,
        ),
    )
    .await;
    assert_eq!(prop.status.as_u16(), 200, "{}", prop.text());
    assert_eq!(prop.json()["operation"]["state"], "applied");
    let row = f
        .env
        .store
        .roadmap_version(&f.project.to_string(), 1)
        .unwrap()
        .unwrap();
    assert!(row.proposed_by.starts_with("cos:"), "{}", row.proposed_by);
    assert_eq!(row.status, task_core::roadmap::RoadmapStatus::Proposed);
    // CoS の決定: registry の EXCLUDED（422 human_only）。人でない token（operator）は 403。
    let via_ops = send(
        &f.app,
        post_json_with(
            OPS,
            &op_body(
                "rm-decide",
                "POST",
                &f.path("/proposals/1/decide"),
                json!({"decision": "approve"}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&via_ops, 422, "cos_operation_not_allowed");
    // CoS の run の credential での直接の呼び出しは、既存の middleware が /cos/operations の外として止める。
    let direct = f.decide(1, "approve", &headers).await;
    assert_problem(&direct, 422, "cos_audit_context_required");
    let op_bearer = format!("Bearer {OP_TOKEN}");
    let operator = f
        .decide(1, "approve", &[("authorization", op_bearer.as_str())])
        .await;
    assert_problem(&operator, 403, "roadmap_decide_requires_human");
    let status_by_cos = send(
        &f.app,
        post_json_with(
            OPS,
            &op_body(
                "rm-status",
                "POST",
                &f.path("/outcomes/o1/status"),
                json!({"status": "reached", "evidence_refs": []}),
            ),
            &headers,
        ),
    )
    .await;
    assert_problem(&status_by_cos, 422, "cos_operation_not_allowed");
    for (who, code) in [
        ("cos", via_ops.status.as_u16()),
        ("cos-direct", direct.status.as_u16()),
        ("operator", operator.status.as_u16()),
    ] {
        raw(
            "project_roadmap_cos_and_operator_cannot_decide",
            json!({"version": 1, "actor": who, "decide_status": code}),
        );
    }
    let row = f
        .env
        .store
        .roadmap_version(&f.project.to_string(), 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.status,
        task_core::roadmap::RoadmapStatus::Proposed,
        "still undecided"
    );
    assert!(
        f.env
            .store
            .roadmap_approved(&f.project.to_string())
            .unwrap()
            .is_none()
    );
    // 人が決める。
    let human = f.decide(1, "approve", &admin_headers()).await;
    assert_eq!(human.status.as_u16(), 200, "{}", human.text());
}

#[tokio::test]
async fn project_roadmap_stable_ids_cross_versions() {
    let f = fx();
    f.approve_first(vec![outcome("o1", &[]), outcome("o2", &["o1"])])
        .await;
    let mut dropped = outcome("o2", &["o1"]);
    dropped["dropped"] = json!(true);
    let mut changed = outcome("o1", &[]);
    changed["acceptance"] = json!(["o1 を確かめる", "o1 の計測を 3 回繰り返す"]);
    let p = f
        .propose(&proposal(
            Some(1),
            vec![changed, dropped, outcome("o3", &["o1"])],
        ))
        .await;
    assert_eq!(p.status.as_u16(), 201, "{}", p.text());
    f.decide(2, "approve", &admin_headers()).await;
    let v1 = send(&f.app, get_admin(&f.path("?version=1"))).await.json();
    let v2 = f.current().await.json();
    let ids = |v: &Value| -> Vec<(String, String)> {
        v["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| {
                (
                    o["id"].as_str().unwrap().to_owned(),
                    o["status"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    assert_eq!(
        ids(&v1),
        vec![("o1".into(), "open".into()), ("o2".into(), "open".into())]
    );
    assert_eq!(
        ids(&v2),
        vec![
            ("o1".into(), "open".into()),
            ("o2".into(), "dropped".into()),
            ("o3".into(), "open".into())
        ]
    );
    assert_eq!(v1["status"], "superseded");
    // 状態の記録で dropped は付けられない。
    let drop = send(
        &f.app,
        post_admin(
            &f.path("/outcomes/o1/status"),
            &json!({"status": "dropped"}),
        ),
    )
    .await;
    assert_problem(&drop, 422, "roadmap_drop_needs_version");
    // 取り下げた Outcome は snapshot に出ない。
    let snap = f.snapshot();
    let snap_ids: Vec<&str> = snap.outcomes.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(snap_ids, vec!["o1", "o3"]);
}

#[tokio::test]
async fn project_roadmap_root_links_and_reached_need_rust_evidence() {
    let f = fx();
    f.approve_first(vec![outcome("o1", &[])]).await;
    let r1 = f.root("1 回目", "o1 を確かめる");
    let r2 = f.root("2 回目", "o1 を確かめる");
    let status = |body: Value| {
        let app = f.app.clone();
        let path = f.path("/outcomes/o1/status");
        async move { send(&app, post_admin(&path, &body)).await }
    };
    let prog =
        status(json!({"status": "in_progress", "evidence_refs": [format!("task:{r1}")]})).await;
    assert_eq!(prog.status.as_u16(), 200, "{}", prog.text());
    // 終わっていない root を根拠に reached にはできない（自己申告では進まない）。
    let early = status(json!({"status": "reached", "evidence_refs": [format!("task:{r1}")]})).await;
    assert_problem(&early, 422, "roadmap_evidence_unverified");
    let empty = status(json!({"status": "reached", "evidence_refs": []})).await;
    assert_problem(&empty, 422, "roadmap_evidence_required");
    let foreign =
        status(json!({"status": "reached", "evidence_refs": [format!("task:{}", TaskId::new())]}))
            .await;
    assert_problem(&foreign, 422, "roadmap_evidence_unknown");
    f.complete(r2);
    let reached = status(json!({"status": "reached", "evidence_refs": [format!("task:{r2}")], "reason": "2 回目で合格"})).await;
    assert_eq!(reached.status.as_u16(), 200, "{}", reached.text());
    let o1 = &reached.json()["outcomes"][0];
    assert_eq!(o1["status"], "reached");
    assert_eq!(o1["evidence"], json!([format!("task:{r2}")]));
    assert_eq!(
        o1["root_task_ids"],
        json!([r1.to_string(), r2.to_string()]),
        "both roots in order"
    );
    let h = f.history().await;
    let kinds: Vec<(String, Option<String>)> = h["outcome_events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["kind"].as_str().unwrap().to_owned(),
                e["status"].as_str().map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("root_linked".into(), None),
            ("status".into(), Some("in_progress".into())),
            ("root_linked".into(), None),
            ("status".into(), Some("reached".into())),
        ]
    );
    for e in h["outcome_events"].as_array().unwrap() {
        raw(
            "project_roadmap_root_links_and_reached_need_rust_evidence",
            json!({"outcome": e["outcome_id"], "kind": e["kind"], "status": e["status"], "root": e["root_task_id"], "actor": e["actor"]}),
        );
    }
    // operator の token は状態を記録できない。
    let op_bearer = format!("Bearer {OP_TOKEN}");
    let op = send(
        &f.app,
        post_json_with(
            &f.path("/outcomes/o1/status"),
            &json!({"status": "open"}),
            &[("authorization", op_bearer.as_str())],
        ),
    )
    .await;
    assert_problem(&op, 403, "roadmap_status_requires_human");
}

#[tokio::test]
async fn project_roadmap_legacy_milestones_stay_gone() {
    let f = fx();
    f.approve_first(vec![outcome("o1", &[])]).await;
    let count = |store: &task_core::SqliteStore| -> i64 {
        let _ = store;
        let conn = rusqlite::Connection::open(&f.env.db_path).expect("db");
        conn.query_row("SELECT COUNT(*) FROM milestones", [], |r| r.get(0))
            .expect("milestones table")
    };
    let before = count(&f.env.store);
    let create = send(
        &f.app,
        post_admin(
            &format!("/api/v1/projects/{}/milestones", f.project),
            &json!({"title": "旧"}),
        ),
    )
    .await;
    assert_eq!(create.status.as_u16(), 410, "{}", create.text());
    let patch = send(
        &f.app,
        patch_admin("/api/v1/milestones/01M00000000000000000000000", &json!({})),
    )
    .await;
    assert_eq!(patch.status.as_u16(), 410, "{}", patch.text());
    assert_eq!(
        count(&f.env.store),
        before,
        "the roadmap never writes milestones"
    );
}

#[tokio::test]
async fn project_roadmap_ignores_task_milestone_and_phase() {
    let f = fx();
    f.approve_first(vec![outcome("o1", &[])]).await;
    // 旧 milestone の id を持つ root（凍結の履歴の形）と、Outcome の文を持たない root。
    let legacy = f.root("旧 milestone の root", "別の条件");
    {
        let conn = rusqlite::Connection::open(&f.env.db_path).expect("db");
        conn.execute(
            "UPDATE tasks SET json = json_set(json, '$.milestone_id', '01M00000000000000000000000') WHERE id = ?1",
            [legacy.to_string()],
        )
        .expect("legacy milestone id");
    }
    f.complete(legacy);
    let v = f.current().await.json();
    assert_eq!(
        v["outcomes"][0]["root_task_ids"],
        json!([]),
        "milestone_id is not a link"
    );
    assert_eq!(v["outcomes"][0]["status"], "open");
    let bad = send(
        &f.app,
        post_admin(
            &f.path("/outcomes/o1/status"),
            &json!({"status": "reached", "evidence_refs": [format!("task:{legacy}")]}),
        ),
    )
    .await;
    assert_problem(&bad, 422, "roadmap_evidence_unverified");
}

#[tokio::test]
async fn project_roadmap_brief_replace_does_not_change_the_roadmap() {
    let f = fx();
    f.approve_first(vec![outcome("o1", &[])]).await;
    let before = f.current().await.json();
    let hist_before = f.history().await;
    for state in ["一つ目", "二つ目"] {
        let put = send(
            &f.app,
            put_json_with(
                &format!("/api/v1/projects/{}/status", f.project),
                &json!({"objective": "別の目的に見える要約", "state": state, "open_issues": [],
                        "next_actions": ["o9 を作る"], "waiting_on": [], "evidence_refs": []}),
                &admin_headers(),
            ),
        )
        .await;
        assert_eq!(put.status.as_u16(), 200, "{}", put.text());
    }
    assert_eq!(f.current().await.json(), before);
    assert_eq!(f.history().await, hist_before);
    let snap = f.snapshot();
    assert_eq!(snap.outcomes.len(), 1);
    assert_eq!(snap.brief.as_ref().unwrap()["state"], "二つ目");
}

#[tokio::test]
async fn project_roadmap_takes_precedence_over_the_request_block() {
    let f = fx();
    let request = "依頼\n```celeris-outcomes\n[{\"id\":\"o1\",\"title\":\"一\",\"acceptance\":[\"o1 を確かめる\"]},{\"id\":\"o2\",\"title\":\"二\",\"acceptance\":[\"o2 を確かめる\"]}]\n```\n";
    {
        let conn = rusqlite::Connection::open(&f.env.db_path).expect("db");
        conn.execute(
            "UPDATE projects SET request = ?1 WHERE id = ?2",
            [request, &f.project.to_string()],
        )
        .expect("request");
    }
    let block = f.snapshot();
    assert!(block.roadmap.is_none());
    let ids: Vec<(&str, &str)> = block
        .outcomes
        .iter()
        .map(|o| (o.id.as_str(), o.source.as_str()))
        .collect();
    assert_eq!(ids, vec![("o1", "request_block"), ("o2", "request_block")]);
    // 提案だけでは block のまま。
    let p = f
        .propose(&proposal(
            None,
            vec![outcome("o1", &[]), outcome("o3", &[])],
        ))
        .await;
    assert_eq!(
        p.json()["request_sha256"],
        task_core::project_proposal::sha256_hex(request)
    );
    assert_eq!(f.snapshot().outcomes.len(), 2);
    assert_eq!(f.snapshot().outcomes[1].id, "o2");
    f.decide(1, "approve", &admin_headers()).await;
    let snap = f.snapshot();
    let ids: Vec<(&str, &str)> = snap
        .outcomes
        .iter()
        .map(|o| (o.id.as_str(), o.source.as_str()))
        .collect();
    assert_eq!(ids, vec![("o1", "roadmap"), ("o3", "roadmap")]);
    let roadmap = snap.roadmap.expect("roadmap in snapshot");
    assert_eq!(roadmap["version"], 1);
    assert_eq!(roadmap["status"], "approved");
    raw(
        "project_roadmap_takes_precedence_over_the_request_block",
        json!({"block_outcomes": 2, "roadmap_outcomes": snap.outcomes.len(), "source": "roadmap"}),
    );
}

#[tokio::test]
async fn project_roadmap_reject_keeps_the_current_version() {
    let f = fx();
    f.approve_first(vec![outcome("o1", &[])]).await;
    f.propose(&proposal(
        Some(1),
        vec![outcome("o1", &[]), outcome("o2", &[])],
    ))
    .await;
    let r = f.decide(2, "reject", &admin_headers()).await;
    assert_eq!(r.status.as_u16(), 200, "{}", r.text());
    assert_eq!(r.json()["status"], "rejected");
    raw(
        "project_roadmap_reject_keeps_the_current_version",
        json!({"version": 2, "from": "proposed", "to": "rejected", "actor": r.json()["decided_by"]}),
    );
    let cur = f.current().await.json();
    assert_eq!(cur["version"], 1);
    assert_eq!(cur["status"], "approved");
    let missing = f.decide(9, "approve", &admin_headers()).await;
    assert_problem(&missing, 404, "roadmap_version_not_found");
    let unknown = send(
        &f.app,
        get_admin(&format!("/api/v1/projects/{}/roadmap", ProjectId::new())),
    )
    .await;
    assert_problem(&unknown, 404, "project_not_found");
}
