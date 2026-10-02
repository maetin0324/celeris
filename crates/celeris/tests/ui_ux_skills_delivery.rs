//! ADR-0122 D4 / この task（深さ2: ui-ux 課の worker 作業場所に 4 skill の本文が届く結合試験）。
//!
//! `config/org.example.toml` の `ui-ux` に書いた `skills_mounts` を実際に解決し、`config/skills/` を
//! 一時 KB に取り込んだ上で、`task-worker` の届け先（`claude-code` の `.claude/skills/<name>/SKILL.md`、
//! `codex` の `AGENTS.md` 節）に本文が実際に書かれることを確かめる。LLM は呼ばない（本物の `SKILL.md` を
//! 読んで写すだけ・決定的）。外部ネットワークにも出ない。

use std::path::{Path, PathBuf};

use task_ops::knowledge::{SkillUse, skill_applies_to, skill_description, skills_get};
use task_worker::protocol::SkillMount;

fn config_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config")
        .canonicalize()
        .expect("config dir")
}

/// `celeris.example.toml` + `org.example.toml` を組み合わせて読み、`cfg.org_nodes(...)` を返す
/// （既存の `the_two_example_files_load_together_through_org_include` と同じ読み方）。
fn load_example_org_nodes() -> Vec<task_core::OrgNode> {
    let dir = tempfile::tempdir().expect("tempdir");
    let example =
        std::fs::read_to_string(config_dir().join("celeris.example.toml")).expect("read example");
    let enabled = example.replace("# org_include = \"org.toml\"", "org_include = \"org.toml\"");
    std::fs::write(dir.path().join("config.toml"), enabled).expect("write config");
    std::fs::copy(
        config_dir().join("org.example.toml"),
        dir.path().join("org.toml"),
    )
    .expect("copy org.toml");
    let cfg = celeris::Config::load(&dir.path().join("config.toml")).expect("load config");
    cfg.validate().expect("validate config");
    cfg.org_nodes(time::OffsetDateTime::now_utc())
}

const UI_SKILLS: [&str; 4] = [
    "frontend-design",
    "shadcn",
    "web-design",
    "ui-ux-quality-gate",
];

/// `org.example.toml` の `ui-ux` が持つべき `skills_mounts`（決定的な種。routing 用の `profile.skills` は
/// 変わらない — 既存の `example_org_routes_ui_work_to_ui_ux_and_api_work_to_software_engineering` が
/// それを確かめる）。
#[test]
fn ui_ux_skills_mounts_resolve_from_org_example_toml() {
    let nodes = load_example_org_nodes();
    let ui_ux = task_core::resolve_profile(&nodes, "ui-ux");
    assert_eq!(ui_ux.skills_mounts, UI_SKILLS);
    assert_eq!(
        ui_ux.skills,
        [
            // "software" は親（engineering）から継いだもの。routing の既存試験はここを変えない。
            "software",
            "ui-design",
            "ux",
            "accessibility",
            "frontend",
            "react",
            "typescript",
            "css",
            "responsive",
            "usability",
        ]
    );

    // 他の課には skills_mounts が漏れない。
    let swe = task_core::resolve_profile(&nodes, "software-engineering");
    assert!(swe.skills_mounts.is_empty(), "{:?}", swe.skills_mounts);
}

/// `ui-ux` が実行時に本当に使う skill 名（resolve 済みの `skills_mounts`）を KB から `SkillMount` に解決する
/// （`task-dispatch` の `Dispatcher::skills_context` と同じ決定的なロジック: `skills_get` + `skill_applies_to`
/// + `skill_description`。ここでは `pub(super)` の内部関数を呼べないので、同じ公開 API で組み立て直す）。
fn resolve_skill_mounts(root: &Path, names: &[&str], run: SkillUse) -> Vec<SkillMount> {
    names
        .iter()
        .filter_map(|name| {
            let detail = skills_get(root, name)?;
            if !skill_applies_to(&detail.skill_md, run) {
                return None;
            }
            Some(SkillMount {
                name: (*name).to_string(),
                path: root.join("skills").join(name).display().to_string(),
                description: skill_description(&detail.skill_md),
            })
        })
        .collect()
}

#[tokio::test]
async fn ui_ux_skills_reach_claude_code_workspace_with_real_bodies() {
    let kb = tempfile::tempdir().expect("kb tempdir");
    let root = kb.path().join("knowledge");
    task_ops::knowledge::init(&root).expect("kb init");
    let names: Vec<String> = UI_SKILLS.iter().map(|s| s.to_string()).collect();
    task_ops::knowledge::skills_import_dir(&root, &config_dir().join("skills"), &names, None)
        .expect("import config/skills into temp kb");

    let work_mounts = resolve_skill_mounts(&root, &UI_SKILLS, SkillUse::Work);
    assert_eq!(
        work_mounts
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        UI_SKILLS,
        "work run should receive all 4 mounted skills"
    );

    let workspace = tempfile::tempdir().expect("workspace tempdir");
    task_worker::skills::deliver_claude_code(workspace.path(), &work_mounts)
        .await
        .expect("deliver claude-code skills");

    for name in UI_SKILLS {
        let delivered = workspace
            .path()
            .join(".claude/skills")
            .join(name)
            .join("SKILL.md");
        let delivered_body = std::fs::read_to_string(&delivered)
            .unwrap_or_else(|e| panic!("{} not delivered: {e}", delivered.display()));
        let source_body =
            std::fs::read_to_string(config_dir().join("skills").join(name).join("SKILL.md"))
                .expect("source SKILL.md");
        assert_eq!(
            delivered_body, source_body,
            "{name}: delivered SKILL.md body must match the vendored source verbatim"
        );
    }

    // vet-skills で scripts/ を除いた ui-ux-quality-gate は、取り込み元にも作業場所にも scripts/ が無い。
    assert!(
        !config_dir()
            .join("skills/ui-ux-quality-gate/scripts")
            .exists(),
        "vendored source must not carry scripts/"
    );
    assert!(
        !workspace
            .path()
            .join(".claude/skills/ui-ux-quality-gate/scripts")
            .exists(),
        "scripts/ must not reach the worker workspace"
    );

    // Codex: AGENTS.md は一覧のみで、skill 本体は作業場所内の相対パスに置く。
    let agents_cwd = tempfile::tempdir().expect("agents workspace");
    task_worker::skills::deliver_agent_skills(agents_cwd.path(), &work_mounts)
        .await
        .expect("deliver codex skill directories");
    task_worker::skills::deliver_agents_md(agents_cwd.path(), &work_mounts)
        .await
        .expect("deliver codex AGENTS.md");
    let agents_md =
        std::fs::read_to_string(agents_cwd.path().join("AGENTS.md")).expect("AGENTS.md written");
    assert!(agents_md.contains(task_worker::skills::SECTION_BEGIN));
    assert!(agents_md.contains(task_worker::skills::SECTION_END));
    for mount in &work_mounts {
        assert!(
            agents_md.contains(&format!("`{}`", mount.name)),
            "missing {}",
            mount.name
        );
        assert!(agents_md.contains(&format!(".agents/skills/{}/SKILL.md", mount.name)));
    }
    let web_body =
        std::fs::read_to_string(config_dir().join("skills/web-design/SKILL.md")).unwrap();
    assert!(
        !agents_md.contains(&web_body),
        "AGENTS.md embeds web-design body"
    );
    assert!(agents_md.len() < web_body.len());
    assert_delivered_tree(agents_cwd.path(), &work_mounts);
    assert!(
        !agents_md.contains("scripts/init_frontend_quality.py"),
        "AGENTS.md must not reference the excluded scripts/ entry point"
    );
}

#[tokio::test]
async fn ui_ux_skills_acp_delivery_copies_attachments_and_unmounts_owned_files() {
    let kb = tempfile::tempdir().expect("kb tempdir");
    let root = kb.path().join("knowledge");
    task_ops::knowledge::init(&root).expect("kb init");
    let names: Vec<String> = UI_SKILLS.iter().map(|s| s.to_string()).collect();
    task_ops::knowledge::skills_import_dir(&root, &config_dir().join("skills"), &names, None)
        .expect("import config/skills into temp kb");
    let mounts = resolve_skill_mounts(&root, &UI_SKILLS, SkillUse::Work);
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let human_file = workspace.path().join(".agents/skills/manual-note.md");
    std::fs::create_dir_all(human_file.parent().unwrap()).unwrap();
    std::fs::write(&human_file, "keep me").unwrap();

    task_worker::skills::deliver_agent_skills(workspace.path(), &mounts)
        .await
        .unwrap();
    let preamble = task_worker::skills::preamble_section(&mounts);
    let web_body =
        std::fs::read_to_string(config_dir().join("skills/web-design/SKILL.md")).unwrap();
    assert!(
        !preamble.contains(&web_body),
        "ACP preamble embeds web-design body"
    );
    assert!(preamble.contains(".agents/skills/web-design/SKILL.md"));
    assert!(preamble.len() < web_body.len());
    assert_delivered_tree(workspace.path(), &mounts);

    task_worker::skills::deliver_agent_skills(workspace.path(), &[])
        .await
        .unwrap();
    for name in UI_SKILLS {
        assert!(
            !workspace.path().join(".agents/skills").join(name).exists(),
            "{name} remains mounted"
        );
    }
    assert_eq!(std::fs::read_to_string(&human_file).unwrap(), "keep me");
}

fn assert_delivered_tree(workspace: &Path, mounts: &[SkillMount]) {
    for mount in mounts {
        let source = Path::new(&mount.path);
        let dest = workspace.join(".agents/skills").join(&mount.name);
        let mut files = Vec::new();
        collect_files(source, source, &mut files);
        for relative in files {
            assert!(
                dest.join(&relative).is_file(),
                "{} missing {}",
                mount.name,
                relative.display()
            );
            assert_eq!(
                std::fs::read(source.join(&relative)).unwrap(),
                std::fs::read(dest.join(&relative)).unwrap()
            );
        }
    }
    for relative in [
        "shadcn/cli.md",
        "shadcn/rules/forms.md",
        "ui-ux-quality-gate/references/workflow.md",
        "ui-ux-quality-gate/templates/DESIGN.md",
    ] {
        assert!(
            workspace.join(".agents/skills").join(relative).is_file(),
            "missing attachment {relative}"
        );
    }
}

fn collect_files(root: &Path, current: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(current).unwrap() {
        let entry = entry.unwrap();
        let ty = entry.file_type().unwrap();
        if ty.is_dir() {
            collect_files(root, &entry.path(), files);
        } else if ty.is_file() {
            files.push(entry.path().strip_prefix(root).unwrap().to_path_buf());
        }
    }
}

#[tokio::test]
async fn ui_ux_skills_review_run_only_delivers_the_quality_gate() {
    let kb = tempfile::tempdir().expect("kb tempdir");
    let root = kb.path().join("knowledge");
    task_ops::knowledge::init(&root).expect("kb init");
    let names: Vec<String> = UI_SKILLS.iter().map(|s| s.to_string()).collect();
    task_ops::knowledge::skills_import_dir(&root, &config_dir().join("skills"), &names, None)
        .expect("import config/skills into temp kb");

    let review_mounts = resolve_skill_mounts(&root, &UI_SKILLS, SkillUse::Review);
    assert_eq!(
        review_mounts
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["ui-ux-quality-gate"],
        "review run must only receive the quality gate, not the design/component skills"
    );

    let workspace = tempfile::tempdir().expect("workspace tempdir");
    task_worker::skills::deliver_claude_code(workspace.path(), &review_mounts)
        .await
        .expect("deliver claude-code skills for review");

    let delivered = workspace.path().join(".claude/skills");
    let entries: Vec<String> = std::fs::read_dir(&delivered)
        .expect("skills dir")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, ["ui-ux-quality-gate"], "{entries:?}");
    assert!(
        !workspace
            .path()
            .join(".claude/skills/ui-ux-quality-gate/scripts")
            .exists()
    );

    for name in ["frontend-design", "shadcn", "web-design"] {
        assert!(
            !workspace.path().join(".claude/skills").join(name).exists(),
            "{name} must not reach a review run's workspace"
        );
    }
}
