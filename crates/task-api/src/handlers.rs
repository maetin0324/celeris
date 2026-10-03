//! ルーティングとハンドラ（`docs/gui/api.md` §2〜§3）。HTTP の写像だけを行い、判断は task-ops / ストアに任せる。

use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use futures_util::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use task_core::{OrgNode, ProjectId, SqliteStore, TaskStore};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::MAX_BODY_BYTES;
use crate::problem::{ApiProblem, store_problem};
use crate::query::QueryParams;
use crate::state::ApiState;
use crate::types::ValidationError;

mod accounts;
mod clusters;
mod org;
mod projects;
mod providers;
mod secrets;
mod system;
pub(crate) mod task_actions;
mod task_io;
pub(crate) mod tasks;

use accounts::{
    accounts, cancel_account_login, check_account, create_account, delete_account,
    start_account_login, submit_account_login_code,
};
use clusters::{
    cancel_cluster_connect, clusters, put_cluster_settings, start_cluster_connect,
    submit_cluster_connect_code,
};
use org::{create_org_node, delete_org_node, org_list, patch_org_node};
use projects::{
    create_milestone, create_project, patch_milestone, patch_project, project_detail, project_list,
};
use providers::{
    check_provider, create_provider, delete_provider, patch_provider, providers, reload,
};
use secrets::{delete_secret, put_secret, secrets_list};
use system::{config, create_plan, daemon, graph, health, inbox, metrics_scratch, replay, schema};
use task_actions::{
    accept, answer, approve, cancel, create_comment, list_comments, patch_task, reject, reopen,
    rereview, retry,
};
use task_io::{
    artifact_body, artifact_list, events, run_prompt, run_request, run_result, run_stderr,
    run_stdout, task_events, task_runs,
};
use tasks::{create_task, list_tasks, task_detail};

pub(crate) type ApiResult = Result<Response, ApiProblem>;

const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

pub(crate) fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/inbox", get(inbox))
        .merge(crate::inbox_notifications::routes())
        .route("/api/v1/tasks", get(list_tasks).post(create_task))
        .route("/api/v1/tasks/{id}", get(task_detail).patch(patch_task))
        // ADR-0044 D2（Phase 53）: タスク単位のコメントと再開。
        .route(
            "/api/v1/tasks/{id}/comments",
            get(list_comments).post(create_comment),
        )
        .route("/api/v1/tasks/{id}/reopen", post(reopen))
        .route("/api/v1/tasks/{id}/rereview", post(rereview))
        .route("/api/v1/tasks/{id}/events", get(task_events))
        .route("/api/v1/tasks/{id}/runs", get(task_runs))
        .route("/api/v1/tasks/{id}/runs/{run_id}/request", get(run_request))
        .route("/api/v1/tasks/{id}/runs/{run_id}/prompt", get(run_prompt))
        .route("/api/v1/tasks/{id}/runs/{run_id}/stdout", get(run_stdout))
        .route("/api/v1/tasks/{id}/runs/{run_id}/stderr", get(run_stderr))
        .route("/api/v1/tasks/{id}/runs/{run_id}/result", get(run_result))
        .route("/api/v1/tasks/{id}/artifacts", get(artifact_list))
        .route("/api/v1/tasks/{id}/artifacts/{idx}", get(artifact_body))
        .route("/api/v1/tasks/{id}/approve", post(approve))
        // ADR-0070 D2 追記（Phase 116）: `draft` だけを `ready` にする専用の道具（`approve` は
        // `Approval` タスクの承認とも兼用でわかりにくかった）。
        .route("/api/v1/tasks/{id}/accept", post(accept))
        .route("/api/v1/tasks/{id}/reject", post(reject))
        .route("/api/v1/tasks/{id}/answer", post(answer))
        .route("/api/v1/tasks/{id}/cancel", post(cancel))
        .route("/api/v1/tasks/{id}/retry", post(retry))
        .route("/api/v1/plans", post(create_plan))
        .route("/api/v1/replay", post(replay))
        .route("/api/v1/graph", get(graph))
        .route("/api/v1/events", get(events))
        .route("/api/v1/stream", get(crate::sse::stream))
        .route("/api/v1/providers", get(providers).post(create_provider))
        .route(
            "/api/v1/providers/{id}",
            patch(patch_provider).delete(delete_provider),
        )
        .route("/api/v1/providers/{id}/check", post(check_provider))
        .route("/api/v1/reload", post(reload))
        .route("/api/v1/accounts", get(accounts).post(create_account))
        .route("/api/v1/accounts/{id}", delete(delete_account))
        .route("/api/v1/accounts/{id}/check", post(check_account))
        .route(
            "/api/v1/accounts/{id}/login",
            post(start_account_login).delete(cancel_account_login),
        )
        .route(
            "/api/v1/accounts/{id}/login/code",
            post(submit_account_login_code),
        )
        .route("/api/v1/clusters", get(clusters))
        .route("/api/v1/clusters/{id}/settings", put(put_cluster_settings))
        .route(
            "/api/v1/clusters/{id}/connect",
            post(start_cluster_connect).delete(cancel_cluster_connect),
        )
        .route(
            "/api/v1/clusters/{id}/connect/code",
            post(submit_cluster_connect_code),
        )
        .route("/api/v1/secrets", get(secrets_list))
        .route(
            "/api/v1/secrets/{id}",
            put(put_secret).delete(delete_secret),
        )
        .route("/api/v1/org", get(org_list).post(create_org_node))
        .route(
            "/api/v1/org/{id}",
            patch(patch_org_node).delete(delete_org_node),
        )
        // ADR-0033 D4（Phase 24）: 対話。実装は `crate::conversation`。
        .route(
            "/api/v1/org/{id}/messages",
            get(crate::conversation::list_messages).post(crate::conversation::post_message),
        )
        .route("/api/v1/projects", get(project_list).post(create_project))
        .route(
            "/api/v1/projects/{id}",
            get(project_detail).patch(patch_project),
        )
        .route("/api/v1/projects/{id}/milestones", post(create_milestone))
        // ADR-0043 D1（Phase 52）: 案件のリポジトリ。実装は `crate::repos`。
        .merge(crate::repos::routes())
        // ADR-0043 D6（Phase 52）: タスクの作業ツリーの閲覧。実装は `crate::tree`。
        .merge(crate::tree::routes())
        .merge(crate::changes::routes())
        // ADR-0044 D7（Phase 57）: 案件の文書（git が正本）。実装は `crate::docs`。
        .merge(crate::docs::routes())
        // ADR-0047（Phase 61）: 知識ベース。実装は `crate::knowledge`。
        .merge(crate::knowledge::routes())
        .route("/api/v1/milestones/{id}", patch(patch_milestone))
        .merge(crate::project_plan::routes())
        // ADR-0038 D2（Phase 41）: 途中目標の判定（ok / 議論 / ng）。実装は `crate::milestones`。
        .merge(crate::milestones::routes())
        // ADR-0044 D6（Phase 55）: 案件・途中目標の中止・一時停止・アーカイブ。実装は `crate::lifecycle`。
        .merge(crate::lifecycle::routes())
        .merge(crate::memory::routes())
        .merge(crate::reports::routes())
        .merge(crate::approvals::routes())
        // ADR-0037 D4（Phase 39）: 通知（Discord）。実装は `crate::notify`。
        .merge(crate::notify::routes())
        // ADR-0040 D6（Phase 48）: リリースの一覧と昇格。実装は `crate::releases`。
        .merge(crate::releases::routes())
        // ADR-0044 D5（Phase 53）: タスクのタイムライン。実装は `crate::timeline`。
        .merge(crate::timeline::routes())
        // ADR-0069 D5: タスクの routing の監査。実装は `crate::routing`。
        .merge(crate::routing::routes())
        // ADR-0080 D5: browser の人待ち（登録依頼・承認）。
        .merge(crate::browser::routes())
        .merge(crate::browser_identity::routes())
        .merge(crate::browser_live::routes())
        .merge(crate::browser_control::routes())
        // ADR-0048 D1（Phase 60a）: Console の読み取り側。実装は `crate::console`。
        .merge(crate::console::routes())
        // ADR-0053 D4（Phase 65）: LLM source の観測。実装は `crate::llm_sources`。
        .merge(crate::llm_sources::routes())
        // ADR-0056 D4（Phase 78）: MCP クライアント / 呼び出しログの観測。実装は `crate::mcp_admin`。
        .merge(crate::mcp_admin::routes())
        // ADR-0056 D3 続き（Phase 82）: skills を GUI から見る・作る・mount する。実装は `crate::skills`。
        .merge(crate::skills::routes())
        // ADR-0072 D14（Phase E2）: ExecutionPlan の採用。実装は `crate::execution`。
        .merge(crate::execution::routes())
        // ADR-0079 D7（Phase R3a）: 決定の要求の一覧・回答・取り下げ・revise。実装は `crate::decisions`。
        .merge(crate::decisions::routes())
        .route("/api/v1/daemon", get(daemon))
        // ADR-0075 D6（Phase G1）: scratch pool の観測値（`celerisctl scratch status --json` と同じ schema）。
        .route("/api/v1/metrics/scratch", get(metrics_scratch))
        .route("/api/v1/config", get(config))
        .route("/api/v1/schema", get(schema))
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::guard,
        ))
        .with_state(state)
}

// ---- 共通 ----

pub(crate) fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_else(|_| t.to_string())
}

pub(crate) fn now_rfc3339() -> String {
    rfc3339(OffsetDateTime::now_utc())
}

pub(crate) fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => {
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = status;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static(JSON_CONTENT_TYPE),
            );
            response
        }
        Err(e) => {
            ApiProblem::internal(format!("failed to serialize the response: {e}")).into_response()
        }
    }
}

/// path の値。解析の失敗は 400 `bad_request`（axum の既定の text 応答にしない）。
pub(crate) struct Params<T>(pub(crate) T);

impl<S, T> FromRequestParts<S> for Params<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiProblem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(Params(value)),
            Err(rejection) => Err(ApiProblem::bad_request(rejection.body_text())),
        }
    }
}

/// 本文を 1 MiB まで読む（超えたら 413）。
pub(crate) async fn read_body(body: Body) -> Result<Vec<u8>, ApiProblem> {
    let mut stream = body.into_data_stream();
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            ApiProblem::bad_request(format!("failed to read the request body: {e}"))
        })?;
        if buf.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(ApiProblem::payload_too_large());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// JSON 本文を解析する。構文誤り・未知フィールド・型誤りは 400。`empty_is_object` なら空本体を `{}` とみなす。
pub(crate) async fn read_json<T: DeserializeOwned>(
    body: Body,
    empty_is_object: bool,
) -> Result<T, ApiProblem> {
    let bytes = read_body(body).await?;
    let text: &[u8] = if empty_is_object && bytes.iter().all(u8::is_ascii_whitespace) {
        b"{}"
    } else {
        &bytes
    };
    serde_json::from_slice(text)
        .map_err(|e| ApiProblem::bad_request(format!("invalid JSON body: {e}")))
}

pub(crate) fn no_query(raw: &Option<String>) -> Result<(), ApiProblem> {
    QueryParams::parse(raw.as_deref(), &[]).map(|_| ())
}

async fn fallback() -> ApiProblem {
    ApiProblem::not_found()
}

async fn method_not_allowed() -> ApiProblem {
    ApiProblem::method_not_allowed()
}

// ---- ADR-0033 D1/D2（Phase 23）: 組織・案件・途中目標 ----
//
// 読み取り（`GET /org`、`GET /projects`、`GET /projects/{id}`）は他の読み取りと同じで無認証でよい。
// 組織の編集（POST/PATCH/DELETE `/org`）は**管理系**なので `token_file` 未設定でも 401（ADR-0017 D1 と同じ規律）。
// 案件と途中目標の作成・状態変更は人の操作（`POST /tasks` と同じ扱い）なので通常の認証だけ。

/// `id` を組織のノードとして読む（存在しなければ 404）。
pub(crate) fn load_org_node(store: &SqliteStore, id: &str) -> Result<OrgNode, ApiProblem> {
    store
        .org_get(id)
        .map_err(store_problem)?
        .ok_or_else(|| ApiProblem::org_node_not_found(id))
}

pub(crate) fn parse_project_id(raw: &str) -> Result<ProjectId, ApiProblem> {
    raw.parse::<ProjectId>()
        .map_err(|_| ApiProblem::project_not_found(raw))
}

/// ADR-0039 D1 / D5: 案件の作業場所を受け取るときの検証と正規化（純粋に近い: 設定の一覧と `$HOME` を見るだけ）。
/// `Remote` の `cluster` は `[[clusters]]` にあること（無ければ 422）、`Local` の `~` は `$HOME` で展開する。
pub(crate) fn validated_workspace(
    state: &ApiState,
    spec: task_core::WorkspaceSpec,
) -> Result<task_core::WorkspaceSpec, ApiProblem> {
    if let task_core::WorkspaceSpec::Remote { cluster, .. } = &spec
        && !state
            .inner
            .config_view
            .clusters
            .iter()
            .any(|c| &c.id == cluster)
    {
        return Err(ApiProblem::validation(vec![ValidationError {
            field: Some("workspace.cluster".into()),
            message: format!("cluster {cluster:?} is not configured"),
        }]));
    }
    Ok(spec.with_home_expanded(task_core::home_dir().as_deref()))
}

#[cfg(test)]
#[path = "handlers/tests.rs"]
mod tests;
