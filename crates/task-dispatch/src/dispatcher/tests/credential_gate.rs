//! ADR 2026-10-15-mlsys-min-decision-assisted D4: credential 未登録の入口（`credential_gate_hold`）と
//! 起動時の復元。待つのは該当 work unit だけで、受信箱 item は秘密 id ごとに 1 件、値はどこにも出ない。

use super::*;
use crate::dispatcher::route_select::RoutePick;
use crate::dispatcher::{CredentialGate, CredentialGates};
use task_core::WorkUnitRow;
use task_core::execution_plan::{
    WorkUnitBlockedReason, WorkUnitContext, WorkUnitKind, WorkUnitSpec, WorkUnitStatus,
};
use task_core::llm_route::Candidate;

const SECRET: &str = "jev_api_key";
const SECRET_VALUE: &str = "sk-dummy-credential-value-123";

fn unit(task: &Task, key: &str, seq: u32) -> WorkUnitRow {
    let spec = WorkUnitSpec {
        key: key.to_string(),
        kind: WorkUnitKind::Implement,
        title: key.to_string(),
        objective: format!("do {key}"),
        depends_on: vec![],
        done_when: vec![],
        checks: vec![],
        context: WorkUnitContext::default(),
        harness: None,
        features: None,
        budget: None,
        outputs: vec![],
        phase: None,
    };
    WorkUnitRow::new(
        format!("{}-{key}", task.id),
        task.id.to_string(),
        "plan".to_string(),
        seq,
        spec,
        WorkUnitStatus::Ready,
        "2026-10-15T00:00:00Z".to_string(),
    )
}

fn jev_pick() -> RoutePick {
    RoutePick {
        harness: "h".into(),
        candidate: Candidate {
            source: "jev".into(),
            model: "m".into(),
            api_format: "openai_compatible".into(),
        },
        wire_model_id: "m".into(),
        account: "shared".into(),
    }
}

struct Env {
    _dir: tempfile::TempDir,
    secrets: PathBuf,
    store: Arc<dyn TaskStore>,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let secrets = dir.path().join("secrets");
        std::fs::create_dir_all(&secrets).unwrap();
        let store: Arc<dyn TaskStore> =
            Arc::new(SqliteStore::open(&dir.path().join("db.sqlite")).unwrap());
        Env {
            _dir: dir,
            secrets,
            store,
        }
    }

    fn dispatcher(&self) -> Dispatcher {
        let mut d = pool_dispatcher(
            self.store.clone(),
            done_adapter(),
            None,
            PathBuf::from("/accounts/claude"),
            1,
            1,
        );
        d.set_credential_gates(CredentialGates {
            secrets_dir: Some(self.secrets.clone()),
            by_source: HashMap::from([(
                "jev".into(),
                CredentialGate {
                    secret_id: SECRET.into(),
                    provider: "jev".into(),
                    purpose: "Jev で依頼を分類する".into(),
                },
            )]),
        });
        d
    }

    /// 計画つきの task（unit `a`・兄弟 `b`）。
    fn task_with_units(&self) -> (Task, WorkUnitRow, WorkUnitRow) {
        let task = new_task(self._dir.path(), Check::Reviewer, 1);
        self.store.insert(&task).unwrap();
        let a = unit(&task, "a", 0);
        let b = unit(&task, "b", 1);
        self.store
            .work_units_apply(task.id, vec![a.clone(), b.clone()], Vec::new(), Vec::new())
            .unwrap();
        (task, a, b)
    }

    fn unit_state(
        &self,
        task: &Task,
        key: &str,
    ) -> (WorkUnitStatus, Option<WorkUnitBlockedReason>) {
        let u = self
            .store
            .work_units_for(task.id)
            .unwrap()
            .into_iter()
            .find(|u| u.key == key)
            .unwrap();
        (u.status, u.blocked_reason)
    }

    fn inbox_items(&self) -> Vec<task_ops::human_inbox::InboxItem> {
        let ctx = task_ops::view::ViewContext {
            workspace_roots: task_core::local_machine::WorkspaceRoots::new(
                self._dir.path().to_path_buf(),
                None,
            ),
            retry_backoff_base: Duration::ZERO,
            retry_backoff_max: Duration::ZERO,
            max_requeues: 5,
            clusters: Default::default(),
        };
        task_ops::human_inbox::human_inbox(
            self.store.as_ref(),
            None,
            &ctx,
            OffsetDateTime::now_utc(),
            &|_, _| Vec::new(),
        )
        .unwrap()
        .items
        .into_iter()
        .filter(|i| i.kind == task_ops::human_inbox::InboxKind::CredentialWait)
        .collect()
    }
}

#[test]
fn missing_credential_blocks_only_that_unit_and_leaves_the_task_and_siblings() {
    let env = Env::new();
    let mut d = env.dispatcher();
    let (task, a, _b) = env.task_with_units();

    assert!(
        d.credential_gate_hold(&task, Some(&a), &jev_pick())
            .unwrap()
    );

    assert_eq!(
        env.unit_state(&task, "a"),
        (
            WorkUnitStatus::Blocked,
            Some(WorkUnitBlockedReason::WaitingForAuth)
        )
    );
    // 兄弟は止めない。task の Status も増やさない（ready のまま）。
    assert_eq!(env.unit_state(&task, "b"), (WorkUnitStatus::Ready, None));
    assert_eq!(
        env.store.get(task.id).unwrap().unwrap().status,
        Status::Ready
    );
    let waits = env.store.credential_waits_list_waiting().unwrap();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].secret_id, SECRET);
    assert_eq!(waits[0].work_unit_id, a.id);
}

#[test]
fn registered_credential_passes_the_gate_and_an_undeclared_source_is_never_gated() {
    let env = Env::new();
    let mut d = env.dispatcher();
    let (task, a, _b) = env.task_with_units();
    std::fs::write(env.secrets.join(SECRET), format!("{SECRET_VALUE}\n")).unwrap();
    assert!(
        !d.credential_gate_hold(&task, Some(&a), &jev_pick())
            .unwrap()
    );
    let mut other = jev_pick();
    other.candidate.source = "qwen".into();
    std::fs::remove_file(env.secrets.join(SECRET)).unwrap();
    assert!(!d.credential_gate_hold(&task, Some(&a), &other).unwrap());
    assert!(
        env.store
            .credential_waits_list_waiting()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn tasks_waiting_for_the_same_secret_share_one_inbox_item() {
    let env = Env::new();
    let mut d = env.dispatcher();
    let (t1, a1, _) = env.task_with_units();
    let (t2, a2, _) = env.task_with_units();
    assert!(d.credential_gate_hold(&t1, Some(&a1), &jev_pick()).unwrap());
    assert!(d.credential_gate_hold(&t2, Some(&a2), &jev_pick()).unwrap());
    // 同じ gate を繰り返しても wait も item も増えない（冪等）。
    assert!(d.credential_gate_hold(&t1, Some(&a1), &jev_pick()).unwrap());

    let items = env.inbox_items();
    assert_eq!(items.len(), 1, "{items:?}");
    let item = &items[0];
    assert_eq!(item.id, "credential_wait-jev_api_key");
    assert_eq!(item.blocking.tasks.len(), 2);
    assert_eq!(item.blocking.units.len(), 2);
    let detail = item.detail.clone().unwrap();
    assert!(detail.contains("Jev で依頼を分類する"), "{detail}");
    assert!(detail.contains("/accounts"), "{detail}");
    assert!(detail.contains("自動で続行"), "{detail}");
    assert!(item.links.iter().any(|l| l.href == "/accounts"));
    assert_eq!(env.store.credential_waits_list_waiting().unwrap().len(), 2);
    // unit の遷移 event は 1 回だけ。
    let blocked_events = env
        .store
        .events_for(t1.id)
        .unwrap()
        .iter()
        .filter(|e| format!("{e:?}").contains("waiting_for_auth"))
        .count();
    assert_eq!(blocked_events, 1);
}

#[test]
fn waits_are_restored_after_a_restart_and_a_half_entered_wait_is_finished() {
    let env = Env::new();
    let (t1, a1, _) = env.task_with_units();
    let (t2, a2, _) = env.task_with_units();
    {
        let mut d = env.dispatcher();
        assert!(d.credential_gate_hold(&t1, Some(&a1), &jev_pick()).unwrap());
    }
    // 入口の途中で落ちた: wait だけ書かれ、unit は ready のまま。
    env.store
        .credential_wait_enter(
            &task_core::CredentialWaitEnter {
                work_unit_id: &a2.id,
                task_id: t2.id,
                secret_id: SECRET,
                provider: "jev",
                purpose: "Jev で依頼を分類する",
            },
            OffsetDateTime::now_utc(),
        )
        .unwrap();
    assert_eq!(env.unit_state(&t2, "a"), (WorkUnitStatus::Ready, None));

    // 再起動（新しい dispatcher）。
    let mut restarted = env.dispatcher();
    let restored = restarted.restore_credential_waits().unwrap();
    assert_eq!(restored.len(), 2);
    assert_eq!(
        env.unit_state(&t2, "a"),
        (
            WorkUnitStatus::Blocked,
            Some(WorkUnitBlockedReason::WaitingForAuth)
        )
    );
    assert_eq!(env.inbox_items().len(), 1);
}

#[test]
fn restore_closes_the_wait_of_a_cancelled_task() {
    let env = Env::new();
    let mut d = env.dispatcher();
    let (task, a, _b) = env.task_with_units();
    assert!(
        d.credential_gate_hold(&task, Some(&a), &jev_pick())
            .unwrap()
    );
    env.store
        .apply_transition(task.id, task_core::Trigger::Cancel, None)
        .unwrap();
    let restored = env.dispatcher().restore_credential_waits().unwrap();
    assert!(restored.is_empty());
    assert!(
        env.store
            .credential_waits_list_waiting()
            .unwrap()
            .is_empty()
    );
    assert!(env.inbox_items().is_empty());
}

#[test]
fn a_task_without_a_plan_waits_as_one_unit_and_never_rewrites_the_wait() {
    let env = Env::new();
    let mut d = env.dispatcher();
    let task = new_task(env._dir.path(), Check::Reviewer, 1);
    env.store.insert(&task).unwrap();
    assert!(d.credential_gate_hold(&task, None, &jev_pick()).unwrap());
    assert!(d.credential_gate_hold(&task, None, &jev_pick()).unwrap());
    let waits = env.store.credential_waits_list_waiting().unwrap();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].work_unit_id, format!("task:{}", task.id));
}

#[test]
fn the_secret_value_appears_in_no_wait_row_inbox_item_event_or_worker_route() {
    let env = Env::new();
    let mut d = env.dispatcher();
    let (task, a, _b) = env.task_with_units();
    // 登録済みの鍵があっても gate は値を読み捨てるだけで、どこにも写さない。
    std::fs::write(env.secrets.join("other"), SECRET_VALUE).unwrap();
    assert!(
        d.credential_gate_hold(&task, Some(&a), &jev_pick())
            .unwrap()
    );
    let mut haystack = format!("{:?}", env.store.credential_waits_list_waiting().unwrap());
    haystack.push_str(&format!("{:?}", env.inbox_items()));
    haystack.push_str(&format!("{:?}", env.store.events_for(task.id).unwrap()));
    assert!(!haystack.contains(SECRET_VALUE));

    // worker に渡る経路は参照（env 名）だけ。値は adapter / broker の境界で注入される。
    let mut registry = task_core::llm_route::Registry::default();
    registry.sources.insert(
        "jev".into(),
        task_core::llm_route::SourceSpec {
            kind: "openai_compatible".into(),
            api_format: "openai_compatible".into(),
            accounts: vec![],
            models: vec!["m".into()],
            max_runs: None,
            account_max_runs: Default::default(),
            quota: None,
            endpoint: Some("https://openrouter.ai/api".into()),
            credential_ref: Some(SECRET.into()),
        },
    );
    let route = crate::dispatcher::selected_llm_route(&registry, &jev_pick(), None).unwrap();
    let wire = serde_json::to_string(&route).unwrap();
    assert!(wire.contains(SECRET));
    assert!(!wire.contains(SECRET_VALUE));
}
