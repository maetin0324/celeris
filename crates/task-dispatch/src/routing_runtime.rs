//! ADR 2026-11-11 dispatcher-state-ownership D1・D2: 経路選択の状態の owner
//! （ADR-0012 D2、ADR-0024/0025、ADR 2026-10-08 account-concurrency-limits、ADR 2026-10-11 llm-source-harness-selection、
//! ADR 2026-10-22-jev-classification-before-launch）。
//!
//! - [`RoutingRuntime`] の field は private で、`Dispatcher` の側の薄い入口（`dispatcher/route_select.rs`・
//!   `dispatcher/route_launch.rs`・`dispatcher/lane_assist.rs` ほか）は method だけを呼ぶ。method は `Dispatcher` を受けず、
//!   store・時計・`[accounts]` の root・走っている run の `(source, account)` は引数で受ける。
//! - [`RoutingRuntime::route_state`] は account 一覧の cache（走査の結果）を更新してから、純粋関数
//!   [`route_state::build_route_state`] に入力を渡す。判断はその関数にあり、cache の更新と入力の収集だけがここにある。
//! - quota の窓と較正は下位 owner [`quota::QuotaBook`]（`quota_activity`・`quota_calibration`）に置く。見積りの判断は純粋関数
//!   [`quota_estimate::propose_estimate`]、適用（較正の更新と他の member の event の追記）は `quota.rs` の `RoutingRuntime` の method。
//! - 評価 run の追跡（`eval_runs`）と knowledge の到達性の cache（`knowledge_probe_cache`）もここに置く。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use task_core::llm_route::{AccountId, HarnessId, Registry, SourceId};
use task_core::model_router::context_registry::RoutingContextRegistry;
use task_core::route_select::RoutingMode;
use task_core::{AccountAdapter, StoreError, TaskId, TaskStore};

use crate::accounts::{AccountBook, AccountCooldown, AccountDir, scan_accounts};
use task_worker::{Reachability, WorkerAdapter};

pub(crate) mod classification_board;
pub(crate) mod quota;
pub(crate) mod quota_estimate;
pub mod route_state;

use route_state::{RouteState, RouteStateInputs, SavedLimits, account_adapter_for_source_kind};

/// lane の決定（`decide_lane`）が使う routing の設定（retry の escalation の閾値）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DispatchRoutingSettings {
    pub escalation: task_core::EscalationThresholds,
}

/// `route_state` の呼び出しごとの入力（owner の外の値）。
pub(crate) struct RouteStateContext<'a> {
    pub(crate) cos: bool,
    pub(crate) now: i64,
    pub(crate) worker_reserve_five_hour: Option<f64>,
    /// 走っている run と判定中の review の `(source, account)`。
    pub(crate) holds: &'a [(SourceId, AccountId)],
    /// CoS の対話 run の予約。
    pub(crate) cos_holds: &'a [(SourceId, AccountId)],
    /// `[accounts]` の根ディレクトリ（`[accounts]` が無ければ `None`）。
    pub(crate) account_roots: Option<&'a HashMap<AccountAdapter, PathBuf>>,
    pub(crate) default_max_runs: usize,
}

/// 起動した評価 run の追跡（プロセス内のみ。真実は `eval_trials` の行と `runs` 索引）。
pub(crate) struct EvalRunEntry {
    pub(crate) trial_id: String,
    /// この run が押さえている `(source, account)`（評価の admission で本番の run と合算する）。
    pub(crate) route: (SourceId, AccountId),
    pub(crate) handle: tokio::task::JoinHandle<()>,
}

/// 経路選択の状態: 宣言（source・harness・route）、account の上限・走査 cache・帳簿・cooldown、routing mode、分類 board、
/// 経路なしの task の記録、RoutingContext の登録簿。
pub struct RoutingRuntime {
    /// ADR 2026-10-11 llm-source-harness-selection: declared sources, harnesses and routes.
    llm_route_registry: Arc<Registry>,
    /// one base adapter per declared harness (`[harnesses.<id>]` over `[adapters.<kind>]`); the route's model and
    /// account go on at launch.
    harness_adapters: HashMap<HarnessId, Arc<dyn WorkerAdapter>>,
    /// 保存された account ごとの上限。`None` は読めていない（読めるまで account の起動を止める）。
    account_limits: Option<SavedLimits>,
    /// ADR-0024 D1: 選択のたびにディレクトリを読み直さないよう、tick につき高々 1 回だけスキャンする（アダプタごと）。
    accounts_scan_cache: HashMap<AccountAdapter, Vec<AccountDir>>,
    /// ADR-0024 D5/D7 / ADR-0025 D5: 進行中のログイン中継を持つ account（キーは `"<adapter>:<id>"`）。
    login_pending_accounts: std::collections::HashSet<String>,
    /// ADR-0024 D4 / ADR-0025 D1: アダプタごとの account の観測値・cooldown・確認の帳簿。実行中の run のシンクとも共有する。
    account_books: HashMap<AccountAdapter, Arc<StdMutex<AccountBook>>>,
    /// ADR 2026-10-08 §(e): cooldowns of the implicit accounts (id = source id) of sources without an account pool.
    source_cooldowns: HashMap<SourceId, AccountCooldown>,
    /// `routing.mode`（既定 fixed）。
    routing_mode: RoutingMode,
    /// 分類段が書く結果の置き場（読むだけ。Jev は呼ばない）。
    classification_board: classification_board::ClassificationBoard,
    /// 分類段が動いているとき、board に項目が無い subject の dispatch を待たせる上限。
    classification_wait: Option<Duration>,
    /// board に項目が無くて最初に dispatch を見送った時刻（subject ごと）。
    classification_deferred_since: HashMap<String, Instant>,
    /// 「設定に合う経路が無い」警告を出した（連続 tick で繰り返さない）タスク（ADR-0012 D2）。
    warned_unroutable: std::collections::HashSet<TaskId>,
    /// この tick で経路が無かった ready タスク（`is_idle` で待ち対象から外す）。
    unroutable: std::collections::HashSet<TaskId>,
    dispatch_routing: DispatchRoutingSettings,
    /// run に結ぶ RoutingContext の登録簿。未設定なら登録も `context_ref` の発行もしない。
    routing_context_registry: Option<Arc<dyn RoutingContextRegistry>>,
    /// quota の窓と較正（下位 owner）。
    quota: quota::QuotaBook,
    /// ADR 2026-10-11-llm-source-model-routing-evaluation 付記 2026-10-12（eval-dispatch）D4:
    /// 起動した評価 run（run id → 試行）。`running` に入れないので本番の全体並列度に数えない
    /// （予約枠は `config.eval.reserved_runs`）。真実は `eval_trials` の行。
    eval_runs: HashMap<String, EvalRunEntry>,
    /// ADR-0052 D1: 検査の結果のキャッシュ（`base_url` → (いつ調べたか, 結果)）。60 秒。
    knowledge_probe_cache: HashMap<String, (Instant, Reachability)>,
}

fn limit_key(adapter: &str, id: &str) -> (SourceId, AccountId) {
    (SourceId::from(adapter), AccountId::from(id))
}

fn load_limits(store: &dyn TaskStore) -> Result<SavedLimits, StoreError> {
    Ok(store
        .account_limits()?
        .into_iter()
        .map(|r| {
            (
                (SourceId::from(r.source_id), AccountId::from(r.account_id)),
                r.max_runs,
            )
        })
        .collect())
}

fn login_pending_key(adapter: AccountAdapter, id: &str) -> String {
    format!("{adapter}:{id}")
}

impl RoutingRuntime {
    /// `account_roots`: `[accounts]` のアダプタごとの根（帳簿を読む）。保存された上限は `store` から読む。
    pub fn new(
        store: &dyn TaskStore,
        account_roots: Option<&HashMap<AccountAdapter, PathBuf>>,
    ) -> Self {
        let account_limits = match load_limits(store) {
            Ok(limits) => Some(limits),
            Err(error) => {
                tracing::error!(%error, "cannot load account limits; account dispatch is blocked");
                None
            }
        };
        // ADR-0024 D4 / ADR-0025 D1: `<root>/.celeris-usage.json` から観測値・cooldown を読む（無ければ空から始める）。
        let account_books = account_roots
            .map(|roots| {
                roots
                    .iter()
                    .map(|(adapter, root)| {
                        (
                            *adapter,
                            Arc::new(StdMutex::new(AccountBook::load(
                                &root.join(".celeris-usage.json"),
                            ))),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            llm_route_registry: Arc::default(),
            harness_adapters: HashMap::new(),
            account_limits,
            accounts_scan_cache: HashMap::new(),
            login_pending_accounts: std::collections::HashSet::new(),
            account_books,
            source_cooldowns: HashMap::new(),
            routing_mode: RoutingMode::default(),
            classification_board: classification_board::ClassificationBoard::default(),
            classification_wait: None,
            classification_deferred_since: HashMap::new(),
            warned_unroutable: std::collections::HashSet::new(),
            unroutable: std::collections::HashSet::new(),
            dispatch_routing: DispatchRoutingSettings::default(),
            routing_context_registry: None,
            quota: quota::QuotaBook::new(),
            eval_runs: HashMap::new(),
            knowledge_probe_cache: HashMap::new(),
        }
    }

    // ---- 宣言と harness の adapter ----

    pub(crate) fn set_registry(&mut self, registry: Registry) {
        self.llm_route_registry = Arc::new(registry);
    }

    pub(crate) fn registry(&self) -> &Registry {
        &self.llm_route_registry
    }

    pub(crate) fn registry_arc(&self) -> Arc<Registry> {
        Arc::clone(&self.llm_route_registry)
    }

    pub(crate) fn set_harness_adapters(
        &mut self,
        adapters: HashMap<HarnessId, Arc<dyn WorkerAdapter>>,
    ) {
        self.harness_adapters = adapters;
    }

    pub(crate) fn harness_adapter(&self, harness: &HarnessId) -> Option<&Arc<dyn WorkerAdapter>> {
        self.harness_adapters.get(harness)
    }

    pub(crate) fn harness_adapters(
        &self,
    ) -> impl Iterator<Item = (&HarnessId, &Arc<dyn WorkerAdapter>)> {
        self.harness_adapters.iter()
    }

    /// 宣言された source の account のアダプタ（OAuth プールを持たない source は `None`）。
    pub(crate) fn source_account_adapter(&self, source: &SourceId) -> Option<AccountAdapter> {
        self.llm_route_registry
            .sources
            .get(source)
            .and_then(|spec| account_adapter_for_source_kind(spec.kind.as_ref()))
    }

    // ---- account の上限（保存値） ----

    pub(crate) fn account_limits_loaded(&self) -> bool {
        self.account_limits.is_some()
    }

    /// 保存された上限（読めていなければ `None`）。
    pub(crate) fn stored_limit(&self, adapter: &str, id: &str) -> Option<usize> {
        self.account_limits
            .as_ref()?
            .get(&limit_key(adapter, id))
            .copied()
    }

    /// 権威ある設定を読み直す。失敗したら account の起動を止めたまま。
    pub(crate) fn reload_account_limits(
        &mut self,
        store: &dyn TaskStore,
    ) -> Result<(), StoreError> {
        self.account_limits = None;
        self.account_limits = Some(load_limits(store)?);
        Ok(())
    }

    /// 上書きを SQLite に書いてから、commit の後でだけメモリを更新する。
    pub(crate) fn set_account_limit(
        &mut self,
        store: &dyn TaskStore,
        adapter: &str,
        id: &str,
        max_runs: Option<usize>,
    ) -> Result<(), StoreError> {
        store.set_account_limit(adapter, id, max_runs)?;
        if let Some(limits) = self.account_limits.as_mut() {
            let key = limit_key(adapter, id);
            match max_runs {
                Some(max_runs) => limits.insert(key, max_runs),
                None => limits.remove(&key),
            };
            Ok(())
        } else {
            self.reload_account_limits(store)
        }
    }

    // ---- account の走査 cache・ログイン中継・帳簿・cooldown ----

    /// tick の頭で捨てる（ADR-0024 D1）。
    pub(crate) fn clear_scan_cache(&mut self) {
        self.accounts_scan_cache.clear();
    }

    /// そのアダプタの account ディレクトリの一覧（tick 内は cache）。
    pub(crate) fn scanned_accounts(
        &mut self,
        adapter: AccountAdapter,
        root: &Path,
    ) -> Vec<AccountDir> {
        self.accounts_scan_cache
            .entry(adapter)
            .or_insert_with(|| scan_accounts(root, adapter))
            .clone()
    }

    pub(crate) fn set_login_pending(&mut self, adapter: AccountAdapter, id: &str, pending: bool) {
        let key = login_pending_key(adapter, id);
        if pending {
            self.login_pending_accounts.insert(key);
        } else {
            self.login_pending_accounts.remove(&key);
        }
    }

    pub(crate) fn login_pending(&self, adapter: AccountAdapter, id: &str) -> bool {
        self.login_pending_accounts
            .contains(&login_pending_key(adapter, id))
    }

    pub(crate) fn account_book(
        &self,
        adapter: AccountAdapter,
    ) -> Option<Arc<StdMutex<AccountBook>>> {
        self.account_books.get(&adapter).cloned()
    }

    /// 期限の切れた cooldown を帳簿から外す（tick の頭）。
    pub(crate) fn expire_account_books(&self, now: i64) {
        for book in self.account_books.values() {
            if let Ok(mut book) = book.lock() {
                book.clear_expired(now);
            }
        }
    }

    pub(crate) fn source_cooldown(&self, source: &SourceId) -> Option<&AccountCooldown> {
        self.source_cooldowns.get(source)
    }

    pub(crate) fn set_source_cooldown(&mut self, source: SourceId, cooldown: AccountCooldown) {
        self.source_cooldowns.insert(source, cooldown);
    }

    pub(crate) fn source_cooldowns(&self) -> impl Iterator<Item = (&SourceId, &AccountCooldown)> {
        self.source_cooldowns.iter()
    }

    // ---- routing mode と分類 board ----

    pub(crate) fn set_mode(&mut self, mode: RoutingMode) {
        self.routing_mode = mode;
    }

    pub(crate) fn mode(&self) -> RoutingMode {
        self.routing_mode
    }

    pub(crate) fn board(&self) -> classification_board::ClassificationBoard {
        self.classification_board.clone()
    }

    pub(crate) fn set_classification_wait(&mut self, wait: Option<Duration>) {
        self.classification_wait = wait;
        if wait.is_none() {
            self.classification_deferred_since.clear();
        }
    }

    pub(crate) fn classification_wait(&self) -> Option<Duration> {
        self.classification_wait
    }

    /// board に項目が無い subject の起動を、`max`（単調時計）まで見送るか。項目が載れば待ちを閉じ、上限を過ぎても閉じる。
    pub(crate) fn defer_until_classified(
        &mut self,
        subject: &str,
        now: Instant,
        max: Duration,
    ) -> bool {
        if self.classification_board.get(subject).is_some() {
            self.classification_deferred_since.remove(subject);
            return false;
        }
        let since = *self
            .classification_deferred_since
            .entry(subject.to_owned())
            .or_insert(now);
        if now.saturating_duration_since(since) < max {
            tracing::debug!(
                subject,
                "jev: dispatch deferred until the classification runs"
            );
            true
        } else {
            self.classification_deferred_since.remove(subject);
            tracing::warn!(
                subject,
                "jev: classification wait expired; dispatching unclassified"
            );
            false
        }
    }

    // ---- 経路なしの task ----

    /// 新しい dispatch の走査の頭（この tick の「経路なし」を空にする）。
    pub(crate) fn begin_dispatch_pass(&mut self) {
        self.unroutable.clear();
    }

    pub(crate) fn mark_unroutable(&mut self, task: TaskId) {
        self.unroutable.insert(task);
    }

    pub(crate) fn is_unroutable(&self, task: &TaskId) -> bool {
        self.unroutable.contains(task)
    }

    /// 警告を出した印を付ける。初めてなら `true`（続きの警告と記録は 1 回だけ）。
    pub(crate) fn warn_unroutable_once(&mut self, task: TaskId) -> bool {
        self.warned_unroutable.insert(task)
    }

    pub(crate) fn clear_warned_unroutable(&mut self, task: &TaskId) {
        self.warned_unroutable.remove(task);
    }

    pub(crate) fn warned_unroutable_count(&self) -> usize {
        self.warned_unroutable.len()
    }

    // ---- escalation の閾値・RoutingContext の登録簿 ----

    pub(crate) fn escalation(&self) -> &task_core::EscalationThresholds {
        &self.dispatch_routing.escalation
    }

    pub(crate) fn set_context_registry(&mut self, registry: Arc<dyn RoutingContextRegistry>) {
        self.routing_context_registry = Some(registry);
    }

    pub(crate) fn context_registry(&self) -> Option<&Arc<dyn RoutingContextRegistry>> {
        self.routing_context_registry.as_ref()
    }

    // ---- 評価 run の追跡 ----

    pub(crate) fn eval_run_count(&self) -> usize {
        self.eval_runs.len()
    }

    pub(crate) fn holds_eval_run(&self, run_id: &str) -> bool {
        self.eval_runs.contains_key(run_id)
    }

    pub(crate) fn track_eval_run(&mut self, run_id: String, entry: EvalRunEntry) {
        self.eval_runs.insert(run_id, entry);
    }

    /// 終わった評価 run を表から外す（追跡していなければ `None`）。
    pub(crate) fn finish_eval_run(&mut self, run_id: &str) -> Option<EvalRunEntry> {
        self.eval_runs.remove(run_id)
    }

    /// 手元の評価 run を全部表から外して返す（止めるのは呼び出し側）。
    pub(crate) fn drain_eval_runs(&mut self) -> Vec<(String, EvalRunEntry)> {
        self.eval_runs.drain().collect()
    }

    /// 走っている評価 run が押さえている `(source, account)`。
    pub(crate) fn eval_run_routes(&self) -> impl Iterator<Item = &(SourceId, AccountId)> {
        self.eval_runs.values().map(|e| &e.route)
    }

    // ---- knowledge の到達性の cache ----

    pub(crate) fn clear_knowledge_probe_cache(&mut self) {
        self.knowledge_probe_cache.clear();
    }

    /// cache の期限判定と更新を所有し、期限内なら probe を呼ばない。
    pub(crate) fn knowledge_reachability(
        &mut self,
        base_url: &str,
        now: Instant,
        probe: impl FnOnce() -> Reachability,
    ) -> Reachability {
        if let Some((checked_at, cached)) = self.knowledge_probe_cache.get(base_url)
            && now.duration_since(*checked_at) < task_worker::PROBE_CACHE_TTL
        {
            return cached.clone();
        }
        let outcome = probe();
        self.knowledge_probe_cache
            .insert(base_url.to_string(), (now, outcome.clone()));
        outcome
    }

    // ---- RouteState ----

    /// admission の状態を捕える。走査の cache を満たしてから、帳簿を借りて純粋関数に渡す。
    pub(crate) fn route_state(&mut self, ctx: &RouteStateContext<'_>) -> RouteState {
        if let Some(roots) = ctx.account_roots {
            let adapters: Vec<AccountAdapter> = self
                .llm_route_registry
                .sources
                .values()
                .filter_map(|spec| account_adapter_for_source_kind(spec.kind.as_ref()))
                .collect();
            for adapter in adapters {
                if let Some(root) = roots.get(&adapter) {
                    self.scanned_accounts(adapter, root);
                }
            }
        }
        let guards: Vec<(AccountAdapter, std::sync::MutexGuard<'_, AccountBook>)> = self
            .account_books
            .iter()
            .map(|(adapter, book)| (*adapter, book.lock().unwrap_or_else(|e| e.into_inner())))
            .collect();
        let books: HashMap<AccountAdapter, &AccountBook> =
            guards.iter().map(|(a, g)| (*a, &**g)).collect();
        route_state::build_route_state(&RouteStateInputs {
            registry: &self.llm_route_registry,
            cos: ctx.cos,
            now: ctx.now,
            worker_reserve_five_hour: ctx.worker_reserve_five_hour,
            holds: ctx.holds,
            cos_holds: ctx.cos_holds,
            scanned: &self.accounts_scan_cache,
            books: &books,
            implicit_cooldowns: &self.source_cooldowns,
            saved_limits: self.account_limits.as_ref(),
            default_max_runs: ctx.default_max_runs,
        })
    }

    // ---- 試験が使う入口（field は露出しない） ----

    #[cfg(test)]
    pub(crate) fn eval_run_handles_for_test(
        &self,
    ) -> impl Iterator<Item = &tokio::task::JoinHandle<()>> {
        self.eval_runs.values().map(|e| &e.handle)
    }

    #[cfg(test)]
    pub(crate) fn insert_source_cooldown_for_test(
        &mut self,
        source: SourceId,
        cooldown: AccountCooldown,
    ) {
        self.source_cooldowns.insert(source, cooldown);
    }

    #[cfg(test)]
    pub(crate) fn set_stored_limit_for_test(
        &mut self,
        source: SourceId,
        account: AccountId,
        max_runs: usize,
    ) {
        if let Some(limits) = self.account_limits.as_mut() {
            limits.insert((source, account), max_runs);
        }
    }
}

#[cfg(test)]
mod structure_guard_tests {
    /// 不変条件: cache は URL ごとに期限未満だけ再利用し、期限ちょうどと明示の消去後は再検査する。
    /// 既存の構造試験は cache の期限境界や probe の呼び出し回数を検出しない。
    #[test]
    fn knowledge_cache_expiry_and_clear() {
        use super::*;
        let store = task_core::SqliteStore::open_in_memory().unwrap();
        let mut routing = RoutingRuntime::new(&store, None);
        let now = Instant::now();
        let calls = std::cell::Cell::new(0);
        let probe = || {
            calls.set(calls.get() + 1);
            Reachability::Ok
        };
        for (url, elapsed, expected) in [
            ("a", Duration::ZERO, 1),
            (
                "a",
                task_worker::PROBE_CACHE_TTL - Duration::from_nanos(1),
                1,
            ),
            ("b", Duration::ZERO, 2),
            ("a", task_worker::PROBE_CACHE_TTL, 3),
        ] {
            assert!(matches!(
                routing.knowledge_reachability(url, now + elapsed, probe),
                Reachability::Ok
            ));
            assert_eq!(calls.get(), expected);
        }
        routing.clear_knowledge_probe_cache();
        routing.knowledge_reachability("a", now + task_worker::PROBE_CACHE_TTL, probe);
        assert_eq!(calls.get(), 4);
    }

    /// 不変条件: ADR 2026-11-11 D2 の 1〜3・7（field が private（`#[cfg(test)]` の field は `dispatcher/tests/structure_guard.rs`）、
    /// `Dispatcher` を受けない・`dispatcher` module を使わない、field は 18 件）。既存の試験は振る舞いだけを見るので、
    /// field を `pub` に戻しても通る。純粋 module `route_state.rs` は読み書き・時計・lock を持たない。
    #[test]
    fn structure_guard_routing_runtime() {
        let src = include_str!("routing_runtime.rs");
        let src = &src[..src
            .find("mod structure_guard_tests")
            .expect("the guard module is in the source")];
        let start = src
            .find("pub struct RoutingRuntime {")
            .expect("RoutingRuntime is defined");
        let body = &src[start..];
        let body = &body[..body.find("\n}\n").expect("the struct body ends")];
        let mut fields = 0;
        for line in body.lines().skip(1) {
            let t = line.trim_start();
            if t.starts_with("///") || t.is_empty() {
                continue;
            }
            assert!(
                !t.starts_with("pub"),
                "a RoutingRuntime field is not private: {line}"
            );
            if t.split_once(": ").is_some_and(|(name, _)| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            }) {
                fields += 1;
            }
        }
        assert_eq!(
            fields, 18,
            "RoutingRuntime owns the 15 routing fields, the quota book, the eval runs and the knowledge probe cache"
        );
        let owner = ["Dis", "patcher"].concat();
        for bad in [
            format!("&mut {owner}"),
            format!("&{owner}"),
            format!("impl {owner}"),
            format!("crate::{}", owner.to_lowercase()),
            format!("super::{}", owner.to_lowercase()),
        ] {
            assert!(
                !src.contains(&bad),
                "routing_runtime.rs must not contain {bad:?}"
            );
        }
        for pure in [
            include_str!("routing_runtime/route_state.rs"),
            include_str!("routing_runtime/quota_estimate.rs"),
        ] {
            let pure = &pure[..pure
                .find("#[cfg(test)]")
                .expect("the pure module has tests")];
            for bad in [
                owner.clone(),
                owner.to_lowercase(),
                "TaskStore".to_string(),
                "store".to_string(),
                "&mut".to_string(),
                "Instant".to_string(),
                "SystemTime".to_string(),
                "now_utc".to_string(),
                "tracing".to_string(),
                "Mutex".to_string(),
                "fs::".to_string(),
            ] {
                assert!(
                    !pure.contains(&bad),
                    "a pure routing module must not contain {bad:?}"
                );
            }
        }
        let quota = include_str!("routing_runtime/quota.rs");
        let quota_struct = quota
            .find("pub struct QuotaBook {")
            .expect("QuotaBook is defined");
        let quota_body = &quota[quota_struct..];
        let quota_body = &quota_body[..quota_body.find("\n}\n").expect("the struct body ends")];
        assert!(
            !quota_body.contains("pub ") || quota_body.matches("pub ").count() == 1,
            "QuotaBook fields are private"
        );
        for bad in [
            format!("&mut {owner}"),
            format!("&{owner}"),
            format!("impl {owner}"),
        ] {
            assert!(!quota.contains(&bad), "quota.rs must not contain {bad:?}");
        }
    }
}
