//! ADR 2026-11-12-cos-triage-scope D4: the daemon's match is computed from the store only (no LLM,
//! no clock but the one passed) and is the same for the same items. `task_ops::triage_match` tests
//! the rules; this pins that the launch path hands each item its line.

use task_core::chat::triage::CosTriageItemReport;
use task_core::store::SqliteStore;
use time::OffsetDateTime;

use super::triage_hints;

fn item(id: &str, kind: &str, summary: &str, detail: &str) -> CosTriageItemReport {
    CosTriageItemReport {
        item_id: id.into(),
        thread_id: "no-such-thread".into(),
        source_kind: kind.into(),
        source_key: format!("{kind}-{id}"),
        source_revision: "1".into(),
        state: "pending".into(),
        summary: summary.into(),
        detail: detail.into(),
        reason: None,
        operation_id: None,
        run_id: None,
        created_at: "2026-11-12T00:00:00Z".into(),
        route: None,
        decision: None,
    }
}

#[test]
fn cos_triage_hints_are_deterministic_store_reads() {
    let store = SqliteStore::open_in_memory().expect("store");
    let now = OffsetDateTime::from_unix_timestamp(1_794_441_600).expect("time");
    let items = vec![
        item(
            "a",
            "authorization",
            "認可",
            "クラスタ `pegasus` の job の待ちが上限（3600 秒）に達しました: 1 Q",
        ),
        item("b", "credential_wait", "login", ""),
        item("c", "notice", "知らせ", ""),
    ];
    let hints = triage_hints(&store, &items, now).expect("hints");
    assert_eq!(
        hints,
        vec![
            Some("job_wait_extend の候補".to_string()),
            Some("人に残す件（credential_input）。to=human で回す".to_string()),
            None,
        ]
    );
    assert_eq!(triage_hints(&store, &items, now).expect("again"), hints);
}
