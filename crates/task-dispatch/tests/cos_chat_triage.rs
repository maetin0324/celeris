//! ADR 2026-10-05-cos-chat-home D6「導入順と受け入れ条件」の cos-run 担当行を、dispatcher の
//! 公開 API だけで通しで見る結合試験（一次対応 A・B・C、通知一本化、代答の修正）。
//!
//! 外部 LLM は使わない。CoS worker は `FakeAdapter`（sh の台本）で、CoS の resolve API の役は
//! 試験コードが task-core の同じ store 操作（`cos_triage_resolve`・`cos_triage_outbox_claim`）で
//! 務める。DB は一時 SQLite、Discord の送信は outbox（`notifications` の行）を数えて見る。
//! 時刻は sleep に頼らない: run の終わりは `chat_runs` の状態という出来事を待ち（長い保険付き）、
//! 期限超過は item の `created_at` を過去に置いて注入する。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use task_core::{
    Budget, Check, Criterion, DelegationLimits, Event, NoticeEvent, NoticeKind, NoticeStore,
    SqliteStore, Status, Task, TaskId, TaskKind, TaskStore, Tier, WorkerHint, WorkspaceSpec,
};
use task_dispatch::dispatcher::cos_chat::launch::CosChatLaunchConfig;
use task_dispatch::dispatcher::{DispatchConfig, Dispatcher};
use task_dispatch::policy::{ProviderSpec, StaticPolicy};
use task_worker::{FakeAdapter, WorkerAdapter};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const DONE: &str =
    "cat >/dev/null; echo '{\"type\":\"done\",\"summary\":\"done\",\"evidence\":[]}'";
const FAIL: &str = "cat >/dev/null; echo boom >&2; exit 3";
const QUOTA: &str =
    "CoS unavailable: no usable account for provider p1 (quota, cooldown, or login)";

/// Kinds that the notifier may still send (D6 通知の一本化).
const COS_KINDS: &str = "('cos_escalation','cos_fallback')";

struct Fixture {
    dir: tempfile::TempDir,
    db_path: PathBuf,
    store: Arc<SqliteStore>,
    d: Dispatcher,
}

fn launch_config(dir: &Path, db_path: &Path, enabled: bool) -> CosChatLaunchConfig {
    CosChatLaunchConfig {
        enabled,
        harness: "fake".into(),
        llm_source: Some("test".into()),
        provider: Some("p1".into()),
        account_id: None,
        model: None,
        tier: Tier::Frontier,
        max_turns: 2,
        max_wall_secs: 30,
        unavailable_reason: None,
        data_dir: dir.to_path_buf(),
        db_path: db_path.to_path_buf(),
        attachment_limits: Default::default(),
        api_base_url: "http://127.0.0.1:7700/api/v1".into(),
        triage: Default::default(),
    }
}

fn dispatcher(store: Arc<SqliteStore>, script: &str, dir: &Path) -> Dispatcher {
    let adapter: Arc<dyn WorkerAdapter> = Arc::new(FakeAdapter::new(vec![
        "sh".into(),
        "-c".into(),
        script.into(),
    ]));
    let policy = StaticPolicy::new(
        vec![ProviderSpec {
            id: "p1".into(),
            adapter: "fake".into(),
            tiers: vec![Tier::Frontier, Tier::Standard, Tier::Cheap],
            concurrency: 2,
            model: "m".into(),
        }],
        Duration::from_secs(1),
    );
    let mut adapters: HashMap<String, Arc<dyn WorkerAdapter>> = HashMap::new();
    adapters.insert("p1".into(), adapter);
    let knowledge_root = dir.join("knowledge");
    for name in ["cos-operator", "cos-inbox-triage"] {
        let skill_dir = knowledge_root.join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test skill\n---\n# {name}\n"),
        )
        .expect("skill");
    }
    let execution = task_dispatch::ExecutionConfig {
        max_cos_runs: 1,
        ..Default::default()
    };
    let store: Arc<dyn TaskStore> = store;
    Dispatcher::new(
        store,
        Box::new(policy),
        HashMap::from([("p1".to_string(), "m".to_string())]),
        adapters,
        HashSet::new(),
        DispatchConfig {
            delivery: Default::default(),
            max_concurrency: 2,
            lease_grace: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(30),
            kill_grace: Duration::from_millis(200),
            review_timeout: Duration::from_secs(5),
            workspace_root: dir.join("workspaces"),
            plan_auto_accept: false,
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            reviewer_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: None,
            },
            reviewer_tier_override: None,
            clusters: HashMap::new(),
            cluster_cooldown: Duration::from_secs(1),
            max_requeues: 5,
            max_reviewer_retries: 3,
            max_infra_retries: 5,
            min_free_disk_mb: 0,
            roles: Vec::new(),
            genres: Vec::new(),
            delegation: DelegationLimits::default(),
            accounts: None,
            memory_dir: None,
            worktree_branch_prefix: task_worker::DEFAULT_BRANCH_PREFIX.to_string(),
            releases_dir: None,
            containers: task_dispatch::ContainersRuntimeConfig::default(),
            knowledge: task_dispatch::KnowledgeRuntimeConfig {
                root: knowledge_root,
                ..Default::default()
            },
            session_rollover_tokens: 400_000,
            shared_build_cache: false,
            build_cache_dir: PathBuf::from("/nonexistent-build-cache"),
            scratch: task_worker::scratch::ScratchSettings::disabled(),
            workspace_prune_after_secs: 0,
            execution,
        },
    )
}

fn fixture(enabled: bool, script: &str) -> Fixture {
    fixture_with(script, |c| c.enabled = enabled)
}

fn fixture_with(script: &str, edit: impl FnOnce(&mut CosChatLaunchConfig)) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("celeris.db");
    let store = Arc::new(SqliteStore::open(&db_path).expect("store"));
    let mut d = dispatcher(store.clone(), script, dir.path());
    let mut config = launch_config(dir.path(), &db_path, true);
    edit(&mut config);
    d.set_cos_chat_launch(store.clone(), config);
    Fixture {
        dir,
        db_path,
        store,
        d,
    }
}

fn new_task(dir: &Path, title: &str) -> Task {
    let now = OffsetDateTime::now_utc();
    Task {
        tree: None,
        paused_at: None,
        routing: None,
        mode: Default::default(),
        skills: Vec::new(),
        repos: Vec::new(),
        id: TaskId::new(),
        parent_id: None,
        kind: TaskKind::Execute,
        title: title.into(),
        objective: title.into(),
        acceptance: vec![Criterion {
            text: "c".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Blocked,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
            adapter: None,
        },
        workspace: WorkspaceSpec::Local {
            path: dir.to_path_buf(),
            mode: None,
        },
        budget: Budget {
            max_turns: 1,
            max_wall_secs: 60,
            max_retries: 0,
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

impl Fixture {
    fn conn(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.db_path).expect("db")
    }

    fn count(&self, sql: &str) -> i64 {
        self.conn()
            .query_row(sql, [], |r| r.get(0))
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
    }

    fn tick(&mut self) {
        self.d.tick().expect("tick");
    }

    fn ticks(&mut self, n: usize) {
        for _ in 0..n {
            self.tick();
        }
    }

    fn items(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM cos_inbox_items")
    }

    fn state_count(&self, state: &str) -> i64 {
        self.count(&format!(
            "SELECT COUNT(*) FROM cos_inbox_items WHERE state='{state}'"
        ))
    }

    fn inbox_runs(&self) -> i64 {
        self.count(
            "SELECT COUNT(*) FROM chat_runs r JOIN chat_threads t ON t.id=r.thread_id \
             WHERE t.kind='inbox'",
        )
    }

    /// Every outbox row (what the notifier could ever POST).
    fn outbox_total(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM notifications")
    }

    fn outbox_kind(&self, kind: &str) -> i64 {
        self.count(&format!(
            "SELECT COUNT(*) FROM notifications WHERE kind='{kind}'"
        ))
    }

    fn item_ids(&self) -> Vec<String> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT id FROM cos_inbox_items ORDER BY created_at,id")
            .expect("prepare");
        stmt.query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<Vec<String>, _>>()
            .expect("ids")
    }

    fn fallback_body(&self) -> serde_json::Value {
        let body: String = self
            .conn()
            .query_row(
                "SELECT body FROM notifications WHERE kind='cos_fallback'",
                [],
                |r| r.get(0),
            )
            .expect("fallback body");
        serde_json::from_str(&body).expect("json body")
    }

    /// Wait for the event "no inbox run is live" (bounded insurance, no fixed sleep).
    async fn runs_settled(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let live = self.count(
                "SELECT COUNT(*) FROM chat_runs WHERE state IN ('queued','running','stopping')",
            );
            if live == 0 {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the inbox run never finished"
            );
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A blocked task with an open question: one `question` wait of the derived inbox.
    fn question(&self, title: &str) -> TaskId {
        let task = new_task(self.dir.path(), title);
        self.store
            .create_task(
                &task,
                vec![Event::QuestionRaised {
                    run_id: "r0".into(),
                    text: format!("{title}?"),
                }],
            )
            .expect("task");
        task.id
    }

    fn notice(&self, key: &str, kind: NoticeKind) {
        self.store
            .notice_record(&NoticeEvent {
                source_key: key.into(),
                kind,
                group_key: format!("g:{key}"),
                title: key.into(),
                summary: key.into(),
                project_id: None,
                task_id: None,
                target: None,
                links: Vec::new(),
                at: OffsetDateTime::now_utc(),
            })
            .expect("notice");
    }

    /// The CoS resolve API's terminal write for an answer or observe.
    fn cos_resolves(&self, item: &str, outcome: &str) {
        assert!(
            self.store
                .cos_triage_resolve(
                    item,
                    outcome,
                    Some("op-test"),
                    Some("承認済みの範囲"),
                    OffsetDateTime::now_utc(),
                )
                .expect("resolve"),
            "the item was still open"
        );
    }

    fn assert_one_fallback(&self, reason_part: &str) {
        assert_eq!(
            self.outbox_kind("cos_fallback"),
            1,
            "one outbox per revision"
        );
        assert_eq!(self.state_count("fallback"), 1);
        let body = self.fallback_body();
        let reason = body["unavailable_reason"].as_str().expect("reason");
        assert!(reason.contains(reason_part), "{reason}");
        assert!(
            body["web_path"]
                .as_str()
                .expect("web_path")
                .starts_with("/tasks/"),
            "{body}"
        );
    }

    /// A daemon restart: the process-local launch state is rebuilt.
    fn restart(&mut self, enabled: bool) {
        let config = launch_config(self.dir.path(), &self.db_path, enabled);
        self.d.set_cos_chat_launch(self.store.clone(), config);
    }
}

// ---- 一次対応 A（人不要）----

#[tokio::test]
async fn cos_chat_triage_a_answered_wait_sends_nothing_and_is_not_redelivered() {
    let mut f = fixture(true, DONE);
    f.tick();
    f.question("approved-scope");
    f.tick();
    assert_eq!(f.items(), 1);
    assert_eq!(f.inbox_runs(), 1, "one CoS run for the wait");
    let item = f.item_ids().remove(0);
    // CoS answers inside the approved scope while its run is live.
    f.cos_resolves(&item, "answered");
    f.runs_settled().await;
    f.ticks(3);
    assert_eq!(f.outbox_total(), 0, "webhook 0 通: no outbox row at all");
    assert_eq!(f.state_count("answered"), 1);

    // Redelivery: a reconcile pass and a restart see the same revision.
    f.d.request_cos_triage_reconcile();
    f.ticks(2);
    f.restart(true);
    f.ticks(2);
    f.runs_settled().await;
    assert_eq!(f.items(), 1, "no second item for the same revision");
    assert_eq!(f.inbox_runs(), 1, "no re-answer run");
    assert_eq!(f.outbox_total(), 0);
    assert!(
        !f.store
            .cos_triage_resolve(&item, "answered", None, None, OffsetDateTime::now_utc())
            .expect("resolve"),
        "an answered item cannot be answered again"
    );
}

#[tokio::test]
async fn cos_chat_triage_a_observed_notice_sends_nothing() {
    let mut f = fixture(true, DONE);
    f.tick();
    f.notice("done-1", NoticeKind::TaskDone);
    f.tick();
    assert_eq!(f.items(), 1);
    let item = f.item_ids().remove(0);
    f.cos_resolves(&item, "observed");
    f.runs_settled().await;
    f.ticks(3);
    assert_eq!(f.outbox_total(), 0, "observe sends nothing");
}

// ---- 一次対応 B（人必要）----

#[tokio::test]
async fn cos_chat_triage_b_escalation_is_one_outbox_and_wait_stays_open() {
    let mut f = fixture(true, DONE);
    f.tick();
    let task = f.question("external-push");
    f.tick();
    let item = f.item_ids().remove(0);
    let claimed = f
        .store
        .cos_triage_outbox_claim(
            &item,
            "escalation",
            "{\"headline\":\"push\"}",
            OffsetDateTime::now_utc(),
        )
        .expect("claim");
    assert!(claimed.is_some());
    assert!(
        f.store
            .cos_triage_resolve(
                &item,
                "escalated",
                Some("op-esc"),
                Some("外部 push"),
                OffsetDateTime::now_utc()
            )
            .expect("resolve")
    );
    f.runs_settled().await;
    f.ticks(3);
    assert_eq!(f.outbox_kind("cos_escalation"), 1);
    assert_eq!(
        f.outbox_kind("cos_fallback"),
        0,
        "escalated is not also a fallback"
    );
    assert_eq!(f.outbox_total(), 1);
    // The original wait is not consumed by the escalation.
    let status = f.store.get(task).expect("get").expect("task").status;
    assert_eq!(status, Status::Blocked);
    // A second claim for the same revision (e.g. a recovered CoS) adds nothing.
    assert!(
        f.store
            .cos_triage_outbox_claim(&item, "escalation", "{}", OffsetDateTime::now_utc())
            .expect("claim")
            .is_none()
    );
    assert_eq!(f.outbox_total(), 1);
}

#[tokio::test]
async fn cos_chat_triage_b_human_answer_before_post_withdraws_the_escalation() {
    let mut f = fixture(true, DONE);
    f.tick();
    f.question("design-change");
    f.tick();
    let item = f.item_ids().remove(0);
    let id = f
        .store
        .cos_triage_outbox_claim(&item, "escalation", "{}", OffsetDateTime::now_utc())
        .expect("claim")
        .expect("claimed");
    f.runs_settled().await;
    // The person answered on the web: the notifier's recheck finds the source resolved.
    assert!(
        !f.store
            .cos_triage_outbox_sendable(&id, false, OffsetDateTime::now_utc())
            .expect("sendable")
    );
    assert_eq!(f.state_count("resolved"), 1);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM notifications WHERE ok IS NULL"),
        0,
        "nothing left to send after the human answer"
    );
    f.ticks(3);
    assert_eq!(f.outbox_kind("cos_fallback"), 0);
}

// ---- 一次対応 C（CoS 不在）: 条件ごとに独立の fixture ----

#[tokio::test]
async fn cos_chat_triage_c_run_failure_falls_back_once() {
    let mut f = fixture(true, FAIL);
    f.tick();
    f.question("deploy");
    f.tick();
    assert_eq!(f.inbox_runs(), 1);
    f.runs_settled().await;
    f.ticks(4);
    f.assert_one_fallback("CoS run が失敗");
    assert_eq!(
        f.inbox_runs(),
        1,
        "the fallback item is never claimed again"
    );
}

#[tokio::test]
async fn cos_chat_triage_c_quota_or_login_falls_back_once() {
    let mut f = fixture_with(DONE, |c| c.unavailable_reason = Some(QUOTA.into()));
    f.tick();
    f.question("quota");
    f.ticks(4);
    f.runs_settled().await;
    f.ticks(2);
    f.assert_one_fallback("quota 切れ・ログイン不可");
}

#[tokio::test]
async fn cos_chat_triage_c_disabled_falls_back_without_a_run() {
    let mut f = fixture(false, DONE);
    f.tick();
    f.question("disabled");
    f.ticks(3);
    assert_eq!(f.inbox_runs(), 0, "no LLM run while CoS is disabled");
    f.assert_one_fallback("cos.enabled=false");
}

#[tokio::test]
async fn cos_chat_triage_c_deadline_falls_back_when_cos_never_starts() {
    let mut f = fixture(true, DONE);
    // Intake of new work is stopped: CoS never starts, only the deadline can fire.
    f.d.set_accepting_new_work(false);
    f.tick();
    f.question("waiting");
    f.ticks(2);
    assert_eq!(f.outbox_total(), 0, "within unavailable_after_secs");
    assert_eq!(f.state_count("pending"), 1);
    // Inject the elapsed time: the item was created well past the 120 s limit.
    let past = (OffsetDateTime::now_utc() - time::Duration::seconds(600))
        .format(&Rfc3339)
        .expect("format");
    f.conn()
        .execute("UPDATE cos_inbox_items SET created_at=?1", [past])
        .expect("age item");
    f.tick();
    f.assert_one_fallback("120 秒");
    assert_eq!(f.inbox_runs(), 0);
}

#[tokio::test]
async fn cos_chat_triage_c_restart_and_recovery_neither_resend_nor_answer() {
    let mut f = fixture(false, DONE);
    f.tick();
    f.question("handed");
    f.ticks(2);
    f.assert_one_fallback("cos.enabled=false");
    // CoS comes back after a restart; the reconcile sees the same revision.
    f.restart(true);
    f.ticks(3);
    f.d.request_cos_triage_reconcile();
    f.ticks(2);
    f.runs_settled().await;
    assert_eq!(f.inbox_runs(), 0, "no CoS run for a handed-over revision");
    assert_eq!(f.items(), 1);
    assert_eq!(f.outbox_total(), 1, "no second notice");
    assert_eq!(f.state_count("fallback"), 1);
}

// ---- 通知一本化 ----

#[tokio::test]
async fn cos_chat_triage_unified_no_direct_outbox_for_any_notice_kind() {
    let mut f = fixture(true, DONE);
    f.tick();
    for (i, kind) in NoticeKind::ALL.iter().enumerate() {
        f.notice(&format!("n{i}"), *kind);
    }
    f.question("asks");
    f.ticks(2);
    assert!(f.items() >= 2, "notices and the wait reach the CoS inbox");
    // While CoS is live nothing is sent directly, for any kind.
    assert_eq!(f.outbox_total(), 0, "no direct sending");
    // CoS observes every notice and escalates the one human matter.
    for item in f.item_ids() {
        let kind: String = f
            .conn()
            .query_row(
                "SELECT source_kind FROM cos_inbox_items WHERE id=?1",
                [&item],
                |r| r.get(0),
            )
            .expect("kind");
        if kind == "notice" {
            f.cos_resolves(&item, "observed");
        } else {
            f.store
                .cos_triage_outbox_claim(&item, "escalation", "{}", OffsetDateTime::now_utc())
                .expect("claim")
                .expect("claimed");
            f.cos_resolves(&item, "escalated");
        }
    }
    f.runs_settled().await;
    f.ticks(3);
    assert_eq!(
        f.outbox_kind("cos_escalation"),
        1,
        "only the escalation is sent"
    );
    assert_eq!(
        f.count(&format!(
            "SELECT COUNT(*) FROM notifications WHERE kind NOT IN {COS_KINDS}"
        )),
        0
    );
    assert_eq!(f.outbox_total(), 1);
}

// ---- 代答の修正 ----

#[tokio::test]
async fn cos_chat_triage_override_new_revision_reopens_triage_and_old_is_closed() {
    let mut f = fixture(true, DONE);
    f.tick();
    let task = f.question("revoked");
    f.tick();
    let first = f.item_ids().remove(0);
    f.cos_resolves(&first, "answered");
    f.runs_settled().await;
    f.ticks(2);
    // A person revokes the answer: the wait reopens as a new revision.
    f.store
        .append_event(
            task,
            &Event::QuestionRaised {
                run_id: "r1".into(),
                text: "revoked? (again)".into(),
            },
        )
        .expect("reopen");
    f.ticks(2);
    f.runs_settled().await;
    let ids = f.item_ids();
    assert_eq!(ids.len(), 2, "the new revision is a new item");
    assert_eq!(
        f.state_count("answered"),
        1,
        "the old answer stays in the record"
    );
    assert_eq!(f.inbox_runs(), 2, "CoS looks at the new revision once");
    // The old revision cannot be answered again (409 at the API).
    assert!(
        !f.store
            .cos_triage_resolve(&first, "answered", None, None, OffsetDateTime::now_utc())
            .expect("resolve")
    );
    assert_eq!(f.outbox_total(), 0);
}
