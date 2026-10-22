//! root task の完了まとめ（ADR 2026-10-15-root-done-cos-summary）の dispatcher 側の配線。
//! 時間は注入した試験用の時計、run は worker の handle を join して待つ（sleep しない）。

use super::*;

impl Fixture {
    /// root（または子）の task を done にする。`parent` を渡すと木の子になる。
    fn done_task(&self, parent: Option<&Task>) -> Task {
        self.done_task_in(parent, None)
    }

    /// `done_task` in a project (ADR 2026-10-22-cos-fixed-threads D3).
    fn done_task_in(&self, parent: Option<&Task>, project: Option<task_core::ProjectId>) -> Task {
        let mut task = new_task(self.dir.path(), Check::Human, 0);
        task.title = "調査の仕事".into();
        task.project_id = project;
        task.status = Status::Reviewing;
        if let Some(parent) = parent {
            task.parent_id = Some(parent.id);
            task.tree = Some(task_core::TreeInfo::child_of(
                parent,
                task_core::ParentUnit {
                    task_id: parent.id,
                    plan_id: "plan".into(),
                    unit_key: "c".into(),
                    stage: "s1".into(),
                    attempt: 1,
                },
                None,
            ));
        }
        self.store.create_task(&task, vec![]).expect("task");
        self.store
            .apply_transition(task.id, Trigger::ReviewPass, None)
            .expect("done");
        assert_eq!(
            self.store.get(task.id).expect("get").expect("task").status,
            Status::Done
        );
        task
    }

    fn summary_rows(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM root_done_summaries")
    }

    fn summary_notifications(&self) -> Vec<String> {
        let conn = rusqlite::Connection::open(&self.db_path).expect("db");
        let mut stmt = conn
            .prepare("SELECT body FROM notifications WHERE kind='root_done_summary'")
            .expect("prepare");
        stmt.query_map([], |r| r.get::<_, String>(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows")
    }

    /// 起こされた CoS run を全部待つ。
    async fn join_all(&mut self) {
        let handles: Vec<_> = self
            .d
            .cos_chat_launch
            .as_mut()
            .expect("launch")
            .running
            .drain()
            .map(|(_, h)| h)
            .collect();
        for handle in handles {
            tokio::time::timeout(std::time::Duration::from_secs(30), handle)
                .await
                .expect("run deadline")
                .expect("worker did not panic");
        }
    }
}

#[tokio::test]
async fn root_done_starts_one_run_in_a_dedicated_thread_and_notifies_once() {
    let mut f = fixture(true);
    f.tick(); // 導入: cursor は現在の末尾から
    let root = f.done_task(None);
    f.tick();
    assert_eq!(f.summary_rows(), 1);
    let thread: String = f.count_text("SELECT thread_id FROM root_done_summaries");
    // ADR 2026-10-22-cos-fixed-threads D3: a root without a project goes to the general CoS.
    assert_eq!(f.count("SELECT COUNT(*) FROM chat_threads"), 1);
    assert_eq!(
        f.count_text("SELECT kind FROM chat_threads"),
        "general".to_string()
    );
    f.join_all().await;
    for _ in 0..3 {
        f.tick();
    }
    let bodies = f.summary_notifications();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(!bodies[0].contains("決定的な短文"), "{}", bodies[0]);
    assert!(bodies[0].starts_with("[統括 CoS] 完了: "), "{}", bodies[0]);
    assert!(bodies[0].contains(&format!("http://celeris.test/?thread={thread}")));
    assert!(bodies[0].contains(&format!("http://celeris.test/tasks/{}", root.id)));
    assert_eq!(f.count("SELECT COUNT(*) FROM chat_runs WHERE thread_id IN (SELECT thread_id FROM root_done_summaries)"), 1);
    // 再起動相当（cursor を巻き戻して同じ done を読み直す）でも行・run・通知は増えない。
    f.store
        .feed_cursor_set(
            crate::dispatcher::cos_chat::root_done::ROOT_DONE_CURSOR,
            "0",
        )
        .expect("rewind");
    for _ in 0..3 {
        f.tick();
    }
    f.join_all().await;
    assert_eq!(f.summary_rows(), 1);
    assert_eq!(f.summary_notifications().len(), 1);
    assert_eq!(f.count("SELECT COUNT(*) FROM chat_runs WHERE thread_id IN (SELECT thread_id FROM root_done_summaries)"), 1);
}

/// ADR 2026-10-22-cos-fixed-threads D3: roots of one project report into that project's CoS
/// thread (one thread for both), a root without a project into the general CoS; no other thread.
#[tokio::test]
async fn root_done_of_a_project_goes_to_the_project_cos_thread_without_new_threads() {
    let mut f = fixture(true);
    f.tick();
    let thread = crate::test_support::project_cos_thread(&f.store, "案件A");
    let project: String = f.count_text("SELECT project_id FROM chat_threads WHERE kind='project'");
    let project: task_core::ProjectId = project.parse().expect("project id");
    let first = f.done_task_in(None, Some(project));
    f.tick();
    f.join_all().await;
    for _ in 0..3 {
        f.tick();
    }
    let second = f.done_task_in(None, Some(project));
    f.tick();
    f.join_all().await;
    for _ in 0..3 {
        f.tick();
    }
    assert_eq!(f.summary_rows(), 2);
    assert_eq!(
        f.count(&format!(
            "SELECT COUNT(*) FROM root_done_summaries WHERE thread_id='{thread}'"
        )),
        2
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM chat_threads"),
        1,
        "no new thread"
    );
    let bodies = f.summary_notifications();
    assert_eq!(bodies.len(), 2, "{bodies:?}");
    for body in &bodies {
        assert!(body.starts_with("[CoS: 案件A] 完了: "), "{body}");
        assert!(body.contains(&format!("?thread={thread}")), "{body}");
    }
    assert!(bodies.iter().any(|b| b.contains(&first.id.to_string())));
    assert!(bodies.iter().any(|b| b.contains(&second.id.to_string())));
    // A root outside any project goes to the general CoS.
    f.done_task(None);
    f.tick();
    assert_eq!(f.count("SELECT COUNT(*) FROM chat_threads"), 2);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM root_done_summaries r JOIN chat_threads t ON t.id=r.thread_id WHERE t.kind='general'"),
        1
    );
    f.join_all().await;
}

#[tokio::test]
async fn a_child_done_neither_starts_a_run_nor_notifies() {
    let mut f = fixture(true);
    f.tick();
    let mut root = new_task(f.dir.path(), Check::Human, 0);
    root.status = Status::Running;
    f.store.create_task(&root, vec![]).expect("root");
    f.done_task(Some(&root));
    for _ in 0..3 {
        f.tick();
    }
    assert_eq!(f.summary_rows(), 0);
    assert!(f.summary_notifications().is_empty());
    assert_eq!(
        f.count("SELECT COUNT(*) FROM chat_threads WHERE title LIKE '完了: %'"),
        0
    );
}

#[tokio::test]
async fn a_root_done_before_the_first_tick_is_not_replayed() {
    let mut f = fixture(true);
    f.done_task(None);
    f.tick();
    f.tick();
    assert_eq!(f.summary_rows(), 0);
}

#[tokio::test]
async fn without_cos_the_deterministic_fallback_is_notified_once() {
    let mut f = fixture(false);
    f.tick();
    let root = f.done_task(None);
    for _ in 0..3 {
        f.tick();
    }
    let bodies = f.summary_notifications();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].contains("決定的な短文"), "{}", bodies[0]);
    assert!(bodies[0].contains(&format!("http://celeris.test/tasks/{}", root.id)));
    assert_eq!(f.count("SELECT COUNT(*) FROM chat_runs"), 0);
}
