//! ADR 2026-11-22-strategy-review-critic D5・D7.1 の回帰: 通常の低抽象度の task（strategy の無いバグ修正）の経路は、案件に実験段階の
//! 戦略があっても変わらない。
//!
//! 守る不変条件:
//! - strategy の無い `task.create` は gate を通らず、案件に承認済みの実験段階の版（selected 無し）と起票済みの実験があっても、
//!   `execution` を省いたままで通る。応答・`cos_operations` の行・監査の `cos_operation` の event は、戦略の無い案件に同じ要求を
//!   送ったときと（id・時刻・event の番号を除いて）同じ。
//! - Critic の対象（`critic_subjects`）は戦略の文書を作った task だけにある。strategy の無い task と、strategy を参照して作った実験 task
//!   （文書を作らない）には無く、保存された受け入れ条件にも Critic の条件は足されない。dispatcher の reviewer run の条件
//!   （`reviewer_run_required`）と暗黙の条件 `strategy_critic` はこの対象が空なら base と同じ（task-dispatch の `strategy_critic_absent_*`）。
//!
//! 既存の試験で検出できない理由: 子 2 の `strategy_gate_absent_field_keeps_task_create_unchanged` は戦略の無い案件で strategy の無い
//! 要求を見るだけで、実験段階の戦略が同じ案件にあるときに gate の新しい規則（D2.4: compound・execution の省略を拒否）が案件ごとに
//! 掛かっていないことを確かめない。critic-review の試験は手で組んだ events の task を見るだけで、`/cos/operations` で実際に作った
//! 実験 task が Critic の対象にならないことを見ない。

mod common;

use axum::Router;
use common::cos_ops::{cos_run_bearer, post_op};
use common::*;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use task_core::chat::ChatOpenThreadRequest;
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::model::{ArtifactRef, Event, Task, TaskId};
use task_core::strategy;
use task_core::strategy_critic::critic_subjects;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// 子 2・子 3 の試験が共有する基準の文書（実験段階: open の候補 2 件、selected 無し）。
const BASELINE: &str = include_str!("../../task-core/src/strategy/fixtures/baseline-v1.json");
/// 2 つの env で同じ案件の id（要求の request_hash が同じになる）。
const PROJECT: &str = "01M2WTS3DKNZBSZ2JMVB4CZMBW";
const RUN: &str = "01M7REGRESSIONRUN000000001";

struct World {
    env: TestEnv,
    app: Router,
    bearer: String,
    project: task_core::Project,
}

fn sha_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl World {
    fn new() -> Self {
        let env = admin_env();
        let app = env.router();
        let now = OffsetDateTime::now_utc();
        let project = task_core::Project {
            id: PROJECT.parse().expect("project id"),
            title: "agent-platform".into(),
            request: "Celeris を Celeris 上の組織で改善する".into(),
            status: task_core::ProjectStatus::Active,
            workspace: None,
            archived_at: None,
            paused_from: None,
            auto_advance: false,
            slug: None,
            created_at: now,
            updated_at: now,
        };
        env.store.project_create(&project).expect("project");
        let thread = env
            .store
            .cos_thread_open(
                &ChatOpenThreadRequest {
                    project_id: Some(project.id.to_string()),
                },
                now,
            )
            .expect("thread")
            .thread;
        let bearer = cos_run_bearer(&env, &thread.id, "regression");
        Self {
            env,
            app,
            bearer,
            project,
        }
    }

    fn seeded(&self, title: &str) -> Task {
        let mut t = new_task(task_core::TaskKind::Execute, task_core::Status::Done);
        t.title = title.into();
        t.project_id = Some(self.project.id);
        self.env.seed(&t);
        t
    }

    fn task(&self, id: &str) -> Task {
        let id: TaskId = id.parse().expect("task id");
        self.env.store.get(id).expect("get").expect("task exists")
    }

    /// `task` の workspace に file を書き、`ArtifactProduced` を積む。sha256 を返す。
    fn produce(&self, task: &Task, run: &str, name: &str, bytes: &[u8]) -> String {
        let dir = task_ops::workspace::local_dir(
            &self.env.store,
            &self.env.view_context().workspace_roots,
            task,
        )
        .expect("workspace");
        let path = format!("artifacts/{name}");
        let file = dir.join(&path);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        std::fs::write(&file, bytes).expect("write");
        let sha256 = sha_hex(bytes);
        self.env
            .store
            .append_event(
                task.id,
                &Event::ArtifactProduced {
                    run_id: run.into(),
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

    async fn send(&self, key: &str, body: Value, strategy: Option<&Value>) -> (u16, Value) {
        let mut op = json!({
            "idempotency_key": key,
            "expected_revision": null,
            "reason": "人が依頼したバグ修正",
            "policy_version": "1",
            "request": {"method": "POST", "path": "/api/v1/tasks", "body": body},
        });
        if let Some(s) = strategy {
            op["strategy"] = s.clone();
        }
        let resp = post_op(&self.app, &self.bearer, &op).await;
        (resp.status.as_u16(), resp.json())
    }

    /// 承認済みの実験段階の版（基準の文書）と、その open の候補の実験 1 件を案件に置く。文書の参照と実験 task を返す。
    async fn with_experiment_phase_strategy(&self) -> (Task, Task) {
        let evidence = self.seeded("証拠の測定");
        let artifact_sha = self.produce(&evidence, "run-evidence", "struct-deps.tsv", b"scc\t55\n");
        let source = self.seeded("Strategy Review v1");
        let mut doc: Value = serde_json::from_str(BASELINE).expect("fixture");
        let evidence_task = json!({"kind": "task", "task_id": evidence.id.to_string()});
        doc["project_id"] = json!(self.project.id.to_string());
        doc["actor"]["id"] = json!(RUN);
        doc["source_revision"] = json!({"request_sha256": sha_hex(self.project.request.as_bytes()),
                                        "roadmap_version": null});
        doc["research_question"]["references"] = json!([evidence_task]);
        doc["evidence"][0]["references"] = json!([{"kind": "artifact", "task_id": evidence.id.to_string(),
                                                    "name": "struct-deps.tsv", "sha256": artifact_sha}]);
        doc["evidence"][1]["references"] =
            json!([evidence_task, {"kind": "url", "url": "https://arxiv.org/abs/2401.00001"}]);
        doc["evidence"][2]["source"] = json!({"kind": "celeris_task_run", "label": "測定の run"});
        doc["evidence"][2]["references"] = json!([evidence_task]);
        let id = doc["strategy_id"]
            .as_str()
            .expect("strategy_id")
            .to_string();
        let name = strategy::artifact_name(&id, 1);
        let bytes = serde_json::to_vec_pretty(&doc).expect("json");
        let sha256 = self.produce(&source, RUN, &name, &bytes);
        approve(&self.env, &source, &id, &sha256);
        let pointer = json!({"task_id": source.id.to_string(), "name": name, "sha256": sha256,
                             "candidate": "cut-min-edges"});
        let experiment = json!({
            "title": "識別実験: cut-min-edges",
            "objective": "上位 3 辺を切る pilot",
            "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
            "project_id": self.project.id.to_string(),
            "execution": "atomic",
        });
        let (status, resp) = self.send("experiment", experiment, Some(&pointer)).await;
        assert_eq!(status, 200, "{resp}");
        let experiment = self.task(resp["operation"]["target_id"].as_str().expect("target"));
        self.produce(&experiment, "run-experiment", "result.md", b"SCC 55\n");
        // 同じ案件で strategy を付けた compound は実験段階として拒否される（この案件の gate は効いている）。
        let mut plan = bug_fix_body(self);
        plan["execution"] = json!("compound");
        let (status, resp) = self.send("plan", plan, Some(&pointer)).await;
        assert_eq!(status, 422, "{resp}");
        assert_eq!(
            resp["codes"],
            json!(["strategy_experiment_phase"]),
            "{resp}"
        );
        (source, experiment)
    }

    /// `idempotency_key` の操作の `cos_operations` の行（全列）。
    fn operation_row(&self, key: &str) -> Value {
        let conn = rusqlite::Connection::open(&self.env.db_path).expect("db");
        let mut stmt = conn
            .prepare("SELECT * FROM cos_operations WHERE idempotency_key = ?1")
            .expect("prepare");
        let names: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
        stmt.query_row([key], |row| {
            let mut out = Map::new();
            for (i, n) in names.iter().enumerate() {
                let v = match row.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(x) => json!(x),
                    rusqlite::types::ValueRef::Real(x) => json!(x),
                    rusqlite::types::ValueRef::Text(t) => {
                        json!(String::from_utf8_lossy(t).into_owned())
                    }
                    rusqlite::types::ValueRef::Blob(_) => json!("<blob>"),
                };
                out.insert(n.clone(), v);
            }
            Ok(Value::Object(out))
        })
        .expect("row")
    }

    /// 操作 `operation_id` の監査の `cos_operation` の event（古い順）。
    fn audit(&self, operation_id: &str) -> Vec<Value> {
        let conn = rusqlite::Connection::open(&self.env.db_path).expect("db");
        let mut stmt = conn
            .prepare(
                "SELECT task_id, json FROM events WHERE json_extract(json, '$.type') = 'cos_operation' \
                 AND json_extract(json, '$.operation_id') = ?1 ORDER BY seq",
            )
            .expect("prepare");
        stmt.query_map([operation_id], |r| {
            let task: String = r.get(0)?;
            let raw: String = r.get(1)?;
            Ok(json!({"task_id": task, "event": serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null)}))
        })
        .expect("query")
        .map(|r| r.expect("row"))
        .collect()
    }
}

/// 人（`human:`）が文書の task の決定 `strategy-approve-v1` に approve と答えた（子 2 D2.2）。
fn approve(env: &TestEnv, source: &Task, strategy_id: &str, sha256: &str) {
    let id = format!("d-{}", TaskId::new());
    let opt = |key: &str| DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    };
    let request = DecisionRequest {
        id: id.clone(),
        key: strategy::approval_decision_key(1),
        kind: DecisionKind::Choice,
        question: strategy::approval_question(strategy_id, 1, sha256),
        options: vec![opt(strategy::APPROVE_OPTION), opt(strategy::REJECT_OPTION)],
        recommended: strategy::APPROVE_OPTION.into(),
        cost_of_reversal: CostOfReversal::Medium,
        cost_note: None,
        needed_before: vec!["self".into()],
        path: vec![DecisionPathEntry {
            task_id: source.id,
            title: source.title.clone(),
            stage: None,
            unit: None,
        }],
        raised_by: DecisionRaisedBy {
            task_id: source.id,
            run_id: None,
            origin: DecisionOrigin::Worker,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    };
    env.store
        .append_event(
            source.id,
            &Event::DecisionRequested {
                decision: Box::new(request),
            },
        )
        .expect("decision");
    env.store
        .append_event(
            source.id,
            &Event::DecisionAnswered {
                id,
                option: strategy::APPROVE_OPTION.into(),
                note: None,
                by: "human:regression".into(),
            },
        )
        .expect("answer");
}

/// 明確なバグ修正（`execution` を省く。実験段階の規則が掛かれば拒否される形）。
fn bug_fix_body(w: &World) -> Value {
    json!({
        "title": "バグ修正: chat の添付の上限の off-by-one",
        "objective": "上限ちょうどの添付が 413 になる不具合を直す",
        "acceptance": [{"type": "artifact_exists", "name": "fix-report.md"}],
        "project_id": w.project.id.to_string(),
    })
}

/// Crockford base32 の 26 文字の連なり（ULID）を `<id>` に置き換える。
fn mask_ids(s: &str) -> String {
    let ulid = |c: char| c.is_ascii_digit() || (c.is_ascii_uppercase() && !"ILOU".contains(c));
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let run = chars[i..].iter().take_while(|c| ulid(**c)).count();
        if run == 26 {
            out.push_str("<id>");
            i += run;
        } else if run > 0 {
            out.extend(&chars[i..i + run]);
            i += run;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// 環境ごとに変わる値（id・時刻・event と行の番号）を伏せる。
fn normalize(v: &Value) -> Value {
    match v {
        Value::String(s) if OffsetDateTime::parse(s, &Rfc3339).is_ok() => json!("<ts>"),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(inner @ (Value::Object(_) | Value::Array(_))) => normalize(&inner),
            _ => json!(mask_ids(s)),
        },
        Value::Array(a) => Value::Array(a.iter().map(normalize).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    let v = match k.as_str() {
                        // event の番号は DB ごとの連番（応答では文字列、行では整数）。
                        "event_id" | "seq" if !v.is_null() => json!("<n>"),
                        _ => normalize(v),
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// strategy の無いバグ修正の `task.create` は、案件に承認済みの実験段階の版と起票済みの実験があっても `execution` を省いたまま通り、
/// 応答・`cos_operations` の行・監査の event は戦略の無い案件と同じ。
#[tokio::test]
async fn strategy_review_regression_bug_fix_without_strategy_unchanged_by_experiment_phase() {
    let with = World::new();
    with.with_experiment_phase_strategy().await;
    let without = World::new();
    let (s1, r1) = with.send("bug-fix", bug_fix_body(&with), None).await;
    let (s2, r2) = without.send("bug-fix", bug_fix_body(&without), None).await;
    assert_eq!((s1, s2), (200, 200), "{r1}\n{r2}");
    assert!(r1["operation"]["payload"].get("strategy").is_none(), "{r1}");
    assert_eq!(normalize(&r1), normalize(&r2), "response\n{r1}\n{r2}");
    let (row1, row2) = (
        with.operation_row("bug-fix"),
        without.operation_row("bug-fix"),
    );
    assert_eq!(row1["state"], "applied");
    assert_eq!(row1["request_hash"], row2["request_hash"]);
    assert_eq!(normalize(&row1), normalize(&row2), "row\n{row1}\n{row2}");
    let (a1, a2) = (
        with.audit(row1["id"].as_str().expect("id")),
        without.audit(row2["id"].as_str().expect("id")),
    );
    assert!(!a1.is_empty(), "the operation left no audit event");
    assert_eq!(normalize(&json!(a1)), normalize(&json!(a2)), "audit");
    let created = with.task(r1["operation"]["target_id"].as_str().expect("target"));
    assert_eq!(created.title, "バグ修正: chat の添付の上限の off-by-one");
}

/// Critic の対象は戦略の文書を作った task だけ。strategy の無いバグ修正の task と、strategy を参照して作った実験 task（成果物は
/// `result.md` だけ）には対象が無く、保存された受け入れ条件も要求のまま（Critic の条件が足されない）。
#[tokio::test]
async fn strategy_review_regression_no_critic_subject_without_strategy_artifact() {
    let w = World::new();
    let (source, experiment) = w.with_experiment_phase_strategy().await;
    let (status, resp) = w.send("bug-fix", bug_fix_body(&w), None).await;
    assert_eq!(status, 200, "{resp}");
    let bug_fix = w.task(resp["operation"]["target_id"].as_str().expect("target"));
    w.produce(&bug_fix, "run-bug-fix", "result.md", b"fixed\n");
    let subjects = |t: &Task| critic_subjects(&w.env.store.events_for(t.id).expect("events"));
    let own = subjects(&source);
    assert_eq!(
        own.len(),
        1,
        "the strategy task is a Critic subject: {own:?}"
    );
    assert!(own[0].name.starts_with("strategy/"));
    for t in [&bug_fix, &experiment] {
        assert!(subjects(t).is_empty(), "{}: {:?}", t.title, subjects(t));
    }
    let sent = bug_fix_body(&w)["acceptance"].clone();
    assert_eq!(bug_fix.acceptance.len(), 1);
    assert_eq!(
        serde_json::to_value(&bug_fix.acceptance).expect("acceptance"),
        json!([{"text": bug_fix.acceptance[0].text, "check": sent[0]}]),
    );
}
