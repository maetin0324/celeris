use super::*;

fn write_skill(
    dir: &std::path::Path,
    name: &str,
    description: &str,
    body_extra: &str,
) -> SkillMount {
    let skill_dir = dir.join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\n本文{body_extra}\n"
        ),
    )
    .unwrap();
    SkillMount {
        name: name.to_string(),
        path: skill_dir.display().to_string(),
        description: description.to_string(),
    }
}

#[test]
fn skills_block_is_none_for_empty_skills() {
    assert_eq!(skills_block(&[]), None);
}

#[test]
fn skills_block_lists_name_description_and_path_without_body() {
    let dir = tempfile::tempdir().unwrap();
    let mount = write_skill(dir.path(), "rust-review", "Rust のレビュー観点", "");
    let block = skills_block(std::slice::from_ref(&mount)).unwrap();
    assert!(block.starts_with("## Skills（celeris）\n"));
    assert!(block.contains("- `rust-review`"));
    assert!(block.contains("Rust のレビュー観点"));
    assert!(block.contains(".agents/skills/rust-review/SKILL.md"));
    assert!(block.contains("付属ファイル"));
    assert!(!block.contains("# rust-review"));
    assert!(!block.contains("本文"));
}

#[test]
fn skills_block_survives_a_missing_skill_md_without_panicking() {
    let mount = SkillMount {
        name: "ghost".to_string(),
        path: "/does/not/exist".to_string(),
        description: String::new(),
    };
    let block = skills_block(&[mount]).unwrap();
    assert!(block.contains("- `ghost`"));
}

#[test]
fn rewrite_agents_md_appends_the_section_to_an_empty_file() {
    let out = rewrite_agents_md("", Some("## Skills（celeris）\n\n### a\nbody\n"));
    assert!(out.starts_with(SECTION_BEGIN));
    assert!(out.trim_end().ends_with(SECTION_END));
    assert!(out.contains("### a"));
}

/// 既存の `AGENTS.md` の内容は壊さず、節はその後ろに足される。
#[test]
fn rewrite_agents_md_preserves_existing_content() {
    let existing = "# Project notes\n\nDo not break the build.\n";
    let out = rewrite_agents_md(existing, Some("## Skills（celeris）\n\n### a\nbody\n"));
    assert!(out.starts_with(existing.trim_end()));
    assert!(out.contains(SECTION_BEGIN));
    assert!(out.contains("### a"));
}

/// 2 回かけても中身が増殖しない（冪等）。2 回目は 1 回目の出力を `existing` として渡す。
#[test]
fn rewrite_agents_md_is_idempotent() {
    let existing = "# Project notes\n";
    let once = rewrite_agents_md(existing, Some("## Skills（celeris）\n\n### a\nbody\n"));
    let twice = rewrite_agents_md(&once, Some("## Skills（celeris）\n\n### a\nbody\n"));
    assert_eq!(once, twice);
    assert_eq!(twice.matches(SECTION_BEGIN).count(), 1);
    assert_eq!(twice.matches("### a").count(), 1);
}

/// 節の中身が変われば（skill が増減すれば）次の run で置き換わる。
#[test]
fn rewrite_agents_md_replaces_the_section_when_the_block_changes() {
    let existing = "# Project notes\n";
    let once = rewrite_agents_md(existing, Some("## Skills（celeris）\n\n### a\nbody\n"));
    let twice = rewrite_agents_md(&once, Some("## Skills（celeris）\n\n### b\nother\n"));
    assert!(!twice.contains("### a"));
    assert!(twice.contains("### b"));
    assert_eq!(twice.matches(SECTION_BEGIN).count(), 1);
}

/// `block = None`（skills が無くなった）なら節を取り除くだけ。
#[test]
fn rewrite_agents_md_removes_the_section_when_the_block_is_none() {
    let existing = "# Project notes\n";
    let once = rewrite_agents_md(existing, Some("## Skills（celeris）\n\n### a\nbody\n"));
    let removed = rewrite_agents_md(&once, None);
    assert!(!removed.contains(SECTION_BEGIN));
    assert!(removed.contains("# Project notes"));
}

#[test]
fn preamble_section_is_empty_for_no_skills() {
    assert_eq!(preamble_section(&[]), "");
}

#[test]
fn preamble_section_carries_the_skills_block() {
    let dir = tempfile::tempdir().unwrap();
    let mount = write_skill(dir.path(), "writing", "文章の書き方", "");
    let section = preamble_section(std::slice::from_ref(&mount));
    assert!(section.contains("## Skills（celeris）"));
    assert!(section.contains("- `writing`"));
}

#[tokio::test]
async fn deliver_claude_code_writes_skill_md_and_sibling_files() {
    let kb = tempfile::tempdir().unwrap();
    let skill_dir = kb.path().join("rust-review");
    std::fs::create_dir_all(skill_dir.join("references")).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: rust-review\ndescription: d\n---\n\nbody\n",
    )
    .unwrap();
    std::fs::write(skill_dir.join("references/checklist.md"), "1. …\n").unwrap();
    let mount = SkillMount {
        name: "rust-review".to_string(),
        path: skill_dir.display().to_string(),
        description: "d".to_string(),
    };

    let cwd = tempfile::tempdir().unwrap();
    deliver_claude_code(cwd.path(), &[mount]).await.unwrap();

    let dest = cwd.path().join(".claude/skills/rust-review");
    assert!(dest.join("SKILL.md").exists());
    assert_eq!(
        std::fs::read_to_string(dest.join("references/checklist.md")).unwrap(),
        "1. …\n"
    );
}

/// 既存の `.claude/` の他の内容（別の skill・別の設定ファイル）には触れない。対象の skill の
/// ディレクトリだけを置き換える。
#[tokio::test]
async fn deliver_claude_code_only_touches_its_own_skill_directories() {
    let kb = tempfile::tempdir().unwrap();
    let skill_dir = kb.path().join("writing");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: writing\ndescription: d\n---\n\nnew body\n",
    )
    .unwrap();
    let mount = SkillMount {
        name: "writing".to_string(),
        path: skill_dir.display().to_string(),
        description: "d".to_string(),
    };

    let cwd = tempfile::tempdir().unwrap();
    // 既存の `.claude/skills/other-skill/` と `.claude/settings.json` は人・別の仕組みが置いたもの。
    std::fs::create_dir_all(cwd.path().join(".claude/skills/other-skill")).unwrap();
    std::fs::write(
        cwd.path().join(".claude/skills/other-skill/SKILL.md"),
        "untouched\n",
    )
    .unwrap();
    std::fs::write(cwd.path().join(".claude/settings.json"), "{}").unwrap();
    // 古い `writing` の中身（今回の mount で置き換わるはず）。
    std::fs::create_dir_all(cwd.path().join(".claude/skills/writing")).unwrap();
    std::fs::write(
        cwd.path().join(".claude/skills/writing/SKILL.md"),
        "stale body\n",
    )
    .unwrap();
    std::fs::write(
        cwd.path().join(".claude/skills/writing/stale.txt"),
        "gone\n",
    )
    .unwrap();

    deliver_claude_code(cwd.path(), &[mount]).await.unwrap();

    assert_eq!(
        std::fs::read_to_string(cwd.path().join(".claude/skills/writing/SKILL.md")).unwrap(),
        "---\nname: writing\ndescription: d\n---\n\nnew body\n"
    );
    assert!(!cwd.path().join(".claude/skills/writing/stale.txt").exists());
    assert_eq!(
        std::fs::read_to_string(cwd.path().join(".claude/skills/other-skill/SKILL.md")).unwrap(),
        "untouched\n"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.path().join(".claude/settings.json")).unwrap(),
        "{}"
    );
}

#[tokio::test]
async fn deliver_claude_code_does_nothing_for_empty_skills() {
    let cwd = tempfile::tempdir().unwrap();
    deliver_claude_code(cwd.path(), &[]).await.unwrap();
    assert!(!cwd.path().join(".claude").exists());
}

#[tokio::test]
async fn deliver_agents_md_does_nothing_for_empty_skills() {
    let cwd = tempfile::tempdir().unwrap();
    deliver_agents_md(cwd.path(), &[]).await.unwrap();
    assert!(!cwd.path().join("AGENTS.md").exists());
}

#[tokio::test]
async fn deliver_agents_md_creates_the_file_and_preserves_reruns() {
    let dir = tempfile::tempdir().unwrap();
    let mount = write_skill(dir.path(), "rust-review", "d", "");
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(
        cwd.path().join("AGENTS.md"),
        "# Notes\n\nBuild with cargo.\n",
    )
    .unwrap();

    deliver_agents_md(cwd.path(), std::slice::from_ref(&mount))
        .await
        .unwrap();
    let first = std::fs::read_to_string(cwd.path().join("AGENTS.md")).unwrap();
    assert!(first.contains("# Notes"));
    assert!(first.contains("Build with cargo."));
    assert!(first.contains(SECTION_BEGIN));
    assert!(first.contains("- `rust-review`"));

    // 同じ skills でもう一度届けても増殖しない。
    deliver_agents_md(cwd.path(), std::slice::from_ref(&mount))
        .await
        .unwrap();
    let second = std::fs::read_to_string(cwd.path().join("AGENTS.md")).unwrap();
    assert_eq!(first, second);
}

// ---- ADR-0056 Phase 81 追記: unmount 後の stale な skill ディレクトリの掃除 ----

/// mount A+B → マーカーに両方が載る。次に mount A だけにすると、B のディレクトリは消え、
/// マーカーに無い（celeris が書いていない）C は生き残る。
#[tokio::test]
async fn deliver_claude_code_removes_unmounted_directories_but_keeps_user_authored_ones() {
    let kb = tempfile::tempdir().unwrap();
    let mount_a = write_skill(kb.path(), "a", "skill a", "");
    let mount_b = write_skill(kb.path(), "b", "skill b", "");
    let cwd = tempfile::tempdir().unwrap();

    // ユーザーが自分で置いた C（celeris は一度も書いていない）。
    std::fs::create_dir_all(cwd.path().join(".claude/skills/c")).unwrap();
    std::fs::write(
        cwd.path().join(".claude/skills/c/SKILL.md"),
        "user authored\n",
    )
    .unwrap();

    // 1 回目: A + B を mount。
    deliver_claude_code(cwd.path(), &[mount_a.clone(), mount_b.clone()])
        .await
        .unwrap();
    assert!(cwd.path().join(".claude/skills/a/SKILL.md").exists());
    assert!(cwd.path().join(".claude/skills/b/SKILL.md").exists());
    assert!(cwd.path().join(".claude/skills/c/SKILL.md").exists());
    let marker_path = cwd.path().join(".celeris/skills.json");
    let marker: SkillsMarker =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    assert_eq!(
        marker.roots[CLAUDE_ROOT],
        vec!["a".to_string(), "b".to_string()]
    );

    // 2 回目: A だけを mount。B は消え、C（マーカーに無い）は残る。
    deliver_claude_code(cwd.path(), &[mount_a]).await.unwrap();
    assert!(cwd.path().join(".claude/skills/a/SKILL.md").exists());
    assert!(
        !cwd.path().join(".claude/skills/b").exists(),
        "b was unmounted; its directory must be removed"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.path().join(".claude/skills/c/SKILL.md")).unwrap(),
        "user authored\n",
        "c is not in the marker (celeris never wrote it); it must survive"
    );
    let marker: SkillsMarker =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    assert_eq!(marker.roots[CLAUDE_ROOT], vec!["a".to_string()]);
}

/// mount A+B の後に skills が空になったら、A と B の両方が消え、マーカーも空になる
/// （マーカーに無い C は触れない）。
#[tokio::test]
async fn deliver_claude_code_removes_all_previously_mounted_when_skills_becomes_empty() {
    let kb = tempfile::tempdir().unwrap();
    let mount_a = write_skill(kb.path(), "a", "skill a", "");
    let mount_b = write_skill(kb.path(), "b", "skill b", "");
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join(".claude/skills/c")).unwrap();
    std::fs::write(cwd.path().join(".claude/skills/c/SKILL.md"), "user\n").unwrap();

    deliver_claude_code(cwd.path(), &[mount_a, mount_b])
        .await
        .unwrap();
    deliver_claude_code(cwd.path(), &[]).await.unwrap();

    assert!(!cwd.path().join(".claude/skills/a").exists());
    assert!(!cwd.path().join(".claude/skills/b").exists());
    assert!(cwd.path().join(".claude/skills/c").exists());
    let marker_path = cwd.path().join(".celeris/skills.json");
    let marker: SkillsMarker =
        serde_json::from_str(&std::fs::read_to_string(&marker_path).unwrap()).unwrap();
    assert!(marker.roots[CLAUDE_ROOT].is_empty());
}

/// `codex`: mount A+B → AGENTS.md の節に両方。次に mount A だけにすると、節から B が消え、
/// A だけ残る（`rewrite_agents_md` が節を丸ごと書き直すので、`claude-code` のような別マーカーは
/// 要らない）。
#[tokio::test]
async fn deliver_agents_md_shrinks_the_section_when_a_skill_is_unmounted() {
    let dir = tempfile::tempdir().unwrap();
    let mount_a = write_skill(dir.path(), "a", "skill a", "");
    let mount_b = write_skill(dir.path(), "b", "skill b", "");
    let cwd = tempfile::tempdir().unwrap();

    deliver_agents_md(cwd.path(), &[mount_a.clone(), mount_b])
        .await
        .unwrap();
    let with_both = std::fs::read_to_string(cwd.path().join("AGENTS.md")).unwrap();
    assert!(with_both.contains("- `a`"));
    assert!(with_both.contains("- `b`"));

    deliver_agents_md(cwd.path(), &[mount_a]).await.unwrap();
    let with_one = std::fs::read_to_string(cwd.path().join("AGENTS.md")).unwrap();
    assert!(with_one.contains("- `a`"));
    assert!(
        !with_one.contains("- `b`"),
        "b was unmounted; the section must shrink to just a"
    );
}

#[tokio::test]
async fn skills_agent_delivery_copies_nested_files_and_unmounts_only_owned_directories() {
    let kb = tempfile::tempdir().unwrap();
    let a = write_skill(kb.path(), "a", "A", " BODY_MARKER");
    let b = write_skill(kb.path(), "b", "B", " BODY_MARKER");
    std::fs::create_dir_all(kb.path().join("a/rules/nested")).unwrap();
    std::fs::write(kb.path().join("a/rules/nested/check.md"), "auxiliary").unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join(".agents/skills/human")).unwrap();
    std::fs::write(cwd.path().join(".agents/skills/human/SKILL.md"), "human").unwrap();
    std::fs::write(cwd.path().join(".agents/config.json"), "{}").unwrap();
    deliver_agent_skills(cwd.path(), &[a.clone(), b])
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(cwd.path().join(".agents/skills/a/rules/nested/check.md")).unwrap(),
        "auxiliary"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.path().join(".agents/skills/a/.gitignore")).unwrap(),
        COPY_IGNORE
    );
    let block = skills_block(std::slice::from_ref(&a)).unwrap();
    assert!(!block.contains("BODY_MARKER"));
    assert!(block.contains(".agents/skills/a/SKILL.md"));
    deliver_agent_skills(cwd.path(), &[a]).await.unwrap();
    assert!(!cwd.path().join(".agents/skills/b").exists());
    deliver_agent_skills(cwd.path(), &[]).await.unwrap();
    assert!(!cwd.path().join(".agents/skills/a").exists());
    assert_eq!(
        std::fs::read_to_string(cwd.path().join(".agents/skills/human/SKILL.md")).unwrap(),
        "human"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.path().join(".agents/config.json")).unwrap(),
        "{}"
    );
    let marker = read_skills_marker(cwd.path()).await;
    assert!(marker.roots[AGENTS_ROOT].is_empty());
}

#[tokio::test]
async fn skills_agent_delivery_cleans_imprinted_copy_without_marker_and_preserves_same_name_human_copy()
 {
    let kb = tempfile::tempdir().unwrap();
    let mount = write_skill(kb.path(), "orphan", "d", "");
    let cwd = tempfile::tempdir().unwrap();
    let orphan = cwd.path().join(".agents/skills/orphan");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join(".gitignore"), COPY_IGNORE).unwrap();
    std::fs::write(orphan.join("SKILL.md"), "old").unwrap();
    deliver_agent_skills(cwd.path(), &[]).await.unwrap();
    assert!(!orphan.exists());
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("SKILL.md"), "human").unwrap();
    let error = deliver_agent_skills(cwd.path(), &[mount])
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        std::fs::read_to_string(orphan.join("SKILL.md")).unwrap(),
        "human"
    );
}

#[tokio::test]
async fn skills_switches_destinations_and_reads_legacy_marker() {
    let kb = tempfile::tempdir().unwrap();
    let a = write_skill(kb.path(), "a", "A", "");
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join(".celeris")).unwrap();
    std::fs::create_dir_all(cwd.path().join(".claude/skills/a")).unwrap();
    std::fs::write(cwd.path().join(SKILLS_MARKER_REL), "[\"a\"]").unwrap();
    deliver_agent_skills(cwd.path(), std::slice::from_ref(&a))
        .await
        .unwrap();
    assert!(!cwd.path().join(".claude/skills/a").exists());
    assert!(cwd.path().join(".agents/skills/a/SKILL.md").exists());
    deliver_claude_code(cwd.path(), &[a]).await.unwrap();
    assert!(!cwd.path().join(".agents/skills/a").exists());
    assert!(cwd.path().join(".claude/skills/a/SKILL.md").exists());
}

#[tokio::test]
async fn skills_marker_ignore_is_idempotent_and_preserves_existing_lines() {
    let cwd = tempfile::tempdir().unwrap();
    let dir = cwd.path().join(".celeris");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".gitignore"), "# human line\n!keep.txt\n").unwrap();
    write_skills_marker(cwd.path(), AGENTS_ROOT, &[])
        .await
        .unwrap();
    let first = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    write_skills_marker(cwd.path(), AGENTS_ROOT, &[])
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
        first
    );
    assert_eq!(first, "# human line\n!keep.txt\n*\n");
}

#[test]
fn skills_listing_uses_frontmatter_fallback_and_limits_description() {
    let kb = tempfile::tempdir().unwrap();
    let mut mount = write_skill(kb.path(), "small", "fallback", " BODY_MARKER");
    mount.description.clear();
    let block = skills_block(&[mount.clone()]).unwrap();
    assert!(block.contains("fallback"));
    assert!(!block.contains("BODY_MARKER"));
    mount.description = "x".repeat(1200);
    let block = skills_block(&[mount]).unwrap();
    assert_eq!(block.matches('x').count(), 1024);
}

#[tokio::test]
async fn skills_copy_is_excluded_from_git_status_without_touching_repo_exclude() {
    use std::process::Command;
    let kb = tempfile::tempdir().unwrap();
    let mount = write_skill(kb.path(), "example", "Example", "");
    let cwd = tempfile::tempdir().unwrap();
    let init = Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(cwd.path())
        .status()
        .unwrap();
    assert!(init.success());
    let repo_exclude = cwd.path().join(".git/info/exclude");
    let before = std::fs::read(&repo_exclude).unwrap();
    deliver_agent_skills(cwd.path(), &[mount]).await.unwrap();
    let status = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(String::from_utf8(status.stdout).unwrap(), "");
    assert_eq!(std::fs::read(repo_exclude).unwrap(), before);
}

#[tokio::test]
async fn skills_agents_md_removes_only_celeris_section_on_unmount() {
    let kb = tempfile::tempdir().unwrap();
    let mount = write_skill(kb.path(), "example", "Example", " BODY_MARKER");
    let cwd = tempfile::tempdir().unwrap();
    let path = cwd.path().join("AGENTS.md");
    std::fs::write(&path, "# Human instructions\n").unwrap();
    deliver_agents_md(cwd.path(), &[mount]).await.unwrap();
    let mounted = std::fs::read_to_string(&path).unwrap();
    assert!(mounted.contains(".agents/skills/example/SKILL.md"));
    assert!(!mounted.contains("BODY_MARKER"));
    deliver_agents_md(cwd.path(), &[]).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# Human instructions\n"
    );
}
