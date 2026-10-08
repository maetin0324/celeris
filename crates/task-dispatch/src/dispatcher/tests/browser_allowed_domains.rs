use super::super::worker_task::browser_run_policy;
use super::*;

// ---- ADR 2026-10-05-browser-department-web-live-view D2.0: task ∩ grant at run time ----

fn browser_org(store: &dyn TaskStore, grant: &[&str]) {
    let mut node = org_node_of(
        "browser-execution",
        Some("secretary"),
        OrgKind::Department,
        None,
    );
    node.profile.browser = Some(task_core::BrowserCapability {
        approval_actions: vec![],
        allowed_domains: grant.iter().map(|o| o.to_string()).collect(),
        ..Default::default()
    });
    for n in [
        org_node_of("secretary", None, OrgKind::Secretary, Some("secretary")),
        node,
    ] {
        store.org_upsert(&n).unwrap();
    }
}

/// The run context hands the worker the task policy narrowed to the task's origins and the grant
/// read from the org for this run; shrinking the grant after the task was created applies to
/// the existing task's next run (nothing from creation time is kept).
#[tokio::test]
async fn browser_allowed_domains_grant_shrink_reaches_next_run_context() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    browser_org(store.as_ref(), &["https://*.example.com"]);
    let mut task = new_task(
        dir.path(),
        Check::Command {
            cmd: "true".into(),
            expect_exit: 0,
        },
        0,
    );
    task.assignee = Some("browser-execution".into());
    task.skills = vec![task_core::browser::BROWSER_SKILL.into()];
    task.requirements.browser = Some(task_core::BrowserRequirements {
        allowed_domains: vec![
            "https://billing.example.com".into(),
            "https://app.example.com".into(),
        ],
    });
    store.insert(&task).unwrap();
    store
        .browser_task_policy_set(
            task.id,
            &task_core::BrowserTaskPolicy {
                policy_id: "p".into(),
                revision: 1,
                domain_mode: task_core::BrowserDomainMode::CommonHosts,
                navigation_origins: vec![],
                network_domains: vec!["https://*.example.com".into()],
                allowed_actions: vec![task_core::BrowserAction::Navigate],
                approval_actions: vec![],
                credential_policy_ids: vec![],
                artifact_policy_id: None,
            },
        )
        .unwrap();
    let adapter = Arc::new(FileAdapter {
        plan_json: String::new(),
        review_json: String::new(),
        delay: Duration::from_millis(5),
    });
    let d = dispatcher(store.clone(), adapter, 1);
    let effective = |d: &Dispatcher| {
        let extras = d.run_extras(&task, None, None, "claude-code").unwrap();
        let grant = extras.profile.unwrap().browser.unwrap();
        let policy = browser_run_policy(store.as_ref(), &task).unwrap();
        assert_eq!(
            policy.as_ref().unwrap().network_domains,
            ["https://app.example.com", "https://billing.example.com"]
        );
        task_worker::browser_policy::prepare(&grant, policy.as_ref(), "0.38.1")
            .map(|p| p.allowed_domains().to_vec())
    };
    assert_eq!(
        effective(&d).unwrap(),
        ["https://app.example.com", "https://billing.example.com"]
    );
    browser_org(store.as_ref(), &["https://app.example.com"]);
    assert_eq!(effective(&d).unwrap(), ["https://app.example.com"]);
    browser_org(store.as_ref(), &["http://127.0.0.1:3000"]);
    assert_eq!(
        effective(&d).unwrap_err(),
        task_core::BrowserPolicyError::EmptyDomains
    );
}
