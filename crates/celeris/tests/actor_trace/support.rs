//! Shared actor-trace fixture. Extend by adding helpers; keep existing semantics stable.
#![allow(dead_code)]
#[path = "../../../../tests/support/http_async.rs"]
mod fake_http;

use serde_json::{Value, json};
use std::{fs, path::PathBuf};
use task_core::chat::triage::CosTriageSource;
use task_core::chat::{AuditContext, ChatActor, ChatOpenThreadRequest, ChatRunState};
use task_core::decision::*;
use task_core::decision_log::{CampaignIdentity, DecisionLogProposal};
use task_core::decision_log_export::{CampaignExport, ExportParams};
use task_core::{Event, SqliteStore, Status, Task, TaskId, TaskKind, TaskStore};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::io::AsyncWriteExt;

pub const CAMPAIGN: &str = "actor-trace";
pub const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const DECISION: &str = "authorized-queue";
pub const RUN: &str = "actor-trace-cos-1";
pub const POLICY: &str = "4";
pub const REASON: &str = "既存の認可で許可された local queue を使う";

pub fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::parse("2026-10-28T10:00:00Z", &Rfc3339).unwrap() + Duration::seconds(seconds)
}
pub fn ts(seconds: i64) -> String {
    at(seconds).format(&Rfc3339).unwrap()
}

pub struct Harness {
    pub dir: tempfile::TempDir,
    pub db: PathBuf,
    pub store: SqliteStore,
    pub task: Task,
    pub thread: String,
}

impl Harness {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("trace.db");
        let store = SqliteStore::open(&db).unwrap();
        let project = task_core::Project {
            id: task_core::ProjectId::new(),
            title: "actor trace".into(),
            request: REASON.into(),
            status: task_core::ProjectStatus::Active,
            workspace: None,
            archived_at: None,
            paused_from: None,
            auto_advance: false,
            auto_integrate: false,
            slug: None,
            created_at: at(0),
            updated_at: at(0),
        };
        store.project_create(&project).unwrap();
        let thread = store
            .cos_thread_open(
                &ChatOpenThreadRequest {
                    project_id: Some(project.id.to_string()),
                },
                at(0),
            )
            .unwrap()
            .thread
            .id;
        let mut task = new_task();
        task.project_id = Some(project.id);
        store.create_task_by(&task, "daemon", vec![]).unwrap();
        let identity = CampaignIdentity {
            campaign_id: CAMPAIGN.into(),
            case_id: Some("flow1".into()),
            arm: Some("B".into()),
            repetition: Some(1),
            manifest_hash: Some(HASH.into()),
            routing_setting: None,
            variant: None,
            variant_org: None,
        };
        store
            .task_campaign_set(&task.id.to_string(), &identity, at(0))
            .unwrap();
        // The task's routing record supplies the trial identity to the report join; it carries no usage.
        let mut proposal = DecisionLogProposal {
            run_id: "task-routing-1".into(),
            task_id: Some(task.id.to_string()),
            project_id: Some(project.id.to_string()),
            ..Default::default()
        };
        proposal.apply_campaign(&identity);
        store
            .decision_log_record_proposal(&proposal, at(0))
            .unwrap();
        Self {
            dir,
            db,
            store,
            task,
            thread,
        }
    }
    pub fn append(&self, event: &Event, seconds: i64) {
        let mut conn = rusqlite::Connection::open(&self.db).unwrap();
        let tx = conn.transaction().unwrap();
        SqliteStore::append_event_tx_at(&tx, self.task.id, event, at(seconds)).unwrap();
        tx.commit().unwrap();
    }
    pub fn raise_and_claim(&self) -> (String, String) {
        let option = |key: &str| DecisionOption {
            key: key.into(),
            label: key.into(),
            consequence: None,
        };
        self.append(
            &Event::DecisionRequested {
                decision: Box::new(DecisionRequest {
                    id: DECISION.into(),
                    key: "queue".into(),
                    kind: DecisionKind::Choice,
                    question: "認可済みの queue を選ぶ".into(),
                    options: vec![option("local"), option("other")],
                    recommended: "local".into(),
                    cost_of_reversal: CostOfReversal::Low,
                    cost_note: None,
                    needed_before: vec![],
                    path: vec![],
                    raised_by: DecisionRaisedBy {
                        task_id: self.task.id,
                        run_id: None,
                        origin: DecisionOrigin::Planner,
                    },
                    status: DecisionStatus::Open,
                    answer: None,
                    withdrawn_reason: None,
                }),
            },
            1,
        );
        let view = task_ops::view::ViewContext {
            workspace_roots: task_core::local_machine::WorkspaceRoots::new(
                self.dir.path().into(),
                None,
            ),
            retry_backoff_base: std::time::Duration::from_secs(1),
            retry_backoff_max: std::time::Duration::from_secs(60),
            max_requeues: 3,
            clusters: Default::default(),
        };
        let inbox =
            task_ops::human_inbox::human_inbox(&self.store, None, &view, at(2), &|_, _| vec![])
                .unwrap();
        let item = inbox
            .items
            .iter()
            .find(|i| i.id == format!("decision-{DECISION}"))
            .unwrap();
        let source = CosTriageSource {
            source_kind: item.kind.as_str().into(),
            source_key: item.id.clone(),
            source_revision: item.created_at.clone(),
            source_event_id: None,
            operation_id: None,
            summary: item.title.clone(),
            policy_version: POLICY.into(),
            task_id: Some(self.task.id.to_string()),
        };
        self.store
            .cos_triage_ingest_batch("events", "1", &[source], at(2))
            .unwrap();
        let claim = self
            .store
            .cos_triage_claim(&self.thread, RUN, at(3))
            .unwrap()
            .unwrap();
        assert_eq!(claim.item_ids.len(), 1);
        (claim.item_ids[0].clone(), item.created_at.clone())
    }
    pub fn answer(
        &self,
        item: &str,
        revision: &str,
        reply: &Value,
    ) -> task_core::chat::CosOperation {
        let by = format!("cos:{}", self.thread);
        let plan = task_ops::decision::plan_answer(
            &self.store,
            DECISION,
            reply["option"].as_str(),
            reply["note"].as_str(),
            &by,
            at(5),
        )
        .unwrap();
        let ctx = AuditContext {
            actor: ChatActor::Cos,
            thread_id: self.thread.clone(),
            run_id: RUN.into(),
            operation_id: ulid::Ulid::new().to_string(),
            reason: REASON.into(),
            policy_version: POLICY.into(),
        };
        let op = self
            .store
            .cos_operation_apply_at(
                &ctx,
                "flow1-answer",
                "fixed-reply-v1",
                "decision",
                DECISION,
                Some(revision),
                "decision.answer",
                reply,
                at(5),
                |tx, _| {
                    assert!(SqliteStore::decision_resolve_apply_tx_at(
                        tx,
                        plan.node_id,
                        &plan.decision_id,
                        plan.expect,
                        plan.rows(),
                        plan.events(),
                        at(5)
                    )?);
                    Ok(json!({"decision_id": DECISION, "task_id": self.task.id.to_string()}))
                },
            )
            .unwrap();
        task_ops::decision::finish_answer(&self.store, plan, at(5)).unwrap();
        assert!(
            self.store
                .cos_triage_resolve(item, "answered", Some(&op.id), Some(REASON), at(5))
                .unwrap()
        );
        let usage: task_core::Usage = serde_json::from_value(reply["usage"].clone()).unwrap();
        self.store
            .chat_run_finish_with_telemetry(
                RUN,
                ChatRunState::Completed,
                Some("local queue を選んだ"),
                None,
                at(8),
                Some(&usage),
                0,
                Some(at(4)),
            )
            .unwrap();
        op
    }
    pub fn outputs(&self, human_ops: &std::path::Path) -> Outputs {
        self.store
            .decision_log_project_interventions(&self.task.id.to_string())
            .unwrap();
        self.store.decision_log_project_cos_turns(at(10)).unwrap();
        let export = self
            .store
            .decision_log_export_campaign(&ExportParams {
                campaign_id: CAMPAIGN.into(),
                manifest_hash: Some(HASH.into()),
                export_cursor: None,
            })
            .unwrap();
        let records = self.dir.path().join("records.jsonl");
        fs::write(&records, export.records_jsonl()).unwrap();
        let report = celeris_campaign::human_ops_report::build(human_ops, &[&records])
            .unwrap()
            .json;
        let requests =
            serde_json::to_value(self.store.request_ledger(at(0), at(60)).unwrap()).unwrap();
        Outputs {
            export,
            report,
            requests,
        }
    }
}
pub struct Outputs {
    pub export: CampaignExport,
    pub report: Value,
    pub requests: Value,
}

/// Existing fake HTTP framing handles shared-host probes; response is fixed, local and one-shot.
pub async fn fake_cos_response() -> Value {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/cos", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, raw) = fake_http::accept(&listener, "POST", "/cos").await;
        assert!(raw.contains(REASON));
        let body = json!({"option":"local", "note": REASON, "usage": {
            "input_tokens": 120, "output_tokens": 30, "cache_read_tokens": 40,
            "cache_creation_tokens": 10, "cost_usd": 0.002
        }})
        .to_string();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
    });
    let reply = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(url)
        .json(&json!({"instruction": REASON}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    server.await.unwrap();
    reply
}

use task_core::{Budget, Check, Criterion, Tier, WorkerHint, WorkspaceSpec};
pub fn new_task() -> Task {
    let id = TaskId::new();
    let now = at(0);
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
        kind: TaskKind::Execute,
        title: "actor trace".into(),
        objective: "make it work".into(),
        acceptance: vec![Criterion {
            text: "a human is happy".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Ready,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: PathBuf::from(id.to_string()),
            mode: None,
        },
        budget: Budget {
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
        project_id: None,
        milestone_id: None,
        assignee: None,
        conversation: None,
        labels: Vec::new(),
        category: Default::default(),
    }
}

use celeris_campaign::{human_ops::*, ledger::TrialKey, manifest::ConfigId};
use std::collections::BTreeMap;
fn human_record_base(set: ExperimentSet, arm: ConfigId, rt: RecordType) -> HumanOpRecord {
    let key = TrialKey {
        manifest_sha256: HASH.into(),
        problem_id: "flow1".into(),
        arm,
        trial_no: 1,
        rep: 1,
    };
    HumanOpRecord {
        schema: HUMAN_OP_SCHEMA.into(),
        record_id: String::new(),
        record_type: rt,
        experiment_set: set,
        campaign_id: CAMPAIGN.into(),
        manifest_sha256: HASH.into(),
        case_id: Some("flow1".into()),
        arm: Some(arm),
        repetition: Some(1),
        trial_no: Some(1),
        trial_key: Some(key.idempotency_key()),
        task_id: None,
        run_id: None,
        job_ref: None,
        logical_op_id: None,
        attempt: None,
        recorded_at: ts(10),
        clock_source: ClockSource::SystemUtc,
        host: "h1".into(),
        start: None,
        end: None,
        occurred_at: None,
        job_submit: None,
        job_start: None,
        job_end: None,
        time_basis: None,
        time_precision: None,
        missing: BTreeMap::new(),
        op_kind: None,
        mode: None,
        intent: None,
        actor: None,
        decided_by: None,
        executed_by: None,
        approved_by: None,
        credential: None,
        request_ref: None,
        request_revision: None,
        policy_ref: None,
        reason: None,
        trigger: None,
        targets: vec![],
        celeris_event_ref: None,
        outcome: None,
        wait_kind: None,
        result: None,
        corrects: None,
        set: None,
        note: None,
    }
}

/// Evaluator observations of the executed operation, keyed to the actual audit receipt.
/// Manual mode deliberately proves that actor kind (not automatic mode) excludes AI from human counts.
pub fn record_observed_operation(
    h: &Harness,
    op: &task_core::chat::CosOperation,
    revision: &str,
) -> PathBuf {
    let file = h.dir.path().join("human-ops.jsonl");
    let log = HumanOpLog::new(&file);
    for (rt, start, end) in [
        (RecordType::OpStart, Some(ts(4)), None),
        (RecordType::OpEnd, None, Some(ts(5))),
    ] {
        let mut r = human_record_base(ExperimentSet::MainAb, ConfigId::B, rt);
        r.task_id = Some(h.task.id.to_string());
        r.logical_op_id = Some(op.id.clone());
        r.attempt = Some(1);
        r.start = start;
        r.end = end;
        r.time_basis = Some(TimeBasis::HarnessTranscript);
        r.time_precision = Some(TimePrecision::Second);
        r.op_kind = Some(OpKind::ResearchDirection);
        r.mode = Some(Mode::Manual);
        r.intent = Some(Intent::Research);
        r.actor = Some(Actor {
            kind: ActorKind::CosProject,
            id: op.thread_id.clone(),
        });
        r.decided_by = r.actor.clone();
        r.executed_by = r.actor.clone();
        r.credential = Some("cos_run".into());
        r.request_ref = Some(format!("decision:decision-{DECISION}"));
        r.request_revision = Some(revision.into());
        r.policy_ref = Some(format!("cos_operation_policy:{}", op.policy_version));
        r.reason = Some(op.reason.clone());
        r.trigger = Some(Trigger::SystemRequest);
        if rt == RecordType::OpEnd {
            r.outcome = Some(Outcome::Ok);
        }
        log.append(&r.with_record_id()).unwrap();
    }
    file
}
