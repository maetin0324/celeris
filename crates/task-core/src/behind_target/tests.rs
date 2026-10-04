use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::*;

fn t(s: &str) -> OffsetDateTime {
    OffsetDateTime::parse(s, &Rfc3339).expect("time")
}

fn obs(target_ref: &str, commits: Option<u64>, at: &str) -> BehindTargetObservation {
    BehindTargetObservation {
        task_id: TaskId::new(),
        repo_id: RepoId(ulid::Ulid::nil()),
        target_ref: target_ref.into(),
        target_sha: Some("t".into()),
        head_sha: Some("h".into()),
        commits,
        observed_at: at.into(),
    }
}

fn snap(o: &BehindTargetObservation, since: Option<String>) -> BehindTargetSnapshot {
    BehindTargetSnapshot {
        task_id: o.task_id,
        repo_id: o.repo_id,
        target_ref: o.target_ref.clone(),
        target_sha: o.target_sha.clone(),
        head_sha: o.head_sha.clone(),
        behind_target_commits: o.commits,
        behind_target_since: since,
        behind_target_observed_at: o.observed_at.clone(),
    }
}

#[test]
fn behind_since_kept_while_positive_and_cleared_at_zero() {
    let first = obs("refs/heads/main", Some(2), "2026-10-02T00:00:00Z");
    let since = next_behind_since(None, &first);
    assert_eq!(since.as_deref(), Some("2026-10-02T00:00:00Z"));
    let prev = snap(&first, since);

    // target advanced: still the same series, `since` is kept
    let later = obs("refs/heads/main", Some(5), "2026-10-02T01:00:00Z");
    assert_eq!(
        next_behind_since(Some(&prev), &later).as_deref(),
        Some("2026-10-02T00:00:00Z")
    );
    // unreadable keeps the old value, zero clears it
    let unreadable = obs("refs/heads/main", None, "2026-10-02T02:00:00Z");
    assert_eq!(
        next_behind_since(Some(&prev), &unreadable).as_deref(),
        Some("2026-10-02T00:00:00Z")
    );
    let zero = obs("refs/heads/main", Some(0), "2026-10-02T03:00:00Z");
    assert_eq!(next_behind_since(Some(&prev), &zero), None);
    // another target series restarts the clock
    let other = obs("refs/heads/parent", Some(1), "2026-10-02T04:00:00Z");
    assert_eq!(
        next_behind_since(Some(&prev), &other).as_deref(),
        Some("2026-10-02T04:00:00Z")
    );
}

#[test]
fn behind_summary_picks_max_commits_and_keeps_null() {
    let a = obs("refs/heads/main", Some(3), "2026-10-02T01:00:00Z");
    let mut b = obs("refs/heads/main", None, "2026-10-02T01:00:00Z");
    b.repo_id = RepoId(ulid::Ulid::from_parts(1, 1));
    let snapshots = vec![
        snap(&a, Some("2026-10-02T00:00:00Z".into())),
        snap(&b, None),
    ];
    let summary = summarize_behind_target(&snapshots, t("2026-10-02T00:30:00Z"));
    assert_eq!(summary.behind_target_commits, Some(3));
    assert_eq!(summary.behind_target_age_seconds, Some(1800));
    assert_eq!(summary.repos.len(), 2);
    let unread = summary
        .repos
        .iter()
        .find(|r| r.behind_target_commits.is_none())
        .expect("null repo");
    assert_eq!(unread.behind_target_age_seconds, None);

    // all unreadable -> null, never 0
    let summary = summarize_behind_target(&[snap(&b, None)], t("2026-10-02T00:30:00Z"));
    assert_eq!(summary.behind_target_commits, None);
    assert_eq!(summary.behind_target_age_seconds, None);
    // up to date -> 0 seconds
    let zero = obs("refs/heads/main", Some(0), "2026-10-02T01:00:00Z");
    let summary = summarize_behind_target(&[snap(&zero, None)], t("2026-10-02T05:00:00Z"));
    assert_eq!(summary.behind_target_commits, Some(0));
    assert_eq!(summary.behind_target_age_seconds, Some(0));
}
