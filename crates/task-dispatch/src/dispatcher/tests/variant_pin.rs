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
