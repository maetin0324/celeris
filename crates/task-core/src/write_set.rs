//! ADR-0130 D1-D3: repository-relative write path hints and exact overlap.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Committed Git diff observation for a single repository and run or WU.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteSetRecord {
    pub owner_id: String,
    pub task_id: crate::model::TaskId,
    pub work_unit_id: Option<String>,
    pub repo_id: crate::repos::RepoId,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub paths: Vec<String>,
    pub status: WriteSetStatus,
    pub reason: Option<String>,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteSetStatus {
    Complete,
    Incomplete,
    Unavailable,
}

impl WriteSetStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::Unavailable => "unavailable",
        }
    }
}

/// One normalized Git repository-relative path prefix.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WritePathHint(String);

impl WritePathHint {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let path = raw.strip_suffix('/').unwrap_or(raw);
        if raw.is_empty()
            || raw.starts_with('/')
            || raw.contains('\0')
            || raw.contains('\\')
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(format!("invalid write path hint: {raw:?}"));
        }
        Ok(Self(path.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Sort, deduplicate and validate hints before storing them.
pub fn normalize_write_paths(paths: &[String]) -> Result<Vec<String>, String> {
    paths
        .iter()
        .map(|path| WritePathHint::parse(path).map(|hint| hint.0))
        .collect::<Result<BTreeSet<_>, _>>()
        .map(|set| set.into_iter().collect())
}

/// ADR-0130 D1: an own explicit hint wins, otherwise the inherited one is used.
/// `None` and an empty list both mean "no hint" and never become a gate input.
pub fn inherit_write_paths(
    own: Option<&[String]>,
    inherited: Option<&[String]>,
) -> Option<Vec<String>> {
    own.filter(|paths| !paths.is_empty())
        .or(inherited.filter(|paths| !paths.is_empty()))
        .map(<[String]>::to_vec)
}

/// Strong overlap is at least one equal or segment-ancestor pair.
/// Callers compare only paths belonging to the same repository.
pub fn write_set_overlap(a: &[String], b: &[String]) -> bool {
    a.iter().any(|left| {
        b.iter().any(|right| {
            let (Ok(left), Ok(right)) = (WritePathHint::parse(left), WritePathHint::parse(right))
            else {
                return false;
            };
            left == right
                || right
                    .0
                    .strip_prefix(&left.0)
                    .is_some_and(|tail| tail.starts_with('/'))
                || left
                    .0
                    .strip_prefix(&right.0)
                    .is_some_and(|tail| tail.starts_with('/'))
        })
    })
}

/// Repository-aware entry point for dispatcher reservations.
pub fn write_set_overlap_for_repo(
    left_repo: crate::repos::RepoId,
    left: &[String],
    right_repo: crate::repos::RepoId,
    right: &[String],
) -> bool {
    left_repo == right_repo && write_set_overlap(left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_set_overlap_respects_path_segments_and_missing_hints() {
        let paths = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(write_set_overlap(&paths(&["src/"]), &paths(&["src/a.rs"])));
        assert!(write_set_overlap(&paths(&["src/a.rs"]), &paths(&["src/"])));
        assert!(write_set_overlap(
            &paths(&["src/a.rs"]),
            &paths(&["src/a.rs"])
        ));
        assert!(!write_set_overlap(
            &paths(&["src/a.rs"]),
            &paths(&["src/ab.rs"])
        ));
        assert!(!write_set_overlap(
            &paths(&["src/"]),
            &paths(&["srcx/a.rs"])
        ));
        assert!(!write_set_overlap(&[], &paths(&["src/"])));
        assert!(!write_set_overlap_for_repo(
            crate::repos::RepoId::new(),
            &paths(&["src/"]),
            crate::repos::RepoId::new(),
            &paths(&["src/a.rs"]),
        ));
    }

    #[test]
    fn write_set_inherit_prefers_own_hint_and_treats_empty_as_unspecified() {
        let own = vec!["src/a.rs".to_string()];
        let parent = vec!["src".to_string()];
        assert_eq!(
            inherit_write_paths(Some(&own), Some(&parent)),
            Some(own.clone())
        );
        assert_eq!(
            inherit_write_paths(Some(&[]), Some(&parent)),
            Some(parent.clone())
        );
        assert_eq!(inherit_write_paths(None, Some(&parent)), Some(parent));
        assert_eq!(inherit_write_paths(None, None), None);
        assert_eq!(inherit_write_paths(Some(&[]), Some(&[])), None);
    }

    #[test]
    fn write_set_hints_normalize_and_reject_invalid_paths() {
        assert_eq!(
            normalize_write_paths(&["src/".into(), "src".into()]).unwrap(),
            vec!["src"]
        );
        for bad in [
            "", "/src", "./src", "src/../a", "src//a", "src\\a", "src\0a",
        ] {
            assert!(WritePathHint::parse(bad).is_err(), "{bad:?}");
        }
    }
}
