mod common;
use common::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use task_api::browser_readiness::BrowserReadiness;

#[tokio::test]
async fn browser_doctor_readiness_requires_token_and_uses_daemon_probe() {
    let env = admin_env();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let state = env
        .state
        .clone()
        .with_browser_readiness(Arc::new(move |store| {
            let mut report = BrowserReadiness::default();
            report.push(
                "NG",
                "site-policies",
                format!(
                    "{} 件; 修正: web で登録",
                    store.browser_site_policy_list().unwrap().len()
                ),
            );
            report.push("OK", "runtime", "launcher");
            seen.fetch_add(1, Ordering::SeqCst);
            report
        }));
    let app = task_api::router(state);
    let denied = send(&app, get_with("/api/v1/browser/readiness", &[])).await;
    assert_eq!(denied.status.as_u16(), 401);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let auth = format!("Bearer {TOKEN}");
    let response = send(
        &app,
        get_with("/api/v1/browser/readiness", &[("authorization", &auth)]),
    )
    .await;
    assert_eq!(response.status.as_u16(), 200, "{}", response.text());
    assert_eq!(response.json()["items"][0]["status"], "NG");
    assert_eq!(response.json()["items"][1]["detail"], "launcher");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn browser_doctor_readiness_without_daemon_probe_fails_closed() {
    let env = admin_env();
    let app = task_api::router(env.state.clone());
    let auth = format!("Bearer {TOKEN}");
    let response = send(
        &app,
        get_with("/api/v1/browser/readiness", &[("authorization", &auth)]),
    )
    .await;
    assert_eq!(response.status.as_u16(), 503);
    assert_eq!(response.json()["code"], "browser_readiness_unavailable");
}

#[tokio::test]
async fn browser_doctor_readiness_never_skips_auth_in_tokenless_loopback_config() {
    let env = TestEnv::new();
    let state = env
        .state
        .clone()
        .with_browser_readiness(Arc::new(|_| panic!("must not probe anonymously")));
    let app = task_api::router(state);
    let response = send(&app, get_with("/api/v1/browser/readiness", &[])).await;
    assert_eq!(response.status.as_u16(), 401);
}
