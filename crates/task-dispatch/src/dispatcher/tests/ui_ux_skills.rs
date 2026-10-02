use super::*;

const UI_SKILLS: [&str; 4] = [
    "frontend-design",
    "shadcn",
    "web-design",
    "ui-ux-quality-gate",
];

type CapturedSkills = Vec<(TaskKind, Vec<String>)>;

struct SkillCaptureAdapter {
    seen: Arc<StdMutex<CapturedSkills>>,
}

#[async_trait]
impl WorkerAdapter for SkillCaptureAdapter {
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
        self.seen.lock().unwrap().push((
            req.task.kind,
            req.context.skills.iter().map(|s| s.name.clone()).collect(),
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

fn skill_names_for(assignee: &str) -> Vec<(TaskKind, Vec<String>)> {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
        let mut ui = org_node_of("ui-ux", Some("engineering"), OrgKind::Section, None);
        ui.profile.skills_mounts = UI_SKILLS.iter().map(|s| (*s).into()).collect();
        for node in [
            org_node_of("cos", None, OrgKind::Secretary, None),
            org_node_of("engineering", Some("cos"), OrgKind::Department, None),
            org_node_of(
                "software-engineering",
                Some("engineering"),
                OrgKind::Section,
                None,
            ),
            ui,
        ] {
            store.org_upsert(&node).unwrap();
        }
        let workspace = tempfile::tempdir().unwrap();
        let mut task = new_task(workspace.path(), Check::Reviewer, 0);
        task.assignee = Some(assignee.into());
        store.insert(&task).unwrap();
        let kb = tempfile::tempdir().unwrap();
        for name in UI_SKILLS {
            let dir = kb.path().join("skills").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let usage = if name == "ui-ux-quality-gate" {
                "celeris-use: work, review\n"
            } else {
                ""
            };
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {name}\n{usage}---\nbody\n"),
            )
            .unwrap();
        }
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let adapter = Arc::new(SkillCaptureAdapter { seen: seen.clone() });
        let mut dispatcher = dispatcher(store.clone(), adapter, 1);
        dispatcher.config.knowledge.root = kb.path().to_path_buf();
        assert!(run_until_idle(&mut dispatcher, 100).await.idle);
        assert_eq!(store.get(task.id).unwrap().unwrap().status, Status::Done);
        seen.lock().unwrap().clone()
    })
}

#[test]
fn ui_ux_skills_work_run_receives_four_mounted_skills() {
    let runs = skill_names_for("ui-ux");
    let work = runs
        .iter()
        .find(|(kind, _)| *kind == TaskKind::Execute)
        .unwrap();
    assert_eq!(work.1, UI_SKILLS);
}

#[test]
fn ui_ux_skills_review_run_receives_only_subjects_quality_gate() {
    let runs = skill_names_for("ui-ux");
    let review = runs
        .iter()
        .find(|(kind, _)| *kind == TaskKind::Review)
        .unwrap();
    assert_eq!(review.1, ["ui-ux-quality-gate"]);
}

#[test]
fn ui_ux_skills_do_not_leak_to_software_engineering_runs() {
    let runs = skill_names_for("software-engineering");
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|(_, skills)| skills.is_empty()), "{runs:?}");
}

#[test]
fn ui_ux_skills_frontmatter_defaults_and_review_only() {
    use task_ops::knowledge::{SkillUse, skill_applies_to};
    let skill = |use_field: &str| format!("---\nname: example\n{use_field}---\nbody\n");
    for field in ["", "celeris-use: unexpected\n"] {
        assert!(skill_applies_to(&skill(field), SkillUse::Work));
        assert!(!skill_applies_to(&skill(field), SkillUse::Review));
    }
    assert!(!skill_applies_to(
        &skill("celeris-use: review\n"),
        SkillUse::Work
    ));
    assert!(skill_applies_to(
        &skill("celeris-use: review\n"),
        SkillUse::Review
    ));
}
