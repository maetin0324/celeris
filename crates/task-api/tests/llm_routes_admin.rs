#![allow(clippy::duplicate_mod)] // Included integration suites each own their standalone `common` module.

//! API-v2 source/account admin regression suite.
//!
//! The focused route suites are included here so the combined API contract can be
//! run as `cargo test -p task-api --test llm_routes_admin`.

mod common;
#[path = "llm_accounts_admin.rs"]
mod llm_accounts_admin;
#[path = "llm_sources.rs"]
mod llm_sources;

mod harnesses {
    use super::common::*;
    use std::sync::Arc;
    use task_api::{HarnessCandidateView, HarnessRuleView, HarnessView, HarnessesReader};
    use tower::ServiceExt;

    fn g(path: &str) -> axum::http::Request<axum::body::Body> {
        let auth = format!("Bearer {TOKEN}");
        get_with(path, &[("authorization", auth.as_str())])
    }

    struct Reader;
    impl HarnessesReader for Reader {
        fn harnesses(&self) -> Vec<HarnessView> {
            vec![HarnessView {
                id: "coding".into(),
                kind: "acp".into(),
                api_formats: vec!["openai".into()],
                source_kinds: vec!["oauth".into()],
                capabilities: vec!["repo".into()],
                rules: vec![HarnessRuleView {
                    genre: Some("code".into()),
                    role: None,
                    required_capabilities: vec![],
                    requires_repo: Some(true),
                    execution_class: None,
                }],
                execution_class_defaults: vec!["interactive".into()],
                route_defaults: vec![HarnessCandidateView {
                    source_id: "src".into(),
                    model_id: "model".into(),
                }],
            }]
        }

        fn source_models(&self) -> Vec<(String, Vec<String>)> {
            vec![("src".into(), vec!["model".into()])]
        }
    }

    #[tokio::test]
    async fn harness_list_detail_missing_and_unconfigured() {
        let env = TestEnv::with(EnvOptions {
            token: Some(TOKEN.into()),
            harnesses: Some(Arc::new(Reader)),
            ..Default::default()
        });
        let list = env.router().oneshot(g("/api/v1/harnesses")).await.unwrap();
        assert_eq!(list.status(), axum::http::StatusCode::OK);
        let detail = env
            .router()
            .oneshot(g("/api/v1/harnesses/coding"))
            .await
            .unwrap();
        assert_eq!(detail.status(), axum::http::StatusCode::OK);
        let missing = env
            .router()
            .oneshot(g("/api/v1/harnesses/nope"))
            .await
            .unwrap();
        assert_eq!(missing.status(), axum::http::StatusCode::NOT_FOUND);
        let empty = TestEnv::with(EnvOptions {
            token: Some(TOKEN.into()),
            ..Default::default()
        });
        let unavailable = empty
            .router()
            .oneshot(g("/api/v1/harnesses"))
            .await
            .unwrap();
        assert_eq!(
            unavailable.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

mod run_selection {
    use super::common::*;
    use serde_json::Value;
    use task_core::{Event, RunIndexRole, RunIndexStatus, RunRow, TaskKind, TaskStore};
    use tower::ServiceExt;

    #[tokio::test]
    async fn task_runs_expose_persisted_selection_and_legacy_unresolved() {
        let env = TestEnv::new();
        let task = new_task(TaskKind::Execute, task_core::Status::Running);
        let selected = "selected-run";
        let legacy = "legacy-run";
        env.seed_with(
            &task,
            vec![
                Event::WorkerStarted {
                    run_id: selected.into(),
                    adapter: "fake".into(),
                    model: "m".into(),
                    provider: Some("old-provider".into()),
                    account: None,
                    role: None,
                    task_role: None,
                },
                Event::WorkerStarted {
                    run_id: legacy.into(),
                    adapter: "fake".into(),
                    model: "m".into(),
                    provider: None,
                    account: None,
                    role: None,
                    task_role: None,
                },
            ],
        );
        for (run_id, seq) in [(selected, 1), (legacy, 2)] {
            env.store
                .run_index_start(RunRow {
                    run_id: run_id.into(),
                    task_id: task.id.to_string(),
                    work_unit_id: None,
                    role: RunIndexRole::Worker,
                    seq,
                    status: RunIndexStatus::Running,
                    adapter: None,
                    model: None,
                    account: None,
                    session_id: None,
                    checkpoint: None,
                    usage: None,
                    metrics: None,
                    started_at: format!("2026-10-12T00:00:0{seq}Z"),
                    finished_at: None,
                })
                .unwrap();
        }
        let selection = task_core::llm_route::RunSelection {
            harness: "h".into(),
            source: "s".into(),
            model: "m2".into(),
            account: "a".into(),
            metadata: serde_json::json!({"version":1,"basis":"route"}),
        };
        env.store
            .run_record_selection(selected, &selection)
            .unwrap();
        let response = env
            .router()
            .oneshot(get(&format!("/api/v1/tasks/{}/runs", task.id)))
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            body["runs"][0]["selection"],
            serde_json::json!({"status":"selected","harness_id":"h","source_id":"s","model_id":"m2","account_id":"a","basis":{"version":1,"basis":"route"}})
        );
        assert_eq!(
            body["runs"][1]["selection"],
            serde_json::json!({"status":"legacy_unresolved"})
        );
        let detail = env
            .router()
            .oneshot(get(&format!("/api/v1/tasks/{}", task.id)))
            .await
            .unwrap();
        let detail: Value = serde_json::from_slice(
            &axum::body::to_bytes(detail.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(detail["runs"][0]["selection"], body["runs"][0]["selection"]);
    }
}
