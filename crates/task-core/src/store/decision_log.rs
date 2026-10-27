//! 判断・実行・成果・消費の最小一貫ログの読み書き（ADR 2026-10-15-decision-log-export D1〜D7）。
//!
//! 書込みは主キー（`run_id` / `dedup_key` / `intervention_id`）の upsert で、最後の値で置換する。
//! 合算（`+=`）はしない。提案・実行・成果は欄の群が別で、1 つの群の upsert は他の群の列に触れない。
//! 結果と消費は実行（`exec_*`）の側にだけ付く。LLM 呼び出しは無い。
use rusqlite::{OptionalExtension, Row, params};
use time::OffsetDateTime;

use super::{SqliteStore, StoreError, format_rfc3339};
use crate::decision_log::{
    CampaignIdentity, CampaignVerdict, CandidateRef, ClassifierRecord, DecisionLogExecution,
    DecisionLogIntervention, DecisionLogOutcome, DecisionLogProposal, DecisionLogRun,
    DecisionLogUsage, ExclusionRef, InterventionActorKind, InterventionKind, RunEndLabel, UsageKey,
    UsageRole, WuVerdict,
};

fn json_opt<T: serde::Serialize>(v: &T, empty: bool) -> Result<Option<String>, StoreError> {
    if empty {
        Ok(None)
    } else {
        Ok(Some(serde_json::to_string(v)?))
    }
}

fn u(v: Option<u64>) -> Option<i64> {
    v.map(|n| n.min(i64::MAX as u64) as i64)
}

fn from_u(v: Option<i64>) -> Option<u64> {
    v.and_then(|n| u64::try_from(n).ok())
}

const RUN_COLUMNS: &str = "run_id, decision_id, campaign_id, arm, project_id, task_id, work_unit_id, \
    parent_run_id, attempt_no, request_class, routing_mode, classifier_kind, classifier_model, \
    classifier_snapshot, classifier_confidence, classifier_probs_json, propensity, proposed_lane, \
    decided_lane, proposed_candidates_json, proposed_first_source_id, proposed_first_model, \
    proposed_first_harness, exclusions_json, policy_version, prompt_version, question_schema_version, \
    provider_model_version, features_hash, options_hash, exec_source_id, exec_model, exec_account, \
    exec_harness, exec_adapter, fallback_reason, classifier_fallback, attempts_json, run_end, \
    run_end_raw, run_retryable, wu_verdict, wu_failure_class, campaign_verdict, wall_ms, \
    case_id, repetition, manifest_hash, routing_setting, classifier_status, classifier_call_id, \
    fixed_lane, jev_lane, constrained_lane, lane_basis, \
    variant_id, variant_content_sha256";

fn parse_json<T: serde::de::DeserializeOwned>(s: Option<String>) -> Result<Option<T>, StoreError> {
    match s {
        Some(s) => Ok(Some(serde_json::from_str(&s)?)),
        None => Ok(None),
    }
}

fn read_run(row: &Row<'_>) -> Result<DecisionLogRun, StoreError> {
    let run_id: String = row.get(0)?;
    let kind: Option<String> = row.get(11)?;
    let classifier = match kind {
        Some(kind) => Some(ClassifierRecord {
            kind,
            model: row.get(12)?,
            snapshot: row.get(13)?,
            confidence: row.get(14)?,
            probs: parse_json(row.get(15)?)?,
            fallback: row.get(36)?,
        }),
        None => None,
    };
    let first_source: Option<String> = row.get(20)?;
    let first_model: Option<String> = row.get(21)?;
    let first_harness: Option<String> = row.get(22)?;
    let proposed_first = match (first_source, first_model) {
        (Some(source_id), Some(model_id)) => Some(CandidateRef {
            source_id,
            model_id,
            harness_id: first_harness.unwrap_or_default(),
        }),
        _ => None,
    };
    let proposed_candidates: Vec<CandidateRef> = parse_json(row.get(19)?)?.unwrap_or_default();
    let exclusions: Vec<ExclusionRef> = parse_json(row.get(23)?)?.unwrap_or_default();
    let attempt_no: Option<i64> = row.get(8)?;
    let run_end: Option<String> = row.get(38)?;
    let retryable: Option<i64> = row.get(40)?;
    let wu: Option<String> = row.get(41)?;
    let cv: Option<String> = row.get(43)?;
    Ok(DecisionLogRun {
        proposal: DecisionLogProposal {
            run_id: run_id.clone(),
            decision_id: row.get(1)?,
            campaign_id: row.get(2)?,
            arm: row.get(3)?,
            case_id: row.get(45)?,
            repetition: row
                .get::<_, Option<i64>>(46)?
                .and_then(|n| u32::try_from(n).ok()),
            manifest_hash: row.get(47)?,
            routing_setting: row.get(48)?,
            variant_id: row.get(55)?,
            variant_content_sha256: row.get(56)?,
            project_id: row.get(4)?,
            task_id: row.get(5)?,
            work_unit_id: row.get(6)?,
            parent_run_id: row.get(7)?,
            attempt_no: attempt_no.and_then(|n| u32::try_from(n).ok()),
            request_class: row.get(9)?,
            routing_mode: row.get(10)?,
            classifier,
            classifier_status: row.get(49)?,
            classifier_call_id: row.get(50)?,
            fixed_lane: row.get(51)?,
            jev_lane: row.get(52)?,
            constrained_lane: row.get(53)?,
            lane_basis: row.get(54)?,
            propensity: row.get(16)?,
            proposed_lane: row.get(17)?,
            decided_lane: row.get(18)?,
            proposed_candidates,
            proposed_first,
            exclusions,
            policy_version: row.get(24)?,
            prompt_version: row.get(25)?,
            question_schema_version: row.get(26)?,
            provider_model_version: row.get(27)?,
            features_hash: row.get(28)?,
            options_hash: row.get(29)?,
        },
        execution: DecisionLogExecution {
            run_id: run_id.clone(),
            exec_source_id: row.get(30)?,
            exec_model: row.get(31)?,
            exec_account: row.get(32)?,
            exec_harness: row.get(33)?,
            exec_adapter: row.get(34)?,
            fallback_reason: row.get(35)?,
            attempts: parse_json(row.get(37)?)?,
        },
        outcome: DecisionLogOutcome {
            run_id,
            run_end: run_end.as_deref().and_then(RunEndLabel::parse),
            run_end_raw: row.get(39)?,
            run_retryable: retryable.map(|n| n != 0),
            wu_verdict: wu.as_deref().and_then(WuVerdict::parse),
            wu_failure_class: row.get(42)?,
            campaign_verdict: cv.as_deref().and_then(CampaignVerdict::parse),
            wall_ms: from_u(row.get(44)?),
        },
    })
}

impl SqliteStore {
    /// 提案の群を書く（同じ `run_id` の 2 回目は置換。実行・成果の列は変えない）。
    pub fn decision_log_record_proposal(
        &self,
        p: &DecisionLogProposal,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        if p.run_id.is_empty() {
            return Err(StoreError::Invalid("decision log run_id is empty".into()));
        }
        if let Some(c) = p.classifier.as_ref().and_then(|c| c.confidence)
            && !(0.0..=1.0).contains(&c)
        {
            return Err(StoreError::Invalid(
                "classifier_confidence must be within 0..=1".into(),
            ));
        }
        let c = p.classifier.as_ref();
        let first = p.proposed_first.as_ref();
        let candidates = json_opt(&p.proposed_candidates, p.proposed_candidates.is_empty())?;
        let exclusions = json_opt(&p.exclusions, p.exclusions.is_empty())?;
        let probs = match c.and_then(|c| c.probs.as_ref()) {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO decision_log_runs (run_id, decision_id, campaign_id, arm, project_id, task_id, \
             work_unit_id, parent_run_id, attempt_no, request_class, routing_mode, classifier_kind, \
             classifier_model, classifier_snapshot, classifier_confidence, classifier_probs_json, \
             classifier_fallback, propensity, proposed_lane, decided_lane, proposed_candidates_json, \
             proposed_first_source_id, proposed_first_model, proposed_first_harness, exclusions_json, \
             policy_version, prompt_version, question_schema_version, provider_model_version, \
             features_hash, options_hash, updated_at, case_id, repetition, manifest_hash, routing_setting, \
             classifier_status, classifier_call_id, fixed_lane, jev_lane, constrained_lane, lane_basis, \
             variant_id, variant_content_sha256) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43,?44) \
             ON CONFLICT(run_id) DO UPDATE SET decision_id=excluded.decision_id, campaign_id=excluded.campaign_id, \
             arm=excluded.arm, project_id=excluded.project_id, task_id=excluded.task_id, \
             work_unit_id=excluded.work_unit_id, parent_run_id=excluded.parent_run_id, \
             attempt_no=excluded.attempt_no, request_class=excluded.request_class, \
             routing_mode=excluded.routing_mode, classifier_kind=excluded.classifier_kind, \
             classifier_model=excluded.classifier_model, classifier_snapshot=excluded.classifier_snapshot, \
             classifier_confidence=excluded.classifier_confidence, classifier_probs_json=excluded.classifier_probs_json, \
             classifier_fallback=excluded.classifier_fallback, propensity=excluded.propensity, \
             proposed_lane=excluded.proposed_lane, decided_lane=excluded.decided_lane, \
             proposed_candidates_json=excluded.proposed_candidates_json, \
             proposed_first_source_id=excluded.proposed_first_source_id, \
             proposed_first_model=excluded.proposed_first_model, proposed_first_harness=excluded.proposed_first_harness, \
             exclusions_json=excluded.exclusions_json, policy_version=excluded.policy_version, \
             prompt_version=excluded.prompt_version, question_schema_version=excluded.question_schema_version, \
             provider_model_version=excluded.provider_model_version, features_hash=excluded.features_hash, \
             options_hash=excluded.options_hash, updated_at=excluded.updated_at, \
             case_id=excluded.case_id, repetition=excluded.repetition, manifest_hash=excluded.manifest_hash, \
             routing_setting=excluded.routing_setting, classifier_status=excluded.classifier_status, \
             classifier_call_id=excluded.classifier_call_id, fixed_lane=excluded.fixed_lane, \
             jev_lane=excluded.jev_lane, constrained_lane=excluded.constrained_lane, \
             lane_basis=excluded.lane_basis, variant_id=excluded.variant_id, \
             variant_content_sha256=excluded.variant_content_sha256",
            params![
                p.run_id,
                p.decision_id,
                p.campaign_id,
                p.arm,
                p.project_id,
                p.task_id,
                p.work_unit_id,
                p.parent_run_id,
                p.attempt_no.map(i64::from),
                p.request_class,
                p.routing_mode,
                c.map(|c| c.kind.clone()),
                c.and_then(|c| c.model.clone()),
                c.and_then(|c| c.snapshot.clone()),
                c.and_then(|c| c.confidence),
                probs,
                c.and_then(|c| c.fallback.clone()),
                p.propensity,
                p.proposed_lane,
                p.decided_lane,
                candidates,
                first.map(|f| f.source_id.clone()),
                first.map(|f| f.model_id.clone()),
                first.map(|f| f.harness_id.clone()),
                exclusions,
                p.policy_version,
                p.prompt_version,
                p.question_schema_version,
                p.provider_model_version,
                p.features_hash,
                p.options_hash,
                format_rfc3339(now)?,
                p.case_id,
                p.repetition.map(i64::from),
                p.manifest_hash,
                p.routing_setting,
                p.classifier_status,
                p.classifier_call_id,
                p.fixed_lane,
                p.jev_lane,
                p.constrained_lane,
                p.lane_basis,
                p.variant_id,
                p.variant_content_sha256,
            ],
        )?;
        Ok(())
    }

    /// 実行の群を書く（提案・成果の列は変えない）。
    pub fn decision_log_record_execution(
        &self,
        e: &DecisionLogExecution,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        if e.run_id.is_empty() {
            return Err(StoreError::Invalid("decision log run_id is empty".into()));
        }
        let attempts = match &e.attempts {
            Some(v) => Some(serde_json::to_string(v)?),
            None => None,
        };
        self.lock()?.execute(
            "INSERT INTO decision_log_runs (run_id, exec_source_id, exec_model, exec_account, exec_harness, \
             exec_adapter, fallback_reason, attempts_json, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) \
             ON CONFLICT(run_id) DO UPDATE SET exec_source_id=excluded.exec_source_id, exec_model=excluded.exec_model, \
             exec_account=excluded.exec_account, exec_harness=excluded.exec_harness, exec_adapter=excluded.exec_adapter, \
             fallback_reason=excluded.fallback_reason, attempts_json=excluded.attempts_json, updated_at=excluded.updated_at",
            params![
                e.run_id,
                e.exec_source_id,
                e.exec_model,
                e.exec_account,
                e.exec_harness,
                e.exec_adapter,
                e.fallback_reason,
                attempts,
                format_rfc3339(now)?,
            ],
        )?;
        Ok(())
    }

    /// 成果の群を書く（提案・実行の列は変えない）。run 終了・WU 検証・campaign 合否は別の欄。
    pub fn decision_log_record_outcome(
        &self,
        o: &DecisionLogOutcome,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        if o.run_id.is_empty() {
            return Err(StoreError::Invalid("decision log run_id is empty".into()));
        }
        self.lock()?.execute(
            "INSERT INTO decision_log_runs (run_id, run_end, run_end_raw, run_retryable, wu_verdict, \
             wu_failure_class, campaign_verdict, wall_ms, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) \
             ON CONFLICT(run_id) DO UPDATE SET run_end=excluded.run_end, run_end_raw=excluded.run_end_raw, \
             run_retryable=excluded.run_retryable, wu_verdict=excluded.wu_verdict, \
             wu_failure_class=excluded.wu_failure_class, campaign_verdict=excluded.campaign_verdict, \
             wall_ms=excluded.wall_ms, updated_at=excluded.updated_at",
            params![
                o.run_id,
                o.run_end.map(RunEndLabel::as_str),
                o.run_end_raw,
                o.run_retryable.map(i64::from),
                o.wu_verdict.map(WuVerdict::as_str),
                o.wu_failure_class,
                o.campaign_verdict.map(CampaignVerdict::as_str),
                u(o.wall_ms),
                format_rfc3339(now)?,
            ],
        )?;
        Ok(())
    }

    pub fn decision_log_run_get(&self, run_id: &str) -> Result<Option<DecisionLogRun>, StoreError> {
        let conn = self.lock()?;
        let sql = format!("SELECT {RUN_COLUMNS} FROM decision_log_runs WHERE run_id = ?1");
        let row = conn
            .query_row(&sql, [run_id], |r| Ok(read_run(r)))
            .optional()?;
        row.transpose()
    }

    /// campaign の run 全部（`None` なら campaign 外を含む全部）。`run_id` 順。
    pub fn decision_log_runs_list(
        &self,
        campaign_id: Option<&str>,
    ) -> Result<Vec<DecisionLogRun>, StoreError> {
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM decision_log_runs \
             WHERE (?1 IS NULL OR campaign_id = ?1) ORDER BY run_id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([campaign_id], |r| Ok(read_run(r)))?;
        rows.map(|r| r?).collect()
    }

    /// 実行した候補（`exec_*`）に帰属する run。提案先頭（`proposed_first_*`）では引かない。
    pub fn decision_log_runs_by_exec(
        &self,
        source_id: &str,
        model: &str,
    ) -> Result<Vec<DecisionLogRun>, StoreError> {
        let conn = self.lock()?;
        let sql = format!(
            "SELECT {RUN_COLUMNS} FROM decision_log_runs \
             WHERE exec_source_id = ?1 AND exec_model = ?2 ORDER BY run_id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([source_id, model], |r| Ok(read_run(r)))?;
        rows.map(|r| r?).collect()
    }

    /// 消費 1 行を書く（`dedup_key` の置換。合算しない）。
    pub fn decision_log_record_usage(
        &self,
        usage: &DecisionLogUsage,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        let (run_id, decision_id) = match &usage.key {
            UsageKey::Run { run_id, .. } => (Some(run_id.clone()), usage.decision_id.clone()),
            UsageKey::Call { decision_id, .. } => (usage.run_id.clone(), Some(decision_id.clone())),
        };
        self.lock()?.execute(
            "INSERT INTO decision_log_usage (dedup_key, run_id, decision_id, campaign_id, task_id, role, is_retry, \
             input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, list_cost_usd, \
             list_cost_usd_estimated, cash_cost_usd, wall_ms, cluster_wait_ms, cluster_run_ms, \
             cluster_node_hours, cluster_queue_ms, updated_at, attribution_rule, shared_overhead) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,1,?13,?14,?15,?16,?17,?18,?19,?20,?21) \
             ON CONFLICT(dedup_key) DO UPDATE SET run_id=excluded.run_id, decision_id=excluded.decision_id, \
             campaign_id=excluded.campaign_id, task_id=excluded.task_id, role=excluded.role, is_retry=excluded.is_retry, \
             input_tokens=excluded.input_tokens, output_tokens=excluded.output_tokens, \
             cache_read_tokens=excluded.cache_read_tokens, cache_creation_tokens=excluded.cache_creation_tokens, \
             list_cost_usd=COALESCE(excluded.list_cost_usd, list_cost_usd), \
             cash_cost_usd=COALESCE(excluded.cash_cost_usd, cash_cost_usd), wall_ms=excluded.wall_ms, \
             cluster_wait_ms=excluded.cluster_wait_ms, cluster_run_ms=excluded.cluster_run_ms, \
             cluster_node_hours=excluded.cluster_node_hours, cluster_queue_ms=excluded.cluster_queue_ms, \
             updated_at=excluded.updated_at, attribution_rule=excluded.attribution_rule, \
             shared_overhead=excluded.shared_overhead",
            params![
                usage.key.dedup_key(),
                run_id,
                decision_id,
                usage.campaign_id,
                usage.task_id,
                usage.role.as_str(),
                i64::from(usage.is_retry),
                u(usage.input_tokens),
                u(usage.output_tokens),
                u(usage.cache_read_tokens),
                u(usage.cache_creation_tokens),
                usage.list_cost_usd,
                usage.cash_cost_usd,
                u(usage.wall_ms),
                u(usage.cluster_wait_ms),
                u(usage.cluster_run_ms),
                usage.cluster_node_hours,
                u(usage.cluster_queue_ms),
                format_rfc3339(now)?,
                usage.attribution_rule,
                i64::from(usage.shared_overhead),
            ],
        )?;
        Ok(())
    }

    /// campaign の消費行（`None` なら全部）。`dedup_key` 順。
    pub fn decision_log_usage_list(
        &self,
        campaign_id: Option<&str>,
    ) -> Result<Vec<DecisionLogUsage>, StoreError> {
        self.read_decision_log_usage(campaign_id, false)
    }

    fn read_decision_log_usage(
        &self,
        campaign_id: Option<&str>,
        include_unassigned_shared: bool,
    ) -> Result<Vec<DecisionLogUsage>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT d.dedup_key, COALESCE(d.run_id, c.run_id), d.decision_id, d.campaign_id, d.task_id, \
             d.role, d.is_retry, d.input_tokens, d.output_tokens, d.cache_read_tokens, \
             d.cache_creation_tokens, d.list_cost_usd, d.cash_cost_usd, d.wall_ms, d.cluster_wait_ms, \
             d.cluster_run_ms, d.cluster_node_hours, d.cluster_queue_ms, d.attribution_rule, d.shared_overhead, \
             c.thread_id, t.kind, t.project_id, \
             COALESCE(json_extract(CASE WHEN json_valid(c.resolved_config_json) \
                 THEN c.resolved_config_json ELSE '{}' END, '$.model'), n.model), \
             NULLIF(n.session_id, '') \
             FROM decision_log_usage d \
             LEFT JOIN chat_runs c ON d.role='cos' AND c.run_id=substr(d.decision_id,5) \
             AND d.dedup_key='decision:cos:' || c.run_id || ':1' \
             LEFT JOIN chat_threads t ON t.id=c.thread_id \
             LEFT JOIN node_sessions n ON n.id=c.session_row_id \
             WHERE (?1 IS NULL OR d.campaign_id = ?1 \
                 OR (?2 AND d.campaign_id IS NULL AND d.shared_overhead=1)) ORDER BY d.dedup_key",
        )?;
        let rows = stmt.query_map(params![campaign_id, include_unassigned_shared], |r| {
            let dedup: String = r.get(0)?;
            let run_id: Option<String> = r.get(1)?;
            let decision_id: Option<String> = r.get(2)?;
            let role: String = r.get(5)?;
            Ok((
                dedup,
                run_id,
                decision_id,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                role,
                r.get::<_, i64>(6)?,
                [
                    r.get::<_, Option<i64>>(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                ],
                [r.get::<_, Option<f64>>(11)?, r.get(12)?],
                [
                    r.get::<_, Option<i64>>(13)?,
                    r.get(14)?,
                    r.get(15)?,
                    r.get(17)?,
                ],
                r.get::<_, Option<f64>>(16)?,
                (r.get::<_, Option<String>>(18)?, r.get::<_, i64>(19)?),
                [
                    r.get::<_, Option<String>>(20)?,
                    r.get(21)?,
                    r.get(22)?,
                    r.get(23)?,
                    r.get(24)?,
                ],
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                dedup,
                run_id,
                decision_id,
                campaign_id,
                task_id,
                role,
                is_retry,
                tok,
                cost,
                ms,
                nh,
                (attribution_rule, shared_overhead),
                [thread_id, thread_kind, project_id, model, session],
            ) = row?;
            let role = UsageRole::parse(&role)
                .ok_or_else(|| StoreError::Invalid(format!("unknown usage role: {role}")))?;
            let key = match (dedup.strip_prefix("run:"), dedup.strip_prefix("decision:")) {
                (Some(rest), _) => {
                    let run = run_id.clone().unwrap_or_default();
                    let source = rest
                        .strip_prefix(&format!("{run}:"))
                        .unwrap_or(rest)
                        .to_string();
                    UsageKey::Run {
                        run_id: run,
                        usage_source: source,
                    }
                }
                (_, Some(rest)) => {
                    let attempt = rest
                        .rsplit(':')
                        .next()
                        .and_then(|a| a.parse::<u32>().ok())
                        .ok_or_else(|| StoreError::Invalid(format!("bad dedup_key: {dedup}")))?;
                    UsageKey::Call {
                        decision_id: decision_id.clone().unwrap_or_default(),
                        call_attempt: attempt,
                    }
                }
                _ => return Err(StoreError::Invalid(format!("bad dedup_key: {dedup}"))),
            };
            out.push(DecisionLogUsage {
                key,
                role,
                thread_id,
                thread_kind,
                project_id,
                model,
                session,
                is_retry: is_retry != 0,
                decision_id,
                run_id,
                campaign_id,
                task_id,
                input_tokens: from_u(tok[0]),
                output_tokens: from_u(tok[1]),
                cache_read_tokens: from_u(tok[2]),
                cache_creation_tokens: from_u(tok[3]),
                list_cost_usd: cost[0],
                cash_cost_usd: cost[1],
                wall_ms: from_u(ms[0]),
                cluster_wait_ms: from_u(ms[1]),
                cluster_run_ms: from_u(ms[2]),
                cluster_node_hours: nh,
                cluster_queue_ms: from_u(ms[3]),
                attribution_rule,
                shared_overhead: shared_overhead != 0,
            });
        }
        Ok(out)
    }

    /// 介入 1 件を書く（`intervention_id` の置換。二重に数えない）。
    pub fn decision_log_record_intervention(
        &self,
        i: &DecisionLogIntervention,
    ) -> Result<(), StoreError> {
        self.lock()?.execute(
            "INSERT INTO decision_log_interventions (intervention_id, kind, actor_kind, actor_id, task_id, campaign_id, at, wall_ms, source_event) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(intervention_id) DO UPDATE SET kind=excluded.kind, \
             actor_kind=excluded.actor_kind, actor_id=excluded.actor_id, \
             task_id=excluded.task_id, campaign_id=excluded.campaign_id, at=excluded.at, wall_ms=excluded.wall_ms, \
             source_event=excluded.source_event",
            params![i.intervention_id, i.kind.as_str(), i.actor_kind.as_str(), i.actor_id, i.task_id, i.campaign_id, i.at, u(i.wall_ms), i.source_event],
        )?;
        Ok(())
    }

    pub fn decision_log_interventions_list(
        &self,
        campaign_id: Option<&str>,
    ) -> Result<Vec<DecisionLogIntervention>, StoreError> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT intervention_id, kind, task_id, campaign_id, at, wall_ms, source_event, actor_kind, actor_id \
             FROM decision_log_interventions WHERE (?1 IS NULL OR campaign_id = ?1) ORDER BY intervention_id",
        )?;
        let rows = stmt.query_map([campaign_id], |r| {
            Ok((
                (
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ),
                (r.get::<_, String>(7)?, r.get::<_, Option<String>>(8)?),
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (
                (intervention_id, kind, task_id, campaign_id, at, wall_ms, source_event),
                (actor_kind, actor_id),
            ) = row?;
            out.push(DecisionLogIntervention {
                intervention_id,
                kind: InterventionKind::parse(&kind).ok_or_else(|| {
                    StoreError::Invalid(format!("unknown intervention kind: {kind}"))
                })?,
                actor_kind: InterventionActorKind::parse(&actor_kind).ok_or_else(|| {
                    StoreError::Invalid(format!("unknown intervention actor kind: {actor_kind}"))
                })?,
                actor_id,
                task_id,
                campaign_id,
                at,
                wall_ms: from_u(wall_ms),
                source_event,
            });
        }
        Ok(out)
    }
}

impl SqliteStore {
    /// campaign の最後の活動時刻を進める（`MAX` で畳むので同じ入力の再処理・逆順でも変わらない）。
    pub fn decision_log_campaign_touch(
        &self,
        campaign_id: &str,
        at: OffsetDateTime,
    ) -> Result<(), StoreError> {
        let at = format_rfc3339(at)?;
        self.lock()?.execute(
            "INSERT INTO decision_log_campaign_activity (campaign_id, last_activity_at) VALUES (?1, ?2) \
             ON CONFLICT(campaign_id) DO UPDATE SET last_activity_at = MAX(last_activity_at, excluded.last_activity_at)",
            params![campaign_id, at],
        )?;
        Ok(())
    }

    /// campaign の計測。直接帰属と共有オーバーヘッドは別、elapsed（壁時計）と run 時間の総和も別欄。
    pub fn decision_log_campaign_metering(
        &self,
        campaign_id: &str,
    ) -> Result<crate::decision_log::CampaignMetering, StoreError> {
        let usage = self.decision_log_usage_list(Some(campaign_id))?;
        let mut m = crate::decision_log::CampaignMetering::from_usage(campaign_id, &usage);
        let conn = self.lock()?;
        let started: Option<String> = conn.query_row(
            "SELECT MIN(created_at) FROM task_campaigns WHERE campaign_id = ?1",
            [campaign_id],
            |r| r.get(0),
        )?;
        let last: Option<String> = conn
            .query_row(
                "SELECT last_activity_at FROM decision_log_campaign_activity WHERE campaign_id = ?1",
                [campaign_id],
                |r| r.get(0),
            )
            .optional()?;
        if let (Some(a), Some(b)) = (started, last) {
            let parse = |s: &str| {
                OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                    .map_err(|e| StoreError::Invalid(format!("bad timestamp {s}: {e}")))
            };
            let ms = (parse(&b)? - parse(&a)?).whole_milliseconds();
            m.elapsed_ms = u64::try_from(ms.max(0)).ok();
        }
        Ok(m)
    }

    /// campaign 単位の export（records + summary）。読み出しのみ。
    pub fn decision_log_export_campaign(
        &self,
        params: &crate::decision_log_export::ExportParams,
    ) -> Result<crate::decision_log_export::CampaignExport, StoreError> {
        let c = Some(params.campaign_id.as_str());
        let runs = self.decision_log_runs_list(c)?;
        let usage = self.read_decision_log_usage(c, true)?;
        let ivs = self.decision_log_interventions_list(c)?;
        let mut export =
            crate::decision_log_export::build_campaign_export(params, &runs, &usage, &ivs);
        export.summary.elapsed_ms = self
            .decision_log_campaign_metering(&params.campaign_id)?
            .elapsed_ms;
        Ok(export)
    }
}

// ---- campaign 識別（ADR 2026-10-16-campaign-identity-propagation）----

/// 祖先を辿る上限（task 木の最大深さ 3 より十分大きい。循環の保険）。
pub(crate) const CAMPAIGN_ANCESTOR_LIMIT: usize = 32;

/// `task_campaign_resend` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CampaignResend {
    New,
    Same(String),
    Conflict(String),
}

/// `task_campaigns` の読み出し列（`own_row` の順）。
const CAMPAIGN_COLUMNS: &str = "campaign_id, case_id, arm, repetition, manifest_hash, routing_setting, variant_json, variant_org_json";

/// 保存値を識別へ戻す。壊れた JSON・片欠け・形の不正は `Invalid`（黙って variant 無しにしない）。
fn campaign_from_row(task_id: &str, r: &Row<'_>) -> Result<CampaignIdentity, StoreError> {
    let bad = |what: String| {
        StoreError::Invalid(format!(
            "task {task_id}: stored campaign identity is invalid: {what}"
        ))
    };
    let repetition = match r.get::<_, Option<i64>>(3)? {
        Some(n) => Some(u32::try_from(n).map_err(|_| bad(format!("repetition {n}")))?),
        None => None,
    };
    let variant_json: Option<String> = r.get(6)?;
    let variant_org_json: Option<String> = r.get(7)?;
    let id = CampaignIdentity {
        campaign_id: r.get(0)?,
        case_id: r.get(1)?,
        arm: r.get(2)?,
        repetition,
        manifest_hash: r.get(4)?,
        routing_setting: r.get(5)?,
        variant: variant_json
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| bad(format!("variant_json: {e}")))?,
        variant_org: variant_org_json
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| bad(format!("variant_org_json: {e}")))?,
    };
    id.validate().map_err(bad)?;
    Ok(id)
}

fn campaign_mismatch(task_id: &str, other: &str) -> StoreError {
    StoreError::Invalid(format!(
        "campaign identity of task {task_id} differs from task {other} in its ancestor chain"
    ))
}

impl SqliteStore {
    /// campaign の root task に識別を付ける（同じ識別の再設定は何もしない）。
    pub fn task_campaign_set(
        &self,
        task_id: &str,
        id: &CampaignIdentity,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        Self::task_campaign_set_tx(&tx, task_id, id, now)?;
        tx.commit()?;
        Ok(())
    }

    /// 呼び出し側の transaction の中で付ける（task 作成と同じ transaction に入れるため）。
    /// 既に違う識別を持つ task・祖先や子孫の識別と食い違う task には付けない（`Invalid`。
    /// 呼び出し側の transaction ごと捨てれば task・識別のどちらも残らない）。
    pub fn task_campaign_set_tx(
        conn: &rusqlite::Connection,
        task_id: &str,
        id: &CampaignIdentity,
        now: OffsetDateTime,
    ) -> Result<(), StoreError> {
        id.validate().map_err(StoreError::Invalid)?;
        if let Some(own) = Self::task_campaign_own_tx(conn, task_id)? {
            if &own == id {
                return Ok(());
            }
            return Err(StoreError::Invalid(format!(
                "task {task_id} already has a different campaign identity"
            )));
        }
        // 祖先（root まで全部）が持つ識別と 1 欄でも違えば拒否。
        let parent: Option<String> = conn
            .query_row(
                "SELECT parent_id FROM tasks WHERE id = ?1",
                [task_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let Some(parent) = parent
            && let Some((holder, inherited)) = Self::task_campaign_resolve_holder_tx(conn, &parent)?
            && &inherited != id
        {
            return Err(campaign_mismatch(task_id, &holder));
        }
        // 子孫が既に持つ識別とも食い違わせない（root と子孫の不一致を作らない）。
        let descendants: Vec<String> = {
            let mut stmt = conn.prepare(
                "WITH RECURSIVE d(id, depth) AS ( \
                   SELECT id, 1 FROM tasks WHERE parent_id = ?1 \
                   UNION SELECT t.id, d.depth + 1 FROM tasks t JOIN d ON t.parent_id = d.id \
                   WHERE d.depth < ?2) \
                 SELECT c.task_id FROM d JOIN task_campaigns c ON c.task_id = d.id ORDER BY c.task_id",
            )?;
            stmt.query_map(params![task_id, CAMPAIGN_ANCESTOR_LIMIT as i64], |r| {
                r.get(0)
            })?
            .collect::<Result<_, _>>()?
        };
        for d in descendants {
            if Self::task_campaign_own_tx(conn, &d)?.as_ref() != Some(id) {
                return Err(campaign_mismatch(&d, task_id));
            }
        }
        let variant = id.variant.as_ref().map(serde_json::to_string).transpose()?;
        let variant_org = id
            .variant_org
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        conn.execute(
            "INSERT INTO task_campaigns (task_id, campaign_id, case_id, arm, repetition, manifest_hash, \
             routing_setting, variant_json, variant_org_json, created_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                task_id,
                id.campaign_id,
                id.case_id,
                id.arm,
                id.repetition.map(i64::from),
                id.manifest_hash,
                id.routing_setting,
                variant,
                variant_org,
                format_rfc3339(now)?,
            ],
        )?;
        Ok(())
    }

    /// 再送の判定。同じ試行（campaign / case / arm / repetition）の task が既にあれば、識別が全欄一致
    /// （variant と構成 pin を含む）なら `Same`、違えば `Conflict`（variant の変更・脱落・追加も別の試行として
    /// 黙って上書きしない）。冪等キーの照合は `task_idempotency_keys`（ADR 2026-10-27-task-idempotency-keys）。
    pub fn task_campaign_resend_tx(
        conn: &rusqlite::Connection,
        id: &CampaignIdentity,
    ) -> Result<CampaignResend, StoreError> {
        let Some(task_id) = Self::task_campaign_by_trial_tx(conn, id)? else {
            return Ok(CampaignResend::New);
        };
        Ok(match Self::task_campaign_own_tx(conn, &task_id)? {
            Some(own) if &own == id => CampaignResend::Same(task_id),
            _ => CampaignResend::Conflict(task_id),
        })
    }

    /// `task_campaign_resend_tx` を、variant の構成 pin の org 照合と同じ lock で行う。
    /// `Same` でも org が pin と食い違っていれば `Err(Invalid)`（drift を成功扱いしない）。
    pub fn task_campaign_resend_org_checked(
        &self,
        id: &CampaignIdentity,
    ) -> Result<CampaignResend, StoreError> {
        let conn = self.lock()?;
        let out = Self::task_campaign_resend_tx(&conn, id)?;
        if matches!(out, CampaignResend::Same(_))
            && let Some(pin) = &id.variant_org
        {
            Self::campaign_org_check_tx(&conn, pin)?;
        }
        Ok(out)
    }

    pub fn task_campaign_resend(
        &self,
        id: &CampaignIdentity,
    ) -> Result<CampaignResend, StoreError> {
        let conn = self.lock()?;
        Self::task_campaign_resend_tx(&conn, id)
    }

    /// 同じ試行（campaign / case / arm / repetition が全て一致）の識別を持つ task。最古を返す。
    pub fn task_campaign_by_trial_tx(
        conn: &rusqlite::Connection,
        id: &CampaignIdentity,
    ) -> Result<Option<String>, StoreError> {
        Ok(conn
            .query_row(
                "SELECT task_id FROM task_campaigns WHERE campaign_id = ?1 AND case_id IS ?2 \
                 AND arm IS ?3 AND repetition IS ?4 ORDER BY task_id LIMIT 1",
                params![
                    id.campaign_id,
                    id.case_id,
                    id.arm,
                    id.repetition.map(i64::from)
                ],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// 呼び出し側の transaction の中で、task 自身に付いた識別を読む。保存値が壊れていれば `Invalid`。
    pub fn task_campaign_own_tx(
        conn: &rusqlite::Connection,
        task_id: &str,
    ) -> Result<Option<CampaignIdentity>, StoreError> {
        let mut stmt = conn.prepare(&format!(
            "SELECT {CAMPAIGN_COLUMNS} FROM task_campaigns WHERE task_id = ?1"
        ))?;
        let mut rows = stmt.query([task_id])?;
        match rows.next()? {
            Some(r) => Ok(Some(campaign_from_row(task_id, r)?)),
            None => Ok(None),
        }
    }

    /// task 自身に付いた識別（継承しない）。
    pub fn task_campaign_own(&self, task_id: &str) -> Result<Option<CampaignIdentity>, StoreError> {
        self.with_read_conn(|conn| Self::task_campaign_own_tx(conn, task_id))
    }

    /// 自身から root まで鎖を辿り、最も近い識別とその持ち主を返す。鎖の上の識別は全て一致
    /// しなければならない（最寄りで矛盾を隠さない）。循環・上限超えも `Invalid`。
    fn task_campaign_resolve_holder_tx(
        conn: &rusqlite::Connection,
        task_id: &str,
    ) -> Result<Option<(String, CampaignIdentity)>, StoreError> {
        let mut nearest: Option<(String, CampaignIdentity)> = None;
        let mut seen = std::collections::HashSet::new();
        let mut current = task_id.to_string();
        loop {
            if !seen.insert(current.clone()) {
                return Err(StoreError::Invalid(format!(
                    "task {task_id}: parent chain has a cycle at task {current}"
                )));
            }
            if seen.len() > CAMPAIGN_ANCESTOR_LIMIT {
                return Err(StoreError::Invalid(format!(
                    "task {task_id}: parent chain exceeds {CAMPAIGN_ANCESTOR_LIMIT} tasks"
                )));
            }
            if let Some(id) = Self::task_campaign_own_tx(conn, &current)? {
                match &nearest {
                    None => nearest = Some((current.clone(), id)),
                    Some((holder, near)) if near != &id => {
                        return Err(campaign_mismatch(holder, &current));
                    }
                    Some(_) => {}
                }
            }
            let parent: Option<String> = conn
                .query_row(
                    "SELECT parent_id FROM tasks WHERE id = ?1",
                    [&current],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            match parent {
                Some(p) => current = p,
                None => return Ok(nearest),
            }
        }
    }

    /// task の識別。自身に無ければ `parent_id` の鎖を辿って最も近い祖先のものを返す。
    /// 鎖の上（root まで）の識別が食い違う・循環・上限超え・保存値の不正は `Invalid`。
    /// campaign に属さない task は `None`（従来どおり識別 NULL）。
    pub fn task_campaign_resolve(
        &self,
        task_id: &str,
    ) -> Result<Option<CampaignIdentity>, StoreError> {
        self.with_read_conn(|conn| {
            Ok(Self::task_campaign_resolve_holder_tx(conn, task_id)?.map(|(_, id)| id))
        })
    }
}

// ---- CoS の実 turn・人の介入の記録（ADR 2026-10-16-campaign-identity-propagation 付記）----

/// 帰属しない CoS turn の再評価を続ける期間（campaign の窓が後から延びて当たる場合の保険）。
const COS_REATTRIBUTE_WINDOW_SECS: i64 = 24 * 3600;

struct CosTurnUsageRow {
    run_id: String,
    started: Option<String>,
    finished: Option<String>,
    usage_json: Option<String>,
}

fn parse_ts(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

/// 判断・操作の event を介入の区分と主体の `by` へ写す（ADR 2026-10-27-decision-log-intervention-actors D2）。
/// 主体は問わない（人以外の処理も行に残し、主体の欄で区別する）。介入でない event は `None`。
pub(crate) fn intervention_of(event: &crate::Event) -> Option<(InterventionKind, Option<&str>)> {
    use crate::Event;
    Some(match event {
        // blocked・decision への回答と計画の承認待ちの判断は研究の方針の選択。
        Event::Answered { by, .. } => (InterventionKind::ResearchJudgment, by.as_deref()),
        Event::DecisionAnswered { by, .. } | Event::PlanGateDecided { by, .. } => {
            (InterventionKind::ResearchJudgment, Some(by.as_str()))
        }
        // 失敗した task の再実行と、自動統合の失敗からの回復。
        Event::Retried { by, .. } | Event::IntegrationAnswered { by, .. } => {
            (InterventionKind::Recovery, by.as_deref())
        }
        Event::ApprovalDecided { by, .. } | Event::Edited { by, .. } => {
            (InterventionKind::Unclassified, Some(by.as_str()))
        }
        _ => return None,
    })
}

/// event の終了の主体（wait の `end_actor_kind`）。`by` を持たない event は `None`。
fn event_by(event: &crate::Event) -> Option<Option<&str>> {
    intervention_of(event).map(|(_, by)| by)
}

impl SqliteStore {
    /// `cos:<thread>` の thread の種類（読めなければ `None`）。
    fn chat_thread_kind_of(&self, thread_id: &str) -> Option<crate::chat::ChatThreadKind> {
        let kind: Option<String> = self
            .lock()
            .ok()?
            .query_row(
                "SELECT kind FROM chat_threads WHERE id = ?1",
                [thread_id],
                |r| r.get(0),
            )
            .optional()
            .ok()??;
        serde_json::from_value(serde_json::Value::String(kind?)).ok()
    }

    /// event の `by` を主体の種類と id へ（D1）。
    fn intervention_actor(&self, by: Option<&str>) -> (InterventionActorKind, Option<String>) {
        InterventionActorKind::classify(by, |t| self.chat_thread_kind_of(t))
    }

    /// task の events から介入を投影する（`intervention_id = <task_id>:<seq>` で冪等）。主体は event の `by` から決め、
    /// 人以外の処理も `actor_kind` 付きで残す。campaign 識別（自身か祖先）のある task だけ書く。
    /// 人の所要時間は取れないので `wall_ms` は欠測。書いた件数（upsert した数）を返す。
    pub fn decision_log_project_interventions(&self, task_id: &str) -> Result<usize, StoreError> {
        let Some(identity) = self.task_campaign_resolve(task_id)? else {
            return Ok(0);
        };
        let rows: Vec<(i64, String, String)> = {
            let conn = self.lock()?;
            let mut stmt = conn
                .prepare("SELECT seq, ts, json FROM events WHERE task_id = ?1 ORDER BY seq ASC")?;
            stmt.query_map([task_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?
        };
        let mut n = 0;
        for (seq, ts, json) in rows {
            let event: crate::Event = serde_json::from_str(&json)?;
            let Some((kind, by)) = intervention_of(&event) else {
                continue;
            };
            let (actor_kind, actor_id) = self.intervention_actor(by);
            self.decision_log_record_intervention(&DecisionLogIntervention {
                intervention_id: format!("{task_id}:{seq}"),
                kind,
                actor_kind,
                actor_id,
                task_id: Some(task_id.to_string()),
                campaign_id: Some(identity.campaign_id.clone()),
                at: ts,
                wall_ms: None,
                source_event: Some(format!("event:{task_id}:{seq}")),
            })?;
            n += 1;
        }
        Ok(n)
    }

    /// campaign の task 木（識別を持つ task とその子孫）の events から、人の応答待ちの区間を投影する
    /// （`record_type: "wait"`、`dedup_key = wait:<task_id>:<開始 seq>`）。export の追加行で、DB には書かない。
    ///
    /// - `question`: `QuestionRaised`（開始。`run_id` はきっかけの run）→ 次の `Answered`（終了）。
    /// - `approval`: `ApprovalRequested` → 次の `ApprovalDecided`。
    /// - `decision`: `DecisionRequested` → 同じ decision id の最初の `DecisionAnswered` か `DecisionWithdrawn`
    ///   （decision id で対にする。revise は終了にしない。ADR 2026-10-27-decision-log-intervention-actors D4）。
    /// - 終了が無い区間は `ended_at: null`（open。0 秒にしない）。開始・終了の event は `event:<task_id>:<seq>` で残す。
    /// - 終了の主体は `end_actor_kind`・`end_actor_id`（終了が無い・`by` を持たない終了は `null`）。
    pub fn decision_log_human_waits(
        &self,
        campaign_id: &str,
    ) -> Result<Vec<serde_json::Value>, StoreError> {
        let rows: Vec<(String, i64, String, String)> = {
            let conn = self.lock()?;
            let mut stmt = conn.prepare(
                "WITH RECURSIVE tree(id) AS ( \
                   SELECT task_id FROM task_campaigns WHERE campaign_id = ?1 \
                   UNION SELECT t.id FROM tasks t JOIN tree ON t.parent_id = tree.id) \
                 SELECT e.task_id, e.seq, e.ts, e.json FROM events e JOIN tree ON e.task_id = tree.id \
                 ORDER BY e.id",
            )?;
            stmt.query_map([campaign_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<Result<_, _>>()?
        };
        // 全 event の追記順に読み（decision の回答は別の task の列に積まれ得る）、出力は (task_id, 開始 seq) の順に並べる。
        let mut out = Vec::new();
        let mut order: Vec<(String, i64)> = Vec::new();
        // 対の鍵（question・approval は (task_id, kind)、decision は decision id）→ 開いている区間の index
        // （先に開いたものから閉じる）。
        let mut open: std::collections::BTreeMap<(String, &'static str), Vec<usize>> =
            std::collections::BTreeMap::new();
        for (task_id, seq, ts, json) in rows {
            let Ok(event) = serde_json::from_str::<crate::Event>(&json) else {
                continue;
            };
            let (kind, key, start, run_id) = match &event {
                crate::Event::QuestionRaised { run_id, .. } => {
                    ("question", task_id.clone(), true, Some(run_id.clone()))
                }
                crate::Event::Answered { .. } => ("question", task_id.clone(), false, None),
                crate::Event::ApprovalRequested => ("approval", task_id.clone(), true, None),
                crate::Event::ApprovalDecided { .. } => ("approval", task_id.clone(), false, None),
                crate::Event::DecisionRequested { decision } => {
                    ("decision", decision.id.clone(), true, None)
                }
                crate::Event::DecisionAnswered { id, .. }
                | crate::Event::DecisionWithdrawn { id, .. } => {
                    ("decision", id.clone(), false, None)
                }
                _ => continue,
            };
            let ev_ref = format!("event:{task_id}:{seq}");
            let slot = open.entry((key, kind)).or_default();
            if start {
                slot.push(out.len());
                order.push((task_id.clone(), seq));
                out.push(serde_json::json!({
                    "record_type": "wait",
                    "schema": crate::decision_log_export::RECORD_SCHEMA,
                    "dedup_key": format!("wait:{task_id}:{seq}"),
                    "campaign_id": campaign_id,
                    "task_id": task_id,
                    "run_id": run_id,
                    "wait_kind": kind,
                    "started_at": ts,
                    "ended_at": null,
                    "start_event": ev_ref,
                    "end_event": null,
                    "end_actor_kind": null,
                    "end_actor_id": null,
                }));
            } else if !slot.is_empty() {
                let i = slot.remove(0);
                out[i]["ended_at"] = serde_json::json!(ts);
                out[i]["end_event"] = serde_json::json!(ev_ref);
                if let Some(by) = event_by(&event) {
                    let (actor_kind, actor_id) = self.intervention_actor(by);
                    out[i]["end_actor_kind"] = serde_json::json!(actor_kind.as_str());
                    out[i]["end_actor_id"] = serde_json::json!(actor_id);
                }
            }
        }
        let mut sorted: Vec<_> = order.into_iter().zip(out).collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(sorted.into_iter().map(|(_, w)| w).collect())
    }

    /// 時刻 `at` に活動中（最初の識別付与〜最後の活動）の campaign がちょうど 1 つならその id。
    /// 0 個・2 個以上は `None`（帰属を推測しない）。
    fn campaign_active_at(&self, at: OffsetDateTime) -> Result<Option<String>, StoreError> {
        let windows: Vec<(String, String, Option<String>)> = {
            let conn = self.lock()?;
            let mut stmt = conn.prepare(
                "SELECT t.campaign_id, MIN(t.created_at), a.last_activity_at FROM task_campaigns t \
                 LEFT JOIN decision_log_campaign_activity a ON a.campaign_id = t.campaign_id \
                 GROUP BY t.campaign_id",
            )?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<_, _>>()?
        };
        let mut hit = windows.into_iter().filter(|(_, start, end)| {
            let (Some(s), Some(e)) = (parse_ts(start), end.as_deref().and_then(parse_ts)) else {
                return false;
            };
            s <= at && at <= e
        });
        let first = hit.next().map(|(c, _, _)| c);
        Ok(if hit.next().is_some() { None } else { first })
    }

    /// CoS chat の終端 run（`usage_json` 欠測も含む）を、CoS の実 turn として消費行へ投影する。
    /// 常に共有オーバーヘッド。campaign は活動窓から一意に決まるときだけ付ける。観測できなかった欄は欠測のまま
    /// （0 にしない）。費用は定価換算の欄だけで、実支出には書かない。冪等（`decision:cos:<run_id>:1`）。
    pub fn decision_log_project_cos_turns(&self, now: OffsetDateTime) -> Result<usize, StoreError> {
        let cutoff = format_rfc3339(now - time::Duration::seconds(COS_REATTRIBUTE_WINDOW_SECS))?;
        let rows: Vec<CosTurnUsageRow> = {
            let conn = self.lock()?;
            let mut stmt = conn.prepare(
                "SELECT c.run_id, c.started_at, c.finished_at, c.usage_json FROM chat_runs c \
                 WHERE c.finished_at IS NOT NULL \
                 AND c.state IN ('completed','stopped','failed','interrupted') \
                 AND NOT EXISTS (SELECT 1 FROM decision_log_usage d \
                     WHERE d.dedup_key = 'decision:cos:' || c.run_id || ':1' \
                     AND (d.campaign_id IS NOT NULL OR c.finished_at < ?1)) \
                 ORDER BY c.run_id",
            )?;
            stmt.query_map([cutoff], |r| {
                Ok(CosTurnUsageRow {
                    run_id: r.get(0)?,
                    started: r.get(1)?,
                    finished: r.get(2)?,
                    usage_json: r.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?
        };
        let mut n = 0;
        for CosTurnUsageRow {
            run_id,
            started,
            finished,
            usage_json,
        } in rows
        {
            let usage = usage_json
                .as_deref()
                .and_then(|json| serde_json::from_str::<crate::Usage>(json).ok())
                .unwrap_or_default();
            let at = started
                .as_deref()
                .and_then(parse_ts)
                .or_else(|| finished.as_deref().and_then(parse_ts));
            let campaign = match at {
                Some(at) => self.campaign_active_at(at)?,
                None => None,
            };
            let mut row = DecisionLogUsage::cos_turn(&run_id, 1, campaign.as_deref());
            row.input_tokens = usage.input_tokens;
            row.output_tokens = usage.output_tokens;
            row.cache_read_tokens = usage.cache_read_tokens;
            row.cache_creation_tokens = usage.cache_creation_tokens;
            row.list_cost_usd = usage.cost_usd;
            row.wall_ms = match (
                started.as_deref().and_then(parse_ts),
                finished.as_deref().and_then(parse_ts),
            ) {
                (Some(a), Some(b)) => u64::try_from((b - a).whole_milliseconds()).ok(),
                _ => None,
            };
            self.decision_log_record_usage(&row, now)?;
            n += 1;
        }
        Ok(n)
    }
}
