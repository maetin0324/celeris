//! API-v2 source/account admin regression suite.
//!
//! The focused route suites are included here so the combined API contract can be
//! run as `cargo test -p task-api --test llm_routes_admin`.

#[path = "accounts_admin.rs"]
mod accounts_admin;
mod common;
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
