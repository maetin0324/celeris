//! Integration tests link task-worker without `cfg(test)`, matching its production build.

#[test]
fn daemon_runtime_has_no_test_loopback_allow_in_non_test_build() {
    assert!(
        task_worker::browser::daemon_test_loopback_allow_for_test_build_check().is_empty(),
        "production-linked daemon runtime must not receive test loopback exceptions"
    );
}
