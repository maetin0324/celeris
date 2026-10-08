//! ADR 2026-10-08-browser-prod-enablement D2 末尾・D4: 保存 browser policy の無い browser task は台帳より先に
//! `browser_policy_missing` で dispatch の前に止まり（worker・LLM を起こさない）、`PUT /tasks/{id}/browser/policy`
//! と同じ store の書き込みの後の tick で ready に戻る。worker が `browser policy rejected` を返しても
//! infra_requeue にしない。fake adapter・注入の `LedgerWatch`・一時 file だけ（userns・外部ネットワーク不要）。

use super::browser_allowed_domains::browser_org;
use super::browser_ledger_gate::{
    CountingBrowserAdapter, RELEASE, prerequisite_events, tick_n, write_ledger,
};
use super::*;
use task_core::browser_prerequisite::BrowserPrerequisiteCode;
use task_worker::browser_ledger::LedgerWatch;

/// 有効な台帳（policy だけが足りない状態にする）。
fn valid_ledger(dir: &std::path::Path) -> LedgerWatch {
    let ledger = dir.join("conformance.json");
    write_ledger(&ledger, RELEASE, true);
    LedgerWatch::fixed(Some(ledger), Some(RELEASE.into()), None)
}

/// requirements 無しで起票した task に後から browser skill を足す（`PATCH` の skills 編集や、自動付与より前に
/// 作られた task と同じ形）。起票時の自動付与は requirements が無いので何も付けない。
fn browser_task_without_policy(store: &Arc<dyn TaskStore>, dir: &std::path::Path) -> Task {
    let task = new_task(
        dir,
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    let mut edited = task.clone();
    edited.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    let task = store
        .update_task(
            &edited,
            Event::Edited {
                fields: vec!["skills".into()],
                by: "human".into(),
            },
        )
        .unwrap();
    assert!(task.requirements.browser.is_none());
    assert!(store.browser_task_policy_get(task.id).unwrap().is_none());
    task
}

fn origin_policy(origin: &str) -> task_core::BrowserTaskPolicy {
    task_core::BrowserTaskPolicy {
        policy_id: "web-human".into(),
        revision: 1,
        domain_mode: task_core::BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: vec![origin.into()],
        allowed_actions: vec![task_core::BrowserAction::Navigate],
        approval_actions: vec![],
        credential_policy_ids: vec![],
        artifact_policy_id: None,
    }
}

/// 保存 policy が無い: 台帳が有効でも dispatch の前に `blocked(browser_prerequisite)` で止まり、adapter は
/// 呼ばれず、attempts は変わらず、tick を重ねても event を再追記しない。
#[tokio::test]
async fn browser_policy_missing_blocks_before_dispatch_without_waking_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let ledger_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = browser_task_without_policy(&store, dir.path());
    let adapter = Arc::new(CountingBrowserAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.set_browser_ledger_watch(valid_ledger(ledger_dir.path()));
    tick_n(&mut d, 1).await;
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Blocked, 0));
    assert_eq!(
        prerequisite_events(&store, task.id),
        [BrowserPrerequisiteCode::BrowserPolicyMissing]
    );
    // 再 tick（task ごとの見直しの周期を越える）でも同じ code を再追記しない。
    tick_n(&mut d, 40).await;
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Blocked, 0));
    assert_eq!(
        transition_reasons(&store, task.id),
        ["browser_prerequisite"]
    );
    assert_eq!(
        prerequisite_events(&store, task.id),
        [BrowserPrerequisiteCode::BrowserPolicyMissing]
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert!(
        BrowserPrerequisiteCode::BrowserPolicyMissing
            .message()
            .contains("task 詳細")
    );
}

/// policy を入れる（`PUT /tasks/{id}/browser/policy` と同じ store の書き込み。止まった task も受ける）と、
/// 次の見直しの tick で `BrowserPrerequisiteResumed` を足して ready に戻り、dispatch される。
#[tokio::test]
async fn browser_policy_missing_resumes_after_policy_put() {
    let dir = tempfile::tempdir().unwrap();
    let ledger_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = browser_task_without_policy(&store, dir.path());
    let adapter = Arc::new(CountingBrowserAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.set_browser_ledger_watch(valid_ledger(ledger_dir.path()));
    tick_n(&mut d, 1).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Blocked);

    store
        .browser_task_policy_set(task.id, &origin_policy("https://app.example.com"))
        .unwrap();
    let resumed = |store: &Arc<dyn TaskStore>| {
        store.events_for(task.id).unwrap().iter().any(|(_, e)| {
            matches!(
                e,
                Event::BrowserPrerequisiteResumed { code }
                    if *code == BrowserPrerequisiteCode::BrowserPolicyMissing
            )
        })
    };
    // task ごとの code は一定 tick ごとに見直す（台帳が変わらなくても）。周期の 1 回分で必ず戻る。
    for _ in 0..31 {
        if resumed(&store) {
            break;
        }
        tick_n(&mut d, 1).await;
    }
    assert!(resumed(&store));
    let reasons = transition_reasons(&store, task.id);
    assert_eq!(
        &reasons[..3],
        [
            "browser_prerequisite",
            "browser_prerequisite_resolved",
            "dispatch"
        ]
    );
    assert!(!reasons.iter().any(|r| r == "infra_requeue"));
    assert_eq!(store.get(task.id).unwrap().unwrap().attempts, 0);
}

/// 保存 policy が task の origin と交わらない（requirements の外だけ）も同じ code で止める。
#[tokio::test]
async fn browser_policy_missing_policy_outside_task_origins_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let ledger_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    task.requirements.browser = Some(task_core::BrowserRequirements {
        allowed_domains: vec!["https://app.example.com".into()],
    });
    store.insert(&task).unwrap();
    store
        .browser_task_policy_set(task.id, &origin_policy("https://other.example.org"))
        .unwrap();
    let adapter = Arc::new(CountingBrowserAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.set_browser_ledger_watch(valid_ledger(ledger_dir.path()));
    tick_n(&mut d, 3).await;
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Blocked, 0));
    assert_eq!(
        prerequisite_events(&store, task.id),
        [BrowserPrerequisiteCode::BrowserPolicyMissing]
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
}

/// worker 側で `browser policy rejected`（task policy と担当の grant が交わらない）が返っても infra_requeue に
/// 分類せず、`browser_prerequisite(browser_policy_missing)` で止める（attempts 不変）。
#[tokio::test]
async fn browser_policy_missing_worker_rejection_is_not_infra_requeue() {
    let dir = tempfile::tempdir().unwrap();
    let ledger_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    browser_org(store.as_ref(), &["https://grant-only.example.net"]);
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.assignee = Some("browser-execution".into());
    task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    task.requirements.browser = Some(task_core::BrowserRequirements {
        allowed_domains: vec!["https://app.example.com".into()],
    });
    store.insert(&task).unwrap();
    assert!(store.browser_task_policy_get(task.id).unwrap().is_some());
    let adapter = Arc::new(CountingBrowserAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.set_browser_ledger_watch(valid_ledger(ledger_dir.path()));
    tick_n(&mut d, 5).await;
    let reasons = transition_reasons(&store, task.id);
    assert_eq!(reasons, ["dispatch", "browser_prerequisite"]);
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Blocked, 0));
    assert_eq!(
        prerequisite_events(&store, task.id),
        [BrowserPrerequisiteCode::BrowserPolicyMissing]
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    let finished = store
        .events_for(task.id)
        .unwrap()
        .into_iter()
        .any(|(_, e)| {
            matches!(e, Event::WorkerFinished { outcome, .. }
            if outcome.starts_with("browser_prerequisite: browser_policy_missing: ")
                && outcome.contains("browser policy rejected"))
        });
    assert!(finished);
    // 分類の規則そのもの（台帳由来の文はそれぞれの code、それ以外は infra のまま）。
    assert_eq!(
        super::super::browser_prereq::worker_error_code("adapter: browser policy rejected: empty"),
        Some(BrowserPrerequisiteCode::BrowserPolicyMissing)
    );
    assert_eq!(
        super::super::browser_prereq::worker_error_code("browser conformance record unavailable"),
        Some(BrowserPrerequisiteCode::Missing)
    );
    assert_eq!(
        super::super::browser_prereq::worker_error_code("io error"),
        None
    );
}
