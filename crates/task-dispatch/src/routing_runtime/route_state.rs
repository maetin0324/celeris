//! 経路選択が読む admission の状態 [`RouteState`] の組み立ての判断（ADR 2026-11-11 の D3（状態の所有））。
//!
//! 純粋関数 [`build_route_state`] は、owner が読んだ値（宣言・走っている run の `(source, account)`・account の
//! 一覧と帳簿の観測値・cooldown・保存された上限）と設定の値を [`RouteStateInputs`] で受け、[`RouteState`] を返す。
//! 読み書き・時計・ファイルの走査は持たない。走査の結果の cache の更新と入力の収集は `RoutingRuntime` の method が行う。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use task_core::AccountAdapter;
use task_core::llm_route::{
    AccountId, AccountSnapshot, CandidateKey, HarnessId, QuotaSnapshot, Registry,
    ReservationSnapshot, SourceId,
};
use task_core::model_policy::RouteFeaturesRef;
use task_core::route_select::RoutingStage;
use task_core::routing_evidence::EvidenceRow;

use crate::accounts::{
    AccountBook, AccountCandidate, AccountCooldown, AccountDir, ExcludedReason, evaluate,
};

/// Mutable state the selection and admission steps read, captured once per decision.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteState {
    pub accounts: AccountSnapshot,
    pub quota: QuotaSnapshot,
    pub reservations: ReservationSnapshot,
    /// Per `(source, account)` run limit (persisted `account_limits` or the default).
    pub account_max_runs: BTreeMap<(SourceId, AccountId), u32>,
    /// Evidence table snapshot per harness (ADR 2026-10-11 D2). Empty = no evidence yet, so the
    /// `route_defaults` column decides.
    pub evidence: BTreeMap<HarnessId, Vec<EvidenceRow>>,
    /// Stage per harness (ADR D3). A harness without a row is `shadow`.
    pub stages: BTreeMap<HarnessId, RoutingStage>,
    /// Live harness versions; evidence is usable only while the major version agrees (ADR D2).
    pub harness_versions: BTreeMap<HarnessId, String>,
    /// Seed of the deterministic ε exploration (the run id; the task id when there is no run yet).
    pub run_seed: Option<String>,
    /// The one task features snapshot of this dispatch (ADR-task-routing-features D6).
    pub features: Option<RouteFeaturesRef>,
    /// The work unit this dispatch runs, when it runs one (the decision subject).
    pub work_unit_id: Option<String>,
    /// guardrail が止めた実行クラス（`harness_balance_trips`。ADR 2026-10-22 D2）。止まったクラスは宣言順。
    pub balance_tripped: BTreeSet<String>,
    /// 付記 S5-3: `route_priority` の TS の候補列（`RouteContext::priority_override`）。
    pub priority_override: Option<(HarnessId, Vec<CandidateKey>)>,
}

/// 永続化された account ごとの run 上限（`(source, account)` → 件数）。
pub type SavedLimits = HashMap<(SourceId, AccountId), usize>;

/// 組み立ての入力。すべて owner が読んで渡す値で、この関数は何も読み書きしない。
pub struct RouteStateInputs<'a> {
    pub registry: &'a Registry,
    /// CoS の起動の判断か（CoS は自分の枠と予約を数える）。
    pub cos: bool,
    /// 判断の時刻（Unix 秒）。
    pub now: i64,
    /// `[cos] worker_reserve_five_hour`（CoS が有効なときだけ `Some`）。
    pub worker_reserve_five_hour: Option<f64>,
    /// 走っている run と判定中の review が持つ `(source, account)`。CoS の判断でなければ CoS 自身の run を除いて渡す。
    pub holds: &'a [(SourceId, AccountId)],
    /// CoS の対話 run の予約。`cos` のときだけ、宣言された source のものを数える。
    pub cos_holds: &'a [(SourceId, AccountId)],
    /// アダプタごとの、ログイン済みか否かを含む account ディレクトリの一覧。
    pub scanned: &'a HashMap<AccountAdapter, Vec<AccountDir>>,
    /// アダプタごとの帳簿（観測値と cooldown）。
    pub books: &'a HashMap<AccountAdapter, &'a AccountBook>,
    /// account プールを持たない source の暗黙の account の cooldown。
    pub implicit_cooldowns: &'a HashMap<SourceId, AccountCooldown>,
    /// 永続化された上限。`None` は読めていない（上限を書かない）。
    pub saved_limits: Option<&'a SavedLimits>,
    /// 永続値が無い account の既定の上限。
    pub default_max_runs: usize,
}

/// The account pool that backs an OAuth source kind (login dirs, usage observations, cooldowns).
pub fn account_adapter_for_source_kind(kind: &str) -> Option<AccountAdapter> {
    match kind {
        "claude_oauth" => Some(AccountAdapter::ClaudeCode),
        "codex_oauth" => Some(AccountAdapter::Codex),
        "opencode_go" => Some(AccountAdapter::OpencodeGo),
        _ => None,
    }
}

/// The account pool `source_id` offers to route selection. `logged_in` is the logged-in account
/// directories when the source kind has an account adapter (OAuth pools), `None` otherwise.
///
/// - OAuth pool: the declared accounts narrowed to the logged-in ones, or every logged-in one when
///   none are declared.
/// - No account adapter: the declared accounts, or the source id itself as its one implicit account
///   when none are declared (ADR 2026-10-08 account-concurrency-limits §(e),
///   ADR 2026-10-26-ldr-proxy-and-route-fixes C1).
///
/// The daemon's route selection and `celerisctl worker` both build their pools here.
pub fn source_account_pool(
    source_id: &SourceId,
    declared: &[AccountId],
    logged_in: Option<&[AccountId]>,
) -> Vec<AccountId> {
    match logged_in {
        Some(logged_in) if declared.is_empty() => logged_in.to_vec(),
        Some(logged_in) => declared
            .iter()
            .filter(|a| logged_in.contains(a))
            .cloned()
            .collect(),
        None if declared.is_empty() => vec![AccountId::from(source_id.as_ref())],
        None => declared.to_vec(),
    }
}

/// 走っている run・review と、CoS の判断のときの宣言済み source への予約から、`(source, account)` の保有数を数える。
fn reservations(input: &RouteStateInputs<'_>) -> ReservationSnapshot {
    let mut counted = ReservationSnapshot::default();
    let cos_holds = input
        .cos_holds
        .iter()
        .filter(|(source, _)| input.cos && input.registry.sources.contains_key(source));
    for (source, account) in input.holds.iter().chain(cos_holds) {
        *counted.source_in_use.entry(source.clone()).or_default() += 1;
        *counted
            .account_in_use
            .entry((source.clone(), account.clone()))
            .or_default() += 1;
    }
    counted
}

/// Capture the admission state of every declared source: logged-in accounts of OAuth pools (or the
/// declared accounts of other sources), cooldowns and exhausted windows from the account books,
/// persisted per-account limits, and runs holding a `(source, account)`.
pub fn build_route_state(input: &RouteStateInputs<'_>) -> RouteState {
    let RouteStateInputs {
        registry, cos, now, ..
    } = *input;
    let mut state = RouteState {
        reservations: reservations(input),
        ..RouteState::default()
    };
    for (source_id, spec) in &registry.sources {
        let adapter = account_adapter_for_source_kind(spec.kind.as_ref());
        // OAuth pools are filled from the logged-in directories below.
        let mut accounts = match adapter {
            Some(_) => Vec::new(),
            None => source_account_pool(source_id, &spec.accounts, None),
        };
        if let Some(adapter) = adapter {
            let logged_in: Vec<AccountId> = input
                .scanned
                .get(&adapter)
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter(|d| d.logged_in)
                .map(|d| AccountId::from(d.id.as_str()))
                .collect();
            accounts = source_account_pool(source_id, &spec.accounts, Some(&logged_in));
            if let Some(book) = input.books.get(&adapter).copied() {
                // Preserve the account pool's headroom policy when adapting it to
                // source×model routing. Admission chooses the first eligible account.
                accounts.sort_by(|left, right| {
                    let in_use = |account: &AccountId| {
                        state
                            .reservations
                            .account_in_use
                            .get(&(source_id.clone(), account.clone()))
                            .copied()
                            .unwrap_or(0)
                    };
                    let score = |account: &AccountId| {
                        evaluate(
                            &AccountCandidate {
                                id: account.as_ref(),
                                logged_in: true,
                                in_use: in_use(account) as usize,
                            },
                            book.state(account.as_ref()),
                            usize::MAX,
                            now,
                        )
                        .score
                    };
                    score(right)
                        .partial_cmp(&score(left))
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| left.cmp(right))
                });
                // ADR 2026-10-22 D1: source の残量。最良 account の ADR-0024 D3 スコア（席の減点なし）と、
                // 最も逼迫した窓の残り割合。除外された account は 0。
                let (mut best_score, mut best_remaining) = (0.0_f64, 0.0_f64);
                for account in &accounts {
                    let key = (source_id.clone(), account.clone());
                    let candidate = AccountCandidate {
                        id: account.as_ref(),
                        logged_in: true,
                        in_use: 0,
                    };
                    let evaluation =
                        evaluate(&candidate, book.state(account.as_ref()), usize::MAX, now);
                    // ADR 2026-11-06-cos-run-launch-accounting-and-drain: 5 時間枠の使用率が
                    // `[cos] worker_reserve_five_hour` 以上の Claude account は CoS の予約分で、worker は入れない。
                    if !cos
                        && adapter == AccountAdapter::ClaudeCode
                        && input.worker_reserve_five_hour.is_some_and(|reserve| {
                            crate::accounts::five_hour_utilization(
                                book.state(account.as_ref()),
                                now,
                            ) >= reserve
                        })
                    {
                        state.quota.cos_reserved.insert(key.clone());
                    }
                    match evaluation.excluded {
                        Some(ExcludedReason::Cooldown) => {
                            state.quota.cooling_down.insert(key);
                        }
                        Some(ExcludedReason::Rejected) => {
                            state.quota.auth_failed.insert(key);
                        }
                        Some(_) => {
                            state.quota.account_remaining.insert(key, 0.0);
                        }
                        None => {
                            best_score = best_score.max(evaluation.score.unwrap_or(0.0));
                            best_remaining =
                                best_remaining.max(crate::accounts::remaining_fraction(
                                    book.state(account.as_ref()),
                                    now,
                                ));
                        }
                    }
                }
                state
                    .quota
                    .source_headroom
                    .insert(source_id.clone(), best_score);
                // 全 account が除外のときは account 単位の理由（cooldown・枠切れ）を残すため、
                // source 単位の 0 は書かない。
                if best_remaining > 0.0 {
                    state
                        .quota
                        .source_remaining
                        .insert(source_id.clone(), best_remaining);
                }
            }
        }
        // ADR 2026-10-08 §(e): a source without an account pool cools down as one implicit account.
        if adapter.is_none()
            && input
                .implicit_cooldowns
                .get(source_id)
                .is_some_and(|c| c.until > now)
        {
            for account in &accounts {
                state
                    .quota
                    .cooling_down
                    .insert((source_id.clone(), account.clone()));
            }
        }
        if let Some(saved_limits) = input.saved_limits {
            for account in &accounts {
                let key = (source_id.clone(), account.clone());
                let (effective, _) = crate::capacity::effective_account_limit(
                    saved_limits.get(&key).copied(),
                    input.default_max_runs,
                );
                // 暗黙 account（プールを持たない source）には CoS の +1 を付けない（ADR 2026-10-08 (c)・(e)）。
                let limit = crate::capacity::account_run_limit(effective, cos && adapter.is_some());
                state
                    .account_max_runs
                    .insert(key, u32::try_from(limit).unwrap_or(u32::MAX));
            }
        }
        if !accounts.is_empty() {
            state.accounts.available.insert(source_id.clone(), accounts);
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounts::AccountCooldownReason;
    use std::path::PathBuf;
    use task_core::llm_route::SourceSpec;

    fn registry(sources: &[(&str, &str)]) -> Registry {
        let mut registry = Registry::default();
        for (id, kind) in sources {
            registry.sources.insert(
                (*id).into(),
                SourceSpec {
                    kind: (*kind).into(),
                    api_format: "openai_chat".into(),
                    accounts: Vec::new(),
                    models: vec!["m".into()],
                    max_runs: None,
                    account_max_runs: BTreeMap::new(),
                    quota: None,
                    endpoint: None,
                    credential_ref: None,
                },
            );
        }
        registry
    }

    struct Fixture {
        registry: Registry,
        holds: Vec<(SourceId, AccountId)>,
        cos_holds: Vec<(SourceId, AccountId)>,
        scanned: HashMap<AccountAdapter, Vec<AccountDir>>,
        cooldowns: HashMap<SourceId, AccountCooldown>,
        limits: Option<SavedLimits>,
    }

    impl Fixture {
        fn new(registry: Registry) -> Self {
            Self {
                registry,
                holds: Vec::new(),
                cos_holds: Vec::new(),
                scanned: HashMap::new(),
                cooldowns: HashMap::new(),
                limits: None,
            }
        }

        fn state(&self, cos: bool) -> RouteState {
            build_route_state(&RouteStateInputs {
                registry: &self.registry,
                cos,
                now: 1_000,
                worker_reserve_five_hour: None,
                holds: &self.holds,
                cos_holds: &self.cos_holds,
                scanned: &self.scanned,
                books: &HashMap::new(),
                implicit_cooldowns: &self.cooldowns,
                saved_limits: self.limits.as_ref(),
                default_max_runs: 2,
            })
        }
    }

    fn pair(source: &str, account: &str) -> (SourceId, AccountId) {
        (source.into(), account.into())
    }

    #[test]
    fn cos_reservations_count_only_for_a_cos_decision_and_a_declared_source() {
        let mut f = Fixture::new(registry(&[("api", "openai")]));
        f.holds = vec![pair("api", "api")];
        f.cos_holds = vec![pair("api", "api"), pair("gone", "x")];
        let leaf = f.state(false);
        assert_eq!(leaf.reservations.source_in_use.get(&"api".into()), Some(&1));
        let cos = f.state(true);
        assert_eq!(cos.reservations.source_in_use.get(&"api".into()), Some(&2));
        assert_eq!(
            cos.reservations.account_in_use.get(&pair("api", "api")),
            Some(&2)
        );
        assert!(!cos.reservations.source_in_use.contains_key(&"gone".into()));
    }

    #[test]
    fn an_implicit_account_cools_down_only_until_the_deadline() {
        let mut f = Fixture::new(registry(&[("api", "openai")]));
        f.cooldowns.insert(
            "api".into(),
            AccountCooldown {
                until: 2_000,
                reason: AccountCooldownReason::Throttled,
            },
        );
        assert!(
            f.state(false)
                .quota
                .cooling_down
                .contains(&pair("api", "api"))
        );
        f.cooldowns.insert(
            "api".into(),
            AccountCooldown {
                until: 1_000,
                reason: AccountCooldownReason::Throttled,
            },
        );
        assert!(f.state(false).quota.cooling_down.is_empty());
    }

    #[test]
    fn run_limits_are_written_only_when_the_saved_limits_were_read() {
        let mut f = Fixture::new(registry(&[("api", "openai")]));
        assert!(f.state(false).account_max_runs.is_empty());
        f.limits = Some(SavedLimits::new());
        // 既定 2。暗黙 account には CoS の +1 を付けない。
        assert_eq!(
            f.state(true).account_max_runs.get(&pair("api", "api")),
            Some(&2)
        );
        f.limits = Some(SavedLimits::from([(pair("api", "api"), 5)]));
        assert_eq!(
            f.state(false).account_max_runs.get(&pair("api", "api")),
            Some(&5)
        );
    }

    #[test]
    fn an_oauth_pool_offers_only_the_logged_in_accounts() {
        let mut f = Fixture::new(registry(&[("claude", "claude_oauth")]));
        f.scanned.insert(
            AccountAdapter::ClaudeCode,
            vec![
                AccountDir {
                    id: "in".into(),
                    dir: PathBuf::from("in"),
                    logged_in: true,
                },
                AccountDir {
                    id: "out".into(),
                    dir: PathBuf::from("out"),
                    logged_in: false,
                },
            ],
        );
        let state = f.state(false);
        assert_eq!(
            state.accounts.available.get(&"claude".into()),
            Some(&vec![AccountId::from("in")])
        );
    }
}
