//! ADR 2026-10-29-stagnation-triage-notify-wiring: 停滞 4 種の CoS の escalation を送信直前に照合する
//! （`notify::triage::withdraw_if_resolved`、daemon の notifier tick と同じ関数）。停滞の snapshot を渡せば生きている
//! revision は送れて、照合を繰り返しても行は増えない。改訂・解消すれば撤回し、snapshot が無ければ撤回も送信も
//! しない（保留）。時刻は固定値か壁時計からの差で、外部ネットワークに出ない。

use celeris::notify::triage::Sendable;
use celeris::notify::{self};
use task_core::chat::triage::CosTriageSource;
use task_core::{Event, NotificationStore, SqliteStore, TaskId, TaskStore};
use task_ops::daemon::DaemonSnapshot;
use task_ops::human_inbox::InboxKind;
use task_ops::stagnation::{StagnationFinding, StagnationKind, StagnationPolicy};
use time::OffsetDateTime;

struct Case {
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
    store: SqliteStore,
    view: task_ops::view::ViewContext,
    now: OffsetDateTime,
}

fn case() -> Case {
    let dir = tempfile::tempdir().expect("dir");
    let db = dir.path().join("test.db");
    let store = SqliteStore::open(&db).expect("db");
    let view = task_ops::view::ViewContext {
        workspace_roots: task_core::local_machine::WorkspaceRoots::new(dir.path().into(), None),
        retry_backoff_base: std::time::Duration::from_secs(1),
        retry_backoff_max: std::time::Duration::from_secs(60),
        max_requeues: 3,
        clusters: Default::default(),
    };
    Case {
        _dir: dir,
        db,
        store,
        view,
        now: OffsetDateTime::now_utc() + time::Duration::hours(3),
    }
}

fn finding(kind: StagnationKind, key: &str, since: &str) -> StagnationFinding {
    StagnationFinding {
        kind,
        key: key.into(),
        project_id: None,
        task_id: None,
        subject: key.into(),
        since: since.into(),
        detail: "閾値を過ぎた".into(),
    }
}

fn snapshot(findings: Vec<StagnationFinding>, human_wait_secs: u64) -> DaemonSnapshot {
    DaemonSnapshot::stagnation_only(
        findings,
        StagnationPolicy {
            human_wait_secs,
            ..Default::default()
        },
    )
}

impl Case {
    /// The CoS path: the derived inbox with `snap` → one `cos_inbox_items` row → CoS escalates
    /// (outbox claim + `escalated`). Returns the pending escalation row.
    fn escalate(&self, kind: InboxKind, snap: &DaemonSnapshot) -> task_core::Notification {
        let item = task_ops::human_inbox::human_inbox(
            &self.store,
            Some(snap),
            &self.view,
            self.now,
            &|_, _| Vec::new(),
        )
        .expect("inbox")
        .items
        .into_iter()
        .find(|i| i.kind == kind)
        .expect("the stagnation item is in the inbox");
        let source = CosTriageSource {
            source_kind: kind.as_str().into(),
            source_key: item.id.clone(),
            source_revision: item.created_at.clone(),
            source_event_id: None,
            operation_id: None,
            summary: item.title.clone(),
            detail: String::new(),
            policy_version: "1".into(),
            task_id: None,
        };
        self.store
            .cos_triage_reconcile(kind.as_str(), &[source], self.now)
            .expect("ingest");
        let id = self.item_id();
        self.store
            .cos_triage_outbox_claim(
                &id,
                "escalation",
                &serde_json::json!({"summary": item.title, "web_path": "/inbox"}).to_string(),
                self.now,
            )
            .expect("claim")
            .expect("claimed");
        assert!(
            self.store
                .cos_triage_resolve(&id, "escalated", None, None, self.now)
                .expect("escalate")
        );
        let mut pending = self.store.notification_pending().expect("pending");
        assert_eq!(pending.len(), 1);
        pending.remove(0)
    }

    fn item_id(&self) -> String {
        let conn = rusqlite::Connection::open(&self.db).expect("db");
        conn.query_row("SELECT id FROM cos_inbox_items", [], |r| r.get(0))
            .expect("item")
    }

    fn item_state(&self) -> String {
        let conn = rusqlite::Connection::open(&self.db).expect("db");
        conn.query_row("SELECT state FROM cos_inbox_items", [], |r| r.get(0))
            .expect("state")
    }

    fn check(&self, snap: Option<&DaemonSnapshot>, row: &task_core::Notification) -> Sendable {
        notify::triage::withdraw_if_resolved(&self.store, &self.db, &self.view, snap, row, self.now)
            .expect("recheck")
    }

    /// Live with the snapshot (twice: re-checks change nothing), held without it.
    fn live_then_held(&self, snap: &DaemonSnapshot, row: &task_core::Notification) {
        for _ in 0..2 {
            assert_eq!(self.check(Some(snap), row), Sendable::Send);
        }
        for _ in 0..2 {
            assert_eq!(
                self.check(None, row),
                Sendable::Held,
                "no snapshot: neither sent nor withdrawn"
            );
        }
        assert_eq!(self.store.notification_pending().expect("pending").len(), 1);
        assert_eq!(self.item_state(), "escalated", "the item is untouched");
        // The snapshot is back: the same row is sendable again.
        assert_eq!(self.check(Some(snap), row), Sendable::Send);
    }

    fn assert_withdrawn(&self, snap: &DaemonSnapshot, row: &task_core::Notification) {
        assert_eq!(self.check(Some(snap), row), Sendable::Withdrawn);
        assert!(
            self.store
                .notification_pending()
                .expect("pending")
                .is_empty()
        );
        // The escalated item itself is closed by the triage stale sweep (dispatcher), not here.
        assert_eq!(self.item_state(), "escalated");
        let conn = rusqlite::Connection::open(&self.db).expect("db");
        let error: String = conn
            .query_row("SELECT error FROM notifications", [], |r| r.get(0))
            .expect("error");
        assert_eq!(error, "superseded");
        // Once withdrawn, a missing snapshot does not bring it back.
        assert_eq!(self.check(None, row), Sendable::Withdrawn);
    }
}

/// `project_stalled`・`release_unpromoted`・`task_unscheduled`: snapshot の判定から来る項目。生きている revision は
/// 送れて、snapshot が無い間は保留、同じ key の新しい revision（状態が変わった）なら旧い通知を撤回する。
#[test]
fn stagnation_findings_are_live_held_and_withdrawn_on_revision() {
    for (stagnation, kind, key) in [
        (
            StagnationKind::ProjectStalled,
            InboxKind::ProjectStalled,
            "project_stalled-01M62H6RMC2H720JZR8T9JNVZP",
        ),
        (
            StagnationKind::ReleaseUnpromoted,
            InboxKind::ReleaseUnpromoted,
            "release_unpromoted-selfdeploy",
        ),
        (
            StagnationKind::TaskUnscheduled,
            InboxKind::TaskUnscheduled,
            "task_unscheduled-01M62H7AZ2EHSTV8Z6YT6AYG5T",
        ),
    ] {
        let c = case();
        let first = snapshot(vec![finding(stagnation, key, "2026-10-29T01:00:00Z")], 0);
        let row = c.escalate(kind, &first);
        c.live_then_held(&first, &row);
        let revised = snapshot(vec![finding(stagnation, key, "2026-10-29T02:00:00Z")], 0);
        c.assert_withdrawn(&revised, &row);
    }
}

/// `release_unpromoted` の解消（昇格で判定から消えた）: 旧い通知は送らずに撤回する。
#[test]
fn resolved_stagnation_is_withdrawn_before_post() {
    let c = case();
    let first = snapshot(
        vec![finding(
            StagnationKind::ReleaseUnpromoted,
            "release_unpromoted-selfdeploy",
            "2026-10-29T01:00:00Z",
        )],
        0,
    );
    let row = c.escalate(InboxKind::ReleaseUnpromoted, &first);
    assert_eq!(c.check(Some(&first), &row), Sendable::Send);
    c.assert_withdrawn(&snapshot(Vec::new(), 0), &row);
}

/// `human_wait_aging`: 閾値を過ぎた人の待ち（質問）から組む項目。snapshot（の閾値）があれば生きていて、無ければ保留、
/// 待ちが答えられて閾値を過ぎた待ちが無くなれば撤回する。
#[test]
fn human_wait_aging_is_live_held_and_withdrawn_when_answered() {
    let c = case();
    let task = question_task(&c.store);
    let snap = snapshot(Vec::new(), 600);
    let row = c.escalate(InboxKind::HumanWaitAging, &snap);
    c.live_then_held(&snap, &row);
    c.store
        .apply_transition_with_events(
            task.id,
            task_core::Trigger::Answer,
            vec![Event::Answered {
                by: None,
                question: "どちらにする?".into(),
                answer: "A".into(),
            }],
        )
        .expect("answer");
    c.assert_withdrawn(&snap, &row);
}

fn question_task(store: &SqliteStore) -> task_core::Task {
    use task_core::{Budget, Check, Criterion, Status, TaskKind, Tier, WorkerHint, WorkspaceSpec};
    let id = TaskId::new();
    let now = OffsetDateTime::now_utc();
    let task = task_core::Task {
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
        title: "答えを待つ task".into(),
        objective: "o".into(),
        acceptance: vec![Criterion {
            text: "a human is happy".into(),
            check: Check::Human,
        }],
        inputs: vec![],
        depends_on: vec![],
        status: Status::Blocked,
        priority: 0,
        worker_hint: WorkerHint {
            tier: Tier::Standard,
        },
        workspace: WorkspaceSpec::Local {
            path: std::path::PathBuf::from(id.to_string()),
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
    };
    store
        .create_task(
            &task,
            vec![Event::QuestionRaised {
                run_id: "r0".into(),
                text: "どちらにする?".into(),
            }],
        )
        .expect("task");
    task
}
