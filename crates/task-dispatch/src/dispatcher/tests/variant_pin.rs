//! variant 付き試行の割当前検査（`dispatcher/variant_pin.rs`）。正の経路は既存 `matching::decide` の結果、
//! 負の経路は blocked＋理由 event。org は読むだけ（書き換えない）。LLM・外部通信なし。

use super::*;
use task_core::decision_log::CampaignIdentity;
use task_core::variant_wire::{CampaignOrgPin, CampaignVariantRef, effective_profile_sha256};

const H: &str = "b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44b6132a44";

fn seed(store: &dyn TaskStore, sections: &[&str]) {
    store
        .org_upsert(&org_node_of("cos", None, OrgKind::Secretary, None))
        .unwrap();
    for s in sections {
        let mut n = org_node_of(s, Some("cos"), OrgKind::Section, None);
        n.profile.harnesses.allowed = vec!["coding".into()];
        store.org_upsert(&n).unwrap();
    }
}

fn pin_for(store: &dyn TaskStore, nodes: &[&str], coding: &str) -> CampaignOrgPin {
    let org = store.org_list().unwrap();
    CampaignOrgPin {
        nodes: nodes.iter().map(|s| s.to_string()).collect(),
        coding_node: coding.into(),
        effective_profile_sha256: effective_profile_sha256(&task_core::resolve_profile(
            &org, coding,
        ))
        .unwrap(),
    }
}

fn identity(pin: CampaignOrgPin) -> CampaignIdentity {
    CampaignIdentity {
        campaign_id: "camp-pin".into(),
        case_id: Some("p1".into()),
        arm: Some("A".into()),
        repetition: Some(1),
        manifest_hash: None,
        routing_setting: None,
        variant: Some(CampaignVariantRef {
            comparison_id: "cmp".into(),
            id: "M1-spec-off".into(),
            content_sha256: H.into(),
            selection_sha256: H.into(),
        }),
        variant_org: Some(pin),
    }
}

struct Fx {
    sqlite: Arc<SqliteStore>,
    store: Arc<dyn TaskStore>,
    dir: tempfile::TempDir,
}

fn fx() -> Fx {
    let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
    let store: Arc<dyn TaskStore> = sqlite.clone();
    Fx {
        sqlite,
        store,
        dir: tempfile::tempdir().unwrap(),
    }
}

impl Fx {
    fn task(&self, parent: Option<TaskId>, assignee: Option<&str>) -> Task {
        let mut t = new_task(
            self.dir.path(),
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
            1,
        );
        t.genre = Some("coding".into());
        t.parent_id = parent;
        t.assignee = assignee.map(str::to_string);
        self.store.insert(&t).unwrap();
        t
    }

    fn mark(&self, t: &Task, pin: CampaignOrgPin) {
        self.sqlite
            .task_campaign_set(&t.id.to_string(), &identity(pin), OffsetDateTime::now_utc())
            .unwrap();
    }

    fn assign(&self, t: Task) -> Option<Task> {
        let mut d = dispatcher(
            self.store.clone(),
            Arc::new(InstantAdapter {
                terminal: Terminal::Question { text: "x".into() },
                delay: Duration::ZERO,
            }),
            1,
        );
        d.assign_if_needed(t).unwrap()
    }

    fn blocked_with_reason(&self, id: TaskId) -> bool {
        let t = self.store.get(id).unwrap().unwrap();
        t.status == Status::Blocked
            && self.store.events_for(id).unwrap().iter().any(|(_, e)| {
                matches!(e, Event::QuestionRaised { text, .. } if text.contains("variant pin check failed"))
            })
    }
}

#[test]
fn spec_off_assigns_the_single_pinned_section_via_existing_matching() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let t = f.task(None, None);
    f.mark(&t, pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b"));
    let got = f.assign(t).expect("assigned");
    assert_eq!(got.assignee.as_deref(), Some("sw-b"));
}

#[test]
fn spec_on_assigns_per_matching_and_leaves_the_org_unchanged() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-a", "sw-b"]);
    let before = f.store.org_list().unwrap();
    let t = f.task(None, None);
    f.mark(
        &t,
        pin_for(f.store.as_ref(), &["cos", "sw-a", "sw-b"], "sw-a"),
    );
    let got = f.assign(t).expect("assigned");
    assert!(got.assignee.is_some());
    assert_eq!(f.store.org_list().unwrap(), before);
}

#[test]
fn org_node_drift_blocks_with_a_reason_event_and_does_not_assign() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let t = f.task(None, None);
    f.mark(&t, pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b"));
    // 共有 org にあとから課が増えた（spec-off の単一 node が崩れる）。
    seed(f.store.as_ref(), &["sw-extra"]);
    let id = t.id;
    assert!(f.assign(t).is_none());
    assert!(f.blocked_with_reason(id));
    assert!(f.store.get(id).unwrap().unwrap().assignee.is_none());
}

#[test]
fn coding_profile_drift_blocks() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let t = f.task(None, None);
    f.mark(&t, pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b"));
    let mut n = f.store.org_get("sw-b").unwrap().unwrap();
    n.profile.skills = vec!["drifted".into()];
    f.store.org_upsert(&n).unwrap();
    let id = t.id;
    assert!(f.assign(t).is_none());
    assert!(f.blocked_with_reason(id));
}

#[test]
fn descendant_inherits_the_pin_and_drift_blocks_it() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let root = f.task(None, Some("sw-b"));
    f.mark(&root, pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b"));
    let child = f.task(Some(root.id), None);
    let ok = f.assign(child.clone()).expect("child assigned");
    assert_eq!(ok.assignee.as_deref(), Some("sw-b"));
    seed(f.store.as_ref(), &["sw-extra"]);
    let grand = f.task(Some(child.id), None);
    let id = grand.id;
    assert!(f.assign(grand).is_none());
    assert!(f.blocked_with_reason(id));
}

#[test]
fn retry_keeps_the_existing_assignee_when_it_is_the_coding_node() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let t = f.task(None, Some("sw-b"));
    f.mark(&t, pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b"));
    let got = f.assign(t).expect("kept");
    assert_eq!(got.assignee.as_deref(), Some("sw-b"));
}

#[test]
fn retry_assignee_outside_the_single_section_is_blocked() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let t = f.task(None, Some("cos"));
    f.mark(&t, pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b"));
    let id = t.id;
    assert!(f.assign(t).is_none());
    assert!(f.blocked_with_reason(id));
}

#[test]
fn task_without_variant_is_matched_as_before() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-a", "sw-b"]);
    let t = f.task(None, None);
    let got = f.assign(t).expect("assigned");
    assert!(got.assignee.is_some());
}

fn checker(f: &Fx) -> Dispatcher {
    dispatcher(
        f.store.clone(),
        Arc::new(InstantAdapter {
            terminal: Terminal::Question { text: "x".into() },
            delay: Duration::ZERO,
        }),
        1,
    )
}

#[test]
fn start_check_passes_for_child_retry_and_grandchild_while_org_matches() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let root = f.task(None, Some("sw-b"));
    f.mark(&root, pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b"));
    let child = f.task(Some(root.id), Some("sw-b"));
    let grand = f.task(Some(child.id), Some("sw-b"));
    let d = checker(&f);
    for t in [&root, &child, &grand] {
        assert_eq!(d.variant_run_pin(t, None).map(|_| ()), Ok(()));
    }
}

#[test]
fn start_check_catches_org_drift_after_assignment_for_every_generation() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let root = f.task(None, Some("sw-b"));
    f.mark(&root, pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b"));
    let child = f.task(Some(root.id), Some("sw-b"));
    let grand = f.task(Some(child.id), Some("sw-b"));
    let d = checker(&f);
    // 割当後（run 起動前）に共有 org が変わった。
    let mut n = f.store.org_get("sw-b").unwrap().unwrap();
    n.profile.skills = vec!["drifted".into()];
    f.store.org_upsert(&n).unwrap();
    for t in [&root, &child, &grand] {
        assert!(d.variant_run_pin(t, None).map(|_| ()).is_err());
    }
}

#[test]
fn start_check_rejects_an_assignee_outside_the_pin_and_a_missing_assignee() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let root = f.task(None, Some("cos"));
    f.mark(&root, pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b"));
    let none = f.task(Some(root.id), None);
    let d = checker(&f);
    assert!(d.variant_run_pin(&root, None).map(|_| ()).is_err());
    assert!(d.variant_run_pin(&none, None).map(|_| ()).is_err());
}

#[test]
fn start_check_is_a_noop_without_variant() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-a", "sw-b"]);
    let t = f.task(None, None);
    assert_eq!(checker(&f).variant_run_pin(&t, None).map(|_| ()), Ok(()));
}

#[test]
fn contradicting_child_pin_is_rejected_at_save_and_root_pin_wins() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-a", "sw-b"]);
    let root = f.task(None, None);
    f.mark(
        &root,
        pin_for(f.store.as_ref(), &["cos", "sw-a", "sw-b"], "sw-a"),
    );
    let child = f.task(Some(root.id), None);
    let other = pin_for(f.store.as_ref(), &["cos", "sw-a", "sw-b"], "sw-b");
    // 保存側が祖先と矛盾する識別を拒否するので、子は root の pin のまま解決される。
    assert!(
        f.sqlite
            .task_campaign_set(
                &child.id.to_string(),
                &identity(other),
                OffsetDateTime::now_utc(),
            )
            .is_err()
    );
    let d = checker(&f);
    let got = d.variant_org_pin(&child).unwrap().expect("root pin");
    assert_eq!(got.coding_node, "sw-a");
}

// ---- run 起動点: RunContext.campaign が worker（adapter）へ届く ----

type Seen = Arc<StdMutex<Vec<Option<task_worker::protocol::CampaignRunPin>>>>;

/// `req.context.campaign` と run の有無を記録するだけの stub（LLM・外部通信なし）。
struct CampaignCapture {
    seen: Seen,
}

#[async_trait::async_trait]
impl WorkerAdapter for CampaignCapture {
    fn id(&self) -> &str {
        "instant"
    }
    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.seen.lock().unwrap().push(req.context.campaign.clone());
        Ok(RunOutcome {
            terminal: Terminal::Question { text: "x".into() },
            exit_code: Some(0),
        })
    }
}

fn capturing(f: &Fx, observe: bool) -> (Dispatcher, Seen) {
    let seen: Seen = Arc::default();
    let mut d = dispatcher(
        f.store.clone(),
        Arc::new(CampaignCapture { seen: seen.clone() }),
        1,
    );
    if observe {
        d.set_variant_observation("inst-1".into(), H.into());
    }
    (d, seen)
}

async fn start_one(d: &mut Dispatcher, id: TaskId) {
    d.tick().unwrap();
    if d.running.contains_key(&RunKey {
        task: id,
        work_unit: None,
    }) {
        await_worker_completion(d, id).await;
    }
}

#[tokio::test]
async fn variant_pin_reaches_the_worker_for_root_and_child() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let pin = pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b");
    let root = f.task(None, Some("sw-b"));
    f.mark(&root, pin.clone());
    let (mut d, seen) = capturing(&f, true);
    start_one(&mut d, root.id).await;
    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "root run started");
    let c = got[0].as_ref().expect("campaign pin in RunContext");
    assert_eq!(c.identity.variant_org.as_ref(), Some(&pin));
    assert_eq!(c.daemon_instance_id, "inst-1");
    assert_eq!(c.config_sha256, H);
    assert_eq!(
        c.org_nodes_sha256,
        task_core::variant_wire::org_effective_digest(&f.store.org_list().unwrap()).unwrap()
    );
    assert!(!c.selected.harness_id.is_empty() && !c.selected.model_id.is_empty());
    c.validate().unwrap();

    // 子は祖先の pin を連鎖解決で継ぎ、同じ identity が届く。
    let child = f.task(Some(root.id), Some("sw-b"));
    let (mut d2, seen2) = capturing(&f, true);
    start_one(&mut d2, child.id).await;
    let got2 = seen2.lock().unwrap().clone();
    assert_eq!(got2.len(), 1, "child run started");
    assert_eq!(got2[0].as_ref().unwrap().identity, c.identity);
}

#[tokio::test]
async fn variant_plain_task_gets_no_campaign_pin() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let t = f.task(None, Some("sw-b"));
    let (mut d, seen) = capturing(&f, true);
    start_one(&mut d, t.id).await;
    let got = seen.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    assert!(got[0].is_none());
}

#[tokio::test]
async fn variant_task_without_daemon_observation_is_blocked_before_the_run() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let root = f.task(None, Some("sw-b"));
    f.mark(&root, pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b"));
    let (mut d, seen) = capturing(&f, false);
    start_one(&mut d, root.id).await;
    assert!(seen.lock().unwrap().is_empty(), "no run was started");
    assert!(f.blocked_with_reason(root.id));
}

#[tokio::test]
async fn variant_org_drift_at_start_blocks_the_run_and_delivers_nothing() {
    let f = fx();
    seed(f.store.as_ref(), &["sw-b"]);
    let root = f.task(None, Some("sw-b"));
    f.mark(&root, pin_for(f.store.as_ref(), &["cos", "sw-b"], "sw-b"));
    let mut n = org_node_of("sw-b", Some("cos"), OrgKind::Section, None);
    n.profile.harnesses.allowed = vec!["coding".into(), "other".into()];
    f.store.org_upsert(&n).unwrap();
    let (mut d, seen) = capturing(&f, true);
    start_one(&mut d, root.id).await;
    assert!(seen.lock().unwrap().is_empty());
    assert!(f.blocked_with_reason(root.id));
}
