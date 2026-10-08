//! ADR 2026-10-08-browser-prod-enablement D2: 適合台帳が未配置・古いとき browser task は dispatch の前に
//! 理由つきで止まり、infra_requeue を繰り返さない。配置済みなら従来どおり dispatch する。
//! fake adapter と一時 file だけ（userns・外部ネットワーク不要）。

use super::*;
use task_core::browser_prerequisite::BrowserPrerequisiteCode;
use task_worker::browser_ledger::LedgerWatch;

pub(super) const RELEASE: &str = "1a2b3c4d5e6f";

pub(super) fn write_ledger(path: &std::path::Path, release: &str, with_timestamp: bool) {
    let cases = [
        "open_allowed_origin",
        "refuse_denied_origin",
        "resume_after_crash",
        "snapshot_has_refs",
        "click_by_ref",
        "screenshot_artifact",
        "download_to_artifacts",
    ];
    let results: Vec<_> = ["claude-code", "browser-specialist"]
        .into_iter()
        .map(|id| {
            serde_json::json!({
                "backend_id": id,
                "version": task_worker::browser::SUPPORTED_VERSION,
                "passed": cases,
            })
        })
        .collect();
    let mut generated_for = serde_json::json!({
        "celeris_release": release,
        "agent_browser": task_worker::browser::SUPPORTED_VERSION,
    });
    if with_timestamp {
        generated_for["generated_at"] = serde_json::json!("2026-10-08T00:00:00Z");
    }
    std::fs::write(
        path,
        serde_json::to_vec(&serde_json::json!({
            "schema": 1,
            "source": "celeris-browser-conformance",
            "generated_for": generated_for,
            "results": results,
        }))
        .unwrap(),
    )
    .unwrap();
}

/// worker まで届いたら呼ばれた回数を数える adapter（届けば台帳の無い worker 経路で失敗する）。
pub(super) struct CountingBrowserAdapter {
    pub(super) calls: AtomicUsize,
}

#[async_trait]
impl WorkerAdapter for CountingBrowserAdapter {
    fn id(&self) -> &str {
        "claude-code"
    }
    async fn run(
        &self,
        _req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(done_outcome())
    }
}

fn browser_task(dir: &std::path::Path) -> Task {
    let mut task = new_task(
        dir,
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
    task
}

pub(super) fn prerequisite_events(
    store: &Arc<dyn TaskStore>,
    id: TaskId,
) -> Vec<BrowserPrerequisiteCode> {
    store
        .events_for(id)
        .unwrap()
        .into_iter()
        .filter_map(|(_, e)| match e {
            Event::BrowserPrerequisiteBlocked { code, message } => {
                assert_eq!(message, code.message());
                Some(code)
            }
            _ => None,
        })
        .collect()
}

pub(super) async fn tick_n(d: &mut Dispatcher, n: usize) {
    for _ in 0..n {
        d.tick().unwrap();
        // infra 再試行のバックオフで隠れないよう毎 tick 外す（繰り返しが起きるなら見えるように）。
        d.infra_backoff.clear();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// 未配置: dispatch の前に `blocked(browser_prerequisite)` で止まり、adapter は呼ばれず、tick を重ねても
/// infra_requeue も event の再追記も起きない。
#[tokio::test]
async fn browser_ledger_gate_missing_ledger_blocks_before_dispatch_without_infra_requeue() {
    let dir = tempfile::tempdir().unwrap();
    let ledger_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = browser_task(dir.path());
    store.insert(&task).unwrap();
    let adapter = Arc::new(CountingBrowserAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.set_browser_ledger_watch(LedgerWatch::fixed(
        Some(ledger_dir.path().join("conformance.json")),
        Some(RELEASE.into()),
        None,
    ));
    tick_n(&mut d, 40).await;
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Blocked, 0));
    assert_eq!(
        transition_reasons(&store, task.id),
        ["browser_prerequisite"]
    );
    assert_eq!(
        prerequisite_events(&store, task.id),
        [BrowserPrerequisiteCode::Missing]
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    // 人に分かる文が event に入っている。
    assert!(
        BrowserPrerequisiteCode::Missing
            .message()
            .contains("browser の適合台帳が未配置")
    );
}

/// 古い version（別 release 用の台帳）: 同じく止まり、台帳を作り直すと人の操作なしで ready に戻り dispatch される。
#[tokio::test]
async fn browser_ledger_gate_stale_release_blocks_and_resumes_after_regeneration() {
    let dir = tempfile::tempdir().unwrap();
    let ledger_dir = tempfile::tempdir().unwrap();
    let ledger = ledger_dir.path().join("conformance.json");
    write_ledger(&ledger, "ffffffffffff", false);
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = browser_task(dir.path());
    store.insert(&task).unwrap();
    let adapter = Arc::new(CountingBrowserAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.set_browser_ledger_watch(LedgerWatch::fixed(
        Some(ledger.clone()),
        Some(RELEASE.into()),
        None,
    ));
    tick_n(&mut d, 5).await;
    assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Blocked);
    assert_eq!(
        prerequisite_events(&store, task.id),
        [BrowserPrerequisiteCode::StaleRelease]
    );
    assert_eq!(store.get(task.id).unwrap().unwrap().attempts, 0);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);

    write_ledger(&ledger, RELEASE, true);
    d.tick().unwrap();
    let events = store.events_for(task.id).unwrap();
    assert!(events.iter().any(|(_, e)| matches!(
        e,
        Event::BrowserPrerequisiteResumed { code } if *code == BrowserPrerequisiteCode::StaleRelease
    )));
    let reasons = transition_reasons(&store, task.id);
    assert_eq!(
        &reasons[..3],
        [
            "browser_prerequisite",
            "browser_prerequisite_resolved",
            "dispatch"
        ]
    );
}

/// 配置済み: gate を通って従来どおり dispatch する（worker 側で台帳が読めなければ、それも
/// infra_requeue ではなく `browser_prerequisite` で止まり、変わらない台帳では往復しない）。
#[tokio::test]
async fn browser_ledger_gate_placed_ledger_dispatches_and_worker_ledger_error_is_not_infra() {
    let dir = tempfile::tempdir().unwrap();
    let ledger_dir = tempfile::tempdir().unwrap();
    let ledger = ledger_dir.path().join("conformance.json");
    write_ledger(&ledger, RELEASE, true);
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = browser_task(dir.path());
    store.insert(&task).unwrap();
    let adapter = Arc::new(CountingBrowserAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.set_browser_ledger_watch(LedgerWatch::fixed(Some(ledger), Some(RELEASE.into()), None));
    tick_n(&mut d, 1).await;
    let reasons = transition_reasons(&store, task.id);
    assert_eq!(reasons.first().map(String::as_str), Some("dispatch"));
    // この試験の worker には台帳の path が無い（daemon が configure していない）ので、worker は台帳由来の
    // 失敗を返す。dispatcher はそれを infra_requeue にせず止め、台帳が変わらない限り起こし直さない。
    tick_n(&mut d, 40).await;
    let reasons = transition_reasons(&store, task.id);
    assert_eq!(reasons, ["dispatch", "browser_prerequisite"]);
    assert!(!reasons.iter().any(|r| r == "infra_requeue"));
    let t = store.get(task.id).unwrap().unwrap();
    assert_eq!((t.status, t.attempts), (Status::Blocked, 0));
    assert_eq!(
        prerequisite_events(&store, task.id),
        [BrowserPrerequisiteCode::Missing]
    );
    let finished = store.events_for(task.id).unwrap().into_iter().any(|(_, e)| {
        matches!(e, Event::WorkerFinished { outcome, .. } if outcome.starts_with("browser_prerequisite: missing: "))
    });
    assert!(finished);
}

/// 非 browser の task は gate を通らない（従来どおり）。
#[tokio::test]
async fn browser_ledger_gate_ignores_tasks_without_browser_skill() {
    let dir = tempfile::tempdir().unwrap();
    let ledger_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    store.insert(&task).unwrap();
    let adapter = Arc::new(CountingBrowserAdapter {
        calls: AtomicUsize::new(0),
    });
    let mut d = dispatcher(store.clone(), adapter.clone(), 1);
    d.set_browser_ledger_watch(LedgerWatch::fixed(
        Some(ledger_dir.path().join("conformance.json")),
        None,
        None,
    ));
    let report = run_until_idle(&mut d, 200).await;
    assert!(report.idle);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert!(prerequisite_events(&store, task.id).is_empty());
}
