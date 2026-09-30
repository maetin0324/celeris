//! ADR-0089（Phase R6-5）: CoS の対話 run（Console の一言）を全体の並列度 `max_concurrency` と
//! アカウントプールのプロバイダの `concurrency` から外す規則（純粋関数。LLM は呼ばない）。
//!
//! - 規則 1: CoS run の判定は [`is_cos_run`] の 1 か所だけ。
//! - 規則 2: CoS run は `max_concurrency` に数えず、`in_flight ≥ max_concurrency` でも起こす。プールの
//!   プロバイダの `concurrency` も超えてよい。アカウントは消費する（走っている run の最も少ない
//!   アカウント。`max_runs_per_account` は CoS に限り +1 まで緩める）。
//! - 規則 3: 非 CoS の run は CoS run を数えずに `max_concurrency` を埋める（例外が葉を飢えさせない）。
//!   `GET /providers` は CoS run を `in_use_cos` として別に出す。
//! - 規則 4: 絶対の上限 `[execution] max_cos_runs`（既定 2）。`0` なら例外そのものを無効にする
//!   （CoS run も通常の run と同じ規則で待つ）。

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

/// ADR-0089 規則 2 / 3: プロバイダの `concurrency` に照らして満杯か。
///
/// - `in_use` は CoS 以外の run 数、`in_use_cos` は CoS run 数（`GET /providers` と同じ分け方）。
/// - アカウントプールのプロバイダ: CoS run は `concurrency` を見ない（超えてよい）。非 CoS は CoS を
///   数えない `in_use` で比べる（CoS が葉の枠を食わない）。
/// - プールでないプロバイダ（API キーの行など）: 例外は無く、これまでどおり合計で比べる。
pub fn provider_full(
    cos: bool,
    account_pool: bool,
    in_use: usize,
    in_use_cos: usize,
    limit: usize,
) -> bool {
    match (cos, account_pool) {
        (true, true) => false,
        (false, true) => in_use >= limit,
        (_, false) => in_use + in_use_cos >= limit,
    }
}

/// ADR-0089 規則 2: 1 アカウントあたりの run の上限。CoS run に限り `max_runs_per_account + 1`
/// （葉がアカウントを埋めていても Console の一言は (max+1) 本目として載せられる）。
pub fn account_run_limit(max_runs_per_account: usize, cos: bool) -> usize {
    if cos {
        max_runs_per_account.saturating_add(1)
    } else {
        max_runs_per_account
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
    fn provider_full_exempts_cos_only_on_pool_providers() {
        assert!(!provider_full(true, true, 4, 3, 4));
        assert!(provider_full(false, true, 4, 0, 4));
        assert!(!provider_full(false, true, 3, 2, 4));
        assert!(provider_full(true, false, 1, 1, 2));
        assert!(provider_full(false, false, 1, 1, 2));
        assert!(!provider_full(true, false, 0, 1, 2));
    }

    #[test]
    fn account_limit_is_relaxed_by_one_for_cos() {
        assert_eq!(account_run_limit(2, false), 2);
        assert_eq!(account_run_limit(2, true), 3);
    }
}
