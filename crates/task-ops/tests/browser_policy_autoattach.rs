//! ADR 2026-10-08-browser-prod-enablement D4: 起票・retry 時の最小 browser policy の自動付与と
//! retry での引き継ぎ（`task_ops::add::create_task` と `task_ops::retry::retry_task` を本物の store で通す）。

use task_core::browser_wait::BrowserWaitStore;
use task_core::org::{OrgKind, OrgNode};
use task_core::{
    BrowserAction, BrowserCapability, BrowserDomainMode, BrowserSitePolicy, BrowserTaskPolicy,
    SqliteStore, Task, TaskStore, Trigger,
};
use task_ops::add::{NewTaskSpec, create_task};
use task_ops::retry::retry_task;
use time::OffsetDateTime;

const ORIGIN: &str = "https://manaba.example";

fn store_with_grant() -> SqliteStore {
    let store = SqliteStore::open_in_memory().expect("store");
    let now = OffsetDateTime::now_utc();
    for (id, origin) in [("manaba", ORIGIN), ("elsewhere", "https://other.example")] {
        store
            .browser_site_policy_upsert(
                &BrowserSitePolicy {
                    policy_id: id.into(),
                    exact_origin: origin.into(),
                    login_url: format!("{origin}/login"),
                    password_selector: "#p".into(),
                    submit_selector: None,
                },
                "admin",
                now,
            )
            .expect("site policy");
    }
    let mut node = OrgNode {
        profile: Default::default(),
        id: "secretary".into(),
        parent_id: None,
        name: "秘書".into(),
        kind: OrgKind::Secretary,
        genre: None,
        brief: String::new(),
        position: 0,
        created_at: now,
        updated_at: now,
    };
    node.profile.browser = Some(BrowserCapability {
        allowed_domains: vec![ORIGIN.into(), "https://other.example".into()],
        allowed_actions: None,
        credential_policy_ids: vec!["manaba".into(), "elsewhere".into()],
        credential_identity_ids: Default::default(),
        live_view_url: None,
    });
    store.org_upsert(&node).expect("org");
    store
}

fn create(store: &SqliteStore, browser: bool) -> Task {
    let mut spec = serde_json::json!({
        "title": "manaba の課題を見る",
        "objective": "課題の一覧を取る",
        "acceptance": [{"type": "command", "cmd": "true"}],
    });
    if browser {
        spec["skills"] = serde_json::json!(["browser-enabled"]);
        spec["requirements"] = serde_json::json!({"browser": {"allowed_domains": [ORIGIN]}});
    }
    let spec: NewTaskSpec = serde_json::from_value(spec).expect("spec");
    create_task(store, spec, OffsetDateTime::now_utc()).expect("create")
}

fn cancel(store: &SqliteStore, task: &Task) {
    store
        .apply_transition(task.id, Trigger::Cancel, None)
        .expect("cancel");
}

fn human_policy() -> BrowserTaskPolicy {
    BrowserTaskPolicy {
        policy_id: "human".into(),
        revision: 3,
        domain_mode: BrowserDomainMode::CommonHosts,
        navigation_origins: vec![],
        network_domains: vec![ORIGIN.into()],
        allowed_actions: vec![BrowserAction::Navigate, BrowserAction::Snapshot],
        approval_actions: vec![],
        credential_policy_ids: vec![],
        artifact_policy_id: None,
    }
}

#[test]
fn browser_policy_autoattach_on_create() {
    let store = store_with_grant();
    let task = create(&store, true);
    let policy = store
        .browser_task_policy_get(task.id)
        .expect("get")
        .expect("自動付与されている");
    assert_eq!(policy.policy_id, "auto");
    assert_eq!(policy.revision, 1);
    assert_eq!(policy.network_domains, vec![ORIGIN.to_string()]);
    // origin が requirements に入る site policy だけ（grant にあっても他の origin は入れない）。
    assert_eq!(policy.credential_policy_ids, vec!["manaba".to_string()]);
    let mut expected = BrowserAction::PHASE1.to_vec();
    expected.push(BrowserAction::CredentialUse);
    assert_eq!(policy.allowed_actions, expected);
    // 承認方針は変えない: standing approval を作らず、click/download/credential_use は毎回承認。
    assert!(policy.approval_actions.is_empty());
    assert!(policy.artifact_policy_id.is_none());

    // browser 要件の無い task には付けない。
    let plain = create(&store, false);
    assert!(
        store
            .browser_task_policy_get(plain.id)
            .expect("get")
            .is_none()
    );
}

#[test]
fn browser_policy_autoattach_without_grant_has_no_credential() {
    let store = SqliteStore::open_in_memory().expect("store");
    let task = create(&store, true);
    let policy = store
        .browser_task_policy_get(task.id)
        .expect("get")
        .expect("policy");
    assert!(policy.credential_policy_ids.is_empty());
    assert_eq!(policy.allowed_actions, BrowserAction::PHASE1.to_vec());
}

#[test]
fn browser_policy_autoattach_retry_inherits_original_policy() {
    let store = store_with_grant();
    let original = create(&store, true);
    let auto = store
        .browser_task_policy_get(original.id)
        .expect("get")
        .expect("policy");
    cancel(&store, &original);
    let retried =
        retry_task(&store, original.id, true, None, OffsetDateTime::now_utc()).expect("retry");
    assert_ne!(
        retried.task_id, original.id,
        "retry は新しい task id を作る"
    );
    let inherited = store
        .browser_task_policy_get(retried.task_id)
        .expect("get")
        .expect("新しい task に policy が引き継がれている");
    assert_eq!(inherited, auto);
}

#[test]
fn browser_policy_autoattach_does_not_overwrite_human_put() {
    let store = store_with_grant();
    let task = create(&store, true);
    // 人の PUT（ready の間）で自動付与を置き換える。
    store
        .browser_task_policy_set(task.id, &human_policy())
        .expect("put");
    assert_eq!(
        store.browser_task_policy_get(task.id).expect("get"),
        Some(human_policy())
    );
    // retry は自動付与し直さず、人の policy を写す（revision だけ 1 に戻す）。
    cancel(&store, &task);
    let retried =
        retry_task(&store, task.id, true, None, OffsetDateTime::now_utc()).expect("retry");
    let inherited = store
        .browser_task_policy_get(retried.task_id)
        .expect("get")
        .expect("policy");
    let mut expected = human_policy();
    expected.revision = 1;
    assert_eq!(inherited, expected);
    // 元の task の人の policy もそのまま。
    assert_eq!(
        store.browser_task_policy_get(task.id).expect("get"),
        Some(human_policy())
    );
}

#[test]
fn browser_policy_autoattach_put_accepted_while_prerequisite_blocked() {
    let store = store_with_grant();
    let task = create(&store, true);
    store
        .apply_transition(task.id, Trigger::Accept, None)
        .expect("accept");
    store
        .apply_transition(task.id, Trigger::BrowserPrereqBlock, None)
        .expect("block");
    // D2 の前提 gate で止めた task は人が policy を直せる（直せば dispatcher が再開する）。
    store
        .browser_task_policy_set(task.id, &human_policy())
        .expect("put while browser_prerequisite blocked");
    assert_eq!(
        store.browser_task_policy_get(task.id).expect("get"),
        Some(human_policy())
    );
}
