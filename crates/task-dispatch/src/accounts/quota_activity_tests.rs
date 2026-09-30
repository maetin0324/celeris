use super::*;
use task_core::TaskId;

fn member(run_id: &str, weighted_tokens: f64) -> QuotaGroupMember {
    QuotaGroupMember {
        task_id: TaskId::new(),
        run_id: run_id.to_string(),
        work_unit_id: None,
        source: "claude-oauth".to_string(),
        weighted_tokens,
        list_price_usd: None,
    }
}

#[test]
fn quota_source_label_maps_adapters() {
    assert_eq!(
        quota_source_label(AccountAdapter::ClaudeCode),
        "claude-oauth"
    );
    assert_eq!(quota_source_label(AccountAdapter::Codex), "codex-oauth");
}

#[test]
fn a_solo_run_is_exclusive() {
    let mut activity = QuotaActivity::new();
    activity.begin(AccountAdapter::ClaudeCode, "a", "r1", None, true);
    let outcome = activity.end(AccountAdapter::ClaudeCode, "a", member("r1", 100.0));
    assert!(matches!(outcome, QuotaEndOutcome::Exclusive { .. }));
}

#[test]
fn overlapping_runs_form_a_group_that_closes_when_the_last_one_ends() {
    let mut activity = QuotaActivity::new();
    activity.begin(AccountAdapter::ClaudeCode, "a", "r1", None, true);
    activity.begin(AccountAdapter::ClaudeCode, "a", "r2", None, true); // r1 と重なる
    let first = activity.end(AccountAdapter::ClaudeCode, "a", member("r1", 300.0));
    assert!(
        matches!(first, QuotaEndOutcome::Pending),
        "r2 がまだ走っているので pending"
    );
    let second = activity.end(AccountAdapter::ClaudeCode, "a", member("r2", 700.0));
    match second {
        QuotaEndOutcome::Closed { members, .. } => {
            assert_eq!(members.len(), 2);
            let total: f64 = members.iter().map(|m| m.weighted_tokens).sum();
            assert!((total - 1_000.0).abs() < 1e-9);
        }
        other => panic!("expected Closed, got {other:?}"),
    }
}

#[test]
fn a_third_run_that_starts_after_the_first_two_close_gets_its_own_group() {
    let mut activity = QuotaActivity::new();
    activity.begin(AccountAdapter::ClaudeCode, "a", "r1", None, true);
    activity.begin(AccountAdapter::ClaudeCode, "a", "r2", None, true);
    let _ = activity.end(AccountAdapter::ClaudeCode, "a", member("r1", 100.0));
    let _ = activity.end(AccountAdapter::ClaudeCode, "a", member("r2", 100.0));
    // グループが閉じた後の新しい run は、単独なら排他的。
    activity.begin(AccountAdapter::ClaudeCode, "a", "r3", None, true);
    let outcome = activity.end(AccountAdapter::ClaudeCode, "a", member("r3", 100.0));
    assert!(matches!(outcome, QuotaEndOutcome::Exclusive { .. }));
}

#[test]
fn different_accounts_do_not_share_a_group() {
    let mut activity = QuotaActivity::new();
    activity.begin(AccountAdapter::ClaudeCode, "a", "r1", None, true);
    activity.begin(AccountAdapter::ClaudeCode, "b", "r2", None, true);
    let outcome_a = activity.end(AccountAdapter::ClaudeCode, "a", member("r1", 100.0));
    let outcome_b = activity.end(AccountAdapter::ClaudeCode, "b", member("r2", 100.0));
    assert!(matches!(outcome_a, QuotaEndOutcome::Exclusive { .. }));
    assert!(matches!(outcome_b, QuotaEndOutcome::Exclusive { .. }));
}

#[test]
fn ending_a_run_that_never_began_is_exclusive_with_no_observation() {
    let mut activity = QuotaActivity::new();
    let outcome = activity.end(AccountAdapter::ClaudeCode, "a", member("ghost", 100.0));
    match outcome {
        QuotaEndOutcome::Exclusive {
            before,
            before_valid,
        } => {
            assert!(before.is_none());
            assert!(!before_valid);
        }
        other => panic!("expected Exclusive, got {other:?}"),
    }
}
