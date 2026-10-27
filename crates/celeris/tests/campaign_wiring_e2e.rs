//! ADR 2026-10-16-campaign-identity-propagation 通し試験: 通常の `POST /tasks`（HTTP）で作った campaign の
//! 子・孫・再試行・planner/reviewer・CoS の実 turn・Jev の複数呼び出し・人の介入を、dispatcher と同じ投影経路
//! （`record_task_decision_log`）で決定ログへ書き、`export --verify-connected` 相当（`verify_connected`）で確かめる。
//!
//! 識別は HTTP でだけ入れる（`task_campaigns` への直挿入なし）。子 task は dispatcher が作るので store に挿入し、
//! run の events は dispatcher が追記するものと同じ形で積む。temp DB・loopback・mock のみ（実 LLM・本番 DB なし）。

use std::net::SocketAddr;
use std::time::Duration;

use celeris::Config;
use serde_json::{Value, json};
use task_core::chat::{ChatOpenThreadRequest, ChatPostMessageRequest, ChatRunState, ChatSendMode};
use task_core::decision_log::{
    CampaignVerdict, CandidateRef, DecisionLogOutcome, DecisionLogProposal, DecisionLogUsage,
    InterventionKind, UsageRole,
};
use task_core::decision_log_export::{ExportParams, verify_connected};
use task_core::{
    DaemonMode, Event, InstanceRole, RunEnd, RunMetrics, RunRole, SharedRole, SqliteStore, TaskId,
    TaskStore, Usage,
};
use time::OffsetDateTime;

const TOKEN: &str = "campaign-wiring-token";
const CAMPAIGN: &str = "wiring-campaign";

struct Daemon {
    _dir: tempfile::TempDir,
    store: SqliteStore,
    base: String,
    _daemon_tx: tokio::sync::watch::Sender<Option<task_ops::daemon::DaemonSnapshot>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<Result<(), task_api::ApiError>>>,
}

impl Daemon {
    async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        std::fs::create_dir_all(home.join("ws")).expect("ws");
        std::fs::write(home.join("api.token"), format!("{TOKEN}\n")).expect("token");
        std::fs::write(
            home.join("config.toml"),
            r#"
db = "celeris.sqlite3"
workspace_root = "ws"
tick_ms = 200

[api]
listen = "127.0.0.1:0"
token_file = "api.token"

[selfdeploy]
releases_dir = "releases"
repo = "repo"
detach = "inline"

[llm_sources.local]
kind = "fake"
accounts = ["a1"]
models = ["fake"]

[harnesses.fake]
kind = "fake"
api_formats = ["fake"]
source_kinds = ["fake"]

[route_defaults.fake]
candidates = [{ source = "local", model = "fake" }]

[harness_selection]
execution_class_defaults = {}
rules = [{ harness = "fake", required_capabilities = [] }]
"#,
        )
        .expect("config");
        let config = Config::load(&home.join("config.toml")).expect("load");
        let store = SqliteStore::open(&config.db.path).expect("open");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr: SocketAddr = listener.local_addr().expect("addr");
        let settings = celeris::api_settings(
            &config,
            addr,
            vec![task_api::ApiToken::new(
                task_api::Principal::Human {
                    name: "test".into(),
                },
                TOKEN,
            )],
            "01J00000000000000000000000".to_string(),
            "2026-10-16T00:00:00Z".to_string(),
            None,
            "dev".to_string(),
            DaemonMode::Normal,
            SharedRole::new(InstanceRole::Active),
            None,
        );
        let (daemon_tx, daemon_rx) = tokio::sync::watch::channel(None);
        let state = task_api::ApiState::new(settings, daemon_rx).expect("state");
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(task_api::serve_with_listener(listener, state, async move {
            let _ = stopped.await;
        }));
        Self {
            _dir: dir,
            store,
            base: format!("http://{addr}"),
            _daemon_tx: daemon_tx,
            stop: Some(stop),
            handle: Some(handle),
        }
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let resp = reqwest::Client::new()
            .post(format!("{}{path}", self.base))
            .header("authorization", format!("Bearer {TOKEN}"))
            .json(&body)
            .send()
            .await
            .expect("post");
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            tokio::time::timeout(Duration::from_secs(60), handle)
                .await
                .expect("server stopped")
                .expect("server task")
                .expect("server result");
        }
    }
}

fn body(arm: &str, rep: u32) -> Value {
    json!({
        "title": format!("trial {arm} {rep}"), "objective": "o",
        "acceptance": [{"type": "artifact_exists", "name": "result.md"}],
        "campaign": {"campaign_id": CAMPAIGN, "case_id": "case-a", "arm": arm, "repetition": rep,
                     "manifest_hash": "mh-1", "routing_setting": "fixed"},
        "idempotency_key": format!("campaign-{}-{rep}", arm.to_lowercase()),
    })
}

async fn child_of(d: &Daemon, parent: TaskId, title: &str) -> TaskId {
    let (s, v) = d
        .post(
            "/api/v1/tasks",
            json!({"title": title, "objective": "child", "parent": parent.to_string(),
                   "acceptance": [{"type": "artifact_exists", "name": "r"}]}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    v["id"].as_str().expect("id").parse().expect("task id")
}

fn started(run: &str, role: Option<RunRole>) -> Event {
    Event::WorkerStarted {
        run_id: run.into(),
        adapter: "claude-code".into(),
        model: "m-exec".into(),
        provider: Some("src".into()),
        account: None,
        role,
        task_role: None,
    }
}

fn finished(run: &str, role: Option<RunRole>, end: RunEnd, tokens: u64) -> Event {
    Event::WorkerFinished {
        run_id: run.into(),
        outcome: "x".into(),
        usage: Some(Usage {
            input_tokens: Some(tokens),
            output_tokens: Some(1),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            cost_usd: Some(0.5),
            duplicate_reads: None,
            session_resumed: None,
            context_tokens: None,
        }),
        role,
        metrics: Some(RunMetrics {
            wall_ms: 100,
            retries: 0,
            peak_context_tokens: None,
            turns: None,
        }),
        end: Some(end),
    }
}

/// 試験用の routing 提案と grader 判定（routing 側・採点側の writer が書く値。識別は書かない）。
fn router_and_grader(store: &SqliteStore, run: &str, task: TaskId) {
    store
        .decision_log_record_proposal(
            &DecisionLogProposal {
                run_id: run.into(),
                task_id: Some(task.to_string()),
                proposed_first: Some(CandidateRef {
                    source_id: "src".into(),
                    model_id: "m-exec".into(),
                    harness_id: "claude-code".into(),
                }),
                ..Default::default()
            },
            OffsetDateTime::now_utc(),
        )
        .expect("proposal");
    store
        .decision_log_record_outcome(
            &DecisionLogOutcome {
                run_id: run.into(),
                campaign_verdict: Some(CampaignVerdict::Pass),
                ..Default::default()
            },
            OffsetDateTime::now_utc(),
        )
        .expect("grader");
}

fn project_all(store: &SqliteStore, tasks: &[TaskId]) {
    let now = OffsetDateTime::now_utc() + time::Duration::seconds(5);
    for t in tasks {
        task_dispatch::record_task_decision_log(store, *t, now).expect("project");
    }
}

#[tokio::test]
async fn post_tasks_to_export_connects_children_retries_review_cos_jev_and_interventions() {
    let d = Daemon::start().await;
    // 1. 通常の POST /tasks。冪等な再送は同じ task、別の試行（arm）は別 task。
    let (s, a) = d.post("/api/v1/tasks", body("B", 1)).await;
    assert_eq!(s, 201, "{a}");
    let root: TaskId = a["id"].as_str().expect("id").parse().expect("task id");
    let (s, again) = d.post("/api/v1/tasks", body("B", 1)).await;
    assert_eq!(s, 200, "{again}");
    assert_eq!(again["id"], a["id"]);
    let (s, other) = d.post("/api/v1/tasks", body("C", 1)).await;
    assert_eq!(s, 201, "{other}");
    let other_root: TaskId = other["id"].as_str().expect("id").parse().expect("task id");
    assert_ne!(root, other_root);

    // 2. 子・孫（dispatcher が作る）。自分の識別行は持たず、祖先から継ぐ。
    let child = child_of(&d, root, "child").await;
    let grand = child_of(&d, child, "grandchild").await;
    assert!(
        d.store
            .task_campaign_own(&grand.to_string())
            .expect("own")
            .is_none()
    );

    // 3. run の events: root は planner + 失敗再試行 + 成功、子は worker、孫は reviewer。
    let ev = |t: TaskId, e: Event| {
        d.store.append_event(t, &e).expect("event");
    };
    ev(root, started("r-plan", Some(RunRole::Planner)));
    ev(
        root,
        finished("r-plan", Some(RunRole::Planner), RunEnd::Completed, 50),
    );
    ev(root, started("r-1", None));
    ev(
        root,
        finished("r-1", None, RunEnd::Failed { retryable: true }, 100),
    );
    ev(root, started("r-2", None));
    ev(root, finished("r-2", None, RunEnd::Completed, 200));
    ev(child, started("r-child", None));
    ev(child, finished("r-child", None, RunEnd::Completed, 300));
    ev(grand, started("r-review", Some(RunRole::Reviewer)));
    ev(
        grand,
        finished("r-review", Some(RunRole::Reviewer), RunEnd::Completed, 20),
    );
    // 他 arm の trial（混ざらないことの確認用）。
    ev(other_root, started("r-other", None));
    ev(
        other_root,
        finished("r-other", None, RunEnd::Completed, 999),
    );
    // 人の介入: 子への回答（研究判断）。
    ev(
        child,
        Event::Answered {
            question: "q".into(),
            answer: "a".into(),
        },
    );
    for (run, t) in [
        ("r-plan", root),
        ("r-1", root),
        ("r-2", root),
        ("r-child", child),
        ("r-review", grand),
        ("r-other", other_root),
    ] {
        router_and_grader(&d.store, run, t);
    }

    // 4. CoS の実 turn（観測できた欄だけ。費用は取れていない）と Jev の 3 呼び出し（失敗・成功・再分類）。
    let now = OffsetDateTime::now_utc();
    let thread = d
        .store
        .cos_thread_open(&ChatOpenThreadRequest { project_id: None }, now)
        .expect("thread")
        .thread
        .id;
    d.store
        .chat_message_post(
            &thread,
            &ChatPostMessageRequest {
                client_message_id: "m1".into(),
                text: "hi".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            now,
        )
        .expect("post");
    d.store
        .chat_run_claim_next(
            &thread,
            "cos-run-1",
            &json!({"harness": "claude-code"}),
            now,
        )
        .expect("claim")
        .expect("claimed");
    d.store
        .chat_run_finish_with_telemetry(
            "cos-run-1",
            ChatRunState::Completed,
            Some("ok"),
            None,
            now + time::Duration::milliseconds(1500),
            Some(&Usage {
                input_tokens: Some(1000),
                output_tokens: Some(40),
                ..Default::default()
            }),
            0,
            None,
        )
        .expect("finish cos");
    let cid = child.to_string();
    for (n, cost) in [(1, None), (2, Some(0.01)), (3, Some(0.02))] {
        let mut jev = DecisionLogUsage::jev_call(&cid, n, &cid, Some(CAMPAIGN), cost);
        jev.is_retry = n > 1;
        d.store
            .decision_log_record_usage(&jev, now)
            .expect("jev usage");
    }

    // 5. 投影（dispatcher と同じ経路）。3 回繰り返しても行・消費・介入は増えない。
    let tasks = [root, child, grand, other_root];
    project_all(&d.store, &tasks);
    let snapshot = |s: &SqliteStore| {
        (
            s.decision_log_runs_list(Some(CAMPAIGN))
                .expect("runs")
                .len(),
            s.decision_log_usage_list(Some(CAMPAIGN)).expect("usage"),
            s.decision_log_interventions_list(Some(CAMPAIGN))
                .expect("iv"),
            s.decision_log_campaign_metering(CAMPAIGN)
                .expect("metering"),
        )
    };
    let first = snapshot(&d.store);
    project_all(&d.store, &tasks);
    project_all(&d.store, &tasks);
    let (runs, usage, ivs, metering) = snapshot(&d.store);
    assert_eq!(runs, first.0);
    assert_eq!(usage, first.1);
    assert_eq!(ivs, first.2);
    assert_eq!(metering.direct, first.3.direct);
    assert_eq!(metering.shared_overhead, first.3.shared_overhead);

    // 全 run（planner・再試行・reviewer・別 arm を含む）が同じ campaign に結合している。
    assert_eq!(runs, 6);
    let all = d
        .store
        .decision_log_runs_list(Some(CAMPAIGN))
        .expect("runs");
    for r in &all {
        assert_eq!(
            r.proposal.case_id.as_deref(),
            Some("case-a"),
            "{}",
            r.proposal.run_id
        );
        assert_eq!(r.proposal.manifest_hash.as_deref(), Some("mh-1"));
    }
    let arm = |run: &str| {
        all.iter()
            .find(|r| r.proposal.run_id == run)
            .and_then(|r| r.proposal.arm.clone())
    };
    assert_eq!(arm("r-review").as_deref(), Some("B"));
    assert_eq!(arm("r-other").as_deref(), Some("C"));
    assert!(
        all.iter()
            .find(|r| r.proposal.run_id == "r-2")
            .expect("r-2")
            .is_retry()
    );

    // worker run の消費は runs[].usage と同じ（Jev・CoS は別 subject で足さない）。
    let run_rows: Vec<_> = usage
        .iter()
        .filter(|u| u.run_id.is_some() && u.role != UsageRole::Cos)
        .collect();
    assert_eq!(run_rows.len(), 6);
    assert_eq!(
        run_rows.iter().filter_map(|u| u.input_tokens).sum::<u64>(),
        50 + 100 + 200 + 300 + 20 + 999
    );
    // Jev は 3 呼び出しが全部残り、取れなかった費用は欠測（0 ではない）。
    let jev: Vec<_> = usage.iter().filter(|u| u.role == UsageRole::Jev).collect();
    assert_eq!(jev.len(), 3);
    assert_eq!(jev.iter().filter(|u| u.list_cost_usd.is_none()).count(), 1);
    assert!(jev.iter().all(|u| u.cash_cost_usd.is_none()));
    // CoS の実 turn は共有オーバーヘッドとして 1 行。費用は欠測のまま、token は観測値。
    let cos: Vec<_> = usage.iter().filter(|u| u.role == UsageRole::Cos).collect();
    assert_eq!(cos.len(), 1, "{cos:?}");
    assert!(cos[0].shared_overhead);
    assert_eq!(cos[0].campaign_id.as_deref(), Some(CAMPAIGN));
    assert_eq!(cos[0].input_tokens, Some(1000));
    assert!(cos[0].list_cost_usd.is_none() && cos[0].cash_cost_usd.is_none());
    assert_eq!(metering.shared_overhead.n_rows, 1);
    assert_eq!(metering.shared_overhead.input_tokens.sum, Some(1000.0));
    // 人の介入は 1 件（回答）。所要時間は取れないので欠測。
    assert_eq!(ivs.len(), 1);
    assert_eq!(ivs[0].kind, InterventionKind::ResearchJudgment);
    assert_eq!(ivs[0].wall_ms, None);

    // 6. export --verify-connected 相当。
    let export = d
        .store
        .decision_log_export_campaign(&ExportParams {
            campaign_id: CAMPAIGN.into(),
            manifest_hash: Some("mh-1".into()),
            export_cursor: None,
        })
        .expect("export");
    let problems = verify_connected(&export);
    assert!(problems.is_empty(), "{problems:?}");

    // campaign の活動窓の外の CoS turn は帰属しない（campaign には入らず、帰属なしの共有行になる）。
    let later = now + time::Duration::hours(3);
    d.store
        .chat_message_post(
            &thread,
            &ChatPostMessageRequest {
                client_message_id: "m2".into(),
                text: "later".into(),
                attachment_ids: vec![],
                reply_to_id: None,
                mode: ChatSendMode::Queue,
                resume_queue: false,
            },
            later,
        )
        .expect("post 2");
    d.store
        .chat_run_claim_next(
            &thread,
            "cos-run-2",
            &json!({"harness": "claude-code"}),
            later,
        )
        .expect("claim 2")
        .expect("claimed 2");
    d.store
        .chat_run_finish_with_telemetry(
            "cos-run-2",
            ChatRunState::Completed,
            Some("ok"),
            None,
            later + time::Duration::seconds(1),
            Some(&Usage {
                input_tokens: Some(7),
                ..Default::default()
            }),
            0,
            None,
        )
        .expect("finish cos 2");
    project_all(&d.store, &tasks);
    assert_eq!(snapshot(&d.store).1, usage);
    let all_cos: Vec<_> = d
        .store
        .decision_log_usage_list(None)
        .expect("all usage")
        .into_iter()
        .filter(|u| u.role == UsageRole::Cos)
        .collect();
    assert_eq!(all_cos.len(), 2);
    assert!(
        all_cos
            .iter()
            .any(|u| u.campaign_id.is_none() && u.shared_overhead)
    );

    // 識別の無い task は export に混ざらない。
    let (s, plain) = d
        .post(
            "/api/v1/tasks",
            json!({"title": "p", "objective": "o",
                   "acceptance": [{"type": "artifact_exists", "name": "r"}]}),
        )
        .await;
    assert_eq!(s, 201, "{plain}");
    assert_eq!(
        d.store
            .task_campaign_resolve(plain["id"].as_str().expect("id"))
            .expect("resolve"),
        None
    );
    d.shutdown().await;
}
