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
//!
//! cargo の target は試行ごとの scratch（ADR 2026-10-14-eval-trial-scratch-target D1）: owner は
//! `agent-eval-<trial_id>`（試行どうしで共有しない）、run と hidden check の両方に同じ `CARGO_TARGET_DIR` を
//! 渡し、試行の終端（採点の後・失敗・回復・停止）で target を消して lease を解放する。scratch が無効なら
//! ルート fs に置かず試行を `error` にする。

use super::*;

use sha2::{Digest, Sha256};
use task_core::llm_route::{
    AccountId, Admission, Candidate, ExclusionReason, HarnessId, ModelId, SourceId, admit_source,
    constrain_for_harness, wire_model_id,
};
use task_core::route_record::{
    FeaturesAux, ROUTE_OPTIONS_SCHEMA, RouteOption, RouteOptionBinds, RouteOptions,
    TASK_FEATURES_SCHEMA, TaskFeatures,
};
use task_core::routing_eval_trial::{EvalTrial, EvalTrialStatus};
use task_core::store::StoredRouteDecision;
use task_core::{Budget, WorkerHint};
use task_worker::protocol::SelectedLlmRoute;

use super::route_select::{RoutePick, with_account_limits};
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
/// 試行の target の lease の TTL の余裕（run の壁時計 + hidden check の上限に足す）。daemon が落ちて
/// 後始末できなかった target は、この TTL が切れると `scratch_gc` が回収する（ADR-0075 D2 の外部 owner）。
const EVAL_TARGET_LEASE_MARGIN_SECS: u64 = 600;

/// 評価 run の経路（harness 固定・source×model 固定。account だけ本番と同じ admission で決める）。
struct EvalRoute {
    pick: RoutePick,
    adapter: Arc<dyn WorkerAdapter>,
    llm_route: SelectedLlmRoute,
}

/// 評価 run の経路を作れない理由。
enum EvalRouteError {
    /// 宣言に無い harness・source・model、または harness が話せない source（試行は `error`）。
    Unroutable(String),
    /// 宣言はあるが今は起こせない（account の同時実行・quota・cooldown。試行は `queued` のまま）。
    Deferred(ExclusionReason),
}

/// 起動した評価 run の追跡（プロセス内のみ。真実は `eval_trials` の行と `runs` 索引）。
pub(super) struct EvalRunEntry {
    trial_id: String,
    /// この run が押さえている `(source, account)`（評価の admission で本番の run と合算する）。
    route: (SourceId, AccountId),
    pub(super) handle: JoinHandle<()>,
}

/// id の比べ方（`claude-oauth` と `claude_oauth`、`claude-code` と `claude_code` は同じ。
/// 評価の manifest・候補表は `-`、設定の宣言は `_` で書く）。
fn normalize_id(id: &str) -> String {
    id.trim().to_ascii_lowercase().replace('-', "_")
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
            // 宣言に無い経路は待っても起こせないので `error` にして次の試行を見る。
            let admitted = match self.eval_admit(&trial, in_flight) {
                Ok(admitted) => admitted,
                Err(why) => {
                    self.fail_eval_trial(&trial, &why);
                    continue;
                }
            };
            if let Err(why) = admitted {
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
        self.remove_eval_trial_files(trial);
    }

    /// 試行の終端の後始末: 作業ツリーと scratch の target を消す（lease は解放する）。
    fn remove_eval_trial_files(&self, trial: &EvalTrial) {
        if let Some(path) = trial.worktree_path.as_deref() {
            let _ = std::process::Command::new("git")
                .args(["-C", &trial.repo, "worktree", "remove", "--force", path])
                .output();
            let _ = std::fs::remove_dir_all(path);
        }
        self.remove_eval_target(&trial.trial_id);
    }

    /// 試行の scratch target を消し、lease に `released_at` を書く（lease の記録は `scratch_gc` が後で片付ける）。
    /// scratch が今は無効でも、設定の pool の下に残っていれば消す。
    fn remove_eval_target(&self, trial_id: &str) {
        let Ok(owner) = eval_target_owner(trial_id) else {
            return;
        };
        let pool = self.config.scratch.pool();
        if pool.root().as_os_str().is_empty() {
            return;
        }
        if let Err(e) = task_worker::scratch::release(&pool, &owner) {
            tracing::warn!(%trial_id, %owner, error = %e, "eval trials: could not release the scratch lease");
        }
        let target = pool.target_dir(&owner);
        match std::fs::remove_dir_all(&target) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(%trial_id, target = %target.display(), error = %e, "eval trials: could not remove the scratch target");
            }
        }
    }

    /// 試行の cargo の env（`CARGO_TARGET_DIR` = `<scratch>/targets/agent-eval-<trial_id>/target` と
    /// `[scratch.cargo]`）。scratch が無効なら `Err`（ルート fs・作業ツリーの直下に target を置かない）。
    fn eval_cargo_env(&self, trial_id: &str) -> Result<task_worker::scratch::CargoEnv, String> {
        if !self.scratch_active() {
            return Err(
                "scratch is disabled; an evaluation trial needs a per-trial scratch CARGO_TARGET_DIR \
                 (ADR 2026-10-14-eval-trial-scratch-target D1)"
                    .to_owned(),
            );
        }
        let owner = eval_target_owner(trial_id)?;
        Ok(super::worker_task::scratch_cargo_env(
            &self.config.scratch,
            &owner,
        ))
    }

    /// 試行の target の lease を作る（adopt しない。seed は通常の run と同じ条件）。TTL は run の壁時計と
    /// hidden check の上限に余裕を足した値。`CARGO_TARGET_DIR` に渡す path を返す。
    fn eval_allocate_target(&self, trial: &EvalTrial, worktree: &Path) -> Result<PathBuf, String> {
        let owner = eval_target_owner(&trial.trial_id)?;
        let settings = &self.config.scratch;
        let pool = settings.pool();
        let no_distance = |_: &task_worker::scratch::AdoptCandidate| None;
        let req = task_worker::scratch::AllocateRequest {
            owner: &owner,
            repo_path: Path::new(&trial.repo),
            base_commit: Some(trial.base_commit.clone()),
            work_unit_key: None,
            checkout: None,
            candidates: &[],
            distance: &no_distance,
            adopt: false,
            max_distance: 0,
        };
        let rustc = || rustc_version(worktree);
        let seed = task_worker::scratch::SeedPolicy {
            enabled: settings.seed_reflink,
            cargo: Some(settings.cargo.clone()),
            rustc: Some(&rustc),
        };
        let ops = task_worker::scratch::SeedCopyOps::real();
        let allocation = task_worker::scratch::allocate_with_seed(&pool, &req, &seed, &ops)
            .map_err(|e| format!("scratch lease for {owner}: {e}"))?;
        let ttl =
            EVAL_MAX_WALL_SECS + trial.check_timeout_secs as u64 + EVAL_TARGET_LEASE_MARGIN_SECS;
        task_worker::scratch::set_ttl(&pool, &owner, Some(ttl))
            .map_err(|e| format!("scratch lease ttl for {owner}: {e}"))?;
        Ok(allocation.target_dir)
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
        // hidden check も run と同じ試行の target を使う（ADR 2026-10-14-eval-trial-scratch-target D1）。
        let env = match self.eval_cargo_env(&trial.trial_id) {
            Ok(env) => env,
            Err(why) => {
                self.fail_and_cleanup_eval_trial(trial, &format!("hidden check: {why}"));
                return;
            }
        };
        if let Ok(owner) = eval_target_owner(&trial.trial_id) {
            let _ = task_worker::scratch::touch(&self.config.scratch.pool(), &owner);
        }
        let scored = run_eval_check(
            cwd,
            &trial.check_cmd,
            &env.set,
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
        self.remove_eval_trial_files(trial);
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

    /// 予約枠・account の同時実行・quota 上限（本番と同じ規則）を見る。外側の `Err` は宣言に無い経路
    /// （試行を `error` にする理由）、内側の `Err` は今は起こせない理由（`queued` に戻す）。
    fn eval_admit(
        &mut self,
        trial: &EvalTrial,
        eval_in_flight: usize,
    ) -> Result<Result<(), EvalDefer>, String> {
        let reserved = self.config.eval.reserved_runs;
        // 枠の判定が先（`reserved_runs = 0`・枠満杯は account を見ずに返す）。
        if let Err(defer) = crate::capacity::eval_admission(reserved, eval_in_flight, 0, 1, true) {
            return Ok(Err(defer));
        }
        // account の同時実行・quota・cooldown は本番と同じ `admit_source` が決める。
        let (account_in_use, account_limit, quota_ok) = match self.eval_pick(trial) {
            Ok(_) => (0, 1, true),
            Err(EvalRouteError::Unroutable(why)) => return Err(why),
            Err(EvalRouteError::Deferred(
                ExclusionReason::SourceQuota
                | ExclusionReason::AccountQuota
                | ExclusionReason::Cooldown,
            )) => (0, 1, false),
            Err(EvalRouteError::Deferred(_)) => (1, 1, true),
        };
        Ok(crate::capacity::eval_admission(
            reserved,
            eval_in_flight,
            account_in_use,
            account_limit,
            quota_ok,
        ))
    }

    /// trial の harness・source・model を宣言（`[harnesses]`・`[llm_sources]`）から引き、account を
    /// 本番と同じ admission（`admit_source`。手元の評価 run の `(source, account)` も合算）で選ぶ。
    /// 評価は routing を**しない**（source×model は trial を作ったときに人が決めている）ので、候補は 1 件だけ。
    fn eval_pick(&mut self, trial: &EvalTrial) -> Result<RoutePick, EvalRouteError> {
        let registry = Arc::clone(&self.llm_route_registry);
        let harness = registry
            .harnesses
            .keys()
            .find(|h| normalize_id(h.as_ref()) == normalize_id(&trial.harness_id))
            .cloned()
            .ok_or_else(|| {
                EvalRouteError::Unroutable(format!("harness {} is not declared", trial.harness_id))
            })?;
        let (source_id, source) = registry
            .sources
            .iter()
            .find(|(id, _)| normalize_id(id.as_ref()) == normalize_id(&trial.source_id))
            .ok_or_else(|| {
                EvalRouteError::Unroutable(format!("source {} is not declared", trial.source_id))
            })?;
        let model = ModelId::from(trial.model_id.as_str());
        if !source.models.contains(&model) {
            return Err(EvalRouteError::Unroutable(format!(
                "source {} does not declare model {}",
                trial.source_id, trial.model_id
            )));
        }
        let candidate = Candidate {
            source: source_id.clone(),
            model,
            api_format: source.api_format.clone(),
        };
        let spec = registry.harnesses.get(&harness).ok_or_else(|| {
            EvalRouteError::Unroutable(format!("harness {} is not declared", trial.harness_id))
        })?;
        let (eligible, _) = constrain_for_harness(vec![candidate], spec, &registry, &harness);
        let Some(candidate) = eligible.into_iter().next() else {
            return Err(EvalRouteError::Unroutable(format!(
                "harness {} cannot talk to source {}",
                trial.harness_id, trial.source_id
            )));
        };
        let mut state = self.route_state(false);
        for (source, account) in self.eval_runs.values().map(|e| &e.route) {
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
        let limited = with_account_limits(&registry, &state);
        match admit_source(
            &candidate,
            &state.accounts,
            &state.quota,
            &state.reservations,
            &limited,
        ) {
            Admission::Admitted(owner) => Ok(RoutePick {
                wire_model_id: wire_model_id(&registry, &harness, &candidate.key()),
                harness,
                candidate,
                account: owner.account_id,
            }),
            Admission::Rejected(reason) => Err(EvalRouteError::Deferred(reason)),
        }
    }

    /// 選んだ経路の adapter（model・account の環境を載せたもの）と worker に渡す `SelectedLlmRoute`。
    fn eval_route(&mut self, trial: &EvalTrial) -> Result<EvalRoute, String> {
        let pick = match self.eval_pick(trial) {
            Ok(pick) => pick,
            Err(EvalRouteError::Unroutable(why)) => return Err(why),
            Err(EvalRouteError::Deferred(reason)) => {
                return Err(format!("route not admitted: {reason:?}"));
            }
        };
        let adapter = self.route_adapter(&pick).map_err(|e| e.to_string())?;
        let llm_route = self.route_launch_target(&pick).map_err(|e| e.to_string())?;
        Ok(EvalRoute {
            pick,
            adapter,
            llm_route,
        })
    }

    /// 評価 run を 1 本起こす（worktree → 記録 → adapter の起動）。
    fn start_eval_trial(&mut self, trial: EvalTrial) -> Result<(), String> {
        let route = self.eval_route(&trial).map_err(|e| {
            self.fail_eval_trial(&trial, &format!("route: {e}"));
            e
        })?;
        let harness_version = self.eval_harness_version(&trial.harness_id);
        let run_id = ulid::Ulid::new().to_string();
        let base = self
            .config
            .workspace_root
            .join("eval")
            .join(trial.trial_id.as_str());
        let worktree = base.join(EVAL_WORKTREE_DIR);
        let artifacts = base.join(EVAL_ARTIFACTS_DIR);
        // scratch が無効なら作業ツリーも作らずに `error`（target をルート fs に置かない）。
        let cargo_env = self.eval_cargo_env(&trial.trial_id).map_err(|e| {
            self.fail_eval_trial(&trial, &format!("cargo target: {e}"));
            e
        })?;
        self.eval_prepare_worktree(&trial, &worktree, &artifacts)
            .map_err(|e| {
                self.fail_eval_trial(&trial, &format!("worktree: {e}"));
                e
            })?;
        // ここから先の起動の失敗は作業ツリーと target を消す（`fail_eval_trial` だけでは残る）。
        let started_trial = EvalTrial {
            worktree_path: Some(worktree.to_string_lossy().into_owned()),
            ..trial.clone()
        };
        let target_dir = self.eval_allocate_target(&trial, &worktree).map_err(|e| {
            self.fail_and_cleanup_eval_trial(&started_trial, &format!("cargo target: {e}"));
            e
        })?;
        let Some(adapter) = route.adapter.with_env(&cargo_env.set) else {
            let why = format!(
                "cargo target: adapter {} cannot take CARGO_TARGET_DIR",
                route.adapter.id()
            );
            self.fail_and_cleanup_eval_trial(&started_trial, &why);
            return Err(why);
        };
        // 記録（route_features / route_options / route_decisions / runs）。失敗したら run を起こさない。
        let decision_id =
            match self.record_eval_decision(&trial, &run_id, harness_version.as_deref()) {
                Ok(id) => id,
                Err(e) => {
                    self.fail_and_cleanup_eval_trial(&started_trial, &format!("route record: {e}"));
                    return Err(e.to_string());
                }
            };
        let session_id = ulid::Ulid::new().to_string();
        if let Err(e) = self.record_eval_run_row(&trial, &run_id, &session_id, &route) {
            self.fail_and_cleanup_eval_trial(&started_trial, &format!("runs index: {e}"));
            return Err(e.to_string());
        }
        let req = self.eval_run_request(
            &trial,
            &base,
            &session_id,
            &route.pick.harness,
            route.llm_route.clone(),
            target_dir,
        );
        if let Err(e) = self.store.eval_trial_mark_started(
            &trial.trial_id,
            &run_id,
            &decision_id,
            harness_version.as_deref().unwrap_or("unknown"),
            &worktree.to_string_lossy(),
        ) {
            self.fail_and_cleanup_eval_trial(&started_trial, &format!("mark started: {e}"));
            return Err(e.to_string());
        }
        let limits = RunLimits {
            wall_clock: Duration::from_secs(EVAL_MAX_WALL_SECS),
            idle_timeout: self.config.idle_timeout,
            kill_grace: self.config.kill_grace,
        };
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
                route: (
                    route.pick.candidate.source.clone(),
                    route.pick.account.clone(),
                ),
                handle,
            },
        );
        tracing::info!(
            trial_id = %trial.trial_id,
            problem_id = %trial.problem_id,
            harness = %trial.harness_id,
            source = %trial.source_id,
            model = %route.pick.candidate.model,
            account = %route.pick.account,
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
    ///
    /// 置き場は通常の worker run と同じ形（ADR 2026-10-14-eval-trial-result-placement D1）:
    /// `workspace` = 試行の dir（`<data>/eval/<trial_id>`）、`work_dir` = その下の作業ツリー（cwd）、
    /// `artifacts_dir` = その下の `artifacts`。`artifacts_rel()` は絶対パスになり、prompt の示す
    /// result.json と adapter が読む場所が一致する。adapter の `run_dir` も作業ツリーの外にできる。
    /// `cargo_target_dir` は試行の scratch target（`eval_allocate_target`。adapter の env の
    /// `CARGO_TARGET_DIR` と同じ path。ADR 2026-10-14-eval-trial-scratch-target D1）。
    pub(super) fn eval_run_request(
        &self,
        trial: &EvalTrial,
        trial_dir: &Path,
        session_id: &str,
        harness: &HarnessId,
        llm_route: SelectedLlmRoute,
        cargo_target_dir: PathBuf,
    ) -> RunRequest {
        let worktree = trial_dir.join(EVAL_WORKTREE_DIR);
        let artifacts = trial_dir.join(EVAL_ARTIFACTS_DIR);
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
            harness_id: Some(harness.clone()),
            worker_hint: WorkerHint {
                tier: Tier::Standard,
            },
            workspace: WorkspaceSpec::Local {
                path: trial_dir.to_path_buf(),
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
            workspace: trial_dir.to_path_buf(),
            work_dir: Some(worktree),
            artifacts_dir: artifacts,
            context: RunContext {
                // 新 session（resume しない・checkpoint なし）。
                session: Some(task_worker::protocol::SessionHandle {
                    adapter: harness.to_string(),
                    session_id: session_id.to_owned(),
                    resume: false,
                }),
                comments_enabled: false,
                ..RunContext::default()
            },
            llm_route: Some(llm_route),
            cargo_target_dir: Some(cargo_target_dir),
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
            model: Some(route.pick.candidate.model.to_string()),
            account: Some(route.pick.account.to_string()),
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
        // runs 行は終端どおりの状態にし、usage は取れれば残す（ADR 2026-10-14-eval-trial-result-placement D3）。
        let (index_status, usage) = match &result {
            Ok(outcome) => eval_run_index_end(&outcome.terminal),
            Err(_) => (task_core::RunIndexStatus::HarnessError, None),
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
                let reason = format!("adapter: {e}");
                tracing::warn!(trial_id = %entry.trial_id, %run_id, kind = "adapter", %reason, "evaluation run failed");
                if let Ok(Some(trial)) = self.store.eval_trial_get(&entry.trial_id) {
                    self.fail_and_cleanup_eval_trial(&trial, &reason);
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
                // 終了理由の本文を残す（ADR 2026-10-14-eval-trial-result-placement D2）。
                let kind = terminal_kind(other);
                let reason = format!("run ended as {kind}: {}", terminal_reason(other));
                tracing::warn!(trial_id = %entry.trial_id, %run_id, kind, %reason, "evaluation run did not finish as done");
                self.fail_and_cleanup_eval_trial(&trial, &reason);
            }
        }
    }

    /// 手元の評価 run を止める（draining・shutdown）。
    pub(super) fn abort_eval_runs(&mut self) {
        let runs: Vec<_> = self.eval_runs.drain().collect();
        for (run_id, entry) in runs {
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
            // 停止で `error` にした試行は回復の段が拾わないので、ここで target を消す。
            self.remove_eval_target(&entry.trial_id);
            if let Err(e) = self.store.run_index_finish(
                &run_id,
                task_core::RunIndexStatus::Cancelled,
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

/// 試行の scratch target の owner（`agent-eval-<trial_id>`。試行どうしで共有しない）。外部 owner の形なので
/// `scratch_gc` は lease の `released_at` か TTL で回収する（DB の tasks 行を引かない）。
fn eval_target_owner(trial_id: &str) -> Result<task_worker::scratch::Owner, String> {
    task_worker::scratch::Owner::parse(&format!("agent-eval-{trial_id}"))
}

fn run_eval_check(
    cwd: &str,
    command: &str,
    env: &[(String, String)],
    timeout: Duration,
) -> EvalCheckResult {
    let mut child = match std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
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

/// 試行の dir（`<data>/eval/<trial_id>`）の下の作業ツリー（worker の cwd・採点の場所）。
const EVAL_WORKTREE_DIR: &str = "repo";
/// 試行の dir の下の成果物 dir（作業ツリーの外。ADR 2026-10-14-eval-trial-result-placement D1）。
const EVAL_ARTIFACTS_DIR: &str = "artifacts";

/// 終端の本文（`eval_trials.error` の `run ended as <kind>: <本文>` の後半）。
fn terminal_reason(terminal: &Terminal) -> String {
    match terminal {
        Terminal::Done { summary, .. } => summary.clone(),
        Terminal::Error { message, .. } | Terminal::BudgetExhausted { message, .. } => {
            message.clone()
        }
        Terminal::Question { text } => text.clone(),
        Terminal::Yielded { checkpoint, .. } => checkpoint.to_string(),
        Terminal::Waiting { request, .. } if !request.summary.is_empty() => request.summary.clone(),
        Terminal::Waiting { request, .. } => format!("cluster jobs {}", request.jobs.join(",")),
    }
}

/// 評価 run の `runs` 行の終了状態と usage（Error を completed にしない。usage は終端が持てば残す）。
fn eval_run_index_end(
    terminal: &Terminal,
) -> (task_core::RunIndexStatus, Option<task_core::Usage>) {
    use task_core::RunIndexStatus as S;
    match terminal {
        Terminal::Done { usage, .. } => (S::Completed, *usage),
        Terminal::Yielded { usage, .. } => (S::Yielded, *usage),
        Terminal::BudgetExhausted { usage, .. } => (S::BudgetExhausted, *usage),
        Terminal::Waiting { usage, .. } => (S::Waiting, *usage),
        Terminal::Question { .. } => (S::Question, None),
        Terminal::Error { .. } => (S::Failed, None),
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
            run_eval_check(cwd, "exit 0", &[], Duration::from_secs(1)).exit,
            Some(0)
        );
        assert_eq!(
            run_eval_check(cwd, "exit 7", &[], Duration::from_secs(1)).exit,
            Some(7)
        );
    }

    #[test]
    fn eval_trial_check_timeout_kills_a_sleeping_stub() {
        let cwd = Path::new(env!("CARGO_MANIFEST_DIR"))
            .to_str()
            .expect("manifest path");
        let result = run_eval_check(cwd, "exec sleep 5", &[], Duration::from_millis(30));
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
