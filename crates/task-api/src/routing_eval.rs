//! 評価 run の試行の受け口（ADR 2026-10-11-llm-source-model-routing-evaluation 付記 2026-10-12
//! 「評価 run の受け口（eval-dispatch）」契約 2）。
//!
//! - `POST /routing-eval/trials` — 試行をまとめて投入する（1 回の POST = 1 batch）→ 201
//!   `{batch_id, trials:[{trial_id, problem_id, trial_no, status}]}`。同じ試行
//!   （manifest_sha・problem_id・trial_no・source・model・harness）の再投入は既存行を返す（冪等）。
//!   空 `trials`・未知の source/model/harness・`repo` の不在は 400 `eval_trial_invalid`。
//! - `GET /routing-eval/trials?batch_id=&status=` — 一覧。**`check_cmd` は返さない**（採点専用で
//!   worker にも API の利用者にも見せない）。
//! - `POST /routing-eval/trials/{trial_id}/cancel` — 200（`queued`/`running` だけ）。それ以外は 409
//!   `eval_trial_not_cancellable`、無ければ 404 `eval_trial_not_found`。
//!
//! 認可は他の書き込み API と同じ（middleware が変更系メソッドに bearer を要求する）。
//! ハンドラは HTTP と検証だけで、行の書き込み・状態遷移は `task_core` の store（`eval_trials`）が行う。
//! 評価 run そのものは dispatcher の `eval_trials` 段が起こす（この API は run を直接起こさない）。

use std::collections::BTreeSet;
use std::path::Path;

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use task_core::routing_eval_trial::{EvalTrial, EvalTrialSpec, EvalTrialStatus};
use task_core::{LlmSourceRef, SqliteStore, StoreError, known_harness_ids};

use crate::handlers::{ApiResult, Params, json_response, no_query, read_json};
use crate::problem::{ApiProblem, store_problem};
use crate::query::QueryParams;
use crate::state::ApiState;

/// `check.timeout_secs` を省略したときの値（WU の check と同じくらいの余裕）。
pub const DEFAULT_CHECK_TIMEOUT_SECS: u32 = 900;

/// 1 試行の `check.timeout_secs` の上限（1 日）。これ以上は投入ミスとして 400。
pub const MAX_CHECK_TIMEOUT_SECS: u32 = 86_400;

/// 1 回の POST で受け付ける試行の数（body の上限 1 MiB とは別に、batch を正しく保つための上限）。
pub const MAX_TRIALS_PER_BATCH: usize = 256;

/// 採点用の check（`check_cmd` は worker に渡さない。応答にも出さない）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvalTrialCheckBody {
    /// 採点側が worktree で `sh -c` する command。
    pub cmd: String,
    /// 省略時は `DEFAULT_CHECK_TIMEOUT_SECS`。
    #[serde(default = "default_check_timeout_secs")]
    pub timeout_secs: u32,
}

fn default_check_timeout_secs() -> u32 {
    DEFAULT_CHECK_TIMEOUT_SECS
}

/// `POST /routing-eval/trials` の 1 試行。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvalTrialCreateItem {
    /// manifest の問題 id（`celeris.routing-eval-manifest/1`）。
    pub problem_id: String,
    /// 同じ問題の何回目か（1 始まり。candidates の `k`）。
    pub trial_no: u32,
    /// LLM source（例 `claude-oauth`）。daemon の catalog に無ければ 400。
    pub source_id: String,
    /// model（例 `claude-opus-5-5`）。
    pub model_id: String,
    /// harness（例 `claude-code`）。この harness で固定して起こす。
    pub harness_id: String,
    /// worktree を作る元の repo（daemon から見える path）。無ければ 400。
    pub repo: String,
    /// worktree の detach 先 commit。
    pub base_commit: String,
    /// worker に見せる本文（acceptance の command は含めない）。
    pub objective: String,
    pub check: EvalTrialCheckBody,
}

/// `POST /routing-eval/trials` の本文。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvalTrialsCreateBody {
    /// 凍結した manifest の sha256。
    pub manifest_sha: String,
    pub trials: Vec<EvalTrialCreateItem>,
}

/// 投入の応答の 1 試行（`check` は返さない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct EvalTrialSummary {
    pub trial_id: String,
    pub problem_id: String,
    pub trial_no: u32,
    /// `queued` など（`task_core::routing_eval_trial::EvalTrialStatus` の文字列）。
    pub status: String,
}

/// `POST /routing-eval/trials` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct EvalTrialsCreated {
    /// この batch の id。既存の試行が混ざる再投入では、最初の試行が属する batch を返す。
    pub batch_id: String,
    pub trials: Vec<EvalTrialSummary>,
}

/// `eval_trials` 1 行の利用者向け view。`check_cmd`（採点専用）と `objective`（worker 専用）は含めない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct EvalTrialView {
    pub trial_id: String,
    pub batch_id: String,
    pub manifest_sha: String,
    pub problem_id: String,
    pub trial_no: u32,
    pub source_id: String,
    pub model_id: String,
    pub harness_id: String,
    /// 起動時に埋まる（`--version` の生出力）。まだなら `null`。
    pub harness_version: Option<String>,
    pub repo: String,
    pub base_commit: String,
    pub check_timeout_secs: u32,
    pub status: String,
    pub run_id: Option<String>,
    pub decision_id: Option<String>,
    pub worktree_path: Option<String>,
    pub check_exit: Option<i32>,
    pub timed_out: bool,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// `GET /routing-eval/trials` の応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct EvalTrialList {
    pub trials: Vec<EvalTrialView>,
}

impl EvalTrialView {
    fn of(trial: &EvalTrial) -> Self {
        Self {
            trial_id: trial.trial_id.clone(),
            batch_id: trial.batch_id.clone(),
            manifest_sha: trial.manifest_sha.clone(),
            problem_id: trial.problem_id.clone(),
            trial_no: trial.trial_no,
            source_id: trial.source_id.clone(),
            model_id: trial.model_id.clone(),
            harness_id: trial.harness_id.clone(),
            harness_version: trial.harness_version.clone(),
            repo: trial.repo.clone(),
            base_commit: trial.base_commit.clone(),
            check_timeout_secs: trial.check_timeout_secs,
            status: trial.status.as_str().to_string(),
            run_id: trial.run_id.clone(),
            decision_id: trial.decision_id.clone(),
            worktree_path: trial.worktree_path.clone(),
            check_exit: trial.check_exit,
            timed_out: trial.timed_out,
            error: trial.error.clone(),
            created_at: trial.created_at.clone(),
            started_at: trial.started_at.clone(),
            finished_at: trial.finished_at.clone(),
        }
    }
}

fn summary_of(trial: &EvalTrial) -> EvalTrialSummary {
    EvalTrialSummary {
        trial_id: trial.trial_id.clone(),
        problem_id: trial.problem_id.clone(),
        trial_no: trial.trial_no,
        status: trial.status.as_str().to_string(),
    }
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route(
            "/api/v1/routing-eval/trials",
            get(list_trials).post(create_trials),
        )
        .route(
            "/api/v1/routing-eval/trials/{trial_id}/cancel",
            post(cancel_trial),
        )
}

// ---- 検証（純粋関数） ----

/// source id の比べ方（`claude-oauth` と `claude_oauth` は同じ）。
fn normalize_source(id: &str) -> String {
    id.trim().to_ascii_lowercase().replace('-', "_")
}

/// daemon が知っている source/model/harness の id（設定・routing catalog・`[[genres]]` から作る写し）。
/// 集合に無い id は未知として拒否する。設定されていない source/model/harness を黙って許可しない。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct KnownRouteIds {
    /// 正規化した source id（`normalize_source` 済み）。
    pub sources: BTreeSet<String>,
    pub models: BTreeSet<String>,
    pub harnesses: BTreeSet<String>,
}

impl KnownRouteIds {
    fn knows_source(&self, id: &str) -> bool {
        self.sources.contains(&normalize_source(id))
    }

    fn knows_model(&self, id: &str) -> bool {
        self.models.contains(id)
    }

    fn knows_harness(&self, id: &str) -> bool {
        self.harnesses.contains(id)
    }
}

/// `ApiState`（設定の写し・routing catalog・`[[genres]]`）から照合用の id 集合を作る。
pub(crate) fn known_route_ids(state: &ApiState) -> KnownRouteIds {
    let mut known = KnownRouteIds::default();
    // source: 組み込みの LLM source と、設定の provider が解決した source。
    for source in [
        LlmSourceRef::Celeris,
        LlmSourceRef::ClaudeOauth,
        LlmSourceRef::CodexOauth,
        LlmSourceRef::OpencodeGo,
    ] {
        known.sources.insert(normalize_source(source.as_str()));
        known.sources.insert(normalize_source(&source.to_key()));
    }
    for provider in &state.inner.config_view.providers {
        if let Some(resolved) = &provider.llm_source {
            known
                .sources
                .insert(normalize_source(resolved.source.as_str()));
            known
                .sources
                .insert(normalize_source(&resolved.source.to_key()));
        }
        // harness: provider の adapter はそのまま harness id。
        if !provider.adapter.trim().is_empty() {
            known.harnesses.insert(provider.adapter.clone());
        }
        // model: 実効モデルと lane ごとの束縛。
        if let Some(model) = provider.model.as_deref() {
            known.models.insert(model.to_string());
        }
        for binding in provider.tier_models.values() {
            if !binding.name.trim().is_empty() {
                known.models.insert(binding.name.clone());
            }
            if let Some(model_id) = binding.model_id.as_deref() {
                known.models.insert(model_id.to_string());
            }
        }
    }
    // harness: `[[genres]]`（= ハーネスの射影）と組み込み。genres が空なら空（検証しない）。
    for id in known_harness_ids(&state.inner.genres) {
        known.harnesses.insert(id);
    }
    // catalog（daemon が llm-proxy の設定を持つときだけある）。
    if let Some(reader) = state.inner.routing_catalog.as_ref() {
        let view = reader.view();
        for model in &view.models {
            known.models.insert(model.id.clone());
        }
        for deployment in &view.deployments {
            known
                .sources
                .insert(normalize_source(&deployment.source_ref));
            known.models.insert(deployment.model_profile_id.clone());
            known.models.insert(deployment.upstream_model.clone());
        }
    }
    known
}

fn invalid(detail: impl Into<String>) -> ApiProblem {
    ApiProblem::new(StatusCode::BAD_REQUEST, "eval_trial_invalid", detail)
}

fn require_text<'a>(errors: &mut Vec<String>, field: &str, value: &'a str) -> Option<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(format!("{field} must not be empty"));
        return None;
    }
    Some(trimmed)
}

/// 投入の検証（純粋。DB も FS 以外の I/O もしない。`repo` の実在だけ見る）。
/// 誤りは 1 件で止めずに集めて 400 の `detail` に並べる。
fn validate(
    body: &EvalTrialsCreateBody,
    known: &KnownRouteIds,
) -> Result<Vec<EvalTrialSpec>, ApiProblem> {
    let mut errors: Vec<String> = Vec::new();
    let manifest_sha = require_text(&mut errors, "manifest_sha", &body.manifest_sha).unwrap_or("");
    if body.trials.is_empty() {
        errors.push("trials must not be empty".to_string());
    }
    if body.trials.len() > MAX_TRIALS_PER_BATCH {
        errors.push(format!(
            "trials must have at most {MAX_TRIALS_PER_BATCH} items (got {})",
            body.trials.len()
        ));
    }
    let mut seen: BTreeSet<(String, u32, String, String, String)> = BTreeSet::new();
    let mut specs = Vec::with_capacity(body.trials.len());
    for (index, item) in body.trials.iter().enumerate() {
        let at = format!("trials[{index}]");
        require_text(&mut errors, &format!("{at}.problem_id"), &item.problem_id);
        require_text(&mut errors, &format!("{at}.source_id"), &item.source_id);
        require_text(&mut errors, &format!("{at}.model_id"), &item.model_id);
        require_text(&mut errors, &format!("{at}.harness_id"), &item.harness_id);
        require_text(&mut errors, &format!("{at}.base_commit"), &item.base_commit);
        require_text(&mut errors, &format!("{at}.objective"), &item.objective);
        require_text(&mut errors, &format!("{at}.check.cmd"), &item.check.cmd);
        if !known.knows_source(&item.source_id) {
            errors.push(format!("{at}.source_id: unknown source {:?}", item.source_id));
        }
        if !known.knows_model(&item.model_id) {
            errors.push(format!("{at}.model_id: unknown model {:?}", item.model_id));
        }
        if !known.knows_harness(&item.harness_id) {
            errors.push(format!(
                "{at}.harness_id: unknown harness {:?}",
                item.harness_id
            ));
        }
        let repo = item.repo.trim();
        if repo.is_empty() {
            errors.push(format!("{at}.repo must not be empty"));
        } else if !Path::new(repo).is_dir() {
            errors.push(format!("{at}.repo: no such repository directory: {repo}"));
        }
        if item.check.timeout_secs == 0 || item.check.timeout_secs > MAX_CHECK_TIMEOUT_SECS {
            errors.push(format!(
                "{at}.check.timeout_secs must be 1..={MAX_CHECK_TIMEOUT_SECS} (got {})",
                item.check.timeout_secs
            ));
        }
        let key = (
            item.problem_id.clone(),
            item.trial_no,
            item.source_id.clone(),
            item.model_id.clone(),
            item.harness_id.clone(),
        );
        if !seen.insert(key) {
            errors.push(format!(
                "{at}: duplicate trial (problem_id, trial_no, source_id, model_id, harness_id) \
                 in the same batch"
            ));
        }
        specs.push(EvalTrialSpec {
            manifest_sha: manifest_sha.to_string(),
            problem_id: item.problem_id.clone(),
            trial_no: item.trial_no,
            source_id: item.source_id.clone(),
            model_id: item.model_id.clone(),
            harness_id: item.harness_id.clone(),
            repo: repo.to_string(),
            base_commit: item.base_commit.clone(),
            objective: item.objective.clone(),
            check_cmd: item.check.cmd.clone(),
            check_timeout_secs: item.check.timeout_secs,
        });
    }
    if errors.is_empty() {
        return Ok(specs);
    }
    Err(invalid(errors.join("; ")))
}

/// 取り消しの失敗 → HTTP。行が無ければ 404、終端・`scoring` は 409。
fn cancel_problem(trial_id: &str, err: StoreError) -> ApiProblem {
    match err {
        StoreError::Invalid(detail) if detail.contains("not found") => ApiProblem::new(
            StatusCode::NOT_FOUND,
            "eval_trial_not_found",
            format!("eval trial {trial_id} not found"),
        ),
        StoreError::Invalid(detail) => ApiProblem::new(
            StatusCode::CONFLICT,
            "eval_trial_not_cancellable",
            format!("eval trial {trial_id}: {detail}"),
        ),
        other => store_problem(other),
    }
}

// ---- ハンドラ ----

async fn create_trials(
    State(state): State<ApiState>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    let payload: EvalTrialsCreateBody = read_json(body, false).await?;
    let known = known_route_ids(&state);
    let specs = validate(&payload, &known)?;
    let created = state
        .blocking(move |store| {
            let batch_id = ulid::Ulid::new().to_string();
            let rows = store
                .insert_eval_trials(&batch_id, &specs)
                .map_err(store_problem)?;
            // 冪等: 既存の試行は元の batch_id のまま返るので、応答もそれに合わせる。
            let batch_id = rows
                .first()
                .map(|row| row.batch_id.clone())
                .unwrap_or(batch_id);
            Ok(EvalTrialsCreated {
                batch_id,
                trials: rows.iter().map(summary_of).collect(),
            })
        })
        .await?;
    tracing::info!(
        op = "routing_eval_trials_create",
        batch_id = %created.batch_id,
        trials = created.trials.len(),
        "routing eval trials accepted"
    );
    Ok(json_response(StatusCode::CREATED, &created))
}

async fn list_trials(State(state): State<ApiState>, RawQuery(raw): RawQuery) -> ApiResult {
    let query = QueryParams::parse(raw.as_deref(), &["batch_id", "status"])?;
    let batch_id = query.single("batch_id")?.map(str::to_string);
    let status = match query.single("status")? {
        Some(value) => Some(EvalTrialStatus::parse(value).ok_or_else(|| {
            invalid(format!("status must be one of queued|running|scoring|passed|failed|error|cancelled (got {value:?})"))
        })?,
        ),
        None => None,
    };
    let list = state
        .blocking(move |store| {
            let rows = store
                .eval_trials_list(batch_id.as_deref(), status)
                .map_err(store_problem)?;
            Ok(EvalTrialList {
                trials: rows.iter().map(EvalTrialView::of).collect(),
            })
        })
        .await?;
    Ok(json_response(StatusCode::OK, &list))
}

async fn cancel_trial(
    State(state): State<ApiState>,
    Params(trial_id): Params<String>,
    RawQuery(raw): RawQuery,
    body: Body,
) -> ApiResult {
    no_query(&raw)?;
    // 本文は取らない（空でも `{}` でもよい）。未知の欄は 400。
    let _: serde_json::Value = read_json(body, true).await?;
    let id = trial_id.clone();
    let cancelled = state
        .blocking(move |store: &SqliteStore| {
            store
                .eval_trial_cancel(&id)
                .map(|trial| EvalTrialView::of(&trial))
                .map_err(|err| cancel_problem(&id, err))
        })
        .await?;
    tracing::info!(op = "routing_eval_trial_cancel", trial_id = %trial_id, "routing eval trial cancelled");
    Ok(json_response(StatusCode::OK, &cancelled))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(problem: &str, trial_no: u32, repo: &str) -> EvalTrialCreateItem {
        EvalTrialCreateItem {
            problem_id: problem.into(),
            trial_no,
            source_id: "claude-oauth".into(),
            model_id: "claude-opus-5-5".into(),
            harness_id: "claude-code".into(),
            repo: repo.into(),
            base_commit: "abc123".into(),
            objective: "fix the bug".into(),
            check: EvalTrialCheckBody {
                cmd: "sh notes/check.sh".into(),
                timeout_secs: DEFAULT_CHECK_TIMEOUT_SECS,
            },
        }
    }

    fn known() -> KnownRouteIds {
        KnownRouteIds {
            sources: BTreeSet::from([normalize_source("claude_oauth")]),
            models: BTreeSet::from(["claude-opus-5-5".to_string()]),
            harnesses: BTreeSet::from(["claude-code".to_string()]),
        }
    }

    #[test]
    fn validate_accepts_a_known_route_and_keeps_the_check_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = EvalTrialsCreateBody {
            manifest_sha: "sha-1".into(),
            trials: vec![item("p1", 1, dir.path().to_str().expect("utf-8 path"))],
        };
        let specs = validate(&body, &known()).expect("valid");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].check_cmd, "sh notes/check.sh");
        assert_eq!(specs[0].manifest_sha, "sha-1");
        assert_eq!(specs[0].check_timeout_secs, DEFAULT_CHECK_TIMEOUT_SECS);
    }

    #[test]
    fn validate_rejects_empty_batch_unknown_route_and_missing_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().to_str().expect("utf-8 path");
        let empty = EvalTrialsCreateBody {
            manifest_sha: "sha-1".into(),
            trials: vec![],
        };
        let err = validate(&empty, &known()).expect_err("empty batch");
        assert_eq!(err.status(), StatusCode::BAD_REQUEST);
        assert_eq!(err.code(), "eval_trial_invalid");
        assert!(err.detail().contains("trials must not be empty"), "{err:?}");

        let mut bad = item("p1", 1, repo);
        bad.source_id = "no-such-source".into();
        bad.model_id = "no-such-model".into();
        bad.harness_id = "no-such-harness".into();
        let body = EvalTrialsCreateBody {
            manifest_sha: "sha-1".into(),
            trials: vec![bad],
        };
        let err = validate(&body, &known()).expect_err("unknown route");
        let detail = err.detail().to_string();
        assert!(detail.contains("unknown source"), "{detail}");
        assert!(detail.contains("unknown model"), "{detail}");
        assert!(detail.contains("unknown harness"), "{detail}");

        let missing = EvalTrialsCreateBody {
            manifest_sha: "sha-1".into(),
            trials: vec![item("p1", 1, "/nonexistent/repo/for-eval-trial")],
        };
        let err = validate(&missing, &known()).expect_err("missing repo");
        assert!(
            err.detail().contains("no such repository directory"),
            "{err:?}"
        );
    }

    #[test]
    fn validate_rejects_duplicates_and_bad_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path().to_str().expect("utf-8 path");
        let mut zero = item("p1", 1, repo);
        zero.check.timeout_secs = 0;
        let body = EvalTrialsCreateBody {
            manifest_sha: "sha-1".into(),
            trials: vec![item("p1", 1, repo), zero],
        };
        let err = validate(&body, &known()).expect_err("duplicate + timeout");
        let detail = err.detail().to_string();
        assert!(detail.contains("duplicate trial"), "{detail}");
        assert!(detail.contains("timeout_secs"), "{detail}");
    }

    #[test]
    fn empty_known_ids_reject_routes() {
        // daemon が catalog を持たない場合も、未知の選択肢を fail closed にする。
        let dir = tempfile::tempdir().expect("tempdir");
        let body = EvalTrialsCreateBody {
            manifest_sha: "sha-1".into(),
            trials: vec![item("p1", 1, dir.path().to_str().expect("utf-8 path"))],
        };
        let err = validate(&body, &KnownRouteIds::default()).expect_err("unknown route");
        assert!(err.detail().contains("unknown source"), "{err:?}");
        assert!(err.detail().contains("unknown model"), "{err:?}");
        assert!(err.detail().contains("unknown harness"), "{err:?}");
    }

    #[test]
    fn cancel_problem_splits_not_found_and_conflict() {
        let missing = cancel_problem(
            "01J",
            StoreError::Invalid("eval trial 01J not found".into()),
        );
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(missing.code(), "eval_trial_not_found");
        let conflict = cancel_problem(
            "01J",
            StoreError::Invalid("eval trial 01J: cannot cancel from status passed".into()),
        );
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(conflict.code(), "eval_trial_not_cancellable");
    }
}
