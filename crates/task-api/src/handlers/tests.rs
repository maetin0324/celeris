use std::path::PathBuf;
use std::time::Duration;

use axum::http::Request;
use task_ops::view::ViewContext;
use tower::ServiceExt;

use super::*;
use crate::types::{ApiConfigView, ConfigView, ReviewerConfigView};
use crate::{ApiSettings, ApiState};

fn state(dir: &std::path::Path) -> ApiState {
    state_rx(dir, tokio::sync::watch::channel(None).1)
}

fn state_rx(
    dir: &std::path::Path,
    rx: tokio::sync::watch::Receiver<Option<task_ops::daemon::DaemonSnapshot>>,
) -> ApiState {
    let settings = ApiSettings {
        browser: Default::default(),
        documentation_state_dir: None,
        listen: "127.0.0.1:7710".parse().unwrap_or_else(|e| panic!("{e}")),
        // ADR-0044 §5 Phase 53 追記（Phase 55）: `POST /replay` は管理系になったので、
        // この単体テストのルータにもトークンを持たせる（下の要求は Bearer を付ける）。
        token: Some(REPLAY_TEST_TOKEN.to_string()),
        allowed_hosts: vec![],
        db_path: dir.join("celeris.db"),
        busy_timeout: Duration::from_millis(5000),
        background_checkpoint: false,
        view: ViewContext {
            workspace_root: dir.join("ws"),
            retry_backoff_base: Duration::from_secs(0),
            retry_backoff_max: Duration::from_secs(0),
            max_requeues: 5,
            clusters: Default::default(),
        },
        config_view: ConfigView {
            config_path: String::new(),
            db: String::new(),
            workspace_root: String::new(),
            tick_ms: 2000,
            max_concurrency: 1,
            lease_grace_secs: 0,
            idle_timeout_secs: 0,
            kill_grace_secs: 0,
            review_timeout_secs: 0,
            error_cooldown_secs: 0,
            retry_backoff_base_secs: 0,
            retry_backoff_max_secs: 0,
            max_requeues: 5,
            plan_auto_accept: false,
            reviewer: ReviewerConfigView {
                adapter: None,
                tier: Some(task_core::Tier::Standard),
            },
            providers: vec![],
            clusters: vec![],
            roles: vec![],
            genres: vec![],
            delegation: task_core::DelegationLimits::default(),
            api: ApiConfigView {
                bind: "127.0.0.1:7710".into(),
                auth_required: false,
                allowed_hosts: vec![],
            },
        },
        roles: vec![],
        genres: vec![],
        conversation_genre: task_core::CONVERSATION_GENRE.to_string(),
        celeris_version: "test".into(),
        instance_id: "01J00000000000000000000000".into(),
        started_at: "2026-09-14T00:00:00Z".into(),
        providers_dir: None,
        openai_compatible_source_ids: Default::default(),
        admin_tx: None,
        accounts_roots: std::collections::HashMap::new(),
        max_runs_per_account: 0,
        secrets_dir: None,
        secret_usage: std::collections::HashMap::new(),
        memory_dir: None,
        notify_secret_id: task_core::DEFAULT_WEBHOOK_SECRET_ID.to_string(),
        notify_gui_base_url: None,
        notify_inbox_batch_secs: 60,
        notify_inbox_reminder_secs: 86_400,
        notify_digest_interval_secs: 3_600,
        notify_digest_max_lines: 10,
        releases: None,
        release: "dev".to_string(),
        mode: task_core::DaemonMode::Normal,
        role: task_core::SharedRole::new(task_core::InstanceRole::Active),
        github: crate::GithubSettings::default(),
        knowledge_root: None,
        docs_repo_root: Some(dir.join("workspace")),
        llm_sources: None,
        tree_limits: task_core::TreeLimits::default(),
    };
    ApiState::new(settings, rx).unwrap_or_else(|e| panic!("{e}"))
}

fn state_with_tx(
    dir: &std::path::Path,
) -> (
    ApiState,
    tokio::sync::watch::Sender<Option<task_ops::daemon::DaemonSnapshot>>,
) {
    let (tx, rx) = tokio::sync::watch::channel(None);
    (state_rx(dir, rx), tx)
}

/// ADR-0075 §5 G1 受け入れ条件 8: `GET /api/v1/metrics/scratch` は `DaemonSnapshot.scratch`（`celerisctl scratch
/// status --json` と同じ `task_ops::daemon::ScratchStatus`）を返し、その JSON のキーは committed schema の
/// `ScratchStatus` の properties と一致する。スナップショットが無ければ 404。
#[tokio::test]
async fn metrics_scratch_matches_the_status_schema() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let (state, tx) = state_with_tx(dir.path());
    let app = router(state);
    let get = || {
        Request::get("/api/v1/metrics/scratch")
            .header("host", "127.0.0.1:7710")
            .header("authorization", format!("Bearer {REPLAY_TEST_TOKEN}"))
            .body(Body::empty())
            .unwrap_or_else(|e| panic!("{e}"))
    };
    let resp = app
        .clone()
        .oneshot(get())
        .await
        .unwrap_or_else(|e| match e {});
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let status = task_ops::daemon::ScratchStatus {
        schema: task_ops::daemon::SCRATCH_STATUS_SCHEMA.to_string(),
        enabled: true,
        disabled_reason: None,
        dir: "/var/lib/celeris/scratch".into(),
        observed_at: "2026-09-28T00:00:00Z".into(),
        fs_total_bytes: Some(252 << 30),
        fs_free_bytes: Some(91 << 30),
        targets_bytes: 62 << 30,
        pinned_bytes: 18 << 30,
        targets_max_bytes: 100 << 30,
        total_max_bytes: 150 << 30,
        effective_max_bytes: 106 << 30,
        high_watermark: 0.9,
        low_watermark: 0.7,
        pressure: "none".into(),
        owners: vec![task_ops::daemon::ScratchOwnerView {
            owner: "task-01ABC".into(),
            kind: "task".into(),
            class: "p0".into(),
            reason: "task running".into(),
            has_target: true,
            size_bytes: Some(18 << 30),
            estimated_bytes: 18 << 30,
            measured_at: None,
            lease_mtime: Some("2026-09-28T00:00:00Z".into()),
            repo_key: Some("agent-platform-0123456789".into()),
            base_commit: Some("0123456789ab".into()),
            adopted_from: None,
            work_unit_key: None,
        }],
        legacy: vec![task_ops::daemon::ScratchLegacyView {
            path: "/var/lib/celeris/build-cache/cargo/agent-platform-dev".into(),
            class: "legacy".into(),
            size_bytes: Some(21 << 30),
            last_write: None,
        }],
        last_gc: Some(task_ops::daemon::ScratchGcView {
            at: "2026-09-28T00:00:00Z".into(),
            pressure: "none".into(),
            emergency: false,
            removed: vec![task_ops::daemon::ScratchGcRemovedView {
                id: "task-01ABC/wu-01DEF".into(),
                class: "p3".into(),
                estimated_bytes: 3 << 30,
                why: "immediate".into(),
            }],
            reclaimed_bytes: 3 << 30,
        }),
        // ADR-0129 (1): sccache と cache server は Celeris の外。欄は型を残すが常に `None`。
        sccache: None,
        cache: None,
    };
    let mut snapshot: task_ops::daemon::DaemonSnapshot = serde_json::from_value(serde_json::json!({
            "instance_id": "01TEST", "pid": 1, "hostname": "h", "started_at": "2026-09-28T00:00:00Z",
            "last_tick_at": "2026-09-28T00:00:00Z", "ticks": 1, "tick_ms": 1000, "in_flight": [],
            "cooldowns": [], "awaiting_human": [], "unroutable": [], "providers": []
        }))
        .unwrap_or_else(|e| panic!("{e}"));
    snapshot.scratch = Some(status.clone());
    tx.send_replace(Some(snapshot));
    let resp = app.oneshot(get()).await.unwrap_or_else(|e| match e {});
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        json,
        serde_json::to_value(&status).unwrap_or_else(|e| panic!("{e}"))
    );
    // committed schema の `ScratchStatus` と同じキー（CLI の `--json` も同じ型を出す）。
    let schema = crate::schema::api_v1_schema_value();
    let props = schema["$defs"]["ScratchStatus"]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("ScratchStatus is not in the api schema"));
    let mut schema_keys: Vec<&String> = props.keys().collect();
    schema_keys.sort();
    let obj = json.as_object().unwrap_or_else(|| panic!("not an object"));
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    assert_eq!(keys, schema_keys);
    assert_eq!(json["schema"], "celeris.scratch-status/1");
    // ADR-0129 (1): sccache と cache server は Celeris の外。欄は null で出る。
    assert!(json["sccache"].is_null());
    assert!(json["cache"].is_null());
}

/// この単体テストだけで使う管理系トークン。
const REPLAY_TEST_TOKEN: &str = "replay-test-token";

fn replay_request() -> Request<Body> {
    Request::post("/api/v1/replay")
        .header("host", "127.0.0.1:7710")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {REPLAY_TEST_TOKEN}"))
        .body(Body::from("{}"))
        .unwrap_or_else(|e| panic!("{e}"))
}

/// ADR-0044 §5 Phase 53 追記（Phase 55）: トークンが無ければ 401。
#[tokio::test]
async fn replay_without_a_token_is_unauthorized() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let app = router(state(dir.path()));
    let request = Request::post("/api/v1/replay")
        .header("host", "127.0.0.1:7710")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap_or_else(|e| panic!("{e}"));
    let resp = app.oneshot(request).await.unwrap_or_else(|e| match e {});
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn second_concurrent_replay_is_rejected_with_503() {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let state = state(dir.path());
    let app = router(state.clone());

    let guard = state.try_begin_replay();
    assert!(guard.is_some());
    let busy = app
        .clone()
        .oneshot(replay_request())
        .await
        .unwrap_or_else(|e| match e {});
    assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        busy.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("5")
    );
    let body = axum::body::to_bytes(busy.into_body(), usize::MAX)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let problem: serde_json::Value =
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(problem["code"], "replay_in_progress");

    drop(guard);
    let ok = app
        .oneshot(replay_request())
        .await
        .unwrap_or_else(|e| match e {});
    assert_eq!(ok.status(), StatusCode::OK);
    let _ = PathBuf::new();
}
