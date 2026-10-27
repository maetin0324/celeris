//! web 端末の許可・対象の解決・監査（ADR 2026-10-27-web-terminal D3・D4）。
//!
//! PTY は持たない（web gateway の helper が持つ。D1）。ここは主体が `human:web` のときだけ、対象（`local` /
//! `cluster:<id>`）を argv と cwd に解決し、開始・終了を追記専用の表に書く。入出力・token は扱わない。

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use task_core::{TaskStore, TerminalSessionEvent};
use time::OffsetDateTime;

use crate::principal::{Caller, Principal};
use crate::problem::{ApiProblem, store_problem};
use crate::state::ApiState;

use super::{ApiResult, Params, json_response, no_query, read_json, rfc3339};

const CLIENT_MAX: usize = 200;
const REASONS: &[&str] = &[
    "user",
    "disconnect",
    "idle_timeout",
    "max_duration",
    "logout",
    "exited",
    "spawn_failed",
    "gateway_shutdown",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalOpenBody {
    /// `local` / `cluster:<id>`。
    pub target: String,
    /// 接続元（gateway が見た Host と address）。監査にだけ使う。
    #[serde(default)]
    pub client: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TerminalOpened {
    pub session_id: String,
    pub target: String,
    /// helper が exec する argv。
    pub argv: Vec<String>,
    /// helper の作業 dir（クラスタは手元の HOME。クラスタ側の dir は argv の中）。
    pub cwd: String,
    /// クラスタ側の作業 dir（クラスタだけ）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalCloseBody {
    pub reason: String,
    #[serde(default)]
    pub duration_secs: Option<i64>,
    #[serde(default)]
    pub exit_code: Option<i64>,
}

fn require_web(caller: &Caller) -> Result<(), ApiProblem> {
    match &caller.0 {
        Principal::Human { name } if name == "web" => Ok(()),
        _ => Err(ApiProblem::new(
            StatusCode::FORBIDDEN,
            "terminal_forbidden",
            "only the web gateway (human:web) can open terminals",
        )),
    }
}

/// shell の単一引用符で囲む。
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn home_dir() -> Option<String> {
    std::env::var("HOME").ok().filter(|h| h.starts_with('/'))
}

/// daemon を動かすユーザーの login shell（`/etc/passwd`。無ければ `$SHELL`、それも無ければ `/bin/sh`）。
fn login_shell() -> String {
    use std::os::unix::fs::MetadataExt;
    // `/proc/self` の持ち主はこの process の uid。
    let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let fields: Vec<&str> = line.split(':').collect();
                (fields.len() == 7 && uid.is_some() && fields[2].parse::<u32>().ok() == uid)
                    .then(|| fields[6].to_string())
            })
        })
        .or_else(|| std::env::var("SHELL").ok())
        .filter(|s| s.starts_with('/'))
        .unwrap_or_else(|| "/bin/sh".to_string())
}

fn new_session_id() -> String {
    ulid::Ulid::new().to_string()
}

fn resolve(
    state: &ApiState,
    store: &task_core::SqliteStore,
    target: &str,
) -> Result<(Vec<String>, String, Option<String>), ApiProblem> {
    let home = home_dir().ok_or_else(|| ApiProblem::internal("HOME is not set"))?;
    if target == "local" {
        return Ok((vec![login_shell(), "-l".to_string()], home, None));
    }
    let Some(id) = target.strip_prefix("cluster:") else {
        return Err(ApiProblem::bad_request(
            "target must be \"local\" or \"cluster:<id>\"",
        ));
    };
    let cluster = state
        .inner
        .config_view
        .clusters
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| ApiProblem::cluster_not_found(id))?;
    let connected = state
        .snapshot()
        .and_then(|s| s.clusters.iter().find(|c| c.id == id).map(|c| c.connected))
        .unwrap_or(false);
    if !connected {
        return Err(ApiProblem::new(
            StatusCode::CONFLICT,
            "cluster_not_connected",
            "the cluster's ssh ControlMaster is not connected; connect it from the clusters screen first",
        ));
    }
    let work_dir = store
        .cluster_settings_get(id)
        .map_err(store_problem)?
        .and_then(|s| s.work_dir)
        .or_else(|| cluster.work_dir.clone())
        .filter(|w| w.starts_with('/'))
        .ok_or_else(|| {
            ApiProblem::new(
                StatusCode::CONFLICT,
                "work_dir_unset",
                "the cluster has no absolute work_dir; set it on the clusters screen",
            )
        })?;
    let remote = format!(
        "cd {} && exec \"${{SHELL:-/bin/sh}}\" -l",
        sh_quote(&work_dir)
    );
    let argv = [
        "ssh",
        "-tt",
        "-o",
        "BatchMode=yes",
        "-o",
        "ControlMaster=no",
        &cluster.host,
        "--",
        &remote,
    ]
    .map(String::from)
    .to_vec();
    Ok((argv, home, Some(work_dir)))
}

/// `POST /terminals`。
pub(super) async fn open_terminal(
    State(state): State<ApiState>,
    caller: Caller,
    RawQuery(raw): RawQuery,
    body: axum::body::Body,
) -> ApiResult {
    no_query(&raw)?;
    require_web(&caller)?;
    let body: TerminalOpenBody = read_json(body, false).await?;
    let client = body.client.map(|c| c.chars().take(CLIENT_MAX).collect());
    let actor = caller.actor();
    let resolve_state = state.clone();
    let opened = state
        .blocking(move |store| {
            let (argv, cwd, remote_dir) = resolve(&resolve_state, store, &body.target)?;
            let session_id = new_session_id();
            store
                .terminal_session_event_append(&TerminalSessionEvent {
                    session_id: session_id.clone(),
                    ts: rfc3339(OffsetDateTime::now_utc()),
                    kind: "opened".into(),
                    target: body.target.clone(),
                    actor,
                    client,
                    reason: None,
                    duration_secs: None,
                    exit_code: None,
                })
                .map_err(store_problem)?;
            Ok(TerminalOpened {
                session_id,
                target: body.target,
                argv,
                cwd,
                remote_dir,
            })
        })
        .await?;
    tracing::info!(
        op = "terminal_open",
        session = %opened.session_id,
        target = %opened.target,
        "terminal: opened"
    );
    Ok(json_response(StatusCode::CREATED, &opened))
}

/// `POST /terminals/{id}/close`。
pub(super) async fn close_terminal(
    State(state): State<ApiState>,
    caller: Caller,
    Params(id): Params<String>,
    RawQuery(raw): RawQuery,
    body: axum::body::Body,
) -> ApiResult {
    no_query(&raw)?;
    require_web(&caller)?;
    let body: TerminalCloseBody = read_json(body, false).await?;
    if !REASONS.contains(&body.reason.as_str()) {
        return Err(ApiProblem::bad_request("unknown close reason"));
    }
    let actor = caller.actor();
    let session = id.clone();
    let reason = body.reason.clone();
    state
        .blocking(move |store| {
            let opened = store
                .terminal_session_opened(&session)
                .map_err(store_problem)?
                .ok_or_else(ApiProblem::not_found)?;
            store
                .terminal_session_event_append(&TerminalSessionEvent {
                    session_id: session,
                    ts: rfc3339(OffsetDateTime::now_utc()),
                    kind: "closed".into(),
                    target: opened.target,
                    actor,
                    client: opened.client,
                    reason: Some(body.reason),
                    duration_secs: body.duration_secs.map(|d| d.max(0)),
                    exit_code: body.exit_code,
                })
                .map_err(store_problem)
        })
        .await?;
    tracing::info!(op = "terminal_close", session = %id, reason = %reason, "terminal: closed");
    Ok(axum::response::IntoResponse::into_response(
        StatusCode::NO_CONTENT,
    ))
}
