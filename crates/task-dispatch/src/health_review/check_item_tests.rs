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
