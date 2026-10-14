//! ADR 2026-10-14-local-machine-settings D5: dispatcher の足回りは `WorkspaceRoots` で解き、足回りを初めて
//! 用意するときに root を固定する。

use super::*;
use task_core::local_machine::WorkspaceRoots;

fn set_store_root(store: &Arc<dyn TaskStore>, root: &std::path::Path) {
    store
        .local_settings_set(
            &task_core::LocalSettingsOverrides {
                workspace_root: Some(root.to_string_lossy().into_owned()),
                max_concurrency: None,
            },
            "admin",
            OffsetDateTime::now_utc(),
        )
        .unwrap();
}

/// 新しい task は store の root に固定され、その後 store の root を変えても固定された root から解く
/// （走行中 run の cwd・検査ログが動かない）。子 task は親の固定を継ぐ。
#[test]
fn dispatcher_pins_new_task_to_store_root_and_child_inherits() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let (cfg, first, second) = (
        dir.path().join("cfg"),
        dir.path().join("first"),
        dir.path().join("second"),
    );
    let parent = remote_task(dir.path(), None, vec![]);
    store.insert(&parent).unwrap();
    let mut child = remote_task(dir.path(), None, vec![]);
    child.parent_id = Some(parent.id);
    store.insert(&child).unwrap();
    let mut d = dispatcher(store.clone(), done_adapter(), 1);
    d.config.workspace_roots = WorkspaceRoots::new(cfg.clone(), None);

    set_store_root(&store, &first);
    d.pin_task_root(&parent).unwrap();
    assert_eq!(d.task_dir(&parent), Some(first.join(parent.id.to_string())));
    assert_eq!(
        d.check_log_root(parent.id),
        first.join(parent.id.to_string())
    );

    set_store_root(&store, &second);
    // 固定済みの親は動かない。子は親の root を継ぐ（今の store の root ではない）。
    assert_eq!(d.task_dir(&parent), Some(first.join(parent.id.to_string())));
    d.pin_task_root(&child).unwrap();
    assert_eq!(d.task_dir(&child), Some(first.join(child.id.to_string())));
    assert_eq!(
        store.task_workspace_root_get(&child.id).unwrap().as_deref(),
        first.to_str()
    );
}

/// この ADR 以前の task（固定なし、設定ファイルの root に足回りがある）は固定せず、store の root を変えても
/// 設定ファイルの root から解く。
#[test]
fn dispatcher_keeps_existing_unpinned_workspace_on_config_root() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let (cfg, moved) = (dir.path().join("cfg"), dir.path().join("moved"));
    let task = remote_task(dir.path(), None, vec![]);
    store.insert(&task).unwrap();
    let existing = cfg.join(task.id.to_string());
    std::fs::create_dir_all(existing.join("runs")).unwrap();
    let mut d = dispatcher(store.clone(), done_adapter(), 1);
    d.config.workspace_roots = WorkspaceRoots::new(cfg.clone(), None);

    set_store_root(&store, &moved);
    d.pin_task_root(&task).unwrap();
    assert_eq!(store.task_workspace_root_get(&task.id).unwrap(), None);
    assert_eq!(d.task_dir(&task), Some(existing.clone()));
    assert_eq!(d.check_log_root(task.id), existing);
}

/// disk watch の census は設定ファイルの root と固定されうる root（監査の流れに出た store の root）を全部走査する。
#[test]
fn disk_watch_census_roots_cover_config_and_store_roots() {
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let (cfg, first, second) = (
        dir.path().join("cfg"),
        dir.path().join("first"),
        dir.path().join("second"),
    );
    let roots = WorkspaceRoots::new(cfg.clone(), None);
    set_store_root(&store, &first);
    set_store_root(&store, &second);
    assert_eq!(
        crate::disk_watch::task_workspace_roots(store.as_ref(), &roots),
        vec![cfg, second, first]
    );
}
