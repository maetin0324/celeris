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
    git_text(dir.path(), &["commit", "--allow-empty", "-m", "main moved"]).unwrap();
    let moved_main = sha(dir.path(), "main").unwrap();
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
    assert!(repair.spec.objective.contains(&moved_main));
    assert!(repair.spec.objective.contains(&blocked.detail));
    assert!(!repair.spec.objective.contains(transcript_marker));
    assert!(!repair.spec.objective.contains("implement"));
    assert!(!repair.spec.objective.contains("work_units"));

    // Simulate the repair worker rebasing feature onto the new main and passing review.
    git_text(dir.path(), &["checkout", "feature"]).unwrap();
    git_text(dir.path(), &["rebase", "main"]).unwrap();
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
    assert_eq!(requeued.base, moved_main);
    assert_eq!(requeued.head, repaired_head);
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

// ---- ADR-0137 D1d・D2・D5: merge_base 系の失敗の自動解消。ここから ----

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

    // gate は新しい候補 SHA で start_prepare からやり直す（旧 release を流用しない）。
    let script = cfg
        .selfdeploy
        .releases_dir
        .parent()
        .unwrap()
        .join("current/scripts/prepare.sh");
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    advance(&store, &cfg, &requeued, OffsetDateTime::now_utc()).unwrap();
    let preparing = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(preparing.state, State::Preparing);
    assert_eq!(preparing.release.as_deref(), Some(&requeued.head[..12]));
    assert_eq!(sha(p, "main").unwrap(), requeued.head);
}

#[test]
fn auto_resolve_code_conflict_falls_back_to_repair_without_touching_the_branch() {
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
    // 従来経路: merge_base の局所修復を作る。
    let units = store.work_units_for(d.task_id).unwrap();
    assert!(
        units
            .iter()
            .any(|u| u.spec.title.starts_with("repair (merge_base):"))
    );
    let still = store.delivery_get(d.task_id).unwrap().unwrap();
    assert_eq!(still.state, State::Blocked);
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
        cfg.delivery.auto_resolve.max_attempts = 1;
        cfg.delivery.auto_resolve.enabled = !disabled;
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
        assert!(
            store
                .work_units_for(d.task_id)
                .unwrap()
                .iter()
                .any(|u| u.spec.title.starts_with("repair (merge_base):")),
            "disabled={disabled}"
        );
    }
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
