//! ADR-0079 付記「R7-9」: 統合済みの段階に unit が増えたら段階の統合をやり直す。本番の root task
//! 01M3PAX6RVE7AX8Z6118KADME3（replan v7 / v8 が統合済みの段階 `phase-4` / `phase-4-inject` に `land2` / `gaps` /
//! `closeout` を足したが、done の統合 WU はそのまま持ち越され、`land2` の done でそのまま最終レビューに出た。root の
//! ブランチは 99d5d0bf のまま）の再現。実 git の一時リポジトリと偽のアダプタだけで、外部ネットワークに出ない。

use super::tree::assert_replay_is_clean;
use super::*;

/// `(key, merged keys with skipped)` of every `PhaseIntegrated` for `phase`, in order.
fn integrations_of(
    store: &Arc<dyn TaskStore>,
    id: TaskId,
    phase: &str,
) -> Vec<Vec<(String, bool)>> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::PhaseIntegrated {
                phase: p, merged, ..
            } if p == phase => Some(merged.into_iter().map(|m| (m.key, m.skipped)).collect()),
            _ => None,
        })
        .collect()
}

/// `(key, reason)` of every `WorkUnitTransitioned{from: done, to: pending}`.
fn reopen_events(store: &Arc<dyn TaskStore>, id: TaskId) -> Vec<(String, String)> {
    events_of(store, id)
        .into_iter()
        .filter_map(|e| match e {
            Event::WorkUnitTransitioned {
                key,
                from: task_core::WorkUnitStatus::Done,
                to: task_core::WorkUnitStatus::Pending,
                reason,
                ..
            } => Some((key, reason)),
            _ => None,
        })
        .collect()
}

/// D2: 段階 s1（a）の統合の後、s2 の b が失敗して planner の replan（差分）が **統合済みの s1** に a2 を足す。
/// 修正後は `integrate-s1` が `pending` に戻り（`replan v2: stage_reopened`）、a2 の done の後に s1 の統合が
/// もう一度走って a2 のブランチを task のブランチに入れ、その後で s2 に進む。最終レビューの check
/// （a.txt・a2.txt・b.txt が task の worktree にある）が通って done。修正前は a2 が merge されず、check が落ちる。
#[tokio::test]
async fn a_replan_adding_a_unit_to_an_integrated_stage_reintegrates_that_stage() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(
        repo.path(),
        "test -f a.txt && test -f a2.txt && test -f b.txt",
    );
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["s1", "s2"],
        vec![v2_wu("a", "s1", &[]), v2_wu("b", "s2", &[])],
    );
    let delta = serde_json::json!({
        "schema": task_core::execution_plan::EXECUTION_PLAN_DELTA_SCHEMA,
        "base_version": 1,
        "rationale": "b needs a2 in s1 first (the planner assumes integrate-s1 runs again)",
        "add": [serde_json::to_value(v2_wu("a2", "s1", &[])).unwrap()],
        "modify": [{"key": "b", "objective": "do b, this time after a2 landed"}]
    })
    .to_string();
    let boom = || Terminal::Error {
        message: "b cannot work without a2".into(),
        retryable: true,
    };
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(5))
            .with_file("a", "a.txt", "a")
            .with_file("a2", "a2.txt", "a2")
            .with_file("b", "b.txt", "b")
            .with_script("b", vec![boom(), boom(), boom(), boom()])
            .with_planner_output(delta),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    d.config.execution.planner.adapter = "instant".to_string();
    run_until_idle(&mut d, 1500).await;

    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(
        t.status,
        Status::Done,
        "{:?}",
        events_of(&store, task.id)
            .iter()
            .filter(|e| matches!(e, Event::WorkerFinished { .. } | Event::Transitioned { .. }))
            .collect::<Vec<_>>()
    );
    assert_eq!(store.execution_plan_list(task.id).unwrap().len(), 2);
    assert_eq!(
        reopen_events(&store, task.id),
        vec![(
            "integrate-s1".to_string(),
            "replan v2: stage_reopened".to_string()
        )]
    );
    // s1 の統合は 2 回: 1 回目は a、2 回目で a2 が merge される（a は既に入っているので skipped）。
    let s1 = integrations_of(&store, task.id, "s1");
    assert_eq!(s1.len(), 2, "{s1:?}");
    assert!(s1[1].contains(&("a2".to_string(), false)), "{s1:?}");
    let task_branch = format!("celeris/{}", task.id);
    let a2_branch = format!("celeris-wu/{}/a2", task.id);
    assert!(git_ok(
        repo.path(),
        &["merge-base", "--is-ancestor", &a2_branch, &task_branch]
    ));
    let units = store.work_units_for(task.id).unwrap();
    let integ = units.iter().find(|u| u.key == "integrate-s1").unwrap();
    assert_eq!(integ.status, task_core::WorkUnitStatus::Done);
    assert_eq!(integ.depends_on, vec!["a", "a2"]);
    assert!(task_core::stale_stage_integrations(&units).is_empty());
    assert_replay_is_clean(&store);
}

/// 修正前の replan が書いた形を store に直接書く（本番の v7 / v8 と同じ events）: 新しい版の `ExecutionPlanned` と
/// 新しい unit の行だけを足し、done の統合 WU には触れない。足した unit は done で、自分のブランチに commit を持つ。
fn pre_fix_replan_adding_a_done_unit(
    store: &Arc<dyn TaskStore>,
    repo: &std::path::Path,
    task_id: TaskId,
    key: &str,
    phase: &str,
    file: &str,
) -> String {
    let active = store.execution_plan_active(task_id).unwrap().unwrap();
    let mut spec = active.spec.clone();
    spec.rationale = format!("pre-R7-9 replan adds {key} to the integrated stage {phase}");
    spec.work_units.push(v2_wu(key, phase, &[]));
    let validated = task_core::validate(&spec, task_core::ExecutionLimits::default(), &[]).unwrap();
    let plan_id = task_core::new_id();
    let created_at = rfc3339(OffsetDateTime::now_utc());
    let rows = task_core::materialize_work_units(
        &task_id.to_string(),
        &plan_id,
        &validated.spec,
        &validated.topological_order,
        &created_at,
        &mut |_| task_core::new_id(),
    );
    let mut row = rows.into_iter().find(|r| r.key == key).unwrap();
    row.status = task_core::WorkUnitStatus::Ready;
    let new_plan = task_core::ExecutionPlanRow {
        id: plan_id.clone(),
        task_id: task_id.to_string(),
        version: active.version + 1,
        origin: task_core::PlanOrigin::Planner,
        planner_run_id: None,
        status: task_core::PlanStatus::Active,
        spec: validated.spec.clone(),
        created_at: created_at.clone(),
        superseded_at: None,
    };
    store
        .execution_plan_replan(
            task_id,
            active.id.clone(),
            new_plan,
            Vec::new(),
            vec![row.clone()],
            vec![Event::PausePointsResolved {
                plan_id: plan_id.clone(),
                phases: Vec::new(),
                source: Default::default(),
            }],
            Event::ExecutionPlanned {
                plan_id,
                version: active.version + 1,
                origin: task_core::PlanOrigin::Planner,
                supersedes: Some(active.id),
                reason: Some("replan (planner run) (added=1, changed=0, removed=0)".into()),
                plan: Box::new(validated.spec),
            },
        )
        .unwrap();
    // 足した unit の仕事: task のブランチの先から切ったブランチに 1 commit（本番の land2 の 68323b11）。
    let task_branch = format!("celeris/{task_id}");
    let branch = format!("celeris-wu/{task_id}/{key}");
    let wt = tempfile::tempdir().unwrap();
    let wt_path = wt.path().join(key);
    git_out(
        repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &branch,
            wt_path.to_str().unwrap(),
            &task_branch,
        ],
    );
    let base = git_out(&wt_path, &["rev-parse", "HEAD"]);
    std::fs::write(wt_path.join(file), key).unwrap();
    git_out(&wt_path, &["add", "-A"]);
    git_out(&wt_path, &["commit", "-q", "-m", &format!("wu/{key}")]);
    let head = git_out(&wt_path, &["rev-parse", "HEAD"]);
    git_out(
        repo,
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
    );
    let mut done = row.clone();
    done.status = task_core::WorkUnitStatus::Done;
    done.branch = Some(branch.clone());
    done.base_commit = Some(base.clone());
    done.head_commit = Some(head.clone());
    store
        .work_unit_transition(
            task_id,
            done,
            Event::WorkUnitTransitioned {
                work_unit_id: row.id.clone(),
                key: key.to_string(),
                from: task_core::WorkUnitStatus::Ready,
                to: task_core::WorkUnitStatus::Done,
                reason: "completed".into(),
                run_id: None,
            },
        )
        .unwrap();
    store
        .append_event(
            task_id,
            &Event::WorkUnitCommitted {
                work_unit_id: row.id,
                key: key.to_string(),
                branch,
                base: Some(base),
                commit: head.clone(),
            },
        )
        .unwrap();
    head
}

/// D3 / D5（本番の task の救済）: 修正前の replan が統合済みの段階 s1 に `late` を足し、`late` は done（統合 WU の依存に
/// 無い。本番の `land2` / `gaps` / `closeout`）。task は終端（本番は 3 回目の review_fail で `failed`、ここでは done）。
/// 人が `POST /tasks/{id}/reopen` すると、最初の dispatch の gate が `integrate-s1` を `pending` に戻し
/// （`stage_reopened`、依存に `late` を足す）、s1 の統合が `late` のブランチを task のブランチに merge して check を
/// 走らせ、その後で最終レビューに出る（check は late.txt を要求する）。done の unit は走り直さない。replay も同じ行。
#[tokio::test]
async fn reopening_a_task_whose_integrated_stage_gained_units_merges_them_before_review() {
    let repo = tempfile::tempdir().unwrap();
    init_test_repo(repo.path());
    let root = tempfile::tempdir().unwrap();
    let marker_dir = tempfile::tempdir().unwrap();
    let marker = marker_dir.path().join("expect-late");
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = parallel_task(
        repo.path(),
        &format!(
            "test -f a.txt && test -f b.txt && {{ test ! -e {m} || test -f late.txt; }}",
            m = marker.display()
        ),
    );
    store.insert(&task).unwrap();
    adopt_v2_plan(
        &store,
        task.id,
        &["s1", "s2"],
        vec![v2_wu("a", "s1", &[]), v2_wu("b", "s2", &[])],
    );
    let adapter = Arc::new(
        ParallelWuAdapter::new(Duration::from_millis(5))
            .with_file("a", "a.txt", "a")
            .with_file("b", "b.txt", "b"),
    );
    let mut d = parallel_dispatcher(store.clone(), adapter.clone(), root.path(), 3, 3, 3);
    run_until_idle(&mut d, 800).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
    let task_branch = format!("celeris/{}", task.id);
    let before = git_out(repo.path(), &["rev-parse", &task_branch]);

    let late_head =
        pre_fix_replan_adding_a_done_unit(&store, repo.path(), task.id, "late", "s1", "late.txt");
    // 修正前の形: 生きた行はすべて done（計画の仕事は残っていないように見える）が、late は統合されていない。
    let units = store.work_units_for(task.id).unwrap();
    assert!(task_core::plan_work_finished(&units));
    let integ = units.iter().find(|u| u.key == "integrate-s1").unwrap();
    assert_eq!(integ.depends_on, vec!["a"]);
    assert_eq!(
        task_core::stale_stage_integrations(&units),
        vec![(integ.id.clone(), vec!["late".to_string()])]
    );
    assert!(!git_ok(
        repo.path(),
        &["merge-base", "--is-ancestor", &late_head, &task_branch]
    ));
    assert_replay_is_clean(&store);

    // 人の操作: reopen（retry は task を複製して計画も unit も捨てるので使わない）。
    std::fs::write(&marker, "1").unwrap();
    let reopened = task_ops::comment::reopen(store.as_ref(), task.id, None).unwrap();
    assert_eq!(reopened.to, Status::Ready);
    let runs_before = adapter.keys_seen().len();
    run_until_idle(&mut d, 800).await;

    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!(
        t.status,
        Status::Done,
        "{:?}",
        events_of(&store, task.id)
            .iter()
            .filter(|e| matches!(
                e,
                Event::WorkerFinished { .. }
                    | Event::Transitioned { .. }
                    | Event::WorkUnitTransitioned { .. }
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        adapter.keys_seen().len(),
        runs_before,
        "done の unit は走り直さない（統合は LLM run を起こさない）"
    );
    assert_eq!(
        reopen_events(&store, task.id),
        vec![("integrate-s1".to_string(), "stage_reopened".to_string())]
    );
    let s1 = integrations_of(&store, task.id, "s1");
    assert_eq!(s1.len(), 2, "{s1:?}");
    assert!(s1[1].contains(&("late".to_string(), false)), "{s1:?}");
    assert!(git_ok(
        repo.path(),
        &["merge-base", "--is-ancestor", &late_head, &task_branch]
    ));
    assert!(git_ok(
        repo.path(),
        &["merge-base", "--is-ancestor", &before, &task_branch]
    ));
    let units = store.work_units_for(task.id).unwrap();
    let integ = units.iter().find(|u| u.key == "integrate-s1").unwrap();
    assert_eq!(integ.status, task_core::WorkUnitStatus::Done);
    assert_eq!(integ.depends_on, vec!["a", "late"]);
    assert_eq!(
        integ.integrated_commit.as_deref(),
        Some(git_out(repo.path(), &["rev-parse", &task_branch]).as_str())
    );
    // 統合の後の最終レビュー（`worker_done` → `review_pass`）。
    let reasons: Vec<String> = events_of(&store, task.id)
        .into_iter()
        .filter_map(|e| match e {
            Event::Transitioned { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    let after_reopen: Vec<&str> = reasons
        .iter()
        .skip_while(|r| r.as_str() != "reopen")
        .map(String::as_str)
        .collect();
    assert!(after_reopen.contains(&"worker_done"), "{after_reopen:?}");
    assert_replay_is_clean(&store);
}
