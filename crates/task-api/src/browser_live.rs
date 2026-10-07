//! Task-scoped live relay API. A GUI-signed assertion is checked on every relay call.
use axum::Json;
use axum::extract::{Path, Query, State, rejection::JsonRejection};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use task_core::browser_live::{
    self, LiveDenied, LiveEvent, LiveGrant, LiveResume, LiveTab, LiveTarget, LiveViewer,
    ScrubbedLiveEvent,
};
use task_core::browser_store::{BrowserSessionKey, StoredLiveEvent};
use task_core::browser_wait::{BrowserWaitState, BrowserWaitStore};
use task_core::{TaskId, TaskStore};
use time::OffsetDateTime;
use ulid::Ulid;

use crate::browser::HumanAttestation;
use crate::problem::ApiProblem;
use crate::state::ApiState;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayClaims {
    task_id: String,
    run_id: String,
    browser_session_id: String,
    owner_session_id: String,
    owner_session: bool,
    origin_ok: bool,
    expires_at: i64,
}

#[derive(Clone)]
pub(crate) struct LiveGrantRecord {
    pub(crate) grant: LiveGrant,
    pub(crate) browser_session_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantRequest {
    assertion: HumanAttestation,
}
#[derive(Serialize)]
struct GrantResponse {
    grant_id: String,
    expires_at: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayRequest {
    assertion: HumanAttestation,
    grant_id: String,
}
#[derive(Serialize)]
struct CheckResponse {
    connected: bool,
}
#[derive(Serialize)]
struct ReadResponse {
    plan: LiveResume,
    events: Vec<StoredLiveEvent>,
}
#[derive(Deserialize)]
struct After {
    after: Option<u64>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum EventBody {
    Status { state: String },
    Tabs { tabs: Vec<LiveTab> },
    Url { url: String },
    Console { level: String, text: String },
}
#[derive(Serialize)]
struct EventResponse {
    seq: u64,
}

fn denied(reason: LiveDenied) -> ApiProblem {
    ApiProblem::new(
        StatusCode::from_u16(reason.http_status()).unwrap_or(StatusCode::FORBIDDEN),
        reason.code(),
        "live access denied",
    )
}
fn invalid() -> ApiProblem {
    ApiProblem::new(
        StatusCode::FORBIDDEN,
        "not_owner_session",
        "live access denied",
    )
}
pub(crate) fn parse_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<T, ApiProblem> {
    body.map(|Json(value)| value).map_err(|_| {
        ApiProblem::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "browser_body_invalid",
            "invalid live request",
        )
    })
}
fn internal() -> ApiProblem {
    ApiProblem::internal("browser live store unavailable")
}
pub(crate) fn now() -> u64 {
    u64::try_from(OffsetDateTime::now_utc().unix_timestamp()).unwrap_or(0)
}

/// web の Ed25519 署名を公開鍵で確かめる（鍵の設定が無ければ `live_view_disabled`、署名不正は
/// `not_owner_session`）。claims の解釈は呼び手が行う（Live View は `RelayClaims`、信頼端末は
/// `browser_trusted_devices::DeviceClaims`）。
pub(crate) fn verify_signature(
    state: &ApiState,
    assertion: &HumanAttestation,
) -> Result<(), ApiProblem> {
    let key = state
        .browser
        .attestation_public_key
        .as_deref()
        .ok_or_else(|| denied(LiveDenied::LiveViewDisabled))?;
    let signature = crate::browser::decode_hex(&assertion.signature).ok_or_else(invalid)?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(assertion.payload.as_bytes(), &signature)
        .map_err(|_| invalid())
}

pub(crate) fn verify(
    state: &ApiState,
    assertion: &HumanAttestation,
    path: &(String, String, String),
) -> Result<LiveViewer, ApiProblem> {
    verify_signature(state, assertion)?;
    let claims: RelayClaims = serde_json::from_str(&assertion.payload).map_err(|_| invalid())?;
    if claims.task_id != path.0
        || claims.run_id != path.1
        || claims.browser_session_id != path.2
        || claims.owner_session_id.is_empty()
        || claims.expires_at < now() as i64
        || claims.expires_at > now() as i64 + 30
    {
        return Err(invalid());
    }
    Ok(LiveViewer {
        auth_enabled: state.inner.token_digest.is_some(),
        single_owner_instance: true,
        session_id: Some(claims.owner_session_id),
        owner_session: claims.owner_session,
        origin_ok: claims.origin_ok,
    })
}

async fn target(
    state: &ApiState,
    path: &(String, String, String),
) -> Result<LiveTarget, ApiProblem> {
    let (task, run, session) = path.clone();
    state.blocking(move |store| {
        let id: TaskId = task.parse().map_err(|_| denied(LiveDenied::OtherTask))?;
        let row = store.run_index_get(&run).map_err(|_| internal())?;
        let task_row = store.get(id).map_err(|_| internal())?;
        let run_active = row.as_ref().is_some_and(|r| r.task_id == task && r.status == task_core::execution_plan::RunIndexStatus::Running)
            && task_row.as_ref().is_some_and(|t| t.status == task_core::Status::Running);
        if row.as_ref().is_some_and(|r| r.task_id != task) { return Err(denied(LiveDenied::OtherTask)); }
        let waits = store.browser_waits_for_task(id).map_err(|_| internal())?;
        let stopped_by_credential = waits.iter().any(|w| w.run_id == run && w.session_id == session
            && w.state == BrowserWaitState::Resumed && w.credential.is_some());
        let key = BrowserSessionKey { task_id: &task, run_id: &run, session_id: &session };
        let page = store.browser_live_after(key, 0, 1).map_err(|_| internal())?;
        let stopped_by_event = if let Some(latest) = page.latest_seq {
            let last = store.browser_live_after(key, latest.saturating_sub(1), 1).map_err(|_| internal())?;
            last.events.iter().any(|e| matches!(&e.body, task_core::browser_live::PersistedLiveEvent::Status { state } if state == "observation_stopped"))
        } else { false };
        Ok(LiveTarget { task_id: task, run_id: run, run_active, credential_interval: stopped_by_credential || stopped_by_event })
    }).await
}

async fn checked(
    state: &ApiState,
    path: &(String, String, String),
    body: &RelayRequest,
) -> Result<(), ApiProblem> {
    let viewer = verify(state, &body.assertion, path)?;
    let record: LiveGrantRecord = state
        .live_grants
        .lock()
        .map_err(|_| internal())?
        .get(&body.grant_id)
        .cloned()
        .ok_or_else(|| denied(LiveDenied::GrantExpired))?;
    if record.browser_session_id != path.2 {
        return Err(denied(LiveDenied::OtherTask));
    }
    let target = target(state, path).await?;
    browser_live::check_connection(
        &record.grant,
        viewer.session_id.as_deref().unwrap_or(""),
        &target,
        now(),
    )
    .map_err(denied)?;
    if !viewer.owner_session {
        return Err(denied(LiveDenied::NotOwnerSession));
    }
    if !viewer.origin_ok {
        return Err(denied(LiveDenied::OriginMismatch));
    }
    Ok(())
}

async fn grant(
    State(state): State<ApiState>,
    Path(path): Path<(String, String, String)>,
    body: Result<Json<GrantRequest>, JsonRejection>,
) -> Result<Json<GrantResponse>, ApiProblem> {
    let body = parse_body(body)?;
    let viewer = verify(&state, &body.assertion, &path)?;
    let target = target(&state, &path).await?;
    let grant =
        browser_live::authorize_live(&viewer, &target, &path.0, &path.1, now()).map_err(denied)?;
    let expires_at = grant.expires_at;
    let grant_id = Ulid::new().to_string();
    let mut grants = state.live_grants.lock().map_err(|_| internal())?;
    grants.retain(|_, g| g.grant.expires_at > now());
    grants.insert(
        grant_id.clone(),
        LiveGrantRecord {
            grant,
            browser_session_id: path.2.clone(),
        },
    );
    Ok(Json(GrantResponse {
        grant_id,
        expires_at,
    }))
}
async fn check(
    State(state): State<ApiState>,
    Path(path): Path<(String, String, String)>,
    body: Result<Json<RelayRequest>, JsonRejection>,
) -> Result<Json<CheckResponse>, ApiProblem> {
    let body = parse_body(body)?;
    checked(&state, &path, &body).await?;
    Ok(Json(CheckResponse { connected: true }))
}
async fn read(
    State(state): State<ApiState>,
    Path(path): Path<(String, String, String)>,
    Query(after): Query<After>,
    body: Result<Json<RelayRequest>, JsonRejection>,
) -> Result<Json<ReadResponse>, ApiProblem> {
    let body = parse_body(body)?;
    checked(&state, &path, &body).await?;
    let page = state
        .blocking(move |store| {
            let key = BrowserSessionKey {
                task_id: &path.0,
                run_id: &path.1,
                session_id: &path.2,
            };
            store
                .browser_live_after(key, after.after.unwrap_or(0), 1000)
                .map_err(|_| internal())
        })
        .await?;
    let plan = browser_live::resume_plan(
        after.after,
        page.oldest_seq.unwrap_or(1),
        page.latest_seq.unwrap_or(0),
    );
    let events = if matches!(plan, LiveResume::Replay { .. }) {
        page.events
    } else {
        Vec::new()
    };
    Ok(Json(ReadResponse { plan, events }))
}
async fn event(
    State(state): State<ApiState>,
    Path(path): Path<(String, String, String)>,
    body: Result<Json<EventBody>, JsonRejection>,
) -> Result<Json<EventResponse>, ApiProblem> {
    let body = parse_body(body)?;
    // Worker calls this with the daemon bearer. The current run and task are checked before writing.
    if state.inner.token_digest.is_none() {
        return Err(denied(LiveDenied::LiveViewDisabled));
    }
    let target = target(&state, &path).await?;
    if !target.run_active {
        return Err(denied(LiveDenied::RunEnded));
    }
    let is_stop = matches!(&body, EventBody::Status { state } if state == "observation_stopped");
    if target.credential_interval && !is_stop {
        return Err(denied(LiveDenied::ObservationStopped));
    }
    let event = match body {
        EventBody::Status { state } => LiveEvent::Status { state },
        EventBody::Tabs { tabs } => LiveEvent::Tabs { tabs },
        EventBody::Url { url } => LiveEvent::Url { url },
        EventBody::Console { level, text } => LiveEvent::Console { level, text },
    };
    let scrubbed = ScrubbedLiveEvent::from_event(&event).ok_or_else(invalid)?;
    let seq = state
        .blocking(move |store| {
            let key = BrowserSessionKey {
                task_id: &path.0,
                run_id: &path.1,
                session_id: &path.2,
            };
            store
                .browser_live_append(key, &scrubbed)
                .map_err(|_| internal())
        })
        .await?;
    Ok(Json(EventResponse { seq }))
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::post;
    axum::Router::new()
        .route(
            "/api/v1/tasks/{id}/browser/live/{run}/{session}/grant",
            post(grant),
        )
        .route(
            "/api/v1/tasks/{id}/browser/live/{run}/{session}/check",
            post(check),
        )
        .route(
            "/api/v1/tasks/{id}/browser/live/{run}/{session}/events",
            post(event),
        )
        .route(
            "/api/v1/tasks/{id}/browser/live/{run}/{session}/read",
            post(read),
        )
}
