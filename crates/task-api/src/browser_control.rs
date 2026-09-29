//! ADR-0081 D3: browser の pause・takeover・renew・resume・stop と、worker が参照する状態取得。
//!
//! 人の操作は Live View と同じ署名付き assertion を要し、lease の holder は
//! assertion の owner session に固定する（本人の session だけ）。遷移は store の
//! 1 トランザクション（`browser_control_mutate`）で行う。
use axum::Json;
use axum::extract::{Path, State, rejection::JsonRejection};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use task_core::SqliteStore;
use task_core::browser_control::{
    BrowserControl, ControlCommand, ControlError, ControlOutcome, ControlPhase, ControlRequest,
};
use task_core::browser_store::{BrowserSessionKey, BrowserStoreError};

use crate::browser::HumanAttestation;
use crate::browser_live::{now, parse_body, verify};
use crate::problem::ApiProblem;
use crate::state::ApiState;

type SessionPath = (String, String, String);

/// 人が送る command。holder は assertion から決めるので body には持たない。
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum HumanCommand {
    Pause,
    Takeover {
        ttl_secs: Option<u64>,
    },
    Renew {
        ttl_secs: Option<u64>,
    },
    Resume {
        fresh_snapshot: bool,
        policy_origin_ok: bool,
    },
    Stop,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlBody {
    assertion: HumanAttestation,
    command: HumanCommand,
    expected_version: u64,
    idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DisconnectBody {
    assertion: HumanAttestation,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ControlStatus {
    phase: ControlPhase,
    version: u64,
    lease_holder: Option<String>,
    lease_expires_at: Option<u64>,
    /// agent（worker）が browser を操作してよいか。
    agent_may_act: bool,
    in_flight: u32,
    auth_section: bool,
}

fn status_of(s: &BrowserControl) -> ControlStatus {
    ControlStatus {
        phase: s.phase(),
        version: s.version(),
        lease_holder: s.lease().map(|l| l.holder.clone()),
        lease_expires_at: s.lease().map(|l| l.expires_at),
        agent_may_act: s.phase() == ControlPhase::AgentRunning,
        in_flight: s.in_flight(),
        auth_section: s.auth_section_active(),
    }
}

pub(crate) fn control_problem(e: &ControlError) -> ApiProblem {
    let (status, code) = match e {
        ControlError::VersionConflict { .. } => (StatusCode::CONFLICT, "version_conflict"),
        ControlError::NotConverged { .. } => (StatusCode::CONFLICT, "not_converged"),
        ControlError::IdempotencyConflict => (StatusCode::CONFLICT, "idempotency_key_conflict"),
        ControlError::NotLeaseHolder => (StatusCode::FORBIDDEN, "not_lease_holder"),
        ControlError::InvalidPhase { .. } => (StatusCode::CONFLICT, "invalid_phase"),
        ControlError::AuthSectionActive => (StatusCode::CONFLICT, "auth_section_active"),
        ControlError::LeaseExpired => (StatusCode::CONFLICT, "lease_expired"),
        ControlError::LeaseTooLong { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "lease_too_long"),
        ControlError::ResumeNotVerified => {
            (StatusCode::UNPROCESSABLE_ENTITY, "resume_not_verified")
        }
        ControlError::MissingIdempotencyKey => {
            (StatusCode::UNPROCESSABLE_ENTITY, "idempotency_key_required")
        }
    };
    ApiProblem::new(status, code, e.to_string())
}

fn store_problem(e: BrowserStoreError) -> ApiProblem {
    match e {
        BrowserStoreError::Control(c) => control_problem(&c),
        _ => ApiProblem::internal("browser control store unavailable"),
    }
}

fn mutate<T>(
    store: &SqliteStore,
    path: &SessionPath,
    f: impl FnOnce(&mut BrowserControl) -> Result<T, ControlError>,
) -> Result<T, ApiProblem> {
    let key = BrowserSessionKey {
        task_id: &path.0,
        run_id: &path.1,
        session_id: &path.2,
    };
    store.browser_control_mutate(key, f).map_err(store_problem)
}

/// task cancel（handlers::cancel）から呼ぶ。
pub(crate) fn stop_task(store: &SqliteStore, task_id: &str) -> Result<usize, ApiProblem> {
    store
        .browser_control_stop_task(task_id, now())
        .map_err(store_problem)
}

fn require_daemon(state: &ApiState) -> Result<(), ApiProblem> {
    // worker は daemon bearer で呼ぶ（middleware が検査済み）。認証が無い構成では開かない。
    if state.inner.token_digest.is_none() {
        return Err(ApiProblem::new(
            StatusCode::FORBIDDEN,
            "browser_control_disabled",
            "browser control requires daemon auth",
        ));
    }
    Ok(())
}

async fn control(
    State(state): State<ApiState>,
    Path(path): Path<SessionPath>,
    body: Result<Json<ControlBody>, JsonRejection>,
) -> Result<Json<ControlOutcome>, ApiProblem> {
    let body = parse_body(body)?;
    let viewer = verify(&state, &body.assertion, &path)?;
    if !viewer.owner_session {
        return Err(ApiProblem::new(
            StatusCode::FORBIDDEN,
            "not_owner_session",
            "browser control is limited to the owner session",
        ));
    }
    let holder = viewer.session_id.unwrap_or_default();
    let command = match body.command {
        HumanCommand::Pause => ControlCommand::Pause,
        HumanCommand::Takeover { ttl_secs } => ControlCommand::Takeover { holder, ttl_secs },
        HumanCommand::Renew { ttl_secs } => ControlCommand::Renew { holder, ttl_secs },
        HumanCommand::Resume {
            fresh_snapshot,
            policy_origin_ok,
        } => ControlCommand::Resume {
            holder,
            fresh_snapshot,
            // assertion の origin 照合も policy/origin の再確認に含める。
            policy_origin_ok: policy_origin_ok && viewer.origin_ok,
        },
        HumanCommand::Stop => ControlCommand::Stop,
    };
    let req = ControlRequest {
        command,
        expected_version: body.expected_version,
        idempotency_key: body.idempotency_key,
    };
    let outcome = state
        .blocking(move |store| mutate(store, &path, |s| s.apply(&req, now())))
        .await?;
    Ok(Json(outcome))
}

async fn disconnect(
    State(state): State<ApiState>,
    Path(path): Path<SessionPath>,
    body: Result<Json<DisconnectBody>, JsonRejection>,
) -> Result<Json<ControlStatus>, ApiProblem> {
    let body = parse_body(body)?;
    let viewer = verify(&state, &body.assertion, &path)?;
    let holder = viewer.session_id.unwrap_or_default();
    let status = state
        .blocking(move |store| {
            mutate(store, &path, |s| {
                s.human_disconnected(&holder);
                Ok(status_of(s))
            })
        })
        .await?;
    Ok(Json(status))
}

/// worker が参照する状態。lease の期限切れはここで Paused に戻す（自動再開しない）。
async fn get_status(
    State(state): State<ApiState>,
    Path(path): Path<SessionPath>,
) -> Result<Json<ControlStatus>, ApiProblem> {
    require_daemon(&state)?;
    let status = state
        .blocking(move |store| {
            mutate(store, &path, |s| {
                s.expire(now());
                Ok(status_of(s))
            })
        })
        .await?;
    Ok(Json(status))
}

async fn agent_begin(
    State(state): State<ApiState>,
    Path(path): Path<SessionPath>,
) -> Result<Json<ControlStatus>, ApiProblem> {
    require_daemon(&state)?;
    let status = state
        .blocking(move |store| {
            mutate(store, &path, |s| {
                s.expire(now());
                s.begin_agent_action()?;
                Ok(status_of(s))
            })
        })
        .await?;
    Ok(Json(status))
}

async fn agent_end(
    State(state): State<ApiState>,
    Path(path): Path<SessionPath>,
) -> Result<Json<ControlStatus>, ApiProblem> {
    require_daemon(&state)?;
    let status = state
        .blocking(move |store| {
            mutate(store, &path, |s| {
                s.end_agent_action();
                Ok(status_of(s))
            })
        })
        .await?;
    Ok(Json(status))
}

/// ADR-0080 H3: credential を注入した。takeover・renew を拒否し、既存 lease を取り上げる。
async fn auth_section(
    State(state): State<ApiState>,
    Path(path): Path<SessionPath>,
) -> Result<Json<ControlStatus>, ApiProblem> {
    require_daemon(&state)?;
    let status = state
        .blocking(move |store| {
            mutate(store, &path, |s| {
                s.enter_auth_section();
                Ok(status_of(s))
            })
        })
        .await?;
    Ok(Json(status))
}

pub(crate) fn routes() -> axum::Router<ApiState> {
    use axum::routing::{get, post};
    const BASE: &str = "/api/v1/tasks/{id}/browser/control/{run}/{session}";
    axum::Router::new()
        .route(BASE, get(get_status).post(control))
        .route(&format!("{BASE}/disconnect"), post(disconnect))
        .route(&format!("{BASE}/agent/begin"), post(agent_begin))
        .route(&format!("{BASE}/agent/end"), post(agent_end))
        .route(&format!("{BASE}/auth-section"), post(auth_section))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_errors_map_to_http() {
        let conflict = control_problem(&ControlError::VersionConflict {
            expected: 1,
            actual: 2,
        });
        assert_eq!(conflict.code(), "version_conflict");
        assert_eq!(
            control_problem(&ControlError::NotConverged { in_flight: 1 }).code(),
            "not_converged"
        );
        assert_eq!(
            control_problem(&ControlError::ResumeNotVerified).code(),
            "resume_not_verified"
        );
        assert_eq!(
            control_problem(&ControlError::LeaseTooLong {
                requested: 301,
                max: 300
            })
            .code(),
            "lease_too_long"
        );
    }

    #[test]
    fn routes_are_registered() {
        let _ = routes();
    }
}
