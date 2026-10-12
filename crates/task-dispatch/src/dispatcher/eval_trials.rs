//! 評価 run の起動段（tick の `eval_trials`）。
//! ADR 2026-10-11-llm-source-model-routing-evaluation 付記 2026-10-12（eval-dispatch）D4。
//!
//! 1 試行 = harness 固定・source×model 固定・新 session・新 worktree の評価 run 1 本。
//! worker に渡す本文は `objective` だけで、採点専用の `check_cmd` は `RunRequest` に載せない
//! （`acceptance` は空、`checks` は持たない）。評価 run は本番の全体並列度に数えず、
//! `[routing_eval] reserved_runs` の予約枠の中だけで走る（tick では本番の dispatch の後）。
//!
//! 完了側（採点・`route_outcomes`・worktree の削除）は別の段（`trial-score`）。ここは run が
//! 終わったら `eval_trials.status = scoring` にして handing off するだけ。

use super::*;

use sha2::{Digest, Sha256};
use task_core::route_record::{
    FeaturesAux, ROUTE_OPTIONS_SCHEMA, RouteOption, RouteOptionBinds, RouteOptions,
    TASK_FEATURES_SCHEMA, TaskFeatures,
};
use task_core::routing_eval_trial::{EvalTrial, EvalTrialStatus};
use task_core::store::StoredRouteDecision;
use task_core::{Budget, WorkerHint};

use crate::capacity::EvalDefer;

/// 評価 run の `route_features.template`（本番の `task-brief/3` とは別の特徴量）。
const EVAL_FEATURES_TEMPLATE: &str = "eval";
/// 評価 run の `route_features.subject_kind` / `route_decisions.subject_kind`。
const EVAL_SUBJECT_KIND: &str = "eval_problem";
/// 評価 run の `route_decisions.stage`。
const EVAL_STAGE: &str = "eval";
/// 評価 run の `route_options.builder`（候補は 1 件だけ。人が trial を作った時点で決まっている）。
const EVAL_OPTIONS_BUILDER: &str = "eval-trial/1";
/// 評価 run 1 本の予算（harness 固定の評価は長い対話をしない。`[routing_eval]` の予約枠の中で走る）。
const EVAL_MAX_TURNS: u64 = 80;
const EVAL_MAX_WALL_SECS: u64 = 1800;
/// harness の `--version` の待ち上限（版が取れなければ記録せずに起動する）。
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
/// `--version` の出力の記録上限（生の出力をそのまま `harness_version` にはしない）。
const VERSION_MAX_CHARS: usize = 200;

/// 評価 run の経路（harness 固定・source×model 固定）。
struct EvalRoute {
    provider: ProviderId,
    adapter: Arc<dyn WorkerAdapter>,
    /// `runs.account`（= 解決した LLM source の id）。
    account: String,
    account_adapter: Option<AccountAdapter>,
    model: String,
}

/// 起動した評価 run の追跡（プロセス内のみ。真実は `eval_trials` の行と `runs` 索引）。
pub(super) struct EvalRunEntry {
    trial_id: String,
    account: String,
    account_adapter: Option<AccountAdapter>,
    handle: JoinHandle<()>,
}

/// `route_features.hash` の材料（`task_core::route_record::FeatureBody` と同じ形の直列化。
/// 内容アドレスなので、同じ特徴量は同じ hash になる）。
#[derive(serde::Serialize)]
struct EvalFeatureBody<'a> {
    schema: &'a str,
    template: &'a str,
    brief: &'a str,
    aux: &'a FeaturesAux,
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

impl Dispatcher {
    /// tick の段 `eval_trials`: 予約枠の中で `queued` の試行を起動する。起動した本数を返す。
    ///
    /// 本番の `dispatch_ready` の**後**に呼ぶ（本番を先に配る。付記 D3）。
    pub(super) fn tick_eval_trials(&mut self) -> usize {
        self.reconcile_eval_trials();
        if self.config.eval.reserved_runs == 0 {
            return 0;
        }
        let mut started = 0usize;
        loop {
            let reserved = self.config.eval.reserved_runs;
            let in_flight = match self.eval_in_flight() {
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!(error = %e, "eval trials: could not count the runs in flight");
                    return started;
                }
            };
            if in_flight >= reserved {
                return started;
            }
            let trial = match self.store.eval_trial_claim_next() {
                Ok(Some(t)) => t,
                Ok(None) => return started,
                Err(e) => {
                    tracing::warn!(error = %e, "eval trials: claim failed");
                    return started;
                }
            };
            // 枠・account・quota のどれかで起こせないなら `queued` に戻して次の tick へ（attempts を消費しない）。
            if let Err(why) = self.eval_admit(&trial, in_flight) {
                if let Err(e) = self.store.eval_trial_unclaim(&trial.trial_id) {
                    tracing::warn!(trial_id = %trial.trial_id, error = %e, "eval trials: unclaim failed");
                }
                tracing::debug!(trial_id = %trial.trial_id, reason = why.as_str(), "eval trial deferred");
                return started;
            }
            match self.start_eval_trial(trial) {
                Ok(()) => started += 1,
                Err(e) => {
                    // 起動そのものが失敗した試行は `error`（`queued` に戻すと毎 tick 回ってしまう）。
                    tracing::warn!(error = %e, "eval trial failed to start");
                    return started;
                }
            }
        }
    }

    /// Recover rows left behind by a daemon restart and finish any durable scoring work.
    fn reconcile_eval_trials(&mut self) {
        let rows = match self.store.eval_trials_list(None, None) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "eval trials: recovery scan failed");
                return;
            }
        };
        for trial in rows {
            match eval_recovery_action(
                trial.status,
                trial
                    .run_id
                    .as_deref()
                    .is_some_and(|id| self.eval_runs.contains_key(id)),
            ) {
                EvalRecoveryAction::Score => {
                    let run = trial
                        .run_id
                        .as_deref()
                        .and_then(|id| self.store.run_index_get(id).ok().flatten());
                    let wall_secs = trial
                        .started_at
                        .as_deref()
                        .and_then(|s| {
                            time::OffsetDateTime::parse(
                                s,
                                &time::format_description::well_known::Rfc3339,
                            )
                            .ok()
                        })
                        .map(|start| (OffsetDateTime::now_utc() - start).as_seconds_f64());
                    self.score_eval_trial(&trial, run.and_then(|r| r.usage), wall_secs);
                }
                EvalRecoveryAction::Error => {
                    self.fail_and_cleanup_eval_trial(
                        &trial,
                        "daemon recovery: run disappeared while trial was running",
                    );
                }
                EvalRecoveryAction::None => {}
            }
        }
    }

    fn fail_and_cleanup_eval_trial(&self, trial: &EvalTrial, reason: &str) {
        let _ = self.store.eval_trial_finish(
            &trial.trial_id,
            EvalTrialStatus::Error,
            None,
            false,
            Some(reason),
        );
        self.remove_eval_worktree(trial);
    }

    fn remove_eval_worktree(&self, trial: &EvalTrial) {
        if let Some(path) = trial.worktree_path.as_deref() {
            let _ = std::process::Command::new("git")
                .args(["-C", &trial.repo, "worktree", "remove", "--force", path])
                .output();
            let _ = std::fs::remove_dir_all(path);
        }
    }

    fn score_eval_trial(
        &self,
        trial: &EvalTrial,
        usage: Option<task_core::Usage>,
        wall_secs: Option<f64>,
    ) {
        let Some(decision_id) = trial.decision_id.as_deref() else {
            self.fail_and_cleanup_eval_trial(trial, "scoring trial has no route decision");
            return;
        };
        let Some(cwd) = trial.worktree_path.as_deref() else {
            self.fail_and_cleanup_eval_trial(trial, "scoring trial has no worktree");
            return;
        };
        let scored = run_eval_check(
            cwd,
            &trial.check_cmd,
            Duration::from_secs(trial.check_timeout_secs as u64),
        );
        if scored.exit.is_none() && !scored.timed_out {
            self.fail_and_cleanup_eval_trial(trial, "hidden check could not be started");
            return;
        }
        let passed = !scored.timed_out && scored.exit == Some(0);
        let labels = serde_json::json!({
            "passed": passed, "check_exit": scored.exit, "timed_out": scored.timed_out,
            "wall_secs": wall_secs, "usage": usage, "usd_list": usage.and_then(|u| u.cost_usd),
            "run_status": "done"
        });
        let record = task_core::store::StoredRouteOutcome {
            decision_id: decision_id.to_owned(),
            outcome: task_core::route_record::RouteOutcome {
                schema: task_core::route_record::ROUTE_OUTCOME_SCHEMA.into(),
                labels,
            },
            recorded_at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        };
        let status = if passed {
            EvalTrialStatus::Passed
        } else {
            EvalTrialStatus::Failed
        };
        if let Err(e) = self.store.insert_route_outcome(&record) {
            self.fail_and_cleanup_eval_trial(
                trial,
                &format!("could not record route outcome: {e}"),
            );
            return;
        }
        if let Err(e) = self.store.eval_trial_finish(
            &trial.trial_id,
            status,
            scored.exit,
            scored.timed_out,
            None,
        ) {
            tracing::warn!(trial_id = %trial.trial_id, error = %e, "eval trials: could not finish scoring");
        }
        self.remove_eval_worktree(trial);
    }

    /// 動いている評価 run の本数（`running` と `scoring`。daemon 再起動をまたいで `eval_trials` から数える）。
    fn eval_in_flight(&self) -> Result<usize, DispatchError> {
        let mut n = 0;
        for status in [EvalTrialStatus::Running, EvalTrialStatus::Scoring] {
            let rows = self
                .store
                .eval_trials_list(None, Some(status))
                .map_err(DispatchError::Store)?;
            n += rows.len();
        }
        Ok(n)
    }

    /// 予約枠・account の同時実行・quota 上限（本番と同じ規則）を見る。
    fn eval_admit(&self, trial: &EvalTrial, eval_in_flight: usize) -> Result<(), EvalDefer> {
        let route = self
            .eval_route(trial)
            .ok_or(crate::capacity::EvalDefer::AccountFull)?;
        let account_key = route.account_adapter.map(|a| (a, route.account.clone()));
        let state = self.source_state_of(
            route.provider.as_str(),
            account_key.as_ref(),
            Instant::now(),
        );
        let account_in_use = state.in_use.unwrap_or(0) as usize
            + self.eval_account_in_use(route.account_adapter, &route.account);
        let account_limit = self.eval_account_limit(route.account_adapter, &route.account);
        // quota 上限（quota_book の見積り）は本番と同じ判定（`enforce_quota_verdict`）。circuit が
        // 開いている source も起こさない。
        let quota_ok = !state.circuit_open
            && super::routing_enforce::enforce_quota_verdict(
                task_core::Tier::Standard,
                state.quota_remaining,
            )
            .is_ok();
        crate::capacity::eval_admission(
            self.config.eval.reserved_runs,
            eval_in_flight,
            account_in_use,
            account_limit,
            !state.cooldown && quota_ok,
        )
    }

    /// `account_in_use`（本番の `running` + `reviewing`）に載っていない評価 run の分を足す。
    fn eval_account_in_use(&self, account_adapter: Option<AccountAdapter>, account: &str) -> usize {
        self.eval_runs
            .values()
            .filter(|e| e.account == account && e.account_adapter == account_adapter)
            .count()
    }

    /// account の同時実行の上限（本番と同じ解決: account の上書き → 既定。CoS の +1 は無い）。
    fn eval_account_limit(&self, account_adapter: Option<AccountAdapter>, account: &str) -> usize {
        let stored = account_adapter.and_then(|a| {
            self.account_limits
                .as_ref()
                .and_then(|m| m.get(&(a.as_str().to_owned(), account.to_owned())))
                .copied()
        });
        let (limit, _source) =
            crate::capacity::effective_account_limit(stored, self.config.max_concurrency);
        crate::capacity::account_run_limit(limit, false)
    }

    /// harness（adapter id）と LLM source から経路を引く。評価は routing を**しない**
    /// （source×model は trial を作ったときに人が決めている）ので、候補は 1 件だけ。
    fn eval_route(&self, trial: &EvalTrial) -> Option<EvalRoute> {
        let harness = trial.harness_id.as_str();
        let mut found: Option<(ProviderId, Arc<dyn WorkerAdapter>)> = None;
        for (provider, adapter) in &self.adapters {
            if adapter.id() != harness {
                continue;
            }
            // 行が LLM source を持つなら、trial の source と一致する行だけを使う。
            if let Some(source) = self.provider_llm_sources.get(provider)
                && source != &trial.source_id
            {
                continue;
            }
            found = Some((provider.clone(), Arc::clone(adapter)));
            break;
        }
        let (provider, base) = found?;
        let account_adapter = self.pool_adapter_of(&provider, base.id());
        let adapter = match account_adapter {
            // プールの行: source id をアカウント id として資格情報を載せる。
            Some(a) => self.adapter_for_account(&base, a, &trial.source_id)?,
            None => Arc::clone(&base),
        };
        let adapter = adapter.with_model(&trial.model_id)?;
        Some(EvalRoute {
            provider,
            adapter,
            account: trial.source_id.clone(),
            account_adapter,
            model: trial.model_id.clone(),
        })
    }

    /// 評価 run を 1 本起こす（worktree → 記録 → adapter の起動）。
    fn start_eval_trial(&mut self, trial: EvalTrial) -> Result<(), String> {
        let route = self.eval_route(&trial).ok_or_else(|| {
            format!(
                "no provider serves harness {} / source {}",
                trial.harness_id, trial.source_id
            )
        })?;
        let harness_version = self.eval_harness_version(&trial.harness_id);
        let run_id = ulid::Ulid::new().to_string();
        let base = self
            .config
            .workspace_root
            .join("eval")
            .join(trial.trial_id.as_str());
        let worktree = base.join("repo");
        let artifacts = base.join("artifacts");
        self.eval_prepare_worktree(&trial, &worktree, &artifacts)
            .map_err(|e| {
                self.fail_eval_trial(&trial, &format!("worktree: {e}"));
                e
            })?;
        // 記録（route_features / route_options / route_decisions / runs）。失敗したら run を起こさない。
        let decision_id =
            match self.record_eval_decision(&trial, &run_id, harness_version.as_deref()) {
                Ok(id) => id,
                Err(e) => {
                    self.fail_eval_trial(&trial, &format!("route record: {e}"));
                    return Err(e.to_string());
                }
            };
        let session_id = ulid::Ulid::new().to_string();
        if let Err(e) = self.record_eval_run_row(&trial, &run_id, &session_id, &route) {
            self.fail_eval_trial(&trial, &format!("runs index: {e}"));
            return Err(e.to_string());
        }
        let req = self.eval_run_request(&trial, &worktree, &artifacts, &session_id);
        if let Err(e) = self.store.eval_trial_mark_started(
            &trial.trial_id,
            &run_id,
            &decision_id,
            harness_version.as_deref().unwrap_or("unknown"),
            &worktree.to_string_lossy(),
        ) {
            self.fail_eval_trial(&trial, &format!("mark started: {e}"));
            return Err(e.to_string());
        }
        let limits = RunLimits {
            wall_clock: Duration::from_secs(EVAL_MAX_WALL_SECS),
            idle_timeout: self.config.idle_timeout,
            kill_grace: self.config.kill_grace,
        };
        let adapter = Arc::clone(&route.adapter);
        let tx = self.tx.clone();
        let completion_run_id = run_id.clone();
        let handle = tokio::spawn(async move {
            // 評価 run は worker の run 経路（`run_worker`）を通らない: tasks 行を持たないので、
            // `store.get(task_id)` から組み立てる文脈（担当・組織・計画）が存在しない。adapter を
            // 直接呼ぶ（CoS の対話 run と同じ形）。
            let result = adapter
                .run(
                    req,
                    &completion_run_id,
                    limits,
                    &task_worker::adapter::NullSink,
                )
                .await;
            let _ = tx.send(Completion::EvalWorker {
                run_id: completion_run_id,
                result,
            });
        });
        self.eval_runs.insert(
            run_id,
            EvalRunEntry {
                trial_id: trial.trial_id.clone(),
                account: route.account.clone(),
                account_adapter: route.account_adapter,
                handle,
            },
        );
        tracing::info!(
            trial_id = %trial.trial_id,
            problem_id = %trial.problem_id,
            harness = %trial.harness_id,
            source = %trial.source_id,
            model = %route.model,
            "started an evaluation run"
        );
        Ok(())
    }

    /// 評価 run 用の新しい worktree（`git worktree add --detach <path> <base_commit>`）。
    fn eval_prepare_worktree(
        &self,
        trial: &EvalTrial,
        worktree: &Path,
        artifacts: &Path,
    ) -> Result<(), String> {
        if let Err(e) = std::fs::create_dir_all(artifacts) {
            return Err(format!("artifacts dir: {e}"));
        }
        let out = std::process::Command::new("git")
            .arg("worktree")
            .arg("add")
            .arg("--detach")
            .arg(worktree)
            .arg(trial.base_commit.as_str())
            .current_dir(trial.repo.as_str())
            .output()
            .map_err(|e| format!("git worktree add: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git worktree add failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    }

    /// harness の版（実行 file の `--version`）。生の出力と major の両方を返す
    /// （記録するのは生の出力。major は `task_core::routing_evidence::harness_version_major`）。
    fn eval_harness_version(&self, harness_id: &str) -> Option<String> {
        if let Some(probe) = &self.eval_version_probe {
            let raw = probe(harness_id)?;
            return Some(raw);
        }
        let raw = probe_harness_version(harness_id)?;
        // major が出せるなら生の出力の後ろに添える（付記 D4: 両方を記録する）。
        let major = task_core::routing_evidence::harness_version_major(harness_id, &raw);
        if major.is_empty() || major == raw.trim() {
            Some(raw)
        } else {
            Some(format!("{raw} (major {major})"))
        }
    }

    /// worker に渡す `RunRequest`（objective だけ。acceptance は空 = 採点の command が見えない）。
    fn eval_run_request(
        &self,
        trial: &EvalTrial,
        worktree: &Path,
        artifacts: &Path,
        session_id: &str,
    ) -> RunRequest {
        let now = OffsetDateTime::now_utc();
        let task_id = trial
            .batch_id
            .parse::<TaskId>()
            .unwrap_or_else(|_| TaskId::new());
        let task = Task {
            requirements: Default::default(),
            tree: None,
            paused_at: None,
            routing: None,
            mode: Default::default(),
            skills: Vec::new(),
            repos: Vec::new(),
            id: task_id,
            parent_id: None,
            kind: TaskKind::Execute,
            title: format!("routing-eval {} #{}", trial.problem_id, trial.trial_no),
            objective: trial.objective.clone(),
            // 採点の command は worker に見せない（付記 D4）。
            acceptance: Vec::new(),
            inputs: Vec::new(),
            depends_on: Vec::new(),
            status: Status::Running,
            priority: 0,
            worker_hint: WorkerHint {
                tier: Tier::Standard,
                adapter: Some(trial.harness_id.clone()),
            },
            workspace: WorkspaceSpec::Local {
                path: worktree.to_path_buf(),
                mode: None,
            },
            budget: Budget {
                max_turns: EVAL_MAX_TURNS as u32,
                max_wall_secs: EVAL_MAX_WALL_SECS,
                max_retries: 0,
            },
            attempts: 0,
            lease: None,
            created_at: now,
            updated_at: now,
            role: None,
            genre: None,
            aggregate: false,
            project_id: None,
            milestone_id: None,
            assignee: None,
            conversation: None,
            labels: Vec::new(),
            category: Default::default(),
        };
        RunRequest {
            protocol: task_worker::protocol::PROTOCOL_VERSION,
            task,
            workspace: worktree.to_path_buf(),
            work_dir: None,
            artifacts_dir: artifacts.to_path_buf(),
            context: RunContext {
                // 新 session（resume しない・checkpoint なし）。
                session: Some(task_worker::protocol::SessionHandle {
                    adapter: trial.harness_id.clone(),
                    session_id: session_id.to_owned(),
                    resume: false,
                }),
                comments_enabled: false,
                ..RunContext::default()
            },
            cargo_target_dir: None,
        }
    }

    /// `route_features` / `route_options` / `route_decisions`（stage = "eval"）を書く。decision_id を返す。
    fn record_eval_decision(
        &self,
        trial: &EvalTrial,
        run_id: &str,
        harness_version: Option<&str>,
    ) -> Result<String, String> {
        let created_at = rfc3339(OffsetDateTime::now_utc());
        let brief = format!(
            "eval problem {} trial {} (manifest {})",
            trial.problem_id, trial.trial_no, trial.manifest_sha
        );
        let aux = FeaturesAux {
            genre: Some(EVAL_SUBJECT_KIND.to_owned()),
            harness: Some(trial.harness_id.clone()),
            ..FeaturesAux::default()
        };
        let features = TaskFeatures {
            schema: TASK_FEATURES_SCHEMA.to_owned(),
            template: EVAL_FEATURES_TEMPLATE.to_owned(),
            brief: brief.clone(),
            hash: sha256(
                &serde_json::to_vec(&EvalFeatureBody {
                    schema: TASK_FEATURES_SCHEMA,
                    template: EVAL_FEATURES_TEMPLATE,
                    brief: &brief,
                    aux: &aux,
                })
                .map_err(|e| e.to_string())?,
            ),
            aux,
        };
        self.store
            .insert_route_features(
                &features.hash,
                EVAL_SUBJECT_KIND,
                &trial.problem_id,
                &features,
                &created_at,
            )
            .map_err(|e| e.to_string())?;
        let candidate = task_core::route_record::Candidate {
            source_id: trial.source_id.clone(),
            model_id: trial.model_id.clone(),
            harness_id: trial.harness_id.clone(),
            label: format!(
                "{} × {} on {}",
                trial.source_id, trial.model_id, trial.harness_id
            ),
        };
        let options = RouteOptions {
            schema: ROUTE_OPTIONS_SCHEMA.to_owned(),
            builder: EVAL_OPTIONS_BUILDER.to_owned(),
            question: brief.clone(),
            options: vec![RouteOption {
                option_id: "eval-trial".to_owned(),
                text: candidate.label.clone(),
                binds: Some(RouteOptionBinds {
                    source_id: candidate.source_id.clone(),
                    model_id: candidate.model_id.clone(),
                    harness_id: candidate.harness_id.clone(),
                }),
            }],
        };
        let options_hash = sha256(&serde_json::to_vec(&options).map_err(|e| e.to_string())?);
        self.store
            .insert_route_options(&options_hash, &options, &created_at)
            .map_err(|e| e.to_string())?;
        let decision_id = ulid::Ulid::new().to_string();
        self.store
            .insert_route_decision(&StoredRouteDecision {
                decision_id: decision_id.clone(),
                subject_kind: EVAL_SUBJECT_KIND.to_owned(),
                subject_id: trial.problem_id.clone(),
                run_id: Some(run_id.to_owned()),
                features_hash: features.hash.clone(),
                options_hash,
                input: serde_json::json!({
                    "manifest_sha": trial.manifest_sha,
                    "trial_no": trial.trial_no,
                    "candidate": {
                        "source_id": trial.source_id,
                        "model_id": trial.model_id,
                        "harness_id": trial.harness_id,
                    },
                }),
                output: serde_json::json!({
                    "selected": {
                        "source_id": trial.source_id,
                        "model_id": trial.model_id,
                        "harness_id": trial.harness_id,
                        "harness_version": harness_version,
                    },
                    "action": "execute",
                }),
                judge: serde_json::json!({"kind": "fixed", "by": "routing-eval"}),
                stage: EVAL_STAGE.to_owned(),
                created_at: created_at.clone(),
            })
            .map_err(|e| e.to_string())?;
        Ok(decision_id)
    }

    /// `runs` 索引の 1 行（role = "eval"、task_id = batch_id、work_unit_id NULL）。
    fn record_eval_run_row(
        &self,
        trial: &EvalTrial,
        run_id: &str,
        session_id: &str,
        route: &EvalRoute,
    ) -> Result<(), DispatchError> {
        self.store.run_index_start(task_core::RunRow {
            run_id: run_id.to_owned(),
            task_id: trial.batch_id.clone(),
            work_unit_id: None,
            role: task_core::RunIndexRole::Eval,
            seq: 1,
            status: task_core::RunIndexStatus::Running,
            adapter: Some(route.adapter.id().to_owned()),
            model: Some(route.model.clone()),
            account: Some(route.account.clone()),
            session_id: Some(session_id.to_owned()),
            checkpoint: None,
            usage: None,
            metrics: None,
            started_at: rfc3339(OffsetDateTime::now_utc()),
            finished_at: None,
        })?;
        Ok(())
    }

    /// 起動に失敗した試行を `error` にする（`running` からだけ。worktree は採点側が消す）。
    fn fail_eval_trial(&self, trial: &EvalTrial, why: &str) {
        if let Err(e) = self.store.eval_trial_finish(
            &trial.trial_id,
            EvalTrialStatus::Error,
            None,
            false,
            Some(why),
        ) {
            tracing::warn!(trial_id = %trial.trial_id, error = %e, "eval trials: could not record the failure");
        }
    }

    /// 評価 run の完了（`Completion::EvalWorker`）。run を閉じて試行を `scoring` にする。
    /// 採点（hidden check・`route_outcomes`・worktree の削除）は `trial-score` の段が拾う。
    pub(super) fn on_eval_worker_finished(
        &mut self,
        run_id: String,
        result: Result<RunOutcome, AdapterError>,
    ) {
        let Some(entry) = self.eval_runs.remove(&run_id) else {
            tracing::warn!(%run_id, "eval run finished but is not tracked here");
            return;
        };
        let (index_status, usage) = match &result {
            Ok(outcome) => match &outcome.terminal {
                Terminal::Done { usage, .. } => (task_core::RunIndexStatus::Completed, *usage),
                Terminal::Yielded { usage, .. } => (task_core::RunIndexStatus::Yielded, *usage),
                Terminal::BudgetExhausted { usage, .. } => {
                    (task_core::RunIndexStatus::BudgetExhausted, *usage)
                }
                _ => (task_core::RunIndexStatus::Completed, None),
            },
            Err(_) => (task_core::RunIndexStatus::Completed, None),
        };
        if let Err(e) = self.store.run_index_finish(
            &run_id,
            index_status,
            None,
            usage,
            None,
            OffsetDateTime::now_utc(),
        ) {
            tracing::warn!(%run_id, error = %e, "eval trials: could not finish the runs index row");
        }
        // 起動・実行の異常は `error`（採点に進まない）。終端まで走ったものだけ `scoring` へ。
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(e) => {
                if let Ok(Some(trial)) = self.store.eval_trial_get(&entry.trial_id) {
                    self.fail_and_cleanup_eval_trial(&trial, &format!("adapter: {e}"));
                }
                return;
            }
        };
        let trial = match self.store.eval_trial_get(&entry.trial_id) {
            Ok(Some(trial)) => trial,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "eval trials: trial lookup failed after worker completion");
                return;
            }
        };
        match &outcome.terminal {
            Terminal::Done { usage, .. } => {
                if let Err(e) = self.store.eval_trial_mark_scoring(&entry.trial_id) {
                    tracing::warn!(trial_id = %entry.trial_id, error = %e, "eval trials: could not mark scoring");
                    self.fail_and_cleanup_eval_trial(
                        &trial,
                        &format!("could not mark scoring: {e}"),
                    );
                } else {
                    let wall_secs = trial
                        .started_at
                        .as_deref()
                        .and_then(|s| {
                            time::OffsetDateTime::parse(
                                s,
                                &time::format_description::well_known::Rfc3339,
                            )
                            .ok()
                        })
                        .map(|start| (OffsetDateTime::now_utc() - start).as_seconds_f64());
                    self.score_eval_trial(&trial, *usage, wall_secs);
                }
            }
            other => {
                self.fail_and_cleanup_eval_trial(
                    &trial,
                    &format!("run ended as {}", terminal_kind(other)),
                );
            }
        }
    }

    /// 手元の評価 run を止める（draining・shutdown）。
    pub(super) fn abort_eval_runs(&mut self) {
        for (run_id, entry) in self.eval_runs.drain() {
            tracing::info!(%run_id, trial_id = %entry.trial_id, "aborting an evaluation run");
            entry.handle.abort();
            self.store
                .eval_trial_finish(
                    &entry.trial_id,
                    EvalTrialStatus::Error,
                    None,
                    false,
                    Some("dispatcher stopped the run"),
                )
                .ok();
            if let Err(e) = self.store.run_index_finish(
                &run_id,
                task_core::RunIndexStatus::Completed,
                None,
                None,
                None,
                OffsetDateTime::now_utc(),
            ) {
                tracing::warn!(%run_id, error = %e, "eval trials: could not close the runs index row");
            }
        }
    }

    /// このインスタンスが評価 run（run id）を手元に持っているか。
    pub(super) fn holds_eval_run(&self, run_id: &str) -> bool {
        self.eval_runs.contains_key(run_id)
    }

    /// 評価 run の harness の版の測り方を差し替える（試験用。既定は `probe_harness_version`）。
    pub fn set_eval_version_probe<F>(&mut self, probe: F)
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        self.eval_version_probe = Some(Arc::new(probe));
    }
}

struct EvalCheckResult {
    exit: Option<i32>,
    timed_out: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvalRecoveryAction {
    None,
    Score,
    Error,
}

fn eval_recovery_action(status: EvalTrialStatus, tracked: bool) -> EvalRecoveryAction {
    match status {
        EvalTrialStatus::Scoring => EvalRecoveryAction::Score,
        EvalTrialStatus::Running if !tracked => EvalRecoveryAction::Error,
        _ => EvalRecoveryAction::None,
    }
}

fn run_eval_check(cwd: &str, command: &str, timeout: Duration) -> EvalCheckResult {
    let mut child = match std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            return EvalCheckResult {
                exit: None,
                timed_out: false,
            };
        }
    };
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return EvalCheckResult {
                    exit: status.code(),
                    timed_out: false,
                };
            }
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return EvalCheckResult {
                    exit: None,
                    timed_out: true,
                };
            }
        }
    }
}

/// 終端の短い名前（`eval_trials.error` の理由に書く）。
fn terminal_kind(terminal: &Terminal) -> &'static str {
    match terminal {
        Terminal::Done { .. } => "done",
        Terminal::Error { .. } => "error",
        Terminal::Question { .. } => "question",
        Terminal::Yielded { .. } => "yielded",
        Terminal::BudgetExhausted { .. } => "budget_exhausted",
        Terminal::Waiting { .. } => "waiting",
    }
}

/// harness の実行 file を `--version` で叩いて、出力の先頭行を返す（`VERSION_TIMEOUT` で打ち切る）。
///
/// 実行 file は `harness_id` そのもの、無ければ既知の別名（`claude-code` → `claude`）を PATH から引く。
pub fn probe_harness_version(harness_id: &str) -> Option<String> {
    let candidates: [&str; 2] = [harness_id, alias_of(harness_id)];
    for program in candidates.into_iter().filter(|p| !p.is_empty()) {
        if let Some(out) = run_version(program) {
            return Some(out);
        }
    }
    None
}

fn alias_of(harness_id: &str) -> &'static str {
    match harness_id {
        "claude-code" => "claude",
        _ => "",
    }
}

fn run_version(program: &str) -> Option<String> {
    let mut child = std::process::Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + VERSION_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    };
    if !status.success() {
        return None;
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next().unwrap_or("").trim().to_owned();
    if first.is_empty() {
        return None;
    }
    Some(first.chars().take(VERSION_MAX_CHARS).collect())
}

#[cfg(test)]
mod score_tests {
    use super::{EvalRecoveryAction, eval_recovery_action, run_eval_check};
    use std::{path::Path, time::Duration};
    use task_core::routing_eval_trial::EvalTrialStatus;

    #[test]
    fn eval_trial_check_passes_and_fails_by_exit_code() {
        let cwd = Path::new(env!("CARGO_MANIFEST_DIR"))
            .to_str()
            .expect("manifest path");
        assert_eq!(
            run_eval_check(cwd, "exit 0", Duration::from_secs(1)).exit,
            Some(0)
        );
        assert_eq!(
            run_eval_check(cwd, "exit 7", Duration::from_secs(1)).exit,
            Some(7)
        );
    }

    #[test]
    fn eval_trial_check_timeout_kills_a_sleeping_stub() {
        let cwd = Path::new(env!("CARGO_MANIFEST_DIR"))
            .to_str()
            .expect("manifest path");
        let result = run_eval_check(cwd, "exec sleep 5", Duration::from_millis(30));
        assert!(result.timed_out);
        assert_eq!(result.exit, None);
    }

    #[test]
    fn eval_trial_recovery_marks_untracked_running_as_error_and_rescores() {
        assert_eq!(
            eval_recovery_action(EvalTrialStatus::Running, false),
            EvalRecoveryAction::Error
        );
        assert_eq!(
            eval_recovery_action(EvalTrialStatus::Scoring, false),
            EvalRecoveryAction::Score
        );
        assert_eq!(
            eval_recovery_action(EvalTrialStatus::Running, true),
            EvalRecoveryAction::None
        );
    }
}
