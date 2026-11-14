//! ADR 2026-11-07-dispatch-ready-phase-timing: `dispatch_ready` の内側の段が tick の `step_us` に
//! `dispatch_ready.*` として出る。件数に関わらず名前は決まっており、時間は tick ごとに作り直される。

use super::*;

fn parts(report: &TickReport) -> Vec<&'static str> {
    report
        .step_us
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| name.starts_with("dispatch_ready."))
        .collect()
}

#[tokio::test]
async fn tick_reports_dispatch_ready_phases_and_resets_each_tick() {
    let tmp = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let adapter = Arc::new(InstantAdapter {
        terminal: Terminal::Done {
            summary: "ok".into(),
            evidence: vec![],
            usage: None,
        },
        delay: Duration::ZERO,
    });
    let mut d = dispatcher(store.clone(), adapter, 1);

    // 空の tick でも、ready task の取得の段が出る。
    let empty = d.tick().unwrap();
    let names = parts(&empty);
    assert!(
        names.contains(&"dispatch_ready.ready_tasks"),
        "missing: {names:?}"
    );
    assert!(!names.contains(&"dispatch_ready.dispatch_one"));

    // ready の task が 1 件あれば、候補ごとの段（dispatch_one）も出る。
    let task = new_task(tmp.path(), Check::Reviewer, 0);
    store.insert(&task).unwrap();
    let busy = d.tick().unwrap();
    let names = parts(&busy);
    let want = "dispatch_ready.dispatch_one";
    assert!(names.contains(&want), "{want} missing: {names:?}");
    // 名前は 1 tick に 1 回だけ（候補が増えても合計される）。
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "duplicated: {names:?}");
}
