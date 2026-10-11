//! Harness selection and LLM source×model routing at dispatch time
//! (ADR 2026-10-11 llm-source-harness-selection).
//!
//! The order is fixed: the task's `harness` field (a human's explicit choice), otherwise the
//! declared `[harness_selection]` rules; then the `route_defaults.<harness>` candidates in their
//! declared order, narrowed by an explicit task model; harness/source compatibility (direct API
//! format or a registered `[[llm_routes]]` entry); `(source, account)` admission; and finally the
//! route policy. Every step fails closed: nothing falls back to an undeclared source.

use super::*;
use std::collections::BTreeMap;
use task_core::llm_route::{
    AccountId, AccountSnapshot, Admission, Candidate, CandidateKey, CandidateRejection,
    DefaultRoutePolicy, ExclusionReason, HarnessId, HarnessRequest, HarnessSelectionError, ModelId,
    QuotaSnapshot, Registry, ReservationSnapshot, RoutePolicy, RouteRequest, RunSelection,
    SourceId, admit_source, constrain_for_harness, enumerate_candidates, route_priority,
    select_harness,
};

/// Version of the `selection_metadata_json` / `llm_route_selected.metadata` shape.
pub const ROUTE_SELECTION_VERSION: u32 = 1;

/// Capability a task needs when it carries browser requirements.
pub const BROWSER_CAPABILITY: &str = "browser_context";

/// Mutable state the admission step reads, captured once per decision.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteState {
    pub accounts: AccountSnapshot,
    pub quota: QuotaSnapshot,
    pub reservations: ReservationSnapshot,
    /// Per `(source, account)` run limit (stored `account_limits` or the default).
    pub account_max_runs: BTreeMap<(SourceId, AccountId), u32>,
}

/// One routing request: the harness inputs plus the source/model inputs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteInput {
    pub task_id: String,
    pub harness: HarnessRequest,
    pub model_hint: Option<ModelId>,
    /// Kept for the replaceable policy boundary; the default policy does not read it.
    pub lane: Option<String>,
    /// CoS chat: priority comes from `cos.route_candidates`.
    pub cos: bool,
}

/// The admitted route.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutePick {
    pub harness: HarnessId,
    pub candidate: Candidate,
    pub account: AccountId,
}

/// Why no route was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteFailure {
    Harness(HarnessSelectionError),
    /// No `route_defaults.<harness>` (or CoS) candidates are declared.
    NoDeclaredDefault(HarnessId),
    /// The task names a model that no declared candidate offers.
    ModelHintUnmatched(ModelId),
    /// Every declared candidate was rejected (see the trace).
    NoEligible(HarnessId),
}

impl RouteFailure {
    pub fn reason(&self) -> String {
        match self {
            RouteFailure::Harness(e) => format!("harness selection failed: {e}"),
            RouteFailure::NoDeclaredDefault(h) => {
                format!("no route_defaults candidates are declared for harness {h}")
            }
            RouteFailure::ModelHintUnmatched(m) => {
                format!("no declared route candidate offers the requested model {m}")
            }
            RouteFailure::NoEligible(h) => {
                format!("no declared route candidate for harness {h} is eligible")
            }
        }
    }
}

/// What was considered: declared candidates in priority order and why each was dropped.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteTrace {
    pub harness: Option<HarnessId>,
    pub candidates: Vec<CandidateKey>,
    pub rejections: Vec<CandidateRejection>,
}

impl RouteTrace {
    /// Versioned payload for `llm_route_selected.metadata` and `runs.selection_metadata_json`.
    pub fn metadata(
        &self,
        input: &RouteInput,
        decision: Option<&RoutePick>,
        failure: Option<&RouteFailure>,
    ) -> serde_json::Value {
        serde_json::json!({
            "version": ROUTE_SELECTION_VERSION,
            "harness": self.harness,
            "harness_explicit": input.harness.harness.is_some(),
            "cos": input.cos,
            "model_hint": input.model_hint,
            "lane": input.lane,
            "candidates": self.candidates,
            "rejections": self.rejections.iter().map(|r| serde_json::json!({
                "source": r.candidate.source,
                "model": r.candidate.model,
                "reason": r.reason,
            })).collect::<Vec<_>>(),
            "decision": decision.map(|p| serde_json::json!({
                "source": p.candidate.source,
                "model": p.candidate.model,
                "account": p.account,
            })),
            "failure": failure.map(RouteFailure::reason),
        })
    }
}

/// Pure decision: no I/O, no clock. Same inputs give the same pick.
pub fn route_select(
    input: &RouteInput,
    registry: &Registry,
    state: &RouteState,
) -> (Result<RoutePick, RouteFailure>, RouteTrace) {
    let mut trace = RouteTrace::default();
    let harness = match select_harness(&input.harness, registry) {
        Ok(h) => h,
        Err(e) => return (Err(RouteFailure::Harness(e)), trace),
    };
    trace.harness = Some(harness.clone());
    let priority = route_priority(registry, &harness, input.cos).to_vec();
    if priority.is_empty() {
        return (Err(RouteFailure::NoDeclaredDefault(harness)), trace);
    }
    trace.candidates = priority.clone();
    let request = RouteRequest {
        task_id: input.task_id.clone(),
        harness: harness.clone(),
        model_hint: input.model_hint.clone(),
        lane: input.lane.clone(),
        execution_class: input.harness.execution_class.clone(),
    };
    let declared = enumerate_candidates(&request, registry);
    // Declared priority entries in order; drop (and record) the ones the source does not offer
    // or the explicit model hint excludes.
    let mut ordered = Vec::new();
    for key in &priority {
        let Some(source) = registry.sources.get(&key.source) else {
            trace
                .rejections
                .push(rejection(key, "", ExclusionReason::UnknownSource));
            continue;
        };
        if let Some(c) = declared.iter().find(|c| c.key() == *key) {
            ordered.push(c.clone());
        } else {
            let reason = if source.models.contains(&key.model) {
                ExclusionReason::ModelHintMismatch
            } else {
                ExclusionReason::ModelNotDeclared
            };
            trace
                .rejections
                .push(rejection(key, source.api_format.as_ref(), reason));
        }
    }
    if ordered.is_empty()
        && let Some(hint) = &input.model_hint
    {
        return (Err(RouteFailure::ModelHintUnmatched(hint.clone())), trace);
    }
    let Some(spec) = registry.harnesses.get(&harness) else {
        return (
            Err(RouteFailure::Harness(HarnessSelectionError::Unknown(
                harness,
            ))),
            trace,
        );
    };
    let (eligible, incompatible) = constrain_for_harness(ordered, spec, registry, &harness);
    trace.rejections.extend(incompatible);
    let limited = with_account_limits(registry, state);
    let mut admitted = Vec::new();
    let mut owners = Vec::new();
    for c in eligible {
        match admit_source(
            &c,
            &state.accounts,
            &state.quota,
            &state.reservations,
            &limited,
        ) {
            Admission::Admitted(owner) => {
                owners.push((c.key(), owner.account_id));
                admitted.push(c);
            }
            Admission::Rejected(reason) => trace.rejections.push(CandidateRejection {
                candidate: c,
                reason,
            }),
        }
    }
    let policy = DefaultRoutePolicy { priority };
    match policy.choose(&admitted) {
        Ok(candidate) => {
            let account = owners
                .iter()
                .find(|(k, _)| *k == candidate.key())
                .map(|(_, a)| a.clone())
                .unwrap_or_default();
            (
                Ok(RoutePick {
                    harness,
                    candidate,
                    account,
                }),
                trace,
            )
        }
        Err(_) => (Err(RouteFailure::NoEligible(harness)), trace),
    }
}

fn rejection(key: &CandidateKey, api_format: &str, reason: ExclusionReason) -> CandidateRejection {
    CandidateRejection {
        candidate: Candidate {
            source: key.source.clone(),
            model: key.model.clone(),
            api_format: api_format.into(),
        },
        reason,
    }
}

/// The registry with the per-account limits of `state` applied to its sources.
fn with_account_limits(registry: &Registry, state: &RouteState) -> Registry {
    let mut out = registry.clone();
    for ((source, account), max) in &state.account_max_runs {
        if let Some(spec) = out.sources.get_mut(source) {
            spec.account_max_runs.insert(account.clone(), *max);
        }
    }
    out
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

/// Harness inputs taken from the task's structured fields only (never its prose).
pub fn harness_request_for(task: &Task) -> HarnessRequest {
    let mut required_capabilities = std::collections::BTreeSet::new();
    if task.requirements.browser.is_some() {
        required_capabilities.insert(BROWSER_CAPABILITY.to_string());
    }
    HarnessRequest {
        harness: task.harness_id.clone(),
        genre: task.genre.clone(),
        role: task.role.clone(),
        required_capabilities,
        has_repo: !task.repos.is_empty(),
        execution_class: None,
    }
}

impl Dispatcher {
    /// Daemon wiring: the declared sources, harnesses, routes and defaults.
    pub fn set_llm_route_registry(&mut self, registry: Registry) {
        self.llm_route_registry = Arc::new(registry);
    }

    pub fn llm_route_registry(&self) -> &Registry {
        &self.llm_route_registry
    }

    /// Capture the admission state of every declared source: logged-in accounts of OAuth
    /// pools (or the declared accounts of other sources), cooldowns and exhausted windows from
    /// the account books, stored per-account limits, and runs holding a `(source, account)`.
    pub(super) fn route_state(&mut self, cos: bool) -> RouteState {
        let registry = Arc::clone(&self.llm_route_registry);
        let now = (self.now_unix_fn)();
        let mut state = RouteState::default();
        for entry in self.running.values() {
            if let Some((source, account)) = &entry.route {
                // A leaf run's admission does not count CoS chat runs (they hold their own slot).
                if !cos && entry.cos {
                    continue;
                }
                *state
                    .reservations
                    .source_in_use
                    .entry(source.clone())
                    .or_default() += 1;
                *state
                    .reservations
                    .account_in_use
                    .entry((source.clone(), account.clone()))
                    .or_default() += 1;
            }
        }
        // Reviewer runs hold their `(source, account)` the same way.
        for (source, account) in self.reviewing.values().filter_map(|e| e.route.as_ref()) {
            *state
                .reservations
                .source_in_use
                .entry(source.clone())
                .or_default() += 1;
            *state
                .reservations
                .account_in_use
                .entry((source.clone(), account.clone()))
                .or_default() += 1;
        }
        for (source_id, spec) in &registry.sources {
            if cos && let Some(launch) = self.cos_chat_launch.as_ref() {
                for (source, account) in launch.route_reservations.values() {
                    if source == source_id {
                        *state
                            .reservations
                            .source_in_use
                            .entry(source.clone())
                            .or_default() += 1;
                        *state
                            .reservations
                            .account_in_use
                            .entry((source.clone(), account.clone()))
                            .or_default() += 1;
                    }
                }
            }
            let mut accounts: Vec<AccountId> = spec.accounts.clone();
            if let Some(adapter) = account_adapter_for_source_kind(spec.kind.as_ref()) {
                let scanned = match (
                    self.config.accounts.clone(),
                    self.accounts_scan_cache.get(&adapter),
                ) {
                    (_, Some(dirs)) => dirs.clone(),
                    (Some(cfg), None) => match cfg.root_for(adapter) {
                        Some(root) => {
                            let dirs = scan_accounts(root, adapter);
                            self.accounts_scan_cache.insert(adapter, dirs.clone());
                            dirs
                        }
                        None => Vec::new(),
                    },
                    (None, None) => Vec::new(),
                };
                let logged_in: Vec<AccountId> = scanned
                    .iter()
                    .filter(|d| d.logged_in)
                    .map(|d| AccountId::from(d.id.as_str()))
                    .collect();
                accounts = if accounts.is_empty() {
                    logged_in
                } else {
                    accounts
                        .into_iter()
                        .filter(|a| logged_in.contains(a))
                        .collect()
                };
                if let Some(book) = self.account_book(adapter) {
                    let book = book.lock().unwrap_or_else(|e| e.into_inner());
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
                    for account in &accounts {
                        let key = (source_id.clone(), account.clone());
                        let candidate = AccountCandidate {
                            id: account.as_ref(),
                            logged_in: true,
                            in_use: 0,
                        };
                        match evaluate(&candidate, book.state(account.as_ref()), usize::MAX, now)
                            .excluded
                        {
                            Some(ExcludedReason::Cooldown) => {
                                state.quota.cooling_down.insert(key);
                            }
                            Some(_) => {
                                state.quota.account_remaining.insert(key, 0.0);
                            }
                            None => {}
                        }
                    }
                }
            }
            if self.account_limits.is_some() {
                for account in &accounts {
                    let key = (source_id.clone(), account.clone());
                    let stored = self
                        .account_limits
                        .as_ref()
                        .and_then(|l| l.get(&key).copied());
                    let (effective, _) =
                        crate::capacity::effective_account_limit(stored, self.default_max_runs());
                    let limit = crate::capacity::account_run_limit(effective, cos);
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

    /// Decide the route for `input` against the current registry and admission state.
    pub fn select_route(
        &mut self,
        input: &RouteInput,
    ) -> (Result<RoutePick, RouteFailure>, RouteTrace) {
        let state = self.route_state(input.cos);
        route_select(input, &self.llm_route_registry, &state)
    }

    /// Select from the remaining CoS candidates after earlier attempts failed.
    pub(super) fn select_cos_route_from(
        &mut self,
        input: &RouteInput,
        start: usize,
    ) -> (Result<RoutePick, RouteFailure>, RouteTrace) {
        let state = self.route_state(true);
        let mut registry = (*self.llm_route_registry).clone();
        registry.cos_route_candidates = registry
            .cos_route_candidates
            .into_iter()
            .skip(start)
            .collect();
        route_select(input, &registry, &state)
    }

    /// Append `harness_selected` and `llm_route_selected` for an admitted route and write the
    /// selection onto the run row. A refused route appends the refusal as worker progress.
    pub fn record_route_selection(
        &self,
        task_id: TaskId,
        run_id: Option<&str>,
        input: &RouteInput,
        outcome: &Result<RoutePick, RouteFailure>,
        trace: &RouteTrace,
    ) -> Result<(), DispatchError> {
        let (pick, failure) = match outcome {
            Ok(p) => (Some(p), None),
            Err(f) => (None, Some(f)),
        };
        let metadata = trace.metadata(input, pick, failure);
        if let Some(harness) = &trace.harness {
            self.store.append_event(
                task_id,
                &Event::HarnessSelected {
                    harness_id: harness.to_string(),
                },
            )?;
        }
        match pick {
            Some(pick) => {
                self.store.append_event(
                    task_id,
                    &Event::LlmRouteSelected {
                        source_id: pick.candidate.source.to_string(),
                        model_id: pick.candidate.model.to_string(),
                        account_id: pick.account.to_string(),
                        metadata: metadata.clone(),
                    },
                )?;
                if let Some(run_id) = run_id {
                    self.store.run_record_selection(
                        run_id,
                        &RunSelection {
                            harness: pick.harness.clone(),
                            source: pick.candidate.source.clone(),
                            model: pick.candidate.model.clone(),
                            account: pick.account.clone(),
                            metadata,
                        },
                    )?;
                }
            }
            None => {
                self.store.append_event(
                    task_id,
                    &Event::worker_progress(
                        "routing",
                        format!(
                            "route refused: {}; selection: {metadata}",
                            failure.map(RouteFailure::reason).unwrap_or_default()
                        ),
                    ),
                )?;
            }
        }
        Ok(())
    }
}
