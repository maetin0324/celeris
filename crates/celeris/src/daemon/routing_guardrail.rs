//! ADR 2026-10-11-llm-source-model-routing-evaluation D3: daemon の tick で `enforce` の guardrail を
//! 決定的に判定し、止める条件に当たった genre×harness を `shadow` へ戻す。
//!
//! - 入力は直近 `guardrail_window` run の窓（[`task_ops::routing_outcome::route_windows`] の投影）と
//!   根拠表（`routing_evidence`）。判定そのものは [`task_core::route_select::evaluate_guardrail`] の純粋関数。
//! - 戻したときは `routing_stage_changed`（actor `daemon`、理由と窓の数値）を追記し、受信箱に通知する。
//! - quota の残量が `guardrail_quota_floor` を切ったときは**段階を戻さず**通知だけする（fallback で済むため）。
//! - `enforce` へ自動的に進めることはしない（進むのは人の承認だけ）。既に `shadow` の pair は何もしない
//!   （同じ判定を tick ごとに繰り返し書かない）。
//! - LLM を呼ばない。時計は引数で注入する（試験は sleep しない）。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use task_core::llm_route::{CandidateKey, HarnessId, Registry};
use task_core::route_select::{
    GuardrailInput, GuardrailReason, RoutingStage, RoutingStageChange, evaluate_guardrail,
};
use task_core::routing_evidence::EvidenceRow;
use task_core::{
    AccountAdapter, NoticeEvent, NoticeKind, NoticeRecordOutcome, NoticeTarget, TaskId, TaskStore,
};
use task_dispatch::{AccountBook, Dispatcher, account_adapter_for_source_kind};
use task_ops::routing_outcome::RouteWindow;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Wilson 95%（ADR D3 の固定値。設定できない）。
pub(crate) const Z: f64 = 1.96;
/// 窓の遡り（ADR D3「2 週間（または 200 run）」の期間側。run 数は `guardrail_window` が決める）。
pub(crate) const LOOKBACK: Duration = Duration::from_secs(14 * 24 * 60 * 60);
/// tick の間隔。判定は決定的なので間隔は結果を変えない（書く量だけ変える）。
pub(crate) const INTERVAL: Duration = Duration::from_secs(900);

#[derive(Debug, thiserror::Error)]
pub(crate) enum GuardrailError {
    #[error("store: {0}")]
    Store(#[from] task_core::StoreError),
    #[error("route window: {0}")]
    Ops(#[from] task_ops::OpsError),
    #[error("config: {0}")]
    Config(String),
    #[error("time: {0}")]
    Time(#[from] time::error::Format),
}

/// 判定の設定（config の `[route_selection]` から。品質閾値は ADR D3 の固定値なのでここに無い）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GuardrailSettings {
    pub window: u64,
    pub quota_floor: f64,
}

impl GuardrailSettings {
    pub(crate) fn from_registry(registry: &Registry) -> Self {
        Self {
            window: registry.route_selection.guardrail_window.max(1),
            quota_floor: registry.route_selection.guardrail_quota_floor,
        }
    }
}

/// 選択先 source の account の最も逼迫した窓の残量（0..=1）。測れなければ `None`（捏造しない）。
pub(crate) trait QuotaProbe: Send + Sync {
    fn min_remaining(&self, sources: &[String], now: OffsetDateTime) -> Option<f64>;
}

/// `dispatcher` の account book（rate limit の観測値の帳簿）から引く既定の実装。
pub(crate) struct AccountBookQuota {
    books: Vec<(AccountAdapter, Arc<StdMutex<AccountBook>>)>,
    adapter_by_source: BTreeMap<String, AccountAdapter>,
    accounts_by_source: BTreeMap<String, Vec<String>>,
}

impl AccountBookQuota {
    pub(crate) fn new(dispatcher: &Dispatcher, registry: &Registry) -> Self {
        let mut books = Vec::new();
        for adapter in [
            AccountAdapter::ClaudeCode,
            AccountAdapter::Codex,
            AccountAdapter::OpencodeGo,
        ] {
            if let Some(book) = dispatcher.account_book(adapter) {
                books.push((adapter, book));
            }
        }
        let mut adapter_by_source = BTreeMap::new();
        let mut accounts_by_source = BTreeMap::new();
        for (id, spec) in &registry.sources {
            let id = id.as_ref().to_owned();
            if let Some(adapter) = account_adapter_for_source_kind(spec.kind.as_ref()) {
                adapter_by_source.insert(id.clone(), adapter);
            }
            accounts_by_source.insert(
                id,
                spec.accounts
                    .iter()
                    .map(|a| a.as_ref().to_owned())
                    .collect(),
            );
        }
        Self {
            books,
            adapter_by_source,
            accounts_by_source,
        }
    }
}

impl QuotaProbe for AccountBookQuota {
    fn min_remaining(&self, sources: &[String], now: OffsetDateTime) -> Option<f64> {
        let now_unix = now.unix_timestamp();
        let mut min: Option<f64> = None;
        for source in sources {
            let adapter = self.adapter_by_source.get(source)?;
            let accounts = self.accounts_by_source.get(source)?;
            let Some((_, book)) = self.books.iter().find(|(a, _)| a == adapter) else {
                continue;
            };
            let Ok(book) = book.lock() else {
                continue;
            };
            for account in accounts {
                let Some(usage) = book.state(account).and_then(|s| s.usage.as_ref()) else {
                    continue;
                };
                if let Some(remaining) =
                    task_dispatch::accounts::measured_remaining(usage, now_unix)
                {
                    min = Some(min.map_or(remaining, |m| m.min(remaining)));
                }
            }
        }
        min
    }
}

/// 1 tick の判定入力（pair 1 つ分）。試験はこれを直接注入する。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PairInput {
    pub window: RouteWindow,
    pub stage: RoutingStage,
    pub evidence: Vec<EvidenceRow>,
    /// 選択先 source の account の最も逼迫した窓の残量。`None` は測れない（低いとは扱わない）。
    pub quota_remaining_fraction: Option<f64>,
    /// 本番の harness の版。`None` は未知（版の食い違いは判定できない）。
    pub live_harness_version: Option<String>,
}

/// 判定の結果（書くもの）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GuardrailAction {
    /// `enforce` → `shadow`。event と通知の両方を書く。
    Revert {
        genre: String,
        harness_id: String,
        task_id: Option<TaskId>,
        reason: String,
        window: serde_json::Value,
        evidence_version: Option<String>,
    },
    /// quota の残量だけが低い。段階は戻さず通知だけする。
    QuotaNotice {
        genre: String,
        harness_id: String,
        remaining: f64,
    },
}

/// 1 tick の報告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GuardrailReport {
    /// 判定した enforce の pair の数。
    pub evaluated: usize,
    /// `shadow` へ戻した数。
    pub reverted: usize,
    /// 受信箱に書いた通知の数（重複は数えない）。
    pub notices: usize,
    /// 根拠表の行が無く判定できない pair（`enforce` のまま何もしない）。
    pub skipped_no_evidence: usize,
    /// 本番の harness の版が未知で、版の食い違いを判定できなかった pair。
    pub harness_version_unknown: usize,
}

/// pair 1 つ分の判定（純粋）。既に `shadow` なら何も返さない。
pub(crate) fn plan_pair(
    input: &PairInput,
    settings: &GuardrailSettings,
    priority: &[CandidateKey],
) -> Option<GuardrailAction> {
    if input.stage == RoutingStage::Shadow {
        return None;
    }
    let selected = select_evidence(&input.evidence, priority)?;
    let baseline_runs = selected.n;
    let failure_baseline = (selected.timeout_rate * baseline_runs as f64).round() as u64;
    let guardrail_input = GuardrailInput {
        successes: input.window.successes,
        runs: input.window.runs,
        failures: input.window.failures,
        failure_baseline,
        baseline_runs,
        wall_median_s: input.window.wall_median_s,
        evidence_wall_median_s: selected.wall_p90_s,
        selected_pass_lower: selected.pass_lower,
        // 測れない quota を「低い」とは扱わない（通知も出さない）。
        quota_remaining_fraction: input.quota_remaining_fraction.unwrap_or(1.0),
        // 本番の版が未知の間は版の食い違いを判定できない（根拠表の版をそのまま使う＝食い違わない）。
        live_harness_version: input
            .live_harness_version
            .clone()
            .unwrap_or_else(|| selected.key.harness_version.clone()),
        evidence_harness_version: selected.key.harness_version.clone(),
        window: settings.window,
        z: Z,
        quota_floor: settings.quota_floor,
    };
    let result = evaluate_guardrail(input.stage, &guardrail_input);
    let stopping: Vec<GuardrailReason> = result
        .reasons
        .iter()
        .filter(|reason| **reason != GuardrailReason::QuotaLow)
        .cloned()
        .collect();
    if !stopping.is_empty() {
        return Some(GuardrailAction::Revert {
            genre: input.window.genre.clone(),
            harness_id: input.window.harness_id.clone(),
            task_id: input.window.task_id,
            reason: format!(
                "guardrail: {}",
                stopping
                    .iter()
                    .map(reason_label)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            window: window_json(input, &guardrail_input, &stopping),
            evidence_version: Some(selected.evidence_version.clone()),
        });
    }
    if result.notify {
        return Some(GuardrailAction::QuotaNotice {
            genre: input.window.genre.clone(),
            harness_id: input.window.harness_id.clone(),
            remaining: input.quota_remaining_fraction.unwrap_or(1.0),
        });
    }
    None
}

fn reason_label(reason: &GuardrailReason) -> &'static str {
    match reason {
        GuardrailReason::PassRate => "pass_rate",
        GuardrailReason::FailureRate => "failure_rate",
        GuardrailReason::WallClock => "wall_clock",
        GuardrailReason::EvidenceHarnessMajorMismatch => "evidence_harness_major_mismatch",
        GuardrailReason::QuotaLow => "quota_low",
    }
}

/// event と通知に載せる窓の数値（人が「なぜ戻ったか」を判定だけで読めるようにする）。
fn window_json(
    input: &PairInput,
    guardrail: &GuardrailInput,
    reasons: &[GuardrailReason],
) -> serde_json::Value {
    serde_json::json!({
        "runs": input.window.runs,
        "successes": input.window.successes,
        "failures": input.window.failures,
        "wall_median_s": input.window.wall_median_s,
        "window": guardrail.window,
        "reasons": reasons.iter().map(reason_label).collect::<Vec<_>>(),
        "selected_pass_lower": guardrail.selected_pass_lower,
        "failure_baseline": guardrail.failure_baseline,
        "baseline_runs": guardrail.baseline_runs,
        "evidence_wall_median_s": guardrail.evidence_wall_median_s,
        "quota_remaining_fraction": guardrail.quota_remaining_fraction,
    })
}

/// 根拠表の行から「選ばれた候補」を 1 行選ぶ（D2 と同じ優先順: `route_defaults` の順、そこに無ければ
/// `n` が最も大きい行、同数なら key の辞書順で決定的に）。
pub(crate) fn select_evidence<'a>(
    rows: &'a [EvidenceRow],
    priority: &[CandidateKey],
) -> Option<&'a EvidenceRow> {
    for key in priority {
        if let Some(row) = rows.iter().find(|row| {
            row.key.source_id == key.source.as_ref() && row.key.model_id == key.model.as_ref()
        }) {
            return Some(row);
        }
    }
    rows.iter()
        .max_by(|a, b| a.n.cmp(&b.n).then_with(|| b.key.cmp(&a.key)))
}

/// 判定を書いて反映する（stage の書き込みと event の追記は store の 1 transaction、通知は受信箱の束）。
pub(crate) fn apply(
    store: &dyn TaskStore,
    actions: &[GuardrailAction],
    now: OffsetDateTime,
) -> Result<GuardrailReport, GuardrailError> {
    let since = now.format(&Rfc3339)?;
    let mut report = GuardrailReport::default();
    for action in actions {
        match action {
            GuardrailAction::Revert {
                genre,
                harness_id,
                task_id,
                reason,
                window,
                evidence_version,
            } => {
                // event は窓で最も新しい run の task の events へ追記する（宛先が無ければ書かない）。
                let Some(task_id) = task_id else {
                    tracing::warn!(
                        genre,
                        harness_id,
                        "routing guardrail: no task to attach routing_stage_changed; skipped"
                    );
                    continue;
                };
                let change = RoutingStageChange {
                    genre: genre.clone(),
                    harness_id: harness_id.clone(),
                    stage: RoutingStage::Shadow,
                    since: since.clone(),
                    evidence_version: evidence_version.clone(),
                    reason: reason.clone(),
                    window: window.clone(),
                    actor: "daemon".into(),
                };
                store.set_routing_stage(*task_id, &change)?;
                report.reverted += 1;
                report.notices += usize::from(record_notice(
                    store,
                    &NoticeEvent {
                        source_key: format!(
                            "routing_stage:{genre}:{harness_id}:{}",
                            now.unix_timestamp()
                        ),
                        kind: NoticeKind::BadNews,
                        group_key: format!("routing_stage:{genre}:{harness_id}"),
                        title: format!("routing: {genre}/{harness_id} を shadow に戻した"),
                        summary: reason.clone(),
                        project_id: None,
                        task_id: Some(task_id.to_string()),
                        target: Some(NoticeTarget {
                            kind: "task".into(),
                            id: task_id.to_string(),
                        }),
                        links: Vec::new(),
                        at: now,
                    },
                )?);
            }
            GuardrailAction::QuotaNotice {
                genre,
                harness_id,
                remaining,
            } => {
                // 1 日 1 件（同じ日の同じ判定を tick ごとに書き直さない）。
                let day = now.date();
                report.notices += usize::from(record_notice(
                    store,
                    &NoticeEvent {
                        source_key: format!("routing_quota:{genre}:{harness_id}:{day}"),
                        kind: NoticeKind::BadNews,
                        group_key: format!("routing_quota:{genre}:{harness_id}"),
                        title: format!("routing: {genre}/{harness_id} の quota 残量が低い"),
                        summary: format!(
                            "最も逼迫した窓の残量 {remaining:.3}（段階は enforce のまま。fallback で対応する）"
                        ),
                        project_id: None,
                        task_id: None,
                        target: None,
                        links: Vec::new(),
                        at: now,
                    },
                )?);
            }
        }
    }
    Ok(report)
}

/// 通知を 1 件書く。同じ `source_key` のときは数えない（`Duplicate`）。
fn record_notice(store: &dyn TaskStore, event: &NoticeEvent) -> Result<bool, GuardrailError> {
    Ok(!matches!(
        store.notice_record(event)?,
        NoticeRecordOutcome::Duplicate(_)
    ))
}

/// 1 tick: 窓を集め、`enforce` の pair だけを判定し、結果を書く。
pub(crate) fn run_tick(
    store: &dyn TaskStore,
    registry: &Registry,
    quota: &dyn QuotaProbe,
    live_harness_versions: &BTreeMap<String, String>,
    now: OffsetDateTime,
) -> Result<GuardrailReport, GuardrailError> {
    let settings = GuardrailSettings::from_registry(registry);
    let windows = task_ops::routing_outcome::route_windows(store, now - LOOKBACK, settings.window)?;
    let mut inputs = Vec::new();
    let mut report = GuardrailReport::default();
    for window in windows {
        let stage = store.routing_stage(&window.genre, &window.harness_id)?;
        if stage == RoutingStage::Shadow {
            // 既に shadow なら書くものが無い（event も通知も繰り返さない）。
            continue;
        }
        let evidence = store.latest_routing_evidence(&window.genre, &window.harness_id)?;
        if evidence.is_empty() {
            report.skipped_no_evidence += 1;
            continue;
        }
        if live_harness_versions.get(&window.harness_id).is_none() {
            report.harness_version_unknown += 1;
        }
        let sources: Vec<String> = evidence
            .iter()
            .map(|row| row.key.source_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        inputs.push(PairInput {
            quota_remaining_fraction: quota.min_remaining(&sources, now),
            live_harness_version: live_harness_versions.get(&window.harness_id).cloned(),
            window,
            stage,
            evidence,
        });
    }
    let actions: Vec<GuardrailAction> = inputs
        .iter()
        .filter_map(|input| {
            let harness = HarnessId::from(input.window.harness_id.clone());
            plan_pair(
                input,
                &settings,
                registry
                    .route_defaults
                    .get(&harness)
                    .map_or(&[], Vec::as_slice),
            )
        })
        .collect();
    report.evaluated = inputs.len();
    let mut applied = apply(store, &actions, now)?;
    applied.evaluated = report.evaluated;
    applied.skipped_no_evidence = report.skipped_no_evidence;
    applied.harness_version_unknown = report.harness_version_unknown;
    Ok(applied)
}

/// tick の間隔の throttle（時計は注入する。`due` は状態を変える）。
#[derive(Debug, Default)]
pub(crate) struct Ticker {
    last: Option<OffsetDateTime>,
}

impl Ticker {
    pub(crate) fn due(&mut self, interval: Duration, now: OffsetDateTime) -> bool {
        let seconds = interval.as_secs() as i64;
        let run = self
            .last
            .is_none_or(|last| (now - last).whole_seconds() >= seconds);
        if run {
            self.last = Some(now);
        }
        run
    }
}

/// tick ループからの入口（`active` のときだけ、`INTERVAL` 間隔で呼ぶ）。
pub(crate) fn tick(
    dispatcher: &Dispatcher,
    config: &crate::Config,
    now: OffsetDateTime,
) -> Result<GuardrailReport, GuardrailError> {
    let registry = config
        .llm_route_registry()
        .map_err(|error| GuardrailError::Config(error.to_string()))?;
    let store = dispatcher.store();
    let quota = AccountBookQuota::new(dispatcher, &registry);
    // 本番の harness の版を測る口はまだ無い（根拠表の `harness_version` を書くのは評価 runner）。
    // 未知の間は版の食い違いを判定しない（値を捏造しない）。
    let live_harness_versions = BTreeMap::new();
    run_tick(
        store.as_ref(),
        &registry,
        &quota,
        &live_harness_versions,
        now,
    )
}

#[cfg(test)]
#[path = "routing_guardrail_tests.rs"]
mod tests;
