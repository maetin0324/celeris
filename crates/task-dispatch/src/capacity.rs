//! Run capacity accounting (ADR-0089 and ADR 2026-10-08 account-concurrency-limits).
//!
//! CoS runs have a separate global `max_cos_runs` cap and one extra slot per real
//! account. Ordinary runs count only ordinary account usage. Provider concurrency is
//! deprecated and never compared: pool rows are limited by their accounts, non-pool
//! rows by their LLM source's implicit account (`implicit/<llm source id>`, no CoS +1).

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

/// ADR 2026-10-08 §(e): an implicit account counts every run on every non-pool row that uses
/// its LLM source (ordinary, CoS and provider-backed reviews) against `effective_max_runs`;
/// CoS gets no +1.
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

/// `[routing_eval]`（ADR 2026-10-11-llm-source-model-routing-evaluation 付記「評価 run の受け口」）:
/// 評価 run（routing-eval の試行）専用の予約枠。`Config::dispatch_config` が組み立てて
/// [`DispatchConfig`](crate::DispatchConfig)`.eval` に入る。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvalRuntimeConfig {
    /// 同時に走らせてよい評価 run の数。既定 0 = 評価 run を起こさない。
    pub reserved_runs: usize,
}

/// 評価 run を今起こさない理由（[`eval_admission`] の `Err`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalDefer {
    /// `[routing_eval] reserved_runs = 0`（既定）。評価の受け口は無効。
    Disabled,
    /// 評価の予約枠が満杯（`eval_in_flight >= reserved_runs`）。本番の並列度とは別の枠。
    ReservedFull,
    /// account の同時実行が上限（`account_in_use >= account_limit`。本番と同じ規則）。
    AccountFull,
    /// quota 上限（quota_book の見積り）を超える。
    QuotaCap,
}

impl EvalDefer {
    /// 記録・ログ用の短い名前（`route_decisions` の input/output には書かない。defer の理由だけ）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::ReservedFull => "reserved_full",
            Self::AccountFull => "account_full",
            Self::QuotaCap => "quota_cap",
        }
    }
}

/// 評価 run を今起こしてよいか（純関数。ADR 2026-10-11-llm-source-model-routing-evaluation 付記 D3）。
///
/// 評価 run は本番の全体並列度（`max_concurrency`）に数えず、専用枠 `reserved_runs` の中だけで走る。
/// account の admission は本番と同じ: account の同時実行（[`effective_account_limit`]・
/// [`implicit_account_full`] と同じ形）と quota 上限（quota_book の見積り）を越えるなら起こさない。
/// 呼び出し側は `Err` のとき試行を `queued` のまま残す（次の tick で再び見る）。
pub fn eval_admission(
    reserved_runs: usize,
    eval_in_flight: usize,
    account_in_use: usize,
    account_limit: usize,
    quota_ok: bool,
) -> Result<(), EvalDefer> {
    if reserved_runs == 0 {
        return Err(EvalDefer::Disabled);
    }
    if eval_in_flight >= reserved_runs {
        return Err(EvalDefer::ReservedFull);
    }
    if account_in_use >= account_limit {
        return Err(EvalDefer::AccountFull);
    }
    if !quota_ok {
        return Err(EvalDefer::QuotaCap);
    }
    Ok(())
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

    /// 評価 run の admission（表駆動）: 枠 0・枠満杯・account 満杯・quota 超過・通る場合と、
    /// 理由の優先順（reserved_runs = 0 が最も強い）と境目（上限ちょうどで満杯）。
    #[test]
    fn eval_admission_table() {
        use EvalDefer::*;
        /// 表の 1 行（入力 5 欄と期待する判定）。
        #[derive(Debug)]
        struct Case {
            name: &'static str,
            reserved_runs: usize,
            eval_in_flight: usize,
            account_in_use: usize,
            account_limit: usize,
            quota_ok: bool,
            want: Result<(), EvalDefer>,
        }
        let table = [
            // 枠 0（既定）は何があっても起こさない。account 満杯・quota 超過より先に無効として返る。
            Case {
                name: "disabled",
                reserved_runs: 0,
                eval_in_flight: 0,
                account_in_use: 0,
                account_limit: 4,
                quota_ok: true,
                want: Err(Disabled),
            },
            Case {
                name: "disabled-wins",
                reserved_runs: 0,
                eval_in_flight: 3,
                account_in_use: 9,
                account_limit: 1,
                quota_ok: false,
                want: Err(Disabled),
            },
            // 予約枠が満杯（eval_in_flight >= reserved_runs）。本番の account・quota が空いていても起こさない。
            Case {
                name: "reserved-full-1of1",
                reserved_runs: 1,
                eval_in_flight: 1,
                account_in_use: 0,
                account_limit: 4,
                quota_ok: true,
                want: Err(ReservedFull),
            },
            Case {
                name: "reserved-full-2of2",
                reserved_runs: 2,
                eval_in_flight: 2,
                account_in_use: 0,
                account_limit: 4,
                quota_ok: true,
                want: Err(ReservedFull),
            },
            // account の同時実行が上限（本番と同じ規則）。quota より先に見る。
            Case {
                name: "account-full",
                reserved_runs: 2,
                eval_in_flight: 0,
                account_in_use: 2,
                account_limit: 2,
                quota_ok: true,
                want: Err(AccountFull),
            },
            Case {
                name: "account-full-before-quota",
                reserved_runs: 2,
                eval_in_flight: 0,
                account_in_use: 3,
                account_limit: 3,
                quota_ok: false,
                want: Err(AccountFull),
            },
            // account_limit = 0（account を閉じた）なら通らない。
            Case {
                name: "account-limit-zero",
                reserved_runs: 2,
                eval_in_flight: 0,
                account_in_use: 0,
                account_limit: 0,
                quota_ok: true,
                want: Err(AccountFull),
            },
            // quota 上限。評価が 1 本も走っていなくても起こさない。
            Case {
                name: "quota-cap",
                reserved_runs: 2,
                eval_in_flight: 0,
                account_in_use: 0,
                account_limit: 4,
                quota_ok: false,
                want: Err(QuotaCap),
            },
            Case {
                name: "quota-cap-in-flight",
                reserved_runs: 4,
                eval_in_flight: 2,
                account_in_use: 1,
                account_limit: 4,
                quota_ok: false,
                want: Err(QuotaCap),
            },
            // 境目: 枠・account とも上限の 1 つ下なら通る。
            Case {
                name: "admit",
                reserved_runs: 2,
                eval_in_flight: 1,
                account_in_use: 1,
                account_limit: 2,
                quota_ok: true,
                want: Ok(()),
            },
            Case {
                name: "admit-first",
                reserved_runs: 1,
                eval_in_flight: 0,
                account_in_use: 0,
                account_limit: 1,
                quota_ok: true,
                want: Ok(()),
            },
        ];
        for case in &table {
            let got = eval_admission(
                case.reserved_runs,
                case.eval_in_flight,
                case.account_in_use,
                case.account_limit,
                case.quota_ok,
            );
            assert_eq!(got, case.want, "{}", case.name);
        }
        assert_eq!(table.len(), 11);
    }

    /// defer の理由は記録・ログに出せる短い名前を持つ（未知の理由を足したらここも増える）。
    #[test]
    fn eval_defer_reasons_have_stable_names() {
        assert_eq!(EvalDefer::Disabled.as_str(), "disabled");
        assert_eq!(EvalDefer::ReservedFull.as_str(), "reserved_full");
        assert_eq!(EvalDefer::AccountFull.as_str(), "account_full");
        assert_eq!(EvalDefer::QuotaCap.as_str(), "quota_cap");
    }

    /// 評価の予約枠の既定は 0（評価 run を起こさない）。
    #[test]
    fn eval_runtime_config_defaults_to_no_reserved_runs() {
        let cfg = EvalRuntimeConfig::default();
        assert_eq!(cfg.reserved_runs, 0);
        assert_eq!(
            eval_admission(cfg.reserved_runs, 0, 0, 4, true),
            Err(EvalDefer::Disabled)
        );
    }
}
