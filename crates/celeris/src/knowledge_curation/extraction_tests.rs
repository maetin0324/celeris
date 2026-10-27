use super::*;
use task_core::KnowledgeRunStore;

fn source(env: &Env) -> Task {
    let spec = serde_json::from_value(serde_json::json!({"title":"Reusable fact", "objective":"Implement", "acceptance":[], "assignee":"coding"})).unwrap();
    let t = task_ops::add::create_support_task(&env.store, spec, &[], &[], env.t0).unwrap();
    for trigger in [Trigger::Dispatch, Trigger::WorkerDone, Trigger::ReviewPass] {
        env.store.apply_transition(t.id, trigger, None).unwrap();
    }
    t
}

fn pending(env: &Env) -> Task {
    env.store
        .list(None)
        .unwrap()
        .into_iter()
        .find(curation::is_daily_extraction)
        .unwrap()
}

pub(super) fn complete_empty(env: &Env) {
    complete(env, &pending(env), serde_json::json!({"candidates":[]}));
}

fn complete(env: &Env, task: &Task, output: serde_json::Value) {
    std::fs::write(
        env.artifacts(task).join("knowledge-candidates.json"),
        output.to_string(),
    )
    .unwrap();
    for trigger in [Trigger::Dispatch, Trigger::WorkerDone, Trigger::ReviewPass] {
        env.store.apply_transition(task.id, trigger, None).unwrap();
    }
}

fn candidate(id: TaskId) -> serde_json::Value {
    serde_json::json!({"op":"create","path":"environment/tools/reusable.md","title":"Reusable fact", "tags":["testing"], "scope":"environment", "body":"Use an injected clock in timing tests.", "sources":[format!("task:{id}")],"confidence":"high"})
}

#[test]
fn daily_extraction_precedes_snapshot_and_is_recovered_without_duplicate_candidates() {
    let env = setup("dry_run");
    env.git_init(Remote::None);
    let original = source(&env);
    std::fs::write(
        env.artifacts(&original).join("result.json"),
        r#"{"summary":"Result excerpt"}"#,
    )
    .unwrap();
    env.store
        .comment_add(
            &task_core::comment::TaskComment {
                id: Default::default(),
                task_id: original.id,
                author_kind: task_core::comment::CommentAuthorKind::Human,
                author: None,
                body: "Remember this durable rule".into(),
                run_id: None,
                created_at: env.t0,
            },
            None,
        )
        .unwrap();
    let now = env.now(50);
    let task = env.fire(now);
    assert!(!curation::inputs_ready(&env.store, &task).unwrap());
    assert!(env.tick(now).prepared.is_empty());
    let extraction = pending(&env);
    assert_eq!(
        extraction.harness_id.as_ref().map(|h| h.as_ref()),
        Some("langmem")
    );
    assert!(extraction.objective.contains(&original.id.to_string()));
    assert!(extraction.objective.contains("Result excerpt"));
    assert!(extraction.objective.contains("Remember this durable rule"));
    assert!(!env.dir(&task).join("inputs/kb").exists());
    // Fresh tick reloads state from durable records, including a task created before claims.
    assert!(env.tick(now).prepared.is_empty());
    assert_eq!(pending(&env).id, extraction.id);
    complete(
        &env,
        &extraction,
        serde_json::json!({"candidates":[candidate(original.id)]}),
    );
    assert_eq!(env.tick(now).prepared, vec![task.id]);
    assert!(curation::inputs_ready(&env.store, &task).unwrap());
    let items = task_ops::knowledge::inbox_list(env.kb.path());
    assert_eq!(
        items.len(),
        1,
        "{:?}",
        extraction::summary(&env.store, &task).unwrap()
    );
    assert!(
        !env.kb.path().join("environment/tools/reusable.md").exists(),
        "high confidence still goes to inbox"
    );
    assert_eq!(
        task_ops::knowledge::inbox_list(&env.dir(&task).join("inputs/kb")).len(),
        1
    );
    // Simulate interruption after KB writes but before finishing the SQLite record.
    let input: serde_json::Value = serde_json::from_str(&extraction.objective).unwrap();
    assert_eq!(input["reports"].as_array().unwrap().len(), 1);
    let c: task_core::knowledge::Candidate =
        serde_json::from_value(candidate(original.id)).unwrap();
    task_ops::knowledge::apply_candidates_with_policy(
        env.kb.path(),
        &extraction.id.to_string(),
        &[c],
        task_ops::knowledge::ApplyPolicy::Inbox,
    );
    assert_eq!(task_ops::knowledge::inbox_list(env.kb.path()).len(), 1);
    assert!(env.store.knowledge_run_exists(original.id).unwrap());
}

#[test]
fn failed_extraction_does_not_block_curation_and_reaches_summary_and_inbox() {
    let env = setup("dry_run");
    source(&env);
    let now = env.now(50);
    let task = env.fire(now);
    env.tick(now);
    let extraction = pending(&env);
    for trigger in [Trigger::Dispatch, Trigger::WorkerError { retryable: false }] {
        env.store
            .apply_transition(extraction.id, trigger, None)
            .unwrap();
    }
    assert_eq!(env.tick(now).prepared, vec![task.id]);
    assert!(curation::inputs_ready(&env.store, &task).unwrap());
    let plan = CurationPlan {
        version: 1,
        kb: vec![],
        inbox: vec![],
        human_decisions: vec![],
    };
    env.worker_done(&task, &plan, None);
    env.tick(now);
    assert!(env.reports(&task)[0].body.contains("抽出が完了しなかった"));
    let decision = open_or_answered(&env.store, &task, HUMAN_KEY)
        .unwrap()
        .unwrap();
    assert_eq!(decision.status, DecisionStatus::Open);
    assert!(
        crate::knowledge_maint::retry_failed(
            &env.store,
            env.kb.path(),
            true,
            2,
            None,
            &[],
            &[],
            now
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn malformed_missing_unattributed_and_oversized_outputs_are_visible_failures() {
    for output in [
        serde_json::json!({}),
        serde_json::json!({"candidates":[candidate(TaskId::new())]}),
        serde_json::json!({"candidates":vec![candidate(TaskId::new());41]}),
    ] {
        let env = setup("dry_run");
        source(&env);
        let now = env.now(50);
        let task = env.fire(now);
        env.tick(now);
        complete(&env, &pending(&env), output);
        assert_eq!(env.tick(now).prepared, vec![task.id]);
        assert_eq!(task_ops::knowledge::inbox_list(env.kb.path()).len(), 0);
        assert!(
            !extraction::summary(&env.store, &task)
                .unwrap()
                .unwrap()
                .discarded_reasons
                .is_empty()
        );
    }
}

#[test]
fn stuck_extraction_has_a_deterministic_deadline() {
    let env = setup("dry_run");
    source(&env);
    let now = env.now(50);
    let task = env.fire(now);
    env.tick(now);
    assert_eq!(
        env.tick(now + time::Duration::minutes(30)).prepared,
        vec![task.id]
    );
    assert!(
        !extraction::summary(&env.store, &task)
            .unwrap()
            .unwrap()
            .discarded_reasons
            .is_empty()
    );
}

#[test]
fn repeating_backfill_day_excludes_claimed_source_tasks() {
    let env = setup("dry_run");
    let original = source(&env);
    let now = env.now(50);
    let task = env.fire(now);
    env.tick(now);
    complete_empty(&env);
    env.tick(now);
    let mut repeated = task.clone();
    repeated.id = TaskId::new();
    env.store.insert(&repeated).unwrap();
    let result = extraction::advance(
        &env.store,
        env.kb.path(),
        &env.roots(),
        &repeated,
        env.t0,
        now,
        now,
    )
    .unwrap()
    .unwrap();
    assert_eq!(result.candidates, 0);
    assert!(env.store.knowledge_run_exists(original.id).unwrap());
    assert_eq!(
        env.store
            .list(None)
            .unwrap()
            .iter()
            .filter(|t| curation::is_daily_extraction(t))
            .count(),
        1
    );
}

#[test]
fn backfill_batches_ten_sources_and_continues_the_same_utc_day() {
    let env = setup("dry_run");
    let sources: Vec<_> = (0..12).map(|_| source(&env)).collect();
    let date = env
        .store
        .get(sources[0].id)
        .unwrap()
        .unwrap()
        .updated_at
        .date();
    let now = env.now(50);
    let mut task = env.fire(now);
    task.labels.push(format!("extraction-date-{date}"));
    // The supplied ordinary interval excludes the sources; the date label must override it.
    let run = |t: &Task| {
        extraction::advance(&env.store, env.kb.path(), &env.roots(), t, now, now, now).unwrap()
    };
    assert!(run(&task).is_none());
    let batch = pending(&env);
    let input: serde_json::Value = serde_json::from_str(&batch.objective).unwrap();
    assert_eq!(input["reports"].as_array().unwrap().len(), 10);
    complete(&env, &batch, serde_json::json!({"candidates":[]}));
    assert!(run(&task).is_some());
    task.id = TaskId::new();
    env.store.insert(&task).unwrap();
    assert!(run(&task).is_none());
    let batch2 = env
        .store
        .list(None)
        .unwrap()
        .into_iter()
        .find(|t| curation::is_daily_extraction(t) && t.id != batch.id)
        .unwrap();
    let input: serde_json::Value = serde_json::from_str(&batch2.objective).unwrap();
    assert_eq!(input["reports"].as_array().unwrap().len(), 2);
    complete(&env, &batch2, serde_json::json!({"candidates":[]}));
    assert!(run(&task).is_some());
    task.id = TaskId::new();
    env.store.insert(&task).unwrap();
    assert!(run(&task).is_some());
    assert_eq!(
        env.store
            .list(None)
            .unwrap()
            .iter()
            .filter(|t| curation::is_daily_extraction(t))
            .count(),
        2
    );
}

#[test]
fn backfill_interval_includes_midnight_and_excludes_next_midnight() {
    let env = setup("dry_run");
    let mut task = env.fire(env.now(50));
    task.labels.push("extraction-date-2026-10-03".into());
    let (since, until) = extraction::interval(&task, env.t0, env.now(50)).unwrap();
    assert_eq!(rfc3339(since), "2026-10-02T23:59:59.999999999Z");
    assert_eq!(rfc3339(until), "2026-10-03T23:59:59.999999999Z");
}

#[test]
fn full_inbox_defers_backfill_without_claiming_sources() {
    let env = setup("dry_run");
    let original = source(&env);
    let now = env.now(50);
    let mut task = env.fire(now);
    let date = env
        .store
        .get(original.id)
        .unwrap()
        .unwrap()
        .updated_at
        .date();
    task.labels.push(format!("extraction-date-{date}"));
    std::fs::create_dir(env.kb.path().join("_inbox")).unwrap();
    for i in 0..40 {
        std::fs::write(env.kb.path().join(format!("_inbox/{i}.md")), PAGE_A).unwrap();
    }
    assert_eq!(task_ops::knowledge::inbox_list(env.kb.path()).len(), 40);
    let outcome = extraction::advance(
        &env.store,
        env.kb.path(),
        &env.roots(),
        &task,
        env.t0,
        now,
        now,
    )
    .unwrap()
    .unwrap();
    assert!(!outcome.discarded_reasons.is_empty());
    assert!(!env.store.knowledge_run_exists(original.id).unwrap());
    assert!(
        !env.store
            .list(None)
            .unwrap()
            .iter()
            .any(curation::is_daily_extraction)
    );
}

#[test]
fn failed_sources_require_an_explicit_rerun_before_another_extraction() {
    let env = setup("dry_run");
    let original = source(&env);
    let now = env.now(50);
    let mut task = env.fire(now);
    env.tick(now);
    let batch = pending(&env);
    for trigger in [Trigger::Dispatch, Trigger::WorkerError { retryable: false }] {
        env.store.apply_transition(batch.id, trigger, None).unwrap();
    }
    env.tick(now);
    task.id = TaskId::new();
    env.store.insert(&task).unwrap();
    assert!(
        extraction::advance(
            &env.store,
            env.kb.path(),
            &env.roots(),
            &task,
            env.t0,
            now,
            now
        )
        .unwrap()
        .is_some()
    );
    assert!(env.store.knowledge_run_reset(original.id).unwrap());
    assert!(
        extraction::advance(
            &env.store,
            env.kb.path(),
            &env.roots(),
            &task,
            env.t0,
            now,
            now
        )
        .unwrap()
        .is_none()
    );
    assert_ne!(
        env.store
            .knowledge_run_get(original.id)
            .unwrap()
            .unwrap()
            .run_task_id,
        batch.id
    );
}

#[test]
fn daily_extraction_caps_sources_and_reports_the_overflow_days() {
    let env = setup("dry_run");
    let sources: Vec<_> = (0..23).map(|_| source(&env)).collect();
    let date = env
        .store
        .get(sources[0].id)
        .unwrap()
        .unwrap()
        .updated_at
        .date();
    let now = env.now(50);
    let task = env.fire(now);
    env.tick(now);
    let batch = pending(&env);
    let input: serde_json::Value = serde_json::from_str(&batch.objective).unwrap();
    assert_eq!(input["reports"].as_array().unwrap().len(), 20);
    complete(&env, &batch, serde_json::json!({"candidates":[]}));
    assert_eq!(env.tick(now).prepared, vec![task.id]);
    let summary = extraction::summary(&env.store, &task).unwrap().unwrap();
    assert_eq!(summary.discarded_reasons.len(), 1, "{summary:?}");
    assert!(summary.discarded_reasons[0].contains("報告 3 件"));
    assert!(summary.discarded_reasons[0].contains(&date.to_string()));
    // The 3 overflow sources stay unclaimed so a backfill for that day can pick them up.
    let unclaimed = sources
        .iter()
        .filter(|s| !env.store.knowledge_run_exists(s.id).unwrap())
        .count();
    assert_eq!(unclaimed, 3);
}
