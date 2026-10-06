use super::*;

fn kb_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("knowledge");
    init(&root).expect("init");
    (dir, root)
}

/// repo の `config/skills/`（ADR-0122 D1 の写し）。
fn config_skills_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/skills")
}

/// `SOURCE.md` の `key: value` 行（取得記録。frontmatter ではない）。
fn source_field(dir: &Path, key: &str) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join("SOURCE.md")).ok()?;
    raw.lines().find_map(|l| {
        l.strip_prefix(&format!("{key}:"))
            .map(|v| v.trim().to_string())
    })
}

/// (名前, description の書き出し, KB に入るべきライセンスの写し)
const UI_UX_SKILLS: [(&str, &str, &str); 4] = [
    (
        "frontend-design",
        "Guidance for distinctive, intentional visual design",
        "LICENSE.txt",
    ),
    (
        "shadcn",
        "Manages shadcn components and projects",
        "LICENSE.upstream",
    ),
    (
        "ui-ux-quality-gate",
        "Use as the UI/UX quality gate for frontend work",
        "LICENSE.upstream",
    ),
    (
        "web-design",
        "Web design reference for building production-grade interfaces.",
        "LICENSE.upstream",
    ),
];

/// ADR-0122 D1/D2: `config/skills/` の 4 件（`license: none` のものは除く）を一時 KB に取り込むと、
/// `skills_get` / `skill_description` が期待どおりで、出典とライセンスの写しも付属ファイルとして入り、
/// バイナリ（shadcn のアイコン）と除外済みの `scripts/` は入らない。再取り込みはコミットを増やさない。
#[test]
fn ui_ux_skills_config_skills_import_into_a_temp_kb() {
    let (_dir, root) = kb_dir();
    let config = config_skills_dir();
    let names: Vec<String> = UI_UX_SKILLS
        .iter()
        .map(|(n, ..)| n.to_string())
        .filter(|n| source_field(&config.join(n), "license").as_deref() != Some("none"))
        .collect();
    assert_eq!(names.len(), 4, "{names:?}");

    let reports = skills_import_dir(&root, &config, &names, Some("celerisctl")).expect("import");
    assert_eq!(
        reports.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        names.iter().map(String::as_str).collect::<Vec<_>>()
    );

    for (name, description_head, license_file) in UI_UX_SKILLS {
        let repo_dir = config.join(name);
        let repo_md = std::fs::read_to_string(repo_dir.join("SKILL.md")).expect("repo SKILL.md");
        let detail = skills_get(&root, name).unwrap_or_else(|| panic!("{name} not in KB"));

        // description は frontmatter から取れ、引用符や `>` が残らない。
        let description = skill_description(&detail.skill_md);
        assert!(
            description.starts_with(description_head),
            "{name}: {description:?}"
        );
        assert_eq!(description, skill_description(&repo_md), "{name}");
        // 本文は原文のまま（`source:` の追記だけ）。
        assert!(
            detail.skill_md.contains("source: celerisctl"),
            "{name}: {}",
            detail.skill_md
        );
        let (_, repo_body) = repo_md.split_once("\n---").expect("repo body");
        assert!(detail.skill_md.ends_with(repo_body), "{name}");

        // 出典とライセンスの写しは KB に入る。
        assert!(detail.files.iter().any(|f| f == "SOURCE.md"), "{name}");
        assert!(detail.files.iter().any(|f| f == license_file), "{name}");
        // 取り込み経路は scripts/ を拾わない（vet-skills で除外済み。repo にも無い）。
        assert!(
            detail.files.iter().all(|f| !f.starts_with("scripts/")),
            "{name}: {:?}",
            detail.files
        );
        let report = reports.iter().find(|r| r.name == name).expect("report");
        assert_eq!(report.path, format!("skills/{name}/SKILL.md"));
        assert_eq!(report.files, detail.files.len(), "{name}");
    }

    let gate = config.join("ui-ux-quality-gate");
    assert!(!gate.join("scripts").exists());
    assert!(!root.join("skills/ui-ux-quality-gate/scripts").exists());

    // shadcn のアイコン 2 件はバイナリとして飛ばし、テキストの付属ファイルは入る。
    let shadcn = reports.iter().find(|r| r.name == "shadcn").expect("shadcn");
    let skipped: Vec<(&str, SkipReason)> = shadcn
        .skipped
        .iter()
        .map(|s| (s.path.as_str(), s.reason))
        .collect();
    assert_eq!(
        skipped,
        vec![
            ("assets/shadcn-small.png", SkipReason::Binary),
            ("assets/shadcn.png", SkipReason::Binary),
        ]
    );
    let shadcn_files = skills_get(&root, "shadcn").expect("shadcn").files;
    assert!(shadcn_files.iter().any(|f| f == "rules/forms.md"));
    assert!(shadcn_files.iter().all(|f| !f.ends_with(".png")));

    let listed = skills_list(&root);
    assert_eq!(listed.len(), 4);
    assert!(listed.iter().all(|s| !s.description.is_empty()));

    // 冪等: 同じ内容の再取り込みは KB の HEAD を進めない。
    let head_before = head(&root).expect("head");
    skills_import_dir(&root, &config, &names, Some("celerisctl")).expect("re-import");
    assert_eq!(head(&root).expect("head"), head_before);
}

/// ADR-0122 D2: 付属ファイルの扱い（バイナリ・シンボリックリンク・隠しファイルは飛ばす）と、
/// `<dir>` が skill 1 件そのものの場合・`names` の絞り込み・無い名前。
#[test]
fn ui_ux_skills_import_skips_binary_symlink_and_hidden_files() {
    let (dir, root) = kb_dir();
    let src = dir.path().join("src");
    let one = src.join("demo-skill");
    std::fs::create_dir_all(one.join("rules")).expect("mkdir");
    std::fs::write(
        one.join("SKILL.md"),
        "---\nname: demo-skill\ndescription: >-\n  Demo skill\n  over two lines.\nlicense: MIT\n---\n\n# demo\n",
    )
    .expect("write");
    std::fs::write(one.join("rules/a.md"), "rule a\n").expect("write");
    std::fs::write(one.join("icon.png"), [0x89u8, 0x50, 0xff, 0xfe]).expect("write");
    std::fs::write(one.join(".DS_Store"), "x").expect("write");
    std::os::unix::fs::symlink("/etc/passwd", one.join("link.md")).expect("symlink");
    std::fs::create_dir_all(src.join("other")).expect("mkdir");
    std::fs::write(
        src.join("other/SKILL.md"),
        "---\nname: other\ndescription: other\n---\n",
    )
    .expect("write");
    std::fs::create_dir_all(src.join("not-a-skill")).expect("mkdir");

    // `<dir>` が SKILL.md を持てばその 1 件だけ。
    let reports = skills_import_dir(&root, &one, &[], None).expect("import one");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].files, 1);
    let skipped: Vec<(&str, SkipReason)> = reports[0]
        .skipped
        .iter()
        .map(|s| (s.path.as_str(), s.reason))
        .collect();
    assert_eq!(
        skipped,
        vec![
            (".DS_Store", SkipReason::Hidden),
            ("icon.png", SkipReason::Binary),
            ("link.md", SkipReason::Symlink),
        ]
    );
    let detail = skills_get(&root, "demo-skill").expect("get");
    assert_eq!(detail.files, vec!["rules/a.md".to_string()]);
    assert_eq!(
        skill_description(&detail.skill_md),
        "Demo skill over two lines."
    );

    // 親ディレクトリなら SKILL.md を持つ直下だけ。`names` で絞れる。
    assert_eq!(
        skill_dirs_in(&src, &[]).expect("dirs"),
        vec![one.clone(), src.join("other")]
    );
    let only = skills_import_dir(&root, &src, &["other".to_string()], None).expect("import other");
    assert_eq!(only.len(), 1);
    assert_eq!(only[0].name, "other");
    assert!(matches!(
        skills_import_dir(&root, &src, &["missing".to_string()], None),
        Err(SkillError::Failed(_))
    ));

    // ディレクトリ名と frontmatter の name が違えば skills_put の NameMismatch で止まる。
    let renamed = src.join("renamed");
    std::fs::create_dir_all(&renamed).expect("mkdir");
    std::fs::write(
        renamed.join("SKILL.md"),
        "---\nname: ui-ux\ndescription: d\n---\n",
    )
    .expect("write");
    assert!(matches!(
        skills_import_dir(&root, &renamed, &[], None),
        Err(SkillError::NameMismatch { .. })
    ));
}
