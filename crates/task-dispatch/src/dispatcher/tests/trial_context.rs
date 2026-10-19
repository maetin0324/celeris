//! ADR 2026-10-17-mlsys-trial-context-isolation: campaign 試行に属する task（worker・planner・子 task・
//! reviewer）の run 文脈は試行用 `kb/` だけを使い、通常 KB・skills・記憶・会話履歴・永続の認可を載せない。
//! 実 Dispatcher の tick を通し、adapter が受け取った `RunContext` と env を canary で確かめる。

use super::*;

const NORMAL_KB: &str = "NORMAL_KB_CANARY_7f3a";
const SKILL: &str = "SKILL_CANARY_51c2";
const MEMORY: &str = "MEMORY_CANARY_9d04";
const CHAT: &str = "CHAT_CANARY_2b8e";
const RULE: &str = "RULE_CANARY_c6a1";
const TRIAL_KB: &str = "TRIAL_KB_CANARY_e19f";
const NODE: &str = "software-engineering";

/// (run の task id, kind, RunContext の JSON, adapter に重ねた env)。
type CapturedRun = (TaskId, TaskKind, String, Vec<(String, String)>);
type Captured = Vec<CapturedRun>;

#[derive(Clone)]
struct ContextCaptureAdapter {
    seen: Arc<StdMutex<Captured>>,
    env: Vec<(String, String)>,
}

#[async_trait]
impl WorkerAdapter for ContextCaptureAdapter {
    fn id(&self) -> &str {
        "instant"
    }

    fn with_env(&self, extra: &[(String, String)]) -> Option<Arc<dyn WorkerAdapter>> {
        let mut next = self.clone();
        next.env.extend(extra.iter().cloned());
        Some(Arc::new(next))
    }

    async fn run(
        &self,
        req: RunRequest,
        _run_id: &str,
        _limits: RunLimits,
        _sink: &dyn EventSink,
    ) -> Result<RunOutcome, AdapterError> {
        self.seen.lock().unwrap().push((
            req.task.id,
            req.task.kind,
            serde_json::to_string(&req.context).unwrap(),
            self.env.clone(),
        ));
        if req.task.kind == TaskKind::Review {
            std::fs::create_dir_all(&req.artifacts_dir).unwrap();
            std::fs::write(
                req.artifacts_dir.join("review.json"),
                r#"{"verdicts":[{"criterion":0,"pass":true,"reason":"ok"}]}"#,
            )
            .unwrap();
        }
        Ok(RunOutcome {
            terminal: Terminal::Done {
                summary: "ok".into(),
                evidence: vec![],
                usage: None,
            },
            exit_code: Some(0),
        })
    }
}

struct Fixture {
    captured: Captured,
    worker: TaskId,
    planner: TaskId,
    child: TaskId,
    trial_kb: PathBuf,
}

fn identity(arm: &str) -> task_core::decision_log::CampaignIdentity {
    task_core::decision_log::CampaignIdentity {
        campaign_id: "camp-trial-context".into(),
        case_id: Some("p1".into()),
        arm: Some(arm.into()),
        repetition: Some(1),
        manifest_hash: None,
        routing_setting: None,
        variant: None,
        variant_org: None,
    }
}

/// 通常運用の KB（`kb:user` の default mount）・skill・記憶・会話・認可に canary を置き、試行の
/// 作業場所の `kb/` には別の canary を置いて、worker・planner・子 task（と reviewer）を実 tick で走らせる。
fn run_with_canaries(trial: bool) -> Fixture {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let sqlite = Arc::new(SqliteStore::open_in_memory().unwrap());
        let store: Arc<dyn TaskStore> = sqlite.clone();
        let mut se = org_node_of(NODE, Some("engineering"), OrgKind::Section, None);
        se.profile.skills_mounts = vec!["canary-skill".into()];
        se.profile.knowledge = vec![task_core::KnowledgeMount::kb("user")];
        for node in [
            org_node_of("cos", None, OrgKind::Secretary, None),
            org_node_of("engineering", Some("cos"), OrgKind::Department, None),
            se,
        ] {
            store.org_upsert(&node).unwrap();
        }

        // 通常運用の KB（git の KB。索引に canary の題が出る）と skill。
        let kb = tempfile::tempdir().unwrap();
        task_ops::knowledge::init(kb.path()).unwrap();
        std::fs::create_dir_all(kb.path().join("user")).unwrap();
        std::fs::write(
            kb.path().join("user/normal.md"),
            format!("# {NORMAL_KB}\n\nbody\n"),
        )
        .unwrap();
        task_ops::knowledge::reindex(kb.path()).unwrap();
        let skill_dir = kb.path().join("skills/canary-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: canary-skill\ndescription: {SKILL}\nceleris-use: work, review\n---\nbody\n"),
        )
        .unwrap();

        // 担当の記憶・会話履歴・永続の認可。
        let memory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(memory.path().join(NODE)).unwrap();
        std::fs::write(
            memory.path().join(NODE).join("notes.md"),
            format!("- 2026-10-01: {MEMORY}\n"),
        )
        .unwrap();
        let now = OffsetDateTime::now_utc();
        store
            .message_append(&Message {
                id: MessageId::new(),
                node_id: NODE.into(),
                project_id: None,
                role: MessageRole::User,
                text: CHAT.into(),
                run_id: None,
                task_id: None,
                metadata: None,
                created_at: now,
            })
            .unwrap();
        store
            .standing_rule_append(&StandingRule {
                id: StandingRuleId::new(),
                node_id: None,
                rule: RULE.into(),
                created_at: now,
            })
            .unwrap();

        // 試行の作業場所（runner の build_seed と同じ形: 作業場所の直下に `kb/`）。
        let ws = tempfile::tempdir().unwrap();
        let trial_kb = ws.path().join("kb");
        std::fs::create_dir_all(&trial_kb).unwrap();
        std::fs::write(trial_kb.join(format!("{TRIAL_KB}.md")), "# trial page\n").unwrap();

        let set = |id: TaskId, arm: &str| {
            if trial {
                sqlite
                    .task_campaign_set(&id.to_string(), &identity(arm), now)
                    .unwrap();
            }
        };
        // worker（reviewer 付き）。
        let mut worker = new_task(ws.path(), Check::Reviewer, 0);
        worker.assignee = Some(NODE.into());
        store.insert(&worker).unwrap();
        set(worker.id, "B");
        // planner。
        let mut planner = plan_task(ws.path(), 0);
        planner.assignee = Some(NODE.into());
        store.insert(&planner).unwrap();
        set(planner.id, "C");
        // 子 task: 識別は親（試行の根）にだけあり、子の作業場所は別の dir。試行用 KB は根の作業場所から引く。
        let mut root = new_task(ws.path(), Check::Reviewer, 0);
        root.status = Status::Blocked;
        root.assignee = Some(NODE.into());
        store.insert(&root).unwrap();
        set(root.id, "C");
        let child_ws = tempfile::tempdir().unwrap();
        let mut child = new_task(
            child_ws.path(),
            Check::Command {
                cmd: "true".into(),
                expect_exit: 0,
            },
            0,
        );
        child.parent_id = Some(root.id);
        child.assignee = Some(NODE.into());
        store.insert(&child).unwrap();

        let seen = Arc::new(StdMutex::new(Vec::new()));
        let adapter = Arc::new(ContextCaptureAdapter {
            seen: seen.clone(),
            env: Vec::new(),
        });
        let mut d = dispatcher(store.clone(), adapter, 4);
        d.config.knowledge.root = kb.path().to_path_buf();
        d.config.knowledge.default_mounts = vec![task_core::KnowledgeMount::kb("user")];
        d.config.memory_dir = Some(memory.path().to_path_buf());
        run_until_idle(&mut d, 100).await;
        let captured = seen.lock().unwrap().clone();
        Fixture {
            captured,
            worker: worker.id,
            planner: planner.id,
            child: child.id,
            trial_kb,
        }
    })
}

fn runs_of(f: &Fixture, id: TaskId, kind: TaskKind) -> Vec<&CapturedRun> {
    f.captured
        .iter()
        .filter(|(t, k, _, _)| *t == id && *k == kind)
        .collect()
}

/// reviewer run は対象 task の子の Review task として走る（試行の識別は親の鎖で解決される）。
fn reviews_of(f: &Fixture) -> Vec<&CapturedRun> {
    f.captured
        .iter()
        .filter(|(_, k, _, _)| *k == TaskKind::Review)
        .collect()
}

#[test]
fn trial_context_runs_see_only_the_trial_kb() {
    let f = run_with_canaries(true);
    let expect_env = (
        "CELERIS_KNOWLEDGE_ROOT".to_string(),
        f.trial_kb.display().to_string(),
    );
    for (id, kind, label) in [
        (f.worker, TaskKind::Execute, "worker"),
        (f.planner, TaskKind::Plan, "planner"),
        (f.child, TaskKind::Execute, "child task"),
    ] {
        let runs = runs_of(&f, id, kind);
        assert!(!runs.is_empty(), "{label} run が走っていない");
        for (_, _, ctx, env) in runs {
            for canary in [NORMAL_KB, SKILL, MEMORY, CHAT, RULE] {
                assert!(
                    !ctx.contains(canary),
                    "{label} の RunContext に通常運用の {canary} が出た: {ctx}"
                );
            }
            assert!(
                ctx.contains(TRIAL_KB),
                "{label} の knowledge index に試行用 kb/ の canary が無い: {ctx}"
            );
            assert!(
                env.contains(&expect_env),
                "{label} の env が試行用 kb/ を根にしていない: {env:?}"
            );
        }
    }
    // reviewer: 通常 KB の skills も載せない（knowledge・記憶・会話は元から載せない）。
    let reviews = reviews_of(&f);
    assert!(!reviews.is_empty(), "reviewer run が走っていない");
    for (_, _, ctx, _) in reviews {
        for canary in [NORMAL_KB, SKILL, MEMORY, CHAT, RULE] {
            assert!(
                !ctx.contains(canary),
                "reviewer の RunContext に {canary}: {ctx}"
            );
        }
    }
}

#[test]
fn trial_context_leaves_non_trial_runs_unchanged() {
    let f = run_with_canaries(false);
    for (id, kind, label) in [
        (f.worker, TaskKind::Execute, "worker"),
        (f.planner, TaskKind::Plan, "planner"),
        (f.child, TaskKind::Execute, "child task"),
    ] {
        let runs = runs_of(&f, id, kind);
        assert!(!runs.is_empty(), "{label} run が走っていない");
        for (_, _, ctx, env) in runs {
            for canary in [NORMAL_KB, SKILL, MEMORY, CHAT, RULE] {
                assert!(
                    ctx.contains(canary),
                    "非試行の {label} から通常の {canary} が消えた: {ctx}"
                );
            }
            assert!(!ctx.contains(TRIAL_KB), "非試行の {label} に試行用 kb/");
            assert!(
                env.iter().all(|(k, _)| k != "CELERIS_KNOWLEDGE_ROOT"),
                "非試行の {label} に KB の根の env: {env:?}"
            );
        }
    }
    let reviews = reviews_of(&f);
    assert!(
        reviews.iter().any(|(_, _, ctx, _)| ctx.contains(SKILL)),
        "非試行の reviewer から skill が消えた"
    );
}
