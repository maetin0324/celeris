//! ADR-0122 D1: `celerisctl skills import <dir>` が repo の `config/skills/` を KB に取り込む
//! （DB を開かない。mount はしない）。

use std::path::Path;
use std::process::Command;

fn celerisctl(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_celerisctl"))
        .args(args)
        .env_remove("CELERIS_KNOWLEDGE_ROOT")
        .env_remove("CELERIS_CONFIG")
        .output()
        .expect("run celerisctl")
}

#[test]
fn ui_ux_skills_cli_imports_config_skills_without_a_db() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("knowledge");
    let root_s = root.to_str().expect("utf-8");
    let config = tmp.path().join("missing-config.toml");
    let config_s = config.to_str().expect("utf-8");
    let db = tmp.path().join("never-created.sqlite3");
    let skills = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/skills");
    let skills_s = skills.to_str().expect("utf-8");

    let init = celerisctl(&["knowledge", "init", "--root", root_s]);
    assert!(init.status.success(), "{init:?}");

    let out = celerisctl(&[
        "--db",
        db.to_str().expect("utf-8"),
        "skills",
        "import",
        skills_s,
        "--root",
        root_s,
        "--config",
        config_s,
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{out:?}");
    for name in [
        "frontend-design",
        "shadcn",
        "ui-ux-quality-gate",
        "web-design",
    ] {
        assert!(
            stdout.contains(&format!("imported skills/{name}/SKILL.md")),
            "{stdout}"
        );
        let md = std::fs::read_to_string(root.join(format!("skills/{name}/SKILL.md")))
            .expect("SKILL.md in KB");
        assert!(md.contains("source: celerisctl"), "{md}");
        assert!(root.join(format!("skills/{name}/SOURCE.md")).is_file());
    }
    assert!(
        stdout.contains("skipped (binary): shadcn/assets/shadcn.png"),
        "{stdout}"
    );
    assert!(!root.join("skills/shadcn/assets/shadcn.png").exists());
    assert!(!root.join("skills/ui-ux-quality-gate/scripts").exists());
    assert!(!db.exists(), "skills import must not open the DB");

    // `--name` で絞る・無い名前は失敗。
    let one = celerisctl(&[
        "skills", "import", skills_s, "--name", "shadcn", "--root", root_s, "--config", config_s,
    ]);
    let one_out = String::from_utf8_lossy(&one.stdout);
    assert!(one.status.success(), "{one:?}");
    assert_eq!(one_out.matches("imported ").count(), 1, "{one_out}");
    let missing = celerisctl(&[
        "skills", "import", skills_s, "--name", "nope", "--root", root_s, "--config", config_s,
    ]);
    assert!(!missing.status.success());
}
