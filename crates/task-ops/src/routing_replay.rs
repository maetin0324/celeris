//! Frozen, read-only routing replay. Only explicitly listed audit fields leave the database;
//! event JSON, prompts, responses and credentials are never copied into a dataset.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use task_core::Event;
use task_core::model_router::metrics::{self, PairedBaseline, PairedCurvePoint};
use task_core::model_router::trace::RoutingTraceV1;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub const DATASET_SCHEMA: &str = "celeris.routing.dataset.v1";
pub const REPORT_SCHEMA: &str = "celeris.routing.report.v1";

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("timestamp: {0}")]
    Time(#[from] time::error::Parse),
    #[error("invalid replay input: {0}")]
    Invalid(&'static str),
}

/// The caller supplies the frozen input identities. The extraction never reads configuration files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportOptions {
    pub policy_hash: String,
    pub catalog_hash: String,
    pub estimator_hash: String,
    pub from_utc: Option<String>,
    pub until_utc: Option<String>,
    pub seed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestV1 {
    pub schema: String,
    pub policy_hash: String,
    pub catalog_hash: String,
    pub estimator_hash: String,
    pub from_utc: Option<String>,
    pub until_utc: Option<String>,
    pub extraction: String,
    pub masking: String,
    pub split: String,
    pub seed: u64,
    pub rows: usize,
    pub missing_rate: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Split {
    Train,
    Calibration,
    Test,
}

/// Stable across process runs and platform hash implementations.
pub fn split_for_task(task_id: &str, seed: u64) -> Split {
    let mut h = Sha256::new();
    h.update(seed.to_be_bytes());
    h.update(task_id.as_bytes());
    match u64::from_be_bytes(h.finalize()[..8].try_into().unwrap_or([0; 8])) % 10 {
        0..=6 => Split::Train,
        7 => Split::Calibration,
        _ => Split::Test,
    }
}

/// One decision and its correlated audit observations. No arbitrary event payloads are exported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetRowV1 {
    pub schema: String,
    pub task_id: String,
    pub split: Split,
    pub run_id: String,
    pub decision_id: String,
    pub primary_model: Option<String>,
    pub primary_source: Option<String>,
    pub candidates: Vec<CandidateV1>,
    pub context_version: Option<String>,
    pub task_kind: Option<String>,
    pub role: Option<String>,
    pub missing_features: Vec<String>,
    pub request_ids: Vec<String>,
    pub acceptance_passed: Option<bool>,
    pub review_passed: Option<bool>,
    pub failed_criterion_ids: Vec<String>,
    pub cash_usd: Option<f64>,
    pub effective_usd: Option<f64>,
    pub api_latencies_ms: Vec<u64>,
    pub task_wall_ms: Option<u64>,
    pub retries: Option<u32>,
    pub escalated: bool,
    pub quota_exhausted: bool,
    pub shadows: Vec<ShadowV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateV1 {
    pub model: String,
    pub source: String,
    pub eligible: bool,
    pub score: Option<f64>,
    pub cash_usd: Option<f64>,
    pub effective_usd: Option<f64>,
    pub latency_ms: Option<f64>,
    pub excluded_reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowV1 {
    pub kind: String,
    pub status: String,
    pub reason: Option<String>,
    pub policy_version: String,
    pub model: Option<String>,
    pub source: Option<String>,
    pub cash_usd: Option<f64>,
    pub effective_usd: Option<f64>,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetV1 {
    pub manifest: ManifestV1,
    pub rows: Vec<DatasetRowV1>,
}

impl DatasetV1 {
    pub fn jsonl(&self) -> Result<String, ReplayError> {
        let mut out = String::new();
        for row in &self.rows {
            out.push_str(&serde_json::to_string(row)?);
            out.push('\n');
        }
        Ok(out)
    }
}

fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|n| n.is_finite() && *n >= 0.0)
        .map(|n| (n * 1_000_000.0).round() / 1_000_000.0)
}

fn wire_enum<T: Serialize>(value: &T) -> Option<String> {
    serde_json::to_value(value)
        .ok()?
        .as_str()
        .map(str::to_owned)
}

fn candidates(trace: Option<&RoutingTraceV1>) -> Vec<CandidateV1> {
    let mut out: Vec<_> = trace
        .into_iter()
        .flat_map(|t| &t.candidates)
        .map(|c| CandidateV1 {
            model: c.model_profile_id.clone(),
            source: c.deployment_id.clone(),
            eligible: c.excluded_reasons.is_empty(),
            score: finite(c.score),
            cash_usd: finite(c.cash_usd.or(c.cost_usd)),
            effective_usd: finite(c.effective_usd),
            latency_ms: finite(c.latency_ms),
            excluded_reasons: {
                let mut reasons = c.excluded_reasons.clone();
                reasons.sort();
                reasons.dedup();
                reasons
            },
        })
        .collect();
    out.sort_by(|a, b| a.source.cmp(&b.source).then_with(|| a.model.cmp(&b.model)));
    out
}

/// Opens the existing SQLite file without migration, WAL creation or any write-capable handle.
pub fn export(db_path: &Path, options: &ExportOptions) -> Result<DatasetV1, ReplayError> {
    let from = options
        .from_utc
        .as_deref()
        .map(|s| OffsetDateTime::parse(s, &Rfc3339))
        .transpose()?;
    let until = options
        .until_utc
        .as_deref()
        .map(|s| OffsetDateTime::parse(s, &Rfc3339))
        .transpose()?;
    if from.zip(until).is_some_and(|(from, until)| from > until) {
        return Err(ReplayError::Invalid("from_utc exceeds until_utc"));
    }
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare("SELECT task_id, ts, json FROM events ORDER BY task_id, seq")?;
    let raw = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut tasks: BTreeMap<String, Vec<Event>> = BTreeMap::new();
    for item in raw {
        let (task_id, ts, json) = item?;
        let ts = OffsetDateTime::parse(&ts, &Rfc3339)?;
        if from.is_some_and(|from| ts < from) || until.is_some_and(|until| ts >= until) {
            continue;
        }
        // Decoding into Event prevents arbitrary JSON from reaching the output.
        tasks
            .entry(task_id)
            .or_default()
            .push(serde_json::from_str(&json)?);
    }
    let mut rows = Vec::new();
    for (task_id, events) in tasks {
        let split = split_for_task(&task_id, options.seed);
        let mut by_run: BTreeMap<String, Vec<DatasetRowV1>> = BTreeMap::new();
        for event in &events {
            let Event::RoutingDecided { run_id, record } = event else {
                continue;
            };
            let trace = record.optimizer.as_ref();
            if trace.is_some_and(|t| t.stage == "proxy") {
                continue;
            }
            let decision_id = trace.map_or_else(
                || format!("legacy:{task_id}:{run_id}"),
                |t| t.decision_id.clone(),
            );
            by_run
                .entry(run_id.clone())
                .or_default()
                .push(DatasetRowV1 {
                    schema: DATASET_SCHEMA.into(),
                    task_id: task_id.clone(),
                    split: split.clone(),
                    run_id: run_id.clone(),
                    decision_id,
                    primary_model: trace.and_then(|t| t.model.clone()).or_else(|| {
                        (!record.resolution.model_id.is_empty())
                            .then(|| record.resolution.model_id.clone())
                    }),
                    primary_source: trace
                        .and_then(|t| t.source_id.clone().or_else(|| t.selected.clone()))
                        .or_else(|| record.resolution.provider.clone()),
                    candidates: candidates(trace),
                    context_version: None,
                    task_kind: None,
                    role: None,
                    missing_features: Vec::new(),
                    request_ids: Vec::new(),
                    acceptance_passed: None,
                    review_passed: None,
                    failed_criterion_ids: Vec::new(),
                    cash_usd: None,
                    effective_usd: None,
                    api_latencies_ms: Vec::new(),
                    task_wall_ms: None,
                    retries: None,
                    escalated: record.escalation.is_some(),
                    quota_exhausted: record.quota_reason.is_some(),
                    shadows: Vec::new(),
                });
        }
        let mut last_worker_run: Option<String> = None;
        let mut failed_review_criteria = Vec::new();
        for event in &events {
            match event {
                Event::WorkerStarted { run_id, role, .. }
                    if *role != Some(task_core::RunRole::Reviewer) =>
                {
                    last_worker_run = Some(run_id.clone());
                }
                Event::WorkerFinished {
                    run_id,
                    usage,
                    metrics,
                    role,
                    ..
                } if *role != Some(task_core::RunRole::Reviewer) => {
                    for row in by_run.get_mut(run_id).into_iter().flatten() {
                        if let Some(usage) = usage {
                            row.cash_usd = finite(usage.cost_usd);
                        }
                        if let Some(metrics) = metrics {
                            row.task_wall_ms = Some(metrics.wall_ms);
                            row.retries = Some(metrics.retries);
                        }
                    }
                }
                Event::ReviewVerdict {
                    criterion_idx,
                    pass: false,
                    ..
                } => {
                    failed_review_criteria.push(format!("acceptance:{criterion_idx}"));
                }
                Event::Transitioned { reason, .. }
                    if reason == "review_pass" || reason == "review_fail" =>
                {
                    if let Some(run_id) = &last_worker_run {
                        for row in by_run.get_mut(run_id).into_iter().flatten() {
                            row.review_passed = Some(reason == "review_pass");
                            row.failed_criterion_ids = failed_review_criteria.clone();
                        }
                    }
                    failed_review_criteria.clear();
                }
                Event::RoutingFeaturesRecorded { record } => {
                    for row in by_run
                        .values_mut()
                        .flatten()
                        .filter(|r| r.decision_id == record.decision_id)
                    {
                        row.context_version = Some(record.context_version.clone());
                        row.task_kind = record
                            .features
                            .get("task_kind")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned);
                        row.role = record
                            .features
                            .get("role")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned);
                        row.missing_features = record.missing_fields.clone();
                        row.missing_features.sort();
                        row.missing_features.dedup();
                    }
                }
                Event::RoutingRequestDecided { record } => {
                    let parent = record.parent_decision_id.as_deref();
                    for row in by_run.values_mut().flatten().filter(|r| {
                        Some(r.decision_id.as_str()) == parent
                            || (parent.is_none()
                                && record.run_id.as_deref() == Some(r.run_id.as_str()))
                    }) {
                        row.request_ids.push(record.request_id.clone());
                    }
                }
                Event::RoutingDecided { run_id, record }
                    if record
                        .optimizer
                        .as_ref()
                        .is_some_and(|t| t.stage == "proxy") =>
                {
                    if let Some(trace) = &record.optimizer
                        && let Some(request_id) = &trace.request_id
                    {
                        for row in by_run.values_mut().flatten().filter(|r| {
                            trace.parent_decision_id.as_deref() == Some(r.decision_id.as_str())
                                || (trace.parent_decision_id.is_none()
                                    && trace.run_id.as_deref().unwrap_or(run_id) == r.run_id)
                        }) {
                            row.request_ids.push(request_id.clone());
                        }
                    }
                }
                Event::RoutingOutcomeRecorded { outcome } => {
                    if outcome.request_id.is_some() {
                        continue;
                    }
                    let has_decision = by_run
                        .values()
                        .flatten()
                        .any(|r| r.decision_id == outcome.decision_id);
                    for row in by_run.values_mut().flatten().filter(|r| {
                        r.decision_id == outcome.decision_id
                            || (!has_decision
                                && outcome.run_id.as_deref() == Some(r.run_id.as_str()))
                    }) {
                        // Append-only correction: later events supersede earlier observations.
                        row.acceptance_passed = outcome.acceptance_passed;
                        row.review_passed = outcome.review_passed;
                        row.failed_criterion_ids = outcome.failed_criterion_ids.clone();
                        row.cash_usd = finite(outcome.cash_usd);
                        row.task_wall_ms = outcome.wall_ms;
                        row.retries = outcome.retries;
                    }
                }
                Event::RoutingShadowRecorded { record } => {
                    for row in by_run
                        .values_mut()
                        .flatten()
                        .filter(|r| r.decision_id == record.primary_decision_id)
                    {
                        row.shadows.push(ShadowV1 {
                            kind: wire_enum(&record.kind).unwrap_or_default(),
                            status: wire_enum(&record.status).unwrap_or_default(),
                            reason: record.reason.as_ref().and_then(wire_enum),
                            policy_version: record.policy_version.clone(),
                            model: record.candidate_model.clone(),
                            source: record.candidate_source.clone(),
                            cash_usd: finite(record.cash_usd),
                            effective_usd: finite(record.effective_usd),
                            latency_ms: record.latency_ms,
                        });
                    }
                }
                _ => {}
            }
        }
        for mut row in by_run.into_values().flatten() {
            row.request_ids.sort();
            row.request_ids.dedup();
            row.failed_criterion_ids.sort();
            row.failed_criterion_ids.dedup();
            row.shadows.sort_by(|a, b| {
                a.policy_version
                    .cmp(&b.policy_version)
                    .then_with(|| a.kind.cmp(&b.kind))
                    .then_with(|| a.model.cmp(&b.model))
            });
            // API latency comes from correlated primary request logs, never shadow or task wall time.
            if let Ok(mut latency_stmt) =
                conn.prepare("SELECT latency_ms FROM llm_proxy_requests WHERE id = ?1")
            {
                for request_id in &row.request_ids {
                    if let Some(ms) = latency_stmt
                        .query_row([request_id], |r| r.get::<_, i64>(0))
                        .optional()?
                        && let Ok(ms) = u64::try_from(ms)
                    {
                        row.api_latencies_ms.push(ms);
                    }
                }
            }
            row.effective_usd = row
                .candidates
                .iter()
                .find(|c| Some(&c.model) == row.primary_model.as_ref())
                .and_then(|c| c.effective_usd);
            rows.push(row);
        }
    }
    rows.sort_by(|a, b| {
        a.task_id
            .cmp(&b.task_id)
            .then_with(|| a.run_id.cmp(&b.run_id))
            .then_with(|| a.decision_id.cmp(&b.decision_id))
    });
    let n = rows.len();
    let missing_rate = [
        (
            "features",
            rows.iter().filter(|r| r.context_version.is_none()).count(),
        ),
        (
            "outcome",
            rows.iter()
                .filter(|r| r.acceptance_passed.is_none() && r.review_passed.is_none())
                .count(),
        ),
        ("cash", rows.iter().filter(|r| r.cash_usd.is_none()).count()),
        (
            "api_latency",
            rows.iter()
                .filter(|r| r.api_latencies_ms.is_empty())
                .count(),
        ),
        (
            "task_wall",
            rows.iter().filter(|r| r.task_wall_ms.is_none()).count(),
        ),
    ]
    .into_iter()
    .map(|(k, v)| {
        (
            k.into(),
            if n == 0 {
                0.0
            } else {
                finite(Some(v as f64 / n as f64)).unwrap_or(0.0)
            },
        )
    })
    .collect();
    Ok(DatasetV1 { manifest: ManifestV1 {
        schema: DATASET_SCHEMA.into(), policy_hash: options.policy_hash.clone(),
        catalog_hash: options.catalog_hash.clone(), estimator_hash: options.estimator_hash.clone(),
        from_utc: options.from_utc.clone(), until_utc: options.until_utc.clone(),
        extraction: "events by task_id, timestamp in [from_utc,until_utc)".into(),
        masking: "allowlisted audit metadata only; no prompt/response/credential/event JSON".into(),
        split: "SHA-256(seed big-endian || task_id), first 8 bytes modulo 10: 0-6 train, 7 calibration, 8-9 test".into(),
        seed: options.seed, rows: n, missing_rate,
    }, rows })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkModelV1 {
    pub quality: Option<f64>,
    pub cost_usd: Option<f64>,
}

/// External RouterBench-style figures remain a separate, labeled section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkBaselineV1 {
    pub name: String,
    pub models: BTreeMap<String, BenchmarkModelV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyReportV1 {
    pub decisions: usize,
    pub observed_outcomes: usize,
    pub acceptance_success_rate: Option<f64>,
    pub failed_criteria: BTreeMap<String, usize>,
    pub cash_usd_mean: Option<f64>,
    pub effective_usd_mean: Option<f64>,
    pub estimated_cash_usd_mean: Option<f64>,
    pub estimated_effective_usd_mean: Option<f64>,
    pub api_latency_p50_ms: Option<u64>,
    pub api_latency_p95_ms: Option<u64>,
    pub task_wall_p50_ms: Option<u64>,
    pub task_wall_p95_ms: Option<u64>,
    pub retry_rate: Option<f64>,
    pub escalation_rate: Option<f64>,
    pub quota_exhaustion_rate: Option<f64>,
    pub constraint_violations: usize,
    pub source_ratio: BTreeMap<String, f64>,
    pub unknown_rate: f64,
    pub timeout_rate: f64,
    pub drop_rate: f64,
    pub counterfactual_coverage: f64,
    pub unknown_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportV1 {
    pub schema: String,
    pub dataset_schema: String,
    pub policy_hash: String,
    pub catalog_hash: String,
    pub estimator_hash: String,
    pub seed: u64,
    pub policies: BTreeMap<String, PolicyReportV1>,
    pub paired_metrics: BTreeMap<String, String>,
    pub external_benchmark_baseline: Option<BenchmarkBaselineV1>,
}

/// Only choices are supplied; all quality and cost values come from observed dataset rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedMetricsInputV1 {
    pub task_ids: Vec<String>,
    pub weak_model: String,
    pub strong_model: String,
    pub points: Vec<PairedPointV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedPointV1 {
    /// One observed model per task, on the same paired task set.
    pub selected_model_by_task: BTreeMap<String, String>,
}

fn observed_model(dataset: &DatasetV1, task_id: &str, model: &str) -> Option<(f64, f64)> {
    let mut matches = dataset
        .rows
        .iter()
        .filter(|r| r.task_id == task_id && r.primary_model.as_deref() == Some(model));
    let row = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some((
        f64::from(row.acceptance_passed.or(row.review_passed)? as u8),
        row.cash_usd?,
    ))
}

impl ReportV1 {
    pub fn json(&self) -> Result<String, ReplayError> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

fn ratio(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        finite(Some(n as f64 / d as f64)).unwrap_or(0.0)
    }
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty())
        .then(|| finite(Some(values.iter().sum::<f64>() / values.len() as f64)))
        .flatten()
}

fn percentile(values: &[u64], p: usize) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort();
    Some(sorted[((sorted.len() * p).div_ceil(100)).saturating_sub(1)])
}

#[derive(Clone, Copy)]
enum PolicyChoice {
    Legacy,
    Heuristic,
    Shadow,
}

fn policy_report(rows: &[DatasetRowV1], choice: PolicyChoice) -> PolicyReportV1 {
    let mut observed = 0;
    let mut passed = 0;
    let mut unknown = 0;
    let mut timeouts = 0;
    let mut drops = 0;
    let mut retries = 0;
    let mut escalations = 0;
    let mut quotas = 0;
    let mut violations = 0;
    let mut sources = BTreeMap::<String, usize>::new();
    let mut failures = BTreeMap::<String, usize>::new();
    let mut cash = Vec::new();
    let mut effective = Vec::new();
    let mut estimated_cash = Vec::new();
    let mut estimated_effective = Vec::new();
    let mut api = Vec::new();
    let mut wall = Vec::new();
    for row in rows {
        let selected = match choice {
            PolicyChoice::Legacy => row
                .primary_model
                .as_deref()
                .map(|m| (m, row.primary_source.as_deref())),
            PolicyChoice::Heuristic => row
                .candidates
                .iter()
                .filter(|c| c.eligible && c.score.is_some())
                .max_by(|a, b| {
                    a.score
                        .unwrap_or(f64::NEG_INFINITY)
                        .total_cmp(&b.score.unwrap_or(f64::NEG_INFINITY))
                        .then_with(|| b.source.cmp(&a.source))
                })
                .map(|c| (c.model.as_str(), Some(c.source.as_str()))),
            PolicyChoice::Shadow => row
                .shadows
                .iter()
                .find(|s| s.kind == "decision" && s.status == "completed")
                .and_then(|s| s.model.as_deref().map(|m| (m, s.source.as_deref()))),
        };
        let known = selected.is_some_and(|(m, s)| {
            Some(m) == row.primary_model.as_deref()
                && s.is_none_or(|source| Some(source) == row.primary_source.as_deref())
        });
        if let Some((_, Some(source))) = selected {
            *sources.entry(source.to_owned()).or_default() += 1;
        }
        if let Some((model, source)) = selected
            && let Some(candidate) = row
                .candidates
                .iter()
                .find(|c| c.model == model && source.is_none_or(|s| c.source == s))
        {
            if let Some(v) = candidate.cash_usd {
                estimated_cash.push(v);
            }
            if let Some(v) = candidate.effective_usd {
                estimated_effective.push(v);
            }
        }
        if matches!(choice, PolicyChoice::Shadow) {
            if row
                .shadows
                .iter()
                .any(|s| s.reason.as_deref() == Some("timeout"))
            {
                timeouts += 1;
            }
            if row.shadows.iter().any(|s| s.status == "dropped") {
                drops += 1;
            }
        }
        if selected.is_some_and(|(m, s)| {
            row.candidates
                .iter()
                .any(|c| c.model == m && s.is_some_and(|source| c.source == source) && !c.eligible)
        }) {
            violations += 1;
        }
        if !known {
            unknown += 1;
            continue;
        }
        if let Some(success) = row.acceptance_passed.or(row.review_passed) {
            observed += 1;
            if success {
                passed += 1;
            } else {
                for id in &row.failed_criterion_ids {
                    *failures.entry(id.clone()).or_default() += 1;
                }
            }
        } else {
            unknown += 1;
        }
        if let Some(v) = row.cash_usd {
            cash.push(v);
        }
        if let Some(v) = row.effective_usd {
            effective.push(v);
        }
        api.extend_from_slice(&row.api_latencies_ms);
        if let Some(v) = row.task_wall_ms {
            wall.push(v);
        }
        if row.retries.is_some_and(|v| v > 0) {
            retries += 1;
        }
        if row.escalated {
            escalations += 1;
        }
        if row.quota_exhausted {
            quotas += 1;
        }
    }
    let n = rows.len();
    PolicyReportV1 {
        decisions: n, observed_outcomes: observed,
        acceptance_success_rate: (observed > 0).then(|| ratio(passed, observed)),
        failed_criteria: failures, cash_usd_mean: mean(&cash), effective_usd_mean: mean(&effective),
        estimated_cash_usd_mean: mean(&estimated_cash), estimated_effective_usd_mean: mean(&estimated_effective),
        api_latency_p50_ms: percentile(&api, 50), api_latency_p95_ms: percentile(&api, 95),
        task_wall_p50_ms: percentile(&wall, 50), task_wall_p95_ms: percentile(&wall, 95),
        retry_rate: (observed > 0).then(|| ratio(retries, observed)),
        escalation_rate: (observed > 0).then(|| ratio(escalations, observed)),
        quota_exhaustion_rate: (observed > 0).then(|| ratio(quotas, observed)),
        constraint_violations: violations,
        source_ratio: sources.into_iter().map(|(k,v)| (k, ratio(v,n))).collect(),
        unknown_rate: ratio(unknown,n), timeout_rate: ratio(timeouts,n), drop_rate: ratio(drops,n),
        counterfactual_coverage: ratio(observed,n),
        unknown_reason: "Unselected models have no observed task outcome; decision/execution shadow does not establish acceptance quality".into(),
    }
}

/// Re-evaluates candidate choices from the frozen trace. An unselected model has unknown quality.
/// APGR/AIQ/IBC require paired task outcomes and are not inferred from model-level benchmarks.
pub fn evaluate(
    dataset: &DatasetV1,
    baseline: Option<BenchmarkBaselineV1>,
) -> Result<ReportV1, ReplayError> {
    evaluate_with_paired(dataset, baseline, None)
}

pub fn evaluate_with_paired(
    dataset: &DatasetV1,
    baseline: Option<BenchmarkBaselineV1>,
    paired: Option<&PairedMetricsInputV1>,
) -> Result<ReportV1, ReplayError> {
    if dataset.manifest.schema != DATASET_SCHEMA
        || dataset.rows.iter().any(|r| {
            r.schema != DATASET_SCHEMA
                || r.split != split_for_task(&r.task_id, dataset.manifest.seed)
        })
    {
        return Err(ReplayError::Invalid("dataset schema or split mismatch"));
    }
    let mut policies = BTreeMap::new();
    policies.insert(
        "legacy".into(),
        policy_report(&dataset.rows, PolicyChoice::Legacy),
    );
    policies.insert(
        "heuristic".into(),
        policy_report(&dataset.rows, PolicyChoice::Heuristic),
    );
    policies.insert(
        "shadow_recorded".into(),
        policy_report(&dataset.rows, PolicyChoice::Shadow),
    );
    let mut paired_metrics: BTreeMap<String, String> = ["APGR", "AIQ", "IBC"]
        .into_iter()
        .map(|k| {
            (
                k.into(),
                "undefined: paired task outcomes for weak, strong and routed choices are missing"
                    .into(),
            )
        })
        .collect();
    if let Some(paired) = paired {
        let known: std::collections::BTreeSet<&str> =
            dataset.rows.iter().map(|r| r.task_id.as_str()).collect();
        let ids: std::collections::BTreeSet<&str> =
            paired.task_ids.iter().map(String::as_str).collect();
        if ids.is_empty()
            || ids.len() != paired.task_ids.len()
            || !ids.is_subset(&known)
            || paired.points.is_empty()
        {
            return Err(ReplayError::Invalid(
                "paired task ids or curve points invalid",
            ));
        }
        if paired.weak_model == paired.strong_model {
            return Err(ReplayError::Invalid("weak and strong model must differ"));
        }
        let mut weak = Vec::new();
        let mut strong = Vec::new();
        for task_id in &paired.task_ids {
            weak.push(observed_model(dataset, task_id, &paired.weak_model).ok_or(
                ReplayError::Invalid("weak paired outcome missing or ambiguous"),
            )?);
            strong.push(
                observed_model(dataset, task_id, &paired.strong_model).ok_or(
                    ReplayError::Invalid("strong paired outcome missing or ambiguous"),
                )?,
            );
        }
        let aggregate = |values: &[(f64, f64)]| {
            (
                values.iter().map(|v| v.0).sum::<f64>() / values.len() as f64,
                values.iter().map(|v| v.1).sum::<f64>() / values.len() as f64,
            )
        };
        let (weak_quality, weak_cost) = aggregate(&weak);
        let (strong_quality, strong_cost) = aggregate(&strong);
        let b = PairedBaseline {
            weak_quality,
            strong_quality,
            weak_cost,
            strong_cost,
            paired_outcomes: ids.len(),
        };
        let mut points = Vec::new();
        for p in &paired.points {
            if p.selected_model_by_task.len() != ids.len() {
                return Err(ReplayError::Invalid(
                    "routed point must select every paired task",
                ));
            }
            let mut outcomes = Vec::new();
            let mut strong_calls = 0;
            for task_id in &paired.task_ids {
                let model = p
                    .selected_model_by_task
                    .get(task_id)
                    .ok_or(ReplayError::Invalid("routed point task missing"))?;
                if model != &paired.weak_model && model != &paired.strong_model {
                    return Err(ReplayError::Invalid(
                        "routed point model is not paired baseline",
                    ));
                }
                if model == &paired.strong_model {
                    strong_calls += 1;
                }
                outcomes.push(observed_model(dataset, task_id, model).ok_or(
                    ReplayError::Invalid("routed paired outcome missing or ambiguous"),
                )?);
            }
            let (quality, cost) = aggregate(&outcomes);
            points.push(PairedCurvePoint {
                strong_call_fraction: ratio(strong_calls, ids.len()),
                quality,
                cost,
                paired_outcomes: ids.len(),
            });
        }
        let metric_text = |value: metrics::MetricValue| match value {
            metrics::MetricValue::Defined(v) => format!("{v:.6}"),
            metrics::MetricValue::Undefined { reason } => format!("undefined: {reason:?}"),
        };
        paired_metrics.insert("APGR".into(), metric_text(metrics::apgr(b, &points)));
        paired_metrics.insert("AIQ".into(), metric_text(metrics::aiq(b, &points)));
        if let Some(point) = points.last() {
            paired_metrics.insert("IBC".into(), metric_text(metrics::ibc(b, *point)));
        }
    }
    Ok(ReportV1 {
        schema: REPORT_SCHEMA.into(),
        dataset_schema: DATASET_SCHEMA.into(),
        policy_hash: dataset.manifest.policy_hash.clone(),
        catalog_hash: dataset.manifest.catalog_hash.clone(),
        estimator_hash: dataset.manifest.estimator_hash.clone(),
        seed: dataset.manifest.seed,
        policies,
        paired_metrics,
        external_benchmark_baseline: baseline,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::add::{NewTaskSpec, create_task};
    use task_core::model_policy::RoutingRecord;
    use task_core::model_router::feedback::RoutingOutcome;
    use task_core::model_router::policy::RoutingMode;
    use task_core::model_router::trace::CandidateTrace;
    use task_core::model_routing::LaneResolution;
    use task_core::{LaneCeiling, SqliteStore, Task, TaskStore, Tier};

    fn task(store: &SqliteStore) -> Task {
        let spec: NewTaskSpec = serde_json::from_value(serde_json::json!({
            "title":"secret title", "objective":"secret prompt response credential",
            "acceptance":[{"type":"command", "cmd":"echo secret"}]
        }))
        .unwrap();
        create_task(store, spec, time::OffsetDateTime::now_utc()).unwrap()
    }

    fn decided(task: &Task, run: &str) -> Event {
        let decision = task_core::decide_for_task(task, &LaneCeiling::default()).unwrap();
        let primary = CandidateTrace {
            model_profile_id: "model-a".into(),
            deployment_id: "source-a".into(),
            score: Some(0.3),
            cash_usd: Some(0.1),
            effective_usd: Some(0.2),
            ..CandidateTrace::default()
        };
        let alternative = CandidateTrace {
            model_profile_id: "model-b".into(),
            deployment_id: "source-b".into(),
            score: Some(0.9),
            cash_usd: Some(0.05),
            effective_usd: Some(0.08),
            ..CandidateTrace::default()
        };
        Event::RoutingDecided {
            run_id: run.into(),
            record: Box::new(RoutingRecord {
                org_node: None,
                harness: None,
                decision,
                resolution: LaneResolution {
                    model_id: "model-a".into(),
                    ..LaneResolution::default()
                },
                quota_reason: None,
                work_unit_id: None,
                escalation: None,
                optimizer: Some(RoutingTraceV1 {
                    decision_id: format!("d-{run}"),
                    parent_decision_id: None,
                    task_id: Some(task.id.to_string()),
                    work_unit_id: None,
                    run_id: Some(run.into()),
                    request_id: None,
                    stage: "dispatch".into(),
                    mode: RoutingMode::Shadow,
                    policy_version: "p1".into(),
                    catalog_version: "c1".into(),
                    feature_version: "1".into(),
                    estimator_version: "e1".into(),
                    snapshot_id: "s1".into(),
                    observed_at: None,
                    requested_lane: Tier::Standard,
                    selected_lane: Some(Tier::Standard),
                    candidates: vec![primary, alternative],
                    selected: Some("source-a".into()),
                    fallback_order: vec![],
                    reasons: vec![],
                    source_id: Some("source-a".into()),
                    model: Some("model-a".into()),
                    account_id: None,
                }),
            }),
        }
    }

    #[test]
    fn routing_offline_replay_is_deterministic_and_split_by_task() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fixture.sqlite");
        {
            let store = SqliteStore::open(&path).unwrap();
            let first = task(&store);
            let second = task(&store);
            for run in ["r1", "r2"] {
                store.append_event(first.id, &decided(&first, run)).unwrap();
                store
                    .append_event(
                        first.id,
                        &Event::RoutingOutcomeRecorded {
                            outcome: Box::new(RoutingOutcome {
                                outcome_id: format!("o-{run}"),
                                decision_id: format!("d-{run}"),
                                run_id: Some(run.into()),
                                request_id: None,
                                evaluation_version: "1".into(),
                                supersedes: None,
                                acceptance_passed: Some(true),
                                review_passed: Some(true),
                                failed_criterion_ids: vec![],
                                failure_class: None,
                                cash_usd: Some(0.1),
                                tokens: Some(10),
                                wall_ms: Some(500),
                                retries: Some(if run == "r2" { 1 } else { 0 }),
                                reward: None,
                            }),
                        },
                    )
                    .unwrap();
            }
            store
                .append_event(second.id, &decided(&second, "r3"))
                .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        let options = ExportOptions {
            policy_hash: "p-hash".into(),
            catalog_hash: "c-hash".into(),
            estimator_hash: "e-hash".into(),
            from_utc: None,
            until_utc: None,
            seed: 42,
        };
        let first = export(&path, &options).unwrap();
        let second = export(&path, &options).unwrap();
        assert_eq!(first.jsonl().unwrap(), second.jsonl().unwrap());
        assert_eq!(first.rows.len(), 3);
        let retry_rows: Vec<_> = first
            .rows
            .iter()
            .filter(|r| r.run_id == "r1" || r.run_id == "r2")
            .collect();
        assert_eq!(retry_rows.len(), 2);
        assert_eq!(retry_rows[0].task_id, retry_rows[1].task_id);
        assert_eq!(retry_rows[0].split, retry_rows[1].split);
        assert_eq!(
            retry_rows[0].split,
            split_for_task(&retry_rows[0].task_id, 42)
        );
        let report = evaluate(
            &first,
            Some(BenchmarkBaselineV1 {
                name: "external".into(),
                models: BTreeMap::from([(
                    "model-b".into(),
                    BenchmarkModelV1 {
                        quality: Some(0.8),
                        cost_usd: Some(0.2),
                    },
                )]),
            }),
        )
        .unwrap();
        assert_eq!(
            report.json().unwrap(),
            evaluate(&second, report.external_benchmark_baseline.clone())
                .unwrap()
                .json()
                .unwrap()
        );
        assert_eq!(report.policies["heuristic"].observed_outcomes, 0);
        assert_eq!(report.policies["heuristic"].unknown_rate, 1.0);
        assert_eq!(report.policies["heuristic"].cash_usd_mean, None);
        assert_eq!(
            report.policies["heuristic"].estimated_cash_usd_mean,
            Some(0.05)
        );
        assert_eq!(report.policies["legacy"].observed_outcomes, 2);
        assert!(report.external_benchmark_baseline.is_some());
        assert!(!first.jsonl().unwrap().contains("secret"));
        assert_eq!(before, std::fs::read(&path).unwrap());
    }

    #[test]
    fn routing_paired_metrics_require_observed_outcomes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("paired.sqlite");
        {
            let store = SqliteStore::open(&path).unwrap();
            let task = task(&store);
            store
                .append_event(task.id, &decided(&task, "template"))
                .unwrap();
        }
        let options = ExportOptions {
            policy_hash: "p".into(),
            catalog_hash: "c".into(),
            estimator_hash: "e".into(),
            from_utc: None,
            until_utc: None,
            seed: 4,
        };
        let mut dataset = export(&path, &options).unwrap();
        let template = dataset.rows.pop().unwrap();
        for task_id in ["A", "B"] {
            for (model, source, cost, passed) in [
                ("weak", "s-weak", 1.0, task_id == "B"),
                ("strong", "s-strong", 5.0, true),
            ] {
                let mut row = template.clone();
                row.task_id = task_id.into();
                row.split = split_for_task(task_id, 4);
                row.run_id = format!("{task_id}-{model}");
                row.decision_id = row.run_id.clone();
                row.primary_model = Some(model.into());
                row.primary_source = Some(source.into());
                row.acceptance_passed = Some(passed);
                row.cash_usd = Some(cost);
                dataset.rows.push(row);
            }
        }
        dataset.manifest.rows = dataset.rows.len();
        let input = PairedMetricsInputV1 {
            task_ids: vec!["A".into(), "B".into()],
            weak_model: "weak".into(),
            strong_model: "strong".into(),
            points: vec![PairedPointV1 {
                selected_model_by_task: BTreeMap::from([
                    ("A".into(), "strong".into()),
                    ("B".into(), "weak".into()),
                ]),
            }],
        };
        let report = evaluate_with_paired(&dataset, None, Some(&input)).unwrap();
        assert_ne!(
            report.paired_metrics["APGR"],
            "undefined: paired task outcomes for weak, strong and routed choices are missing"
        );
        assert_ne!(
            report.paired_metrics["AIQ"],
            "undefined: paired task outcomes for weak, strong and routed choices are missing"
        );
        assert_ne!(
            report.paired_metrics["IBC"],
            "undefined: paired task outcomes for weak, strong and routed choices are missing"
        );
        dataset.rows[0].acceptance_passed = None;
        assert!(evaluate_with_paired(&dataset, None, Some(&input)).is_err());
    }
}
