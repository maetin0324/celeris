//! ADR 2026-11-21-strategy-evidence-contract D4・D6: `POST /cos/operations` の `task.create` に `strategy` を付けた
//! 要求の試験。守る不変条件: 人（`human:<名前>`）が承認した、最新の、同じ案件の、出典のそろった文書だけが task を
//! 作る。それ以外は 422 と規則の code で、task も成果物の行も作らない。`strategy` の無い要求は今と同じ。
//! task-core の `strategy_contract_` の試験は純関数だけを見るので、store から文脈を集める経路と、実行の入口で
//! task が作られないことは検出できない（この試験が見る）。

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use task_core::chat::{ChatOpenThreadRequest, ChatPostMessageRequest, ChatSendMode};
use task_core::decision::{
    CostOfReversal, DecisionKind, DecisionOption, DecisionOrigin, DecisionPathEntry,
    DecisionRaisedBy, DecisionRequest, DecisionStatus,
};
use task_core::model::{ArtifactRef, Event, Task, TaskId};
use task_core::project_proposal::sha256_hex;
use task_core::store::SqliteStore;
use task_core::strategy;
use time::OffsetDateTime;
use tower::ServiceExt;

const BASELINE: &str = include_str!("../../../../task-core/src/strategy/fixtures/baseline-v1.json");
const RUN: &str = "01M7ZRUN00000000000000STV1";
const OPS: &str = "/api/v1/cos/operations";

struct World {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    app: axum::Router,
    store: SqliteStore,
    project: task_core::Project,
    other: task_core::Project,
    source: Task,
    evidence: Task,
    /// 案件の thread の message（CoS の run の入力。chat の参照にも使う）。
    thread: String,
    message: String,
    other_thread: String,
    other_message: String,
    bearer: String,
}

fn project(title: &str, request: &str) -> task_core::Project {
    let now = OffsetDateTime::now_utc();
    task_core::Project {
        id: task_core::ProjectId::new(),
        title: title.into(),
        request: request.into(),
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

fn task(project: &task_core::Project) -> Task {
    let id = TaskId::new();
    let now = OffsetDateTime::now_utc();
    Task {
        harness_id: None,
        requirements: Default::default(),
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id,
        parent_id: None,
        kind: task_core::TaskKind::Execute,
        title: "戦略 review".into(),
        objective: "戦略の文書を作る".into(),
        acceptance: vec![task_core::Criterion {
            text: "文書がある".into(),
            check: task_core::Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: task_core::Status::Done,
        priority: 0,
        worker_hint: task_core::WorkerHint {
            tier: task_core::Tier::Standard,
        },
        workspace: task_core::WorkspaceSpec::Local {
            path: PathBuf::from(id.to_string()),
            mode: None,
        },
        budget: task_core::Budget {
            max_turns: 10,
            max_wall_secs: 600,
            max_retries: 2,
            max_tokens: None,
        },
        attempts: 0,
        lease: None,
        created_at: now,
        updated_at: now,
        role: None,
        genre: None,
        aggregate: false,
        project_id: Some(project.id),
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

fn post_message(store: &SqliteStore, thread: &str, key: &str) -> String {
    store
        .chat_message_post(
            thread,
            &ChatPostMessageRequest {
                client_message_id: format!("message-{key}"),
                text: "最小の辺の集合を先に測りたい".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            OffsetDateTime::now_utc(),
        )
        .expect("message")
        .response
        .message
        .id
}

fn open_thread(store: &SqliteStore, project: &task_core::Project) -> String {
    store
        .cos_thread_open(
            &ChatOpenThreadRequest {
                project_id: Some(project.id.to_string()),
            },
            OffsetDateTime::now_utc(),
        )
        .expect("thread")
        .thread
        .id
}

/// 静的トークン（人の主体）と mcp のトークン。要求は CoS の run の bearer で送る。
const HUMAN_TOKEN: &str = "strategy-gate-test-human";
const MCP_TOKEN: &str = "celeris-mcp.strategy-gate-test";

/// 単体試験の API の状態（`dir` の下に DB と `ws/`）。`handlers::tests::state` は handlers の試験の私的な部品なので、
/// ここでは同じ形の設定を自分で組む。
fn api_state(dir: &std::path::Path) -> crate::ApiState {
    let settings = crate::ApiSettings {
        browser: Default::default(),
        documentation_state_dir: None,
        listen: "127.0.0.1:7710".parse().unwrap_or_else(|e| panic!("{e}")),
        // 静的トークンを 1 つ持たせ、認証を要る状態にする（CoS の run の bearer が CoS の主体として読まれる）。
        tokens: vec![crate::ApiToken::new(
            crate::Principal::Human {
                name: "test".into(),
            },
            HUMAN_TOKEN,
        )],
        mcp_token: Some(MCP_TOKEN.into()),
        allowed_hosts: vec![],
        db_path: dir.join("celeris.db"),
        busy_timeout: std::time::Duration::from_millis(5000),
        background_checkpoint: false,
        view: task_ops::view::ViewContext {
            workspace_roots: task_core::local_machine::WorkspaceRoots::new(dir.join("ws"), None),
            retry_backoff_base: std::time::Duration::from_secs(0),
            retry_backoff_max: std::time::Duration::from_secs(0),
            max_requeues: 5,
            clusters: Default::default(),
        },
        config_view: crate::types::ConfigView {
            config_path: String::new(),
            db: String::new(),
            workspace_root: String::new(),
            tick_ms: 2000,
            max_concurrency: 1,
            lease_grace_secs: 0,
            idle_timeout_secs: 0,
            kill_grace_secs: 0,
            review_timeout_secs: 0,
            error_cooldown_secs: 0,
            retry_backoff_base_secs: 0,
            retry_backoff_max_secs: 0,
            max_requeues: 5,
            plan_auto_accept: false,
            reviewer: crate::types::ReviewerConfigView {
                adapter: None,
                tier: Some(task_core::Tier::Standard),
            },
            clusters: vec![],
            roles: vec![],
            genres: vec![],
            delegation: task_core::DelegationLimits::default(),
            api: crate::types::ApiConfigView {
                bind: "127.0.0.1:7710".into(),
                auth_required: false,
                allowed_hosts: vec![],
            },
            identity: crate::types::DaemonIdentityView {
                instance_id: "01J00000000000000000000000".into(),
                release: "dev".into(),
                pid: 1,
                started_at: "2026-09-14T00:00:00Z".into(),
                config_sha256: None,
                overrides: vec![],
                effective: crate::types::EffectiveConfigView {
                    db: String::new(),
                    workspace_root: String::new(),
                    knowledge_root: String::new(),
                    knowledge_default_mounts: vec![],
                    memory_dir: None,
                    langmem_enabled: false,
                    knowledge_gc_enabled: false,
                    cron_seed_count: 0,
                    selfdeploy_releases_dir: String::new(),
                    selfdeploy_repo: String::new(),
                    selfdeploy_push: false,
                    api_listen: "127.0.0.1:7710".into(),
                    llm_proxy_listen: None,
                },
            },
        },
        roles: vec![],
        genres: vec![],
        conversation_genre: task_core::CONVERSATION_GENRE.to_string(),
        celeris_version: "test".into(),
        instance_id: "01J00000000000000000000000".into(),
        started_at: "2026-09-14T00:00:00Z".into(),
        admin_tx: None,
        accounts_roots: std::collections::HashMap::new(),
        max_runs_per_account: 0,
        secrets_dir: None,
        secret_usage: std::collections::HashMap::new(),
        memory_dir: None,
        notify_secret_id: task_core::DEFAULT_WEBHOOK_SECRET_ID.to_string(),
        notify_web_base_url: None,
        releases: None,
        integration_releases_dir: None,
        release: "dev".to_string(),
        cos_policy_version: None,
        mode: task_core::DaemonMode::Normal,
        role: task_core::SharedRole::new(task_core::InstanceRole::Active),
        github: crate::GithubSettings::default(),
        knowledge_root: None,
        docs_repo_root: Some(dir.join("workspace")),
        llm_sources: None,
        harnesses: None,
        model_discovery: None,
        tree_limits: task_core::TreeLimits::default(),
        budget_rates: task_core::pricing::BudgetRates::default(),
        local_machine: crate::LocalMachineSettings::default(),
    };
    crate::ApiState::new(settings, tokio::sync::watch::channel(None).1)
        .unwrap_or_else(|e| panic!("{e}"))
}

fn world() -> World {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let state = api_state(&root);
    let app = crate::router(state);
    let store = SqliteStore::open(&root.join("celeris.db")).expect("store");
    let project = project("agent-platform", "人の依頼 v1: 戦略層を作る");
    let other = project_other();
    store.project_create(&project).expect("project");
    store.project_create(&other).expect("other project");
    let (source, evidence) = (task(&project), task(&project));
    for t in [&source, &evidence] {
        store.insert(t).expect("task");
        std::fs::create_dir_all(root.join("ws").join(t.id.to_string())).expect("ws");
    }
    let thread = open_thread(&store, &project);
    let message = post_message(&store, &thread, "cos");
    let now = OffsetDateTime::now_utc();
    store
        .chat_run_claim_next(&thread, "run-cos", &json!({}), now)
        .expect("claim")
        .expect("run");
    let bearer = crate::cos::issue_run_bearer(&store, &thread, "run-cos", time::Duration::hours(1))
        .expect("bearer");
    store
        .chat_run_record_policy("run-cos", "1", "test")
        .expect("policy");
    let other_thread = open_thread(&store, &other);
    let other_message = post_message(&store, &other_thread, "other");
    let w = World {
        _tmp: tmp,
        root,
        app,
        store,
        project,
        other,
        source,
        evidence,
        thread,
        message,
        other_thread,
        other_message,
        bearer: format!("Bearer {bearer}"),
    };
    produce(
        &w,
        &w.evidence,
        "struct-deps.tsv",
        "artifacts/struct-deps.tsv",
        b"scc\t55\n",
    );
    w
}

fn project_other() -> task_core::Project {
    project("other", "別の案件の依頼")
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// `task` の workspace に file を書き、`ArtifactProduced` を積む。sha256 を返す。
fn produce(w: &World, task: &Task, name: &str, path: &str, bytes: &[u8]) -> String {
    let file = w.root.join("ws").join(task.id.to_string()).join(path);
    std::fs::create_dir_all(file.parent().expect("parent")).expect("dir");
    std::fs::write(&file, bytes).expect("write");
    let sha256 = sha(bytes);
    w.store
        .append_event(
            task.id,
            &Event::ArtifactProduced {
                run_id: RUN.into(),
                artifact: ArtifactRef {
                    name: name.into(),
                    path: path.into(),
                    sha256: sha256.clone(),
                    kind: "file".into(),
                    declared: true,
                },
            },
        )
        .expect("artifact");
    sha256
}

fn evidence_sha() -> String {
    sha(b"scc\t55\n")
}

/// 基準の状態 B の文書（承認前。案件・依頼の sha256・参照は World のもの）。
fn baseline(w: &World) -> Value {
    let mut doc: Value = serde_json::from_str(BASELINE).expect("fixture");
    doc["project_id"] = json!(w.project.id.to_string());
    doc["actor"]["id"] = json!(RUN);
    doc["source_revision"] =
        json!({"request_sha256": sha256_hex(&w.project.request), "roadmap_version": null});
    let evidence_task = json!({"kind": "task", "task_id": w.evidence.id.to_string()});
    doc["research_question"]["references"] = json!([evidence_task]);
    doc["evidence"][0]["references"] = json!([
        {"kind": "artifact", "task_id": w.evidence.id.to_string(), "name": "struct-deps.tsv", "sha256": evidence_sha()},
        {"kind": "chat", "thread_id": w.thread, "message_id": w.message},
    ]);
    doc["evidence"][1]["references"] =
        json!([evidence_task, {"kind": "url", "url": "https://arxiv.org/abs/2401.00001"}]);
    doc["evidence"][2]["source"]["thread_id"] = json!(w.thread);
    doc["evidence"][2]["references"] = json!([evidence_task]);
    doc
}

/// 文書を `source` の成果物 `strategy/<id>/v<n>.json` として置き、参照 `{task_id, name, sha256}` を返す。
fn publish(w: &World, doc: &Value) -> Value {
    let bytes = serde_json::to_vec_pretty(doc).expect("json");
    let id = doc["strategy_id"].as_str().expect("strategy_id");
    let version = u32::try_from(doc["version"].as_u64().expect("version")).expect("u32");
    let name = strategy::artifact_name(id, version);
    let sha256 = produce(w, &w.source, &name, &format!("artifacts/{name}"), &bytes);
    json!({"task_id": w.source.id.to_string(), "name": name, "sha256": sha256})
}

/// 承認の決定（D2.2）を出し、`answer` が `Some((option, by))` なら答える。
fn decide(w: &World, doc: &Value, strategy: &Value, answer: Option<(&str, &str)>) {
    let version = u32::try_from(doc["version"].as_u64().expect("version")).expect("u32");
    let id = format!("d-{}", TaskId::new());
    let opt = |key: &str| DecisionOption {
        key: key.into(),
        label: key.into(),
        consequence: None,
    };
    let request = DecisionRequest {
        id: id.clone(),
        key: strategy::approval_decision_key(version),
        kind: DecisionKind::Choice,
        question: strategy::approval_question(
            doc["strategy_id"].as_str().expect("strategy_id"),
            version,
            strategy["sha256"].as_str().expect("sha256"),
        ),
        options: vec![opt(strategy::APPROVE_OPTION), opt(strategy::REJECT_OPTION)],
        recommended: strategy::APPROVE_OPTION.into(),
        cost_of_reversal: CostOfReversal::Medium,
        cost_note: None,
        needed_before: vec!["self".into()],
        path: vec![DecisionPathEntry {
            task_id: w.source.id,
            title: "戦略 review".into(),
            stage: None,
            unit: None,
        }],
        raised_by: DecisionRaisedBy {
            task_id: w.source.id,
            run_id: Some(RUN.into()),
            origin: DecisionOrigin::Worker,
        },
        status: DecisionStatus::Open,
        answer: None,
        withdrawn_reason: None,
    };
    w.store
        .append_event(
            w.source.id,
            &Event::DecisionRequested {
                decision: Box::new(request),
            },
        )
        .expect("decision");
    if let Some((option, by)) = answer {
        w.store
            .append_event(
                w.source.id,
                &Event::DecisionAnswered {
                    id,
                    option: option.into(),
                    note: None,
                    by: by.into(),
                },
            )
            .expect("answer");
    }
}

fn task_body(w: &World, project: &task_core::Project) -> Value {
    let _ = w;
    json!({
        "title": "戦略の実行: c-cut-edges",
        "objective": "承認された戦略の候補を試す",
        "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
        "project_id": project.id.to_string(),
    })
}

fn op(key: &str, path: &str, body: Value, strategy: Option<&Value>) -> Value {
    let mut op = json!({
        "idempotency_key": key,
        "expected_revision": null,
        "reason": "承認された戦略を実行する",
        "policy_version": "1",
        "request": {"method": "POST", "path": path, "body": body},
    });
    if let Some(s) = strategy {
        op["strategy"] = s.clone();
    }
    op
}

async fn post(w: &World, body: &Value) -> (StatusCode, Value) {
    let request = Request::post(OPS)
        .header("host", "127.0.0.1:7710")
        .header("content-type", "application/json")
        .header("authorization", &w.bearer)
        .body(Body::from(body.to_string()))
        .expect("request");
    let resp = w
        .app
        .clone()
        .oneshot(request)
        .await
        .unwrap_or_else(|e| match e {});
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn created_tasks(w: &World) -> i64 {
    let conn = rusqlite::Connection::open(w.root.join("celeris.db")).expect("db");
    conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE title = '戦略の実行: c-cut-edges'",
        [],
        |row| row.get(0),
    )
    .expect("count")
}

/// `strategy` 付きの `task.create` を送り、`(status, problem の code, 全 code)` を返す。
async fn create(
    w: &World,
    key: &str,
    strategy: &Value,
) -> (StatusCode, String, Vec<String>, Value) {
    let (status, body) = post(
        w,
        &op(
            key,
            "/api/v1/tasks",
            task_body(w, &w.project),
            Some(strategy),
        ),
    )
    .await;
    let code = body["code"].as_str().unwrap_or_default().to_string();
    let codes = body["codes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (status, code, codes, body)
}

/// 422 と code、task が作られないこと。
async fn refused(w: &World, key: &str, strategy: &Value, want: &str) -> Value {
    let before = created_tasks(w);
    let (status, code, codes, body) = create(w, key, strategy).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(code, want, "{body}");
    assert!(codes.iter().any(|c| c == want), "{body}");
    assert_eq!(
        created_tasks(w),
        before,
        "a refused strategy created a task: {body}"
    );
    body
}

fn approved(w: &World, doc: &Value) -> Value {
    let s = publish(w, doc);
    decide(w, doc, &s, Some((strategy::APPROVE_OPTION, "human:rmaeda")));
    s
}

/// 人が承認した最新の文書は通り、task ができ、監査の payload に strategy が残る。同じ key の再送は同じ操作、
/// 別の strategy で同じ key は 409（idempotency は strategy を含む要求全体に掛かる）。
#[tokio::test]
async fn strategy_gate_human_approved_latest_document_creates_the_task() {
    let w = world();
    let s = approved(&w, &baseline(&w));
    let (status, _, _, body) = create(&w, "ok-1", &s).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["operation"]["action"], "task.create", "{body}");
    assert_eq!(body["operation"]["payload"]["strategy"], s, "{body}");
    assert_eq!(created_tasks(&w), 1);
    let (again, _, _, replay) = create(&w, "ok-1", &s).await;
    assert_eq!(again, StatusCode::OK, "{replay}");
    assert_eq!(replay["operation"]["id"], body["operation"]["id"]);
    assert_eq!(created_tasks(&w), 1);
    let mut changed = s.clone();
    changed["name"] = json!("strategy/arch-v4-cycle/v2.json");
    let (conflict, _, _, c) = create(&w, "ok-1", &changed).await;
    assert_eq!(conflict, StatusCode::CONFLICT, "{c}");
}

/// 未承認: discussion の文書（人が approve と答えても）、決定の無い proposed、答えの無い決定、人の却下。
#[tokio::test]
async fn strategy_gate_unapproved_discussion_and_proposed_are_422() {
    let w = world();
    let mut discussion = baseline(&w);
    discussion["decision_state"] = json!("discussion");
    discussion["strategy_id"] = json!("disc");
    discussion["idempotency_key"] = json!("strategy:disc:v1");
    let s = approved(&w, &discussion);
    refused(&w, "u-disc", &s, "strategy_unapproved").await;

    let s = publish(&w, &baseline(&w));
    refused(&w, "u-none", &s, "strategy_unapproved").await;
    decide(&w, &baseline(&w), &s, None);
    refused(&w, "u-open", &s, "strategy_unapproved").await;
    decide(
        &w,
        &baseline(&w),
        &s,
        Some((strategy::REJECT_OPTION, "human:rmaeda")),
    );
    refused(&w, "u-reject", &s, "strategy_unapproved").await;
}

/// CoS が自分で approved と書いた文書と、人でない主体（CoS・operator・mcp・接頭辞の無い human）の approve は 422。
#[tokio::test]
async fn strategy_gate_cos_self_approval_is_422() {
    let w = world();
    let mut written = baseline(&w);
    written["decision_state"] = json!("approved");
    let s = approved(&w, &written);
    let body = refused(&w, "c-written", &s, "strategy_invalid").await;
    assert!(
        body["rules"]
            .as_array()
            .is_some_and(|r| r.contains(&json!("state_not_writable"))),
        "{body}"
    );

    for (i, by) in [
        format!("cos:{}", w.thread),
        "operator:claude-code".into(),
        "mcp:chatgpt".into(),
        "human".into(),
    ]
    .iter()
    .enumerate()
    {
        let mut doc = baseline(&w);
        let id = format!("by-{i}");
        doc["strategy_id"] = json!(id);
        doc["idempotency_key"] = json!(format!("strategy:{id}:v1"));
        let s = publish(&w, &doc);
        decide(&w, &doc, &s, Some((strategy::APPROVE_OPTION, by)));
        refused(&w, &format!("c-by-{i}"), &s, "strategy_unapproved").await;
    }
}

/// stale: 文書の後に Project.request が変わった、Roadmap の版が前提と違う、より新しい版が人に承認された。
/// 人の発言の中継（instructed_by）でも同じく拒否する。
#[tokio::test]
async fn strategy_gate_stale_revision_is_422() {
    let w = world();
    let v1 = baseline(&w);
    let s1 = approved(&w, &v1);
    let mut roadmap = baseline(&w);
    roadmap["strategy_id"] = json!("roadmap");
    roadmap["idempotency_key"] = json!("strategy:roadmap:v1");
    roadmap["source_revision"]["roadmap_version"] = json!(3);
    let sr = approved(&w, &roadmap);
    refused(&w, "s-roadmap", &sr, "strategy_stale").await;

    let mut v2 = v1.clone();
    v2["version"] = json!(2);
    v2["idempotency_key"] = json!("strategy:arch-v4-cycle:v2");
    v2["supersedes"] = s1.clone();
    v2["change_reason"] = json!("反証を受けて候補を絞った");
    let s2 = approved(&w, &v2);
    refused(&w, "s-newer", &s1, "strategy_stale").await;
    let (status, _, _, body) = create(&w, "s-v2", &s2).await;
    assert_eq!(status, StatusCode::OK, "the approved v2 passes: {body}");

    let conn = rusqlite::Connection::open(w.root.join("celeris.db")).expect("db");
    conn.execute(
        "UPDATE projects SET request = ?1 WHERE id = ?2",
        rusqlite::params!["人の依頼 v2: 範囲を変えた", w.project.id.to_string()],
    )
    .expect("request");
    refused(&w, "s-request", &s2, "strategy_stale").await;
    let mut relayed = op(
        "s-relayed",
        "/api/v1/tasks",
        task_body(&w, &w.project),
        Some(&s2),
    );
    relayed["instructed_by"] = json!(w.message);
    let (status, body) = post(&w, &relayed).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "strategy_stale", "{body}");
}

/// 別の案件: 証拠が別の案件の thread の message を指す、作る task が別の案件、文書の project_id が別の案件。
#[tokio::test]
async fn strategy_gate_other_project_material_is_422() {
    let w = world();
    let mut doc = baseline(&w);
    doc["evidence"][0]["references"][1] =
        json!({"kind": "chat", "thread_id": w.other_thread, "message_id": w.other_message});
    let s = approved(&w, &doc);
    refused(&w, "p-chat", &s, "strategy_other_project").await;

    let s = approved(&w, &{
        let mut d = baseline(&w);
        d["strategy_id"] = json!("target");
        d["idempotency_key"] = json!("strategy:target:v1");
        d
    });
    let (status, body) = post(
        &w,
        &op(
            "p-target",
            "/api/v1/tasks",
            task_body(&w, &w.other),
            Some(&s),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "strategy_other_project", "{body}");

    let mut foreign = baseline(&w);
    foreign["project_id"] = json!(w.other.id.to_string());
    foreign["strategy_id"] = json!("foreign");
    foreign["idempotency_key"] = json!("strategy:foreign:v1");
    let s = approved(&w, &foreign);
    refused(&w, "p-doc", &s, "strategy_other_project").await;
}

/// 出典の欠損: 証拠の成果物が無い、参照した文書の成果物が無い、file が記録の後に上書きされた、証拠の参照が空。
#[tokio::test]
async fn strategy_gate_missing_source_is_422() {
    let w = world();
    let mut doc = baseline(&w);
    doc["evidence"][0]["references"][0]["name"] = json!("no-such.tsv");
    let s = approved(&w, &doc);
    refused(&w, "m-ref", &s, "strategy_source_missing").await;

    let mut ghost = approved(&w, &baseline(&w));
    ghost["name"] = json!("strategy/arch-v4-cycle/v9.json");
    refused(&w, "m-artifact", &ghost, "strategy_source_missing").await;

    let mut empty = baseline(&w);
    empty["strategy_id"] = json!("empty");
    empty["idempotency_key"] = json!("strategy:empty:v1");
    empty["evidence"][2]["references"] = json!([]);
    let s = approved(&w, &empty);
    refused(&w, "m-empty", &s, "strategy_source_missing").await;

    let mut over = baseline(&w);
    over["strategy_id"] = json!("over");
    over["idempotency_key"] = json!("strategy:over:v1");
    let s = approved(&w, &over);
    let file = w
        .root
        .join("ws")
        .join(w.source.id.to_string())
        .join("artifacts/strategy/over/v1.json");
    std::fs::write(&file, b"{}").expect("overwrite");
    refused(&w, "m-over", &s, "strategy_source_missing").await;
}

/// 欄の無い task.create は今と同じ: 通り、payload に strategy が無く、request_hash は従来の式のまま。
/// `strategy` を task.create 以外に付けた要求は 422 `strategy_not_applicable`。
#[tokio::test]
async fn strategy_gate_absent_field_keeps_task_create_unchanged() {
    let w = world();
    let body = task_body(&w, &w.project);
    let (status, resp) = post(&w, &op("plain", "/api/v1/tasks", body.clone(), None)).await;
    assert_eq!(status, StatusCode::OK, "{resp}");
    let operation = &resp["operation"];
    assert!(operation["payload"].get("strategy").is_none(), "{resp}");
    assert_eq!(
        operation["request_hash"],
        json!(crate::cos::operations::request_hash(
            "POST",
            "/api/v1/tasks",
            &body
        )),
    );
    assert_eq!(created_tasks(&w), 1);

    let s = approved(&w, &baseline(&w));
    let path = format!("/api/v1/tasks/{}/comments", w.source.id);
    let (status, resp) = post(&w, &op("comment", &path, json!({"body": "メモ"}), Some(&s))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{resp}");
    assert_eq!(resp["code"], super::STRATEGY_NOT_APPLICABLE, "{resp}");
}

/// ADR D6 の ablation（task.create の経路）: B は通り、N-x はその規則の code だけで 422 になる。
#[tokio::test]
async fn strategy_gate_ablation_four_rules() {
    let w = world();
    let named = |id: &str, mut d: Value| {
        d["strategy_id"] = json!(id);
        d["idempotency_key"] = json!(format!("strategy:{id}:v1"));
        d
    };
    let b = approved(&w, &named("b", baseline(&w)));
    let mut src = named("n-src", baseline(&w));
    src["evidence"][0]["references"][0]["name"] = json!("no-such.tsv");
    let src = approved(&w, &src);
    let mut stale = named("n-stale", baseline(&w));
    stale["source_revision"]["request_sha256"] = json!(sha256_hex("前の依頼"));
    let stale = approved(&w, &stale);
    let mut proj = named("n-proj", baseline(&w));
    proj["evidence"][0]["references"][1] =
        json!({"kind": "chat", "thread_id": w.other_thread, "message_id": w.other_message});
    let proj = approved(&w, &proj);
    let appr_doc = named("n-appr", baseline(&w));
    let appr = publish(&w, &appr_doc);
    decide(
        &w,
        &appr_doc,
        &appr,
        Some((strategy::APPROVE_OPTION, "operator:claude-code")),
    );

    let mut rows = Vec::new();
    for (label, s, want) in [
        ("R-src N-src", &src, "strategy_source_missing"),
        ("R-stale N-stale", &stale, "strategy_stale"),
        ("R-proj N-proj", &proj, "strategy_other_project"),
        ("R-appr N-appr", &appr, "strategy_unapproved"),
    ] {
        let (status, code, codes, body) = create(&w, label, s).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{label}: {body}");
        assert_eq!(
            codes,
            vec![want.to_string()],
            "{label}: only its rule refuses: {body}"
        );
        assert_eq!(code, want);
        rows.push(format!("| {label} | {} | {:?} |", status.as_u16(), codes));
    }
    let (status, _, codes, body) = create(&w, "B", &b).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    rows.push(format!(
        "| B（人の承認） | {} | {codes:?} |",
        status.as_u16()
    ));
    assert_eq!(created_tasks(&w), 1);
    println!(
        "| 状態 | status | codes |\n|---|---|---|\n{}",
        rows.join("\n")
    );
}
