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

/// controller が `enabled` の案件の root の完了では、CoS への指示に「次の root は controller が起票する」を足す
/// （付記 A3 の 9）。`off` の案件の root は従来の文のまま。
#[tokio::test]
async fn project_cos_root_done_tells_the_cos_the_controller_launches() {
    let mut f = fixture(true);
    f.tick();
    let _thread = crate::test_support::project_cos_thread(&f.store, "案件A");
    let project: String = f.count_text("SELECT project_id FROM chat_threads WHERE kind='project'");
    let project_id: task_core::ProjectId = project.parse().expect("project id");
    f.done_task_in(None, Some(project_id));
    f.tick();
    f.join_all().await;
    f.store
        .project_controller_set_mode(
            &project,
            task_core::project_wake::ControllerMode::Enabled,
            None,
            "human:test",
            time::OffsetDateTime::now_utc(),
        )
        .expect("mode");
    f.done_task_in(None, Some(project_id));
    f.tick();
    f.join_all().await;
    let conn = rusqlite::Connection::open(&f.db_path).expect("db");
    let mut stmt = conn
        .prepare(
            "SELECT m.text FROM chat_messages m JOIN root_done_summaries r ON r.input_message_id = m.id \
             ORDER BY m.seq",
        )
        .expect("prepare");
    let texts: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert_eq!(texts.len(), 2, "{texts:?}");
    assert!(!texts[0].contains("project controller"), "{}", texts[0]);
    assert!(
        texts[1].contains("CoS は root を起票しない"),
        "{}",
        texts[1]
    );
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

/// (e) ADR 2026-11-11-cos-fallback-timing D4: the root-done summary follows the same rule as an
/// inbox item. Queued while CoS cannot start (stopped intake), it is not cut at 120 seconds; the
/// deterministic short text goes out once the safety net (1200 s) has passed since it became
/// pickable.
#[tokio::test]
async fn root_done_summary_waits_for_the_safety_net_when_cos_cannot_start() {
    let mut f = fixture(true);
    f.d.accepting_new_work = false;
    f.tick();
    f.done_task(None);
    f.tick();
    f.advance(130);
    f.tick();
    assert!(f.summary_notifications().is_empty(), "no 120-second cut");
    f.advance(1000);
    f.tick();
    assert!(f.summary_notifications().is_empty());
    f.advance(100);
    f.tick();
    let bodies = f.summary_notifications();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].contains("決定的な短文"), "{}", bodies[0]);
}

/// (e) While CoS writes the summary (its run is live) nothing goes out; a run that hangs past the
/// safety net, counted from its start, gets the short text once.
#[tokio::test]
async fn root_done_summary_waits_for_its_live_run_up_to_the_safety_net() {
    let mut f = fixture(true);
    f.tick();
    f.done_task(None);
    f.tick();
    assert_eq!(
        f.count("SELECT COUNT(*) FROM chat_runs WHERE state='running'"),
        1
    );
    f.advance(1100);
    f.tick();
    assert!(f.summary_notifications().is_empty(), "CoS is writing it");
    f.advance(110);
    f.tick();
    f.tick();
    let bodies = f.summary_notifications();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].contains("決定的な短文"), "{}", bodies[0]);
}

/// ADR 2026-11-04-project-controller §A4: a root the general CoS delegated from a conversation
/// (an applied `task.create` operation of the general thread) reports into that general thread,
/// not into its project's thread, with the task id in the facts, so the person goes on from the
/// same place. The facts say 「合格」 only from a `ReviewVerdict`: without one they read
/// 「記録なし」, so the summary cannot claim a pass the reviewer did not give.
#[tokio::test]
async fn general_cos_consult_done_summary_returns_to_the_origin_thread() {
    let mut f = fixture(true);
    f.tick();
    let general = f
        .store
        .cos_general_thread_ensure(OffsetDateTime::now_utc())
        .expect("general");
    let project_thread = crate::test_support::project_cos_thread(&f.store, "案件A");
    let project: String = f.count_text(&format!(
        "SELECT project_id FROM chat_threads WHERE id='{project_thread}'"
    ));
    let project: task_core::ProjectId = project.parse().expect("project id");
    // Two roots of the project: one delegated from the general thread, one not.
    let delegated = f.done_task_in(None, Some(project));
    let reviewed = f.done_task_in(None, Some(project));
    for (task, key) in [(&delegated, "delegate-1"), (&reviewed, "delegate-2")] {
        f.store
            .cos_operation_apply(
                &task_core::chat::AuditContext {
                    actor: task_core::chat::ChatActor::Cos,
                    thread_id: general.clone(),
                    run_id: "run-general".into(),
                    operation_id: ulid::Ulid::new().to_string(),
                    reason: "人の依頼「これを修正して」".into(),
                    policy_version: "5".into(),
                },
                key,
                "hash",
                "task",
                &task.id.to_string(),
                None,
                "task.create",
                &serde_json::json!({}),
                |_, _| Ok(serde_json::json!({"id": task.id.to_string()})),
            )
            .expect("operation");
    }
    f.store
        .append_event(
            reviewed.id,
            &task_core::Event::ReviewVerdict {
                run_id: "review-1".into(),
                criterion_idx: 0,
                pass: true,
                reason: "ok".into(),
            },
        )
        .expect("verdict");
    // A third root, created without the general CoS, still goes to the project thread.
    let plain = f.done_task_in(None, Some(project));
    // Re-read the done transitions written before the operations existed.
    f.store
        .feed_cursor_set(
            crate::dispatcher::cos_chat::root_done::ROOT_DONE_CURSOR,
            "0",
        )
        .expect("rewind");
    // One CoS run at a time (max_cos_runs = 1): each queued summary input gets its turn.
    for _ in 0..6 {
        f.tick();
        f.join_all().await;
    }
    let thread_of = |task: &Task| {
        f.count_text(&format!(
            "SELECT thread_id FROM root_done_summaries WHERE task_id='{}'",
            task.id
        ))
    };
    assert_eq!(thread_of(&delegated), general);
    assert_eq!(thread_of(&reviewed), general);
    assert_eq!(thread_of(&plain), project_thread);
    let facts = |task: &Task| {
        f.count_text(&format!(
            "SELECT text FROM chat_messages WHERE thread_id='{general}' AND role='user' \
             AND text LIKE '%{}%'",
            task.id
        ))
    };
    let delegated_facts = facts(&delegated);
    assert!(
        delegated_facts.contains("受け入れ判定: 記録なし"),
        "{delegated_facts}"
    );
    assert!(!delegated_facts.contains("合格"), "{delegated_facts}");
    let reviewed_facts = facts(&reviewed);
    assert!(reviewed_facts.contains("0:合格"), "{reviewed_facts}");
    // The summary run answered in the general thread (one reply per delegated root).
    let replies = f.count(&format!(
        "SELECT COUNT(*) FROM chat_messages WHERE thread_id='{general}' AND role='assistant'"
    ));
    assert_eq!(replies, 2);
    if let Ok(dir) = std::env::var("CELERIS_GCC_E2E_OUT") {
        let row = serde_json::json!({
            "schema": "celeris.general-cos-consult/1",
            "test": "general_cos_consult_done_summary_returns_to_the_origin_thread",
            "llm_launches": 0,
            "task_id": delegated.id.to_string(),
            "origin_thread": general,
            "summary_thread": thread_of(&delegated),
            "summary_in_origin": thread_of(&delegated) == general,
            "summary_mentions_task": delegated_facts.contains(&delegated.id.to_string()),
            "summary_says_pass_without_verdict": delegated_facts.contains("合格"),
            "plain_root_thread_is_project": thread_of(&plain) == project_thread,
            "origin_replies": replies,
        });
        std::fs::write(
            std::path::Path::new(&dir)
                .join("general_cos_consult_done_summary_returns_to_the_origin_thread.jsonl"),
            format!("{row}\n"),
        )
        .expect("raw record");
    }
}

fn probe_value(parts: &[(&'static str, u64)], name: &str) -> Option<u64> {
    parts.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// ADR 2026-11-07-root-done-tick-phase-timing D3/D4: `cos.root_done.*` の内訳は、root_done_tick が閾値を
/// 超えた pass だけ tick_cos_chat_launch の内訳と `TickReport::step_us`（slow tick の警告が出す列）に載る。
/// 閾値は試験で 0 と最大に差し替え、時計・sleep・負荷は使わない。
#[tokio::test]
async fn root_done_probe_parts_reach_the_tick_steps_only_over_the_threshold() {
    let mut f = fixture(false);
    f.tick(); // 導入: cursor は現在の末尾から
    let set_threshold = |f: &mut Fixture, t: std::time::Duration| {
        f.d.cos_chat_launch
            .as_mut()
            .expect("launch")
            .root_done_probe_threshold = t;
    };
    // 閾値を超えない pass には足さない（通常の tick は今までと同じ列）。
    set_threshold(&mut f, std::time::Duration::MAX);
    f.done_task(None);
    f.advance(1);
    let parts = f.d.tick_cos_chat_launch();
    assert!(
        parts.iter().all(|(n, _)| !n.starts_with("cos.root_done.")),
        "{parts:?}"
    );
    assert!(probe_value(&parts, "cos.root_done_tick").is_some());
    assert_eq!(f.summary_rows(), 1);

    // 閾値 0: 取り込みと前進の段が全部出る。
    set_threshold(&mut f, std::time::Duration::ZERO);
    f.done_task(None);
    f.advance(1);
    let parts = f.d.tick_cos_chat_launch();
    assert_eq!(f.summary_rows(), 2);
    for name in [
        "cos.root_done.ingest_us",
        "cos.root_done.cursor_get.wait_us",
        "cos.root_done.cursor_get.work_us",
        "cos.root_done.events_since.wait_us",
        "cos.root_done.events_since.work_us",
        "cos.root_done.candidate_get.work_us",
        "cos.root_done.candidate_events_for.work_us",
        "cos.root_done.insert.work_us",
        "cos.root_done.cursor_set.work_us",
        "cos.root_done.advance_us",
        "cos.root_done.open.wait_us",
        "cos.root_done.open.work_us",
        "cos.root_done.facts.get.work_us",
        "cos.root_done.facts.events_for.work_us",
        "cos.root_done.facts.delivery_get.work_us",
        "cos.root_done.facts.children.work_us",
        "cos.root_done.facts.progress_us",
        "cos.root_done.facts.row_max_us",
        "cos.root_done.advance_other_us",
    ] {
        assert!(
            probe_value(&parts, name).is_some(),
            "{name} missing: {parts:?}"
        );
    }
    // 件数: この pass で読んだ event は done 遷移を含み、done の root 1 件を候補にして 1 行入れた。
    assert!(probe_value(&parts, "cos.root_done.events_since_rows").expect("rows") >= 1);
    assert_eq!(
        probe_value(&parts, "cos.root_done.events_since_calls"),
        Some(1)
    );
    assert_eq!(probe_value(&parts, "cos.root_done.candidate_rows"), Some(1));
    assert!(probe_value(&parts, "cos.root_done.candidate_events_for_max").expect("max") >= 1);
    assert_eq!(probe_value(&parts, "cos.root_done.insert_calls"), Some(1));
    // CoS 無効: 新しい行は同じ pass で事実を作って fallback へ進む（open の行は 1、事実づくり 1 回）。
    assert_eq!(probe_value(&parts, "cos.root_done.open_rows"), Some(1));
    assert_eq!(probe_value(&parts, "cos.root_done.facts_calls"), Some(1));
    assert!(probe_value(&parts, "cos.root_done.facts.events_for_max").expect("max") >= 1);
    // delivery が無いので進捗ファイルは読まず、git は起こさない。
    assert_eq!(
        probe_value(&parts, "cos.root_done.facts.progress_git_calls"),
        Some(0)
    );

    // 通常の tick でも同じ項目が `step_us` に載る（"slow tick phases" はこの列を出す）。
    f.done_task(None);
    f.advance(1);
    let report = f.d.tick().expect("tick");
    assert_eq!(
        probe_value(&report.step_us, "cos.root_done.candidate_rows"),
        Some(1),
        "{:?}",
        report.step_us
    );
    assert!(probe_value(&report.step_us, "tick_cos_chat_launch").is_some());
}

/// `git -C <dir> <args...>` の stdout（trim 済み）。commit の作者は引数で渡す。失敗したら panic。
fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// ADR 2026-11-07-root-done-tick-fix の試験の 1 回分: 進捗ファイルを `fillers + 2` 件持つ repo と、Reviewing の
/// delivery を持つ root の new 行を作り、待ちの pass を 3 回、delivery を Ready にした後の pass を 1 回まわす。
/// 返すのは待ちの pass の内訳 3 つと、待ちが終わった pass の内訳。
fn root_done_row_waiting_on_delivery(
    fillers: usize,
) -> (Vec<Vec<(&'static str, u64)>>, Vec<(&'static str, u64)>) {
    use task_core::DeliveryStore;
    let mut f = fixture(false);
    f.tick(); // 導入: cursor は現在の末尾から
    f.d.cos_chat_launch
        .as_mut()
        .expect("launch")
        .root_done_probe_threshold = std::time::Duration::ZERO;
    crate::test_support::project_cos_thread(&f.store, "案件A");
    let project: String = f.count_text("SELECT project_id FROM chat_threads WHERE kind='project'");
    let project: task_core::ProjectId = project.parse().expect("project id");
    let root = f.done_task_in(None, Some(project));

    // 進捗ファイル: 別の task の file を `fillers` 件（名前順で前）、本文だけでこの id に触れる file、
    // 名前順の最後にこの task の file（『本番への移行』に中身がある）。
    let repo = f.dir.path().join("repo");
    let progress = repo.join("agent-docs/progress");
    std::fs::create_dir_all(&progress).expect("progress dir");
    let other = TaskId::new();
    for i in 0..fillers {
        std::fs::write(
            progress.join(format!("2026-01-01-filler-{i:03}.md")),
            format!("---\ntasks: [{other}]\n---\n# filler {i}\n"),
        )
        .expect("filler");
    }
    std::fs::write(
        progress.join("2026-06-01-mentions.md"),
        format!(
            "---\ntasks: [{other}]\n---\n# 前の task {} の続き\n",
            root.id
        ),
    )
    .expect("mentions");
    std::fs::write(
        progress.join("2026-12-31-root.md"),
        format!(
            "---\ntitle: root\ntasks: [{}]\n---\n# root\n## 本番への移行\n- 本番 config に欄を足す\n",
            root.id
        ),
    )
    .expect("root progress");
    git_in(&repo, &["init", "-q"]);
    git_in(&repo, &["add", "-A"]);
    git_in(&repo, &["commit", "-q", "-m", "progress"]);
    let head = git_in(&repo, &["rev-parse", "HEAD"]);
    let repo_row = task_core::ProjectRepo {
        id: task_core::RepoId::new(),
        project_id: project,
        name: "r".into(),
        kind: task_core::RepoKind::Git,
        location: task_core::WorkspaceSpec::local(repo.clone()),
        default_branch: Some("main".into()),
        sync: None,
        run: task_core::RepoRun::Auto,
        auto_integrate: false,
        is_primary: true,
        created_at: OffsetDateTime::now_utc(),
    };
    f.store.repo_create(&repo_row).expect("repo");
    let delivery = task_core::Delivery {
        task_id: root.id,
        project_id: project,
        repo_id: repo_row.id,
        repo: "r".into(),
        branch: format!("celeris/{}", root.id),
        base: head.clone(),
        head: head.clone(),
        target_sha: None,
        reviewed_sha: None,
        merge_candidate_sha: None,
        default_branch: "main".into(),
        department: "software-engineering".into(),
        review_run: "review".into(),
        worker_run: "worker".into(),
        criterion_idx: 0,
        decision: None,
        state: task_core::DeliveryState::Reviewing,
        detail: String::new(),
        release: None,
        prepare_pid: None,
        notification: None,
        pushed_at: None,
        push_error: None,
    };
    assert!(f.store.delivery_save(None, &delivery).expect("delivery"));

    let mut waiting = Vec::new();
    for _ in 0..3 {
        f.advance(1);
        waiting.push(f.d.tick_cos_chat_launch());
        assert_eq!(
            f.count_text("SELECT state FROM root_done_summaries"),
            "new".to_string()
        );
    }
    let ready = task_core::Delivery {
        state: task_core::DeliveryState::Ready,
        ..delivery.clone()
    };
    assert!(
        f.store
            .delivery_save(Some(&delivery), &ready)
            .expect("ready")
    );
    f.advance(1);
    let after = f.d.tick_cos_chat_launch();
    assert_eq!(
        f.count_text("SELECT state FROM root_done_summaries"),
        "fallback".to_string()
    );
    // 名前順で最後の、この task の進捗ファイルを読んだ（『本番への移行』に中身がある）。
    let bodies = f.summary_notifications();
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].contains("本番への移行: あり"), "{}", bodies[0]);
    (waiting, after)
}

/// ADR 2026-11-07-root-done-tick-fix D1/D2/D4: delivery の待ち（Reviewing）の間、new の行は delivery を 1 回
/// 読むだけで事実を作らず、git を起こさない。待ちが終わった pass に事実を 1 回作り、git は grep 1 回 +
/// 候補の show 1 回。進捗ファイルが 3 件でも 60 件でも、待ちの pass が何回でも回数は同じ（file 数・tick 数に
/// 比例しない）。時間は試験の時計だけを進め、sleep・負荷は使わない。
#[tokio::test]
async fn root_done_row_waiting_on_delivery_skips_facts_and_reads_progress_with_two_git_calls() {
    let expect_waiting = [
        ("cos.root_done.open_rows", 1),
        ("cos.root_done.delivery_wait_rows", 1),
        ("cos.root_done.facts.delivery_get_calls", 1),
        ("cos.root_done.facts_calls", 0),
        ("cos.root_done.facts.progress_git_calls", 0),
    ];
    let expect_after = [
        ("cos.root_done.open_rows", 1),
        ("cos.root_done.delivery_wait_rows", 0),
        ("cos.root_done.facts.delivery_get_calls", 1),
        ("cos.root_done.facts_calls", 1),
        ("cos.root_done.facts.progress_git_calls", 2),
    ];
    for fillers in [1, 58] {
        let (waiting, after) = root_done_row_waiting_on_delivery(fillers);
        for parts in &waiting {
            for (name, value) in expect_waiting {
                assert_eq!(
                    probe_value(parts, name),
                    Some(value),
                    "{fillers} fillers, waiting: {name}: {parts:?}"
                );
            }
        }
        for (name, value) in expect_after {
            assert_eq!(
                probe_value(&after, name),
                Some(value),
                "{fillers} fillers, after: {name}: {after:?}"
            );
        }
    }
}
