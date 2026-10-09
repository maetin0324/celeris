use super::*;
use crate::workspace_prune::tests::insert_task;
use task_core::SqliteStore;
use time::Duration;

/// `<task_dir>/repos/<repo>/target/debug/.cargo-lock` と deps を 1 つ持つ作業場所を作る。
fn make_target(task_dir: &Path, repo: &str, tag: bool) -> PathBuf {
    let target = task_dir.join("repos").join(repo).join("target");
    std::fs::create_dir_all(target.join("debug").join("deps")).unwrap();
    std::fs::write(target.join("debug").join(CARGO_LOCK), "").unwrap();
    std::fs::write(
        target
            .join("debug")
            .join("deps")
            .join("libx-0123456789abcdef.rlib"),
        vec![0u8; 8192],
    )
    .unwrap();
    if tag {
        std::fs::write(
            target.join(CACHEDIR_TAG),
            "Signature: 8a477f597d28d172789f06886806bc55",
        )
        .unwrap();
    }
    std::fs::create_dir_all(task_dir.join("repos").join(repo).join("src")).unwrap();
    target
}

#[test]
fn workspace_targets_detects_cargo_targets_by_tag_or_profile_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let with_tag = tmp.path().join("a");
    std::fs::create_dir_all(&with_tag).unwrap();
    std::fs::write(with_tag.join(CACHEDIR_TAG), "x").unwrap();
    assert!(is_cargo_target(&with_tag));
    let triple = tmp.path().join("b");
    std::fs::create_dir_all(triple.join("x86_64-unknown-linux-gnu").join("release")).unwrap();
    std::fs::write(
        triple
            .join("x86_64-unknown-linux-gnu")
            .join("release")
            .join(CARGO_LOCK),
        "",
    )
    .unwrap();
    assert!(is_cargo_target(&triple));
    // ただの `target` という名前の dir（cargo のものではない）は対象外。
    let plain = tmp.path().join("c");
    std::fs::create_dir_all(plain.join("debug")).unwrap();
    assert!(!is_cargo_target(&plain));
    // symlink は辿らない。
    let link = tmp.path().join("d");
    std::os::unix::fs::symlink(&with_tag, &link).unwrap();
    assert!(!is_cargo_target(&link));
}

#[test]
fn workspace_targets_only_finished_tasks_past_the_grace_are_listed() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = OffsetDateTime::now_utc();
    let old = now - Duration::hours(7);
    let fresh = now - Duration::hours(1);
    let done_old = insert_task(&store, Status::Done, old);
    let cancelled_old = insert_task(&store, Status::Cancelled, old);
    let failed_old = insert_task(&store, Status::Failed, old);
    let done_fresh = insert_task(&store, Status::Done, fresh);
    let running = insert_task(&store, Status::Running, old);
    for id in [done_old, cancelled_old, failed_old, done_fresh, running] {
        make_target(&tmp.path().join(id.to_string()), "agent-platform", true);
    }
    let found = finished_task_targets(&store, tmp.path(), now, 6 * 3600).unwrap();
    let mut ids: Vec<TaskId> = found.iter().map(|f| f.task_id).collect();
    ids.sort_by_key(|i| i.to_string());
    let mut expected = vec![done_old, cancelled_old, failed_old];
    expected.sort_by_key(|i| i.to_string());
    assert_eq!(ids, expected);
    for f in &found {
        assert_eq!(f.targets.len(), 1);
    }
}

#[test]
fn workspace_targets_locked_target_is_kept_and_unlocked_is_moved() {
    let tmp = tempfile::tempdir().unwrap();
    let task_dir = tmp.path().join("t");
    let target = make_target(&task_dir, "r", false);
    // cargo の build 中を模して、別の fd で profile の lock を持つ。
    let held = Flock::lock(
        File::open(target.join("debug").join(CARGO_LOCK)).unwrap(),
        FlockArg::LockExclusiveNonblock,
    )
    .unwrap();
    assert_eq!(
        remove_target(&target, "x1", true).unwrap(),
        RemoveTarget::Locked
    );
    assert!(target.is_dir());
    drop(held);
    // dry run は木を変えない。
    let dry = remove_target(&target, "x2", false).unwrap();
    assert!(matches!(dry, RemoveTarget::Moved { bytes, .. } if bytes >= 8192));
    assert!(target.is_dir());
    let RemoveTarget::Moved { moved, .. } = remove_target(&target, "x3", true).unwrap() else {
        panic!("expected moved");
    };
    assert!(!target.exists());
    assert!(moved.is_dir());
    // 次の回は残り（`.deleting-*`）として見つかる。ソースは残る。
    let (targets, leftovers) = task_targets(&task_dir);
    assert!(targets.is_empty());
    assert_eq!(leftovers, vec![moved]);
    assert!(task_dir.join("repos").join("r").join("src").is_dir());
}

#[test]
fn workspace_prune_keeps_a_locked_cargo_target() {
    let tmp = tempfile::tempdir().unwrap();
    let task_dir = tmp.path().join("t");
    let target = make_target(&task_dir, "r", true);
    let candidate = crate::workspace_prune::PruneCandidate {
        task_id: TaskId::new(),
        task_dir: task_dir.clone(),
        paths: vec![target.clone()],
    };
    let held = Flock::lock(
        File::open(target.join("debug").join(CARGO_LOCK)).unwrap(),
        FlockArg::LockExclusiveNonblock,
    )
    .unwrap();
    assert!(crate::workspace_prune::prune(&candidate).is_empty());
    assert!(target.is_dir());
    drop(held);
    assert_eq!(
        crate::workspace_prune::prune(&candidate),
        vec![target.clone()]
    );
    assert!(!target.exists());
}

#[test]
fn workspace_targets_finds_units_and_ignores_symlinked_ancestors_and_unowned_trash() {
    let tmp = tempfile::tempdir().unwrap();
    let task = tmp.path().join("task");
    let target = make_target(&task.join("wu/leaf"), "r", true);
    assert_eq!(task_targets(&task).0, vec![target.clone()]);
    let other = tmp.path().join("other");
    make_target(&other, "r", true);
    std::os::unix::fs::symlink(&other, task.join("wu/link")).unwrap();
    std::fs::create_dir_all(task.join("repos")).unwrap();
    std::os::unix::fs::symlink(other.join("repos/r"), task.join("repos/link")).unwrap();
    let trash = target.parent().unwrap().join(".deleting-important");
    std::fs::create_dir(&trash).unwrap();
    let (found, leftovers) = task_targets(&task);
    assert_eq!(found, vec![target]);
    assert!(leftovers.is_empty());
    let link = tmp.path().join("task-link");
    std::os::unix::fs::symlink(&task, &link).unwrap();
    assert!(task_targets(&link).0.is_empty());
}

#[test]
fn workspace_targets_redirected_lock_is_not_safe_to_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let target = make_target(tmp.path(), "r", true);
    let path = target.join("debug/.cargo-lock");
    std::fs::remove_file(&path).unwrap();
    std::fs::write(tmp.path().join("elsewhere"), "").unwrap();
    std::os::unix::fs::symlink(tmp.path().join("elsewhere"), path).unwrap();
    assert_eq!(
        remove_target(&target, "unsafe", true).unwrap(),
        RemoveTarget::Locked
    );
    assert!(target.exists());
}
