use super::*;
use std::fs;
use task_core::{ProjectId, RepoId, TaskId};

fn record() -> Delivery {
    Delivery {
        task_id: TaskId::new(),
        project_id: ProjectId::new(),
        repo_id: RepoId::new(),
        repo: "test".into(),
        branch: "feature".into(),
        base: String::new(),
        head: String::new(),
        target_sha: None,
        reviewed_sha: None,
        merge_candidate_sha: None,
        default_branch: "main".into(),
        department: "engineering".into(),
        review_run: "review-run".into(),
        worker_run: "worker-run".into(),
        criterion_idx: 0,
        decision: Some(true),
        state: State::Reviewing,
        detail: String::new(),
        release: None,
        prepare_pid: None,
        notification: None,
        pushed_at: None,
        push_error: None,
    }
}
fn repository() -> (tempfile::TempDir, Delivery) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git_text(p, &["init", "-b", "main"]).unwrap();
    git_text(p, &["config", "user.email", "test@example.invalid"]).unwrap();
    git_text(p, &["config", "user.name", "test"]).unwrap();
    fs::write(p.join("base"), "base").unwrap();
    fs::write(p.join("user-file"), "keep").unwrap();
    git_text(p, &["add", "."]).unwrap();
    git_text(p, &["commit", "-m", "base"]).unwrap();
    let mut d = record();
    d.base = sha(p, "HEAD").unwrap();
    git_text(p, &["checkout", "-b", "feature"]).unwrap();
    fs::write(p.join("feature"), "implemented").unwrap();
    git_text(p, &["add", "."]).unwrap();
    git_text(p, &["commit", "-m", "feature"]).unwrap();
    d.head = sha(p, "HEAD").unwrap();
    // ADR-0118 D3: review 前の同期が記録する target / reviewed / merge candidate。
    d.target_sha = Some(d.base.clone());
    d.reviewed_sha = Some(d.head.clone());
    d.merge_candidate_sha = Some(d.head.clone());
    git_text(p, &["checkout", "main"]).unwrap();
    (dir, d)
}
#[test]
fn reviewed_merge_preserves_unrelated_user_deletion_and_branch() {
    let (dir, d) = repository();
    fs::remove_file(dir.path().join("user-file")).unwrap();
    merge_reviewed(dir.path(), &d).unwrap();
    assert_eq!(sha(dir.path(), "HEAD").unwrap(), d.head);
    assert!(!dir.path().join("user-file").exists());
    assert_eq!(sha(dir.path(), "feature").unwrap(), d.head);
}
#[test]
fn new_commits_or_moved_base_cannot_reuse_approval() {
    let (dir, d) = repository();
    let p = dir.path();
    git_text(p, &["checkout", "feature"]).unwrap();
    git_text(p, &["commit", "--allow-empty", "-m", "unreviewed"]).unwrap();
    git_text(p, &["checkout", "main"]).unwrap();
    assert!(merge_reviewed(p, &d).is_err());
    assert_eq!(sha(p, "HEAD").unwrap(), d.base);
    let (dir, d) = repository();
    let p = dir.path();
    git_text(p, &["commit", "--allow-empty", "-m", "main moved"]).unwrap();
    let moved = sha(p, "HEAD").unwrap();
    assert!(merge_reviewed(p, &d).is_err());
    assert_eq!(sha(p, "HEAD").unwrap(), moved);
}
#[test]
fn overlapping_untracked_files_block_merge_without_loss() {
    let (dir, d) = repository();
    fs::write(dir.path().join("feature"), "human draft").unwrap();
    assert!(merge_reviewed(dir.path(), &d).is_err());
    assert_eq!(
        fs::read_to_string(dir.path().join("feature")).unwrap(),
        "human draft"
    );
    assert_eq!(sha(dir.path(), "HEAD").unwrap(), d.base);
}
fn stored_delivery() -> (task_core::SqliteStore, Delivery) {
    use task_core::*;
    let store = SqliteStore::open_in_memory().unwrap();
    let now = OffsetDateTime::now_utc();
    for (id, parent, kind) in [
        ("cos", None, OrgKind::Secretary),
        ("engineering", Some("cos"), OrgKind::Department),
    ] {
        store
            .org_upsert(&OrgNode {
                id: id.into(),
                parent_id: parent.map(str::to_string),
                name: id.into(),
                kind,
                genre: None,
                brief: String::new(),
                profile: Default::default(),
                position: 0,
                created_at: now,
                updated_at: now,
            })
            .unwrap();
    }
    let p = Project {
        auto_advance: false,
        slug: None,
        id: ProjectId::new(),
        title: "test".into(),
        request: "fix".into(),
        status: ProjectStatus::Active,
        secretary_summary: None,
        workspace: None,
        archived_at: None,
        paused_from: None,
        created_at: now,
        updated_at: now,
    };
    store.project_create(&p).unwrap();
    let spec:task_ops::add::NewTaskSpec=serde_json::from_value(serde_json::json!({"title":"fix","objective":"implement","acceptance":[],"project_id":p.id,"assignee":"engineering"})).unwrap();
    let task = task_ops::add::create_support_task(&store, spec, &[], &[], now).unwrap();
    for trigger in [Trigger::Dispatch, Trigger::WorkerDone, Trigger::ReviewPass] {
        store.apply_transition(task.id, trigger, None).unwrap();
    }
    let mut d = record();
    d.task_id = task.id;
    d.project_id = p.id;
    (store, d)
}
#[test]
fn delivery_failure_classification_is_limited_to_merge_base_and_format() {
    assert_eq!(
        classify_delivery_failure(State::Blocked, "[merge-base] git failed", None),
        Some(RepairClass::MergeBase)
    );
    assert_eq!(
        classify_delivery_failure(
            State::Blocked,
            "リリース準備に失敗しました",
            Some("cargo-fmt-check")
        ),
        Some(RepairClass::Format)
    );
    assert_eq!(
        classify_delivery_failure(
            State::Blocked,
            "リリース準備に失敗しました",
            Some("cargo-test")
        ),
        None
    );
    assert_eq!(
        classify_delivery_failure(
            State::Blocked,
            "リリース準備に失敗しました",
            Some("pnpm-e2e-mock")
        ),
        None
    );
    assert_eq!(
        classify_delivery_failure(State::Ready, "[merge-base] git failed", None),
        None
    );
}
#[test]
fn preparation_requires_all_gates_and_notifies_cos_only_when_ready() {
    use task_core::DeliveryStore;
    let (store, mut d) = stored_delivery();
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg: Config = toml::from_str("").unwrap();
    cfg.selfdeploy.releases_dir = tmp.path().join("releases");
    d.head = "b".repeat(40);
    d.release = Some("b".repeat(12));
    d.state = State::Preparing;
    let dir = preparation_dir(&cfg, &d);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("result.json"), r#"{"ok":true}"#).unwrap();
    let rel = cfg
        .selfdeploy
        .releases_dir
        .join(d.release.as_ref().unwrap());
    fs::create_dir_all(&rel).unwrap();
    fs::write(rel.join("gate.json"), r#"{"ok":true}"#).unwrap();
    fs::write(rel.join("verify.json"), r#"{"ok":true}"#).unwrap();
    fs::write(rel.join("manifest.json"), r#"{"sha":"wrong"}"#).unwrap();
    store.delivery_save(None, &d).unwrap();
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    let blocked = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(blocked.state, State::Blocked);
    store.delivery_save(Some(&blocked), &d).unwrap();
    fs::write(
        rel.join("manifest.json"),
        serde_json::json!({"sha":d.head}).to_string(),
    )
    .unwrap();
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    let ready = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(ready.state, State::Ready);
    advance(&store, &cfg, &ready, OffsetDateTime::now_utc()).unwrap();
    let notified = store.delivery_get(d.task_id).unwrap().unwrap();
    advance(&store, &cfg, &notified, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(
        store
            .message_list("cos", Some(d.project_id), 20)
            .unwrap()
            .len(),
        1
    );
    assert!(!tmp.path().join("current").exists());
}
#[test]
fn other_gate_failure_keeps_legacy_reopen_once() {
    use task_core::{DeliveryStore, Trigger};
    let (store, mut d) = stored_delivery();
    let cfg: Config = toml::from_str("").unwrap();
    d.state = State::Blocked;
    d.detail = "ビルドの検査失敗".into();
    store.delivery_save(None, &d).unwrap();
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(store.get(d.task_id).unwrap().unwrap().status, Status::Ready);
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(store.comments_for(d.task_id).unwrap().len(), 1);
    for trigger in [Trigger::Dispatch, Trigger::WorkerDone, Trigger::ReviewPass] {
        store.apply_transition(d.task_id, trigger, None).unwrap();
    }
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(store.get(d.task_id).unwrap().unwrap().status, Status::Done);
    assert_eq!(
        store
            .message_list("engineering", Some(d.project_id), 20)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .message_list("cos", Some(d.project_id), 20)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn technical_failure_is_repaired_once_with_no_cos_review_or_message() {
    use task_core::DeliveryStore;
    let (store, mut d) = stored_delivery();
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = cfg_for(tmp.path(), &tmp.path().join("releases"));
    cfg.execution.max_repairs_per_class = 1;
    d.state = State::Blocked;
    d.detail = "リリース準備に失敗しました。ログ: prepare.log".into();
    d.head = "a".repeat(40);
    d.release = Some("a".repeat(12));
    let rel = cfg
        .selfdeploy
        .releases_dir
        .join(d.release.as_deref().unwrap());
    fs::create_dir_all(&rel).unwrap();
    fs::write(
        rel.join("gate.json"),
        r#"{"ok":false,"failed_step":"cargo-fmt-check"}"#,
    )
    .unwrap();
    fs::write(
        rel.join(".gate-cargo-fmt-check.log"),
        "Diff in src/main.rs:1",
    )
    .unwrap();
    store.delivery_save(None, &d).unwrap();
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(store.get(d.task_id).unwrap().unwrap().status, Status::Ready);
    let units = store.work_units_for(d.task_id).unwrap();
    assert_eq!(units.len(), 2);
    assert_eq!(units[0].status, WorkUnitStatus::Done);
    assert_eq!(units[1].status, WorkUnitStatus::Ready);
    assert!(units[1].spec.title.starts_with("repair (format):"));
    assert!(units[1].spec.objective.contains("Diff in src/main.rs:1"));
    assert!(!units[1].spec.objective.contains("implement"));
    assert_eq!(
        task_core::next_work_unit(&units),
        task_core::NextStep::RunWorkUnit(units[1].id.clone())
    );
    assert!(
        store
            .comments_for(d.task_id)
            .unwrap()
            .iter()
            .any(|c| c.body.contains("repair-1 class=format"))
    );

    // 同じ class の上限に達した再失敗は人へ戻す。
    for trigger in [
        task_core::Trigger::Dispatch,
        task_core::Trigger::WorkerDone,
        task_core::Trigger::ReviewPass,
    ] {
        store.apply_transition(d.task_id, trigger, None).unwrap();
    }
    let mut completed = units[1].clone();
    completed.status = WorkUnitStatus::Done;
    store
        .work_unit_transition(
            d.task_id,
            completed,
            Event::WorkUnitTransitioned {
                work_unit_id: units[1].id.clone(),
                key: units[1].key.clone(),
                from: WorkUnitStatus::Ready,
                to: WorkUnitStatus::Done,
                reason: "test".into(),
                run_id: None,
            },
        )
        .unwrap();
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    let requeued = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(requeued.state, State::MergeQueued);
    let mut failed_again = requeued.clone();
    failed_again.state = State::Blocked;
    failed_again.detail = d.detail.clone();
    failed_again.release = d.release.clone();
    store.delivery_save(Some(&requeued), &failed_again).unwrap();
    advance(&store, &cfg, &failed_again, OffsetDateTime::now_utc()).unwrap();
    assert!(
        store
            .delivery_get(d.task_id)
            .unwrap()
            .unwrap()
            .detail
            .starts_with("[needs-human]")
    );
}

#[test]
fn merge_base_repair_revalidates_and_delivers_only_the_repaired_branch() {
    use task_core::{DeliveryStore, Trigger};
    let (store, dir, d) = merge_queued_delivery();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_for(dir.path(), &tmp.path().join("releases"));
    // The previous run may have a large transcript; it must not enter the repair objective.
    let transcript_marker = "ORIGINAL_RUN_TRANSCRIPT_MARKER";
    store
        .append_event(
            d.task_id,
            &Event::worker_progress("original-run", transcript_marker),
        )
        .unwrap();
    // An untracked file that the reviewed commit would overwrite makes the ff-only merge fail.
    // A moved main is no longer a merge failure: it asks for a re-review (ADR-0118 D4).
    fs::write(dir.path().join("feature"), "human draft").unwrap();
    let main_head = sha(dir.path(), "main").unwrap();
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    let blocked = store.delivery_get(d.task_id).unwrap().unwrap();
    assert!(blocked.detail.starts_with(MERGE_BASE_FAILURE));
    advance(&store, &cfg, &blocked, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(store.get(d.task_id).unwrap().unwrap().status, Status::Ready);
    let units = store.work_units_for(d.task_id).unwrap();
    assert_eq!(units.len(), 2);
    let repair = &units[1];
    assert_eq!(repair.kind, WorkUnitKind::Repair);
    assert_eq!(repair.status, WorkUnitStatus::Ready);
    assert!(repair.spec.title.starts_with("repair (merge_base):"));
    assert_eq!(
        store
            .execution_plan_active(d.task_id)
            .unwrap()
            .unwrap()
            .status,
        task_core::PlanStatus::Active
    );
    assert!(repair.spec.objective.contains(&d.branch));
    assert!(repair.spec.objective.contains(&main_head));
    assert!(repair.spec.objective.contains(&blocked.detail));
    assert!(!repair.spec.objective.contains(transcript_marker));
    assert!(!repair.spec.objective.contains("implement"));
    assert!(!repair.spec.objective.contains("work_units"));

    // Simulate the repair worker committing a fix after the human moved the draft away.
    fs::remove_file(dir.path().join("feature")).unwrap();
    git_text(dir.path(), &["checkout", "feature"]).unwrap();
    git_text(dir.path(), &["commit", "--allow-empty", "-m", "repair"]).unwrap();
    let repaired_head = sha(dir.path(), "feature").unwrap();
    git_text(dir.path(), &["checkout", "main"]).unwrap();
    let mut done = repair.clone();
    done.status = WorkUnitStatus::Done;
    store
        .work_unit_transition(
            d.task_id,
            done,
            Event::WorkUnitTransitioned {
                work_unit_id: repair.id.clone(),
                key: repair.key.clone(),
                from: WorkUnitStatus::Ready,
                to: WorkUnitStatus::Done,
                reason: "test_repair_completed".into(),
                run_id: None,
            },
        )
        .unwrap();
    for trigger in [Trigger::Dispatch, Trigger::WorkerDone, Trigger::ReviewPass] {
        store.apply_transition(d.task_id, trigger, None).unwrap();
    }
    let now = OffsetDateTime::now_utc();
    advance(&store, &cfg, &blocked, now).unwrap();
    let requeued = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(requeued.state, State::MergeQueued);
    assert_eq!(requeued.base, main_head);
    assert_eq!(requeued.head, repaired_head);
    // The repaired head was never reviewed: it is not merged and goes back to review.
    advance(&store, &cfg, &requeued, now).unwrap();
    let rereview = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(rereview.state, State::Reviewing);
    assert_eq!(sha(dir.path(), "main").unwrap(), main_head);
    assert_eq!(
        store.get(d.task_id).unwrap().unwrap().status,
        Status::Reviewing
    );
    // The re-review syncs, checks and reviews the repaired head and records it as the candidate.
    let mut requeued = rereview.clone();
    requeued.state = State::MergeQueued;
    requeued.decision = Some(true);
    requeued.target_sha = Some(main_head.clone());
    requeued.reviewed_sha = Some(repaired_head.clone());
    requeued.merge_candidate_sha = Some(repaired_head.clone());
    assert!(store.delivery_save(Some(&rereview), &requeued).unwrap());
    store
        .apply_transition(d.task_id, Trigger::ReviewPass, None)
        .unwrap();
    // A local fake prepare script lets the merge proceed without touching a real release.
    let script = cfg
        .selfdeploy
        .releases_dir
        .parent()
        .unwrap()
        .join("current/scripts/prepare.sh");
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    advance(&store, &cfg, &requeued, now).unwrap();
    let delivered = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(delivered.state, State::Preparing);
    assert_eq!(sha(dir.path(), "main").unwrap(), repaired_head);
    assert_eq!(delivered.release.as_deref(), Some(&repaired_head[..12]));
}

// ---- ADR-0051 Phase 106追記: merge直後・release前のpush。ここから ----

fn bare_remote() -> tempfile::TempDir {
    let bare = tempfile::tempdir().unwrap();
    git_text(bare.path(), &["init", "--bare", "-b", "main"]).unwrap();
    bare
}

#[test]
fn push_merged_pushes_new_commits_to_a_bare_remote() {
    let (dir, d) = repository();
    let p = dir.path();
    merge_reviewed(p, &d).unwrap();
    let bare = bare_remote();
    git_text(
        p,
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    )
    .unwrap();
    push_merged(p, "origin", "main", Duration::from_secs(20)).unwrap();
    assert_eq!(sha(bare.path(), "refs/heads/main").unwrap(), d.head);
}

#[test]
fn push_merged_reports_failure_reason_when_remote_is_unreachable() {
    let (dir, d) = repository();
    let p = dir.path();
    merge_reviewed(p, &d).unwrap();
    git_text(p, &["remote", "add", "origin", "/no/such/path-phase106"]).unwrap();
    let err = push_merged(p, "origin", "main", Duration::from_secs(20)).unwrap_err();
    assert!(!err.trim().is_empty());
}

#[test]
fn push_merged_skips_when_remote_is_already_up_to_date() {
    let (dir, d) = repository();
    let p = dir.path();
    merge_reviewed(p, &d).unwrap();
    // 実際には壊れたリモートだが、追跡refだけを直接作って「既に同じ」を再現する
    // （ネットワークにもリモートにも触れずに済む）。
    git_text(p, &["remote", "add", "origin", "/no/such/path-phase106"]).unwrap();
    git_text(p, &["update-ref", "refs/remotes/origin/main", &d.head]).unwrap();
    push_merged(p, "origin", "main", Duration::from_secs(5)).unwrap();
}

/// `stored_delivery()`（task がDone・cos/engineeringの組織）と `repository()`（git リポジトリ）を
/// 組み合わせ、`State::MergeQueued` から `advance` を通す。
fn merge_queued_delivery() -> (task_core::SqliteStore, tempfile::TempDir, Delivery) {
    use task_core::DeliveryStore;
    let (store, mut d) = stored_delivery();
    let (dir, repo_d) = repository();
    d.base = repo_d.base;
    d.head = repo_d.head;
    d.target_sha = repo_d.target_sha;
    d.reviewed_sha = repo_d.reviewed_sha;
    d.merge_candidate_sha = repo_d.merge_candidate_sha;
    d.branch = repo_d.branch;
    d.default_branch = repo_d.default_branch;
    d.state = State::MergeQueued;
    store.delivery_save(None, &d).unwrap();
    (store, dir, d)
}

fn cfg_for(repo: &Path, releases_dir: &Path) -> Config {
    let mut cfg: Config = toml::from_str("").unwrap();
    cfg.selfdeploy.repo = repo.to_path_buf();
    cfg.selfdeploy.releases_dir = releases_dir.to_path_buf();
    cfg
}

#[test]
fn advance_pushes_to_origin_right_after_merge_before_release_prep() {
    use task_core::DeliveryStore;
    let (store, dir, d) = merge_queued_delivery();
    let bare = bare_remote();
    git_text(
        dir.path(),
        &["remote", "add", "origin", bare.path().to_str().unwrap()],
    )
    .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_for(dir.path(), &tmp.path().join("releases"));
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    let after = store.delivery_get(d.task_id).unwrap().unwrap();
    assert!(after.pushed_at.is_some());
    assert!(after.push_error.is_none());
    assert_eq!(after.state, State::Preparing);
    assert!(after.prepare_pid.is_some());
    assert_eq!(sha(bare.path(), "refs/heads/main").unwrap(), after.head);
}

#[test]
fn push_failure_after_merge_does_not_block_release_prep_and_retries_once() {
    use task_core::DeliveryStore;
    let (store, dir, d) = merge_queued_delivery();
    git_text(
        dir.path(),
        &["remote", "add", "origin", "/no/such/path-phase106-retry"],
    )
    .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_for(dir.path(), &tmp.path().join("releases"));

    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    let after_merge = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(after_merge.state, State::Preparing, "release準備は進む");
    assert!(after_merge.pushed_at.is_none());
    let first_err = after_merge
        .push_error
        .clone()
        .expect("push は失敗しているはず");
    assert!(!first_err.starts_with(PUSH_RETRIED_PREFIX));

    // 次のtickで1度だけ再試行。
    advance(&store, &cfg, &after_merge, OffsetDateTime::now_utc()).unwrap();
    let after_retry = store.delivery_get(d.task_id).unwrap().unwrap();
    assert!(after_retry.pushed_at.is_none());
    let retried_err = after_retry
        .push_error
        .clone()
        .expect("再試行後も失敗しているはず");
    assert!(retried_err.starts_with(PUSH_RETRIED_PREFIX));

    // それでも失敗したら以後は触らない: もう一度 advance しても push_error は変わらない。
    advance(&store, &cfg, &after_retry, OffsetDateTime::now_utc()).unwrap();
    let after_third = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(after_third.push_error, after_retry.push_error);
    assert!(after_third.pushed_at.is_none());
}

#[test]
fn maybe_retry_push_noop_when_push_disabled() {
    let store = task_core::SqliteStore::open_in_memory().unwrap();
    let mut cfg: Config = toml::from_str("").unwrap();
    cfg.selfdeploy.push = false;
    let mut d = record();
    d.release = Some("a".repeat(12));
    d.push_error = Some("boom".into());
    let out = maybe_retry_push(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(out, d, "push=falseなら何もしない");
}

/// Preparing → Ready の通知文に、pushの結果を1行足す（成功）。
#[test]
fn ready_notification_reports_successful_push() {
    use task_core::DeliveryStore;
    let (store, mut d) = stored_delivery();
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg: Config = toml::from_str("").unwrap();
    cfg.selfdeploy.releases_dir = tmp.path().join("releases");
    d.head = "c".repeat(40);
    d.release = Some("c".repeat(12));
    d.state = State::Preparing;
    d.pushed_at = Some(OffsetDateTime::now_utc());
    let dir = preparation_dir(&cfg, &d);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("result.json"), r#"{"ok":true}"#).unwrap();
    let rel = cfg
        .selfdeploy
        .releases_dir
        .join(d.release.as_ref().unwrap());
    fs::create_dir_all(&rel).unwrap();
    fs::write(rel.join("gate.json"), r#"{"ok":true}"#).unwrap();
    fs::write(rel.join("verify.json"), r#"{"ok":true}"#).unwrap();
    fs::write(
        rel.join("manifest.json"),
        serde_json::json!({"sha": d.head}).to_string(),
    )
    .unwrap();
    store.delivery_save(None, &d).unwrap();
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    let ready = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(ready.state, State::Ready);
    advance(&store, &cfg, &ready, OffsetDateTime::now_utc()).unwrap();
    let messages = store.message_list("cos", Some(d.project_id), 20).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].text.contains("origin へ push 済み"));
    assert!(messages[0].text.contains(&d.head));
}

/// Preparing → Ready の通知文に、pushの結果を1行足す（失敗。`[retried]` の目印は見せない）。
#[test]
fn ready_notification_reports_push_failure_without_the_retry_marker() {
    use task_core::DeliveryStore;
    let (store, mut d) = stored_delivery();
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg: Config = toml::from_str("").unwrap();
    cfg.selfdeploy.releases_dir = tmp.path().join("releases");
    d.head = "d".repeat(40);
    d.release = Some("d".repeat(12));
    d.state = State::Preparing;
    d.push_error = Some(format!("{PUSH_RETRIED_PREFIX}fatal: repository not found"));
    let dir = preparation_dir(&cfg, &d);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("result.json"), r#"{"ok":true}"#).unwrap();
    let rel = cfg
        .selfdeploy
        .releases_dir
        .join(d.release.as_ref().unwrap());
    fs::create_dir_all(&rel).unwrap();
    fs::write(rel.join("gate.json"), r#"{"ok":true}"#).unwrap();
    fs::write(rel.join("verify.json"), r#"{"ok":true}"#).unwrap();
    fs::write(
        rel.join("manifest.json"),
        serde_json::json!({"sha": d.head}).to_string(),
    )
    .unwrap();
    store.delivery_save(None, &d).unwrap();
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    let ready = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(ready.state, State::Ready);
    advance(&store, &cfg, &ready, OffsetDateTime::now_utc()).unwrap();
    let messages = store.message_list("cos", Some(d.project_id), 20).unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].text.contains("push に失敗"));
    assert!(messages[0].text.contains("fatal: repository not found"));
    assert!(!messages[0].text.contains(PUSH_RETRIED_PREFIX));
}
// ---- ADR-0051 Phase 106追記: ここまで ----

// ---- ADR-0118 D4: merge 直前の target 再進行と merge candidate の照合 ----

fn target_advanced_events(store: &task_core::SqliteStore, d: &Delivery) -> Vec<Event> {
    store
        .events_for(d.task_id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| matches!(e, Event::ReviewTargetAdvanced { .. }))
        .collect()
}

#[test]
fn target_advanced_before_merge_requests_rereview_instead_of_stale_reviewed_sha() {
    use task_core::DeliveryStore;
    let (store, dir, d) = merge_queued_delivery();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_for(dir.path(), &tmp.path().join("releases"));
    git_text(dir.path(), &["commit", "--allow-empty", "-m", "main moved"]).unwrap();
    let moved = sha(dir.path(), "main").unwrap();

    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();

    // No ff-only merge and no merge_base repair: the stale review simply does not count.
    assert_eq!(sha(dir.path(), "main").unwrap(), moved);
    assert_eq!(sha(dir.path(), "feature").unwrap(), d.head);
    let after = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(after.state, State::Reviewing);
    assert_eq!(after.decision, None);
    assert_eq!(after.merge_candidate_sha, None);
    assert_eq!(after.reviewed_sha, None);
    assert!(after.detail.contains(&moved));
    assert!(store.work_units_for(d.task_id).unwrap().is_empty());
    let task = store.get(d.task_id).unwrap().unwrap();
    assert_eq!(task.status, Status::Reviewing);
    assert_eq!(
        target_advanced_events(&store, &d),
        vec![Event::ReviewTargetAdvanced {
            review_run: d.review_run.clone(),
            repo_id: d.repo_id,
            reviewed_sha: d.head.clone(),
            target_sha: moved,
            attempt: 1,
        }]
    );
    // While the task is being re-reviewed the delivery waits; nothing is merged or blocked.
    advance(&store, &cfg, &after, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(store.delivery_get(d.task_id).unwrap().unwrap(), after);
}

#[test]
fn target_advanced_rereview_stops_for_human_after_limit_reviewed_sha() {
    use task_core::{DeliveryStore, Trigger};
    let (store, dir, d) = merge_queued_delivery();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_for(dir.path(), &tmp.path().join("releases"));
    let now = OffsetDateTime::now_utc();
    let mut current = d.clone();
    // Simulates the dispatcher's re-sync/check/review recording the then-current target.
    let reviewed_again = |current: &Delivery| {
        let mut next = current.clone();
        let target = sha(dir.path(), "main").unwrap();
        next.state = State::MergeQueued;
        next.decision = Some(true);
        next.base = target.clone();
        next.target_sha = Some(target);
        next.reviewed_sha = Some(d.head.clone());
        next.merge_candidate_sha = Some(d.head.clone());
        assert!(store.delivery_save(Some(current), &next).unwrap());
        store
            .apply_transition(d.task_id, Trigger::ReviewPass, None)
            .unwrap();
        next
    };
    for attempt in 1..=task_ops::delivery::MAX_TARGET_RESYNCS {
        git_text(dir.path(), &["commit", "--allow-empty", "-m", "main moved"]).unwrap();
        advance(&store, &cfg, &current, now).unwrap();
        let after = store.delivery_get(d.task_id).unwrap().unwrap();
        assert_eq!(after.state, State::Reviewing, "attempt {attempt}");
        current = reviewed_again(&after);
    }
    git_text(
        dir.path(),
        &["commit", "--allow-empty", "-m", "main moved again"],
    )
    .unwrap();
    let moved = sha(dir.path(), "main").unwrap();
    advance(&store, &cfg, &current, now).unwrap();
    let stopped = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(stopped.state, State::Blocked);
    assert!(stopped.detail.starts_with("[needs-human]"));
    assert!(stopped.detail.contains(&moved) && stopped.detail.contains(&d.head));
    assert!(!stopped.detail.starts_with(MERGE_BASE_FAILURE));
    assert_eq!(stopped.merge_candidate_sha, None);
    assert_eq!(sha(dir.path(), "main").unwrap(), moved);
    assert_eq!(store.get(d.task_id).unwrap().unwrap().status, Status::Done);
    assert_eq!(target_advanced_events(&store, &d).len(), 3);
    // The stop is visible to the human through the secretary, not repaired automatically.
    advance(&store, &cfg, &stopped, now).unwrap();
    let notified = store.delivery_get(d.task_id).unwrap().unwrap();
    assert!(notified.notification.is_some());
    assert!(store.work_units_for(d.task_id).unwrap().is_empty());

    // An explicit human re-review restarts the automatic budget.
    store
        .apply_transition(d.task_id, Trigger::Rereview, None)
        .unwrap();
    store
        .append_event(d.task_id, &Event::worker_progress("review", "synced"))
        .unwrap();
    let mut fresh = notified.clone();
    fresh.notification = None;
    assert!(store.delivery_save(Some(&notified), &fresh).unwrap());
    let current = reviewed_again(&fresh);
    git_text(
        dir.path(),
        &["commit", "--allow-empty", "-m", "main moved after restart"],
    )
    .unwrap();
    advance(&store, &cfg, &current, now).unwrap();
    assert_eq!(
        store.delivery_get(d.task_id).unwrap().unwrap().state,
        State::Reviewing
    );
}

#[test]
fn target_advanced_absent_merges_only_the_matching_reviewed_sha_candidate() {
    use task_core::DeliveryStore;
    // A candidate that differs from the reviewed SHA is never fast-forwarded.
    let (store, dir, d) = merge_queued_delivery();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_for(dir.path(), &tmp.path().join("releases"));
    let mut mismatched = d.clone();
    mismatched.reviewed_sha = Some(d.base.clone());
    assert!(store.delivery_save(Some(&d), &mismatched).unwrap());
    advance(&store, &cfg, &mismatched, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(sha(dir.path(), "main").unwrap(), d.base);
    assert_eq!(
        store.delivery_get(d.task_id).unwrap().unwrap().state,
        State::Reviewing
    );
    // A branch that moved after review is not the candidate either.
    let (store, dir, d) = merge_queued_delivery();
    let cfg = cfg_for(dir.path(), &tmp.path().join("releases"));
    git_text(dir.path(), &["checkout", "feature"]).unwrap();
    git_text(dir.path(), &["commit", "--allow-empty", "-m", "unreviewed"]).unwrap();
    git_text(dir.path(), &["checkout", "main"]).unwrap();
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(sha(dir.path(), "main").unwrap(), d.base);
    assert_eq!(
        store.delivery_get(d.task_id).unwrap().unwrap().state,
        State::Reviewing
    );
    // Legacy rows without a recorded candidate are not treated as reviewed.
    let (store, dir, d) = merge_queued_delivery();
    let cfg = cfg_for(dir.path(), &tmp.path().join("releases"));
    let mut legacy = d.clone();
    legacy.target_sha = None;
    legacy.reviewed_sha = None;
    legacy.merge_candidate_sha = None;
    assert!(store.delivery_save(Some(&d), &legacy).unwrap());
    advance(&store, &cfg, &legacy, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(sha(dir.path(), "main").unwrap(), d.base);
    // The matching candidate on an unchanged target is fast-forwarded exactly.
    let (store, dir, d) = merge_queued_delivery();
    let cfg = cfg_for(dir.path(), &tmp.path().join("releases"));
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(sha(dir.path(), "main").unwrap(), d.head);
    let merged = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(merged.state, State::Preparing);
    assert_eq!(merged.merge_candidate_sha.as_deref(), Some(d.head.as_str()));
    assert!(target_advanced_events(&store, &d).is_empty());
    // merge_reviewed itself refuses a candidate that is not the reviewed SHA.
    let (dir, mut d) = repository();
    d.merge_candidate_sha = Some(d.base.clone());
    assert!(merge_reviewed(dir.path(), &d).is_err());
    assert_eq!(sha(dir.path(), "main").unwrap(), d.base);
}

/// ADR-0118 D6 付記: review 前同期が衝突で諦めた行（merge candidate が NULL）で main が分岐していれば、
/// 再レビューを要求せず（無限の rereview にしない）、従来どおり [merge-base] の Blocked（局所修復の対象）に進む。
#[test]
fn pre_review_sync_conflict_unsynced_candidate_goes_to_merge_base_repair() {
    use task_core::DeliveryStore;
    let (store, dir, mut d) = merge_queued_delivery();
    let mut unsynced = d.clone();
    unsynced.target_sha = None;
    unsynced.reviewed_sha = None;
    unsynced.merge_candidate_sha = None;
    assert!(store.delivery_save(Some(&d), &unsynced).unwrap());
    d = unsynced;
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_for(dir.path(), &tmp.path().join("releases"));
    fs::write(dir.path().join("base"), "main changed").unwrap();
    git_text(dir.path(), &["commit", "-am", "main diverged"]).unwrap();
    let moved = sha(dir.path(), "main").unwrap();
    // review 開始時の base は分岐後の main（衝突した同期と同じ状況）。
    let mut diverged = d.clone();
    diverged.base = moved.clone();
    assert!(store.delivery_save(Some(&d), &diverged).unwrap());

    advance(&store, &cfg, &diverged, OffsetDateTime::now_utc()).unwrap();

    assert_eq!(sha(dir.path(), "main").unwrap(), moved);
    assert_eq!(sha(dir.path(), "feature").unwrap(), d.head);
    let after = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(after.state, State::Blocked);
    assert!(
        after.detail.starts_with(MERGE_BASE_FAILURE),
        "{}",
        after.detail
    );
    assert_eq!(
        classify_delivery_failure(after.state, &after.detail, None),
        Some(RepairClass::MergeBase)
    );
    assert!(target_advanced_events(&store, &d).is_empty());
    assert_eq!(store.get(d.task_id).unwrap().unwrap().status, Status::Done);
}

/// ADR-0120 付記（fallback の解除）: IntegrationRepair を打ち切った fallback（未同期 HEAD の review、
/// merge candidate なし）から、[merge-base] 局所修復が main を取り込み、再 review の同期で merge candidate を
/// 記録して配送まで進む。途中で自動の再レビュー上限（[needs-human]）に達せず、review の試行回数も使わない。
///
/// review は dispatcher（review_spawn の `task_ops::delivery::begin` と同期・review_verdict の判定記録）が行う。
/// ここではその delivery 行への書き込みを同じ形で再現し、同期するかは fallback の解除規則（打ち切り時の
/// HEAD から動いた、または target が HEAD の祖先）で決める。配送側（check_candidate・make_repair・
/// validate_candidate・merge）は本物を通す。
#[test]
fn integration_repair_fallback_delivers() {
    use task_core::{DeliveryStore, Trigger};
    let (store, dir, d) = merge_queued_delivery();
    let p = dir.path();
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = cfg_for(p, &tmp.path().join("releases"));
    cfg.selfdeploy.delivery.auto_resolve.enabled = false;
    let attempts = store.get(d.task_id).unwrap().unwrap().attempts;
    // main が同じ行を変えて分岐し、review 前同期の衝突を IntegrationRepair が直せずに打ち切った。
    fs::write(p.join("feature"), "main side").unwrap();
    git_text(p, &["add", "feature"]).unwrap();
    git_text(p, &["commit", "-m", "main diverged"]).unwrap();
    let diverged = sha(p, "main").unwrap();
    store
        .append_event(
            d.task_id,
            &Event::IntegrationRepairExhausted {
                work_unit_id: Some("integration-repair-1".into()),
                repo_id: d.repo_id,
                target_sha: diverged.clone(),
                before_sha: d.head.clone(),
                attempt: task_ops::delivery::MAX_INTEGRATION_REPAIRS,
                reason: task_core::IntegrationRepairExhaustReason::PlanIssue,
                rollback_to_sha: None,
                fallback: true,
            },
        )
        .unwrap();
    // dispatcher の review: begin が行を作り直し、fallback が解けていれば同期の結果を候補に記録し、
    // reviewer の合格で MergeQueued にする。
    let review_pass = |old: &Delivery| -> (Delivery, bool) {
        let main = sha(p, "main").unwrap();
        let head = sha(p, "feature").unwrap();
        let released =
            head != d.head || git_text(p, &["merge-base", "--is-ancestor", &main, &head]).is_ok();
        let mut next = old.clone();
        next.state = State::MergeQueued;
        next.decision = Some(true);
        next.base = main.clone();
        next.head = head.clone();
        next.notification = None;
        (next.target_sha, next.reviewed_sha, next.merge_candidate_sha) = if released {
            (Some(main), Some(head.clone()), Some(head))
        } else {
            (None, None, None)
        };
        assert!(store.delivery_save(Some(old), &next).unwrap());
        store
            .apply_transition(d.task_id, Trigger::ReviewPass, None)
            .unwrap();
        (next, released)
    };
    let current = store.delivery_get(d.task_id).unwrap().unwrap();
    store
        .apply_transition(d.task_id, Trigger::Rereview, None)
        .unwrap();
    let (fallback, released) = review_pass(&current);
    assert!(!released);
    assert_eq!(fallback.merge_candidate_sha, None);

    // 候補なし・target が HEAD の祖先でない: 再レビューに戻さず [merge-base] へ。
    advance(&store, &cfg, &fallback, OffsetDateTime::now_utc()).unwrap();
    let blocked = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(blocked.state, State::Blocked);
    assert!(
        blocked.detail.starts_with(MERGE_BASE_FAILURE),
        "{}",
        blocked.detail
    );
    assert!(target_advanced_events(&store, &d).is_empty());
    assert_eq!(sha(p, "main").unwrap(), diverged);
    // 局所修復（make_repair）を起こす。
    advance(&store, &cfg, &blocked, OffsetDateTime::now_utc()).unwrap();
    let units = store.work_units_for(d.task_id).unwrap();
    let repair = units
        .iter()
        .find(|u| u.kind == WorkUnitKind::Repair)
        .unwrap()
        .clone();
    assert!(repair.spec.title.starts_with("repair (merge_base):"));
    assert_eq!(repair.status, WorkUnitStatus::Ready);
    assert_eq!(store.get(d.task_id).unwrap().unwrap().status, Status::Ready);

    // 修復 worker が main を merge して衝突を解く（作業ツリーの未追跡物は無い）。
    git_text(p, &["checkout", "feature"]).unwrap();
    assert!(git_text(p, &["merge", "--no-edit", "main"]).is_err());
    fs::write(p.join("feature"), "implemented on main side").unwrap();
    git_text(p, &["add", "feature"]).unwrap();
    git_text(p, &["commit", "--no-edit"]).unwrap();
    let repaired = sha(p, "feature").unwrap();
    git_text(p, &["checkout", "main"]).unwrap();
    let mut done = repair.clone();
    done.status = WorkUnitStatus::Done;
    store
        .work_unit_transition(
            d.task_id,
            done,
            Event::WorkUnitTransitioned {
                work_unit_id: repair.id.clone(),
                key: repair.key.clone(),
                from: WorkUnitStatus::Ready,
                to: WorkUnitStatus::Done,
                reason: "test_repair_completed".into(),
                run_id: None,
            },
        )
        .unwrap();
    for trigger in [Trigger::Dispatch, Trigger::WorkerDone] {
        store.apply_transition(d.task_id, trigger, None).unwrap();
    }
    // 修復後の review: HEAD が打ち切り時から動き、main を祖先に含むので fallback が解けて候補が記録される。
    let blocked = store.delivery_get(d.task_id).unwrap().unwrap();
    let (synced, released) = review_pass(&blocked);
    assert!(released);
    assert_eq!(
        synced.merge_candidate_sha.as_deref(),
        Some(repaired.as_str())
    );
    assert_eq!(synced.target_sha.as_deref(), Some(diverged.as_str()));
    assert!(validate_candidate(p, &synced).is_ok());
    assert_eq!(
        check_candidate(p, &synced).unwrap(),
        Candidate::Fresh,
        "the repaired, synced candidate must not ask for another re-review"
    );
    let script = cfg
        .selfdeploy
        .releases_dir
        .parent()
        .unwrap()
        .join("current/scripts/prepare.sh");
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    advance(&store, &cfg, &synced, OffsetDateTime::now_utc()).unwrap();
    let delivered = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(delivered.state, State::Preparing, "{}", delivered.detail);
    assert_eq!(sha(p, "main").unwrap(), repaired);
    assert_eq!(delivered.release.as_deref(), Some(&repaired[..12]));
    // 自動の再レビューの上限に触れず、review の不合格も試行回数の消費も無い。
    assert!(
        (target_advanced_events(&store, &d).len() as u32) < task_ops::delivery::MAX_TARGET_RESYNCS
    );
    assert!(!delivered.detail.starts_with("[needs-human]"));
    let events: Vec<Event> = store
        .events_for(d.task_id)
        .unwrap()
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::Transitioned { reason, .. } if reason == "review_fail"))
    );
    let task = store.get(d.task_id).unwrap().unwrap();
    assert_eq!(task.status, Status::Done);
    assert_eq!(task.attempts, attempts);
}

// ---- ADR 2026-10-02-parallel-integration-auto-resolve D1d・D2・D5: merge_base 系の失敗の自動解消。ここから ----

/// base に `file` を置き、main と feature がそれぞれ `main_text`・`feature_text` で書き換えた repo と、
/// main が進んだので `merge_base` 失敗で止まった配送（承認済み・task は done）。
fn auto_resolve_blocked(
    file: &str,
    base_text: &str,
    main_text: &str,
    feature_text: &str,
) -> (task_core::SqliteStore, tempfile::TempDir, Delivery) {
    use task_core::DeliveryStore;
    let (store, mut d) = stored_delivery();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git_text(p, &["init", "-b", "main"]).unwrap();
    git_text(p, &["config", "user.email", "test@example.invalid"]).unwrap();
    git_text(p, &["config", "user.name", "test"]).unwrap();
    let path = p.join(file);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, base_text).unwrap();
    git_text(p, &["add", "."]).unwrap();
    git_text(p, &["commit", "-m", "base"]).unwrap();
    d.base = sha(p, "HEAD").unwrap();
    git_text(p, &["checkout", "-b", "feature"]).unwrap();
    fs::write(&path, feature_text).unwrap();
    git_text(p, &["commit", "-am", "feature edit"]).unwrap();
    d.head = sha(p, "HEAD").unwrap();
    git_text(p, &["checkout", "main"]).unwrap();
    fs::write(&path, main_text).unwrap();
    git_text(p, &["commit", "--allow-empty", "-am", "main edit"]).unwrap();
    d.state = State::Blocked;
    d.detail = format!("{MERGE_BASE_FAILURE}対象コミットまたは既定ブランチが変わりました");
    store.delivery_save(None, &d).unwrap();
    (store, dir, d)
}

fn auto_resolve_comments(store: &task_core::SqliteStore, d: &Delivery) -> Vec<String> {
    store
        .comments_for(d.task_id)
        .unwrap()
        .into_iter()
        .map(|c| c.body)
        .filter(|b| b.starts_with("[delivery-auto-resolve"))
        .collect()
}

/// 未回答の統合の依頼（task の両端の head の組ごとに 1 件）の数。
fn open_integration_count(store: &task_core::SqliteStore, task_id: TaskId) -> usize {
    use task_core::TaskStore;
    store
        .open_integration_requests()
        .unwrap()
        .iter()
        .filter(|row| row.task_id == task_id)
        .count()
}

/// 未回答の統合の依頼を 1 件だけ読む（ADR parallel integration D4: 通知ではなく追記事象）。
fn open_integration_request(
    store: &task_core::SqliteStore,
    task_id: TaskId,
) -> task_core::integration_request::IntegrationRequest {
    use task_core::TaskStore;
    let rows = store.open_integration_requests().unwrap();
    let rows: Vec<_> = rows.iter().filter(|row| row.task_id == task_id).collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    match &rows[0].event {
        task_core::Event::IntegrationRequested { request, .. } => (**request).clone(),
        other => panic!("integration_requested を期待: {other:?}"),
    }
}

/// 統合の依頼の notice が一般通知に載っていないこと。
fn no_integration_notice(store: &task_core::SqliteStore) -> bool {
    use task_core::NoticeStore;
    store
        .notice_list(&task_core::NoticeQuery::default())
        .unwrap()
        .items
        .iter()
        .all(|n| {
            n.target
                .as_ref()
                .is_none_or(|t| t.kind != "integration_request")
        })
}

fn no_notice(store: &task_core::SqliteStore) -> bool {
    use task_core::NoticeStore;
    store
        .notice_list(&task_core::NoticeQuery::default())
        .unwrap()
        .items
        .is_empty()
}

#[test]
fn auto_resolve_progress_append_conflict_is_resolved_and_regated() {
    use task_core::DeliveryStore;
    let (store, dir, d) = auto_resolve_blocked(
        "docs/PROGRESS.md",
        "# PROGRESS\n- old\n",
        "# PROGRESS\n- old\n- main entry\n",
        "# PROGRESS\n- old\n- feature entry\n",
    );
    let p = dir.path();
    let main_before = sha(p, "main").unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_for(p, &tmp.path().join("releases"));
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();

    let requeued = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(requeued.state, State::MergeQueued);
    assert_eq!(requeued.base, main_before);
    assert_ne!(requeued.head, d.head);
    assert_eq!(requeued.release, None);
    // task branch は新しい候補へ早送りされ、main と旧 head の両方を含む。
    assert_eq!(sha(p, "feature").unwrap(), requeued.head);
    git_text(p, &["merge-base", "--is-ancestor", &d.head, &requeued.head]).unwrap();
    git_text(
        p,
        &["merge-base", "--is-ancestor", &main_before, &requeued.head],
    )
    .unwrap();
    assert_eq!(
        git_text(p, &["show", &format!("{}:docs/PROGRESS.md", requeued.head)]).unwrap(),
        "# PROGRESS\n- old\n- main entry\n- feature entry"
    );
    // 本番 checkout は動かさず、scratch の worktree も残さない。
    assert_eq!(sha(p, "HEAD").unwrap(), main_before);
    assert_eq!(
        git_text(p, &["status", "--porcelain"]).unwrap(),
        "",
        "production checkout must stay clean"
    );
    assert_eq!(
        git_text(p, &["worktree", "list", "--porcelain"])
            .unwrap()
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        1
    );
    let comments = auto_resolve_comments(&store, &d);
    assert_eq!(comments.len(), 1, "{comments:?}");
    assert!(comments[0].starts_with("[delivery-auto-resolved] docs/PROGRESS.md (Record)"));
    // 依頼も局所修復も作らない。
    assert!(no_notice(&store));
    assert!(store.work_units_for(d.task_id).unwrap().is_empty());
    assert_eq!(store.get(d.task_id).unwrap().unwrap().status, Status::Done);

    // The auto-resolved commit has not been reviewed. Candidate validation sends it
    // through a fresh review before any ff-only delivery.
    advance(&store, &cfg, &requeued, OffsetDateTime::now_utc()).unwrap();
    let reviewing = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(reviewing.state, State::Reviewing);
    assert_eq!(reviewing.merge_candidate_sha, None);
    assert_eq!(sha(p, "main").unwrap(), main_before);
}

#[test]
fn auto_resolve_code_conflict_requests_integration_and_aborts_merge() {
    use task_core::DeliveryStore;
    let (store, dir, d) = auto_resolve_blocked(
        "src/lib.rs",
        "fn f() -> u32 { 0 }\n",
        "fn f() -> u32 { 1 }\n",
        "fn f() -> u32 { 2 }\n",
    );
    let p = dir.path();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_for(p, &tmp.path().join("releases"));
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(sha(p, "feature").unwrap(), d.head);
    assert_eq!(git_text(p, &["status", "--porcelain"]).unwrap(), "");
    let comments = auto_resolve_comments(&store, &d);
    assert_eq!(comments.len(), 1, "{comments:?}");
    assert!(comments[0].starts_with("[delivery-auto-resolve-fallback] 人の判断が必要"));
    assert!(store.work_units_for(d.task_id).unwrap().is_empty());
    let still = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(still.state, State::Blocked);
    assert!(still.detail.starts_with("[needs-human] 統合の依頼:"));
    let request = open_integration_request(&store, d.task_id);
    assert!(request.conflict_files.iter().any(|f| f == "src/lib.rs"));
    assert_eq!(request.source_branch, "feature");
    assert_eq!(request.target_branch, "main");
    assert!(no_integration_notice(&store));
    assert_eq!(
        git_text(p, &["worktree", "list", "--porcelain"])
            .unwrap()
            .lines()
            .filter(|l| l.starts_with("worktree "))
            .count(),
        1
    );
    advance(&store, &cfg, &still, OffsetDateTime::now_utc()).unwrap();
    // 再 tick でも同じ両端の依頼は 1 件のまま。
    assert_eq!(open_integration_count(&store, d.task_id), 1);
    assert!(no_integration_notice(&store));
}

#[test]
fn auto_resolve_stops_at_max_attempts_and_when_disabled() {
    use task_core::DeliveryStore;
    for disabled in [false, true] {
        let (store, dir, d) = auto_resolve_blocked(
            "docs/PROGRESS.md",
            "# PROGRESS\n",
            "# PROGRESS\n- main\n",
            "# PROGRESS\n- feature\n",
        );
        let p = dir.path();
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = cfg_for(p, &tmp.path().join("releases"));
        cfg.selfdeploy.delivery.auto_resolve.max_attempts = 1;
        cfg.selfdeploy.delivery.auto_resolve.enabled = !disabled;
        if !disabled {
            // 前の試行が 1 回ある（上限 1 に到達）。
            store
                .comment_add(
                    &task_core::TaskComment {
                        id: task_core::CommentId::new(),
                        task_id: d.task_id,
                        author_kind: task_core::CommentAuthorKind::System,
                        author: None,
                        run_id: None,
                        created_at: OffsetDateTime::now_utc(),
                        body: "[delivery-auto-resolve-fallback] 衝突なしの main 追従".into(),
                    },
                    None,
                )
                .unwrap();
        }
        let before = auto_resolve_comments(&store, &d).len();
        advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
        assert_eq!(sha(p, "feature").unwrap(), d.head, "disabled={disabled}");
        assert_eq!(auto_resolve_comments(&store, &d).len(), before);
        assert_eq!(
            store.delivery_get(d.task_id).unwrap().unwrap().state,
            State::Blocked
        );
        if disabled {
            assert!(
                store
                    .work_units_for(d.task_id)
                    .unwrap()
                    .iter()
                    .any(|u| u.spec.title.starts_with("repair (merge_base):"))
            );
        } else {
            assert!(store.work_units_for(d.task_id).unwrap().is_empty());
            assert!(
                store
                    .delivery_get(d.task_id)
                    .unwrap()
                    .unwrap()
                    .detail
                    .starts_with("[needs-human] 統合の依頼:")
            );
            let request = open_integration_request(&store, d.task_id);
            assert!(request.reason.contains("1 回"));
            assert!(no_integration_notice(&store));
        }
    }
}

#[test]
fn auto_resolve_local_repair_limit_requests_integration() {
    use task_core::DeliveryStore;
    let (store, dir, d) = auto_resolve_blocked(
        "src/lib.rs",
        "fn f() -> u32 { 0 }\n",
        "fn f() -> u32 { 1 }\n",
        "fn f() -> u32 { 2 }\n",
    );
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = cfg_for(dir.path(), &tmp.path().join("releases"));
    cfg.selfdeploy.delivery.auto_resolve.enabled = false;
    cfg.execution.max_repairs = 0;
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    let still = store.delivery_get(d.task_id).unwrap().unwrap();
    assert!(still.detail.starts_with("[needs-human] 統合の依頼:"));
    let request = open_integration_request(&store, d.task_id);
    assert!(request.reason.contains("局所修復が上限"));
    assert!(request.reason.contains("全体 0/0 回"));
    assert!(no_integration_notice(&store));
    assert_eq!(open_integration_count(&store, d.task_id), 1);
    assert!(store.work_units_for(d.task_id).unwrap().is_empty());
    assert_eq!(
        git_text(dir.path(), &["status", "--porcelain"]).unwrap(),
        ""
    );
}

#[test]
fn auto_resolve_clean_main_follow_is_counted_and_left_to_the_legacy_path() {
    let (store, dir, d) = auto_resolve_blocked(
        "docs/PROGRESS.md",
        "# PROGRESS\n",
        "# PROGRESS\n",
        "# PROGRESS\n- feature\n",
    );
    let p = dir.path();
    // main の変更は別ファイル（衝突なし）。
    fs::write(p.join("other"), "x").unwrap();
    git_text(p, &["add", "."]).unwrap();
    git_text(p, &["commit", "-m", "other"]).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let cfg = cfg_for(p, &tmp.path().join("releases"));
    advance(&store, &cfg, &d, OffsetDateTime::now_utc()).unwrap();
    assert_eq!(sha(p, "feature").unwrap(), d.head);
    let comments = auto_resolve_comments(&store, &d);
    assert_eq!(comments.len(), 1, "{comments:?}");
    assert!(comments[0].contains("衝突なしの main 追従"));
    assert!(no_notice(&store));
}
