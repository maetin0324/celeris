//! Run capacity accounting (ADR-0089 and ADR 2026-10-08 account-concurrency-limits).
//!
//! CoS runs have a separate global `max_cos_runs` cap and one extra slot per real
//! account. Ordinary runs count only ordinary account usage. Provider concurrency is
//! deprecated and never compared: pool rows are limited by their accounts, non-pool
//! rows by their implicit account (`implicit/<provider id>`, no CoS +1).

use task_core::{OrgKind, OrgNode, Task};

/// `[execution] max_cos_runs` の既定（ADR-0089 規則 4）。
pub const DEFAULT_MAX_COS_RUNS: usize = 2;

/// ADR-0089 規則 1: CoS の対話 run か。**人が話しかけた対話用タスク**（`Task.conversation` あり。
/// 途中目標レビューの対話〈`milestone_id` あり、裏方〉は除く）で、担当が組織の秘書（`OrgKind::Secretary`
/// = CoS、ADR-0046 D6）であるもの。`POST /console/instruct` の既定の宛先・`POST /org/cos/messages`
/// が作るタスクがこれに当たる。判定は決定的（ストアの `org` を引くだけ）。
pub fn is_cos_run(task: &Task, org: &[OrgNode]) -> bool {
    is_cos_conversation(
        task_core::is_conversation(task),
        task_core::is_milestone_review(task),
        task.assignee.as_deref(),
        org,
    )
}

/// [`is_cos_run`] の芯（Task を組み立てずにテストできるように分けた）。
fn is_cos_conversation(
    conversation: bool,
    milestone_review: bool,
    assignee: Option<&str>,
    org: &[OrgNode],
) -> bool {
    conversation
        && !milestone_review
        && assignee.is_some_and(|id| {
            org.iter()
                .any(|n| n.id == id && n.kind == OrgKind::Secretary)
        })
}

/// 並列度の会計（ADR-0089 規則 2〜4）。`workers_in_flight` は **CoS run を数えない**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLoad {
    /// CoS 以外の run（+ プロバイダを使うレビュー run）の数。
    pub workers_in_flight: usize,
    pub max_concurrency: usize,
    /// 走っている CoS run の数。
    pub cos_in_flight: usize,
    pub max_cos_runs: usize,
}

impl RunLoad {
    /// この run を今起こしてよいか（CoS なら `max_cos_runs` だけを見る。非 CoS なら `max_concurrency`）。
    pub fn admits(&self, cos: bool) -> bool {
        if cos {
            self.cos_in_flight < self.max_cos_runs
        } else {
            self.workers_in_flight < self.max_concurrency
        }
    }

    /// CoS か非 CoS のどちらかにまだ枠があるか（`false` なら dispatch の走査そのものを省ける）。
    pub fn any_slot(&self) -> bool {
        self.admits(false) || self.admits(true)
    }
}

/// Adapter name of implicit accounts (ADR 2026-10-08 §(e)). Reserved: no
/// `AccountAdapter` uses it, so implicit ids never collide with real accounts.
pub const IMPLICIT_ACCOUNT_ADAPTER: &str = "implicit";

/// Which value [`effective_account_limit`] used (`GET /accounts` の `max_runs_source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxRunsSource {
    /// The account's stored `max_runs` (set by `PATCH /accounts/{adapter}/{id}`).
    Account,
    /// `[accounts] max_runs_per_account` (or the built-in default).
    Default,
}

impl MaxRunsSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Default => "default",
        }
    }
}

/// ADR 2026-10-08 §(c): the stored value, else the default. The deprecated provider
/// `concurrency` is not an input: it never limits a run.
pub fn effective_account_limit(stored: Option<usize>, default: usize) -> (usize, MaxRunsSource) {
    match stored {
        Some(n) => (n, MaxRunsSource::Account),
        None => (default, MaxRunsSource::Default),
    }
}

/// ADR 2026-10-08 §(e): an implicit account counts every run on its row (ordinary,
/// CoS and provider-backed reviews) against `effective_max_runs`; CoS gets no +1.
pub fn implicit_account_full(in_use_total: usize, effective_max_runs: usize) -> bool {
    in_use_total >= effective_max_runs
}

/// Real-account admission limit: resolved account override (or global default),
/// with one extra CoS slot under ADR-0089.
pub fn account_run_limit(effective_max_runs: usize, cos: bool) -> usize {
    if cos {
        effective_max_runs.saturating_add(1)
    } else {
        effective_max_runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    fn node(id: &str, kind: OrgKind) -> OrgNode {
        let now = OffsetDateTime::now_utc();
        OrgNode {
            id: id.into(),
            parent_id: None,
            name: id.into(),
            kind,
            genre: None,
            brief: String::new(),
            profile: task_core::Profile::default(),
            position: 0,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn cos_run_is_a_human_conversation_assigned_to_the_secretary() {
        let org = vec![
            node("cos", OrgKind::Secretary),
            node("eng", OrgKind::Department),
        ];
        assert!(is_cos_conversation(true, false, Some("cos"), &org));
        // 対話でない（CoS に割り当てた仕事のタスク）。
        assert!(!is_cos_conversation(false, false, Some("cos"), &org));
        // 途中目標レビューの対話（裏方）。
        assert!(!is_cos_conversation(true, true, Some("cos"), &org));
        // 他のノードとの対話。
        assert!(!is_cos_conversation(true, false, Some("eng"), &org));
        // 担当なし・組織に居ない担当。
        assert!(!is_cos_conversation(true, false, None, &org));
        assert!(!is_cos_conversation(true, false, Some("cos"), &[]));
    }

    #[test]
    fn run_load_admits_cos_over_max_concurrency_up_to_the_cos_cap() {
        let load = RunLoad {
            workers_in_flight: 1,
            max_concurrency: 1,
            cos_in_flight: 0,
            max_cos_runs: 2,
        };
        assert!(!load.admits(false));
        assert!(load.admits(true));
        assert!(load.any_slot());
        let full = RunLoad {
            cos_in_flight: 2,
            ..load
        };
        assert!(!full.admits(true));
        assert!(!full.any_slot());
        // CoS が走っていても非 CoS の枠は CoS を数えない。
        let leaves = RunLoad {
            workers_in_flight: 0,
            cos_in_flight: 2,
            ..load
        };
        assert!(leaves.admits(false));
        // max_cos_runs = 0 は例外の無効化（CoS の枠が無い）。
        assert!(
            !RunLoad {
                max_cos_runs: 0,
                ..load
            }
            .admits(true)
        );
    }

    #[test]
    fn effective_limit_prefers_stored_then_default() {
        use MaxRunsSource::*;
        assert_eq!(effective_account_limit(Some(3), 2), (3, Account));
        assert_eq!(effective_account_limit(Some(0), 2), (0, Account));
        assert_eq!(effective_account_limit(None, 2), (2, Default));
    }

    #[test]
    fn implicit_account_counts_all_runs_without_cos_slack() {
        assert!(implicit_account_full(1, 1));
        assert!(!implicit_account_full(1, 2));
        assert!(implicit_account_full(2, 2));
    }

    #[test]
    fn account_limit_is_relaxed_by_one_for_cos() {
        assert_eq!(account_run_limit(2, false), 2);
        assert_eq!(account_run_limit(2, true), 3);
    }
}
