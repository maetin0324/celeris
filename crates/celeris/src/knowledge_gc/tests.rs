use super::*;
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    task_ops::knowledge::init(dir.path()).unwrap();
    for path in task_ops::knowledge::list_pages(dir.path()) {
        std::fs::remove_file(dir.path().join(path)).unwrap();
    }
    for path in ["projects/demo/a.md", "projects/demo/b.md"] {
        std::fs::create_dir_all(dir.path().join(path).parent().unwrap()).unwrap();
        std::fs::write(dir.path().join(path), "---\ntitle: Same title\nscope: project:demo\ntags: [rust, api]\nsources:\n  - task:original\nconfidence: low\n---\nExisting facts.\n").unwrap();
    }
    task_ops::knowledge::reindex(dir.path()).unwrap();
    dir
}
#[test]
fn scanner_bounds_determinism_neighbours_and_hash_reviews() {
    let root = fixture();
    let cfg = GcConfig {
        large_page_chars: 1,
        batch_pages: 2,
        ..Default::default()
    };
    let mut state = State::default();
    let batch = scan(root.path(), &cfg, &state, 100000);
    assert_eq!(batch.len(), 2);
    assert_eq!(
        batch[0].path,
        scan(root.path(), &cfg, &state, 100000)[0].path
    );
    for reason in [
        "low confidence",
        "review overdue",
        "large page",
        "similar title/tags",
        "shared source",
    ] {
        assert!(batch[0].reasons.iter().any(|r| r == reason), "{reason}");
    }
    for p in &batch {
        state.reviewed.insert(
            p.path.clone(),
            Reviewed {
                hash: p.hash.clone(),
                at: 100000,
            },
        );
    }
    assert!(scan(root.path(), &cfg, &state, 100001).is_empty());
    let p = root.path().join(&batch[0].path);
    std::fs::write(
        &p,
        format!("{}\nChanged", std::fs::read_to_string(&p).unwrap()),
    )
    .unwrap();
    assert_eq!(scan(root.path(), &cfg, &state, 100001).len(), 1);
    assert!(
        scan(
            root.path(),
            &GcConfig {
                max_context_chars: 1,
                ..cfg.clone()
            },
            &State::default(),
            100000
        )
        .is_empty()
    );
    assert_eq!(
        scan(
            root.path(),
            &GcConfig {
                batch_pages: 1,
                ..cfg
            },
            &State::default(),
            100000
        )
        .len(),
        1
    );
}
#[test]
fn gc_rejects_create_and_always_inboxes_updates_without_touching_dirty_content() {
    let root = fixture();
    let before = std::fs::read(root.path().join("projects/demo/a.md")).unwrap();
    let candidate: kb::Candidate = serde_json::from_value(serde_json::json!({"op":"update", "path":"projects/demo/a.md", "title":"Updated", "tags":[], "scope":"project:demo", "body":"Existing facts reorganized.","sources":["task:original"],"confidence":"high"})).unwrap();
    let out = task_ops::knowledge::apply_candidates_with_policy(
        root.path(),
        "gc",
        std::slice::from_ref(&candidate),
        task_ops::knowledge::ApplyPolicy::Gc,
    );
    assert_eq!(out.inboxed.len(), 1);
    assert!(out.committed.is_empty());
    assert_eq!(
        std::fs::read(root.path().join("projects/demo/a.md")).unwrap(),
        before
    );
    for op in [kb::CandidateOp::Merge, kb::CandidateOp::Retire] {
        let mut proposal = candidate.clone();
        proposal.op = op;
        let out = task_ops::knowledge::apply_candidates_with_policy(
            root.path(),
            "gc-other",
            &[proposal],
            task_ops::knowledge::ApplyPolicy::Gc,
        );
        assert_eq!(out.inboxed.len(), 1);
        assert!(out.committed.is_empty());
        assert_eq!(
            std::fs::read(root.path().join("projects/demo/a.md")).unwrap(),
            before
        );
    }
    let mut create = candidate;
    create.op = kb::CandidateOp::Create;
    assert_eq!(
        task_ops::knowledge::apply_candidates_with_policy(
            root.path(),
            "gc",
            &[create],
            task_ops::knowledge::ApplyPolicy::Gc
        )
        .dropped
        .len(),
        1
    );
}
fn store() -> task_core::SqliteStore {
    let store = task_core::SqliteStore::open_in_memory().unwrap();
    store
        .org_upsert(&task_core::org::OrgNode {
            id: "cos".into(),
            parent_id: None,
            name: "cos".into(),
            kind: task_core::org::OrgKind::Secretary,
            genre: None,
            brief: String::new(),
            position: 0,
            profile: Default::default(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        })
        .unwrap();
    store
}
#[test]
fn schedules_once_across_restarts_disabled_and_inflight_do_not_schedule() {
    let root = fixture();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("gc.json");
    let store = store();
    let now = OffsetDateTime::now_utc();
    let mut cfg = GcConfig::default();
    tick(&store, root.path(), &path, tmp.path(), &cfg, &[], &[], now).unwrap();
    assert!(!path.exists());
    cfg.enabled = true;
    tick(&store, root.path(), &path, tmp.path(), &cfg, &[], &[], now).unwrap();
    let initial = std::fs::read(&path).unwrap();
    let state: State = serde_json::from_slice(&initial).unwrap();
    let task = store.get(state.pending.unwrap().task).unwrap().unwrap();
    assert_eq!(task.worker_hint.adapter.as_deref(), Some("langmem"));
    assert_eq!(task_core::report::support_kind(&task), Some("knowledge"));
    tick(
        &store,
        root.path(),
        &path,
        tmp.path(),
        &cfg,
        &[],
        &[],
        now + time::Duration::hours(25),
    )
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), initial);
    assert!(!due(
        Some(now.unix_timestamp()),
        now.unix_timestamp() + 1,
        24
    ));
}
#[test]
fn failed_or_missing_output_is_not_reviewed_and_interval_is_consumed() {
    let root = fixture();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("gc.json");
    let store = store();
    let now = OffsetDateTime::now_utc();
    let cfg = GcConfig {
        enabled: true,
        ..Default::default()
    };
    // A missing task behaves like a failed/cancelled run and cannot count as review.
    let state = State {
        last_attempt: Some(now.unix_timestamp()),
        pending: Some(Pending {
            task: TaskId::new(),
            pages: scan(root.path(), &cfg, &State::default(), now.unix_timestamp()),
        }),
        ..Default::default()
    };
    save(&path, &state).unwrap();
    tick(&store, root.path(), &path, tmp.path(), &cfg, &[], &[], now).unwrap();
    let after: State = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(after.pending.is_none());
    assert!(after.reviewed.is_empty());
    assert_eq!(after.last_attempt, state.last_attempt);
}
#[test]
fn successful_noop_preserves_canonical_and_excluded_pages_never_enter_batch() {
    let root = fixture();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("gc.json");
    for dir in ["skills", "_inbox", "_retired"] {
        std::fs::create_dir_all(root.path().join(dir)).unwrap();
        std::fs::write(root.path().join(dir).join("excluded.md"), "# Excluded").unwrap();
    }
    let store = store();
    let now = OffsetDateTime::now_utc();
    let cfg = GcConfig {
        enabled: true,
        ..Default::default()
    };
    let before = std::fs::read(root.path().join("projects/demo/a.md")).unwrap();
    tick(&store, root.path(), &path, tmp.path(), &cfg, &[], &[], now).unwrap();
    let mut initial: State = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut pending = initial.pending.take().unwrap();
    assert!(pending.pages.iter().all(|p| !p.path.contains("excluded")));
    let mut task = store.get(pending.task).unwrap().unwrap();
    task.status = Status::Done;
    task.id = TaskId::new();
    store.insert(&task).unwrap();
    pending.task = task.id;
    let reviewed_count = pending.pages.len();
    initial.pending = Some(pending);
    save(&path, &initial).unwrap();
    let artifacts =
        task_core::artifacts::artifacts_dir_for(&task, &tmp.path().join(task.id.to_string()));
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(
        artifacts.join("knowledge-candidates.json"),
        "{\"candidates\":[]}",
    )
    .unwrap();
    tick(&store, root.path(), &path, tmp.path(), &cfg, &[], &[], now).unwrap();
    let reviewed: State = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(reviewed.reviewed.len(), reviewed_count);
    assert_eq!(
        std::fs::read(root.path().join("projects/demo/a.md")).unwrap(),
        before
    );
    assert!(scan(root.path(), &cfg, &reviewed, now.unix_timestamp()).is_empty());
}
#[test]
fn lost_derived_state_still_does_not_duplicate_active_support_task() {
    let root = fixture();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("gc.json");
    let store = store();
    let now = OffsetDateTime::now_utc();
    let cfg = GcConfig {
        enabled: true,
        ..Default::default()
    };
    tick(&store, root.path(), &path, tmp.path(), &cfg, &[], &[], now).unwrap();
    std::fs::remove_file(&path).unwrap();
    tick(
        &store,
        root.path(),
        &path,
        tmp.path(),
        &cfg,
        &[],
        &[],
        now + time::Duration::hours(25),
    )
    .unwrap();
    assert!(!path.exists());
    assert_eq!(store.list(None).unwrap().len(), 1);
}
#[test]
fn missing_and_malformed_success_output_are_not_reviews() {
    for output in [None, Some("not json")] {
        let root = fixture();
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("gc.json");
        let store = store();
        let now = OffsetDateTime::now_utc();
        let cfg = GcConfig {
            enabled: true,
            ..Default::default()
        };
        tick(&store, root.path(), &path, tmp.path(), &cfg, &[], &[], now).unwrap();
        let mut state: State = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let pending = state.pending.as_mut().unwrap();
        let mut task = store.get(pending.task).unwrap().unwrap();
        task.status = Status::Done;
        task.id = TaskId::new();
        store.insert(&task).unwrap();
        pending.task = task.id;
        save(&path, &state).unwrap();
        if let Some(output) = output {
            let artifacts = task_core::artifacts::artifacts_dir_for(
                &task,
                &tmp.path().join(task.id.to_string()),
            );
            std::fs::create_dir_all(&artifacts).unwrap();
            std::fs::write(artifacts.join("knowledge-candidates.json"), output).unwrap();
        }
        tick(&store, root.path(), &path, tmp.path(), &cfg, &[], &[], now).unwrap();
        let after: State = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(after.reviewed.is_empty());
        assert!(after.pending.is_none());
    }
}
