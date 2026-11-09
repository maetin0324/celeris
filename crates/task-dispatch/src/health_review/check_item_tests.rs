//! [`super`] の写しの規則の試験。dispatcher の流れの試験（`dispatcher/tests/health_enforce.rs`）は経路を通すが、reason の
//! 書式から bucket と Error を読み戻す規則の境界（他の検査の reason・複数の rule・Error の語の誤認）までは見ない。

use super::*;

/// 不変条件: 段の統合の修復は rule ごとに `max_repairs_per_class` を数える（D3.2・D4.1）。1 回の Fail に複数の rule が
/// あれば rule ごとの bucket になり、同じ rule は 1 つにまとまる。health の Repair でない reason からは作らない。
#[test]
fn health_enforce_integration_repair_buckets_per_rule() {
    let reason = "health:deps.violation:0000000000000001,health:deps.violation:0000000000000002（evidence: src/a.rs）; \
                  health:size.x:0000000000000003";
    assert_eq!(
        repair_buckets(reason),
        vec![
            "health:deps.violation".to_string(),
            "health:size.x".to_string()
        ]
    );
    assert!(repair_buckets("health_check_error: checker_missing").is_empty());
    assert!(repair_buckets("command timed out after 5s: cargo test").is_empty());
}

/// 不変条件: Error（`health_check_error:`）と読むのは health の項目の不合格だけで、他の検査の reason や合格の行を Error に
/// しない（D3.4。誤って Blocked にしない）。
#[test]
fn health_enforce_integration_repair_error_is_read_only_from_health_item() {
    let why = "health_check_error: checker_missing";
    assert_eq!(check_error(HEALTH_CHECK_ID, false, why), Some(why));
    assert_eq!(check_error("cargo test", false, why), None);
    assert_eq!(check_error(HEALTH_CHECK_ID, true, why), None);
    assert_eq!(
        check_error(HEALTH_CHECK_ID, false, "health:deps.violation:01"),
        None
    );
}

/// 不変条件: 段の統合の記録の行は、検査の後の余りの 1 行を health の項目（`HEALTH_CHECK_ID`）として足し、検査の無い段では
/// merge の auto_resolve の行を消さない（D1.2。health の項目を足したことで統合の記録が減らない）。流れの試験は検査のある段
/// だけを通るので、検査の無い段の行の扱いはここで見る。
#[test]
fn health_enforce_phase_integration_rows_keep_auto_resolve_without_checks() {
    let auto = ("auto_resolve".to_string(), true, "{}".to_string());
    let mut rows = vec![auto.clone()];
    put_rows(&mut rows, &[], vec![(true, "health: pass".to_string())]);
    assert_eq!(
        rows,
        vec![
            auto.clone(),
            (
                HEALTH_CHECK_ID.to_string(),
                true,
                "health: pass".to_string()
            )
        ]
    );
    let check = task_core::WorkUnitCheck {
        cmd: "cargo test".to_string(),
        expect_exit: 0,
        scope: false,
    };
    let mut rows = vec![auto];
    put_rows(&mut rows, &[check], vec![(true, "ok".to_string())]);
    assert_eq!(
        rows,
        vec![("cargo test".to_string(), true, "ok".to_string())]
    );
}

/// 不変条件（D3.4・D4.3・D4.4）: 統合を replan せずに止めるのは health の Error・往復・上限の使い切りの質問（先頭の語の後に
/// `:`）だけで、他の検査の失敗の文や語の一部が同じ文は replan の判定のまま。dispatcher の試験は 3 種の止まりを 1 つずつ
/// 通すが、似た語の誤認（replan を止めすぎる）までは見ない。
#[test]
fn health_limits_stops_without_replan_only_health_stops() {
    for why in [
        "health_check_error: checker_missing",
        "health_findings_repeated: 段 build の統合: …",
        "health_repair_limit: 段 build の統合: …",
    ] {
        assert!(stops_without_replan(why), "{why}");
    }
    for why in [
        "phase build の統合後の検査が失敗しました: health:deps.violation:0000000000000001",
        "integration_check_timeout: phase build の統合後の検査が失敗しました",
        "health_check_errors are not this",
        "health_repair_limited",
    ] {
        assert!(!stops_without_replan(why), "{why}");
    }
}

/// 不変条件（D4.3）: WU の check を往復で止めるのは、不合格がすべて health の項目のときだけ。health でない失敗が混ざる・
/// 不合格が無い列は往復の判定に入らない（health でない失敗の retry を変えない）。
#[test]
fn health_limits_only_health_fails_ignores_mixed_rows() {
    assert!(only_health_fails([
        ("true", true),
        (HEALTH_CHECK_ID, false)
    ]));
    assert!(!only_health_fails([
        ("false", false),
        (HEALTH_CHECK_ID, false)
    ]));
    assert!(!only_health_fails([
        ("true", true),
        (HEALTH_CHECK_ID, true)
    ]));
    assert!(!only_health_fails(std::iter::empty::<(&str, bool)>()));
}
