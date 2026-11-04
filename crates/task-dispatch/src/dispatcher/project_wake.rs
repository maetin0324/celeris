//! ADR 2026-11-04-project-controller D12・A1: 案件ごとの durable wake の tick の段。LLM は呼ばない（I3）。
//!
//! 流れ: `events`・停滞の検出・cron・人の操作 → `project_wakes`（`UNIQUE(project_id, wake_key)` で再配送を
//! 束ねる）→ coalesce（`coalesce_secs`）→ lease → [`ProjectDecider`] → tx1（CAS: `revision`・`human_epoch`・
//! lease。判断の行・wake の消費・cursor・効果 `pending`）→ tx2（効果を冪等に実行）→ `applied`。
//!
//! - 効果は tx1 で判断の行に書いてから実行するので、どこで落ちても再起動後に同じ key で 1 回だけ効く
//!   （root は `task_idempotency_keys`、CoS 入力は `client_message_id`）。crash の境界は [`Boundary`]。
//! - backpressure: 1 tick の評価は `max_evaluations_per_tick` 件（古い wake の順）、CoS 入力は案件ごとに
//!   未処理 `max_cos_inputs_open_per_project` 件まで。
//! - 判定（[`ProjectDecider`]）は A2・A3 が TS の proposal と接続する。A1 の既定は常に効果なし。
//! - mode `off`・状態の行が無い案件は wake を積まず、何も書かない（I13）。
//! - `pending` の効果は実行の直前に門（mode・phase・案件の状態）を通し、止まった案件では `suppressed` にする。
//!   効果の `Err` と CoS 入力の Busy は `pending` のまま backoff で延期し、`Err` が `max_retries` を超えたら
//!   notice を書いて `failed`・phase `blocked` にする（ADR 付記 A1 の 10）。

use sha2::{Digest, Sha256};
use task_core::project_wake::{
    EffectState, NewProjectWake, ProjectControllerDecision, ProjectDecideInput, ProjectWakeKind,
};
use task_core::store::{
    CommitOutcome, EffectAdmit, EffectDeferKind, EffectDeferral, IdempotencyClaim,
    NewProjectDecision, RootCreate, SqliteStore,
};
use task_core::{Event, NoticeStore, Status, TaskId, TaskStore};
use task_ops::add::NewTaskSpec;
use time::OffsetDateTime;

use crate::dispatcher::Dispatcher;
use crate::dispatcher::cos_chat::launch::CosChatLaunch;
use crate::project_controller::{
    Boundary, ControllerTickStats, ProjectControllerSettings, ProjectDecider, ProjectEffect,
};

/// 全体の取り込み cursor（`feed_cursor`）の名前。
pub const PROJECT_WAKE_CURSOR: &str = "project_wake";
const EVENT_BATCH: usize = 512;
const EVENT_PAGES_PER_TICK: usize = 8;

pub(crate) type BoundaryHook = Box<dyn FnMut(Boundary) + Send>;
/// 試験: 効果の実行の前に `true` を返すと、その効果を実行せず失敗として扱う（一時的な失敗の注入）。
pub(crate) type EffectFailHook = Box<dyn FnMut(&ProjectControllerDecision) -> bool + Send>;

/// controller の実行時の状態（`CosChatLaunch` が持つ。`dispatcher.rs` に欄を足さない）。
pub(crate) struct ProjectControllerRuntime {
    pub settings: ProjectControllerSettings,
    pub decider: Box<dyn ProjectDecider>,
    /// lease の持ち主（daemon の instance id）。
    pub owner: String,
    pub hook: Option<BoundaryHook>,
    pub fail_effect: Option<EffectFailHook>,
    pub last: ControllerTickStats,
}

impl ProjectControllerRuntime {
    fn at(&mut self, b: Boundary) {
        if let Some(hook) = self.hook.as_mut() {
            hook(b);
        }
    }
}

impl Dispatcher {
    /// controller を入れる（CoS chat の後に呼ぶ。CoS chat が無い構成では入らず `false`）。
    pub fn set_project_controller(
        &mut self,
        settings: ProjectControllerSettings,
        decider: Box<dyn ProjectDecider>,
        owner: String,
    ) -> bool {
        let Some(launch) = self.cos_chat_launch.as_mut() else {
            return false;
        };
        launch.project_controller = Some(ProjectControllerRuntime {
            settings,
            decider,
            owner,
            hook: None,
            fail_effect: None,
            last: ControllerTickStats::default(),
        });
        true
    }

    /// 試験: crash の境界の hook。
    #[cfg(test)]
    pub(crate) fn set_project_controller_hook(&mut self, hook: BoundaryHook) {
        if let Some(rt) = self
            .cos_chat_launch
            .as_mut()
            .and_then(|l| l.project_controller.as_mut())
        {
            rt.hook = Some(hook);
        }
    }

    /// 試験: 効果の失敗の注入。
    #[cfg(test)]
    pub(crate) fn set_project_controller_effect_fail(&mut self, hook: EffectFailHook) {
        if let Some(rt) = self
            .cos_chat_launch
            .as_mut()
            .and_then(|l| l.project_controller.as_mut())
        {
            rt.fail_effect = Some(hook);
        }
    }

    /// 試験・診断: 直前の tick の集計。
    #[cfg(test)]
    pub(crate) fn project_controller_last(&self) -> Option<ControllerTickStats> {
        self.cos_chat_launch
            .as_ref()
            .and_then(|l| l.project_controller.as_ref())
            .map(|rt| rt.last.clone())
    }

    /// tick の段（`tick_cron_jobs` から呼ぶ。draining・disk 不足では呼ばれない）。
    pub(crate) fn tick_project_controller(&mut self) {
        let now = self.now_utc();
        let findings = self.stagnation.findings.clone();
        let roles = self.config.roles.clone();
        let genres = self.config.genres.clone();
        // A3: その tick の領域 `project_controller` の宛先（mode の表は tick ごとに 1 回読む）。
        let access = if self
            .cos_chat_launch
            .as_ref()
            .and_then(|l| l.project_controller.as_ref())
            .is_some_and(|rt| rt.settings.enabled)
        {
            use task_core::script_compare::{ScriptDomain, ScriptMode};
            self.script_target(
                ScriptDomain::ProjectController,
                &[ScriptMode::Compare, ScriptMode::Canary, ScriptMode::Ts],
            )
            .map(|t| crate::project_decider_ts::ScriptAccess {
                mode: t.mode,
                canary_bps: t.canary_bps,
                generation_id: t.generation_id,
                call: t.call.map(|(script, _)| script),
            })
        } else {
            None
        };
        let Some(launch) = self.cos_chat_launch.as_mut() else {
            return;
        };
        let Some(mut rt) = launch.project_controller.take() else {
            return;
        };
        if rt.settings.enabled {
            let store = launch.store.clone();
            rt.decider.attach(&store, access);
            let mut pass = Pass {
                store: &store,
                rt: &mut rt,
                roles: &roles,
                genres: &genres,
                now,
                stats: ControllerTickStats::default(),
            };
            pass.run(&findings);
            rt.last = pass.stats;
        }
        if let Some(launch) = self.cos_chat_launch.as_mut() {
            launch.project_controller = Some(rt);
        }
    }
}

/// cron の `project_reconcile`: 案件の wake `cron:<job>:<予定時刻 unix 秒>` を積む。controller が無い・
/// 案件が `off` なら積まずに `Posted`（履歴は `created`。案件の再評価は起きない）。
pub(crate) fn post_reconcile_wake(
    launch: Option<&CosChatLaunch>,
    job: &task_core::CronJob,
    scheduled_for: OffsetDateTime,
    project_id: &str,
    now: OffsetDateTime,
) -> Result<task_ops::cron_jobs::CronInputPost, String> {
    let Some(launch) = launch else {
        return Err("CoS chat is not configured; no project controller".into());
    };
    let Some(rt) = launch.project_controller.as_ref() else {
        return Ok(task_ops::cron_jobs::CronInputPost::Posted(
            "project controller is not installed".into(),
        ));
    };
    if !rt.settings.enabled {
        return Ok(task_ops::cron_jobs::CronInputPost::Posted(
            "project controller is disabled".into(),
        ));
    }
    let key = format!("cron:{}:{}", job.id, scheduled_for.unix_timestamp());
    let n = launch
        .store
        .project_wake_ingest(
            &[NewProjectWake {
                project_id: project_id.into(),
                wake_key: key.clone(),
                kind: ProjectWakeKind::Cron,
                source_event_id: None,
                source_ref: Some(job.id.to_string()),
            }],
            None,
            rt.settings.coalesce_secs,
            now,
        )
        .map_err(|e| e.to_string())?;
    Ok(task_ops::cron_jobs::CronInputPost::Posted(if n > 0 {
        format!("wake {key}")
    } else {
        format!("wake {key} not added (already queued or project controller off)")
    }))
}

struct Pass<'a> {
    store: &'a SqliteStore,
    rt: &'a mut ProjectControllerRuntime,
    roles: &'a [task_core::RoleSpec],
    genres: &'a [task_core::GenreSpec],
    now: OffsetDateTime,
    stats: ControllerTickStats,
}

impl Pass<'_> {
    fn run(&mut self, findings: &[task_ops::stagnation::StagnationFinding]) {
        self.rt.at(Boundary::BeforeIngest);
        if let Err(error) = self.ingest_events() {
            tracing::warn!(%error, "project controller: event intake failed");
        }
        if let Err(error) = self.ingest_stagnation(findings) {
            tracing::warn!(%error, "project controller: stagnation intake failed");
        }
        self.rt.at(Boundary::AfterIngest);
        // B4・B5 の回復: tx1 で書いた効果が `pending` のまま残っていれば、同じ key で実行し直す。
        match self.store.project_controller_pending_effects() {
            Ok(pending) => {
                for d in pending {
                    self.apply(&d);
                }
            }
            Err(error) => tracing::warn!(%error, "project controller: pending effects unreadable"),
        }
        let due = match self
            .store
            .project_controller_due(self.now, self.rt.settings.max_evaluations_per_tick)
        {
            Ok(due) => due,
            Err(error) => {
                tracing::warn!(%error, "project controller: due projects unreadable");
                return;
            }
        };
        for project in due {
            if let Err(error) = self.evaluate(&project) {
                tracing::warn!(%error, project = %project, "project controller: evaluation failed");
            }
        }
    }

    /// events を cursor から読んで wake にする。cursor が無い初回は今の末尾から（過去を遡らない）。
    fn ingest_events(&mut self) -> Result<(), String> {
        let store = self.store;
        let Some(raw) = store
            .feed_cursor_get(PROJECT_WAKE_CURSOR)
            .map_err(|e| e.to_string())?
        else {
            let head = store.latest_event_id().map_err(|e| e.to_string())?;
            return store
                .feed_cursor_set(PROJECT_WAKE_CURSOR, &head.to_string())
                .map_err(|e| e.to_string());
        };
        let mut cursor: u64 = raw
            .parse()
            .map_err(|_| format!("invalid project wake cursor {raw}"))?;
        for _ in 0..EVENT_PAGES_PER_TICK {
            let rows = store
                .events_since(cursor, EVENT_BATCH)
                .map_err(|e| e.to_string())?;
            let Some(last) = rows.last().map(|r| r.id) else {
                break;
            };
            let mut wakes = Vec::new();
            for row in &rows {
                wakes.extend(wakes_for_event(store, row.id, row.task_id, &row.event)?);
            }
            self.stats.ingested += store
                .project_wake_ingest(
                    &wakes,
                    Some((PROJECT_WAKE_CURSOR, last)),
                    self.rt.settings.coalesce_secs,
                    self.now,
                )
                .map_err(|e| e.to_string())?;
            cursor = last;
            if rows.len() < EVENT_BATCH {
                break;
            }
        }
        Ok(())
    }

    /// 停滞の検出（`StagnationFinding`）を wake にする。論理キーと since で 1 件。
    fn ingest_stagnation(
        &mut self,
        findings: &[task_ops::stagnation::StagnationFinding],
    ) -> Result<(), String> {
        let wakes: Vec<NewProjectWake> = findings
            .iter()
            .filter_map(|f| {
                let project = f.project_id?;
                Some(NewProjectWake {
                    project_id: project.to_string(),
                    wake_key: format!("stagnation:{}:{}", f.key, f.since),
                    kind: ProjectWakeKind::Stagnation,
                    source_event_id: None,
                    source_ref: Some(f.key.clone()),
                })
            })
            .collect();
        if wakes.is_empty() {
            return Ok(());
        }
        self.stats.ingested += self
            .store
            .project_wake_ingest(&wakes, None, self.rt.settings.coalesce_secs, self.now)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn evaluate(&mut self, project: &str) -> Result<(), String> {
        let s = &self.rt.settings;
        let Some(lease) = self
            .store
            .project_controller_lease(project, &self.rt.owner, s.lease_secs, self.now)
            .map_err(|e| e.to_string())?
        else {
            return Ok(());
        };
        self.rt.at(Boundary::AfterLease);
        let input = ProjectDecideInput {
            project_id: project.into(),
            revision: lease.state.revision,
            human_epoch: lease.state.human_epoch,
            mode: lease.state.mode,
            wakes: lease.wakes.clone(),
            now: format_time(self.now),
        };
        self.stats.decided += 1;
        let decision = match self.rt.decider.decide(&input) {
            Ok(d) => d,
            Err(error) => {
                self.stats.decide_failed += 1;
                let s = &self.rt.settings;
                let failure = self
                    .store
                    .project_controller_decide_failed(
                        project,
                        &self.rt.owner,
                        (s.retry_base_secs, s.retry_max_secs, s.max_retries),
                        self.now,
                        &|state, n| decide_exhausted_notice(state, n, &error, self.now),
                    )
                    .map_err(|e| e.to_string())?;
                tracing::warn!(%error, project, ?failure, "project controller: decide failed");
                return Ok(());
            }
        };
        self.rt.at(Boundary::BeforeCommit);
        let input_json = serde_json::to_string(&input).map_err(|e| e.to_string())?;
        let effect_json = serde_json::to_string(&decision.effect).map_err(|e| e.to_string())?;
        let request_sha256 = project_request_sha256(self.store, project);
        let new = NewProjectDecision {
            id: ulid::Ulid::new().to_string(),
            based_on_revision: input.revision,
            human_epoch: input.human_epoch,
            snapshot_sha256: sha256_hex(input_json.as_bytes()),
            request_sha256,
            generation_id: None,
            proposal_json: self.rt.decider.take_record().unwrap_or(input_json),
            validation: decision.rule_id.clone(),
            effect_json,
            has_effect: decision.effect.has_effect(),
            next_wake_at: decision
                .next_wake_after_secs
                .map(|secs| self.now + time::Duration::seconds(secs as i64)),
            phase: decision.phase,
        };
        let wake_ids: Vec<i64> = lease.wakes.iter().map(|w| w.id).collect();
        match self
            .store
            .project_controller_commit(project, &self.rt.owner, &wake_ids, &new, self.now)
            .map_err(|e| e.to_string())?
        {
            CommitOutcome::Stale(reason) => {
                // I7: 人の操作（または他の instance）が先。起票は捨て、wake は残して次の tick で評価し直す。
                self.stats.stale.push(reason.code().into());
                tracing::info!(
                    project,
                    reason = reason.code(),
                    "project controller: decision discarded"
                );
            }
            CommitOutcome::Committed {
                decision_id,
                effect_state,
            } => {
                self.rt.at(Boundary::AfterCommit);
                if effect_state == EffectState::Pending
                    && let Some(d) = self
                        .store
                        .project_controller_decision(&decision_id)
                        .map_err(|e| e.to_string())?
                {
                    self.apply(&d);
                }
            }
        }
        Ok(())
    }

    /// tx2: 効果を冪等に実行して `applied` へ進める。効果が既に効いていれば（crash の B5）`applied` と
    /// 記録する。そうでなければ実行の前に門（mode・phase・案件の状態・判断の `human_epoch`）を通し（I7・I13）、
    /// 書き込みの tx でも同じ門を読む（付記 A1 の 11）。CoS 入力の Busy と効果の `Err` は `pending` のまま
    /// backoff で延期し、`Err` が `max_retries` を超えたら `failed`・phase `blocked` にして人に知らせる（notice）。
    fn apply(&mut self, d: &ProjectControllerDecision) {
        let parsed = serde_json::from_str::<ProjectEffect>(&d.effect_json);
        if let Ok(effect) = &parsed {
            match self.effect_in_place(&d.project_id, effect) {
                Ok(Some(effect_ref)) => {
                    self.finish(d, EffectState::Applied, Some(&effect_ref));
                    self.stats.applied += 1;
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, decision = %d.id, "project controller: effect state unreadable");
                    return;
                }
            }
        }
        if !self.admit(d) {
            return;
        }
        let effect = match parsed {
            Ok(e) => e,
            Err(error) => {
                self.finish(
                    d,
                    EffectState::Failed,
                    Some(&format!("effect_json: {error}")),
                );
                return;
            }
        };
        let injected = self.rt.fail_effect.as_mut().is_some_and(|fail| fail(d));
        let done = match effect {
            _ if injected => Err("injected effect failure".to_string()),
            ProjectEffect::None => Ok(None),
            ProjectEffect::QueueCosInput { key, text } => {
                match self.store.project_controller_queue_cos_input(
                    &d.project_id,
                    &key,
                    &text,
                    self.rt.settings.max_cos_inputs_open_per_project,
                    d.human_epoch,
                    self.now,
                ) {
                    Ok(task_core::chat::ControllerCosInput::Queued(id)) => Ok(Some(id)),
                    Ok(task_core::chat::ControllerCosInput::Busy) => {
                        self.stats.busy += 1;
                        self.defer(d, EffectDeferKind::Busy, None);
                        return;
                    }
                    Ok(task_core::chat::ControllerCosInput::Refused) => {
                        self.admit(d);
                        return;
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
            ProjectEffect::OpenEscalation {
                question_key,
                category,
                summary,
            } => self
                .open_escalation(&d.project_id, &question_key, &category, &summary)
                .map(Some),
            ProjectEffect::CreateRoot {
                idempotency_key,
                spec,
                meta,
            } => match self.create_root(
                &d.project_id,
                &idempotency_key,
                *spec,
                meta.as_deref(),
                d.human_epoch,
            ) {
                Ok(RootCreate::Created(id) | RootCreate::Existing(id)) => Ok(Some(id)),
                Ok(RootCreate::Refused) => {
                    self.admit(d);
                    return;
                }
                Err(e) => Err(e),
            },
        };
        match done {
            Ok(effect_ref) => {
                self.rt.at(Boundary::AfterEffect);
                self.finish(d, EffectState::Applied, effect_ref.as_deref());
                self.stats.applied += 1;
                self.rt.at(Boundary::AfterApplied);
            }
            Err(error) => {
                self.stats.effect_failed += 1;
                self.defer(d, EffectDeferKind::Failed, Some(&error));
            }
        }
    }

    /// 効果の門。実行してよければ `true`。閉じていれば門が判断を `suppressed` か `pending` のままにする。
    fn admit(&mut self, d: &ProjectControllerDecision) -> bool {
        match self.store.project_controller_effect_admit(&d.id, self.now) {
            Ok(EffectAdmit::Run) => true,
            Ok(EffectAdmit::Hold) => false,
            Ok(EffectAdmit::Halted(reason)) => {
                self.stats.halted += 1;
                tracing::info!(decision = %d.id, project = %d.project_id, %reason,
                    "project controller: pending effect halted");
                false
            }
            Err(error) => {
                tracing::warn!(%error, decision = %d.id, "project controller: effect gate unreadable");
                false
            }
        }
    }

    /// 効果が既に効いていれば、その参照（root の task id・CoS 入力の message id）。実在する root を
    /// `halted` と記録しないために、門より先に読む。
    fn effect_in_place(
        &self,
        project: &str,
        effect: &ProjectEffect,
    ) -> Result<Option<String>, String> {
        match effect {
            ProjectEffect::None | ProjectEffect::OpenEscalation { .. } => Ok(None),
            ProjectEffect::QueueCosInput { key, text } => self
                .store
                .project_controller_cos_input_existing(project, key, text)
                .map_err(|e| e.to_string()),
            ProjectEffect::CreateRoot {
                idempotency_key,
                spec,
                ..
            } => {
                let claim = root_claim(idempotency_key, spec)?;
                match self
                    .store
                    .idempotency_lookup(&claim)
                    .map_err(|e| e.to_string())?
                {
                    task_core::store::IdempotencyLookup::Same { task_id, .. }
                    | task_core::store::IdempotencyLookup::Conflict { task_id } => {
                        Ok(Some(task_id))
                    }
                    task_core::store::IdempotencyLookup::New => Ok(None),
                }
            }
        }
    }

    /// 効果を延期する。上限を超える `Err` では、先に notice を記録してから `failed`・`blocked` にする
    /// （notice は判断の id で冪等。間で落ちても次の tick が同じ notice を 1 件だけ書いて進める）。
    fn defer(&mut self, d: &ProjectControllerDecision, kind: EffectDeferKind, error: Option<&str>) {
        let s = &self.rt.settings;
        let policy = (s.retry_base_secs, s.retry_max_secs, s.max_retries);
        if kind == EffectDeferKind::Failed {
            let failures = match self.store.project_controller_effect_retry_state(&d.id) {
                Ok(Some(r)) => r.failures,
                Ok(None) => return,
                Err(error) => {
                    tracing::warn!(%error, decision = %d.id, "project controller: effect retry state unreadable");
                    return;
                }
            };
            if failures + 1 > policy.2
                && let Err(e) = self.notify_exhausted(d, failures + 1, error.unwrap_or("-"))
            {
                // notice が書けなければ failed にしない（次の tick でやり直す）。
                tracing::warn!(error = %e, decision = %d.id, "project controller: exhaustion notice not saved");
                return;
            }
        }
        match self
            .store
            .project_controller_effect_defer(&d.id, kind, error, policy, self.now)
        {
            Ok(EffectDeferral::Retry {
                attempts,
                failures,
                retry_at,
            }) => {
                if kind == EffectDeferKind::Failed {
                    tracing::warn!(decision = %d.id, project = %d.project_id, attempts, failures,
                        retry_at = %format_time(retry_at), error = error.unwrap_or("-"),
                        "project controller: effect failed; retrying with backoff");
                }
            }
            Ok(EffectDeferral::Exhausted { failures }) => {
                self.stats.exhausted += 1;
                tracing::warn!(decision = %d.id, project = %d.project_id, failures,
                    error = error.unwrap_or("-"),
                    "project controller: effect failed past max_retries; project blocked");
            }
            Ok(EffectDeferral::NotPending) => {}
            Err(e) => {
                tracing::warn!(error = %e, decision = %d.id, "project controller: effect deferral not saved");
            }
        }
    }

    fn notify_exhausted(
        &self,
        d: &ProjectControllerDecision,
        failures: u32,
        error: &str,
    ) -> Result<(), task_core::StoreError> {
        let title = format!(
            "案件 {} の controller の効果が {failures} 回失敗し、controller を止めました",
            d.project_id
        );
        let summary = format!(
            "{title}。判断 {} の効果は failed、controller の phase は blocked です。最後の失敗: {error}。原因を直してから controller の mode を置き直すと再開します。",
            d.id
        );
        self.store
            .notice_record(&task_core::feed::NoticeEvent {
                source_key: format!("project_controller:effect_failed:{}", d.id),
                kind: task_core::feed::NoticeKind::BadNews,
                group_key: format!("bad_news:project_controller:{}", d.project_id),
                title,
                summary,
                project_id: Some(d.project_id.clone()),
                task_id: None,
                target: None,
                links: Vec::new(),
                at: self.now,
            })
            .map(|_| ())
    }

    /// A3 の `escalate`: 人への判断の要求を notice 1 件で出す。source key に `question_key` を入れるので、
    /// 同じ問いは 2 回目を作らない（既に聞いた問いを聞き直さない）。同じ tx で phase を `blocked` にする。
    /// 参照に notice の source key を返す。
    fn open_escalation(
        &self,
        project: &str,
        question_key: &str,
        category: &str,
        summary: &str,
    ) -> Result<String, String> {
        let source_key = format!("project_controller:escalate:{project}:{question_key}");
        let title = format!("案件 {project} の controller が人の判断を待っています（{category}）");
        let body = format!(
            "{summary}。controller の phase は blocked で、人が controller の mode を置き直すまで判断しません。問いの key: {question_key}"
        );
        self.store
            .project_controller_escalate(
                project,
                &task_core::feed::NoticeEvent {
                    source_key: source_key.clone(),
                    kind: task_core::feed::NoticeKind::BadNews,
                    group_key: format!("bad_news:project_controller:{project}"),
                    title,
                    summary: body,
                    project_id: Some(project.to_owned()),
                    task_id: None,
                    target: None,
                    links: Vec::new(),
                    at: self.now,
                },
                self.now,
            )
            .map_err(|e| e.to_string())?;
        Ok(source_key)
    }

    fn finish(&mut self, d: &ProjectControllerDecision, state: EffectState, r: Option<&str>) {
        if let Err(error) = self
            .store
            .project_controller_effect_done(&d.id, state, r, self.now)
        {
            tracing::warn!(%error, decision = %d.id, "project controller: effect state not saved");
        }
    }

    /// root を `POST /tasks` と同じ組み立て（`build_task_with_roles`）で作る。案件・label・出自は
    /// controller が決める（`created_by = daemon`、`origin = Agent`、初期状態 `ready`）。
    fn create_root(
        &self,
        project: &str,
        key: &str,
        mut spec: NewTaskSpec,
        meta: Option<&str>,
        decision_human_epoch: i64,
    ) -> Result<RootCreate, String> {
        let project_id = project
            .parse()
            .map_err(|e| format!("project id {project}: {e}"))?;
        let claim = root_claim(key, &spec)?;
        spec.project_id = Some(project_id);
        spec.parent = None;
        if !spec
            .labels
            .iter()
            .any(|l| l == crate::project_controller::CONTROLLER_LABEL)
        {
            spec.labels
                .push(crate::project_controller::CONTROLLER_LABEL.into());
        }
        spec.status.get_or_insert(Status::Ready);
        spec.provenance.origin = task_ops::add::SpecOrigin::Agent;
        let task = task_ops::add::build_task_with_roles(
            self.store,
            spec,
            self.roles,
            self.genres,
            self.now,
        )
        .map_err(|e| e.to_string())?;
        self.store
            .project_controller_create_root(
                &task,
                &claim,
                task_core::DAEMON_ACTOR,
                meta,
                decision_human_epoch,
                self.now,
            )
            .map_err(|e| e.to_string())
    }
}

/// decide の失敗が `max_retries` を超えて phase `blocked` にするときの notice（付記 A1 の 12）。
/// source_key は案件と `revision`。同じ止まり方の再試行（crash の後など）では 1 件のまま、人が mode を
/// 置き直す（`revision + 1`）と次に止まったときに新しい notice になる。
fn decide_exhausted_notice(
    state: &task_core::project_wake::ProjectControllerState,
    failures: u32,
    error: &str,
    now: OffsetDateTime,
) -> task_core::feed::NoticeEvent {
    let project = &state.project_id;
    let title = format!(
        "案件 {project} の controller の判断が {failures} 回失敗し、controller を止めました"
    );
    let summary = format!(
        "{title}。controller の phase は blocked で、以後この案件の判断をしません。最後の失敗: {error}。原因を直してから controller の mode を置き直すと再開します。"
    );
    task_core::feed::NoticeEvent {
        source_key: format!(
            "project_controller:decide_failed:{project}:{}",
            state.revision
        ),
        kind: task_core::feed::NoticeKind::BadNews,
        group_key: format!("bad_news:project_controller:{project}"),
        title,
        summary,
        project_id: Some(project.clone()),
        task_id: None,
        target: None,
        links: Vec::new(),
        at: now,
    }
}

fn root_claim(key: &str, spec: &NewTaskSpec) -> Result<IdempotencyClaim, String> {
    let request_hash = sha256_hex(
        serde_json::to_string(spec)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    );
    Ok(IdempotencyClaim {
        key: key.into(),
        operation: "project_controller.create_root".into(),
        request_hash,
    })
}

fn wakes_for_event(
    store: &SqliteStore,
    event_id: u64,
    task_id: TaskId,
    event: &Event,
) -> Result<Vec<NewProjectWake>, String> {
    let kind = match event {
        Event::Transitioned { to, reason, .. } => match to {
            Status::Done => Some(ProjectWakeKind::RootDone),
            Status::Failed if reason == "review_fail" => Some(ProjectWakeKind::ReviewFailed),
            Status::Failed => Some(ProjectWakeKind::RootFailed),
            Status::Cancelled => None,
            _ => return Ok(Vec::new()),
        },
        Event::DecisionAnswered { .. }
        | Event::Answered { .. }
        | Event::ApprovalDecided { .. }
        | Event::PlanGateDecided { .. } => Some(ProjectWakeKind::DecisionAnswered),
        Event::ClusterJobWaitFinished { .. }
        | Event::BrowserWaitResolved { .. }
        | Event::BrowserPrerequisiteResumed { .. } => Some(ProjectWakeKind::ExternalReleased),
        _ => return Ok(Vec::new()),
    };
    let Some(task) = store.get(task_id).map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    let Some(project) = task.project_id else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut push = |kind: ProjectWakeKind, subject: TaskId| {
        let prefix = match kind {
            ProjectWakeKind::ExternalReleased => "external",
            other => other.as_str(),
        };
        out.push(NewProjectWake {
            project_id: project.to_string(),
            wake_key: format!("{prefix}:{subject}:{event_id}"),
            kind,
            source_event_id: Some(event_id as i64),
            source_ref: Some(subject.to_string()),
        });
    };
    match kind {
        Some(ProjectWakeKind::RootDone) => {
            if task_ops::root_done::is_root_done(&task) {
                push(ProjectWakeKind::RootDone, task.id);
            }
        }
        Some(k @ (ProjectWakeKind::RootFailed | ProjectWakeKind::ReviewFailed)) => {
            if task_core::is_root_task(&task) {
                push(k, task.id);
            }
        }
        Some(k) => push(k, task.id),
        None => {}
    }
    // 子の終端: 親の待ち（`awaited_children`）が空になったら親の案件に `children_settled`（I15: 親を起こすのは
    // 既存の tree。controller は観測するだけ）。
    if let (Event::Transitioned { to, .. }, Some(parent)) = (event, task.parent_id)
        && to.is_terminal()
    {
        let awaited =
            task_ops::delegate::child_wait::awaited_children(store, parent, true, &mut |_| {
                Ok(false)
            })
            .map_err(|e| e.to_string())?;
        if awaited.is_empty() {
            push(ProjectWakeKind::ChildrenSettled, parent);
        }
    }
    Ok(out)
}

fn project_request_sha256(store: &SqliteStore, project: &str) -> String {
    let request = project
        .parse()
        .ok()
        .and_then(|id| store.project_get(id).ok().flatten())
        .map(|p| p.request)
        .unwrap_or_default();
    sha256_hex(request.as_bytes())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn format_time(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
