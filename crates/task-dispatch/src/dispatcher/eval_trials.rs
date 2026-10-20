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
    ///
    /// account・quota で起こせない試行に当たっても tick を終えない。その試行の source を
    /// `full_sources` に入れて `queued` に戻し、同じ tick で後ろの別 source の試行を見る（先頭詰まりを
    /// 避ける。source の中の作成順は保つ）。tick の中で埋まった source が空くことは無いので、同じ試行を
    /// 2 度 claim しない。終わりは「見る queued が尽きた」か「`reserved_runs` に達した」。
    ///
    /// 自分の instance id（`set_orphan_takeover`）を持たない dispatcher（`--mode verify`・設定を渡さない
    /// 組み立て）は回復も claim もせず 0 を返す。写しの DB の試行・lease・`runs` にも、DB に書かれた
    /// 本番の絶対パスにも触れない（ADR 2026-10-14-eval-verify-isolation D1）。
    pub(super) fn tick_eval_trials(&mut self) -> usize {
        if !self.owns_eval_trials() {
            return 0;
        }
        self.reconcile_eval_trials();
        if self.config.eval.reserved_runs == 0 {
            return 0;
        }
        let Some(instance_id) = self.orphan_takeover.as_ref().map(|t| t.instance_id.clone()) else {
            return 0;
        };
        let mut started = 0usize;
        let mut full_sources: Vec<String> = Vec::new();
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
            let trial = match self
                .store
                .eval_trial_claim_next(&instance_id, &full_sources)
            {
                Ok(Some(t)) => t,
                Ok(None) => return started,
                Err(e) => {
                    tracing::warn!(error = %e, "eval trials: claim failed");
                    return started;
                }
            };
            // account・quota で起こせないなら `queued` に戻し（attempts を消費しない）、この tick ではその
            // source を飛ばして次の試行を見る。宣言に無い経路は待っても起こせないので `error` にして次の試行を見る。
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
                if matches!(why, EvalDefer::Disabled | EvalDefer::ReservedFull) {
                    return started;
                }
                full_sources.push(trial.source_id.clone());
                continue;
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

    /// 再起動・live 昇格の後に残った行の回復と、採点の続き（ADR 2026-10-14-eval-verify-isolation D3.3・D3.4）。
    ///
    /// `scoring` は採点する。`running` で手元の `eval_runs` に無い行は、記録された持ち主（`instance_id`）が
    /// 生きていないときだけ `error` にする（[`eval_running_recovery`]）。生きている他の instance（live 昇格の
    /// draining の旧 instance）が走らせている試行は、作業ツリーも target も残してそのまま待つ。`error` に
    /// した試行の `runs` 行は同じ処理で `cancelled` に閉じる。`daemon_instances` が読めない tick は
    /// `running` の行に触れない。
    fn reconcile_eval_trials(&mut self) {
        let rows = match self.store.eval_trials_list(None, None) {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "eval trials: recovery scan failed");
                return;
            }
        };
        let now = self.now_utc();
        let mut instances: Option<Option<Vec<task_core::DaemonInstance>>> = None;
        for trial in rows {
            match trial.status {
                EvalTrialStatus::Scoring => {
                    let run = trial
                        .run_id
                        .as_deref()
                        .and_then(|id| self.store.run_index_get(id).ok().flatten());
                    let wall_secs = parse_rfc3339(trial.started_at.as_deref())
                        .map(|start| (OffsetDateTime::now_utc() - start).as_seconds_f64());
                    self.score_eval_trial(&trial, run.and_then(|r| r.usage), wall_secs);
                }
                EvalTrialStatus::Running
                    if !trial
                        .run_id
                        .as_deref()
                        .is_some_and(|id| self.eval_runs.contains_key(id)) =>
                {
                    let Some(takeover) = self.orphan_takeover.clone() else {
                        return;
                    };
                    let rows = instances.get_or_insert_with(|| match self.store.instance_list() {
                        Ok(rows) => Some(rows),
                        Err(e) => {
                            tracing::warn!(error = %e, "eval trials: could not read daemon_instances; leaving running trials alone");
                            None
                        }
                    });
                    let Some(rows) = rows.as_deref() else {
                        continue;
                    };
                    let deadline = Duration::from_secs(
                        EVAL_MAX_WALL_SECS + u64::from(trial.check_timeout_secs),
                    ) + self.config.lease_grace;
                    let Some(why) = eval_running_recovery(
                        &EvalOwnerFacts {
                            owner: trial.instance_id.as_deref(),
                            self_id: &takeover.instance_id,
                            instances: rows,
                            freshness: takeover.freshness,
                            alive: takeover.pid_alive.as_ref(),
                        },
                        parse_rfc3339(trial.started_at.as_deref()),
                        deadline,
                        now,
                    ) else {
                        continue;
                    };
                    tracing::warn!(trial_id = %trial.trial_id, owner = ?trial.instance_id, why, "eval trials: recovering a running trial as error");
                    self.recover_eval_trial_as_error(&trial, why);
                }
                _ => {}
            }
        }
    }

    /// 回復で `running` の試行を `error`（`interrupted: eval recovery: <why>`）にし、作業ツリーと target を消し、
    /// `run_id` の `runs` 行を `cancelled` で閉じる（ADR 2026-10-14-eval-verify-isolation D3.5）。
    fn recover_eval_trial_as_error(&self, trial: &EvalTrial, why: &str) {
        self.fail_and_cleanup_eval_trial(trial, &format!("interrupted: eval recovery: {why}"));
        let Some(run_id) = trial.run_id.as_deref() else {
            return;
        };
        if let Err(e) = self.store.run_index_finish(
            run_id,
            task_core::RunIndexStatus::Cancelled,
            None,
            None,
            None,
            self.now_utc(),
        ) {
            tracing::warn!(%run_id, error = %e, "eval trials: could not close the runs index row of a recovered trial");
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
    /// 作業ツリーは `<workspace_root>/eval/<trial_id>/` の配下（実 path で比較）のときだけ消し、
    /// `git -C <repo>` も同じときだけ走らせる。外なら消さずに warn だけ（`eval_trials.error` は
    /// 書き換えない。ADR 2026-10-14-eval-verify-isolation D2）。
    fn remove_eval_trial_files(&self, trial: &EvalTrial) {
        if !self.owns_eval_trials() {
            return;
        }
        if let Some(path) = trial.worktree_path.as_deref() {
            // ADR 2026-10-14-local-machine-settings D5: 試行は用意した時点の実効の root に置かれる。
            // 今の実効の root と固定なしの root（CLI か設定ファイル）の `eval/<trial_id>/` を順に見る。
            let roots = &self.config.workspace_roots;
            let mut candidates: Vec<std::path::PathBuf> = Vec::new();
            if let Ok(effective) = roots.effective_root_in(self.store.as_ref()) {
                candidates.push(effective.value);
            }
            if !candidates.iter().any(|r| r == roots.unpinned_root()) {
                candidates.push(roots.unpinned_root().to_path_buf());
            }
            let mut within = Err(String::from("no workspace root"));
            for root in &candidates {
                let trial_dir = root.join("eval").join(trial.trial_id.as_str());
                within = eval_path_within(&trial_dir, &trial.trial_id, Path::new(path));
                if within.is_ok() {
                    break;
                }
            }
            match within {
                Ok(real) => {
                    let _ = std::process::Command::new("git")
                        .arg("-C")
                        .arg(&trial.repo)
                        .args(["worktree", "remove", "--force"])
                        .arg(&real)
                        .output();
                    match std::fs::remove_dir_all(&real) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            tracing::warn!(trial_id = %trial.trial_id, worktree = %real.display(), error = %e, "eval trials: could not remove the work tree");
                        }
                    }
                }
                Err(why) => {
                    tracing::warn!(trial_id = %trial.trial_id, worktree = %path, %why, "eval trials: refused to remove a work tree outside the trial's root");
                }
            }
        }
        self.remove_eval_target(&trial.trial_id);
    }

    /// 試行の scratch target を消し、lease に `released_at` を書く（lease の記録は `scratch_gc` が後で片付ける）。
    /// scratch が今は無効でも、設定の pool の下に残っていれば消す。消すのは `pool.target_dir(owner)`
    /// （`targets/` の直下）だけ。pool の root が空・相対なら何もしない（ADR 2026-10-14-eval-verify-isolation D2）。
    fn remove_eval_target(&self, trial_id: &str) {
        if !self.owns_eval_trials() {
            return;
        }
        let Ok(owner) = eval_target_owner(trial_id) else {
            return;
        };
        let pool = self.config.scratch.pool();
        if pool.root().as_os_str().is_empty() || !pool.root().is_absolute() {
            return;
        }
        if let Err(e) = task_worker::scratch::release(&pool, &owner) {
            tracing::warn!(%trial_id, %owner, error = %e, "eval trials: could not release the scratch lease");
        }
        let target = pool.target_dir(&owner);
        let real = match eval_path_within(&pool.targets_dir(), "", &target) {
            Ok(real) => real,
            Err(why) => {
                if target.symlink_metadata().is_ok() {
                    tracing::warn!(%trial_id, target = %target.display(), %why, "eval trials: refused to remove a scratch target outside the pool");
                }
                return;
            }
        };
        match std::fs::remove_dir_all(&real) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(%trial_id, target = %real.display(), error = %e, "eval trials: could not remove the scratch target");
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
        // ADR 2026-10-14-local-machine-settings D5: 新しい試行は実効の root に置く（場所は
        // `worktree_path` に記録し、以後はそれを読む）。
        let root = self
            .config
            .workspace_roots
            .effective_root_in(self.store.as_ref())
            .map_err(|e| {
                let e = format!("workspace root: {e}");
                self.fail_eval_trial(&trial, &e);
                e
            })?
            .value;
        let base = root.join("eval").join(trial.trial_id.as_str());
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
                max_tokens: None,
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
    /// active の instance はそのまま採点（hidden check・`route_outcomes`・worktree の削除）する。draining の
    /// instance は採点せず `scoring` で残し、新しい active の `reconcile_eval_trials` が採点する
    /// （ADR 2026-10-14-eval-verify-isolation D3.3）。
    pub(super) fn on_eval_worker_finished(
        &mut self,
        run_id: String,
        result: Result<RunOutcome, AdapterError>,
    ) {
        let Some(entry) = self.eval_runs.remove(&run_id) else {
            tracing::warn!(%run_id, "eval run finished but is not tracked here");
            return;
        };
        if !self.owns_eval_trials() {
            tracing::warn!(%run_id, "eval run finished on a dispatcher without an instance id; not recording it");
            return;
        }
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
                } else if !self.accepting_new_work {
                    tracing::info!(trial_id = %entry.trial_id, %run_id, "draining: the evaluation run finished; leaving the scoring to the active instance");
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

    /// 手元の評価 run を止める（SIGTERM の shutdown と drain の打ち切り。live 昇格の drain は止めずに完了を待つ）。instance id の無い dispatcher は止めるだけで
    /// 試行・target・`runs` に書かない（ADR 2026-10-14-eval-verify-isolation D1）。
    pub(super) fn abort_eval_runs(&mut self) {
        let runs: Vec<_> = self.eval_runs.drain().collect();
        for (run_id, entry) in runs {
            tracing::info!(%run_id, trial_id = %entry.trial_id, "aborting an evaluation run");
            entry.handle.abort();
            if !self.owns_eval_trials() {
                continue;
            }
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

    /// 手元に無い `running` の評価 run の `runs` 行の照合（`reconcile_ownerless_runs` から。ADR
    /// 2026-10-14-eval-verify-isolation D3.5）。対応する試行（`eval_trials.run_id`）が無い・`running` でない
    /// （終端・`scoring`）、または試行の持ち主が生きていない（[`eval_run_row_orphaned`]）なら `cancelled` で閉じる。
    /// 持ち主が生きている試行（live 昇格の draining の旧 instance・自分）の行は残す（自分の試行の行は
    /// `reconcile_eval_trials` の回復が試行ごと閉じる）。
    pub(super) fn reconcile_ownerless_eval_run(
        &self,
        row: &task_core::RunRow,
        now: OffsetDateTime,
    ) {
        let Some(takeover) = self.orphan_takeover.as_ref() else {
            return;
        };
        let trial = match self.store.eval_trials_list(Some(&row.task_id), None) {
            Ok(trials) => trials
                .into_iter()
                .find(|t| t.run_id.as_deref() == Some(row.run_id.as_str())),
            Err(e) => {
                tracing::warn!(run_id = %row.run_id, error = %e, "eval trials: could not read the trial of a running eval runs row");
                return;
            }
        };
        let instances = match trial.as_ref().map(|t| t.status) {
            Some(EvalTrialStatus::Running) => match self.store.instance_list() {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::warn!(run_id = %row.run_id, error = %e, "eval trials: could not read daemon_instances; leaving the eval runs row alone");
                    return;
                }
            },
            _ => Vec::new(),
        };
        let Some(why) = eval_run_row_orphaned(
            trial.as_ref(),
            &EvalOwnerFacts {
                owner: trial.as_ref().and_then(|t| t.instance_id.as_deref()),
                self_id: &takeover.instance_id,
                instances: &instances,
                freshness: takeover.freshness,
                alive: takeover.pid_alive.as_ref(),
            },
            now,
        ) else {
            return;
        };
        tracing::warn!(run_id = %row.run_id, batch_id = %row.task_id, why, "closing an eval runs index row left running without an owner");
        if let Err(e) = self.store.run_index_finish(
            &row.run_id,
            task_core::RunIndexStatus::Cancelled,
            None,
            None,
            None,
            now,
        ) {
            tracing::warn!(run_id = %row.run_id, error = %e, "eval trials: could not close an ownerless eval runs row");
        }
    }

    /// 評価試行の起動・回復・後始末・書き込みをしてよいか: 自分の `daemon_instances` の行
    /// （`set_orphan_takeover` 済みの instance id）を持つときだけ。`DaemonMode` や config では判定しない
    /// （ADR 2026-10-14-eval-verify-isolation D1）。
    fn owns_eval_trials(&self) -> bool {
        self.orphan_takeover.is_some()
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

/// 回復の持ち主判定の材料（`daemon_instances` の行と自分の instance id）。
struct EvalOwnerFacts<'a> {
    /// 試行の `instance_id`（migration 0078 より前に起動した行は `None`）。
    owner: Option<&'a str>,
    self_id: &'a str,
    instances: &'a [task_core::DaemonInstance],
    freshness: Duration,
    alive: &'a dyn Fn(u32) -> bool,
}

/// 回復で `error` にする理由（`interrupted: eval recovery: <why>`）。
const EVAL_RECOVERY_LOST_HERE: &str = "this instance started the run but no longer holds it";
const EVAL_RECOVERY_OWNER_GONE: &str = "the instance that started the run is gone";
const EVAL_RECOVERY_NO_LIVE_INSTANCE: &str = "no live daemon instance can hold the run";
const EVAL_RECOVERY_DEADLINE: &str = "the run outlived its deadline";

/// 手元に無い `running` の試行を回復で `error` にするか（`Some(why)`）、持ち主に任せて待つか（`None`）。
/// ADR 2026-10-14-eval-verify-isolation D3.4:
///
/// - 持ち主が自分 → 手元に無い＝失われた run なので `error`。
/// - 持ち主が他の instance → その 1 instance が生きていなければ（[`crate::orphan::instance_alive`]）`error`。
/// - 持ち主の記録が無い（migration 前の行）→ 他に生きている instance が 1 つも無ければ
///   （[`crate::orphan::holder_gone`]）`error`。
/// - 持ち主が生きている（または記録が無く他に生きている instance がある）行も、`started_at + deadline`
///   （`EVAL_MAX_WALL_SECS + check_timeout_secs + lease_grace`）を過ぎたら `error`（閉じ忘れの上限）。
///   `started_at` が無い・読めない行は期限では閉じない。
fn eval_running_recovery(
    facts: &EvalOwnerFacts<'_>,
    started_at: Option<OffsetDateTime>,
    deadline: Duration,
    now: OffsetDateTime,
) -> Option<&'static str> {
    match facts.owner {
        Some(owner) if owner == facts.self_id => return Some(EVAL_RECOVERY_LOST_HERE),
        Some(owner) => {
            if !crate::orphan::instance_alive(
                facts.instances,
                owner,
                now,
                facts.freshness,
                facts.alive,
            ) {
                return Some(EVAL_RECOVERY_OWNER_GONE);
            }
        }
        None => {
            if crate::orphan::holder_gone(
                facts.instances,
                facts.self_id,
                now,
                facts.freshness,
                facts.alive,
            ) {
                return Some(EVAL_RECOVERY_NO_LIVE_INSTANCE);
            }
        }
    }
    let started_at = started_at?;
    let limit = time::Duration::try_from(deadline).unwrap_or(time::Duration::MAX);
    (now - started_at >= limit).then_some(EVAL_RECOVERY_DEADLINE)
}

/// 手元に無い `running` の評価 run の `runs` 行を閉じるか（`Some(why)`）、残すか（`None`）。
/// 試行が無い・`running` でない → 閉じる。`running` の試行は持ち主が生きていなければ閉じる
/// （記録された持ち主は [`crate::orphan::instance_alive`]、記録が無い行は [`crate::orphan::holder_gone`]）。
fn eval_run_row_orphaned(
    trial: Option<&EvalTrial>,
    facts: &EvalOwnerFacts<'_>,
    now: OffsetDateTime,
) -> Option<&'static str> {
    let Some(trial) = trial else {
        return Some("no evaluation trial holds the run");
    };
    if trial.status != EvalTrialStatus::Running {
        return Some("the evaluation trial is no longer running");
    }
    let gone = match facts.owner {
        Some(owner) => !crate::orphan::instance_alive(
            facts.instances,
            owner,
            now,
            facts.freshness,
            facts.alive,
        ),
        None => crate::orphan::holder_gone(
            facts.instances,
            facts.self_id,
            now,
            facts.freshness,
            facts.alive,
        ),
    };
    gone.then_some(EVAL_RECOVERY_OWNER_GONE)
}

fn parse_rfc3339(s: Option<&str>) -> Option<OffsetDateTime> {
    s.and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
}

/// `path` の実 path（`..`・symlink を解く。対象が無ければ親の実 path + 名前）が `root` の実 path の
/// 真下より深いところにあれば、その実 path を返す（ADR 2026-10-14-eval-verify-isolation D2）。
/// `leaf` が空でなければ `root` の最後の名前が `leaf` と一致しなければ拒む（`eval/<trial_id>` の dir 名）。
/// `root` が相対・無い・解けないときも拒む（消さない側に倒す）。
fn eval_path_within(root: &Path, leaf: &str, path: &Path) -> Result<PathBuf, String> {
    use std::path::Component;
    if !root.is_absolute() {
        return Err(format!("root {} is not absolute", root.display()));
    }
    if !leaf.is_empty() {
        let mut parts = Path::new(leaf).components();
        let single = matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none();
        if !single || root.file_name() != Some(std::ffi::OsStr::new(leaf)) {
            return Err(format!(
                "trial dir name {leaf:?} does not match {}",
                root.display()
            ));
        }
    }
    if !path.is_absolute() {
        return Err(format!("{} is not absolute", path.display()));
    }
    let real_root = root
        .canonicalize()
        .map_err(|e| format!("root {}: {e}", root.display()))?;
    let real = match path.canonicalize() {
        Ok(real) => real,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let (Some(parent), Some(Component::Normal(name))) =
                (path.parent(), path.components().next_back())
            else {
                return Err(format!("{} has no parent", path.display()));
            };
            parent
                .canonicalize()
                .map_err(|e| format!("parent of {}: {e}", path.display()))?
                .join(name)
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if real == real_root || !real.starts_with(&real_root) {
        return Err(format!(
            "{} (real {}) is not under {}",
            path.display(),
            real.display(),
            real_root.display()
        ));
    }
    Ok(real)
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
    use super::{
        EVAL_RECOVERY_DEADLINE, EVAL_RECOVERY_LOST_HERE, EVAL_RECOVERY_NO_LIVE_INSTANCE,
        EVAL_RECOVERY_OWNER_GONE, EvalOwnerFacts, eval_running_recovery, run_eval_check,
    };
    use std::{path::Path, time::Duration};
    use task_core::{DaemonInstance, InstanceRole};
    use time::OffsetDateTime;

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

    fn instance(
        id: &str,
        pid: u32,
        role: InstanceRole,
        heartbeat: OffsetDateTime,
    ) -> DaemonInstance {
        DaemonInstance {
            instance_id: id.into(),
            release: "dev".into(),
            pid,
            role,
            started_at: heartbeat,
            heartbeat_at: heartbeat,
            handoff_requested_at: None,
            drained_at: None,
        }
    }

    #[test]
    fn eval_running_recovery_errors_only_when_the_owner_is_gone() {
        let now = OffsetDateTime::now_utc();
        let fresh = Duration::from_secs(60);
        let deadline = Duration::from_secs(3600);
        let alive = |pid: u32| pid != 99;
        let rows = vec![
            instance("new", 1, InstanceRole::Active, now),
            instance("old", 2, InstanceRole::Draining, now),
            instance(
                "stale",
                3,
                InstanceRole::Draining,
                now - time::Duration::minutes(10),
            ),
            instance("dead", 99, InstanceRole::Draining, now),
        ];
        let decide =
            |owner: Option<&str>, rows: &[DaemonInstance], started: Option<OffsetDateTime>| {
                eval_running_recovery(
                    &EvalOwnerFacts {
                        owner,
                        self_id: "new",
                        instances: rows,
                        freshness: fresh,
                        alive: &alive,
                    },
                    started,
                    deadline,
                    now,
                )
            };
        let recent = Some(now - time::Duration::minutes(5));
        // 生きている他の持ち主（live 昇格の draining の旧 instance）の試行は待つ。
        assert_eq!(decide(Some("old"), &rows, recent), None);
        // 持ち主が自分なら、手元に無い＝失われた run。
        assert_eq!(
            decide(Some("new"), &rows, recent),
            Some(EVAL_RECOVERY_LOST_HERE)
        );
        // heartbeat が古い・pid が無い・行が無い持ち主は死んでいる。
        assert_eq!(
            decide(Some("stale"), &rows, recent),
            Some(EVAL_RECOVERY_OWNER_GONE)
        );
        assert_eq!(
            decide(Some("dead"), &rows, recent),
            Some(EVAL_RECOVERY_OWNER_GONE)
        );
        assert_eq!(
            decide(Some("missing"), &rows, recent),
            Some(EVAL_RECOVERY_OWNER_GONE)
        );
        // 記録の無い行は、他に生きている instance があれば待ち、無ければ error。
        assert_eq!(decide(None, &rows, recent), None);
        assert_eq!(
            decide(None, &rows[..1], recent),
            Some(EVAL_RECOVERY_NO_LIVE_INSTANCE)
        );
        // 生きている持ち主でも期限を過ぎたら error。started_at が無ければ期限では閉じない。
        let long_ago = Some(now - time::Duration::hours(2));
        assert_eq!(
            decide(Some("old"), &rows, long_ago),
            Some(EVAL_RECOVERY_DEADLINE)
        );
        assert_eq!(decide(None, &rows, long_ago), Some(EVAL_RECOVERY_DEADLINE));
        assert_eq!(decide(Some("old"), &rows, None), None);
    }
}
