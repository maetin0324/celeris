//! ADR 2026-11-23-project-observability-publish D1: `GET /api/v1/projects/{id}/overview`（読み取り専用）の統合試験。
//!
//! 守る不変条件: 14 項目を決めた順で返し、既存の GET の欄を写さない。読むだけで行を変えない。戦略の版は decisions から引く。
//! 人でない回答・回答待ちの戦略を承認済みと出さない。反証・矛盾・古い戦略・未署名の manifest は強い警告。根拠の無い評価の主張は
//! confirmed にならず、評価の 3 軸と negative_result は分けて出す。別の案件の資料を出さない。
//! base にはこの route が無いので、どの試験も変更前は 404 で落ちる。

mod common;

use axum::Router;
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::chat::ChatOpenThreadRequest;
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::model::{ArtifactRef, Event, Task};
use task_core::parent_checkpoint as pc;
use task_core::project_proposal as pp;
use task_core::{publish, strategy};
use time::OffsetDateTime;

const PROJECT: &str = "01M2WTS3DKNZBSZ2JMVB4CZMBW";
const OTHER: &str = "01M2WTS3DKNZBSZ2JMVB4CZMBX";
const WEB: &str = "web-token-value-0123456789";
const OPERATOR: &str = "operator-token-value-0123456789";
const RUN: &str = "01M84Q76RPS3T3E4RQYMDWGD7W";
const BASELINE: &str = include_str!("../../task-core/src/strategy/fixtures/baseline-v1.json");
const KEYS: [&str; 14] = [
    "original_goal",
    "approved_strategy",
    "candidates",
    "hypotheses",
    "verification",
    "refutations",
    "latest_findings",
    "discussion_sources",
    "experiments_and_tasks",
    "budget_spent",
    "next_verification",
    "pending_human_decisions",
    "risks",
    "publish_readiness",
];

struct World {
    env: TestEnv,
    app: Router,
    thread_id: String,
}

fn sha_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn project(id: &str) -> task_core::Project {
    let now = OffsetDateTime::now_utc();
    task_core::Project {
        id: id.parse().expect("project id"),
        title: format!("project {id}"),
        request: format!("Celeris を改善する（{id}）"),
        status: task_core::ProjectStatus::Active,
        workspace: None,
        archived_at: None,
        paused_from: None,
        auto_advance: false,
        slug: None,
        created_at: now,
        updated_at: now,
    }
}

fn opt(key: &str) -> DecisionOption {
    DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    }
}

impl World {
    fn new() -> Self {
        let env = web_operator_env(WEB, OPERATOR);
        let app = env.router();
        env.store
            .project_create(&project(PROJECT))
            .expect("project");
        env.store.project_create(&project(OTHER)).expect("other");
        let thread_id = env
            .store
            .cos_thread_open(
                &ChatOpenThreadRequest {
                    project_id: Some(PROJECT.into()),
                },
                OffsetDateTime::now_utc(),
            )
            .expect("thread")
            .thread
            .id;
        Self {
            env,
            app,
            thread_id,
        }
    }

    fn task(&self, project_id: &str) -> Task {
        let mut t = new_task(task_core::TaskKind::Execute, task_core::Status::Running);
        t.project_id = Some(project_id.parse().expect("project"));
        self.env.seed(&t);
        t
    }

    fn produce(&self, task: &Task, name: &str, bytes: &[u8]) -> String {
        let path = format!("artifacts/{name}");
        let dir = task_ops::workspace::local_dir(
            &self.env.store,
            &self.env.view_context().workspace_roots,
            task,
        )
        .expect("workspace");
        let file = dir.join(&path);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        std::fs::write(&file, bytes).expect("write");
        let sha256 = sha_hex(bytes);
        self.env
            .store
            .append_event(
                task.id,
                &Event::ArtifactProduced {
                    run_id: RUN.into(),
                    artifact: ArtifactRef {
                        name: name.into(),
                        path,
                        sha256: sha256.clone(),
                        kind: "file".into(),
                        declared: true,
                    },
                },
            )
            .expect("artifact");
        sha256
    }

    fn raise(&self, task: &Task, id: &str, key: String, question: String, options: [&str; 2]) {
        self.env
            .store
            .append_event(
                task.id,
                &Event::DecisionRequested {
                    decision: Box::new(DecisionRequest {
                        id: id.into(),
                        key,
                        kind: DecisionKind::Choice,
                        question,
                        options: vec![opt(options[0]), opt(options[1])],
                        recommended: options[1].into(),
                        cost_of_reversal: CostOfReversal::High,
                        cost_note: None,
                        needed_before: vec!["self".into()],
                        path: vec![DecisionPathEntry {
                            task_id: task.id,
                            title: "overview".into(),
                            stage: None,
                            unit: None,
                        }],
                        raised_by: DecisionRaisedBy {
                            task_id: task.id,
                            run_id: Some(RUN.into()),
                            origin: DecisionOrigin::Planner,
                        },
                        status: DecisionStatus::Open,
                        answer: None,
                        withdrawn_reason: None,
                    }),
                },
            )
            .expect("decision");
    }

    /// 戦略の文書 v1 を作り、承認の決定 `id` を出す（子 2 D2.2）。`edit` で文書を変える。
    fn strategy(&self, project_id: &str, id: &str, edit: impl FnOnce(&mut Value)) -> Task {
        let task = self.task(project_id);
        let mut doc: Value = serde_json::from_str(BASELINE).expect("fixture");
        doc["project_id"] = json!(project_id);
        doc["source_revision"]["request_sha256"] = json!(task_core::project_proposal::sha256_hex(
            &project(project_id).request
        ));
        doc["source_revision"]["roadmap_version"] = Value::Null;
        edit(&mut doc);
        let bytes = doc.to_string().into_bytes();
        let sha = self.produce(&task, "strategy/arch-v4-cycle/v1.json", &bytes);
        self.raise(
            &task,
            id,
            strategy::approval_decision_key(1),
            strategy::approval_question("arch-v4-cycle", 1, &sha),
            [strategy::APPROVE_OPTION, strategy::REJECT_OPTION],
        );
        task
    }

    async fn answer(&self, bearer: &str, id: &str, option: &str) {
        let resp = send(
            &self.app,
            post_json_with(
                &format!("/api/v1/decisions/{id}/answer"),
                &json!({ "option": option }),
                &[("authorization", bearer)],
            ),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
    }

    async fn overview(&self, project_id: &str) -> Value {
        let resp = send(
            &self.app,
            get_with(
                &format!("/api/v1/projects/{project_id}/overview"),
                &[("authorization", &format!("Bearer {WEB}"))],
            ),
        )
        .await;
        assert_eq!(resp.status.as_u16(), 200, "{}", resp.text());
        resp.json()
    }
}

fn web() -> String {
    format!("Bearer {WEB}")
}

fn item<'a>(o: &'a Value, key: &str) -> &'a Value {
    o["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|i| i["key"] == key)
        .expect("item")
}

fn entries<'a>(o: &'a Value, key: &str) -> &'a Vec<Value> {
    item(o, key)["entries"].as_array().expect("entries")
}

fn strong(o: &Value, code: &str) -> bool {
    o["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .any(|w| w["code"] == code && w["strong"] == true)
}

#[tokio::test]
async fn project_overview_lists_fourteen_items_in_order() {
    let w = World::new();
    let o = w.overview(PROJECT).await;
    assert_eq!(o["schema"], "celeris.project-overview/1");
    let keys: Vec<&str> = o["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| i["key"].as_str().expect("key"))
        .collect();
    assert_eq!(keys, KEYS);
}

#[tokio::test]
async fn project_overview_does_not_copy_existing_get_fields() {
    let w = World::new();
    w.strategy(PROJECT, "dec-s", |_| {});
    let text = w.overview(PROJECT).await.to_string();
    for field in [
        "\"request\"",
        "\"root_totals\"",
        "\"outcomes\"",
        "\"next_wake\"",
        "\"budget_limits\"",
        "\"title\":\"project",
    ] {
        assert!(!text.contains(field), "{field} is copied: {text}");
    }
    assert!(!text.contains(&project(PROJECT).request));
}

#[tokio::test]
async fn project_overview_request_sha_matches_project_request() {
    let w = World::new();
    let o = w.overview(PROJECT).await;
    let want = task_core::project_proposal::sha256_hex(&project(PROJECT).request);
    assert_eq!(o["request_sha256"], want);
    let goal = &entries(&o, "original_goal")[0];
    assert_eq!(goal["mark"], "confirmed");
    assert_eq!(goal["detail"]["request_sha256"], want);
}

#[tokio::test]
async fn project_overview_is_read_only() {
    let w = World::new();
    let task = w.strategy(PROJECT, "dec-s", |_| {});
    let snapshot = |w: &World| {
        let conn = rusqlite::Connection::open(&w.env.db_path).expect("db");
        let mut out = Vec::new();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("tables")
            .collect::<Result<_, _>>()
            .expect("names");
        for t in tables {
            let n: i64 = conn
                .query_row(&format!("SELECT count(*) FROM \"{t}\""), [], |r| r.get(0))
                .expect("count");
            out.push((t, n));
        }
        out
    };
    let before = snapshot(&w);
    let events = w.env.store.events_for(task.id).expect("events").len();
    w.overview(PROJECT).await;
    // 応答時間の記録（進捗に書く。時間で合否を決めない）。
    let started = std::time::Instant::now();
    w.overview(PROJECT).await;
    eprintln!("project_overview fixture response: {:?}", started.elapsed());
    assert_eq!(snapshot(&w), before);
    assert_eq!(
        w.env.store.events_for(task.id).expect("events").len(),
        events
    );
}

#[tokio::test]
async fn project_overview_strategy_versions_from_decisions_not_artifact_scan() {
    let w = World::new();
    // 決定を出していない下書きの版は概要に出ない（D1.5）。
    let draft = w.task(PROJECT);
    let bytes: Value = serde_json::from_str(BASELINE).expect("fixture");
    w.produce(
        &draft,
        "strategy/draft-only/v1.json",
        bytes.to_string().as_bytes(),
    );
    let o = w.overview(PROJECT).await;
    assert_eq!(item(&o, "approved_strategy")["recorded"], false);
    w.strategy(PROJECT, "dec-s", |_| {});
    let o = w.overview(PROJECT).await;
    let s = entries(&o, "approved_strategy");
    assert_eq!(s.len(), 1);
    assert_eq!(s[0]["detail"]["strategy_id"], "arch-v4-cycle");
}

#[tokio::test]
async fn project_overview_unapproved_strategy_is_not_marked_approved() {
    let w = World::new();
    w.strategy(PROJECT, "dec-s", |_| {});
    let o = w.overview(PROJECT).await;
    let s = &entries(&o, "approved_strategy")[0];
    assert_eq!(s["mark"], "needs_decision");
    assert_eq!(s["detail"]["approved"], Value::Null);
    assert_eq!(s["detail"]["pending_versions"], json!([1]));
    // 回答待ちの版の候補も要判断。
    assert!(
        entries(&o, "candidates")
            .iter()
            .all(|c| c["mark"] == "needs_decision")
    );
    w.answer(&web(), "dec-s", strategy::APPROVE_OPTION).await;
    let o = w.overview(PROJECT).await;
    let s = &entries(&o, "approved_strategy")[0];
    assert_eq!(s["mark"], "confirmed", "{s}");
    assert_eq!(s["detail"]["approved"]["approved_by"], "human:web");
}

#[tokio::test]
async fn project_overview_operator_answered_strategy_is_not_approved() {
    let w = World::new();
    w.strategy(PROJECT, "dec-s", |_| {});
    w.answer(
        &format!("Bearer {OPERATOR}"),
        "dec-s",
        strategy::APPROVE_OPTION,
    )
    .await;
    let o = w.overview(PROJECT).await;
    let s = &entries(&o, "approved_strategy")[0];
    assert_ne!(s["mark"], "confirmed", "{s}");
    assert_eq!(s["detail"]["approved"], Value::Null);
}

#[tokio::test]
async fn project_overview_stale_strategy_is_contradicted_with_strong_warning() {
    let w = World::new();
    w.strategy(PROJECT, "dec-s", |d| {
        d["source_revision"]["request_sha256"] = json!("0".repeat(64));
    });
    w.answer(&web(), "dec-s", strategy::APPROVE_OPTION).await;
    let o = w.overview(PROJECT).await;
    let s = &entries(&o, "approved_strategy")[0];
    assert_eq!(s["mark"], "contradicted", "{s}");
    assert!(strong(&o, "strategy_stale"), "{}", o["warnings"]);
}

#[tokio::test]
async fn project_overview_contradicted_hypothesis_is_strong_warning() {
    let w = World::new();
    w.strategy(PROJECT, "dec-s", |_| {});
    w.answer(&web(), "dec-s", strategy::APPROVE_OPTION).await;
    let o = w.overview(PROJECT).await;
    let h = entries(&o, "hypotheses")
        .iter()
        .find(|h| h["detail"]["id"] == "h-pilots-enough")
        .expect("hypothesis");
    assert_eq!(h["mark"], "contradicted");
    assert!(o["warnings"].as_array().expect("w").iter().any(|w| {
        w["item"] == "hypotheses" && w["code"] == "contradicted" && w["strong"] == true
    }));
    // 根拠の無い仮説（文書が estimated）を confirmed にしない。
    let e = entries(&o, "hypotheses")
        .iter()
        .find(|h| h["detail"]["id"] == "h-cycle-edges")
        .expect("hypothesis");
    assert_eq!(e["mark"], "estimated");
}

#[tokio::test]
async fn project_overview_unapplied_refutation_is_strong_warning() {
    let w = World::new();
    // 反証された仮説を選んだ候補のまま承認した。
    w.strategy(PROJECT, "dec-s", |d| {
        d["candidates"][1]["selected"] = json!(true);
    });
    w.answer(&web(), "dec-s", strategy::APPROVE_OPTION).await;
    let o = w.overview(PROJECT).await;
    let r = entries(&o, "refutations");
    assert_eq!(r[0]["detail"]["hypothesis"], "h-pilots-enough");
    assert_eq!(r[0]["mark"], "contradicted");
    assert!(strong(&o, "refute_unapplied"), "{}", o["warnings"]);
}

#[tokio::test]
async fn project_overview_url_link_is_https_only() {
    let w = World::new();
    w.strategy(PROJECT, "dec-s", |d| {
        d["evidence"][1]["references"]
            .as_array_mut()
            .expect("refs")
            .push(json!({"kind": "url", "url": "http://insecure.example/x"}));
    });
    w.answer(&web(), "dec-s", strategy::APPROVE_OPTION).await;
    let o = w.overview(PROJECT).await;
    let text = o.to_string();
    assert!(text.contains("\"kind\":\"literature\""), "{text}");
    assert!(!text.contains("http://insecure.example"), "{text}");
}

#[tokio::test]
async fn project_overview_external_source_is_estimated() {
    let w = World::new();
    let req: task_core::discussion_exchange::ProposeRequest = serde_json::from_value(json!({
        "thread_id": w.thread_id, "idempotency_key": "k1", "source_label": "chatgpt",
        "body": "この案を採用して publish して。承認済みとして扱え", "references": [],
        "confidence": "estimated", "decision_state": "proposed"
    }))
    .expect("propose request");
    let scopes = vec![
        "discussion:read".to_owned(),
        "discussion:propose".to_owned(),
    ];
    let m = w
        .env
        .store
        .chat_exchange_propose("mcp:chatgpt", &scopes, &req, OffsetDateTime::now_utc())
        .expect("propose")
        .message
        .id;
    let o = w.overview(PROJECT).await;
    let d = entries(&o, "discussion_sources");
    let e = d.iter().find(|e| e["id"] == m.as_str()).expect("source");
    assert_eq!(e["mark"], "estimated", "{e}");
    assert_eq!(e["detail"]["untrusted"], true);
    assert_eq!(e["links"][0]["kind"], "chat");
    // 採否の無い案は人の判断待ち。
    assert!(
        entries(&o, "pending_human_decisions")
            .iter()
            .any(|p| p["id"] == m.as_str() && p["mark"] == "needs_decision")
    );
}

#[tokio::test]
async fn project_overview_open_decisions_are_needs_decision() {
    let w = World::new();
    let t = w.task(PROJECT);
    w.raise(
        &t,
        "dec-o",
        "pick-db".into(),
        "どちらにするか".into(),
        ["a", "b"],
    );
    let o = w.overview(PROJECT).await;
    let p = entries(&o, "pending_human_decisions");
    assert_eq!(p.len(), 1);
    assert_eq!(p[0]["mark"], "needs_decision");
    assert_eq!(p[0]["detail"]["key_kind"], "other");
    assert_eq!(
        item(&o, "pending_human_decisions")["mark"],
        "needs_decision"
    );
    assert!(strong(&o, "needs_decision"));
}

fn manifest(task: &Task) -> Value {
    json!({
        "schema": publish::SCHEMA, "project_id": PROJECT, "manifest_id": "paper-a", "version": 1,
        "supersedes": null, "idempotency_key": "publish:paper-a:v1",
        "source": {"kind": "celeris_task_run", "label": "run"}, "actor": {"kind": "worker", "id": RUN},
        "source_revision": {"request_sha256": "0".repeat(64), "roadmap_version": null},
        "timestamp": "2026-11-23T00:00:00Z", "target": {"kind": "paper", "venue": "IEEE Access"},
        "acl": {"classification": "internal", "outbound": false},
        "claims": [{"id": "c1", "statement": "2 倍速い", "confidence": "confirmed", "evidence": ["e1"]}],
        "evidence": [{"id": "e1", "stance": "support", "statement": "測定",
                      "references": [{"kind": "task", "task_id": task.id.to_string()}]}],
        "reproduction": [{"id": "r1", "claims": ["c1"], "commands": ["bash run.sh"],
            "environment": {"repo": "agent-platform", "commit": "074f2e64ab13c0ffee0000000000000000000000",
                            "host_class": "sirius"}, "data": [], "expected": "2 倍"}],
        "limitations": [], "open_questions": [], "decision_state": "proposed"
    })
}

#[tokio::test]
async fn project_overview_publish_readiness_from_signoff_decisions() {
    let w = World::new();
    let task = w.task(PROJECT);
    let sha = w.produce(
        &task,
        "publish/paper-a/v1.json",
        manifest(&task).to_string().as_bytes(),
    );
    // 決定が無ければ manifest の file があっても出ない（decisions から引く）。
    let o = w.overview(PROJECT).await;
    assert_eq!(item(&o, "publish_readiness")["recorded"], false);
    w.raise(
        &task,
        "dec-p",
        publish::signoff_decision_key(1),
        publish::signoff_question("paper-a", 1, &sha),
        [publish::SIGN_OPTION, publish::REJECT_OPTION],
    );
    let o = w.overview(PROJECT).await;
    let p = &entries(&o, "publish_readiness")[0];
    assert_eq!(p["mark"], "needs_decision", "{p}");
    assert_eq!(p["detail"]["signoff"], "unsigned");
    assert!(strong(&o, "needs_decision"));
    w.answer(&web(), "dec-p", publish::SIGN_OPTION).await;
    let o = w.overview(PROJECT).await;
    let p = &entries(&o, "publish_readiness")[0];
    assert_eq!(p["mark"], "confirmed", "{p}");
    assert_eq!(p["detail"]["signoff"], "signed");
}

#[tokio::test]
async fn project_overview_unsigned_or_stale_manifest_is_strong_warning() {
    let w = World::new();
    let task = w.task(PROJECT);
    let sha = w.produce(
        &task,
        "publish/paper-a/v1.json",
        manifest(&task).to_string().as_bytes(),
    );
    w.raise(
        &task,
        "dec-p",
        publish::signoff_decision_key(1),
        publish::signoff_question("paper-a", 1, &sha),
        [publish::SIGN_OPTION, publish::REJECT_OPTION],
    );
    w.answer(&web(), "dec-p", publish::SIGN_OPTION).await;
    // 署名の後に file を書き換えた（失効）。
    let dir =
        task_ops::workspace::local_dir(&w.env.store, &w.env.view_context().workspace_roots, &task)
            .expect("workspace");
    std::fs::write(dir.join("artifacts/publish/paper-a/v1.json"), b"{}").expect("overwrite");
    let o = w.overview(PROJECT).await;
    let p = &entries(&o, "publish_readiness")[0];
    assert_eq!(p["mark"], "contradicted", "{p}");
    assert!(strong(&o, "manifest_invalid"), "{}", o["warnings"]);
    assert_eq!(item(&o, "risks")["mark"], "contradicted");
}

/// root の評価の判断 1 件と最終の記録を置く（子 6 D3.4）。
fn evaluation(w: &World, root: &Task, decision_id: &str, evaluation: Value) {
    let proposal = pp::ProjectProposal {
        action: pp::ProjectAction::Evaluate {
            root_id: root.id.to_string(),
            triggers: vec!["children:2".into()],
        },
        gap: pp::ProposalGap {
            reached: vec![],
            open: vec!["o1".into()],
            unknown: vec![],
            progress_epoch: 0,
        },
        explanation: pp::ProposalExplanation {
            rule_id: "pc/checkpoint/evaluate".into(),
            signals: vec![],
        },
        based_on: pp::ProposalBasedOn {
            revision: 0,
            human_epoch: 0,
            request_sha256: String::new(),
        },
    };
    let conn = rusqlite::Connection::open(&w.env.db_path).expect("db");
    conn.execute(
        "INSERT INTO project_controller_decisions (id, project_id, based_on_revision, human_epoch, \
         snapshot_sha256, request_sha256, mode, proposal_json, validation, effect_json, effect_state, \
         created_at) VALUES (?1, ?2, 0, 0, '', '', 'enabled', ?3, 'ok', '{}', 'applied', \
         '2026-11-23T00:00:00Z')",
        rusqlite::params![
            decision_id,
            PROJECT,
            serde_json::to_string(&proposal).expect("proposal")
        ],
    )
    .expect("insert decision");
    let facts = pc::EvaluationFacts {
        resolved: vec![],
        goal_reached: false,
        children: pc::ChildOutcomes::default(),
    };
    let checked = pc::check_evaluation(Some(&evaluation), &facts);
    let record = pc::ParentEvaluationRecord::new(
        decision_id,
        &root.id.to_string(),
        Some(evaluation),
        &checked,
    );
    w.produce(
        root,
        &pc::evaluation_artifact_name(decision_id),
        &serde_json::to_vec(&record).expect("record"),
    );
}

fn off_track_negative() -> Value {
    json!({
        "schema": pc::EVALUATION_SCHEMA, "verdict": "off_track", "next": "pivot",
        "axes": {"execution": "succeeded", "epistemic": "advanced", "goal": "not_met"},
        "measured": [{"claim": "SCC は 55 のまま", "metric": "scc", "before": "55", "after": "55",
                      "references": [{"kind": "url", "url": "https://example.org/scc"}]}],
        "learned": [], "refuted": [], "uncertainty": [],
        "reason": "子は done だが親の目標は動かない"
    })
}

#[tokio::test]
async fn project_overview_evaluation_axes_shown_separately() {
    let w = World::new();
    let root = w.task(PROJECT);
    evaluation(&w, &root, "D1", off_track_negative());
    let o = w.overview(PROJECT).await;
    let f = entries(&o, "latest_findings");
    let id = |s: &str| {
        f.iter()
            .find(|e| e["id"] == format!("D1/{s}"))
            .unwrap_or_else(|| panic!("{s} in {f:?}"))
    };
    let record = &id("verdict")["detail"];
    assert_eq!(id("axis/execution")["detail"]["axis"], "execution");
    assert_eq!(id("axis/epistemic")["detail"]["axis"], "epistemic");
    assert_eq!(id("axis/goal")["detail"]["axis"], "goal");
    assert!(
        id("negative_result")["detail"]
            .get("negative_result")
            .is_some()
    );
    // 値は記録の `effective`（子 6 の検査が子の事実から導いた値）で、評価器の自己申告をそのまま写さない。
    for axis in ["execution", "epistemic", "goal"] {
        assert!(id(&format!("axis/{axis}"))["detail"]["value"].is_string());
    }
    assert!(record.get("verdict").is_some() && record.get("next").is_some());
    assert!(
        strong(&o, "evaluation_off_track") || strong(&o, "evaluation_invalid"),
        "{}",
        o["warnings"]
    );
}

#[tokio::test]
async fn project_overview_unsupported_evaluation_claim_is_estimated_with_warning() {
    let w = World::new();
    let root = w.task(PROJECT);
    evaluation(&w, &root, "D1", off_track_negative());
    let o = w.overview(PROJECT).await;
    let f = entries(&o, "latest_findings");
    assert!(
        f.iter()
            .all(|e| e["mark"] != "confirmed" || e["id"] != "D1/measured[0]")
    );
    if let Some(m) = f.iter().find(|e| e["id"] == "D1/measured[0]") {
        assert_eq!(m["mark"], "estimated");
        assert_eq!(m["detail"]["supported"], false);
    }
    assert!(
        strong(&o, "evaluation_unsupported") || strong(&o, "evaluation_invalid"),
        "{}",
        o["warnings"]
    );
}

#[tokio::test]
async fn project_overview_other_project_data_excluded() {
    let w = World::new();
    let other = w.strategy(OTHER, "dec-x", |_| {});
    let o_task = w.task(OTHER);
    w.raise(&o_task, "dec-y", "pick".into(), "q".into(), ["a", "b"]);
    evaluation(&w, &o_task, "DX", off_track_negative());
    // この案件の戦略は別の案件の task を参照する。
    w.strategy(PROJECT, "dec-s", |d| {
        d["evidence"][1]["references"] = json!([{"kind": "task", "task_id": other.id.to_string()}]);
    });
    w.answer(&web(), "dec-s", strategy::APPROVE_OPTION).await;
    let o = w.overview(PROJECT).await;
    let text = o.to_string();
    assert!(!text.contains(&other.id.to_string()), "{text}");
    assert!(!text.contains(&o_task.id.to_string()), "{text}");
    assert!(!text.contains("dec-y") && !text.contains("dec-x"), "{text}");
    assert_eq!(entries(&o, "approved_strategy").len(), 1);
}

#[tokio::test]
async fn project_overview_empty_project_items_absent() {
    let w = World::new();
    let o = w.overview(PROJECT).await;
    for key in KEYS
        .iter()
        .filter(|k| **k != "original_goal" && **k != "risks")
    {
        let i = item(&o, key);
        assert_eq!(i["recorded"], false, "{key}: {i}");
        assert!(i.get("mark").is_none(), "{key}: {i}");
    }
    assert_eq!(item(&o, "risks")["mark"], "confirmed");
    assert_eq!(o["warnings"], json!([]));
    // 別の案件の id は 404。
    let resp = send(
        &w.app,
        get_with(
            "/api/v1/projects/01M2WTS3DKNZBSZ2JMVB4CZMBZ/overview",
            &[("authorization", &web())],
        ),
    )
    .await;
    assert_eq!(resp.status.as_u16(), 404);
}

// ---- drill-down（D1.4）: 実験の task の log・notebook・task 付きの git -------------------------------------------
//
// 守る不変条件: 実験の task（X(v)。O5・O9・O10）ごとに、最後の run を `log`、`.ipynb` の成果物を `notebook`、
// 最後の `IntegrationMerged` を `task_id` 付きの `git` にする。戦略の文書の `commit` の参照も、取り込みの sha と
// 一致すれば `task_id` を付ける。既存の試験は戦略の文書だけを置き、実験の task の run・成果物・取り込みを
// 置かないので、これらの link が無くても通っていた（fake-daemon の fixture の link を押す e2e も本物の backend を見ない）。

const STRATEGY_NAME: &str = "strategy/arch-v4-cycle/v1.json";
const REPO: &str = "agent-platform";
const MERGED_SHA: &str = "4a7c0d5e9b1f2a3c4d5e6f708192a3b4c5d6e7f8";
const OTHER_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

impl World {
    /// 戦略の文書を実行する X(v) の task（CoS の strategy 付きの `task.create` の行。子 5 D2.3）。
    fn experiment(
        &self,
        strategy: &Task,
        candidate: &str,
        repo: Option<&task_core::RepoRef>,
    ) -> Task {
        let mut t = new_task(task_core::TaskKind::Execute, task_core::Status::Running);
        t.project_id = Some(PROJECT.parse().expect("project"));
        t.repos = repo.into_iter().cloned().collect();
        self.env.seed(&t);
        let sha = self
            .env
            .store
            .events_for(strategy.id)
            .expect("events")
            .into_iter()
            .find_map(|(_, e)| match e {
                Event::ArtifactProduced { artifact, .. } if artifact.name == STRATEGY_NAME => {
                    Some(artifact.sha256)
                }
                _ => None,
            })
            .expect("strategy artifact");
        let conn = rusqlite::Connection::open(&self.env.db_path).expect("db");
        let n: i64 = conn
            .query_row("SELECT count(*) FROM cos_operations", [], |r| r.get(0))
            .expect("count");
        let payload = json!({ "strategy": { "task_id": strategy.id.to_string(), "name": STRATEGY_NAME,
                                            "sha256": sha, "candidate": candidate } });
        conn.execute(
            "INSERT INTO cos_operations (id, thread_id, run_id, idempotency_key, request_hash, target_kind, \
             target_id, action, payload_json, reason, policy_version, state, created_at, updated_at) \
             VALUES (?1, ?2, 'cos-run', ?1, 'h', 'task', ?3, 'task.create', ?4, 'r', '11', 'applied', ?5, ?5)",
            rusqlite::params![
                format!("op-{n}"),
                self.thread_id,
                t.id.to_string(),
                payload.to_string(),
                format!("2026-11-23T01:00:{n:02}Z")
            ],
        )
        .expect("task.create");
        t
    }

    /// worker の run 1 本（`runs` の索引）。
    fn run(&self, task: &Task, run_id: &str, started_at: &str) {
        self.env
            .store
            .run_index_start(task_core::RunRow {
                run_id: run_id.into(),
                task_id: task.id.to_string(),
                work_unit_id: None,
                role: task_core::RunIndexRole::Worker,
                seq: 1,
                status: task_core::RunIndexStatus::Completed,
                adapter: Some("fake".into()),
                model: Some("m".into()),
                account: None,
                session_id: None,
                checkpoint: None,
                usage: Some(task_core::Usage {
                    input_tokens: Some(1000),
                    output_tokens: Some(100),
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    cost_usd: Some(0.25),
                    duplicate_reads: None,
                    session_resumed: None,
                    context_tokens: None,
                }),
                metrics: None,
                started_at: started_at.into(),
                finished_at: Some(started_at.into()),
            })
            .expect("run");
    }

    /// task のブランチを取り込んだ（delivery と sweep が使う store の口。`IntegrationMerged` を足す）。
    fn merged(&self, task: &Task, sha: &str) {
        let now = OffsetDateTime::now_utc();
        self.env
            .store
            .task_integration_upsert(&task_core::TaskRepoIntegrationState {
                task_id: task.id,
                repo_id: task.repos[0].repo_id,
                branch: format!("celeris/{}", task.id),
                head_sha: sha.into(),
                state: task_core::TaskRepoIntegrationStatus::Integrated,
                reason: None,
                actor: task_core::DAEMON_ACTOR.into(),
                checked_at: now,
                updated_at: now,
            })
            .expect("integrated");
    }
}

fn repo_ref() -> task_core::RepoRef {
    task_core::RepoRef {
        repo_id: task_core::RepoId::new(),
        name: REPO.into(),
    }
}

fn entry_of<'a>(o: &'a Value, key: &str, id: &str) -> &'a Value {
    entries(o, key)
        .iter()
        .find(|e| e["id"] == id)
        .unwrap_or_else(|| panic!("{key}/{id} in {}", item(o, key)))
}

fn links_of<'a>(e: &'a Value, kind: &str) -> Vec<&'a Value> {
    e["links"]
        .as_array()
        .expect("links")
        .iter()
        .filter(|l| l["kind"] == kind)
        .collect()
}

#[tokio::test]
async fn project_overview_drilldown_log_link_points_to_latest_run_of_experiment_task() {
    let w = World::new();
    let strategy = w.strategy(PROJECT, "dec-s", |_| {});
    w.answer(&web(), "dec-s", strategy::APPROVE_OPTION).await;
    let exp = w.experiment(&strategy, "cut-min-edges", None);
    let idle = w.experiment(&strategy, "continue-pilots", None);
    // 新しい run を先に索引へ入れる（挿入の順でなく started_at の順で最後を選ぶ）。
    let (old, new) = (ulid::Ulid::new().to_string(), ulid::Ulid::new().to_string());
    w.run(&exp, &new, "2026-11-23T11:00:00Z");
    w.run(&exp, &old, "2026-11-23T10:00:00Z");
    let o = w.overview(PROJECT).await;
    let want = json!({ "kind": "log", "task_id": exp.id.to_string(), "run_id": new });
    // O9: 実験の task ごと。
    let e = entry_of(&o, "experiments_and_tasks", &exp.id.to_string());
    assert_eq!(links_of(e, "log"), vec![&want], "{e}");
    // run の無い実験の task には log を出さない。
    let e = entry_of(&o, "experiments_and_tasks", &idle.id.to_string());
    assert!(links_of(e, "log").is_empty(), "{e}");
    // O5: 識別実験の task の log。
    let e = entry_of(&o, "verification", "arch-v4-cycle/v1/h-cycle-edges");
    assert_eq!(links_of(e, "log"), vec![&want], "{e}");
    // O10: X(v) の task の log。
    let e = entry_of(&o, "budget_spent", "arch-v4-cycle/v1");
    assert_eq!(links_of(e, "log"), vec![&want], "{e}");
    assert!(!o.to_string().contains(&old), "older run is linked: {o}");
}

#[tokio::test]
async fn project_overview_drilldown_notebook_link_from_experiment_ipynb_artifact() {
    let w = World::new();
    let strategy = w.strategy(PROJECT, "dec-s", |_| {});
    w.answer(&web(), "dec-s", strategy::APPROVE_OPTION).await;
    let exp = w.experiment(&strategy, "cut-min-edges", None);
    w.produce(
        &exp,
        "analysis/scc.ipynb",
        br#"{"cells": [], "nbformat": 4}"#,
    );
    w.produce(&exp, "report.md", b"# report");
    // X(v) でない task の Notebook は O5 に出さない。
    let stray = w.task(PROJECT);
    w.produce(&stray, "stray.ipynb", b"{}");
    let o = w.overview(PROJECT).await;
    let e = entry_of(&o, "verification", "arch-v4-cycle/v1/h-cycle-edges");
    assert_eq!(
        links_of(e, "notebook"),
        vec![
            &json!({ "kind": "notebook", "task_id": exp.id.to_string(), "name": "analysis/scc.ipynb" })
        ],
        "{e}"
    );
    assert!(links_of(e, "artifact").is_empty(), "{e}");
    assert!(!o.to_string().contains("stray.ipynb"), "{o}");
    // 別の仮説（実験の task が無い）には notebook が付かない。
    let e = entry_of(&o, "verification", "arch-v4-cycle/v1/h-pilots-enough");
    assert!(links_of(e, "notebook").is_empty(), "{e}");
}

#[tokio::test]
async fn project_overview_drilldown_git_link_carries_task_id_from_integration() {
    let w = World::new();
    // 反証の証拠が、実験の task が取り込んだ commit と、取り込みの記録の無い commit を引く。
    let strategy = w.strategy(PROJECT, "dec-s", |d| {
        let refs = d["evidence"][1]["references"].as_array_mut().expect("refs");
        refs.push(json!({"kind": "commit", "repo": REPO, "sha": MERGED_SHA}));
        refs.push(json!({"kind": "commit", "repo": REPO, "sha": OTHER_SHA}));
    });
    w.answer(&web(), "dec-s", strategy::APPROVE_OPTION).await;
    let repo = repo_ref();
    let exp = w.experiment(&strategy, "cut-min-edges", Some(&repo));
    w.merged(&exp, MERGED_SHA);
    let pending = w.experiment(&strategy, "continue-pilots", Some(&repo));
    let o = w.overview(PROJECT).await;
    let want =
        json!({ "kind": "git", "repo": REPO, "sha": MERGED_SHA, "task_id": exp.id.to_string() });
    // O9: 取り込まれた実験の task。
    let e = entry_of(&o, "experiments_and_tasks", &exp.id.to_string());
    assert_eq!(links_of(e, "git"), vec![&want], "{e}");
    // 取り込みの記録の無い実験の task には git を出さない。
    let e = entry_of(&o, "experiments_and_tasks", &pending.id.to_string());
    assert!(links_of(e, "git").is_empty(), "{e}");
    // O6: 戦略の文書の commit の参照は、取り込みの sha と一致したものだけ task_id を持つ。
    let e = entries(&o, "refutations")
        .iter()
        .find(|e| e["detail"]["hypothesis"] == "h-pilots-enough")
        .expect("refutation");
    assert_eq!(
        links_of(e, "git"),
        vec![
            &want,
            &json!({ "kind": "git", "repo": REPO, "sha": OTHER_SHA })
        ],
        "{e}"
    );
}

/// 概要の JSON の id を初出順の記号に置き換える（T1・R1・TH1・M1・P1）。`generated_at`・`approved_at` は固定する。
fn normalized(mut o: Value, ids: &[(&str, String)]) -> String {
    fn fix_times(v: &mut Value) {
        match v {
            Value::Object(map) => {
                for (k, v) in map.iter_mut() {
                    if (k == "generated_at" || k == "approved_at") && v.is_string() {
                        *v = json!("2026-11-24T00:00:00Z");
                    } else {
                        fix_times(v);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(fix_times),
            _ => {}
        }
    }
    fix_times(&mut o);
    let mut text = serde_json::to_string_pretty(&o).expect("pretty");
    let mut found: Vec<(usize, &str, &str)> = ids
        .iter()
        .filter_map(|(prefix, id)| text.find(id.as_str()).map(|p| (p, *prefix, id.as_str())))
        .collect();
    found.sort();
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for (_, prefix, id) in found {
        let n = counts.entry(prefix).or_default();
        *n += 1;
        text = text.replace(id, &format!("{prefix}{n}"));
    }
    text.push('\n');
    text
}

/// 本物の route が出す drill-down の形を fixture に固定する（Web の e2e が同じ JSON を使う）。
/// `UPDATE_FIXTURE=1` で書き直す。
#[tokio::test]
async fn project_overview_drilldown_fixture_matches_generated() {
    let w = World::new();
    let strategy = w.strategy(PROJECT, "dec-s", |d| {
        d["evidence"][1]["references"]
            .as_array_mut()
            .expect("refs")
            .push(json!({"kind": "commit", "repo": REPO, "sha": MERGED_SHA}));
    });
    w.answer(&web(), "dec-s", strategy::APPROVE_OPTION).await;
    let repo = repo_ref();
    let exp = w.experiment(&strategy, "cut-min-edges", Some(&repo));
    let idle = w.experiment(&strategy, "continue-pilots", Some(&repo));
    let (r1, r2) = (ulid::Ulid::new().to_string(), ulid::Ulid::new().to_string());
    w.run(&exp, &r1, "2026-11-23T10:00:00Z");
    w.run(&exp, &r2, "2026-11-23T11:00:00Z");
    w.produce(
        &exp,
        "analysis/scc.ipynb",
        br#"{"cells": [], "nbformat": 4}"#,
    );
    w.merged(&exp, MERGED_SHA);
    let req: task_core::discussion_exchange::ProposeRequest = serde_json::from_value(json!({
        "thread_id": w.thread_id, "idempotency_key": "k1", "source_label": "chatgpt",
        "body": "SCC の辺を切る案を先に試す", "references": [],
        "confidence": "estimated", "decision_state": "proposed"
    }))
    .expect("propose request");
    let message = w
        .env
        .store
        .chat_exchange_propose(
            "mcp:chatgpt",
            &[
                "discussion:read".to_owned(),
                "discussion:propose".to_owned(),
            ],
            &req,
            OffsetDateTime::now_utc(),
        )
        .expect("propose")
        .message
        .id;
    let o = w.overview(PROJECT).await;
    let ids = [
        ("P", PROJECT.to_string()),
        ("T", strategy.id.to_string()),
        ("T", exp.id.to_string()),
        ("T", idle.id.to_string()),
        ("R", r1),
        ("R", r2),
        ("TH", w.thread_id.clone()),
        ("M", message),
    ];
    let got = normalized(o, &ids);
    for kind in ["\"log\"", "\"notebook\"", "\"git\""] {
        assert!(
            got.contains(&format!("\"kind\": {kind}")),
            "{kind} missing: {got}"
        );
    }
    assert!(
        got.contains(&format!("\"sha\": \"{MERGED_SHA}\",\n")),
        "{got}"
    );
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/project_overview_drilldown.json"
    );
    if std::env::var_os("UPDATE_FIXTURE").is_some_and(|v| v == "1") {
        std::fs::write(path, &got).expect("write fixture");
    }
    let want = std::fs::read_to_string(path).unwrap_or_default();
    assert_eq!(
        got, want,
        "fixture differs; rerun with UPDATE_FIXTURE=1 and review the diff"
    );
}
